//! The Trusted contacts surface
//! (ADR-0021, ADR-0022, ADR-0019): the Trust List, the Keypad Code, its
//! failed-attempt count and the user's drop of a live call.
//!
//! One list holds phone numbers for Calls and email addresses or bare
//! domains for mail. The addresses of the user's own mail Connections
//! are owner without a row, so this API neither returns them nor
//! removes them.
//!
//! No tool reaches any of this. The user edits the list through the
//! daemon, as ADR-0013 does for the vault's navigation, because a list
//! an Agent can extend is a list a caller or a sender can talk their
//! way onto.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use pagis_core::{AgentId, TrustEntry, TrustEntryId, TrustTier, now_ms, parse_trust_subject};
use pagis_mail::connection_account;
use pagis_vault::KeypadCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One Trust List entry. A row with no `agent_id` is Workspace-wide.
#[derive(Debug, Serialize, ToSchema)]
pub struct TrustEntryDto {
    pub id: String,
    pub agent_id: Option<String>,
    /// `number`, `address` or `domain`.
    pub subject: String,
    /// The E.164 number, the bare address or the bare domain.
    pub value: String,
    /// `owner` or `trusted`.
    pub tier: String,
    pub label: String,
    pub created_at: i64,
}

/// One address that is owner because the user holds the account it
/// belongs to (ADR-0019). It is not a row, so it has no id and the user
/// cannot remove it.
#[derive(Debug, Serialize, ToSchema)]
pub struct OwnAddressDto {
    pub address: String,
    /// The Connection this address is the account of.
    pub connection_alias: String,
}

/// Whether the Workspace holds a Keypad Code, and the wrong codes its
/// callers entered. It is everything the API says about the code: there
/// is no reveal and no export.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeypadCodeDto {
    pub configured: bool,
    /// The wrong codes on every Call and every line of the Workspace,
    /// since a correct code or the person last cleared the count.
    pub failed_attempts: u32,
    /// The end of the latest delay, in Unix milliseconds. `null` until
    /// the first delay starts. It stays when the delay ends, until the
    /// count is cleared.
    pub suspended_until: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrustListPage {
    pub items: Vec<TrustEntryDto>,
    /// The addresses that are owner with no row.
    pub own_addresses: Vec<OwnAddressDto>,
    pub keypad_code: KeypadCodeDto,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ListContactRequest {
    /// The Agent whose list this row joins, or absent for the
    /// Workspace-wide list.
    pub agent_id: Option<String>,
    /// An E.164 number, an email address or a bare domain. The daemon
    /// reads which of the three it is.
    pub value: String,
    /// `owner` or `trusted`.
    pub tier: String,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetKeypadCodeRequest {
    /// 6 to 8 digits. It is hashed at once and never read back.
    pub code: String,
}

fn dto(row: TrustEntry) -> TrustEntryDto {
    TrustEntryDto {
        id: row.id.to_string(),
        agent_id: row.agent_id.map(|id| id.to_string()),
        subject: row.subject.as_str().to_string(),
        value: row.value,
        tier: row.tier.as_str().to_string(),
        label: row.label,
        created_at: row.created_at,
    }
}

/// The Keypad Code of the signed-in person's Workspace (ADR-0021).
fn keypad<'a>(state: &'a AppState, tenant: &Tenant) -> KeypadCode<'a> {
    KeypadCode::new(state.secrets.as_ref(), &tenant.workspace_id)
}

/// The Keypad Code card of the signed-in person's Workspace.
async fn keypad_card(state: &AppState, tenant: &Tenant) -> Result<KeypadCodeDto, ApiError> {
    let configured = keypad(state, tenant)
        .is_set()
        .map_err(|_| ApiError::internal())?;
    let failures = state.keypad_failures.get(&tenant.workspace_id).await?;
    Ok(KeypadCodeDto {
        configured,
        failed_attempts: failures.failed_attempts,
        suspended_until: failures.suspended_until,
    })
}

/// The account addresses of the user's mail Connections. They are owner
/// with no row, so the tab greys them instead of offering a Remove.
async fn own_addresses(state: &AppState, tenant: &Tenant) -> Result<Vec<OwnAddressDto>, ApiError> {
    let mut addresses: Vec<OwnAddressDto> = state
        .connections
        .list(&tenant.workspace_id)
        .await?
        .into_iter()
        .filter_map(|connection| {
            connection_account(&connection).map(|address| OwnAddressDto {
                address,
                connection_alias: connection.alias,
            })
        })
        .collect();
    addresses.sort_by(|a, b| a.address.cmp(&b.address));
    Ok(addresses)
}

#[utoipa::path(get, path = "/api/v1/settings/trust-list", responses(
    (status = 200, body = TrustListPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_trust_entries(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<TrustListPage>, ApiError> {
    let items = state
        .trust_list
        .list(&tenant.workspace_id)
        .await?
        .into_iter()
        .map(dto)
        .collect();
    Ok(Json(TrustListPage {
        items,
        own_addresses: own_addresses(&state, &tenant).await?,
        keypad_code: keypad_card(&state, &tenant).await?,
    }))
}

#[utoipa::path(post, path = "/api/v1/settings/trust-list",
    request_body = ListContactRequest, responses(
        (status = 201, body = TrustEntryDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn add_trust_entry(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<ListContactRequest>,
) -> Result<(StatusCode, Json<TrustEntryDto>), ApiError> {
    let (subject, value) = parse_trust_subject(&request.value).map_err(ApiError::validation)?;
    let tier = TrustTier::listed(&request.tier)
        .ok_or_else(|| ApiError::validation("a listed tier is `owner` or `trusted`"))?;
    let row = TrustEntry {
        id: TrustEntryId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        agent_id: request.agent_id.map(AgentId::from),
        subject,
        value,
        tier,
        label: request.label.unwrap_or_default(),
        created_at: now_ms(),
    };
    state.trust_list.upsert(&row).await?;
    Ok((StatusCode::CREATED, Json(dto(row))))
}

#[utoipa::path(delete, path = "/api/v1/settings/trust-list/{trust_entry_id}", responses(
    (status = 204),
    (status = 401, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody),
))]
pub async fn delete_trust_entry(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(trust_entry_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    match state
        .trust_list
        .delete(&tenant.workspace_id, &TrustEntryId::from(trust_entry_id))
        .await?
    {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("that listed contact")),
    }
}

#[utoipa::path(put, path = "/api/v1/settings/keypad-code",
    request_body = SetKeypadCodeRequest, responses(
        (status = 200, body = KeypadCodeDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_keypad_code(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SetKeypadCodeRequest>,
) -> Result<Json<KeypadCodeDto>, ApiError> {
    keypad(&state, &tenant)
        .set(request.code.trim())
        .map_err(|error| ApiError::validation(error.to_string()))?;
    Ok(Json(keypad_card(&state, &tenant).await?))
}

#[utoipa::path(delete, path = "/api/v1/settings/keypad-code", responses(
    (status = 204),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn delete_keypad_code(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<StatusCode, ApiError> {
    keypad(&state, &tenant)
        .clear()
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

/// Clear the failed-attempt count of the signed-in person's Workspace,
/// and end its delay (ADR-0021). A correct code clears it too. The
/// person is the only other party who can know that the wrong codes
/// were not an attack, so no tool reaches this. The clear publishes
/// `keypad.cleared`, so the keypad item leaves the Needs-You Queue.
#[utoipa::path(delete, path = "/api/v1/settings/keypad-code/failures", responses(
    (status = 204),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn clear_keypad_failures(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<StatusCode, ApiError> {
    state.keypad_failures.clear(&tenant.workspace_id).await?;
    state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "keypad.cleared".into(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({}),
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
