//! Backup and restore of a whole installation.
//!
//! The claim the deployment procedure rests on is that a state
//! directory and its database, taken together while the daemon is
//! stopped, are the whole installation. This proves it end to end and
//! on both backends: a person and a sprite's memory go into one
//! installation, an archive comes out, and the archive brings both back
//! in a fresh state directory with a fresh database.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis::backup::{DatabaseKind, Installation, Tools};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message;

const EMAIL: &str = "ada@example.net";
const PASSWORD: &str = "correct horse battery";
const FACT: &str = "- [User](user.md) — birthday is 3 May\n";

/// A brain that writes one fact file and answers, which is how a sprite
/// puts something in memory.
fn remembering_brain() -> Arc<ScriptedBrain> {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({
            "path": "shared/MEMORY.md",
            "content": FACT,
            "expected_memory_revision": "missing",
        }),
    ));
    brain.push(Script::reply(&["Noted!"]));
    brain
}

/// Tell the sprite the fact and wait until the Run that writes it ends.
async fn remember(daemon: &TestDaemon) {
    let (mut socket, _) = tokio_tungstenite::connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("ws send");
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": "p1", "text": "my birthday is 3 May" }))
        .send()
        .await
        .expect("send the message");
    assert_eq!(response.status(), 201);
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a frame before the deadline")
            .expect("the socket stays open")
            .expect("the frame arrives");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("the frame is JSON");
        if frame["type"] == "run.state_changed" && frame["payload"]["payload"]["to"] == "completed"
        {
            return;
        }
    }
}

/// Give the administrator the address and the password a person on a
/// server signs in with.
async fn give_a_password(daemon: &TestDaemon) {
    daemon
        .stores()
        .users
        .set_email_and_password(
            &daemon.user_id,
            EMAIL,
            &pagis_server::hash_password(PASSWORD).expect("hash the password"),
            pagis_core::now_ms(),
        )
        .await
        .expect("write the address and the password");
}

/// Sign in with the address and the password, as a person on a server
/// does, and answer the Session cookie.
async fn sign_in(base_url: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{base_url}/api/v1/sessions"))
        .json(&serde_json::json!({ "email": EMAIL, "password": PASSWORD }))
        .send()
        .await
        .expect("sign in");
    assert_eq!(response.status(), 200, "the restored person signs in");
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("a cookie header of text"))
        .map(|value| value.split(';').next().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The fact file as the product interface reads it.
async fn read_memory(base_url: &str, cookie: &str) -> String {
    let response = reqwest::Client::new()
        .get(format!(
            "{base_url}/api/v1/memory/file?scope=shared&path=MEMORY.md"
        ))
        .header("cookie", cookie)
        .send()
        .await
        .expect("read the memory file");
    assert_eq!(response.status(), 200, "the memory file is there");
    let body: serde_json::Value = response.json().await.expect("a JSON body");
    body["content"]
        .as_str()
        .expect("the file content")
        .to_string()
}

/// Prove the restored installation holds the person and the memory.
async fn prove(daemon: &TestDaemon) {
    let cookie = sign_in(&daemon.base_url).await;
    assert!(
        read_memory(&daemon.base_url, &cookie)
            .await
            .contains(FACT.trim())
    );
}

#[tokio::test]
async fn a_sqlite_installation_comes_back_with_its_person_and_its_memory() {
    let source = TempDir::new().expect("a state directory");
    let daemon = TestDaemon::start_on(
        source,
        TestDaemonOptions {
            brain: remembering_brain() as _,
            ..TestDaemonOptions::default()
        },
    )
    .await;
    give_a_password(&daemon).await;
    remember(&daemon).await;
    prove(&daemon).await;
    let source = daemon.stop().await;

    let archive = TempDir::new().expect("a directory for the archive");
    let manifest = Installation::at(source.path())
        .expect("read the installation")
        .back_up(&archive.path().join("pagis"))
        .expect("back the installation up");
    assert_eq!(manifest.database, DatabaseKind::Sqlite);

    // A fresh host: a state directory nothing has written and no
    // database of its own, because SQLite lives in the directory.
    let fresh = TempDir::new().expect("a fresh state directory");
    Installation::from_archive(&archive.path().join("pagis"), fresh.path())
        .expect("read the archive")
        .restore(&archive.path().join("pagis"))
        .expect("restore the installation");

    let restored = TestDaemon::start_on(fresh, TestDaemonOptions::default()).await;
    prove(&restored).await;
}

/// The same proof for a server, where Postgres holds the records and
/// the state directory holds the files. The database is dumped
/// and restored by the PostgreSQL client, and the restore lands in a
/// database that was empty a moment before.
#[tokio::test]
async fn a_postgres_installation_comes_back_with_its_person_and_its_memory() {
    let Some(cluster) = pagis_testkit::postgres::Cluster::start().await else {
        return;
    };
    let database = cluster.database().await;
    let tools = Tools {
        pg_dump: cluster.client().pg_dump.clone(),
        pg_restore: cluster.client().pg_restore.clone(),
    };

    let source = TempDir::new().expect("a state directory");
    let daemon = TestDaemon::start_on(
        source,
        TestDaemonOptions {
            installation: pagis::Installation::Server,
            brain: remembering_brain() as _,
            database: Some(database.url.clone()),
            ..TestDaemonOptions::default()
        },
    )
    .await;
    give_a_password(&daemon).await;
    remember(&daemon).await;
    prove(&daemon).await;
    let source = daemon.stop().await;

    let archive = TempDir::new().expect("a directory for the archive");
    let out = archive.path().join("pagis");
    let manifest = Installation {
        home: source.path().to_path_buf(),
        database: Some(database.inside.clone()),
        tools: tools.clone(),
    }
    .back_up(&out)
    .expect("back the installation up");
    assert_eq!(manifest.database, DatabaseKind::Postgres);
    assert!(out.join("database.dump").exists());

    // A fresh host: an empty state directory and an empty database.
    let restored_database = cluster.database().await;
    let fresh = TempDir::new().expect("a fresh state directory");
    Installation {
        home: fresh.path().to_path_buf(),
        database: Some(restored_database.inside.clone()),
        tools,
    }
    .restore(&out)
    .expect("restore the installation");

    let restored = TestDaemon::start_on(
        fresh,
        TestDaemonOptions {
            installation: pagis::Installation::Server,
            database: Some(restored_database.url.clone()),
            ..TestDaemonOptions::default()
        },
    )
    .await;
    prove(&restored).await;
}

/// A Backup holds no Model Request Capture (ADR-0031): the archive's
/// copy of the database has no row of the table and no byte of a
/// capture, also not in a free page or in the write-ahead log.
#[tokio::test]
async fn a_backup_leaves_the_model_request_captures_out() {
    const MARKER: &str = "captured-7713-marker";
    let source = TempDir::new().expect("a state directory");
    let daemon = TestDaemon::start_on(source, TestDaemonOptions::default()).await;
    let mut channel = pagis_testkit::fixture::channel(&daemon.workspace_id);
    channel.title = Some("captures".into());
    daemon.stores().channels.create(&channel).await.unwrap();
    let run = pagis_testkit::fixture::queued_run(
        &daemon.workspace_id,
        &pagis_core::AgentId::from(daemon.agent_id.clone()),
        &channel.id,
    );
    daemon.stores().runs.create(&run).await.unwrap();
    daemon
        .stores()
        .model_request_captures
        .record(&pagis_core::ModelRequestCapture {
            id: pagis_core::ModelRequestCaptureId::generate(),
            workspace_id: daemon.workspace_id.clone(),
            run_id: run.id.clone(),
            phase: "reply".into(),
            phase_request: 0,
            request: serde_json::json!({ "system": MARKER }),
            answer: serde_json::json!({ "outcome": "completed" }),
            created_at: pagis_core::now_ms(),
        })
        .await
        .unwrap();
    let source = daemon.stop().await;

    let archive = TempDir::new().expect("a directory for the archive");
    Installation::at(source.path())
        .expect("read the installation")
        .back_up(&archive.path().join("pagis"))
        .expect("back the installation up");

    let state = archive.path().join("pagis/state");
    for entry in std::fs::read_dir(&state).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(MARKER.len())
                    .any(|window| window == MARKER.as_bytes()),
                "{} holds a capture",
                path.display()
            );
        }
    }
    // The installation itself keeps its capture.
    let fresh = TempDir::new().expect("a fresh state directory");
    Installation::from_archive(&archive.path().join("pagis"), fresh.path())
        .expect("read the archive")
        .restore(&archive.path().join("pagis"))
        .expect("restore the installation");
    let restored = TestDaemon::start_on(fresh, TestDaemonOptions::default()).await;
    assert!(
        restored
            .stores()
            .model_request_captures
            .list_for_run(&restored.workspace_id, &run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A Backup holds no Computer token. The daemon writes a new token at
/// each start of a Computer, so a restored token opens nothing. While
/// its Computer runs, the token is live, and an archive is copied and
/// kept.
#[test]
fn a_backup_leaves_the_computer_tokens_behind() {
    let dir = TempDir::new().expect("a directory");
    let home = dir.path().join("pagis-home");
    std::fs::create_dir_all(home.join("computer-tokens/w1")).expect("the token directory");
    std::fs::create_dir_all(home.join("memory/w1")).expect("the memory directory");
    for (name, contents) in [
        ("config.toml", ""),
        ("runtime-release", "0.1.0\n"),
        // An empty file is an empty SQLite database.
        ("pagis.db", ""),
        ("memory/w1/note.md", "remembered"),
        ("computer-tokens/w1/a1.token", "a live token"),
    ] {
        std::fs::write(home.join(name), contents).expect("a file of the state directory");
    }
    let archive = dir.path().join("archive");

    Installation::at(&home)
        .expect("read the installation")
        .back_up(&archive)
        .expect("back the installation up");

    let entries = crate::boot::tree(&archive);
    assert!(
        entries
            .iter()
            .any(|path| path.ends_with("state/memory/w1/note.md")),
        "the Backup holds the memory: {entries:#?}"
    );
    let tokens: Vec<_> = entries
        .iter()
        .filter(|path| path.to_string_lossy().contains("computer-tokens"))
        .collect();
    assert!(tokens.is_empty(), "the Backup holds tokens: {tokens:#?}");
    assert!(
        home.join("computer-tokens/w1/a1.token").exists(),
        "the backup leaves the token of the running Computer in place"
    );
}

/// A Backup is readable by its owner alone, whatever the umask of the
/// shell that takes it and whatever the modes of the files it copies.
/// The files here have modes that let other users read them.
#[test]
fn a_backup_under_umask_022_is_private() {
    use std::os::unix::fs::PermissionsExt;

    use crate::boot::{mode, pagis_after, tree};

    let dir = TempDir::new().expect("a directory");
    let home = dir.path().join("pagis-home");
    std::fs::create_dir_all(home.join("memory/w1")).expect("the memory directory");
    for (name, contents) in [
        ("config.toml", ""),
        ("runtime-release", "0.1.0\n"),
        // An empty file is an empty SQLite database.
        ("pagis.db", ""),
        ("memory/w1/note.md", "remembered"),
    ] {
        std::fs::write(home.join(name), contents).expect("a file of the state directory");
        std::fs::set_permissions(home.join(name), std::fs::Permissions::from_mode(0o644))
            .expect("a mode that other users can read");
    }
    for directory in ["", "memory", "memory/w1"] {
        std::fs::set_permissions(home.join(directory), std::fs::Permissions::from_mode(0o755))
            .expect("a mode that other users can read");
    }
    let backup = dir.path().join("backups/nightly");

    let output = pagis_after("umask 022")
        .arg("backup")
        .arg(&backup)
        .env("PAGIS_HOME", &home)
        .output()
        .expect("the pagis binary starts");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(mode(&backup), 0o700, "the Backup root is private");
    let entries = tree(&backup);
    assert!(
        entries
            .iter()
            .any(|path| path.ends_with("state/memory/w1/note.md")),
        "the Backup holds the memory: {entries:#?}"
    );
    let open: Vec<String> = entries
        .iter()
        .filter(|path| mode(path) & 0o077 != 0)
        .map(|path| format!("{} {:o}", path.display(), mode(path)))
        .collect();
    assert!(open.is_empty(), "other users can read: {open:#?}");
}
