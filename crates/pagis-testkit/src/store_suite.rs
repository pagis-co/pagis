//! The store-trait suite: one set of test bodies, both backends.
//!
//! The traits of `pagis-core` are the contract between the daemon and
//! its records. Two crates implement them, so one suite proves both: a
//! body written here runs on SQLite and on Postgres, and a method that
//! behaves differently fails instead of surprising somebody later.
//!
//! A body takes a [`Backend`], which gives it the store set and a small
//! portable way to write and read a row directly. It never names a pool
//! type, so a body cannot be written for one backend only.
//!
//! [`store_suite!`] turns every body a module names into a SQLite test
//! and a Postgres test. A body that its module does not name fails the
//! guard test at the bottom of this file, so a body cannot be added and
//! left out of a backend either.

use std::path::PathBuf;
use std::sync::Arc;

use pagis_core::{
    MemorySecretStore, SecretStore, Stores, TenantKeys, Workspace, WorkspaceId, now_ms,
};
use tempfile::TempDir;

pub use crate::sql::{Bind, Rows};

/// The name of the SQLite database file in its temp directory.
const SQLITE_FILE: &str = "pagis.db";

/// The store set of one empty database, with a portable way to reach
/// the rows behind it.
pub struct Backend {
    stores: Stores,
    rows: Rows,
    /// The `secrets.enc` of the installation, which holds the Tenant
    /// Data Key of each Workspace. It is not in the database.
    secrets: Arc<dyn SecretStore>,
    /// The one holder of the Tenant Data Keys that the daemon passes to
    /// the Forget and acquisition calls.
    keys: TenantKeys,
    /// The directory the SQLite file lives in, held so it outlives the
    /// pool.
    home: Option<TempDir>,
}

impl Backend {
    /// A fresh SQLite database with the migrations applied.
    pub async fn sqlite() -> Self {
        let home = tempfile::tempdir().expect("a temp directory for the database");
        let pool = pagis_storage_sqlite::connect(&home.path().join(SQLITE_FILE))
            .await
            .expect("open the test database");
        pagis_storage_sqlite::MIGRATOR
            .run(&pool)
            .await
            .expect("apply the migrations");
        Self::over(
            pagis_storage_sqlite::stores(pool.clone()),
            Rows::Sqlite(pool),
            Some(home),
        )
    }

    /// A fresh Postgres database with the migrations applied, or `None`
    /// when Docker is not reachable. The caller skips on `None`; the
    /// reason is already on stderr.
    pub async fn postgres() -> Option<Self> {
        let database = crate::postgres::database().await?;
        Some(Self::over(
            pagis_storage_postgres::stores(database.pool.clone()),
            Rows::Postgres(database.pool),
            None,
        ))
    }

    fn over(stores: Stores, rows: Rows, home: Option<TempDir>) -> Self {
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        Self {
            stores,
            rows,
            keys: TenantKeys::new(Arc::clone(&secrets)),
            secrets,
            home,
        }
    }

    pub fn stores(&self) -> &Stores {
        &self.stores
    }

    /// The Tenant Data Keys of the installation, which derive the
    /// suppression key that a Forget and an acquisition take.
    pub fn keys(&self) -> &TenantKeys {
        &self.keys
    }

    /// The Tenant Data Keys after a daemon restart: a new holder over
    /// the same `secrets.enc`.
    pub fn restarted_keys(&self) -> TenantKeys {
        TenantKeys::new(Arc::clone(&self.secrets))
    }

    /// The rows behind the stores, for a body that plants a row no store
    /// method writes or reads a column no trait answers.
    pub fn rows(&self) -> &Rows {
        &self.rows
    }

    /// The files of the SQLite database: the main file and its WAL. A
    /// body that reads the disk the way an attacker does reads them.
    /// Postgres keeps its files in its own server, so the list is empty
    /// there.
    pub fn database_files(&self) -> Vec<PathBuf> {
        self.home
            .iter()
            .flat_map(|home| {
                [
                    home.path().join(SQLITE_FILE),
                    home.path().join(format!("{SQLITE_FILE}-wal")),
                ]
            })
            .collect()
    }

    /// Which backend holds these rows. A body asserts nothing on
    /// it: only a body whose property differs by engine reads it, and it
    /// says in a comment why (ADR-0008).
    pub fn is_postgres(&self) -> bool {
        matches!(self.rows, Rows::Postgres(_))
    }

    /// The Org, the administrator and the Workspace row a body needs
    /// before it writes anything of a Workspace. The Workspace is
    /// written, so a body does not write it again.
    pub async fn seeded_workspace(&self) -> Workspace {
        let person = pagis_core::seed_org_and_administrator(
            self.stores.orgs.as_ref(),
            self.stores.users.as_ref(),
            "Org",
            now_ms(),
        )
        .await
        .expect("seed the Org and its administrator");
        let workspace = Workspace {
            id: WorkspaceId::generate(),
            user_id: person.id,
            ..crate::fixture::workspace()
        };
        self.stores
            .workspaces
            .create(&workspace)
            .await
            .expect("write the Workspace");
        workspace
    }

    /// Run one statement of the portable subset. See [`Rows`].
    pub async fn execute(&self, sql: &str, binds: &[Bind]) -> Result<u64, String> {
        self.rows.execute(sql, binds).await
    }

    /// One integer, for a body that counts rows.
    pub async fn count(&self, sql: &str, binds: &[Bind]) -> Result<i64, String> {
        self.rows.count(sql, binds).await
    }
}

/// The modules that hold the bodies. Each one is the trait tests of one
/// area, and each one carries its own `store_suite_<module>!` list right
/// under its bodies.
pub mod continuation;
pub mod conversation_evidence;
pub mod hosts;
pub mod installation;
pub mod keypad_failures;
pub mod knowledge;
pub mod knowledge_events;
pub mod mailboxes;
pub mod memory_pages;
pub mod model_request_captures;
pub mod pending_reviews;
pub mod push_subscriptions;
pub mod sign_in;
pub mod stores;
pub mod text_records;
pub mod workspaces;

/// Write both backends' tests for every body of the suite. One line in
/// a test binary runs the whole suite:
///
/// ```ignore
/// pagis_testkit::store_suite!();
/// ```
///
/// One line for each module. A module's own macro names its bodies, and
/// the guard test below fails while a body of a module is missing from
/// its list, so a body cannot reach one backend only.
#[macro_export]
macro_rules! store_suite {
    () => {
        $crate::store_suite_continuation!($crate::__store_suite_emit);
        $crate::store_suite_conversation_evidence!($crate::__store_suite_emit);
        $crate::store_suite_hosts!($crate::__store_suite_emit);
        $crate::store_suite_installation!($crate::__store_suite_emit);
        $crate::store_suite_keypad_failures!($crate::__store_suite_emit);
        $crate::store_suite_knowledge!($crate::__store_suite_emit);
        $crate::store_suite_knowledge_events!($crate::__store_suite_emit);
        $crate::store_suite_mailboxes!($crate::__store_suite_emit);
        $crate::store_suite_memory_pages!($crate::__store_suite_emit);
        $crate::store_suite_model_request_captures!($crate::__store_suite_emit);
        $crate::store_suite_pending_reviews!($crate::__store_suite_emit);
        $crate::store_suite_push_subscriptions!($crate::__store_suite_emit);
        $crate::store_suite_sign_in!($crate::__store_suite_emit);
        $crate::store_suite_stores!($crate::__store_suite_emit);
        $crate::store_suite_text_records!($crate::__store_suite_emit);
        $crate::store_suite_workspaces!($crate::__store_suite_emit);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __store_suite_emit {
    ($module:ident, $($name:ident),* $(,)?) => {
        mod $module {
            mod on_sqlite {
                $(
                    #[tokio::test]
                    async fn $name() {
                        let backend = $crate::store_suite::Backend::sqlite().await;
                        $crate::store_suite::$module::$name(&backend).await;
                    }
                )*
            }

            mod on_postgres {
                $(
                    #[tokio::test]
                    async fn $name() {
                        let Some(backend) = $crate::store_suite::Backend::postgres().await
                        else {
                            return;
                        };
                        $crate::store_suite::$module::$name(&backend).await;
                    }
                )*
            }
        }
    };
}

/// The module files of the suite, with the macro that lists each one's
/// bodies. The guard test reads both.
#[cfg(test)]
const MODULES: &[(&str, &str, &str)] = &[
    (
        "continuation",
        include_str!("store_suite/continuation.rs"),
        "macro_rules! store_suite_continuation",
    ),
    (
        "conversation_evidence",
        include_str!("store_suite/conversation_evidence.rs"),
        "macro_rules! store_suite_conversation_evidence",
    ),
    (
        "hosts",
        include_str!("store_suite/hosts.rs"),
        "macro_rules! store_suite_hosts",
    ),
    (
        "installation",
        include_str!("store_suite/installation.rs"),
        "macro_rules! store_suite_installation",
    ),
    (
        "keypad_failures",
        include_str!("store_suite/keypad_failures.rs"),
        "macro_rules! store_suite_keypad_failures",
    ),
    (
        "knowledge",
        include_str!("store_suite/knowledge.rs"),
        "macro_rules! store_suite_knowledge",
    ),
    (
        "knowledge_events",
        include_str!("store_suite/knowledge_events.rs"),
        "macro_rules! store_suite_knowledge_events",
    ),
    (
        "mailboxes",
        include_str!("store_suite/mailboxes.rs"),
        "macro_rules! store_suite_mailboxes",
    ),
    (
        "memory_pages",
        include_str!("store_suite/memory_pages.rs"),
        "macro_rules! store_suite_memory_pages",
    ),
    (
        "model_request_captures",
        include_str!("store_suite/model_request_captures.rs"),
        "macro_rules! store_suite_model_request_captures",
    ),
    (
        "pending_reviews",
        include_str!("store_suite/pending_reviews.rs"),
        "macro_rules! store_suite_pending_reviews",
    ),
    (
        "push_subscriptions",
        include_str!("store_suite/push_subscriptions.rs"),
        "macro_rules! store_suite_push_subscriptions",
    ),
    (
        "sign_in",
        include_str!("store_suite/sign_in.rs"),
        "macro_rules! store_suite_sign_in",
    ),
    (
        "stores",
        include_str!("store_suite/stores.rs"),
        "macro_rules! store_suite_stores",
    ),
    (
        "text_records",
        include_str!("store_suite/text_records.rs"),
        "macro_rules! store_suite_text_records",
    ),
    (
        "workspaces",
        include_str!("store_suite/workspaces.rs"),
        "macro_rules! store_suite_workspaces",
    ),
];

#[cfg(test)]
mod guard {
    /// Every `pub async fn` of a module of the suite is named in that
    /// module's list.
    ///
    /// Without this a body could be written and never run, which is the
    /// one failure a shared suite exists to stop.
    #[test]
    fn every_body_of_the_suite_is_named_in_its_list() {
        let mut missing = Vec::new();
        for (module, source, macro_name) in super::MODULES {
            let listed = source
                .split(macro_name)
                .nth(1)
                .unwrap_or_else(|| panic!("{module} holds no {macro_name}"));
            for line in source.lines() {
                let Some(rest) = line.trim().strip_prefix("pub async fn ") else {
                    continue;
                };
                let name = rest
                    .split(['(', '<'])
                    .next()
                    .expect("a function name")
                    .trim();
                if !listed.contains(name) {
                    missing.push(format!("{module}::{name}"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "these suite bodies are not named in their module's list, so they run on \
             no backend: {missing:?}"
        );
    }
}
