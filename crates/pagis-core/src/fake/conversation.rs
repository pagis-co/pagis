use super::*;

#[async_trait::async_trait]
impl ChannelStore for FakeChannelStore {
    async fn get_or_create_conversation(
        &self,
        workspace_id: &WorkspaceId,
        participants: &[WorkspaceId],
    ) -> Result<Channel, StoreError> {
        let ids: std::collections::BTreeSet<_> = participants.iter().cloned().collect();
        if ids.len() < 2 {
            return Err(conflict("a conversation needs two Workspaces"));
        }
        let mut data = self.0.lock().unwrap();
        if let Some(channel) = data
            .channels
            .rows(workspace_id)
            .find(|c| {
                c.kind == ChannelKind::Conversation
                    && data
                        .participants
                        .rows(workspace_id)
                        .filter(|p| p.channel_id == c.id)
                        .filter_map(|p| p.participant_workspace_id.clone())
                        .collect::<std::collections::BTreeSet<_>>()
                        == ids
            })
            .cloned()
        {
            return Ok(channel);
        }
        let now = now_ms();
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace_id.clone(),
            kind: ChannelKind::Conversation,
            title: None,
            created_at: now,
            updated_at: now,
        };
        data.channels
            .insert(workspace_id, channel.id.clone(), channel.clone())?;
        for id in ids {
            let participant = ChannelParticipant {
                id: ParticipantId::generate(),
                workspace_id: workspace_id.clone(),
                channel_id: channel.id.clone(),
                kind: ParticipantKind::Workspace,
                participant_workspace_id: Some(id),
                person_id: None,
                agent_id: None,
                joined_at: now,
            };
            data.participants
                .insert(workspace_id, participant.id.clone(), participant)?;
        }
        Ok(channel)
    }

    async fn create(&self, channel: &Channel) -> Result<(), StoreError> {
        self.0.lock().unwrap().channels.insert(
            &channel.workspace_id,
            channel.id.clone(),
            channel.clone(),
        )
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ChannelId,
    ) -> Result<Option<Channel>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .channels
            .get(workspace_id, id)
            .cloned())
    }
    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Channel>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .channels
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn find_user_dm(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<Channel>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .channels
            .rows(workspace_id)
            .find(|c| {
                c.kind == ChannelKind::Dm
                    && data
                        .participants
                        .rows(workspace_id)
                        .any(|p| p.channel_id == c.id && p.kind == ParticipantKind::Person)
                    && data
                        .participants
                        .rows(workspace_id)
                        .any(|p| p.channel_id == c.id && p.agent_id.as_ref() == Some(agent_id))
            })
            .cloned())
    }
    async fn find_agent_dm(
        &self,
        workspace_id: &WorkspaceId,
        a: &AgentId,
        b: &AgentId,
    ) -> Result<Option<Channel>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .channels
            .rows(workspace_id)
            .find(|c| {
                c.kind == ChannelKind::Dm
                    && [a, b].iter().all(|agent| {
                        data.participants
                            .rows(workspace_id)
                            .any(|p| p.channel_id == c.id && p.agent_id.as_ref() == Some(*agent))
                    })
                    && !data
                        .participants
                        .rows(workspace_id)
                        .any(|p| p.channel_id == c.id && p.kind == ParticipantKind::Person)
            })
            .cloned())
    }
}

#[async_trait::async_trait]
impl ParticipantStore for FakeParticipantStore {
    async fn create(&self, participant: &ChannelParticipant) -> Result<(), StoreError> {
        self.0.lock().unwrap().participants.insert(
            &participant.workspace_id,
            participant.id.clone(),
            participant.clone(),
        )
    }
    async fn agents_in_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Vec<AgentId>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .participants
            .rows(workspace_id)
            .filter(|p| &p.channel_id == channel_id && p.kind == ParticipantKind::Agent)
            .filter_map(|p| p.agent_id.clone())
            .collect())
    }
    async fn list_for_channel(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Vec<ChannelParticipant>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .participants
            .rows(workspace_id)
            .filter(|p| &p.channel_id == channel_id)
            .cloned()
            .collect())
    }
}

#[async_trait::async_trait]
impl MessageStore for FakeMessageStore {
    async fn insert(&self, message: &Message) -> Result<SendOutcome, StoreError> {
        insert_message(&mut self.0.lock().unwrap(), message, None)
    }
    async fn insert_stamped(
        &self,
        message: &Message,
        exposures: &[crate::MemoryExposure],
    ) -> Result<SendOutcome, StoreError> {
        insert_message(&mut self.0.lock().unwrap(), message, Some(exposures))
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<Option<Message>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .messages
            .get(workspace_id, id)
            .cloned())
    }
    async fn is_forgotten(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<bool, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .forgotten_messages
            .contains(&(workspace_id.clone(), id.clone())))
    }
    async fn set_exposures(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
        exposures: &[crate::MemoryExposure],
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.messages.get(workspace_id, id).is_some() {
            data.message_exposures
                .put(workspace_id, id.clone(), exposures.to_vec());
        }
        Ok(())
    }
    async fn exposures(
        &self,
        workspace_id: &WorkspaceId,
        id: &MessageId,
    ) -> Result<Option<Vec<crate::MemoryExposure>>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .message_exposures
            .get(workspace_id, id)
            .cloned())
    }
    async fn list_top_level(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> Result<Vec<TimelineEntry>, StoreError> {
        let data = self.0.lock().unwrap();
        let rows = data
            .messages
            .rows(workspace_id)
            .filter(|m| &m.channel_id == channel_id && m.parent_message_id.is_none())
            .map(|m| {
                let mut replies: Vec<_> = data
                    .messages
                    .rows(workspace_id)
                    .filter(|r| r.parent_message_id.as_ref() == Some(&m.id))
                    .collect();
                replies.sort_by_key(|r| std::cmp::Reverse((r.created_at, r.id.clone())));
                let mut authors = Vec::new();
                for reply in &replies {
                    let author = reply.author.clone();
                    if !authors.contains(&author) && authors.len() < REPLY_AUTHORS_MAX {
                        authors.push(author);
                    }
                }
                (
                    m.id.clone(),
                    TimelineEntry {
                        message: m.clone(),
                        reply_count: replies.len() as u32,
                        last_reply_at: replies.first().map(|r| r.created_at),
                        reply_authors: authors,
                    },
                )
            })
            .collect();
        Ok(page(rows, before, limit))
    }
    async fn last_message(
        &self,
        workspace_id: &WorkspaceId,
        channel_id: &ChannelId,
    ) -> Result<Option<Message>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .messages
            .rows(workspace_id)
            .filter(|m| {
                &m.channel_id == channel_id
                    && m.parent_message_id.is_none()
                    && !m.blocks.iter().any(Block::is_progress)
            })
            .max_by_key(|m| &m.id)
            .cloned())
    }
    async fn list_agent_elsewhere(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        exclude_channel: &ChannelId,
        before: Option<&MessageId>,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .messages
                .rows(workspace_id)
                .filter(|m| {
                    m.author.agent_id() == Some(agent_id)
                        && &m.channel_id != exclude_channel
                        && m.status == MessageStatus::Complete
                        && !m.blocks.iter().any(Block::is_progress)
                })
                .map(|m| (m.id.clone(), m.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn list_thread(
        &self,
        workspace_id: &WorkspaceId,
        root_id: &MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .messages
            .rows(workspace_id)
            .filter(|m| &m.id == root_id || m.parent_message_id.as_ref() == Some(root_id))
            .cloned()
            .collect())
    }
    async fn latest_agent_message_of_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<Message>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .messages
            .rows(workspace_id)
            .filter(|m| {
                m.author.agent_id().is_some()
                    && m.run_id.as_ref() == Some(run_id)
                    && m.status == MessageStatus::Complete
            })
            .max_by_key(|m| &m.id)
            .cloned())
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
        if let Some(row) = self.0.lock().unwrap().messages.get_mut(workspace_id, id) {
            row.status = status;
            row.blocks = blocks.to_vec();
            row.text_content = text_content.to_string();
            row.completed_at = Some(completed_at);
        }
        Ok(())
    }
    async fn fail_streaming(&self, completed_at: UnixMillis) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut count = 0;
        for row in data
            .messages
            .0
            .values_mut()
            .filter(|row| row.status == MessageStatus::Streaming)
        {
            row.status = MessageStatus::Failed;
            row.completed_at = Some(completed_at);
            count += 1;
        }
        Ok(count)
    }
}

#[async_trait::async_trait]
impl RunStore for FakeRunStore {
    async fn create(&self, run: &Run) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .runs
            .insert(&run.workspace_id, run.id.clone(), run.clone())
    }
    async fn get(&self, workspace_id: &WorkspaceId, id: &RunId) -> Result<Option<Run>, StoreError> {
        Ok(self.0.lock().unwrap().runs.get(workspace_id, id).cloned())
    }
    async fn update(&self, run: &Run) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.runs.get_mut(&run.workspace_id, &run.id) else {
            return Ok(());
        };
        *row = run.clone();
        Ok(())
    }
    async fn list_unfinished(&self) -> Result<Vec<Run>, StoreError> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .runs
            .0
            .values()
            .filter(|r| !r.state.is_terminal())
            .cloned()
            .collect();
        rows.sort_by_key(|r| (r.created_at, r.id.clone()));
        Ok(rows)
    }
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        channel_id: Option<&ChannelId>,
        states: &[crate::domain::RunState],
        before: Option<&RunId>,
        limit: u32,
    ) -> Result<Vec<Run>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .runs
                .rows(workspace_id)
                .filter(|r| {
                    agent_id.is_none_or(|id| &r.agent_id == id)
                        && channel_id.is_none_or(|id| r.channel_id.as_ref() == Some(id))
                        && (states.is_empty() || states.contains(&r.state))
                })
                .map(|r| (r.id.clone(), r.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn dismiss(
        &self,
        workspace_id: &WorkspaceId,
        id: &RunId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.runs.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.dismissed_at.get_or_insert(at);
        Ok(true)
    }
    async fn record_source_reads(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        connection_id: &ConnectionId,
        reads: &[crate::knowledge::SourceRead],
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.runs.get(workspace_id, run_id).is_some() {
            let held = data
                .source_reads
                .0
                .entry((
                    workspace_id.clone(),
                    (run_id.clone(), connection_id.clone()),
                ))
                .or_default();
            for read in reads {
                if !held.contains(read) {
                    held.push(read.clone());
                }
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl RequestStore for FakeRequestStore {
    async fn create(&self, request: &Request) -> Result<(), StoreError> {
        self.0.lock().unwrap().requests.insert(
            &request.workspace_id,
            request.id.clone(),
            request.clone(),
        )
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &RequestId,
    ) -> Result<Option<Request>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .requests
            .get(workspace_id, id)
            .cloned())
    }
    async fn list_by_state(
        &self,
        workspace_id: &WorkspaceId,
        state: RequestState,
        kind: Option<&str>,
    ) -> Result<Vec<Request>, StoreError> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .requests
            .rows(workspace_id)
            .filter(|r| r.state == state && kind.is_none_or(|k| r.kind == k))
            .cloned()
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.id.clone()));
        Ok(rows)
    }
    async fn decide(
        &self,
        workspace_id: &WorkspaceId,
        id: &RequestId,
        state: RequestState,
        values: Option<serde_json::Value>,
        decided_by: crate::Author,
        decided_at: UnixMillis,
    ) -> Result<DecideOutcome, StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = data
            .requests
            .get_mut(workspace_id, id)
            .ok_or_else(|| conflict("Request does not exist"))?;
        if row.state != RequestState::Pending {
            return Ok(DecideOutcome::NotPending(row.clone()));
        }
        row.state = state;
        row.values = values;
        row.decided_by = Some(decided_by);
        row.decided_at = Some(decided_at);
        Ok(DecideOutcome::Decided(row.clone()))
    }
    async fn expire_pending_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        decided_at: UnixMillis,
    ) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut count = 0;
        for row in data
            .requests
            .rows_mut(workspace_id)
            .filter(|r| r.run_id.as_ref() == Some(run_id) && r.state == RequestState::Pending)
        {
            row.state = RequestState::Expired;
            row.decided_by = Some(crate::Author::System);
            row.decided_at = Some(decided_at);
            count += 1;
        }
        Ok(count)
    }
}

#[async_trait::async_trait]
impl UsageStore for FakeUsageStore {
    async fn record(&self, usage: &UsageRecord) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .usage
            .insert(&usage.workspace_id, usage.id.clone(), usage.clone())
    }
    async fn total_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
        period: UsagePeriod,
    ) -> Result<UsageTotal, StoreError> {
        Ok(usage_total(
            self.0
                .lock()
                .unwrap()
                .usage
                .rows(workspace_id)
                .filter(|r| period.from <= r.created_at && r.created_at < period.to),
        ))
    }
    async fn totals_by_workspace(
        &self,
        period: UsagePeriod,
    ) -> Result<Vec<WorkspaceUsage>, StoreError> {
        let data = self.0.lock().unwrap();
        let ids: BTreeSet<_> = data
            .usage
            .0
            .values()
            .filter(|r| period.from <= r.created_at && r.created_at < period.to)
            .map(|r| r.workspace_id.clone())
            .collect();
        let mut rows: Vec<_> = ids
            .iter()
            .map(|id| WorkspaceUsage {
                workspace_id: id.clone(),
                total: usage_total(
                    data.usage
                        .rows(id)
                        .filter(|r| period.from <= r.created_at && r.created_at < period.to),
                ),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.total
                .cost_usd
                .total_cmp(&a.total.cost_usd)
                .then_with(|| a.workspace_id.cmp(&b.workspace_id))
        });
        Ok(rows)
    }
    async fn runs_for_workspace(
        &self,
        workspace_id: &WorkspaceId,
        period: UsagePeriod,
        limit: u32,
    ) -> Result<Vec<RunUsage>, StoreError> {
        let data = self.0.lock().unwrap();
        let ids: BTreeSet<_> = data
            .usage
            .rows(workspace_id)
            .filter(|r| period.from <= r.created_at && r.created_at < period.to)
            .filter_map(|r| r.run_id.clone())
            .collect();
        let mut rows: Vec<_> = ids
            .iter()
            .map(|id| {
                let held: Vec<_> = data
                    .usage
                    .rows(workspace_id)
                    .filter(|r| {
                        r.run_id.as_ref() == Some(id)
                            && (period.from <= r.created_at && r.created_at < period.to)
                    })
                    .collect();
                RunUsage {
                    run_id: id.clone(),
                    last_at: held.iter().map(|r| r.created_at).max().unwrap_or(0),
                    total: usage_total(held.into_iter()),
                }
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse((r.last_at, r.run_id.clone())));
        rows.truncate(limit as usize);
        Ok(rows)
    }
}

#[async_trait::async_trait]
impl ModelRequestCaptureStore for FakeModelRequestCaptureStore {
    async fn record(&self, capture: &ModelRequestCapture) -> Result<(), StoreError> {
        self.0.lock().unwrap().captures.insert(
            &capture.workspace_id,
            capture.id.clone(),
            capture.clone(),
        )
    }
    async fn list_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<ModelRequestCapture>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .captures
            .rows(workspace_id)
            .filter(|r| &r.run_id == run_id)
            .cloned()
            .collect())
    }
    async fn delete_before(&self, cutoff: UnixMillis) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let before = data.captures.0.len();
        let held: BTreeSet<_> = data
            .workspaces
            .values()
            .filter(|w| w.legal_hold.is_some())
            .map(|w| w.id.clone())
            .collect();
        data.captures
            .0
            .retain(|_, r| r.created_at >= cutoff || held.contains(&r.workspace_id));
        Ok((before - data.captures.0.len()) as u64)
    }
    async fn delete_all(&self) -> Result<u64, StoreError> {
        let mut data = self.0.lock().unwrap();
        let count = data.captures.0.len();
        let held: BTreeSet<_> = data
            .workspaces
            .values()
            .filter(|w| w.legal_hold.is_some())
            .map(|w| w.id.clone())
            .collect();
        data.captures
            .0
            .retain(|_, r| held.contains(&r.workspace_id));
        Ok((count - data.captures.0.len()) as u64)
    }
}

#[async_trait::async_trait]
impl CodingSessionStore for FakeCodingSessionStore {
    async fn insert(&self, session: &CodingSession) -> Result<(), StoreError> {
        self.0.lock().unwrap().coding_sessions.insert(
            &session.workspace_id,
            session.id.clone(),
            session.clone(),
        )
    }
    async fn update(&self, session: &CodingSession) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .coding_sessions
            .get_mut(&session.workspace_id, &session.id)
        else {
            return Ok(false);
        };
        *row = session.clone();
        Ok(true)
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CodingSessionId,
    ) -> Result<Option<CodingSession>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .coding_sessions
            .get(workspace_id, id)
            .cloned())
    }
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        state: Option<CodingSessionState>,
        before: Option<&CodingSessionId>,
        limit: u32,
    ) -> Result<Vec<CodingSession>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .coding_sessions
                .rows(workspace_id)
                .filter(|r| {
                    agent_id.is_none_or(|id| &r.agent_id == id)
                        && state.is_none_or(|s| r.state == s)
                })
                .map(|r| (r.id.clone(), r.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn count_open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u32, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .coding_sessions
            .rows(workspace_id)
            .filter(|r| &r.agent_id == agent_id && !r.state.is_terminal())
            .count() as u32)
    }
    async fn list_open(&self) -> Result<Vec<CodingSession>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .coding_sessions
            .0
            .values()
            .filter(|r| !r.state.is_terminal())
            .cloned()
            .collect())
    }
    async fn append_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        event: NewCodingSessionEvent,
    ) -> Result<Vec<CodingSessionEvent>, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .coding_sessions
            .get(workspace_id, coding_session_id)
            .is_none()
        {
            return Ok(vec![]);
        }
        let rows = data
            .coding_events
            .0
            .entry((workspace_id.clone(), coding_session_id.clone()))
            .or_default();
        let mut written = Vec::new();
        for write in crate::coding_session::fold(rows.last(), event) {
            let row = match write {
                TranscriptWrite::Replace { seq, payload } => {
                    let row = rows.iter_mut().find(|r| r.seq == seq).ok_or_else(|| {
                        StoreError::Corrupt("transcript row does not exist".into())
                    })?;
                    row.payload = payload;
                    row.clone()
                }
                TranscriptWrite::Append {
                    seq,
                    at,
                    kind,
                    payload,
                } => {
                    let row = CodingSessionEvent {
                        workspace_id: workspace_id.clone(),
                        coding_session_id: coding_session_id.clone(),
                        seq,
                        at,
                        kind,
                        payload,
                    };
                    rows.push(row.clone());
                    row
                }
            };
            written.push(row);
        }
        Ok(written)
    }
    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<CodingSessionEvent>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .coding_events
            .get(workspace_id, coding_session_id)
            .into_iter()
            .flatten()
            .filter(|r| r.seq > after.unwrap_or(0))
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn latest_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        kinds: &[CodingSessionEventKind],
    ) -> Result<Option<CodingSessionEvent>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .coding_events
            .get(workspace_id, coding_session_id)
            .into_iter()
            .flatten()
            .filter(|r| kinds.contains(&r.kind))
            .max_by_key(|r| r.seq)
            .cloned())
    }
    async fn set_model_token(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        hash: &str,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .coding_sessions
            .get(workspace_id, coding_session_id)
            .is_none()
        {
            return Ok(false);
        }
        data.model_tokens
            .put(workspace_id, coding_session_id.clone(), hash.to_string());
        Ok(true)
    }
    async fn model_token_owner(&self, hash: &str) -> Result<Option<ModelTokenOwner>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data.model_tokens.0.iter().find_map(|((w, id), held)| {
            if held == hash {
                data.coding_sessions
                    .get(w, id)
                    .filter(|r| {
                        matches!(
                            r.state,
                            CodingSessionState::Working
                                | CodingSessionState::NeedsDecision
                                | CodingSessionState::Idle
                        )
                    })
                    .map(|r| ModelTokenOwner {
                        workspace_id: w.clone(),
                        agent_id: r.agent_id.clone(),
                        session_id: id.clone(),
                        run_id: r.run_id.clone(),
                    })
            } else {
                None
            }
        }))
    }
}

#[async_trait::async_trait]
impl ConversationEvidenceStore for FakeConversationEvidenceStore {
    async fn search_workspace_messages(
        &self,
        workspace_id: &WorkspaceId,
        participant: Option<&WorkspaceId>,
        query: &str,
        limit: u32,
    ) -> Result<Vec<WorkspaceMessageHit>, StoreError> {
        let words: Vec<_> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect();
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let data = self.0.lock().unwrap();
        let mut hits: Vec<_> = data
            .messages
            .rows(workspace_id)
            .filter(|message| {
                message_readable(&data, message)
                    && participant.is_none_or(|workspace| {
                        data.participants.rows(workspace_id).any(|p| {
                            p.channel_id == message.channel_id
                                && p.kind == ParticipantKind::Workspace
                                && p.participant_workspace_id.as_ref() == Some(workspace)
                        })
                    })
                    && words
                        .iter()
                        .any(|word| message.text_content.to_lowercase().contains(word))
            })
            .map(|message| WorkspaceMessageHit {
                message_id: message.id.clone(),
                channel_id: message.channel_id.clone(),
                root_message_id: message
                    .parent_message_id
                    .clone()
                    .unwrap_or_else(|| message.id.clone()),
                snippet: message.text_content.clone(),
                created_at: message.created_at,
            })
            .collect();
        hits.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.message_id.cmp(&left.message_id))
        });
        hits.truncate(limit as usize);
        Ok(hits)
    }

    async fn search(
        &self,
        scope: &ConversationScope,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ConversationEvidenceHit>, StoreError> {
        let data = self.0.lock().unwrap();
        let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Ok(vec![]);
        }
        let mut rows = Vec::new();
        for m in data.messages.rows(&scope.workspace_id).filter(|m| {
            in_scope(m, scope)
                && message_readable(&data, m)
                && words
                    .iter()
                    .any(|word| m.text_content.to_lowercase().contains(word))
        }) {
            rows.push(ConversationEvidenceHit {
                kind: "message".into(),
                reference: format!("message:{}", m.id),
                message_id: m.id.clone(),
                speaker: speaker(m),
                created_at: m.created_at,
                snippet: m.text_content.clone(),
                complete: true,
            });
        }
        for (reference, t) in data
            .tool_evidence
            .rows(&scope.workspace_id)
            .filter(|(_, t)| {
                &t.scope == scope
                    && tool_readable(&data, t)
                    && words.iter().any(|w| t.content.to_lowercase().contains(w))
            })
        {
            rows.push(ConversationEvidenceHit {
                kind: "tool".into(),
                reference: reference.clone(),
                message_id: t.source_message_id.clone(),
                speaker: t.tool_name.clone(),
                created_at: t.created_at,
                snippet: t.content.clone(),
                complete: t.complete,
            });
        }
        rows.sort_by_key(|r| (r.message_id.clone(), r.reference.clone()));
        rows.retain(|r| {
            after.is_none_or(|after| {
                evidence_cursor(&r.message_id, &r.reference) > cursor_parts(after)
            })
        });
        rows.truncate(limit as usize);
        Ok(rows)
    }
    async fn read_range(
        &self,
        scope: &ConversationScope,
        after_exclusive: Option<&MessageId>,
        through_inclusive: &MessageId,
        limit: u32,
    ) -> Result<Vec<ConversationEvidenceMessage>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .messages
            .rows(&scope.workspace_id)
            .filter(|m| {
                in_scope(m, scope)
                    && &m.id <= through_inclusive
                    && after_exclusive.is_none_or(|after| &m.id > after)
                    && message_readable(&data, m)
            })
            .take(limit as usize)
            .map(|message| evidence_message(&data, message))
            .collect())
    }
    async fn read_message(
        &self,
        scope: &ConversationScope,
        message_id: &MessageId,
    ) -> Result<Option<ConversationEvidenceMessage>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .messages
            .get(&scope.workspace_id, message_id)
            .filter(|m| in_scope(m, scope) && message_readable(&data, m))
            .map(|message| evidence_message(&data, message)))
    }
    async fn retain_tool(&self, evidence: &RetainedToolEvidence) -> Result<String, StoreError> {
        let reference = format!("tool:{}:{}", evidence.run_id, evidence.tool_call_id);
        self.0.lock().unwrap().tool_evidence.put(
            &evidence.scope.workspace_id,
            reference.clone(),
            (reference.clone(), evidence.clone()),
        );
        Ok(reference)
    }
    async fn read_tool(
        &self,
        scope: &ConversationScope,
        reference: &str,
    ) -> Result<Option<ConversationToolEvidence>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .tool_evidence
            .get(&scope.workspace_id, &reference.to_string())
            .filter(|(_, t)| &t.scope == scope && tool_readable(&data, t))
            .map(|(reference, t)| ConversationToolEvidence {
                kind: "tool".into(),
                reference: reference.clone(),
                source_message_id: t.source_message_id.clone(),
                tool_name: t.tool_name.clone(),
                created_at: t.created_at,
                content: t.content.clone(),
                complete: t.complete,
            }))
    }
}

#[async_trait::async_trait]
impl RunJournal for FakeRunJournal {
    async fn checkpoint(
        &self,
        run_id: &RunId,
        turn: usize,
        state: &Value,
    ) -> Result<(), StoreError> {
        self.0.lock().unwrap().journal.insert(
            run_id.clone(),
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
    async fn turn_started(&self, run_id: &RunId, state: &Value) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        journal_entry(&mut data, run_id).pending_turn = Some(state.clone());
        Ok(())
    }
    async fn tool_started(
        &self,
        run_id: &RunId,
        call_id: &str,
        effect_class: &str,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = journal_entry(&mut data, run_id);
        if row.calls.iter().any(|call| call.call_id == call_id) {
            return Err(conflict("tool call exists"));
        }
        row.calls.push(JournalCall {
            call_id: call_id.to_string(),
            effect_class: effect_class.to_string(),
            result: None,
        });
        Ok(())
    }
    async fn tool_ended(
        &self,
        run_id: &RunId,
        call_id: &str,
        result: &Value,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let call = data
            .journal
            .get_mut(run_id)
            .and_then(|row| row.calls.iter_mut().find(|call| call.call_id == call_id))
            .ok_or_else(|| StoreError::Corrupt("tool result has no start".into()))?;
        call.result = Some(result.clone());
        Ok(())
    }
    async fn last(&self, run_id: &RunId) -> Result<Option<RunJournalEntry>, StoreError> {
        Ok(self.0.lock().unwrap().journal.get(run_id).cloned())
    }
    async fn mark_resumed(&self, run_id: &RunId) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .journal
            .get_mut(run_id)
            .filter(|row| row.checkpoint.is_some() && !row.resumed)
        else {
            return Ok(false);
        };
        row.resumed = true;
        Ok(true)
    }
    async fn delete(&self, run_id: &RunId) -> Result<(), StoreError> {
        self.0.lock().unwrap().journal.remove(run_id);
        Ok(())
    }
}

fn insert_message(
    data: &mut Data,
    message: &Message,
    exposures: Option<&[MemoryExposure]>,
) -> Result<SendOutcome, StoreError> {
    if let Some(pending) = &message.pending_id
        && let Some(held) = data
            .messages
            .rows(&message.workspace_id)
            .find(|m| m.channel_id == message.channel_id && m.pending_id.as_ref() == Some(pending))
    {
        return Ok(SendOutcome::Deduplicated(held.clone()));
    }
    data.messages
        .insert(&message.workspace_id, message.id.clone(), message.clone())?;
    if let Some(exposures) = exposures {
        data.message_exposures.put(
            &message.workspace_id,
            message.id.clone(),
            exposures.to_vec(),
        );
    }
    Ok(SendOutcome::Created(message.clone()))
}
fn usage_total<'a>(rows: impl Iterator<Item = &'a UsageRecord>) -> UsageTotal {
    let mut total = UsageTotal::default();
    for row in rows {
        total.input_tokens += row.input_tokens;
        total.output_tokens += row.output_tokens;
        total.cache_read_tokens += row.cache_read_tokens;
        total.cache_write_tokens += row.cache_write_tokens;
        total.cost_usd += row.cost_usd.unwrap_or(0.0);
        total.calls += 1;
        total.unpriced_calls += i64::from(row.cost_usd.is_none());
    }
    total
}
fn in_scope(message: &Message, scope: &ConversationScope) -> bool {
    message.workspace_id == scope.workspace_id
        && message.channel_id == scope.channel_id
        && match &scope.root_message_id {
            Some(root) => &message.id == root || message.parent_message_id.as_ref() == Some(root),
            None => message.parent_message_id.is_none(),
        }
}
fn live_exposure(
    data: &Data,
    workspace: &WorkspaceId,
    agent: &AgentId,
    exposure: &MemoryExposure,
) -> bool {
    data.grants
        .get(workspace, &exposure.grant_id)
        .is_some_and(|g| {
            &g.agent_id == agent && g.revision == exposure.revision && g.revoked_at.is_none()
        })
}
fn message_readable(data: &Data, message: &Message) -> bool {
    !data
        .forgotten_messages
        .contains(&(message.workspace_id.clone(), message.id.clone()))
        && (message.author.person_id().is_some()
            || message.author.agent_id().is_some_and(|agent| {
                data.message_exposures
                    .get(&message.workspace_id, &message.id)
                    .is_some_and(|exposures| {
                        exposures
                            .iter()
                            .all(|e| live_exposure(data, &message.workspace_id, agent, e))
                    })
            }))
}
fn tool_readable(data: &Data, tool: &RetainedToolEvidence) -> bool {
    !data.forgotten_messages.contains(&(
        tool.scope.workspace_id.clone(),
        tool.source_message_id.clone(),
    )) && live_exposure(
        data,
        &tool.scope.workspace_id,
        &tool.scope.agent_id,
        &tool.exposure,
    )
}
fn speaker(message: &Message) -> String {
    message
        .author
        .agent_id()
        .as_ref()
        .map(|a| format!("agent:{a}"))
        .unwrap_or_else(|| message.author.kind().to_string())
}
fn evidence_message(data: &Data, message: &Message) -> ConversationEvidenceMessage {
    ConversationEvidenceMessage {
        reference: format!("message:{}", message.id),
        message_id: message.id.clone(),
        speaker: speaker(message),
        created_at: message.created_at,
        text: message.text_content.clone(),
        files: message
            .blocks
            .iter()
            .filter_map(|block| {
                let id = match block {
                    Block::Known(KnownBlock::Image { file_id, .. })
                    | Block::Known(KnownBlock::File { file_id, .. }) => file_id,
                    _ => return None,
                };
                let file = data
                    .files
                    .get(&message.workspace_id, &FileId::from(id.clone()));
                Some(EvidenceFileRef {
                    reference: format!("file:{id}"),
                    available: file.is_some(),
                    filename: file.and_then(|row| row.filename.clone()),
                    mime: file.map(|row| row.mime.clone()),
                    size_bytes: file.map(|row| row.size_bytes),
                })
            })
            .collect(),
    }
}
fn evidence_cursor(id: &MessageId, reference: &str) -> (String, String) {
    (id.as_str().to_string(), reference.to_string())
}
fn cursor_parts(reference: &str) -> (String, String) {
    let id = reference
        .strip_prefix("message:")
        .or_else(|| reference.strip_prefix("tool:"))
        .unwrap_or(reference)
        .split(':')
        .next()
        .unwrap_or("");
    (id.to_string(), reference.to_string())
}
fn journal_entry<'a>(data: &'a mut Data, run: &RunId) -> &'a mut RunJournalEntry {
    data.journal
        .entry(run.clone())
        .or_insert_with(|| RunJournalEntry {
            checkpoint: None,
            pending_turn: None,
            calls: vec![],
            resumed: false,
        })
}
