use super::*;

#[async_trait::async_trait]
impl ForgetStore for FakeForgetStore {
    async fn refuse_delete(&self, workspace: &WorkspaceId) -> Result<(), StoreError> {
        super::audit::refuse_delete(&self.0.lock().unwrap(), workspace)
    }

    async fn preview(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
    ) -> Result<ForgetPreview, StoreError> {
        preview(&self.0.lock().unwrap(), workspace, target)
    }
    async fn preview_affects(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        let data = self.0.lock().unwrap();
        validate_target(workspace, target)?;
        Ok(match target {
            ForgetTarget::Account { connection_id } => exposures.iter().any(|e| {
                data.grants.get(workspace, &e.grant_id).is_none_or(|g| {
                    g.resource_kind == "connection"
                        && g.resource_id.as_deref() == Some(connection_id.as_str())
                })
            }),
            ForgetTarget::Source { .. } => false,
        })
    }
    async fn begin(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        expected_revision: &str,
        now: i64,
        keys: &dyn ForgetKeys,
    ) -> Result<ForgetOperation, StoreError> {
        let mut data = self.0.lock().unwrap();
        super::audit::refuse_delete(&data, workspace)?;
        let current = preview(&data, workspace, target)?;
        if current.revision != expected_revision {
            return Err(conflict("Forget preview changed"));
        }
        let connection = target_connection(target);
        let sources = selected_sources(&data, workspace, target);
        let key = keys.suppression_key(workspace)?;
        let operation = ForgetOperation {
            id: EventId::generate().to_string(),
            connection_id: connection.clone(),
            phase: ForgetPhase::Blocked,
            created_at: now,
            error: None,
            reopted_at: None,
        };
        let account = matches!(target, ForgetTarget::Account { .. });
        for (source, id) in &sources {
            data.suppressions.put(
                workspace,
                key.identity(source, id),
                Suppression {
                    operation: operation.id.clone(),
                    connection: connection.clone(),
                },
            );
        }
        data.raw_blocks
            .put(workspace, operation.id.clone(), connection.clone());
        let grants: Vec<_> = data
            .grants
            .rows(workspace)
            .filter(|g| {
                account
                    && g.resource_kind == "connection"
                    && g.resource_id.as_deref() == Some(connection.as_str())
            })
            .map(|g| g.id.clone())
            .collect();
        for grant in grants {
            data.forgotten_grants
                .put(workspace, grant, operation.id.clone());
        }
        data.forget_operations.put(
            workspace,
            operation.id.clone(),
            ForgetRecord {
                operation: operation.clone(),
                target: Some(target.clone()),
                sources,
            },
        );
        Ok(operation)
    }
    async fn is_suppressed(
        &self,
        source: &SourceKey,
        source_id: &str,
        keys: &dyn ForgetKeys,
    ) -> Result<bool, StoreError> {
        suppressed(&self.0.lock().unwrap(), source, source_id, keys)
    }
    async fn permits_provenance(
        &self,
        workspace: &WorkspaceId,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(!exposures
            .iter()
            .any(|e| data.forgotten_grants.get(workspace, &e.grant_id).is_some()))
    }
    async fn run_allowed(&self, workspace: &WorkspaceId, run: &RunId) -> Result<bool, StoreError> {
        Ok(!self
            .0
            .lock()
            .unwrap()
            .forgotten_runs
            .contains(&(workspace.clone(), run.clone())))
    }
    async fn raw_connection_allowed(
        &self,
        workspace: &WorkspaceId,
        connection: &ConnectionId,
    ) -> Result<bool, StoreError> {
        Ok(!self
            .0
            .lock()
            .unwrap()
            .raw_blocks
            .rows(workspace)
            .any(|id| id == connection))
    }
    async fn operation(
        &self,
        workspace: &WorkspaceId,
        id: &str,
    ) -> Result<Option<ForgetOperation>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .forget_operations
            .get(workspace, &id.to_string())
            .map(|r| r.operation.clone()))
    }
    async fn operations(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<ForgetOperation>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .forget_operations
            .rows(workspace)
            .map(|r| r.operation.clone())
            .collect())
    }
    async fn unfinished(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Vec<ForgetOperation>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .forget_operations
            .rows(workspace)
            .filter(|r| r.operation.phase != ForgetPhase::Complete)
            .map(|r| r.operation.clone())
            .collect())
    }
    async fn purge_structured(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        super::audit::refuse_delete(&data, workspace)?;
        let Some(record) = data
            .forget_operations
            .get(workspace, &id.to_string())
            .cloned()
        else {
            return Ok(());
        };
        if record.operation.phase != ForgetPhase::Blocked {
            return Ok(());
        }
        let connection = &record.operation.connection_id;
        let account = matches!(record.target, Some(ForgetTarget::Account { .. }));
        let affected_runs: BTreeSet<_> = data
            .source_reads
            .0
            .iter()
            .filter(|((w, (_, c)), reads)| {
                w == workspace
                    && c == connection
                    && (account
                        || reads.iter().any(|read| {
                            record.sources.iter().any(|(source, id)| match read {
                                SourceRead::Item {
                                    resource,
                                    id: read_id,
                                } => source.resource == *resource && id == read_id,
                                SourceRead::Parent {
                                    resource,
                                    id: parent,
                                } => {
                                    source.resource == *resource
                                        && data
                                            .knowledge
                                            .get(
                                                workspace,
                                                &(
                                                    source.connection_id.clone(),
                                                    source.resource.clone(),
                                                ),
                                            )
                                            .is_some_and(|r| {
                                                r.versions.values().any(|v| {
                                                    &v.version.source.id == id
                                                        && v.version.source.parent.as_ref()
                                                            == Some(parent)
                                                })
                                            })
                                }
                            })
                        }))
            })
            .map(|((_, (run, _)), _)| run.clone())
            .collect();
        for resource in data
            .knowledge
            .rows_mut(workspace)
            .filter(|r| r.status.config.connection_id == *connection)
        {
            resource.versions.retain(|_, version| {
                !account
                    && !record.sources.iter().any(|(source, id)| {
                        source.resource == resource.status.config.resource
                            && id == &version.version.source.id
                    })
            });
        }
        for run in &affected_runs {
            data.forgotten_runs.insert((workspace.clone(), run.clone()));
            data.journal.remove(run);
        }
        let messages: Vec<_> = data
            .messages
            .rows(workspace)
            .filter(|m| {
                m.run_id.as_ref().is_some_and(|r| affected_runs.contains(r))
                    || data
                        .message_exposures
                        .get(workspace, &m.id)
                        .is_some_and(|exposures| {
                            exposures.iter().any(|e| {
                                data.forgotten_grants.get(workspace, &e.grant_id).is_some()
                            })
                        })
            })
            .map(|m| m.id.clone())
            .collect();
        for id in messages {
            data.forgotten_messages
                .insert((workspace.clone(), id.clone()));
            if let Some(m) = data.messages.get_mut(workspace, &id) {
                m.text_content.clear();
                m.blocks.clear();
            }
        }
        let forgotten = data.forgotten_messages.clone();
        data.tool_evidence.0.retain(|(w, _), (_, t)| {
            w != workspace || !forgotten.contains(&(w.clone(), t.source_message_id.clone()))
        });
        data.source_reads.0.retain(|(w, (run, c)), _| {
            w != workspace || c != connection || !affected_runs.contains(run)
        });
        data.incoming_events.0.retain(|(w, _), event| {
            w != workspace
                || event.source.connection_id() != Some(connection)
                || (!account
                    && !record
                        .sources
                        .iter()
                        .any(|(_, id)| id == &event.provider_event_id))
        });
        let row = data
            .forget_operations
            .get_mut(workspace, &id.to_string())
            .unwrap();
        row.operation.phase = ForgetPhase::StructuredPurged;
        row.target = None;
        row.sources.clear();
        Ok(())
    }
    async fn memory_purged(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        super::audit::refuse_delete(&data, workspace)?;
        if let Some(row) = data
            .forget_operations
            .get_mut(workspace, &id.to_string())
            .filter(|r| r.operation.phase == ForgetPhase::StructuredPurged)
        {
            row.operation.phase = ForgetPhase::MemoryPurged;
        }
        Ok(())
    }
    async fn complete(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(row) = data
            .forget_operations
            .get_mut(workspace, &id.to_string())
            .filter(|r| r.operation.phase == ForgetPhase::MemoryPurged)
        {
            row.operation.phase = ForgetPhase::Complete;
            row.operation.error = None;
        }
        Ok(())
    }
    async fn record_failure(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        error: &str,
    ) -> Result<(), StoreError> {
        if let Some(row) = self
            .0
            .lock()
            .unwrap()
            .forget_operations
            .get_mut(workspace, &id.to_string())
        {
            row.operation.error = Some(error.to_string());
        }
        Ok(())
    }
    async fn retry(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
        if let Some(row) = self
            .0
            .lock()
            .unwrap()
            .forget_operations
            .get_mut(workspace, &id.to_string())
        {
            row.operation.error = None;
        }
        Ok(())
    }
    async fn reopt_in(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .forget_operations
            .get_mut(workspace, &id.to_string())
            .filter(|r| r.operation.phase == ForgetPhase::Complete)
        else {
            return Err(conflict("Forget is not complete"));
        };
        row.operation.reopted_at = Some(now);
        data.suppressions
            .0
            .retain(|(w, _), r| w != workspace || r.operation != id);
        data.forgotten_grants
            .0
            .retain(|(w, _), operation| w != workspace || operation != id);
        data.raw_blocks.remove(workspace, &id.to_string());
        Ok(())
    }
}

impl ForgetKeys for FakeForgetKeys {
    fn suppression_key(&self, workspace_id: &WorkspaceId) -> Result<SuppressionKey, SealError> {
        use sha2::{Digest, Sha256};
        let mut held = self.0.lock().unwrap();
        let key = held
            .forget_keys
            .entry(workspace_id.clone())
            .or_insert_with(|| {
                let mut key = [0u8; 32];
                rand::fill(&mut key);
                key
            });
        let derived: [u8; 32] = Sha256::digest(key).into();
        Ok(SuppressionKey::derived(derived))
    }
}

#[derive(Clone)]
pub(super) struct ForgetRecord {
    pub operation: ForgetOperation,
    target: Option<ForgetTarget>,
    sources: Vec<(SourceKey, String)>,
}
pub(super) struct Suppression {
    operation: String,
    connection: ConnectionId,
}
fn validate_target(workspace: &WorkspaceId, target: &ForgetTarget) -> Result<(), StoreError> {
    if let ForgetTarget::Source { source, .. } = target
        && &source.workspace_id != workspace
    {
        return Err(conflict("Forget target is outside this Workspace"));
    }
    Ok(())
}
fn target_connection(target: &ForgetTarget) -> ConnectionId {
    match target {
        ForgetTarget::Source { source, .. } => source.connection_id.clone(),
        ForgetTarget::Account { connection_id } => connection_id.clone(),
    }
}
fn selected_sources(
    data: &Data,
    workspace: &WorkspaceId,
    target: &ForgetTarget,
) -> Vec<(SourceKey, String)> {
    let connection = target_connection(target);
    let mut sources = Vec::new();
    for row in data
        .knowledge
        .rows(workspace)
        .filter(|r| r.status.config.connection_id == connection)
    {
        for version in row.versions.values() {
            let source = row.status.key();
            let id = &version.version.source.id;
            if (matches!(target, ForgetTarget::Account { .. })
                || matches!(target,ForgetTarget::Source { source: selected, source_id } if selected == &source && source_id == id))
                && !sources.contains(&(source.clone(), id.clone()))
            {
                sources.push((source, id.clone()));
            }
        }
    }
    sources
}
fn preview(
    data: &Data,
    workspace: &WorkspaceId,
    target: &ForgetTarget,
) -> Result<ForgetPreview, StoreError> {
    use sha2::{Digest, Sha256};
    validate_target(workspace, target)?;
    let sources = selected_sources(data, workspace, target);
    let revisions: Vec<_> = data
        .knowledge
        .rows(workspace)
        .filter(|r| r.status.config.connection_id == target_connection(target))
        .map(|r| (r.status.key(), r.status.revision))
        .collect();
    let operations: Vec<_> = data
        .forget_operations
        .rows(workspace)
        .map(|r| &r.operation)
        .collect();
    let serialized = serde_json::to_vec(&(target, &sources, revisions, operations))
        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
    let revision = crate::seal::hex_encode(&Sha256::digest(serialized));
    Ok(ForgetPreview {
        revision,
        source_items: sources.len() as u64,
        raw_account_retrieval_blocked: true,
        ..Default::default()
    })
}
pub(super) fn suppressed(
    data: &Data,
    source: &SourceKey,
    id: &str,
    keys: &dyn ForgetKeys,
) -> Result<bool, StoreError> {
    if !data
        .suppressions
        .rows(&source.workspace_id)
        .any(|r| r.connection == source.connection_id)
    {
        return Ok(false);
    }
    let key = keys.suppression_key(&source.workspace_id)?;
    Ok(data
        .suppressions
        .get(&source.workspace_id, &key.identity(source, id))
        .is_some())
}
