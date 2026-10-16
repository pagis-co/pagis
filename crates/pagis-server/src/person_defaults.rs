//! Administrator defaults for a new Person.
use crate::{
    AppState,
    auth::Administrator,
    error::ApiError,
    mailboxes::{MailboxOfferDto, mailbox_error, offer_dto},
};
use axum::{Json, extract::State, http::StatusCode};
use pagis_core::{AuditEntry, AuditEntryId, Author, PersonDefaults, WorkspaceKind};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct PersonDefaultsPage {
    pub defaults: PersonDefaults,
    pub mailbox_offers: Vec<MailboxOfferDto>,
}

#[utoipa::path(get, operation_id = "get_person_defaults", path = "/api/v1/administration/person-defaults", responses(
    (status = 200, body = PersonDefaultsPage), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)
))]
pub async fn get(
    State(state): State<Arc<AppState>>,
    _: Administrator,
) -> Result<Json<PersonDefaultsPage>, ApiError> {
    let org = crate::administration::org(&state).await?;
    Ok(Json(PersonDefaultsPage {
        defaults: state.orgs.person_defaults(&org.id).await?,
        mailbox_offers: offers(&state).await?.into_iter().map(offer_dto).collect(),
    }))
}

pub(crate) async fn offers(state: &AppState) -> Result<Vec<pagis_mail::MailboxOffer>, ApiError> {
    Ok(state
        .mailbox_desk
        .offers("")
        .await
        .map_err(mailbox_error)?
        .into_iter()
        .filter(|offer| {
            state
                .mailbox_desk
                .supports_automatic_workspace_mail(&offer.connection)
        })
        .collect())
}

pub(crate) async fn validate(state: &AppState, defaults: &PersonDefaults) -> Result<(), ApiError> {
    if defaults.address_rule.is_some() != defaults.mailbox_connection_id.is_some() {
        return Err(ApiError::validation(
            "Select an address rule and a Mailbox Provider together.",
        ));
    }
    if let Some(id) = &defaults.mailbox_connection_id
        && !offers(state)
            .await?
            .iter()
            .any(|offer| &offer.connection.id == id)
    {
        return Err(ApiError::validation(
            "Select a connected Mailbox Provider that creates Workspace Mailboxes.",
        ));
    }
    let mut ids = BTreeSet::new();
    for member in &defaults.shared_workspaces {
        if !ids.insert(&member.workspace_id) {
            return Err(ApiError::validation("Select each Shared Workspace once."));
        }
        if !state
            .workspaces
            .get(&member.workspace_id)
            .await?
            .is_some_and(|w| w.kind == WorkspaceKind::Shared)
        {
            return Err(ApiError::validation(
                "A default membership needs a Shared Workspace.",
            ));
        }
    }
    Ok(())
}

#[utoipa::path(put, operation_id = "set_person_defaults", path = "/api/v1/administration/person-defaults", request_body = PersonDefaults, responses(
    (status = 204), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)
))]
pub async fn set(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Json(defaults): Json<PersonDefaults>,
) -> Result<StatusCode, ApiError> {
    validate(&state, &defaults).await?;
    let org = crate::administration::org(&state).await?;
    let entry = AuditEntry {
        id: AuditEntryId::generate(),
        org_workspace_id: org.workspace_id.clone(),
        actor: Author::Person(administrator.0.user_id),
        action: "person_defaults.changed".into(),
        subject_workspace_id: org.workspace_id,
        reason: Some(serde_json::to_string(&defaults).map_err(|_| ApiError::internal())?),
        at: state.clock.now_ms(),
    };
    state
        .orgs
        .set_person_defaults(&org.id, &defaults, &entry)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn mailbox_request(
    state: &AppState,
    defaults: &PersonDefaults,
    name: &str,
    explicit: Option<&str>,
) -> Result<Option<pagis_mail::PreparedWorkspaceMailbox>, ApiError> {
    let Some(rule) = defaults.address_rule else {
        if explicit.is_some() {
            return Err(ApiError::validation(
                "Set an address rule before you select a Workspace Mailbox address.",
            ));
        }
        return Ok(None);
    };
    let offer = offers(state)
        .await?
        .into_iter()
        .find(|offer| Some(&offer.connection.id) == defaults.mailbox_connection_id.as_ref())
        .ok_or_else(|| {
            ApiError::validation("The address rule needs a connected Mailbox Provider.")
        })?;
    let local_part = match explicit {
        Some(address) => {
            let (local, domain) = address.trim().rsplit_once('@').ok_or_else(|| {
                ApiError::validation("Enter a complete Workspace Mailbox address.")
            })?;
            if !domain.eq_ignore_ascii_case(&offer.settings.domain) {
                return Err(ApiError::validation(
                    "Use the Org mail domain for the Workspace Mailbox.",
                ));
            }
            local.to_owned()
        }
        None => rule.local_part(name).map_err(ApiError::validation)?,
    };
    let request = pagis_mail::NewMailbox {
        connection_id: offer.connection.id,
        local_part,
        outgoing_cap: None,
        password: None,
    };
    Ok(Some(
        state
            .mailbox_desk
            .prepare_workspace(request)
            .await
            .map_err(mailbox_error)?,
    ))
}
