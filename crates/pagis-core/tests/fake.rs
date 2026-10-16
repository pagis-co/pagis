use futures::{FutureExt, StreamExt};
use pagis_core::{
    fake::*,
    knowledge::{CatalogueProvider, SourceChange, SourceKey, SourceOperation, SyncConfig},
    reflection_filter::*,
    *,
};
use serde_json::json;

fn owner() -> WorkspaceId {
    "workspace".to_string().into()
}
fn other() -> WorkspaceId {
    "other".to_string().into()
}

#[tokio::test]
async fn agents_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Agent = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "name": "test", "job": "record", "description": "record", "personality": "record", "model_alias": "record", "avatar": {"sprite": "pixie", "preset": "mint", "colors": {}, "accessories": {}}, "status": "active", "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.agents.create(&row).await.unwrap();
    assert!(
        stores
            .agents
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .agents
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn channels_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Channel = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "kind": "dm", "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.channels.create(&row).await.unwrap();
    assert!(
        stores
            .channels
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .channels
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn messages_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Message = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "channel_id": "channel", "author": {"kind": "agent", "agent_id": "agent"}, "status": "complete", "blocks": [], "text_content": "hello", "created_at": 0}"#).unwrap();
    stores.messages.insert(&row).await.unwrap();
    assert!(
        stores
            .messages
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .messages
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn runs_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Run = serde_json::from_str(r#"{"id": "record", "title": "record", "workspace_id": "workspace", "agent_id": "agent", "trigger_kind": "schedule", "hop_count": 0, "state": "queued", "created_at": 0}"#).unwrap();
    stores.runs.create(&row).await.unwrap();
    assert!(stores.runs.get(&other(), &row.id).await.unwrap().is_none());
    assert_eq!(
        stores
            .runs
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn requests_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Request = serde_json::from_str(r#"{"decided_by": null, "id": "record", "workspace_id": "workspace", "agent_id": "agent", "kind": "tool_approval", "payload": {}, "state": "pending", "created_at": 0}"#).unwrap();
    stores.requests.create(&row).await.unwrap();
    assert!(
        stores
            .requests
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .requests
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn grants_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Grant = serde_json::from_str(r#"{"created_by": {"kind":"system"}, "id": "record", "workspace_id": "workspace", "agent_id": "agent", "resource_kind": "record", "scope": {}, "revision": 1, "created_at": 0}"#).unwrap();
    stores.grants.create(&row).await.unwrap();
    assert!(
        stores
            .grants
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .grants
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn connections_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Connection = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "provider": "test", "alias": "default", "display_name": "record", "status": "record", "authorized_capabilities": [], "config": {}, "created_at": 0}"#).unwrap();
    stores.connections.create(&row).await.unwrap();
    assert!(
        stores
            .connections
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .connections
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn credentials_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Credential = serde_json::from_str(r#"{"created_by": {"kind":"system"}, "id": "record", "workspace_id": "workspace", "domain": "record", "username": "record", "login_url": "record", "secret": [1, 2, 3], "recipe": "record", "provenance": "user_supplied", "created_at": 0}"#).unwrap();
    stores.credentials.create(&row).await.unwrap();
    assert!(
        stores
            .credentials
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .credentials
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn files_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: File = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "kind": "other", "mime": "text/plain", "size_bytes": 0, "sha256": "record", "storage_key": "record", "created_at": 0}"#).unwrap();
    stores.files.insert(&row).await.unwrap();
    assert!(stores.files.get(&other(), &row.id).await.unwrap().is_none());
    assert_eq!(
        stores
            .files
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn phone_numbers_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: PhoneNumber = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "connection_id": "record", "e164": "record", "provider_number_id": "record", "status": "active", "outgoing_cap": 0, "sends_today": 0, "messaging_readiness": {"state": "unknown"}, "relay_state": {"state": "absent"}, "relay_consent": false, "created_at": 0}"#).unwrap();
    stores.phone_numbers.create(&row).await.unwrap();
    assert!(
        stores
            .phone_numbers
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .phone_numbers
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn agent_mailboxes_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: AgentMailbox = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "agent_id": "agent", "connection_id": "record", "address": "agent@example.com", "state": "active", "outgoing_cap": 0, "allow_rules": [], "sends_today": 0, "created_at": 0}"#).unwrap();
    stores.agent_mailboxes.create(&row).await.unwrap();
    assert!(
        stores
            .agent_mailboxes
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

fn agent_mailbox() -> AgentMailbox {
    AgentMailbox {
        id: "mailbox".to_string().into(),
        workspace_id: owner(),
        agent_id: "agent".to_string().into(),
        connection_id: "connection".to_string().into(),
        address: "agent@example.com".into(),
        state: AgentMailboxState::Active,
        reason: None,
        outgoing_cap: 20,
        allow_rules: Vec::new(),
        cursor: Some(MailboxCursor {
            folder: "INBOX".into(),
            uid_validity: 7,
            last_uid: 9,
        }),
        sends_day: Some(0),
        sends_today: 3,
        created_at: 0,
        deleted_at: None,
    }
}

#[tokio::test]
async fn agent_mailbox_addresses_stay_in_the_ledger_after_delete() {
    let stores = FakeDatabase::default().stores();
    let row = agent_mailbox();
    stores.agent_mailboxes.create(&row).await.unwrap();
    assert!(
        stores
            .agent_mailboxes
            .delete(&owner(), &row.id, 10)
            .await
            .unwrap()
    );
    assert!(
        stores
            .agent_mailboxes
            .address_taken("AGENT@EXAMPLE.COM")
            .await
            .unwrap()
    );
    let same_address = AgentMailbox {
        id: "another-mailbox".to_string().into(),
        workspace_id: other(),
        agent_id: "another-agent".to_string().into(),
        connection_id: "another-provider".to_string().into(),
        ..row.clone()
    };
    assert!(matches!(
        stores.agent_mailboxes.create(&same_address).await,
        Err(StoreError::Conflict(_))
    ));
    let new_address = AgentMailbox {
        id: "new-mailbox".to_string().into(),
        address: "new@example.com".into(),
        ..row
    };
    stores.agent_mailboxes.create(&new_address).await.unwrap();
    assert_eq!(
        stores
            .agent_mailboxes
            .for_agent(&owner(), &new_address.agent_id)
            .await
            .unwrap(),
        Some(new_address)
    );
}

#[tokio::test]
async fn agent_mailbox_creation_refuses_a_deleted_address_in_another_workspace() {
    let stores = FakeDatabase::default().stores();
    let row = agent_mailbox();
    stores.agent_mailboxes.create(&row).await.unwrap();
    stores
        .agent_mailboxes
        .delete(&owner(), &row.id, 10)
        .await
        .unwrap();
    let another = AgentMailbox {
        id: "another-mailbox".to_string().into(),
        workspace_id: other(),
        agent_id: "another-agent".to_string().into(),
        ..row
    };
    assert!(matches!(
        stores.agent_mailboxes.create(&another).await,
        Err(StoreError::Conflict(_))
    ));
}

#[tokio::test]
async fn agent_mailbox_allow_rules_need_a_live_mailbox_in_the_named_workspace() {
    let stores = FakeDatabase::default().stores();
    let row = agent_mailbox();
    stores.agent_mailboxes.create(&row).await.unwrap();
    let rules = vec![
        "example.org".into(),
        "example.net".into(),
        "example.org".into(),
    ];
    for (workspace, id) in [
        (other(), row.id.clone()),
        (owner(), "missing".to_string().into()),
    ] {
        assert!(matches!(
            stores
                .agent_mailboxes
                .add_allow_rules(&workspace, &id, &rules)
                .await,
            Err(StoreError::Conflict(_))
        ));
    }
    assert_eq!(
        stores.agent_mailboxes.get(&owner(), &row.id).await.unwrap(),
        Some(row.clone())
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .add_allow_rules(&owner(), &row.id, &rules)
            .await
            .unwrap(),
        ["example.org", "example.net"]
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .add_allow_rules(
                &owner(),
                &row.id,
                &["example.net".into(), "example.edu".into()]
            )
            .await
            .unwrap(),
        ["example.org", "example.net", "example.edu"]
    );
    stores
        .agent_mailboxes
        .delete(&owner(), &row.id, 10)
        .await
        .unwrap();
    assert!(matches!(
        stores
            .agent_mailboxes
            .add_allow_rules(&owner(), &row.id, &rules)
            .await,
        Err(StoreError::Conflict(_))
    ));
}

#[tokio::test]
async fn deleted_agent_mailboxes_leave_the_live_lists() {
    let stores = FakeDatabase::default().stores();
    let row = agent_mailbox();
    let live = AgentMailbox {
        id: "new-mailbox".to_string().into(),
        agent_id: "another-agent".to_string().into(),
        address: "new@example.com".into(),
        ..row.clone()
    };
    stores.agent_mailboxes.create(&live).await.unwrap();
    stores.agent_mailboxes.create(&row).await.unwrap();
    assert_eq!(
        stores.agent_mailboxes.list(&owner()).await.unwrap(),
        [row.clone(), live.clone()]
    );
    assert!(
        !stores
            .agent_mailboxes
            .delete(&other(), &row.id, 10)
            .await
            .unwrap()
    );
    stores
        .agent_mailboxes
        .delete(&owner(), &row.id, 10)
        .await
        .unwrap();
    assert_eq!(stores.agent_mailboxes.list(&owner()).await.unwrap(), [live]);
    assert!(
        stores
            .agent_mailboxes
            .list(&other())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        stores
            .agent_mailboxes
            .for_agent(&owner(), &row.agent_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .count_live_for_connection(&owner(), &row.connection_id)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .count_live_for_connection(&other(), &row.connection_id)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn deleted_agent_mailboxes_keep_their_tombstone() {
    let stores = FakeDatabase::default().stores();
    let row = AgentMailbox {
        state: AgentMailboxState::Unavailable,
        reason: Some("login refused".into()),
        ..agent_mailbox()
    };
    stores.agent_mailboxes.create(&row).await.unwrap();
    stores
        .agent_mailboxes
        .delete(&owner(), &row.id, 10)
        .await
        .unwrap();
    let tombstone = AgentMailbox {
        state: AgentMailboxState::Deleted,
        deleted_at: Some(10),
        reason: None,
        cursor: None,
        ..row.clone()
    };
    assert_eq!(
        stores.agent_mailboxes.get(&owner(), &row.id).await.unwrap(),
        Some(tombstone.clone())
    );
    assert!(
        !stores
            .agent_mailboxes
            .set_state(&owner(), &row.id, AgentMailboxState::Active, None)
            .await
            .unwrap()
    );
    assert!(
        !stores
            .agent_mailboxes
            .set_cursor(&owner(), &row.id, row.cursor.as_ref().unwrap())
            .await
            .unwrap()
    );
    assert!(
        stores
            .agent_mailboxes
            .make_dormant_for_agent(&owner(), &row.agent_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !stores
            .agent_mailboxes
            .drop_reservation(&owner(), &row.id)
            .await
            .unwrap()
    );
    assert!(
        !stores
            .agent_mailboxes
            .delete(&owner(), &row.id, 20)
            .await
            .unwrap()
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .note_send(&owner(), &row.id, 86_400_000)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        stores.agent_mailboxes.get(&owner(), &row.id).await.unwrap(),
        Some(tombstone)
    );
}

#[tokio::test]
async fn agent_mailbox_state_changes_do_not_write_tombstones() {
    let stores = FakeDatabase::default().stores();
    let row = agent_mailbox();
    stores.agent_mailboxes.create(&row).await.unwrap();
    assert!(matches!(
        stores
            .agent_mailboxes
            .set_state(&owner(), &row.id, AgentMailboxState::Deleted, None)
            .await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(
        stores.agent_mailboxes.get(&owner(), &row.id).await.unwrap(),
        Some(row)
    );
}

#[tokio::test]
async fn dormant_agent_mailboxes_clear_the_refusal_reason() {
    let stores = FakeDatabase::default().stores();
    let row = AgentMailbox {
        state: AgentMailboxState::Unavailable,
        reason: Some("login refused".into()),
        ..agent_mailbox()
    };
    stores.agent_mailboxes.create(&row).await.unwrap();
    assert!(
        stores
            .agent_mailboxes
            .make_dormant_for_agent(&other(), &row.agent_id)
            .await
            .unwrap()
            .is_none()
    );
    let dormant = stores
        .agent_mailboxes
        .make_dormant_for_agent(&owner(), &row.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        dormant,
        AgentMailbox {
            state: AgentMailboxState::Dormant,
            reason: None,
            ..row
        }
    );
    assert_eq!(
        stores
            .agent_mailboxes
            .get(&owner(), &dormant.id)
            .await
            .unwrap(),
        Some(dormant)
    );
}

#[tokio::test]
async fn calls_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Call = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "agent_id": "agent", "run_id": "record", "phone_number_id": "record", "direction": "outbound", "remote_e164": "record", "agent_name": "record", "own_e164": "record", "purpose": "record", "tools": [], "tier": "trusted", "state": "dialing", "message_left": false, "transcript": [], "created_at": 0}"#).unwrap();
    stores.calls.insert(&row).await.unwrap();
    assert!(stores.calls.get(&other(), &row.id).await.unwrap().is_none());
    assert_eq!(
        stores
            .calls
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn text_records_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: TextRecord = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "phone_number_id": "record", "counterpart_e164": "record", "direction": "outbound", "tier": "trusted", "body": "record", "segments": 0, "media_file_ids": [], "carrier_message_id": "record", "occurred_at": 0}"#).unwrap();
    stores.text_records.insert(&row).await.unwrap();
    assert!(
        stores
            .text_records
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .text_records
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn coding_sessions_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: CodingSession = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "agent_id": "agent", "harness_id": "record", "harness_version": "record", "place": "host", "directory": "record", "approval_mode": "person", "harness_modes": [], "title": "record", "state": "working", "channel_id": "channel", "root_message_id": "root", "message_id": "message", "run_id": "record", "usage": {}, "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.coding_sessions.insert(&row).await.unwrap();
    assert!(
        stores
            .coding_sessions
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .coding_sessions
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn contributions_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Contribution = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "package_id": "record", "base_version": "record", "latest_at_open": "record", "fork_package_id": "record", "fork_version": "record", "patch": "record", "summary": "record", "status": "open", "created_at": 0, "run_id": "record"}"#).unwrap();
    stores.contributions.create(&row).await.unwrap();
    assert!(
        stores
            .contributions
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .contributions
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}

#[tokio::test]
async fn model_aliases_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: ModelAlias = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "alias": "default", "candidates": [], "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.model_aliases.create(&row).await.unwrap();
    assert!(
        stores
            .model_aliases
            .get_by_alias(&other(), &row.alias)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .model_aliases
            .get_by_alias(&owner(), &row.alias)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn sent_mail_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: SentMail = serde_json::from_str(r#"{"message_id": "message", "workspace_id": "workspace", "agent_id": "agent", "mailbox_id": "record", "run_id": "record", "sent_at": 0}"#).unwrap();
    stores.sent_mail.record(&row).await.unwrap();
    assert!(
        stores
            .sent_mail
            .get(&other(), &row.message_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .sent_mail
            .get(&owner(), &row.message_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn software_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: SoftwarePackage = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "name": "test", "author_agent_id": "agent", "description": "record", "keywords": [], "latest_version": "record", "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.software.create_package(&row).await.unwrap();
    assert!(
        stores
            .software
            .get_by_name(&other(), &row.name)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .software
            .get_by_name(&owner(), &row.name)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn workspaces_belong_to_the_named_person() {
    let stores = pagis_core::fake::stores();
    let row = Workspace {
        kind: pagis_core::WorkspaceKind::Private,
        monthly_spend_cap_usd: None,
        id: owner(),
        user_id: Some("person".to_string().into()),
        name: "Desk".to_string(),
        timezone: "UTC".to_string(),
        created_at: 0,
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    stores.workspaces.create(&row).await.unwrap();
    assert!(stores.workspaces.get(&other()).await.unwrap().is_none());
    assert!(
        stores
            .workspaces
            .for_user(&"other".to_string().into())
            .await
            .unwrap()
            .is_none()
    );
    assert!(stores.workspaces.get(&owner()).await.unwrap().is_some());
}

#[tokio::test]
async fn onboarding_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    stores
        .onboarding
        .set_model_verification(
            &owner(),
            &OnboardingModelVerification {
                provider: "test".to_string(),
                available: 1,
                proof: "yes".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(
        stores
            .onboarding
            .model_verifications(&other())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .onboarding
            .model_verifications(&owner())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn retention_policies_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let kind = FileKind::ALL[0];
    stores
        .retention_policies
        .set(&owner(), kind, Some(1), 0)
        .await
        .unwrap();
    assert!(
        stores
            .retention_policies
            .list(&other())
            .await
            .unwrap()
            .iter()
            .all(|row| row.retain_days.is_none())
    );
    assert_eq!(
        stores.retention_policies.list(&owner()).await.unwrap()[0].retain_days,
        Some(1)
    );
}

#[tokio::test]
async fn keypad_failures_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    stores
        .keypad_failures
        .record_failure(&owner(), 0)
        .await
        .unwrap();
    assert_eq!(
        stores.keypad_failures.get(&other()).await.unwrap(),
        KeypadFailures::default()
    );
    assert_eq!(
        stores
            .keypad_failures
            .get(&owner())
            .await
            .unwrap()
            .failed_attempts,
        1
    );
}

#[tokio::test]
async fn hosts_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = stores
        .hosts
        .register(&owner(), "host", "linux", &[], 0)
        .await
        .unwrap();
    assert!(stores.hosts.get(&other(), &row.id).await.unwrap().is_none());
    assert!(stores.hosts.list(&other()).await.unwrap().is_empty());
    assert!(stores.hosts.get(&owner(), &row.id).await.unwrap().is_some());
}

#[tokio::test]
async fn briefs_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let agent: AgentId = "agent".to_string().into();
    let channel = "channel".to_string().into();
    let cursor = BriefCursor {
        initialized: true,
        memory_revision: Some("revision".to_string()),
        shown_paths: vec!["shared/page.md".to_string()],
    };
    stores
        .briefs
        .save(&owner(), &agent, &channel, None, &cursor)
        .await
        .unwrap();
    assert_eq!(
        stores
            .briefs
            .load(&other(), &agent, &channel, None)
            .await
            .unwrap(),
        BriefCursor::default()
    );
    assert_eq!(
        stores
            .briefs
            .load(&owner(), &agent, &channel, None)
            .await
            .unwrap(),
        cursor
    );
}

#[tokio::test]
async fn continuations_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = ContinuationCheckpoint {
        key: ContinuationKey {
            workspace_id: owner(),
            agent_id: "agent".to_string().into(),
            channel_id: "channel".to_string().into(),
            root_message_id: None,
        },
        revision: 1,
        after_exclusive: None,
        through_inclusive: "message".to_string().into(),
        state: ContinuationState::default(),
        source_message_ids: vec![],
        exposures: vec![],
        created_at: 0,
    };
    assert!(stores.continuations.replace(&row, None).await.unwrap());
    let mut key = row.key.clone();
    key.workspace_id = other();
    assert!(stores.continuations.get(&key).await.unwrap().is_none());
    assert_eq!(stores.continuations.get(&row.key).await.unwrap(), Some(row));
}

#[tokio::test]
async fn event_log_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = stores.events.append(new_event()).await.unwrap();
    assert!(
        stores
            .events
            .list_after(Some(&other()), 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .events
            .list_after(Some(&owner()), 0, 10)
            .await
            .unwrap()[0]
            .id,
        row.id
    );
}

fn new_event() -> NewEvent {
    NewEvent {
        workspace_id: owner(),
        event_type: "test".to_string(),
        agent_id: None,
        run_id: None,
        channel_id: None,
        payload: json!({}),
    }
}

#[tokio::test]
async fn event_bus_filters_replay_and_live_events_by_workspace() {
    let bus = FakeEventBus::default();
    let first = bus.publish(new_event()).await.unwrap();
    let mut events = bus
        .subscribe(EventScope::workspace(&other()), Some(0))
        .await;
    assert!(events.next().now_or_never().is_none());
    let mut foreign = new_event();
    foreign.workspace_id = other();
    let second = bus.publish(foreign).await.unwrap();
    assert_eq!(events.next().await.unwrap().id, second.id);
    let mut replay = bus
        .subscribe(EventScope::workspace(&owner()), Some(0))
        .await;
    assert_eq!(replay.next().await.unwrap().id, first.id);
    assert!(replay.next().now_or_never().is_none());
}

#[test]
fn forget_keys_have_separate_workspace_identities() {
    let keys = FakeForgetKeys::default();
    let source = SourceKey {
        workspace_id: owner(),
        connection_id: "connection".to_string().into(),
        resource: "mail".to_string(),
    };
    let own = keys
        .suppression_key(&owner())
        .unwrap()
        .identity(&source, "message");
    let foreign = keys
        .suppression_key(&other())
        .unwrap()
        .identity(&source, "message");
    assert_ne!(own, foreign);
    assert_eq!(
        own,
        keys.suppression_key(&owner())
            .unwrap()
            .identity(&source, "message")
    );
}

#[tokio::test]
async fn forget_operations_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let keys = FakeForgetKeys::default();
    let target = ForgetTarget::Account {
        connection_id: "connection".to_string().into(),
    };
    let preview = stores.forget.preview(&owner(), &target).await.unwrap();
    let op = stores
        .forget
        .begin(&owner(), &target, &preview.revision, 0, &keys)
        .await
        .unwrap();
    assert!(
        stores
            .forget
            .operation(&other(), &op.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(stores.forget.operations(&other()).await.unwrap().is_empty());
    assert!(
        stores
            .forget
            .raw_connection_allowed(&other(), &op.connection_id)
            .await
            .unwrap()
    );
    assert!(
        !stores
            .forget
            .raw_connection_allowed(&owner(), &op.connection_id)
            .await
            .unwrap()
    );
    assert!(
        stores
            .forget
            .operation(&owner(), &op.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn schedules_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Schedule = serde_json::from_str(r#"{"created_by": {"kind":"system"}, "id": "record", "workspace_id": "workspace", "agent_id": "agent", "name": "test", "instruction": "record", "channel_id": "channel", "kind": "one_shot", "timezone": "record", "scheduled_at": 0, "state": "active", "revision": 1, "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.schedules.create(&row).await.unwrap();
    assert!(
        stores
            .schedules
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .schedules
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn subscriptions_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: EventSubscription = serde_json::from_str(r#"{"created_by": {"kind":"system"}, "id": "record", "workspace_id": "workspace", "agent_id": "agent", "source": {"kind": "connection", "connection_id": "connection"}, "event_kind": "record", "source_version": "record", "name": "test", "instruction": "record", "channel_id": "channel", "filter": {}, "state": "active", "revision": 1, "failures": 0, "next_attempt_at": 0, "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.subscriptions.create(&row).await.unwrap();
    assert!(
        stores
            .subscriptions
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .subscriptions
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn plugins_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: Plugin = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "name": "test", "source": {"kind": "upload"}, "installed_commit": "record", "manifest_version": "record", "state": "enabled", "created_at": 0, "updated_at": 0}"#).unwrap();
    stores.plugins.create(&row, &[]).await.unwrap();
    assert!(
        stores
            .plugins
            .get(&other(), &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .plugins
            .get(&owner(), &row.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn participants_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: ChannelParticipant = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "channel_id": "channel", "kind": "agent", "joined_at": 0}"#).unwrap();
    stores.participants.create(&row).await.unwrap();
    assert!(
        stores
            .participants
            .list_for_channel(&other(), &row.channel_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .participants
            .list_for_channel(&owner(), &row.channel_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn push_subscriptions_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = PushSubscription {
        id: "push".to_string().into(),
        workspace_id: owner(),
        session_id: "session".to_string().into(),
        endpoint: "endpoint".into(),
        p256dh: "key".into(),
        auth: "auth".into(),
        created_at: 0,
        last_sent_at: None,
    };
    stores.push_subscriptions.upsert(&row).await.unwrap();
    assert!(
        stores
            .push_subscriptions
            .list(&other())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .push_subscriptions
            .list(&owner())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn model_request_captures_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = ModelRequestCapture {
        id: "capture".to_string().into(),
        workspace_id: owner(),
        run_id: "run".to_string().into(),
        phase: "reply".into(),
        phase_request: 0,
        request: json!({}),
        answer: json!({}),
        created_at: 0,
    };
    stores.model_request_captures.record(&row).await.unwrap();
    assert!(
        stores
            .model_request_captures
            .list_for_run(&other(), &row.run_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .model_request_captures
            .list_for_run(&owner(), &row.run_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn trust_list_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row: TrustEntry = serde_json::from_str(r#"{"id": "record", "workspace_id": "workspace", "subject": "number", "value": "record", "tier": "trusted", "label": "record", "created_at": 0}"#).unwrap();
    stores.trust_list.upsert(&row).await.unwrap();
    assert!(stores.trust_list.list(&other()).await.unwrap().is_empty());
    assert_eq!(stores.trust_list.list(&owner()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn users_read_only_the_named_person_or_org() {
    let stores = pagis_core::fake::stores();
    let row = User {
        id: "person".to_string().into(),
        org_id: "org".to_string().into(),
        email: None,
        name: None,
        password_hash: None,
        role: UserRole::Member,
        disabled_at: None,
        last_signed_in_at: None,
        created_at: 0,
        updated_at: 0,
    };
    stores.users.create(&row).await.unwrap();
    assert!(
        stores
            .users
            .list_by_org(&"other".to_string().into())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores.users.list_by_org(&row.org_id).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn sessions_read_only_the_named_person_or_org() {
    let stores = pagis_core::fake::stores();
    let row = Session {
        id: "session".to_string().into(),
        user_id: "person".to_string().into(),
        token_hash: "token".into(),
        client_kind: ClientKind::Browser,
        client_name: None,
        created_at: 0,
        last_used_at: 0,
        expires_at: 100,
    };
    stores.sessions.create(&row).await.unwrap();
    assert!(
        stores
            .sessions
            .list_live_for_user(&"other".to_string().into(), 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .sessions
            .list_live_for_user(&row.user_id, 0)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn orgs_are_installation_records() {
    let stores = pagis_core::fake::stores();
    let row = Org {
        id: "org".to_string().into(),
        name: "Org".to_string(),
        google_client_id: None,
        workspace_id: "org-workspace".to_string().into(),
        created_at: 0,
    };
    stores.orgs.create(&row).await.unwrap();
    assert_eq!(stores.orgs.list().await.unwrap()[0].id, row.id);
    assert!(
        stores
            .workspaces
            .get(&row.workspace_id)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn sign_in_links_return_only_the_named_person_once() {
    let stores = pagis_core::fake::stores();
    let row = SignInLink {
        id: "link".to_string().into(),
        user_id: "person".to_string().into(),
        token_hash: "secret".to_string(),
        kind: SignInLinkKind::PublicOrigin,
        created_at: 0,
        expires_at: 100,
        used_at: None,
    };
    stores.sign_in_links.create(&row).await.unwrap();
    assert!(
        stores
            .sign_in_links
            .consume("other", row.kind, 0)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .sign_in_links
            .consume(&row.token_hash, row.kind, 0)
            .await
            .unwrap(),
        Some(row.user_id)
    );
    assert!(
        stores
            .sign_in_links
            .consume(&row.token_hash, row.kind, 0)
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn usage_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = UsageRecord {
        id: "usage".to_string().into(),
        workspace_id: owner(),
        run_id: "run".to_string().into(),
        provider: None,
        model: None,
        input_tokens: 3,
        output_tokens: 4,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: Some(1.0),
        created_at: 1,
    };
    stores.usage.record(&row).await.unwrap();
    let period = UsagePeriod { from: 0, to: 100 };
    assert_eq!(
        stores
            .usage
            .total_for_workspace(&other(), period)
            .await
            .unwrap()
            .calls,
        0
    );
    assert!(
        stores
            .usage
            .runs_for_workspace(&other(), period, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .usage
            .total_for_workspace(&owner(), period)
            .await
            .unwrap()
            .calls,
        1
    );
}
#[tokio::test]
async fn capability_snapshots_bind_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let row = CapabilitySnapshotRecord {
        id: "snapshot".to_string(),
        hash: "hash".to_string(),
        content: json!({}),
        created_at: 0,
    };
    let run = "run".to_string().into();
    stores.capability_snapshots.put(&row).await.unwrap();
    stores
        .capability_snapshots
        .bind_run(&owner(), &run, &row.id)
        .await
        .unwrap();
    assert!(
        stores
            .capability_snapshots
            .for_run(&other(), &run)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .capability_snapshots
            .for_run(&owner(), &run)
            .await
            .unwrap()
            .is_some()
    );
}
#[tokio::test]
async fn pending_evidence_claims_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let agent: AgentId = "agent".to_string().into();
    let row = PendingEvidence {
        workspace_id: owner(),
        agent_id: agent,
        channel_id: "channel".to_string().into(),
        root_message_id: None,
        subject: "subject".to_string(),
        after_exclusive: None,
        through_inclusive: "message".to_string().into(),
        source_message_ids: vec!["message".to_string().into()],
        exposures: vec![],
        reason: "test".to_string(),
        urgency: PendingUrgency::Urgent,
        created_at: 0,
    };
    stores
        .pending_evidence
        .record(row.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(
        stores
            .pending_evidence
            .claim(&other(), &row.agent_id, 1, 100000)
            .await
            .unwrap()
            .is_empty()
    );
    let claims = stores
        .pending_evidence
        .claim(&owner(), &row.agent_id, 1, 100000)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert!(
        stores
            .pending_evidence
            .for_run(&other(), &claims[0].run.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .pending_evidence
            .for_run(&owner(), &claims[0].run.id)
            .await
            .unwrap()
            .is_some()
    );
}
#[tokio::test]
async fn knowledge_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let config = SyncConfig {
        workspace_id: owner(),
        connection_id: "connection".to_string().into(),
        resource: "mail".to_string(),
        agent_id: "agent".to_string().into(),
        required_capability: "read".to_string(),
        enabled: true,
        since: 0,
        filter: ReflectionFilter {
            rules: vec![],
            default: Verdict::Reflect,
        },
    };
    let status = stores.knowledge.configure(config, 0).await.unwrap();
    let key = status.key();
    let mut foreign = key.clone();
    foreign.workspace_id = other();
    stores
        .knowledge
        .acquire(
            &key,
            &pagis_core::knowledge::SourceBatch {
                historical: false,
                arrivals: vec![],
                expected_revision: status.cursor_revision,
                checkpoint: json!({"next":1}),
                caught_up: true,
                changes: vec![SourceChange {
                    id: "item".to_string(),
                    version: "1".to_string(),
                    kind: "mail".to_string(),
                    parent: Some("thread".to_string()),
                    source_at: Some(0),
                    operation: SourceOperation::Upsert,
                    metadata: json!({"subject":"test"}),
                }],
            },
            0,
            &FakeForgetKeys::default(),
        )
        .await
        .unwrap();
    assert!(
        stores
            .knowledge
            .status(&foreign, 0)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .knowledge
            .versions(&foreign, 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        stores
            .knowledge
            .parent_metadata(&foreign, "thread")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores.knowledge.versions(&key, 0, 10).await.unwrap().len(),
        1
    );
}
#[tokio::test]
async fn catalogue_reads_only_the_named_workspace() {
    let catalogue = FakeCatalogueProvider::default();
    let key = SourceKey {
        workspace_id: owner(),
        connection_id: "connection".to_string().into(),
        resource: "mail".to_string(),
    };
    let mut foreign = key.clone();
    foreign.workspace_id = other();
    let value = Catalogue { signals: vec![] };
    let filter = ReflectionFilter {
        rules: vec![],
        default: Verdict::Reflect,
    };
    catalogue.set(&key, value, filter.clone(), vec![PageSignals::default()]);
    assert_eq!(
        catalogue.preview(&foreign, &filter, 0).await.unwrap().total,
        0
    );
    assert_eq!(catalogue.preview(&key, &filter, 0).await.unwrap().total, 1);
}
#[tokio::test]
async fn run_journal_reads_only_the_named_run() {
    let stores = pagis_core::fake::stores();
    let run = "run".to_string().into();
    stores
        .run_journal
        .checkpoint(&run, 1, &json!({"text":"hello"}))
        .await
        .unwrap();
    assert!(
        stores
            .run_journal
            .last(&"other".to_string().into())
            .await
            .unwrap()
            .is_none()
    );
    assert!(stores.run_journal.last(&run).await.unwrap().is_some());
}
#[tokio::test]
async fn secret_store_reads_only_the_named_workspace_secret() {
    let store = MemorySecretStore::default();
    let own = workspace_secret_name(&owner(), "secret");
    let foreign = workspace_secret_name(&other(), "secret");
    store.set(&own, "value").unwrap();
    assert!(store.get(&foreign).unwrap().is_none());
    assert_eq!(store.get(&own).unwrap().as_deref(), Some("value"));
}

#[tokio::test]
async fn conversation_evidence_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let scope = ConversationScope {
        workspace_id: owner(),
        agent_id: "agent".to_string().into(),
        channel_id: "channel".to_string().into(),
        root_message_id: None,
    };
    let row = Message {
        id: "message".to_string().into(),
        workspace_id: owner(),
        channel_id: scope.channel_id.clone(),
        parent_message_id: None,
        author: Author::Person(pagis_core::UserId::generate()),
        run_id: None,
        status: MessageStatus::Complete,
        blocks: vec![],
        text_content: "hello".to_string(),
        pending_id: None,
        created_at: 0,
        completed_at: Some(0),
    };
    stores.messages.insert(&row).await.unwrap();
    let mut foreign = scope.clone();
    foreign.workspace_id = other();
    assert!(
        stores
            .conversation_evidence
            .read_message(&foreign, &row.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .conversation_evidence
            .search(&foreign, "hello", None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        stores
            .conversation_evidence
            .read_message(&scope, &row.id)
            .await
            .unwrap()
            .is_some()
    );
    stores
        .grants
        .create(&Grant {
            created_by: pagis_core::Author::System,
            id: "grant".to_string().into(),
            workspace_id: owner(),
            agent_id: scope.agent_id.clone(),
            resource_kind: "connection".into(),
            resource_id: Some("connection".into()),
            scope: json!({}),
            revision: 1,
            created_at: 0,
            revoked_at: None,
        })
        .await
        .unwrap();
    let exposure = MemoryExposure {
        grant_id: "grant".to_string().into(),
        revision: 1,
    };
    let reference = stores
        .conversation_evidence
        .retain_tool(&RetainedToolEvidence {
            scope: scope.clone(),
            source_message_id: row.id,
            run_id: "run".to_string().into(),
            tool_call_id: "call".to_string(),
            tool_name: "read".to_string(),
            content: "text".to_string(),
            complete: true,
            exposure,
            created_at: 0,
        })
        .await
        .unwrap();
    assert!(
        stores
            .conversation_evidence
            .read_tool(&foreign, &reference)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .conversation_evidence
            .read_tool(&scope, &reference)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn plugin_tools_read_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let plugin: Plugin = serde_json::from_value(json!({"id":"plugin", "workspace_id":"workspace", "name":"test", "source":{"kind":"upload"}, "installed_commit":"commit", "manifest_version":"1", "state":"enabled", "created_at":0, "updated_at":0})).unwrap();
    stores.plugins.create(&plugin, &[]).await.unwrap();
    let row = PluginTools {
        plugin_id: plugin.id,
        version: "1".to_string(),
        installed_commit: "commit".to_string(),
        tools: vec![],
        tools_changed: false,
        created_at: 0,
    };
    stores.plugin_tools.put(&row).await.unwrap();
    assert!(
        stores
            .plugin_tools
            .get(&other(), &row.plugin_id, &row.version)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        stores
            .plugin_tools
            .get(&owner(), &row.plugin_id, &row.version)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn page_index_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let update = PageIndexUpdate {
        replace: true,
        head: PageIndexHead {
            revision: "revision".to_string(),
            next_position: 2,
        },
        changed: vec![],
        restamped: vec![],
        removed: vec![],
    };
    stores.memory_pages.apply(&owner(), &update).await.unwrap();
    assert!(stores.memory_pages.head(&other()).await.unwrap().is_none());
    assert!(stores.memory_pages.head(&owner()).await.unwrap().is_some());
}

#[tokio::test]
async fn trigger_cursor_reads_only_the_named_workspace() {
    let stores = pagis_core::fake::stores();
    let connection: ConnectionId = "connection".to_string().into();
    struct Matcher;
    impl EventMatcher for Matcher {
        fn matches(&self, _: &serde_json::Value, _: &serde_json::Value) -> bool {
            true
        }
    }
    let batch = IngestBatch {
        workspace_id: owner(),
        source: EventSource::connection(connection.clone()),
        agent_id: None,
        event_kind: "mail".to_string(),
        cursor: Some("cursor".to_string()),
        events: vec![],
        received_at: 0,
        baseline: true,
    };
    stores
        .triggers
        .ingest(batch, &[], &Matcher, None, None, &FakeForgetKeys::default())
        .await
        .unwrap();
    assert!(
        stores
            .triggers
            .cursor(&other(), &connection, "mail")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stores
            .triggers
            .cursor(&owner(), &connection, "mail")
            .await
            .unwrap()
            .as_deref(),
        Some("cursor")
    );
}

#[tokio::test]
async fn memory_reads_only_the_named_workspace() {
    let store = FakeMemoryStore::default();
    let agent: AgentId = "agent".to_string().into();
    let access = MemoryAccess::agent(agent.clone(), []);
    let path = ScopedPath::parse("shared/page.md").unwrap();
    let mut changes = MemoryChangeset::default();
    changes.writes.insert(path.clone(), "hello".to_string());
    let author = MemoryAuthor {
        name: "Agent".to_string(),
        email: "agent@example.com".to_string(),
    };
    store
        .commit(
            &owner(),
            &agent,
            &access,
            &author,
            &changes,
            None,
            "write",
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        store.read(&other(), &access, &path).await,
        Err(MemoryError::NotFound(_))
    ));
    assert_eq!(
        store.read(&owner(), &access, &path).await.unwrap().content,
        "hello"
    );
}

#[tokio::test]
async fn evidence_files_read_only_the_message_workspace() {
    let stores = pagis_core::fake::stores();
    let scope = ConversationScope {
        workspace_id: owner(),
        agent_id: AgentId::generate(),
        channel_id: ChannelId::generate(),
        root_message_id: None,
    };
    let file = File {
        id: FileId::generate(),
        workspace_id: other(),
        kind: FileKind::Other,
        creator_agent_id: None,
        run_id: None,
        filename: Some("foreign.txt".into()),
        mime: "text/plain".into(),
        size_bytes: 4,
        sha256: "hash".into(),
        storage_key: "file".into(),
        created_at: 0,
    };
    stores.files.insert(&file).await.unwrap();
    let message = Message {
        id: MessageId::generate(),
        workspace_id: owner(),
        channel_id: scope.channel_id.clone(),
        parent_message_id: None,
        author: Author::Person(pagis_core::UserId::generate()),
        run_id: None,
        status: MessageStatus::Complete,
        blocks: vec![Block::file(file.id.as_str(), "file", None, None)],
        text_content: "file".into(),
        pending_id: None,
        created_at: 0,
        completed_at: Some(0),
    };
    stores.messages.insert(&message).await.unwrap();
    let read = stores
        .conversation_evidence
        .read_message(&scope, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.files.len(), 1);
    assert!(!read.files[0].available);
    assert!(read.files[0].filename.is_none());
    let mut file = file;
    file.workspace_id = owner();
    file.id = FileId::generate();
    stores.files.insert(&file).await.unwrap();
    let mut message = message;
    message.id = MessageId::generate();
    message.blocks = vec![Block::file(file.id.as_str(), "file", None, None)];
    stores.messages.insert(&message).await.unwrap();
    let read = stores
        .conversation_evidence
        .read_message(&scope, &message.id)
        .await
        .unwrap()
        .unwrap();
    assert!(read.files[0].available);
    assert_eq!(read.files[0].filename.as_deref(), Some("foreign.txt"));
}

#[tokio::test]
async fn event_bus_keeps_the_sequence_bound_for_live_events() {
    let bus = FakeEventBus::default();
    let mut events = bus
        .subscribe(EventScope::workspace(&owner()), Some(2))
        .await;
    for _ in 0..2 {
        bus.publish(new_event()).await.unwrap();
        assert!(events.next().now_or_never().is_none());
    }
    let third = bus.publish(new_event()).await.unwrap();
    assert_eq!(events.next().await.unwrap().seq, third.seq);
}

#[tokio::test]
async fn message_views_keep_the_thread_and_completion_rules() {
    let stores = pagis_core::fake::stores();
    let agent = AgentId::generate();
    let channel = ChannelId::generate();
    let run = RunId::generate();
    let root = Message {
        id: MessageId::generate(),
        workspace_id: owner(),
        channel_id: channel.clone(),
        parent_message_id: None,
        author: Author::Person(pagis_core::UserId::generate()),
        run_id: None,
        status: MessageStatus::Complete,
        blocks: vec![],
        text_content: "root".into(),
        pending_id: None,
        created_at: 0,
        completed_at: Some(0),
    };
    stores.messages.insert(&root).await.unwrap();
    let reply = Message {
        id: MessageId::generate(),
        parent_message_id: Some(root.id.clone()),
        author: Author::Agent(agent.clone()),
        run_id: Some(run.clone()),
        text_content: "reply".into(),
        ..root.clone()
    };
    stores.messages.insert_stamped(&reply, &[]).await.unwrap();
    let progress = Message {
        id: MessageId::generate(),
        parent_message_id: None,
        blocks: vec![Block::progress(run.as_str(), "Working")],
        ..reply.clone()
    };
    stores
        .messages
        .insert_stamped(&progress, &[])
        .await
        .unwrap();
    let partial = Message {
        id: MessageId::generate(),
        status: MessageStatus::Streaming,
        blocks: vec![],
        ..reply.clone()
    };
    stores.messages.insert_stamped(&partial, &[]).await.unwrap();
    assert!(
        stores
            .messages
            .list_thread(&other(), &root.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .messages
            .list_thread(&owner(), &root.id)
            .await
            .unwrap()
            .iter()
            .map(|m| &m.id)
            .collect::<Vec<_>>(),
        vec![&root.id, &reply.id, &partial.id]
    );
    assert_eq!(
        stores
            .messages
            .last_message(&owner(), &channel)
            .await
            .unwrap()
            .unwrap()
            .id,
        root.id
    );
    assert_eq!(
        stores
            .messages
            .list_agent_elsewhere(&owner(), &agent, &ChannelId::generate(), None, 10)
            .await
            .unwrap()
            .iter()
            .map(|m| &m.id)
            .collect::<Vec<_>>(),
        vec![&reply.id]
    );
    assert_eq!(
        stores
            .messages
            .latest_agent_message_of_run(&owner(), &run)
            .await
            .unwrap()
            .unwrap()
            .id,
        progress.id
    );
}

#[test]
fn file_classes_use_the_retention_wire_names() {
    let names = ["screenshot", "call_recording", "call_transcript", "other"];
    for (kind, name) in FileKind::ALL.into_iter().zip(names) {
        assert_eq!(kind.as_str(), name);
        assert_eq!(FileKind::parse(name), Some(kind));
        assert_eq!(serde_json::to_value(kind).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<FileKind>(serde_json::json!(name)).unwrap(),
            kind
        );
    }
    assert_eq!(FileKind::parse("file"), None);
    assert!(serde_json::from_value::<FileKind>(serde_json::json!("file")).is_err());
}
