//! The baseline applies to an empty database, and it holds the schema
//! the store modules are written against.

/// Every table of the schema, less `sqlite_sequence` and the FTS5 shadow
/// tables, which are engine internals. The Postgres test holds the same
/// list, so a table that lands on one backend and not the other is
/// caught.
const TABLES: &[&str] = &[
    "agent_mailboxes",
    "agents",
    "artifact_retention_policies",
    "artifacts",
    "calls",
    "capability_snapshots",
    "channel_participants",
    "channels",
    "coding_session_events",
    "coding_sessions",
    "connections",
    "continuation_checkpoint_exposures",
    "continuation_checkpoint_messages",
    "continuation_checkpoints",
    "contributions",
    "conversation_message_search",
    "conversation_tool_evidence",
    "conversation_tool_search",
    "credentials",
    "event_subscriptions",
    "events",
    "forget_operations",
    "forget_suppressions",
    "forgotten_messages",
    "grants",
    "hosts",
    "incoming_events",
    "keypad_failures",
    "knowledge_generations",
    "knowledge_invalidations",
    "memory_brief_cursors",
    "memory_page_index",
    "memory_page_index_heads",
    "memory_page_link",
    "memory_page_search",
    "message_exposure_sets",
    "message_exposures",
    "messages",
    "model_aliases",
    "model_request_captures",
    "onboarding_model_verifications",
    "orgs",
    "page_reflections",
    "pending_evidence",
    "pending_evidence_exposures",
    "pending_evidence_messages",
    "pending_review_cursors",
    "phone_number_purchase_intents",
    "phone_numbers",
    "plugin_bindings",
    "plugin_tools",
    "plugins",
    "provider_cursors",
    "push_subscriptions",
    "requests",
    "run_capability_snapshots",
    "run_source_reads",
    "runs",
    "schedule_occurrences",
    "schedule_revisions",
    "schedule_subject_pages",
    "schedules",
    "sent_mail",
    "sessions",
    "sign_in_links",
    "software_packages",
    "software_versions",
    "source_batches",
    "source_items",
    "source_versions",
    "sync_resources",
    "text_records",
    "trust_entries",
    "usage",
    "users",
    "wakeup_sources",
    "wakeups",
    "workspaces",
];

/// The five shadow tables FTS5 keeps for each full-text index.
const FTS5_SHADOWS: &[&str] = &["_config", "_content", "_data", "_docsize", "_idx"];

#[tokio::test]
async fn the_baseline_applies_to_an_empty_database_and_holds_every_table() {
    let pool = pagis_storage_sqlite::connect_memory().await.unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();

    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let present: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| {
            !FTS5_SHADOWS.iter().any(|suffix| {
                name.strip_suffix(suffix)
                    .is_some_and(|index| names.iter().any(|other| other == index))
            })
        })
        .collect();

    assert_eq!(present, TABLES);
}

#[tokio::test]
async fn the_run_state_constraint_accepts_reflecting() {
    let pool = pagis_storage_sqlite::connect_memory().await.unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO workspaces(id,name,timezone,created_at) VALUES('w','Home','UTC',1); \
         INSERT INTO model_aliases(id,workspace_id,alias,candidates,created_at,updated_at) \
             VALUES('model','w','default','[]',1,1); \
         INSERT INTO agents(id,workspace_id,name,job,personality,model_alias,status,created_at,updated_at) \
             VALUES('a','w','Sage','assistant','plain','default','active',1,1); \
         INSERT INTO runs(id,workspace_id,agent_id,trigger_kind,state,created_at,title) \
             VALUES('r','w','a','arrival','reflecting',1,'Bring a source into memory');",
    )
    .execute(&pool)
    .await
    .unwrap();
}

/// Each full-text index carries the tenant of its rows, so one person's
/// corpus never reaches another person's results (ADR-0008).
#[tokio::test]
async fn every_search_table_holds_the_tenant() {
    let pool = pagis_storage_sqlite::connect_memory().await.unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();

    for table in [
        "memory_page_search",
        "conversation_message_search",
        "conversation_tool_search",
    ] {
        let names: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            names.iter().any(|name| name == "workspace_id"),
            "{table} columns: {names:?}"
        );
    }
}

/// A Forget key never sits in the database beside the identities that
/// it keys. The key derives from the Tenant Data Key, so no table of
/// the baseline holds one and no query of this crate reads one
/// (ADR-0008).
#[test]
fn no_query_reads_a_forget_key_from_the_database() {
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let mut readers = Vec::new();
    for directory in ["src", "migrations"] {
        for entry in std::fs::read_dir(root.join(directory)).expect("read the crate") {
            let path = entry.expect("a file of the crate").path();
            let text = std::fs::read_to_string(&path).expect("read a file of the crate");
            if text.contains("forget_keys") {
                readers.push(path.display().to_string());
            }
        }
    }
    assert_eq!(
        readers,
        Vec::<String>::new(),
        "these files keep a Forget key in the database"
    );
}

/// The migration that gives an event source a Coding Session makes the
/// three tables again in SQLite. The rows of a Connection stay, with
/// the Wake-ups that point at them (ADR-0033).
#[tokio::test]
async fn the_rules_batches_and_events_of_a_connection_stay_through_coding_session_events() {
    let pool = pagis_storage_sqlite::connect_memory().await.unwrap();
    let (before, _earlier) = pagis_testkit::migration::migrator_before(
        &migrations(),
        pagis_testkit::migration::CODING_SESSION_EVENTS,
    )
    .await;
    before.run(&pool).await.unwrap();
    sqlx::raw_sql(pagis_testkit::migration::CONNECTION_EVENT_ROWS)
        .execute(&pool)
        .await
        .unwrap();

    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();

    let broken: Vec<String> = sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(broken, Vec::<String>::new());
    pagis_testkit::migration::assert_connection_event_rows(&pagis_storage_sqlite::stores(pool))
        .await;
}

/// The migration directory of this crate.
fn migrations() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("migrations")
}

#[tokio::test]
async fn run_titles_are_backfilled_without_losing_references() {
    let pool = pagis_storage_sqlite::connect_memory().await.unwrap();
    let (before, _earlier) = pagis_testkit::migration::migrator_before(&migrations(), 11).await;
    before.run(&pool).await.unwrap();
    sqlx::raw_sql(pagis_testkit::migration::CONNECTION_EVENT_ROWS)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(pagis_testkit::migration::RUN_TITLE_ROWS)
        .execute(&pool)
        .await
        .unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();
    let broken: Vec<String> = sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(broken.is_empty(), "{broken:?}");
    pagis_testkit::migration::assert_run_titles(&pagis_storage_sqlite::stores(pool)).await;
}
