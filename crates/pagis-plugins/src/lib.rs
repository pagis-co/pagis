//! The MCP host of the installed Plugins (ADR-0017).
//!
//! One supervised session for each declared server serves every Agent
//! of a Workspace that holds the Plugin Grant. The broker is the caller.
//! [`session::Session`] lists tools, calls a tool with a deadline, and
//! closes a connection. HTTP uses [`McpHttp`]. The daemon supplies stdio
//! sessions and log tails through [`StdioServers`].
//!
//! [`substitute`] expands Plugin Bindings. [`freeze`] fixes the tool list
//! of an installed state. [`supervisor`] controls starts, deadlines,
//! failures and idle shutdown. The `native` feature holds the stdio
//! client, process streams, logs and idle reaper task. An Embedder can
//! call [`PluginHosts::reap_idle`] from its own timer.

pub mod freeze;
pub mod host;
pub mod hosts;
pub mod http;
#[cfg(feature = "native")]
pub mod log;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "native")]
pub mod process;
pub mod runtime;
pub mod session;
pub mod stdio;
pub mod substitute;
pub mod supervisor;

pub use freeze::{FreezeError, capability_manifest, freeze_tools, qualified_name};
pub use host::{
    ConnectionTokens, HostError, Manifests, NoConnectionTokens, PluginHost, PluginHostDeps,
};
#[cfg(feature = "native")]
pub use hosts::spawn_idle_reaper;
pub use hosts::{PluginHosts, PluginHostsDeps};
pub use http::{HttpBody, McpHttp, ReqwestMcpHttp};
#[cfg(feature = "native")]
pub use log::{LOG_FULL_MARKER, MAX_LOG_BYTES, PluginLog, PluginLogs, tail_of};
#[cfg(feature = "native")]
pub use native::NativeStdioServers;
#[cfg(feature = "native")]
pub use process::{ServerIo, ServerProcesses};
pub use runtime::PluginToolRuntime;
pub use session::{Session, SessionError};
pub use stdio::{
    CONTAINER_PLUGIN_DATA, CONTAINER_PLUGIN_ROOT, NoStdioServers, StdioServers, container_paths,
};
pub use substitute::{
    Bound, CONTAINER_ENV, Endpoint, Spawn, SubstitutionError, endpoint_of, spawn_of,
};
pub use supervisor::{
    BACKOFF_CEILING, BACKOFF_FLOOR, DEFAULT_CALL_TIMEOUT, IDLE_SHUTDOWN, Launch, MAX_CALL_TIMEOUT,
    MAX_FRAME_BYTES, MAX_IN_FLIGHT, MAX_OUTPUT_OVERRUNS, MAX_START_FAILURES, STARTUP_TIMEOUT,
    ServerError, ServerSupervisor, ToolListChanged, backoff,
};
