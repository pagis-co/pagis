//! Helpers that the test modules share.

use std::path::PathBuf;

use xtask::{Action, Cmd, Step};

pub fn workspace_root() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level under the workspace root")
        .to_path_buf()
}

/// The hosts that have a pin: macOS arm64, Linux amd64 and Linux arm64.
pub fn host_is_pinned() -> bool {
    matches!(
        (std::env::consts::OS, std::env::consts::ARCH),
        ("macos", "aarch64") | ("linux", "x86_64") | ("linux", "aarch64")
    )
}

/// The position of this host in each archive table of `pagis-versions`.
pub fn host_index() -> usize {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => 0,
        ("linux", "x86_64") => 1,
        _ => 2,
    }
}

pub fn script(step: &Step) -> String {
    let Action::Run(cmds) = &step.action else {
        panic!("step {} must run, got {:?}", step.name, step.action);
    };
    cmds.iter()
        .map(|cmd| format!("{} {}", cmd.program, cmd.args.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Run `cmd` and return whether it passed, and what it printed.
pub fn run(cmd: &Cmd) -> (bool, String) {
    let mut command = std::process::Command::new(&cmd.program);
    command.args(&cmd.args);
    command.envs(cmd.env.iter().map(|(key, value)| (key, value)));
    if let Some(cwd) = &cmd.cwd {
        command.current_dir(cwd);
    }
    let output = command.output().expect("start the command");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}
