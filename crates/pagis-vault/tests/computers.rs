use std::sync::Arc;

use pagis_core::{AgentId, Computers, FillField, WorkspaceId};
use pagis_testkit::{ComputerCall, FakeComputers};

#[tokio::test]
async fn a_fill_opens_the_record_address_and_releases_the_switch() {
    let computers = FakeComputers::default();
    let workspace = WorkspaceId::generate();
    let agent = AgentId::generate();
    let computer = computers.get(&workspace);
    pagis_vault::fill::open_and_fill(
        &computer,
        &agent,
        "example.com",
        "https://login.example.com/signin",
        "alice",
        "secret",
    )
    .await
    .unwrap();
    let calls = computers.calls();
    assert!(
        calls
            .iter()
            .all(|call| call.workspace_id == workspace && call.agent_id == agent)
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.call.clone())
            .collect::<Vec<_>>(),
        vec![
            ComputerCall::EnsureAwake,
            ComputerCall::DaemonHold,
            ComputerCall::Open("https://login.example.com/signin".into()),
            ComputerCall::Fill {
                origin: "https://login.example.com".into(),
                fields: vec![FillField::text("alice"), FillField::password("secret")]
            },
            ComputerCall::Release,
        ]
    );
}

#[tokio::test]
async fn a_redirect_to_another_domain_or_http_writes_nothing_and_releases() {
    for page in [
        "https://other.example.org/login",
        "http://example.com/login",
    ] {
        let computers = FakeComputers::default();
        computers.set_page(page);
        let computer = computers.get(&WorkspaceId::generate());
        let error = pagis_vault::fill::open_and_fill(
            &computer,
            &AgentId::generate(),
            "example.com",
            "https://example.com/login",
            "alice",
            "secret",
        )
        .await;
        assert!(error.is_err());
        let calls = computers.calls();
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call.call, ComputerCall::Fill { .. }))
        );
        assert_eq!(calls.last().unwrap().call, ComputerCall::Release);
    }
}

#[tokio::test]
async fn a_code_reads_the_current_page_without_navigation() {
    let computers = Arc::new(FakeComputers::default());
    computers.set_page("https://example.com/otp");
    let computer = computers.get(&WorkspaceId::generate());
    pagis_vault::fill::fill_code(&computer, &AgentId::generate(), "example.com", "123456")
        .await
        .unwrap();
    let calls = computers.calls();
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call.call, ComputerCall::Open(_)))
    );
    assert_eq!(calls[2].call, ComputerCall::Page);
    assert_eq!(
        calls[3].call,
        ComputerCall::Fill {
            origin: "https://example.com".into(),
            fields: vec![FillField::text("123456")]
        }
    );
    assert_eq!(calls.last().unwrap().call, ComputerCall::Release);
}

#[tokio::test]
async fn a_browser_failure_releases_the_switch_and_returns_its_reason() {
    for successful_steps in [2, 3] {
        let computers = FakeComputers::default();
        for _ in 0..successful_steps {
            computers.push_answer(Ok(()));
        }
        computers.push_error(pagis_core::ComputerError::Runtime("browser refused".into()));
        let computer = computers.get(&WorkspaceId::generate());
        let agent = AgentId::generate();
        let error = pagis_vault::fill::open_and_fill(
            &computer,
            &agent,
            "example.com",
            "https://example.com/login",
            "alice",
            "secret",
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "the fill failed: browser refused");
        assert_eq!(
            computers.calls().last().unwrap().call,
            ComputerCall::Release
        );
        drop(computer.lease(&agent).await.unwrap());
    }
}
