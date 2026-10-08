//! The deployment of the Push Relay (`deploy/push-relay/compose.yaml`),
//! as Docker Compose itself reads it.

use std::net::Ipv4Addr;
use std::path::PathBuf;

use serde_json::Value;

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn repository() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"))
        .join("../..")
}

fn deployment(file: &str) -> PathBuf {
    repository().join("deploy/push-relay").join(file)
}

fn compose() -> std::process::Command {
    let mut command = std::process::Command::new("docker");
    command
        .args(["compose", "--file"])
        .arg(deployment("compose.yaml"));
    command
}

fn config_of(mut command: std::process::Command) -> Value {
    let output = command.output().expect("the docker CLI runs");
    assert!(
        output.status.success(),
        "docker compose config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("compose prints JSON")
}

/// The deployment as `docker compose config` reads it, with no setting
/// put in.
fn compose_config() -> Value {
    let mut command = compose();
    command.args(["config", "--no-interpolate", "--format", "json"]);
    config_of(command)
}

/// The deployment as Compose resolves it for the settings of
/// `.env.example`.
fn example_config() -> Value {
    let mut command = compose();
    command
        .arg("--env-file")
        .arg(deployment(".env.example"))
        .args(["config", "--format", "json"]);
    for name in example_settings() {
        command.env_remove(name);
    }
    config_of(command)
}

/// The names that `.env.example` sets, in the order of the file.
fn example_settings() -> Vec<String> {
    std::fs::read_to_string(deployment(".env.example"))
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

/// The global options of the Caddyfile: the lines of the block that
/// opens the file, without comments.
fn caddy_global_options() -> Vec<String> {
    let caddyfile = std::fs::read_to_string(deployment("Caddyfile")).expect("the Caddyfile");
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

/// The value of `ENV <name>=` in the relay Dockerfile.
fn image_env(name: &str) -> String {
    let dockerfile =
        std::fs::read_to_string(repository().join("crates/pagis-push-relay/Dockerfile"))
            .expect("the relay Dockerfile");
    dockerfile
        .lines()
        .find_map(|line| line.trim().strip_prefix(&format!("ENV {name}=")))
        .unwrap_or_else(|| panic!("the image sets no {name}"))
        .trim()
        .to_string()
}

/// Caddy serves its admin API on its own loopback unless the Caddyfile
/// turns it off, and a request there can stop the proxy. Nothing in the
/// deployment uses it.
#[test]
fn the_proxy_serves_no_admin_api() {
    let options = caddy_global_options();

    assert!(
        options.iter().any(|option| option == "admin off"),
        "the global options of deploy/push-relay/Caddyfile are {options:?}"
    );
}

/// Each setting of `.env.example` reaches the relay. A setting that
/// nothing reads would look like a setting and change nothing.
#[test]
fn every_example_setting_reaches_the_relay() {
    let relay = compose_config()["services"]["relay"].to_string();

    let settings = example_settings();
    assert!(!settings.is_empty());
    for name in settings {
        assert!(
            relay.contains(&format!("${{{name}}}")) || relay.contains(&format!("${{{name}:")),
            "the relay of deploy/push-relay/compose.yaml does not read {name}"
        );
    }
}

/// Each setting that the relay reads from `.env` is in `.env.example`, so
/// an operator who fills in the example sets each of them.
#[test]
fn every_setting_of_the_relay_is_in_the_example() {
    let relay = compose_config()["services"]["relay"].to_string();
    let settings = example_settings();

    let read: Vec<&str> = relay
        .split("${")
        .skip(1)
        .map(|rest| rest.split([':', '}']).next().expect("a name after ${"))
        .collect();

    assert!(!read.is_empty());
    for name in read {
        assert!(
            settings.iter().any(|setting| setting == name),
            "the relay of deploy/push-relay/compose.yaml reads {name}, and \
             .env.example does not set it"
        );
    }
}

/// The deployment serves both APNs environments, each with a key pair of
/// its own.
#[test]
fn the_deployment_holds_a_key_pair_for_each_apns_environment() {
    let config = example_config();
    let environment = &config["services"]["relay"]["environment"];

    for name in [
        "PUSH_RELAY_APNS_PRODUCTION_KEY_PATH",
        "PUSH_RELAY_APNS_PRODUCTION_KEY_ID",
        "PUSH_RELAY_APNS_SANDBOX_KEY_PATH",
        "PUSH_RELAY_APNS_SANDBOX_KEY_ID",
        "PUSH_RELAY_APNS_TEAM_ID",
        "PUSH_RELAY_APNS_TOPIC",
    ] {
        assert!(
            environment[name]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "the relay of deploy/push-relay/compose.yaml sets no {name}: {environment}"
        );
    }
}

/// The relay believes `X-Forwarded-For` only from the address that
/// `PUSH_RELAY_TRUSTED_PROXY` names. Caddy has that fixed address on the
/// network of the deployment, so the registration limit counts the
/// address of each phone and not the address of Caddy.
#[test]
fn the_relay_trusts_the_fixed_address_of_the_proxy() {
    let config = example_config();
    let relay = &config["services"]["relay"];
    let proxy = &config["services"]["proxy"];
    let networks = config["networks"].as_object().expect("the networks");
    assert_eq!(networks.len(), 1, "{networks:?}");
    let (network, definition) = networks.iter().next().unwrap();
    let subnet = definition["ipam"]["config"][0]["subnet"]
        .as_str()
        .unwrap_or_else(|| panic!("the network {network} has no fixed subnet: {definition}"));
    let (base, prefix) = subnet.split_once('/').expect("a CIDR subnet");
    let base: Ipv4Addr = base.parse().expect("an IPv4 subnet");
    let mask = u32::MAX << (32 - prefix.parse::<u32>().expect("a prefix length"));

    let address: Ipv4Addr = proxy["networks"][network]["ipv4_address"]
        .as_str()
        .unwrap_or_else(|| panic!("the proxy has no fixed address: {proxy}"))
        .parse()
        .expect("an IPv4 address");

    assert_eq!(
        relay["environment"]["PUSH_RELAY_TRUSTED_PROXY"],
        address.to_string()
    );
    assert_eq!(
        u32::from(address) & mask,
        u32::from(base) & mask,
        "{subnet}"
    );
    assert!(relay["networks"].get(network).is_some(), "{relay}");
    assert_eq!(
        relay["environment"]["PUSH_RELAY_PUBLIC_ORIGIN"],
        format!(
            "https://{}",
            proxy["environment"]["PUSH_RELAY_DOMAIN"]
                .as_str()
                .expect("the domain of the proxy")
        )
    );
}

/// The APNs key of each environment and the FCM credentials are Compose
/// secrets, at the paths that the relay reads. Compose outside Swarm mounts a secret
/// read only and ignores `uid`, `gid` and `mode`, so the deployment
/// states none of them.
#[test]
fn the_keys_are_secrets_at_the_paths_that_the_relay_reads() {
    let config = example_config();
    let relay = &config["services"]["relay"];
    let secrets = relay["secrets"].as_array().expect("the secrets");
    let targets: Vec<&str> = secrets
        .iter()
        .map(|secret| {
            let keys: Vec<&str> = secret
                .as_object()
                .expect("a secret is a mapping")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, ["source", "target"], "{secret}");
            secret["target"].as_str().expect("a target")
        })
        .collect();

    for variable in [
        "PUSH_RELAY_APNS_PRODUCTION_KEY_PATH",
        "PUSH_RELAY_APNS_SANDBOX_KEY_PATH",
        "PUSH_RELAY_FCM_CREDENTIALS_PATH",
    ] {
        let path = relay["environment"][variable]
            .as_str()
            .unwrap_or_else(|| panic!("the relay sets no {variable}"));
        assert!(
            targets
                .iter()
                .any(|target| path == *target || path == format!("/run/secrets/{target}")),
            "{variable} is {path}, and the secrets are at {targets:?}"
        );
    }
    let mut unique = targets.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), targets.len(), "each key is its own secret");
}

/// The SQLite file of the image is in its state directory, and the
/// deployment keeps that directory in a named volume.
#[test]
fn the_state_directory_is_a_named_volume() {
    let config = compose_config();
    let database = image_env("PUSH_RELAY_DATABASE");
    let volumes = config["services"]["relay"]["volumes"]
        .as_array()
        .expect("the relay has volumes");

    let state = volumes
        .iter()
        .find(|volume| {
            volume["target"]
                .as_str()
                .is_some_and(|target| database.starts_with(&format!("{target}/")))
        })
        .unwrap_or_else(|| panic!("no volume holds {database}: {volumes:?}"));

    assert_eq!(state["type"], "volume", "{state}");
}
