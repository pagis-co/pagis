//! The one Workspace Mailbox record. Mail bodies stay at the mail host.
use crate::{ConnectionId, StoreError, UnixMillis, WorkspaceId};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMailbox {
    pub workspace_id: WorkspaceId,
    pub connection_id: ConnectionId,
    pub address: String,
    pub outgoing_cap: u32,
    pub visibility: crate::MailVisibility,
    pub spam_mode: MailScoringMode,
    pub created_at: UnixMillis,
}

#[async_trait]
pub trait WorkspaceMailboxRecordStore: Send + Sync {
    async fn set_visibility(
        &self,
        workspace: &WorkspaceId,
        visibility: crate::MailVisibility,
    ) -> Result<(), StoreError>;
    /// Record an Agent escalation by the stable Message-ID header or host id.
    async fn escalate(
        &self,
        workspace: &WorkspaceId,
        message: &str,
        agent: &crate::AgentId,
    ) -> Result<(), StoreError>;
    async fn is_escalated(
        &self,
        workspace: &WorkspaceId,
        message: &str,
    ) -> Result<bool, StoreError>;
    async fn set_spam_mode(
        &self,
        workspace: &WorkspaceId,
        mode: MailScoringMode,
    ) -> Result<(), StoreError>;
    /// Reserve one daily send before SMTP. Concurrent sends share the cap.
    async fn reserve_send(&self, workspace: &WorkspaceId, day: i64) -> Result<bool, StoreError>;
    /// Return the allowance when SMTP refuses a send.
    async fn release_send(&self, workspace: &WorkspaceId, day: i64) -> Result<(), StoreError>;
    async fn get(&self, workspace_id: &WorkspaceId)
    -> Result<Option<WorkspaceMailbox>, StoreError>;
    /// Reserve both the Workspace and the address in one write.
    async fn create(&self, mailbox: &WorkspaceMailbox) -> Result<(), StoreError>;
    /// Undo a host create that failed. The address must match the reservation.
    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        address: &str,
    ) -> Result<bool, StoreError>;
    async fn count_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError>;
}

/// Score-only mode records verdicts and leaves messages in their folders.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MailScoringMode {
    ScoreOnly,
    MoveToJunk,
}

impl MailScoringMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScoreOnly => "score_only",
            Self::MoveToJunk => "move_to_junk",
        }
    }
}
impl std::str::FromStr for MailScoringMode {
    type Err = StoreError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "score_only" => Ok(Self::ScoreOnly),
            "move_to_junk" => Ok(Self::MoveToJunk),
            _ => Err(StoreError::Corrupt("unknown mail scoring mode".into())),
        }
    }
}

/// Keep the System's default Grant with the active Chief of Staff.
/// Manual Grants are independent and stay when the designation changes.
/// The caller publishes the returned Grant events through its event sink.
pub async fn reconcile_workspace_mailbox_grants(
    workspace: &WorkspaceId,
    workspaces: &dyn crate::WorkspaceStore,
    agents: &dyn crate::AgentStore,
    mailboxes: &dyn WorkspaceMailboxRecordStore,
    grants: &dyn crate::GrantStore,
) -> Result<Vec<crate::NewEvent>, StoreError> {
    use crate::{AgentStatus, Author, Grant, GrantId, now_ms};
    let mailbox = mailboxes.get(workspace).await?;
    let chief = match workspaces
        .get(workspace)
        .await?
        .and_then(|w| w.chief_of_staff_agent_id)
    {
        Some(id) if mailbox.is_some() => agents
            .get(workspace, &id)
            .await?
            .filter(|agent| agent.status == AgentStatus::Active)
            .map(|agent| agent.id),
        _ => None,
    };
    let mut events = Vec::new();
    let mut held = false;
    for mut grant in grants.list_live(workspace).await? {
        if grant.resource_kind != Grant::WORKSPACE_MAILBOX_KIND
            || grant.created_by != Author::System
            || grant.scope["default"] != true
        {
            continue;
        }
        if chief.as_ref() == Some(&grant.agent_id)
            && grant.resource_id.as_deref() == Some(workspace.as_str())
            && !held
        {
            held = true;
        } else if grants.revoke(workspace, &grant.id, now_ms()).await? {
            grant.revoked_at = Some(now_ms());
            events.push(workspace_mailbox_grant_event(&grant, "grant.revoked"));
        }
    }
    if !held && let Some(agent_id) = chief {
        let grant = Grant {
            created_by: Author::System,
            id: GrantId::generate(),
            workspace_id: workspace.clone(),
            agent_id,
            resource_kind: Grant::WORKSPACE_MAILBOX_KIND.into(),
            resource_id: Some(workspace.to_string()),
            scope: serde_json::json!({"default":true,"allow":[]}),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        };
        grants.create(&grant).await?;
        events.push(workspace_mailbox_grant_event(&grant, "grant.created"));
    }
    Ok(events)
}

fn workspace_mailbox_grant_event(grant: &crate::Grant, kind: &str) -> crate::NewEvent {
    crate::NewEvent {
        workspace_id: grant.workspace_id.clone(),
        event_type: kind.into(),
        agent_id: Some(grant.agent_id.clone()),
        run_id: None,
        channel_id: None,
        payload: serde_json::json!({"grant_id":grant.id,"resource_kind":grant.resource_kind,
            "resource_id":grant.resource_id,"allow":grant.allow_rules(),"revision":grant.revision}),
    }
}
