//! The agent loop: resident actors, the run state
//! machine, always-streaming turns against the LLM router, tool
//! dispatch through the capability broker with request parking, and
//! the live delta hub the WebSocket serves.

pub mod admission;
mod agent_dm;
mod brain;
mod briefing;
mod computer_action;
mod computer_use;
pub use computer_action::{DISPLAY_HEIGHT, DISPLAY_WIDTH};
mod context_budget;
mod conversations;
mod hub;
mod image_tokens;
mod memory;
mod mention;
mod model_call;
mod model_call_gate;
mod model_catalog;
mod progress;
mod recover;
mod run;
mod run_queue;
mod run_signals;
mod skills;
mod speaker;
mod spend;
mod system;
mod takeover;
mod tool_runtime;
mod ui;

pub use agent_dm::AgentDm;
pub use brain::{
    Brain, BrainError, JsonSchemaFormat, RouterBrain, ToolInvocation, TurnDelta, TurnEnd,
    TurnMessage, TurnRequest, TurnRole, TurnStream, Usage,
};
pub use hub::{DeltaFrame, StreamHub};
pub use mention::is_mentioned;
pub use model_call_gate::{GateStop, ModelCallBound, ModelCallGate};
pub use model_catalog::{MODEL_LIST_REFRESH, ModelCatalog, ModelListError};
pub use progress::{ProgressFrame, ProgressHub};
pub use recover::{RESTART_ERROR, resume_unfinished_runs};
pub use run_queue::RunQueue;
pub use run_signals::RunSignals;
pub use spend::{CapReads, CapStop, SpendCapGate, cap_stop};
pub use system::{AgentDeps, AgentLoopConfig, AgentSystem, CancelOutcome};
pub use tool_runtime::{CoreToolRuntime, ToolRuntimeDeps};
pub use ui::{MAX_BLOCKS, MAX_BLOCKS_BYTES, MAX_TABLE_ROWS};

pub use run::UNKNOWN_OUTCOME;
