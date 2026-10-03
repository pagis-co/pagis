//! The machines a person's clients run on, on both backends.
//!
//! The register method is the whole of the write path, so most bodies
//! here prove what a second registration of one machine does. Name every
//! new body in `store_suite_hosts!` below; the guard test of the parent
//! module fails while one is missing.

use pagis_core::{EXIT_CAPABILITY, HostId, SHELL_CAPABILITY, Workspace, WorkspaceId};

use super::Backend;

fn shell() -> Vec<String> {
    vec![SHELL_CAPABILITY.to_string()]
}

/// A second Workspace of the same Org, for the reads that must not cross
/// one.
async fn second_workspace(backend: &Backend, first: &Workspace) -> Workspace {
    let second = Workspace {
        id: WorkspaceId::generate(),
        ..first.clone()
    };
    backend
        .stores()
        .workspaces
        .create(&second)
        .await
        .expect("write the second Workspace");
    second
}

pub async fn a_registered_host_reads_back_with_its_capabilities(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let hosts = &backend.stores().hosts;

    let host = hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();

    let read = hosts.get(&workspace.id, &host.id).await.unwrap().unwrap();
    assert_eq!(read, host);
    assert_eq!(read.name, "Air");
    assert_eq!(read.platform, "macos");
    assert_eq!(read.capabilities, shell());
    assert_eq!(read.last_seen_at, 1_000);
    assert_eq!(read.created_at, 1_000);
    assert!(read.can(SHELL_CAPABILITY));
}

/// The same machine that connects again is the same Host, so a Grant
/// that names it survives a restart of the client. The client is the
/// authority for the platform and the capabilities, so both are replaced.
pub async fn the_same_machine_registers_onto_one_record(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let hosts = &backend.stores().hosts;

    let first = hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();
    let second = hosts
        .register(&workspace.id, "Air", "macos", &[], 2_000)
        .await
        .unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(second.created_at, 1_000);
    assert_eq!(second.last_seen_at, 2_000);
    assert!(second.capabilities.is_empty());
    assert_eq!(hosts.list(&workspace.id).await.unwrap().len(), 1);
}

pub async fn two_machines_of_one_person_are_two_hosts(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let hosts = &backend.stores().hosts;

    let air = hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();
    let phone = hosts
        .register(&workspace.id, "Phone", "ios", &[], 2_000)
        .await
        .unwrap();

    let listed = hosts.list(&workspace.id).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|host| host.id.clone())
            .collect::<Vec<_>>(),
        vec![air.id, phone.id]
    );
    assert!(!listed[1].can(SHELL_CAPABILITY));
}

/// One person never reads another person's machine. The read names the
/// Workspace, so a Host of another Workspace is absent and no handler has
/// to remember to compare.
pub async fn one_workspace_never_reads_another_workspace_host(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let hosts = &backend.stores().hosts;

    let mine = hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();

    assert_eq!(hosts.get(&other.id, &mine.id).await.unwrap(), None);
    assert!(hosts.list(&other.id).await.unwrap().is_empty());
}

/// Two people may call their machines the same thing: the name is an
/// identity inside one Workspace and not across the installation.
pub async fn two_people_may_hold_a_machine_of_one_name(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let hosts = &backend.stores().hosts;

    let mine = hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();
    let theirs = hosts
        .register(&other.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();

    assert_ne!(mine.id, theirs.id);
    assert_eq!(hosts.list(&workspace.id).await.unwrap(), vec![mine]);
    assert_eq!(hosts.list(&other.id).await.unwrap(), vec![theirs]);
}

pub async fn the_last_seen_time_moves_and_a_missing_host_refuses(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let hosts = &backend.stores().hosts;
    let host = hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();

    assert!(hosts.touch(&host.id, 5_000).await.unwrap());
    assert!(!hosts.touch(&HostId::generate(), 5_000).await.unwrap());

    let read = hosts.get(&workspace.id, &host.id).await.unwrap().unwrap();
    assert_eq!(read.last_seen_at, 5_000);
}

/// The Home Exit of a Workspace (ADR-0029): the Person names one of their
/// own Hosts, the Workspace reads it back, and the Person clears it.
pub async fn a_person_names_one_of_their_hosts_as_the_home_exit_and_clears_it(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let stores = backend.stores();
    let air = stores
        .hosts
        .register(
            &workspace.id,
            "Air",
            "macos",
            &[SHELL_CAPABILITY.to_string(), EXIT_CAPABILITY.to_string()],
            1_000,
        )
        .await
        .unwrap();
    let home_exit = async || {
        stores
            .workspaces
            .get(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .home_exit_host_id
    };
    assert_eq!(home_exit().await, None);

    assert!(
        stores
            .workspaces
            .set_home_exit(&workspace.id, Some(&air.id))
            .await
            .unwrap()
    );
    assert_eq!(home_exit().await, Some(air.id.clone()));
    // A second registration of the machine keeps it the Home Exit: the
    // record is the same Host.
    stores
        .hosts
        .register(&workspace.id, "Air", "macos", &[], 2_000)
        .await
        .unwrap();
    assert_eq!(home_exit().await, Some(air.id.clone()));

    assert!(
        stores
            .workspaces
            .set_home_exit(&workspace.id, None)
            .await
            .unwrap()
    );
    assert_eq!(home_exit().await, None);
}

/// A Person's Computers never leave through another Person's machine, so
/// the store never names a Host of another Workspace, or a Host that does
/// not exist, as the Home Exit. The refused write changes nothing.
pub async fn a_host_of_another_person_is_never_the_home_exit(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let other = second_workspace(backend, &workspace).await;
    let stores = backend.stores();
    let mine = stores
        .hosts
        .register(&workspace.id, "Air", "macos", &shell(), 1_000)
        .await
        .unwrap();
    let theirs = stores
        .hosts
        .register(&other.id, "Their Air", "macos", &shell(), 1_000)
        .await
        .unwrap();
    assert!(
        stores
            .workspaces
            .set_home_exit(&workspace.id, Some(&mine.id))
            .await
            .unwrap()
    );

    for host in [theirs.id.clone(), HostId::generate()] {
        assert!(
            !stores
                .workspaces
                .set_home_exit(&workspace.id, Some(&host))
                .await
                .unwrap(),
            "the store named {host} as the Home Exit"
        );
    }
    assert!(
        !stores
            .workspaces
            .set_home_exit(&WorkspaceId::generate(), Some(&mine.id))
            .await
            .unwrap()
    );

    let read = stores.workspaces.get(&workspace.id).await.unwrap().unwrap();
    assert_eq!(read.home_exit_host_id, Some(mine.id));
    let theirs_read = stores.workspaces.get(&other.id).await.unwrap().unwrap();
    assert_eq!(theirs_read.home_exit_host_id, None);
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_hosts {
    ($emit:path) => {
        $emit!(
            hosts,
            a_registered_host_reads_back_with_its_capabilities,
            the_same_machine_registers_onto_one_record,
            two_machines_of_one_person_are_two_hosts,
            one_workspace_never_reads_another_workspace_host,
            two_people_may_hold_a_machine_of_one_name,
            the_last_seen_time_moves_and_a_missing_host_refuses,
            a_person_names_one_of_their_hosts_as_the_home_exit_and_clears_it,
            a_host_of_another_person_is_never_the_home_exit,
        );
    };
}
