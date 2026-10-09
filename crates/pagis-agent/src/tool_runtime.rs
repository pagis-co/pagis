//! Daemon-side execution adapter for the tools Pagis dispatches
//! itself: core capability tools and `add_block` from the
//! `ui` manifest. `ask_user` never reaches here — the broker
//! mints its Request row and the run loop parks on it, and the Event
//! Subscription tools route to the Trigger module instead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CoreTool, ToolExecutor, ToolResult, ToolRoute, UiTool};
use pagis_core::{
    Agent, AgentStatus, AgentStore, Block, Channel, ChannelId, ChannelStore, EventBus, Grant,
    GrantStore, MemoryAccess, MemoryError, MemoryExposure, MemoryStore, ParticipantStore, RunId,
    RunStore, ScopedPath, Skill, Skills, WorkspaceId,
};

use crate::memory::MemoryOverlay;
use crate::ui::BlockBuffer;

pub struct ToolRuntimeDeps {
    pub agents: Arc<dyn AgentStore>,
    pub channels: Arc<dyn ChannelStore>,
    pub messages: Arc<dyn pagis_core::MessageStore>,
    /// The run behind a call, which says where the run's own reply
    /// lands: `send_message` refuses to speak there a second time.
    pub runs: Arc<dyn RunStore>,
    pub participants: Arc<dyn ParticipantStore>,
    pub memory: Arc<dyn MemoryStore>,
    pub pending_evidence: Arc<dyn pagis_core::PendingEvidenceStore>,
    pub conversation_evidence: Arc<dyn pagis_core::ConversationEvidenceStore>,
    pub grants: Arc<dyn GrantStore>,
    /// The Skills the agent holds (ADR-0017): `skill_load` reads
    /// the document, and the agent's notes come from its own memory.
    pub skills: Arc<dyn Skills>,
    /// The agent computers, one manager per tenant: a
    /// `computer_shell` call runs in the caller's own Workspace.
    pub computers: Arc<pagis_computer::ComputerManagers>,
    pub bus: Arc<dyn EventBus>,
    /// The logical now a memory write commits at. It is the clock that
    /// source acquisition and Subject Page updates use, so a write lands
    /// on the same timeline.
    pub clock: Arc<dyn pagis_core::Clock>,
    /// The same bounded conversation window used to build a model turn.
    pub context_messages: u32,
    /// Which of the person's machines are connected now. A host
    /// action dispatches through it; the daemon runs no host command
    /// itself.
    pub presence: Arc<pagis_broker::HostPresence>,
}

/// The `computer_shell` deadline: the default, and the cap a
/// larger request is clamped to.
const SHELL_TIMEOUT_DEFAULT: u64 = 120;
const SHELL_TIMEOUT_MAX: u64 = 600;
/// The in-container deadline reports itself with this code (GNU
/// `timeout`), and a stopped container kills its execs with 137.
const EXIT_TIMED_OUT: i64 = 124;
const EXIT_CONTAINER_GONE: i64 = 137;

pub struct CoreToolRuntime {
    deps: ToolRuntimeDeps,
    overlays: Mutex<HashMap<RunId, Arc<tokio::sync::Mutex<MemoryOverlay>>>>,
    /// The blocks `add_block` has appended to the turn in flight.
    blocks: BlockBuffer,
}

impl CoreToolRuntime {
    /// Replace the daemon-owned Schedules section in one staged Subject Page.
    pub async fn refresh_subject_page_schedules(
        &self,
        call: &AuthorizedCall,
        path: &str,
        schedules: Vec<pagis_core::subject_page::OpenSchedule>,
    ) -> Result<(), MemoryError> {
        let path = ScopedPath::parse(path)?;
        let access = self
            .memory_access(call)
            .await
            .map_err(|error| MemoryError::Storage(error.to_string()))?;
        let overlay = self.overlay(&call.run_id);
        let mut overlay = overlay.lock().await;
        let content = overlay
            .read(&self.deps.memory, &call.workspace_id, &access, &path)
            .await?;
        let mut parsed = pagis_core::subject_page::SubjectPage::parse(&content);
        if !parsed.layout_valid {
            return Err(MemoryError::Invalid("the Subject Page is malformed".into()));
        }
        parsed.page.schedules = schedules;
        overlay.write(path, parsed.page.render());
        Ok(())
    }

    pub fn new(deps: ToolRuntimeDeps) -> Self {
        Self {
            deps,
            overlays: Mutex::new(HashMap::new()),
            blocks: BlockBuffer::default(),
        }
    }

    pub(crate) fn overlay(&self, run_id: &RunId) -> Arc<tokio::sync::Mutex<MemoryOverlay>> {
        self.overlays
            .lock()
            .expect("memory overlay lock")
            .entry(run_id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(MemoryOverlay::default())))
            .clone()
    }

    /// Permission provenance for a daemon message emitted by this run.
    pub async fn retained_exposures(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        run_id: &RunId,
        agent_id: &pagis_core::AgentId,
    ) -> Result<Vec<MemoryExposure>, String> {
        let grants = self
            .deps
            .grants
            .list_live_for_agent(workspace_id, agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let access = MemoryAccess::agent(
            agent_id.clone(),
            grants
                .into_iter()
                .filter(|grant| grant.resource_kind == Grant::CONNECTION_KIND)
                .map(|grant| MemoryExposure {
                    grant_id: grant.id,
                    revision: grant.revision,
                }),
        );
        let overlay = self.overlay(run_id);
        let mut overlay = overlay.lock().await;
        overlay
            .observe(&access)
            .map_err(|error| error.to_string())?;
        Ok(overlay.access(agent_id.clone()).exposures().to_vec())
    }

    pub(crate) fn clear_run(&self, run_id: &RunId) {
        self.overlays
            .lock()
            .expect("memory overlay lock")
            .remove(run_id);
        self.blocks.clear(run_id);
    }

    /// The blocks this turn added, and an empty buffer after it. The
    /// run loop calls it once per turn, when it settles the reply.
    pub(crate) fn take_blocks(&self, run_id: &RunId) -> Vec<Block> {
        self.blocks.take(run_id)
    }

    /// Append one daemon-minted block to the turn (ADR-0004).
    pub(crate) fn mint_block(&self, run_id: &RunId, block: Block) {
        self.blocks.mint(run_id, block);
    }

    fn execute_add_block(&self, call: &AuthorizedCall) -> ToolResult {
        match self.blocks.push(&call.run_id, &call.arguments["block"]) {
            Ok(ack) => ToolResult::success(ack),
            Err(error) => error,
        }
    }

    async fn execute_core(&self, call: AuthorizedCall, tool: CoreTool) -> ToolResult {
        match tool {
            CoreTool::HostShell => self.execute_host(&call).await,
            CoreTool::ComputerShell => self.execute_computer_shell(&call).await,
            CoreTool::SendMessage => self.execute_send(&call).await,
            CoreTool::MemoryRead => self.execute_memory_read(&call).await,
            CoreTool::MemorySearch => self.execute_memory_search(&call).await,
            CoreTool::MemoryWrite => self.execute_memory_write(&call).await,
            CoreTool::MemoryDelete => self.execute_memory_delete(&call).await,
            CoreTool::MemoryReview => self.execute_memory_review(&call).await,
            CoreTool::ConversationPage => self.execute_conversation_page(&call).await,
            CoreTool::ConversationSearch => self.execute_conversation_search(&call).await,
            CoreTool::ConversationRead => self.execute_conversation_read(&call).await,
            CoreTool::SkillLoad => self.execute_skill_load(&call).await,
            // The Event Subscription tools join the Trigger module,
            // the connections, and the channels, so the daemon routes
            // them to their own executor.
            CoreTool::EventSubscriptionCreate
            | CoreTool::EventSubscriptionList
            | CoreTool::EventSubscriptionUpdate
            | CoreTool::ScheduleCreate
            | CoreTool::ScheduleList
            | CoreTool::ScheduleUpdate
            | CoreTool::PhoneCall
            | CoreTool::CallTranscript
            | CoreTool::SoftwarePublish
            | CoreTool::ToolSearch
            | CoreTool::SoftwareFork
            | CoreTool::SoftwareContribute
            | CoreTool::ContributionView
            | CoreTool::ContributionClose
            | CoreTool::CodingSessionStart
            | CoreTool::CodingSessionSend
            | CoreTool::CodingSessionRead
            | CoreTool::CodingSessionCancel
            | CoreTool::CodingSessionClose
            | CoreTool::CodingSessionList
            | CoreTool::CodingSessionResume
            | CoreTool::CodingSessionDecide
            | CoreTool::CodingSessionEscalate
            | CoreTool::CodingSessionAnswer
            | CoreTool::CodingSessionSetMode => ToolResult::error(
                "temporarily_unavailable",
                format!("{} is not registered on this daemon", call.tool_name),
            ),
        }
    }

    async fn conversation_scope(
        &self,
        call: &AuthorizedCall,
    ) -> Result<pagis_core::ConversationScope, ToolResult> {
        let run = self
            .deps
            .runs
            .get(&call.workspace_id, &call.run_id)
            .await
            .map_err(|error| ToolResult::error("temporarily_unavailable", error.to_string()))?
            .ok_or_else(|| ToolResult::error("not_found", "conversation run not found"))?;
        if run.workspace_id != call.workspace_id || run.agent_id != call.agent_id {
            return Err(ToolResult::error(
                "permission_denied",
                "the run does not belong to this Agent and Workspace",
            ));
        }
        if run.trigger_kind == pagis_core::TriggerKind::Review {
            return Err(ToolResult::error(
                "unsupported_scope",
                "pending review can span source conversations; retrieve evidence in a source conversation",
            ));
        }
        let channel_id = run.channel_id.ok_or_else(|| {
            ToolResult::error("unsupported_scope", "this run has no conversation source")
        })?;
        Ok(pagis_core::ConversationScope {
            workspace_id: call.workspace_id.clone(),
            agent_id: call.agent_id.clone(),
            channel_id,
            root_message_id: run.root_message_id,
        })
    }

    /// Write the Subject Page of this conversation. The Channel
    /// comes from the Run, as it does for every other conversation
    /// tool, so an Agent writes the page of the conversation it works
    /// in and of no other.
    ///
    /// The page has the layout of every other Subject Page: the front
    /// matter block names it, the compiled truth says where the
    /// conversation stands, the Facts table holds the decisions the
    /// conversation already made, the open questions stand in their own
    /// section, and the Timeline takes one entry for each turn of work.
    /// The write stages in the Run's overlay, so the foreground commit
    /// of the reply carries it.
    ///
    /// This is the reply-time cause of a Workstream page of open work.
    /// Most conversations never compact, so a turn that leaves
    /// a next step or an open question writes the page here.
    async fn execute_conversation_page(&self, call: &AuthorizedCall) -> ToolResult {
        let text = |field: &str| {
            call.arguments[field]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let (Some(title), Some(truth), Some(settled)) =
            (text("title"), text("truth"), text("settled"))
        else {
            return ToolResult::error(
                "invalid_request",
                "conversation_page: title, truth and settled are required",
            );
        };
        // A conversation is work the owner and the Agent do, so it is a
        // Project or a Workstream. The other page kinds describe
        // something the conversation is about, not the conversation.
        let Some(kind @ ("Project" | "Workstream")) = text("kind") else {
            return ToolResult::error(
                "invalid_request",
                "conversation_page: kind must be Project or Workstream",
            );
        };
        let run = match self.deps.runs.get(&call.workspace_id, &call.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return ToolResult::error("not_found", "conversation_page: run not found"),
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        let Some(channel_id) = run.channel_id else {
            return ToolResult::error(
                "invalid_request",
                "conversation_page: this run has no conversation",
            );
        };
        let path = match ScopedPath::parse(&pagis_core::memory_page::conversation_page_path(
            channel_id.as_str(),
        )) {
            Ok(path) => path,
            Err(error) => return ToolResult::error("invalid_request", error.to_string()),
        };
        let access = match self.memory_access(call).await {
            Ok(access) => access,
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        let overlay = self.overlay(&call.run_id);
        let mut overlay = overlay.lock().await;
        let mut page = match overlay
            .read(&self.deps.memory, &call.workspace_id, &access, &path)
            .await
        {
            Ok(content) => {
                let parsed = pagis_core::subject_page::SubjectPage::parse(&content);
                if !parsed.layout_valid {
                    return ToolResult::error(
                        "invalid_request",
                        format!(
                            "conversation_page: {} is not a valid subject page",
                            path.display()
                        ),
                    );
                }
                parsed.page
            }
            Err(MemoryError::NotFound(_)) => pagis_core::subject_page::SubjectPage::default(),
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        page.front_matter.title = Some(title.to_string());
        page.front_matter.kind = Some(kind.to_string());
        page.front_matter.links = named_pages(&call.arguments, page.front_matter.links);
        page.truth = truth.to_string();
        // An open question is not a fact, so it has a section of its
        // own. The call states every question that is still open, so
        // the section is replaced and a question the turn answered
        // leaves it.
        page.open_questions = string_list(&call.arguments, "open_questions");
        // The Run is the turn: a second call in one Run says the turn
        // settled something else, and it writes no second entry. The
        // reference names no connection, because a conversation is not
        // a source arrival.
        let reference = format!("conversation::{}:1", call.run_id);
        // A decision the conversation made is a fact, and it rests on
        // the Timeline entry of the turn that made it. A
        // decision the table already holds is not added again.
        for decision in string_list(&call.arguments, "decisions") {
            if page.facts.iter().any(|fact| fact.claim == decision) {
                continue;
            }
            page.facts.push(pagis_core::subject_page::Fact {
                claim: decision,
                kind: pagis_core::memory_page::DECISION_FACT_KIND.to_string(),
                source_reference: reference.clone(),
                status: pagis_core::subject_page::FactStatus::Active,
            });
        }
        match page
            .timeline
            .iter_mut()
            .find(|entry| entry.source_reference == reference)
        {
            Some(entry) => entry.words = settled.to_string(),
            None => {
                page.append(pagis_core::subject_page::TimelineEntry {
                    source_reference: reference,
                    source_time: self.deps.clock.now_ms(),
                    words: settled.to_string(),
                });
            }
        }
        let content = page.render();
        if let Err(error) = pagis_core::validate_content(&content) {
            return ToolResult::error("invalid_request", error.to_string());
        }
        let page_path = path.display();
        overlay.write(path, content);
        ToolResult::success(format!("staged the conversation page at {page_path}"))
    }

    async fn execute_conversation_search(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(query) = call.arguments["query"].as_str().map(str::trim) else {
            return ToolResult::error("invalid_request", "conversation_search: query is required");
        };
        let limit = match evidence_limit(call, "conversation_search") {
            Ok(limit) => limit,
            Err(result) => return result,
        };
        let after = call.arguments["after"].as_str();
        let scope = match self.conversation_scope(call).await {
            Ok(scope) => scope,
            Err(result) => return result,
        };
        match self
            .deps
            .conversation_evidence
            .search(&scope, query, after, limit)
            .await
        {
            Ok(hits) => {
                let next = hits
                    .last()
                    .map(|hit| format!("{}|{}", hit.message_id.as_str(), hit.reference));
                let content =
                    serde_json::to_string(&hits).expect("conversation search hits serialize");
                ToolResult::success(pagis_core::wrap_untrusted(
                    "conversation_evidence",
                    &content,
                ))
                .with_metadata(serde_json::json!({
                    "hit_count": hits.len(),
                    "references": hits.iter().map(|hit| &hit.reference).collect::<Vec<_>>(),
                    "next": next
                }))
            }
            Err(error) => ToolResult::error("temporarily_unavailable", error.to_string()),
        }
    }

    async fn execute_conversation_read(&self, call: &AuthorizedCall) -> ToolResult {
        let scope = match self.conversation_scope(call).await {
            Ok(scope) => scope,
            Err(result) => return result,
        };
        if let Some(reference) = call.arguments["reference"].as_str() {
            if let Some(message_id) = reference.strip_prefix("message:") {
                let message_id = pagis_core::MessageId::from(message_id.to_string());
                return match self
                    .deps
                    .conversation_evidence
                    .read_message(&scope, &message_id)
                    .await
                {
                    Ok(Some(message)) => {
                        let content = serde_json::to_string(&message)
                            .expect("conversation message serializes");
                        ToolResult::success(pagis_core::wrap_untrusted(
                            "conversation_evidence",
                            &content,
                        ))
                        .with_metadata(serde_json::json!({"references": [reference]}))
                    }
                    Ok(None) => ToolResult::error(
                        "unavailable",
                        "the message evidence is outside this conversation or no longer permitted",
                    ),
                    Err(error) => ToolResult::error("temporarily_unavailable", error.to_string()),
                };
            }
            if reference.starts_with("tool:") {
                return match self
                    .deps
                    .conversation_evidence
                    .read_tool(&scope, reference)
                    .await
                {
                    Ok(Some(evidence)) => {
                        let content =
                            serde_json::to_string(&evidence).expect("tool evidence serializes");
                        ToolResult::success(pagis_core::wrap_untrusted(
                            "conversation_evidence",
                            &content,
                        ))
                    }
                    Ok(None) => ToolResult::error(
                        "unavailable",
                        "the tool evidence was not retained or its source access is no longer valid",
                    ),
                    Err(error) => ToolResult::error("temporarily_unavailable", error.to_string()),
                };
            }
            return ToolResult::error(
                "invalid_request",
                "conversation_read: reference must name message or retained tool evidence",
            );
        }
        let Some(through) = call.arguments["through_message_id"].as_str() else {
            return ToolResult::error(
                "invalid_request",
                "conversation_read: through_message_id is required",
            );
        };
        let limit = match evidence_limit(call, "conversation_read") {
            Ok(limit) => limit,
            Err(result) => return result,
        };
        let after = call.arguments["after_message_id"]
            .as_str()
            .map(|value| pagis_core::MessageId::from(value.to_string()));
        let through = pagis_core::MessageId::from(through.to_string());
        if after
            .as_ref()
            .is_some_and(|after| after.as_str() >= through.as_str())
        {
            return ToolResult::error(
                "invalid_request",
                "conversation_read: after_message_id must precede through_message_id",
            );
        }
        match self
            .deps
            .conversation_evidence
            .read_range(&scope, after.as_ref(), &through, limit)
            .await
        {
            Ok(messages) => {
                let content =
                    serde_json::to_string(&messages).expect("conversation messages serialize");
                ToolResult::success(pagis_core::wrap_untrusted(
                    "conversation_evidence",
                    &content,
                ))
            .with_metadata(serde_json::json!({
                "message_count": messages.len(),
                "references": messages.iter().map(|message| &message.reference).collect::<Vec<_>>()
            }))
            }
            Err(error) => ToolResult::error("temporarily_unavailable", error.to_string()),
        }
    }

    async fn execute_memory_review(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(subject) = call.arguments["subject"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return ToolResult::error("invalid_request", "memory_review: subject is required");
        };
        let Some(reason) = call.arguments["reason"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return ToolResult::error("invalid_request", "memory_review: reason is required");
        };
        let urgency = match call.arguments["urgency"].as_str() {
            Some("normal") => pagis_core::PendingUrgency::Normal,
            Some("urgent") => pagis_core::PendingUrgency::Urgent,
            _ => {
                return ToolResult::error(
                    "invalid_request",
                    "memory_review: urgency must be normal or urgent",
                );
            }
        };
        let run = match self.deps.runs.get(&call.workspace_id, &call.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return ToolResult::error("not_found", "memory_review: run not found"),
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        let Some(channel_id) = run.channel_id.clone() else {
            return ToolResult::error(
                "invalid_request",
                "memory_review: this run has no conversation source",
            );
        };
        let (history, window_after) = match &run.root_message_id {
            Some(root) => match self
                .deps
                .messages
                .list_thread(&call.workspace_id, root)
                .await
            {
                Ok(messages) => (messages, None),
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            },
            None => {
                let limit = self.deps.context_messages;
                let mut window = match self
                    .deps
                    .messages
                    .list_top_level(&call.workspace_id, &channel_id, None, limit)
                    .await
                {
                    Ok(entries) => entries
                        .into_iter()
                        .map(|entry| entry.message)
                        .collect::<Vec<_>>(),
                    Err(error) => {
                        return ToolResult::error("temporarily_unavailable", error.to_string());
                    }
                };
                window.reverse();
                let after = if window.len() == limit as usize {
                    match self
                        .deps
                        .messages
                        .list_top_level(
                            &call.workspace_id,
                            &channel_id,
                            window.first().map(|message| &message.id),
                            1,
                        )
                        .await
                    {
                        Ok(entries) => entries.into_iter().next().map(|entry| entry.message.id),
                        Err(error) => {
                            return ToolResult::error("temporarily_unavailable", error.to_string());
                        }
                    }
                } else {
                    None
                };
                (window, after)
            }
        };
        let cursor = match self
            .deps
            .pending_evidence
            .review_cursor(
                &call.workspace_id,
                &call.agent_id,
                &channel_id,
                run.root_message_id.as_ref(),
                subject,
            )
            .await
        {
            Ok(cursor) => cursor,
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        let after_exclusive = cursor.clone().or(window_after);
        let mut source_message_ids = Vec::new();
        let mut exposures = Vec::new();
        for message in history
            .iter()
            .filter(|message| message.status == pagis_core::MessageStatus::Complete)
            .filter(|message| {
                cursor
                    .as_ref()
                    .is_none_or(|cursor| message.id.as_str() > cursor.as_str())
            })
        {
            match pagis_core::message_source_is_live(
                self.deps.messages.as_ref(),
                self.deps.grants.as_ref(),
                message,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            }
            let message_exposures = match self
                .deps
                .messages
                .exposures(&call.workspace_id, &message.id)
                .await
            {
                Ok(Some(required)) => required,
                Ok(None) if message.author_kind == pagis_core::AuthorKind::User => Vec::new(),
                Ok(None) => continue,
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            };
            source_message_ids.push(message.id.clone());
            for exposure in message_exposures {
                if !exposures.contains(&exposure) {
                    exposures.push(exposure);
                }
            }
        }
        let Some(through_inclusive) = source_message_ids
            .iter()
            .max_by_key(|id| id.as_str())
            .cloned()
        else {
            return ToolResult::error("invalid_request", "memory_review: no permitted evidence");
        };
        let evidence = pagis_core::PendingEvidence {
            workspace_id: call.workspace_id.clone(),
            agent_id: call.agent_id.clone(),
            channel_id,
            root_message_id: run.root_message_id,
            subject: subject.to_string(),
            after_exclusive,
            through_inclusive,
            source_message_ids,
            exposures,
            reason: reason.to_string(),
            urgency,
            created_at: self.deps.clock.now_ms(),
        };
        match self.deps.pending_evidence.record(evidence).await {
            Ok(Some(record)) => {
                let _ = self
                    .deps
                    .bus
                    .publish(pagis_core::NewEvent {
                        workspace_id: call.workspace_id.clone(),
                        event_type: "memory.review_pending".to_string(),
                        agent_id: Some(call.agent_id.clone()),
                        run_id: Some(call.run_id.clone()),
                        channel_id: Some(record.channel_id.clone()),
                        payload: serde_json::json!({
                            "pending_id": record.id.as_str(),
                            "subject": record.subject,
                            "urgency": record.urgency,
                            "eligible_at": record.eligible_at,
                            "maximum_due_at": record.maximum_due_at,
                        }),
                    })
                    .await;
                ToolResult::success(format!(
                    "pending review {} recorded; eligible_at={}, maximum_due_at={}",
                    record.id, record.eligible_at, record.maximum_due_at
                ))
            }
            Ok(None) => ToolResult::success("that evidence range was already reviewed"),
            Err(error) => ToolResult::error("temporarily_unavailable", error.to_string()),
        }
    }

    /// One command on a machine of the person, through the Pagis client
    /// running on it. Nothing runs in the daemon: the daemon is
    /// never a Host, on a local installation or a server, so there is no shell here to
    /// escape through on a shared server.
    ///
    /// The broker settled which machine before the card, so this reads
    /// the machine off the call and dispatches. A machine that went away
    /// between the approval and now is an answer the person can act on,
    /// not a hang.
    async fn execute_host(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(command) = call.arguments["command"].as_str() else {
            return ToolResult::error("invalid_request", "host_shell: command must not be empty");
        };
        let Some(host) = &call.host else {
            return ToolResult::error(
                "host_not_connected",
                "no computer of yours is connected. Open the Pagis client on the computer this \
                 should run on, then ask again.",
            );
        };
        let timeout = call
            .call_timeout
            .unwrap_or(pagis_broker::HOST_DISPATCH_TIMEOUT);
        let host_fact = serde_json::json!({ "host_id": host.id.as_str(), "host_name": host.name });
        match self
            .deps
            .presence
            .run(&host.id, command, call.approved_by_rule, timeout)
            .await
        {
            Ok(outcome) => {
                let mut content = match outcome.exit_code {
                    Some(code) => format!("exit code: {code}"),
                    None => "exit code: killed by signal".to_string(),
                };
                if !outcome.stdout.is_empty() {
                    content.push_str("\nstdout:\n");
                    content.push_str(&outcome.stdout);
                }
                if !outcome.stderr.is_empty() {
                    content.push_str("\nstderr:\n");
                    content.push_str(&outcome.stderr);
                }
                let mut metadata = host_fact;
                metadata["ok"] = serde_json::Value::Bool(outcome.exit_code == Some(0));
                metadata["exit_code"] = serde_json::json!(outcome.exit_code);
                ToolResult::success(content).with_metadata(metadata)
            }
            Err(pagis_broker::HostDispatchError::NotConnected) => ToolResult::error(
                "host_not_connected",
                format!(
                    "your {} is not connected; open the Pagis client on it, then ask again.",
                    host.name
                ),
            )
            .with_metadata(host_fact),
            Err(pagis_broker::HostDispatchError::Timeout(waited)) => ToolResult::error(
                "host_timed_out",
                format!(
                    "your {} took longer than {} seconds and the command was stopped.",
                    host.name,
                    waited.as_secs()
                ),
            )
            .with_metadata(host_fact),
        }
    }

    /// One command in the agent's own computer. A non-zero exit
    /// is a plain result the model reads; `is_error` is for Pagis
    /// failing: the wake failed, the runtime refused the exec, or the
    /// container went away under the command.
    async fn execute_computer_shell(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(command) = call.arguments["command"].as_str() else {
            return ToolResult::error(
                "invalid_request",
                "computer_shell: command must not be empty",
            );
        };
        let seconds = call.arguments["timeout_s"]
            .as_u64()
            .unwrap_or(SHELL_TIMEOUT_DEFAULT)
            .clamp(1, SHELL_TIMEOUT_MAX);
        let outcome = match self
            .deps
            .computers
            .get(&call.workspace_id)
            .shell(
                &call.agent_id,
                pagis_computer::ShellCommand {
                    command: command.to_string(),
                    timeout: std::time::Duration::from_secs(seconds),
                    cwd: call.arguments["cwd"].as_str().map(str::to_string),
                    stdin: None,
                    output_cap: None,
                },
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                return ToolResult::error(
                    "temporarily_unavailable",
                    format!("computer_shell: {error}"),
                );
            }
        };

        let timed_out = outcome.exit_code == EXIT_TIMED_OUT;
        // The daemon writes a launch failure into the stdout stream,
        // because the connection is already hijacked. Such text
        // is not the command's own output.
        if matches!(outcome.exit_code, 126 | 127)
            && outcome.stdout.starts_with("OCI runtime exec failed")
        {
            return ToolResult::error(
                "temporarily_unavailable",
                format!("computer_shell: {}", outcome.stdout.trim()),
            );
        }
        if outcome.exit_code == EXIT_CONTAINER_GONE {
            return ToolResult::error(
                "temporarily_unavailable",
                "computer_shell: the computer stopped while the command ran",
            );
        }

        let mut content = format!("exit code: {}", outcome.exit_code);
        if timed_out {
            content.push_str(&format!(" (no result after {seconds}s)"));
        }
        if !outcome.stdout.is_empty() {
            content.push_str("\nstdout:\n");
            content.push_str(&outcome.stdout);
        }
        if !outcome.stderr.is_empty() {
            content.push_str("\nstderr:\n");
            content.push_str(&outcome.stderr);
        }
        ToolResult::success(content).with_metadata(serde_json::json!({
            "exit_code": outcome.exit_code,
            "timed_out": timed_out,
            "truncated": outcome.truncated,
        }))
    }

    async fn execute_memory_read(&self, call: &AuthorizedCall) -> ToolResult {
        let path = match parse_path(call) {
            Ok(path) => path,
            Err(result) => return result,
        };
        let Some(_agent) = self.load_agent(call).await else {
            return ToolResult::error("temporarily_unavailable", "memory_read: agent not found");
        };
        let overlay = self.overlay(&call.run_id);
        let mut overlay = overlay.lock().await;
        let access = match self.memory_access(call).await {
            Ok(access) => access,
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        match overlay
            .read(&self.deps.memory, &call.workspace_id, &access, &path)
            .await
        {
            Ok(content) => ToolResult::success(content),
            Err(MemoryError::Unavailable) => {
                ToolResult::error("not_found", format!("no memory file at {}", path.display()))
            }
            Err(error @ (MemoryError::NotFound(_) | MemoryError::Invalid(_))) => {
                ToolResult::error("not_found", error.to_string())
            }
            Err(error) => ToolResult::error("temporarily_unavailable", error.to_string()),
        }
    }

    async fn execute_memory_search(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(query) = call.arguments["query"].as_str().map(str::trim) else {
            return ToolResult::error("invalid_request", "memory_search: query is required");
        };
        if query.is_empty() {
            return ToolResult::error("invalid_request", "memory_search: query must not be empty");
        }
        let limit = match call.arguments["limit"].as_u64() {
            Some(limit @ 1..=25) => limit as usize,
            Some(_) => {
                return ToolResult::error(
                    "invalid_request",
                    "memory_search: limit must be from 1 through 25",
                );
            }
            None if call.arguments.get("limit").is_some() => {
                return ToolResult::error(
                    "invalid_request",
                    "memory_search: limit must be an integer",
                );
            }
            None => 10,
        };
        let access = match self.memory_access(call).await {
            Ok(access) => access,
            Err(error) => return ToolResult::error("temporarily_unavailable", error.to_string()),
        };
        let mut hits = Vec::new();
        for scope in [
            pagis_core::MemoryScope::Shared,
            pagis_core::MemoryScope::Private,
        ] {
            match self
                .deps
                .memory
                .search_pages(&call.workspace_id, &access, scope, query, limit)
                .await
            {
                Ok(found) => hits.extend(found),
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            }
        }
        hits.sort_by(|left, right| left.rank.total_cmp(&right.rank));
        hits.truncate(limit);
        let paths: Vec<_> = hits.iter().map(|hit| hit.path.clone()).collect();
        let content = if hits.is_empty() {
            "no memory page matches the query".to_string()
        } else {
            serde_json::to_string(&hits).expect("memory search hits serialize")
        };
        ToolResult::success(content).with_metadata(serde_json::json!({
            "query": query,
            "hit_count": hits.len(),
            "paths": paths,
        }))
    }

    async fn execute_memory_write(&self, call: &AuthorizedCall) -> ToolResult {
        let path = match parse_path(call) {
            Ok(path) => path,
            Err(result) => return result,
        };
        let Some(content) = call.arguments["content"].as_str() else {
            return ToolResult::error("invalid_request", "memory_write: content is required");
        };
        if let Err(error) = pagis_core::validate_content(content) {
            return ToolResult::error("invalid_request", error.to_string());
        }
        let expected = call.arguments["expected_memory_revision"]
            .as_str()
            .unwrap_or_default();
        let subject_page = path.rel.starts_with("subjects/") && path.rel.ends_with(".md");
        let access = if subject_page {
            match self.memory_access(call).await {
                Ok(access) => Some(access),
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            }
        } else {
            None
        };
        let overlay = self.overlay(&call.run_id);
        let mut overlay = overlay.lock().await;
        let actual = overlay.base_revision().unwrap_or("missing");
        if expected != actual {
            return ToolResult::error(
                "revision_conflict",
                "memory changed after the supplied base revision; read the current memory and retry",
            );
        }
        let page_path = path.display();
        let mut notes: Vec<String> = Vec::new();
        let content = if let Some(access) = access {
            let current = match overlay
                .read(&self.deps.memory, &call.workspace_id, &access, &path)
                .await
            {
                Ok(content) => {
                    let parsed = pagis_core::subject_page::SubjectPage::parse(&content);
                    if !parsed.layout_valid {
                        tracing::warn!(path = %page_path, "the stored subject page is malformed");
                        return ToolResult::error(
                            "invalid_request",
                            format!("memory_write: {page_path} is not a valid subject page"),
                        );
                    }
                    parsed.page
                }
                Err(MemoryError::NotFound(_)) => Default::default(),
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            };
            let Some((page, strategy, warnings)) =
                pagis_core::subject_page::repair_reflection(content, &current)
            else {
                tracing::warn!(path = %page_path, "reflection page update could not be repaired");
                return ToolResult::error(
                    "invalid_request",
                    format!("memory_write: the update for {page_path} could not be parsed"),
                );
            };
            for warning in &warnings {
                tracing::warn!(path = %page_path, code = warning.code, message = %warning.message, "reflection facts warning");
            }
            tracing::debug!(path = %page_path, ?strategy, "reflection page update parsed");
            // The model reads the warnings back, so it can correct the page
            // on its next write.
            notes = warnings
                .iter()
                .map(|warning| match warning.row {
                    Some(row) => format!("row {row}: {}", warning.message),
                    None => warning.message.clone(),
                })
                .collect();
            page.render()
        } else {
            content.to_string()
        };
        overlay.write(path, content);
        if notes.is_empty() {
            return ToolResult::success(format!("staged write to {page_path}"));
        }
        ToolResult::success(format!(
            "staged write to {page_path}; {} warnings: {}",
            notes.len(),
            notes.join("; ")
        ))
    }

    async fn memory_access(
        &self,
        call: &AuthorizedCall,
    ) -> Result<MemoryAccess, pagis_core::StoreError> {
        let exposures = self
            .deps
            .grants
            .list_live_for_agent(&call.workspace_id, &call.agent_id)
            .await?
            .into_iter()
            .filter(|grant| grant.resource_kind == Grant::CONNECTION_KIND)
            .map(|grant| MemoryExposure {
                grant_id: grant.id,
                revision: grant.revision,
            });
        Ok(MemoryAccess::agent(call.agent_id.clone(), exposures))
    }

    /// One Skill: the document the Plugin ships, and the agent's own
    /// notes after it when it has written any (ADR-0017). The
    /// notes are one private fact file per Skill, and the agent writes
    /// them with `memory_write` like any other fact.
    async fn execute_skill_load(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(name) = call.arguments["name"].as_str() else {
            return ToolResult::error("invalid_request", "skill_load: name is required");
        };
        let Some((plugin, skill)) = Skill::split_qualified(name.trim()) else {
            return ToolResult::error(
                "invalid_request",
                format!("skill_load: {name:?} is not a skill name; use <plugin>:<skill>"),
            );
        };
        let Some(body) = self
            .deps
            .skills
            .body(&call.workspace_id, &call.agent_id, plugin, skill)
            .await
        else {
            return ToolResult::error(
                "not_found",
                format!("skill_load: you hold no skill named {name:?}"),
            );
        };
        let mut content = body;
        if let Some(notes) = self.skill_notes(call, plugin, skill).await {
            content.push_str("\n\n## Your notes\n\n");
            content.push_str(&notes);
        }
        ToolResult::success(content)
    }

    /// The agent's Private Memory fact file for one Skill, when it has
    /// one. A missing file is the normal case and is not an error.
    async fn skill_notes(
        &self,
        call: &AuthorizedCall,
        plugin: &str,
        skill: &str,
    ) -> Option<String> {
        let path = ScopedPath::parse(&format!("private/skills/{plugin}/{skill}.md")).ok()?;
        self.load_agent(call).await?;
        let access = self.memory_access(call).await.ok()?;
        let overlay = self.overlay(&call.run_id);
        let mut overlay = overlay.lock().await;
        let notes = overlay
            .read(&self.deps.memory, &call.workspace_id, &access, &path)
            .await
            .ok()?;
        (!notes.trim().is_empty()).then_some(notes)
    }

    async fn execute_memory_delete(&self, call: &AuthorizedCall) -> ToolResult {
        let path = match parse_path(call) {
            Ok(path) => path,
            Err(result) => return result,
        };
        let expected = call.arguments["expected_memory_revision"]
            .as_str()
            .unwrap_or_default();
        let overlay = self.overlay(&call.run_id);
        let mut overlay = overlay.lock().await;
        if expected != overlay.base_revision().unwrap_or("missing") {
            return ToolResult::error(
                "revision_conflict",
                "memory changed after the supplied base revision; read the current memory and retry",
            );
        }
        let display = path.display();
        overlay.delete(path);
        ToolResult::success(format!("staged delete of {display}"))
    }

    async fn load_agent(&self, call: &AuthorizedCall) -> Option<Agent> {
        self.deps
            .agents
            .get(&call.workspace_id, &call.agent_id)
            .await
            .ok()
            .flatten()
    }

    async fn execute_send(&self, call: &AuthorizedCall) -> ToolResult {
        let Some(to) = call.arguments["to"].as_str() else {
            return ToolResult::error("invalid_request", "send_message: `to` is required");
        };
        let Some(text) = call.arguments["text"].as_str() else {
            return ToolResult::error("invalid_request", "send_message: `text` is required");
        };
        let Some(sender) = self.load_agent(call).await else {
            return ToolResult::error("temporarily_unavailable", "send_message: agent not found");
        };
        let channel = if to.eq_ignore_ascii_case(pagis_broker::SEND_TO_USER) {
            match self
                .deps
                .channels
                .find_user_dm(&call.workspace_id, &sender.id)
                .await
            {
                Ok(Some(channel)) => channel,
                Ok(None) => {
                    return ToolResult::error(
                        "not_found",
                        "send_message: you have no DM with the user",
                    );
                }
                Err(error) => {
                    return ToolResult::error("temporarily_unavailable", error.to_string());
                }
            }
        } else {
            let target = match self.resolve_agent(&call.workspace_id, &sender, to).await {
                Ok(target) => target,
                Err(result) => return result,
            };
            match self.agent_dm(call, &sender, &target).await {
                Ok(channel) => channel,
                Err(result) => return result,
            }
        };
        if let Err(result) = self.reply_channel_is_free(call, &channel.id, to).await {
            return result;
        }
        let exposures = match self
            .retained_exposures(&call.workspace_id, &call.run_id, &call.agent_id)
            .await
        {
            Ok(exposures) => exposures,
            Err(error) => return ToolResult::error("temporarily_unavailable", error),
        };
        if let Err(problem) = self
            .dm()
            .post(
                &call.workspace_id,
                &channel,
                &sender.id,
                &call.run_id,
                text,
                &exposures,
            )
            .await
        {
            return ToolResult::error("temporarily_unavailable", problem);
        }
        ToolResult::success(format!(
            "sent to {to} (channel {}). Any reply arrives as a new message.",
            channel.id
        ))
    }

    /// A run speaks once in a channel (ADR-0003). The run's own reply
    /// already goes to the channel the reply target names, so a
    /// message sent there would be a second one, and every message in
    /// a direct channel wakes the reader: the reader would answer
    /// twice. The run answers with its reply instead.
    async fn reply_channel_is_free(
        &self,
        call: &AuthorizedCall,
        channel_id: &ChannelId,
        to: &str,
    ) -> Result<(), ToolResult> {
        let run = match self.deps.runs.get(&call.workspace_id, &call.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return Ok(()),
            Err(error) => {
                return Err(ToolResult::error(
                    "temporarily_unavailable",
                    error.to_string(),
                ));
            }
        };
        if run.reply_channel_id() != Some(channel_id) {
            return Ok(());
        }
        Err(ToolResult::error(
            "invalid_request",
            format!(
                "send_message: your reply this run already goes to the channel you share                  with {to}. Answer {to} in your reply; do not send a message as well."
            ),
        ))
    }

    async fn resolve_agent(
        &self,
        workspace_id: &WorkspaceId,
        sender: &Agent,
        name: &str,
    ) -> Result<Agent, ToolResult> {
        let agents = self
            .deps
            .agents
            .list_by_workspace(workspace_id)
            .await
            .map_err(|error| ToolResult::error("temporarily_unavailable", error.to_string()))?;
        let Some(target) = agents
            .into_iter()
            .find(|agent| agent.name.eq_ignore_ascii_case(name))
        else {
            return Err(ToolResult::error(
                "not_found",
                format!("send_message: no agent named `{name}`"),
            ));
        };
        if target.id == sender.id {
            return Err(ToolResult::error(
                "invalid_request",
                "send_message: you cannot send a message to yourself",
            ));
        }
        if target.status == AgentStatus::Archived {
            return Err(ToolResult::error(
                "not_found",
                format!("send_message: agent `{name}` is archived"),
            ));
        }
        Ok(target)
    }

    async fn agent_dm(
        &self,
        call: &AuthorizedCall,
        sender: &Agent,
        target: &Agent,
    ) -> Result<Channel, ToolResult> {
        self.dm()
            .between(&call.workspace_id, sender, target, &call.run_id)
            .await
            .map_err(|problem| ToolResult::error("temporarily_unavailable", problem))
    }

    /// The direct channel path, over the stores this runtime holds.
    fn dm(&self) -> crate::agent_dm::AgentDm {
        crate::agent_dm::AgentDm {
            channels: Arc::clone(&self.deps.channels),
            participants: Arc::clone(&self.deps.participants),
            messages: Arc::clone(&self.deps.messages),
            bus: Arc::clone(&self.deps.bus),
            clock: Arc::clone(&self.deps.clock),
        }
    }
}

fn parse_path(call: &AuthorizedCall) -> Result<ScopedPath, ToolResult> {
    let Some(path) = call.arguments["path"].as_str() else {
        return Err(ToolResult::error(
            "invalid_request",
            format!("{}: path is required", call.tool_name),
        ));
    };
    // A `[[<scope-relative path>]]` link names the page it points at,
    // so a caller gives a link as it stands on a page.
    ScopedPath::parse(pagis_core::subject_page::unwrap_link(path))
        .map_err(|error| ToolResult::error("invalid_request", error.to_string()))
}

/// The trimmed, non-empty strings of one array argument. A value that
/// is not an array, or an item that is not text, gives nothing.
fn string_list(arguments: &serde_json::Value, field: &str) -> Vec<String> {
    arguments[field]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// The pages a `conversation_page` call names, after the links the
/// page already holds. A target is a scope-relative path,
/// with or without the brackets a link stands in. A target that is not
/// a path is not a link, and a repeated target is kept once.
fn named_pages(arguments: &serde_json::Value, mut links: Vec<String>) -> Vec<String> {
    let targets = arguments["links"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|target| target.as_str());
    for target in targets {
        let target = pagis_core::subject_page::unwrap_link(target.trim());
        if ScopedPath::parse(target).is_ok() && !links.iter().any(|link| link == target) {
            links.push(target.to_string());
        }
    }
    links
}

fn evidence_limit(call: &AuthorizedCall, tool: &str) -> Result<u32, ToolResult> {
    match call.arguments.get("limit") {
        None => Ok(10),
        Some(value) => match value.as_u64() {
            Some(limit @ 1..=25) => Ok(limit as u32),
            Some(_) => Err(ToolResult::error(
                "invalid_request",
                format!("{tool}: limit must be from 1 through 25"),
            )),
            None => Err(ToolResult::error(
                "invalid_request",
                format!("{tool}: limit must be an integer"),
            )),
        },
    }
}

#[async_trait]
impl ToolExecutor for CoreToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        match call.route.clone() {
            ToolRoute::Core { tool } => self.execute_core(call, tool).await,
            ToolRoute::Connection { .. } => ToolResult::error(
                "temporarily_unavailable",
                "the connection runtime is not registered",
            ),
            ToolRoute::Vault { .. } => ToolResult::error(
                "temporarily_unavailable",
                "the vault runtime is not registered",
            ),
            // The daemon routes a Software call to the Software
            // runtime, which owns the version store, and a
            // Mailbox call to the mail runtime.
            ToolRoute::Software { .. } | ToolRoute::Mailbox { .. } => ToolResult::error(
                "temporarily_unavailable",
                "that tool runtime is not registered",
            ),
            // A Plugin call goes to the MCP host, which owns the
            // servers (ADR-0017).
            ToolRoute::Plugin { .. } => ToolResult::error(
                "temporarily_unavailable",
                "the plugin host is not registered",
            ),
            ToolRoute::Ui {
                tool: UiTool::AddBlock,
            } => self.execute_add_block(&call),
            // The broker mints the Request row and returns `Waiting`;
            // no executor runs an `ask_user` call.
            ToolRoute::Ui {
                tool: UiTool::AskUser,
            } => ToolResult::error(
                "invalid_request",
                "ask_user is dispatched by the broker, not by the tool runtime",
            ),
        }
    }
}
