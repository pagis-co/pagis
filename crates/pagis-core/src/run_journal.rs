//! Durable state of one Run, separate from conversation context.
use crate::{RunId, StoreError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCheckpoint {
    /// The next Turn, numbered from zero.
    pub turn: usize,
    /// The Run host owns the state format.
    pub state: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalCall {
    pub call_id: String,
    pub effect_class: String,
    pub result: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunJournalEntry {
    pub checkpoint: Option<RunCheckpoint>,
    /// The unfinished model answer and its call identities.
    pub pending_turn: Option<Value>,
    pub calls: Vec<JournalCall>,
    /// Recovery has used this Turn's one resume attempt.
    pub resumed: bool,
}

#[async_trait]
pub trait RunJournal: Send + Sync {
    /// Commit the next Turn's state and remove the previous Turn's calls.
    async fn checkpoint(
        &self,
        run_id: &RunId,
        turn: usize,
        state: &Value,
    ) -> Result<(), StoreError>;
    /// Keep call identities before any tool dispatch. This is not a completed Turn.
    async fn turn_started(&self, run_id: &RunId, state: &Value) -> Result<(), StoreError>;
    async fn tool_started(
        &self,
        run_id: &RunId,
        call_id: &str,
        effect_class: &str,
    ) -> Result<(), StoreError>;
    async fn tool_ended(
        &self,
        run_id: &RunId,
        call_id: &str,
        result: &Value,
    ) -> Result<(), StoreError>;
    async fn last(&self, run_id: &RunId) -> Result<Option<RunJournalEntry>, StoreError>;
    /// Claim the single resume attempt atomically. False means it is used or absent.
    async fn mark_resumed(&self, run_id: &RunId) -> Result<bool, StoreError>;
    async fn delete(&self, run_id: &RunId) -> Result<(), StoreError>;
}
