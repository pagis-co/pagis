//! Typed fixture builders. Seeds are Rust values, not SQL files.

use pagis_core::{
    Agent, AgentStatus, Artifact, AuthorKind, Block, Channel, ChannelKind, ChannelParticipant,
    Grant, Message, MessageStatus, ParticipantKind, Request, RequestState, Run, RunState,
    TriggerKind, Workspace, now_ms,
};
use pagis_core::{
    AgentId, ArtifactId, ChannelId, GrantId, MessageId, ParticipantId, RequestId, RunId, UserId,
    WorkspaceId,
};

/// A Workspace that belongs to a person, and the Org and the person to
/// go with it. A test that writes the Workspace through a real
/// store needs this one: `workspaces.user_id` names a `users` row.
pub async fn seeded_workspace(pool: &sqlx::SqlitePool) -> Workspace {
    let person = pagis_storage_sqlite::seed_org_and_administrator(pool, "Org", now_ms())
        .await
        .expect("seed the org and its administrator");
    Workspace {
        user_id: person.id,
        ..workspace()
    }
}

/// A Workspace value with a person of its own. Nothing writes that
/// person, so this one is for a test that keeps its records in memory.
pub fn workspace() -> Workspace {
    Workspace {
        id: WorkspaceId::generate(),
        user_id: UserId::generate(),
        name: "Workspace".to_string(),
        timezone: "UTC".to_string(),
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
    }
}

pub fn channel(workspace_id: &WorkspaceId) -> Channel {
    let now = now_ms();
    Channel {
        id: ChannelId::generate(),
        workspace_id: workspace_id.clone(),
        kind: ChannelKind::Dm,
        title: Some("general".to_string()),
        created_at: now,
        updated_at: now,
    }
}

/// A group channel: triggers only on @-mentions.
pub fn group_channel(workspace_id: &WorkspaceId, title: &str) -> Channel {
    Channel {
        kind: ChannelKind::Group,
        title: Some(title.to_string()),
        ..channel(workspace_id)
    }
}

/// A complete user message with a pending_id, ready to insert.
pub fn user_message(workspace_id: &WorkspaceId, channel_id: &ChannelId, text: &str) -> Message {
    let now = now_ms();
    Message {
        id: MessageId::generate(),
        workspace_id: workspace_id.clone(),
        channel_id: channel_id.clone(),
        parent_message_id: None,
        author_kind: AuthorKind::User,
        author_agent_id: None,
        run_id: None,
        status: MessageStatus::Complete,
        blocks: vec![Block::markdown(text)],
        text_content: text.to_string(),
        pending_id: Some(format!("pending-{}", MessageId::generate())),
        created_at: now,
        completed_at: Some(now),
    }
}

/// A queued message-trigger run bound to the channel top level.
pub fn queued_run(workspace_id: &WorkspaceId, agent_id: &AgentId, channel_id: &ChannelId) -> Run {
    Run {
        id: RunId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        channel_id: Some(channel_id.clone()),
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: Some(MessageId::generate().to_string()),
        hop_count: 0,
        origin: None,
        state: RunState::Queued,
        failure_kind: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: now_ms(),
    }
}

/// An agent participant row, making the channel a DM with that agent.
pub fn agent_participant(
    workspace_id: &WorkspaceId,
    channel_id: &ChannelId,
    agent_id: &AgentId,
) -> ChannelParticipant {
    ChannelParticipant {
        id: ParticipantId::generate(),
        workspace_id: workspace_id.clone(),
        channel_id: channel_id.clone(),
        kind: ParticipantKind::Agent,
        agent_id: Some(agent_id.clone()),
        joined_at: now_ms(),
    }
}

/// The user participant row of a channel.
pub fn user_participant(workspace_id: &WorkspaceId, channel_id: &ChannelId) -> ChannelParticipant {
    ChannelParticipant {
        id: ParticipantId::generate(),
        workspace_id: workspace_id.clone(),
        channel_id: channel_id.clone(),
        kind: ParticipantKind::User,
        agent_id: None,
        joined_at: now_ms(),
    }
}

/// A complete agent-authored message, ready to insert.
pub fn agent_message(
    workspace_id: &WorkspaceId,
    channel_id: &ChannelId,
    agent_id: &AgentId,
    text: &str,
) -> Message {
    Message {
        author_kind: AuthorKind::Agent,
        author_agent_id: Some(agent_id.clone()),
        pending_id: None,
        ..user_message(workspace_id, channel_id, text)
    }
}

/// A pending host-action request bound to one run.
pub fn pending_request(
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    run_id: &RunId,
    command: &str,
) -> Request {
    Request {
        id: RequestId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        run_id: Some(run_id.clone()),
        kind: Request::TOOL_ACTION_KIND.to_string(),
        payload: serde_json::json!({
            "tool_name": "host_shell",
            "arguments": {"command": command},
            "body": command,
        }),
        state: RequestState::Pending,
        values: None,
        decided_at: None,
        created_at: now_ms(),
    }
}

/// A pending form request bound to one run, with a required text
/// field and an optional select.
pub fn pending_form_request(
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    run_id: &RunId,
) -> Request {
    Request {
        id: RequestId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        run_id: Some(run_id.clone()),
        kind: Request::FORM_KIND.to_string(),
        payload: serde_json::json!({
            "title": "Book the room",
            "fields": [
                {"key": "who", "label": "Who", "kind": "text", "required": true},
                {"key": "room", "label": "Room", "kind": "select", "options": [
                    {"value": "a", "label": "Room A"},
                    {"value": "b", "label": "Room B"}
                ]}
            ],
            "submit_label": "Book"
        }),
        state: RequestState::Pending,
        values: None,
        decided_at: None,
        created_at: now_ms(),
    }
}

/// A live host grant on one machine, with the given allow rules.
/// A host grant names the Host it reaches, so the caller names one.
pub fn host_grant(
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    host_id: &pagis_core::HostId,
    allow: &[&str],
) -> Grant {
    Grant {
        resource_id: Some(host_id.to_string()),
        ..grant(workspace_id, agent_id, Grant::HOST_KIND, allow)
    }
}

/// A live credential grant with the given allowed domains.
pub fn credential_grant(workspace_id: &WorkspaceId, agent_id: &AgentId, allow: &[&str]) -> Grant {
    grant(workspace_id, agent_id, Grant::CREDENTIAL_KIND, allow)
}

/// A live grant of one resource kind with the given allow rules.
pub fn grant(
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    resource_kind: &str,
    allow: &[&str],
) -> Grant {
    let allow: Vec<String> = allow.iter().map(|r| r.to_string()).collect();
    Grant {
        id: GrantId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        resource_kind: resource_kind.to_string(),
        resource_id: None,
        scope: Grant::allow_scope(&allow),
        revision: 1,
        created_at: now_ms(),
        revoked_at: None,
    }
}

/// An uploaded image artifact with the given content hash.
pub fn artifact(workspace_id: &WorkspaceId, sha256: &str) -> Artifact {
    Artifact {
        id: ArtifactId::generate(),
        workspace_id: workspace_id.clone(),
        creator_agent_id: None,
        run_id: None,
        kind: pagis_core::ArtifactKind::File,
        filename: Some("shot.png".to_string()),
        mime: "image/png".to_string(),
        size_bytes: 3,
        sha256: sha256.to_string(),
        storage_key: format!("{workspace_id}/{sha256}"),
        created_at: now_ms(),
    }
}

/// A Software Package at its first Version.
pub fn software_package(
    workspace_id: &WorkspaceId,
    author_agent_id: &AgentId,
    name: &str,
) -> pagis_core::SoftwarePackage {
    let now = now_ms();
    pagis_core::SoftwarePackage {
        id: pagis_core::SoftwarePackageId::generate(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
        author_agent_id: author_agent_id.clone(),
        description: format!("the {name} package"),
        keywords: vec![name.to_string()],
        latest_version: "v1".to_string(),
        origin_package_id: None,
        origin_version: None,
        created_at: now,
        updated_at: now,
    }
}

/// One published Version of a Software Package.
pub fn software_version(
    package_id: &pagis_core::SoftwarePackageId,
    version: &str,
    run_id: &RunId,
) -> pagis_core::SoftwareVersion {
    pagis_core::SoftwareVersion {
        package_id: package_id.clone(),
        version: version.to_string(),
        notes: format!("{version} notes"),
        commit_id: "0".repeat(40),
        manifest: serde_json::json!({"package": {"name": "weather"}, "tools": []}),
        published_at: now_ms(),
        run_id: run_id.clone(),
    }
}

pub fn agent(workspace_id: &WorkspaceId) -> Agent {
    let now = now_ms();
    Agent {
        id: AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: "Sage".to_string(),
        job: "general assistant".to_string(),
        description: String::new(),
        personality: "warm, direct".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now,
        updated_at: now,
    }
}
