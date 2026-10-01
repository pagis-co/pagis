//! Boot seam: `pagis::boot` on an empty directory creates the full
//! layout and seed data; a second boot is idempotent.

use pagis::Installation;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[tokio::test]
async fn first_boot_creates_the_layout_the_credential_the_db_and_the_seed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let booted = pagis::boot(&home, Installation::Local).await.unwrap();

    assert_eq!(
        mode(&home),
        0o700,
        "the OS user alone enters the state directory"
    );
    assert!(home.join("config.toml").is_file());
    assert!(home.join("client-credential").is_file());
    assert_eq!(
        mode(&home.join("client-credential")),
        0o600,
        "the OS user alone reads it"
    );
    assert!(home.join("logs").is_dir());
    assert!(home.join("pagis.db").is_file());
    assert!(
        booted
            .client_credential
            .is_some_and(|credential| !credential.is_empty())
    );
    assert_eq!(booted.config.port, 4400);

    let workspaces = booted.stores.workspaces.list().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "Workspace");

    // One Org, one administrator, and the Workspace belongs to them.
    let orgs = booted.stores.orgs.list().await.unwrap();
    assert_eq!(orgs.len(), 1);
    let people = booted.stores.users.list_by_org(&orgs[0].id).await.unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].role, pagis_core::UserRole::Administrator);
    assert_eq!(people[0].password_hash, None);
    assert_eq!(workspaces[0].user_id, people[0].id);

    let agents = booted
        .stores
        .agents
        .list_by_workspace(&workspaces[0].id)
        .await
        .unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "Pixie");
    assert_eq!(agents[0].job, "Chief of Staff");

    // The assistant's DM channel, with Pixie as its agent participant.
    let channels = booted
        .stores
        .channels
        .list_by_workspace(&workspaces[0].id)
        .await
        .unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].title.as_deref(), Some("Pixie"));
    let channel_agents = booted
        .stores
        .participants
        .agents_in_channel(&workspaces[0].id, &channels[0].id)
        .await
        .unwrap();
    assert_eq!(channel_agents, vec![agents[0].id.clone()]);
}

#[tokio::test]
async fn second_boot_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let first = pagis::boot(&home, Installation::Local).await.unwrap();
    let first_credential = first.client_credential.clone();
    drop(first);

    let second = pagis::boot(&home, Installation::Local).await.unwrap();

    assert_eq!(second.client_credential, first_credential);

    let workspaces = second.stores.workspaces.list().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    let agents = second
        .stores
        .agents
        .list_by_workspace(&workspaces[0].id)
        .await
        .unwrap();
    assert_eq!(agents.len(), 1);
    let channels = second
        .stores
        .channels
        .list_by_workspace(&workspaces[0].id)
        .await
        .unwrap();
    assert_eq!(channels.len(), 1);
}

#[tokio::test]
async fn boot_adds_new_well_known_aliases_without_replacing_a_user_choice() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let first = pagis::boot(&home, Installation::Local).await.unwrap();
    let workspace = first.stores.workspaces.list().await.unwrap().remove(0);
    let aliases = first.stores.model_aliases.clone();
    aliases
        .update_candidates(
            &workspace.id,
            pagis_telephony::PHONE_ALIAS,
            &["openai/gpt-realtime-2.1-mini".to_string()],
            pagis_core::now_ms(),
        )
        .await
        .unwrap();
    aliases
        .delete(&workspace.id, pagis_telephony::GPT_LIVE_REASONING_ALIAS)
        .await
        .unwrap();
    drop(first);

    let second = pagis::boot(&home, Installation::Local).await.unwrap();
    let aliases = second.stores.model_aliases;
    let phone = aliases
        .get_by_alias(&workspace.id, pagis_telephony::PHONE_ALIAS)
        .await
        .unwrap()
        .unwrap();
    let reasoning = aliases
        .get_by_alias(&workspace.id, pagis_telephony::GPT_LIVE_REASONING_ALIAS)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(phone.candidates, vec!["openai/gpt-realtime-2.1-mini"]);
    assert_eq!(
        reasoning.candidates,
        pagis_telephony::GPT_LIVE_REASONING_MODELS
    );
}

/// A second daemon on the same data directory must fail fast, before it
/// touches the database: on a real restart, the boot recovery further
/// down fails every unfinished run and expires pending requests, so a
/// second daemon that ran recovery and then crashed on the port bind
/// would corrupt the first daemon's in-flight runs out from under it.
#[tokio::test]
async fn a_second_boot_while_the_first_is_still_running_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let first = pagis::boot(&home, Installation::Local).await.unwrap();

    let err = pagis::boot(&home, Installation::Local).await.unwrap_err();
    assert!(err.to_string().contains("already running"), "{err}");

    drop(first);
    // The lock releases with the first handle: a boot afterward succeeds.
    pagis::boot(&home, Installation::Local).await.unwrap();
}

/// A daemon on its way out can hold the lock for a moment after its
/// owner let go (a forked child keeps the file description until it
/// execs). A boot that follows at once waits for it instead of
/// reporting a live sibling.
#[tokio::test]
async fn a_boot_waits_for_a_lock_that_is_about_to_be_released() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let first = pagis::boot(&home, Installation::Local).await.unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        drop(first);
    });

    pagis::boot(&home, Installation::Local).await.unwrap();
}

#[tokio::test]
async fn an_older_server_refuses_before_it_opens_workspace_data() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::create_dir_all(home.join("memory")).unwrap();
    let preserved = [
        ("config.toml", "existing config"),
        ("client-credential", "existing credential"),
        ("pagis.db", "existing database"),
        ("memory/MEMORY.md", "existing memory"),
        ("secrets.enc", "existing secrets"),
    ];
    for (name, contents) in preserved {
        std::fs::write(home.join(name), contents).unwrap();
    }
    std::fs::write(home.join("runtime-release"), "99.0.0\n").unwrap();

    let error = pagis::boot(&home, Installation::Local)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("newer Pagis 99.0.0"), "{error}");
    for (name, contents) in preserved {
        assert_eq!(std::fs::read_to_string(home.join(name)).unwrap(), contents);
    }
    assert_eq!(
        std::fs::read_to_string(home.join("runtime-release")).unwrap(),
        "99.0.0\n"
    );
}

#[tokio::test]
async fn build_metadata_does_not_make_the_same_server_a_downgrade() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::create_dir_all(&home).unwrap();
    let recorded = format!("{}+signed.9\n", pagis_server::VERSION);
    std::fs::write(home.join("runtime-release"), &recorded).unwrap();

    let booted = pagis::boot(&home, Installation::Local).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(home.join("runtime-release")).unwrap(),
        recorded
    );
    drop(booted);
}

/// The upgrade path: a new binary boots on the data directory an
/// older one left behind. It keeps the config, the Client Credential,
/// and every record, and it brings the database up to the current
/// schema.
#[tokio::test]
async fn upgrade_preserves_the_data_directory_and_migrates_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let first = pagis::boot(&home, Installation::Local).await.unwrap();
    assert!(first.first_run);
    let workspace = first.stores.workspaces.list().await.unwrap().remove(0);
    let agents = first.stores.agents.clone();
    let hired = pagis_core::Agent {
        id: pagis_core::AgentId::generate(),
        workspace_id: workspace.id.clone(),
        name: "Rune".into(),
        job: "researcher".into(),
        description: String::new(),
        personality: "Curious".into(),
        model_alias: "default".into(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: pagis_core::AgentStatus::Active,
        created_at: pagis_core::now_ms(),
        updated_at: pagis_core::now_ms(),
    };
    agents.create(&hired).await.unwrap();
    let config_before = std::fs::read_to_string(home.join("config.toml")).unwrap();
    let credential_before = first.client_credential.clone();
    drop(first);

    // The new binary starts on the same directory.
    let second = pagis::boot(&home, Installation::Local).await.unwrap();

    assert!(
        !second.first_run,
        "an upgrade must not reseed the workspace"
    );
    assert_eq!(second.client_credential, credential_before);
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        config_before
    );
    let names: Vec<String> = second
        .stores
        .agents
        .list_by_workspace(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names.contains(&"Rune".to_string()), "{names:?}");

    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&rows(&second).await)
        .await
        .unwrap();
    assert_eq!(
        applied as usize,
        pagis_storage_sqlite::MIGRATOR.iter().count(),
        "boot must apply every migration"
    );
}

/// A Workspace a boot finds keeps the Agent it already has: the seed
/// must not rename the Agent to the name a fresh Workspace takes, or
/// add a second one.
#[tokio::test]
async fn a_boot_keeps_the_agent_of_an_existing_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::create_dir_all(&home).unwrap();
    let pool = pagis_storage_sqlite::connect(&home.join("pagis.db"))
        .await
        .unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();
    for statement in [
        "INSERT INTO workspaces(id,name,created_at) VALUES('w0','Org',1)",
        "INSERT INTO orgs(id,name,workspace_id,created_at) VALUES('o1','Org','w0',1)",
        "INSERT INTO users(id,org_id,role,created_at,updated_at) VALUES('u1','o1','administrator',1,1)",
        "INSERT INTO workspaces(id,name,created_at,user_id) VALUES('w1','Home',1,'u1')",
        "INSERT INTO agents(id,workspace_id,name,job,personality,model_alias,status,created_at,updated_at) VALUES('a1','w1','Sage','assistant','Plain','default','active',1,1)",
        "INSERT INTO channels(id,workspace_id,title,created_at,updated_at,kind) VALUES('ch1','w1','Sage',1,1,'dm')",
        "INSERT INTO channel_participants(id,workspace_id,channel_id,participant_kind,agent_id,joined_at) VALUES('p1','w1','ch1','agent','a1',1)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    drop(pool);

    let booted = pagis::boot(&home, Installation::Local).await.unwrap();

    let agents: Vec<String> = sqlx::query_scalar("SELECT name FROM agents ORDER BY id")
        .fetch_all(&rows(&booted).await)
        .await
        .unwrap();
    assert_eq!(agents, vec!["Sage".to_string()]);
}

/// A server holds no Client Credential in any configuration: the boot
/// writes no file, and it removes a file that a local run left behind.
/// A loopback Public Origin does not change that.
/// The test skips when Docker is not reachable, because a server keeps
/// its records in Postgres.
#[tokio::test]
async fn a_server_boot_holds_no_client_credential() {
    let Some(database) = pagis_testkit::postgres::database().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    let local = pagis::boot(&home, Installation::Local).await.unwrap();
    assert!(home.join("client-credential").is_file());
    drop(local);
    set_database_url(&home, &database.url);

    let server = pagis::boot(&home, Installation::Server).await.unwrap();

    assert_eq!(server.client_credential, None);
    assert!(
        !home.join("client-credential").exists(),
        "the file a local run wrote is gone"
    );
    assert!(
        server
            .config
            .public_origin(4400)
            .starts_with("http://127.0.0.1")
    );
}

/// A local installation runs from one binary with no database service
/// (ADR-0024): nothing set in the configuration means SQLite in
/// the state directory, and the boot makes the file itself.
#[tokio::test]
async fn no_database_setting_keeps_the_records_in_the_state_directory() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let booted = pagis::boot(&home, Installation::Local).await.unwrap();

    assert!(home.join("pagis.db").exists(), "the boot makes the file");
    assert_eq!(booted.config.database.url(), None);
    assert_eq!(booted.stores.workspaces.list().await.unwrap().len(), 1);
}

/// A server keeps its records in Postgres: the `[database] url` names
/// the database, and the state directory then holds no database file.
/// The test skips when Docker is not reachable, and says so on stderr.
#[tokio::test]
async fn a_server_keeps_its_records_in_postgres() {
    let Some(database) = pagis_testkit::postgres::database().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    set_database_url(&home, &database.url);

    let booted = pagis::boot(&home, Installation::Server).await.unwrap();

    assert!(
        !home.join("pagis.db").exists(),
        "a server keeps no database file"
    );
    let seeded = booted.stores.workspaces.list().await.unwrap();
    assert_eq!(seeded.len(), 1, "the seed ran against Postgres");
    // The rows are in the database the setting named, not in the state
    // directory, so a second reader of that database sees them.
    let workspaces: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspaces WHERE user_id IS NOT NULL")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(workspaces, 1);
}

/// A server with no database URL stops at boot, before it writes a
/// database file, and the error names the variable that sets one.
#[tokio::test]
async fn a_server_with_no_database_url_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");

    let error = pagis::boot(&home, Installation::Server)
        .await
        .expect_err("a server does not run on SQLite")
        .to_string();

    assert!(error.contains("PAGIS_DATABASE_URL"), "{error}");
    assert!(
        !home.join("pagis.db").exists(),
        "the boot made no SQLite file"
    );
}

/// A local installation keeps its records in SQLite, whether one person
/// uses it or several. A database URL stops it at boot, before it
/// connects to anything.
#[tokio::test]
async fn a_local_installation_with_a_database_url_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    set_database_url(&home, "postgres://pagis@db.invalid/pagis");

    let error = pagis::boot(&home, Installation::Local)
        .await
        .expect_err("a local installation does not run on Postgres")
        .to_string();

    assert!(error.contains("PAGIS_DATABASE_URL"), "{error}");
    assert!(
        !home.join("pagis.db").exists(),
        "the boot made no SQLite file"
    );
}

/// Write `[database] url` into the configuration file the boot reads.
fn set_database_url(home: &std::path::Path, url: &str) {
    std::fs::create_dir_all(home).unwrap();
    let config = home.join("config.toml");
    let mut settings = pagis::Config::read_file(&config).unwrap();
    settings.database.url = url.to_string();
    settings.save(&config).unwrap();
}

/// A second pool on the database a boot opened, for the assertions that
/// read a column no store method answers. `Booted` names no pool type,
/// because the backend is a setting, and WAL carries a second reader
/// beside the daemon's own pool.
async fn rows(booted: &pagis::Booted) -> sqlx::SqlitePool {
    pagis_storage_sqlite::connect(&booted.home.join("pagis.db"))
        .await
        .expect("open the test database")
}

/// A state directory that other users can enter, for example one that
/// the Client App made, is private after the boot. The boot changes
/// the mode and starts: the owner does nothing.
#[tokio::test]
async fn boot_makes_an_open_state_directory_private() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();

    pagis::boot(&home, Installation::Local).await.unwrap();

    assert_eq!(mode(&home), 0o700);
}

/// A Client Credential that another user can read gives that user a
/// Session of the Administrator, so the boot refuses it, as it refuses
/// a Key File that others can read.
#[tokio::test]
async fn boot_refuses_a_client_credential_that_others_can_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::create_dir(&home).unwrap();
    let credential = home.join("client-credential");
    std::fs::write(&credential, "a".repeat(64)).unwrap();
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o644)).unwrap();

    let error = pagis::boot(&home, Installation::Local)
        .await
        .expect_err("a credential that others can read stops the boot")
        .to_string();

    assert!(error.contains("mode 644"), "{error}");
    assert!(error.contains("client-credential"), "{error}");
}

/// A Client Credential file that holds no credential, for example the
/// empty file of a write that stopped, is refused. An empty credential
/// would match an empty one in the trade.
#[tokio::test]
async fn boot_refuses_a_client_credential_file_that_holds_no_credential() {
    for contents in [String::new(), "abc".to_string(), "z".repeat(64)] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("pagis-home");
        std::fs::create_dir(&home).unwrap();
        let credential = home.join("client-credential");
        std::fs::write(&credential, &contents).unwrap();
        std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).unwrap();

        let error = pagis::boot(&home, Installation::Local)
            .await
            .expect_err("a file with no credential stops the boot")
            .to_string();

        assert!(error.contains("64 hexadecimal"), "{contents:?}: {error}");
    }
}

/// The Client Credential is mode 600 from the moment its file exists,
/// so no other user can open it before a later mode change. The shell
/// sets a file size limit of zero, so the write after the open fails,
/// and the file stays as the open made it. The files that the start
/// writes before the credential are there already.
#[test]
fn the_client_credential_is_private_from_its_first_open() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"), "").unwrap();
    std::fs::write(
        home.join("runtime-release"),
        format!("{}\n", pagis_server::VERSION),
    )
    .unwrap();

    let output = pagis_after("umask 022; ulimit -f 0; trap '' XFSZ")
        .args(["--local", "--no-open", "--port", "0"])
        .env("PAGIS_HOME", &home)
        .env("PAGIS_ADMINISTRATION_PORT", "0")
        .stdin(Stdio::null())
        .output()
        .expect("the pagis binary starts");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "the start stops: {stderr}");
    assert_eq!(
        mode(&home.join("client-credential")),
        0o600,
        "the open made a file that others can read: {stderr}"
    );
    assert!(
        stderr.contains("write the client credential"),
        "the start stops at the write of the credential: {stderr}"
    );
}

/// The daemon keeps every file that it writes from the other OS users of
/// the machine, whatever the umask of the process that starts it. The
/// state directory is mode 755 before the start, as the Client App can
/// make it.
///
/// A provider key seals `secrets.enc`, and the name that the onboarding
/// records is the first memory file. The key is not real, and the proxy
/// variables send the model list request that it starts to a closed
/// port, so the daemon reaches no provider.
#[tokio::test]
async fn a_daemon_under_umask_022_writes_no_file_that_others_can_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("pagis-home");
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&home)
        .unwrap();
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = pagis_after("umask 022");
    for variable in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "NO_PROXY",
        "no_proxy",
    ] {
        command.env_remove(variable);
    }
    for variable in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        command.env(variable, "http://127.0.0.1:1");
    }

    let (daemon, origin) = crate::shutdown::started(command, &home);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let cookie = client_session(&client, &origin, &home).await;
    let key = client
        .put(format!(
            "{origin}/api/v1/settings/onboarding/providers/anthropic/key"
        ))
        .header("cookie", &cookie)
        .json(&serde_json::json!({ "key": "sk-ant-not-a-real-key" }))
        .send()
        .await
        .expect("the daemon stores the key");
    assert_eq!(
        key.status(),
        200,
        "{}",
        key.text().await.unwrap_or_default()
    );
    let onboarding = client
        .post(format!("{origin}/api/v1/settings/onboarding/complete"))
        .header("cookie", &cookie)
        .json(&serde_json::json!({ "user_name": "Ada" }))
        .send()
        .await
        .expect("the daemon completes the onboarding");
    assert_eq!(
        onboarding.status(),
        204,
        "{}",
        onboarding.text().await.unwrap_or_default()
    );
    let status = crate::shutdown::stop(daemon, "-TERM");
    assert_eq!(status.code(), Some(0), "{status}");

    let entries = tree(&home);
    for (what, part) in [
        ("the database", home.join("pagis.db")),
        ("the sealed secrets", home.join("secrets.enc")),
        ("a memory file", home.join("memory")),
        ("a log file", home.join("logs")),
    ] {
        assert!(
            entries
                .iter()
                .any(|path| path.starts_with(&part) && path.is_file()),
            "the daemon wrote no file for {what}: {entries:#?}"
        );
    }
    let open: Vec<String> = entries
        .iter()
        .filter(|path| mode(path) & 0o077 != 0)
        .map(|path| format!("{} {:o}", path.display(), mode(path)))
        .collect();
    assert!(open.is_empty(), "other users can read: {open:#?}");
    assert_eq!(mode(&home), 0o700, "the state directory is private");
}

/// Trade the Client Credential for a Session, as the Client App does,
/// and answer the Session cookie.
async fn client_session(client: &reqwest::Client, origin: &str, home: &Path) -> String {
    let credential = std::fs::read_to_string(home.join("client-credential"))
        .expect("the daemon wrote the Client Credential");
    let response = client
        .post(format!("{origin}/api/v1/sessions/client"))
        .json(&serde_json::json!({ "credential": credential.trim() }))
        .send()
        .await
        .expect("the daemon answers the trade");
    assert_eq!(response.status(), 200, "the trade gives a Session");
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("a cookie header of text"))
        .map(|value| value.split(';').next().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The pagis binary, started by `sh` after `setup`, for example
/// `umask 022`. The arguments of the command go to pagis.
pub(crate) fn pagis_after(setup: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(format!("{setup}; exec \"$0\" \"$@\""))
        .arg(env!("CARGO_BIN_EXE_pagis"));
    command
}

/// The permission bits of one path. A symbolic link gives its own bits.
pub(crate) fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

/// Every path under `root`, `root` too.
pub(crate) fn tree(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.to_path_buf()];
    let mut next = 0;
    while next < paths.len() {
        let path = paths[next].clone();
        next += 1;
        if std::fs::symlink_metadata(&path).unwrap().is_dir() {
            for entry in std::fs::read_dir(&path).unwrap() {
                paths.push(entry.unwrap().path());
            }
        }
    }
    paths
}
