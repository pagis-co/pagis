use super::*;

#[async_trait::async_trait]
impl KnowledgeStore for FakeKnowledgeStore {
    async fn pending_invalidations(
        &self,
        workspace: &WorkspaceId,
        limit: u32,
    ) -> Result<Vec<KnowledgeInvalidation>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .invalidations
            .rows(workspace)
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn acknowledge_invalidation(
        &self,
        workspace: &WorkspaceId,
        id: i64,
    ) -> Result<(), StoreError> {
        self.0.lock().unwrap().invalidations.remove(workspace, &id);
        Ok(())
    }
    async fn configure(&self, config: SyncConfig, now: i64) -> Result<SyncStatus, StoreError> {
        let mut data = self.0.lock().unwrap();
        let key = resource_key(&config);
        let held = data
            .knowledge
            .0
            .entry((config.workspace_id.clone(), key))
            .or_insert_with(|| KnowledgeResource::new(config.clone(), now));
        if held.status.config != config {
            if held.status.config.filter != config.filter {
                held.status.filter_revision += 1;
            }
            held.status.revision += 1;
            held.status.cursor_revision += 1;
            held.status.config = config;
            held.status.updated_at = now;
        }
        Ok(resource_status(held, now))
    }
    async fn status(&self, key: &SourceKey, now: i64) -> Result<Option<SyncStatus>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .map(|r| resource_status(r, now)))
    }
    async fn reset(
        &self,
        key: &SourceKey,
        expected_revision: i64,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        if row.status.cursor_revision != expected_revision {
            return Err(conflict("Sync cursor changed"));
        }
        row.status.checkpoint = Value::Null;
        row.status.caught_up = false;
        row.status.revision += 1;
        row.status.cursor_revision += 1;
        row.status.updated_at = now;
        Ok(())
    }
    async fn acquire(
        &self,
        key: &SourceKey,
        batch: &crate::knowledge::SourceBatch,
        now: i64,
        keys: &dyn crate::ForgetKeys,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .is_none_or(|r| r.status.cursor_revision != batch.expected_revision)
        {
            return Err(conflict("Sync cursor changed"));
        }
        let mut changes = Vec::new();
        for change in &batch.changes {
            if !super::forget::suppressed(&data, key, &change.id, keys)? {
                changes.push(change.clone());
            }
        }
        let row = resource_mut(&mut data, key)?;
        for change in changes {
            if row.versions.values().any(|held| {
                held.version.source.id == change.id
                    && held.version.source.version == change.version
                    && held.version.source.operation == change.operation
            }) {
                continue;
            }
            let sequence = row.next_sequence;
            row.next_sequence += 1;
            let arrival = batch
                .arrivals
                .iter()
                .find(|a| a.provider_event_id == change.id)
                .cloned();
            row.versions.insert(
                sequence,
                AcquiredVersion {
                    version: SourceVersion {
                        sequence,
                        source: change,
                        observed_at: now,
                        historical: batch.historical,
                        arrival,
                    },
                    attempts: 0,
                    next_attempt_at: now,
                    acknowledged: false,
                    skipped: false,
                },
            );
        }
        row.status.checkpoint = batch.checkpoint.clone();
        row.status.caught_up = batch.caught_up;
        row.status.cursor_revision += 1;
        row.status.revision += 1;
        row.status.updated_at = now;
        data.invalidation_sequence += 1;
        let id = data.invalidation_sequence;
        data.invalidations.put(
            &key.workspace_id,
            id,
            KnowledgeInvalidation {
                id,
                source: key.clone(),
            },
        );
        Ok(())
    }
    async fn arrivals(
        &self,
        key: &SourceKey,
        limit: u32,
        now: i64,
    ) -> Result<Vec<SourceVersion>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.versions.values())
            .filter(|r| {
                r.version.arrival.is_some()
                    && !r.acknowledged
                    && !r.skipped
                    && r.next_attempt_at <= now
            })
            .take(limit as usize)
            .map(|r| r.version.clone())
            .collect())
    }
    async fn versions(
        &self,
        key: &SourceKey,
        after_sequence: i64,
        limit: u32,
    ) -> Result<Vec<SourceVersion>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.versions.values())
            .filter(|r| r.version.sequence > after_sequence)
            .take(limit as usize)
            .map(|r| r.version.clone())
            .collect())
    }
    async fn acknowledge_arrival(
        &self,
        key: &SourceKey,
        sequence: i64,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        if let Some(version) = row.versions.get_mut(&sequence) {
            version.acknowledged = true;
        }
        row.status.updated_at = now;
        Ok(())
    }
    async fn fail_arrival(
        &self,
        key: &SourceKey,
        sequence: i64,
        permanent: bool,
        reason: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        if let Some(version) = row.versions.get_mut(&sequence) {
            version.attempts += 1;
            version.skipped = permanent || version.attempts >= 5;
            version.next_attempt_at = now + 60_000 * (1_i64 << version.attempts.min(10));
        }
        row.status.arrival_error = Some(reason.to_string());
        row.status.updated_at = now;
        Ok(())
    }
    async fn collection_error(
        &self,
        key: &SourceKey,
        arrival: bool,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        if arrival {
            row.status.arrival_error = error.map(str::to_string);
        } else {
            row.status.acquisition_error = error.map(str::to_string);
        }
        Ok(())
    }
    async fn record_reflection(
        &self,
        key: &SourceKey,
        reflection: &PageReflection,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        let k = (reflection.page_path.clone(), reflection.filter_revision);
        let (attempts, wakeup) = row
            .reflections
            .get(&k)
            .map(|r| (r.attempts, r.wakeup.clone()))
            .unwrap_or_default();
        row.reflections.insert(
            k,
            ReflectionRecord {
                reflection: reflection.clone(),
                attempts,
                wakeup,
            },
        );
        Ok(())
    }
    async fn backfill_candidates(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        limit: u32,
    ) -> Result<Vec<BackfillPage>, StoreError> {
        let data = self.0.lock().unwrap();
        let Some(row) = data.knowledge.get(&key.workspace_id, &source_key(key)) else {
            return Ok(vec![]);
        };
        let mut pages: BTreeMap<String, i64> = BTreeMap::new();
        for version in row.versions.values().filter(|r| r.version.historical) {
            if let Some(parent) = &version.version.source.parent
                && !row.reflections.values().any(|r| {
                    r.reflection.page_subject == *parent
                        && r.reflection.filter_revision == filter_revision
                })
            {
                let at = version
                    .version
                    .source
                    .source_at
                    .unwrap_or(version.version.observed_at);
                pages
                    .entry(parent.clone())
                    .and_modify(|held| *held = (*held).max(at))
                    .or_insert(at);
            }
        }
        let mut pages: Vec<_> = pages
            .into_iter()
            .map(|(subject, newest_at)| BackfillPage { subject, newest_at })
            .collect();
        pages.sort_by_key(|p| std::cmp::Reverse((p.newest_at, p.subject.clone())));
        pages.truncate(limit as usize);
        Ok(pages)
    }
    async fn release_failed_backfill_runs(&self, key: &SourceKey) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let terminal: BTreeSet<_> = data
            .wakeups
            .rows(&key.workspace_id)
            .filter(|w| {
                w.run_id.as_ref().is_some_and(|id| {
                    data.runs.get(&key.workspace_id, id).is_some_and(|run| {
                        matches!(run.state, RunState::Failed | RunState::Canceled)
                    })
                })
            })
            .map(|w| w.id.clone())
            .collect();
        let Some(row) = data.knowledge.get_mut(&key.workspace_id, &source_key(key)) else {
            return Ok(0);
        };
        let mut count = 0;
        for r in row
            .reflections
            .values_mut()
            .filter(|r| r.wakeup.as_ref().is_some_and(|id| terminal.contains(id)))
        {
            r.wakeup = None;
            r.attempts += 1;
            count += 1;
        }
        Ok(count)
    }
    async fn backfill_batch(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        limit: u32,
    ) -> Result<Vec<String>, StoreError> {
        let data = self.0.lock().unwrap();
        let mut rows: Vec<_> = data
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.reflections.values())
            .filter(|r| {
                r.reflection.filter_revision == filter_revision
                    && r.reflection.decided_by == ReflectionPass::Backfill
                    && r.reflection.selection.verdict == Verdict::Reflect
                    && r.attempts < BACKFILL_ATTEMPTS_PER_PAGE as u32
                    && r.wakeup.is_none()
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.reflection.decided_at));
        Ok(rows
            .into_iter()
            .take(limit as usize)
            .map(|r| r.reflection.page_path.clone())
            .collect())
    }
    async fn record_backfill_run(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        page_paths: &[String],
        wakeup_id: &crate::WakeupId,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        for path in page_paths {
            if let Some(r) = row.reflections.get_mut(&(path.clone(), filter_revision))
                && r.wakeup.is_none()
            {
                r.wakeup = Some(wakeup_id.clone());
            }
        }
        row.backfill_runs.push(now);
        row.status.updated_at = now;
        Ok(())
    }
    async fn versions_without_metadata(
        &self,
        key: &SourceKey,
        limit: u32,
    ) -> Result<Vec<SourceVersion>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.versions.values())
            .filter(|r| {
                r.version.source.operation == SourceOperation::Upsert
                    && (r.version.source.metadata.is_null()
                        || r.version.source.metadata == serde_json::json!({}))
            })
            .take(limit as usize)
            .map(|r| r.version.clone())
            .collect())
    }
    async fn complete_metadata(
        &self,
        key: &SourceKey,
        sequence: i64,
        metadata: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = resource_mut(&mut data, key)?;
        if let Some(version) = row.versions.get_mut(&sequence) {
            version.version.source.metadata = metadata.clone();
            let parent = version.version.source.parent.clone();
            row.reflections.retain(|_, r| {
                Some(&r.reflection.page_subject) != parent.as_ref() || r.wakeup.is_some()
            });
        }
        Ok(())
    }
    async fn parent_metadata(
        &self,
        key: &SourceKey,
        parent: &str,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.versions.values())
            .filter(|r| r.version.source.parent.as_deref() == Some(parent))
            .map(|r| r.version.source.metadata.clone())
            .collect())
    }
    async fn parents_metadata(
        &self,
        key: &SourceKey,
    ) -> Result<Vec<(String, Vec<serde_json::Value>)>, StoreError> {
        let data = self.0.lock().unwrap();
        let mut parents: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for version in data
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.versions.values())
        {
            if let Some(parent) = &version.version.source.parent {
                parents
                    .entry(parent.clone())
                    .or_default()
                    .push(version.version.source.metadata.clone());
            }
        }
        Ok(parents.into_iter().collect())
    }
    async fn parents_with_metadata(
        &self,
        key: &SourceKey,
        field: &str,
        value: &str,
        since: i64,
    ) -> Result<i64, StoreError> {
        let data = self.0.lock().unwrap();
        let parents: BTreeSet<_> = data
            .knowledge
            .get(&key.workspace_id, &source_key(key))
            .into_iter()
            .flat_map(|r| r.versions.values())
            .filter(|r| {
                r.version.source.source_at.unwrap_or(0) >= since
                    && r.version.source.metadata.get(field).is_some_and(|v| {
                        v.as_str() == Some(value)
                            || v.as_array()
                                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(value)))
                    })
            })
            .filter_map(|r| r.version.source.parent.clone())
            .collect();
        Ok(parents.len() as i64)
    }
}

#[async_trait::async_trait]
impl CatalogueProvider for FakeCatalogueProvider {
    async fn catalogue(&self, key: &SourceKey) -> Result<Catalogue, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .catalogues
            .get(&key.workspace_id, &source_key(key))
            .map(|r| r.0.clone())
            .unwrap_or(Catalogue { signals: vec![] }))
    }
    fn default_filter(&self, key: &SourceKey) -> ReflectionFilter {
        self.0
            .lock()
            .unwrap()
            .catalogues
            .get(&key.workspace_id, &source_key(key))
            .map(|r| r.1.clone())
            .unwrap_or(ReflectionFilter {
                rules: vec![],
                default: Verdict::Skip,
            })
    }
    async fn preview(
        &self,
        key: &SourceKey,
        filter: &ReflectionFilter,
        now: i64,
    ) -> Result<FilterPreview, StoreError> {
        let data = self.0.lock().unwrap();
        let rows = data.catalogues.get(&key.workspace_id, &source_key(key));
        Ok(filter.preview(rows.into_iter().flat_map(|r| r.2.iter()), now))
    }
}

impl FakeCatalogueProvider {
    /// Set the signals and filter of one source resource.
    pub fn set(
        &self,
        key: &SourceKey,
        catalogue: Catalogue,
        filter: ReflectionFilter,
        pages: Vec<PageSignals>,
    ) {
        self.0.lock().unwrap().catalogues.put(
            &key.workspace_id,
            source_key(key),
            (catalogue, filter, pages),
        );
    }
}
fn source_key(key: &SourceKey) -> (ConnectionId, String) {
    (key.connection_id.clone(), key.resource.clone())
}
fn resource_key(config: &SyncConfig) -> (ConnectionId, String) {
    (config.connection_id.clone(), config.resource.clone())
}
fn resource_mut<'a>(
    data: &'a mut Data,
    key: &SourceKey,
) -> Result<&'a mut KnowledgeResource, StoreError> {
    data.knowledge
        .get_mut(&key.workspace_id, &source_key(key))
        .ok_or_else(|| conflict("Sync resource does not exist"))
}
pub(super) struct AcquiredVersion {
    pub version: SourceVersion,
    attempts: u32,
    next_attempt_at: i64,
    acknowledged: bool,
    skipped: bool,
}
struct ReflectionRecord {
    reflection: PageReflection,
    attempts: u32,
    wakeup: Option<WakeupId>,
}
pub(super) struct KnowledgeResource {
    pub status: SyncStatus,
    pub versions: BTreeMap<i64, AcquiredVersion>,
    next_sequence: i64,
    reflections: BTreeMap<(String, i64), ReflectionRecord>,
    backfill_runs: Vec<i64>,
}
impl KnowledgeResource {
    fn new(config: SyncConfig, now: i64) -> Self {
        Self {
            status: SyncStatus {
                config,
                revision: 1,
                cursor_revision: 1,
                checkpoint: Value::Null,
                caught_up: false,
                rebuilding: false,
                processed: 0,
                arrival_pending: 0,
                arrival_processed: 0,
                arrival_skipped: 0,
                filter_revision: 1,
                reflected_pages: 0,
                skipped_pages: 0,
                backfill_pending: 0,
                backfill_reflected: 0,
                backfill_failed: 0,
                backfill_capped: false,
                incomplete_versions: 0,
                acquisition_error: None,
                arrival_error: None,
                updated_at: now,
            },
            versions: BTreeMap::new(),
            next_sequence: 1,
            reflections: BTreeMap::new(),
            backfill_runs: vec![],
        }
    }
}
fn resource_status(row: &KnowledgeResource, now: i64) -> SyncStatus {
    let mut status = row.status.clone();
    status.processed = row.versions.len() as i64;
    status.arrival_pending = row
        .versions
        .values()
        .filter(|r| r.version.arrival.is_some() && !r.acknowledged && !r.skipped)
        .count() as i64;
    status.arrival_processed = row.versions.values().filter(|r| r.acknowledged).count() as i64;
    status.arrival_skipped = row.versions.values().filter(|r| r.skipped).count() as i64;
    status.incomplete_versions = row
        .versions
        .values()
        .filter(|r| {
            r.version.source.operation == SourceOperation::Upsert
                && (r.version.source.metadata.is_null()
                    || r.version.source.metadata == serde_json::json!({}))
        })
        .count() as i64;
    for r in row
        .reflections
        .values()
        .filter(|r| r.reflection.filter_revision == status.filter_revision)
    {
        if r.reflection.selection.verdict == Verdict::Reflect {
            status.reflected_pages += 1;
        } else {
            status.skipped_pages += 1;
        }
        if r.reflection.decided_by == ReflectionPass::Backfill
            && r.reflection.selection.verdict == Verdict::Reflect
        {
            if r.wakeup.is_some() {
                status.backfill_reflected += 1;
            } else if r.attempts >= BACKFILL_ATTEMPTS_PER_PAGE as u32 {
                status.backfill_failed += 1;
            } else {
                status.backfill_pending += 1;
            }
        }
    }
    status.backfill_capped = row
        .backfill_runs
        .iter()
        .filter(|at| **at > now - BACKFILL_WINDOW_MS)
        .count() as i64
        >= BACKFILL_RUNS_PER_WINDOW;
    status
}
