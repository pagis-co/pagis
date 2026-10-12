//! The resident actor on native targets and under Node.

#[path = "actor_host/stores.rs"]
mod stores;

#[cfg(target_arch = "wasm32")]
#[path = "actor_host/brain_wasm.rs"]
mod brain_wasm;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use pagis_agent::{
    AgentDeps, AgentLoopConfig, AgentSystem, CoreToolRuntime, ProgressHub, StreamHub,
    ToolRuntimeDeps,
};
use pagis_broker::{
    AuthorizedCall, Broker, BrokerDeps, HostPresence, NoCredentialDirectory, NoSessionStarts,
    ToolExecutor, ToolResult,
};
use pagis_core::*;
use tokio::sync::Notify;

struct ActorHost {
    agent: Agent,
    states: Mutex<Vec<(RunId, RunState)>>,
    journal: Mutex<HashMap<RunId, RunJournalEntry>>,
    messages: Mutex<Vec<Message>>,
    events: Mutex<Vec<Event>>,
    entered: [Notify; 2],
    release: [Notify; 2],
    updated: Notify,
}

impl ActorHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            agent: serde_json::from_value(serde_json::json!({
                "id": "agent", "workspace_id": "workspace", "name": "Agent", "job": "", "description": "", "personality": "", "model_alias": "default", "avatar": AvatarAppearance::default(), "status": "active", "created_at": 0, "updated_at": 0,
            })).unwrap(),
            states: Mutex::new(Vec::new()),
            journal: Mutex::new(HashMap::new()),
            messages: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            entered: [Notify::new(), Notify::new()],
            release: [Notify::new(), Notify::new()],
            updated: Notify::new(),
        })
    }

    fn deps(self: &Arc<Self>, config: AgentLoopConfig) -> AgentDeps {
        let presence = Arc::new(HostPresence::default());
        let skills = Arc::new(NoSkills);
        let clock = Arc::new(SystemClock);
        let runtime = Arc::new(CoreToolRuntime::new(ToolRuntimeDeps {
            agents: self.clone(),
            channels: self.clone(),
            messages: self.clone(),
            runs: self.clone(),
            participants: self.clone(),
            memory: self.clone(),
            pending_evidence: self.clone(),
            conversation_evidence: self.clone(),
            grants: self.clone(),
            skills: skills.clone(),
            computers: self.clone(),
            bus: self.clone(),
            clock: clock.clone(),
            context_messages: config.context_messages,
            presence: presence.clone(),
        }));
        let broker = Arc::new(Broker::new(BrokerDeps {
            run_journal: self.clone(),
            forget: self.clone(),
            grants: self.clone(),
            credentials: Arc::new(NoCredentialDirectory),
            connections: self.clone(),
            requests: self.clone(),
            snapshots: self.clone(),
            runs: self.clone(),
            conversation_evidence: self.clone(),
            bus: self.clone(),
            executor: self.clone(),
            phone_numbers: self.clone(),
            mailboxes: self.clone(),
            hosts: self.clone(),
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
            run_journal: self.clone(),
            forget: self.clone(),
            workspaces: self.clone(),
            users: self.clone(),
            agents: self.clone(),
            channels: self.clone(),
            messages: self.clone(),
            runs: self.clone(),
            model_aliases: self.clone(),
            usage: self.clone(),
            capture: Arc::new(CaptureSetting::default()),
            model_request_captures: self.clone(),
            participants: self.clone(),
            requests: self.clone(),
            grants: self.clone(),
            connections: self.clone(),
            artifacts: self.clone(),
            blobs: Arc::new(object_store::memory::InMemory::new()),
            memory: self.clone(),
            pending_evidence: self.clone(),
            continuations: self.clone(),
            briefs: self.clone(),
            schedules: self.clone(),
            skills,
            computers: self.clone(),
            bus: self.clone(),
            brain: self.clone(),
            gate: self.clone(),
            models: Arc::new(pagis_agent::ModelCatalog::new(keys)),
            broker,
            tool_runtime: runtime,
            hub: Arc::new(StreamHub::default()),
            progress: Arc::new(ProgressHub::default()),
            triggers: self.clone(),
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
        panic!("Forget stops the Run before a model call")
    }
}

#[async_trait::async_trait]
impl pagis_agent::ModelCallGate for ActorHost {
    async fn before_call(&self, _: &Run, _: &[String]) -> Result<(), pagis_agent::GateStop> {
        panic!("Forget stops the Run before the ModelCallGate")
    }
}

#[async_trait::async_trait]
impl EventBus for ActorHost {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        let mut events = self.events.lock().unwrap();
        let event = Event {
            id: EventId::generate(),
            seq: events.len() as i64 + 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        };
        events.push(event.clone());
        Ok(event)
    }

    async fn subscribe(&self, _: EventScope, _: Option<i64>) -> EventStream {
        // Runs enter through enqueue_proactive. No event triggers another Run.
        futures::stream::empty().boxed()
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
    let host = ActorHost::new();
    let system = AgentSystem::start(host.deps(config)).await;
    let runs = [run("first"), run("second")];
    for run in &runs {
        system.enqueue_proactive(run.clone());
    }
    pagis_rt::timeout(Duration::from_secs(3), async {
        host.entered[0].notified().await;
        pagis_rt::sleep(config.idle_timeout * 2).await;
        assert_eq!(
            *host.states.lock().unwrap(),
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
            *host.states.lock().unwrap(),
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
            let changed = host.updated.notified();
            if host.states.lock().unwrap().len() == 4 {
                break;
            }
            changed.await;
        }
        while system.resident_agents() != 0 {
            pagis_rt::sleep(Duration::from_millis(5)).await;
        }
        assert!(ended.elapsed() >= config.idle_timeout);
        assert_eq!(
            *host.states.lock().unwrap(),
            [
                (runs[0].id.clone(), RunState::Running),
                (runs[0].id.clone(), RunState::Failed),
                (runs[1].id.clone(), RunState::Running),
                (runs[1].id.clone(), RunState::Failed),
            ]
        );
    })
    .await
    .expect("the actor ends after both Runs");
}
