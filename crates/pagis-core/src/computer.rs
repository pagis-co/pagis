//! The Computer operations that the domain calls. Container lifecycle
//! operations belong to the host implementation.

use crate::computer_action::InputOp;
use crate::{AgentId, FillField, WorkspaceId};
use async_trait::async_trait;
use serde::Serialize;
use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

/// An opaque screen lease. The implementation holds the action batch
/// until this guard drops. The guard can move with a Run between threads.
pub type ComputerLease = Box<dyn Any + Send + Sync>;

/// The Computers of each Workspace.
pub trait Computers: Send + Sync {
    fn get(&self, workspace_id: &WorkspaceId) -> Arc<dyn Computer>;
}

/// The operations on the Computers of one Workspace.
#[async_trait]
pub trait Computer: Send + Sync {
    /// Serialize one Agent action batch and refuse while another holder controls input.
    async fn lease(&self, agent_id: &AgentId) -> Result<ComputerLease, ComputerError>;
    /// Wake the Computer before an action.
    async fn ensure_awake(self: Arc<Self>, agent_id: &AgentId) -> Result<(), ComputerError>;
    /// Run a command with its deadline, working directory and output cap.
    async fn shell(
        self: Arc<Self>,
        agent_id: &AgentId,
        command: ShellCommand,
    ) -> Result<ExecOutcome, ComputerError>;
    /// Extract a tar into an existing directory.
    async fn upload_archive(
        self: Arc<Self>,
        agent_id: &AgentId,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), ComputerError>;
    /// Read a tar into a spool file that is open at its first byte.
    async fn download_archive(
        self: Arc<Self>,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<std::fs::File, ComputerError>;
    /// Capture the current screen. Capture is refused during a daemon hold.
    async fn live_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError>;
    /// Capture the screen after the effect of the previous input settles.
    async fn settled_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError>;
    /// Send one input batch in order.
    async fn input(&self, agent_id: &AgentId, ops: &[InputOp]) -> Result<(), ComputerError>;
    /// Hold input for the daemon until the browser channel releases or drops.
    async fn daemon_hold(
        self: Arc<Self>,
        agent_id: &AgentId,
    ) -> Result<Box<dyn DaemonHold>, ComputerError>;
    /// The Home Exit or server in use, or none in Direct mode.
    async fn exit_in_use(&self, agent_id: &AgentId) -> Option<ExitInUse>;
}

/// The daemon's browser channel while it holds the input switch.
/// An implementation releases the switch on drop as well as on release.
#[async_trait]
pub trait DaemonHold: Send + Sync {
    async fn open(&self, url: &str) -> Result<String, ComputerError>;
    async fn page(&self) -> Result<String, ComputerError>;
    async fn fill(&self, origin: &str, fields: &[FillField]) -> Result<(), ComputerError>;
    async fn release(self: Box<Self>);
}

/// One shell command for an agent's own computer. The manager
/// owns the wrapping, the environment and the caps; the caller brings
/// the command, the deadline and the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    pub command: String,
    pub timeout: Duration,
    /// The working directory. Absent takes `/data/agent`.
    pub cwd: Option<String>,
    /// Bytes for the command's stdin. The daemon shuts the write half
    /// down after them, so the command sees the end of its input. A
    /// software tool call takes its arguments this way.
    pub stdin: Option<Vec<u8>>,
    /// How much of each output stream to keep. Absent takes the
    /// shell's own cap; a software tool call asks for more, because
    /// its stdout is the result.
    pub output_cap: Option<OutputCap>,
}

/// How much of one command's output the daemon keeps.
/// The first `head` bytes and the last `tail` bytes of a stream
/// survive; what falls between is counted and dropped. The daemon
/// always drains the whole stream, because a command whose output
/// nobody reads blocks on its next write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCap {
    pub head: usize,
    pub tail: usize,
}

/// What one command left behind. The daemon reads `exit_code`
/// only after the exec instance reports that it stopped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecOutcome {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    /// True when a cap dropped bytes from either stream.
    pub truncated: bool,
}

/// Who drives the computer's input: the daemon-held
/// switch. Exactly one source at a time; the default is the agent.
/// `Daemon` is the vault's held switch (ADR-0013): the daemon fills a
/// secret the model never sees through the browser channel, so
/// model-visible capture is suppressed while it holds. The daemon sends
/// no input batch of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputHolder {
    Agent,
    User,
    Daemon,
}

impl InputHolder {
    pub fn as_str(&self) -> &'static str {
        match self {
            InputHolder::Agent => "agent",
            InputHolder::User => "user",
            InputHolder::Daemon => "daemon",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    /// The local image under the pinned tag carries the wrong version
    /// label; the daemon refuses to boot it.
    #[error(
        "the local computer image reports version {found:?}; this daemon needs \
         {expected} — remove the stale image and wake again"
    )]
    VersionMismatch {
        found: Option<String>,
        expected: String,
    },
    /// No live frame and no stored screenshot.
    #[error("no screen preview exists for this agent yet")]
    NoPreview,
    /// A live-view offer arrived while the computer is not awake.
    #[error("the computer is not awake")]
    Asleep,
    /// Someone other than the agent holds the input switch:
    /// the agent's screen-lease requests are denied until the
    /// switch comes back.
    #[error("{} has control of this computer", holder.as_str())]
    SwitchHeld { holder: InputHolder },
    /// A sleep asked for while a command runs in the container.
    /// A stop would kill the command, so the user waits for it.
    #[error("this computer is running a command; it stops when the command ends")]
    Busy,
    /// The awake cap is reached. The Run reads this sentence, so
    /// it says what to do and not only what refused.
    #[error(
        "{scope} already has {cap} computers awake, which is the cap; \
         a computer has to sleep before this one can wake"
    )]
    AwakeCapReached { scope: &'static str, cap: u32 },
    /// The Agent is not of this manager's Workspace. A Computer is
    /// the boundary between tenants, so a caller that resolved the wrong
    /// tenant's manager gets nothing: no container starts, and nothing of
    /// this tenant's is named, labelled or counted for a stranger.
    #[error("that sprite is not in this workspace")]
    ForeignAgent,
    /// The Media Relay could not open a path for this viewer,
    /// which on the `daemon` relay means every port of the range is
    /// taken. The machine is full, not the request wrong, so the sentence
    /// says what to do.
    #[error("this screen has no media path now: {reason}")]
    NoMediaPath { reason: String },
    #[error("computer runtime error: {0}")]
    Runtime(String),
}

/// Where the connections of a Computer in Home mode leave now. The
/// Computer's view and the `computer` tool result show its
/// [`label`](ExitInUse::label), because a change of the address is a
/// signal that sites read, and the Person sees each change. A Computer in
/// Direct mode has no exit in use: it leaves from where it runs, as the
/// Computer of a Person with no Home Exit does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitInUse {
    /// The Person's Home Exit, by the name of its Host.
    HomeExit(String),
    /// The server, while the Home Exit is absent.
    Server,
}

impl ExitInUse {
    /// The one line that names the exit, such as `exit: MacBook Pro` or
    /// `exit: server`.
    pub fn label(&self) -> String {
        match self {
            ExitInUse::HomeExit(host_name) => format!("exit: {host_name}"),
            ExitInUse::Server => "exit: server".to_string(),
        }
    }
}
