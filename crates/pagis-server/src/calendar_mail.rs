//! Calendar scheduling uses the Workspace Mailbox send path.
use crate::{AppState, auth::Tenant, error::ApiError, mailboxes::mailbox_error};
use pagis_core::{
    CalendarEvent, ParticipationStatus,
    calendar::{CalendarInvitation, EventStatus},
    imip::{SchedulingMessage, SchedulingMethod},
};

pub async fn outside_attendees(
    state: &AppState,
    who: &Tenant,
    event: &CalendarEvent,
) -> Result<Vec<String>, ApiError> {
    let person = state
        .users
        .get(&who.user_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let people = state.users.list_by_org(&person.org_id).await?;
    let mut inside = people
        .iter()
        .filter_map(|p| p.email.as_ref().map(|s| s.to_lowercase()))
        .collect::<std::collections::HashSet<_>>();
    for workspace in state.workspaces.list().await? {
        if workspace
            .user_id
            .as_ref()
            .is_some_and(|id| !people.iter().any(|p| &p.id == id))
        {
            continue;
        }
        if let Some(mailbox) = state
            .mailbox_desk
            .workspace_mailbox(&workspace.id)
            .await
            .map_err(mailbox_error)?
        {
            inside.insert(mailbox.address.to_lowercase());
        }
    }
    let mut seen = std::collections::HashSet::new();
    Ok(event
        .data
        .attendees
        .iter()
        .filter(|a| {
            !inside.contains(&a.email.to_lowercase()) && seen.insert(a.email.to_lowercase())
        })
        .map(|a| a.email.clone())
        .collect())
}

pub async fn prepare(
    state: &AppState,
    who: &Tenant,
    event: &mut CalendarEvent,
    previous: Option<&CalendarEvent>,
) -> Result<Vec<String>, ApiError> {
    let recipients = outside_attendees(state, who, event).await?;
    let mailbox = state
        .mailbox_desk
        .workspace_mailbox(&who.workspace_id)
        .await
        .map_err(mailbox_error)?;
    if let Some(previous) = previous
        && let Some(invitation) = &previous.data.invitation
        && !mailbox
            .as_ref()
            .is_some_and(|m| m.address.eq_ignore_ascii_case(&invitation.organizer))
    {
        return Err(ApiError::conflict(
            "Only the organizer can change this invitation. Use an invitation response.",
        ));
    }
    event.data.invitation =
        if recipients.is_empty() && previous.and_then(|p| p.data.invitation.as_ref()).is_none() {
            None
        } else {
            let mailbox = mailbox.ok_or_else(|| {
                ApiError::validation("Create a Workspace Mailbox to invite people outside the Org.")
            })?;
            let sequence = match previous.and_then(|p| p.data.invitation.as_ref()) {
                Some(i) => i
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| ApiError::validation("The invitation sequence is too large."))?,
                None => 0,
            };
            Some(CalendarInvitation {
                organizer: mailbox.address,
                sequence,
                status: EventStatus::Confirmed,
                occurrence: None,
            })
        };
    Ok(recipients)
}

pub async fn send(
    state: &AppState,
    who: &Tenant,
    event: &CalendarEvent,
    method: SchedulingMethod,
    recipients: Vec<String>,
) -> Result<(), ApiError> {
    if recipients.is_empty() {
        return Ok(());
    }
    let calendar = SchedulingMessage::build(event, method)?;
    let mut store = state
        .mailbox_desk
        .open_workspace(&who.workspace_id)
        .await
        .map_err(mailbox_error)?;
    let result = async {
        for recipient in recipients {
            let mail = pagis_mail::OutgoingMessage {
                to: vec![recipient],
                subject: format!("{}: {}", method.as_str(), event.data.title),
                body: format!(
                    "{}\n{} – {} ({})\n{}\n{}",
                    event.data.title,
                    event.data.start,
                    event.data.end,
                    event.data.timezone,
                    event.data.location,
                    event.data.description
                ),
                attachments: vec![pagis_mail::OutgoingAttachment {
                    name: "invite.ics".into(),
                    content_type: format!(
                        "text/calendar; method={}; charset=utf-8",
                        method.as_str()
                    ),
                    data: calendar.as_bytes().to_vec(),
                }],
                ..Default::default()
            };
            state
                .mailbox_desk
                .send_workspace(&who.workspace_id, store.as_mut(), &mail)
                .await
                .map_err(mailbox_error)?;
        }
        Ok(())
    }
    .await;
    let _ = store.close().await;
    result
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct InvitationResponse {
    pub status: ParticipationStatus,
}

#[utoipa::path(post, path = "/api/v1/calendar/events/{event_id}/response", params(("event_id" = String, Path)), request_body = InvitationResponse, responses((status = 200, body = CalendarEvent), (status = 404), (status = 409)), tag = "calendar")]
pub async fn answer_invitation(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<AppState>>,
    who: Tenant,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::Json(response): axum::Json<InvitationResponse>,
) -> Result<axum::Json<CalendarEvent>, ApiError> {
    if response.status == ParticipationStatus::NeedsAction {
        return Err(ApiError::validation("Select accept, maybe or decline."));
    }
    let mut event = state
        .calendar
        .get(&who.workspace_id, &id.into())
        .await?
        .ok_or_else(|| ApiError::not_found("Event"))?;
    let invitation = event
        .data
        .invitation
        .as_mut()
        .ok_or_else(|| ApiError::validation("This Event has no invitation."))?;
    if invitation.status == EventStatus::Cancelled {
        return Err(ApiError::conflict("This Event is cancelled."));
    }
    let mailbox = state
        .mailbox_desk
        .workspace_mailbox(&who.workspace_id)
        .await
        .map_err(mailbox_error)?
        .ok_or_else(|| ApiError::not_found("Workspace Mailbox"))?;
    if invitation.organizer.eq_ignore_ascii_case(&mailbox.address) {
        return Err(ApiError::validation(
            "The organizer cannot answer its own invitation.",
        ));
    }
    let organizer = invitation.organizer.clone();
    invitation.status = if response.status == ParticipationStatus::Accepted {
        EventStatus::Confirmed
    } else {
        EventStatus::Tentative
    };
    let attendee = event
        .data
        .attendees
        .iter_mut()
        .find(|a| a.email.eq_ignore_ascii_case(&mailbox.address))
        .ok_or_else(|| ApiError::validation("This Workspace is not an attendee."))?;
    attendee.status = response.status;
    let mut reply = event.clone();
    reply
        .data
        .attendees
        .retain(|a| a.email.eq_ignore_ascii_case(&mailbox.address));
    send(
        &state,
        &who,
        &reply,
        SchedulingMethod::Reply,
        vec![organizer],
    )
    .await?;
    if !state
        .calendar
        .update(&who.workspace_id, &event.id, &event.data)
        .await?
    {
        return Err(ApiError::conflict("The invitation changed. Open it again."));
    }
    Ok(axum::Json(event))
}
