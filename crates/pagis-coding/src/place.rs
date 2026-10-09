//! Where the harness of a Coding Session runs.
//!
//! [`SessionPlace`] opens one byte stream to one harness process, on a
//! [`Place`]: a Host, or the Agent's own Computer. On a Host, the
//! session socket of the Client App carries the stream, and
//! [`open_on_host`] opens it. In a Computer, the daemon runs the harness
//! with `docker exec`. The request is the open request of the session
//! socket for every place.

use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncWrite};
use pagis_broker::{HostSessions, SessionOpenError};
pub use pagis_broker::{OpenRequest, SessionExit, WorktreeRequest};
use pagis_core::{AgentId, HostId, WorkspaceId};
use tokio::sync::oneshot;

/// Where the harness of one Coding Session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// One of the Person's machines.
    Host(HostId),
    /// The own Computer of this Agent.
    Computer(AgentId),
}

impl Place {
    /// The Host of the place, or `None` for a Computer.
    pub fn host_id(&self) -> Option<&HostId> {
        match self {
            Place::Host(host_id) => Some(host_id),
            Place::Computer(_) => None,
        }
    }
}

/// Starts the harness process of a Coding Session and gives its stdio as
/// one byte stream.
#[async_trait]
pub trait SessionPlace: Send + Sync {
    /// Starts the process that `request` names, on `place` in
    /// `workspace_id`.
    async fn open(
        &self,
        workspace_id: &WorkspaceId,
        place: &Place,
        request: OpenRequest,
    ) -> Result<OpenedStream, OpenFailure>;
}

/// The byte stream of a harness process: its stdin (written) and its
/// stdout (read).
pub trait PlaceStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> PlaceStream for T {}

/// One harness process that started.
pub struct OpenedStream {
    pub stream: Box<dyn PlaceStream>,
    /// The directory that the process runs in: the worktree directory
    /// when the session has a worktree.
    pub cwd: String,
    /// How the process ended. An error when the place was lost first.
    pub exit: oneshot::Receiver<SessionExit>,
    /// The place names the model in the configuration of the harness, and
    /// the harness lists models that the place does not serve. The
    /// session then keeps no Harness Model (`ModelRoute::fixes_model`).
    pub fixed_model: bool,
}

/// Why no harness process started.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the session did not open ({}): {message}", code.as_str())]
pub struct OpenFailure {
    pub code: OpenFailureCode,
    pub message: String,
}

/// The code of an [`OpenFailure`]. A failed session keeps it as its end
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailureCode {
    /// The command is not on the Person's `PATH`.
    NotFound,
    /// The directory does not exist or is not a directory.
    BadDirectory,
    /// The git worktree was not made.
    WorktreeFailed,
    /// The process did not start, or the Computer did not wake for it.
    SpawnFailed,
    /// The Host has no session socket open, or its socket did not carry
    /// the open request and its answer.
    HostNotConnected,
}

impl OpenFailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            OpenFailureCode::NotFound => "not_found",
            OpenFailureCode::BadDirectory => "bad_directory",
            OpenFailureCode::WorktreeFailed => "worktree_failed",
            OpenFailureCode::SpawnFailed => "spawn_failed",
            OpenFailureCode::HostNotConnected => "host_not_connected",
        }
    }
}

/// Opens the stream of a session on a Host, over the session socket of
/// its Client App.
pub async fn open_on_host(
    sessions: &HostSessions,
    workspace_id: &WorkspaceId,
    host_id: &HostId,
    request: OpenRequest,
) -> Result<OpenedStream, OpenFailure> {
    let opened = sessions
        .open(workspace_id, host_id, &request)
        .await
        .map_err(|error| {
            let code = match &error {
                SessionOpenError::Refused { code, .. } => match code {
                    pagis_broker::OpenFailure::NotFound => OpenFailureCode::NotFound,
                    pagis_broker::OpenFailure::BadDirectory => OpenFailureCode::BadDirectory,
                    pagis_broker::OpenFailure::WorktreeFailed => OpenFailureCode::WorktreeFailed,
                    pagis_broker::OpenFailure::SpawnFailed => OpenFailureCode::SpawnFailed,
                },
                SessionOpenError::NotConnected | SessionOpenError::Failed(_) => {
                    OpenFailureCode::HostNotConnected
                }
            };
            let message = match error {
                SessionOpenError::Refused { message, .. } => message,
                other => other.to_string(),
            };
            OpenFailure { code, message }
        })?;
    Ok(OpenedStream {
        stream: Box::new(opened.stream),
        cwd: opened.cwd,
        exit: opened.exit,
        fixed_model: false,
    })
}
