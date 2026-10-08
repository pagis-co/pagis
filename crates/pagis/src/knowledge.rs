//! Source acquisition and Subject Page arrivals.

use crate::arrival_commit::{PassCommit, PendingAppend, RefusedAppend};
use async_trait::async_trait;
use pagis_core::knowledge::*;
use pagis_core::reflection_filter::{Catalogue, FilterPreview, PageSignals, Selection, Verdict};
use pagis_core::{
    AgentStatus, AgentStore, Clock, Connection, ConnectionStore, Grant, GrantStore, MemoryAccess,
    MemoryAuthor, MemoryExposure, MemoryStore, ScopedPath, WorkspaceId, WorkspaceStore,
};
use pagis_google::{GogRunner, GoogleProvider, ProviderError, ProviderErrorCode};
use pagis_knowledge::{Error, Knowledge, Source, SourcePage};
use pagis_mail::SenderTrust;
use pagis_trigger::Trigger;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How many historical pages one collect pass decides about. Selection
/// reads stored metadata alone, so it stays ahead of the Runs it feeds
/// (ADR-0011).
const BACKFILL_DECISIONS_PER_PASS: u32 = 100;

pub struct Worker {
    pub bus: Arc<dyn pagis_core::EventBus>,
    /// Every Workspace on this installation. A pass reads the tenant off
    /// the rows it processes, so one pass serves every person.
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub store: Arc<dyn KnowledgeStore>,
    /// The source of the suppression key that blocks a forgotten item.
    pub forget_keys: Arc<dyn pagis_core::ForgetKeys>,
    pub connections: Arc<dyn ConnectionStore>,
    pub connection_state: Arc<crate::connections::ConnectionState>,
    pub grants: Arc<dyn GrantStore>,
    pub agents: Arc<dyn AgentStore>,
    pub google: Arc<crate::connections::GoogleBinder>,
    pub trigger: Arc<Trigger>,
    /// The source record pages that acquisition updates without a model.
    pub memory: Arc<dyn MemoryStore>,
    /// The logical time for acquisition and Subject Page entries.
    pub clock: Arc<dyn Clock>,
    /// The tier of one sender, for the `sender_trust` signal (ADR-0011).
    pub trust: Arc<dyn SenderTrust>,
}

impl Worker {
    pub async fn publish_changes(&self) -> Result<(), pagis_core::StoreError> {
        for workspace in self.workspaces.list().await? {
            self.publish_workspace_changes(&workspace.id).await?;
        }
        Ok(())
    }

    async fn publish_workspace_changes(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<(), pagis_core::StoreError> {
        for change in self.store.pending_invalidations(workspace_id, 100).await? {
            self.bus
                .publish(pagis_core::NewEvent {
                    workspace_id: workspace_id.clone(),
                    event_type: "knowledge.changed".into(),
                    agent_id: None,
                    run_id: None,
                    channel_id: None,
                    payload: serde_json::json!({
                        "invalidation_id": change.id,
                        "connection_id": change.source.connection_id,
                        "resource": change.source.resource,
                    }),
                })
                .await?;
            // Publication is durable first. A crash before acknowledgment can
            // replay this harmless invalidation, but cannot lose it.
            self.store
                .acknowledge_invalidation(workspace_id, change.id)
                .await?;
        }
        Ok(())
    }
    /// The Connection of one authorized sync resource. A refusal names
    /// the check that failed in the log, because the sync status shows
    /// only that the source requires authorization.
    async fn authorized(&self, config: &SyncConfig) -> Result<Connection, Error> {
        let stored = self
            .store
            .status(
                &SourceKey {
                    workspace_id: config.workspace_id.clone(),
                    connection_id: config.connection_id.clone(),
                    resource: config.resource.clone(),
                },
                self.clock.now_ms(),
            )
            .await?
            .map(|state| state.config);
        let agent = self
            .agents
            .get(&config.workspace_id, &config.agent_id)
            .await?;
        let connection = self
            .connections
            .get(&config.workspace_id, &config.connection_id)
            .await?;
        let grant = self
            .grants
            .live_for_resource(
                &config.workspace_id,
                &config.agent_id,
                Grant::CONNECTION_KIND,
                config.connection_id.as_str(),
            )
            .await?;
        if let Some(reason) = refusal(
            config,
            stored.as_ref(),
            agent.as_ref(),
            connection.as_ref(),
            grant.as_ref(),
        ) {
            tracing::warn!(
                connection_id = %config.connection_id,
                resource = %config.resource,
                reason,
                "sync authorization refused"
            );
            return Err(Error::Unauthorized);
        }
        Ok(connection.expect("a refusal names a missing connection"))
    }

    /// Acquires source versions, appends arrivals, and ingests triggers.
    pub async fn collect_pass(
        self: &Arc<Self>,
    ) -> std::collections::HashSet<pagis_core::ConnectionId> {
        let mut shared = std::collections::HashSet::new();
        let Ok(workspaces) = self.workspaces.list().await else {
            return shared;
        };
        for workspace in workspaces {
            self.collect_workspace_pass(&workspace.id, &mut shared)
                .await;
        }
        shared
    }

    /// One Workspace's pass. The tenant comes off the Workspace row, so
    /// the loop serves every person in one pass.
    async fn collect_workspace_pass(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        shared: &mut std::collections::HashSet<pagis_core::ConnectionId>,
    ) {
        let Ok(connections) = self.connections.list(workspace_id).await else {
            return;
        };
        for connection in connections.into_iter().filter(|c| c.provider == "google") {
            let key = SourceKey {
                workspace_id: workspace_id.clone(),
                connection_id: connection.id.clone(),
                resource: pagis_broker::MAIL_SYNC_RESOURCE.into(),
            };
            let status = match self.store.status(&key, self.clock.now_ms()).await {
                Ok(Some(status)) => status,
                Ok(None) => continue,
                Err(error) => {
                    tracing::error!(%error, connection_id = %connection.id, "loading shared acquisition status failed");
                    continue;
                }
            };
            if !status.config.enabled {
                continue;
            }
            shared.insert(connection.id.clone());
            if let Err(error) = self.authorized(&status.config).await {
                let _ = self
                    .store
                    .collection_error(&key, false, Some(&error.to_string()))
                    .await;
                continue;
            }
            let source = Gmail {
                worker: Arc::clone(self),
                config: status.config.clone(),
            };
            let knowledge = Knowledge::new(Arc::clone(&self.store), Arc::clone(&self.forget_keys));
            let mut result = knowledge.collect(&key, &source, self.clock.now_ms()).await;
            // A version acquired without metadata gets it here, before the
            // backfill decides about its page (ADR-0011).
            if result.is_ok() {
                result = knowledge.complete(&key, &source, self.clock.now_ms()).await;
            }
            let error = result.err().map(|e| e.to_string());
            let _ = self
                .store
                .collection_error(&key, false, error.as_deref())
                .await;
            // An acquisition failure does not discard an already acquired arrival.
            let result = self.deliver_arrivals(&key, &status, &source, &source).await;
            let error = result.err().map(|e| e.to_string());
            let _ = self
                .store
                .collection_error(&key, true, error.as_deref())
                .await;
            // The Backfill Reflection follows live delivery, and it
            // reflects at most one batch, so live work always goes
            // first (ADR-0011).
            if let Err(error) = self.backfill(&key, &source).await {
                tracing::warn!(connection_id = %key.connection_id, %error, "backfill reflection failed");
            }
        }
    }

    /// The pinned Google client of one configured resource.
    async fn google_provider(
        &self,
        config: &SyncConfig,
    ) -> Result<GoogleProvider<Arc<dyn GogRunner>>, Error> {
        let connection = self.authorized(config).await?;
        self.google.provider(&connection).ok_or(Error::Unauthorized)
    }

    /// The account's own Gmail labels, else none. The catalogue still
    /// offers the system labels when the account cannot be read, so the
    /// editor works before the user configures the resource.
    async fn gmail_labels(&self, key: &SourceKey) -> Vec<String> {
        let Ok(Some(connection)) = self
            .connections
            .get(&key.workspace_id, &key.connection_id)
            .await
        else {
            return Vec::new();
        };
        if connection.status != Connection::CONNECTED
            || !connection
                .authorized_capabilities
                .iter()
                .any(|capability| capability == "gmail_read")
        {
            return Vec::new();
        }
        let Some(provider) = self.google.provider(&connection) else {
            return Vec::new();
        };
        provider.gmail_labels().await.unwrap_or_default()
    }

    async fn deliver_arrivals(
        &self,
        key: &SourceKey,
        status: &SyncStatus,
        source: &dyn Source,
        signals: &dyn SignalSource,
    ) -> Result<(), Error> {
        let config = &status.config;
        self.authorized(config).await?;
        let mut arrivals = self.store.arrivals(key, 100, self.clock.now_ms()).await?;
        if arrivals.is_empty() {
            return Ok(());
        }
        arrivals.sort_by_key(|item| {
            item.arrival
                .as_ref()
                .map(|event| event.occurred_at)
                .or(item.source.source_at)
                .unwrap_or(item.observed_at)
        });
        // The pass reads each arrival on its own, then commits every
        // append together (ADR-0007).
        let mut read: Vec<ReadArrival> = Vec::new();
        let mut appends: Vec<PendingAppend> = Vec::new();
        let mut appended = Vec::new();
        for arrival in &arrivals {
            // One arrival fails alone. Only lost authorization stops the pass,
            // because every later arrival then fails the same way.
            match self.read_arrival(key, source, arrival).await {
                // An arrival without a source occurrence has nothing
                // for a Subject Page.
                Ok(None) => appended.push(arrival.sequence),
                Ok(Some((path, front_matter, entry))) => {
                    appends.push(PendingAppend {
                        sequence: arrival.sequence,
                        path: path.clone(),
                        front_matter,
                        entry,
                    });
                    read.push(ReadArrival {
                        sequence: arrival.sequence,
                        path,
                        subject: subject_of(arrival).unwrap_or_default(),
                        event: if arrival.historical {
                            None
                        } else {
                            arrival.arrival.clone()
                        },
                    });
                }
                Err(Error::Unauthorized) => return Err(Error::Unauthorized),
                Err(error) => {
                    let permanent = error.permanent();
                    tracing::warn!(
                        connection_id = %key.connection_id,
                        sequence = arrival.sequence,
                        provider_event_id = arrival
                            .arrival
                            .as_ref()
                            .map(|event| event.provider_event_id.as_str())
                            .unwrap_or_default(),
                        permanent,
                        reason = %error,
                        "arrival delivery failed"
                    );
                    self.store
                        .fail_arrival(
                            key,
                            arrival.sequence,
                            permanent,
                            &error.to_string(),
                            self.clock.now_ms(),
                        )
                        .await?;
                }
            }
        }
        // A commit that fails after its retries acknowledges nothing
        // of the pass and starts nothing. The arrivals come again on
        // the next pass, with their attempts untouched (ADR-0007).
        let refused = self.commit_appends(key, config, &appends).await?;
        for refusal in &refused {
            tracing::warn!(
                connection_id = %key.connection_id,
                sequence = refusal.sequence,
                reason = %refusal.reason,
                "arrival delivery failed"
            );
            self.store
                .fail_arrival(
                    key,
                    refusal.sequence,
                    false,
                    &refusal.reason,
                    self.clock.now_ms(),
                )
                .await?;
        }
        let mut live_pages: Vec<LivePage> = Vec::new();
        for arrival in read {
            if refused
                .iter()
                .any(|refusal| refusal.sequence == arrival.sequence)
            {
                continue;
            }
            appended.push(arrival.sequence);
            if let Some(event) = arrival.event {
                match live_pages.iter_mut().find(|page| page.path == arrival.path) {
                    Some(page) => page.events.push(event),
                    None => live_pages.push(LivePage {
                        path: arrival.path,
                        subject: arrival.subject,
                        events: vec![event],
                    }),
                }
            }
        }
        // The Reflection Filter decides which pages reflect. A page the
        // filter skips today still reflects on a later arrival that
        // matches; the record keeps no memory beyond the decision
        // (ADR-0011).
        let mut live_events = Vec::new();
        let mut live_subject_paths = Vec::new();
        for page in &live_pages {
            if self
                .decide(
                    key,
                    status,
                    signals,
                    &page.path,
                    &page.subject,
                    ReflectionPass::Live,
                )
                .await
            {
                live_subject_paths.push(page.path.clone());
                live_events.extend(page.events.iter().cloned());
            }
        }
        if !live_events.is_empty() {
            self.trigger
                .ingest_arrivals(
                    pagis_core::IngestBatch {
                        workspace_id: key.workspace_id.clone(),
                        source: pagis_core::EventSource::connection(key.connection_id.clone()),
                        agent_id: None,
                        event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
                        cursor: None,
                        events: live_events,
                        received_at: self.clock.now_ms(),
                        baseline: false,
                    },
                    pagis_trigger::ArrivalRun {
                        agent_id: config.agent_id.clone(),
                        subject_paths: live_subject_paths.iter().map(ScopedPath::display).collect(),
                        historical: false,
                    },
                )
                .await
                .map_err(|error| {
                    tracing::warn!(
                        connection_id = %key.connection_id,
                        %error,
                        "arrival ingest failed"
                    );
                    Error::Unavailable
                })?;
        }
        // Trigger's durable provider-event key makes a crash before acknowledgment safe.
        for sequence in appended {
            self.store
                .acknowledge_arrival(key, sequence, self.clock.now_ms())
                .await?;
        }
        Ok(())
    }

    /// Selects and records the verdict for one page.
    async fn decide(
        &self,
        key: &SourceKey,
        status: &SyncStatus,
        source: &dyn SignalSource,
        path: &ScopedPath,
        subject: &str,
        decided_by: ReflectionPass,
    ) -> bool {
        let now = self.clock.now_ms();
        let selection = match source.signals(subject, now).await {
            Ok(signals) => status.config.filter.select(&signals, now),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "page signals are unavailable");
                // A signal the store cannot read holds the filter back,
                // never the reflection (ADR-0011). The live pass reflects
                // at once and writes no record. The backfill records the
                // same verdict, because a page it does not record is a
                // page it decides about again at every pass (ADR-0011).
                if decided_by == ReflectionPass::Live {
                    return true;
                }
                Selection {
                    verdict: Verdict::Reflect,
                    rule: None,
                    reason: "the page signals are unavailable".into(),
                }
            }
        };
        if !selection.verdict.reflects() {
            tracing::debug!(
                path = %path.display(),
                reason = %selection.reason,
                "the reflection filter skipped the page"
            );
        }
        let reflects = selection.verdict.reflects();
        if let Err(error) = self
            .store
            .record_reflection(
                key,
                &PageReflection {
                    page_path: path.display(),
                    page_subject: subject.to_string(),
                    filter_revision: status.filter_revision,
                    decided_by,
                    selection,
                    decided_at: now,
                },
            )
            .await
        {
            tracing::warn!(path = %path.display(), %error, "recording the filter verdict failed");
        }
        reflects
    }

    /// The Backfill Reflection of one resource: it decides about the
    /// historical Subject Pages the filter has not seen, newest first,
    /// and reflects one batch of the selected pages in one Run
    /// (ADR-0011).
    async fn backfill(&self, key: &SourceKey, signals: &dyn SignalSource) -> Result<(), Error> {
        let now = self.clock.now_ms();
        // A Run that failed reflected nothing: its pages wait again and
        // its Wake-up leaves the budget before the budget is read
        // (ADR-0011).
        let released = self.store.release_failed_backfill_runs(key).await?;
        if released > 0 {
            tracing::info!(
                connection_id = %key.connection_id,
                pages = released,
                "the pages of failed backfill Runs wait for a Run again"
            );
        }
        let Some(status) = self.store.status(key, now).await? else {
            return Ok(());
        };
        // History is still arriving, the budget of the window is spent,
        // or the resource is paused: the backfill waits.
        if !status.caught_up || !status.config.enabled || status.backfill_capped {
            return Ok(());
        }
        self.authorized(&status.config).await?;
        for page in self
            .store
            .backfill_candidates(key, status.filter_revision, BACKFILL_DECISIONS_PER_PASS)
            .await?
        {
            let path = subject_path(&key.resource, &page.subject)?;
            self.decide(
                key,
                &status,
                signals,
                &path,
                &page.subject,
                ReflectionPass::Backfill,
            )
            .await;
        }
        let paths = self
            .store
            .backfill_batch(key, status.filter_revision, BACKFILL_PAGES_PER_RUN)
            .await?;
        if paths.is_empty() {
            return Ok(());
        }
        // The batch has no live occurrence. The Run starts from the
        // Subject Pages alone.
        let outcome = self
            .trigger
            .ingest_arrivals(
                pagis_core::IngestBatch {
                    workspace_id: key.workspace_id.clone(),
                    source: pagis_core::EventSource::connection(key.connection_id.clone()),
                    agent_id: None,
                    event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
                    cursor: None,
                    events: Vec::new(),
                    received_at: now,
                    baseline: false,
                },
                pagis_trigger::ArrivalRun {
                    agent_id: status.config.agent_id.clone(),
                    subject_paths: paths.clone(),
                    historical: true,
                },
            )
            .await
            .map_err(|error| {
                tracing::warn!(connection_id = %key.connection_id, %error, "backfill ingest failed");
                Error::Unavailable
            })?;
        let Some(wakeup) = outcome
            .created
            .iter()
            .find(|wakeup| matches!(wakeup.rule, pagis_core::WakeupRule::Arrival { .. }))
        else {
            return Err(Error::Unavailable);
        };
        self.store
            .record_backfill_run(key, status.filter_revision, &paths, &wakeup.id, now)
            .await?;
        tracing::info!(
            connection_id = %key.connection_id,
            pages = paths.len(),
            "the backfill reflection started a Run"
        );
        Ok(())
    }

    /// Read one arrival from its source and make the Timeline entry
    /// of its Subject Page. The commit of the pass follows.
    async fn read_arrival(
        &self,
        key: &SourceKey,
        source: &dyn Source,
        arrival: &SourceVersion,
    ) -> Result<
        Option<(
            ScopedPath,
            pagis_core::subject_page::FrontMatter,
            pagis_core::subject_page::TimelineEntry,
        )>,
        Error,
    > {
        let Some(event) = &arrival.arrival else {
            return Ok(None);
        };
        let subject = event.metadata["thread_id"]
            .as_str()
            .unwrap_or(&event.provider_event_id);
        let path = subject_path(&key.resource, subject)?;
        let source_reference = format!(
            "{}:{}:{}:{}",
            key.resource, key.connection_id, event.provider_event_id, arrival.source.version
        );
        let content = source.read(&arrival.source).await?;
        let source_text = content
            .contents
            .into_iter()
            .find(|item| item.id == arrival.source.id && item.version == arrival.source.version)
            .ok_or(Error::NotFound)?;
        let words = serde_json::from_str::<serde_json::Value>(&source_text.text)
            .ok()
            .and_then(|value| value["body"].as_str().map(str::to_string))
            .unwrap_or(source_text.text)
            .trim()
            .to_string();
        Ok(Some((
            path,
            mail_front_matter(event),
            pagis_core::subject_page::TimelineEntry {
                source_reference,
                source_time: event.occurred_at,
                words,
            },
        )))
    }

    /// Commit every append of the pass once, and report the arrivals
    /// whose Subject Page refused the entry (ADR-0007).
    async fn commit_appends(
        &self,
        key: &SourceKey,
        config: &SyncConfig,
        appends: &[PendingAppend],
    ) -> Result<Vec<RefusedAppend>, Error> {
        if appends.is_empty() {
            return Ok(Vec::new());
        }
        let grants = self
            .grants
            .list_live_for_agent(&config.workspace_id, &config.agent_id)
            .await?;
        let access = MemoryAccess::agent(
            config.agent_id.clone(),
            grants
                .into_iter()
                .filter(|grant| {
                    grant.resource_kind == Grant::CONNECTION_KIND
                        && grant.resource_id.as_deref() == Some(key.connection_id.as_str())
                })
                .map(|grant| MemoryExposure {
                    grant_id: grant.id,
                    revision: grant.revision,
                }),
        );
        let author = MemoryAuthor {
            name: "Pagis".into(),
            email: "system@pagis.local".into(),
        };
        PassCommit {
            memory: self.memory.as_ref(),
            workspace_id: &key.workspace_id,
            agent_id: &config.agent_id,
            access: &access,
            author: &author,
        }
        .run(appends)
        .await
    }
}

/// The front matter of a mail page: the mail subject as the title, and
/// `Source`, one of the committed page kinds, as the kind. A mail thread nobody has read yet is a source,
/// and reflection replaces the kind when it reads the page.
fn mail_front_matter(event: &pagis_core::NormalizedEvent) -> pagis_core::subject_page::FrontMatter {
    let Some(title) = event.metadata["subject"].as_str() else {
        return pagis_core::subject_page::FrontMatter::default();
    };
    let title = title
        .split(['\r', '\n'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.trim();
    if title.is_empty() {
        return pagis_core::subject_page::FrontMatter::default();
    }
    pagis_core::subject_page::FrontMatter {
        title: Some(title.to_string()),
        kind: Some("Source".into()),
        aliases: Vec::new(),
        links: Vec::new(),
    }
}

/// One arrival of the pass that the source delivered, with what the
/// live gating needs after the commit.
struct ReadArrival {
    sequence: i64,
    path: ScopedPath,
    /// The source subject of the page, such as a mail thread id.
    subject: String,
    /// The live occurrence of the arrival. A historical arrival has
    /// none, and it starts no Run.
    event: Option<pagis_core::NormalizedEvent>,
}

/// One Subject Page that live arrivals of this pass changed.
struct LivePage {
    path: ScopedPath,
    /// The source subject of the page, such as a mail thread id.
    subject: String,
    events: Vec<pagis_core::NormalizedEvent>,
}

/// The source subject of one arrival, such as its mail thread.
fn subject_of(arrival: &SourceVersion) -> Option<String> {
    let event = arrival.arrival.as_ref()?;
    Some(
        event.metadata["thread_id"]
            .as_str()
            .unwrap_or(&event.provider_event_id)
            .to_string(),
    )
}

/// The Subject Page of one source subject, such as one mail thread.
fn subject_path(resource: &str, subject: &str) -> Result<ScopedPath, Error> {
    ScopedPath::parse(&format!(
        "private/subjects/{}/{}.md",
        page_segment(resource),
        page_segment(subject)
    ))
    .map_err(|_| Error::Unavailable)
}

fn page_segment(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("_{byte:02x}"));
        }
    }
    if output.is_empty() {
        "item".into()
    } else {
        output
    }
}

struct Gmail {
    worker: Arc<Worker>,
    config: SyncConfig,
}
impl Gmail {
    async fn error(&self, error: ProviderError) -> Error {
        if matches!(
            error.code,
            ProviderErrorCode::ReauthRequired | ProviderErrorCode::PermissionRevoked
        ) && let Err(store_error) = self
            .worker
            .connection_state
            .require_reauthorization(&self.config.workspace_id, &self.config.connection_id)
            .await
        {
            return Error::Store(store_error);
        }
        provider_error(&self.config.connection_id, error)
    }
    async fn provider(&self) -> Result<GoogleProvider<Arc<dyn GogRunner>>, Error> {
        self.worker.google_provider(&self.config).await
    }
}

fn provider_error(connection_id: &pagis_core::ConnectionId, error: ProviderError) -> Error {
    tracing::warn!(
        %connection_id,
        code = error.code.as_str(),
        detail = error.detail,
        retryable = error.retryable,
        "provider read failed"
    );
    match error.code {
        ProviderErrorCode::NotFound => Error::NotFound,
        ProviderErrorCode::ReauthRequired | ProviderErrorCode::PermissionRevoked => {
            Error::Unauthorized
        }
        // The request itself is wrong, so the same item fails the same way on
        // every retry.
        ProviderErrorCode::InvalidRequest => Error::Rejected(
            if error.detail.is_empty() {
                "the provider refused the request"
            } else {
                error.detail
            }
            .into(),
        ),
        _ => Error::Unavailable,
    }
}

#[async_trait]
impl CatalogueProvider for Worker {
    async fn catalogue(&self, key: &SourceKey) -> Result<Catalogue, pagis_core::StoreError> {
        Ok(pagis_google::gmail_filter::catalogue(
            &self.gmail_labels(key).await,
        ))
    }

    fn default_filter(&self, _key: &SourceKey) -> pagis_core::reflection_filter::ReflectionFilter {
        pagis_google::gmail_filter::default_filter()
    }

    async fn preview(
        &self,
        key: &SourceKey,
        filter: &pagis_core::reflection_filter::ReflectionFilter,
        now: i64,
    ) -> Result<FilterPreview, pagis_core::StoreError> {
        // The sender tier belongs to the responsible agent. A resource
        // that no agent syncs has no stored pages, so the counts are zero.
        let Some(status) = self.store.status(key, now).await? else {
            return Ok(filter.preview([], now));
        };
        let threads = self.store.parents_metadata(key).await?;
        let messages: Vec<Vec<serde_json::Value>> =
            threads.into_iter().map(|(_, messages)| messages).collect();
        let senders: Vec<Option<(String, String)>> = messages
            .iter()
            .map(|thread| crate::page_signals::thread_sender(thread))
            .collect();
        let mut tiers: BTreeMap<&str, pagis_core::TrustTier> = BTreeMap::new();
        for address in senders
            .iter()
            .flatten()
            .map(|(address, _)| address.as_str())
        {
            if !tiers.contains_key(address) {
                let tier = self
                    .trust
                    .tier(
                        &key.workspace_id,
                        &status.config.agent_id,
                        &crate::page_signals::sender_evidence(address),
                    )
                    .await?;
                tiers.insert(address, tier);
            }
        }
        let counts = crate::page_signals::sender_thread_counts(
            &messages,
            now.saturating_sub(crate::page_signals::SENDER_WINDOW_MS),
        );
        let pages: Vec<PageSignals> = messages
            .iter()
            .zip(&senders)
            .map(|(thread, sender)| {
                let (trust, threads) = match sender {
                    Some((address, domain)) => (
                        tiers[address.as_str()],
                        counts.get(domain).copied().unwrap_or_default(),
                    ),
                    None => (pagis_core::TrustTier::Unknown, 0),
                };
                crate::page_signals::mail_signals(thread, trust, threads)
            })
            .collect();
        Ok(filter.preview(&pages, now))
    }
}

#[async_trait]
impl SignalSource for Gmail {
    async fn signals(
        &self,
        subject: &str,
        now: i64,
    ) -> Result<PageSignals, pagis_core::StoreError> {
        let key = SourceKey {
            workspace_id: self.config.workspace_id.clone(),
            connection_id: self.config.connection_id.clone(),
            resource: self.config.resource.clone(),
        };
        let messages = self.worker.store.parent_metadata(&key, subject).await?;
        let sender = crate::page_signals::thread_sender(&messages);
        let trust = match &sender {
            Some((address, _)) => {
                self.worker
                    .trust
                    .tier(
                        &key.workspace_id,
                        &self.config.agent_id,
                        &crate::page_signals::sender_evidence(address),
                    )
                    .await?
            }
            None => pagis_core::TrustTier::Unknown,
        };
        let threads = match &sender {
            Some((_, domain)) => {
                self.worker
                    .store
                    .parents_with_metadata(
                        &key,
                        crate::page_signals::SENDER_DOMAIN_FIELD,
                        domain,
                        now.saturating_sub(crate::page_signals::SENDER_WINDOW_MS),
                    )
                    .await?
            }
            None => 0,
        };
        Ok(crate::page_signals::mail_signals(&messages, trust, threads))
    }
}

#[async_trait]
impl Source for Gmail {
    async fn collect(
        &self,
        checkpoint: &serde_json::Value,
        since: i64,
    ) -> Result<SourcePage, Error> {
        let mut page = match self.provider().await?.sync_gmail(checkpoint, since).await {
            Ok(page) => page,
            Err(error) if error.code == ProviderErrorCode::NotFound && !checkpoint.is_null() => {
                return Err(Error::ResetRequired);
            }
            Err(error) => return Err(self.error(error).await),
        };
        let connection = self.worker.authorized(&self.config).await?;
        for event in &mut page.arrivals {
            event.metadata["mailbox"] = connection.alias.clone().into();
        }
        Ok(SourcePage {
            historical: page.historical,
            arrivals: page.arrivals,
            checkpoint: page.checkpoint,
            caught_up: page.caught_up,
            changes: page.changes,
        })
    }
    async fn read(&self, item: &SourceChange) -> Result<SourceContent, Error> {
        let content = match self
            .provider()
            .await?
            .knowledge_thread(item.parent.as_deref().ok_or_else(|| Error::NotFound)?)
            .await
        {
            Ok(content) => content,
            Err(error) => return Err(self.error(error).await),
        };
        self.worker.authorized(&self.config).await?;
        Ok(content)
    }
    async fn metadata(&self, item: &SourceChange) -> Result<serde_json::Value, Error> {
        match self
            .provider()
            .await?
            .gmail_message_metadata(&item.id, &item.version)
            .await
        {
            Ok(metadata) => Ok(metadata),
            Err(error) => Err(self.error(error).await),
        }
    }
}

/// Why the sync of one resource is not authorized, or `None` when every
/// check holds. `stored` is the configuration the store holds now.
fn refusal(
    config: &SyncConfig,
    stored: Option<&SyncConfig>,
    agent: Option<&pagis_core::Agent>,
    connection: Option<&Connection>,
    grant: Option<&Grant>,
) -> Option<&'static str> {
    let Some(stored) = stored else {
        return Some("the sync resource is not configured");
    };
    if stored != config {
        return Some("the sync configuration changed during the pass");
    }
    if !stored.enabled {
        return Some("the sync is paused");
    }
    let Some(agent) = agent else {
        return Some("the agent is gone");
    };
    if agent.workspace_id != config.workspace_id {
        return Some("the agent is not in the workspace");
    }
    if agent.status != AgentStatus::Active {
        return Some("the agent is not active");
    }
    let Some(connection) = connection else {
        return Some("the connection is gone");
    };
    if connection.status != Connection::CONNECTED {
        return Some("the connection is not connected");
    }
    if !connection
        .authorized_capabilities
        .contains(&config.required_capability)
    {
        return Some("the connection does not authorize the capability");
    }
    let Some(grant) = grant else {
        return Some("the agent has no live grant for the connection");
    };
    if !grant.capabilities().contains(&config.required_capability) {
        return Some("the grant does not cover the capability");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{Agent, ConnectionId, GrantId};
    use pagis_testkit::fixture;

    fn parts() -> (SyncConfig, Agent, Connection, Grant) {
        let workspace = fixture::workspace();
        let agent = fixture::agent(&workspace.id);
        let connection = Connection {
            id: ConnectionId::generate(),
            workspace_id: workspace.id.clone(),
            provider: "google".into(),
            alias: "personal".into(),
            display_name: "Personal".into(),
            status: Connection::CONNECTED.into(),
            authorized_capabilities: vec!["gmail_read".into()],
            config: serde_json::json!({}),
            created_at: 1,
        };
        let grant = Grant {
            id: GrantId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent.id.clone(),
            resource_kind: Grant::CONNECTION_KIND.into(),
            resource_id: Some(connection.id.to_string()),
            scope: Grant::connection_scope(&["gmail_read".into()]),
            revision: 1,
            created_at: 1,
            revoked_at: None,
        };
        let config = SyncConfig {
            workspace_id: workspace.id,
            connection_id: connection.id.clone(),
            resource: "gmail".into(),
            agent_id: agent.id.clone(),
            required_capability: "gmail_read".into(),
            enabled: true,
            since: 0,
            filter: pagis_google::gmail_filter::default_filter(),
        };
        (config, agent, connection, grant)
    }

    /// Each refusal names the check that failed, so the log says why
    /// the sync status shows "requires authorization".
    #[test]
    fn a_refusal_names_the_check_that_failed() {
        let (config, agent, connection, grant) = parts();
        let check = |stored: Option<&SyncConfig>,
                     agent: &Agent,
                     connection: &Connection,
                     grant: Option<&Grant>| {
            refusal(&config, stored, Some(agent), Some(connection), grant)
        };
        assert_eq!(
            check(Some(&config), &agent, &connection, Some(&grant)),
            None
        );
        assert_eq!(
            refusal(&config, None, None, None, None),
            Some("the sync resource is not configured")
        );
        let changed = SyncConfig {
            since: 5,
            ..config.clone()
        };
        assert_eq!(
            check(Some(&changed), &agent, &connection, Some(&grant)),
            Some("the sync configuration changed during the pass")
        );
        let paused = SyncConfig {
            enabled: false,
            ..config.clone()
        };
        assert_eq!(
            refusal(
                &paused,
                Some(&paused),
                Some(&agent),
                Some(&connection),
                Some(&grant)
            ),
            Some("the sync is paused")
        );
        assert_eq!(
            refusal(
                &config,
                Some(&config),
                None,
                Some(&connection),
                Some(&grant)
            ),
            Some("the agent is gone")
        );
        let retired = Agent {
            status: AgentStatus::Archived,
            ..agent.clone()
        };
        assert_eq!(
            check(Some(&config), &retired, &connection, Some(&grant)),
            Some("the agent is not active")
        );
        let disconnected = Connection {
            status: Connection::REAUTH_REQUIRED.into(),
            ..connection.clone()
        };
        assert_eq!(
            check(Some(&config), &agent, &disconnected, Some(&grant)),
            Some("the connection is not connected")
        );
        let narrow = Connection {
            authorized_capabilities: vec!["calendar_read".into()],
            ..connection.clone()
        };
        assert_eq!(
            check(Some(&config), &agent, &narrow, Some(&grant)),
            Some("the connection does not authorize the capability")
        );
        assert_eq!(
            check(Some(&config), &agent, &connection, None),
            Some("the agent has no live grant for the connection")
        );
        let other = Grant {
            scope: Grant::connection_scope(&["calendar_read".into()]),
            ..grant.clone()
        };
        assert_eq!(
            check(Some(&config), &agent, &connection, Some(&other)),
            Some("the grant does not cover the capability")
        );
    }

    #[test]
    fn mail_front_matter_is_one_line_and_an_empty_subject_gives_no_block() {
        let event = |subject: serde_json::Value| pagis_core::NormalizedEvent {
            provider_event_id: "message-1".into(),
            metadata: serde_json::json!({"subject": subject}),
            occurred_at: 0,
            landing: None,
        };

        assert_eq!(
            mail_front_matter(&event(serde_json::json!("  School\r\nupdates  "))),
            pagis_core::subject_page::FrontMatter {
                title: Some("School updates".into()),
                kind: Some("Source".into()),
                aliases: Vec::new(),
                links: Vec::new(),
            }
        );
        assert!(pagis_core::subject_page::is_page_kind("Source"));
        let empty = pagis_core::subject_page::FrontMatter::default();
        assert_eq!(mail_front_matter(&event(serde_json::json!(" \n "))), empty);
        assert_eq!(mail_front_matter(&event(serde_json::Value::Null)), empty);
    }
}
