//! Requests REST: list the pending
//! requests, read one (the card renders its state from the row), and
//! write the decision that wakes the parked run.
//!
//! A `form` or a `choice` submits `values`, which are validated
//! against the field schema on the row and never against the block's
//! denormalized copy: a client can post back a mutated block. A
//! validation failure is a 422 and leaves the run parked.
//!
//! `scope` is meaningful for a `tool_action` only. An `always` approve
//! also writes the daemon-derived allow rules into the agent's grant
//! for that resource kind, and a plain approve bootstraps the grant
//! with an empty rule list. A host `tool_action` writes host rules; a
//! `credential_action` writes the one registrable domain the
//! Credential record carried. A `mail__send` from an Agent Mailbox
//! writes its recipient domains onto the mailbox record, because that
//! mailbox is the Agent's own identity and carries no Grant.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use pagis_core::{
    AgentMailboxId, DecideOutcome, Grant, GrantId, NewEvent, Request, RequestId, RequestState,
    now_ms, validate_values,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

#[derive(Debug, Serialize, ToSchema)]
pub struct RequestDto {
    pub id: String,
    pub agent_id: String,
    pub run_id: Option<String>,
    /// `tool_action`, `credential_action`, `form`,
    /// or `choice`.
    pub kind: String,
    /// The trusted content the daemon minted the row with: tool name
    /// and arguments for an action, the field schema for a form, the
    /// options for a choice.
    pub payload: serde_json::Value,
    /// `pending`, `approved`, `denied`, `expired`, or `superseded`
    /// (the user sent a message in place of a decision).
    pub state: String,
    /// What a form or a choice submitted with its decision.
    pub values: Option<serde_json::Value>,
    pub decided_at: Option<i64>,
    pub created_at: i64,
}

impl From<Request> for RequestDto {
    fn from(request: Request) -> Self {
        RequestDto {
            id: request.id.to_string(),
            agent_id: request.agent_id.to_string(),
            run_id: request.run_id.map(|r| r.to_string()),
            kind: request.kind,
            payload: request.payload,
            state: request.state.as_str().to_string(),
            values: request.values,
            decided_at: request.decided_at,
            created_at: request.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RequestPage {
    pub items: Vec<RequestDto>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListRequestsQuery {
    /// `pending` (the default), `approved`, `denied`, `expired`, or
    /// `superseded`.
    pub state: Option<String>,
    /// Narrow to one kind.
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DecisionRequest {
    /// `approved` or `denied`. A form submission is `approved` with
    /// `values`; a dismissal is `denied`.
    pub decision: String,
    /// `once` (the default), or `always` to also write the request's
    /// proposed allow rules into the agent's grant.
    /// Meaningful for a `tool_action` only.
    pub scope: Option<String>,
    /// What a `form` or a `choice` submits, validated against the
    /// field schema on the row.
    pub values: Option<serde_json::Value>,
}

#[utoipa::path(
    get,
    path = "/api/v1/requests",
    params(ListRequestsQuery),
    responses(
        (status = 200, body = RequestPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn list_requests(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<ListRequestsQuery>,
) -> Result<Json<RequestPage>, ApiError> {
    let request_state = query
        .state
        .as_deref()
        .unwrap_or("pending")
        .parse::<RequestState>()
        .map_err(ApiError::validation)?;
    let items = state
        .requests
        .list_by_state(&tenant.workspace_id, request_state, query.kind.as_deref())
        .await?
        .into_iter()
        .map(RequestDto::from)
        .collect();
    Ok(Json(RequestPage { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/requests/{request_id}",
    params(("request_id" = String, Path,)),
    responses(
        (status = 200, body = RequestDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_request(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(request_id): Path<String>,
) -> Result<Json<RequestDto>, ApiError> {
    let request = state
        .requests
        .get(&tenant.workspace_id, &RequestId::from(request_id))
        .await?
        .ok_or_else(|| ApiError::not_found("request"))?;
    Ok(Json(request.into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/requests/{request_id}/decision",
    params(("request_id" = String, Path,)),
    request_body = DecisionRequest,
    responses(
        (status = 200, description = "Decision recorded", body = RequestDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, description = "Already decided or expired", body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn decide_request(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(request_id): Path<String>,
    Json(body): Json<DecisionRequest>,
) -> Result<Json<RequestDto>, ApiError> {
    let decision = match body.decision.as_str() {
        "approved" => RequestState::Approved,
        "denied" => RequestState::Denied,
        other => {
            return Err(ApiError::validation(format!(
                "decision must be approved or denied, not {other}"
            )));
        }
    };
    let always = match body.scope.as_deref() {
        None | Some("once") => false,
        Some("always") => true,
        Some(other) => {
            return Err(ApiError::validation(format!(
                "scope must be once or always, not {other}"
            )));
        }
    };
    if always && decision != RequestState::Approved {
        return Err(ApiError::validation("scope always needs decision approved"));
    }

    let request_id = RequestId::from(request_id);
    let current = state
        .requests
        .get(&tenant.workspace_id, &request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("request"))?;
    let proposed_rules = proposed_rules(&current);
    if always && proposed_rules.is_empty() {
        return Err(ApiError::validation(
            "scope always needs proposed allow rules; this request has none",
        ));
    }
    let values = submitted_values(&current, decision, body.values.as_ref())?;
    // The rules an approve writes are checked before the decision is
    // stored. A refusal then leaves the Request pending, and a stored
    // decision always writes its grant and wakes its run.
    let rules: &[String] = if always { &proposed_rules } else { &[] };
    let target = grant_target(&current);
    if decision == RequestState::Approved
        && let Some((kind, resource)) = &target
    {
        check_added_rules(&state, &current, kind, resource.as_deref(), rules).await?;
    }

    match state
        .requests
        .decide(
            &tenant.workspace_id,
            &request_id,
            decision,
            values,
            now_ms(),
        )
        .await?
    {
        DecideOutcome::Decided(decided) => {
            // An approve writes the grant before the wake event:
            // the grant exists once the decision is visible.
            if decided.state == RequestState::Approved
                && let Some((kind, resource)) = &target
            {
                write_grant(&state, &decided, kind, resource.as_deref(), rules).await?;
            }
            // A mail rule lives on the mailbox record and not in a
            // Grant: the Agent holds its own mailbox with no Grant
            // (ADR-0019).
            if always
                && decided.state == RequestState::Approved
                && let Some(mailbox_id) = mailbox_of(&decided)
            {
                state
                    .mailbox_desk
                    .allow_recipient_domains(&tenant.workspace_id, &mailbox_id, &proposed_rules)
                    .await
                    .map_err(crate::mailboxes::mailbox_error)?;
            }
            // The event wakes the parked run and drives WS
            // invalidation of every card view.
            state
                .bus
                .publish(NewEvent {
                    workspace_id: decided.workspace_id.clone(),
                    event_type: "request.decided".to_string(),
                    agent_id: Some(decided.agent_id.clone()),
                    run_id: decided.run_id.clone(),
                    channel_id: None,
                    payload: serde_json::json!({
                        "request_id": decided.id.as_str(),
                        "decision": body.decision,
                    }),
                })
                .await?;
            Ok(Json(decided.into()))
        }
        DecideOutcome::NotPending(decided) => Err(ApiError::conflict(format!(
            "request is already {}",
            decided.state.as_str()
        ))),
    }
}

/// The values the decision writes onto the row. Only a `form`
/// or a `choice` carries any, and only an approve submits: a denial is
/// a dismissal. The schema comes from the row, so a client that posts
/// back a mutated block gets a 422 and the run stays parked.
fn submitted_values(
    current: &Request,
    decision: RequestState,
    values: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, ApiError> {
    if !Request::takes_values(&current.kind) || decision != RequestState::Approved {
        if values.is_some() {
            return Err(ApiError::validation(format!(
                "a {} {} decision takes no values",
                current.kind,
                decision.as_str()
            )));
        }
        return Ok(None);
    }
    validate_values(&current.kind, &current.payload, values)
        .map(Some)
        .map_err(ApiError::validation)
}

/// The daemon-derived rules the request proposes.
fn proposed_rules(request: &Request) -> Vec<String> {
    request.payload["proposed_rules"]
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .filter_map(|rule| rule.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The Agent Mailbox an `always` writes its Mail Recipient Domain
/// rules onto (ADR-0019), or `None` when the request is not a send
/// from an Agent Mailbox.
fn mailbox_of(request: &Request) -> Option<AgentMailboxId> {
    if request.kind != Request::TOOL_ACTION_KIND {
        return None;
    }
    request.payload["mailbox_id"]
        .as_str()
        .map(|id| AgentMailboxId::from(id.to_string()))
}

/// The resource a request grants against, or `None` when the request
/// grants nothing. A Plugin Grant names one Plugin, so the rule needs
/// the record and not only the kind (ADR-0017).
fn grant_target(request: &Request) -> Option<(&'static str, Option<String>)> {
    if request.kind == Request::TOOL_ACTION_KIND
        && request.payload["tool_name"] == pagis_broker::HOST_SHELL
    {
        // A host Grant names the machine the card named. A card
        // that named none grants nothing: the daemon is never a Host.
        let host_id = request.payload["host_id"].as_str()?;
        Some((Grant::HOST_KIND, Some(host_id.to_string())))
    } else if request.kind == Request::CREDENTIAL_ACTION_KIND {
        Some((Grant::CREDENTIAL_KIND, None))
    } else {
        request.payload["plugin_id"]
            .as_str()
            .map(|plugin_id| (Grant::PLUGIN_KIND, Some(plugin_id.to_string())))
    }
}

/// The live grant an approve writes onto, if the agent holds one.
async fn live_grant(
    state: &AppState,
    request: &Request,
    resource_kind: &str,
    resource_id: Option<&str>,
) -> Result<Option<Grant>, ApiError> {
    Ok(match resource_id {
        Some(resource_id) => {
            state
                .grants
                .live_for_resource(
                    &request.workspace_id,
                    &request.agent_id,
                    resource_kind,
                    resource_id,
                )
                .await?
        }
        None => {
            state
                .grants
                .live_grant(&request.workspace_id, &request.agent_id, resource_kind)
                .await?
        }
    })
}

/// Refuse the rules an approve would add, before the decision is
/// stored. Only the added rules are checked, so a rule that the grant
/// already stores refuses nothing: a stored host rule for a program that
/// runs other programs matches no command. A credential grant counts
/// the domains it already holds against its cap, and a grant that would
/// pass the cap is refused rather than silently trimmed: the user drops
/// a domain in settings first.
async fn check_added_rules(
    state: &AppState,
    request: &Request,
    resource_kind: &str,
    resource_id: Option<&str>,
    rules: &[String],
) -> Result<(), ApiError> {
    if resource_kind != Grant::CREDENTIAL_KIND {
        return crate::grants::check_rules(resource_kind, rules);
    }
    let stored = live_grant(state, request, resource_kind, resource_id)
        .await?
        .map(|grant| grant.allow_rules())
        .unwrap_or_default();
    let added: Vec<String> = rules
        .iter()
        .filter(|rule| !stored.contains(rule))
        .cloned()
        .collect();
    if added.is_empty() {
        return Ok(());
    }
    crate::grants::check_rules(resource_kind, &[stored, added].concat())
}

/// The grant write behind an approve: bootstrap the
/// agent's grant for this resource kind when it is missing, and append
/// the proposed rules on an `always`. A no-change write publishes no
/// event. [`check_added_rules`] has already checked the rules.
async fn write_grant(
    state: &AppState,
    request: &Request,
    resource_kind: &str,
    resource_id: Option<&str>,
    rules: &[String],
) -> Result<(), ApiError> {
    match live_grant(state, request, resource_kind, resource_id).await? {
        // A Grant on a Connection or a Plugin is the user's own act: an
        // approval widens one and never mints one (ADR-0017). A host
        // Grant is the exception, and the approval card is that act: the
        // person read the machine's name on it and said yes.
        None if resource_id.is_some() && resource_kind != Grant::HOST_KIND => Ok(()),
        None => {
            let grant = Grant {
                id: GrantId::generate(),
                workspace_id: request.workspace_id.clone(),
                agent_id: request.agent_id.clone(),
                resource_kind: resource_kind.to_string(),
                resource_id: resource_id.map(str::to_string),
                scope: Grant::allow_scope(rules),
                revision: 1,
                created_at: now_ms(),
                revoked_at: None,
            };
            state.grants.create(&grant).await?;
            crate::grants::publish_grant_event(state, &grant, "grant.changed").await
        }
        Some(mut grant) => {
            let mut allow = grant.allow_rules();
            let missing: Vec<String> = rules
                .iter()
                .filter(|rule| !allow.contains(rule))
                .cloned()
                .collect();
            if missing.is_empty() {
                return Ok(());
            }
            allow.extend(missing);
            grant.scope = grant.with_allow_rules(&allow);
            state
                .grants
                .set_scope(&request.workspace_id, &grant.id, &grant.scope)
                .await?;
            grant.revision += 1;
            crate::grants::publish_grant_event(state, &grant, "grant.changed").await
        }
    }
}
