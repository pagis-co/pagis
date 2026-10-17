use super::*;

#[async_trait::async_trait]
impl ScheduleStore for FakeScheduleStore {
    async fn create(&self, schedule: &Schedule) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        data.schedules.insert(
            &schedule.workspace_id,
            schedule.id.clone(),
            schedule.clone(),
        )?;
        data.schedule_revisions.put(
            &schedule.workspace_id,
            (schedule.id.clone(), schedule.revision),
            schedule_revision(schedule),
        );
        Ok(())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Option<Schedule>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .schedules
            .get(workspace_id, id)
            .cloned())
    }
    async fn edit(
        &self,
        previous: &Schedule,
        replacement: &Schedule,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut data = self.0.lock().unwrap();
        if previous.workspace_id != replacement.workspace_id
            || previous.id != replacement.id
            || data
                .schedules
                .get(&previous.workspace_id, &previous.id)
                .is_none_or(|row| {
                    row.revision != previous.revision || row.state == ScheduleState::Archived
                })
        {
            return Ok(None);
        }
        data.schedules.put(
            &replacement.workspace_id,
            replacement.id.clone(),
            replacement.clone(),
        );
        data.schedule_revisions.put(
            &replacement.workspace_id,
            (replacement.id.clone(), replacement.revision),
            schedule_revision(replacement),
        );
        withdraw(
            &mut data,
            &replacement.workspace_id,
            &WakeupRule::Schedule {
                schedule_id: replacement.id.clone(),
            },
        );
        Ok(Some(replacement.clone()))
    }
    async fn list_revisions(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Vec<ScheduleRevision>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .schedule_revisions
            .rows(workspace_id)
            .filter(|row| &row.schedule_id == id)
            .cloned()
            .collect())
    }
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        before: Option<&ScheduleId>,
        limit: u32,
    ) -> Result<Vec<Schedule>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .schedules
                .rows(workspace_id)
                .map(|r| (r.id.clone(), r.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn list_open_for_subject_page(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<Vec<crate::subject_page::OpenSchedule>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .schedules
            .rows(workspace_id)
            .filter(|row| {
                &row.agent_id == agent_id
                    && row.subject_page_path.as_deref() == Some(path)
                    && matches!(
                        row.state,
                        ScheduleState::Active | ScheduleState::Paused | ScheduleState::Blocked
                    )
            })
            .filter_map(|row| {
                row.next_due_at.map(|at| crate::subject_page::OpenSchedule {
                    id: row.id.to_string(),
                    next_due_at: at,
                    purpose: row.instruction.clone(),
                })
            })
            .collect())
    }
    async fn archive(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .schedules
            .get_mut(workspace_id, id)
            .filter(|row| row.state != ScheduleState::Archived)
        else {
            return Ok(None);
        };
        row.state = ScheduleState::Archived;
        row.archived_at = Some(at);
        row.next_due_at = None;
        row.updated_at = at;
        let row = row.clone();
        withdraw(
            &mut data,
            workspace_id,
            &WakeupRule::Schedule {
                schedule_id: id.clone(),
            },
        );
        Ok(Some(row))
    }
    async fn pause(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .schedules
            .get_mut(workspace_id, id)
            .filter(|row| row.state != ScheduleState::Archived)
        else {
            return Ok(None);
        };
        row.state = ScheduleState::Paused;
        row.updated_at = at;
        let row = row.clone();
        withdraw(
            &mut data,
            workspace_id,
            &WakeupRule::Schedule {
                schedule_id: id.clone(),
            },
        );
        Ok(Some(row))
    }
    async fn resume(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        next_due_at: UnixMillis,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .schedules
            .get_mut(workspace_id, id)
            .filter(|row| row.state != ScheduleState::Archived)
        else {
            return Ok(None);
        };
        row.state = ScheduleState::Active;
        row.next_due_at = Some(next_due_at);
        row.updated_at = at;
        let row = row.clone();
        Ok(Some(row))
    }
    async fn skip_next(
        &self,
        schedule: &Schedule,
        expected_due_at: UnixMillis,
        next_due_at: Option<UnixMillis>,
        at: UnixMillis,
    ) -> Result<ScheduleOccurrence, StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = data
            .schedules
            .get_mut(&schedule.workspace_id, &schedule.id)
            .filter(|row| {
                row.revision == schedule.revision
                    && row.state == ScheduleState::Active
                    && row.next_due_at == Some(expected_due_at)
            })
            .ok_or_else(|| conflict("Schedule changed"))?;
        row.next_due_at = next_due_at;
        row.updated_at = at;
        if next_due_at.is_none() {
            row.state = ScheduleState::Completed;
        }
        let occurrence = ScheduleOccurrence {
            id: ScheduleOccurrenceId::generate(),
            workspace_id: schedule.workspace_id.clone(),
            schedule_id: schedule.id.clone(),
            schedule_revision: schedule.revision,
            scheduled_at: expected_due_at,
            processed_at: at,
            outcome: "skipped".into(),
            wakeup_id: None,
        };
        data.occurrences.put(
            &schedule.workspace_id,
            occurrence.id.clone(),
            occurrence.clone(),
        );
        Ok(occurrence)
    }
    async fn list_occurrences(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&ScheduleOccurrenceId>,
        limit: u32,
    ) -> Result<Vec<ScheduleOccurrence>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .occurrences
                .rows(workspace_id)
                .filter(|row| &row.schedule_id == schedule_id)
                .map(|row| (row.id.clone(), row.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .wakeups
                .rows(workspace_id)
                .filter(|row| {
                    row.rule
                        == WakeupRule::Schedule {
                            schedule_id: schedule_id.clone(),
                        }
                })
                .map(|row| (row.id.clone(), row.clone()))
                .collect(),
            before,
            limit,
        ))
    }
}

#[async_trait::async_trait]
impl TriggerStore for FakeTriggerStore {
    async fn next_due_at(&self) -> Result<Option<UnixMillis>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .schedules
            .0
            .values()
            .filter(|row| row.state == ScheduleState::Active)
            .filter_map(|row| row.next_due_at)
            .min())
    }
    async fn pending_agents(&self) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .wakeups
            .0
            .values()
            .filter(|row| row.state == WakeupState::Pending)
            .map(|row| (row.workspace_id.clone(), row.agent_id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
    async fn process_due(&self, now: UnixMillis) -> Result<DueBatch, StoreError> {
        let mut data = self.0.lock().unwrap();
        let schedules: Vec<_> = data
            .schedules
            .0
            .values()
            .filter(|row| {
                row.state == ScheduleState::Active && row.next_due_at.is_some_and(|at| at <= now)
            })
            .cloned()
            .collect();
        let mut batch = DueBatch {
            blocked_schedules: vec![],
            withdrawn_wakeups: vec![],
            occurrences: vec![],
            wakeups: vec![],
        };
        let windows: Vec<_> = schedules
            .iter()
            .map(|schedule| crate::schedule_window(schedule, now).map_err(StoreError::from))
            .collect::<Result<_, _>>()?;
        for (schedule, (scheduled_at, next)) in schedules.into_iter().zip(windows) {
            let scheduled_at = scheduled_at.expect("due Schedule has a due time");
            if matches!(schedule.created_by, Author::Agent(_))
                && schedule.approved_revision != Some(schedule.revision)
            {
                let row = data
                    .schedules
                    .get_mut(&schedule.workspace_id, &schedule.id)
                    .unwrap();
                row.state = ScheduleState::Blocked;
                row.next_due_at = None;
                batch.blocked_schedules.push(row.clone());
                batch.withdrawn_wakeups.extend(withdraw(
                    &mut data,
                    &schedule.workspace_id,
                    &WakeupRule::Schedule {
                        schedule_id: schedule.id.clone(),
                    },
                ));
                continue;
            }
            let wakeup = schedule_wakeup(&mut data, &schedule, scheduled_at, now);
            let occurrence = ScheduleOccurrence {
                id: ScheduleOccurrenceId::generate(),
                workspace_id: schedule.workspace_id.clone(),
                schedule_id: schedule.id.clone(),
                schedule_revision: schedule.revision,
                scheduled_at,
                processed_at: now,
                outcome: "wakeup_created".into(),
                wakeup_id: Some(wakeup.id.clone()),
            };
            data.occurrences.put(
                &schedule.workspace_id,
                occurrence.id.clone(),
                occurrence.clone(),
            );
            let row = data
                .schedules
                .get_mut(&schedule.workspace_id, &schedule.id)
                .unwrap();
            row.next_due_at = next;
            row.updated_at = now;
            if next.is_none() {
                row.state = ScheduleState::Completed;
            }
            batch.occurrences.push(occurrence);
            batch.wakeups.push(wakeup);
        }
        Ok(batch)
    }
    async fn wake_now(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        now: UnixMillis,
    ) -> Result<Option<Wakeup>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(schedule) = data
            .schedules
            .get(workspace_id, schedule_id)
            .filter(|row| row.state == ScheduleState::Active)
            .cloned()
        else {
            return Ok(None);
        };
        Ok(Some(schedule_wakeup(&mut data, &schedule, now, now)))
    }
    async fn claim_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        slots: RunSlots,
        now: UnixMillis,
    ) -> Result<Vec<WakeupClaim>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut slots = slots;
        let mut rows: Vec<_> = data
            .wakeups
            .rows(workspace_id)
            .filter(|row| &row.agent_id == agent_id && row.state == WakeupState::Pending)
            .cloned()
            .collect();
        rows.sort_by_key(|row| (row.scheduled_at, row.id.clone()));
        let mut claims = Vec::new();
        for wakeup in rows {
            let slot = if matches!(wakeup.rule, WakeupRule::Arrival { .. }) {
                &mut slots.arrival
            } else {
                &mut slots.conversation
            };
            if *slot == 0 {
                continue;
            }
            *slot -= 1;
            let mut run = queued_run(
                workspace_id,
                agent_id,
                wakeup.channel_id.clone(),
                wakeup.root_message_id.clone(),
                wakeup.rule.trigger_kind(),
                Some(wakeup.id.to_string()),
                now,
            );
            run.title = wakeup.rule_name.clone();
            let row = data.wakeups.get_mut(workspace_id, &wakeup.id).unwrap();
            row.state = WakeupState::Started;
            row.run_id = Some(run.id.clone());
            row.started_at = Some(now);
            let wakeup = row.clone();
            data.runs.put(workspace_id, run.id.clone(), run.clone());
            claims.push(WakeupClaim { wakeup, run });
        }
        Ok(claims)
    }
    async fn get_wakeup(
        &self,
        workspace_id: &WorkspaceId,
        id: &WakeupId,
    ) -> Result<Option<Wakeup>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .wakeups
            .get(workspace_id, id)
            .cloned())
    }
    async fn ingest(
        &self,
        batch: IngestBatch,
        eligible: &[EventSubscription],
        matcher: &dyn EventMatcher,
        arrival: Option<&ArrivalRun>,
        source_resource: Option<&str>,
        keys: &dyn crate::ForgetKeys,
    ) -> Result<IngestOutcome, StoreError> {
        let mut data = self.0.lock().unwrap();
        let mut outcome = IngestOutcome {
            batch: SourceBatch {
                id: SourceBatchId::generate(),
                workspace_id: batch.workspace_id.clone(),
                source: batch.source.clone(),
                event_kind: batch.event_kind.clone(),
                collected_at: batch.received_at,
                collected_count: batch.events.len() as u32,
                stored_count: 0,
                wakeup_count: 0,
                outcome: if batch.baseline {
                    SourceBatchOutcome::Baseline
                } else {
                    SourceBatchOutcome::Collected
                },
                detail: None,
            },
            events: vec![],
            created: vec![],
            combined: vec![],
        };
        let mut accepted = Vec::new();
        if !batch.baseline {
            for event in &batch.events {
                if let (Some(resource), Some(connection_id)) =
                    (source_resource, batch.source.connection_id())
                {
                    let source = SourceKey {
                        workspace_id: batch.workspace_id.clone(),
                        connection_id: connection_id.clone(),
                        resource: resource.to_string(),
                    };
                    if super::forget::suppressed(&data, &source, &event.provider_event_id, keys)? {
                        continue;
                    }
                }
                if data.incoming_events.rows(&batch.workspace_id).any(|row| {
                    row.source == batch.source
                        && row.event_kind == batch.event_kind
                        && row.provider_event_id == event.provider_event_id
                }) {
                    continue;
                }
                accepted.push(event.clone());
            }
        }
        for event in accepted {
            let row = IncomingEvent {
                id: IncomingEventId::generate(),
                workspace_id: batch.workspace_id.clone(),
                source: batch.source.clone(),
                event_kind: batch.event_kind.clone(),
                provider_event_id: event.provider_event_id,
                metadata: event.metadata,
                occurred_at: event.occurred_at,
                received_at: batch.received_at,
                batch_id: outcome.batch.id.clone(),
            };
            data.incoming_events
                .put(&batch.workspace_id, row.id.clone(), row.clone());
            for subscription in eligible.iter().filter(|s| {
                s.workspace_id == batch.workspace_id
                    && s.source == batch.source
                    && s.event_kind == batch.event_kind
                    && s.state == EventSubscriptionState::Active
                    && batch.agent_id.as_ref().is_none_or(|id| &s.agent_id == id)
                    && matcher.matches(&s.filter, &row.metadata)
            }) {
                let rule = WakeupRule::EventSubscription {
                    subscription_id: subscription.id.clone(),
                };
                let landing = event.landing.clone().unwrap_or(WakeupLanding {
                    channel_id: subscription.channel_id.clone(),
                    root_message_id: subscription.root_message_id.clone(),
                });
                let existing = data
                    .wakeups
                    .rows(&batch.workspace_id)
                    .find(|w| {
                        w.rule == rule
                            && w.state == WakeupState::Pending
                            && w.channel_id.as_ref() == Some(&landing.channel_id)
                            && w.root_message_id == landing.root_message_id
                    })
                    .map(|w| w.id.clone());
                let mut wakeup = existing
                    .as_ref()
                    .and_then(|id| data.wakeups.get(&batch.workspace_id, id))
                    .cloned()
                    .unwrap_or_else(|| {
                        new_wakeup(
                            &batch.workspace_id,
                            &subscription.agent_id,
                            rule,
                            subscription.revision,
                            &subscription.name,
                            &subscription.instruction,
                            Some(landing.channel_id),
                            landing.root_message_id,
                            batch.received_at,
                        )
                    });
                if existing.is_some() {
                    wakeup.source_count += 1;
                }
                data.wakeups
                    .put(&batch.workspace_id, wakeup.id.clone(), wakeup.clone());
                data.wakeup_events
                    .0
                    .entry((batch.workspace_id.clone(), wakeup.id.clone()))
                    .or_default()
                    .push(row.id.clone());
                data.subscription_events
                    .0
                    .entry((batch.workspace_id.clone(), subscription.id.clone()))
                    .or_default()
                    .push(row.id.clone());
                if existing.is_some() {
                    outcome.combined.push(wakeup);
                } else {
                    outcome.created.push(wakeup);
                }
            }
            outcome.events.push(row);
        }
        if let Some(arrival) = arrival
            && !arrival.subject_paths.is_empty()
        {
            let wakeup = new_wakeup(
                &batch.workspace_id,
                &arrival.agent_id,
                WakeupRule::Arrival {
                    subject_paths: arrival.subject_paths.clone(),
                    historical: arrival.historical,
                },
                1,
                "Arrival",
                "",
                None,
                None,
                batch.received_at,
            );
            data.wakeups
                .put(&batch.workspace_id, wakeup.id.clone(), wakeup.clone());
            outcome.created.push(wakeup);
        }
        if let (Some(connection), Some(cursor)) = (batch.source.connection_id(), batch.cursor) {
            data.collector_cursors.put(
                &batch.workspace_id,
                (connection.clone(), batch.event_kind),
                cursor,
            );
        }
        outcome.batch.stored_count = outcome.events.len() as u32;
        outcome.batch.wakeup_count = outcome.created.len() as u32;
        data.batches.put(
            &batch.workspace_id,
            outcome.batch.id.clone(),
            outcome.batch.clone(),
        );
        Ok(outcome)
    }
    async fn cursor(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<Option<String>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .collector_cursors
            .get(
                workspace_id,
                &(connection_id.clone(), event_kind.to_string()),
            )
            .cloned())
    }
    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
        code: &str,
        at: UnixMillis,
    ) -> Result<SourceBatch, StoreError> {
        let batch = SourceBatch {
            id: SourceBatchId::generate(),
            workspace_id: workspace_id.clone(),
            source: EventSource::connection(connection_id.clone()),
            event_kind: event_kind.to_string(),
            collected_at: at,
            collected_count: 0,
            stored_count: 0,
            wakeup_count: 0,
            outcome: SourceBatchOutcome::Failed,
            detail: Some(code.to_string()),
        };
        self.0
            .lock()
            .unwrap()
            .batches
            .put(workspace_id, batch.id.clone(), batch.clone());
        Ok(batch)
    }
    async fn event_context(
        &self,
        workspace_id: &WorkspaceId,
        wakeup_id: &WakeupId,
    ) -> Result<Option<EventWakeupContext>, StoreError> {
        let data = self.0.lock().unwrap();
        let Some(wakeup) = data.wakeups.get(workspace_id, wakeup_id) else {
            return Ok(None);
        };
        let WakeupRule::EventSubscription { subscription_id } = &wakeup.rule else {
            return Ok(None);
        };
        let Some(subscription) = data.subscriptions.get(workspace_id, subscription_id) else {
            return Ok(None);
        };
        let event_ids = data.wakeup_events.get(workspace_id, wakeup_id);
        let events = data
            .incoming_events
            .rows(workspace_id)
            .filter(|row| event_ids.is_some_and(|ids| ids.contains(&row.id)))
            .cloned()
            .collect();
        let connection_alias = subscription
            .source
            .connection_id()
            .and_then(|id| data.connections.get(workspace_id, id))
            .map(|row| row.alias.clone());
        Ok(Some(EventWakeupContext {
            subscription_id: subscription_id.clone(),
            source: subscription.source.clone(),
            connection_alias,
            event_kind: subscription.event_kind.clone(),
            events,
        }))
    }
}

#[async_trait::async_trait]
impl EventSubscriptionStore for FakeEventSubscriptionStore {
    async fn create(&self, subscription: &EventSubscription) -> Result<(), StoreError> {
        self.0.lock().unwrap().subscriptions.insert(
            &subscription.workspace_id,
            subscription.id.clone(),
            subscription.clone(),
        )
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
    ) -> Result<Option<EventSubscription>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .subscriptions
            .get(workspace_id, id)
            .cloned())
    }
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        before: Option<&EventSubscriptionId>,
        limit: u32,
    ) -> Result<Vec<EventSubscription>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .subscriptions
                .rows(workspace_id)
                .filter(|row| agent_id.is_none_or(|id| &row.agent_id == id))
                .map(|row| (row.id.clone(), row.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn update(&self, subscription: &EventSubscription) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .subscriptions
            .get_mut(&subscription.workspace_id, &subscription.id)
        else {
            return Ok(false);
        };
        *row = subscription.clone();
        Ok(true)
    }
    async fn update_and_withdraw(
        &self,
        subscription: &EventSubscription,
    ) -> Result<Vec<Wakeup>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .subscriptions
            .get_mut(&subscription.workspace_id, &subscription.id)
        else {
            return Ok(vec![]);
        };
        *row = subscription.clone();
        Ok(withdraw(
            &mut data,
            &subscription.workspace_id,
            &WakeupRule::EventSubscription {
                subscription_id: subscription.id.clone(),
            },
        ))
    }
    async fn collector_targets(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<CollectorTarget>, StoreError> {
        let data = self.0.lock().unwrap();
        let mut targets = BTreeMap::new();
        for subscription in data
            .subscriptions
            .rows(workspace_id)
            .filter(|row| row.state == EventSubscriptionState::Active)
        {
            if let Some(connection) = subscription.source.connection_id() {
                let key = (connection.clone(), subscription.event_kind.clone());
                targets.entry(key.clone()).or_insert_with(|| {
                    data.collector_backoffs
                        .get(workspace_id, &key)
                        .cloned()
                        .unwrap_or_else(|| CollectorTarget {
                            failures: 0,
                            next_attempt_at: 0,
                            workspace_id: workspace_id.clone(),
                            connection_id: connection.clone(),
                            event_kind: subscription.event_kind.clone(),
                        })
                });
            }
        }
        Ok(targets.into_values().collect())
    }
    async fn set_collector_backoff(
        &self,
        target: &CollectorTarget,
        failures: u32,
        next_attempt_at: UnixMillis,
    ) -> Result<(), StoreError> {
        let mut row = target.clone();
        row.failures = failures;
        row.next_attempt_at = next_attempt_at;
        self.0.lock().unwrap().collector_backoffs.put(
            &target.workspace_id,
            (target.connection_id.clone(), target.event_kind.clone()),
            row,
        );
        Ok(())
    }
    async fn live_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &crate::EventSource,
        event_kind: &str,
    ) -> Result<Vec<EventSubscription>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .subscriptions
            .rows(workspace_id)
            .filter(|row| {
                &row.source == source
                    && row.event_kind == event_kind
                    && row.state == EventSubscriptionState::Active
            })
            .cloned()
            .collect())
    }
    async fn system_for_source(
        &self,
        workspace_id: &WorkspaceId,
        agent: &AgentId,
        source: &crate::EventSource,
        event_kind: &str,
        name: &str,
    ) -> Result<Option<EventSubscription>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .subscriptions
            .rows(workspace_id)
            .find(|row| {
                &row.agent_id == agent
                    && &row.source == source
                    && row.event_kind == event_kind
                    && row.name == name
                    && row.created_by == crate::Author::System
                    && row.state == EventSubscriptionState::Active
                    && row.approved_revision == Some(row.revision)
            })
            .cloned())
    }
    async fn list_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &crate::EventSource,
        states: &[&str],
    ) -> Result<Vec<EventSubscription>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .subscriptions
            .rows(workspace_id)
            .filter(|row| {
                &row.source == source && (states.is_empty() || states.contains(&row.state.as_str()))
            })
            .cloned()
            .collect())
    }
    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        subscription_id: &EventSubscriptionId,
        before: Option<&IncomingEventId>,
        limit: u32,
    ) -> Result<Vec<IncomingEvent>, StoreError> {
        let data = self.0.lock().unwrap();
        let ids = data.subscription_events.get(workspace_id, subscription_id);
        Ok(page(
            data.incoming_events
                .rows(workspace_id)
                .filter(|row| ids.is_some_and(|ids| ids.contains(&row.id)))
                .map(|row| (row.id.clone(), row.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        subscription_id: &EventSubscriptionId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .wakeups
                .rows(workspace_id)
                .filter(|row| {
                    row.rule
                        == WakeupRule::EventSubscription {
                            subscription_id: subscription_id.clone(),
                        }
                })
                .map(|row| (row.id.clone(), row.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn collector_health(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<(Option<SourceBatch>, Option<SourceBatch>), StoreError> {
        let data = self.0.lock().unwrap();
        let rows: Vec<_> = data
            .batches
            .rows(workspace_id)
            .filter(|row| {
                row.source.connection_id() == Some(connection_id) && row.event_kind == event_kind
            })
            .collect();
        Ok((
            rows.iter()
                .filter(|row| row.outcome != SourceBatchOutcome::Failed)
                .max_by_key(|row| row.collected_at)
                .map(|row| (*row).clone()),
            rows.iter()
                .filter(|row| row.outcome == SourceBatchOutcome::Failed)
                .max_by_key(|row| row.collected_at)
                .map(|row| (*row).clone()),
        ))
    }
}

fn schedule_revision(s: &Schedule) -> ScheduleRevision {
    ScheduleRevision {
        schedule_id: s.id.clone(),
        revision: s.revision,
        agent_id: s.agent_id.clone(),
        name: s.name.clone(),
        instruction: s.instruction.clone(),
        subject_page_path: s.subject_page_path.clone(),
        channel_id: s.channel_id.clone(),
        root_message_id: s.root_message_id.clone(),
        kind: s.kind,
        cron_expression: s.cron_expression.clone(),
        interval_ms: s.interval_ms,
        anchor_at: s.anchor_at,
        timezone: s.timezone.clone(),
        scheduled_at: s.scheduled_at,
        created_at: s.updated_at,
        creating_run_id: s.creating_run_id.clone(),
    }
}
fn withdraw(data: &mut Data, workspace: &WorkspaceId, rule: &WakeupRule) -> Vec<Wakeup> {
    data.wakeups
        .rows_mut(workspace)
        .filter(|w| &w.rule == rule && w.state == WakeupState::Pending)
        .map(|w| {
            w.state = WakeupState::Withdrawn;
            w.clone()
        })
        .collect()
}
#[expect(
    clippy::too_many_arguments,
    reason = "a Wake-up keeps its rule and destination explicit"
)]
fn new_wakeup(
    workspace: &WorkspaceId,
    agent: &AgentId,
    rule: WakeupRule,
    revision: u32,
    name: &str,
    instruction: &str,
    channel: Option<ChannelId>,
    root: Option<MessageId>,
    at: i64,
) -> Wakeup {
    Wakeup {
        id: WakeupId::generate(),
        workspace_id: workspace.clone(),
        rule,
        rule_revision: revision,
        rule_name: name.to_string(),
        agent_id: agent.clone(),
        channel_id: channel,
        root_message_id: root,
        instruction: instruction.to_string(),
        scheduled_at: at,
        state: WakeupState::Pending,
        run_id: None,
        source_count: 1,
        created_at: at,
        started_at: None,
    }
}
fn schedule_wakeup(data: &mut Data, s: &Schedule, due: i64, now: i64) -> Wakeup {
    let rule = WakeupRule::Schedule {
        schedule_id: s.id.clone(),
    };
    if let Some(row) = data
        .wakeups
        .rows_mut(&s.workspace_id)
        .find(|row| row.rule == rule && row.state == WakeupState::Pending)
    {
        row.source_count += 1;
        return row.clone();
    }
    let mut wakeup = new_wakeup(
        &s.workspace_id,
        &s.agent_id,
        rule,
        s.revision,
        &s.name,
        &s.instruction,
        Some(s.channel_id.clone()),
        s.root_message_id.clone(),
        due,
    );
    wakeup.created_at = now;
    data.wakeups
        .put(&s.workspace_id, wakeup.id.clone(), wakeup.clone());
    wakeup
}
