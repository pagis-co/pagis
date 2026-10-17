//! Calendar tools prepare mail recipients before the broker asks for Approval.
use super::*;
use pagis_broker::CalendarTool;
use pagis_core::calendar::{CalendarInvitation, EventStatus};
use pagis_core::imip::{SchedulingMessage, SchedulingMethod};
use pagis_core::{
    Author, CalendarEvent, CalendarEventData, CalendarEventId, Grant, ParticipationStatus,
};
use serde_json::{Value, json};

fn invalid(message: impl ToString) -> ToolResult {
    ToolResult::error("invalid_request", message.to_string())
}
fn storage(error: pagis_core::StoreError) -> ToolResult {
    ToolResult::error("temporarily_unavailable", error.to_string())
}

impl MailToolRuntime {
    async fn calendar_grant(
        &self,
        workspace: &WorkspaceId,
        agent: &pagis_core::AgentId,
    ) -> Result<(), ToolResult> {
        if self
            .deps
            .grants
            .live_for_resource(workspace, agent, Grant::CALENDAR_KIND, workspace.as_str())
            .await
            .map_err(storage)?
            .is_none()
        {
            return Err(ToolResult::error(
                "permission_revoked",
                "A live Calendar Grant is required.",
            ));
        }
        Ok(())
    }

    async fn calendar_outside(&self, event: &CalendarEvent) -> Result<Vec<String>, ToolResult> {
        let mut inside = std::collections::HashSet::new();
        for workspace in self.deps.workspaces.list().await.map_err(storage)? {
            if let Some(mailbox) = self
                .deps
                .workspace
                .workspace_mailbox(&workspace.id)
                .await
                .map_err(workspace_error)?
            {
                inside.insert(mailbox.address.to_lowercase());
            }
        }
        let mut recipients = Vec::new();
        for attendee in &event.data.attendees {
            if inside.contains(&attendee.email.to_lowercase()) {
                continue;
            }
            if self
                .deps
                .users
                .find_by_email(&attendee.email)
                .await
                .map_err(storage)?
                .is_some_and(|p| !p.is_disabled())
            {
                continue;
            }
            if !recipients
                .iter()
                .any(|email: &String| email.eq_ignore_ascii_case(&attendee.email))
            {
                recipients.push(attendee.email.clone());
            }
        }
        Ok(recipients)
    }

    pub(super) async fn prepare_calendar(
        &self,
        workspace: &WorkspaceId,
        agent: &pagis_core::AgentId,
        tool: CalendarTool,
        arguments: Value,
    ) -> Result<Value, ToolResult> {
        self.calendar_grant(workspace, agent).await?;
        let previous = if tool == CalendarTool::Create {
            None
        } else {
            let id = arguments["event_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| invalid("event_id is required."))?;
            Some(
                self.deps
                    .calendar
                    .get(workspace, &CalendarEventId::from(id.to_owned()))
                    .await
                    .map_err(storage)?
                    .ok_or_else(|| invalid("Event not found."))?,
            )
        };
        let mut event = match tool {
            CalendarTool::Create => CalendarEvent::new(
                workspace.clone(),
                Author::Agent(agent.clone()),
                serde_json::from_value::<CalendarEventData>(arguments["event"].clone())
                    .map_err(invalid)?,
            ),
            _ => previous.clone().expect("existing Event"),
        };
        event.created_by = Author::Agent(agent.clone());
        if tool == CalendarTool::Update {
            event.data = serde_json::from_value(arguments["event"].clone()).map_err(invalid)?;
        }
        event.data.bounds().map_err(invalid)?;
        let mailbox = self
            .deps
            .workspace
            .workspace_mailbox(workspace)
            .await
            .map_err(workspace_error)?;
        let mut recipients;
        let method;
        if tool == CalendarTool::Respond {
            let status: ParticipationStatus =
                serde_json::from_value(arguments["status"].clone()).map_err(invalid)?;
            if status == ParticipationStatus::NeedsAction {
                return Err(invalid("Select accept, maybe or decline."));
            }
            let invitation = event
                .data
                .invitation
                .as_mut()
                .ok_or_else(|| invalid("This Event has no invitation."))?;
            if invitation.status == EventStatus::Cancelled {
                return Err(invalid("This Event is cancelled."));
            }
            let mailbox = mailbox
                .as_ref()
                .ok_or_else(|| invalid("Workspace Mailbox not found."))?;
            if invitation.organizer.eq_ignore_ascii_case(&mailbox.address) {
                return Err(invalid("The organizer cannot answer its own invitation."));
            }
            let attendee = event
                .data
                .attendees
                .iter_mut()
                .find(|a| a.email.eq_ignore_ascii_case(&mailbox.address))
                .ok_or_else(|| invalid("This Workspace is not an attendee."))?;
            attendee.status = status.clone();
            invitation.status = if status == ParticipationStatus::Accepted {
                EventStatus::Confirmed
            } else {
                EventStatus::Tentative
            };
            recipients = vec![invitation.organizer.clone()];
            method = SchedulingMethod::Reply;
        } else {
            recipients = self.calendar_outside(&event).await?;
            if let Some(old) = &previous {
                for email in self.calendar_outside(old).await? {
                    if !recipients.iter().any(|a| a.eq_ignore_ascii_case(&email)) {
                        recipients.push(email);
                    }
                }
                if let Some(invitation) = &old.data.invitation
                    && !mailbox
                        .as_ref()
                        .is_some_and(|m| m.address.eq_ignore_ascii_case(&invitation.organizer))
                {
                    return Err(invalid(
                        "Only the organizer can change this invitation. Use an invitation response.",
                    ));
                }
            }
            event.data.invitation = if recipients.is_empty()
                && previous
                    .as_ref()
                    .and_then(|e| e.data.invitation.as_ref())
                    .is_none()
            {
                None
            } else {
                let mailbox = mailbox.as_ref().ok_or_else(|| {
                    invalid("Create a Workspace Mailbox to invite people outside the Org.")
                })?;
                let sequence = match previous.as_ref().and_then(|e| e.data.invitation.as_ref()) {
                    Some(i) => i
                        .sequence
                        .checked_add(1)
                        .ok_or_else(|| invalid("The invitation sequence is too large."))?,
                    None => 0,
                };
                Some(CalendarInvitation {
                    organizer: mailbox.address.clone(),
                    sequence,
                    status: if tool == CalendarTool::Delete {
                        EventStatus::Cancelled
                    } else {
                        EventStatus::Confirmed
                    },
                    occurrence: None,
                })
            };
            method = if tool == CalendarTool::Delete {
                SchedulingMethod::Cancel
            } else {
                SchedulingMethod::Request
            };
        }
        if tool == CalendarTool::Delete {
            self.deps
                .workspaces
                .refuse_delete(workspace)
                .await
                .map_err(storage)?;
        }
        if !recipients.is_empty() {
            self.deps
                .workspace
                .require_workspace_grant(workspace, agent)
                .await
                .map_err(workspace_error)?;
        }
        Ok(json!({
            "event_id": event.id,
            "event": event.data,
            "calendar_event": event,
            "calendar_previous": previous,
            "calendar_method": method.as_str(),
            "status": arguments["status"],
            "from": mailbox.map(|m| m.address),
            "to": recipients,
            "subject": event.data.title,
            "body": format!("{}\n{} – {} ({})\n{}\n{}", event.data.title, event.data.start, event.data.end, event.data.timezone, event.data.location, event.data.description)
        }))
    }

    pub(super) async fn run_calendar(
        &self,
        tool: CalendarTool,
        call: &AuthorizedCall,
    ) -> Result<Value, ToolResult> {
        self.calendar_grant(&call.workspace_id, &call.agent_id)
            .await?;
        let event: CalendarEvent =
            serde_json::from_value(call.arguments["calendar_event"].clone()).map_err(invalid)?;
        let previous: Option<CalendarEvent> =
            serde_json::from_value(call.arguments["calendar_previous"].clone()).map_err(invalid)?;
        if event.workspace_id != call.workspace_id
            || event.created_by != Author::Agent(call.agent_id.clone())
        {
            return Err(invalid("Invalid Event owner or author."));
        }
        if let Some(previous) = &previous {
            let current = self
                .deps
                .calendar
                .get(&call.workspace_id, &event.id)
                .await
                .map_err(storage)?;
            if current.as_ref() != Some(previous) {
                return Err(ToolResult::error(
                    "conflict",
                    "The Event changed. Open it again.",
                ));
            }
        }
        let recipients = strings(&call.arguments, "to");
        if !recipients.is_empty() {
            self.deps
                .workspace
                .require_workspace_grant(&call.workspace_id, &call.agent_id)
                .await
                .map_err(workspace_error)?;
        }
        if tool == CalendarTool::Delete {
            self.deps
                .workspaces
                .refuse_delete(&call.workspace_id)
                .await
                .map_err(storage)?;
        }
        match tool {
            CalendarTool::Create => self.deps.calendar.create(&event).await.map_err(storage)?,
            CalendarTool::Delete if event.data.invitation.is_none() => {
                self.deps
                    .calendar
                    .delete(&call.workspace_id, &event.id)
                    .await
                    .map_err(storage)?;
            }
            _ => {
                if !self.deps.calendar.change(&event).await.map_err(storage)? {
                    return Err(ToolResult::error(
                        "conflict",
                        "The Event changed. Open it again.",
                    ));
                }
            }
        }
        if !recipients.is_empty() {
            let mut store = self
                .deps
                .workspace
                .open_workspace_for_agent(&call.workspace_id, &call.agent_id)
                .await
                .map_err(workspace_error)?;
            let result = async {
                for recipient in recipients {
                    self.deps
                        .workspace
                        .require_workspace_grant(&call.workspace_id, &call.agent_id)
                        .await
                        .map_err(workspace_error)?;
                    let mut sent_event = event.clone();
                    let method = if tool == CalendarTool::Respond {
                        let address = call.arguments["from"].as_str().unwrap_or_default();
                        sent_event
                            .data
                            .attendees
                            .retain(|a| a.email.eq_ignore_ascii_case(address));
                        SchedulingMethod::Reply
                    } else if tool == CalendarTool::Delete {
                        SchedulingMethod::Cancel
                    } else if !event
                        .data
                        .attendees
                        .iter()
                        .any(|a| a.email.eq_ignore_ascii_case(&recipient))
                    {
                        sent_event = previous.clone().expect("removed attendee");
                        sent_event.data.invitation = event.data.invitation.clone();
                        sent_event
                            .data
                            .attendees
                            .retain(|a| a.email.eq_ignore_ascii_case(&recipient));
                        SchedulingMethod::Cancel
                    } else {
                        SchedulingMethod::Request
                    };
                    let calendar =
                        SchedulingMessage::build(&sent_event, method).map_err(invalid)?;
                    self.deps
                        .workspace
                        .send_workspace(
                            &call.workspace_id,
                            store.as_mut(),
                            &OutgoingMessage {
                                to: vec![recipient],
                                subject: format!("{}: {}", method.as_str(), event.data.title),
                                body: text(&call.arguments, "body"),
                                attachments: vec![crate::OutgoingAttachment {
                                    name: "invite.ics".into(),
                                    content_type: format!(
                                        "text/calendar; method={}; charset=utf-8",
                                        method.as_str()
                                    ),
                                    data: calendar.into_bytes(),
                                }],
                                ..Default::default()
                            },
                        )
                        .await
                        .map_err(workspace_error)?;
                }
                Ok::<_, ToolResult>(())
            }
            .await;
            let _ = store.close().await;
            result?;
        }
        Ok(json!(event))
    }
}
