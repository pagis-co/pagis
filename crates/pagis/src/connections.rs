//! Provider dispatch for connection-backed tools (ADR-0005).
//!
//! One provider instance belongs to one Connection and serves every
//! Agent granted it. The instance is built on the first call and kept,
//! so an account is bound once. Every call re-reads the Connection
//! first, so a deleted or disconnected one stops the next call without
//! cutting short a call already dispatched, and a reconfigured one is
//! bound again rather than served stale.
//!
//! Two rules here are load-bearing, and both exist because an external
//! effect cannot be taken back:
//!
//! - A call is retried only when the provider proves the request never
//!   left. Anything else reports `outcome_unknown` and lets the user
//!   decide, rather than sending one email twice.
//! - A call that outruns its timeout is `outcome_unknown` when it could
//!   have changed something, and only `temporarily_unavailable` when it
//!   could not.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::knowledge::SourceRead;
use pagis_core::{Connection, ConnectionId, ConnectionStore, EventBus, StoreError, WorkspaceId};

/// The timeout a call runs under when its manifest names none.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// A provider failure, already stripped of upstream text. `retryable`
/// means the provider knows the request never reached the far side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub code: String,
    pub retryable: bool,
}

impl ProviderFailure {
    pub fn new(code: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            retryable,
        }
    }
}

/// One bound external account the broker can dispatch to.
#[async_trait]
pub trait ConnectionProvider: Send + Sync {
    /// Whether an interrupted call may already have changed something.
    /// It decides what an unfinished call reports, so a provider that
    /// is unsure answers `true`.
    fn is_write(&self, tool: &str) -> bool;

    /// True only when this provider knows that one result is bounded,
    /// exact evidence from the selected Connection.
    fn retain_result(&self, _tool: &str) -> bool {
        false
    }

    /// The synced source content that one result of `tool` holds, for
    /// the record of what a Run read (ADR-0008). A provider whose
    /// Connection has no synced resource reads none.
    fn source_reads(
        &self,
        _tool: &str,
        _arguments: &serde_json::Value,
        _result: &serde_json::Value,
    ) -> Vec<SourceRead> {
        Vec::new()
    }

    async fn invoke(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderFailure>;
}

/// Builds the provider for one Connection from its trusted config.
pub trait ConnectionProviderFactory: Send + Sync {
    fn build(
        &self,
        connection: &Connection,
    ) -> Result<Arc<dyn ConnectionProvider>, ProviderFailure>;
}

/// Provider-detected Connection state changes. Every caller gets the same
/// durable transition and the same event that refreshes open clients.
pub struct ConnectionState {
    connections: Arc<dyn ConnectionStore>,
    bus: Arc<dyn EventBus>,
}

impl ConnectionState {
    pub fn new(connections: Arc<dyn ConnectionStore>, bus: Arc<dyn EventBus>) -> Self {
        Self { connections, bus }
    }

    pub async fn require_reauthorization(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<bool, StoreError> {
        let Some(connection) = self.connections.get(workspace_id, connection_id).await? else {
            return Ok(false);
        };
        if connection.status == Connection::REAUTH_REQUIRED {
            return Ok(false);
        }
        if !self
            .connections
            .set_status(workspace_id, connection_id, Connection::REAUTH_REQUIRED)
            .await?
        {
            return Ok(false);
        }
        self.bus
            .publish(pagis_core::NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "connection.changed".to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({
                    "connection_id": connection_id.as_str(),
                    "status": Connection::REAUTH_REQUIRED,
                }),
            })
            .await?;
        Ok(true)
    }
}

pub struct ConnectionRuntime {
    connections: Arc<dyn ConnectionStore>,
    factory: Arc<dyn ConnectionProviderFactory>,
    state: Arc<ConnectionState>,
    default_timeout: Duration,
    providers: Mutex<HashMap<ConnectionId, Bound>>,
}

/// What one completed call gave back: the result, whether it is exact
/// evidence to retain, and the synced source content that it holds.
struct Dispatched {
    value: serde_json::Value,
    retain: bool,
    reads: Vec<SourceRead>,
}

/// A built provider and the config it was built from, so a Connection
/// the user reconfigured is rebound instead of served stale.
struct Bound {
    config: serde_json::Value,
    provider: Arc<dyn ConnectionProvider>,
}

impl ConnectionRuntime {
    pub fn new(
        connections: Arc<dyn ConnectionStore>,
        factory: Arc<dyn ConnectionProviderFactory>,
        state: Arc<ConnectionState>,
        default_timeout: Duration,
    ) -> Self {
        Self {
            connections,
            factory,
            state,
            default_timeout,
            providers: Mutex::new(HashMap::new()),
        }
    }

    async fn provider(
        &self,
        connection: &Connection,
    ) -> Result<Arc<dyn ConnectionProvider>, ProviderFailure> {
        let id = &connection.id;
        if let Some(bound) = self
            .providers
            .lock()
            .expect("connection provider lock")
            .get(id)
            .filter(|bound| bound.config == connection.config)
        {
            return Ok(Arc::clone(&bound.provider));
        }
        let provider = self.factory.build(connection)?;
        self.providers
            .lock()
            .expect("connection provider lock")
            .insert(
                id.clone(),
                Bound {
                    config: connection.config.clone(),
                    provider: Arc::clone(&provider),
                },
            );
        Ok(provider)
    }

    async fn dispatch(&self, call: &AuthorizedCall) -> Result<Dispatched, ProviderFailure> {
        let Some(id) = call.selected_connection.as_deref() else {
            return Err(ProviderFailure::new("connection_required", false));
        };
        let connection = self
            .connections
            .get(&call.workspace_id, &ConnectionId::from(id.to_string()))
            .await
            .map_err(|_| ProviderFailure::new("temporarily_unavailable", false))?
            .ok_or_else(|| ProviderFailure::new("permission_revoked", false))?;
        if connection.status != Connection::CONNECTED {
            return Err(ProviderFailure::new("reauth_required", false));
        }
        let provider = self.provider(&connection).await?;
        let write = provider.is_write(&call.tool_name);
        let retain = provider.retain_result(&call.tool_name);
        let timeout = call.call_timeout.unwrap_or(self.default_timeout);
        let outcome = match self.attempt(provider.as_ref(), call, timeout, write).await {
            // The one retry: the provider knows the request never left.
            Err(failure) if failure.retryable => {
                self.attempt(provider.as_ref(), call, timeout, write).await
            }
            outcome => outcome,
        };
        if let Err(failure) = &outcome
            && matches!(
                failure.code.as_str(),
                "reauth_required" | "permission_revoked"
            )
        {
            self.mark_reauth_required(&connection).await;
        }
        outcome.map(|value| Dispatched {
            reads: provider.source_reads(&call.tool_name, &call.arguments, &value),
            value,
            retain,
        })
    }

    /// An authorization the provider no longer accepts is the record's
    /// state, not one run's bad luck: access revoked at Google
    /// surfaces on the Connection card, and the next call stops before
    /// it reaches the provider at all.
    async fn mark_reauth_required(&self, connection: &Connection) {
        if let Err(error) = self
            .state
            .require_reauthorization(&connection.workspace_id, &connection.id)
            .await
        {
            tracing::error!(%error, "cannot record that a connection needs reauthorization");
        }
    }

    async fn attempt(
        &self,
        provider: &dyn ConnectionProvider,
        call: &AuthorizedCall,
        timeout: Duration,
        write: bool,
    ) -> Result<serde_json::Value, ProviderFailure> {
        match tokio::time::timeout(timeout, provider.invoke(&call.tool_name, &call.arguments)).await
        {
            Ok(outcome) => outcome,
            // A call that ran out of time was dispatched, so it is never
            // retried and never reported as a plain failure.
            Err(_) if write => Err(ProviderFailure::new("outcome_unknown", false)),
            Err(_) => Err(ProviderFailure::new("temporarily_unavailable", false)),
        }
    }
}

#[async_trait]
impl ToolExecutor for ConnectionRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        if !matches!(call.route, ToolRoute::Connection { .. }) {
            return ToolResult::error(
                "invalid_request",
                format!("{} is not a connection tool", call.tool_name),
            );
        }
        match self.dispatch(&call).await {
            Ok(Dispatched {
                value,
                retain,
                reads,
            }) => {
                let result = ToolResult::success(value.to_string()).reading(&reads);
                if retain {
                    result.retain_as_conversation_evidence()
                } else {
                    result
                }
            }
            Err(failure) => ToolResult::error(failure.code, "the provider did not complete this"),
        }
    }
}

/// Binds one Google Connection to the daemon's `gog`.
///
/// Three things come from the record and none from the environment: the
/// account, the alias that names the `gog` client, and the `GOG_HOME` of
/// the Workspace that owns the Connection. A `brokered` Connection gets
/// a fourth: the access token the daemon mints per call from the refresh
/// token it holds sealed, so `gog` keeps no token of its own.
///
/// Every caller that reaches Google through a Connection binds it here,
/// so no path can bind one with a shared home or with no token.
pub struct GoogleBinder {
    runner: Arc<dyn pagis_google::GogRunner>,
    gog_root: std::path::PathBuf,
    google: Arc<pagis_connect::GoogleBroker>,
}

impl GoogleBinder {
    pub fn new(
        runner: Arc<dyn pagis_google::GogRunner>,
        gog_root: impl Into<std::path::PathBuf>,
        google: Arc<pagis_connect::GoogleBroker>,
    ) -> Self {
        Self {
            runner,
            gog_root: gog_root.into(),
            google,
        }
    }

    /// The bound provider of one Connection, or `None` when the record
    /// carries no usable Google binding. That is a Connection the user
    /// has to connect again, not a transient fault.
    pub fn provider(
        &self,
        connection: &Connection,
    ) -> Option<pagis_google::GoogleProvider<Arc<dyn pagis_google::GogRunner>>> {
        let binding = pagis_google::ConnectionBinding::new(
            connection.config["account"].as_str().unwrap_or_default(),
            connection.config["client"].as_str().unwrap_or_default(),
            pagis_google::workspace_gog_home(&self.gog_root, &connection.workspace_id),
        )
        .ok()?;
        let provider = pagis_google::GoogleProvider::new(binding, Arc::clone(&self.runner));
        Some(
            match connection.auth_mode == Connection::AUTH_MODE_BROKERED {
                true => provider.with_access_tokens(
                    self.google
                        .access_tokens(&connection.workspace_id, &connection.id),
                ),
                false => provider,
            },
        )
    }
}

/// Builds the Google provider from a Connection's account and client.
/// It runs the same `gog` the connect flow does, so one runner
/// serves both and a test replaces both at once.
pub struct GoogleProviderFactory {
    binder: Arc<GoogleBinder>,
}

impl GoogleProviderFactory {
    pub fn new(binder: Arc<GoogleBinder>) -> Self {
        Self { binder }
    }
}

/// The `gog` the release puts beside the daemon. A development build also
/// uses that location, so a missing package entry cannot be hidden by PATH.
pub fn distributed_gog(
    secrets: Arc<dyn pagis_core::SecretStore>,
) -> anyhow::Result<Arc<dyn pagis_google::GogRunner>> {
    let executable = std::env::current_exe()
        .map_err(|error| anyhow::anyhow!("cannot locate pagis for its packaged gog: {error}"))?;
    let directory = executable
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the pagis executable has no package directory for gog"))?;
    Ok(Arc::new(pagis_google::SystemGogRunner::new(
        directory.join("gog"),
        // `gog` keeps its tokens on its file backend under the tenant's
        // own `GOG_HOME`, with a password the daemon holds.
        Arc::new(pagis_google::SecretStoreKeyring::new(secrets)),
    )))
}

impl ConnectionProviderFactory for GoogleProviderFactory {
    fn build(
        &self,
        connection: &Connection,
    ) -> Result<Arc<dyn ConnectionProvider>, ProviderFailure> {
        // A Connection without its trusted binding is one the user has
        // to connect again; it is not a transient provider fault.
        let provider = self
            .binder
            .provider(connection)
            .ok_or_else(|| ProviderFailure::new("reauth_required", false))?;
        Ok(Arc::new(GoogleConnection { provider }))
    }
}

struct GoogleConnection {
    provider: pagis_google::GoogleProvider<Arc<dyn pagis_google::GogRunner>>,
}

#[async_trait]
impl ConnectionProvider for GoogleConnection {
    fn is_write(&self, tool: &str) -> bool {
        pagis_google::is_write(tool)
    }

    fn retain_result(&self, tool: &str) -> bool {
        matches!(
            tool,
            pagis_broker::MAIL_GET_MESSAGE
                | pagis_broker::MAIL_GET_THREAD
                | "google__calendar_events"
        )
    }

    fn source_reads(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
        result: &serde_json::Value,
    ) -> Vec<SourceRead> {
        pagis_google::source_reads(tool, arguments, result)
    }

    async fn invoke(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderFailure> {
        let call = pagis_google::call_from_tool(tool, arguments)
            .map_err(|_| ProviderFailure::new("invalid_request", false))?;
        self.provider
            .invoke(&call)
            .await
            .map_err(|error| ProviderFailure::new(error.code.as_str(), error.retryable))
    }
}
