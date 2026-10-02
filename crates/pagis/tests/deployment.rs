//! The documented compose deployment (`deploy/compose.yaml`), as
//! Docker Compose itself reads it.

use std::path::PathBuf;

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn repository() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"))
        .join("../..")
}

/// The deployment as `docker compose config` resolves it, with no
/// setting put in and every profile on, so it holds every service.
fn compose_config() -> serde_json::Value {
    let mut command = compose();
    command.args([
        "--profile",
        "*",
        "config",
        "--no-interpolate",
        "--format",
        "json",
    ]);
    config_of(command)
}

/// The deployment as Compose resolves it for the settings of
/// `deploy/.env.example`, with `overrides` from the environment, which
/// win over the file. Only the services of the active profiles are in it.
fn example_config(overrides: &[(&str, &str)]) -> serde_json::Value {
    let mut command = compose();
    command
        .arg("--env-file")
        .arg(repository().join("deploy/.env.example"))
        .args(["config", "--format", "json"])
        .env_remove("COMPOSE_PROFILES")
        .env_remove("PAGIS_REMOTE_ACCESS");
    command.envs(overrides.iter().copied());
    config_of(command)
}

fn compose() -> std::process::Command {
    let mut command = std::process::Command::new("docker");
    command
        .args(["compose", "--file"])
        .arg(repository().join("deploy/compose.yaml"));
    command
}

fn config_of(mut command: std::process::Command) -> serde_json::Value {
    let output = command.output().expect("the docker CLI runs");
    assert!(
        output.status.success(),
        "docker compose config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("compose prints JSON")
}

/// The names of the services in `config`, sorted.
fn service_names(config: &serde_json::Value) -> Vec<String> {
    let mut names: Vec<String> = config["services"]
        .as_object()
        .expect("the services")
        .keys()
        .cloned()
        .collect();
    names.sort();
    names
}

/// The names that `deploy/.env.example` sets, in the order of the file.
fn example_settings() -> Vec<String> {
    std::fs::read_to_string(repository().join("deploy/.env.example"))
        .expect("the example settings")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split_once('=')
                .unwrap_or_else(|| panic!("{line:?} is not NAME=value"))
                .0
                .to_string()
        })
        .collect()
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
/// make the daemon send requests to that loopback
/// (https://docs.pagis.co/plugins#reach). Nothing in the deployment uses the admin API.
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

/// The way in is a Compose profile. `proxy` runs Caddy, which binds 80
/// and 443 and gets the certificate of a domain name. `tailscale` runs
/// the owner's Tailscale Funnel, for a machine at home with no domain
/// name and no open port. The other services run with either one.
#[test]
fn the_way_in_is_a_profile_of_its_own() {
    let config = compose_config();
    let services = config["services"].as_object().expect("the services");

    for (name, service) in services {
        let expected = match name.as_str() {
            "proxy" => serde_json::json!(["proxy"]),
            "tailscale" => serde_json::json!(["tailscale"]),
            _ => serde_json::Value::Null,
        };
        assert_eq!(service["profiles"], expected, "{name}");
    }
    assert!(services.contains_key("proxy"), "{services:?}");
    assert!(services.contains_key("tailscale"), "{services:?}");
}

/// The `tailscale` service shares the host's network, as every service
/// does, so its Funnel reaches the product port and the TURN server of
/// the daemon on loopback. Userspace networking changes no network
/// setting of the host. The Funnel comes from the serve configuration in
/// `deploy/`, and the node keeps its state in a volume, so the auth key
/// is read at the first start alone.
#[test]
fn the_tailscale_service_serves_the_funnel_from_its_file() {
    let config = compose_config();
    let tailscale = &config["services"]["tailscale"];
    let environment = &tailscale["environment"];
    let volumes = tailscale["volumes"].as_array().expect("the volumes");
    let mounted_at = |target: &serde_json::Value| {
        volumes
            .iter()
            .find(|volume| volume["target"] == *target)
            .unwrap_or_else(|| panic!("nothing is mounted at {target}: {volumes:?}"))
    };

    assert_eq!(tailscale["network_mode"], "host");
    assert_eq!(environment["TS_USERSPACE"], "true");
    let serve = mounted_at(&environment["TS_SERVE_CONFIG"]);
    assert_eq!(serve["type"], "bind", "{serve}");
    assert_eq!(serve["read_only"], true, "{serve}");
    let source = std::path::Path::new(serve["source"].as_str().expect("a source"));
    assert_eq!(
        source.canonicalize().expect("the serve configuration"),
        repository()
            .join("deploy/tailscale-serve.json")
            .canonicalize()
            .expect("deploy/tailscale-serve.json"),
    );
    let state = mounted_at(&environment["TS_STATE_DIR"]);
    assert_eq!(state["type"], "volume", "{state}");
    assert_eq!(environment["TS_AUTH_ONCE"], "true");

    // The serve configuration names the product port that the daemon
    // binds. Compose prints a YAML number as a number.
    let port = match &config["services"]["pagis"]["environment"]["PAGIS_PORT"] {
        serde_json::Value::String(port) => port.clone(),
        port => port.to_string(),
    };
    let file = std::fs::read_to_string(source).expect("the serve configuration");
    assert!(
        file.contains(&format!("\"http://127.0.0.1:{port}\"")),
        "{file}"
    );
}

/// `.env.example` resolves as it is, with the `proxy` profile, and with
/// the `tailscale` profile and Remote Access in its place. Compose reads
/// every `${...}` of every service, also of a profile that is off, so a
/// required setting of one way in would stop a server of the other.
#[test]
fn the_example_settings_resolve_for_each_way_in() {
    let proxy = example_config(&[]);
    assert_eq!(service_names(&proxy), ["db", "egress", "pagis", "proxy"]);
    assert_eq!(
        proxy["services"]["pagis"]["environment"]["PAGIS_REMOTE_ACCESS"],
        ""
    );

    let home = example_config(&[
        ("COMPOSE_PROFILES", "tailscale"),
        ("PAGIS_REMOTE_ACCESS", "1"),
    ]);
    assert_eq!(service_names(&home), ["db", "egress", "pagis", "tailscale"]);
    assert_eq!(
        home["services"]["pagis"]["environment"]["PAGIS_REMOTE_ACCESS"],
        "1"
    );
}

/// Each setting of `.env.example` goes somewhere: a service of
/// `compose.yaml` reads it, or Compose itself does (`COMPOSE_*`). A
/// setting that nothing reads would look like a setting and change
/// nothing.
#[test]
fn every_example_setting_reaches_the_deployment() {
    let config = compose_config().to_string();

    for name in example_settings() {
        if name.starts_with("COMPOSE_") {
            continue;
        }
        assert!(
            config.contains(&format!("${{{name}}}")) || config.contains(&format!("${{{name}:")),
            "nothing in deploy/compose.yaml reads {name}"
        );
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
