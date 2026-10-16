//! Call hand-off keeps one Call and changes its Agent and Run.
use async_trait::async_trait;
use pagis_core::*;
use pagis_telephony::handoff::CallHandoffs;
use pagis_telephony::{InboundRuns, RunTools};
use std::sync::Arc;

struct Runs(Arc<dyn RunStore>);
#[async_trait]
impl InboundRuns for Runs {
    async fn open(
        &self,
        ws: &WorkspaceId,
        agent: &AgentId,
        call: &CallId,
        _: &str,
    ) -> Result<Run, String> {
        let run: Run = serde_json::from_value(serde_json::json!({"id":RunId::generate(), "title":"Call", "workspace_id":ws,"agent_id":agent,"trigger_kind":"event", "trigger_ref":call,"hop_count":0,"state":"running","created_at":now_ms(),"started_at":now_ms()})).unwrap();
        self.0.create(&run).await.map_err(|e| e.to_string())?;
        Ok(run)
    }
    async fn close(&self, ws: &WorkspaceId, run: &RunId, _: Option<String>) {
        let mut record = self.0.get(ws, run).await.unwrap().unwrap();
        record.state = RunState::Completed;
        record.ended_at = Some(now_ms());
        self.0.update(&record).await.unwrap();
    }
}
struct Tools;
#[async_trait]
impl RunTools for Tools {
    async fn free_tools(
        &self,
        _: &WorkspaceId,
        _: &RunId,
    ) -> Result<Vec<pagis_broker::ToolDef>, String> {
        Ok(vec![])
    }
}
fn agent(ws: &WorkspaceId, name: &str) -> Agent {
    Agent {
        id: AgentId::generate(),
        workspace_id: ws.clone(),
        name: name.into(),
        job: "clinic".into(),
        description: String::new(),
        personality: "calm".into(),
        model_alias: "default".into(),
        avatar: Default::default(),
        voice: Some("nova".into()),
        standing_brief: Some("Arrange clinic appointments.".into()),
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    }
}
pub(super) async fn setup() -> (Stores, CallHandoffs, Call, Agent) {
    let stores = pagis_core::fake::stores();
    let person = User::new(OrgId::generate(), UserRole::Member, now_ms());
    stores.users.create(&person).await.unwrap();
    let ws = Workspace {
        id: WorkspaceId::generate(),
        user_id: person.id,
        name: "Workspace".into(),
        timezone: "UTC".into(),
        monthly_spend_cap_usd: None,
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    stores.workspaces.create(&ws).await.unwrap();
    let chief = agent(&ws.id, "Robin");
    let target = agent(&ws.id, "Wren");
    stores.agents.create(&chief).await.unwrap();
    stores.agents.create(&target).await.unwrap();
    stores
        .workspaces
        .set_chief_of_staff(&ws.id, Some(&chief.id))
        .await
        .unwrap();
    let number = PhoneNumber::new(
        PhoneNumberId::generate(),
        ws.id.clone(),
        ConnectionId::generate(),
        "+14155550123".into(),
        "carrier-1".into(),
        now_ms(),
    );
    stores.phone_numbers.create(&number).await.unwrap();
    let runs = Arc::new(Runs(stores.runs.clone()));
    let call_id = CallId::generate();
    let run = runs
        .open(&ws.id, &chief.id, &call_id, "+16505550100")
        .await
        .unwrap();
    let call: Call = serde_json::from_value(serde_json::json!({"id":call_id,"workspace_id":ws.id,"agent_id":chief.id,"run_id":run.id,"phone_number_id":number.id,"direction":"inbound","remote_e164":"+16505550100","agent_name":"Robin","own_e164":number.e164,"purpose":"Take a message","tools":[],"tier":"trusted","state":"live","message_left":false,"transcript":[],"created_at":now_ms()})).unwrap();
    stores.calls.insert(&call).await.unwrap();
    let handoffs = CallHandoffs {
        messages: stores.messages.clone(),
        run_records: stores.runs.clone(),
        bus: Arc::new(pagis_audit::AuditEventBus::new(stores.events.clone())),
        agents: stores.agents.clone(),
        workspaces: stores.workspaces.clone(),
        users: stores.users.clone(),
        numbers: stores.phone_numbers.clone(),
        calls: stores.calls.clone(),
        runs,
        run_tools: Arc::new(Tools),
    };
    (stores, handoffs, call, target)
}
#[tokio::test]
async fn hand_off_changes_the_call_holder_and_uses_its_brief_and_voice() {
    let (stores, handoffs, call, target) = setup().await;
    let (run, brief) = handoffs
        .prepare(&call.workspace_id, &call.id, &call.run_id, &target.id)
        .await
        .unwrap();
    let held = stores
        .calls
        .get(&call.workspace_id, &call.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.agent_id, target.id);
    assert_eq!(held.run_id, run);
    assert_eq!(held.tier, TrustTier::Trusted);
    assert_eq!(brief.agent_id, target.id);
    assert_eq!(brief.voice, target.voice);
    assert_eq!(Some(brief.purpose), target.standing_brief);
    assert_eq!(
        stores
            .runs
            .get(&call.workspace_id, &run)
            .await
            .unwrap()
            .unwrap()
            .agent_id,
        target.id
    );
    assert_eq!(
        stores
            .runs
            .get(&call.workspace_id, &call.run_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RunState::Completed
    );
}
#[tokio::test]
async fn hand_off_refuses_an_agent_of_another_workspace() {
    let (stores, handoffs, call, _) = setup().await;
    let foreign = agent(&WorkspaceId::generate(), "Other");
    stores.agents.create(&foreign).await.unwrap();
    assert!(
        handoffs
            .prepare(&call.workspace_id, &call.id, &call.run_id, &foreign.id)
            .await
            .unwrap_err()
            .contains("active Agent of this Workspace")
    );
    assert_eq!(
        stores
            .calls
            .get(&call.workspace_id, &call.id)
            .await
            .unwrap()
            .unwrap(),
        call
    );
}
#[tokio::test]
async fn hand_off_refuses_an_archived_agent() {
    let (stores, handoffs, call, target) = setup().await;
    let mut target = target;
    target.status = AgentStatus::Archived;
    stores.agents.update(&target).await.unwrap();
    assert!(
        handoffs
            .prepare(&call.workspace_id, &call.id, &call.run_id, &target.id)
            .await
            .is_err()
    );
    assert_eq!(
        stores
            .calls
            .get(&call.workspace_id, &call.id)
            .await
            .unwrap()
            .unwrap(),
        call
    );
}
