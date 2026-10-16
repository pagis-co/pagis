use super::*;

#[async_trait::async_trait]
impl PhoneNumberStore for FakePhoneNumberStore {
    async fn create_intent(&self, intent: &PurchaseIntent) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .purchase_intents
            .rows(&intent.workspace_id)
            .any(|row| row.state == PurchaseIntentState::Pending)
        {
            return Err(conflict("this Workspace has a pending number purchase"));
        }
        data.purchase_intents
            .insert(&intent.workspace_id, intent.id.clone(), intent.clone())
    }
    async fn list_pending_intents(&self) -> Result<Vec<PurchaseIntent>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .purchase_intents
            .0
            .values()
            .filter(|r| r.state == PurchaseIntentState::Pending)
            .cloned()
            .collect())
    }
    async fn confirm_purchase(
        &self,
        workspace_id: &WorkspaceId,
        intent_id: &PurchaseIntentId,
        number: &PhoneNumber,
        at: UnixMillis,
    ) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if &number.workspace_id != workspace_id
            || data.purchase_intents.get(workspace_id, intent_id).is_none()
        {
            return Err(conflict("purchase intent does not exist"));
        }
        create_number(&mut data, number)?;
        let row = data
            .purchase_intents
            .get_mut(workspace_id, intent_id)
            .unwrap();
        row.state = PurchaseIntentState::Bought;
        row.settled_at = Some(at);
        Ok(())
    }
    async fn create(&self, number: &PhoneNumber) -> Result<(), StoreError> {
        create_number(&mut self.0.lock().unwrap(), number)
    }
    async fn abandon_intent(
        &self,
        workspace_id: &WorkspaceId,
        intent_id: &PurchaseIntentId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .purchase_intents
            .get_mut(workspace_id, intent_id)
            .filter(|row| row.state == PurchaseIntentState::Pending)
        else {
            return Ok(false);
        };
        row.state = PurchaseIntentState::Abandoned;
        row.settled_at = Some(at);
        Ok(true)
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<Option<PhoneNumber>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .phone_numbers
            .get(workspace_id, id)
            .cloned())
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<PhoneNumber>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .phone_numbers
            .rows(workspace_id)
            .cloned()
            .collect())
    }

    async fn live_for_e164(&self, e164: &str) -> Result<Vec<PhoneNumber>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .phone_numbers
            .0
            .values()
            .filter(|r| r.e164 == e164 && r.status != PhoneNumberStatus::Released)
            .cloned()
            .collect())
    }

    async fn release(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .phone_numbers
            .get_mut(workspace_id, id)
            .filter(|row| row.status != PhoneNumberStatus::Released)
        else {
            return Ok(false);
        };
        row.status = PhoneNumberStatus::Released;
        Ok(true)
    }
    async fn any_live_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<bool, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .phone_numbers
            .rows(workspace_id)
            .any(|row| {
                &row.connection_id == connection_id && row.status != PhoneNumberStatus::Released
            }))
    }
    async fn set_outgoing_cap(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        cap: u32,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.outgoing_cap = cap;
        Ok(true)
    }

    async fn note_text_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        at: UnixMillis,
    ) -> Result<u32, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(0);
        };
        Ok(note_send(&mut row.sends_day, &mut row.sends_today, at))
    }
    async fn set_messaging_readiness(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        readiness: &MessagingReadiness,
        at: UnixMillis,
        error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.messaging_readiness = readiness.clone();
        row.messaging_readiness_at = Some(at);
        row.messaging_readiness_error = error.map(str::to_string);
        Ok(true)
    }
    async fn set_text_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        cursor: &str,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.text_cursor = Some(cursor.to_string());
        Ok(true)
    }
    async fn set_messaging_object_id(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        object_id: &str,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.messaging_object_id = Some(object_id.to_string());
        Ok(true)
    }
    async fn set_relay_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        state: &TelnyxRelayState,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.relay_state = state.clone();
        Ok(true)
    }
    async fn set_relay_consent(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
        consent: bool,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.phone_numbers.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.relay_consent = consent;
        Ok(true)
    }
}

#[async_trait::async_trait]
impl AgentMailboxStore for FakeAgentMailboxStore {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .workspace_mailboxes
            .values()
            .any(|row| row.address == mailbox.address)
            || data.agent_mailboxes.0.values().any(|row| {
                row.address == mailbox.address
                    || (row.state.is_live()
                        && row.workspace_id == mailbox.workspace_id
                        && row.agent_id == mailbox.agent_id)
            })
        {
            return Err(conflict("Agent Mailbox exists"));
        }
        data.agent_mailboxes
            .insert(&mailbox.workspace_id, mailbox.id.clone(), mailbox.clone())
    }
    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .agent_mailboxes
            .get(workspace_id, id)
            .is_some_and(|row| row.state == AgentMailboxState::Provisioning)
        {
            data.agent_mailboxes.remove(workspace_id, id);
            return Ok(true);
        }
        Ok(false)
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .agent_mailboxes
            .get(workspace_id, id)
            .cloned())
    }
    async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .agent_mailboxes
            .rows(workspace_id)
            .find(|row| &row.agent_id == agent_id && row.state.is_live())
            .cloned())
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<AgentMailbox>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .agent_mailboxes
            .rows(workspace_id)
            .filter(|row| row.state.is_live())
            .cloned()
            .collect())
    }
    async fn address_taken(&self, address: &str) -> Result<bool, StoreError> {
        let address = address.to_lowercase();
        let data = self.0.lock().unwrap();
        Ok(data
            .workspace_mailboxes
            .values()
            .any(|row| row.address == address)
            || data
                .agent_mailboxes
                .0
                .values()
                .any(|row| row.address == address))
    }
    async fn set_state(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        state: AgentMailboxState,
        reason: Option<&str>,
    ) -> Result<bool, StoreError> {
        if state == AgentMailboxState::Deleted {
            return Err(conflict(
                "a tombstone is written by a delete, not by a state change",
            ));
        }
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .agent_mailboxes
            .get_mut(workspace_id, id)
            .filter(|row| row.state.is_live())
        else {
            return Ok(false);
        };
        row.state = state;
        row.reason = reason.map(str::to_string);
        Ok(true)
    }
    async fn set_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        cursor: &MailboxCursor,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .agent_mailboxes
            .get_mut(workspace_id, id)
            .filter(|row| row.state.is_live())
        else {
            return Ok(false);
        };
        row.cursor = Some(cursor.clone());
        Ok(true)
    }
    async fn make_dormant_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .agent_mailboxes
            .rows_mut(workspace_id)
            .find(|row| &row.agent_id == agent_id && row.state.is_live())
        else {
            return Ok(None);
        };
        row.state = AgentMailboxState::Dormant;
        row.reason = None;
        Ok(Some(row.clone()))
    }
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data
            .agent_mailboxes
            .get_mut(workspace_id, id)
            .filter(|row| row.state.is_live())
        else {
            return Ok(false);
        };
        row.state = AgentMailboxState::Deleted;
        row.deleted_at = Some(at);
        row.reason = None;
        row.cursor = None;
        Ok(true)
    }
    async fn note_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<u32, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.agent_mailboxes.get_mut(workspace_id, id) else {
            return Ok(0);
        };
        if !row.state.is_live() {
            return Ok(row.sends_today);
        }
        Ok(note_send(&mut row.sends_day, &mut row.sends_today, at))
    }
    async fn add_allow_rules(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        rules: &[String],
    ) -> Result<Vec<String>, StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = data
            .agent_mailboxes
            .get_mut(workspace_id, id)
            .filter(|row| row.state.is_live())
            .ok_or_else(|| conflict("that mailbox is gone, so it takes no rules"))?;
        add_rules(&mut row.allow_rules, rules);
        Ok(row.allow_rules.clone())
    }
    async fn count_live_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .agent_mailboxes
            .rows(workspace_id)
            .filter(|row| &row.connection_id == connection_id && row.state.is_live())
            .count() as u32)
    }
}

#[async_trait::async_trait]
impl SentMailStore for FakeSentMailStore {
    async fn record(&self, sent: &SentMail) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .sent_mail
            .get(&sent.workspace_id, &sent.message_id)
            .is_none()
        {
            data.sent_mail
                .put(&sent.workspace_id, sent.message_id.clone(), sent.clone());
        }
        Ok(())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        message_id: &str,
    ) -> Result<Option<SentMail>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .sent_mail
            .get(workspace_id, &message_id.to_string())
            .cloned())
    }
}

#[async_trait::async_trait]
impl CallStore for FakeCallStore {
    async fn insert(&self, call: &Call) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .calls
            .insert(&call.workspace_id, call.id.clone(), call.clone())
    }
    async fn update(&self, call: &Call) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.calls.get_mut(&call.workspace_id, &call.id) else {
            return Ok(false);
        };
        *row = call.clone();
        Ok(true)
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CallId,
    ) -> Result<Option<Call>, StoreError> {
        Ok(self.0.lock().unwrap().calls.get(workspace_id, id).cloned())
    }
    async fn list_unsettled(&self) -> Result<Vec<Call>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .calls
            .0
            .values()
            .filter(|row| row.state != CallState::Ended)
            .cloned()
            .collect())
    }
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        direction: Option<crate::domain::CallDirection>,
        state: Option<crate::domain::CallState>,
        before: Option<&CallId>,
        limit: u32,
    ) -> Result<Vec<Call>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .calls
                .rows(workspace_id)
                .filter(|r| {
                    agent_id.is_none_or(|id| &r.agent_id == id)
                        && direction.is_none_or(|d| r.direction == d)
                        && state.is_none_or(|s| r.state == s)
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
        id: &CallId,
        at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.calls.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.dismissed_at.get_or_insert(at);
        Ok(true)
    }
}

#[async_trait::async_trait]
impl TextRecordStore for FakeTextRecordStore {
    async fn insert(&self, record: &TextRecord) -> Result<(), StoreError> {
        self.0.lock().unwrap().text_records.insert(
            &record.workspace_id,
            record.id.clone(),
            record.clone(),
        )
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &TextRecordId,
    ) -> Result<Option<TextRecord>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .text_records
            .get(workspace_id, id)
            .cloned())
    }
    async fn set_delivery_status(
        &self,
        workspace_id: &WorkspaceId,
        id: &TextRecordId,
        status: &TextDeliveryStatus,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(row) = data.text_records.get_mut(workspace_id, id) else {
            return Ok(false);
        };
        row.delivery_status = Some(status.clone());
        Ok(true)
    }
    async fn list_conversation(
        &self,
        workspace_id: &WorkspaceId,
        phone_number_id: &PhoneNumberId,
        counterpart_e164: &str,
        max: u32,
        before: Option<&TextRecordId>,
    ) -> Result<Vec<TextRecord>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .text_records
                .rows(workspace_id)
                .filter(|r| {
                    &r.phone_number_id == phone_number_id && r.counterpart_e164 == counterpart_e164
                })
                .map(|r| (r.id.clone(), r.clone()))
                .collect(),
            before,
            max,
        ))
    }
    async fn list_conversations(
        &self,
        workspace_id: &WorkspaceId,
        phone_number_id: &PhoneNumberId,
        max: u32,
    ) -> Result<Vec<TextConversation>, StoreError> {
        let data = self.0.lock().unwrap();
        let mut latest: BTreeMap<String, TextConversation> = BTreeMap::new();
        for row in data
            .text_records
            .rows(workspace_id)
            .filter(|r| &r.phone_number_id == phone_number_id)
        {
            let held = latest
                .entry(row.counterpart_e164.clone())
                .or_insert_with(|| TextConversation {
                    counterpart_e164: row.counterpart_e164.clone(),
                    last_at: row.occurred_at,
                    last_direction: row.direction,
                });
            if row.occurred_at >= held.last_at {
                held.last_at = row.occurred_at;
                held.last_direction = row.direction;
            }
        }
        let mut rows: Vec<_> = latest.into_values().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.last_at));
        rows.truncate(max as usize);
        Ok(rows)
    }
    async fn landing_thread(
        &self,
        workspace_id: &WorkspaceId,
        phone_number_id: &PhoneNumberId,
        counterpart_e164: &str,
        now: UnixMillis,
    ) -> Result<Option<MessageId>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .text_records
            .rows(workspace_id)
            .filter(|r| {
                &r.phone_number_id == phone_number_id
                    && r.counterpart_e164 == counterpart_e164
                    && r.direction == TextDirection::Outbound
                    && r.occurred_at >= now - TEXT_THREAD_WINDOW_MS
            })
            .max_by_key(|r| r.occurred_at)
            .and_then(|r| r.thread_id.clone()))
    }
}

#[async_trait::async_trait]
impl TrustListStore for FakeTrustListStore {
    async fn upsert(&self, row: &crate::TrustEntry) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        let id = data
            .trust_list
            .rows(&row.workspace_id)
            .find(|r| {
                r.agent_id == row.agent_id && r.subject == row.subject && r.value == row.value
            })
            .map(|r| r.id.clone())
            .unwrap_or_else(|| row.id.clone());
        let mut row = row.clone();
        row.id = id.clone();
        data.trust_list.put(&row.workspace_id, id, row.clone());
        Ok(())
    }
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<crate::TrustEntry>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .trust_list
            .rows(workspace_id)
            .cloned()
            .collect())
    }
    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &crate::TrustEntryId,
    ) -> Result<bool, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .trust_list
            .remove(workspace_id, id)
            .is_some())
    }
    async fn candidate(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        subject: crate::TrustSubject,
        value: &str,
    ) -> Result<Option<crate::TrustTier>, StoreError> {
        let data = self.0.lock().unwrap();
        let domain = mail_domain(value);
        Ok(data
            .trust_list
            .rows(workspace_id)
            .filter(|r| r.agent_id.is_none() || r.agent_id.as_ref() == Some(agent_id))
            .filter(|r| {
                (r.subject == subject && r.value.eq_ignore_ascii_case(value))
                    || (subject == TrustSubject::Address
                        && r.subject == TrustSubject::Domain
                        && domain
                            .as_deref()
                            .is_some_and(|domain| r.value.eq_ignore_ascii_case(domain)))
            })
            .map(|r| r.tier)
            .max())
    }
}

#[async_trait::async_trait]
impl KeypadFailureStore for FakeKeypadFailureStore {
    async fn get(&self, workspace_id: &WorkspaceId) -> Result<KeypadFailures, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .keypad
            .get(workspace_id)
            .copied()
            .unwrap_or_default())
    }
    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        now: UnixMillis,
    ) -> Result<KeypadFailures, StoreError> {
        let mut data = self.0.lock().unwrap();
        let row = data.keypad.entry(workspace_id.clone()).or_default();
        *row = row.after_failure(now);
        Ok(*row)
    }
    async fn clear(&self, workspace_id: &WorkspaceId) -> Result<(), StoreError> {
        self.0.lock().unwrap().keypad.remove(workspace_id);
        Ok(())
    }
}

fn create_number(data: &mut Data, number: &PhoneNumber) -> Result<(), StoreError> {
    if number.status != PhoneNumberStatus::Released
        && data.phone_numbers.0.values().any(|row| {
            row.status != PhoneNumberStatus::Released
                && (row.workspace_id == number.workspace_id || row.e164 == number.e164)
        })
    {
        return Err(conflict("Workspace Number exists"));
    }
    data.phone_numbers
        .insert(&number.workspace_id, number.id.clone(), number.clone())
}

fn add_rules(held: &mut Vec<String>, rules: &[String]) {
    for rule in rules {
        if !held.contains(rule) {
            held.push(rule.clone());
        }
    }
}
fn note_send(day: &mut Option<i64>, count: &mut u32, at: i64) -> u32 {
    let today = day_start(at);
    if *day != Some(today) {
        *day = Some(today);
        *count = 0;
    }
    *count += 1;
    *count
}

#[async_trait::async_trait]
impl crate::WorkspaceMailboxRecordStore for FakeWorkspaceMailboxStore {
    async fn reserve_send(&self, workspace: &WorkspaceId, day: i64) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        let Some(mailbox) = data.workspace_mailboxes.get(workspace) else {
            return Ok(false);
        };
        let cap = mailbox.outgoing_cap;
        let counter = data
            .workspace_mail_sends
            .entry(workspace.clone())
            .or_default();
        if counter.0 != day {
            *counter = (day, 0);
        }
        if counter.1 >= cap {
            return Ok(false);
        }
        counter.1 += 1;
        Ok(true)
    }
    async fn release_send(&self, workspace: &WorkspaceId, day: i64) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if let Some(counter) = data.workspace_mail_sends.get_mut(workspace)
            && counter.0 == day
        {
            counter.1 = counter.1.saturating_sub(1);
        }
        Ok(())
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<Option<crate::WorkspaceMailbox>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .workspace_mailboxes
            .get(workspace)
            .cloned())
    }
    async fn create(&self, mailbox: &crate::WorkspaceMailbox) -> Result<(), StoreError> {
        let mut data = self.0.lock().unwrap();
        if data.workspace_mailboxes.contains_key(&mailbox.workspace_id)
            || data
                .workspace_mailboxes
                .values()
                .any(|m| m.address == mailbox.address)
            || data
                .agent_mailboxes
                .0
                .values()
                .any(|m| m.address == mailbox.address)
        {
            return Err(conflict("that Workspace or address already has a mailbox"));
        }
        data.workspace_mailboxes
            .insert(mailbox.workspace_id.clone(), mailbox.clone());
        Ok(())
    }
    async fn drop_reservation(
        &self,
        workspace: &WorkspaceId,
        address: &str,
    ) -> Result<bool, StoreError> {
        let mut data = self.0.lock().unwrap();
        if data
            .workspace_mailboxes
            .get(workspace)
            .is_some_and(|m| m.address == address)
        {
            data.workspace_mailboxes.remove(workspace);
            data.workspace_mail_sends.remove(workspace);
            return Ok(true);
        }
        Ok(false)
    }
    async fn count_for_connection(
        &self,
        workspace: &WorkspaceId,
        connection: &ConnectionId,
    ) -> Result<u32, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .workspace_mailboxes
            .values()
            .filter(|m| m.workspace_id == *workspace && m.connection_id == *connection)
            .count() as u32)
    }
}
