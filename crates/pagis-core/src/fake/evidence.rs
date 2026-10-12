use super::*;

#[async_trait::async_trait]
impl BriefStore for FakeBriefStore {
    async fn load(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &crate::ChannelId,
        root_message_id: Option<&crate::MessageId>,
    ) -> Result<BriefCursor, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .briefs
            .get(
                workspace_id,
                &(
                    agent_id.clone(),
                    channel_id.clone(),
                    root_message_id.cloned(),
                ),
            )
            .cloned()
            .unwrap_or_default())
    }
    async fn save(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &crate::ChannelId,
        root_message_id: Option<&crate::MessageId>,
        cursor: &BriefCursor,
    ) -> Result<(), StoreError> {
        self.0.lock().unwrap().briefs.put(
            workspace_id,
            (
                agent_id.clone(),
                channel_id.clone(),
                root_message_id.cloned(),
            ),
            cursor.clone(),
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl ContinuationStore for FakeContinuationStore {
    async fn get(
        &self,
        key: &ContinuationKey,
    ) -> Result<Option<ContinuationCheckpoint>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .continuations
            .get(
                &key.workspace_id,
                &(
                    key.agent_id.clone(),
                    key.channel_id.clone(),
                    key.root_message_id.clone(),
                ),
            )
            .cloned())
    }
    async fn replace(
        &self,
        checkpoint: &ContinuationCheckpoint,
        expected_revision: Option<u32>,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let key = (
            checkpoint.key.agent_id.clone(),
            checkpoint.key.channel_id.clone(),
            checkpoint.key.root_message_id.clone(),
        );
        if data
            .continuations
            .get(&checkpoint.key.workspace_id, &key)
            .map(|row| row.revision)
            != expected_revision
        {
            return Ok(false);
        }
        data.continuations
            .put(&checkpoint.key.workspace_id, key, checkpoint.clone());
        Ok(true)
    }
}

#[async_trait::async_trait]
impl PendingEvidenceStore for FakePendingEvidenceStore {
    async fn record(
        &self,
        evidence: crate::PendingEvidence,
    ) -> Result<Option<crate::PendingEvidenceRecord>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut evidence = evidence;
        let key = review_key(&evidence);
        if let Some(cursor) = data.review_cursors.get(&evidence.workspace_id, &key) {
            evidence.source_message_ids.retain(|id| id > cursor);
            let Some(through) = evidence.source_message_ids.iter().max().cloned() else {
                return Ok(None);
            };
            evidence.after_exclusive = Some(cursor.clone());
            evidence.through_inclusive = through;
        }
        if evidence.source_message_ids.is_empty() {
            return Ok(None);
        }
        let held = data
            .pending_evidence
            .rows(&evidence.workspace_id)
            .find(|row| {
                row.agent_id == evidence.agent_id
                    && row.subject == evidence.subject
                    && row.state == PendingEvidenceState::Pending
            })
            .map(|row| row.id.clone());
        if let Some(id) = held {
            let row = data
                .pending_evidence
                .get_mut(&evidence.workspace_id, &id)
                .unwrap();
            row.evidence.after_exclusive = row
                .after_exclusive
                .clone()
                .zip(evidence.after_exclusive.clone())
                .map(|(a, b)| a.min(b));
            row.evidence.through_inclusive = row
                .through_inclusive
                .clone()
                .max(evidence.through_inclusive);
            for id in evidence.source_message_ids {
                if !row.evidence.source_message_ids.contains(&id) {
                    row.evidence.source_message_ids.push(id);
                }
            }
            for exposure in evidence.exposures {
                if !row.evidence.exposures.contains(&exposure) {
                    row.evidence.exposures.push(exposure);
                }
            }
            if row.reason != evidence.reason {
                row.evidence
                    .reason
                    .push_str(&format!("\n{}", evidence.reason));
            }
            if evidence.urgency == PendingUrgency::Urgent {
                row.evidence.urgency = PendingUrgency::Urgent;
            }
            row.eligible_at = evidence.created_at
                + if row.urgency == PendingUrgency::Urgent {
                    0
                } else {
                    300_000
                };
            row.maximum_due_at = row.maximum_due_at.min(
                evidence.created_at
                    + if row.urgency == PendingUrgency::Urgent {
                        60_000
                    } else {
                        1_800_000
                    },
            );
            row.revision += 1;
            return Ok(Some(row.clone()));
        }
        let maximum_due_at = evidence.created_at
            + if evidence.urgency == PendingUrgency::Urgent {
                60_000
            } else {
                1_800_000
            };
        let eligible_at = evidence.created_at
            + if evidence.urgency == PendingUrgency::Urgent {
                0
            } else {
                300_000
            };
        let failed_overlap_id = data
            .pending_evidence
            .rows(&evidence.workspace_id)
            .filter(|row| {
                row.agent_id == evidence.agent_id
                    && row.subject == evidence.subject
                    && row.state == PendingEvidenceState::Failed
            })
            .max_by_key(|row| row.created_at)
            .map(|row| row.id.clone());
        let row = PendingEvidenceRecord {
            id: PendingEvidenceId::generate(),
            evidence,
            eligible_at,
            maximum_due_at,
            attempt_count: 0,
            state: PendingEvidenceState::Pending,
            revision: 1,
            lease_run_id: None,
            error: None,
            memory_revision: None,
            completed_at: None,
            failed_overlap_id,
        };
        data.pending_evidence
            .put(&row.workspace_id, row.id.clone(), row.clone());
        Ok(Some(row))
    }
    async fn review_cursor(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &ChannelId,
        root_message_id: Option<&MessageId>,
        subject: &str,
    ) -> Result<Option<MessageId>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .review_cursors
            .get(
                workspace_id,
                &(
                    agent_id.clone(),
                    channel_id.clone(),
                    root_message_id.cloned(),
                    subject.to_string(),
                ),
            )
            .cloned())
    }
    async fn settle_compaction(
        &self,
        evidence: &crate::PendingEvidence,
        memory_revision: Option<&str>,
        now: UnixMillis,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        data.review_cursors.put(
            &evidence.workspace_id,
            review_key(evidence),
            evidence.through_inclusive.clone(),
        );
        for row in data
            .pending_evidence
            .rows_mut(&evidence.workspace_id)
            .filter(|row| {
                row.agent_id == evidence.agent_id
                    && row.state == PendingEvidenceState::Pending
                    && row
                        .source_message_ids
                        .iter()
                        .all(|id| evidence.source_message_ids.contains(id))
            })
        {
            row.state = PendingEvidenceState::Completed;
            row.memory_revision = memory_revision.map(str::to_string);
            row.completed_at = Some(now);
            row.revision += 1;
        }
        Ok(())
    }
    async fn next_due_at(&self) -> Result<Option<UnixMillis>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .pending_evidence
            .0
            .values()
            .filter_map(|row| match row.state {
                PendingEvidenceState::Pending => Some(row.eligible_at.min(row.maximum_due_at)),
                PendingEvidenceState::Leased => {
                    data.review_leases.get(&row.workspace_id, &row.id).copied()
                }
                _ => None,
            })
            .min())
    }
    async fn pending_agents(
        &self,
        now: UnixMillis,
    ) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError> {
        let data = self.0.lock().unwrap();
        Ok(data
            .pending_evidence
            .0
            .values()
            .filter(|row| pending_due(&data, row, now))
            .map(|row| (row.workspace_id.clone(), row.agent_id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
    async fn claim(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        arrival_slots: u32,
        now: UnixMillis,
    ) -> Result<Vec<crate::PendingReviewClaim>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let ids: Vec<_> = data
            .pending_evidence
            .rows(workspace_id)
            .filter(|row| &row.agent_id == agent_id && pending_due(&data, row, now))
            .map(|row| row.id.clone())
            .collect();
        let mut claims = Vec::new();
        for id in ids {
            if claims.len() >= arrival_slots as usize {
                break;
            }
            let row = data.pending_evidence.get_mut(workspace_id, &id).unwrap();
            if row.attempt_count >= 3 {
                row.state = PendingEvidenceState::Failed;
                continue;
            }
            let mut run = queued_run(
                workspace_id,
                agent_id,
                Some(row.channel_id.clone()),
                row.root_message_id.clone(),
                TriggerKind::Review,
                Some(id.to_string()),
                now,
            );
            run.title = row.subject.clone();
            row.state = PendingEvidenceState::Leased;
            row.lease_run_id = Some(run.id.clone());
            row.attempt_count += 1;
            row.revision += 1;
            let evidence = row.clone();
            data.review_leases.put(workspace_id, id, now + 600_000);
            data.runs.put(workspace_id, run.id.clone(), run.clone());
            claims.push(PendingReviewClaim {
                lease_revision: evidence.revision,
                evidence,
                run,
            });
        }
        Ok(claims)
    }
    async fn for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<crate::PendingEvidenceRecord>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .pending_evidence
            .rows(workspace_id)
            .find(|row| row.lease_run_id.as_ref() == Some(run_id))
            .cloned())
    }
    async fn leased(&self) -> Result<Vec<crate::PendingEvidenceRecord>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .pending_evidence
            .0
            .values()
            .filter(|row| row.state == PendingEvidenceState::Leased)
            .cloned()
            .collect())
    }
    async fn complete(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        memory_revision: Option<&str>,
        now: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.pending_evidence.rows_mut(workspace_id).find(|row| {
            row.lease_run_id.as_ref() == Some(run_id)
                && row.state == PendingEvidenceState::Leased
                && row.revision == lease_revision
        }) else {
            return Ok(false);
        };
        row.state = PendingEvidenceState::Completed;
        row.memory_revision = memory_revision.map(str::to_string);
        row.completed_at = Some(now);
        row.revision += 1;
        let key = review_key(&row.evidence);
        let through = row.through_inclusive.clone();
        data.review_cursors.put(workspace_id, key, through);
        Ok(true)
    }
    async fn fail(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        error: &str,
        now: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.pending_evidence.rows_mut(workspace_id).find(|row| {
            row.lease_run_id.as_ref() == Some(run_id)
                && row.state == PendingEvidenceState::Leased
                && row.revision == lease_revision
        }) else {
            return Ok(false);
        };
        row.state = if row.attempt_count >= 3 {
            PendingEvidenceState::Failed
        } else {
            PendingEvidenceState::Pending
        };
        row.eligible_at = now
            + if row.attempt_count == 1 {
                60_000
            } else {
                300_000
            };
        row.error = Some(error.to_string());
        row.revision += 1;
        Ok(true)
    }
    async fn invalidate(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        now: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.pending_evidence.rows_mut(workspace_id).find(|row| {
            row.lease_run_id.as_ref() == Some(run_id)
                && row.state == PendingEvidenceState::Leased
                && row.revision == lease_revision
        }) else {
            return Ok(false);
        };
        row.state = PendingEvidenceState::Invalidated;
        row.completed_at = Some(now);
        row.revision += 1;
        Ok(true)
    }
    async fn retry(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::PendingEvidenceId,
        now: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .pending_evidence
            .get_mut(workspace_id, id)
            .filter(|row| row.state == PendingEvidenceState::Failed)
        else {
            return Ok(false);
        };
        row.state = PendingEvidenceState::Pending;
        row.attempt_count = 0;
        row.error = None;
        row.eligible_at = now;
        row.maximum_due_at = now;
        row.lease_run_id = None;
        row.revision += 1;
        Ok(true)
    }
}

fn review_key(e: &PendingEvidence) -> (AgentId, ChannelId, Option<MessageId>, String) {
    (
        e.agent_id.clone(),
        e.channel_id.clone(),
        e.root_message_id.clone(),
        e.subject.clone(),
    )
}
fn pending_due(data: &Data, row: &PendingEvidenceRecord, now: i64) -> bool {
    match row.state {
        PendingEvidenceState::Pending => row.eligible_at.min(row.maximum_due_at) <= now,
        PendingEvidenceState::Leased => {
            data.review_leases
                .get(&row.workspace_id, &row.id)
                .is_some_and(|at| *at <= now)
                && row.lease_run_id.as_ref().is_some_and(|id| {
                    data.runs
                        .get(&row.workspace_id, id)
                        .is_some_and(|run| run.state.is_terminal())
                })
        }
        _ => false,
    }
}
