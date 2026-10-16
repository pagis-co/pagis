//! A live Call changes its Agent and Run without a new carrier leg.
use crate::{CallBrief, InboundRuns, RunTools};
use pagis_core::{
    AgentId, AgentStatus, AgentStore, CallId, CallState, CallStore, EventBus, MessageStore,
    PhoneNumberStore, RunId, RunStore, UserStore, WorkspaceId, WorkspaceStore,
};
use std::sync::Arc;

/// The stores and Run services that move a live Call to another Agent.
pub struct CallHandoffs {
    pub agents: Arc<dyn AgentStore>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub users: Arc<dyn UserStore>,
    pub numbers: Arc<dyn PhoneNumberStore>,
    pub calls: Arc<dyn CallStore>,
    pub runs: Arc<dyn InboundRuns>,
    pub messages: Arc<dyn MessageStore>,
    pub run_records: Arc<dyn RunStore>,
    pub bus: Arc<dyn EventBus>,
    pub run_tools: Arc<dyn RunTools>,
}
impl CallHandoffs {
    /// Open the target Agent's Run, then move the live Call and close its previous Run.
    pub async fn prepare(
        &self,
        workspace_id: &WorkspaceId,
        call_id: &CallId,
        run_id: &RunId,
        target: &AgentId,
    ) -> Result<(RunId, CallBrief), String> {
        let mut call = self
            .calls
            .get(workspace_id, call_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Call not found")?;
        if call.state != CallState::Live || &call.run_id != run_id {
            return Err("this Run does not hold a live Call".into());
        }
        let agent = self
            .agents
            .get(workspace_id, target)
            .await
            .map_err(|e| e.to_string())?
            .filter(|agent| agent.status == AgentStatus::Active)
            .ok_or("the target is not an active Agent of this Workspace")?;
        if agent.id == call.agent_id {
            return Err("the Agent already holds this Call".into());
        }
        let number = self
            .numbers
            .get(workspace_id, &call.phone_number_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Workspace Number not found")?;
        let person = pagis_core::honesty::person_name(
            self.workspaces.as_ref(),
            self.users.as_ref(),
            workspace_id,
        )
        .await
        .map_err(|e| e.to_string())?;
        let next = self
            .runs
            .open(workspace_id, &agent.id, call_id, &call.remote_e164)
            .await?;
        let tools = match self.run_tools.free_tools(workspace_id, &next.id).await {
            Ok(tools) => tools,
            Err(error) => {
                self.runs
                    .close(workspace_id, &next.id, Some(error.clone()))
                    .await;
                return Err(error);
            }
        };
        let mut brief = CallBrief::inbound(
            &agent,
            &person,
            &number,
            &call.remote_e164,
            call.tier,
            tools,
        );
        brief.workspace_kind = self
            .workspaces
            .get(workspace_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the Workspace is absent".to_string())?
            .kind;
        call.agent_name = agent.name.clone();
        call.purpose = brief.purpose.clone();
        call.tools = brief.tools.iter().map(|tool| tool.name.clone()).collect();
        call.agent_id = agent.id.clone();
        call.run_id = next.id.clone();
        match self.calls.update(&call).await {
            Ok(true) => {}
            result => {
                let error = format!("the Call holder could not change: {result:?}");
                self.runs
                    .close(workspace_id, &next.id, Some(error.clone()))
                    .await;
                return Err(error);
            }
        }
        crate::phone_tool::mint_strip(
            self.messages.as_ref(),
            self.run_records.as_ref(),
            self.bus.as_ref(),
            workspace_id,
            &agent.id,
            &next.id,
            call_id,
        )
        .await;
        self.runs.close(workspace_id, run_id, None).await;
        Ok((next.id, brief))
    }
}
