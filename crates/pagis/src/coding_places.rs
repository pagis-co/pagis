//! The places of the Coding Sessions (ADR-0033): a Host, through the
//! session socket of its Client App, and the Agent's own Computer, with a
//! `docker exec` of the installed harness.
//!
//! In a Computer the harness runs as uid `agent`, in its session directory
//! under `/data/agent`, with the environment of `computer_shell`. No
//! credential enters the Computer (ADR-0005): the harness reaches the
//! Harness Model Endpoint with a token of its session, which each open
//! mints again, so a resume gets a new token and the old one stops.
//!
//! The end of the harness's stdout is the end of the process. The exec
//! reports no exit code, so the exit holds only the last 4 KB of stderr.
//! When the container no longer runs at that time, the Computer stopped,
//! and the place is lost: the session is `interrupted`.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use pagis_broker::HostSessions;
use pagis_broker::host_sessions::STDERR_TAIL_LIMIT;
use pagis_coding::{
    OpenFailure, OpenFailureCode, OpenRequest, OpenedStream, Place, SessionExit, SessionPlace,
    open_on_host,
};
use pagis_computer::{
    ComputerManager, ComputerManagers, ExecPin, ExecRequest, OutputCap, SHELL_USER, ShellCommand,
};
use pagis_core::{AgentId, WorkspaceId, harness};
use pagis_server::HarnessModelTokens;
use tokio::sync::{mpsc, oneshot};

/// How long the directory of a session may take to make.
const MKDIR_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the exit waits for the rest of stderr after the end of
/// stdout. Docker ends both streams together, so the wait is short.
const STDERR_GRACE: Duration = Duration::from_secs(2);

/// The places of the Coding Sessions of the daemon.
pub struct SessionPlaces {
    hosts: Arc<HostSessions>,
    computers: ComputerPlace,
}

impl SessionPlaces {
    pub fn new(hosts: Arc<HostSessions>, computers: ComputerPlace) -> Self {
        Self { hosts, computers }
    }
}

#[async_trait]
impl SessionPlace for SessionPlaces {
    async fn open(
        &self,
        workspace_id: &WorkspaceId,
        place: &Place,
        request: OpenRequest,
    ) -> Result<OpenedStream, OpenFailure> {
        match place {
            Place::Host(host_id) => open_on_host(&self.hosts, workspace_id, host_id, request).await,
            Place::Computer(agent_id) => self.computers.open(workspace_id, agent_id, request).await,
        }
    }
}

/// The place of a Coding Session in the Agent's own Computer.
pub struct ComputerPlace {
    computers: Arc<ComputerManagers>,
    tokens: HarnessModelTokens,
    /// The port of the Harness Model Endpoint, or `None` when the daemon
    /// serves none.
    model_port: Option<u16>,
}

impl ComputerPlace {
    pub fn new(
        computers: Arc<ComputerManagers>,
        tokens: HarnessModelTokens,
        model_port: Option<u16>,
    ) -> Self {
        Self {
            computers,
            tokens,
            model_port,
        }
    }

    /// Makes the session directory, mints the token of the session, and
    /// starts the harness that `request` names.
    async fn open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        request: OpenRequest,
    ) -> Result<OpenedStream, OpenFailure> {
        let entry = harness::computer_sessions()
            .find(|entry| {
                entry
                    .computer
                    .is_some_and(|launch| launch.program == request.command)
            })
            .ok_or_else(|| {
                spawn_failed(format!(
                    "{} is not a Coding Harness that runs in a Computer",
                    request.command
                ))
            })?;
        let port = self.model_port.ok_or_else(|| {
            spawn_failed("this daemon serves no Harness Model Endpoint".to_string())
        })?;
        let manager = self.computers.get(workspace_id);
        make_directory(&manager, agent_id, &request.cwd).await?;
        let token = self
            .tokens
            .mint(workspace_id, &request.session_id)
            .await
            .map_err(|error| spawn_failed(error.to_string()))?
            .ok_or_else(|| spawn_failed("the Coding Session is gone".to_string()))?;
        let exec = harness_request(
            entry.id,
            &request,
            manager.shell_env(agent_id),
            &pagis_computer::model_endpoint(port),
            &token,
        );
        let started = manager
            .harness(agent_id, exec)
            .await
            .map_err(|error| spawn_failed(error.to_string()))?;
        let (ended, end) = oneshot::channel();
        let (exited, exit) = oneshot::channel();
        tokio::spawn(report_exit(
            manager,
            agent_id.clone(),
            started.stream.stderr,
            end,
            exited,
        ));
        Ok(OpenedStream {
            stream: Box::new(HarnessStream {
                stdin: started.stream.stdin,
                stdout: started.stream.stdout,
                ended: Some(ended),
                _pin: started.pin,
            }),
            cwd: request.cwd,
            exit,
        })
    }
}

fn spawn_failed(message: String) -> OpenFailure {
    OpenFailure {
        code: OpenFailureCode::SpawnFailed,
        message,
    }
}

/// Makes the directory of a session as uid `agent`, as the Plugin
/// Computer makes the directory of a server. The Computer wakes first.
async fn make_directory(
    manager: &Arc<ComputerManager>,
    agent_id: &AgentId,
    directory: &str,
) -> Result<(), OpenFailure> {
    let outcome = manager
        .shell(
            agent_id,
            ShellCommand {
                command: format!("mkdir -p {}", shell_quote(directory)),
                timeout: MKDIR_TIMEOUT,
                cwd: None,
                stdin: None,
                output_cap: None,
            },
        )
        .await
        .map_err(|error| spawn_failed(error.to_string()))?;
    if outcome.exit_code != 0 {
        return Err(OpenFailure {
            code: OpenFailureCode::BadDirectory,
            message: format!(
                "the directory {directory} could not be made: {}",
                outcome.stderr.trim()
            ),
        });
    }
    Ok(())
}

/// The exec of a harness in a Computer: the program of the open request
/// as uid `agent` in its directory, with the environment of a shell
/// command and the variables that point the harness at the Harness Model
/// Endpoint. The environment holds the token of the session and no
/// provider key.
fn harness_request(
    harness_id: &str,
    request: &OpenRequest,
    shell_env: Vec<String>,
    endpoint: &str,
    token: &str,
) -> ExecRequest {
    let mut argv = vec![request.command.clone()];
    argv.extend(request.args.iter().cloned());
    let mut env = shell_env;
    env.extend(model_env(harness_id, endpoint, token));
    ExecRequest {
        argv,
        user: SHELL_USER.to_string(),
        cwd: request.cwd.clone(),
        env,
        stdin: None,
        // A stream has no head and tail cap: the session reads stdout to
        // its end, and the exit keeps the tail of stderr.
        output_cap: OutputCap { head: 0, tail: 0 },
    }
}

/// The variables that point a harness at the Harness Model Endpoint.
/// Claude Code takes the base URL of the Messages API and a bearer token,
/// and sends no traffic that the work does not need.
fn model_env(harness_id: &str, endpoint: &str, token: &str) -> Vec<String> {
    match harness_id {
        "claude" => vec![
            format!("ANTHROPIC_BASE_URL={endpoint}/anthropic"),
            format!("ANTHROPIC_AUTH_TOKEN={token}"),
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1".to_string(),
        ],
        _ => Vec::new(),
    }
}

/// One path as a single shell word.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// Keeps the stderr of the harness until its stdout ends, then reports
/// the exit with the last 4 KB of stderr. When the container no longer
/// runs, the Computer stopped: the exit is dropped, so the session reads
/// its place as lost. A session that closed hears nothing.
async fn report_exit(
    manager: Arc<ComputerManager>,
    agent_id: AgentId,
    mut stderr: mpsc::Receiver<Vec<u8>>,
    mut end: oneshot::Receiver<()>,
    exit: oneshot::Sender<SessionExit>,
) {
    let mut tail = Vec::new();
    let mut stderr_open = true;
    loop {
        tokio::select! {
            chunk = stderr.recv(), if stderr_open => match chunk {
                Some(chunk) => keep_tail(&mut tail, &chunk),
                None => stderr_open = false,
            },
            // The stream ended, or the session dropped it.
            _ = &mut end => break,
        }
    }
    if stderr_open {
        let _ = tokio::time::timeout(STDERR_GRACE, async {
            while let Some(chunk) = stderr.recv().await {
                keep_tail(&mut tail, &chunk);
            }
        })
        .await;
    }
    match manager.container_runs(&agent_id).await {
        Ok(true) => {
            let stderr_tail = String::from_utf8_lossy(&tail).into_owned();
            let _ = exit.send(SessionExit::new(None, stderr_tail));
        }
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(%agent_id, %error, "the Computer of an ended harness was not read, so its session is interrupted");
        }
    }
}

/// Appends `chunk` and keeps at most the last `STDERR_TAIL_LIMIT` bytes.
fn keep_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(chunk);
    let over = tail.len().saturating_sub(STDERR_TAIL_LIMIT);
    tail.drain(..over);
}

/// The stdio of a harness in a Computer as one byte stream. It says when
/// stdout ends, and it holds the Computer awake until it drops.
struct HarnessStream {
    stdin: Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
    stdout: Pin<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    ended: Option<oneshot::Sender<()>>,
    _pin: ExecPin,
}

impl HarnessStream {
    fn end(&mut self) {
        if let Some(ended) = self.ended.take() {
            let _ = ended.send(());
        }
    }
}

impl futures::io::AsyncRead for HarnessStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let capacity = buf.len();
        let mut read = tokio::io::ReadBuf::new(buf);
        match this.stdout.as_mut().poll_read(cx, &mut read) {
            Poll::Ready(Ok(())) => {
                let count = read.filled().len();
                if count == 0 && capacity > 0 {
                    this.end();
                }
                Poll::Ready(Ok(count))
            }
            Poll::Ready(Err(error)) => {
                this.end();
                Poll::Ready(Err(error))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl futures::io::AsyncWrite for HarnessStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut().stdin.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().stdin.as_mut().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().stdin.as_mut().poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pagis_core::CodingSessionId;

    use super::*;

    /// Claude Code in a Computer runs as uid `agent` in its directory. It
    /// reaches the Harness Model Endpoint with the token of its session,
    /// and no provider key enters its environment.
    #[test]
    fn the_claude_code_launch_holds_the_endpoint_and_a_token_and_no_provider_key() {
        let launch = harness::entry("claude")
            .and_then(|entry| entry.computer)
            .expect("Claude Code runs in a Computer");
        let request = OpenRequest {
            session_id: CodingSessionId::generate(),
            command: launch.program.to_string(),
            args: Vec::new(),
            cwd: "/data/agent/app".to_string(),
            env: BTreeMap::new(),
            worktree: None,
        };
        let shell_env = vec![
            "HOME=/data/agent".to_string(),
            "USER=agent".to_string(),
            "TZ=Australia/Sydney".to_string(),
        ];

        let exec = harness_request(
            "claude",
            &request,
            shell_env,
            &pagis_computer::model_endpoint(4404),
            "session-token",
        );

        assert_eq!(exec.argv, ["claude-agent-acp"]);
        assert_eq!(exec.user, "agent");
        assert_eq!(exec.cwd, "/data/agent/app");
        for entry in [
            "HOME=/data/agent",
            "USER=agent",
            "TZ=Australia/Sydney",
            "ANTHROPIC_BASE_URL=http://host.docker.internal:4404/anthropic",
            "ANTHROPIC_AUTH_TOKEN=session-token",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1",
        ] {
            assert!(
                exec.env.iter().any(|set| set == entry),
                "{entry}: {:?}",
                exec.env
            );
        }
        for provider in pagis_core::PROVIDERS {
            let name = format!("{}=", provider.env_var());
            assert!(
                !exec.env.iter().any(|set| set.starts_with(&name)),
                "{name}: {:?}",
                exec.env
            );
        }
    }

    #[test]
    fn the_tail_of_stderr_keeps_its_last_4_kb() {
        let mut tail = Vec::new();
        keep_tail(&mut tail, &[b'a'; 3_000]);
        keep_tail(&mut tail, &[b'b'; 3_000]);

        assert_eq!(tail.len(), STDERR_TAIL_LIMIT);
        assert!(tail.ends_with(&[b'b'; 3_000]));
    }
}
