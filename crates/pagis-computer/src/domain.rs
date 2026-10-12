//! The domain traits use the same manager operations as the daemon.

use crate::manager::BrowserHold;
use crate::{ComputerManager, ComputerManagers};
use async_trait::async_trait;
use pagis_core::computer_action::InputOp;
use pagis_core::{
    AgentId, Computer, ComputerError, ComputerLease, Computers, DaemonHold, ExecOutcome, ExitInUse,
    FillField, ShellCommand, WorkspaceId,
};
use std::sync::Arc;

impl Computers for ComputerManagers {
    fn get(&self, workspace_id: &WorkspaceId) -> Arc<dyn Computer> {
        ComputerManagers::get(self, workspace_id)
    }
}

#[async_trait]
impl Computer for ComputerManager {
    async fn lease(&self, agent_id: &AgentId) -> Result<ComputerLease, ComputerError> {
        Ok(Box::new(ComputerManager::lease(self, agent_id).await?))
    }
    async fn ensure_awake(self: Arc<Self>, agent_id: &AgentId) -> Result<(), ComputerError> {
        ComputerManager::ensure_awake(&self, agent_id).await
    }
    async fn shell(
        self: Arc<Self>,
        agent_id: &AgentId,
        command: ShellCommand,
    ) -> Result<ExecOutcome, ComputerError> {
        ComputerManager::shell(&self, agent_id, command).await
    }
    async fn upload_archive(
        self: Arc<Self>,
        agent_id: &AgentId,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), ComputerError> {
        ComputerManager::upload_archive(&self, agent_id, path, tar).await
    }
    async fn download_archive(
        self: Arc<Self>,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<std::fs::File, ComputerError> {
        ComputerManager::download_archive(&self, agent_id, path).await
    }
    async fn live_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        ComputerManager::live_frame(self, agent_id).await
    }
    async fn settled_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        ComputerManager::settled_frame(self, agent_id).await
    }
    async fn input(&self, agent_id: &AgentId, ops: &[InputOp]) -> Result<(), ComputerError> {
        ComputerManager::input(self, agent_id, ops).await
    }
    async fn daemon_hold(
        self: Arc<Self>,
        agent_id: &AgentId,
    ) -> Result<Box<dyn DaemonHold>, ComputerError> {
        Ok(Box::new(
            ComputerManager::daemon_hold(&self, agent_id).await?,
        ))
    }
    async fn exit_in_use(&self, agent_id: &AgentId) -> Option<ExitInUse> {
        ComputerManager::exit_in_use(self, agent_id).await
    }
}

#[async_trait]
impl DaemonHold for BrowserHold {
    async fn open(&self, url: &str) -> Result<String, ComputerError> {
        BrowserHold::open(self, url).await
    }
    async fn page(&self) -> Result<String, ComputerError> {
        BrowserHold::page(self).await
    }
    async fn fill(&self, origin: &str, fields: &[FillField]) -> Result<(), ComputerError> {
        BrowserHold::fill(self, origin, fields).await
    }
    async fn release(self: Box<Self>) {
        BrowserHold::release(*self).await;
    }
}
