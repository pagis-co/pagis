//! The tool executor the broker dispatches to.
//!
//! The broker holds one executor and hands it an `AuthorizedCall` that
//! already carries its route. This is the composition point that sends
//! each route to the runtime that owns it: the core tools to the agent
//! loop's runtime, the vault tools to the vault, the provider tools to
//! the connection runtime, the call tool to telephony, the mail tools
//! to the mail crate. No runtime knows about the others.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, Broker, ToolDef, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::RunId;

/// One executor slot filled after the broker exists. The Event
/// Subscription tools need the Trigger module, and the Trigger module
/// reads the declarations the broker holds, so the two are built in
/// that order and joined here rather than through a cycle.
#[derive(Default)]
pub struct DeferredExecutor {
    inner: std::sync::OnceLock<Arc<dyn ToolExecutor>>,
}

impl DeferredExecutor {
    /// Fill the slot. A second call keeps the first executor.
    pub fn set(&self, executor: Arc<dyn ToolExecutor>) {
        let _ = self.inner.set(executor);
    }
}

#[async_trait]
impl ToolExecutor for DeferredExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        match self.inner.get() {
            Some(executor) => executor.execute(call).await,
            None => ToolResult::error(
                "temporarily_unavailable",
                format!("{} is not registered on this daemon", call.tool_name),
            ),
        }
    }
}

pub struct RoutingExecutor {
    core: Arc<dyn ToolExecutor>,
    vault: Arc<dyn ToolExecutor>,
    connections: Arc<dyn ToolExecutor>,
    /// The Event Subscription core tools. They are core-routed,
    /// but they reach the Trigger module, which the agent loop's own
    /// runtime knows nothing about.
    subscriptions: Arc<dyn ToolExecutor>,
    /// The call tool. Core-routed, and executed by telephony.
    phone: Arc<dyn ToolExecutor>,
    /// `software_publish` and every published Software tool.
    software: Arc<dyn ToolExecutor>,
    /// Every tool of an installed Plugin's MCP server.
    plugins: Arc<dyn ToolExecutor>,
    /// The `mail__*` tools in the Agent's own mailbox. They are
    /// Mailbox-routed, and executed by the mail crate.
    mail: Arc<dyn ToolExecutor>,
}

/// How the call tool reads the tools a Run may use on a call: the
/// broker answers from the Run's Capability Snapshot (ADR-0020).
pub struct BrokerRunTools(pub Arc<Broker>);

#[async_trait]
impl pagis_telephony::RunTools for BrokerRunTools {
    async fn free_tools(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<ToolDef>, String> {
        self.0
            .free_tools(workspace_id, run_id)
            .await
            .map_err(|error| error.to_string())
    }
}

pub struct AutomationExecutor {
    schedules: Arc<dyn ToolExecutor>,
    subscriptions: Arc<dyn ToolExecutor>,
}

impl AutomationExecutor {
    pub fn new(schedules: Arc<dyn ToolExecutor>, subscriptions: Arc<dyn ToolExecutor>) -> Self {
        Self {
            schedules,
            subscriptions,
        }
    }
}

#[async_trait]
impl ToolExecutor for AutomationExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        if crate::schedule_tools::ScheduleToolRuntime::owns(&call.route) {
            self.schedules.execute(call).await
        } else {
            self.subscriptions.execute(call).await
        }
    }
}

impl RoutingExecutor {
    // One argument per tool runtime: this is the composition point.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        core: Arc<dyn ToolExecutor>,
        vault: Arc<dyn ToolExecutor>,
        connections: Arc<dyn ToolExecutor>,
        subscriptions: Arc<dyn ToolExecutor>,
        phone: Arc<dyn ToolExecutor>,
        software: Arc<dyn ToolExecutor>,
        plugins: Arc<dyn ToolExecutor>,
        mail: Arc<dyn ToolExecutor>,
    ) -> Self {
        Self {
            core,
            vault,
            connections,
            subscriptions,
            phone,
            software,
            plugins,
            mail,
        }
    }
}

#[async_trait]
impl ToolExecutor for RoutingExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        if crate::subscription_tools::SubscriptionToolRuntime::owns(&call.route)
            || crate::schedule_tools::ScheduleToolRuntime::owns(&call.route)
        {
            return self.subscriptions.execute(call).await;
        }
        if pagis_telephony::PhoneToolRuntime::owns(&call.route) {
            return self.phone.execute(call).await;
        }
        if crate::software_tools::SoftwareToolRuntime::owns(&call.route) {
            return self.software.execute(call).await;
        }
        if crate::plugin_tools::PluginToolRuntime::owns(&call.route) {
            return self.plugins.execute(call).await;
        }
        if pagis_mail::MailToolRuntime::owns(&call.route) {
            return self.mail.execute(call).await;
        }
        match call.route {
            ToolRoute::Vault { .. } => self.vault.execute(call).await,
            ToolRoute::Connection { .. } => self.connections.execute(call).await,
            // `add_block` writes into the run's own message, which
            // the agent loop owns.
            ToolRoute::Core { .. } | ToolRoute::Ui { .. } => self.core.execute(call).await,
            // Every Software route went to the Software runtime above,
            // and every Mailbox route to the mail runtime.
            ToolRoute::Software { .. } | ToolRoute::Mailbox { .. } => ToolResult::error(
                "temporarily_unavailable",
                "that tool runtime is not registered",
            ),
            ToolRoute::Plugin { .. } => ToolResult::error(
                "temporarily_unavailable",
                "the plugin host is not registered",
            ),
        }
    }
}

/// The Run of a call the Agent answered (ADR-0020). It opens in
/// the Agent's Thread with the user (ADR-0022), already running, with
/// its Capability Snapshot bound by the broker so the call's free
/// tools come from the same place an outbound call's do.
pub struct BrokerInboundRuns {
    pub runs: Arc<dyn pagis_core::RunStore>,
    pub channels: Arc<dyn pagis_core::ChannelStore>,
    pub broker: Arc<Broker>,
    pub bus: Arc<dyn pagis_core::EventBus>,
    /// The daemon's logical now: a Run's recorded start and end come
    /// from it, as every other Run's do.
    pub clock: Arc<dyn pagis_core::Clock>,
}

#[async_trait]
impl pagis_telephony::InboundRuns for BrokerInboundRuns {
    async fn open(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: &pagis_core::AgentId,
        call_id: &pagis_core::CallId,
    ) -> Result<pagis_core::Run, String> {
        use pagis_core::{NewEvent, Run, RunState, TriggerKind};

        let channel = self
            .channels
            .find_user_dm(workspace_id, agent_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "that Agent has no thread with the user".to_string())?;
        let now = self.clock.now_ms();
        let run = Run {
            id: RunId::generate(),
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            channel_id: Some(channel.id),
            root_message_id: None,
            trigger_kind: TriggerKind::Event,
            trigger_ref: Some(call_id.to_string()),
            hop_count: 0,
            origin: None,
            state: RunState::Running,
            failure_kind: None,
            dismissed_at: None,
            error: None,
            started_at: Some(now),
            ended_at: None,
            created_at: now,
        };
        self.runs
            .create(&run)
            .await
            .map_err(|error| error.to_string())?;
        self.broker
            .prepare_run(workspace_id, agent_id, &run.id)
            .await
            .map_err(|error| error.to_string())?;
        self.bus
            .publish(NewEvent {
                workspace_id: run.workspace_id.clone(),
                event_type: "run.created".to_string(),
                agent_id: Some(run.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: run.channel_id.clone(),
                payload: serde_json::json!({
                    "trigger_kind": run.trigger_kind,
                    "trigger_ref": run.trigger_ref,
                    "root_message_id": serde_json::Value::Null,
                }),
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(run)
    }

    async fn close(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        run_id: &RunId,
        error: Option<String>,
    ) {
        use pagis_core::{NewEvent, RunState};

        let mut run = match self.runs.get(workspace_id, run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, %run_id, "the Run of the call was not read");
                return;
            }
        };
        let from = run.state;
        run.state = if error.is_some() {
            RunState::Failed
        } else {
            RunState::Completed
        };
        run.failure_kind = error.as_ref().map(|_| pagis_core::FailureKind::CallFailed);
        run.error = error;
        run.ended_at = Some(self.clock.now_ms());
        if let Err(error) = self.runs.update(&run).await {
            tracing::error!(%error, %run_id, "the Run of the call did not settle");
        }
        let published = self
            .bus
            .publish(NewEvent {
                workspace_id: run.workspace_id.clone(),
                event_type: "run.state_changed".to_string(),
                agent_id: Some(run.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: run.channel_id.clone(),
                payload: serde_json::json!({
                    "from": from,
                    "to": run.state,
                    "reason": "call ended",
                    "error": run.error,
                    "failure_kind": run.failure_kind,
                }),
            })
            .await;
        if let Err(error) = published {
            tracing::error!(%error, %run_id, "run.state_changed publish failed");
        }
    }
}
