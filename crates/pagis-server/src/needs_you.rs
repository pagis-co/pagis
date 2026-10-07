//! The Needs-You Queue (ADR-0022, ADR-0030): what waits for the Person.
//!
//! The daemon derives the queue on each read from the records, and
//! stores no copy of it, so it stays a view. The reader [`NeedsYou`]
//! reads the records of one Workspace, and [`queue`] holds the rules:
//! which records join, in which order, and what each line says.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use pagis_core::{
    AgentId, AgentStore, Call, CallDirection, CallOutcome, CallState, CallStore, FailureKind,
    KeypadFailureStore, KeypadFailures, Request, RequestState, RequestStore, Run, RunId, RunState,
    RunStore, StoreError, Stores, UnixMillis, WorkspaceId, WorkspaceStore, local_date,
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// A pending decision. The row carries the Approve and the Deny.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsYouApproval {
    /// `request:<request_id>`.
    pub id: String,
    pub agent_id: String,
    /// What the item asks of the Person, in one line.
    pub line: String,
    /// The Product App path that answers the item.
    pub url: String,
    /// The time that orders the item inside its kind.
    pub at: i64,
    pub request_id: String,
    pub request_kind: String,
    pub title: String,
    pub body: String,
}

/// A Run that waits for an answer of the Person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsYouWaiting {
    /// `run:<run_id>`.
    pub id: String,
    pub agent_id: String,
    pub line: String,
    pub url: String,
    pub at: i64,
    pub run_id: String,
    pub channel_id: Option<String>,
}

/// Callers entered so many wrong keypad codes that a delay started
/// (ADR-0021). It belongs to the Workspace and to no Agent, and it stays
/// until a correct code or the Person clears the count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsYouKeypad {
    /// `keypad`.
    pub id: String,
    pub line: String,
    pub url: String,
    pub at: i64,
    pub failed_attempts: u32,
    /// The end of the latest delay.
    pub suspended_until: i64,
}

/// An inbound Call of today that nobody answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsYouCall {
    /// `call:<call_id>`.
    pub id: String,
    pub agent_id: String,
    pub line: String,
    pub url: String,
    pub at: i64,
    pub call_id: String,
    /// The Remote Party, in E.164.
    pub remote_e164: String,
    /// The caller left a voicemail message.
    pub left_message: bool,
}

/// A Run that failed today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsYouFailed {
    /// `run:<run_id>`.
    pub id: String,
    pub agent_id: String,
    pub line: String,
    pub url: String,
    pub at: i64,
    pub run_id: String,
    pub channel_id: Option<String>,
    pub failure_kind: Option<FailureKind>,
}

/// One item of the Needs-You Queue. The variants are in the order of
/// the queue: a decision first, then a question, then the keypad delay,
/// then a missed Call, then a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NeedsYouItem {
    Approval(NeedsYouApproval),
    Waiting(NeedsYouWaiting),
    Keypad(NeedsYouKeypad),
    Call(NeedsYouCall),
    Failed(NeedsYouFailed),
}

/// The Needs-You Queue of one Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NeedsYouQueue {
    pub items: Vec<NeedsYouItem>,
    /// The count of `items`.
    pub count: usize,
}

impl NeedsYouItem {
    /// The place of the kind in the queue, most urgent first.
    fn rank(&self) -> u8 {
        match self {
            Self::Approval(_) => 0,
            Self::Waiting(_) => 1,
            Self::Keypad(_) => 2,
            Self::Call(_) => 3,
            Self::Failed(_) => 4,
        }
    }

    fn at(&self) -> UnixMillis {
        match self {
            Self::Approval(item) => item.at,
            Self::Waiting(item) => item.at,
            Self::Keypad(item) => item.at,
            Self::Call(item) => item.at,
            Self::Failed(item) => item.at,
        }
    }
}

/// How many records the reader asks a store for at a time.
const PAGE: u32 = 100;

/// The reader of the Needs-You Queue. It holds the stores of every
/// source and derives the queue of one Workspace on each call.
pub struct NeedsYou {
    requests: Arc<dyn RequestStore>,
    runs: Arc<dyn RunStore>,
    calls: Arc<dyn CallStore>,
    keypad_failures: Arc<dyn KeypadFailureStore>,
    agents: Arc<dyn AgentStore>,
    workspaces: Arc<dyn WorkspaceStore>,
}

impl NeedsYou {
    pub fn new(stores: &Stores) -> Self {
        Self {
            requests: Arc::clone(&stores.requests),
            runs: Arc::clone(&stores.runs),
            calls: Arc::clone(&stores.calls),
            keypad_failures: Arc::clone(&stores.keypad_failures),
            agents: Arc::clone(&stores.agents),
            workspaces: Arc::clone(&stores.workspaces),
        }
    }

    /// The Needs-You Queue of one Workspace at `now`.
    pub async fn derive(
        &self,
        workspace_id: &WorkspaceId,
        now: UnixMillis,
    ) -> Result<NeedsYouQueue, StoreError> {
        let records = self.read(workspace_id, now).await?;
        Ok(queue(&records, now))
    }

    async fn read(
        &self,
        workspace_id: &WorkspaceId,
        now: UnixMillis,
    ) -> Result<Records, StoreError> {
        // A Workspace the store does not hold reads with the Workspace
        // default, UTC, as `local_date` reads a zone it does not know.
        let timezone = self
            .workspaces
            .get(workspace_id)
            .await?
            .map(|workspace| workspace.timezone)
            .unwrap_or_else(|| "UTC".to_string());
        let today = local_date(now, &timezone);
        let agent_names = self
            .agents
            .list_by_workspace(workspace_id)
            .await?
            .into_iter()
            .map(|agent| (agent.id, agent.name))
            .collect();
        let requests = self
            .requests
            .list_by_state(workspace_id, RequestState::Pending, None)
            .await?;
        let mut runs = self
            .runs_in(workspace_id, RunState::WaitingForUser, None)
            .await?;
        runs.extend(
            self.runs_in(workspace_id, RunState::Failed, Some((&today, &timezone)))
                .await?,
        );
        // The Run of each pending Request names the Channel that answers
        // it. A Request parks its Run, so these are few.
        let mut known: HashSet<RunId> = runs.iter().map(|run| run.id.clone()).collect();
        for run_id in requests
            .iter()
            .filter_map(|request| request.run_id.as_ref())
        {
            if known.insert(run_id.clone())
                && let Some(run) = self.runs.get(workspace_id, run_id).await?
            {
                runs.push(run);
            }
        }
        let calls = self
            .ended_inbound_calls(workspace_id, &today, &timezone)
            .await?;
        let keypad = self.keypad_failures.get(workspace_id).await?;
        Ok(Records {
            timezone,
            agent_names,
            requests,
            runs,
            calls,
            keypad,
        })
    }

    /// The Runs of one state, newest first. With `since`, a day and its
    /// time zone, the reading stops after the first page whose oldest Run
    /// started before that day. A Run that started on an earlier day and
    /// ended on that day is read only when it is in a page that is read.
    async fn runs_in(
        &self,
        workspace_id: &WorkspaceId,
        state: RunState,
        since: Option<(&str, &str)>,
    ) -> Result<Vec<Run>, StoreError> {
        let mut runs: Vec<Run> = Vec::new();
        loop {
            let before = runs.last().map(|run| run.id.clone());
            let page = self
                .runs
                .list(workspace_id, None, None, &[state], before.as_ref(), PAGE)
                .await?;
            let last_page = page.len() < PAGE as usize
                || since.is_some_and(|(day, timezone)| {
                    page.last()
                        .is_some_and(|run| local_date(run.created_at, timezone) != day)
                });
            runs.extend(page);
            if last_page {
                return Ok(runs);
            }
        }
    }

    /// The ended inbound Calls, newest first, read as far as the page
    /// whose oldest Call started before `day`.
    async fn ended_inbound_calls(
        &self,
        workspace_id: &WorkspaceId,
        day: &str,
        timezone: &str,
    ) -> Result<Vec<Call>, StoreError> {
        let mut calls: Vec<Call> = Vec::new();
        loop {
            let before = calls.last().map(|call| call.id.clone());
            let page = self
                .calls
                .list(
                    workspace_id,
                    None,
                    Some(CallDirection::Inbound),
                    Some(CallState::Ended),
                    before.as_ref(),
                    PAGE,
                )
                .await?;
            let last_page = page.len() < PAGE as usize
                || page
                    .last()
                    .is_some_and(|call| local_date(call.created_at, timezone) != day);
            calls.extend(page);
            if last_page {
                return Ok(calls);
            }
        }
    }
}

/// The records one derivation reads, as the stores answer them.
#[derive(Debug, Default)]
struct Records {
    /// The IANA time zone of the Workspace. "Today" is its day.
    timezone: String,
    /// The name of each Agent of the Workspace.
    agent_names: HashMap<AgentId, String>,
    requests: Vec<Request>,
    /// The Runs that wait for the Person or failed, and the Run of each
    /// pending Request.
    runs: Vec<Run>,
    calls: Vec<Call>,
    keypad: KeypadFailures,
}

/// The outcomes an inbound Call reaches when nobody spoke to the caller
/// (ADR-0020).
const MISSED_OUTCOMES: [CallOutcome; 4] = [
    CallOutcome::NoAnswer,
    CallOutcome::Busy,
    CallOutcome::Voicemail,
    CallOutcome::Failed,
];

/// The name of an Agent that the store does not hold, as Home names it.
const UNKNOWN_AGENT: &str = "A sprite";

/// The Needs-You Queue of a set of records at `now`.
///
/// A decision comes first, then a question, then the keypad delay, then
/// a Call nobody answered, then a failure; inside one kind the newest is
/// first. Only the Calls and the failures of today join, so the queue
/// stays a queue and not a record, and a Call or a failure the Person
/// dismissed leaves it.
fn queue(records: &Records, now: UnixMillis) -> NeedsYouQueue {
    let today = local_date(now, &records.timezone);
    let is_today = |at: UnixMillis| local_date(at, &records.timezone) == today;
    let name = |agent_id: &AgentId| {
        records
            .agent_names
            .get(agent_id)
            .map_or(UNKNOWN_AGENT, String::as_str)
    };
    let runs: HashMap<&RunId, &Run> = records.runs.iter().map(|run| (&run.id, run)).collect();

    let approvals = records
        .requests
        .iter()
        .filter(|request| request.state == RequestState::Pending)
        .map(|request| {
            let url = match &request.run_id {
                Some(run_id) => match runs.get(run_id) {
                    Some(run) => conversation_url(run),
                    None => format!("/runs/{run_id}"),
                },
                None => "/".to_string(),
            };
            NeedsYouItem::Approval(NeedsYouApproval {
                id: format!("request:{}", request.id),
                agent_id: request.agent_id.to_string(),
                line: format!("{} needs your approval", name(&request.agent_id)),
                url,
                at: request.created_at,
                request_id: request.id.to_string(),
                request_kind: request.kind.clone(),
                title: payload_text(&request.payload, &["action_title", "tool_name", "domain"])
                    .unwrap_or("An action")
                    .to_string(),
                body: payload_text(&request.payload, &["body", "tool_name"])
                    .unwrap_or_default()
                    .to_string(),
            })
        });

    let waiting = records
        .runs
        .iter()
        .filter(|run| run.state == RunState::WaitingForUser)
        .map(|run| {
            NeedsYouItem::Waiting(NeedsYouWaiting {
                id: format!("run:{}", run.id),
                agent_id: run.agent_id.to_string(),
                line: format!("{} waits for your answer", name(&run.agent_id)),
                url: conversation_url(run),
                at: run.created_at,
                run_id: run.id.to_string(),
                channel_id: run.channel_id.as_ref().map(ToString::to_string),
            })
        });

    let keypad = records.keypad.suspended_until.map(|until| {
        NeedsYouItem::Keypad(NeedsYouKeypad {
            id: "keypad".to_string(),
            line: format!(
                "Callers entered a wrong keypad code {} times",
                records.keypad.failed_attempts
            ),
            url: "/".to_string(),
            at: until,
            failed_attempts: records.keypad.failed_attempts,
            suspended_until: until,
        })
    });

    let missed_calls = records
        .calls
        .iter()
        .filter(|call| {
            call.direction == CallDirection::Inbound
                && call.state == CallState::Ended
                && call
                    .outcome
                    .is_some_and(|outcome| MISSED_OUTCOMES.contains(&outcome))
                && call.dismissed_at.is_none()
                && is_today(call.ended_at.unwrap_or(call.created_at))
        })
        .map(|call| {
            NeedsYouItem::Call(NeedsYouCall {
                id: format!("call:{}", call.id),
                agent_id: call.agent_id.to_string(),
                line: format!(
                    "{} missed a call from {}",
                    name(&call.agent_id),
                    call.remote_e164
                ),
                url: "/".to_string(),
                at: call.ended_at.unwrap_or(call.created_at),
                call_id: call.id.to_string(),
                remote_e164: call.remote_e164.clone(),
                left_message: call.outcome == Some(CallOutcome::Voicemail),
            })
        });

    let failures = records
        .runs
        .iter()
        .filter(|run| {
            run.state == RunState::Failed
                && run.dismissed_at.is_none()
                && is_today(run.ended_at.unwrap_or(run.created_at))
        })
        .map(|run| {
            NeedsYouItem::Failed(NeedsYouFailed {
                id: format!("run:{}", run.id),
                agent_id: run.agent_id.to_string(),
                line: format!("{} could not finish the work", name(&run.agent_id)),
                url: format!("/runs/{}", run.id),
                at: run.ended_at.unwrap_or(run.created_at),
                run_id: run.id.to_string(),
                channel_id: run.channel_id.as_ref().map(ToString::to_string),
                failure_kind: run.failure_kind,
            })
        });

    let mut items: Vec<NeedsYouItem> = approvals
        .chain(waiting)
        .chain(keypad)
        .chain(missed_calls)
        .chain(failures)
        .collect();
    items.sort_by(|left, right| {
        left.rank()
            .cmp(&right.rank())
            .then_with(|| right.at().cmp(&left.at()))
    });
    NeedsYouQueue {
        count: items.len(),
        items,
    }
}

/// The conversation of a Run, or its run page when it has no Channel.
fn conversation_url(run: &Run) -> String {
    match &run.channel_id {
        Some(channel_id) => format!("/c/{channel_id}"),
        None => format!("/runs/{}", run.id),
    }
}

/// The first of `fields` that the payload holds as text.
fn payload_text<'a>(payload: &'a serde_json::Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| payload.get(field).and_then(serde_json::Value::as_str))
}

/// The Needs-You Queue of the Person's Workspace.
#[utoipa::path(
    get,
    path = "/api/v1/needs-you",
    responses(
        (status = 200, body = NeedsYouQueue),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn get_needs_you(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<NeedsYouQueue>, ApiError> {
    let queue = state
        .needs_you
        .derive(&tenant.workspace_id, state.clock.now_ms())
        .await?;
    Ok(Json(queue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{CallId, ChannelId, PhoneNumberId, RequestId, TriggerKind, TrustTier};

    /// 2026-09-25 12:00 UTC.
    const NOW: UnixMillis = 1_790_337_600_000;
    const DAY: UnixMillis = 86_400_000;
    const YESTERDAY: UnixMillis = NOW - DAY;

    fn records() -> Records {
        Records {
            timezone: "UTC".to_string(),
            agent_names: HashMap::from([(AgentId::from("agent-1".to_string()), "Sage".into())]),
            ..Records::default()
        }
    }

    fn run(id: &str, state: RunState) -> Run {
        Run {
            id: RunId::from(id.to_string()),
            workspace_id: WorkspaceId::from("workspace-1".to_string()),
            agent_id: AgentId::from("agent-1".to_string()),
            channel_id: Some(ChannelId::from("channel-1".to_string())),
            root_message_id: None,
            trigger_kind: TriggerKind::Message,
            trigger_ref: None,
            hop_count: 0,
            origin: None,
            state,
            failure_kind: None,
            error: None,
            started_at: Some(NOW),
            ended_at: Some(NOW),
            created_at: NOW,
            dismissed_at: None,
        }
    }

    fn call(id: &str) -> Call {
        Call {
            id: CallId::from(id.to_string()),
            workspace_id: WorkspaceId::from("workspace-1".to_string()),
            agent_id: AgentId::from("agent-1".to_string()),
            run_id: RunId::from("call-run".to_string()),
            phone_number_id: PhoneNumberId::from("number-1".to_string()),
            direction: CallDirection::Inbound,
            remote_e164: "+14155550199".to_string(),
            agent_name: "Sage".to_string(),
            own_e164: "+14155550123".to_string(),
            purpose: String::new(),
            tools: Vec::new(),
            tier: TrustTier::Unknown,
            state: CallState::Ended,
            outcome: Some(CallOutcome::NoAnswer),
            ended_reason: Some("no_answer".to_string()),
            classification: None,
            message_left: false,
            transcript: Vec::new(),
            recording_artifact_id: None,
            created_at: NOW,
            ringing_at: Some(NOW),
            answered_at: None,
            ended_at: Some(NOW),
            dismissed_at: None,
        }
    }

    fn request(id: &str) -> Request {
        Request {
            id: RequestId::from(id.to_string()),
            workspace_id: WorkspaceId::from("workspace-1".to_string()),
            agent_id: AgentId::from("agent-1".to_string()),
            run_id: Some(RunId::from("run-1".to_string())),
            kind: Request::TOOL_ACTION_KIND.to_string(),
            payload: serde_json::json!({"action_title": "Open a file", "body": "host__read"}),
            state: RequestState::Pending,
            values: None,
            decided_at: None,
            created_at: NOW,
        }
    }

    /// The items as the wire carries them.
    fn wire(queue: &NeedsYouQueue) -> Vec<serde_json::Value> {
        queue
            .items
            .iter()
            .map(|item| serde_json::to_value(item).expect("an item serializes"))
            .collect()
    }

    /// Each item as `<kind> <id>`.
    fn rows(queue: &NeedsYouQueue) -> Vec<String> {
        wire(queue)
            .iter()
            .map(|item| format!("{} {}", item["kind"], item["id"]).replace('"', ""))
            .collect()
    }

    fn kinds(queue: &NeedsYouQueue) -> Vec<String> {
        wire(queue)
            .iter()
            .map(|item| item["kind"].as_str().expect("a kind").to_string())
            .collect()
    }

    fn lines(queue: &NeedsYouQueue) -> Vec<String> {
        wire(queue)
            .iter()
            .map(|item| item["line"].as_str().expect("a line").to_string())
            .collect()
    }

    #[test]
    fn it_takes_the_pending_approvals_the_waiting_runs_and_the_failures_of_today() {
        let mut approved = request("request-2");
        approved.state = RequestState::Approved;
        let mut failed = run("run-2", RunState::Failed);
        failed.error = Some("the request timed out".to_string());
        let records = Records {
            requests: vec![request("request-1"), approved],
            runs: vec![
                run("run-9", RunState::WaitingForUser),
                run("run-8", RunState::Running),
                failed,
                run("run-3", RunState::Completed),
            ],
            ..records()
        };

        let queue = queue(&records, NOW);

        assert_eq!(
            rows(&queue),
            [
                "approval request:request-1",
                "waiting run:run-9",
                "failed run:run-2"
            ]
        );
        assert_eq!(queue.count, 3);
    }

    #[test]
    fn it_leaves_out_a_failure_from_an_earlier_day() {
        let mut old = run("old", RunState::Failed);
        old.created_at = YESTERDAY;
        old.ended_at = Some(YESTERDAY);
        let records = Records {
            runs: vec![old],
            ..records()
        };

        let queue = queue(&records, NOW);

        assert!(queue.items.is_empty());
        assert_eq!(queue.count, 0);
    }

    #[test]
    fn it_takes_the_inbound_calls_of_today_that_nobody_answered_before_the_failures() {
        let mut voicemail = call("voicemail");
        voicemail.outcome = Some(CallOutcome::Voicemail);
        voicemail.ended_at = Some(NOW - 1_000);
        let mut answered = call("answered");
        answered.outcome = Some(CallOutcome::Answered);
        let mut outbound = call("outbound");
        outbound.direction = CallDirection::Outbound;
        outbound.outcome = Some(CallOutcome::Busy);
        let mut live = call("live");
        live.state = CallState::Live;
        live.outcome = None;
        live.ended_at = None;
        let mut old = call("old");
        old.outcome = Some(CallOutcome::Failed);
        old.ended_at = Some(YESTERDAY);
        let records = Records {
            runs: vec![run("plain", RunState::Failed)],
            calls: vec![call("missed"), voicemail, answered, outbound, live, old],
            ..records()
        };

        let queue = queue(&records, NOW);

        assert_eq!(
            rows(&queue),
            [
                "call call:missed",
                "call call:voicemail",
                "failed run:plain"
            ]
        );
        let wire = wire(&queue);
        assert_eq!(wire[0]["left_message"], false);
        assert_eq!(wire[1]["left_message"], true);
        assert_eq!(wire[1]["remote_e164"], "+14155550199");
    }

    #[test]
    fn it_takes_each_outcome_of_a_call_that_nobody_answered() {
        let outcomes = [
            CallOutcome::NoAnswer,
            CallOutcome::Busy,
            CallOutcome::Voicemail,
            CallOutcome::Failed,
        ];
        let calls = outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| {
                let mut missed = call(outcome.as_str());
                missed.outcome = Some(*outcome);
                missed.ended_at = Some(NOW - index as i64);
                missed
            })
            .collect();
        let records = Records { calls, ..records() };

        assert_eq!(
            rows(&queue(&records, NOW)),
            [
                "call call:no_answer",
                "call call:busy",
                "call call:voicemail",
                "call call:failed"
            ]
        );
    }

    #[test]
    fn it_leaves_out_a_failure_and_a_missed_call_the_person_dismissed() {
        let mut dismissed_run = run("dismissed", RunState::Failed);
        dismissed_run.dismissed_at = Some(NOW);
        let mut dismissed_call = call("dismissed");
        dismissed_call.dismissed_at = Some(NOW);
        let records = Records {
            runs: vec![dismissed_run, run("open", RunState::Failed)],
            calls: vec![dismissed_call, call("open")],
            ..records()
        };

        assert_eq!(
            rows(&queue(&records, NOW)),
            ["call call:open", "failed run:open"]
        );
    }

    #[test]
    fn it_does_not_read_the_failed_run_of_an_answered_call_as_a_missed_call() {
        let mut call_run = run("call-run", RunState::Failed);
        call_run.failure_kind = Some(FailureKind::CallFailed);
        call_run.error = Some("no_answer".to_string());
        let mut answered = call("answered");
        answered.outcome = Some(CallOutcome::Answered);
        let records = Records {
            runs: vec![call_run],
            calls: vec![answered],
            ..records()
        };

        let queue = queue(&records, NOW);

        assert_eq!(kinds(&queue), ["failed"]);
        assert_eq!(wire(&queue)[0]["failure_kind"], "call_failed");
    }

    #[test]
    fn the_newest_item_of_one_kind_comes_first() {
        let mut older = request("older");
        older.created_at = NOW - 60_000;
        let mut early = run("early", RunState::Failed);
        early.ended_at = Some(NOW - 60_000);
        let records = Records {
            requests: vec![older, request("newer")],
            runs: vec![early, run("late", RunState::Failed)],
            ..records()
        };

        assert_eq!(
            rows(&queue(&records, NOW)),
            [
                "approval request:newer",
                "approval request:older",
                "failed run:late",
                "failed run:early"
            ]
        );
    }

    #[test]
    fn the_keypad_item_joins_when_a_delay_starts_and_names_the_end_of_the_delay() {
        let until = NOW + 60_000;
        let records = Records {
            requests: vec![request("request-1")],
            calls: vec![call("missed")],
            keypad: KeypadFailures {
                failed_attempts: 6,
                suspended_until: Some(until),
            },
            ..records()
        };

        let queue = queue(&records, NOW);

        assert_eq!(kinds(&queue), ["approval", "keypad", "call"]);
        let keypad = &wire(&queue)[1];
        assert_eq!(keypad["id"], "keypad");
        assert_eq!(
            keypad["line"],
            "Callers entered a wrong keypad code 6 times"
        );
        assert_eq!(keypad["failed_attempts"], 6);
        assert_eq!(keypad["suspended_until"], until);
        assert_eq!(keypad["url"], "/");
        assert!(keypad.get("agent_id").is_none());
    }

    #[test]
    fn the_keypad_item_stays_after_the_delay_ends_until_the_count_is_cleared() {
        let records = Records {
            keypad: KeypadFailures {
                failed_attempts: 7,
                suspended_until: Some(NOW - 60_000),
            },
            ..records()
        };

        assert_eq!(rows(&queue(&records, NOW)), ["keypad keypad"]);
    }

    #[test]
    fn the_keypad_item_stays_out_while_no_delay_has_started() {
        let records = Records {
            keypad: KeypadFailures {
                failed_attempts: 5,
                suspended_until: None,
            },
            ..records()
        };

        assert!(queue(&records, NOW).items.is_empty());
    }

    #[test]
    fn each_line_says_what_the_item_asks_of_the_person() {
        let records = Records {
            requests: vec![request("request-1")],
            runs: vec![
                run("run-9", RunState::WaitingForUser),
                run("run-2", RunState::Failed),
            ],
            calls: vec![call("missed")],
            ..records()
        };

        assert_eq!(
            lines(&queue(&records, NOW)),
            [
                "Sage needs your approval",
                "Sage waits for your answer",
                "Sage missed a call from +14155550199",
                "Sage could not finish the work",
            ]
        );
    }

    #[test]
    fn an_agent_the_store_does_not_hold_reads_as_a_sprite() {
        let records = Records {
            agent_names: HashMap::new(),
            requests: vec![request("request-1")],
            ..records()
        };

        assert_eq!(
            lines(&queue(&records, NOW)),
            ["A sprite needs your approval"]
        );
    }

    #[test]
    fn no_line_says_agent() {
        // The rule of `ui/src/copy.test.ts`: the copy says sprite, and the
        // word agent alone is not copy.
        let records = Records {
            agent_names: HashMap::new(),
            requests: vec![request("request-1")],
            runs: vec![
                run("run-9", RunState::WaitingForUser),
                run("run-2", RunState::Failed),
            ],
            calls: vec![call("missed")],
            keypad: KeypadFailures {
                failed_attempts: 6,
                suspended_until: Some(NOW),
            },
            ..records()
        };

        let lines = lines(&queue(&records, NOW));

        assert_eq!(lines.len(), 5);
        for line in lines {
            let says_agent = line
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| matches!(word.to_lowercase().as_str(), "agent" | "agents"));
            assert!(!says_agent, "the line {line:?} says agent");
        }
    }

    #[test]
    fn today_is_the_day_of_the_workspace_in_its_time_zone() {
        // Noon on Friday, September 25, 2026, in California (19:00 UTC).
        let now = 1_790_362_800_000;
        // 23:30 on Thursday in California, and 06:30 on Friday in UTC.
        let mut yesterday = run("yesterday", RunState::Failed);
        yesterday.ended_at = Some(1_790_317_800_000);
        // 00:30 on Friday in California.
        let mut today = run("today", RunState::Failed);
        today.ended_at = Some(1_790_321_400_000);
        let records = Records {
            timezone: "America/Los_Angeles".to_string(),
            runs: vec![yesterday, today],
            ..records()
        };

        assert_eq!(rows(&queue(&records, now)), ["failed run:today"]);
    }

    #[test]
    fn each_item_names_the_place_that_answers_it() {
        let mut no_channel = run("no-channel", RunState::Failed);
        no_channel.channel_id = None;
        let mut parked = run("run-1", RunState::WaitingForApproval);
        parked.channel_id = Some(ChannelId::from("channel-7".to_string()));
        let mut unparked = request("unparked");
        unparked.run_id = None;
        unparked.created_at = NOW - 1;
        let records = Records {
            requests: vec![request("request-1"), unparked],
            runs: vec![parked, run("run-9", RunState::WaitingForUser), no_channel],
            calls: vec![call("missed")],
            ..records()
        };

        let urls: Vec<_> = wire(&queue(&records, NOW))
            .iter()
            .map(|item| item["url"].as_str().expect("a url").to_string())
            .collect();

        assert_eq!(
            urls,
            ["/c/channel-7", "/", "/c/channel-1", "/", "/runs/no-channel"]
        );
    }

    #[test]
    fn an_approval_carries_its_title_and_body_from_the_payload() {
        let mut by_tool = request("by-tool");
        by_tool.payload = serde_json::json!({"tool_name": "host_shell"});
        by_tool.created_at = NOW - 1;
        let mut by_domain = request("by-domain");
        by_domain.payload = serde_json::json!({"domain": "example.com", "body": "Sign in"});
        by_domain.created_at = NOW - 2;
        let mut bare = request("bare");
        bare.kind = Request::FORM_KIND.to_string();
        bare.payload = serde_json::json!({});
        bare.created_at = NOW - 3;
        let records = Records {
            requests: vec![request("titled"), by_tool, by_domain, bare],
            ..records()
        };

        let cards: Vec<_> = wire(&queue(&records, NOW))
            .iter()
            .map(|item| {
                format!(
                    "{} | {} | {} | {}",
                    item["request_id"], item["request_kind"], item["title"], item["body"]
                )
                .replace('"', "")
            })
            .collect();

        assert_eq!(
            cards,
            [
                "titled | tool_action | Open a file | host__read",
                "by-tool | tool_action | host_shell | host_shell",
                "by-domain | tool_action | example.com | Sign in",
                "bare | form | An action | ",
            ]
        );
    }
}
