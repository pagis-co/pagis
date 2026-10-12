//! The build and Node tests of the portable crates.

use std::path::Path;

use anyhow::{Context, Result};

use crate::{Action, Cmd, Step};

/// A crate joins this list when it builds and runs on
/// `wasm32-unknown-unknown`. It does not leave the list.
/// Each entry names the crate and its wasm-bindgen-test binaries.
pub const WASM_CRATES: &[(&str, &[&str])] = &[
    ("pagis-core", &["wasm"]),
    ("pagis-memory", &["rules"]),
    ("llm-router", &["rt", "realtime_host"]),
    ("pagis-broker", &["rules_wasm"]),
    ("pagis-rt", &["rt"]),
    ("pagis-agent", &["actor_host"]),
    ("pagis-push", &["wasm"]),
];

pub fn step(root: &Path) -> Step {
    let action = match commands(root) {
        Ok(commands) => Action::Run(commands),
        Err(error) => Action::Run(vec![Cmd::refusal(&format!("wasm32: {error:#}"))]),
    };
    Step {
        name: "wasm32",
        action,
    }
}

fn commands(root: &Path) -> Result<Vec<Cmd>> {
    let lock: toml::Value = toml::from_str(
        &std::fs::read_to_string(root.join("Cargo.lock")).context("read Cargo.lock")?,
    )
    .context("parse Cargo.lock")?;
    let version = lock["package"]
        .as_array()
        .context("Cargo.lock has no packages")?
        .iter()
        .find(|package| package["name"].as_str() == Some("wasm-bindgen"))
        .and_then(|package| package["version"].as_str())
        .context("Cargo.lock has no wasm-bindgen version")?;
    let wrapper = root.join("xtask/src/wasm32-tools.sh");
    let wrap = |args: &[&str]| {
        let mut wrapped = vec![
            wrapper.to_str().expect("UTF-8 workspace path"),
            version,
            "cargo",
        ];
        wrapped.extend(args);
        Cmd::new("sh", &wrapped).in_dir(root)
    };
    let mut clippy = vec!["clippy", "--target", "wasm32-unknown-unknown"];
    for (package, _) in WASM_CRATES {
        clippy.extend(["-p", package]);
    }
    clippy.extend(["--", "-D", "warnings"]);
    let mut commands = vec![wrap(&clippy)];
    for (package, tests) in WASM_CRATES {
        for test in *tests {
            commands.push(wrap(&[
                "test",
                "-p",
                package,
                "--target",
                "wasm32-unknown-unknown",
                "--test",
                test,
            ]));
        }
    }
    Ok(commands)
}
