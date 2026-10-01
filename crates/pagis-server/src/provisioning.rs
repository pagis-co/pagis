//! The seed of one Person's Workspace.
//!
//! A local first run and an Administrator creating an account must give
//! a person the same starting point: one Workspace, one sprite, the DM
//! that sprite greets them in, the well-known model aliases, and the
//! Report Schedule Home reads. One text writes all of it, so the two
//! ways in cannot drift apart.
//!
//! The daemon's boot calls this before the Trigger module exists, so the
//! Schedule row goes straight through the store and only its first
//! firing comes from the cron reader.

use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, Channel, ChannelId, ChannelKind, ChannelParticipant,
    ChannelStore, CreatorKind, DEFAULT_MODEL_ALIAS, EventLog, MessageStore, ModelAlias,
    ModelAliasId, ModelAliasStore, NewEvent, ParticipantId, ParticipantKind, ParticipantStore,
    Schedule, ScheduleId, ScheduleKind, ScheduleState, ScheduleStore, StoreError, UnixMillis,
    UserId, Workspace, WorkspaceId, WorkspaceStore,
};

/// The name a Workspace takes when nobody named it.
pub const DEFAULT_WORKSPACE_NAME: &str = "Workspace";
/// The first sprite of a new Workspace. Onboarding introduces it
/// by this name; the person renames it later like any other Agent.
pub const DEFAULT_ASSISTANT_NAME: &str = "Pixie";
/// The seed names this sprite the Chief of Staff (ADR-0022), so its job
/// says so until the person writes another.
const DEFAULT_ASSISTANT_JOB: &str = "Chief of Staff";
const DEFAULT_ASSISTANT_DESCRIPTION: &str =
    "Ask Pixie for anything that has no other sprite to own it.";
const DEFAULT_ASSISTANT_PERSONALITY: &str =
    "Warm, plainspoken, and practical. Gets to the point, asks before assuming.";

/// The one model the default alias names before anybody picks one: the
/// first model of the Model Preference. The route is one model
/// (ADR-0025). Onboarding replaces it with the model the person picks
/// from the Provider Model List, and a server replaces it with the
/// default route of its keys, at the setup and at each key change
/// ([`crate::model_lists`]). The seed adds no candidate of another
/// provider: a silent fallback changes the provider, the price and the
/// tools under the person.
pub fn seed_default_model() -> &'static str {
    crate::model_lists::DEFAULT_PREFERENCE[0]
}
/// The buffered transcription model behind the `transcribe` alias. The
/// live session picks its own model (ADR-0020).
const TRANSCRIBE_MODEL: &str = "openai/gpt-4o-transcribe";
const SPEAK_MODEL: &str = "openai/gpt-4o-mini-tts";

/// The Report Schedule of the Chief of Staff (ADR-0022): once a day at
/// 7:00 in the Workspace timezone.
pub const REPORT_SCHEDULE_NAME: &str = "Daily report";
pub const REPORT_CRON: &str = "0 7 * * *";
/// The Report's prompt. ADR-0022 fixes the three questions and their
/// order, because Home renders the Needs-You Queue above the Report and
/// the work record below it.
pub const REPORT_INSTRUCTION: &str = "\
Write the report for Home. Answer three questions, in this order and \
in this many words: what needs the user, what is running now, and what \
got done since your last report. Write prose the user can act on, not \
a list of tool names. Name an Agent when its work is what changed. \
When nothing needs the user, say so in one sentence and stop.";

/// Every alias a Workspace needs before anything can think, speak or
/// answer a call.
pub fn well_known_aliases() -> [(&'static str, Vec<&'static str>); 6] {
    [
        (DEFAULT_MODEL_ALIAS, vec![seed_default_model()]),
        (pagis_voice::TRANSCRIBE_ALIAS, vec![TRANSCRIBE_MODEL]),
        (pagis_voice::SPEAK_ALIAS, vec![SPEAK_MODEL]),
        (
            pagis_telephony::PHONE_ALIAS,
            pagis_telephony::PHONE_MODELS.to_vec(),
        ),
        (
            pagis_telephony::GPT_LIVE_REASONING_ALIAS,
            pagis_telephony::GPT_LIVE_REASONING_MODELS.to_vec(),
        ),
        (
            pagis_telephony::PHONE_CLASSIFIER_ALIAS,
            pagis_telephony::PHONE_CLASSIFIER_MODELS.to_vec(),
        ),
    ]
}

/// The stores the seed writes through. It names traits alone, so the
/// daemon's boot passes its store set and an API handler passes the
/// fields of its own state.
pub struct WorkspaceSeed<'a> {
    pub workspaces: &'a dyn WorkspaceStore,
    pub agents: &'a dyn AgentStore,
    pub channels: &'a dyn ChannelStore,
    pub participants: &'a dyn ParticipantStore,
    pub messages: &'a dyn MessageStore,
    pub model_aliases: &'a dyn ModelAliasStore,
    pub schedules: &'a dyn ScheduleStore,
    pub events: &'a dyn EventLog,
}

impl<'a> From<&'a pagis_core::Stores> for WorkspaceSeed<'a> {
    fn from(stores: &'a pagis_core::Stores) -> Self {
        Self {
            workspaces: stores.workspaces.as_ref(),
            agents: stores.agents.as_ref(),
            channels: stores.channels.as_ref(),
            participants: stores.participants.as_ref(),
            messages: stores.messages.as_ref(),
            model_aliases: stores.model_aliases.as_ref(),
            schedules: stores.schedules.as_ref(),
            events: stores.events.as_ref(),
        }
    }
}

/// How a new Workspace starts.
///
/// The local first-run wizard asks the person which model the
/// installation thinks on. On a server the administrator has answered
/// that for everybody before the account exists, so there is nothing to
/// ask and nothing to show: a created person lands in the product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onboarding {
    /// Open the wizard. Only the local seed does this.
    Wizard,
    /// Answered already, by the administrator of a server.
    Done,
}

/// Mark the Workspace of the administrator a server's own setup made
/// as onboarded.
///
/// The boot seeded that person's Workspace before anybody knew whether
/// the installation is a server, so the Workspace still waits for the
/// wizard. The setup is what proves it is a server: an administrator who
/// just typed an address, a password and the installation's provider
/// keys has answered the wizard already.
pub async fn onboard_for_a_server(
    workspaces: &dyn WorkspaceStore,
    user_id: &UserId,
    now: UnixMillis,
) -> Result<(), StoreError> {
    let Some(workspace) = workspaces.for_user(user_id).await? else {
        return Ok(());
    };
    workspaces.set_onboarded(&workspace.id, now).await
}

impl WorkspaceSeed<'_> {
    /// Give one Person their Workspace and everything in it, and answer
    /// the Workspace row.
    pub async fn run(
        &self,
        user_id: &UserId,
        name: &str,
        timezone: &str,
        onboarding: Onboarding,
        now: UnixMillis,
    ) -> Result<Workspace, StoreError> {
        let workspace = Workspace {
            id: WorkspaceId::generate(),
            user_id: user_id.clone(),
            name: name.to_string(),
            timezone: timezone.to_string(),
            created_at: now,
            onboarded_at: match onboarding {
                Onboarding::Wizard => None,
                Onboarding::Done => Some(now),
            },
            // The Default Assistant takes the designation once its row
            // exists (ADR-0022).
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
        };
        self.workspaces.create(&workspace).await?;

        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.id.clone(),
            name: DEFAULT_ASSISTANT_NAME.to_string(),
            job: DEFAULT_ASSISTANT_JOB.to_string(),
            description: DEFAULT_ASSISTANT_DESCRIPTION.to_string(),
            personality: DEFAULT_ASSISTANT_PERSONALITY.to_string(),
            model_alias: DEFAULT_MODEL_ALIAS.to_string(),
            avatar: Default::default(),
            voice: None,
            standing_brief: None,
            status: AgentStatus::Active,
            created_at: now,
            updated_at: now,
        };
        self.agents.create(&agent).await?;
        self.workspaces
            .set_chief_of_staff(&workspace.id, Some(&agent.id))
            .await?;

        // The assistant's DM: the person and Pixie. Messages here
        // trigger the agent loop.
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace.id.clone(),
            kind: ChannelKind::Dm,
            title: Some(agent.name.clone()),
            created_at: now,
            updated_at: now,
        };
        self.channels.create(&channel).await?;
        for (kind, agent_id) in [
            (ParticipantKind::User, None),
            (ParticipantKind::Agent, Some(agent.id.clone())),
        ] {
            self.participants
                .create(&ChannelParticipant {
                    id: ParticipantId::generate(),
                    workspace_id: workspace.id.clone(),
                    channel_id: channel.id.clone(),
                    kind,
                    agent_id,
                    joined_at: now,
                })
                .await?;
        }

        let greeting = agent.greeting(channel.id.clone());
        self.messages.insert_stamped(&greeting, &[]).await?;

        self.ensure_aliases(&workspace.id).await?;
        self.ensure_report_schedule(&workspace, &agent.id, &channel.id)
            .await?;

        for event in [
            NewEvent {
                workspace_id: workspace.id.clone(),
                event_type: "workspace.created".to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({ "name": workspace.name }),
            },
            NewEvent {
                workspace_id: workspace.id.clone(),
                event_type: "agent.created".to_string(),
                agent_id: Some(agent.id.clone()),
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({ "name": agent.name }),
            },
            NewEvent {
                workspace_id: workspace.id.clone(),
                event_type: "channel.created".to_string(),
                agent_id: Some(agent.id.clone()),
                run_id: None,
                channel_id: Some(channel.id.clone()),
                payload: serde_json::json!({ "title": channel.title }),
            },
            NewEvent {
                workspace_id: workspace.id.clone(),
                event_type: "message.completed".to_string(),
                agent_id: greeting.author_agent_id.clone(),
                run_id: None,
                channel_id: Some(greeting.channel_id.clone()),
                payload: serde_json::json!({
                    "message_id": greeting.id,
                    "parent_message_id": null,
                    "author_kind": "agent",
                }),
            },
        ] {
            self.events.append(event).await?;
        }

        // The row the caller reads back holds the designations the seed
        // wrote after it.
        Ok(self
            .workspaces
            .get(&workspace.id)
            .await?
            .unwrap_or(workspace))
    }

    /// Add the well-known aliases this Workspace does not hold. It runs
    /// in the seed and again on every boot, because a new binary can add
    /// one to a Workspace that already exists.
    pub async fn ensure_aliases(&self, workspace_id: &WorkspaceId) -> Result<(), StoreError> {
        for (alias, candidates) in well_known_aliases() {
            if self
                .model_aliases
                .get_by_alias(workspace_id, alias)
                .await?
                .is_some()
            {
                continue;
            }
            let now = pagis_core::now_ms();
            self.model_aliases
                .create(&ModelAlias {
                    id: ModelAliasId::generate(),
                    workspace_id: workspace_id.clone(),
                    alias: alias.to_string(),
                    candidates: candidates.into_iter().map(str::to_string).collect(),
                    created_at: now,
                    updated_at: now,
                })
                .await?;
        }
        Ok(())
    }

    /// Give the Workspace the Schedule the Chief of Staff writes the
    /// Report on (ADR-0022). A Workspace that already has one keeps it.
    pub async fn ensure_report_schedule(
        &self,
        workspace: &Workspace,
        agent_id: &AgentId,
        channel_id: &ChannelId,
    ) -> Result<(), StoreError> {
        if workspace.report_schedule_id.is_some() {
            return Ok(());
        }
        let now = pagis_core::now_ms();
        let scheduled_at =
            pagis_trigger::next_cron_occurrence(REPORT_CRON, &workspace.timezone, now).map_err(
                |error| StoreError::Corrupt(format!("the Report cadence is unreadable: {error}")),
            )?;
        let report = Schedule {
            id: ScheduleId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent_id.clone(),
            name: REPORT_SCHEDULE_NAME.to_string(),
            instruction: REPORT_INSTRUCTION.to_string(),
            subject_page_path: None,
            channel_id: channel_id.clone(),
            root_message_id: None,
            kind: ScheduleKind::Cron,
            cron_expression: Some(REPORT_CRON.to_string()),
            interval_ms: None,
            anchor_at: None,
            timezone: workspace.timezone.clone(),
            scheduled_at,
            next_due_at: Some(scheduled_at),
            last_result: None,
            state: ScheduleState::Active,
            revision: 1,
            approved_revision: Some(1),
            creator: CreatorKind::User,
            creating_run_id: None,
            created_at: now,
            updated_at: now,
            archived_at: None,
        };
        self.schedules.create(&report).await?;
        self.workspaces
            .set_report_schedule(&workspace.id, Some(&report.id))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every well-known alias names at least one candidate, so a seeded
    /// Workspace has a route for each of them.
    #[test]
    fn every_well_known_alias_names_a_candidate() {
        for (alias, candidates) in well_known_aliases() {
            assert!(!candidates.is_empty(), "{alias} names no model");
            for candidate in candidates {
                assert!(candidate.contains('/'), "{candidate} names no provider");
            }
        }
    }

    /// The seeded default route is one model, so the seed never makes
    /// an automatic fallback to another provider.
    #[test]
    fn the_seeded_default_route_is_one_model() {
        let (_, default) = well_known_aliases()
            .into_iter()
            .find(|(alias, _)| *alias == DEFAULT_MODEL_ALIAS)
            .expect("the default alias is seeded");
        assert_eq!(default, vec![seed_default_model()]);
    }
}
