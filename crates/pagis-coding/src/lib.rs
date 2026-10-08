//! The daemon's ACP client for one Coding Session (ADR-0033).
//!
//! An [`AcpSession`] drives one ACP session of one Coding Harness over one
//! byte stream. The caller gives the stream: on a Host it is a yamux
//! stream of the session socket, and in a Computer it is the stdio of
//! `docker exec`. The crate starts no process and opens no socket.
//!
//! The crate keeps no policy. It translates the ACP messages of the
//! harness into [`SessionEvent`]s, in the order that they arrive, and it
//! sends the prompts and cancels of the caller. It hands each permission
//! request and each question of the harness to the caller's
//! [`AskHandler`]. No ACP type crosses the crate boundary, except in the
//! [`fake`] module for tests.
//!
//! [`CodingSessions`] is the daemon's runtime of the Coding Sessions on
//! top of the ACP client: it starts each session on a [`SessionPlace`],
//! records its updates through the Coding Session store, and asks
//! [`SessionDecisions`] for the answer to each permission request and
//! question. [`PolicyDecisions`] applies Pagis policy to each permission
//! request and writes its audit fact.
//!
//! [`CodingSessionStarts`] holds the checks of a start, which the broker
//! asks before the card, and [`CodingToolRuntime`] executes the core
//! tool `coding_session_start` after the approval.

mod ask;
mod decisions;
mod error;
mod event;
pub mod fake;
mod place;
mod policy;
mod session;
mod sessions;
mod starts;
mod tools;

pub use ask::{
    AskHandler, PermissionAnswer, PermissionAsk, PermissionOptionKind, QuestionAnswer, QuestionAsk,
};
pub use decisions::{Pending, RefuseDecisions, SessionDecisions, WaitsFor};
pub use error::CodingError;
pub use event::{
    Cost, Location, PlanEntry, PlanStatus, SessionEvent, StopReason, ToolKind, ToolStatus,
};
pub use place::{
    OpenFailure, OpenFailureCode, OpenRequest, OpenedStream, PlaceStream, SessionExit,
    SessionPlace, WorktreeRequest,
};
pub use policy::{PERMISSION_DECIDED_EVENT, PolicyDecisions};
pub use session::{AcpSession, HarnessInfo, Opening, SignInMethod};
pub use sessions::{
    CloseReason, CodingSessions, CodingSessionsDeps, NewCodingSession, PromptOutcome, SessionError,
    StartFailure,
};
pub use starts::{CodingSessionStarts, MAX_OPEN_SESSIONS, worktree_branch};
pub use tools::CodingToolRuntime;
