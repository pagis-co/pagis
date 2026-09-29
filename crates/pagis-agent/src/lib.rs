//! The agent loop: resident actors, the run state
//! machine, always-streaming turns against the LLM router, tool
//! dispatch through the capability broker with request parking, and
//! the live delta hub the WebSocket serves.

mod agent_dm;
mod brain;
mod briefing;
mod computer_use;
mod context_budget;
mod hub;
mod image_tokens;
mod memory;
mod mention;
mod model_catalog;
mod progress;
mod recover;
mod run;
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
pub use model_catalog::{MODEL_LIST_REFRESH, ModelCatalog, ModelListError};
pub use progress::{ProgressFrame, ProgressHub};
pub use recover::{RESTART_ERROR, fail_unfinished_runs};
pub use spend::CAP_NOTE;
pub use system::{AgentDeps, AgentLoopConfig, AgentSystem, CancelOutcome};
pub use tool_runtime::{CoreToolRuntime, ToolRuntimeDeps};
pub use ui::{MAX_BLOCKS, MAX_BLOCKS_BYTES, MAX_TABLE_ROWS};
