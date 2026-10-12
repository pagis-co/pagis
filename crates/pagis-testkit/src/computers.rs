//! In-memory Computers with a call record and controlled answers.

use async_trait::async_trait;
use pagis_core::computer_action::InputOp;
use pagis_core::{
    AgentId, Computer, ComputerError, ComputerLease, Computers, DaemonHold, ExecOutcome, ExitInUse,
    FillField, InputHolder, ShellCommand, WorkspaceId,
};
use std::collections::{HashMap, VecDeque};
use std::io::{Seek, Write};
use std::sync::{Arc, Mutex};

/// One domain operation. FillField hides its text in debug output.
#[derive(Debug, Clone, PartialEq)]
pub enum ComputerCall {
    Lease,
    EnsureAwake,
    Shell(ShellCommand),
    Upload {
        path: String,
        tar: Vec<u8>,
    },
    Download(String),
    LiveFrame,
    SettledFrame,
    Input(Vec<InputOp>),
    DaemonHold,
    Open(String),
    Page,
    Fill {
        origin: String,
        fields: Vec<FillField>,
    },
    Release,
    ExitInUse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedComputerCall {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub call: ComputerCall,
}

type DownloadReply = Arc<
    dyn Fn() -> futures::future::BoxFuture<'static, Result<Vec<u8>, ComputerError>> + Send + Sync,
>;

type Owner = (WorkspaceId, AgentId);
#[derive(Default)]
struct State {
    calls: Vec<RecordedComputerCall>,
    get_calls: Vec<WorkspaceId>,
    outcomes: VecDeque<ExecOutcome>,
    exec_delay: std::time::Duration,
    answers: VecDeque<Result<(), ComputerError>>,
    downloads: HashMap<String, DownloadReply>,
    page: Option<String>,
    frame: Vec<u8>,
    exit: Option<ExitInUse>,
    leases: HashMap<Owner, Arc<tokio::sync::Mutex<()>>>,
    held: HashMap<Owner, InputHolder>,
}

/// A shared call record. Each handle is fixed to one Workspace.
#[derive(Default)]
pub struct FakeComputers {
    state: Arc<Mutex<State>>,
    computers: Mutex<HashMap<WorkspaceId, Arc<FakeComputer>>>,
}

impl FakeComputers {
    pub fn get_calls(&self) -> Vec<WorkspaceId> {
        self.state.lock().expect("computers").get_calls.clone()
    }
    pub fn calls(&self) -> Vec<RecordedComputerCall> {
        self.state.lock().expect("computers").calls.clone()
    }
    pub fn set_exec_delay(&self, delay: std::time::Duration) {
        self.state.lock().expect("computers").exec_delay = delay;
    }
    pub fn push_exec_outcome(&self, outcome: ExecOutcome) {
        self.state
            .lock()
            .expect("computers")
            .outcomes
            .push_back(outcome);
    }
    /// The next operation that returns a Result receives this error.
    pub fn push_error(&self, error: ComputerError) {
        self.push_answer(Err(error));
    }
    pub fn push_answer(&self, answer: Result<(), ComputerError>) {
        self.state
            .lock()
            .expect("computers")
            .answers
            .push_back(answer);
    }
    /// An open returns this page after redirects. A page read returns it too.
    pub fn set_page(&self, page: &str) {
        self.state.lock().expect("computers").page = Some(page.into());
    }
    pub fn set_frame(&self, png: Vec<u8>) {
        self.state.lock().expect("computers").frame = png;
    }
    pub fn set_exit(&self, exit: Option<ExitInUse>) {
        self.state.lock().expect("computers").exit = exit;
    }
    pub fn set_download(&self, path: &str, tar: Vec<u8>) {
        self.set_download_reply(path, move || {
            let tar = tar.clone();
            Box::pin(async move { Ok(tar) })
        });
    }
    /// A controlled asynchronous download, for tests of concurrent work.
    pub fn set_download_reply(
        &self,
        path: &str,
        reply: impl Fn() -> futures::future::BoxFuture<'static, Result<Vec<u8>, ComputerError>>
        + Send
        + Sync
        + 'static,
    ) {
        self.state
            .lock()
            .expect("computers")
            .downloads
            .insert(path.into(), Arc::new(reply));
    }
    pub fn execs(&self) -> Vec<ShellCommand> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call.call {
                ComputerCall::Shell(command) => Some(command),
                _ => None,
            })
            .collect()
    }
    pub fn uploads(&self) -> Vec<(String, Vec<u8>)> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call.call {
                ComputerCall::Upload { path, tar } => Some((path, tar)),
                _ => None,
            })
            .collect()
    }
    pub fn downloaded(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call.call {
                ComputerCall::Download(path) => Some(path),
                _ => None,
            })
            .collect()
    }
}

impl Computers for FakeComputers {
    fn get(&self, workspace_id: &WorkspaceId) -> Arc<dyn Computer> {
        self.state
            .lock()
            .expect("computers")
            .get_calls
            .push(workspace_id.clone());
        self.computers
            .lock()
            .expect("computers")
            .entry(workspace_id.clone())
            .or_insert_with(|| {
                Arc::new(FakeComputer {
                    workspace_id: workspace_id.clone(),
                    state: self.state.clone(),
                })
            })
            .clone()
    }
}

struct FakeComputer {
    workspace_id: WorkspaceId,
    state: Arc<Mutex<State>>,
}
impl FakeComputer {
    fn record(&self, agent_id: &AgentId, call: ComputerCall) {
        self.state
            .lock()
            .expect("computers")
            .calls
            .push(RecordedComputerCall {
                workspace_id: self.workspace_id.clone(),
                agent_id: agent_id.clone(),
                call,
            });
    }
    fn answer(&self) -> Result<(), ComputerError> {
        self.state
            .lock()
            .expect("computers")
            .answers
            .pop_front()
            .unwrap_or(Ok(()))
    }
    fn lease_mutex(&self, agent_id: &AgentId) -> Arc<tokio::sync::Mutex<()>> {
        self.state
            .lock()
            .expect("computers")
            .leases
            .entry((self.workspace_id.clone(), agent_id.clone()))
            .or_default()
            .clone()
    }
    fn check_holder(&self, agent_id: &AgentId) -> Result<(), ComputerError> {
        match self
            .state
            .lock()
            .expect("computers")
            .held
            .get(&(self.workspace_id.clone(), agent_id.clone()))
        {
            Some(holder) => Err(ComputerError::SwitchHeld { holder: *holder }),
            None => Ok(()),
        }
    }
    fn frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        self.answer()?;
        self.check_holder(agent_id)?;
        let frame = self.state.lock().expect("computers").frame.clone();
        if frame.is_empty() {
            Err(ComputerError::NoPreview)
        } else {
            Ok(frame)
        }
    }
}

#[async_trait]
impl Computer for FakeComputer {
    async fn lease(&self, agent_id: &AgentId) -> Result<ComputerLease, ComputerError> {
        self.record(agent_id, ComputerCall::Lease);
        self.answer()?;
        let lease = self.lease_mutex(agent_id).lock_owned().await;
        self.check_holder(agent_id)?;
        Ok(Box::new(lease))
    }
    async fn ensure_awake(self: Arc<Self>, agent_id: &AgentId) -> Result<(), ComputerError> {
        self.record(agent_id, ComputerCall::EnsureAwake);
        self.answer()
    }
    async fn shell(
        self: Arc<Self>,
        agent_id: &AgentId,
        command: ShellCommand,
    ) -> Result<ExecOutcome, ComputerError> {
        self.record(agent_id, ComputerCall::Shell(command));
        self.answer()?;
        let (outcome, delay) = {
            let mut state = self.state.lock().expect("computers");
            (
                state.outcomes.pop_front().unwrap_or_default(),
                state.exec_delay,
            )
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(outcome)
    }
    async fn upload_archive(
        self: Arc<Self>,
        agent_id: &AgentId,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), ComputerError> {
        self.record(
            agent_id,
            ComputerCall::Upload {
                path: path.into(),
                tar,
            },
        );
        self.answer()
    }
    async fn download_archive(
        self: Arc<Self>,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<std::fs::File, ComputerError> {
        self.record(agent_id, ComputerCall::Download(path.into()));
        self.answer()?;
        let reply = self
            .state
            .lock()
            .expect("computers")
            .downloads
            .get(path)
            .cloned()
            .ok_or_else(|| ComputerError::Runtime(format!("no tar for {path}")))?;
        let bytes = reply().await?;
        let write = || -> std::io::Result<std::fs::File> {
            let mut file = tempfile::tempfile()?;
            file.write_all(&bytes)?;
            file.rewind()?;
            Ok(file)
        };
        write().map_err(|error| ComputerError::Runtime(error.to_string()))
    }
    async fn live_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        self.record(agent_id, ComputerCall::LiveFrame);
        self.frame(agent_id)
    }
    async fn settled_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        self.record(agent_id, ComputerCall::SettledFrame);
        self.frame(agent_id)
    }
    async fn input(&self, agent_id: &AgentId, ops: &[InputOp]) -> Result<(), ComputerError> {
        self.record(agent_id, ComputerCall::Input(ops.to_vec()));
        self.answer()?;
        self.check_holder(agent_id)
    }
    async fn daemon_hold(
        self: Arc<Self>,
        agent_id: &AgentId,
    ) -> Result<Box<dyn DaemonHold>, ComputerError> {
        self.record(agent_id, ComputerCall::DaemonHold);
        self.answer()?;
        let _lease = self.lease_mutex(agent_id).lock_owned().await;
        self.check_holder(agent_id)?;
        self.state.lock().expect("computers").held.insert(
            (self.workspace_id.clone(), agent_id.clone()),
            InputHolder::Daemon,
        );
        Ok(Box::new(FakeHold {
            computer: self,
            agent_id: agent_id.clone(),
        }))
    }
    async fn exit_in_use(&self, agent_id: &AgentId) -> Option<ExitInUse> {
        self.record(agent_id, ComputerCall::ExitInUse);
        self.state.lock().expect("computers").exit.clone()
    }
}

struct FakeHold {
    computer: Arc<FakeComputer>,
    agent_id: AgentId,
}
impl Drop for FakeHold {
    fn drop(&mut self) {
        self.computer.record(&self.agent_id, ComputerCall::Release);
        self.computer
            .state
            .lock()
            .expect("computers")
            .held
            .remove(&(self.computer.workspace_id.clone(), self.agent_id.clone()));
    }
}
#[async_trait]
impl DaemonHold for FakeHold {
    async fn open(&self, url: &str) -> Result<String, ComputerError> {
        self.computer
            .record(&self.agent_id, ComputerCall::Open(url.into()));
        self.computer.answer()?;
        let mut state = self.computer.state.lock().expect("computers");
        Ok(state.page.get_or_insert_with(|| url.into()).clone())
    }
    async fn page(&self) -> Result<String, ComputerError> {
        self.computer.record(&self.agent_id, ComputerCall::Page);
        self.computer.answer()?;
        self.computer
            .state
            .lock()
            .expect("computers")
            .page
            .clone()
            .ok_or_else(|| ComputerError::Runtime("no open page".into()))
    }
    async fn fill(&self, origin: &str, fields: &[FillField]) -> Result<(), ComputerError> {
        self.computer.record(
            &self.agent_id,
            ComputerCall::Fill {
                origin: origin.into(),
                fields: fields.to_vec(),
            },
        );
        self.computer.answer()
    }
    async fn release(self: Box<Self>) {
        drop(self);
    }
}

#[cfg(test)]
mod tests;
