//! The Workspace vault (ADR-0013): the Credential records, the
//! minting, the encryption, and the fill.
//!
//! The hard constraint is that a secret never reaches the model. It is
//! not in a prompt, a tool result, a retained screenshot, or a
//! container file. No tool returns a secret, and the vault ships no
//! export path: an export is a "show password" control under another
//! name, and it would widen the blast radius that unique per-domain
//! secrets exist to bound. Losing the store costs one password reset
//! per domain, and that is a stated position.
//!
//! The vault drives the Computer through the daemon's browser channel
//! (`fill`). `pagis-computer` carries each value as a `FillField`, whose
//! debug print never shows the text.

pub mod domain;
pub mod fill;
pub mod keypad;
pub mod manifest;
pub mod recipe;
pub mod totp;

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{
    AuthorizedCall, CredentialAction, CredentialActionKind, CredentialDirectory, ToolExecutor,
    ToolResult, ToolRoute, VaultTool,
};
use pagis_computer::ComputerManagers;
use pagis_core::{
    AgentId, Credential, CredentialId, CredentialProvenance, CredentialStore, RunId, StoreError,
    WorkspaceId, now_ms,
};

pub use keypad::{KeypadCode, KeypadError};
pub use manifest::{VAULT_NAMESPACE, manifest};
pub use pagis_core::{DataKey, TenantKeys, data_key_name};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{0}")]
    BadLoginUrl(String),
    #[error("{0}")]
    BadDomain(String),
    #[error(
        "the login address is on {login_url_domain}, but the record is for {domain}; \
         one record covers one registrable domain"
    )]
    DomainMismatch {
        domain: String,
        login_url_domain: String,
    },
    #[error("{0}")]
    BadRecipe(String),
    #[error("the one-time code seed is unusable: {0}")]
    BadTotpSeed(String),
    #[error("no credential with id {0}")]
    NotFound(String),
    #[error("vault key error: {0}")]
    Key(String),
    /// The Tenant Data Key could not be read, minted or used.
    #[error(transparent)]
    Seal(#[from] pagis_core::SealError),
    #[error("computer error: {0}")]
    Computer(String),
    /// A fill wrote nothing: a check of its destination failed, or the
    /// browser did not complete a step. The reason goes to the Agent and
    /// into the audit fact.
    #[error("the fill failed: {0}")]
    FillFailed(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl VaultError {
    /// The error code the tool result carries. A caller-fixable problem
    /// reads as an invalid request; everything else is a failure.
    fn code(&self) -> &'static str {
        match self {
            VaultError::BadLoginUrl(_)
            | VaultError::BadDomain(_)
            | VaultError::DomainMismatch { .. }
            | VaultError::BadRecipe(_)
            | VaultError::BadTotpSeed(_) => "invalid_request",
            VaultError::NotFound(_) => "not_found",
            VaultError::Computer(_) => "temporarily_unavailable",
            VaultError::FillFailed(_) => "failed",
            VaultError::Key(_) | VaultError::Seal(_) | VaultError::Store(_) => "internal",
        }
    }

    /// Why a fill did not write, as the audit fact records it.
    fn reason(&self) -> String {
        match self {
            VaultError::FillFailed(reason) => reason.clone(),
            other => other.to_string(),
        }
    }
}

/// What `vault__create` hands the vault to mint a login for an Agent.
#[derive(Debug, Clone)]
pub struct MintRequest {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub run_id: RunId,
    pub domain: String,
    pub username: String,
    pub login_url: String,
    /// The site's published password rules, when it publishes any.
    pub rules: Option<String>,
}

/// What settings hands the vault to save a login the user already has.
#[derive(Debug, Clone)]
pub struct NewCredential {
    pub workspace_id: WorkspaceId,
    pub domain: String,
    pub username: String,
    pub login_url: String,
    pub secret: String,
    pub totp_seed: Option<String>,
}

/// One credential as anything outside the vault may see it. There is
/// no secret here and no way to ask for one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CredentialView {
    pub id: String,
    pub domain: String,
    pub username: String,
    pub login_url: String,
    /// `user_supplied` or `agent_minted`.
    pub provenance: String,
    /// True when the record carries a one-time code seed.
    pub has_totp: bool,
    pub owner_agent_id: Option<String>,
    pub created_at: i64,
}

impl From<&Credential> for CredentialView {
    fn from(credential: &Credential) -> Self {
        Self {
            id: credential.id.to_string(),
            domain: credential.domain.clone(),
            username: credential.username.clone(),
            login_url: credential.login_url.clone(),
            provenance: credential.provenance.as_str().to_string(),
            has_totp: credential.totp_seed.is_some(),
            owner_agent_id: credential.owner_agent_id.as_ref().map(|id| id.to_string()),
            created_at: credential.created_at,
        }
    }
}

/// The vault service.
pub struct Vault {
    store: Arc<dyn CredentialStore>,
    /// One Computer manager per tenant: a fill types into the
    /// computer of the Workspace that owns the Credential.
    computers: Arc<ComputerManagers>,
    /// One Tenant Data Key per Workspace. The vault never holds
    /// a key that is not a named Workspace's. The daemon gives the vault
    /// and the Google broker the same holder.
    keys: Arc<TenantKeys>,
}

impl Vault {
    pub fn new(
        store: Arc<dyn CredentialStore>,
        computers: Arc<ComputerManagers>,
        keys: Arc<TenantKeys>,
    ) -> Self {
        Self {
            store,
            computers,
            keys,
        }
    }

    /// Save a login the user already has (the primary path).
    pub async fn add_user_credential(
        &self,
        new: NewCredential,
    ) -> Result<CredentialView, VaultError> {
        if new.username.trim().is_empty() {
            return Err(VaultError::BadDomain(
                "a credential needs a username".to_string(),
            ));
        }
        if new.secret.is_empty() {
            return Err(VaultError::BadDomain(
                "a credential needs a secret".to_string(),
            ));
        }
        let domain = domain::registrable_domain(&new.domain).ok_or_else(|| {
            VaultError::BadDomain(format!("{:?} is not a registrable domain", new.domain))
        })?;
        domain::check_login_url(&domain, &new.login_url)?;
        // The Workspace names the key, so the record is sealed with the
        // key of the tenant that owns it and no other.
        let key = self.keys.of(&new.workspace_id)?;
        let totp_seed = match new.totp_seed.as_deref().map(str::trim) {
            Some(seed) if !seed.is_empty() => {
                totp::check_seed(seed)?;
                Some(key.seal(seed)?)
            }
            _ => None,
        };
        let credential = Credential {
            id: CredentialId::generate(),
            workspace_id: new.workspace_id,
            domain,
            username: new.username.trim().to_string(),
            login_url: new.login_url.trim().to_string(),
            secret: key.seal(&new.secret)?,
            totp_seed,
            recipe: String::new(),
            owner_agent_id: None,
            provenance: CredentialProvenance::UserSupplied,
            created_run_id: None,
            created_at: now_ms(),
        };
        self.store.create(&credential).await?;
        Ok(CredentialView::from(&credential))
    }

    /// Mint a credential for an Agent (`vault__create`). The secret is
    /// unique and random; nothing returns it.
    pub async fn mint(&self, request: MintRequest) -> Result<CredentialView, VaultError> {
        let domain = domain::registrable_domain(&request.domain).ok_or_else(|| {
            VaultError::BadDomain(format!("{:?} is not a registrable domain", request.domain))
        })?;
        domain::check_login_url(&domain, &request.login_url)?;
        if request.username.trim().is_empty() {
            return Err(VaultError::BadDomain(
                "a credential needs a username".to_string(),
            ));
        }
        let (secret, applied) = recipe::mint_for(request.rules.as_deref())?;
        let key = self.keys.of(&request.workspace_id)?;
        let credential = Credential {
            id: CredentialId::generate(),
            workspace_id: request.workspace_id,
            domain,
            username: request.username.trim().to_string(),
            login_url: request.login_url.trim().to_string(),
            secret: key.seal(&secret)?,
            totp_seed: None,
            recipe: applied,
            owner_agent_id: Some(request.agent_id),
            provenance: CredentialProvenance::AgentMinted,
            created_run_id: Some(request.run_id),
            created_at: now_ms(),
        };
        self.store.create(&credential).await?;
        Ok(CredentialView::from(&credential))
    }

    pub async fn list(
        &self,
        workspace_id: &WorkspaceId,
        domain: Option<&str>,
    ) -> Result<Vec<CredentialView>, VaultError> {
        let domain = domain
            .map(str::trim)
            .filter(|domain| !domain.is_empty())
            .and_then(domain::registrable_domain);
        Ok(self
            .store
            .list(workspace_id, domain.as_deref())
            .await?
            .iter()
            .map(CredentialView::from)
            .collect())
    }

    pub async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CredentialId,
    ) -> Result<Credential, VaultError> {
        self.store
            .get(workspace_id, id)
            .await?
            .ok_or_else(|| VaultError::NotFound(id.to_string()))
    }

    pub async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &CredentialId,
    ) -> Result<(), VaultError> {
        if !self
            .store
            .delete_and_revoke(workspace_id, id, now_ms())
            .await?
        {
            return Err(VaultError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Hand one Agent's minted credentials to the user, on archival.
    pub async fn release_owner(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u64, VaultError> {
        Ok(self.store.release_owner(workspace_id, agent_id).await?)
    }

    /// Open the record's address and fill its credential into the
    /// verified fields of the page (`vault__fill`). The domain check of
    /// the record runs again here: a record with an address that is not
    /// `https` on its own registrable domain fills nothing.
    pub async fn fill(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        id: &CredentialId,
    ) -> Result<(), VaultError> {
        let credential = self.get(workspace_id, id).await?;
        check_record(&credential)?;
        let secret = self.keys.of(workspace_id)?.open(&credential.secret)?;
        fill::open_and_fill(
            &self.computers.get(workspace_id),
            agent_id,
            &credential.domain,
            &credential.login_url,
            &credential.username,
            &secret,
        )
        .await
    }

    /// Fill the record's current one-time code into the verified field
    /// of the page the daemon's tab shows (`vault__totp_fill`).
    pub async fn totp_fill(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        id: &CredentialId,
    ) -> Result<(), VaultError> {
        let credential = self.get(workspace_id, id).await?;
        check_record(&credential)?;
        let seed = credential.totp_seed.as_ref().ok_or_else(|| {
            VaultError::BadTotpSeed(format!(
                "the credential for {} carries no one-time code seed",
                credential.domain
            ))
        })?;
        let code = totp::current_code(&self.keys.of(workspace_id)?.open(seed)?)?;
        fill::fill_code(
            &self.computers.get(workspace_id),
            agent_id,
            &credential.domain,
            &code,
        )
        .await
    }

    /// The Credential record a vault tool call names, as the approval
    /// card and the domain rule read it.
    async fn describe_action(
        &self,
        workspace_id: &WorkspaceId,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<CredentialAction, VaultError> {
        let action = match manifest::tool_of(tool_name) {
            Some(VaultTool::Create) => {
                let domain = arguments["domain"].as_str().unwrap_or_default();
                let login_url = arguments["login_url"].as_str().unwrap_or_default();
                let domain = domain::registrable_domain(domain).ok_or_else(|| {
                    VaultError::BadDomain(format!("{domain:?} is not a registrable domain"))
                })?;
                // The card must state the address that will be opened,
                // so the record's binding is checked before it exists.
                domain::check_login_url(&domain, login_url)?;
                CredentialAction {
                    action: CredentialActionKind::Create,
                    domain,
                    username: arguments["username"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    login_url: login_url.to_string(),
                }
            }
            Some(kind @ (VaultTool::Fill | VaultTool::TotpFill)) => {
                let id =
                    CredentialId::from(arguments["id"].as_str().unwrap_or_default().to_string());
                let credential = self.get(workspace_id, &id).await?;
                CredentialAction {
                    action: if kind == VaultTool::Fill {
                        CredentialActionKind::Fill
                    } else {
                        CredentialActionKind::TotpFill
                    },
                    domain: credential.domain,
                    username: credential.username,
                    login_url: credential.login_url,
                }
            }
            _ => {
                return Err(VaultError::NotFound(format!(
                    "{tool_name} needs no credential request"
                )));
            }
        };
        Ok(action)
    }
}

#[async_trait]
impl CredentialDirectory for Vault {
    async fn describe(
        &self,
        workspace_id: &WorkspaceId,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<CredentialAction, ToolResult> {
        self.describe_action(workspace_id, tool_name, arguments)
            .await
            .map_err(|error| ToolResult::error(error.code(), error.to_string()))
    }
}

/// The daemon-side execution of the five vault tools. Every result is
/// a handle or an outcome; none of them carries a secret.
#[async_trait]
impl ToolExecutor for Vault {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        let ToolRoute::Vault { tool } = call.route else {
            return ToolResult::error(
                "invalid_request",
                "the vault runtime received a call it does not route",
            );
        };
        match self.run(tool, &call).await {
            Ok(result) => result,
            Err(error) => ToolResult::error(error.code(), error.to_string()),
        }
    }
}

impl Vault {
    async fn run(&self, tool: VaultTool, call: &AuthorizedCall) -> Result<ToolResult, VaultError> {
        let id = || {
            CredentialId::from(
                call.arguments["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        };
        match tool {
            VaultTool::List => {
                let items = self
                    .list(&call.workspace_id, call.arguments["domain"].as_str())
                    .await?;
                Ok(ToolResult::success(
                    serde_json::to_string(&items).expect("credential views serialize"),
                ))
            }
            VaultTool::Create => {
                let view = self
                    .mint(MintRequest {
                        workspace_id: call.workspace_id.clone(),
                        agent_id: call.agent_id.clone(),
                        run_id: call.run_id.clone(),
                        domain: string_argument(call, "domain"),
                        username: string_argument(call, "username"),
                        login_url: string_argument(call, "login_url"),
                        rules: call.arguments["rules"].as_str().map(str::to_string),
                    })
                    .await?;
                Ok(ToolResult::success(format!(
                    "Saved a new login for {}. id: {}",
                    view.domain, view.id
                ))
                .with_metadata(audit_fact(&view)))
            }
            VaultTool::Fill => {
                let credential = self.get(&call.workspace_id, &id()).await?;
                let filled = self
                    .fill(&call.workspace_id, &call.agent_id, &credential.id)
                    .await;
                Ok(fill_result(
                    &credential,
                    filled,
                    format!(
                        "Opened {} in Pagis's own tab and filled the saved login for {} into \
                         the username and password fields of the page. Submit the form to \
                         sign in.",
                        credential.login_url, credential.username
                    ),
                ))
            }
            VaultTool::TotpFill => {
                let credential = self.get(&call.workspace_id, &id()).await?;
                let filled = self
                    .totp_fill(&call.workspace_id, &call.agent_id, &credential.id)
                    .await;
                Ok(fill_result(
                    &credential,
                    filled,
                    "Filled the current one-time code into the code field of the page in \
                     Pagis's own tab. Submit the form to continue."
                        .to_string(),
                ))
            }
            VaultTool::Delete => {
                let credential = self.get(&call.workspace_id, &id()).await?;
                self.delete(&call.workspace_id, &credential.id).await?;
                Ok(ToolResult::success(format!(
                    "Deleted the saved login for {} on {}.",
                    credential.username, credential.domain
                )))
            }
        }
    }
}

/// The check of the record before each fill: a record whose address is
/// not `https` on its own registrable domain fills nothing.
fn check_record(credential: &Credential) -> Result<(), VaultError> {
    domain::check_login_url(&credential.domain, &credential.login_url)
        .map_err(|error| VaultError::FillFailed(error.to_string()))
}

/// The result of one fill: `filled` with the text the Agent reads, or
/// `failed` with the reason. The audit fact records the outcome either
/// way.
fn fill_result(
    credential: &Credential,
    filled: Result<(), VaultError>,
    filled_text: String,
) -> ToolResult {
    let mut fact = audit_fact(&CredentialView::from(credential));
    match filled {
        Ok(()) => {
            fact["fill"] = "filled".into();
            ToolResult::success(filled_text).with_metadata(fact)
        }
        Err(error) => {
            fact["fill"] = "failed".into();
            fact["reason"] = error.reason().into();
            ToolResult::error(error.code(), error.to_string()).with_metadata(fact)
        }
    }
}

fn string_argument(call: &AuthorizedCall, field: &str) -> String {
    call.arguments[field]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The audit fields one vault fact carries. Never the secret,
/// never a frame; the broker merges these into its `tool.completed`
/// fact beside the run, the grant revision and the request.
fn audit_fact(view: &CredentialView) -> serde_json::Value {
    serde_json::json!({
        "credential_id": view.id,
        "domain": view.domain,
        "login_url": view.login_url,
    })
}
