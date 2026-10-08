//! The steps of a Run for the work record. The server folds
//! the Run's `tool.called` and `tool.completed` events into steps, so
//! the UI reads timings, tool families and screen references as one
//! list.

use pagis_core::{Event, Run, RunState};
use serde::Serialize;
use utoipa::ToSchema;

/// The tool family a step used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStepKind {
    Memory,
    Mail,
    Calendar,
    Desk,
    Plugin,
}

impl RunStepKind {
    /// The family of a tool name. First-party tools carry their family
    /// in the name; every other tool is a plugin tool.
    fn of_tool(name: &str) -> Self {
        if name.starts_with("memory_") {
            Self::Memory
        } else if name.starts_with("mail__") {
            Self::Mail
        } else if name.contains("calendar") {
            Self::Calendar
        } else if matches!(name, "computer" | "computer_shell" | "host_shell") {
            Self::Desk
        } else {
            Self::Plugin
        }
    }
}

/// What a first-party tool did, in the words the work record reads.
/// The step line is prose a user acts on, so a tool identifier never
/// reaches it. A tool that is not listed here is a plugin tool, whose
/// name is the only thing the daemon knows about it.
fn step_label(name: &str) -> String {
    let known = match name {
        "memory_search" => "Searched memory",
        "memory_read" => "Read from memory",
        "memory_write" => "Wrote to memory",
        "memory_delete" => "Removed a page from memory",
        "mail__search" => "Searched your mail",
        "mail__get_message" => "Read a message",
        "mail__get_thread" => "Read a mail thread",
        "mail__modify_message" => "Filed a message",
        "mail__send" => "Sent mail",
        "google__calendar_events" => "Read your calendar",
        "google__calendar_create_event" => "Put an event on your calendar",
        "google__calendar_update_event" => "Changed an event on your calendar",
        "google__calendar_delete_event" => "Took an event off your calendar",
        "google__calendar_respond_event" => "Answered an invitation",
        "computer" | "computer_shell" | "host_shell" => "Worked at the desk",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    // A plugin tool: `acme__list_orders` reads as `acme: list orders`.
    match name.split_once("__") {
        Some((plugin, tool)) => format!("{plugin}: {}", tool.replace('_', " ")),
        None => name.replace('_', " "),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RunStepDto {
    /// The step's position in the Run, from 1.
    pub index: u32,
    /// The tool name in words.
    pub label: String,
    pub kind: RunStepKind,
    pub started_at: i64,
    /// Null while the step is live.
    pub ended_at: Option<i64>,
    pub duration_ms: Option<i64>,
    /// The Artifact of the screenshot taken during the step, when there
    /// is one.
    pub screenshot_id: Option<String>,
    /// True on the step the Run works on now.
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RunStopDto {
    /// Who stopped the Run: `user`.
    pub by: String,
    /// The time the Run worked before the stop.
    pub after_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RunStepsDto {
    pub run_id: String,
    pub state: String,
    pub steps: Vec<RunStepDto>,
    /// The time from the Run's start to its end, or null while it works.
    pub worked_ms: Option<i64>,
    /// Present on a stopped Run.
    pub stopped: Option<RunStopDto>,
    /// The reason line of a failed Run.
    pub failure: Option<String>,
}

/// Fold a Run's events into its steps. A `tool.called` event opens a
/// step; the next `tool.completed` event closes it. A step still open
/// when the Run ends closes at the Run's end.
pub fn fold_steps(run: &Run, events: &[Event]) -> RunStepsDto {
    let mut steps: Vec<RunStepDto> = Vec::new();
    let mut stop_reason: Option<String> = None;
    for event in events {
        match event.event_type.as_str() {
            "tool.called" => {
                let name = event.payload["name"].as_str().unwrap_or("tool");
                steps.push(RunStepDto {
                    index: steps.len() as u32 + 1,
                    label: step_label(name),
                    kind: RunStepKind::of_tool(name),
                    started_at: event.created_at,
                    ended_at: None,
                    duration_ms: None,
                    screenshot_id: None,
                    live: false,
                });
            }
            "tool.completed" => {
                if let Some(step) = steps.last_mut().filter(|step| step.ended_at.is_none()) {
                    step.ended_at = Some(event.created_at);
                    step.duration_ms = Some(event.created_at - step.started_at);
                    step.screenshot_id = event.payload["artifact_ids"][0]
                        .as_str()
                        .map(str::to_string);
                }
            }
            "run.state_changed" if event.payload["to"] == "canceled" => {
                stop_reason = event.payload["reason"].as_str().map(str::to_string);
            }
            _ => {}
        }
    }
    let terminal = run.state.is_terminal();
    if let Some(step) = steps.last_mut().filter(|step| step.ended_at.is_none()) {
        if terminal {
            step.ended_at = run.ended_at;
            step.duration_ms = run.ended_at.map(|ended| ended - step.started_at);
        } else {
            step.live = true;
        }
    }
    let worked_ms = run
        .started_at
        .or(Some(run.created_at))
        .zip(run.ended_at)
        .map(|(started, ended)| ended - started);
    let stopped = (run.state == RunState::Canceled).then(|| RunStopDto {
        by: stop_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("canceled by "))
            .unwrap_or("user")
            .to_string(),
        after_ms: worked_ms.unwrap_or_default(),
    });
    let failure = (run.state == RunState::Failed)
        .then(|| run.error.clone().unwrap_or_else(|| "Failed".to_string()));
    RunStepsDto {
        run_id: run.id.to_string(),
        state: run.state.as_str().to_string(),
        steps,
        worked_ms,
        stopped,
        failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{AgentId, EventId, RunId, TriggerKind, WorkspaceId};

    fn run(state: RunState, started_at: Option<i64>, ended_at: Option<i64>) -> Run {
        Run {
            title: "A message with an attachment".into(),
            id: RunId::from("run-1".to_string()),
            workspace_id: WorkspaceId::from("ws".to_string()),
            agent_id: AgentId::from("agent".to_string()),
            channel_id: None,
            root_message_id: None,
            trigger_kind: TriggerKind::Message,
            trigger_ref: None,
            hop_count: 0,
            origin: None,
            state,
            failure_kind: None,
            dismissed_at: None,
            error: None,
            started_at,
            ended_at,
            created_at: 1_000,
        }
    }

    fn event(event_type: &str, payload: serde_json::Value, created_at: i64) -> Event {
        Event {
            id: EventId::generate(),
            seq: created_at,
            workspace_id: WorkspaceId::from("ws".to_string()),
            event_type: event_type.to_string(),
            agent_id: None,
            run_id: Some(RunId::from("run-1".to_string())),
            channel_id: None,
            payload,
            created_at,
        }
    }

    fn called(name: &str, at: i64) -> Event {
        event("tool.called", serde_json::json!({ "name": name }), at)
    }

    fn completed(name: &str, at: i64) -> Event {
        event(
            "tool.completed",
            serde_json::json!({ "name": name, "ok": true }),
            at,
        )
    }

    #[test]
    fn a_completed_run_folds_its_tool_calls_into_timed_steps() {
        let run = run(RunState::Completed, Some(1_000), Some(30_000));
        let events = vec![
            event("turn.started", serde_json::json!({ "turn": 1 }), 1_000),
            called("memory_read", 2_000),
            completed("memory_read", 5_000),
            called("mail__search", 6_000),
            completed("mail__search", 15_000),
            called("google__calendar_events", 16_000),
            completed("google__calendar_events", 20_000),
        ];

        let folded = fold_steps(&run, &events);

        assert_eq!(folded.state, "completed");
        assert_eq!(folded.worked_ms, Some(29_000));
        assert_eq!(folded.steps.len(), 3);
        let first = &folded.steps[0];
        assert_eq!(first.index, 1);
        assert_eq!(first.label, "Read from memory");
        assert_eq!(first.kind, RunStepKind::Memory);
        assert_eq!(first.started_at, 2_000);
        assert_eq!(first.ended_at, Some(5_000));
        assert_eq!(first.duration_ms, Some(3_000));
        assert!(!first.live);
        assert_eq!(folded.steps[1].kind, RunStepKind::Mail);
        assert_eq!(folded.steps[1].label, "Searched your mail");
        assert_eq!(folded.steps[2].kind, RunStepKind::Calendar);
        assert_eq!(folded.steps[2].label, "Read your calendar");
        assert!(folded.steps.iter().all(|step| !step.live));
        assert!(folded.stopped.is_none());
        assert!(folded.failure.is_none());
    }

    #[test]
    fn a_step_reads_as_prose_and_never_as_a_tool_name() {
        for (name, label) in [
            ("memory_search", "Searched memory"),
            ("memory_write", "Wrote to memory"),
            ("mail__search", "Searched your mail"),
            (
                "google__calendar_create_event",
                "Put an event on your calendar",
            ),
            ("computer", "Worked at the desk"),
        ] {
            assert_eq!(step_label(name), label);
        }
    }

    #[test]
    fn a_plugin_step_names_its_plugin_and_reads_its_tool_in_words() {
        assert_eq!(step_label("acme__list_orders"), "acme: list orders");
        assert_eq!(step_label("standalone_tool"), "standalone tool");
    }

    #[test]
    fn a_mail_step_belongs_to_the_mail_family() {
        assert_eq!(RunStepKind::of_tool("mail__search"), RunStepKind::Mail);
    }

    #[test]
    fn the_open_step_of_a_running_run_is_live() {
        let run = run(RunState::Running, Some(1_000), None);
        let events = vec![
            called("memory_read", 2_000),
            completed("memory_read", 5_000),
            called("mail__search", 6_000),
        ];

        let folded = fold_steps(&run, &events);

        assert_eq!(folded.worked_ms, None);
        assert!(!folded.steps[0].live);
        let live = &folded.steps[1];
        assert!(live.live);
        assert_eq!(live.ended_at, None);
        assert_eq!(live.duration_ms, None);
    }

    #[test]
    fn a_desk_step_carries_its_screenshot() {
        let run = run(RunState::Running, Some(1_000), None);
        let events = vec![
            called("computer", 2_000),
            event(
                "tool.completed",
                serde_json::json!({ "name": "computer", "ok": true, "artifact_ids": ["art-1"] }),
                4_000,
            ),
            called("host_shell", 5_000),
            completed("host_shell", 6_000),
            called("weather__forecast", 7_000),
            completed("weather__forecast", 8_000),
        ];

        let folded = fold_steps(&run, &events);

        assert_eq!(folded.steps[0].kind, RunStepKind::Desk);
        assert_eq!(folded.steps[0].screenshot_id.as_deref(), Some("art-1"));
        assert_eq!(folded.steps[1].kind, RunStepKind::Desk);
        assert_eq!(folded.steps[1].screenshot_id, None);
        assert_eq!(folded.steps[2].kind, RunStepKind::Plugin);
        assert_eq!(folded.steps[2].label, "weather: forecast");
    }

    #[test]
    fn a_stopped_run_names_who_stopped_it_and_closes_the_open_step() {
        let run = run(RunState::Canceled, Some(1_000), Some(13_000));
        let events = vec![
            called("mail__search", 2_000),
            event(
                "run.state_changed",
                serde_json::json!({ "from": "running", "to": "canceled", "reason": "canceled by user" }),
                13_000,
            ),
        ];

        let folded = fold_steps(&run, &events);

        assert_eq!(
            folded.stopped,
            Some(RunStopDto {
                by: "user".to_string(),
                after_ms: 12_000,
            })
        );
        assert_eq!(folded.steps[0].ended_at, Some(13_000));
        assert!(!folded.steps[0].live);
        assert!(folded.failure.is_none());
    }

    #[test]
    fn a_failed_run_reports_its_reason_line() {
        let mut run = run(RunState::Failed, Some(1_000), Some(4_000));
        run.error = Some("model call failed: rate limited".to_string());

        let folded = fold_steps(&run, &[]);

        assert_eq!(
            folded.failure.as_deref(),
            Some("model call failed: rate limited")
        );
        assert!(folded.stopped.is_none());
        assert!(folded.steps.is_empty());
    }
}
