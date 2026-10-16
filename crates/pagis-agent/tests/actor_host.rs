//! The resident actor on native targets and under Node.

#[cfg(target_arch = "wasm32")]
#[path = "actor_host/brain_wasm.rs"]
mod brain_wasm;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pagis_agent::{
    AgentDeps, AgentLoopConfig, AgentSystem, CoreToolRuntime, ProgressHub, StreamHub,
    ToolRuntimeDeps,
};
use pagis_broker::{
    AuthorizedCall, Broker, BrokerDeps, HostPresence, NoCredentialDirectory, NoSessionStarts,
    ToolExecutor, ToolResult,
};
use pagis_core::fake::{FakeDatabase, FakeEventBus, FakeMemoryStore};
use pagis_core::*;
use tokio::sync::Notify;

struct ActorHost {
    agent: Agent,
    stores: Stores,
    bus: Arc<FakeEventBus>,
    memory: Arc<FakeMemoryStore>,
    entered: [Notify; 2],
    release: [Notify; 2],
}

impl ActorHost {
    async fn new() -> Arc<Self> {
        let database = FakeDatabase::default();
        let stores = database.stores();
        let agent: Agent = serde_json::from_value(serde_json::json!({
            "id": "agent", "workspace_id": "workspace", "name": "Agent", "job": "", "description": "", "personality": "", "model_alias": "default", "avatar": AvatarAppearance::default(), "status": "active", "created_at": 0, "updated_at": 0,
        })).unwrap();
        let person =
            seed_org_and_administrator(stores.orgs.as_ref(), stores.users.as_ref(), "Org", 0)
                .await
                .unwrap();
        stores
            .workspaces
            .create(&Workspace {
                kind: pagis_core::WorkspaceKind::Private,
                monthly_spend_cap_usd: None,
                id: agent.workspace_id.clone(),
                user_id: Some(person.id),
                name: "Desk".into(),
                timezone: "UTC".into(),
                created_at: 0,
                onboarded_at: None,
                chief_of_staff_agent_id: None,
                report_schedule_id: None,
                home_exit_host_id: None,
            })
            .await
            .unwrap();
        stores.agents.create(&agent).await.unwrap();
        stores
            .model_aliases
            .create(&ModelAlias {
                id: ModelAliasId::generate(),
                workspace_id: agent.workspace_id.clone(),
                alias: "default".into(),
                candidates: vec!["anthropic/claude-sonnet-4-6".into()],
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
        stores
            .channels
            .create(&Channel {
                id: ChannelId::from("channel".to_string()),
                workspace_id: agent.workspace_id.clone(),
                kind: ChannelKind::Dm,
                title: None,
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
        Arc::new(Self {
            agent,
            memory: Arc::new(FakeMemoryStore::new(stores.forget.clone())),
            stores,
            bus: Arc::new(database.bus()),
            entered: [Notify::new(), Notify::new()],
            release: [Notify::new(), Notify::new()],
        })
    }

    async fn states(&self) -> Vec<(RunId, RunState)> {
        self.stores
            .events
            .list_after(Some(&self.agent.workspace_id), 0, u32::MAX)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == "run.state_changed")
            .map(|event| {
                (
                    event.run_id.unwrap(),
                    serde_json::from_value(event.payload["to"].clone()).unwrap(),
                )
            })
            .collect()
    }

    fn deps(self: &Arc<Self>, config: AgentLoopConfig) -> AgentDeps {
        let presence = Arc::new(HostPresence::default());
        let skills = Arc::new(NoSkills);
        let clock = Arc::new(SystemClock);
        let runtime = Arc::new(CoreToolRuntime::new(ToolRuntimeDeps {
            agents: self.stores.agents.clone(),
            channels: self.stores.channels.clone(),
            messages: self.stores.messages.clone(),
            runs: self.stores.runs.clone(),
            participants: self.stores.participants.clone(),
            memory: self.memory.clone(),
            pending_evidence: self.stores.pending_evidence.clone(),
            conversation_evidence: self.stores.conversation_evidence.clone(),
            grants: self.stores.grants.clone(),
            skills: skills.clone(),
            computers: self.clone(),
            bus: self.bus.clone(),
            clock: clock.clone(),
            context_messages: config.context_messages,
            presence: presence.clone(),
        }));
        let broker = Arc::new(Broker::new(BrokerDeps {
            numbers: self.stores.phone_numbers.clone(),
            run_journal: self.stores.run_journal.clone(),
            forget: self.stores.forget.clone(),
            grants: self.stores.grants.clone(),
            credentials: Arc::new(NoCredentialDirectory),
            connections: self.stores.connections.clone(),
            requests: self.stores.requests.clone(),
            snapshots: self.stores.capability_snapshots.clone(),
            runs: self.stores.runs.clone(),
            conversation_evidence: self.stores.conversation_evidence.clone(),
            bus: self.bus.clone(),
            executor: self.clone(),

            mailboxes: self.stores.agent_mailboxes.clone(),
            hosts: self.stores.hosts.clone(),
            presence,
            session_starts: Arc::new(NoSessionStarts),
            computer: false,
        }));
        let keys = Arc::new(ProviderKeys::with_env(
            |_| None,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        ));
        AgentDeps {
            run_journal: self.stores.run_journal.clone(),
            forget: self.stores.forget.clone(),
            workspaces: self.stores.workspaces.clone(),
            users: self.stores.users.clone(),
            agents: self.stores.agents.clone(),
            channels: self.stores.channels.clone(),
            messages: self.stores.messages.clone(),
            text_records: self.stores.text_records.clone(),
            runs: self.stores.runs.clone(),
            model_aliases: self.stores.model_aliases.clone(),
            usage: self.stores.usage.clone(),
            capture: Arc::new(CaptureSetting::default()),
            model_request_captures: self.stores.model_request_captures.clone(),
            participants: self.stores.participants.clone(),
            requests: self.stores.requests.clone(),
            grants: self.stores.grants.clone(),
            connections: self.stores.connections.clone(),
            files: self.stores.files.clone(),
            blobs: Arc::new(object_store::memory::InMemory::new()),
            memory: self.memory.clone(),
            pending_evidence: self.stores.pending_evidence.clone(),
            continuations: self.stores.continuations.clone(),
            briefs: self.stores.briefs.clone(),
            schedules: self.stores.schedules.clone(),
            skills,
            computers: self.clone(),
            bus: self.bus.clone(),
            brain: self.clone(),
            gate: self.clone(),
            models: Arc::new(pagis_agent::ModelCatalog::new(keys)),
            broker,
            tool_runtime: runtime,
            hub: Arc::new(StreamHub::default()),
            progress: Arc::new(ProgressHub::default()),
            triggers: self.stores.triggers.clone(),
            clock,
            config,
        }
    }
}

impl Computers for ActorHost {
    fn get(&self, _: &WorkspaceId) -> Arc<dyn Computer> {
        panic!("the actor test has no Computer action")
    }
}

#[async_trait::async_trait]
impl ToolExecutor for ActorHost {
    async fn execute(&self, _: AuthorizedCall) -> ToolResult {
        panic!("the actor test has no tool call")
    }
}

#[async_trait::async_trait]
impl pagis_agent::Brain for ActorHost {
    async fn turn(
        &self,
        _: pagis_agent::TurnRequest,
    ) -> Result<pagis_agent::TurnStream, pagis_agent::BrainError> {
        panic!("the ModelCallGate stops the Run before a model call")
    }
}

#[async_trait::async_trait]
impl pagis_agent::ModelCallGate for ActorHost {
    async fn before_call(
        &self,
        run: &Run,
        _: &pagis_agent::ModelCallBound,
    ) -> Result<(), pagis_agent::GateStop> {
        assert_eq!(run.workspace_id, self.agent.workspace_id);
        let index = if run.id.as_str() == "first" {
            0
        } else {
            assert_eq!(run.id.as_str(), "second");
            1
        };
        self.entered[index].notify_one();
        self.release[index].notified().await;
        Err(pagis_agent::GateStop {
            note: "The test stops the Run.".into(),
            failure_kind: FailureKind::AccessChanged,
        })
    }
}

fn run(id: &str) -> Run {
    serde_json::from_value(serde_json::json!({
        "id": id, "title": "Work", "workspace_id": "workspace", "agent_id": "agent", "channel_id": "channel", "root_message_id": "root", "trigger_kind": "schedule", "state": "queued", "hop_count": 0, "created_at": 0,
    })).unwrap()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn actor_serializes_one_thread_and_ends_when_idle() {
    let config = AgentLoopConfig {
        max_concurrent_runs: 2,
        idle_timeout: Duration::from_millis(25),
        computer: false,
        ..AgentLoopConfig::default()
    };
    let host = ActorHost::new().await;
    let system = AgentSystem::start(host.deps(config)).await;
    let runs = [run("first"), run("second")];
    for run in &runs {
        host.stores.runs.create(run).await.unwrap();
        system.enqueue_proactive(run.clone());
    }
    let result = pagis_rt::timeout(Duration::from_secs(3), async {
        host.entered[0].notified().await;
        pagis_rt::sleep(config.idle_timeout * 2).await;
        assert_eq!(
            host.states().await,
            [(runs[0].id.clone(), RunState::Running)]
        );
        assert_eq!(
            system.resident_agents(),
            1,
            "a running Run prevents idle expiry"
        );
        assert_eq!(
            system.available_slots(&host.agent.id).await.conversation,
            0,
            "the second Run stays queued in Admission"
        );
        host.release[0].notify_one();
        host.entered[1].notified().await;
        assert_eq!(
            host.states().await,
            [
                (runs[0].id.clone(), RunState::Running),
                (runs[0].id.clone(), RunState::Failed),
                (runs[1].id.clone(), RunState::Running),
            ]
        );
        pagis_rt::sleep(config.idle_timeout * 2).await;
        assert_eq!(system.resident_agents(), 1);
        let ended = pagis_rt::Instant::now();
        host.release[1].notify_one();
        loop {
            if host.states().await.len() == 4 {
                break;
            }
            pagis_rt::sleep(Duration::from_millis(5)).await;
        }
        while system.resident_agents() != 0 {
            pagis_rt::sleep(Duration::from_millis(5)).await;
        }
        assert!(ended.elapsed() >= config.idle_timeout);
        assert_eq!(
            host.states().await,
            [
                (runs[0].id.clone(), RunState::Running),
                (runs[0].id.clone(), RunState::Failed),
                (runs[1].id.clone(), RunState::Running),
                (runs[1].id.clone(), RunState::Failed),
            ]
        );
    })
    .await;
    assert!(
        result.is_ok(),
        "the actor ends after both Runs: {result:?}; records: {:?}; states: {:?}",
        host.stores
            .runs
            .list(&host.agent.workspace_id, None, None, &[], None, 10)
            .await
            .unwrap(),
        host.states().await
    );
}
