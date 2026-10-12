use super::*;
use pagis_core::computer_action::InputOp;

#[tokio::test]
async fn every_operation_records_its_workspace_agent_and_arguments() {
    let computers = FakeComputers::default();
    let workspace = WorkspaceId::generate();
    let other = WorkspaceId::generate();
    let agent = AgentId::generate();
    let computer = computers.get(&workspace);
    assert!(Arc::ptr_eq(&computer, &computers.get(&workspace)));
    let another = computers.get(&other);
    assert!(!Arc::ptr_eq(&computer, &another));
    assert_eq!(
        computers.get_calls(),
        vec![workspace.clone(), workspace.clone(), other.clone()]
    );
    computers.set_frame(vec![1, 2]);
    computers.set_exit(Some(ExitInUse::HomeExit("Home".into())));
    computers.set_download("/data/package", b"tar".to_vec());
    let lease = computer.lease(&agent).await.unwrap();
    drop(lease);
    computer.clone().ensure_awake(&agent).await.unwrap();
    computer
        .clone()
        .upload_archive(&agent, "/data/package", b"upload".to_vec())
        .await
        .unwrap();
    let mut file = computer
        .clone()
        .download_archive(&agent, "/data/package")
        .await
        .unwrap();
    let mut tar = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut tar).unwrap();
    assert_eq!(tar, b"tar");
    assert_eq!(computer.live_frame(&agent).await.unwrap(), vec![1, 2]);
    assert_eq!(computer.settled_frame(&agent).await.unwrap(), vec![1, 2]);
    let ops = vec![InputOp::Text {
        text: "hello".into(),
    }];
    computer.input(&agent, &ops).await.unwrap();
    assert_eq!(
        computer.exit_in_use(&agent).await,
        Some(ExitInUse::HomeExit("Home".into()))
    );
    another.input(&agent, &ops).await.unwrap();
    let calls = computers.calls();
    assert!(
        calls[..8]
            .iter()
            .all(|call| call.workspace_id == workspace && call.agent_id == agent)
    );
    assert_eq!(calls[8].workspace_id, other);
    assert_eq!(
        calls[2].call,
        ComputerCall::Upload {
            path: "/data/package".into(),
            tar: b"upload".to_vec()
        }
    );
    assert_eq!(calls[6].call, ComputerCall::Input(ops));
}

#[tokio::test]
async fn a_daemon_hold_denies_agent_capture_and_input_until_it_drops() {
    let computers = FakeComputers::default();
    computers.set_frame(vec![1]);
    let computer = computers.get(&WorkspaceId::generate());
    let agent = AgentId::generate();
    let hold = computer.clone().daemon_hold(&agent).await.unwrap();
    assert!(matches!(
        computer.lease(&agent).await,
        Err(ComputerError::SwitchHeld {
            holder: InputHolder::Daemon
        })
    ));
    assert!(matches!(
        computer.live_frame(&agent).await,
        Err(ComputerError::SwitchHeld { .. })
    ));
    assert!(matches!(
        computer.input(&agent, &[]).await,
        Err(ComputerError::SwitchHeld { .. })
    ));
    drop(hold);
    drop(computer.lease(&agent).await.unwrap());
    assert_eq!(computer.live_frame(&agent).await.unwrap(), vec![1]);
    assert_eq!(
        computers
            .calls()
            .iter()
            .filter(|call| call.call == ComputerCall::Release)
            .count(),
        1
    );
}

#[tokio::test]
async fn an_opaque_lease_holds_the_batch_until_it_drops() {
    use pagis_core::ComputerLease;

    let computers = FakeComputers::default();
    let computer = computers.get(&WorkspaceId::generate());
    let agent = AgentId::generate();
    let lease: ComputerLease = computer.lease(&agent).await.unwrap();
    let waiting = computer.lease(&agent);
    futures::pin_mut!(waiting);
    assert!(futures::poll!(&mut waiting).is_pending());
    drop(lease);
    let next: ComputerLease = waiting.await.unwrap();
    drop(next);
    assert_eq!(computers.calls().len(), 2);
}
