//! The Schedule core tools (ADR-0006).

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CoreTool, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::{AgentId, ChannelId, ChannelStore, Clock, CreatorKind, ScheduleId, WorkspaceId};
use pagis_trigger::{NewSchedule, ScheduleEdit, ScheduleTiming, Trigger, TriggerError};

const USER_CHANNEL: &str = "user";

pub struct ScheduleToolRuntime {
    trigger: Arc<Trigger>,
    channels: Arc<dyn ChannelStore>,
    clock: Arc<dyn Clock>,
    memory: Arc<pagis_agent::CoreToolRuntime>,
}

impl ScheduleToolRuntime {
    pub fn new(
        trigger: Arc<Trigger>,
        channels: Arc<dyn ChannelStore>,
        clock: Arc<dyn Clock>,
        memory: Arc<pagis_agent::CoreToolRuntime>,
    ) -> Self {
        Self {
            trigger,
            channels,
            clock,
            memory,
        }
    }

    pub fn owns(route: &ToolRoute) -> bool {
        matches!(
            route,
            ToolRoute::Core {
                tool: CoreTool::ScheduleCreate | CoreTool::ScheduleList | CoreTool::ScheduleUpdate
            }
        )
    }

    async fn create(&self, call: &AuthorizedCall) -> ToolResult {
        let channel_id = match self
            .channel(
                &call.workspace_id,
                &call.agent_id,
                call.arguments["channel"].as_str(),
            )
            .await
        {
            Ok(channel_id) => channel_id,
            Err(result) => return result,
        };
        let timing = match timing(&call.arguments) {
            Ok(timing) => timing,
            Err(result) => return result,
        };
        let subject_page_path = match subject_page_path(&call.arguments) {
            Ok(path) => path,
            Err(result) => return result,
        };
        match self
            .trigger
            .create_schedule(NewSchedule {
                workspace_id: call.workspace_id.clone(),
                agent_id: call.agent_id.clone(),
                name: call.arguments["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                instruction: call.arguments["instruction"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                subject_page_path,
                channel_id,
                root_message_id: None,
                timing,
                creator: CreatorKind::Agent,
                creating_run_id: Some(call.run_id.clone()),
                now: self.clock.now_ms(),
            })
            .await
        {
            Ok(schedule) => {
                self.refresh_page(call, &schedule).await;
                schedule_result(&schedule)
            }
            Err(error) => trigger_error(error),
        }
    }

    async fn list(&self, call: &AuthorizedCall) -> ToolResult {
        match self
            .trigger
            .list_schedules(&call.workspace_id, None, 1_000)
            .await
        {
            Ok(schedules) => {
                let items: Vec<_> = schedules
                    .into_iter()
                    .filter(|schedule| schedule.agent_id == call.agent_id)
                    .map(|schedule| {
                        serde_json::json!({
                            "schedule_id": schedule.id.as_str(),
                            "name": schedule.name,
                            "instruction": schedule.instruction,
                            "subject_page_path": schedule.subject_page_path,
                            "kind": schedule.kind.as_str(),
                            "state": schedule.state.as_str(),
                            "revision": schedule.revision,
                            "approved_revision": schedule.approved_revision,
                            "next_due_at": schedule.next_due_at,
                            "last_result": schedule.last_result,
                        })
                    })
                    .collect();
                ToolResult::success(serde_json::json!({"items": items}).to_string())
            }
            Err(error) => trigger_error(error),
        }
    }

    async fn update(&self, call: &AuthorizedCall) -> ToolResult {
        let id = ScheduleId::from(
            call.arguments["schedule_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        let schedule = match self.trigger.get_schedule(&call.workspace_id, &id).await {
            Ok(Some(schedule)) if schedule.agent_id == call.agent_id => schedule,
            Ok(_) => return ToolResult::error("not_found", "there is no such Schedule"),
            Err(error) => return trigger_error(error),
        };
        if call.arguments["wake_only"] == true
            && call.arguments["subject_page_path"].as_str() != schedule.subject_page_path.as_deref()
        {
            return ToolResult::error(
                "invalid_request",
                "subject_page_path does not belong to this Schedule",
            );
        }
        let at = self.clock.now_ms();
        let result = match call.arguments["action"].as_str().unwrap_or_default() {
            "pause" => {
                self.trigger
                    .pause_schedule(&call.workspace_id, &id, at)
                    .await
            }
            "resume" => {
                self.trigger
                    .resume_schedule(&call.workspace_id, &id, at)
                    .await
            }
            "archive" => {
                self.trigger
                    .archive_schedule(&call.workspace_id, &id, at)
                    .await
            }
            "skip_next" => {
                let expected = call.arguments["expected_due_at"]
                    .as_i64()
                    .or(schedule.next_due_at);
                let Some(expected) = expected else {
                    return ToolResult::error(
                        "invalid_request",
                        "the Schedule has no next occurrence",
                    );
                };
                return match self
                    .trigger
                    .skip_next(&call.workspace_id, &id, expected, at)
                    .await
                {
                    Ok(occurrence) => ToolResult::success(
                        serde_json::json!({
                            "schedule_id": id.as_str(),
                            "skipped_at": occurrence.scheduled_at,
                        })
                        .to_string(),
                    ),
                    Err(error) => trigger_error(error),
                };
            }
            "edit" => {
                let channel_id = match self
                    .channel(
                        &call.workspace_id,
                        &call.agent_id,
                        call.arguments["channel"].as_str(),
                    )
                    .await
                {
                    Ok(channel_id) => channel_id,
                    Err(result) => return result,
                };
                let timing = match timing(&call.arguments) {
                    Ok(timing) => timing,
                    Err(result) => return result,
                };
                self.trigger
                    .edit_schedule(
                        &call.workspace_id,
                        &id,
                        ScheduleEdit {
                            expected_revision: call.arguments["expected_revision"]
                                .as_u64()
                                .and_then(|value| u32::try_from(value).ok())
                                .unwrap_or(schedule.revision),
                            agent_id: call.agent_id.clone(),
                            name: call.arguments["name"]
                                .as_str()
                                .unwrap_or(&schedule.name)
                                .to_string(),
                            instruction: call.arguments["instruction"]
                                .as_str()
                                .unwrap_or(&schedule.instruction)
                                .to_string(),
                            channel_id,
                            root_message_id: None,
                            timing,
                        },
                        at,
                    )
                    .await
            }
            action => {
                return ToolResult::error(
                    "invalid_request",
                    format!("`{action}` is not a Schedule action"),
                );
            }
        };
        match result {
            Ok(Some(schedule)) => {
                self.refresh_page(call, &schedule).await;
                schedule_result(&schedule)
            }
            Ok(None) => ToolResult::error("not_found", "there is no such Schedule"),
            Err(error) => trigger_error(error),
        }
    }

    async fn channel(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        requested: Option<&str>,
    ) -> Result<ChannelId, ToolResult> {
        let requested = requested.map(str::trim).filter(|value| !value.is_empty());
        if let Some(title) = requested.filter(|value| !value.eq_ignore_ascii_case(USER_CHANNEL)) {
            let channels = self
                .channels
                .list_by_workspace(workspace_id)
                .await
                .map_err(|error| ToolResult::error("temporarily_unavailable", error.to_string()))?;
            return channels
                .into_iter()
                .find(|channel| {
                    channel
                        .title
                        .as_deref()
                        .is_some_and(|found| found.eq_ignore_ascii_case(title))
                })
                .map(|channel| channel.id)
                .ok_or_else(|| {
                    ToolResult::error(
                        "invalid_request",
                        format!("there is no channel called `{title}`"),
                    )
                });
        }
        self.channels
            .find_user_dm(workspace_id, agent_id)
            .await
            .map_err(|error| ToolResult::error("temporarily_unavailable", error.to_string()))?
            .map(|channel| channel.id)
            .ok_or_else(|| {
                ToolResult::error(
                    "invalid_request",
                    "you have no direct channel with the user",
                )
            })
    }

    async fn refresh_page(&self, call: &AuthorizedCall, schedule: &pagis_core::Schedule) {
        let Some(path) = schedule.subject_page_path.as_deref() else {
            return;
        };
        let open = match self
            .trigger
            .list_open_for_subject_page(&call.workspace_id, &call.agent_id, path)
            .await
        {
            Ok(schedules) => schedules,
            Err(error) => {
                tracing::warn!(%error, path, "open Subject Page Schedules could not be read");
                return;
            }
        };
        if let Err(error) = self
            .memory
            .refresh_subject_page_schedules(call, path, open)
            .await
        {
            tracing::warn!(%error, path, "the Subject Page Schedules section was not refreshed");
        }
    }
}

#[async_trait]
impl ToolExecutor for ScheduleToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        match &call.route {
            ToolRoute::Core {
                tool: CoreTool::ScheduleCreate,
            } => self.create(&call).await,
            ToolRoute::Core {
                tool: CoreTool::ScheduleList,
            } => self.list(&call).await,
            ToolRoute::Core {
                tool: CoreTool::ScheduleUpdate,
            } => self.update(&call).await,
            _ => ToolResult::error(
                "invalid_request",
                format!("{} is not a Schedule tool", call.tool_name),
            ),
        }
    }
}

fn timing(arguments: &serde_json::Value) -> Result<ScheduleTiming, ToolResult> {
    match arguments["kind"].as_str().unwrap_or_default() {
        "one_shot" => Ok(ScheduleTiming::OneShot {
            local_time: required(arguments, "local_time")?.to_string(),
            timezone: arguments["timezone"].as_str().unwrap_or("UTC").to_string(),
        }),
        "cron" => Ok(ScheduleTiming::Cron {
            expression: required(arguments, "cron_expression")?.to_string(),
            timezone: arguments["timezone"].as_str().unwrap_or("UTC").to_string(),
        }),
        "interval" => {
            let minutes = arguments["interval_minutes"].as_i64().ok_or_else(|| {
                ToolResult::error("invalid_request", "interval_minutes is required")
            })?;
            let anchor = arguments["anchor_at"]
                .as_i64()
                .ok_or_else(|| ToolResult::error("invalid_request", "anchor_at is required"))?;
            Ok(ScheduleTiming::Interval {
                every_ms: minutes.saturating_mul(60_000),
                anchor,
            })
        }
        _ => Err(ToolResult::error(
            "invalid_request",
            "kind must be one_shot, cron, or interval",
        )),
    }
}

fn subject_page_path(arguments: &serde_json::Value) -> Result<Option<String>, ToolResult> {
    let Some(value) = arguments["subject_page_path"].as_str() else {
        return Ok(None);
    };
    let valid = arguments["wake_only"] == true
        && pagis_core::ScopedPath::parse(value).is_ok_and(|path| {
            path.scope == pagis_core::MemoryScope::Private
                && path.rel.starts_with("subjects/")
                && path.rel.ends_with(".md")
        });
    if !valid {
        return Err(ToolResult::error(
            "invalid_request",
            "a subject_page_path must name a private Subject Page on a wake-only Schedule",
        ));
    }
    Ok(Some(value.to_string()))
}

fn required<'a>(arguments: &'a serde_json::Value, name: &str) -> Result<&'a str, ToolResult> {
    arguments[name]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ToolResult::error("invalid_request", format!("{name} is required")))
}

fn schedule_result(schedule: &pagis_core::Schedule) -> ToolResult {
    ToolResult::success(
        serde_json::json!({
            "schedule_id": schedule.id.as_str(),
            "state": schedule.state.as_str(),
            "revision": schedule.revision,
            "next_due_at": schedule.next_due_at,
        })
        .to_string(),
    )
}

fn trigger_error(error: TriggerError) -> ToolResult {
    match error {
        TriggerError::Store(pagis_core::StoreError::Conflict(message)) => {
            ToolResult::error("conflict", message)
        }
        TriggerError::Store(error) => {
            tracing::error!(%error, "a Schedule write failed");
            ToolResult::error("temporarily_unavailable", "Schedules are not writable")
        }
        other => ToolResult::error("invalid_request", other.to_string()),
    }
}
