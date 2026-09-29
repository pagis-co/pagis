//! Materialization inside a Computer: the temporary
//! sibling, `setup` once, the read-only tree, and the lock that makes
//! a second caller wait.

use crate::support;

use std::sync::Arc;

use pagis_computer::ExecOutcome;
use pagis_software::{Materializer, package_root};
use support::{MemorySource, commands, harness};

const MANIFEST: &str = r#"
[package]
name = "weather"
description = "Weather for a city"
setup = "pnpm install --frozen-lockfile"

[[tool]]
name = "forecast"
description = "Tomorrow's forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
"#;

const NO_SETUP: &str = r#"
[package]
name = "weather"
description = "Weather for a city"

[[tool]]
name = "forecast"
description = "Tomorrow's forecast"
entry = "bin/forecast.py"
schema = "schemas/forecast.json"
"#;

/// The check for an existing tree answers "not there", so the
/// materializer builds one.
fn absent(runtime: &pagis_computer::fake::FakeComputerRuntime) {
    runtime.push_exec_outcome(ExecOutcome {
        exit_code: 1,
        ..ExecOutcome::default()
    });
}

#[tokio::test]
async fn a_first_call_uploads_runs_setup_freezes_and_renames() {
    let h = harness().await;
    let source = MemorySource::with_version("weather", "v1", MANIFEST, b"tar-bytes".to_vec());
    let materializer = Materializer::new(Arc::clone(&h.managers), source);
    absent(&h.runtime);

    let root = materializer
        .ensure(
            &h.workspace_id,
            &h.agent_id,
            "weather",
            "v1",
            Some("pnpm install"),
        )
        .await
        .expect("materialize");

    assert_eq!(root, package_root("weather", "v1"));
    let commands = commands(&h.runtime);
    // test, mkdir, setup, then freeze and rename.
    assert!(commands[0].starts_with("test -d "), "{commands:?}");
    assert!(commands[1].contains("mkdir -p "), "{commands:?}");
    assert_eq!(commands[2], "pnpm install");
    assert!(commands[3].starts_with("chmod -R a-w "), "{commands:?}");
    assert!(commands[3].contains("mv "), "{commands:?}");
    // The upload goes to the temporary sibling, not to the root.
    let (path, tar) = h.runtime.uploads().pop().expect("one upload");
    assert_eq!(tar, b"tar-bytes".to_vec());
    assert!(path.contains("/.tmp-v1-"), "{path}");
    assert_ne!(path, root);
}

#[tokio::test]
async fn a_second_call_reuses_the_ready_tree() {
    let h = harness().await;
    let source = MemorySource::with_version("weather", "v1", NO_SETUP, b"tar".to_vec());
    let materializer = Materializer::new(Arc::clone(&h.managers), source);
    absent(&h.runtime);

    materializer
        .ensure(&h.workspace_id, &h.agent_id, "weather", "v1", None)
        .await
        .expect("materialize");
    let after_first = h.runtime.execs().len();
    materializer
        .ensure(&h.workspace_id, &h.agent_id, "weather", "v1", None)
        .await
        .expect("materialize again");

    assert_eq!(h.runtime.execs().len(), after_first, "no second build");
    assert_eq!(h.runtime.uploads().len(), 1);
}

#[tokio::test]
async fn a_tree_a_restart_forgot_is_adopted_and_not_built_again() {
    let h = harness().await;
    let source = MemorySource::with_version("weather", "v1", NO_SETUP, b"tar".to_vec());
    let materializer = Materializer::new(Arc::clone(&h.managers), source);
    // `test -d` finds the tree the last daemon left behind.
    h.runtime.push_exec_outcome(ExecOutcome::default());

    let root = materializer
        .ensure(&h.workspace_id, &h.agent_id, "weather", "v1", None)
        .await
        .expect("materialize");

    assert_eq!(root, package_root("weather", "v1"));
    assert!(h.runtime.uploads().is_empty(), "nothing was uploaded");
}

#[tokio::test]
async fn a_failed_setup_discards_the_tree() {
    let h = harness().await;
    let source = MemorySource::with_version("weather", "v1", MANIFEST, b"tar".to_vec());
    let materializer = Materializer::new(Arc::clone(&h.managers), source);
    absent(&h.runtime);
    // mkdir succeeds, setup fails.
    h.runtime.push_exec_outcome(ExecOutcome::default());
    h.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 1,
        stderr: "no lockfile".to_string(),
        ..ExecOutcome::default()
    });

    let error = materializer
        .ensure(
            &h.workspace_id,
            &h.agent_id,
            "weather",
            "v1",
            Some("pnpm install"),
        )
        .await
        .expect_err("setup failed");

    assert!(error.contains("no lockfile"), "{error}");
    let commands = commands(&h.runtime);
    let last = commands.last().expect("a command");
    assert!(last.starts_with("rm -rf "), "{last}");
    assert!(last.contains("/.tmp-v1-"), "{last}");
}

#[tokio::test]
async fn a_concurrent_caller_waits_for_the_one_ready_tree() {
    let h = harness().await;
    let source = MemorySource::with_version("weather", "v1", NO_SETUP, b"tar".to_vec());
    let materializer = Arc::new(Materializer::new(Arc::clone(&h.managers), source));
    absent(&h.runtime);
    h.runtime
        .set_exec_delay(std::time::Duration::from_millis(20));

    let first = {
        let materializer = Arc::clone(&materializer);
        let agent_id = h.agent_id.clone();
        let workspace_id = h.workspace_id.clone();
        tokio::spawn(async move {
            materializer
                .ensure(&workspace_id, &agent_id, "weather", "v1", None)
                .await
        })
    };
    let second = {
        let materializer = Arc::clone(&materializer);
        let agent_id = h.agent_id.clone();
        let workspace_id = h.workspace_id.clone();
        tokio::spawn(async move {
            materializer
                .ensure(&workspace_id, &agent_id, "weather", "v1", None)
                .await
        })
    };

    let roots = (
        first.await.unwrap().unwrap(),
        second.await.unwrap().unwrap(),
    );
    assert_eq!(roots.0, roots.1);
    assert_eq!(h.runtime.uploads().len(), 1, "the tree was built once");
}
