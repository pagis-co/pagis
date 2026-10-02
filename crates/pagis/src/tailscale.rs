//! The `tailscale` command of this machine (ADR-0028), which the Remote
//! Access switch of a Local Installation drives. The daemon runs it as the
//! owner, as it runs `git`, and reads its JSON:
//!
//! - `tailscale status --json` says whether Tailscale runs and is signed
//!   in, the name of this machine on the tailnet, and whether the tailnet
//!   allows HTTPS and Funnel (the `https` and `funnel` capabilities).
//! - `tailscale funnel status --json` says what ports 443 and 8443 serve.
//!
//! `tailscale funnel --bg --yes --https=443 http://127.0.0.1:<port>` turns
//! Funnel on for the product port. Where the tailnet has HTTPS or Funnel
//! off, it prints a page of `login.tailscale.com` that turns them on and
//! waits for the approval, so the driver reads its output as it comes.
//! `tailscale funnel --bg --yes --tls-terminated-tcp=8443
//! tcp://127.0.0.1:<turn port>` then publishes the TURN server of the live
//! screen: `tailscaled` ends TLS on port 8443 and forwards plain TCP.
//! `tailscale funnel --https=443 off` and `tailscale funnel
//! --tls-terminated-tcp=8443 off` remove them.
//!
//! The command is the first `tailscale` on `PATH`, else the command inside
//! the macOS app, because a daemon that the Client App starts may have no
//! `PATH` that names the command line tool.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use pagis_server::{FUNNEL_TURN_PORT, FunnelPort, FunnelTargets, Tailscale, TailscaleState};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

/// The command inside the app that the standalone build and the Mac App
/// Store build of Tailscale install on macOS.
const MACOS_APP_COMMAND: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";

/// How long a read of the state may take. The command answers from the
/// local daemon of Tailscale and touches no network.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a turn-on waits for the owner to approve HTTPS and Funnel on
/// the page that Tailscale names.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How many lines of the output of a failed command its failure keeps.
const FAILURE_LINES: usize = 4;

/// Where the owner gets Tailscale for this system. On macOS it is the
/// standalone build, which serves Funnel; Tailscale's documents disagree
/// on whether the Mac App Store build does.
fn install_url() -> &'static str {
    if cfg!(target_os = "macos") {
        "https://pkgs.tailscale.com/stable/#macos"
    } else {
        "https://tailscale.com/download/linux"
    }
}

/// The `tailscale` command of this machine.
pub struct TailscaleCommand {
    /// The directories that the command is looked for in, as `PATH` lists
    /// them. `None` reads the `PATH` of the daemon at each call, so a
    /// Tailscale installed while Pagis runs is found.
    path: Option<OsString>,
    /// The command inside the macOS app, looked for after `path`.
    app: Option<PathBuf>,
}

impl TailscaleCommand {
    /// The command as the daemon finds it on this machine.
    pub fn of_this_machine() -> Self {
        Self {
            path: None,
            app: cfg!(target_os = "macos").then(|| PathBuf::from(MACOS_APP_COMMAND)),
        }
    }

    /// The command looked for in `path` alone, for a test that puts a
    /// script there.
    pub fn in_path(path: OsString) -> Self {
        Self {
            path: Some(path),
            app: None,
        }
    }

    fn program(&self) -> Option<PathBuf> {
        let path = self.path.clone().or_else(|| std::env::var_os("PATH"));
        locate(path.as_deref(), self.app.as_deref())
    }

    /// The state against the loopback ports `targets`, with the command
    /// that reads it.
    async fn read_state(&self, program: &Path, targets: FunnelTargets) -> TailscaleState {
        let status = match run(program, &["status", "--json"], READ_TIMEOUT).await {
            Ok(output) => output,
            Err(reason) => {
                return TailscaleState::NotRunning {
                    detail: Some(reason),
                };
            }
        };
        // `tailscale status --json` writes the state where it can, also
        // with a status that is not zero, so the JSON decides first.
        let status = match read_status(&status.stdout) {
            Ok(read) => read,
            Err(error) => {
                return TailscaleState::NotRunning {
                    detail: Some(status.failure().unwrap_or(format!(
                        "Pagis cannot read the answer of `tailscale status --json`: {error}"
                    ))),
                };
            }
        };
        if !status.running {
            return TailscaleState::NotRunning { detail: None };
        }
        let serve = match run(program, &["funnel", "status", "--json"], READ_TIMEOUT).await {
            Ok(output) => match output.failure() {
                None => output.stdout,
                Some(reason) => {
                    return TailscaleState::NotRunning {
                        detail: Some(reason),
                    };
                }
            },
            Err(reason) => {
                return TailscaleState::NotRunning {
                    detail: Some(reason),
                };
            }
        };
        let ports = port_443(&serve, &status.dns_name, targets.product).and_then(|port_443| {
            Ok((port_443, port_8443(&serve, &status.dns_name, targets.turn)?))
        });
        let (port_443, port_8443) = match ports {
            Ok(ports) => ports,
            Err(error) => {
                return TailscaleState::NotRunning {
                    detail: Some(format!(
                        "Pagis cannot read the answer of `tailscale funnel status --json`: {error}"
                    )),
                };
            }
        };
        if status.funnel_allowed && !status.dns_name.is_empty() {
            TailscaleState::Ready {
                dns_name: status.dns_name,
                port_443,
                port_8443,
            }
        } else {
            TailscaleState::FunnelOff {
                port_443,
                port_8443,
            }
        }
    }
}

#[async_trait::async_trait]
impl Tailscale for TailscaleCommand {
    async fn state(&self, targets: FunnelTargets) -> TailscaleState {
        match self.program() {
            Some(program) => self.read_state(&program, targets).await,
            None => TailscaleState::NotInstalled {
                install_url: install_url().to_string(),
            },
        }
    }

    /// The product port first: its command is the one that waits where
    /// the tailnet has Funnel off. Then the TURN server, on a tailnet that
    /// allows Funnel by then.
    async fn funnel_on(
        &self,
        targets: FunnelTargets,
        enable_url: &(dyn Fn(String) + Send + Sync),
    ) -> Result<(), String> {
        let program = self
            .program()
            .ok_or_else(|| "Tailscale is not installed on this computer".to_string())?;
        let product = product_target(targets.product);
        let product_args = ["funnel", "--bg", "--yes", "--https=443", product.as_str()];
        let turn = format!("tcp://{}", turn_target(targets.turn));
        let turn_port = turn_port_flag();
        let turn_args = ["funnel", "--bg", "--yes", turn_port.as_str(), turn.as_str()];
        let both = async {
            run_streaming(&program, &product_args, enable_url).await?;
            run_streaming(&program, &turn_args, enable_url).await
        };
        match tokio::time::timeout(APPROVAL_TIMEOUT, both).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => Err(refused(&reason)),
            // The child goes with the future, and the runtime stops it.
            Err(_) => Err(format!(
                "Tailscale waited {} minutes for the approval of HTTPS and Funnel; turn on \
                 Remote Access again",
                APPROVAL_TIMEOUT.as_secs() / 60
            )),
        }
    }

    /// Each port that serves Pagis goes, also where the other one fails
    /// to go.
    async fn funnel_off(&self, targets: FunnelTargets) -> Result<(), String> {
        let Some(program) = self.program() else {
            return Ok(());
        };
        let state = self.read_state(&program, targets).await;
        let turn_port = turn_port_flag();
        let mut removals = Vec::new();
        if state.port_443() == Some(&FunnelPort::Pagis) {
            removals.push(["funnel", "--https=443", "off"]);
        }
        if state.port_8443() == Some(&FunnelPort::Pagis) {
            removals.push(["funnel", turn_port.as_str(), "off"]);
        }
        let mut failures = Vec::new();
        for args in removals {
            let removed = match run(&program, &args, READ_TIMEOUT).await {
                Ok(output) => output.failure().map_or(Ok(()), Err),
                Err(reason) => Err(reason),
            };
            if let Err(reason) = removed {
                failures.push(reason);
            }
        }
        match failures.is_empty() {
            true => Ok(()),
            false => Err(failures.join("; ")),
        }
    }
}

/// The failure of `tailscale funnel`, with the build that serves Funnel on
/// macOS.
fn refused(reason: &str) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "{reason}. The standalone build of Tailscale for macOS serves Funnel, and the Mac \
             App Store build may not: {}",
            install_url()
        )
    } else {
        reason.to_string()
    }
}

/// What Funnel forwards port 443 to: the product port on loopback.
fn product_target(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// What Funnel forwards port 8443 to, as Tailscale writes it: the TURN
/// server on loopback.
fn turn_target(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

/// The flag of `tailscale funnel` that ends TLS on port 8443 and forwards
/// plain TCP.
fn turn_port_flag() -> String {
    format!("--tls-terminated-tcp={FUNNEL_TURN_PORT}")
}

/// The first `tailscale` in the directories of `path`, else `app` where
/// it exists.
fn locate(path: Option<&std::ffi::OsStr>, app: Option<&Path>) -> Option<PathBuf> {
    path.into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join("tailscale"))
        .chain(app.map(Path::to_path_buf))
        .find(|candidate| candidate.is_file())
}

/// What `tailscale status --json` says about this machine.
#[derive(Debug, PartialEq, Eq)]
struct Status {
    /// Tailscale runs and is signed in.
    running: bool,
    /// The name of this machine on the tailnet, with no trailing dot.
    dns_name: String,
    /// The tailnet gives this machine the `https` and `funnel`
    /// capabilities.
    funnel_allowed: bool,
}

fn read_status(json: &str) -> Result<Status, serde_json::Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Answer {
        backend_state: String,
        #[serde(rename = "Self")]
        this_machine: Option<Node>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Node {
        #[serde(rename = "DNSName", default)]
        dns_name: String,
        #[serde(default)]
        capabilities: Option<Vec<String>>,
        #[serde(default)]
        cap_map: Option<BTreeMap<String, serde_json::Value>>,
    }

    let answer: Answer = serde_json::from_str(json)?;
    let node = answer.this_machine;
    // Tailscale lists the capabilities in `Capabilities` and in the keys
    // of `CapMap`, and moves from the first to the second.
    let has = |capability: &str| {
        node.as_ref().is_some_and(|node| {
            node.capabilities
                .iter()
                .flatten()
                .any(|held| held == capability)
                || node
                    .cap_map
                    .as_ref()
                    .is_some_and(|map| map.contains_key(capability))
        })
    };
    Ok(Status {
        running: answer.backend_state == "Running",
        dns_name: node
            .as_ref()
            .map(|node| node.dns_name.trim_end_matches('.').to_string())
            .unwrap_or_default(),
        funnel_allowed: has("https") && has("funnel"),
    })
}

/// The part of `tailscale funnel status --json` that the driver reads: the
/// `ipn.ServeConfig` of this machine.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct ServeConfig {
    #[serde(rename = "TCP")]
    tcp: BTreeMap<String, TcpHandler>,
    web: BTreeMap<String, WebServer>,
    allow_funnel: BTreeMap<String, bool>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct TcpHandler {
    #[serde(rename = "TCPForward")]
    tcp_forward: String,
    /// The name whose TLS `tailscaled` ends before it forwards, or empty
    /// where it forwards the TLS as it comes.
    #[serde(rename = "TerminateTLS")]
    terminate_tls: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct WebServer {
    handlers: BTreeMap<String, Handler>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Handler {
    proxy: String,
    path: String,
    text: String,
    redirect: String,
}

impl Handler {
    /// What the handler at `mount` serves, as the switch names it.
    fn describe(&self, mount: &str) -> String {
        let serves = [&self.proxy, &self.path, &self.text, &self.redirect]
            .into_iter()
            .find(|value| !value.is_empty())
            .map(String::as_str)
            .unwrap_or_default();
        format!("{mount} {serves}").trim().to_string()
    }
}

/// What port 443 of `dns_name` serves, from `tailscale funnel status
/// --json`, against the product port `port`.
///
/// Pagis serves port 443 only where Funnel publishes exactly one handler
/// there, `/`, to the product port. A handler of the product port that the
/// tailnet alone reaches is nothing that Funnel publishes, and turning
/// Funnel on keeps it.
fn port_443(json: &str, dns_name: &str, port: u16) -> Result<FunnelPort, serde_json::Error> {
    let config: ServeConfig = serde_json::from_str(json)?;
    if let Some(forward) = config
        .tcp
        .get("443")
        .map(|handler| handler.tcp_forward.as_str())
        .filter(|forward| !forward.is_empty())
    {
        return Ok(FunnelPort::Other {
            target: forward.to_string(),
        });
    }
    let host_port = format!("{dns_name}:443");
    let handlers = config
        .web
        .get(&host_port)
        .map(|web| &web.handlers)
        .filter(|handlers| !handlers.is_empty());
    let Some(handlers) = handlers else {
        return Ok(FunnelPort::Nothing);
    };
    let target = product_target(port);
    let other = handlers
        .iter()
        .find(|(mount, handler)| !(mount.as_str() == "/" && handler.proxy == target));
    if let Some((mount, handler)) = other {
        return Ok(FunnelPort::Other {
            target: handler.describe(mount),
        });
    }
    Ok(match config.allow_funnel.get(&host_port) {
        Some(true) => FunnelPort::Pagis,
        _ => FunnelPort::Nothing,
    })
}

/// What port 8443 of `dns_name` serves, from `tailscale funnel status
/// --json`, against the TURN server at `port` on loopback.
///
/// Pagis serves port 8443 only where Funnel publishes there one TCP
/// forward that ends the TLS of `dns_name`, to the TURN server. A forward
/// that the tailnet alone reaches is nothing that Funnel publishes, and
/// turning Funnel on keeps it. Any other forward, and a web server on port
/// 8443, is something else.
fn port_8443(json: &str, dns_name: &str, port: u16) -> Result<FunnelPort, serde_json::Error> {
    let config: ServeConfig = serde_json::from_str(json)?;
    let port_key = FUNNEL_TURN_PORT.to_string();
    let host_port = format!("{dns_name}:{FUNNEL_TURN_PORT}");
    if let Some(handler) = config
        .tcp
        .get(&port_key)
        .filter(|handler| !handler.tcp_forward.is_empty())
    {
        if handler.tcp_forward != turn_target(port) || handler.terminate_tls != dns_name {
            return Ok(FunnelPort::Other {
                target: handler.tcp_forward.clone(),
            });
        }
        return Ok(match config.allow_funnel.get(&host_port) {
            Some(true) => FunnelPort::Pagis,
            _ => FunnelPort::Nothing,
        });
    }
    let web = config
        .web
        .get(&host_port)
        .and_then(|web| web.handlers.iter().next());
    Ok(match web {
        Some((mount, handler)) => FunnelPort::Other {
            target: handler.describe(mount),
        },
        None => FunnelPort::Nothing,
    })
}

/// The page of `login.tailscale.com` that a line of `tailscale funnel`
/// names, where the owner turns on HTTPS or Funnel for the tailnet.
fn enable_url(line: &str) -> Option<&str> {
    line.split_whitespace()
        .find(|word| word.starts_with("https://login.tailscale.com/"))
}

/// The output of one command.
struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

impl Output {
    /// Why the command failed, in its own words, or `None` where it ended
    /// with status 0.
    fn failure(&self) -> Option<String> {
        if self.success {
            return None;
        }
        let said = [&self.stderr, &self.stdout]
            .into_iter()
            .flat_map(|text| text.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it gave no reason");
        Some(format!("the tailscale command failed: {said}"))
    }
}

/// Run one command that ends by itself, with no input.
async fn run(program: &Path, args: &[&str], timeout: Duration) -> Result<Output, String> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(timeout, output)
        .await
        .map_err(|_| {
            format!(
                "`tailscale {}` did not answer in {} seconds",
                args.join(" "),
                timeout.as_secs()
            )
        })?
        .map_err(|error| format!("Pagis cannot run {}: {error}", program.display()))?;
    Ok(Output {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Run `tailscale funnel` and read both of its outputs as they come. Each
/// page that it names goes to `enable_url`. It succeeds where the command
/// ends with status 0.
async fn run_streaming(
    program: &Path,
    args: &[&str],
    enable_url: &(dyn Fn(String) + Send + Sync),
) -> Result<(), String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Pagis cannot run {}: {error}", program.display()))?;
    let stdout = lines_of(child.stdout.take());
    let stderr = lines_of(child.stderr.take());
    let mut lines = std::pin::pin!(futures::stream::select(stdout, stderr));
    let mut last = VecDeque::with_capacity(FAILURE_LINES);
    while let Some(line) = lines.next().await {
        if let Some(url) = self::enable_url(&line) {
            enable_url(url.to_string());
        }
        let line = line.trim();
        if !line.is_empty() {
            if last.len() == FAILURE_LINES {
                last.pop_front();
            }
            last.push_back(line.to_string());
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("Pagis lost the tailscale command: {error}"))?;
    if status.success() {
        return Ok(());
    }
    let said = Vec::from(last).join(" ");
    Err(format!(
        "`tailscale {}` failed: {}",
        args.join(" "),
        if said.is_empty() {
            status.to_string()
        } else {
            said
        }
    ))
}

/// The lines of one output of a child, as they come, until it closes.
fn lines_of<R>(output: Option<R>) -> impl futures::Stream<Item = String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    futures::stream::unfold(
        output.map(|output| BufReader::new(output).lines()),
        |lines| async move {
            let mut lines = lines?;
            let line = lines.next_line().await.ok().flatten()?;
            Some((line, Some(lines)))
        },
    )
}

#[cfg(test)]
mod tests {
    //! The fixtures under `tests/fixtures/tailscale` hold the JSON of
    //! Tailscale 1.102.4. `status-stopped.json` and
    //! `funnel-status-empty.json` are recorded on a Mac, with the names,
    //! the addresses and the keys replaced. The other `status-*` files
    //! change the `BackendState` and the capabilities of that recording.
    //! The `funnel-status-*` and `serve-status-*` files follow
    //! `ipn.ServeConfig`: `funnel-status-remote-access.json` is what the
    //! two commands of a turn-on leave, and `funnel-status-pagis.json` is
    //! port 443 alone. `funnel-on-enable.txt` follows the output of
    //! `tailscale funnel` where the tailnet has Funnel off.

    use std::sync::Mutex;

    use super::*;

    fn fixture(name: &str) -> String {
        let root = PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest directory"),
        );
        std::fs::read_to_string(root.join("tests/fixtures/tailscale").join(name))
            .unwrap_or_else(|error| panic!("read the fixture {name}: {error}"))
    }

    const DNS_NAME: &str = "owner-mac.tail1234.ts.net";

    /// The loopback ports of the fixtures: the product port and the TURN
    /// server.
    const TARGETS: FunnelTargets = FunnelTargets {
        product: 4400,
        turn: 4402,
    };

    #[test]
    fn a_running_tailnet_with_https_and_funnel_is_ready() {
        assert_eq!(
            read_status(&fixture("status-running.json")).unwrap(),
            Status {
                running: true,
                dns_name: DNS_NAME.to_string(),
                funnel_allowed: true,
            }
        );
    }

    #[test]
    fn a_stopped_or_signed_out_tailscale_does_not_run() {
        for name in ["status-stopped.json", "status-needs-login.json"] {
            assert!(!read_status(&fixture(name)).unwrap().running, "{name}");
        }
    }

    #[test]
    fn a_tailnet_without_the_https_and_funnel_capabilities_has_funnel_off() {
        let status = read_status(&fixture("status-funnel-off.json")).unwrap();

        assert!(status.running);
        assert!(!status.funnel_allowed);
    }

    /// A capability counts in `Capabilities` or in the keys of `CapMap`,
    /// and both `https` and `funnel` must be there.
    #[test]
    fn the_capabilities_count_in_either_list() {
        for (self_node, allowed) in [
            (r#"{"Capabilities": ["https", "funnel"]}"#, true),
            (r#"{"CapMap": {"https": null, "funnel": null}}"#, true),
            (
                r#"{"Capabilities": ["https"], "CapMap": {"funnel": null}}"#,
                true,
            ),
            (r#"{"Capabilities": ["https"]}"#, false),
            (r#"{"CapMap": {"funnel": null}}"#, false),
            (r#"{}"#, false),
        ] {
            let json = format!(r#"{{"BackendState": "Running", "Self": {self_node}}}"#);
            assert_eq!(
                read_status(&json).unwrap().funnel_allowed,
                allowed,
                "{self_node}"
            );
        }
    }

    #[test]
    fn an_answer_that_is_not_the_status_does_not_read() {
        assert!(read_status("not json").is_err());
        assert!(read_status(r#"{"Self": {}}"#).is_err());
    }

    #[test]
    fn port_443_says_what_funnel_serves() {
        for (name, serves) in [
            ("funnel-status-empty.json", FunnelPort::Nothing),
            ("funnel-status-pagis.json", FunnelPort::Pagis),
            ("funnel-status-remote-access.json", FunnelPort::Pagis),
            ("serve-status-pagis-tailnet-only.json", FunnelPort::Nothing),
            ("funnel-status-other-8443.json", FunnelPort::Nothing),
            (
                "funnel-status-other.json",
                FunnelPort::Other {
                    target: "/ http://127.0.0.1:3000".to_string(),
                },
            ),
            (
                "funnel-status-tcp-forward.json",
                FunnelPort::Other {
                    target: "127.0.0.1:5432".to_string(),
                },
            ),
        ] {
            assert_eq!(
                port_443(&fixture(name), DNS_NAME, 4400).unwrap(),
                serves,
                "{name}"
            );
        }
    }

    /// The Funnel of another product port is not this Pagis, and neither
    /// is a second handler beside the one of this Pagis.
    #[test]
    fn a_funnel_of_another_port_or_a_second_handler_is_something_else() {
        assert_eq!(
            port_443(&fixture("funnel-status-pagis.json"), DNS_NAME, 4500).unwrap(),
            FunnelPort::Other {
                target: "/ http://127.0.0.1:4400".to_string(),
            }
        );
        let two = format!(
            r#"{{"Web": {{"{DNS_NAME}:443": {{"Handlers": {{
                "/": {{"Proxy": "http://127.0.0.1:4400"}},
                "/files/": {{"Path": "/srv/files"}}
            }}}}}}, "AllowFunnel": {{"{DNS_NAME}:443": true}}}}"#
        );
        assert_eq!(
            port_443(&two, DNS_NAME, 4400).unwrap(),
            FunnelPort::Other {
                target: "/files/ /srv/files".to_string(),
            }
        );
    }

    /// Port 8443 serves Pagis where Funnel forwards it, with TLS ended, to
    /// the TURN server. A forward that the tailnet alone reaches is
    /// nothing, and so is port 443 alone.
    #[test]
    fn port_8443_says_what_funnel_serves() {
        for (name, serves) in [
            ("funnel-status-empty.json", FunnelPort::Nothing),
            ("funnel-status-remote-access.json", FunnelPort::Pagis),
            ("funnel-status-pagis.json", FunnelPort::Nothing),
            ("funnel-status-tcp-forward.json", FunnelPort::Nothing),
            ("serve-status-turn-tailnet-only.json", FunnelPort::Nothing),
            (
                "funnel-status-other-8443.json",
                FunnelPort::Other {
                    target: "127.0.0.1:5432".to_string(),
                },
            ),
            (
                "funnel-status-web-8443.json",
                FunnelPort::Other {
                    target: "/ http://127.0.0.1:8080".to_string(),
                },
            ),
        ] {
            assert_eq!(
                port_8443(&fixture(name), DNS_NAME, TARGETS.turn).unwrap(),
                serves,
                "{name}"
            );
        }
    }

    /// A forward to another port is not the TURN server of this Pagis, and
    /// neither is a forward of the TLS as it comes: the TURN server reads
    /// plain TCP.
    #[test]
    fn a_forward_to_another_port_or_without_tls_is_something_else() {
        assert_eq!(
            port_8443(&fixture("funnel-status-remote-access.json"), DNS_NAME, 4500).unwrap(),
            FunnelPort::Other {
                target: "127.0.0.1:4402".to_string(),
            }
        );
        let raw = format!(
            r#"{{"TCP": {{"8443": {{"TCPForward": "127.0.0.1:4402"}}}},
                "AllowFunnel": {{"{DNS_NAME}:8443": true}}}}"#
        );
        assert_eq!(
            port_8443(&raw, DNS_NAME, TARGETS.turn).unwrap(),
            FunnelPort::Other {
                target: "127.0.0.1:4402".to_string(),
            }
        );
    }

    /// The `tailscale` service of the compose deployment applies
    /// `deploy/tailscale-serve.json`, and the official image puts the name
    /// of the machine in place of `${TS_CERT_DOMAIN}`. The switch of a
    /// Local Installation reads that Funnel as the Funnel of Pagis on both
    /// ports, at the product port and the TURN server port of the
    /// deployment, so a Headless Server at home serves what the switch
    /// turns on.
    #[test]
    fn the_funnel_of_the_compose_deployment_is_the_funnel_of_pagis() {
        let root = PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest directory"),
        );
        let file = std::fs::read_to_string(root.join("../../deploy/tailscale-serve.json"))
            .expect("read deploy/tailscale-serve.json");
        assert!(file.contains("${TS_CERT_DOMAIN}"), "{file}");
        let serve = file.replace("${TS_CERT_DOMAIN}", DNS_NAME);
        let targets = FunnelTargets {
            product: crate::config::DEFAULT_PORT,
            turn: crate::config::DEFAULT_REMOTE_ACCESS_TURN_PORT,
        };

        assert_eq!(
            port_443(&serve, DNS_NAME, targets.product).unwrap(),
            FunnelPort::Pagis
        );
        assert_eq!(
            port_8443(&serve, DNS_NAME, targets.turn).unwrap(),
            FunnelPort::Pagis
        );
    }

    /// The handlers of another name of this machine are not port 443 of
    /// its tailnet name.
    #[test]
    fn port_443_of_another_name_is_not_this_one() {
        assert_eq!(
            port_443(
                &fixture("funnel-status-other.json"),
                "other.tail1234.ts.net",
                4400
            )
            .unwrap(),
            FunnelPort::Nothing
        );
    }

    #[test]
    fn the_output_of_funnel_names_the_page_that_turns_it_on() {
        let output = fixture("funnel-on-enable.txt");
        let urls: Vec<&str> = output.lines().filter_map(enable_url).collect();

        assert_eq!(
            urls,
            ["https://login.tailscale.com/f/funnel?node=nTEST000000CNTRL"]
        );
        assert_eq!(enable_url("https://owner-mac.tail1234.ts.net/"), None);
        assert_eq!(enable_url("Success."), None);
    }

    /// A `tailscale` of the test in a directory of its own: a shell
    /// script that writes each call to `calls` and answers it from
    /// `script`, a `case` on its arguments. The driver finds it on the
    /// `PATH` it is given, as it finds the real one.
    #[cfg(unix)]
    struct ScriptedCommand {
        directory: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl ScriptedCommand {
        fn new(script: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let directory = tempfile::tempdir().unwrap();
            let fixtures = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
                .join("tests/fixtures/tailscale");
            let calls = directory.path().join("calls");
            let command = directory.path().join("tailscale");
            std::fs::write(
                &command,
                format!(
                    "#!/bin/sh\nF={fixtures}\necho \"$*\" >> {calls}\ncase \"$*\" in\n{script}\nesac\n",
                    fixtures = fixtures.display(),
                    calls = calls.display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { directory }
        }

        fn driver(&self) -> TailscaleCommand {
            TailscaleCommand::in_path(self.directory.path().as_os_str().to_owned())
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.directory.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    #[tokio::test]
    async fn no_command_on_the_path_is_not_installed() {
        let empty = tempfile::tempdir().unwrap();

        let state = TailscaleCommand::in_path(empty.path().as_os_str().to_owned())
            .state(TARGETS)
            .await;

        assert!(
            matches!(state, TailscaleState::NotInstalled { .. }),
            "{state:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_driver_reads_the_state_through_the_command() {
        let command = ScriptedCommand::new(
            r#"  "status --json") cat $F/status-running.json ;;
  "funnel status --json") cat $F/funnel-status-remote-access.json ;;"#,
        );

        assert_eq!(
            command.driver().state(TARGETS).await,
            TailscaleState::Ready {
                dns_name: DNS_NAME.to_string(),
                port_443: FunnelPort::Pagis,
                port_8443: FunnelPort::Pagis,
            }
        );
        assert_eq!(command.calls(), ["status --json", "funnel status --json"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stopped_tailscale_or_one_that_does_not_answer_does_not_run() {
        let stopped = ScriptedCommand::new(r#"  "status --json") cat $F/status-stopped.json ;;"#);
        assert_eq!(
            stopped.driver().state(TARGETS).await,
            TailscaleState::NotRunning { detail: None }
        );

        let no_daemon = ScriptedCommand::new(
            r#"  "status --json") echo "failed to connect to local Tailscale service; is Tailscale running?" >&2; exit 1 ;;"#,
        );
        let TailscaleState::NotRunning {
            detail: Some(detail),
        } = no_daemon.driver().state(TARGETS).await
        else {
            panic!("a daemon that does not answer does not run");
        };
        assert!(detail.contains("is Tailscale running?"), "{detail}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tailnet_with_funnel_off_reads_as_funnel_off() {
        let command = ScriptedCommand::new(
            r#"  "status --json") cat $F/status-funnel-off.json ;;
  "funnel status --json") cat $F/funnel-status-empty.json ;;"#,
        );

        assert_eq!(
            command.driver().state(TARGETS).await,
            TailscaleState::FunnelOff {
                port_443: FunnelPort::Nothing,
                port_8443: FunnelPort::Nothing,
            }
        );
    }

    /// The turn-on runs the two commands of ADR-0028 in turn: port 443 to
    /// the product port, then port 8443 to the TURN server with TLS ended.
    /// It gives the page that the first prints while it waits, and
    /// succeeds on status 0.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_on_names_the_page_while_it_waits_and_ends_on_status_0() {
        let command = ScriptedCommand::new(
            r#"  "funnel --bg --yes --https=443 http://127.0.0.1:4400") cat $F/funnel-on-enable.txt ;;
  "funnel --bg --yes --tls-terminated-tcp=8443 tcp://127.0.0.1:4402") ;;"#,
        );
        let pages = Mutex::new(Vec::new());

        command
            .driver()
            .funnel_on(TARGETS, &|url| pages.lock().unwrap().push(url))
            .await
            .unwrap();

        assert_eq!(
            *pages.lock().unwrap(),
            ["https://login.tailscale.com/f/funnel?node=nTEST000000CNTRL"]
        );
        assert_eq!(
            command.calls(),
            [
                "funnel --bg --yes --https=443 http://127.0.0.1:4400",
                "funnel --bg --yes --tls-terminated-tcp=8443 tcp://127.0.0.1:4402"
            ]
        );
    }

    /// A turn-on whose second command fails says why, in the words of
    /// the command.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_on_that_cannot_publish_port_8443_gives_its_words() {
        let command = ScriptedCommand::new(
            r#"  "funnel --bg --yes --https=443 http://127.0.0.1:4400") ;;
  funnel*) echo "port 8443 is in use" >&2; exit 1 ;;"#,
        );

        let refused = command
            .driver()
            .funnel_on(TARGETS, &|_| {})
            .await
            .expect_err("the second command ended with status 1");

        assert!(refused.contains("port 8443 is in use"), "{refused}");
        assert_eq!(command.calls().len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_on_that_the_command_refuses_gives_its_words() {
        let command = ScriptedCommand::new(
            r#"  funnel*) echo "Funnel not available; "funnel" node attribute not set." >&2; exit 1 ;;"#,
        );

        let refused = command
            .driver()
            .funnel_on(TARGETS, &|_| {})
            .await
            .expect_err("the command ended with status 1");

        assert!(refused.contains("node attribute not set"), "{refused}");
        if cfg!(target_os = "macos") {
            assert!(refused.contains("standalone build"), "{refused}");
        }
    }

    /// A turn-off removes both ports of the Funnel of this Pagis, and
    /// leaves what a port serves for something else.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_off_removes_the_funnel_of_pagis_alone() {
        let ours = ScriptedCommand::new(
            r#"  "status --json") cat $F/status-running.json ;;
  "funnel status --json") cat $F/funnel-status-remote-access.json ;;
  "funnel --https=443 off") ;;
  "funnel --tls-terminated-tcp=8443 off") ;;"#,
        );
        ours.driver().funnel_off(TARGETS).await.unwrap();
        assert_eq!(
            ours.calls(),
            [
                "status --json",
                "funnel status --json",
                "funnel --https=443 off",
                "funnel --tls-terminated-tcp=8443 off"
            ]
        );

        let port_443_alone = ScriptedCommand::new(
            r#"  "status --json") cat $F/status-running.json ;;
  "funnel status --json") cat $F/funnel-status-pagis.json ;;
  "funnel --https=443 off") ;;"#,
        );
        port_443_alone.driver().funnel_off(TARGETS).await.unwrap();
        assert_eq!(
            port_443_alone.calls(),
            [
                "status --json",
                "funnel status --json",
                "funnel --https=443 off"
            ]
        );

        for theirs in ["funnel-status-other.json", "funnel-status-other-8443.json"] {
            let theirs = ScriptedCommand::new(&format!(
                r#"  "status --json") cat $F/status-running.json ;;
  "funnel status --json") cat $F/{theirs} ;;"#
            ));
            theirs.driver().funnel_off(TARGETS).await.unwrap();
            assert_eq!(theirs.calls(), ["status --json", "funnel status --json"]);
        }
    }

    /// A port that does not go still lets the other one go, and the
    /// turn-off says why the first stayed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_off_removes_one_port_where_the_other_fails() {
        let command = ScriptedCommand::new(
            r#"  "status --json") cat $F/status-running.json ;;
  "funnel status --json") cat $F/funnel-status-remote-access.json ;;
  "funnel --https=443 off") echo "access denied" >&2; exit 1 ;;
  "funnel --tls-terminated-tcp=8443 off") ;;"#,
        );

        let failed = command
            .driver()
            .funnel_off(TARGETS)
            .await
            .expect_err("port 443 stayed");

        assert!(failed.contains("access denied"), "{failed}");
        assert_eq!(
            command.calls().last().map(String::as_str),
            Some("funnel --tls-terminated-tcp=8443 off")
        );
    }

    #[test]
    fn the_command_is_on_path_first_then_in_the_app() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let app_command = app.path().join("Tailscale");
        std::fs::write(&app_command, "").unwrap();
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();

        assert_eq!(
            locate(Some(&path), Some(&app_command)),
            Some(app_command.clone())
        );
        assert_eq!(locate(Some(&path), None), None);
        assert_eq!(locate(None, Some(&app.path().join("Missing"))), None);

        std::fs::write(second.path().join("tailscale"), "").unwrap();
        assert_eq!(
            locate(Some(&path), Some(&app_command)),
            Some(second.path().join("tailscale"))
        );
    }
}
