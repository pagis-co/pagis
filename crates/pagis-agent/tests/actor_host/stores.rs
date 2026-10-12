//! Unused store methods fail if the actor crosses the test boundary.

use super::ActorHost;
use pagis_core::SourceBatch;
use pagis_core::knowledge::SourceKey;
use pagis_core::*;

#[async_trait::async_trait]
impl WorkspaceStore for ActorHost {
    async fn create(&self, _workspace: &Workspace) -> Result<(), StoreError> {
        panic!("unexpected WorkspaceStore::create")
    }
    async fn get(&self, _id: &WorkspaceId) -> Result<Option<Workspace>, StoreError> {
        panic!("unexpected WorkspaceStore::get")
    }
    async fn for_user(&self, _user_id: &UserId) -> Result<Option<Workspace>, StoreError> {
        panic!("unexpected WorkspaceStore::for_user")
    }
    async fn list(&self) -> Result<Vec<Workspace>, StoreError> {
        panic!("unexpected WorkspaceStore::list")
    }
    async fn set_onboarded(&self, _id: &WorkspaceId, _at: UnixMillis) -> Result<(), StoreError> {
        panic!("unexpected WorkspaceStore::set_onboarded")
    }
    async fn set_timezone(&self, _id: &WorkspaceId, _timezone: &str) -> Result<(), StoreError> {
        panic!("unexpected WorkspaceStore::set_timezone")
    }
    async fn set_chief_of_staff(
        &self,
        _id: &WorkspaceId,
        _agent_id: Option<&AgentId>,
    ) -> Result<(), StoreError> {
        panic!("unexpected WorkspaceStore::set_chief_of_staff")
    }
    async fn set_report_schedule(
        &self,
        _id: &WorkspaceId,
        _schedule_id: Option<&ScheduleId>,
    ) -> Result<(), StoreError> {
        panic!("unexpected WorkspaceStore::set_report_schedule")
    }
    async fn set_home_exit(
        &self,
        _id: &WorkspaceId,
        _host_id: Option<&HostId>,
    ) -> Result<bool, StoreError> {
        panic!("unexpected WorkspaceStore::set_home_exit")
    }
}

#[async_trait::async_trait]
impl UserStore for ActorHost {
    async fn create(&self, _user: &User) -> Result<(), StoreError> {
        panic!("unexpected UserStore::create")
    }
    async fn get(&self, _id: &UserId) -> Result<Option<User>, StoreError> {
        panic!("unexpected UserStore::get")
    }
    async fn find_by_email(&self, _email: &str) -> Result<Option<User>, StoreError> {
        panic!("unexpected UserStore::find_by_email")
    }
    async fn list_by_org(&self, _org_id: &OrgId) -> Result<Vec<User>, StoreError> {
        panic!("unexpected UserStore::list_by_org")
    }
    async fn set_name(&self, _id: &UserId, _name: &str, _at: UnixMillis) -> Result<(), StoreError> {
        panic!("unexpected UserStore::set_name")
    }
    async fn set_email_and_password(
        &self,
        _id: &UserId,
        _email: &str,
        _password_hash: &str,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected UserStore::set_email_and_password")
    }
    async fn claim_first_administrator(
        &self,
        _id: &UserId,
        _email: &str,
        _password_hash: &str,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected UserStore::claim_first_administrator")
    }
    async fn set_password_hash(
        &self,
        _id: &UserId,
        _hash: Option<&str>,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected UserStore::set_password_hash")
    }
    async fn set_disabled(
        &self,
        _id: &UserId,
        _disabled_at: Option<UnixMillis>,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected UserStore::set_disabled")
    }
    async fn set_monthly_spend_cap(
        &self,
        _id: &UserId,
        _cap_usd: Option<f64>,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected UserStore::set_monthly_spend_cap")
    }
    async fn record_sign_in(&self, _id: &UserId, _at: UnixMillis) -> Result<(), StoreError> {
        panic!("unexpected UserStore::record_sign_in")
    }
}

#[async_trait::async_trait]
impl AgentStore for ActorHost {
    async fn create(&self, _agent: &Agent) -> Result<(), StoreError> {
        panic!("unexpected AgentStore::create")
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<Agent>, StoreError> {
        assert_eq!(workspace_id, &self.agent.workspace_id);
        assert_eq!(id, &self.agent.id);
        Ok(Some(self.agent.clone()))
    }
    async fn list_by_workspace(
        &self,
        _workspace_id: &WorkspaceId,
    ) -> Result<Vec<Agent>, StoreError> {
        panic!("unexpected AgentStore::list_by_workspace")
    }
    async fn update(&self, _agent: &Agent) -> Result<(), StoreError> {
        panic!("unexpected AgentStore::update")
    }
    async fn update_avatar(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentId,
        _avatar: &pagis_core::AvatarAppearance,
        _updated_at: i64,
    ) -> Result<bool, StoreError> {
        panic!("unexpected AgentStore::update_avatar")
    }
}

#[async_trait::async_trait]
impl ChannelStore for ActorHost {
    async fn create(&self, _channel: &Channel) -> Result<(), StoreError> {
        panic!("unexpected ChannelStore::create")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ChannelId,
    ) -> Result<Option<Channel>, StoreError> {
        panic!("unexpected ChannelStore::get")
    }
    async fn list_by_workspace(
        &self,
        _workspace_id: &WorkspaceId,
    ) -> Result<Vec<Channel>, StoreError> {
        panic!("unexpected ChannelStore::list_by_workspace")
    }
    async fn find_user_dm(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Result<Option<Channel>, StoreError> {
        panic!("unexpected ChannelStore::find_user_dm")
    }
    async fn find_agent_dm(
        &self,
        _workspace_id: &WorkspaceId,
        _a: &AgentId,
        _b: &AgentId,
    ) -> Result<Option<Channel>, StoreError> {
        panic!("unexpected ChannelStore::find_agent_dm")
    }
}

#[async_trait::async_trait]
impl MessageStore for ActorHost {
    async fn insert(&self, _message: &Message) -> Result<SendOutcome, StoreError> {
        panic!("unexpected MessageStore::insert")
    }
    async fn insert_stamped(
        &self,
        message: &Message,
        exposures: &[pagis_core::MemoryExposure],
    ) -> Result<SendOutcome, StoreError> {
        assert!(exposures.is_empty());
        self.messages.lock().unwrap().push(message.clone());
        Ok(SendOutcome::Created(message.clone()))
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &MessageId,
    ) -> Result<Option<Message>, StoreError> {
        panic!("unexpected MessageStore::get")
    }
    async fn is_forgotten(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &MessageId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected MessageStore::is_forgotten")
    }
    async fn set_exposures(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &MessageId,
        _exposures: &[pagis_core::MemoryExposure],
    ) -> Result<(), StoreError> {
        panic!("unexpected MessageStore::set_exposures")
    }
    async fn exposures(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &MessageId,
    ) -> Result<Option<Vec<pagis_core::MemoryExposure>>, StoreError> {
        panic!("unexpected MessageStore::exposures")
    }
    async fn list_top_level(
        &self,
        _workspace_id: &WorkspaceId,
        _channel_id: &ChannelId,
        _before: Option<&MessageId>,
        _limit: u32,
    ) -> Result<Vec<TimelineEntry>, StoreError> {
        panic!("unexpected MessageStore::list_top_level")
    }
    async fn last_message(
        &self,
        _workspace_id: &WorkspaceId,
        _channel_id: &ChannelId,
    ) -> Result<Option<Message>, StoreError> {
        panic!("unexpected MessageStore::last_message")
    }
    async fn list_agent_elsewhere(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _exclude_channel: &ChannelId,
        _before: Option<&MessageId>,
        _limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        panic!("unexpected MessageStore::list_agent_elsewhere")
    }
    async fn list_thread(
        &self,
        _workspace_id: &WorkspaceId,
        _root_id: &MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        panic!("unexpected MessageStore::list_thread")
    }
    async fn latest_agent_message_of_run(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Option<Message>, StoreError> {
        panic!("unexpected MessageStore::latest_agent_message_of_run")
    }
    async fn finalize(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
        status: MessageStatus,
        blocks: &[Block],
        text_content: &str,
        completed_at: UnixMillis,
    ) -> Result<(), StoreError> {
        let mut messages = self.messages.lock().unwrap();
        let message = messages
            .iter_mut()
            .find(|message| &message.id == id && &message.workspace_id == workspace_id)
            .expect("progress message");
        message.status = status;
        message.blocks = blocks.to_vec();
        message.text_content = text_content.to_string();
        message.completed_at = Some(completed_at);
        Ok(())
    }
    async fn fail_streaming(&self, _completed_at: UnixMillis) -> Result<u64, StoreError> {
        panic!("unexpected MessageStore::fail_streaming")
    }
}

#[async_trait::async_trait]
impl RunStore for ActorHost {
    async fn create(&self, _run: &Run) -> Result<(), StoreError> {
        panic!("unexpected RunStore::create")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &RunId,
    ) -> Result<Option<Run>, StoreError> {
        panic!("unexpected RunStore::get")
    }
    async fn update(&self, run: &Run) -> Result<(), StoreError> {
        self.states
            .lock()
            .unwrap()
            .push((run.id.clone(), run.state));
        self.updated.notify_one();
        Ok(())
    }
    async fn list_unfinished(&self) -> Result<Vec<Run>, StoreError> {
        Ok(Vec::new())
    }
    async fn list(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: Option<&AgentId>,
        _channel_id: Option<&ChannelId>,
        _states: &[pagis_core::domain::RunState],
        _before: Option<&RunId>,
        _limit: u32,
    ) -> Result<Vec<Run>, StoreError> {
        panic!("unexpected RunStore::list")
    }
    async fn dismiss(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &RunId,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected RunStore::dismiss")
    }
    async fn record_source_reads(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
        _connection_id: &ConnectionId,
        _reads: &[pagis_core::knowledge::SourceRead],
    ) -> Result<(), StoreError> {
        panic!("unexpected RunStore::record_source_reads")
    }
}

#[async_trait::async_trait]
impl ModelAliasStore for ActorHost {
    async fn create(&self, _model_alias: &ModelAlias) -> Result<(), StoreError> {
        panic!("unexpected ModelAliasStore::create")
    }
    async fn get_by_alias(
        &self,
        _workspace_id: &WorkspaceId,
        _alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError> {
        panic!("unexpected ModelAliasStore::get_by_alias")
    }
    async fn list(&self, _workspace_id: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError> {
        panic!("unexpected ModelAliasStore::list")
    }
    async fn update_candidates(
        &self,
        _workspace_id: &WorkspaceId,
        _alias: &str,
        _candidates: &[String],
        _updated_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ModelAliasStore::update_candidates")
    }
    async fn delete(&self, _workspace_id: &WorkspaceId, _alias: &str) -> Result<bool, StoreError> {
        panic!("unexpected ModelAliasStore::delete")
    }
}

#[async_trait::async_trait]
impl UsageStore for ActorHost {
    async fn record(&self, _usage: &UsageRecord) -> Result<(), StoreError> {
        panic!("unexpected UsageStore::record")
    }
    async fn total_for_workspace(
        &self,
        _workspace_id: &WorkspaceId,
        _period: UsagePeriod,
    ) -> Result<UsageTotal, StoreError> {
        panic!("unexpected UsageStore::total_for_workspace")
    }
    async fn totals_by_workspace(
        &self,
        _period: UsagePeriod,
    ) -> Result<Vec<WorkspaceUsage>, StoreError> {
        panic!("unexpected UsageStore::totals_by_workspace")
    }
    async fn runs_for_workspace(
        &self,
        _workspace_id: &WorkspaceId,
        _period: UsagePeriod,
        _limit: u32,
    ) -> Result<Vec<RunUsage>, StoreError> {
        panic!("unexpected UsageStore::runs_for_workspace")
    }
}

#[async_trait::async_trait]
impl ModelRequestCaptureStore for ActorHost {
    async fn record(&self, _capture: &ModelRequestCapture) -> Result<(), StoreError> {
        panic!("unexpected ModelRequestCaptureStore::record")
    }
    async fn list_for_run(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Vec<ModelRequestCapture>, StoreError> {
        panic!("unexpected ModelRequestCaptureStore::list_for_run")
    }
    async fn delete_before(&self, _cutoff: UnixMillis) -> Result<u64, StoreError> {
        panic!("unexpected ModelRequestCaptureStore::delete_before")
    }
    async fn delete_all(&self) -> Result<u64, StoreError> {
        panic!("unexpected ModelRequestCaptureStore::delete_all")
    }
}

#[async_trait::async_trait]
impl ParticipantStore for ActorHost {
    async fn create(&self, _participant: &ChannelParticipant) -> Result<(), StoreError> {
        panic!("unexpected ParticipantStore::create")
    }
    async fn agents_in_channel(
        &self,
        _workspace_id: &WorkspaceId,
        _channel_id: &ChannelId,
    ) -> Result<Vec<AgentId>, StoreError> {
        panic!("unexpected ParticipantStore::agents_in_channel")
    }
    async fn list_for_channel(
        &self,
        _workspace_id: &WorkspaceId,
        _channel_id: &ChannelId,
    ) -> Result<Vec<ChannelParticipant>, StoreError> {
        panic!("unexpected ParticipantStore::list_for_channel")
    }
}

#[async_trait::async_trait]
impl RequestStore for ActorHost {
    async fn create(&self, _request: &Request) -> Result<(), StoreError> {
        panic!("unexpected RequestStore::create")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &RequestId,
    ) -> Result<Option<Request>, StoreError> {
        panic!("unexpected RequestStore::get")
    }
    async fn list_by_state(
        &self,
        _workspace_id: &WorkspaceId,
        _state: RequestState,
        _kind: Option<&str>,
    ) -> Result<Vec<Request>, StoreError> {
        panic!("unexpected RequestStore::list_by_state")
    }
    async fn decide(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &RequestId,
        _state: RequestState,
        _values: Option<serde_json::Value>,
        _decided_at: UnixMillis,
    ) -> Result<DecideOutcome, StoreError> {
        panic!("unexpected RequestStore::decide")
    }
    async fn expire_pending_for_run(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
        _decided_at: UnixMillis,
    ) -> Result<u64, StoreError> {
        panic!("unexpected RequestStore::expire_pending_for_run")
    }
}

#[async_trait::async_trait]
impl GrantStore for ActorHost {
    async fn create(&self, _grant: &Grant) -> Result<(), StoreError> {
        panic!("unexpected GrantStore::create")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &GrantId,
    ) -> Result<Option<Grant>, StoreError> {
        panic!("unexpected GrantStore::get")
    }
    async fn list_live(&self, _workspace_id: &WorkspaceId) -> Result<Vec<Grant>, StoreError> {
        panic!("unexpected GrantStore::list_live")
    }
    async fn live_grant(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _resource_kind: &str,
    ) -> Result<Option<Grant>, StoreError> {
        panic!("unexpected GrantStore::live_grant")
    }
    async fn list_live_for_agent(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Result<Vec<Grant>, StoreError> {
        panic!("unexpected GrantStore::list_live_for_agent")
    }
    async fn live_for_resource(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _resource_kind: &str,
        _resource_id: &str,
    ) -> Result<Option<Grant>, StoreError> {
        panic!("unexpected GrantStore::live_for_resource")
    }
    async fn set_scope(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &GrantId,
        _scope: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        panic!("unexpected GrantStore::set_scope")
    }
    async fn revoke(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &GrantId,
        _revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected GrantStore::revoke")
    }
}

#[async_trait::async_trait]
impl ConnectionStore for ActorHost {
    async fn create(&self, _connection: &Connection) -> Result<(), StoreError> {
        panic!("unexpected ConnectionStore::create")
    }
    async fn set_status(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
        _status: &str,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ConnectionStore::set_status")
    }
    async fn set_authorization(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
        _capabilities: &[String],
    ) -> Result<bool, StoreError> {
        panic!("unexpected ConnectionStore::set_authorization")
    }
    async fn set_config(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
        _config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ConnectionStore::set_config")
    }
    async fn list(&self, _workspace_id: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        panic!("unexpected ConnectionStore::list")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        panic!("unexpected ConnectionStore::get")
    }
    async fn delete_and_revoke(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
        _revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ConnectionStore::delete_and_revoke")
    }
    async fn set_refresh_token(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
        _token: Option<&pagis_core::SealedSecret>,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ConnectionStore::set_refresh_token")
    }
    async fn refresh_token(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::ConnectionId,
    ) -> Result<Option<pagis_core::SealedSecret>, StoreError> {
        panic!("unexpected ConnectionStore::refresh_token")
    }
}

#[async_trait::async_trait]
impl ArtifactStore for ActorHost {
    async fn insert(&self, _artifact: &Artifact) -> Result<ArtifactOutcome, StoreError> {
        panic!("unexpected ArtifactStore::insert")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ArtifactId,
    ) -> Result<Option<Artifact>, StoreError> {
        panic!("unexpected ArtifactStore::get")
    }
    async fn expired_before(
        &self,
        _workspace_id: &WorkspaceId,
        _kind: ArtifactKind,
        _cutoff: UnixMillis,
        _limit: u32,
    ) -> Result<Vec<Artifact>, StoreError> {
        panic!("unexpected ArtifactStore::expired_before")
    }
    async fn delete(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ArtifactId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ArtifactStore::delete")
    }
}

#[async_trait::async_trait]
impl MemoryStore for ActorHost {
    async fn preview_forget(
        &self,
        _workspace: &WorkspaceId,
        _target: &pagis_core::ForgetTarget,
    ) -> Result<pagis_core::ForgetPreview, MemoryError> {
        panic!("unexpected MemoryStore::preview_forget")
    }
    async fn begin_forget(
        &self,
        _workspace: &WorkspaceId,
        _target: &pagis_core::ForgetTarget,
        _preview: &pagis_core::ForgetPreview,
        _now: i64,
        _keys: &dyn pagis_core::ForgetKeys,
    ) -> Result<pagis_core::ForgetOperation, MemoryError> {
        panic!("unexpected MemoryStore::begin_forget")
    }
    async fn purge_forgotten(
        &self,
        _workspace: &WorkspaceId,
        _operation_id: &str,
    ) -> Result<(), MemoryError> {
        panic!("unexpected MemoryStore::purge_forgotten")
    }
    async fn load_indexes(
        &self,
        _workspace_id: &WorkspaceId,
        _access: &MemoryAccess,
    ) -> Result<MemoryIndexes, MemoryError> {
        panic!("unexpected MemoryStore::load_indexes")
    }
    async fn read(
        &self,
        _workspace_id: &WorkspaceId,
        _access: &MemoryAccess,
        _path: &ScopedPath,
    ) -> Result<MemoryFile, MemoryError> {
        panic!("unexpected MemoryStore::read")
    }
    async fn brief_entries(
        &self,
        _workspace_id: &WorkspaceId,
        _access: &MemoryAccess,
        _since_revision: &str,
    ) -> Result<BriefEntries, MemoryError> {
        panic!("unexpected MemoryStore::brief_entries")
    }
    async fn list_pages(
        &self,
        _workspace_id: &WorkspaceId,
        _access: &MemoryAccess,
        _scope: MemoryScope,
        _query: &PageListQuery,
    ) -> Result<MemoryPageList, MemoryError> {
        panic!("unexpected MemoryStore::list_pages")
    }
    async fn search_pages(
        &self,
        _workspace_id: &WorkspaceId,
        _access: &MemoryAccess,
        _scope: MemoryScope,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<MemorySearchHit>, MemoryError> {
        panic!("unexpected MemoryStore::search_pages")
    }
    async fn count_pages(
        &self,
        _workspace_id: &WorkspaceId,
        _access: &MemoryAccess,
        _scope: MemoryScope,
    ) -> Result<MemoryPageCounts, MemoryError> {
        panic!("unexpected MemoryStore::count_pages")
    }
    async fn commit_diff(
        &self,
        _workspace_id: &WorkspaceId,
        _commit_id: &str,
        _access: &MemoryAccess,
    ) -> Result<MemoryCommitDiff, MemoryError> {
        panic!("unexpected MemoryStore::commit_diff")
    }
    async fn commit(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _access: &MemoryAccess,
        _author: &MemoryAuthor,
        _changeset: &MemoryChangeset,
        _expected_revision: Option<&str>,
        _message: &str,
        _run_id: Option<&RunId>,
    ) -> Result<String, MemoryError> {
        panic!("unexpected MemoryStore::commit")
    }
    async fn revert(
        &self,
        _workspace_id: &WorkspaceId,
        _commit_id: &str,
        _access: &MemoryAccess,
        _expected_revision: &str,
        _author: &MemoryAuthor,
    ) -> Result<String, MemoryError> {
        panic!("unexpected MemoryStore::revert")
    }
}

#[async_trait::async_trait]
impl PendingEvidenceStore for ActorHost {
    async fn record(
        &self,
        _evidence: pagis_core::PendingEvidence,
    ) -> Result<Option<pagis_core::PendingEvidenceRecord>, StoreError> {
        panic!("unexpected PendingEvidenceStore::record")
    }
    async fn review_cursor(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _channel_id: &ChannelId,
        _root_message_id: Option<&MessageId>,
        _subject: &str,
    ) -> Result<Option<MessageId>, StoreError> {
        panic!("unexpected PendingEvidenceStore::review_cursor")
    }
    async fn settle_compaction(
        &self,
        _evidence: &pagis_core::PendingEvidence,
        _memory_revision: Option<&str>,
        _now: UnixMillis,
    ) -> Result<(), StoreError> {
        panic!("unexpected PendingEvidenceStore::settle_compaction")
    }
    async fn next_due_at(&self) -> Result<Option<UnixMillis>, StoreError> {
        panic!("unexpected PendingEvidenceStore::next_due_at")
    }
    async fn pending_agents(
        &self,
        _now: UnixMillis,
    ) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError> {
        panic!("unexpected PendingEvidenceStore::pending_agents")
    }
    async fn claim(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _arrival_slots: u32,
        _now: UnixMillis,
    ) -> Result<Vec<pagis_core::PendingReviewClaim>, StoreError> {
        panic!("unexpected PendingEvidenceStore::claim")
    }
    async fn for_run(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Option<pagis_core::PendingEvidenceRecord>, StoreError> {
        panic!("unexpected PendingEvidenceStore::for_run")
    }
    async fn leased(&self) -> Result<Vec<pagis_core::PendingEvidenceRecord>, StoreError> {
        panic!("unexpected PendingEvidenceStore::leased")
    }
    async fn complete(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
        _lease_revision: u32,
        _memory_revision: Option<&str>,
        _now: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PendingEvidenceStore::complete")
    }
    async fn fail(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
        _lease_revision: u32,
        _error: &str,
        _now: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PendingEvidenceStore::fail")
    }
    async fn invalidate(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
        _lease_revision: u32,
        _now: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PendingEvidenceStore::invalidate")
    }
    async fn retry(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::PendingEvidenceId,
        _now: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PendingEvidenceStore::retry")
    }
}

#[async_trait::async_trait]
impl ContinuationStore for ActorHost {
    async fn get(
        &self,
        _key: &ContinuationKey,
    ) -> Result<Option<ContinuationCheckpoint>, StoreError> {
        panic!("unexpected ContinuationStore::get")
    }
    async fn replace(
        &self,
        _checkpoint: &ContinuationCheckpoint,
        _expected_revision: Option<u32>,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ContinuationStore::replace")
    }
}

#[async_trait::async_trait]
impl BriefStore for ActorHost {
    async fn load(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _channel_id: &pagis_core::ChannelId,
        _root_message_id: Option<&pagis_core::MessageId>,
    ) -> Result<BriefCursor, StoreError> {
        panic!("unexpected BriefStore::load")
    }
    async fn save(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _channel_id: &pagis_core::ChannelId,
        _root_message_id: Option<&pagis_core::MessageId>,
        _cursor: &BriefCursor,
    ) -> Result<(), StoreError> {
        panic!("unexpected BriefStore::save")
    }
}

#[async_trait::async_trait]
impl ScheduleStore for ActorHost {
    async fn create(&self, _schedule: &Schedule) -> Result<(), StoreError> {
        panic!("unexpected ScheduleStore::create")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ScheduleId,
    ) -> Result<Option<Schedule>, StoreError> {
        panic!("unexpected ScheduleStore::get")
    }
    async fn edit(
        &self,
        _previous: &Schedule,
        _replacement: &Schedule,
    ) -> Result<Option<Schedule>, StoreError> {
        panic!("unexpected ScheduleStore::edit")
    }
    async fn list_revisions(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ScheduleId,
    ) -> Result<Vec<ScheduleRevision>, StoreError> {
        panic!("unexpected ScheduleStore::list_revisions")
    }
    async fn list(
        &self,
        _workspace_id: &WorkspaceId,
        _before: Option<&ScheduleId>,
        _limit: u32,
    ) -> Result<Vec<Schedule>, StoreError> {
        panic!("unexpected ScheduleStore::list")
    }
    async fn list_open_for_subject_page(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _path: &str,
    ) -> Result<Vec<pagis_core::subject_page::OpenSchedule>, StoreError> {
        panic!("unexpected ScheduleStore::list_open_for_subject_page")
    }
    async fn archive(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ScheduleId,
        _at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        panic!("unexpected ScheduleStore::archive")
    }
    async fn pause(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ScheduleId,
        _at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        panic!("unexpected ScheduleStore::pause")
    }
    async fn resume(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &ScheduleId,
        _next_due_at: UnixMillis,
        _at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        panic!("unexpected ScheduleStore::resume")
    }
    async fn skip_next(
        &self,
        _schedule: &Schedule,
        _expected_due_at: UnixMillis,
        _next_due_at: Option<UnixMillis>,
        _at: UnixMillis,
    ) -> Result<ScheduleOccurrence, StoreError> {
        panic!("unexpected ScheduleStore::skip_next")
    }
    async fn list_occurrences(
        &self,
        _workspace_id: &WorkspaceId,
        _schedule_id: &ScheduleId,
        _before: Option<&ScheduleOccurrenceId>,
        _limit: u32,
    ) -> Result<Vec<ScheduleOccurrence>, StoreError> {
        panic!("unexpected ScheduleStore::list_occurrences")
    }
    async fn list_wakeups(
        &self,
        _workspace_id: &WorkspaceId,
        _schedule_id: &ScheduleId,
        _before: Option<&WakeupId>,
        _limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError> {
        panic!("unexpected ScheduleStore::list_wakeups")
    }
}

#[async_trait::async_trait]
impl TriggerStore for ActorHost {
    async fn next_due_at(&self) -> Result<Option<UnixMillis>, StoreError> {
        panic!("unexpected TriggerStore::next_due_at")
    }
    async fn pending_agents(&self) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError> {
        panic!("unexpected TriggerStore::pending_agents")
    }
    async fn process_due(&self, _now: UnixMillis) -> Result<DueBatch, StoreError> {
        panic!("unexpected TriggerStore::process_due")
    }
    async fn wake_now(
        &self,
        _workspace_id: &WorkspaceId,
        _schedule_id: &ScheduleId,
        _now: UnixMillis,
    ) -> Result<Option<Wakeup>, StoreError> {
        panic!("unexpected TriggerStore::wake_now")
    }
    async fn claim_wakeups(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _slots: RunSlots,
        _now: UnixMillis,
    ) -> Result<Vec<WakeupClaim>, StoreError> {
        panic!("unexpected TriggerStore::claim_wakeups")
    }
    async fn get_wakeup(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &WakeupId,
    ) -> Result<Option<Wakeup>, StoreError> {
        panic!("unexpected TriggerStore::get_wakeup")
    }
    async fn ingest(
        &self,
        _batch: IngestBatch,
        _eligible: &[EventSubscription],
        _matcher: &dyn EventMatcher,
        _arrival: Option<&ArrivalRun>,
        _source_resource: Option<&str>,
        _keys: &dyn pagis_core::ForgetKeys,
    ) -> Result<IngestOutcome, StoreError> {
        panic!("unexpected TriggerStore::ingest")
    }
    async fn cursor(
        &self,
        _workspace_id: &WorkspaceId,
        _connection_id: &ConnectionId,
        _event_kind: &str,
    ) -> Result<Option<String>, StoreError> {
        panic!("unexpected TriggerStore::cursor")
    }
    async fn record_failure(
        &self,
        _workspace_id: &WorkspaceId,
        _connection_id: &ConnectionId,
        _event_kind: &str,
        _code: &str,
        _at: UnixMillis,
    ) -> Result<SourceBatch, StoreError> {
        panic!("unexpected TriggerStore::record_failure")
    }
    async fn event_context(
        &self,
        _workspace_id: &WorkspaceId,
        _wakeup_id: &WakeupId,
    ) -> Result<Option<EventWakeupContext>, StoreError> {
        panic!("unexpected TriggerStore::event_context")
    }
}

#[async_trait::async_trait]
impl ForgetStore for ActorHost {
    async fn preview(
        &self,
        _workspace: &WorkspaceId,
        _target: &ForgetTarget,
    ) -> Result<ForgetPreview, StoreError> {
        panic!("unexpected ForgetStore::preview")
    }
    async fn preview_affects(
        &self,
        _workspace: &WorkspaceId,
        _target: &ForgetTarget,
        _exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        panic!("unexpected ForgetStore::preview_affects")
    }
    async fn begin(
        &self,
        _workspace: &WorkspaceId,
        _target: &ForgetTarget,
        _expected_revision: &str,
        _now: i64,
        _keys: &dyn ForgetKeys,
    ) -> Result<ForgetOperation, StoreError> {
        panic!("unexpected ForgetStore::begin")
    }
    async fn is_suppressed(
        &self,
        _source: &SourceKey,
        _source_id: &str,
        _keys: &dyn ForgetKeys,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ForgetStore::is_suppressed")
    }
    async fn permits_provenance(
        &self,
        _workspace: &WorkspaceId,
        _exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError> {
        panic!("unexpected ForgetStore::permits_provenance")
    }
    async fn run_allowed(&self, workspace: &WorkspaceId, run: &RunId) -> Result<bool, StoreError> {
        assert_eq!(workspace, &self.agent.workspace_id);
        let index = if run.as_str() == "first" {
            0
        } else {
            assert_eq!(run.as_str(), "second");
            1
        };
        self.entered[index].notify_one();
        self.release[index].notified().await;
        Ok(false)
    }
    async fn raw_connection_allowed(
        &self,
        _workspace: &WorkspaceId,
        _connection: &ConnectionId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected ForgetStore::raw_connection_allowed")
    }
    async fn operation(
        &self,
        _workspace: &WorkspaceId,
        _id: &str,
    ) -> Result<Option<ForgetOperation>, StoreError> {
        panic!("unexpected ForgetStore::operation")
    }
    async fn operations(
        &self,
        _workspace: &WorkspaceId,
    ) -> Result<Vec<ForgetOperation>, StoreError> {
        panic!("unexpected ForgetStore::operations")
    }
    async fn unfinished(
        &self,
        _workspace: &WorkspaceId,
    ) -> Result<Vec<ForgetOperation>, StoreError> {
        panic!("unexpected ForgetStore::unfinished")
    }
    async fn purge_structured(
        &self,
        _workspace: &WorkspaceId,
        _id: &str,
    ) -> Result<(), StoreError> {
        panic!("unexpected ForgetStore::purge_structured")
    }
    async fn memory_purged(&self, _workspace: &WorkspaceId, _id: &str) -> Result<(), StoreError> {
        panic!("unexpected ForgetStore::memory_purged")
    }
    async fn complete(&self, _workspace: &WorkspaceId, _id: &str) -> Result<(), StoreError> {
        panic!("unexpected ForgetStore::complete")
    }
    async fn record_failure(
        &self,
        _workspace: &WorkspaceId,
        _id: &str,
        _error: &str,
    ) -> Result<(), StoreError> {
        panic!("unexpected ForgetStore::record_failure")
    }
    async fn retry(&self, _workspace: &WorkspaceId, _id: &str) -> Result<(), StoreError> {
        panic!("unexpected ForgetStore::retry")
    }
    async fn reopt_in(
        &self,
        _workspace: &WorkspaceId,
        _id: &str,
        _now: i64,
    ) -> Result<(), StoreError> {
        panic!("unexpected ForgetStore::reopt_in")
    }
}

#[async_trait::async_trait]
impl ConversationEvidenceStore for ActorHost {
    async fn search(
        &self,
        _scope: &ConversationScope,
        _query: &str,
        _after: Option<&str>,
        _limit: u32,
    ) -> Result<Vec<ConversationEvidenceHit>, StoreError> {
        panic!("unexpected ConversationEvidenceStore::search")
    }
    async fn read_range(
        &self,
        _scope: &ConversationScope,
        _after_exclusive: Option<&MessageId>,
        _through_inclusive: &MessageId,
        _limit: u32,
    ) -> Result<Vec<ConversationEvidenceMessage>, StoreError> {
        panic!("unexpected ConversationEvidenceStore::read_range")
    }
    async fn read_message(
        &self,
        _scope: &ConversationScope,
        _message_id: &MessageId,
    ) -> Result<Option<ConversationEvidenceMessage>, StoreError> {
        panic!("unexpected ConversationEvidenceStore::read_message")
    }
    async fn retain_tool(&self, _evidence: &RetainedToolEvidence) -> Result<String, StoreError> {
        panic!("unexpected ConversationEvidenceStore::retain_tool")
    }
    async fn read_tool(
        &self,
        _scope: &ConversationScope,
        _reference: &str,
    ) -> Result<Option<ConversationToolEvidence>, StoreError> {
        panic!("unexpected ConversationEvidenceStore::read_tool")
    }
}

#[async_trait::async_trait]
impl CapabilitySnapshotStore for ActorHost {
    async fn put(&self, _snapshot: &CapabilitySnapshotRecord) -> Result<(), StoreError> {
        panic!("unexpected CapabilitySnapshotStore::put")
    }
    async fn bind_run(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
        _snapshot_id: &str,
    ) -> Result<(), StoreError> {
        panic!("unexpected CapabilitySnapshotStore::bind_run")
    }
    async fn for_run(
        &self,
        _workspace_id: &WorkspaceId,
        _run_id: &RunId,
    ) -> Result<Option<CapabilitySnapshotRecord>, StoreError> {
        panic!("unexpected CapabilitySnapshotStore::for_run")
    }
}

#[async_trait::async_trait]
impl PhoneNumberStore for ActorHost {
    async fn create_intent(&self, _intent: &PurchaseIntent) -> Result<(), StoreError> {
        panic!("unexpected PhoneNumberStore::create_intent")
    }
    async fn list_pending_intents(&self) -> Result<Vec<PurchaseIntent>, StoreError> {
        panic!("unexpected PhoneNumberStore::list_pending_intents")
    }
    async fn confirm_purchase(
        &self,
        _workspace_id: &WorkspaceId,
        _intent_id: &PurchaseIntentId,
        _number: &PhoneNumber,
        _at: UnixMillis,
    ) -> Result<(), StoreError> {
        panic!("unexpected PhoneNumberStore::confirm_purchase")
    }
    async fn create(&self, _number: &PhoneNumber) -> Result<(), StoreError> {
        panic!("unexpected PhoneNumberStore::create")
    }
    async fn abandon_intent(
        &self,
        _workspace_id: &WorkspaceId,
        _intent_id: &PurchaseIntentId,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::abandon_intent")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        panic!("unexpected PhoneNumberStore::get")
    }
    async fn list(&self, _workspace_id: &WorkspaceId) -> Result<Vec<PhoneNumber>, StoreError> {
        panic!("unexpected PhoneNumberStore::list")
    }
    async fn for_agent(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        panic!("unexpected PhoneNumberStore::for_agent")
    }
    async fn live_for_e164(&self, _e164: &str) -> Result<Vec<PhoneNumber>, StoreError> {
        panic!("unexpected PhoneNumberStore::live_for_e164")
    }
    async fn assign(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _agent_id: &AgentId,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::assign")
    }
    async fn unassign(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::unassign")
    }
    async fn unassign_for_agent(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        panic!("unexpected PhoneNumberStore::unassign_for_agent")
    }
    async fn release(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::release")
    }
    async fn any_live_for_connection(
        &self,
        _workspace_id: &WorkspaceId,
        _connection_id: &ConnectionId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::any_live_for_connection")
    }
    async fn set_outgoing_cap(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _cap: u32,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::set_outgoing_cap")
    }
    async fn add_allow_rules(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _rules: &[String],
    ) -> Result<Vec<String>, StoreError> {
        panic!("unexpected PhoneNumberStore::add_allow_rules")
    }
    async fn note_text_send(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _at: UnixMillis,
    ) -> Result<u32, StoreError> {
        panic!("unexpected PhoneNumberStore::note_text_send")
    }
    async fn set_messaging_readiness(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _readiness: &MessagingReadiness,
        _at: UnixMillis,
        _error: Option<&str>,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::set_messaging_readiness")
    }
    async fn set_text_cursor(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _cursor: &str,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::set_text_cursor")
    }
    async fn set_messaging_object_id(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _object_id: &str,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::set_messaging_object_id")
    }
    async fn set_relay_state(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _state: &TelnyxRelayState,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::set_relay_state")
    }
    async fn set_relay_consent(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &PhoneNumberId,
        _consent: bool,
    ) -> Result<bool, StoreError> {
        panic!("unexpected PhoneNumberStore::set_relay_consent")
    }
}

#[async_trait::async_trait]
impl AgentMailboxStore for ActorHost {
    async fn create(&self, _mailbox: &AgentMailbox) -> Result<(), StoreError> {
        panic!("unexpected AgentMailboxStore::create")
    }
    async fn drop_reservation(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
    ) -> Result<bool, StoreError> {
        panic!("unexpected AgentMailboxStore::drop_reservation")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        panic!("unexpected AgentMailboxStore::get")
    }
    async fn for_agent(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        panic!("unexpected AgentMailboxStore::for_agent")
    }
    async fn list(&self, _workspace_id: &WorkspaceId) -> Result<Vec<AgentMailbox>, StoreError> {
        panic!("unexpected AgentMailboxStore::list")
    }
    async fn address_taken(&self, _address: &str) -> Result<bool, StoreError> {
        panic!("unexpected AgentMailboxStore::address_taken")
    }
    async fn set_state(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
        _state: AgentMailboxState,
        _reason: Option<&str>,
    ) -> Result<bool, StoreError> {
        panic!("unexpected AgentMailboxStore::set_state")
    }
    async fn set_cursor(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
        _cursor: &MailboxCursor,
    ) -> Result<bool, StoreError> {
        panic!("unexpected AgentMailboxStore::set_cursor")
    }
    async fn make_dormant_for_agent(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        panic!("unexpected AgentMailboxStore::make_dormant_for_agent")
    }
    async fn delete(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
        _at: UnixMillis,
    ) -> Result<bool, StoreError> {
        panic!("unexpected AgentMailboxStore::delete")
    }
    async fn note_send(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
        _at: UnixMillis,
    ) -> Result<u32, StoreError> {
        panic!("unexpected AgentMailboxStore::note_send")
    }
    async fn add_allow_rules(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &AgentMailboxId,
        _rules: &[String],
    ) -> Result<Vec<String>, StoreError> {
        panic!("unexpected AgentMailboxStore::add_allow_rules")
    }
    async fn count_live_for_connection(
        &self,
        _workspace_id: &WorkspaceId,
        _connection_id: &ConnectionId,
    ) -> Result<u32, StoreError> {
        panic!("unexpected AgentMailboxStore::count_live_for_connection")
    }
}

#[async_trait::async_trait]
impl HostStore for ActorHost {
    async fn register(
        &self,
        _workspace_id: &WorkspaceId,
        _name: &str,
        _platform: &str,
        _capabilities: &[String],
        _at: UnixMillis,
    ) -> Result<Host, StoreError> {
        panic!("unexpected HostStore::register")
    }
    async fn get(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &HostId,
    ) -> Result<Option<Host>, StoreError> {
        panic!("unexpected HostStore::get")
    }
    async fn list(&self, _workspace_id: &WorkspaceId) -> Result<Vec<Host>, StoreError> {
        panic!("unexpected HostStore::list")
    }
    async fn touch(&self, _id: &HostId, _at: UnixMillis) -> Result<bool, StoreError> {
        panic!("unexpected HostStore::touch")
    }
}

#[async_trait::async_trait]
impl RunJournal for ActorHost {
    async fn checkpoint(
        &self,
        run: &RunId,
        turn: usize,
        state: &serde_json::Value,
    ) -> Result<(), StoreError> {
        self.journal.lock().unwrap().insert(
            run.clone(),
            RunJournalEntry {
                checkpoint: Some(RunCheckpoint {
                    turn,
                    state: state.clone(),
                }),
                pending_turn: None,
                calls: vec![],
                resumed: false,
            },
        );
        Ok(())
    }
    async fn turn_started(&self, run: &RunId, state: &serde_json::Value) -> Result<(), StoreError> {
        self.journal
            .lock()
            .unwrap()
            .entry(run.clone())
            .or_insert(RunJournalEntry {
                checkpoint: None,
                pending_turn: None,
                calls: vec![],
                resumed: false,
            })
            .pending_turn = Some(state.clone());
        Ok(())
    }
    async fn tool_started(&self, run: &RunId, call: &str, effect: &str) -> Result<(), StoreError> {
        self.journal
            .lock()
            .unwrap()
            .get_mut(run)
            .unwrap()
            .calls
            .push(JournalCall {
                call_id: call.into(),
                effect_class: effect.into(),
                result: None,
            });
        Ok(())
    }
    async fn tool_ended(
        &self,
        run: &RunId,
        call: &str,
        result: &serde_json::Value,
    ) -> Result<(), StoreError> {
        self.journal
            .lock()
            .unwrap()
            .get_mut(run)
            .unwrap()
            .calls
            .iter_mut()
            .find(|held| held.call_id == call)
            .unwrap()
            .result = Some(result.clone());
        Ok(())
    }
    async fn last(&self, run: &RunId) -> Result<Option<RunJournalEntry>, StoreError> {
        Ok(self.journal.lock().unwrap().get(run).cloned())
    }
    async fn mark_resumed(&self, run: &RunId) -> Result<bool, StoreError> {
        let mut journal = self.journal.lock().unwrap();
        let Some(entry) = journal.get_mut(run) else {
            return Ok(false);
        };
        if entry.resumed || entry.checkpoint.is_none() {
            return Ok(false);
        }
        entry.resumed = true;
        Ok(true)
    }
    async fn delete(&self, run: &RunId) -> Result<(), StoreError> {
        self.journal.lock().unwrap().remove(run);
        Ok(())
    }
}
