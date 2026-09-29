//! The documented compose deployment (`deploy/compose.yaml`), as
//! Docker Compose itself reads it.

use std::path::PathBuf;

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn repository() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"))
        .join("../..")
}

/// The deployment as `docker compose config` resolves it.
fn compose_config() -> serde_json::Value {
    let compose = repository().join("deploy/compose.yaml");
    let output = std::process::Command::new("docker")
        .args(["compose", "--file"])
        .arg(&compose)
        .args(["config", "--no-interpolate", "--format", "json"])
        .output()
        .expect("the docker CLI runs");
    assert!(
        output.status.success(),
        "docker compose config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("compose prints JSON")
}

/// The global options of `deploy/Caddyfile`: the lines of the block that
/// opens the file, without comments. A file that does not open with a
/// global block has none.
fn caddy_global_options() -> Vec<String> {
    let caddyfile = std::fs::read_to_string(repository().join("deploy/Caddyfile"))
        .expect("the proxy's Caddyfile");
    let mut lines = caddyfile
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty());
    if lines.next() != Some("{") {
        return Vec::new();
    }
    lines
        .take_while(|line| *line != "}")
        .map(str::to_string)
        .collect()
}

/// The state directory of the Headless Server image: its `PAGIS_HOME`.
fn image_home() -> String {
    let dockerfile =
        std::fs::read_to_string(repository().join("Dockerfile")).expect("the server Dockerfile");
    dockerfile
        .lines()
        .find_map(|line| line.trim().strip_prefix("ENV PAGIS_HOME="))
        .expect("the image sets PAGIS_HOME")
        .trim()
        .to_string()
}

/// The daemon asks Docker to bind-mount paths of its state directory
/// into a Computer: the screend token, and the checkout of each Plugin.
/// Docker reads a bind source on the Docker host and not in the
/// daemon's container, so the deployment mounts one host directory at
/// the same path in both places.
#[test]
fn the_state_directory_has_one_path_on_the_docker_host_and_in_the_daemon() {
    let config = compose_config();
    let home = image_home();
    let volumes = config["services"]["pagis"]["volumes"]
        .as_array()
        .expect("the daemon has volumes");
    let state = volumes
        .iter()
        .find(|volume| volume["target"] == home.as_str())
        .unwrap_or_else(|| panic!("nothing is mounted at {home}: {volumes:?}"));

    assert_eq!(state["type"], "bind", "{state}");
    assert_eq!(state["source"], home.as_str(), "{state}");
}

/// Caddy serves its admin API on the loopback of the VM unless the
/// Caddyfile turns it off, and a request there can stop the proxy. The
/// daemon has the VM's network, so every HTTP or SSE Plugin server can
/// make the daemon send requests to that loopback (`docs/PLUGINS.md`,
/// "Reach"). Nothing in the deployment uses the admin API.
#[test]
fn the_proxy_serves_no_admin_api() {
    let options = caddy_global_options();

    assert!(
        options.iter().any(|option| option == "admin off"),
        "the global options of deploy/Caddyfile are {options:?}"
    );
}

/// Compose outside Swarm mounts a file secret as it is on the host and
/// ignores `uid`, `gid` and `mode`, with a warning at every run. The
/// deployment states none of them, so the documentation's rule holds:
/// the mode of the file on the host is the mode the daemon sees.
#[test]
fn the_secrets_carry_no_setting_that_compose_ignores() {
    let config = compose_config();
    let services = config["services"].as_object().expect("the services");
    for (name, service) in services {
        let Some(secrets) = service["secrets"].as_array() else {
            continue;
        };
        for secret in secrets {
            let keys: Vec<&str> = secret
                .as_object()
                .expect("a secret is a mapping")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, ["source", "target"], "{name}: {secret}");
        }
    }
}

/// `deploy/backup.sh` makes its destination private before a container
/// writes into it, whatever the umask of the operator's shell. The
/// containers write under their own umask, and a destination that only
/// the owner can enter keeps every file under it from other users.
///
/// A stub `docker` stands in for Docker Compose. It answers every call
/// with success and prints nothing, so the script finds no Workspace and
/// takes no volume.
#[test]
fn the_backup_script_makes_a_private_destination() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let dir = tempfile::tempdir().expect("a directory");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).expect("the stub directory");
    std::fs::write(bin.join("docker"), "#!/bin/sh\nexit 0\n").expect("the stub docker");
    std::fs::set_permissions(bin.join("docker"), std::fs::Permissions::from_mode(0o755))
        .expect("the stub is executable");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // A destination that is not there, and an empty one that the
    // operator made with a mode that others can enter.
    let missing = dir.path().join("backups/nightly");
    let existing = dir.path().join("weekly");
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&existing)
        .expect("an empty destination");
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755))
        .expect("a destination that others can enter");

    for destination in [missing, existing] {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg("umask 022; exec sh \"$@\"")
            .arg("sh")
            .arg(repository().join("deploy/backup.sh"))
            .arg(&destination)
            .env("PATH", &path)
            .output()
            .expect("the backup script runs");

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mode = std::fs::metadata(&destination)
            .expect("the destination")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "{}", destination.display());
    }
}

/// The `state/` copy of `deploy/backup.sh` holds no Computer token. A
/// token is live while its Computer runs, and the script stops the
/// daemon and not the Computers.
///
/// This stub `docker` runs the real `pagis backup` for the
/// `docker compose run ... pagis backup /backup/installation` call of
/// the script, with `/backup` at the host directory of the `-v` mount.
/// It answers every other call with success and prints nothing, so the
/// script finds no Workspace and takes no volume.
#[test]
fn the_backup_script_leaves_the_computer_tokens_out_of_the_state_copy() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("a directory");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).expect("the stub directory");
    std::fs::write(
        bin.join("docker"),
        r#"#!/bin/sh
[ "$1 $2" = "compose run" ] || exit 0
shift 2
while [ $# -gt 0 ]; do
	case $1 in
	-v) mount=$2; shift 2 ;;
	pagis) shift; break ;;
	*) shift ;;
	esac
done
exec "$PAGIS_BINARY" "$1" "${mount%%:*}${2#/backup}"
"#,
    )
    .expect("the stub docker");
    std::fs::set_permissions(bin.join("docker"), std::fs::Permissions::from_mode(0o755))
        .expect("the stub is executable");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let home = dir.path().join("state");
    std::fs::create_dir_all(home.join("computer-tokens/w1")).expect("the token directory");
    std::fs::create_dir_all(home.join("memory/w1")).expect("the memory directory");
    for (name, contents) in [
        ("config.toml", ""),
        ("runtime-release", "0.1.0\n"),
        ("pagis.db", "records"),
        ("memory/w1/note.md", "remembered"),
        ("computer-tokens/w1/a1.token", "a live token"),
    ] {
        std::fs::write(home.join(name), contents).expect("a file of the state directory");
    }
    let destination = dir.path().join("backups/nightly");

    let output = std::process::Command::new("sh")
        .arg(repository().join("deploy/backup.sh"))
        .arg(&destination)
        .env("PATH", &path)
        .env("PAGIS_HOME", &home)
        .env("PAGIS_BINARY", env!("CARGO_BIN_EXE_pagis"))
        .env_remove("PAGIS_DATABASE_URL")
        .output()
        .expect("the backup script runs");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state = destination.join("installation/state");
    assert!(
        state.join("memory/w1/note.md").exists(),
        "the stub ran `pagis backup` into {}",
        state.display()
    );
    let tokens: Vec<_> = crate::boot::tree(&destination)
        .into_iter()
        .filter(|path| path.to_string_lossy().contains("computer-tokens"))
        .collect();
    assert!(tokens.is_empty(), "the archive holds tokens: {tokens:#?}");
}
