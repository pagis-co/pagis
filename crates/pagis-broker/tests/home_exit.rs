//! Only a Host of the Computer's own Person carries its connections
//! (ADR-0029), on the two-Workspace harness.
//!
//! The Home Exit is one Host of the Person, which the Workspace names, and
//! a Host is present as a Home Exit while its exit socket lives. These
//! tests hold the Hosts of two People with open exit sockets, over the
//! SQLite stores of the harness, and ask the Home Exits of the daemon for
//! a connection of one Person's Computer. The Client App's end of each
//! socket is the fake Home Exit of `pagis_computer`, which speaks the
//! protocol of the Client App.

use std::sync::Arc;
use std::time::Duration;

use pagis_computer::HomeExits;
use pagis_computer::fake::{FakeHomeExit, exit_socket_pair};
use pagis_core::{EXIT_CAPABILITY, Host, HostStore, SHELL_CAPABILITY, WorkspaceId, WorkspaceStore};
use pagis_storage_sqlite::SqliteWorkspaceStore;
use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::capability_broker::{Harness, harness, second_workspace};

/// A target of the test on loopback, which greets each connection.
async fn target() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            stream.write_all(b"hello from the target\n").await.ok();
        }
    });
    address
}

/// One Host of `workspace_id` that declares `exit`.
async fn exit_host(harness: &Harness, workspace_id: &WorkspaceId, name: &str) -> Host {
    harness
        .hosts()
        .register(
            workspace_id,
            name,
            "macos",
            &[SHELL_CAPABILITY.to_string(), EXIT_CAPABILITY.to_string()],
            pagis_core::now_ms(),
        )
        .await
        .unwrap()
}

/// Open the exit socket of `host`, as a Session of its own Workspace
/// opens it, with `exit` at the Client App's end.
async fn open_exit(exits: &Arc<HomeExits>, host: &Host, exit: Arc<FakeHomeExit>) {
    let (daemon_end, client_app_end) = exit_socket_pair();
    let serving = Arc::clone(exits);
    let (workspace_id, host_id, host_name) = (
        host.workspace_id.clone(),
        host.id.clone(),
        host.name.clone(),
    );
    tokio::spawn(async move {
        serving
            .serve(workspace_id, host_id, host_name, daemon_end)
            .await
    });
    tokio::spawn(exit.serve(client_app_end));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !exits.is_open(&host.id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the exit socket did not open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The other Person's Host has an open exit socket and is that Person's
/// Home Exit. It carries nothing for this Person's Computer: not while
/// this Person has no Home Exit, not when this Person names it (the store
/// refuses), and not when a row names it anyway (the socket belongs to
/// another Workspace). The connection then leaves from the server. The
/// same Host still carries its own Person's connections.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_of_another_person_carries_nothing_for_this_computer(pool: SqlitePool) {
    let harness = harness(pool).await;
    let other = second_workspace(&harness).await;
    let workspaces = Arc::new(SqliteWorkspaceStore::new(harness.pool.clone()));
    let exits = HomeExits::new(Arc::clone(&workspaces) as _);
    let theirs = exit_host(&harness, &other.id, "Their Air").await;
    assert!(
        workspaces
            .set_home_exit(&other.id, Some(&theirs.id))
            .await
            .unwrap()
    );
    let their_exit = FakeHomeExit::to(target().await);
    open_exit(&exits, &theirs, Arc::clone(&their_exit)).await;

    assert!(
        exits
            .open(&harness.workspace.id, "example.com", 443)
            .await
            .is_none()
    );

    assert!(
        !workspaces
            .set_home_exit(&harness.workspace.id, Some(&theirs.id))
            .await
            .unwrap(),
        "the store named another Person's Host as this Person's Home Exit"
    );
    assert!(
        exits
            .open(&harness.workspace.id, "example.com", 443)
            .await
            .is_none()
    );

    // A row that names the other Person's Host anyway, as a fault of a
    // store would.
    sqlx::query("UPDATE workspaces SET home_exit_host_id = ? WHERE id = ?")
        .bind(theirs.id.as_str())
        .bind(harness.workspace.id.as_str())
        .execute(&harness.pool)
        .await
        .unwrap();
    assert_eq!(
        workspaces
            .get(&harness.workspace.id)
            .await
            .unwrap()
            .unwrap()
            .home_exit_host_id,
        Some(theirs.id.clone())
    );
    assert!(
        exits
            .open(&harness.workspace.id, "example.com", 443)
            .await
            .is_none()
    );
    assert!(
        their_exit.destinations().is_empty(),
        "another Person's Host carried this Person's connection"
    );
    assert_eq!(exits.bytes(&other.id), Default::default());

    let mut carried = exits
        .open(&other.id, "example.org", 443)
        .await
        .expect("their Home Exit is present")
        .expect("their Home Exit carries their connection");
    let mut greeting = [0; 22];
    carried.read_exact(&mut greeting).await.unwrap();
    assert_eq!(&greeting, b"hello from the target\n");
    assert_eq!(their_exit.destinations(), ["example.org:443"]);
}

/// This Person's own Host carries this Person's connections once they
/// name it, and from then on the other Person's open exit socket changes
/// nothing.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn this_persons_own_host_carries_their_connections(pool: SqlitePool) {
    let harness = harness(pool).await;
    let other = second_workspace(&harness).await;
    let workspaces = Arc::new(SqliteWorkspaceStore::new(harness.pool.clone()));
    let exits = HomeExits::new(Arc::clone(&workspaces) as _);
    let mine = exit_host(&harness, &harness.workspace.id, "Air").await;
    let theirs = exit_host(&harness, &other.id, "Air").await;
    let my_exit = FakeHomeExit::to(target().await);
    let their_exit = FakeHomeExit::to(target().await);
    open_exit(&exits, &mine, Arc::clone(&my_exit)).await;
    open_exit(&exits, &theirs, Arc::clone(&their_exit)).await;

    assert!(
        workspaces
            .set_home_exit(&harness.workspace.id, Some(&mine.id))
            .await
            .unwrap()
    );
    let mut carried = exits
        .open(&harness.workspace.id, "shop.example.com", 443)
        .await
        .expect("my Home Exit is present")
        .expect("my Home Exit carries my connection");
    let mut greeting = [0; 22];
    carried.read_exact(&mut greeting).await.unwrap();

    assert_eq!(my_exit.destinations(), ["shop.example.com:443"]);
    assert!(their_exit.destinations().is_empty());
}
