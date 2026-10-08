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
//! request and writes its audit fact. A permission that asks the Person
//! gets an approval card in the session's Thread. A permission that asks
//! the supervising Agent waits in [`AgentAsks`] for its verdict.
//!
//! Each session raises its news to its Session Rule through
//! [`SessionEvents`], and [`SessionRules`] makes and ends the rule.
//!
//! [`CodingSessionStarts`] holds the checks of a start, which the broker
//! asks before the card. [`CodingToolRuntime`] executes the core tool
//! `coding_session_start` after the approval, and the core tools that
//! prompt, read, cancel, close, list and resume the Agent's own sessions,
//! and that decide or escalate their Harness Permissions.
//!
//! [`SignIns`] starts a Harness Sign-In on a Host for the Person, and
//! [`SignInReports`] tells which harness needs one on which Host.

mod agent;
mod ask;
mod decisions;
mod error;
mod event;
mod events;
pub mod fake;
mod person;
mod place;
mod policy;
mod report;
mod session;
mod sessions;
mod sign_in;
mod starts;
mod tools;

pub use agent::AgentAsks;
pub use ask::{
    AskHandler, PermissionAnswer, PermissionAsk, PermissionOptionKind, QuestionAnswer, QuestionAsk,
};
pub use decisions::{DecidedBy, Pending, RefuseDecisions, SessionDecisions, Waited, WaitsFor};
pub use error::CodingError;
pub use event::{
    Cost, Location, PlanEntry, PlanPriority, PlanStatus, SessionEvent, StopReason, ToolKind,
    ToolStatus,
};
pub use events::{
    DecisionKind, InterruptReason, SESSION_RULE_INSTRUCTION, SessionEventMatcher, SessionEvents,
    SessionNews, SessionRuleError, SessionRules, session_event, session_rule_name,
};
pub use place::{
    OpenFailure, OpenFailureCode, OpenRequest, OpenedStream, PlaceStream, SessionExit,
    SessionPlace, WorktreeRequest,
};
pub use policy::{
    NO_DECISION_NOTE, PERMISSION_DECIDED_EVENT, PolicyDecisions, PolicyDecisionsDeps,
};
pub use session::{AcpSession, HarnessInfo, Opening, SignInMethod};
pub use sessions::{
    CloseReason, CodingSessions, CodingSessionsDeps, NewCodingSession, PromptOutcome,
    ResumeFailure, SessionError, StartFailure,
};
pub use sign_in::{SIGN_IN_CHANGED_EVENT, SignInFailure, SignInReports, SignIns};
pub use starts::{CodingSessionStarts, MAX_OPEN_SESSIONS, worktree_branch};
pub use tools::CodingToolRuntime;
