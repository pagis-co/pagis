//! The baseline applies to an empty database, and it holds the schema
//! the store modules are written against.

use pagis_testkit::postgres;

/// Every table of the schema. The SQLite test holds the same list, so a
/// table that lands on one backend and not the other is caught.
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
    "continuation_checkpoint_exposures",
    "continuation_checkpoint_messages",
    "continuation_checkpoints",
    "contributions",
    "conversation_message_search",
    "conversation_tool_evidence",
    "conversation_tool_search",
    "credentials",
    "connections",
    "events",
    "event_subscriptions",
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

#[tokio::test]
async fn the_baseline_applies_to_an_empty_database_and_holds_every_table() {
    let Some(database) = postgres::database().await else {
        return;
    };

    let present: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' \
           AND table_name <> '_sqlx_migrations' ORDER BY table_name",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();

    let mut expected: Vec<String> = TABLES.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(present, expected);
}

#[tokio::test]
async fn every_text_column_compares_bytes() {
    let Some(database) = postgres::database().await else {
        return;
    };

    // SQLite compares TEXT with BINARY. One store-trait suite holds both
    // backends to the same order, so the locale of the database must not
    // decide it: every text column carries the `C` collation (ADR-0024).
    // `_sqlx_migrations` is the migration runner's own bookkeeping, not a
    // table of this schema.
    let locale_ordered: Vec<String> = sqlx::query_scalar(
        "SELECT table_name || '.' || column_name FROM information_schema.columns \
         WHERE table_schema = current_schema() AND data_type = 'text' \
           AND table_name <> '_sqlx_migrations' \
           AND collation_name IS DISTINCT FROM 'C' ORDER BY 1",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();

    assert_eq!(locale_ordered, Vec::<String>::new());
}

#[tokio::test]
async fn every_full_text_index_carries_the_tenant() {
    let Some(database) = postgres::database().await else {
        return;
    };

    // One person's corpus must not reach another person's results, so
    // the tenant is a column of the index itself and not only of the
    // table it joins (ADR-0008).
    for table in [
        "memory_page_search",
        "conversation_message_search",
        "conversation_tool_search",
    ] {
        let tenant: Option<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 \
               AND column_name = 'workspace_id'",
        )
        .bind(table)
        .fetch_optional(&database.pool)
        .await
        .unwrap();
        assert_eq!(tenant.as_deref(), Some("workspace_id"), "{table}");

        let document: Option<String> = sqlx::query_scalar(
            "SELECT data_type FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1 \
               AND column_name = 'document'",
        )
        .bind(table)
        .fetch_optional(&database.pool)
        .await
        .unwrap();
        assert_eq!(document.as_deref(), Some("tsvector"), "{table}");
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

/// The rows of a Connection stay through the migration that gives an
/// event source a Coding Session (ADR-0033).
#[tokio::test]
async fn the_rules_batches_and_events_of_a_connection_stay_through_coding_session_events() {
    let Some(database) = postgres::database().await else {
        return;
    };
    // The test database comes with every migration, so the schema is
    // made again from the earlier ones.
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&database.pool)
        .await
        .unwrap();
    let (before, _earlier) = pagis_testkit::migration::migrator_before(
        &migrations(),
        pagis_testkit::migration::CODING_SESSION_EVENTS,
    )
    .await;
    before.run(&database.pool).await.unwrap();
    sqlx::raw_sql(pagis_testkit::migration::CONNECTION_EVENT_ROWS)
        .execute(&database.pool)
        .await
        .unwrap();

    pagis_storage_postgres::MIGRATOR
        .run(&database.pool)
        .await
        .unwrap();

    pagis_testkit::migration::assert_connection_event_rows(&pagis_storage_postgres::stores(
        database.pool,
    ))
    .await;
}

/// An `auto` session reads `person` after the migration that leaves
/// `person` and `agent` as the modes, and its transcript and its model
/// token stay (ADR-0033).
#[tokio::test]
async fn an_auto_session_reads_person_and_keeps_its_rows_through_person_and_agent_modes() {
    let Some(database) = postgres::database().await else {
        return;
    };
    // The test database comes with every migration, so the schema is
    // made again from the earlier ones.
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&database.pool)
        .await
        .unwrap();
    let (before, _earlier) = pagis_testkit::migration::migrator_before(
        &migrations(),
        pagis_testkit::migration::PERSON_AND_AGENT_MODES,
    )
    .await;
    before.run(&database.pool).await.unwrap();
    sqlx::raw_sql(pagis_testkit::migration::AUTO_SESSION_ROWS)
        .execute(&database.pool)
        .await
        .unwrap();

    pagis_storage_postgres::MIGRATOR
        .run(&database.pool)
        .await
        .unwrap();

    let refused = sqlx::raw_sql("UPDATE coding_sessions SET approval_mode = 'auto'")
        .execute(&database.pool)
        .await;
    assert!(refused.is_err(), "the check refuses auto");
    pagis_testkit::migration::assert_auto_session_rows(&pagis_storage_postgres::stores(
        database.pool,
    ))
    .await;
}

/// An older session reads no mode and no offered modes after the
/// migration that gives a Coding Session its Harness Mode, its transcript
/// stays, and the transcript then takes a `mode` row (ADR-0033).
#[tokio::test]
async fn an_older_session_reads_no_mode_and_keeps_its_rows_through_harness_modes() {
    let Some(database) = postgres::database().await else {
        return;
    };
    // The test database comes with every migration, so the schema is
    // made again from the earlier ones.
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&database.pool)
        .await
        .unwrap();
    let (before, _earlier) = pagis_testkit::migration::migrator_before(
        &migrations(),
        pagis_testkit::migration::HARNESS_MODES,
    )
    .await;
    before.run(&database.pool).await.unwrap();
    sqlx::raw_sql(pagis_testkit::migration::MODELESS_SESSION_ROWS)
        .execute(&database.pool)
        .await
        .unwrap();

    pagis_storage_postgres::MIGRATOR
        .run(&database.pool)
        .await
        .unwrap();

    pagis_testkit::migration::assert_modeless_session_rows(&pagis_storage_postgres::stores(
        database.pool,
    ))
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
    let Some(database) = postgres::database().await else {
        return;
    };
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(&database.pool)
        .await
        .unwrap();
    let (before, _earlier) = pagis_testkit::migration::migrator_before(&migrations(), 11).await;
    before.run(&database.pool).await.unwrap();
    sqlx::raw_sql(pagis_testkit::migration::CONNECTION_EVENT_ROWS)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::raw_sql(pagis_testkit::migration::RUN_TITLE_ROWS)
        .execute(&database.pool)
        .await
        .unwrap();
    pagis_storage_postgres::MIGRATOR
        .run(&database.pool)
        .await
        .unwrap();
    pagis_testkit::migration::assert_run_titles(&pagis_storage_postgres::stores(
        database.pool.clone(),
    ))
    .await;
}
