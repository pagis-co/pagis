//! The run contract and its failure contract.

use crate::support;

use std::sync::Arc;

use pagis_computer::ExecOutcome;
use pagis_software::{Materializer, SoftwareRunner, package_root};
use support::{MemorySource, harness};

const MANIFEST: &str = r#"
[package]
name = "weather"
description = "Weather for a city"

[[tool]]
name = "forecast"
description = "Tomorrow's forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
timeout_s = 60
"#;

struct Fixture {
    harness: support::Harness,
    runner: SoftwareRunner,
}

async fn fixture() -> Fixture {
    let harness = harness().await;
    let source = MemorySource::with_version("weather", "v1", MANIFEST, b"tar".to_vec());
    let materializer = Arc::new(Materializer::new(
        Arc::clone(&harness.managers),
        Arc::clone(&source) as _,
    ));
    // The tree is already there, so a call is one exec.
    harness.runtime.push_exec_outcome(ExecOutcome::default());
    let runner = SoftwareRunner::new(
        Arc::clone(&harness.managers),
        materializer,
        Arc::clone(&source) as _,
    );
    Fixture { harness, runner }
}

fn city() -> serde_json::Value {
    serde_json::json!({"city": "Berlin"})
}

#[tokio::test]
async fn a_json_result_on_exit_zero_is_the_result() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 0,
        stdout: "{\"high\": 21}".to_string(),
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "{\"high\": 21}");
    assert_eq!(result.metadata["tool"], "forecast");
}

#[tokio::test]
async fn a_call_is_one_process_with_the_package_root_the_arguments_and_two_variables() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        stdout: "{}".to_string(),
        ..ExecOutcome::default()
    });

    f.runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    let request = f.harness.runtime.execs().pop().expect("the call");
    let root = package_root("weather", "v1");
    assert_eq!(request.cwd, root);
    assert_eq!(request.user, "agent");
    let command = request.argv.last().expect("the command");
    assert!(command.contains("PAGIS_TOOL='forecast'"), "{command}");
    assert!(command.contains("PAGIS_PACKAGE_VERSION='v1'"), "{command}");
    assert!(
        command.contains(&format!("exec '{root}/bin/forecast.py'")),
        "{command}"
    );
    // The deadline of the manifest, wrapped by the manager.
    assert_eq!(request.argv[2], "60");
    assert_eq!(
        request.stdin,
        Some(city().to_string().into_bytes()),
        "the arguments are one JSON document on stdin"
    );
    assert_eq!(request.output_cap.head, 256 * 1024);
}

#[tokio::test]
async fn an_error_object_on_exit_zero_is_still_a_result() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        stdout: "{\"error\": \"no such city\"}".to_string(),
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(!result.is_error, "{}", result.content);
}

#[tokio::test]
async fn a_non_zero_exit_carries_the_code_and_the_two_tails() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 3,
        stdout: "half a line".to_string(),
        stderr: "Traceback: boom".to_string(),
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("exit code: 3"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Traceback: boom"),
        "{}",
        result.content
    );
    assert!(result.content.contains("half a line"), "{}", result.content);
}

#[tokio::test]
async fn stdout_that_is_not_json_is_an_error() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        stdout: "the weather is fine".to_string(),
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("not one JSON value"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn stdout_over_the_cap_is_an_error_that_asks_for_less() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        stdout: "{\"a\": 1}".to_string(),
        truncated: true,
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.contains("return less"), "{}", result.content);
}

#[tokio::test]
async fn a_deadline_is_an_error_that_names_it() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 124,
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("deadline of 60 s"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn a_refused_exec_is_an_error() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 126,
        stdout: "exec: permission denied".to_string(),
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("exit code: 126"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn a_tool_the_package_does_not_declare_is_refused() {
    let f = fixture().await;

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "history",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("invalid_request"));
}

#[tokio::test]
async fn a_version_the_store_does_not_hold_is_refused() {
    let f = fixture().await;

    let result = f
        .runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v9",
            "forecast",
            &city(),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("temporarily_unavailable"));
}

#[tokio::test]
async fn one_failed_call_is_never_retried() {
    let f = fixture().await;
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 1,
        ..ExecOutcome::default()
    });
    let before = f.harness.runtime.execs().len();

    f.runner
        .run(
            &f.harness.workspace_id,
            &f.harness.agent_id,
            "weather",
            "v1",
            "forecast",
            &city(),
        )
        .await;

    // The readiness check and the one process, and nothing after the
    // failure: the daemon never retries.
    assert_eq!(f.harness.runtime.execs().len(), before + 2);
}

/// A Software tool runs in the Computer of the tenant that called it.
///
/// The runner resolves the Computer manager from the Workspace of the
/// call. Person B's call starts a container of B's, and it touches
/// nothing of person A's: not A's Tenant Network, not a volume labelled
/// A and not A's awake cap.
#[tokio::test]
async fn a_tool_runs_in_the_calling_tenants_computer_and_no_other() {
    let f = fixture().await;
    // The harness woke A's Computer, so every start the runtime saw so far
    // is A's.
    let before = f.harness.runtime.started_owners().len();

    let tenant_b = pagis_core::WorkspaceId::generate();
    let agent_b = pagis_core::AgentId::generate();
    f.harness.runtime.push_exec_outcome(ExecOutcome {
        stdout: "{}".to_string(),
        ..ExecOutcome::default()
    });

    let result = f
        .runner
        .run(&tenant_b, &agent_b, "weather", "v1", "forecast", &city())
        .await;

    assert!(!result.is_error, "{}", result.content);
    let started: Vec<pagis_computer::ComputerOwner> = f
        .harness
        .runtime
        .started_owners()
        .into_iter()
        .skip(before)
        .collect();
    assert!(
        !started.is_empty(),
        "B's call booted no container of its own"
    );
    for owner in &started {
        assert_eq!(
            owner.workspace_id, tenant_b,
            "B's tool call started a container of another tenant"
        );
        assert_eq!(owner.agent_id, agent_b);
    }
    // And the label the Docker objects carry says B, which is what the
    // network, the volume and the disk figure are keyed on.
    let labels = started[0].labels();
    assert!(
        labels.contains(&(
            pagis_computer::WORKSPACE_LABEL.to_string(),
            tenant_b.to_string()
        )),
        "{labels:?}"
    );
}

/// A Computer manager refuses an Agent that is not of its Workspace.
/// The runner cannot reach the wrong manager, and the manager refuses
/// one anyway: a container is the boundary between
/// tenants, so the second lock is worth holding.
#[tokio::test]
async fn a_manager_refuses_an_agent_of_another_tenant() {
    let runtime = Arc::new(pagis_computer::fake::FakeComputerRuntime::with_image());
    let agents = Arc::new(pagis_computer::fake::FakeAgents::strict());
    let screens = tempfile::tempdir().expect("screens dir");
    let tenant_a = pagis_core::WorkspaceId::generate();
    let tenant_b = pagis_core::WorkspaceId::generate();
    let agent_of_b = pagis_core::AgentId::generate();
    agents.add(&tenant_b, &agent_of_b);
    let managers = pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: Arc::clone(&runtime) as _,
        skills: Arc::new(pagis_core::NoSkills) as _,
        workspaces: Arc::new(pagis_computer::fake::FakeWorkspaces::with_timezone(
            &tenant_a, "UTC",
        )) as _,
        agents: Arc::clone(&agents) as _,
        bus: Arc::new(support::SilentBus) as _,
        screens_dir: screens.path().to_path_buf(),
        idle_stop: std::time::Duration::from_secs(600),
        relay: pagis_computer::fake::loopback_relay(),
        caps: pagis_computer::AwakeCaps::default(),
        cancel: tokio_util::sync::CancellationToken::new(),
        exit: None,
    });

    let refused = managers
        .get(&tenant_a)
        .wake(&agent_of_b)
        .await
        .expect_err("A's manager wakes no Agent of B");

    assert!(
        matches!(refused, pagis_computer::ComputerError::ForeignAgent),
        "{refused}"
    );
    assert!(
        runtime.started_owners().is_empty(),
        "a refused wake started a container"
    );
    // B's own manager wakes it.
    managers
        .get(&tenant_b)
        .wake(&agent_of_b)
        .await
        .expect("B's manager wakes B's Agent");
}
