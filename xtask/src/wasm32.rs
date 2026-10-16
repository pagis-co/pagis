//! The build and Node tests of the portable crates.

use std::path::Path;

use anyhow::{Context, Result};

use crate::{Action, Cmd, Step};

/// One portable crate, its Cargo arguments and its Node test binaries.
#[derive(Debug, PartialEq, Eq)]
pub struct WasmCrate {
    pub package: &'static str,
    pub args: &'static [&'static str],
    pub tests: &'static [&'static str],
}

/// Each crate in this list builds and runs on `wasm32-unknown-unknown`.
pub const WASM_CRATES: &[WasmCrate] = &[
    WasmCrate {
        package: "pagis-core",
        args: &[],
        tests: &["memory_rules", "wasm", "computer_frame"],
    },
    WasmCrate {
        package: "llm-router",
        args: &[],
        tests: &["rt", "realtime_host"],
    },
    WasmCrate {
        package: "pagis-broker",
        args: &[],
        tests: &["rules_wasm"],
    },
    WasmCrate {
        package: "pagis-rt",
        args: &[],
        tests: &["rt"],
    },
    WasmCrate {
        package: "pagis-agent",
        args: &[],
        tests: &["actor_host"],
    },
    WasmCrate {
        package: "pagis-push",
        args: &[],
        tests: &["wasm"],
    },
    WasmCrate {
        package: "pagis-needs-you",
        args: &[],
        tests: &["rules"],
    },
    WasmCrate {
        package: "pagis-voice",
        args: &[],
        tests: &["router_host"],
    },
    WasmCrate {
        package: "pagis-mail",
        args: &[],
        tests: &["rules"],
    },
    WasmCrate {
        package: "pagis-trigger",
        args: &[],
        tests: &["wasm"],
    },
    WasmCrate {
        package: "pagis-telephony",
        args: &[],
        tests: &["wasm"],
    },
    WasmCrate {
        package: "pagis-server",
        args: &["--no-default-features"],
        tests: &["agents_wasm", "conversation_wasm", "plugins_wasm"],
    },
    WasmCrate {
        package: "pagis-plugin",
        args: &["--no-default-features"],
        tests: &["wasm"],
    },
    WasmCrate {
        package: "pagis-plugins",
        args: &["--no-default-features"],
        tests: &["wasm"],
    },
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
    for entry in WASM_CRATES.iter().filter(|entry| entry.args.is_empty()) {
        clippy.extend(["-p", entry.package]);
    }
    clippy.extend(["--", "-D", "warnings"]);
    let mut commands = vec![wrap(&clippy)];
    // A crate with feature arguments builds alone, so another crate
    // cannot turn its default features on through feature unification.
    for entry in WASM_CRATES.iter().filter(|entry| !entry.args.is_empty()) {
        let mut args = vec![
            "clippy",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            entry.package,
        ];
        args.extend(entry.args);
        args.extend(["--", "-D", "warnings"]);
        commands.push(wrap(&args));
    }
    for entry in WASM_CRATES {
        for test in entry.tests {
            let mut args = vec![
                "test",
                "-p",
                entry.package,
                "--target",
                "wasm32-unknown-unknown",
                "--test",
                test,
            ];
            args.extend(entry.args);
            commands.push(wrap(&args));
        }
    }
    Ok(commands)
}
