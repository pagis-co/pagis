use agent_client_protocol::{Error, ErrorCode, is_incoming_transport_closed};

/// A failure of an ACP session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodingError {
    /// The byte stream to the harness is closed.
    #[error("the connection to the Coding Harness is closed")]
    Closed,
    /// The harness needs a Harness Sign-In before it opens a session.
    #[error("the Coding Harness needs a sign-in")]
    AuthRequired,
    /// The harness declares neither `session/resume` nor `session/load`.
    #[error("the Coding Harness cannot restore a session")]
    CannotRestore,
    /// A turn runs. ACP v1 has no steering, so a prompt waits for the end
    /// of the turn.
    #[error("a turn of the Coding Session runs")]
    Busy,
    /// Any other ACP error, with its message.
    #[error("ACP error: {0}")]
    Protocol(String),
}

impl CodingError {
    /// Translates an ACP error. A private function and not `From`, so that
    /// no ACP type appears in the public interface.
    pub(crate) fn from_acp(error: Error) -> Self {
        if error.code == ErrorCode::AuthRequired {
            Self::AuthRequired
        } else if is_incoming_transport_closed(&error) {
            Self::Closed
        } else {
            Self::Protocol(error.message)
        }
    }
}
