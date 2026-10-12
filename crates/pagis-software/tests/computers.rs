use crate::support::MemorySource;
use pagis_core::{AgentId, ExecOutcome, WorkspaceId};
use pagis_software::{Materializer, SoftwareRunner};
use pagis_testkit::{ComputerCall, FakeComputers};
use std::sync::Arc;

const MANIFEST: &str = r#"
[package]
name = "weather"
description = "Weather"
[[tool]]
name = "forecast"
description = "Forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
timeout_s = 60
"#;

#[tokio::test]
async fn a_package_call_uses_the_workspace_computer_and_the_declared_command() {
    let computers = Arc::new(FakeComputers::default());
    let source = MemorySource::with_version("weather", "v1", MANIFEST, b"tar".to_vec());
    let materializer = Arc::new(Materializer::new(computers.clone(), source.clone()));
    let runner = SoftwareRunner::new(computers.clone(), materializer, source);
    computers.push_exec_outcome(ExecOutcome::default());
    computers.push_exec_outcome(ExecOutcome {
        stdout: "{}".into(),
        ..Default::default()
    });
    let workspace = WorkspaceId::generate();
    let agent = AgentId::generate();
    let args = serde_json::json!({"city": "Berlin"});
    let result = runner
        .run(&workspace, &agent, "weather", "v1", "forecast", &args)
        .await;
    assert!(!result.is_error, "{}", result.content);
    let calls = computers.calls();
    assert!(
        calls
            .iter()
            .all(|call| call.workspace_id == workspace && call.agent_id == agent)
    );
    let ComputerCall::Shell(command) = &calls.last().unwrap().call else {
        panic!("a shell call")
    };
    assert_eq!(command.timeout, std::time::Duration::from_secs(60));
    assert_eq!(command.stdin, Some(args.to_string().into_bytes()));
    assert!(command.command.contains("PAGIS_TOOL='forecast'"));
    assert_eq!(command.output_cap.unwrap().head, 256 * 1024);
}
