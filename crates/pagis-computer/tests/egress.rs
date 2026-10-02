//! The egress policy of a Computer (ADR-0014): the rules that
//! `deploy/egress.sh` installs on the Docker host, against Computers
//! that the real runtime starts. The Docker tests are `#[ignore]`-tagged
//! as in `docker_real`, and the gate runs them where Docker is
//! reachable.
//!
//! A test runs the script as the `egress` service of
//! `deploy/compose.yaml` runs it: in the image of that service, with the
//! host's network and NET_ADMIN. The rules of a deployment match every
//! user-defined bridge. A test sets `PAGIS_EGRESS_BRIDGES` to the bridge
//! of its own Tenant Network, so the Docker tests that run beside it keep
//! their reach. The chains of the rules are global to the Docker host:
//! the nextest group `egress` runs one of these tests at a time, and a
//! test removes the chains when it starts and when it ends.
//!
//! The daemon and the Media Relay of a Headless Server run on the Docker
//! host itself. The tests model that topology on the Docker host of the
//! gate:
//!
//! - A listener with the host's network is the Media Relay, or another
//!   service of the Docker host. The Computer reaches it at the gateway
//!   of its Tenant Network, which is an address of the Docker host, as
//!   `host.docker.internal` is on a Headless Server. Under Colima that
//!   name is the Mac, which is outside the Docker host.
//! - A listener on a network of its own is an address outside the Tenant
//!   Networks: 169.254.169.254 for the metadata service of a cloud, and
//!   an address in 10.0.0.0/8 for a service on the LAN. Docker drops the
//!   traffic between two of its bridges, so these networks use the
//!   `nat-unprotected` gateway mode. Without the rules a Computer reaches
//!   them, and each test proves that before it installs the rules.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use pagis_computer::{ComputerOwner, ComputerRuntime, IMAGE, TEST_LABEL, network_name};
use pagis_core::AgentId;

use crate::docker_real::{Real, docker_exec_raw, reaches, tunnel_status};

/// The address of the metadata service of the clouds, and the block of
/// the network that stands in for it.
const METADATA: &str = "169.254.169.254";
const METADATA_SUBNET: &str = "169.254.169.0/24";
/// The port of each TCP listener outside the Docker host.
const SERVICE_PORT: u16 = 80;

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn repository() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"))
        .join("../..")
}

/// The deployment as `docker compose config` resolves it, without the
/// values of `.env`.
fn compose() -> serde_json::Value {
    let output = Command::new("docker")
        .args(["compose", "--file"])
        .arg(repository().join("deploy/compose.yaml"))
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

/// The image of the `egress` service.
fn egress_image() -> &'static str {
    static IMAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IMAGE.get_or_init(|| {
        compose()["services"]["egress"]["image"]
            .as_str()
            .expect("deploy/compose.yaml has an egress service with an image")
            .to_string()
    })
}

/// Run `script` with `sh` on the Docker host as the `egress` service
/// runs its script: in its image, with the host's network and
/// NET_ADMIN. The script goes in on stdin, because Docker reads a bind
/// source on the Docker host, and a file of this machine is not there
/// under every Docker.
fn on_the_docker_host(env: &[(&str, &str)], script: &[u8]) -> Output {
    let mut command = Command::new("docker");
    command.args([
        "run",
        "--rm",
        "-i",
        "--network",
        "host",
        "--cap-add",
        "NET_ADMIN",
        "--entrypoint",
        "sh",
    ]);
    for (key, value) in env {
        command.arg("-e").arg(format!("{key}={value}"));
    }
    command.arg(egress_image()).arg("-s");
    with_stdin(command, script)
}

fn with_stdin(mut command: Command, stdin: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("docker runs");
    child
        .stdin
        .take()
        .expect("the stdin of docker")
        .write_all(stdin)
        .expect("the script reaches docker");
    child.wait_with_output().expect("docker ends")
}

/// Run `deploy/egress.sh` on the Docker host with `env`.
fn run_egress(env: &[(&str, &str)]) -> Output {
    let script = std::fs::read(repository().join("deploy/egress.sh")).expect("deploy/egress.sh");
    on_the_docker_host(env, &script)
}

/// Prints each Pagis rule of the Docker host in the form of
/// `iptables -S`: the jumps from DOCKER-USER and INPUT, and the Pagis
/// chains. It reads the backend that holds Docker's DOCKER-USER chain.
const LIST_RULES: &str = r#"set -e
for t in iptables-nft iptables-legacy; do
  "$t" -S DOCKER-USER >/dev/null 2>&1 || continue
  for chain in DOCKER-USER INPUT PAGIS-FORWARD PAGIS-INPUT; do
    "$t" -S "$chain" 2>/dev/null | grep -e PAGIS || true
  done
  exit 0
done
echo "the Docker host has no DOCKER-USER chain" >&2
exit 1
"#;

/// Removes the jumps to the Pagis chains, and the chains.
const REMOVE_RULES: &str = r#"set -e
for t in iptables-nft iptables-legacy; do
  "$t" -S DOCKER-USER >/dev/null 2>&1 || continue
  for chain in DOCKER-USER INPUT; do
    jumps=$("$t" -S "$chain" | grep -e ' -j PAGIS-' | sed 's/^-A /-D /')
    echo "$jumps" | while read -r rule; do
      [ -z "$rule" ] || "$t" $rule
    done
  done
  for chain in PAGIS-FORWARD PAGIS-INPUT; do
    if "$t" -S "$chain" >/dev/null 2>&1; then
      "$t" -F "$chain"
      "$t" -X "$chain"
    fi
  done
done
"#;

/// Each Pagis rule on the Docker host.
fn host_rules() -> Vec<String> {
    let listed = on_the_docker_host(&[], LIST_RULES.as_bytes());
    assert!(
        listed.status.success(),
        "the rules of the Docker host are not listed: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The Pagis chains on the Docker host while a test holds this. A new
/// guard removes the chains that an earlier run left, and the drop
/// removes them, also when the test fails.
struct HostRules;

impl HostRules {
    fn clean() -> Self {
        let removed = on_the_docker_host(&[], REMOVE_RULES.as_bytes());
        assert!(
            removed.status.success(),
            "the Pagis chains of an earlier run are not removed: {}",
            String::from_utf8_lossy(&removed.stderr)
        );
        assert_eq!(host_rules(), Vec::<String>::new());
        Self
    }

    /// Install the rules on `bridge` alone, with `media` as the Media
    /// Relay's range and `allow` as `PAGIS_COMPUTER_ALLOW`.
    fn install(&self, bridge: &str, media: &MediaRange, allow: &str) {
        let first = media.first.to_string();
        let last = media.last.to_string();
        let output = run_egress(&[
            ("PAGIS_EGRESS_BRIDGES", bridge),
            ("PAGIS_MEDIA_PORT_FIRST", &first),
            ("PAGIS_MEDIA_PORT_LAST", &last),
            ("PAGIS_COMPUTER_ALLOW", allow),
        ]);
        assert!(
            output.status.success(),
            "the egress script failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for HostRules {
    /// A drop that panics during the test's own panic aborts the process
    /// and hides the failure, so a failed removal is reported and the
    /// next test removes what is left.
    fn drop(&mut self) {
        let removed = on_the_docker_host(&[], REMOVE_RULES.as_bytes());
        if !removed.status.success() {
            eprintln!(
                "the Pagis chains stay on the Docker host: {}",
                String::from_utf8_lossy(&removed.stderr)
            );
        }
    }
}

/// The Media Relay's UDP range that a test gives the rules.
struct MediaRange {
    first: u16,
    last: u16,
}

impl MediaRange {
    /// Four ports at a random place, so that a listener of an earlier
    /// run does not hold them.
    fn random() -> Self {
        let first = 51_000 + rand::random::<u16>() % 8_000;
        Self {
            first,
            last: first + 3,
        }
    }
}

/// A listener: a TCP server that accepts and closes each connection, or
/// a UDP server that echoes each datagram. It prints `ready` when it
/// holds its port.
const LISTEN: &str = r#"
import socket, sys
kind, port = sys.argv[1], int(sys.argv[2])
if kind == "udp":
    server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    server.bind(("0.0.0.0", port))
    print("ready", flush=True)
    while True:
        data, peer = server.recvfrom(2048)
        server.sendto(data, peer)
else:
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", port))
    server.listen(16)
    print("ready", flush=True)
    while True:
        server.accept()[0].close()
"#;

/// Sends one datagram twice at most, and exits 0 when the echo comes
/// back.
const UDP_PROBE: &str = r#"
import socket, sys
probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
probe.settimeout(2)
for _ in range(2):
    probe.sendto(b"pagis", (sys.argv[1], int(sys.argv[2])))
    try:
        if probe.recvfrom(64)[0] == b"pagis":
            sys.exit(0)
    except OSError:
        pass
sys.exit(1)
"#;

/// Start a listener of `kind` on `port` from the Computer image, which
/// has Python, and wait until it holds the port. `network` is `host` or
/// a Docker network, where the listener takes `address`.
fn listen(real: &Real, network: &str, address: Option<&str>, kind: &str, port: u16) {
    let mut command = Command::new("docker");
    command
        .args(["run", "--detach", "--label"])
        .arg(format!("{TEST_LABEL}={}", real.docker.mark()))
        .args(["--network", network]);
    if let Some(address) = address {
        command.args(["--ip", address]);
    }
    let started = command
        .args(["--entrypoint", "python3", IMAGE, "-u", "-c", LISTEN, kind])
        .arg(port.to_string())
        .output()
        .expect("docker run");
    assert!(
        started.status.success(),
        "the {kind} listener on {network} did not start: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let container = String::from_utf8_lossy(&started.stdout).trim().to_string();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let logs = Command::new("docker")
            .args(["logs", &container])
            .output()
            .expect("docker logs");
        if String::from_utf8_lossy(&logs.stdout).contains("ready") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the {kind} listener on {network} port {port} is not ready: {}",
            String::from_utf8_lossy(&logs.stderr)
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Create a network outside the Tenant Networks. Docker's own rules let
/// a container of another bridge reach it, so only the egress rules keep
/// a Computer out.
fn outside_network(real: &Real, name: &str, subnet: &str) -> Result<(), String> {
    let created = Command::new("docker")
        .args(["network", "create", "--label"])
        .arg(format!("{TEST_LABEL}={}", real.docker.mark()))
        .args([
            "--opt",
            "com.docker.network.bridge.gateway_mode_ipv4=nat-unprotected",
            "--subnet",
            subnet,
            name,
        ])
        .output()
        .expect("docker network create");
    if created.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&created.stderr).trim().to_string())
    }
}

/// One value of `docker network inspect`.
fn network_field(network: &str, format: &str) -> String {
    let output = Command::new("docker")
        .args(["network", "inspect", "--format", format, network])
        .output()
        .expect("docker network inspect");
    assert!(
        output.status.success(),
        "docker network inspect {network}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// What a test's Computer tries to reach, and at which addresses.
struct Places {
    /// The interface of the Tenant Network on the Docker host.
    bridge: String,
    /// The Docker host, at the gateway of the Tenant Network.
    host: String,
    /// A TCP listener of the Docker host, outside the media range.
    host_port: u16,
    /// The media range. A UDP echo listens on its last port, as the
    /// Media Relay, and one on the port after it.
    media: MediaRange,
    /// A TCP listener at an address on the LAN, and the block of it.
    lan: String,
    lan_block: String,
}

impl Places {
    /// Start every listener for the Computers of `real`'s Workspace.
    fn start(real: &Real) -> Self {
        let tenant = network_name(&real.workspace_id);
        let id = network_field(&tenant, "{{.Id}}");
        let host = network_field(&tenant, "{{range .IPAM.Config}}{{.Gateway}}{{end}}");
        assert!(
            !host.is_empty(),
            "the Tenant Network {tenant} has no gateway"
        );
        // The rules are IPv4 rules, so a Computer has no IPv6 address.
        assert_eq!(network_field(&tenant, "{{.EnableIPv6}}"), "false");

        let metadata = format!("pagis-egress-metadata-{}", real.docker.mark());
        outside_network(real, &metadata, METADATA_SUBNET)
            .unwrap_or_else(|error| panic!("the metadata network is not created: {error}"));
        listen(real, &metadata, Some(METADATA), "tcp", SERVICE_PORT);

        // Docker refuses a subnet that another network holds, so a few
        // random blocks are tried.
        let lan_network = format!("pagis-egress-lan-{}", real.docker.mark());
        let mut lan_block = String::new();
        for _ in 0..20 {
            let block = format!(
                "10.{}.{}.0/24",
                200 + rand::random::<u8>() % 55,
                rand::random::<u8>()
            );
            if outside_network(real, &lan_network, &block).is_ok() {
                lan_block = block;
                break;
            }
        }
        assert!(!lan_block.is_empty(), "no free block for the LAN network");
        let lan = lan_block.replace(".0/24", ".10");
        listen(real, &lan_network, Some(&lan), "tcp", SERVICE_PORT);

        let host_port = 41_000 + rand::random::<u16>() % 8_000;
        listen(real, "host", None, "tcp", host_port);
        let media = MediaRange::random();
        listen(real, "host", None, "udp", media.last);
        listen(real, "host", None, "udp", media.last + 1);

        Self {
            bridge: format!("br-{}", &id[..12]),
            host,
            host_port,
            media,
            lan,
            lan_block,
        }
    }
}

/// Whether a UDP echo at `address:port` answers the Computer.
fn echoes(from: &ComputerOwner, address: &str, port: u16) -> bool {
    docker_exec_raw(
        from,
        &["--user", "agent"],
        &["python3", "-c", UDP_PROBE, address, &port.to_string()],
    )
    .status
    .success()
}

/// Whether the Computer resolves `name`.
fn resolves(from: &ComputerOwner, name: &str) -> bool {
    docker_exec_raw(from, &["--user", "agent"], &["getent", "hosts", name])
        .status
        .success()
}

/// The public name that each test reaches, as `python_opens_an_https_connection` does.
const PUBLIC: &str = "example.com";

/// What `owner`'s Computer reaches before and after the rules. Without
/// them it reaches every listener, which proves that each probe can
/// succeed. With them it reaches the internet and the Media Relay, and
/// nothing else, also through its Exit Proxy.
fn assert_the_policy(real: &Real, owner: &ComputerOwner, rules: &HostRules) {
    let places = Places::start(real);
    let host = places.host.as_str();

    assert!(
        resolves(owner, PUBLIC),
        "no control: without the rules the Computer does not resolve {PUBLIC}"
    );
    assert!(
        reaches(owner, PUBLIC, 443),
        "no control: without the rules the Computer does not reach {PUBLIC}:443"
    );
    assert!(
        reaches(owner, METADATA, SERVICE_PORT),
        "no control: without the rules the Computer does not reach {METADATA}"
    );
    assert!(
        reaches(owner, &places.lan, SERVICE_PORT),
        "no control: without the rules the Computer does not reach the LAN listener {}",
        places.lan
    );
    assert_eq!(
        tunnel_status(owner, &places.lan, SERVICE_PORT),
        Some(200),
        "no control: without the rules the Exit Proxy does not reach the LAN listener {}",
        places.lan
    );
    // The Exit Proxy refuses a link-local address with or without the
    // rules (ADR-0029).
    assert_eq!(
        tunnel_status(owner, METADATA, SERVICE_PORT),
        Some(403),
        "the Exit Proxy did not refuse the metadata service {METADATA}"
    );
    assert!(
        reaches(owner, host, places.host_port),
        "no control: without the rules the Computer does not reach the Docker host at {host}:{}",
        places.host_port
    );
    assert!(
        echoes(owner, host, places.media.last + 1),
        "no control: without the rules the Computer does not reach UDP {host}:{}",
        places.media.last + 1
    );

    rules.install(&places.bridge, &places.media, "");

    assert!(
        !reaches(owner, METADATA, SERVICE_PORT),
        "the Computer reached the metadata service {METADATA}"
    );
    assert_eq!(
        tunnel_status(owner, METADATA, SERVICE_PORT),
        Some(403),
        "the Exit Proxy did not refuse the metadata service {METADATA}"
    );
    assert_ne!(
        tunnel_status(owner, &places.lan, SERVICE_PORT),
        Some(200),
        "the Exit Proxy reached the LAN address {}",
        places.lan
    );
    assert!(
        !reaches(owner, &places.lan, SERVICE_PORT),
        "the Computer reached the LAN address {}",
        places.lan
    );
    assert!(
        !reaches(owner, host, places.host_port),
        "the Computer reached TCP port {} of the Docker host",
        places.host_port
    );
    assert!(
        !echoes(owner, host, places.media.last + 1),
        "the Computer reached UDP port {} of the Docker host, outside the media range",
        places.media.last + 1
    );
    assert!(
        echoes(owner, host, places.media.last),
        "the Computer did not reach the Media Relay at UDP {host}:{}",
        places.media.last
    );
    assert!(
        resolves(owner, PUBLIC),
        "the Computer does not resolve {PUBLIC}"
    );
    assert!(
        reaches(owner, PUBLIC, 443),
        "the Computer does not reach {PUBLIC}:443"
    );
    assert_eq!(
        tunnel_status(owner, PUBLIC, 443),
        Some(200),
        "the Exit Proxy does not reach {PUBLIC}:443"
    );
}

/// A Computer reaches the public internet and the Media Relay, and not
/// the metadata service of a cloud, an address on the LAN or another
/// port of the Docker host. The daemon still reaches its control port.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_computer_reaches_the_internet_and_the_media_relay_and_nothing_private() {
    let rules = HostRules::clean();
    let real = Real::new();
    let owner = real.owner(&AgentId::generate());
    let computer = real
        .runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC"))
        .await
        .expect("the container boots");

    assert_the_policy(&real, &owner, &rules);

    // The answers of the control port come back through the INPUT chain.
    let frame = real
        .runtime
        .fetch_frame(&computer)
        .await
        .expect("the daemon reads a frame under the rules");
    assert!(!frame.is_empty());
}

/// The Plugin Computer holds the Plugin secrets, and it is on the Tenant
/// Network of its Workspace like every Computer, so the same rules hold
/// it.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_plugin_computer_has_the_same_policy() {
    let rules = HostRules::clean();
    let real = Real::new();
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    manager
        .ensure_plugin_computer(Vec::new())
        .await
        .expect("the plugin computer wakes");

    assert_the_policy(&real, &real.owner(&pagis_computer::plugin_agent()), &rules);
}

/// Tries to remove the rules of the Docker host from inside a Computer.
const ATTACK: &str = r#"
for t in iptables-nft iptables-legacy; do
  "$t" -D DOCKER-USER 1
  "$t" -F DOCKER-USER
  "$t" -F INPUT
  "$t" -F PAGIS-FORWARD
  "$t" -F PAGIS-INPUT
  "$t" -F
  "$t" -P INPUT ACCEPT
done
exit 0
"#;

/// An Administrator's block is reachable, and it opens nothing else.
/// Root inside the Computer cannot change the rules, because they are
/// the Docker host's.
///
/// The Computer image has no iptables, so root in the Computer gets the
/// iptables of the egress image: a container that joins the network
/// namespace of the Computer as root, first with the capabilities of
/// the Computer and then with NET_ADMIN as well.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn an_allowed_private_block_is_reachable_and_root_in_the_computer_cannot_change_the_rules() {
    let rules = HostRules::clean();
    let real = Real::new();
    let owner = real.owner(&AgentId::generate());
    real.runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC"))
        .await
        .expect("the container boots");
    let places = Places::start(&real);
    assert!(
        reaches(&owner, METADATA, SERVICE_PORT),
        "no control: without the rules the Computer does not reach {METADATA}"
    );

    rules.install(
        &places.bridge,
        &places.media,
        &format!("192.0.2.0/24, {}", places.lan_block),
    );

    assert!(
        reaches(&owner, &places.lan, SERVICE_PORT),
        "the Computer did not reach {} in the allowed block {}",
        places.lan,
        places.lan_block
    );
    assert!(
        !reaches(&owner, METADATA, SERVICE_PORT),
        "the allow list opened {METADATA}"
    );

    let installed = host_rules();
    for capabilities in [&[][..], &["--cap-add", "NET_ADMIN"][..]] {
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "-i", "--user", "root", "--network"])
            .arg(format!("container:{}", owner.container_name()))
            .args(capabilities)
            .args(["--entrypoint", "sh", egress_image(), "-s"]);
        let tried = with_stdin(command, ATTACK.as_bytes());
        assert!(
            tried.status.success(),
            "the attack did not run: {}",
            String::from_utf8_lossy(&tried.stderr)
        );
    }

    assert_eq!(
        host_rules(),
        installed,
        "a command in the Computer changed the rules of the Docker host"
    );
    assert!(
        !reaches(&owner, METADATA, SERVICE_PORT),
        "the Computer reached {METADATA} after its attack on the rules"
    );
}

/// Each run flushes and rebuilds the Pagis chains, so a second run
/// leaves one copy of each rule, and a run with other settings leaves
/// the rules of those settings alone.
#[test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
fn a_second_run_leaves_one_copy_of_each_rule() {
    let rules = HostRules::clean();
    // Bridges that do not exist: the rules match no packet of another
    // test.
    let bridge = format!("pgt{:08x}", rand::random::<u32>());
    let media = MediaRange {
        first: 50_000,
        last: 50_019,
    };

    rules.install(&bridge, &media, "10.20.0.0/16");
    let first = host_rules();
    rules.install(&bridge, &media, "10.20.0.0/16");
    let second = host_rules();

    assert_eq!(first, second);
    let mut unique = second.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        second.len(),
        "a rule is there twice: {second:#?}"
    );
    let jumps = [
        format!("-A DOCKER-USER -i {bridge} -j PAGIS-FORWARD"),
        format!("-A INPUT -i {bridge} -j PAGIS-INPUT"),
    ];
    for jump in &jumps {
        assert!(second.contains(jump), "no {jump:?} in {second:#?}");
    }
    for block in [
        "169.254.0.0/16",
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "100.64.0.0/10",
    ] {
        let drop = format!("-A PAGIS-FORWARD -d {block} -j DROP");
        assert!(second.contains(&drop), "no {drop:?} in {second:#?}");
    }
    assert!(
        second.contains(&"-A PAGIS-FORWARD -d 10.20.0.0/16 -j RETURN".to_string()),
        "the allowed block is not in {second:#?}"
    );
    assert!(
        second.contains(&"-A PAGIS-INPUT -p udp -m udp --dport 50000:50019 -j RETURN".to_string()),
        "the media range is not in {second:#?}"
    );

    let other = format!("pgt{:08x}", rand::random::<u32>());
    rules.install(&other, &media, "");
    let third = host_rules();
    let jumps_now: Vec<&String> = third
        .iter()
        .filter(|rule| rule.contains(" -j PAGIS-"))
        .collect();
    assert_eq!(
        jumps_now,
        [
            &format!("-A DOCKER-USER -i {other} -j PAGIS-FORWARD"),
            &format!("-A INPUT -i {other} -j PAGIS-INPUT"),
        ],
        "{third:#?}"
    );
    assert!(
        !third.iter().any(|rule| rule.contains("10.20.0.0/16")),
        "the block of the earlier run stays: {third:#?}"
    );
}

/// A setting that the script cannot read stops it before it changes a
/// rule, and the error names the setting.
#[test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
fn the_script_refuses_a_setting_it_cannot_read_and_installs_nothing() {
    let _rules = HostRules::clean();
    let first = ("PAGIS_MEDIA_PORT_FIRST", "50000");
    let last = ("PAGIS_MEDIA_PORT_LAST", "50019");

    for (env, named) in [
        (vec![first], "PAGIS_MEDIA_PORT_LAST"),
        (
            vec![("PAGIS_MEDIA_PORT_FIRST", "fifty"), last],
            "PAGIS_MEDIA_PORT_FIRST",
        ),
        (
            vec![first, ("PAGIS_MEDIA_PORT_LAST", "49999")],
            "PAGIS_MEDIA_PORT_LAST",
        ),
        (
            vec![
                first,
                last,
                ("PAGIS_COMPUTER_ALLOW", "10.0.0.0/8, printer.lan"),
            ],
            "printer.lan",
        ),
    ] {
        let output = run_egress(&env);
        let error = String::from_utf8_lossy(&output.stderr);

        assert!(!output.status.success(), "the script took {env:?}");
        assert!(
            error.contains(named),
            "the error for {env:?} does not name {named}: {error}"
        );
        assert_eq!(
            host_rules(),
            Vec::<String>::new(),
            "the script changed a rule for {env:?}"
        );
    }
}

/// The deployment installs the rules before the daemon starts a
/// Computer, from a service that the daemon's reach does not include,
/// and it installs them again at each start of the VM.
#[test]
fn the_daemon_starts_after_the_egress_rules_are_in_place() {
    let config = compose();
    let egress = &config["services"]["egress"];

    assert_eq!(egress["network_mode"], "host", "{egress}");
    assert_eq!(
        egress["cap_add"],
        serde_json::json!(["NET_ADMIN"]),
        "{egress}"
    );
    // Docker starts a service with this policy again when the VM boots,
    // and a boot clears the rules.
    assert_eq!(egress["restart"], "unless-stopped", "{egress}");
    for setting in [
        "PAGIS_MEDIA_PORT_FIRST",
        "PAGIS_MEDIA_PORT_LAST",
        "PAGIS_COMPUTER_ALLOW",
    ] {
        assert!(
            egress["environment"][setting].is_string(),
            "the egress service does not take {setting}: {egress}"
        );
    }
    let sources: Vec<&str> = egress["volumes"]
        .as_array()
        .expect("the egress service mounts its script")
        .iter()
        .filter_map(|volume| volume["source"].as_str())
        .collect();
    assert!(
        !sources.iter().any(|source| source.contains("docker.sock")),
        "the egress service reaches the Docker socket: {sources:?}"
    );
    assert_eq!(
        config["services"]["pagis"]["depends_on"]["egress"]["condition"], "service_healthy",
        "{}",
        config["services"]["pagis"]["depends_on"]
    );
}
