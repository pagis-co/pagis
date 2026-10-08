//! The Knowledge trait tests of the suite.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_knowledge!`
//! below: the guard test of the parent module fails while one is
//! missing.
//!
//! The Knowledge store holds the JSON reads and the generation counters
//! that the two backends write with different SQL. A body that asserts
//! something about a record reads the parsed JSON, never the record
//! text: the Postgres side writes the record back through `jsonb_set`,
//! which sorts the keys and puts a space after each colon.

use std::sync::Arc;

use pagis_core::knowledge::*;
use pagis_core::reflection_filter::{Selection, Verdict};
use pagis_core::subject_page::{SubjectPage, TimelineEntry};
use pagis_core::{
    AgentId, Connection, ForgetKeys, ForgetTarget, Grant, GrantId, MemoryAccess, MemoryAuthor,
    MemoryChangeset, MemoryExposure, MemoryScope, MemoryStore, ScopedPath, SealError,
    SuppressionKey, WorkspaceId,
};

use super::{Backend, Bind};

async fn authorize(backend: &Backend, workspace: &WorkspaceId, agent: &AgentId, id: &str) {
    backend
        .stores()
        .connections
        .create(&Connection {
            id: id.to_string().into(),
            workspace_id: workspace.clone(),
            provider: "google".into(),
            alias: id.into(),
            display_name: "Gmail".into(),
            status: Connection::CONNECTED.into(),
            authorized_capabilities: vec!["gmail_read".into()],
            config: serde_json::json!({}),
            created_at: 1,
        })
        .await
        .unwrap();
    backend
        .stores()
        .grants
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: workspace.clone(),
            agent_id: agent.clone(),
            resource_kind: Grant::CONNECTION_KIND.into(),
            resource_id: Some(id.into()),
            scope: Grant::connection_scope(&["gmail_read".into()]),
            revision: 1,
            created_at: 1,
            revoked_at: None,
        })
        .await
        .unwrap();
}

async fn setup(backend: &Backend) -> (Arc<dyn KnowledgeStore>, SyncStatus) {
    let ws = backend.seeded_workspace().await;
    let sage = crate::fixture::agent(&ws.id);
    backend.stores().agents.create(&sage).await.unwrap();
    authorize(backend, &ws.id, &sage.id, "g").await;
    let store = Arc::clone(&backend.stores().knowledge);
    let status = store
        .configure(
            SyncConfig {
                workspace_id: ws.id,
                connection_id: "g".to_string().into(),
                resource: "gmail".into(),
                agent_id: sage.id,
                required_capability: "gmail_read".into(),
                enabled: true,
                since: 0,
                filter: pagis_google::gmail_filter::default_filter(),
            },
            10,
        )
        .await
        .unwrap();
    (store, status)
}

fn source(id: &str, at: i64) -> SourceChange {
    SourceChange {
        id: id.into(),
        version: "1".into(),
        kind: "mail".into(),
        parent: Some(format!("thread-{id}")),
        source_at: Some(at),
        operation: SourceOperation::Upsert,
        metadata: serde_json::Value::Null,
    }
}

fn mail(id: &str, thread: &str, at: i64, domain: &str) -> SourceChange {
    SourceChange {
        id: id.into(),
        version: "1".into(),
        kind: "mail".into(),
        parent: Some(thread.into()),
        source_at: Some(at),
        operation: SourceOperation::Upsert,
        metadata: serde_json::json!({
            "message_id": id,
            "from": format!("sender@{domain}"),
            "from_domain": domain,
            "received_at": at,
            "labels": ["INBOX"],
        }),
    }
}

/// One message acquired again, with the labels it carries now. Gmail
/// gives a message a new version each time its labels change.
fn relabelled(id: &str, thread: &str, at: i64, version: &str, labels: &[&str]) -> SourceChange {
    let mut change = mail(id, thread, at, "clinic.test");
    change.version = version.into();
    change.metadata["labels"] = serde_json::json!(labels);
    change
}

/// The change acquisition records for an id the mailbox no longer holds.
fn removed(id: &str, version: &str) -> SourceChange {
    SourceChange {
        id: id.into(),
        version: version.into(),
        kind: "mail".into(),
        parent: None,
        source_at: None,
        operation: SourceOperation::Deleted,
        metadata: serde_json::Value::Null,
    }
}

async fn acquire(
    store: &dyn KnowledgeStore,
    keys: &dyn ForgetKeys,
    key: &SourceKey,
    changes: Vec<SourceChange>,
) {
    let revision = store
        .status(key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap()
        .cursor_revision;
    store
        .acquire(
            key,
            &SourceBatch {
                historical: false,
                arrivals: vec![],
                expected_revision: revision,
                checkpoint: serde_json::json!({"page": 1}),
                caught_up: true,
                changes,
            },
            100,
            keys,
        )
        .await
        .unwrap();
}

fn selection(verdict: Verdict, rule: Option<usize>) -> Selection {
    Selection {
        verdict,
        rule,
        reason: "a test decided".into(),
    }
}

pub async fn a_new_filter_bumps_the_revision_and_starts_its_own_counts(backend: &Backend) {
    let (store, state) = setup(backend).await;
    assert_eq!(state.filter_revision, 1, "the first filter is revision 1");
    let key = state.key();
    store
        .record_reflection(
            &key,
            &PageReflection {
                page_path: "private/subjects/gmail/clinic.md".into(),
                page_subject: "clinic".into(),
                filter_revision: state.filter_revision,
                decided_by: ReflectionPass::Live,
                selection: selection(Verdict::Reflect, Some(0)),
                decided_at: 100,
            },
        )
        .await
        .unwrap();
    store
        .record_reflection(
            &key,
            &PageReflection {
                page_path: "private/subjects/gmail/shop.md".into(),
                page_subject: "shop".into(),
                filter_revision: state.filter_revision,
                decided_by: ReflectionPass::Live,
                // The last line of the filter skips, so no rule named it.
                selection: selection(Verdict::Skip, None),
                decided_at: 100,
            },
        )
        .await
        .unwrap();
    let counted = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counted.reflected_pages, 1);
    assert_eq!(counted.skipped_pages, 1);

    let mut config = state.config.clone();
    config.filter.default = Verdict::Reflect;
    let changed = store.configure(config, 200).await.unwrap();
    assert_eq!(changed.filter_revision, state.filter_revision + 1);
    assert_eq!(changed.reflected_pages, 0);
    assert_eq!(changed.skipped_pages, 0);

    // The same config again keeps the revision, so a save that does not
    // change the rules does not discard the counts.
    let same = store.configure(changed.config.clone(), 300).await.unwrap();
    assert_eq!(same.filter_revision, changed.filter_revision);
}

pub async fn a_later_pass_replaces_the_decision_of_the_same_revision(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    for verdict in [Verdict::Skip, Verdict::Reflect] {
        store
            .record_reflection(
                &key,
                &PageReflection {
                    page_path: "private/subjects/gmail/clinic.md".into(),
                    page_subject: "clinic".into(),
                    filter_revision: state.filter_revision,
                    decided_by: ReflectionPass::Live,
                    selection: selection(verdict, None),
                    decided_at: 100,
                },
            )
            .await
            .unwrap();
    }
    let counted = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(counted.reflected_pages, 1);
    assert_eq!(counted.skipped_pages, 0);
}

pub async fn page_signals_read_the_metadata_of_one_thread(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            mail("m1", "clinic", 10, "clinic.test"),
            mail("m2", "clinic", 20, "clinic.test"),
            mail("m3", "shop", 30, "shop.test"),
        ],
    )
    .await;
    let thread = store.parent_metadata(&key, "clinic").await.unwrap();
    assert_eq!(thread.len(), 2);
    assert_eq!(thread[0]["message_id"], "m1");
    assert_eq!(thread[1]["received_at"], 20);
    assert!(
        store
            .parent_metadata(&key, "absent")
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn a_preview_reads_the_metadata_of_every_thread_in_one_pass(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    assert!(store.parents_metadata(&key).await.unwrap().is_empty());
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            mail("m1", "clinic", 10, "clinic.test"),
            mail("m2", "shop", 30, "shop.test"),
            mail("m3", "clinic", 20, "clinic.test"),
        ],
    )
    .await;
    let threads = store.parents_metadata(&key).await.unwrap();
    assert_eq!(threads.len(), 2);
    let clinic = threads
        .iter()
        .find(|(parent, _)| parent == "clinic")
        .map(|(_, messages)| messages)
        .unwrap();
    assert_eq!(clinic.len(), 2);
    assert_eq!(clinic[0]["message_id"], "m1");
    assert_eq!(clinic[1]["received_at"], 20);
    let other = SourceKey {
        resource: "calendar".into(),
        ..key.clone()
    };
    assert!(store.parents_metadata(&other).await.unwrap().is_empty());
}

pub async fn sender_threads_count_distinct_parents_inside_the_window(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            mail("m1", "one", 10, "shop.test"),
            mail("m2", "one", 20, "shop.test"),
            mail("m3", "two", 30, "shop.test"),
            mail("m4", "three", 5, "shop.test"),
            mail("m5", "four", 40, "clinic.test"),
        ],
    )
    .await;
    assert_eq!(
        store
            .parents_with_metadata(&key, "from_domain", "shop.test", 0)
            .await
            .unwrap(),
        3
    );
    // A thread whose newest message is older than the window is out.
    assert_eq!(
        store
            .parents_with_metadata(&key, "from_domain", "shop.test", 15)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .parents_with_metadata(&key, "from_domain", "clinic.test", 0)
            .await
            .unwrap(),
        1
    );
    assert!(
        store
            .parents_with_metadata(&key, "from_domain'; DROP TABLE source_versions; --", "x", 0)
            .await
            .is_err()
    );
}

pub async fn acquisition_commits_work_and_cursor_together_and_rejects_replay(backend: &Backend) {
    let (store, initial) = setup(backend).await;
    let batch = SourceBatch {
        historical: true,
        arrivals: vec![],
        expected_revision: initial.cursor_revision,
        checkpoint: serde_json::json!({"page":"next"}),
        caught_up: false,
        changes: vec![source("message-1", 5)],
    };
    store
        .acquire(&initial.key(), &batch, 11, backend.keys())
        .await
        .unwrap();
    assert!(
        store
            .acquire(&initial.key(), &batch, 12, backend.keys())
            .await
            .is_err()
    );
    let current = store
        .status(&initial.key(), pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.checkpoint, serde_json::json!({"page":"next"}));
}

pub async fn changing_import_scope_restarts_acquisition(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({"history":"1"}),
                caught_up: true,
                changes: vec![source("message-1", 5)],
            },
            100,
            backend.keys(),
        )
        .await
        .unwrap();
    let mut config = state.config;
    config.since = 50;
    let reset = store.configure(config, 101).await.unwrap();
    assert!(reset.checkpoint.is_null());
    assert!(!reset.caught_up);
}

pub async fn post_baseline_arrival_can_be_discovered_in_the_historical_scan(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let mut batch = SourceBatch {
        expected_revision: state.cursor_revision,
        checkpoint: serde_json::json!({"history":"42"}),
        caught_up: false,
        historical: true,
        arrivals: vec![],
        changes: vec![source("live", 11)],
    };
    store
        .acquire(&key, &batch, 12, backend.keys())
        .await
        .unwrap();
    assert!(store.arrivals(&key, 10, 14).await.unwrap().is_empty());
    batch.expected_revision = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap()
        .cursor_revision;
    batch.historical = false;
    batch.arrivals = vec![pagis_core::NormalizedEvent {
        provider_event_id: "live".into(),
        metadata: serde_json::json!({}),
        occurred_at: 11,
        landing: None,
    }];
    store
        .acquire(&key, &batch, 13, backend.keys())
        .await
        .unwrap();
    let arrivals = store.arrivals(&key, 10, 14).await.unwrap();
    assert_eq!(arrivals.len(), 1);
    assert_eq!(arrivals[0].observed_at, 12);
    assert!(arrivals[0].historical);
}

pub async fn source_forget_blocks_reimport_and_keeps_other_sources(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({"page":1}),
                caught_up: true,
                changes: vec![source("secret", 5), source("retained", 6)],
            },
            11,
            backend.keys(),
        )
        .await
        .unwrap();
    let forget = &backend.stores().forget;
    let target = ForgetTarget::Source {
        source: key.clone(),
        source_id: "secret".into(),
    };
    let preview = forget.preview(&key.workspace_id, &target).await.unwrap();
    forget
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            12,
            backend.keys(),
        )
        .await
        .unwrap();
    assert!(
        forget
            .is_suppressed(&key, "secret", backend.keys())
            .await
            .unwrap()
    );
    assert!(
        !forget
            .is_suppressed(&key, "retained", backend.keys())
            .await
            .unwrap()
    );
    let grant_binds = [
        Bind::from(key.workspace_id.as_str()),
        Bind::from(key.connection_id.as_str()),
    ];
    let grant_id = backend
        .rows()
        .text(
            "SELECT id FROM grants WHERE workspace_id=? AND resource_id=?",
            &grant_binds,
        )
        .await
        .expect("read the grant of the Connection")
        .expect("the grant is there");
    let grant_revision = backend
        .count(
            "SELECT revision FROM grants WHERE workspace_id=? AND resource_id=?",
            &grant_binds,
        )
        .await
        .expect("read the revision of that grant");
    assert!(
        !forget
            .permits_provenance(
                &key.workspace_id,
                &[pagis_core::MemoryExposure {
                    grant_id: grant_id.into(),
                    revision: grant_revision,
                }],
            )
            .await
            .unwrap(),
        "a live grant does not restore suppressed source content"
    );

    let state = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({"page":2}),
                caught_up: true,
                changes: vec![SourceChange {
                    version: "2".into(),
                    ..source("secret", 7)
                }],
            },
            13,
            backend.keys(),
        )
        .await
        .unwrap();
    let item_binds = [
        Bind::from(key.workspace_id.as_str()),
        Bind::from(key.connection_id.as_str()),
        Bind::from(&key.resource),
    ];
    let version = backend
        .rows()
        .text(
            "SELECT version FROM source_items WHERE workspace_id=? AND connection_id=? \
             AND resource=? AND id='secret'",
            &item_binds,
        )
        .await
        .expect("read the suppressed item")
        .expect("the suppressed item is there");
    assert_eq!(version, "1", "the blocked reimport left the first version");
    assert_eq!(
        backend
            .count(
                "SELECT COUNT(*) FROM source_versions WHERE workspace_id=? AND connection_id=? \
                 AND resource=? AND id='secret' AND version='2'",
                &item_binds,
            )
            .await
            .expect("count the versions of the blocked item"),
        0,
        "the blocked reimport wrote a version"
    );
}

pub async fn a_failed_source_purge_stays_blocked_until_retry(backend: &Backend) {
    let (knowledge, state) = setup(backend).await;
    let key = state.key();
    knowledge
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({}),
                caught_up: true,
                changes: vec![source("secret", 5)],
            },
            11,
            backend.keys(),
        )
        .await
        .unwrap();
    let store = &backend.stores().forget;
    let target = ForgetTarget::Source {
        source: key.clone(),
        source_id: "secret".into(),
    };
    let preview = store.preview(&key.workspace_id, &target).await.unwrap();
    let operation = store
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            12,
            backend.keys(),
        )
        .await
        .unwrap();
    assert!(
        store
            .complete(&key.workspace_id, &operation.id)
            .await
            .is_err()
    );
    store
        .record_failure(&key.workspace_id, &operation.id, "storage unavailable")
        .await
        .unwrap();
    let failed = store
        .operation(&key.workspace_id, &operation.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.error.as_deref(), Some("storage unavailable"));
    assert!(
        store
            .is_suppressed(&key, "secret", backend.keys())
            .await
            .unwrap()
    );
    assert!(
        store
            .purge_structured(&key.workspace_id, &operation.id)
            .await
            .is_err()
    );
    store.retry(&key.workspace_id, &operation.id).await.unwrap();
    store
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    store
        .memory_purged(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    store
        .complete(&key.workspace_id, &operation.id)
        .await
        .unwrap();
}

/// A copy of the database alone, such as a dump or a Backup, cannot
/// test a known source id against the suppression rows. The key of the
/// suppression identity is not in the database, so no value that the
/// database holds computes the identity (ADR-0008).
pub async fn a_copy_of_the_database_cannot_compute_the_identity_of_a_forgotten_source(
    backend: &Backend,
) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![source("secret", 5)],
    )
    .await;
    let forget = &backend.stores().forget;
    let target = ForgetTarget::Source {
        source: key.clone(),
        source_id: "secret".into(),
    };
    let preview = forget.preview(&key.workspace_id, &target).await.unwrap();
    let operation = forget
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            12,
            backend.keys(),
        )
        .await
        .unwrap();
    // The purge removes the item and clears the target of the
    // operation. The suppression row then keeps the only keyed record
    // of the id.
    forget
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    let identity = backend
        .rows()
        .text(
            "SELECT identity FROM forget_suppressions WHERE workspace_id=?",
            &[Bind::from(key.workspace_id.as_str())],
        )
        .await
        .expect("read the suppression row")
        .expect("the Forget wrote a suppression row");
    // The key that the Tenant Data Key derives computes the identity,
    // so the check below uses the formula that the stores use.
    assert_eq!(
        backend
            .keys()
            .suppression_key(&key.workspace_id)
            .unwrap()
            .identity(&key, "secret"),
        identity
    );

    let values = backend
        .rows()
        .every_value()
        .await
        .expect("read every value of the database");
    assert!(
        values.iter().any(|value| value == identity.as_bytes()),
        "the read of the database found the suppression row"
    );
    let keys = values
        .iter()
        .filter(|value| pagis_core::suppression_identity(value, &key, "secret") == identity)
        .count();
    assert_eq!(
        keys, 0,
        "a value that the database holds computes the suppression identity of the source id"
    );
}

/// A Forget, and then a new retrieval of the same source id, is still
/// suppressed after the purge and after a daemon restart. The restart
/// is a new holder of the Tenant Data Keys over the same `secrets.enc`,
/// so the Workspace derives the same suppression key (ADR-0008).
pub async fn a_forgotten_source_stays_suppressed_after_the_purge_and_a_restart(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![source("secret", 5), source("retained", 6)],
    )
    .await;
    let forget = &backend.stores().forget;
    let target = ForgetTarget::Source {
        source: key.clone(),
        source_id: "secret".into(),
    };
    let preview = forget.preview(&key.workspace_id, &target).await.unwrap();
    let operation = forget
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            12,
            backend.keys(),
        )
        .await
        .unwrap();
    forget
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();

    let restarted = backend.restarted_keys();
    assert!(
        forget
            .is_suppressed(&key, "secret", &restarted)
            .await
            .unwrap()
    );
    assert!(
        !forget
            .is_suppressed(&key, "retained", &restarted)
            .await
            .unwrap()
    );
    acquire(
        store.as_ref(),
        &restarted,
        &key,
        vec![SourceChange {
            version: "2".into(),
            ..source("secret", 7)
        }],
    )
    .await;
    let binds = [
        Bind::from(key.workspace_id.as_str()),
        Bind::from(key.connection_id.as_str()),
        Bind::from(&key.resource),
    ];
    for table in ["source_items", "source_versions"] {
        assert_eq!(
            backend
                .count(
                    &format!(
                        "SELECT COUNT(*) FROM {table} WHERE workspace_id=? AND connection_id=? \
                         AND resource=? AND id='secret'"
                    ),
                    &binds,
                )
                .await
                .unwrap(),
            0,
            "the new retrieval wrote the forgotten item to {table}"
        );
    }
}

/// A key source with no key, for a path that must not ask for one.
struct NoKeys;

impl ForgetKeys for NoKeys {
    fn suppression_key(&self, _: &WorkspaceId) -> Result<SuppressionKey, SealError> {
        Err(SealError(
            "a Workspace that forgot nothing asked for a key".into(),
        ))
    }
}

/// A Workspace that never forgot anything acquires and checks a source
/// id with no suppression key, so no Tenant Data Key is made for it.
/// The check is a query of the suppression rows (ADR-0008).
pub async fn a_workspace_that_never_forgot_anything_asks_for_no_key(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();

    acquire(store.as_ref(), &NoKeys, &key, vec![source("m-1", 5)]).await;

    assert!(
        !backend
            .stores()
            .forget
            .is_suppressed(&key, "m-1", &NoKeys)
            .await
            .unwrap()
    );
}

/// Every text value of the forgotten item holds this mark, and no other
/// value of a fixture does. A value of the database that holds it is
/// therefore a record of the forgotten item, and not a shared word.
const FORGOTTEN: &str = "forgotten-3f9a";

/// The thread of the forgotten item. It holds no other message.
fn forgotten_thread() -> String {
    format!("thread-{FORGOTTEN}")
}

/// The Subject Page of the forgotten item's thread.
fn forgotten_page() -> String {
    format!("private/subjects/gmail/{}.md", forgotten_thread())
}

/// The item that stays, in a thread of its own.
fn kept() -> SourceChange {
    mail("kept-1", "thread-kept", 22, "kept.test")
}

/// The Subject Page of the thread that stays.
const KEPT_PAGE: &str = "private/subjects/gmail/thread-kept.md";

/// The metadata of the forgotten item. Each value holds [`FORGOTTEN`].
fn forgotten_metadata() -> serde_json::Value {
    serde_json::json!({
        "message_id": FORGOTTEN,
        "from": format!("sender@{FORGOTTEN}.test"),
        "from_domain": format!("{FORGOTTEN}.test"),
        "subject": format!("The {FORGOTTEN} invoice"),
        "received_at": 21,
        "labels": [format!("{FORGOTTEN}-label")],
    })
}

/// One version of the forgotten item as the Sync retrieves it, with or
/// without its metadata. Each text field holds [`FORGOTTEN`].
fn forgotten_item(version: &str, metadata: bool) -> SourceChange {
    SourceChange {
        id: FORGOTTEN.into(),
        version: format!("{FORGOTTEN}-v{version}"),
        kind: format!("{FORGOTTEN}-kind"),
        parent: Some(forgotten_thread()),
        source_at: Some(21),
        operation: SourceOperation::Upsert,
        metadata: if metadata {
            forgotten_metadata()
        } else {
            serde_json::Value::Null
        },
    }
}

/// The normalized event of the live arrival of one message.
fn arrival_of(id: &str, thread: &str) -> pagis_core::NormalizedEvent {
    pagis_core::NormalizedEvent {
        provider_event_id: id.into(),
        metadata: serde_json::json!({"thread_id": thread, "subject": format!("About {id}")}),
        occurred_at: 21,
        landing: None,
    }
}

/// A matcher for a pass that no Event Subscription sees.
struct NoRule;

impl pagis_core::EventMatcher for NoRule {
    fn matches(&self, _: &serde_json::Value, _: &serde_json::Value) -> bool {
        false
    }
}

/// The values of the database that hold `mark`, as text.
async fn holding(backend: &Backend, mark: &str) -> Vec<String> {
    backend
        .rows()
        .every_value()
        .await
        .expect("read every value of the database")
        .into_iter()
        .filter(|value| {
            value
                .windows(mark.len())
                .any(|window| window == mark.as_bytes())
        })
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .collect()
}

/// Forget the forgotten item and run the structured purge. The answer
/// is the id of the operation.
async fn forget_the_item(backend: &Backend, key: &SourceKey) -> String {
    let forget = &backend.stores().forget;
    let target = ForgetTarget::Source {
        source: key.clone(),
        source_id: FORGOTTEN.into(),
    };
    let preview = forget.preview(&key.workspace_id, &target).await.unwrap();
    let operation = forget
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            30,
            backend.keys(),
        )
        .await
        .unwrap();
    forget
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    operation.id
}

/// One count of the rows of the item that stays, for a body that shows
/// the purge left them.
async fn kept_rows(backend: &Backend, key: &SourceKey, sql: &str) -> i64 {
    backend
        .count(
            sql,
            &[
                Bind::from(key.workspace_id.as_str()),
                Bind::from(key.connection_id.as_str()),
            ],
        )
        .await
        .expect("count the rows of the item that stays")
}

/// A Forget and its structured purge leave no value of the forgotten
/// item in any table. The fixture writes what the product writes for
/// one synced message: the current record and each source version with
/// its arrival, the completed metadata, a failed delivery, the Incoming
/// Event and the Wake-up of the delivery, the decision of the filter
/// about the Subject Page of its thread, and a Schedule that Reflection
/// set on that page. After the purge the suppression row, with its
/// keyed identity, is the only record of the item (ADR-0008).
pub async fn a_forget_purge_leaves_no_value_of_the_forgotten_item_in_any_table(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    // A live retrieval of the item, which has no metadata yet, and of
    // the item that stays.
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: false,
                arrivals: vec![
                    arrival_of(FORGOTTEN, &forgotten_thread()),
                    arrival_of("kept-1", "thread-kept"),
                ],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({"page": 1}),
                caught_up: true,
                changes: vec![forgotten_item("1", false), kept()],
            },
            11,
            backend.keys(),
        )
        .await
        .unwrap();
    // Acquisition completes the metadata of the version (ADR-0011).
    let incomplete = store.versions_without_metadata(&key, 10).await.unwrap();
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].source.id, FORGOTTEN);
    store
        .complete_metadata(&key, incomplete[0].sequence, &forgotten_metadata())
        .await
        .unwrap();
    // The first delivery of the arrival fails, and the arrival waits.
    let due = store.arrivals(&key, 10, 12).await.unwrap();
    let arrival = due
        .iter()
        .find(|version| version.source.id == FORGOTTEN)
        .expect("the arrival of the item is due");
    store
        .fail_arrival(&key, arrival.sequence, false, "the source is busy", 12)
        .await
        .unwrap();
    // A new label gives the message a second version.
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("2", true)],
    )
    .await;
    // The filter decides about the Subject Page of each thread.
    for (page, subject) in [
        (forgotten_page(), forgotten_thread()),
        (KEPT_PAGE.to_string(), "thread-kept".to_string()),
    ] {
        store
            .record_reflection(
                &key,
                &PageReflection {
                    page_path: page,
                    page_subject: subject,
                    filter_revision: state.filter_revision,
                    decided_by: ReflectionPass::Live,
                    selection: selection(Verdict::Reflect, Some(0)),
                    decided_at: 13,
                },
            )
            .await
            .unwrap();
    }
    // The delivery stores each occurrence and wakes the Agent for both
    // Subject Pages.
    backend
        .stores()
        .triggers
        .ingest(
            pagis_core::IngestBatch {
                workspace_id: key.workspace_id.clone(),
                source: pagis_core::EventSource::connection(key.connection_id.clone()),
                agent_id: None,
                event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
                cursor: None,
                events: vec![
                    arrival_of(FORGOTTEN, &forgotten_thread()),
                    arrival_of("kept-1", "thread-kept"),
                ],
                received_at: 14,
                baseline: false,
            },
            &[],
            &NoRule,
            Some(&pagis_core::ArrivalRun {
                agent_id: state.config.agent_id.clone(),
                subject_paths: vec![forgotten_page(), KEPT_PAGE.to_string()],
                historical: false,
            }),
            Some(pagis_broker::MAIL_SYNC_RESOURCE),
            backend.keys(),
        )
        .await
        .unwrap();
    // Reflection set a Schedule on the Subject Page of the thread.
    let channel = crate::fixture::channel(&key.workspace_id);
    backend.stores().channels.create(&channel).await.unwrap();
    backend
        .stores()
        .schedules
        .create(&pagis_core::Schedule {
            id: pagis_core::ScheduleId::generate(),
            workspace_id: key.workspace_id.clone(),
            agent_id: state.config.agent_id.clone(),
            name: format!("Pay the {FORGOTTEN} invoice"),
            instruction: format!("Check that the {FORGOTTEN} invoice is paid."),
            subject_page_path: Some(forgotten_page()),
            channel_id: channel.id,
            root_message_id: None,
            kind: pagis_core::ScheduleKind::OneShot,
            cron_expression: None,
            interval_ms: None,
            anchor_at: None,
            timezone: "UTC".into(),
            scheduled_at: 60_000,
            next_due_at: Some(60_000),
            last_result: None,
            state: pagis_core::ScheduleState::Active,
            revision: 1,
            approved_revision: None,
            creator: pagis_core::CreatorKind::Agent,
            creating_run_id: None,
            created_at: 15,
            updated_at: 15,
            archived_at: None,
        })
        .await
        .unwrap();
    assert!(
        !holding(backend, FORGOTTEN).await.is_empty(),
        "the fixture wrote the forgotten item"
    );

    forget_the_item(backend, &key).await;

    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "a value of the database holds the forgotten item after the purge"
    );
    // The purge takes the forgotten item alone.
    assert_eq!(
        kept_rows(
            backend,
            &key,
            "SELECT COUNT(*) FROM source_versions WHERE workspace_id=? AND connection_id=? \
             AND id='kept-1' AND arrival IS NOT NULL",
        )
        .await,
        1,
        "the purge removed the version of the item that stays"
    );
    assert_eq!(
        kept_rows(
            backend,
            &key,
            "SELECT COUNT(*) FROM incoming_events WHERE workspace_id=? AND connection_id=? \
             AND provider_event_id='kept-1'",
        )
        .await,
        1,
        "the purge removed the Incoming Event of the item that stays"
    );
    assert_eq!(
        kept_rows(
            backend,
            &key,
            "SELECT COUNT(*) FROM page_reflections WHERE workspace_id=? AND connection_id=? \
             AND page_subject='thread-kept'",
        )
        .await,
        1,
        "the purge removed the decision of the page that stays"
    );
    let paths = backend
        .rows()
        .text(
            "SELECT subject_paths FROM wakeups WHERE workspace_id=? AND source_kind='arrival'",
            &[Bind::from(key.workspace_id.as_str())],
        )
        .await
        .expect("read the Wake-up of the delivery")
        .expect("the Wake-up of the delivery stays");
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&paths).unwrap(),
        vec![KEPT_PAGE.to_string()],
        "the Wake-up keeps the page that stays"
    );
}

/// A new retrieval of a forgotten item, after the purge, writes no row
/// that holds its id or its record, and the item stays suppressed. The
/// retrieval leaves nothing that a pipeline reads again: no arrival to
/// deliver and no version whose metadata acquisition asks for
/// (ADR-0008).
pub async fn a_new_retrieval_of_a_forgotten_item_writes_nothing_and_stays_suppressed(
    backend: &Backend,
) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    forget_the_item(backend, &key).await;

    // The Sync retrieves the item again: the version it had, with no
    // metadata, and a new version with a live arrival.
    let revision = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap()
        .cursor_revision;
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: false,
                arrivals: vec![arrival_of(FORGOTTEN, &forgotten_thread())],
                expected_revision: revision,
                checkpoint: serde_json::json!({"page": 2}),
                caught_up: true,
                changes: vec![forgotten_item("1", false), forgotten_item("2", true)],
            },
            40,
            backend.keys(),
        )
        .await
        .unwrap();

    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "the new retrieval wrote a value of the forgotten item"
    );
    assert!(
        backend
            .stores()
            .forget
            .is_suppressed(&key, FORGOTTEN, backend.keys())
            .await
            .unwrap(),
        "the new retrieval is not suppressed"
    );
    assert!(
        store
            .arrivals(&key, 10, 1_000)
            .await
            .unwrap()
            .iter()
            .all(|version| version.source.id != FORGOTTEN),
        "the new retrieval left an arrival to deliver"
    );
    assert!(
        store
            .versions_without_metadata(&key, 10)
            .await
            .unwrap()
            .is_empty(),
        "the new retrieval left a version whose metadata acquisition asks for"
    );
}

/// A word of the forgotten item's text. It is one token for both
/// full-text engines, and no other word of the fixture starts with its
/// first letters, so an FTS5 segment holds it whole.
const FORGOTTEN_WORD: &str = "zqxforgot3f9a";

/// Write one memory page, as the arrival of an item or a Reflection
/// does. The answer is the commit.
async fn write_page(
    memory: &dyn MemoryStore,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    access: &MemoryAccess,
    path: &str,
    content: &str,
) -> String {
    let revision = memory
        .load_indexes(workspace_id, access)
        .await
        .unwrap()
        .revision;
    let mut changeset = MemoryChangeset::default();
    changeset
        .writes
        .insert(ScopedPath::parse(path).unwrap(), content.to_string());
    memory
        .commit(
            workspace_id,
            agent_id,
            access,
            &MemoryAuthor {
                name: "Pagis".into(),
                email: "system@pagis.local".into(),
            },
            &changeset,
            revision.as_deref(),
            "Append the arrivals",
            None,
        )
        .await
        .unwrap()
}

/// After a Forget and its purges, the derived full-text index of Memory
/// Search does not find the text of the forgotten item, and no row of
/// the database holds that text or the page of the item. The index
/// applies no Forget rule of its own, so an empty answer means that the
/// index holds no row of the page (ADR-0008).
pub async fn a_forget_leaves_no_text_of_the_item_in_the_memory_search_index(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let grant = backend
        .stores()
        .grants
        .live_for_resource(
            &key.workspace_id,
            &agent,
            Grant::CONNECTION_KIND,
            key.connection_id.as_str(),
        )
        .await
        .unwrap()
        .expect("the Agent holds a grant of the Connection");
    let reader = MemoryAccess::agent(
        agent.clone(),
        [MemoryExposure {
            grant_id: grant.id,
            revision: grant.revision,
        }],
    );
    let root = tempfile::tempdir().expect("a temp directory for the memory");
    let memory = pagis_memory::GitMemoryStore::new(
        root.path(),
        Arc::clone(&backend.stores().forget),
        Arc::clone(&backend.stores().memory_pages),
    );
    // The arrival of the item put its words on the Subject Page of its
    // thread.
    let page = SubjectPage {
        timeline: vec![TimelineEntry {
            source_reference: format!("gmail:g:{FORGOTTEN}:{FORGOTTEN}-v1"),
            source_time: 21,
            words: format!("The {FORGOTTEN_WORD} invoice is due on Friday."),
        }],
        ..Default::default()
    }
    .render();
    write_page(
        &memory,
        &key.workspace_id,
        &agent,
        &reader,
        &forgotten_page(),
        &page,
    )
    .await;
    // A note with no word of the item stays.
    write_page(
        &memory,
        &key.workspace_id,
        &agent,
        &MemoryAccess::agent(agent.clone(), []),
        "shared/kept.md",
        "# Kept\n\nThe lanternfish note stays.\n",
    )
    .await;
    assert_eq!(
        memory
            .search_pages(
                &key.workspace_id,
                &reader,
                MemoryScope::Private,
                FORGOTTEN_WORD,
                10
            )
            .await
            .unwrap()
            .len(),
        1,
        "Memory Search finds the text of the item before the Forget"
    );

    let target = ForgetTarget::Source {
        source: key.clone(),
        source_id: FORGOTTEN.into(),
    };
    let preview = memory
        .preview_forget(&key.workspace_id, &target)
        .await
        .unwrap();
    let operation = memory
        .begin_forget(&key.workspace_id, &target, &preview, 30, backend.keys())
        .await
        .unwrap();
    let forget = &backend.stores().forget;
    forget
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    memory
        .purge_forgotten(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    forget
        .memory_purged(&key.workspace_id, &operation.id)
        .await
        .unwrap();
    forget
        .complete(&key.workspace_id, &operation.id)
        .await
        .unwrap();

    let index = &backend.stores().memory_pages;
    assert!(
        index
            .search(&key.workspace_id, "", FORGOTTEN_WORD, 10)
            .await
            .unwrap()
            .is_empty(),
        "the full-text index finds the text of the forgotten item"
    );
    assert_eq!(
        index
            .search(&key.workspace_id, "", "lanternfish", 10)
            .await
            .unwrap()
            .len(),
        1,
        "the full-text index lost the note that stays"
    );
    assert_eq!(
        holding(backend, FORGOTTEN_WORD).await,
        Vec::<String>::new(),
        "a value of the database holds the text of the forgotten item"
    );
    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "a value of the database holds the forgotten item or its page"
    );
}

/// A matcher for a rule that every event wakes.
struct EveryEvent;

impl pagis_core::EventMatcher for EveryEvent {
    fn matches(&self, _: &serde_json::Value, _: &serde_json::Value) -> bool {
        true
    }
}

/// The cursor that a pass of the Event Subscription collector commits.
const MAIL_CURSOR: &str = "mail-cursor-2";

/// An active Event Subscription of the Agent of the Sync, on the mail
/// of its Connection, with no filter. Each event that a pass stores
/// wakes it.
async fn mail_rule(backend: &Backend, state: &SyncStatus) -> pagis_core::EventSubscription {
    let channel = crate::fixture::channel(&state.config.workspace_id);
    backend.stores().channels.create(&channel).await.unwrap();
    let rule = pagis_core::EventSubscription {
        id: pagis_core::EventSubscriptionId::generate(),
        workspace_id: state.config.workspace_id.clone(),
        agent_id: state.config.agent_id.clone(),
        source: pagis_core::EventSource::connection(state.config.connection_id.clone()),
        event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
        source_version: "1".into(),
        name: "New mail".into(),
        instruction: "Read the new mail.".into(),
        channel_id: channel.id,
        root_message_id: None,
        filter: serde_json::json!({}),
        creator: pagis_core::CreatorKind::User,
        state: pagis_core::EventSubscriptionState::Active,
        revision: 1,
        approved_revision: Some(1),
        watermark_at: None,
        blocked_reason: None,
        created_at: 15,
        updated_at: 15,
        archived_at: None,
    };
    backend.stores().subscriptions.create(&rule).await.unwrap();
    rule
}

/// A later event about the forgotten item, as the Event Subscription
/// collector gives it after a new label: the id of the item, and the
/// metadata of the item, whose text values hold [`FORGOTTEN`].
fn forgotten_event() -> pagis_core::NormalizedEvent {
    pagis_core::NormalizedEvent {
        provider_event_id: FORGOTTEN.into(),
        metadata: forgotten_metadata(),
        occurred_at: 40,
        landing: None,
    }
}

/// The Sync acquires the forgotten item and the item that stays, a rule
/// waits for the mail of the Connection, and the Person forgets the
/// item. The purge follows. The answer is the key of the Sync and the
/// rule.
async fn a_forgotten_mail(backend: &Backend) -> (SourceKey, pagis_core::EventSubscription) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let rule = mail_rule(backend, &state).await;
    forget_the_item(backend, &key).await;
    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "a value of the database holds the forgotten item after the purge"
    );
    (key, rule)
}

/// One pass of the mail of the Connection, with `rule` live. The
/// resource is the one that the declaration of the mail event of a
/// Google Connection names, which is the resource of the Sync.
async fn ingest_mail(
    backend: &Backend,
    key: &SourceKey,
    rule: &pagis_core::EventSubscription,
    events: Vec<pagis_core::NormalizedEvent>,
    keys: &dyn ForgetKeys,
) -> pagis_core::IngestOutcome {
    backend
        .stores()
        .triggers
        .ingest(
            pagis_core::IngestBatch {
                workspace_id: key.workspace_id.clone(),
                source: pagis_core::EventSource::connection(key.connection_id.clone()),
                agent_id: None,
                event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
                cursor: Some(MAIL_CURSOR.into()),
                events,
                received_at: 41,
                baseline: false,
            },
            std::slice::from_ref(rule),
            &EveryEvent,
            None,
            Some(pagis_broker::MAIL_SYNC_RESOURCE),
            keys,
        )
        .await
        .expect("ingest the pass")
}

/// After a Forget of one message and its purge, a later event with the
/// same provider event id writes no row and wakes nothing. The store
/// drops it and keeps no hidden form of it, and the pass still commits
/// its cursor, so the collector moves past the event. An event about
/// another message of the same Connection in the same pass is stored
/// and wakes the Agent, so the pass drops the event of the forgotten
/// message alone (ADR-0008).
pub async fn an_event_of_a_forgotten_item_writes_no_row_and_the_others_still_ingest(
    backend: &Backend,
) {
    let (key, rule) = a_forgotten_mail(backend).await;

    let outcome = ingest_mail(
        backend,
        &key,
        &rule,
        vec![forgotten_event(), arrival_of("kept-1", "thread-kept")],
        backend.keys(),
    )
    .await;

    assert_eq!(
        outcome
            .events
            .iter()
            .map(|event| event.provider_event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["kept-1"],
        "the pass stores the event of the other message alone"
    );
    assert!(
        outcome.combined.is_empty(),
        "the pass combined an event into a Wake-up"
    );
    assert_eq!(outcome.created.len(), 1, "the other message woke no rule");
    assert_eq!(
        outcome.created[0].source_count, 1,
        "the Wake-up links the event of the forgotten item"
    );
    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "the ingest wrote a value of the forgotten item"
    );
    assert_eq!(
        backend
            .count(
                "SELECT COUNT(*) FROM wakeups WHERE subscription_id=?",
                &[Bind::from(rule.id.as_str())],
            )
            .await
            .unwrap(),
        1,
        "the event of the forgotten item made a Wake-up"
    );
    assert_eq!(
        backend
            .stores()
            .triggers
            .cursor(
                &key.workspace_id,
                &key.connection_id,
                pagis_broker::MAIL_MESSAGE_RECEIVED,
            )
            .await
            .unwrap()
            .as_deref(),
        Some(MAIL_CURSOR),
        "the pass did not commit its cursor"
    );
}

/// A Workspace that never forgot anything ingests an event with no
/// suppression key, so no Tenant Data Key is made for it. The check is
/// a query of the suppression rows (ADR-0008).
pub async fn an_ingest_in_a_workspace_that_never_forgot_anything_asks_for_no_key(
    backend: &Backend,
) {
    let (_, state) = setup(backend).await;
    let key = state.key();
    let rule = mail_rule(backend, &state).await;

    let outcome = ingest_mail(
        backend,
        &key,
        &rule,
        vec![arrival_of("m-1", "thread-m-1")],
        &NoKeys,
    )
    .await;

    assert_eq!(outcome.events.len(), 1, "the pass stored no event");
    assert_eq!(outcome.created.len(), 1, "the event woke no rule");
}

/// The note that stays beside the Subject Page of the forgotten item.
const KEPT_NOTE: &str = "shared/kept.md";

/// The title that the arrival of the forgotten item gave its Subject
/// Page. The title is a field of the page, so it holds [`FORGOTTEN`].
fn forgotten_title() -> String {
    format!("The {FORGOTTEN} invoice")
}

/// The commit of a revert that the owner made of the commit that wrote
/// the page of the item. The store keeps it as text, so any id does.
const REVERT_SHA: &str = "5f0c2a4b7d9e1f3a5c7e9b1d3f5a7c9e1b3d5f7a";

/// A memory that holds the Subject Page of the forgotten item and a
/// note that stays, as an arrival and a conversation write them, with
/// the Channel of the conversation.
struct PageOfTheItem {
    key: SourceKey,
    agent: AgentId,
    /// The exposure of the Agent's grant of the Connection. The page of
    /// the item carries it.
    exposure: MemoryExposure,
    memory: pagis_memory::GitMemoryStore,
    /// The commit that wrote the note that stays.
    kept_sha: String,
    /// The commit that wrote the page of the item.
    forgotten_sha: String,
    channel: pagis_core::ChannelId,
    /// The directory of the memory, held so it outlives the store.
    _root: tempfile::TempDir,
}

impl PageOfTheItem {
    /// The access of the Agent while it holds its grant.
    fn reader(&self) -> MemoryAccess {
        MemoryAccess::agent(self.agent.clone(), [self.exposure.clone()])
    }
}

/// Acquire the forgotten item and the item that stays, then write the
/// note that stays and after it the Subject Page of the forgotten item.
async fn memory_with_the_page_of_the_item(backend: &Backend) -> PageOfTheItem {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let grant = backend
        .stores()
        .grants
        .live_for_resource(
            &key.workspace_id,
            &agent,
            Grant::CONNECTION_KIND,
            key.connection_id.as_str(),
        )
        .await
        .unwrap()
        .expect("the Agent holds a grant of the Connection");
    let exposure = MemoryExposure {
        grant_id: grant.id,
        revision: grant.revision,
    };
    let root = tempfile::tempdir().expect("a temp directory for the memory");
    let memory = pagis_memory::GitMemoryStore::new(
        root.path(),
        Arc::clone(&backend.stores().forget),
        Arc::clone(&backend.stores().memory_pages),
    );
    // A conversation wrote a note with no source in it.
    let kept_sha = write_page(
        &memory,
        &key.workspace_id,
        &agent,
        &MemoryAccess::agent(agent.clone(), []),
        KEPT_NOTE,
        "---\ntitle: Kept\n---\n\nThe lanternfish note stays.\n",
    )
    .await;
    // The arrival of the item wrote the Subject Page of its thread.
    let page = SubjectPage {
        front_matter: pagis_core::subject_page::FrontMatter {
            title: Some(forgotten_title()),
            ..Default::default()
        },
        timeline: vec![TimelineEntry {
            source_reference: format!("gmail:g:{FORGOTTEN}:{FORGOTTEN}-v1"),
            source_time: 21,
            words: format!("The {FORGOTTEN_WORD} invoice is due on Friday."),
        }],
        ..Default::default()
    }
    .render();
    let forgotten_sha = write_page(
        &memory,
        &key.workspace_id,
        &agent,
        &MemoryAccess::agent(agent.clone(), [exposure.clone()]),
        &forgotten_page(),
        &page,
    )
    .await;
    let channel = crate::fixture::channel(&key.workspace_id);
    backend.stores().channels.create(&channel).await.unwrap();
    PageOfTheItem {
        key,
        agent,
        exposure,
        memory,
        kept_sha,
        forgotten_sha,
        channel: channel.id,
        _root: root,
    }
}

/// Append one event of the Workspace, as the daemon publishes it.
async fn publish(
    backend: &Backend,
    page: &PageOfTheItem,
    event_type: &str,
    agent: Option<&AgentId>,
    payload: serde_json::Value,
) {
    backend
        .stores()
        .events
        .append(pagis_core::NewEvent {
            workspace_id: page.key.workspace_id.clone(),
            event_type: event_type.into(),
            agent_id: agent.cloned(),
            run_id: None,
            channel_id: None,
            payload,
        })
        .await
        .unwrap();
}

/// The Learning Feed entry of the commit that wrote the note that
/// stays, as a conversation Run publishes it.
fn kept_commit(page: &PageOfTheItem) -> serde_json::Value {
    serde_json::json!({
        "sha": page.kept_sha,
        "scopes": ["shared"],
        "files": [KEPT_NOTE],
        "titles": ["Kept"],
        "message": "Noted that the lanternfish note stays.",
        "exposures": [],
        "run_id": null,
        "source_scoped": false,
        "phase": "foreground",
    })
}

/// The Learning Feed entry of the commit that wrote the page of the
/// item, as a Reflection publishes it. The path, the title and the
/// sentence hold [`FORGOTTEN`].
fn forgotten_commit(page: &PageOfTheItem) -> serde_json::Value {
    serde_json::json!({
        "sha": page.forgotten_sha,
        "scopes": ["private"],
        "files": [forgotten_page()],
        "titles": [forgotten_title()],
        "message": format!("Learned that the {FORGOTTEN} invoice is due on Friday."),
        "exposures": [page.exposure],
        "run_id": null,
        "source_scoped": true,
        "phase": "reflection",
    })
}

/// The cursor of a Brief that showed both pages in the conversation.
async fn show_both_pages(backend: &Backend, page: &PageOfTheItem) {
    backend
        .stores()
        .briefs
        .save(
            &page.key.workspace_id,
            &page.agent,
            &page.channel,
            None,
            &pagis_core::BriefCursor {
                initialized: true,
                memory_revision: Some(page.forgotten_sha.clone()),
                shown_paths: vec![forgotten_page(), KEPT_NOTE.to_string()],
            },
        )
        .await
        .unwrap();
}

/// Block the forgotten item and run the structured purge, as the
/// daemon does. The answer is the id of the operation.
async fn forget_the_item_of_the_page(backend: &Backend, page: &PageOfTheItem) -> String {
    let workspace = &page.key.workspace_id;
    let target = ForgetTarget::Source {
        source: page.key.clone(),
        source_id: FORGOTTEN.into(),
    };
    let preview = page
        .memory
        .preview_forget(workspace, &target)
        .await
        .unwrap();
    let operation = page
        .memory
        .begin_forget(workspace, &target, &preview, 30, backend.keys())
        .await
        .unwrap();
    backend
        .stores()
        .forget
        .purge_structured(workspace, &operation.id)
        .await
        .unwrap();
    operation.id
}

/// Run the memory purge of an operation and the two steps that end the
/// Forget, in the order of the daemon.
async fn purge_the_memory(backend: &Backend, page: &PageOfTheItem, operation: &str) {
    let workspace = &page.key.workspace_id;
    page.memory
        .purge_forgotten(workspace, operation)
        .await
        .unwrap();
    let forget = &backend.stores().forget;
    forget.memory_purged(workspace, operation).await.unwrap();
    forget.complete(workspace, operation).await.unwrap();
}

/// A Forget leaves no path or title of the purged Subject Page in the
/// Learning Feed or in a Brief cursor. The fixture writes what the
/// product writes about one page: the Learning Feed entry of the commit
/// that wrote it, a revert of that commit, a Schedule on the page with
/// its entry, and the cursor of a Brief that showed it. After the full
/// Forget no value of the database holds the path, the title or the
/// sentence, and the entry and the cursor keep the note that stays
/// (ADR-0008).
pub async fn a_forget_leaves_no_path_or_title_of_the_purged_page_in_the_feed_or_a_brief(
    backend: &Backend,
) {
    let page = memory_with_the_page_of_the_item(backend).await;
    let workspace = &page.key.workspace_id;
    publish(
        backend,
        &page,
        "memory.committed",
        Some(&page.agent),
        kept_commit(&page),
    )
    .await;
    publish(
        backend,
        &page,
        "memory.committed",
        Some(&page.agent),
        forgotten_commit(&page),
    )
    .await;
    // The owner reverted the commit. The revert names the files of the
    // inverse commit.
    publish(
        backend,
        &page,
        "memory.reverted",
        None,
        serde_json::json!({
            "sha": REVERT_SHA,
            "reverted_sha": page.forgotten_sha,
            "scopes": ["private"],
            "files": [forgotten_page()],
            "titles": [pagis_core::memory_page::page_title(&forgotten_page(), None)],
            "scope_agent_id": page.agent,
        }),
    )
    .await;
    // Reflection set a Schedule on the page.
    let schedule = pagis_core::Schedule {
        id: pagis_core::ScheduleId::generate(),
        workspace_id: workspace.clone(),
        agent_id: page.agent.clone(),
        name: format!("Pay the {FORGOTTEN} invoice"),
        instruction: format!("Check that the {FORGOTTEN} invoice is paid."),
        subject_page_path: Some(forgotten_page()),
        channel_id: page.channel.clone(),
        root_message_id: None,
        kind: pagis_core::ScheduleKind::OneShot,
        cron_expression: None,
        interval_ms: None,
        anchor_at: None,
        timezone: "UTC".into(),
        scheduled_at: 60_000,
        next_due_at: Some(60_000),
        last_result: None,
        state: pagis_core::ScheduleState::Active,
        revision: 1,
        approved_revision: None,
        creator: pagis_core::CreatorKind::Agent,
        creating_run_id: None,
        created_at: 15,
        updated_at: 15,
        archived_at: None,
    };
    backend.stores().schedules.create(&schedule).await.unwrap();
    publish(
        backend,
        &page,
        "schedule.created",
        Some(&page.agent),
        serde_json::json!({
            "schedule_id": schedule.id.as_str(),
            "state": "active",
            "next_due_at": 60_000,
            "creator": "agent",
            "name": schedule.name,
            "instruction": schedule.instruction,
            "subject_page_path": forgotten_page(),
            "run_id": null,
        }),
    )
    .await;
    show_both_pages(backend, &page).await;
    assert!(
        !holding(backend, FORGOTTEN).await.is_empty(),
        "the fixture wrote the page of the item"
    );

    let operation = forget_the_item_of_the_page(backend, &page).await;
    purge_the_memory(backend, &page, &operation).await;

    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "a value of the database holds the path, the title or the sentence of the purged page"
    );
    let commits = backend
        .stores()
        .events
        .list_by_types(workspace, &["memory.committed"], None, 10)
        .await
        .unwrap();
    let kept = commits
        .iter()
        .find(|event| event.payload["sha"] == page.kept_sha.as_str())
        .expect("the entry of the note that stays");
    assert_eq!(
        kept.payload,
        kept_commit(&page),
        "the purge changed the entry of the note that stays"
    );
    let cursor = backend
        .stores()
        .briefs
        .load(workspace, &page.agent, &page.channel, None)
        .await
        .unwrap();
    assert_eq!(
        cursor.shown_paths,
        vec![KEPT_NOTE.to_string()],
        "the cursor does not keep exactly the note that stays"
    );
}

/// The tables that a Forget changes after its memory purge.
const MEMORY_PURGE_TABLES: [&str; 5] = [
    "events",
    "memory_brief_cursors",
    "memory_page_index",
    "memory_page_search",
    "memory_page_link",
];

/// The file that holds each table of [`MEMORY_PURGE_TABLES`] on
/// Postgres. `VACUUM FULL` writes a table to a new file.
async fn table_files(backend: &Backend) -> Vec<String> {
    let mut files = Vec::new();
    for table in MEMORY_PURGE_TABLES {
        files.push(
            backend
                .rows()
                .text(
                    &format!("SELECT pg_relation_filenode('{table}')::text"),
                    &[],
                )
                .await
                .expect("read the file of a table")
                .expect("the table has a file"),
        );
    }
    files
}

/// After a Forget completes, the files of the database hold no copy of
/// the purged page. The Run that committed the page publishes the
/// Learning Feed entry of the commit after the commit returns, so the
/// entry can land after the structured purge and its compaction. The
/// fixture appends it there, and SQLite writes it to the WAL.
///
/// The property differs by engine. On SQLite the test reads the
/// database file and its WAL, as an attacker who copies the disk does.
/// Postgres keeps its files in its own server, where the test cannot
/// read them. There `VACUUM FULL` writes each table to a new file and
/// unlinks the old one, so a table with a new file holds no dead copy
/// of a purged row (ADR-0008).
pub async fn a_forget_leaves_no_copy_of_the_purged_page_in_the_database_files(backend: &Backend) {
    let page = memory_with_the_page_of_the_item(backend).await;
    let workspace = &page.key.workspace_id;
    assert_eq!(
        page.memory
            .search_pages(
                workspace,
                &page.reader(),
                MemoryScope::Private,
                FORGOTTEN_WORD,
                10
            )
            .await
            .unwrap()
            .len(),
        1,
        "Memory Search holds the text of the page before the Forget"
    );
    show_both_pages(backend, &page).await;
    let before = if backend.is_postgres() {
        table_files(backend).await
    } else {
        Vec::new()
    };

    let operation = forget_the_item_of_the_page(backend, &page).await;
    publish(
        backend,
        &page,
        "memory.committed",
        Some(&page.agent),
        forgotten_commit(&page),
    )
    .await;
    purge_the_memory(backend, &page, &operation).await;

    if backend.is_postgres() {
        let after = table_files(backend).await;
        for ((table, before), after) in MEMORY_PURGE_TABLES.iter().zip(&before).zip(&after) {
            assert_ne!(
                before, after,
                "{table} keeps the file that holds the rows the Forget purged"
            );
        }
        return;
    }
    let mut copies = Vec::new();
    for file in backend.database_files() {
        // The WAL is absent when nothing wrote to it.
        let bytes = std::fs::read(&file).unwrap_or_default();
        for mark in [FORGOTTEN_WORD, FORGOTTEN] {
            if bytes
                .windows(mark.len())
                .any(|window| window == mark.as_bytes())
            {
                copies.push(format!("{} holds {mark}", file.display()));
            }
        }
    }
    assert_eq!(
        copies,
        Vec::<String>::new(),
        "a file of the database holds the purged page after the Forget"
    );
}

/// The question of the Person that starts each conversation Run of a
/// fixture. It holds no word of a source item.
const QUESTION: &str = "What does the mail say?";

/// One reply turn of a conversation that read synced source content
/// and answered with it: the question of the Person, the progress line
/// of the Run, the tool result that the Run kept, and the reply of the
/// Run.
struct QuotingRun {
    scope: pagis_core::ConversationScope,
    question: pagis_core::Message,
    progress: pagis_core::Message,
    reply: pagis_core::Message,
    evidence: String,
}

/// The exposure of the Grant that the Agent holds on the Connection of
/// `key`. The daemon stamps each message that the Agent writes with it
/// (ADR-0004).
async fn connection_exposure(
    backend: &Backend,
    key: &SourceKey,
    agent: &AgentId,
) -> MemoryExposure {
    let grant = backend
        .stores()
        .grants
        .live_for_resource(
            &key.workspace_id,
            agent,
            Grant::CONNECTION_KIND,
            key.connection_id.as_str(),
        )
        .await
        .unwrap()
        .expect("the Agent holds a grant of the Connection");
    MemoryExposure {
        grant_id: grant.id,
        revision: grant.revision,
    }
}

/// Write the rows of one reply turn in a Channel of its own: the Run
/// read `reads` through the Connection of `key`, kept the tool result
/// `words`, and answered with a reply that quotes `words`. These are the
/// rows that the daemon writes for a turn that calls `mail__get_message`
/// (ADR-0009).
async fn quoting_run(
    backend: &Backend,
    key: &SourceKey,
    agent: &AgentId,
    reads: &[SourceRead],
    words: &str,
) -> QuotingRun {
    let stores = backend.stores();
    let channel = crate::fixture::channel(&key.workspace_id);
    stores.channels.create(&channel).await.unwrap();
    let question = crate::fixture::user_message(&key.workspace_id, &channel.id, QUESTION);
    stores.messages.insert(&question).await.unwrap();
    let run = pagis_core::Run {
        trigger_ref: Some(question.id.to_string()),
        ..crate::fixture::queued_run(&key.workspace_id, agent, &channel.id)
    };
    stores.runs.create(&run).await.unwrap();
    // The progress line holds fixed text of the daemon, so its stamp
    // names no Grant.
    let progress = pagis_core::Message {
        run_id: Some(run.id.clone()),
        blocks: vec![pagis_core::Block::progress(run.id.as_str(), "Working")],
        text_content: "Working".into(),
        ..crate::fixture::agent_message(&key.workspace_id, &channel.id, agent, "Working")
    };
    stores
        .messages
        .insert_stamped(&progress, &[])
        .await
        .unwrap();
    stores
        .runs
        .record_source_reads(&key.workspace_id, &run.id, &key.connection_id, reads)
        .await
        .unwrap();
    let exposure = connection_exposure(backend, key, agent).await;
    let scope = pagis_core::ConversationScope {
        workspace_id: key.workspace_id.clone(),
        agent_id: agent.clone(),
        channel_id: channel.id.clone(),
        root_message_id: None,
    };
    let evidence = stores
        .conversation_evidence
        .retain_tool(&pagis_core::RetainedToolEvidence {
            scope: scope.clone(),
            source_message_id: question.id.clone(),
            run_id: run.id.clone(),
            tool_call_id: "call-1".into(),
            tool_name: pagis_broker::MAIL_GET_MESSAGE.into(),
            content: words.into(),
            complete: true,
            exposure: exposure.clone(),
            created_at: 16,
        })
        .await
        .unwrap();
    let reply = pagis_core::Message {
        run_id: Some(run.id.clone()),
        ..crate::fixture::agent_message(&key.workspace_id, &channel.id, agent, words)
    };
    stores
        .messages
        .insert_stamped(&reply, &[exposure])
        .await
        .unwrap();
    QuotingRun {
        scope,
        question,
        progress,
        reply,
        evidence,
    }
}

/// Whether the Person reads the words of `message` now. A message that
/// the Person cannot read shows that it is unavailable (ADR-0004).
async fn readable(backend: &Backend, message: &pagis_core::Message) -> bool {
    let stores = backend.stores();
    let stored = stores
        .messages
        .get(&message.workspace_id, &message.id)
        .await
        .unwrap()
        .expect("the message row stays");
    pagis_core::message_source_is_live(stores.messages.as_ref(), stores.grants.as_ref(), &stored)
        .await
        .unwrap()
}

/// One Run read the forgotten item, kept the tool result that returned
/// it, and answered with its words. A second Run read the thread of
/// the item and did the same. After a Forget of the item and its
/// purge, each reply is forgotten as a whole and the Person reads that
/// it is unavailable. The tool results are gone. The question of the
/// Person and the progress line of each Run are prose of no source, and
/// they stay. No value of any table holds the text of the item: not a
/// message, not a row of the conversation search and not the record of
/// what the Runs read (ADR-0008, ADR-0009).
pub async fn a_forget_leaves_no_text_of_the_item_in_the_conversations_that_quoted_it(
    backend: &Backend,
) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let words = format!("The {FORGOTTEN_WORD} invoice of {FORGOTTEN} is due on Friday.");
    let of_item = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Item {
            resource: key.resource.clone(),
            id: FORGOTTEN.into(),
        }],
        &words,
    )
    .await;
    let of_thread = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Parent {
            resource: key.resource.clone(),
            id: forgotten_thread(),
        }],
        &words,
    )
    .await;
    assert!(
        !holding(backend, FORGOTTEN_WORD).await.is_empty(),
        "the fixture wrote the words of the item"
    );

    forget_the_item(backend, &key).await;

    assert_eq!(
        holding(backend, FORGOTTEN_WORD).await,
        Vec::<String>::new(),
        "a value of the database holds the text of the forgotten item"
    );
    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "a value of the database holds the forgotten item"
    );
    let stores = backend.stores();
    for run in [&of_item, &of_thread] {
        assert!(
            stores
                .messages
                .is_forgotten(&key.workspace_id, &run.reply.id)
                .await
                .unwrap(),
            "the reply that quoted the item is not forgotten"
        );
        assert!(
            !readable(backend, &run.reply).await,
            "the Person reads the reply that quoted the forgotten item"
        );
        assert!(
            stores
                .conversation_evidence
                .read_tool(&run.scope, &run.evidence)
                .await
                .unwrap()
                .is_none(),
            "the tool result that returned the item stays"
        );
        // The question of the Person and the progress line of the Run
        // hold no word of the item, and they stay as they are.
        for (message, text) in [(&run.question, QUESTION), (&run.progress, "Working")] {
            assert!(
                !stores
                    .messages
                    .is_forgotten(&key.workspace_id, &message.id)
                    .await
                    .unwrap(),
                "a message of no source is forgotten"
            );
            assert!(readable(backend, message).await);
            let stored = stores
                .messages
                .get(&key.workspace_id, &message.id)
                .await
                .unwrap()
                .expect("the message stays");
            assert_eq!(stored.text_content, text);
        }
    }
}

/// A Forget of one item forgets the messages of the Runs that read it
/// and no other message. A Run that read only another item, or only
/// the thread of another item, keeps its reply, its tool result and
/// their rows in the conversation search (ADR-0008).
pub async fn a_forget_of_one_item_keeps_the_messages_that_quoted_only_other_items(
    backend: &Backend,
) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let forgotten = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Item {
            resource: key.resource.clone(),
            id: FORGOTTEN.into(),
        }],
        &format!("The {FORGOTTEN_WORD} invoice is due on Friday."),
    )
    .await;
    let kept_words = "The lanternfish order ships on Monday.";
    let of_item = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Item {
            resource: key.resource.clone(),
            id: "kept-1".into(),
        }],
        kept_words,
    )
    .await;
    let of_thread = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Parent {
            resource: key.resource.clone(),
            id: "thread-kept".into(),
        }],
        kept_words,
    )
    .await;

    forget_the_item(backend, &key).await;

    let stores = backend.stores();
    assert!(
        stores
            .messages
            .is_forgotten(&key.workspace_id, &forgotten.reply.id)
            .await
            .unwrap(),
        "the reply that quoted the forgotten item is not forgotten"
    );
    for run in [&of_item, &of_thread] {
        assert!(
            !stores
                .messages
                .is_forgotten(&key.workspace_id, &run.reply.id)
                .await
                .unwrap(),
            "a reply that quoted only another item is forgotten"
        );
        assert!(
            readable(backend, &run.reply).await,
            "the Person cannot read a reply that quoted only another item"
        );
        let reply = stores
            .messages
            .get(&key.workspace_id, &run.reply.id)
            .await
            .unwrap()
            .expect("the reply stays");
        assert_eq!(reply.text_content, kept_words);
        let evidence = stores
            .conversation_evidence
            .read_tool(&run.scope, &run.evidence)
            .await
            .unwrap()
            .expect("the tool result of another item stays");
        assert_eq!(evidence.content, kept_words);
        let mut kinds: Vec<String> = stores
            .conversation_evidence
            .search(&run.scope, "lanternfish", None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|hit| hit.kind)
            .collect();
        kinds.sort();
        assert_eq!(
            kinds,
            ["message", "tool"],
            "the conversation search lost the reply or the tool result of another item"
        );
    }
}

/// A Forget of a whole account forgets each reply that a Grant of its
/// Connection stamped, and deletes each tool result that such a Grant
/// read and each record of a read through the Connection. The Run here
/// read a message that the Sync does not hold, such as mail older than
/// the window of the Sync. No value of the database then holds the
/// words of the account (ADR-0008).
pub async fn an_account_forget_deletes_the_tool_results_and_the_reads_of_its_connection(
    backend: &Backend,
) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(store.as_ref(), backend.keys(), &key, vec![kept()]).await;
    let run = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Item {
            resource: key.resource.clone(),
            id: FORGOTTEN.into(),
        }],
        &format!("The {FORGOTTEN_WORD} invoice is due on Friday."),
    )
    .await;

    let forget = &backend.stores().forget;
    let target = ForgetTarget::Account {
        connection_id: key.connection_id.clone(),
    };
    let preview = forget.preview(&key.workspace_id, &target).await.unwrap();
    let operation = forget
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            30,
            backend.keys(),
        )
        .await
        .unwrap();
    forget
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();

    let stores = backend.stores();
    assert!(
        stores
            .messages
            .is_forgotten(&key.workspace_id, &run.reply.id)
            .await
            .unwrap(),
        "the reply of the account is not forgotten"
    );
    assert!(
        stores
            .conversation_evidence
            .read_tool(&run.scope, &run.evidence)
            .await
            .unwrap()
            .is_none(),
        "the tool result of the account stays"
    );
    assert_eq!(
        backend
            .count(
                "SELECT COUNT(*) FROM run_source_reads WHERE workspace_id=?",
                &[Bind::from(key.workspace_id.as_str())],
            )
            .await
            .expect("count the reads"),
        0,
        "a record of a read through the forgotten account stays"
    );
    assert_eq!(
        holding(backend, FORGOTTEN_WORD).await,
        Vec::<String>::new(),
        "a value of the database holds the words of the forgotten account"
    );
}

/// A Model Request Capture of a Run that read a forgotten source holds
/// what the Run read, so Forget deletes it with the tool results of the
/// Run (ADR-0031).
pub async fn a_forget_deletes_the_model_request_captures_of_a_run_that_read_it(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(store.as_ref(), backend.keys(), &key, vec![kept()]).await;
    let words = format!("The {FORGOTTEN_WORD} invoice is due on Friday.");
    let run = quoting_run(
        backend,
        &key,
        &agent,
        &[SourceRead::Item {
            resource: key.resource.clone(),
            id: FORGOTTEN.into(),
        }],
        &words,
    )
    .await;
    let run_id = run.reply.run_id.clone().expect("the reply names its Run");
    backend
        .stores()
        .model_request_captures
        .record(&pagis_core::ModelRequestCapture {
            id: pagis_core::ModelRequestCaptureId::generate(),
            workspace_id: key.workspace_id.clone(),
            run_id: run_id.clone(),
            phase: "reply".into(),
            phase_request: 1,
            request: serde_json::json!({"messages": [{"role": "tool", "text": words}]}),
            answer: serde_json::json!({"outcome": "completed"}),
            created_at: 17,
        })
        .await
        .unwrap();

    let forget = &backend.stores().forget;
    let target = ForgetTarget::Account {
        connection_id: key.connection_id.clone(),
    };
    let preview = forget.preview(&key.workspace_id, &target).await.unwrap();
    let operation = forget
        .begin(
            &key.workspace_id,
            &target,
            &preview.revision,
            30,
            backend.keys(),
        )
        .await
        .unwrap();
    forget
        .purge_structured(&key.workspace_id, &operation.id)
        .await
        .unwrap();

    assert!(
        backend
            .stores()
            .model_request_captures
            .list_for_run(&key.workspace_id, &run_id)
            .await
            .unwrap()
            .is_empty(),
        "a capture of the Run that read the account stays"
    );
    assert_eq!(
        holding(backend, FORGOTTEN_WORD).await,
        Vec::<String>::new(),
        "a value of the database holds the words of the forgotten account"
    );
}

/// A read that a Run makes while a Forget blocks the Connection has no
/// record, so the broker keeps its result from the model. A call that
/// was in flight when the Forget began then cannot leave the words of
/// the item in the conversation (ADR-0008).
pub async fn a_forget_refuses_the_record_of_a_read_through_its_connection(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let agent = state.config.agent_id.clone();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let channel = crate::fixture::channel(&key.workspace_id);
    backend.stores().channels.create(&channel).await.unwrap();
    let run = crate::fixture::queued_run(&key.workspace_id, &agent, &channel.id);
    backend.stores().runs.create(&run).await.unwrap();

    forget_the_item(backend, &key).await;

    let refused = backend
        .stores()
        .runs
        .record_source_reads(
            &key.workspace_id,
            &run.id,
            &key.connection_id,
            &[SourceRead::Item {
                resource: key.resource.clone(),
                id: FORGOTTEN.into(),
            }],
        )
        .await;
    assert!(
        matches!(refused, Err(pagis_core::StoreError::Conflict(_))),
        "a read through a Connection that a Forget blocks has a record: {refused:?}"
    );
    assert_eq!(
        holding(backend, FORGOTTEN).await,
        Vec::<String>::new(),
        "a value of the database holds the forgotten item"
    );
}

/// Deliver the arrival of message `id` to the Event Subscription, start
/// the Run that its Wake-up makes, and write the reply of that Run with
/// `words`. The reply is what the Agent answers after it reads the
/// event briefing (ADR-0006).
async fn woken_reply(
    backend: &Backend,
    key: &SourceKey,
    subscription: &pagis_core::EventSubscription,
    id: &str,
    thread: &str,
    words: &str,
) -> pagis_core::Message {
    let stores = backend.stores();
    stores
        .triggers
        .ingest(
            pagis_core::IngestBatch {
                workspace_id: key.workspace_id.clone(),
                source: pagis_core::EventSource::connection(key.connection_id.clone()),
                agent_id: None,
                event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
                cursor: None,
                events: vec![arrival_of(id, thread)],
                received_at: 14,
                baseline: false,
            },
            std::slice::from_ref(subscription),
            &EveryEvent,
            None,
            Some(pagis_broker::MAIL_SYNC_RESOURCE),
            backend.keys(),
        )
        .await
        .unwrap();
    let claims = stores
        .triggers
        .claim_wakeups(
            &key.workspace_id,
            &subscription.agent_id,
            pagis_core::RunSlots {
                conversation: 1,
                arrival: 0,
            },
            15,
        )
        .await
        .unwrap();
    assert_eq!(claims.len(), 1, "the arrival of {id} woke no Run");
    let reply = pagis_core::Message {
        run_id: Some(claims[0].run.id.clone()),
        ..crate::fixture::agent_message(
            &key.workspace_id,
            &subscription.channel_id,
            &subscription.agent_id,
            words,
        )
    };
    let exposure = connection_exposure(backend, key, &subscription.agent_id).await;
    stores
        .messages
        .insert_stamped(&reply, &[exposure])
        .await
        .unwrap();
    // The Run ends with its reply. A rule has at most one active Run, so
    // the next arrival wakes a Run only after this one (ADR-0006).
    let mut run = claims[0].run.clone();
    run.state = pagis_core::RunState::Completed;
    run.ended_at = Some(16);
    stores.runs.update(&run).await.unwrap();
    reply
}

/// The Incoming Event of the forgotten item woke a Run through an Event
/// Subscription, and the Run answered with the words of the item. The
/// Wake-up names the Incoming Events that the Run read, so a Forget of
/// the item forgets that reply. The reply of the Run that another item
/// woke stays (ADR-0008, ADR-0006).
pub async fn a_forget_forgets_the_reply_of_the_run_that_the_item_woke(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![forgotten_item("1", true), kept()],
    )
    .await;
    let channel = crate::fixture::channel(&key.workspace_id);
    backend.stores().channels.create(&channel).await.unwrap();
    let subscription = pagis_core::EventSubscription {
        id: pagis_core::EventSubscriptionId::generate(),
        workspace_id: key.workspace_id.clone(),
        agent_id: state.config.agent_id.clone(),
        source: pagis_core::EventSource::connection(key.connection_id.clone()),
        event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
        source_version: "v1".into(),
        name: "New mail".into(),
        instruction: "Tell me about new mail.".into(),
        channel_id: channel.id.clone(),
        root_message_id: None,
        filter: serde_json::json!({}),
        creator: pagis_core::CreatorKind::User,
        state: pagis_core::EventSubscriptionState::Active,
        revision: 1,
        approved_revision: Some(1),
        watermark_at: None,
        blocked_reason: None,
        created_at: 13,
        updated_at: 13,
        archived_at: None,
    };
    backend
        .stores()
        .subscriptions
        .create(&subscription)
        .await
        .unwrap();
    let forgotten = woken_reply(
        backend,
        &key,
        &subscription,
        FORGOTTEN,
        &forgotten_thread(),
        &format!("The {FORGOTTEN_WORD} invoice is due on Friday."),
    )
    .await;
    let kept = woken_reply(
        backend,
        &key,
        &subscription,
        "kept-1",
        "thread-kept",
        "The lanternfish order ships on Monday.",
    )
    .await;

    forget_the_item(backend, &key).await;

    let messages = &backend.stores().messages;
    assert!(
        messages
            .is_forgotten(&key.workspace_id, &forgotten.id)
            .await
            .unwrap(),
        "the reply of the Run that the forgotten item woke is not forgotten"
    );
    assert!(
        !messages
            .is_forgotten(&key.workspace_id, &kept.id)
            .await
            .unwrap(),
        "the reply of the Run that another item woke is forgotten"
    );
    assert_eq!(
        holding(backend, FORGOTTEN_WORD).await,
        Vec::<String>::new(),
        "a value of the database holds the text of the forgotten item"
    );
}

/// One acquisition batch that carries a live arrival for `id`.
fn arrival_batch(state: &SyncStatus, id: &str) -> SourceBatch {
    SourceBatch {
        historical: false,
        arrivals: vec![pagis_core::NormalizedEvent {
            provider_event_id: id.into(),
            metadata: serde_json::json!({"thread_id": format!("thread-{id}")}),
            occurred_at: 10,
            landing: None,
        }],
        expected_revision: state.cursor_revision,
        checkpoint: serde_json::json!({"page":"first"}),
        caught_up: true,
        changes: vec![source(id, 10)],
    }
}

/// One arrival that fails keeps its place in the queue, but it waits for its
/// own retry time instead of holding the pass back.
pub async fn a_failed_arrival_waits_for_its_retry_time(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .acquire(&key, &arrival_batch(&state, "big"), 11, backend.keys())
        .await
        .unwrap();
    let due = store.arrivals(&key, 10, 11).await.unwrap();
    assert_eq!(due.len(), 1);

    store
        .fail_arrival(&key, due[0].sequence, false, "the source is busy", 11)
        .await
        .unwrap();

    assert!(
        store.arrivals(&key, 10, 11).await.unwrap().is_empty(),
        "the failed arrival waits"
    );
    assert_eq!(
        store.arrivals(&key, 10, 11 + 120_000).await.unwrap().len(),
        1,
        "the first failure holds the arrival for two minutes"
    );
    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.arrival_pending, 1);
    assert_eq!(status.arrival_skipped, 0);
}

/// Five failures stop an arrival for good. The count tells the owner that a
/// message never reached its Subject Page.
pub async fn the_fifth_failure_skips_the_arrival(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .acquire(&key, &arrival_batch(&state, "big"), 11, backend.keys())
        .await
        .unwrap();
    let sequence = store.arrivals(&key, 10, 11).await.unwrap()[0].sequence;
    for attempt in 1..=5 {
        store
            .fail_arrival(&key, sequence, false, "the source is busy", 11)
            .await
            .unwrap();
        let skipped = store
            .status(&key, pagis_core::now_ms())
            .await
            .unwrap()
            .unwrap()
            .arrival_skipped;
        assert_eq!(
            skipped,
            i64::from(attempt == 5),
            "attempt {attempt} skips only on the fifth"
        );
    }
    assert!(
        store
            .arrivals(&key, 10, 11 + 3_600_000)
            .await
            .unwrap()
            .is_empty(),
        "a skipped arrival never comes back"
    );
    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.arrival_pending, 0);
    assert_eq!(status.arrival_processed, 0);
}

/// A rejected item cannot succeed on a retry, so the first failure skips it
/// and records why.
pub async fn a_permanent_failure_skips_the_arrival_at_once(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .acquire(&key, &arrival_batch(&state, "big"), 11, backend.keys())
        .await
        .unwrap();
    let sequence = store.arrivals(&key, 10, 11).await.unwrap()[0].sequence;

    store
        .fail_arrival(&key, sequence, true, "the thread is too large", 12)
        .await
        .unwrap();

    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.arrival_skipped, 1);
    assert_eq!(status.arrival_pending, 0);
    let failure = backend
        .rows()
        .text(
            "SELECT arrival_failure FROM source_versions WHERE sequence = ?",
            &[Bind::from(sequence)],
        )
        .await
        .expect("read the arrival failure");
    assert_eq!(failure.as_deref(), Some("the thread is too large"));
}

/// An appended arrival is counted apart from a skipped one.
pub async fn an_acknowledged_arrival_counts_as_appended(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .acquire(&key, &arrival_batch(&state, "note"), 11, backend.keys())
        .await
        .unwrap();
    let sequence = store.arrivals(&key, 10, 11).await.unwrap()[0].sequence;

    store.acknowledge_arrival(&key, sequence, 12).await.unwrap();

    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.arrival_processed, 1);
    assert_eq!(status.arrival_skipped, 0);
    assert_eq!(status.arrival_pending, 0);
}

/// One acquired historical arrival of `thread`.
fn historical_mail(id: &str, thread: &str, at: i64) -> (SourceChange, pagis_core::NormalizedEvent) {
    (
        mail(id, thread, at, "clinic.test"),
        pagis_core::NormalizedEvent {
            provider_event_id: id.into(),
            metadata: serde_json::json!({"thread_id": thread}),
            occurred_at: at,
            landing: None,
        },
    )
}

async fn acquire_history(
    store: &dyn KnowledgeStore,
    keys: &dyn ForgetKeys,
    key: &SourceKey,
    mail: Vec<(SourceChange, pagis_core::NormalizedEvent)>,
) {
    let revision = store
        .status(key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap()
        .cursor_revision;
    let (changes, arrivals) = mail.into_iter().unzip();
    store
        .acquire(
            key,
            &SourceBatch {
                historical: true,
                arrivals,
                expected_revision: revision,
                checkpoint: serde_json::json!({"page": 1}),
                caught_up: true,
                changes,
            },
            100,
            keys,
        )
        .await
        .unwrap();
}

/// One stored arrival Wake-up, the way the Trigger module writes it.
async fn arrival_wakeup(backend: &Backend, state: &SyncStatus) -> pagis_core::WakeupId {
    let id = pagis_core::WakeupId::generate();
    backend
        .execute(
            "INSERT INTO wakeups(id, workspace_id, source_kind, rule_revision, rule_name, agent_id, \
             instruction, scheduled_at, state, source_count, created_at, subject_paths, historical) \
             VALUES (?, ?, 'arrival', 1, 'Synced arrival', ?, 'Reflect.', 0, 'pending', 1, 0, '[]', ?)",
            &[
                Bind::from(id.as_str()),
                Bind::from(state.config.workspace_id.as_str()),
                Bind::from(state.config.agent_id.as_str()),
                // `historical` is BOOLEAN in the Postgres baseline, so
                // the value is bound, not written as 1.
                Bind::Bool(true),
            ],
        )
        .await
        .expect("write the arrival Wake-up");
    id
}

fn backfill_decision(subject: &str, verdict: Verdict, at: i64) -> PageReflection {
    PageReflection {
        page_path: format!("private/subjects/gmail/{subject}.md"),
        page_subject: subject.into(),
        filter_revision: 1,
        decided_by: ReflectionPass::Backfill,
        selection: selection(verdict, None),
        decided_at: at,
    }
}

pub async fn the_backfill_offers_undecided_historical_pages_newest_first(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire_history(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            historical_mail("m1", "clinic", 10),
            historical_mail("m2", "clinic", 40),
            historical_mail("m3", "shop", 30),
            historical_mail("m4", "school", 20),
        ],
    )
    .await;

    let candidates = store.backfill_candidates(&key, 1, 10).await.unwrap();
    assert_eq!(
        candidates,
        vec![
            BackfillPage {
                subject: "clinic".into(),
                newest_at: 40
            },
            BackfillPage {
                subject: "shop".into(),
                newest_at: 30
            },
            BackfillPage {
                subject: "school".into(),
                newest_at: 20
            },
        ]
    );

    // A decided page leaves the walk, whatever the verdict was.
    store
        .record_reflection(&key, &backfill_decision("clinic", Verdict::Reflect, 50))
        .await
        .unwrap();
    store
        .record_reflection(&key, &backfill_decision("shop", Verdict::Skip, 50))
        .await
        .unwrap();
    assert_eq!(
        store.backfill_candidates(&key, 1, 10).await.unwrap(),
        vec![BackfillPage {
            subject: "school".into(),
            newest_at: 20
        }]
    );
    // A new filter revision decides about every page again.
    assert_eq!(
        store.backfill_candidates(&key, 2, 10).await.unwrap().len(),
        3
    );
}

pub async fn a_recorded_run_moves_a_page_from_waiting_to_reflected(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .record_reflection(&key, &backfill_decision("clinic", Verdict::Reflect, 50))
        .await
        .unwrap();
    store
        .record_reflection(&key, &backfill_decision("school", Verdict::Reflect, 60))
        .await
        .unwrap();
    store
        .record_reflection(&key, &backfill_decision("shop", Verdict::Skip, 70))
        .await
        .unwrap();

    let batch = store.backfill_batch(&key, 1, 20).await.unwrap();
    assert_eq!(
        batch,
        vec![
            "private/subjects/gmail/clinic.md".to_string(),
            "private/subjects/gmail/school.md".to_string(),
        ],
        "a skipped page never waits for a Run"
    );
    let counted = store.status(&key, 100).await.unwrap().unwrap();
    assert_eq!(counted.backfill_pending, 2);
    assert_eq!(counted.backfill_reflected, 0);

    let wakeup = arrival_wakeup(backend, &state).await;
    store
        .record_backfill_run(&key, 1, &batch, &wakeup, 100)
        .await
        .unwrap();
    assert!(store.backfill_batch(&key, 1, 20).await.unwrap().is_empty());
    let counted = store.status(&key, 100).await.unwrap().unwrap();
    assert_eq!(counted.backfill_pending, 0);
    assert_eq!(counted.backfill_reflected, 2);
    assert!(!counted.backfill_capped);
}

pub async fn the_budget_counts_the_backfill_runs_of_the_window(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    let at = 10_000_000_000;
    for index in 0..BACKFILL_RUNS_PER_WINDOW {
        let subject = format!("thread-{index}");
        store
            .record_reflection(&key, &backfill_decision(&subject, Verdict::Reflect, at))
            .await
            .unwrap();
        store
            .record_backfill_run(
                &key,
                1,
                &[format!("private/subjects/gmail/{subject}.md")],
                &arrival_wakeup(backend, &state).await,
                at,
            )
            .await
            .unwrap();
    }
    assert!(
        store
            .status(&key, at)
            .await
            .unwrap()
            .unwrap()
            .backfill_capped,
        "the last Run of the window spends the budget"
    );
    assert!(
        !store
            .status(&key, at + BACKFILL_WINDOW_MS)
            .await
            .unwrap()
            .unwrap()
            .backfill_capped,
        "the window moves on and the budget frees"
    );
}

/// A historical message whose version holds no metadata (ADR-0011).
fn bare_historical_mail(
    id: &str,
    thread: &str,
    at: i64,
) -> (SourceChange, pagis_core::NormalizedEvent) {
    let (mut change, arrival) = historical_mail(id, thread, at);
    change.metadata = serde_json::Value::Null;
    (change, arrival)
}

pub async fn a_version_without_metadata_waits_for_its_metadata_before_the_backfill(
    backend: &Backend,
) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire_history(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            bare_historical_mail("m1", "clinic", 40),
            historical_mail("m2", "shop", 30),
        ],
    )
    .await;
    // A deletion has no metadata to complete.
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![removed("m3", "2")],
    )
    .await;

    let incomplete = store.versions_without_metadata(&key, 10).await.unwrap();
    assert_eq!(incomplete.len(), 1, "only the upsert waits for metadata");
    assert_eq!(incomplete[0].source.id, "m1");
    assert_eq!(
        store
            .status(&key, 100)
            .await
            .unwrap()
            .unwrap()
            .incomplete_versions,
        1
    );
    assert_eq!(
        store.backfill_candidates(&key, 1, 10).await.unwrap(),
        vec![BackfillPage {
            subject: "shop".into(),
            newest_at: 30
        }],
        "a page without metadata is not a candidate"
    );

    let metadata = serde_json::json!({"message_id": "m1", "received_at": 40, "labels": ["INBOX"]});
    store
        .complete_metadata(&key, incomplete[0].sequence, &metadata)
        .await
        .unwrap();
    assert!(
        store
            .versions_without_metadata(&key, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .status(&key, 100)
            .await
            .unwrap()
            .unwrap()
            .incomplete_versions,
        0
    );
    assert_eq!(
        store.parent_metadata(&key, "clinic").await.unwrap(),
        vec![metadata]
    );
    assert_eq!(
        store.backfill_candidates(&key, 1, 10).await.unwrap()[0].subject,
        "clinic"
    );
}

pub async fn completing_metadata_drops_the_pending_decision_of_the_subject(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire_history(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            bare_historical_mail("m1", "clinic", 40),
            bare_historical_mail("m2", "school", 30),
        ],
    )
    .await;
    store
        .record_reflection(&key, &backfill_decision("clinic", Verdict::Reflect, 50))
        .await
        .unwrap();
    store
        .record_reflection(&key, &backfill_decision("school", Verdict::Reflect, 50))
        .await
        .unwrap();
    let wakeup = arrival_wakeup(backend, &state).await;
    store
        .record_backfill_run(
            &key,
            1,
            &["private/subjects/gmail/school.md".to_string()],
            &wakeup,
            60,
        )
        .await
        .unwrap();

    let metadata = serde_json::json!({"message_id": "x", "received_at": 40});
    for version in store.versions_without_metadata(&key, 10).await.unwrap() {
        store
            .complete_metadata(&key, version.sequence, &metadata)
            .await
            .unwrap();
    }
    let counted = store.status(&key, 100).await.unwrap().unwrap();
    assert_eq!(
        counted.backfill_pending, 0,
        "the waiting page decides again"
    );
    assert_eq!(
        counted.backfill_reflected, 1,
        "a reflected page keeps its record"
    );
    assert_eq!(
        store.backfill_candidates(&key, 1, 10).await.unwrap(),
        vec![BackfillPage {
            subject: "clinic".into(),
            newest_at: 40
        }]
    );
}

/// The Run of one backfill Wake-up ended in `state` without a reflection.
async fn end_run(
    backend: &Backend,
    state: &SyncStatus,
    wakeup: &pagis_core::WakeupId,
    run_state: &str,
) {
    let run_id = pagis_core::RunId::generate();
    backend
        .execute(
            "INSERT INTO runs (id, workspace_id, agent_id, trigger_kind, trigger_ref, state, created_at) \
             VALUES (?, ?, ?, 'arrival', ?, ?, 0)",
            &[
                Bind::from(run_id.as_str()),
                Bind::from(state.config.workspace_id.as_str()),
                Bind::from(state.config.agent_id.as_str()),
                Bind::from(wakeup.as_str()),
                Bind::from(run_state),
            ],
        )
        .await
        .expect("write the Run");
    backend
        .execute(
            "UPDATE wakeups SET state = 'started', run_id = ? WHERE id = ?",
            &[Bind::from(run_id.as_str()), Bind::from(wakeup.as_str())],
        )
        .await
        .expect("start the Wake-up");
}

/// A failed Run reflected nothing, so its pages wait for a Run again and
/// the Run leaves the budget (ADR-0011).
pub async fn a_failed_run_returns_its_pages_to_the_batch_and_leaves_the_budget(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    for subject in ["clinic", "school", "shop"] {
        store
            .record_reflection(&key, &backfill_decision(subject, Verdict::Reflect, 50))
            .await
            .unwrap();
    }
    let batch = store.backfill_batch(&key, 1, 2).await.unwrap();
    let failed = arrival_wakeup(backend, &state).await;
    store
        .record_backfill_run(&key, 1, &batch, &failed, 100)
        .await
        .unwrap();
    let running = arrival_wakeup(backend, &state).await;
    store
        .record_backfill_run(
            &key,
            1,
            &["private/subjects/gmail/shop.md".to_string()],
            &running,
            100,
        )
        .await
        .unwrap();
    end_run(backend, &state, &failed, "failed").await;
    end_run(backend, &state, &running, "running").await;

    assert_eq!(store.release_failed_backfill_runs(&key).await.unwrap(), 2);
    assert_eq!(
        store.backfill_batch(&key, 1, 20).await.unwrap(),
        batch,
        "the pages of the failed Run wait again; the running Run keeps its page"
    );
    let counted = store.status(&key, 100).await.unwrap().unwrap();
    assert_eq!(counted.backfill_pending, 2);
    assert_eq!(counted.backfill_reflected, 1);
    assert_eq!(counted.backfill_failed, 0);
    assert_eq!(
        store.release_failed_backfill_runs(&key).await.unwrap(),
        0,
        "a released page is released once"
    );
}

/// A page whose Runs keep failing stops after the attempt limit, so one
/// poisoned batch cannot spend the budget of every day.
pub async fn a_page_stops_waiting_after_the_attempt_limit(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    store
        .record_reflection(&key, &backfill_decision("clinic", Verdict::Reflect, 50))
        .await
        .unwrap();
    for attempt in 0..BACKFILL_ATTEMPTS_PER_PAGE {
        let batch = store.backfill_batch(&key, 1, 20).await.unwrap();
        assert_eq!(batch.len(), 1, "attempt {attempt} finds the page waiting");
        let wakeup = arrival_wakeup(backend, &state).await;
        store
            .record_backfill_run(&key, 1, &batch, &wakeup, 100)
            .await
            .unwrap();
        end_run(backend, &state, &wakeup, "failed").await;
        assert_eq!(store.release_failed_backfill_runs(&key).await.unwrap(), 1);
    }
    assert!(store.backfill_batch(&key, 1, 20).await.unwrap().is_empty());
    let counted = store.status(&key, 100).await.unwrap().unwrap();
    assert_eq!(counted.backfill_pending, 0);
    assert_eq!(counted.backfill_reflected, 0);
    assert_eq!(counted.backfill_failed, 1);
    assert!(!counted.backfill_capped, "failed Runs spend no budget");

    // A new filter revision decides again and the page gets fresh attempts.
    store
        .record_reflection(
            &key,
            &PageReflection {
                filter_revision: 2,
                ..backfill_decision("clinic", Verdict::Reflect, 60)
            },
        )
        .await
        .unwrap();
    assert_eq!(store.backfill_batch(&key, 2, 20).await.unwrap().len(), 1);
}

pub async fn page_signals_read_the_newest_version_of_each_message(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            relabelled("m1", "clinic", 10, "1", &["INBOX", "IMPORTANT"]),
            relabelled("m1", "clinic", 10, "2", &["INBOX"]),
        ],
    )
    .await;
    let thread = store.parent_metadata(&key, "clinic").await.unwrap();
    assert_eq!(
        thread.len(),
        1,
        "one message is one message, not one per version"
    );
    assert_eq!(
        thread[0]["labels"],
        serde_json::json!(["INBOX"]),
        "a label the owner removed must not hold a rule open"
    );
}

pub async fn a_removed_message_leaves_the_thread_it_was_in(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            mail("m1", "clinic", 10, "clinic.test"),
            mail("m2", "clinic", 20, "clinic.test"),
        ],
    )
    .await;
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![removed("m2", "2")],
    )
    .await;
    let thread = store.parent_metadata(&key, "clinic").await.unwrap();
    assert_eq!(thread.len(), 1);
    assert_eq!(thread[0]["message_id"], "m1");
    let threads = store.parents_metadata(&key).await.unwrap();
    assert_eq!(threads, vec![("clinic".to_string(), thread)]);
}

pub async fn a_thread_of_removed_messages_counts_for_no_sender_domain(backend: &Backend) {
    let (store, state) = setup(backend).await;
    let key = state.key();
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![
            mail("m1", "one", 10, "shop.test"),
            mail("m2", "two", 20, "shop.test"),
        ],
    )
    .await;
    acquire(
        store.as_ref(),
        backend.keys(),
        &key,
        vec![removed("m2", "2")],
    )
    .await;
    assert_eq!(
        store
            .parents_with_metadata(&key, "from_domain", "shop.test", 0)
            .await
            .unwrap(),
        1
    );
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_knowledge {
    ($emit:path) => {
        $emit!(
            knowledge,
            a_forget_deletes_the_model_request_captures_of_a_run_that_read_it,
            a_new_filter_bumps_the_revision_and_starts_its_own_counts,
            a_later_pass_replaces_the_decision_of_the_same_revision,
            page_signals_read_the_metadata_of_one_thread,
            a_preview_reads_the_metadata_of_every_thread_in_one_pass,
            sender_threads_count_distinct_parents_inside_the_window,
            acquisition_commits_work_and_cursor_together_and_rejects_replay,
            changing_import_scope_restarts_acquisition,
            post_baseline_arrival_can_be_discovered_in_the_historical_scan,
            source_forget_blocks_reimport_and_keeps_other_sources,
            a_failed_source_purge_stays_blocked_until_retry,
            a_copy_of_the_database_cannot_compute_the_identity_of_a_forgotten_source,
            a_forgotten_source_stays_suppressed_after_the_purge_and_a_restart,
            a_workspace_that_never_forgot_anything_asks_for_no_key,
            a_forget_purge_leaves_no_value_of_the_forgotten_item_in_any_table,
            a_new_retrieval_of_a_forgotten_item_writes_nothing_and_stays_suppressed,
            a_forget_leaves_no_text_of_the_item_in_the_memory_search_index,
            an_event_of_a_forgotten_item_writes_no_row_and_the_others_still_ingest,
            an_ingest_in_a_workspace_that_never_forgot_anything_asks_for_no_key,
            a_forget_leaves_no_path_or_title_of_the_purged_page_in_the_feed_or_a_brief,
            a_forget_leaves_no_copy_of_the_purged_page_in_the_database_files,
            a_forget_leaves_no_text_of_the_item_in_the_conversations_that_quoted_it,
            a_forget_of_one_item_keeps_the_messages_that_quoted_only_other_items,
            a_forget_forgets_the_reply_of_the_run_that_the_item_woke,
            an_account_forget_deletes_the_tool_results_and_the_reads_of_its_connection,
            a_forget_refuses_the_record_of_a_read_through_its_connection,
            a_failed_arrival_waits_for_its_retry_time,
            the_fifth_failure_skips_the_arrival,
            a_permanent_failure_skips_the_arrival_at_once,
            an_acknowledged_arrival_counts_as_appended,
            the_backfill_offers_undecided_historical_pages_newest_first,
            a_recorded_run_moves_a_page_from_waiting_to_reflected,
            the_budget_counts_the_backfill_runs_of_the_window,
            a_version_without_metadata_waits_for_its_metadata_before_the_backfill,
            completing_metadata_drops_the_pending_decision_of_the_subject,
            a_failed_run_returns_its_pages_to_the_batch_and_leaves_the_budget,
            a_page_stops_waiting_after_the_attempt_limit,
            page_signals_read_the_newest_version_of_each_message,
            a_removed_message_leaves_the_thread_it_was_in,
            a_thread_of_removed_messages_counts_for_no_sender_domain,
        );
    };
}
