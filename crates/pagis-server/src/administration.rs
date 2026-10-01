//! What an Administrator does to the installation's people.
//!
//! Accounts are Administrator-created with a password: there is no self-signup and no second Org. An Administrator
//! makes an account, disables and re-enables it, resets a password,
//! reads the roster with the last sign-in, reads what each person spent,
//! and sets each person's monthly Spend Cap.
//!
//! Creating an account writes the person's Workspace through the same
//! seed a local first run uses ([`crate::provisioning`]), so a person
//! who signs in for the first time finds a sprite, its DM, the
//! well-known model aliases and the Report, and no provider key of their
//! own: the Org's keys serve everybody.
//!
//! Every route here takes the [`Administrator`] extractor. A Member gets
//! `403` and the handler never runs.
//!
//! One read is the person's own: `/api/v1/usage` answers the signed-in
//! person's spend, because a person may see what they spent even where
//! they may see nothing of anybody else's.
//!
//! # The Administration Interface
//!
//! [`router`] builds the surface of the administration port: the roster,
//! the spend, the live Sessions, the per-person resources, the
//! installation settings, the Org's Plugins, the health of the daemon and
//! the pull of the Computer Image of an Update. Each of them answers on this port alone. The guard is a
//! layer of the router and not a habit of each handler: every route
//! behind it answers `401` without a Session and `403` to a Member,
//! whatever extractor its handler takes. [`crate::routes`] holds the
//! table of those routes, and the daemon's own tests drive every row of
//! it.
//!
//! The documented exceptions sit outside the layer, and
//! [`crate::routes::ADMINISTRATION_PUBLIC_ROUTES`] says why each one
//! does: the first-run setup, which exists because nobody can sign in
//! yet, and the password sign-in, which is what hands out the Session
//! the guard asks for.

use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router, middleware};
use pagis_computer::ImagePullError;
use pagis_core::{
    RunState, RunUsage, UsagePeriod, UsageTotal, User, UserId, UserRole, WorkspaceId,
    WorkspaceUsage,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::{Administrator, Tenant};
use crate::error::ApiError;
use crate::sessions::hash_password;

/// The shortest password an account may hold. An Administrator sets the
/// first one, so this is a floor and not a policy the person negotiates.
const MIN_PASSWORD_LEN: usize = 12;

/// How many Runs the usage read answers for at most.
const RUN_PAGE: u32 = 200;

/// One Person on the roster, with what an Administrator manages.
#[derive(Debug, Serialize, ToSchema)]
pub struct PersonDto {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    /// `administrator` or `member`.
    pub role: String,
    /// The Workspace this Person owns, or `null` where the seed has not
    /// written one. The usage read names the Workspace, so the roster
    /// carries it.
    pub workspace_id: Option<String>,
    pub disabled: bool,
    /// When a client last signed in as this Person.
    pub last_signed_in_at: Option<i64>,
    /// The monthly Spend Cap in US dollars, or `null` for no cap.
    pub monthly_spend_cap_usd: Option<f64>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RosterDto {
    pub items: Vec<PersonDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAccountRequest {
    pub email: String,
    /// What the agents call the person.
    pub name: String,
    /// The first password. The person changes it later; the daemon keeps
    /// only its argon2id hash.
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetSpendCapRequest {
    /// The monthly cap in US dollars, or `null` to take the cap away.
    pub monthly_spend_cap_usd: Option<f64>,
}

/// The period a usage read answers for. Both ends are Unix
/// milliseconds; leaving them out reads the calendar month the
/// installation is in, which is the period the Spend Cap counts.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PeriodQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UsageTotalDto {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// The sum of the known costs.
    pub cost_usd: f64,
    /// How many model calls the total holds.
    pub calls: i64,
    /// How many of those calls ran on a model with no known price, whose
    /// cost `cost_usd` does not hold.
    pub unpriced_calls: i64,
}

/// What one Person spent over the period, for the Administrator's read.
#[derive(Debug, Serialize, ToSchema)]
pub struct PersonUsageDto {
    pub person: PersonDto,
    pub total: UsageTotalDto,
    /// True when the period is the current month and the total has
    /// reached the person's cap, so the roster says who is stopped.
    pub cap_reached: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct InstallationUsageDto {
    pub from: i64,
    pub to: i64,
    pub items: Vec<PersonUsageDto>,
    /// Every person's spend added up.
    pub total: UsageTotalDto,
}

/// One Run's spend, for the person's own Usage page. The Run
/// transcript has a usage shape of its own (`runs::RunUsageDto`), which
/// reports the tokens of one Run and no money.
#[derive(Debug, Serialize, ToSchema)]
pub struct RunSpendDto {
    pub run_id: String,
    pub total: UsageTotalDto,
    pub last_at: i64,
}

/// What the signed-in person spent, and the cap they are under.
#[derive(Debug, Serialize, ToSchema)]
pub struct MyUsageDto {
    pub from: i64,
    pub to: i64,
    pub total: UsageTotalDto,
    /// The person's monthly Spend Cap, or `null` where they have none.
    pub monthly_spend_cap_usd: Option<f64>,
    pub runs: Vec<RunSpendDto>,
}

impl From<UsageTotal> for UsageTotalDto {
    fn from(total: UsageTotal) -> Self {
        Self {
            input_tokens: total.input_tokens,
            output_tokens: total.output_tokens,
            cache_read_tokens: total.cache_read_tokens,
            cache_write_tokens: total.cache_write_tokens,
            cost_usd: total.cost_usd,
            calls: total.calls,
            unpriced_calls: total.unpriced_calls,
        }
    }
}

impl From<RunUsage> for RunSpendDto {
    fn from(usage: RunUsage) -> Self {
        Self {
            run_id: usage.run_id.to_string(),
            total: usage.total.into(),
            last_at: usage.last_at,
        }
    }
}

fn person_dto(person: &User, workspace_id: Option<&WorkspaceId>) -> PersonDto {
    PersonDto {
        id: person.id.to_string(),
        email: person.email.clone(),
        name: person.name.clone(),
        role: person.role.as_str().to_string(),
        workspace_id: workspace_id.map(|id| id.to_string()),
        disabled: person.is_disabled(),
        last_signed_in_at: person.last_signed_in_at,
        monthly_spend_cap_usd: person.monthly_spend_cap_usd,
        created_at: person.created_at,
    }
}

/// The one Org of the installation.
async fn org(state: &AppState) -> Result<pagis_core::Org, ApiError> {
    state.orgs.list().await?.into_iter().next().ok_or_else(|| {
        tracing::error!("the installation has no Org");
        ApiError::internal()
    })
}

/// The period a query names, or the calendar month of the
/// Administrator's own Workspace when it names none.
async fn period(state: &AppState, tenant: &Tenant, query: &PeriodQuery) -> UsagePeriod {
    let timezone = match state.workspaces.get(&tenant.workspace_id).await {
        Ok(Some(workspace)) => workspace.timezone,
        _ => "UTC".to_string(),
    };
    let month = UsagePeriod::calendar_month(state.clock.now_ms(), &timezone);
    UsagePeriod {
        from: query.from.unwrap_or(month.from),
        to: query.to.unwrap_or(month.to),
    }
}

/// The Person this path names, inside this installation's Org.
async fn person_of(state: &AppState, id: &str) -> Result<User, ApiError> {
    let org = org(state).await?;
    let person = state
        .users
        .get(&UserId::from(id.to_string()))
        .await?
        .filter(|person| person.org_id == org.id)
        .ok_or_else(|| ApiError::not_found("that person"))?;
    Ok(person)
}

fn check_password(password: &str) -> Result<String, ApiError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::validation(format!(
            "a password is at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    hash_password(password)
}

fn check_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_lowercase();
    // One `@` with something on each side is the whole check. Anything
    // stricter refuses addresses that work, and the daemon never sends
    // to this address.
    match email.split_once('@') {
        Some((local, domain)) if !local.is_empty() && domain.contains('.') => Ok(email),
        _ => Err(ApiError::validation("that is not an email address")),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/administration/people",
    responses(
        (status = 200, body = RosterDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn list_people(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<RosterDto>, ApiError> {
    let org = org(&state).await?;
    let mut items = Vec::new();
    for person in state.users.list_by_org(&org.id).await? {
        let workspace = state.workspaces.for_user(&person.id).await?;
        items.push(person_dto(&person, workspace.as_ref().map(|w| &w.id)));
    }
    Ok(Json(RosterDto { items }))
}

/// Create an account and the person's Workspace.
///
/// The Workspace comes from the same seed a local first run uses, so the
/// person signs in to a sprite that can already think: the Org's
/// provider keys serve them and they hold none of their own.
#[utoipa::path(
    post,
    path = "/api/v1/administration/people",
    request_body = CreateAccountRequest,
    responses(
        (status = 201, body = PersonDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn create_account(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Json(request): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<PersonDto>), ApiError> {
    let email = check_email(&request.email)?;
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::validation("a person needs a name"));
    }
    let password_hash = check_password(&request.password)?;
    let org = org(&state).await?;
    let now = state.clock.now_ms();

    let person = User {
        email: Some(email),
        name: Some(name.clone()),
        password_hash: Some(password_hash),
        ..User::new(org.id, UserRole::Member, now)
    };
    // The unique index on the address is what refuses a second account
    // for one person, so two requests that race cannot both win. A
    // store conflict is this request's fault and not the daemon's, so
    // it answers `409` rather than the `500` every other store error
    // reads as.
    state
        .users
        .create(&person)
        .await
        .map_err(|error| match error {
            pagis_core::StoreError::Conflict(message) => ApiError::conflict(message),
            other => ApiError::from(other),
        })?;

    // The installation's timezone is the Administrator's own Workspace
    // setting, which the System tab holds; a new person starts there and
    // changes it if they want to.
    let timezone = state
        .workspaces
        .get(&administrator.workspace_id)
        .await?
        .map(|workspace| workspace.timezone)
        .unwrap_or_else(|| "UTC".to_string());
    let workspace = crate::provisioning::WorkspaceSeed {
        workspaces: state.workspaces.as_ref(),
        agents: state.agent_store.as_ref(),
        channels: state.channels.as_ref(),
        participants: state.participants.as_ref(),
        messages: state.messages.as_ref(),
        model_aliases: state.model_aliases.as_ref(),
        schedules: state.schedules.as_ref(),
        events: state.events.as_ref(),
    }
    // An account an administrator creates is on a server by definition,
    // and the administrator configured the installation for everybody:
    // the person lands in the product, not in the local wizard.
    .run(
        &person.id,
        &name,
        &timezone,
        crate::provisioning::Onboarding::Done,
        now,
    )
    .await?;
    // The person answers no model question, so their default route
    // follows the model the Administrator chose, where a key reaches it.
    let route =
        crate::model_lists::route_for_new_person(&state, &administrator.workspace_id).await?;
    if !route.is_empty() {
        crate::model_lists::set_default_candidates(&state, &workspace.id, &route).await?;
    }

    Ok((
        StatusCode::CREATED,
        Json(person_dto(&person, Some(&workspace.id))),
    ))
}

/// Disable an account. Every Session of the person goes with it, so a
/// client that holds one stops working at once rather than at its
/// expiry. The Workspace and everything in it stay, so re-enabling gives
/// the same person the same records back.
#[utoipa::path(
    post,
    path = "/api/v1/administration/people/{user_id}/disable",
    params(("user_id" = String, Path, description = "The person")),
    responses(
        (status = 200, body = PersonDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn disable_account(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path(user_id): Path<String>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = person_of(&state, &user_id).await?;
    if person.id == administrator.user_id {
        return Err(ApiError::validation(
            "an administrator does not disable their own account",
        ));
    }
    let now = state.clock.now_ms();
    state.users.set_disabled(&person.id, Some(now), now).await?;
    let ended = end_every_session(&state, &person.id).await?;
    tracing::info!(person = %person.id, ended, "an account was disabled");
    read_person(&state, &person.id).await
}

/// End every Session of a Person, and everything those Sessions hold
/// open: each socket and each Media Relay path closes, and each
/// Takeover of the Person's Computers ends, because no Session remains
/// that could hand one back. Answer how many Sessions ended.
async fn end_every_session(state: &AppState, person: &UserId) -> Result<u64, ApiError> {
    // The records go first and the live connections after, so a
    // connection that opens between the two finds no Session.
    let ended = state.sessions.delete_for_user(person).await?;
    state.live_connections.end_person(person);
    if let Some(workspace) = state.workspaces.for_user(person).await? {
        state
            .computers
            .get(&workspace.id)
            .handback_all("sessions ended")
            .await;
    }
    Ok(ended)
}

#[utoipa::path(
    post,
    path = "/api/v1/administration/people/{user_id}/enable",
    params(("user_id" = String, Path, description = "The person")),
    responses(
        (status = 200, body = PersonDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn enable_account(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(user_id): Path<String>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = person_of(&state, &user_id).await?;
    let now = state.clock.now_ms();
    state.users.set_disabled(&person.id, None, now).await?;
    read_person(&state, &person.id).await
}

/// Reset a password. Every Session of the person ends with it: a
/// password reset that left a stolen Session alive would reset nothing.
#[utoipa::path(
    post,
    path = "/api/v1/administration/people/{user_id}/password",
    params(("user_id" = String, Path, description = "The person")),
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, body = PersonDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(user_id): Path<String>,
    Json(request): Json<ResetPasswordRequest>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = person_of(&state, &user_id).await?;
    let hash = check_password(&request.password)?;
    let now = state.clock.now_ms();
    state
        .users
        .set_password_hash(&person.id, Some(&hash), now)
        .await?;
    let ended = end_every_session(&state, &person.id).await?;
    tracing::info!(person = %person.id, ended, "a password was reset");
    read_person(&state, &person.id).await
}

#[utoipa::path(
    put,
    path = "/api/v1/administration/people/{user_id}/spend-cap",
    params(("user_id" = String, Path, description = "The person")),
    request_body = SetSpendCapRequest,
    responses(
        (status = 200, body = PersonDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_spend_cap(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(user_id): Path<String>,
    Json(request): Json<SetSpendCapRequest>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = person_of(&state, &user_id).await?;
    if let Some(cap) = request.monthly_spend_cap_usd
        && !(cap.is_finite() && cap > 0.0)
    {
        return Err(ApiError::validation(
            "a spend cap is a positive number of dollars, or null for none",
        ));
    }
    let now = state.clock.now_ms();
    state
        .users
        .set_monthly_spend_cap(&person.id, request.monthly_spend_cap_usd, now)
        .await?;
    read_person(&state, &person.id).await
}

async fn read_person(state: &AppState, id: &UserId) -> Result<Json<PersonDto>, ApiError> {
    let person = state
        .users
        .get(id)
        .await?
        .ok_or_else(|| ApiError::not_found("that person"))?;
    let workspace = state.workspaces.for_user(id).await?;
    Ok(Json(person_dto(
        &person,
        workspace.as_ref().map(|workspace| &workspace.id),
    )))
}

/// Spend per person for a period. This is the read that answers
/// the first billing question, and it is the Administrator's alone.
#[utoipa::path(
    get,
    path = "/api/v1/administration/usage",
    params(PeriodQuery),
    responses(
        (status = 200, body = InstallationUsageDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn installation_usage(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Query(query): Query<PeriodQuery>,
) -> Result<Json<InstallationUsageDto>, ApiError> {
    let period = period(&state, &administrator, &query).await;
    let by_workspace: std::collections::HashMap<String, UsageTotal> = state
        .usage
        .totals_by_workspace(period)
        .await?
        .into_iter()
        .map(
            |WorkspaceUsage {
                 workspace_id,
                 total,
             }| (workspace_id.to_string(), total),
        )
        .collect();

    let org = org(&state).await?;
    let mut items = Vec::new();
    let mut total = UsageTotal::default();
    for person in state.users.list_by_org(&org.id).await? {
        let workspace = state.workspaces.for_user(&person.id).await?;
        let spent = workspace
            .as_ref()
            .and_then(|workspace| by_workspace.get(&workspace.id.to_string()))
            .cloned()
            .unwrap_or_default();
        total.input_tokens += spent.input_tokens;
        total.output_tokens += spent.output_tokens;
        total.cache_read_tokens += spent.cache_read_tokens;
        total.cache_write_tokens += spent.cache_write_tokens;
        total.cost_usd += spent.cost_usd;
        total.calls += spent.calls;
        total.unpriced_calls += spent.unpriced_calls;
        let cap_reached = person
            .monthly_spend_cap_usd
            .is_some_and(|cap| spent.cost_usd >= cap);
        items.push(PersonUsageDto {
            person: person_dto(&person, workspace.as_ref().map(|workspace| &workspace.id)),
            total: spent.into(),
            cap_reached,
        });
    }
    // The biggest spender first: the question this read answers is who
    // spent the budget.
    items.sort_by(|left, right| {
        right
            .total
            .cost_usd
            .total_cmp(&left.total.cost_usd)
            .then_with(|| left.person.id.cmp(&right.person.id))
    });
    Ok(Json(InstallationUsageDto {
        from: period.from,
        to: period.to,
        items,
        total: total.into(),
    }))
}

/// What the signed-in person spent. A Member reads this and nothing
/// else of the installation's accounting.
#[utoipa::path(
    get,
    path = "/api/v1/usage",
    params(PeriodQuery),
    responses(
        (status = 200, body = MyUsageDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn my_usage(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<PeriodQuery>,
) -> Result<Json<MyUsageDto>, ApiError> {
    let period = period(&state, &tenant, &query).await;
    let total = state
        .usage
        .total_for_workspace(&tenant.workspace_id, period)
        .await?;
    let runs = state
        .usage
        .runs_for_workspace(&tenant.workspace_id, period, RUN_PAGE)
        .await?;
    let cap = state
        .users
        .get(&tenant.user_id)
        .await?
        .and_then(|person| person.monthly_spend_cap_usd);
    Ok(Json(MyUsageDto {
        from: period.from,
        to: period.to,
        total: total.into(),
        monthly_spend_cap_usd: cap,
        runs: runs.into_iter().map(RunSpendDto::from).collect(),
    }))
}

// ---- The Administration Interface ----

/// One live Session, for the read that says who is signed in.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionDto {
    pub id: String,
    pub person: PersonDto,
    /// `browser` or `desktop`.
    pub client_kind: String,
    /// The machine name of a Client App Session, or null.
    pub client_name: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
    /// True for the Session the reading Administrator holds, so the
    /// list never reads as somebody else's client.
    pub current: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionsDto {
    pub items: Vec<SessionDto>,
}

/// One machine of one Person, for the read that says which clients can
/// act. A Session says who is signed in; a Host says which machine
/// a host action would run on, which is the other half of the same
/// question.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdministrationHostDto {
    pub id: String,
    pub person: PersonDto,
    pub name: String,
    pub platform: String,
    pub capabilities: Vec<String>,
    /// True while the daemon holds this machine's connection.
    pub present: bool,
    pub last_seen_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AdministrationHostsDto {
    pub items: Vec<AdministrationHostDto>,
}

/// What one Person's Computers take of the host.
#[derive(Debug, Serialize, ToSchema)]
pub struct PersonResourcesDto {
    pub person: PersonDto,
    /// The tenant's containers, awake or not.
    pub containers: u32,
    /// The named volumes of the tenant's Agents.
    pub volumes: u32,
    /// The bytes those volumes take, or `null` where Docker did not
    /// answer: a missing figure is not a failed page.
    pub volume_bytes: Option<u64>,
    /// How many Computers of this Person are awake now.
    pub awake_computers: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResourcesDto {
    pub items: Vec<PersonResourcesDto>,
    /// How many Computers the whole server holds awake now, and how
    /// many it may.
    pub awake_on_server: u32,
    pub awake_cap_per_server: u32,
    pub awake_cap_per_tenant: u32,
}

/// The health of the daemon, as an Administrator reads it.
#[derive(Debug, Serialize, ToSchema)]
pub struct InstallationHealthDto {
    pub version: String,
    /// `sqlite` or `postgres`.
    pub database: String,
    /// The Docker endpoint in use, or `null` where Pagis found none and
    /// no Computer runs.
    pub docker_endpoint: Option<String>,
    /// The Runs waiting to start, over every Workspace.
    pub queued_runs: u32,
    /// The Runs a model is answering now.
    pub running_runs: u32,
    /// Every Run of the installation that has not finished, which is
    /// the two counts above and the Runs parked on a person.
    pub unfinished_runs: u32,
    /// Whether this machine holds a Computer volume to its size:
    /// `supported`, `unsupported`, or `unknown` while Docker does not
    /// answer or the limits name no volume size. `unsupported` means
    /// that nothing bounds what `/data` holds, which
    /// https://docs.pagis.co/server/computers#bound-a-computers-disk says how to fix.
    pub volume_quota: String,
    /// Whether this machine holds the writable container layer of a
    /// Computer to its size: `supported`, `unsupported`, or `unknown`
    /// until a Computer wakes after the daemon starts or when the limits
    /// name no layer size. `unsupported` means that nothing bounds what
    /// an Agent writes outside `/data` while its Computer is awake, which
    /// https://docs.pagis.co/server/computers#bound-a-computers-disk says how to fix.
    pub container_quota: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetSignInRequest {
    pub email: String,
    pub password: String,
}

/// Give a Person the address and the password they sign in with.
///
/// The seeded Person of a local installation holds neither: the
/// Client App trades the Client Credential for a Session, so nothing
/// asks them for a password until they want a browser to sign in without
/// the client. This is where they set one.
///
/// Both halves are written at once, because an address without a
/// password and a password without an address are each half a way in.
/// The Sessions of the Person end with it, as a reset does, unless the
/// Person is the Administrator who asks: signing yourself out of the
/// interface you are using tells nobody anything.
#[utoipa::path(
    put,
    path = "/api/v1/administration/people/{user_id}/sign-in",
    params(("user_id" = String, Path, description = "The person")),
    request_body = SetSignInRequest,
    responses(
        (status = 200, body = PersonDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_sign_in(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path(user_id): Path<String>,
    Json(request): Json<SetSignInRequest>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = person_of(&state, &user_id).await?;
    let email = check_email(&request.email)?;
    let hash = check_password(&request.password)?;
    let now = state.clock.now_ms();
    state
        .users
        .set_email_and_password(&person.id, &email, &hash, now)
        .await
        .map_err(|error| match error {
            pagis_core::StoreError::Conflict(message) => ApiError::conflict(message),
            other => ApiError::from(other),
        })?;
    if person.id != administrator.user_id {
        let ended = end_every_session(&state, &person.id).await?;
        tracing::info!(person = %person.id, ended, "a way in was set for a person");
    }
    read_person(&state, &person.id).await
}

/// Who is signed in, from which kind of client, and since when.
#[utoipa::path(
    get,
    path = "/api/v1/administration/sessions",
    responses(
        (status = 200, body = SessionsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
) -> Result<Json<SessionsDto>, ApiError> {
    let now = state.clock.now_ms();
    let mut items = Vec::new();
    for session in state.sessions.list_live(now).await? {
        // A Session whose Person is gone names nobody, so it is left
        // out rather than shown as a row with no name.
        let Some(person) = state.users.get(&session.user_id).await? else {
            continue;
        };
        let workspace = state.workspaces.for_user(&person.id).await?;
        items.push(SessionDto {
            id: session.id.to_string(),
            person: person_dto(&person, workspace.as_ref().map(|workspace| &workspace.id)),
            client_kind: session.client_kind.as_str().to_string(),
            client_name: session.client_name.clone(),
            created_at: session.created_at,
            last_used_at: session.last_used_at,
            expires_at: session.expires_at,
            current: session.id == administrator.session_id,
        });
    }
    Ok(Json(SessionsDto { items }))
}

/// Every machine of every Person, with whether it is connected.
///
/// It sits beside the Sessions, because a host action needs a present
/// client and not only a Session: an administrator who reads that nobody
/// can run a host command reads it here.
#[utoipa::path(
    get,
    path = "/api/v1/administration/hosts",
    responses(
        (status = 200, body = AdministrationHostsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn list_hosts(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<AdministrationHostsDto>, ApiError> {
    let org = org(&state).await?;
    let mut items = Vec::new();
    for person in state.users.list_by_org(&org.id).await? {
        let Some(workspace) = state.workspaces.for_user(&person.id).await? else {
            continue;
        };
        for host in state.hosts.list(&workspace.id).await? {
            items.push(AdministrationHostDto {
                present: state.host_presence.present(&host.id),
                id: host.id.to_string(),
                person: person_dto(&person, Some(&workspace.id)),
                name: host.name,
                platform: host.platform,
                capabilities: host.capabilities,
                last_seen_at: host.last_seen_at,
            });
        }
    }
    Ok(Json(AdministrationHostsDto { items }))
}

/// What each Person's Computers take of the host.
#[utoipa::path(
    get,
    path = "/api/v1/administration/resources",
    responses(
        (status = 200, body = ResourcesDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn installation_resources(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<ResourcesDto>, ApiError> {
    let org = org(&state).await?;
    let caps = state.computers.ceiling().caps();
    let mut items = Vec::new();
    for person in state.users.list_by_org(&org.id).await? {
        let workspace = state.workspaces.for_user(&person.id).await?;
        let person_dto = person_dto(&person, workspace.as_ref().map(|workspace| &workspace.id));
        let Some(workspace) = workspace else {
            // A Person with no Workspace holds no Docker object either.
            items.push(PersonResourcesDto {
                person: person_dto,
                containers: 0,
                volumes: 0,
                volume_bytes: Some(0),
                awake_computers: 0,
            });
            continue;
        };
        // Docker that cannot answer leaves the figures out and the page
        // standing, as the Desk's disk read does.
        let resources = match state.computers.get(&workspace.id).resources().await {
            Ok(resources) => Some(resources),
            Err(error) => {
                tracing::warn!(%error, workspace = %workspace.id, "the resource read failed");
                None
            }
        };
        items.push(PersonResourcesDto {
            person: person_dto,
            containers: resources.map(|read| read.containers).unwrap_or(0),
            volumes: resources.map(|read| read.volumes).unwrap_or(0),
            volume_bytes: resources.map(|read| read.volume_bytes),
            awake_computers: state.computers.ceiling().awake(&workspace.id),
        });
    }
    Ok(Json(ResourcesDto {
        items,
        awake_on_server: state.computers.ceiling().awake_on_server(),
        awake_cap_per_server: caps.per_server,
        awake_cap_per_tenant: caps.per_tenant,
    }))
}

/// The health of the daemon: what it runs, what it keeps its
/// records in, where it reaches Docker, and how much work waits.
#[utoipa::path(
    get,
    path = "/api/v1/administration/health",
    responses(
        (status = 200, body = InstallationHealthDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn installation_health(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<InstallationHealthDto>, ApiError> {
    let unfinished = state.runs.list_unfinished().await?;
    let count = |wanted: RunState| {
        u32::try_from(unfinished.iter().filter(|run| run.state == wanted).count())
            .unwrap_or(u32::MAX)
    };
    Ok(Json(InstallationHealthDto {
        version: crate::VERSION.to_string(),
        database: state.database_backend.clone(),
        docker_endpoint: state.docker_discovery.probe().await.endpoint,
        queued_runs: count(RunState::Queued),
        running_runs: count(RunState::Running),
        unfinished_runs: u32::try_from(unfinished.len()).unwrap_or(u32::MAX),
        volume_quota: state.computers.volume_quota().await.as_str().to_string(),
        container_quota: state.computers.container_quota().await.as_str().to_string(),
    }))
}

/// The Computer Image that the Client App asks the daemon to pull.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ComputerImagePullRequest {
    /// `<repository>@sha256:<64 lowercase hexadecimal characters>`. The
    /// repository is the repository of the pinned Computer Image.
    pub image: String,
}

/// The Computer Image that the daemon pulled.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComputerImageDto {
    pub image: String,
}

/// Pull a Computer Image, and answer when the pull ends.
///
/// Before a restart to an Update, the Client App of a Local Installation
/// asks for the Computer Image that the next release pins, so the new
/// daemon finds it present (ADR-0027). The daemon pulls only an image of
/// the repository of its own pinned image, and only by its digest. A
/// second request for the same image joins the pull that runs, and a
/// request that goes away does not stop the pull. A failed pull answers
/// `502`, and `503` where Docker does not answer.
#[utoipa::path(
    post,
    path = "/api/v1/administration/computer-image/pull",
    request_body = ComputerImagePullRequest,
    responses(
        (status = 200, body = ComputerImageDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    )
)]
pub async fn pull_computer_image(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<ComputerImagePullRequest>,
) -> Result<Json<ComputerImageDto>, ApiError> {
    state
        .computers
        .pull_image(&request.image)
        .await
        .map_err(|error| match error {
            ImagePullError::Refused(message) => ApiError::validation(message),
            ImagePullError::NoDocker(_) => ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "docker_unavailable",
                message: error.to_string(),
            },
            ImagePullError::Failed(_) => ApiError {
                status: StatusCode::BAD_GATEWAY,
                code: "image_pull_failed",
                message: error.to_string(),
            },
        })?;
    Ok(Json(ComputerImageDto {
        image: request.image,
    }))
}

/// Refuse anybody who is not an Administrator, before the handler runs.
///
/// It reads the [`Tenant`] the session middleware left on the request,
/// so it sits inside that middleware and outside every route of the
/// administration port. A handler that takes [`Tenant`] rather than
/// [`Administrator`] is guarded all the same, which is the point: the
/// guard is the router's and not each handler's.
async fn require_administrator(request: Request, next: Next) -> Result<Response, ApiError> {
    let role = request
        .extensions()
        .get::<Tenant>()
        .ok_or_else(ApiError::unauthorized)?
        .role;
    if role != UserRole::Administrator {
        return Err(ApiError::forbidden(
            "the administration interface belongs to the installation, and an \
             administrator reads it",
        ));
    }
    Ok(next.run(request).await)
}

/// The Administration Interface: every route of the
/// administration port.
///
/// The port is the installation's own, bound to loopback by default, so
/// the product port can face the team while this one stays private. What
/// it serves is the installation: the people, the spend, the Sessions,
/// the resources, the settings and the health.
///
/// Two layers close it. [`crate::auth::authenticate`] resolves the
/// Session cookie, and [`require_administrator`] refuses a Member, so a
/// route added to this router is guarded by construction and a Member
/// gets nothing rather than a partial view.
///
/// The routes outside those layers are the documented exceptions of
/// [`crate::routes::ADMINISTRATION_PUBLIC_ROUTES`]: the first-run setup,
/// and the password sign-in that hands out the Session.
///
/// No CORS layer belongs here. The administration page is served from
/// this port itself, so every call it makes is same-origin; a page on
/// another origin gets no answer it can read, including the product's
/// own origin. The origin check that [`crate::routers`] puts around this
/// router refuses a write from such a page.
pub fn router(state: Arc<AppState>) -> Router {
    let guarded = Router::new()
        .route(
            "/api/v1/administration/people",
            get(list_people).post(create_account),
        )
        .route(
            "/api/v1/administration/people/{user_id}/disable",
            post(disable_account),
        )
        .route(
            "/api/v1/administration/people/{user_id}/enable",
            post(enable_account),
        )
        .route(
            "/api/v1/administration/people/{user_id}/password",
            post(reset_password),
        )
        .route(
            "/api/v1/administration/people/{user_id}/sign-in",
            put(set_sign_in),
        )
        .route(
            "/api/v1/administration/people/{user_id}/spend-cap",
            put(set_spend_cap),
        )
        .route("/api/v1/administration/usage", get(installation_usage))
        .route("/api/v1/administration/sessions", get(list_sessions))
        .route("/api/v1/administration/hosts", get(list_hosts))
        .route(
            "/api/v1/administration/resources",
            get(installation_resources),
        )
        .route("/api/v1/administration/health", get(installation_health))
        .route(
            "/api/v1/administration/computer-image/pull",
            post(pull_computer_image),
        )
        // Who the page serves. A Member who reaches the port gets `403`
        // from the layer here too, so the page says what is wrong
        // instead of showing an empty administration.
        .route("/api/v1/user", get(crate::user::get_user))
        .route(
            "/api/v1/sessions/current",
            delete(crate::sessions::sign_out),
        )
        // The installation settings. They answer on this port alone:
        // the product port serves a Person's own settings.
        .route(
            "/api/v1/settings/system",
            get(crate::system::get_system_settings).put(crate::system::set_system_settings),
        )
        .route(
            "/api/v1/settings/system/multi-user",
            put(crate::system::enable_multi_user).delete(crate::system::disable_multi_user),
        )
        .route(
            "/api/v1/settings/system/analytics",
            put(crate::system::set_analytics),
        )
        .route(
            "/api/v1/settings/system/docker/probe",
            post(crate::system::probe_docker),
        )
        // The installation's setup of every provider: the model keys,
        // the Installation OAuth Client, the carrier account with its
        // SIP sign-in and the mail domain. One set of routes serves each
        // part every provider declares; a person's own use of a provider
        // stays on the product port.
        .route(
            "/api/v1/administration/providers",
            get(crate::providers::list_provider_setups),
        )
        .route(
            "/api/v1/administration/providers/{provider}/{part}",
            put(crate::providers::configure_provider_part)
                .delete(crate::providers::remove_provider_part),
        )
        .route(
            "/api/v1/administration/providers/{provider}/{part}/test",
            post(crate::providers::test_provider_part),
        )
        .route("/api/v1/system/restart", post(crate::system::restart))
        // Which Plugins the Org holds, and what each one is bound to
        // (ADR-0017). A Person grants an installed Plugin to their own
        // Agent on the product port; installing one is the Org's.
        .route(
            "/api/v1/plugins",
            get(crate::plugins::list_plugins).post(crate::plugins::install_plugin),
        )
        .route(
            "/api/v1/plugins/{plugin_id}",
            get(crate::plugins::get_plugin).delete(crate::plugins::uninstall_plugin),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/update",
            post(crate::plugins::update_plugin),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/bindings/{field}",
            put(crate::plugins::bind_plugin_field),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/start",
            post(crate::plugins::start_plugin),
        )
        .layer(middleware::from_fn(require_administrator))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            crate::auth::authenticate,
        ));

    Router::new()
        // The server's own first run. It answers while the
        // installation holds no administrator who can sign in, and
        // `410 Gone` afterwards; there is nobody to hold a Session
        // before it runs, so it holds none. Server Setup posts here
        // alone: this port binds loopback by default, and the product
        // port faces the internet.
        .route(
            "/api/v1/setup",
            get(crate::setup::get_setup).post(crate::setup::complete_setup),
        )
        // The way in on this port. Without it the private port
        // would need the product port to sign a person in, which an
        // administrator who reaches this port through a tunnel cannot
        // do. It hands out a Session and reads no record of anybody.
        .route(
            "/api/v1/sessions",
            post(crate::sessions::sign_in_with_password),
        )
        .merge(guarded)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_needs_a_local_part_and_a_dotted_domain() {
        assert_eq!(
            check_email("  Grace@Example.COM ").unwrap(),
            "grace@example.com"
        );
        for bad in ["", "grace", "@example.com", "grace@example", "grace@"] {
            assert!(check_email(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn a_short_password_is_refused() {
        assert!(check_password("short").is_err());
        assert!(check_password("correct horse battery").is_ok());
    }
}
