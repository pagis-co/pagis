//! The MCP host of the installed Plugins (ADR-0017).
//!
//! One supervised `rmcp` client per declared server of a Plugin runs in
//! the daemon and serves every Agent of one tenant that holds the
//! Plugin Grant. The client is in the daemon; the server process is
//! not: it runs inside that tenant's Plugin Computer, behind
//! [`process::ServerProcesses`].
//! The broker is the only caller: nothing here reads a Grant or writes
//! an audit fact of its own.
//!
//! Four parts stand behind that sentence. [`substitute`] turns the
//! package's `env`, `headers` and `url` into what one process or one
//! request carries, and it is where a bound secret enters. [`freeze`]
//! takes the tool list of an installed state once and never again, so
//! the tools a Run sees do not move under it. [`supervisor`] owns the
//! process: when it starts, how long a call may take, how much output
//! it may write, when it stops, and what happens after it fails.
//! [`log`] keeps the server's stderr in a log of a fixed size.

pub mod freeze;
pub mod host;
pub mod hosts;
pub mod log;
pub mod process;
pub mod substitute;
pub mod supervisor;

pub use freeze::{FreezeError, capability_manifest, freeze_tools, qualified_name};
pub use host::{
    ConnectionTokens, HostError, Manifests, NoConnectionTokens, PluginHost, PluginHostDeps,
};
pub use hosts::{PluginHosts, PluginHostsDeps, spawn_idle_reaper};
pub use log::{LOG_FULL_MARKER, MAX_LOG_BYTES, PluginLog, PluginLogs, tail_of};
pub use process::{
    CONTAINER_PLUGIN_DATA, CONTAINER_PLUGIN_ROOT, ServerIo, ServerProcesses, container_paths,
};
pub use substitute::{
    Bound, CONTAINER_ENV, Endpoint, Spawn, SubstitutionError, endpoint_of, spawn_of,
};
pub use supervisor::{
    BACKOFF_CEILING, BACKOFF_FLOOR, DEFAULT_CALL_TIMEOUT, IDLE_SHUTDOWN, Launch, MAX_CALL_TIMEOUT,
    MAX_FRAME_BYTES, MAX_IN_FLIGHT, MAX_OUTPUT_OVERRUNS, MAX_START_FAILURES, STARTUP_TIMEOUT,
    ServerError, ServerSupervisor, ToolListChanged, backoff,
};
