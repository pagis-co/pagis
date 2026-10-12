//! Store adapters in memory for the portable desk tests.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::{StreamExt, channel::mpsc};
use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxId, AgentMailboxState, AgentMailboxStore, AgentStore,
    AvatarAppearance, Connection, ConnectionId, ConnectionStore, Event, EventBus, EventId,
    EventScope, EventStream, MailboxCursor, NewEvent, SealedSecret, StoreError, WorkspaceId,
    now_ms,
};

#[derive(Default)]
pub struct Records {
    agents: Mutex<Vec<Agent>>,
    connections: Mutex<Vec<Connection>>,
    tokens: Mutex<HashMap<ConnectionId, SealedSecret>>,
    mailboxes: Mutex<Vec<AgentMailbox>>,
    events: Mutex<Events>,
}

#[derive(Default)]
struct Events {
    log: Vec<Event>,
    subscribers: Vec<(EventScope, mpsc::UnboundedSender<Event>)>,
}

impl Records {
    pub fn event_types(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .log
            .iter()
            .map(|event| event.event_type.clone())
            .collect()
    }
}

#[async_trait]
impl AgentStore for Records {
    async fn create(&self, agent: &Agent) -> Result<(), StoreError> {
        let mut agents = self.agents.lock().unwrap();
        if agents.iter().any(|held| held.id == agent.id) {
            return Err(StoreError::Conflict("Agent exists".into()));
        }
        agents.push(agent.clone());
        Ok(())
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<Agent>, StoreError> {
        Ok(self
            .agents
            .lock()
            .unwrap()
            .iter()
            .find(|agent| &agent.workspace_id == workspace && &agent.id == id)
            .cloned())
    }
    async fn list_by_workspace(&self, workspace: &WorkspaceId) -> Result<Vec<Agent>, StoreError> {
        Ok(self
            .agents
            .lock()
            .unwrap()
            .iter()
            .filter(|agent| &agent.workspace_id == workspace)
            .cloned()
            .collect())
    }
    async fn update(&self, agent: &Agent) -> Result<(), StoreError> {
        let mut agents = self.agents.lock().unwrap();
        let held = agents
            .iter_mut()
            .find(|held| held.id == agent.id && held.workspace_id == agent.workspace_id)
            .ok_or(StoreError::Conflict("record is absent".into()))?;
        *held = agent.clone();
        Ok(())
    }
    async fn update_avatar(
        &self,
        workspace: &WorkspaceId,
        id: &AgentId,
        avatar: &AvatarAppearance,
        at: i64,
    ) -> Result<bool, StoreError> {
        let mut agents = self.agents.lock().unwrap();
        let Some(agent) = agents
            .iter_mut()
            .find(|agent| &agent.workspace_id == workspace && &agent.id == id)
        else {
            return Ok(false);
        };
        agent.avatar = avatar.clone();
        agent.updated_at = at;
        Ok(true)
    }
}

#[async_trait]
impl ConnectionStore for Records {
    async fn create(&self, connection: &Connection) -> Result<(), StoreError> {
        let mut connections = self.connections.lock().unwrap();
        if connections.iter().any(|held| {
            held.id == connection.id
                || (held.workspace_id == connection.workspace_id && held.alias == connection.alias)
        }) {
            return Err(StoreError::Conflict("Connection exists".into()));
        }
        connections.push(connection.clone());
        Ok(())
    }
    async fn set_status(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
        status: &str,
    ) -> Result<bool, StoreError> {
        let mut connections = self.connections.lock().unwrap();
        let Some(connection) = connections
            .iter_mut()
            .find(|connection| &connection.workspace_id == workspace && &connection.id == id)
        else {
            return Ok(false);
        };
        connection.status = status.into();
        Ok(true)
    }
    async fn set_authorization(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
        capabilities: &[String],
    ) -> Result<bool, StoreError> {
        let mut connections = self.connections.lock().unwrap();
        let Some(connection) = connections
            .iter_mut()
            .find(|connection| &connection.workspace_id == workspace && &connection.id == id)
        else {
            return Ok(false);
        };
        connection.authorized_capabilities = capabilities.to_vec();
        connection.status = Connection::CONNECTED.into();
        Ok(true)
    }
    async fn set_config(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let mut connections = self.connections.lock().unwrap();
        let Some(connection) = connections
            .iter_mut()
            .find(|connection| &connection.workspace_id == workspace && &connection.id == id)
        else {
            return Ok(false);
        };
        connection.config = config.clone();
        Ok(true)
    }
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        Ok(self
            .connections
            .lock()
            .unwrap()
            .iter()
            .filter(|connection| &connection.workspace_id == workspace)
            .cloned()
            .collect())
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        Ok(self
            .connections
            .lock()
            .unwrap()
            .iter()
            .find(|connection| &connection.workspace_id == workspace && &connection.id == id)
            .cloned())
    }
    async fn delete_and_revoke(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
        _: i64,
    ) -> Result<bool, StoreError> {
        // These records hold no Grants.
        let mut connections = self.connections.lock().unwrap();
        let before = connections.len();
        connections
            .retain(|connection| &connection.workspace_id != workspace || &connection.id != id);
        let removed = connections.len() != before;
        if removed {
            self.tokens.lock().unwrap().remove(id);
        }
        Ok(removed)
    }
    async fn set_refresh_token(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
        token: Option<&SealedSecret>,
    ) -> Result<bool, StoreError> {
        if ConnectionStore::get(self, workspace, id).await?.is_none() {
            return Ok(false);
        }
        let mut tokens = self.tokens.lock().unwrap();
        match token {
            Some(token) => {
                tokens.insert(id.clone(), token.clone());
            }
            None => {
                tokens.remove(id);
            }
        }
        Ok(true)
    }
    async fn refresh_token(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<SealedSecret>, StoreError> {
        if ConnectionStore::get(self, workspace, id).await?.is_none() {
            return Ok(None);
        }
        Ok(self.tokens.lock().unwrap().get(id).cloned())
    }
}

#[async_trait]
impl AgentMailboxStore for Records {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        if mailboxes.iter().any(|held| {
            held.address.eq_ignore_ascii_case(&mailbox.address)
                || (held.workspace_id == mailbox.workspace_id
                    && held.agent_id == mailbox.agent_id
                    && held.state.is_live())
        }) {
            return Err(StoreError::Conflict("Agent Mailbox exists".into()));
        }
        mailboxes.push(mailbox.clone());
        Ok(())
    }
    async fn drop_reservation(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<bool, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let before = mailboxes.len();
        mailboxes.retain(|mailbox| {
            &mailbox.workspace_id != workspace
                || &mailbox.id != id
                || mailbox.state != AgentMailboxState::Provisioning
        });
        Ok(mailboxes.len() != before)
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        Ok(self
            .mailboxes
            .lock()
            .unwrap()
            .iter()
            .find(|mailbox| &mailbox.workspace_id == workspace && &mailbox.id == id)
            .cloned())
    }
    async fn for_agent(
        &self,
        workspace: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        Ok(self
            .mailboxes
            .lock()
            .unwrap()
            .iter()
            .find(|mailbox| {
                &mailbox.workspace_id == workspace
                    && &mailbox.agent_id == id
                    && mailbox.state.is_live()
            })
            .cloned())
    }
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<AgentMailbox>, StoreError> {
        Ok(self
            .mailboxes
            .lock()
            .unwrap()
            .iter()
            .filter(|mailbox| &mailbox.workspace_id == workspace && mailbox.state.is_live())
            .cloned()
            .collect())
    }
    async fn address_taken(&self, address: &str) -> Result<bool, StoreError> {
        Ok(self
            .mailboxes
            .lock()
            .unwrap()
            .iter()
            .any(|mailbox| mailbox.address.eq_ignore_ascii_case(address)))
    }
    async fn set_state(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
        state: AgentMailboxState,
        reason: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let Some(mailbox) = live(&mut mailboxes, workspace, id) else {
            return Ok(false);
        };
        mailbox.state = state;
        mailbox.reason = reason.map(str::to_string);
        Ok(true)
    }
    async fn set_cursor(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
        cursor: &MailboxCursor,
    ) -> Result<bool, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let Some(mailbox) = live(&mut mailboxes, workspace, id) else {
            return Ok(false);
        };
        mailbox.cursor = Some(cursor.clone());
        Ok(true)
    }
    async fn make_dormant_for_agent(
        &self,
        workspace: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<AgentMailbox>, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let Some(mailbox) = mailboxes.iter_mut().find(|mailbox| {
            &mailbox.workspace_id == workspace && &mailbox.agent_id == id && mailbox.state.is_live()
        }) else {
            return Ok(None);
        };
        mailbox.state = AgentMailboxState::Dormant;
        mailbox.reason = None;
        Ok(Some(mailbox.clone()))
    }
    async fn delete(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
        at: i64,
    ) -> Result<bool, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let Some(mailbox) = live(&mut mailboxes, workspace, id) else {
            return Ok(false);
        };
        mailbox.state = AgentMailboxState::Deleted;
        mailbox.reason = None;
        mailbox.deleted_at = Some(at);
        Ok(true)
    }
    async fn note_send(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
        at: i64,
    ) -> Result<u32, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let mailbox = live(&mut mailboxes, workspace, id)
            .ok_or(StoreError::Conflict("record is absent".into()))?;
        let day = pagis_core::day_start(at);
        if mailbox.sends_day.as_ref() != Some(&day) {
            mailbox.sends_today = 0;
            mailbox.sends_day = Some(day);
        }
        mailbox.sends_today += 1;
        Ok(mailbox.sends_today)
    }
    async fn add_allow_rules(
        &self,
        workspace: &WorkspaceId,
        id: &AgentMailboxId,
        rules: &[String],
    ) -> Result<Vec<String>, StoreError> {
        let mut mailboxes = self.mailboxes.lock().unwrap();
        let mailbox = live(&mut mailboxes, workspace, id)
            .ok_or(StoreError::Conflict("record is absent".into()))?;
        for rule in rules {
            if !mailbox.allow_rules.contains(rule) {
                mailbox.allow_rules.push(rule.clone());
            }
        }
        Ok(mailbox.allow_rules.clone())
    }
    async fn count_live_for_connection(
        &self,
        workspace: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<u32, StoreError> {
        Ok(self
            .mailboxes
            .lock()
            .unwrap()
            .iter()
            .filter(|mailbox| {
                &mailbox.workspace_id == workspace
                    && &mailbox.connection_id == id
                    && mailbox.state.is_live()
            })
            .count() as u32)
    }
}

fn live<'a>(
    mailboxes: &'a mut [AgentMailbox],
    workspace: &WorkspaceId,
    id: &AgentMailboxId,
) -> Option<&'a mut AgentMailbox> {
    mailboxes.iter_mut().find(|mailbox| {
        &mailbox.workspace_id == workspace && &mailbox.id == id && mailbox.state.is_live()
    })
}

#[async_trait]
impl EventBus for Records {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        let mut events = self.events.lock().unwrap();
        let event = Event {
            id: EventId::generate(),
            seq: events.log.len() as i64 + 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        };
        events.log.push(event.clone());
        events.subscribers.retain(|(scope, sender)| {
            !scope.covers(&event) || sender.unbounded_send(event.clone()).is_ok()
        });
        Ok(event)
    }
    async fn subscribe(&self, scope: EventScope, after: Option<i64>) -> EventStream {
        let (sender, receiver) = mpsc::unbounded();
        let mut events = self.events.lock().unwrap();
        if let Some(after) = after {
            for event in &events.log {
                if event.seq > after && scope.covers(event) {
                    sender.unbounded_send(event.clone()).unwrap();
                }
            }
        }
        events.subscribers.push((scope, sender));
        receiver.boxed()
    }
}
