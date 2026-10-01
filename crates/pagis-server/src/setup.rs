//! The server's own first run.
//!
//! A server has an Org and a Workspace from its boot, but nobody can
//! sign in to it: the seeded Administrator has no address and no
//! password. Two things close that gap, and both write the same rows.
//!
//! The deployment's configuration: `PAGIS_ADMIN_EMAIL`,
//! `PAGIS_ADMIN_PASSWORD` and the provider key environment variables,
//! read once at the first start that finds no Administrator with a
//! password. [`from_environment`] is that half, and the daemon's boot
//! calls it.
//!
//! The first-run flow: `GET /api/v1/setup` says what the installation
//! still needs and `POST /api/v1/setup` supplies the address, the
//! password and the provider keys. The Administration Interface serves
//! the page for it on the administration port; the routes and the state are here.
//!
//! **Where the routes answer.** `POST /api/v1/setup` answers on the
//! Administration Port alone. That port binds loopback by default, so
//! the first Administrator is made by a person who can reach the
//! Server's own machine, which is the operator. The product port faces
//! the internet, and a write there would give the first Administrator
//! and the installation's provider keys to the first person who posts.
//! The product port keeps `GET /api/v1/setup`, because its sign-in page
//! names the Administration Port from it.
//!
//! **When the routes answer.** They answer while the installation holds
//! no Client Credential and no Administrator holds a password, and they
//! answer `410 Gone` otherwise. Both halves matter. The password is what
//! says nobody can sign in yet, so the flow closes the moment somebody
//! can. The Client Credential is what says this is a local installation,
//! where the seeded person signs in with that file and never sets a
//! password; without this half a local installation would leave an
//! account-making route open for ever.
//!
//! **One claim.** The first password is a claim in the store
//! ([`pagis_core::UserStore::claim_first_administrator`]): the check that
//! no Administrator holds a password and the write are one transaction.
//! Two requests can both find nobody who can sign in, and only one of
//! them writes. The other answers `410 Gone`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use pagis_core::{ClientKind, Provider, ProviderKeys, SecretStore, User, UserRole};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use utoipa::ToSchema;

use crate::AppState;
use crate::error::ApiError;
use crate::sessions::{hash_password, open_session};

/// The address of the first Administrator, read once at the first start
/// that finds nobody who can sign in.
pub const ADMIN_EMAIL_VAR: &str = "PAGIS_ADMIN_EMAIL";
/// That Administrator's first password.
pub const ADMIN_PASSWORD_VAR: &str = "PAGIS_ADMIN_PASSWORD";

/// The shortest password the setup accepts, which is the floor an
/// Administrator's own resets use.
const MIN_PASSWORD_LEN: usize = 12;

/// What the first-run flow still needs.
#[derive(Debug, Serialize, ToSchema)]
pub struct SetupStateDto {
    /// The origin of the Administration Port, where the first
    /// Administrator is made. It binds loopback by default, so a person
    /// on another machine reaches it through an SSH tunnel.
    pub administration_origin: String,
    /// The provider ids the flow may take a key for.
    pub providers: Vec<String>,
    /// Which of them already hold a key, from the environment, the
    /// config file or the secret store.
    pub configured_providers: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompleteSetupRequest {
    pub email: String,
    pub password: String,
    /// What the agents call the person.
    pub name: Option<String>,
    /// Provider keys by provider id: `anthropic`, `openai`,
    /// `openrouter`. Every one of them is the installation's, so every
    /// person the Administrator creates later thinks on them.
    #[serde(default)]
    pub provider_keys: std::collections::BTreeMap<String, String>,
    /// The IANA timezone of the browser that makes the Administrator.
    /// It becomes the Administrator's own, as a first sign-in's does.
    #[serde(default)]
    pub timezone: Option<String>,
}

/// The `410` a spent setup answers. It is not a `404`: the route
/// exists, and the reason it answers nothing is that the installation
/// already has somebody who can sign in.
fn spent() -> ApiError {
    ApiError {
        status: StatusCode::GONE,
        code: "setup_complete",
        message: "this installation already has an administrator".to_string(),
    }
}

/// Whether the first-run flow is still open. See the module note for
/// why both halves are in it.
pub async fn is_open(state: &AppState) -> Result<bool, ApiError> {
    if state.client_credential.is_some() {
        return Ok(false);
    }
    Ok(administrator_without_a_password(state).await?.is_some())
}

/// Whether nobody can sign in yet: the one Org has an Administrator and
/// no Administrator holds a password. A server starts this way, and its
/// start banner then names where the first Administrator is made.
pub async fn awaits_first_administrator(
    orgs: &dyn pagis_core::OrgStore,
    users: &dyn pagis_core::UserStore,
) -> Result<bool, pagis_core::StoreError> {
    let Some(org) = orgs.list().await?.into_iter().next() else {
        return Ok(false);
    };
    let people = users.list_by_org(&org.id).await?;
    Ok(people
        .iter()
        .any(|person| person.role == UserRole::Administrator)
        && !people
            .iter()
            .any(|person| person.role == UserRole::Administrator && person.password_hash.is_some()))
}

/// The Administrator of the one Org while none of them holds a password.
/// `None` once somebody can sign in, and `None` on an installation with
/// no Org at all, which cannot happen after a boot.
async fn administrator_without_a_password(state: &AppState) -> Result<Option<User>, ApiError> {
    let Some(org) = state.orgs.list().await?.into_iter().next() else {
        return Ok(None);
    };
    let people = state.users.list_by_org(&org.id).await?;
    if people
        .iter()
        .any(|person| person.role == UserRole::Administrator && person.password_hash.is_some())
    {
        return Ok(None);
    }
    Ok(people
        .into_iter()
        .find(|person| person.role == UserRole::Administrator))
}

#[utoipa::path(
    get,
    path = "/api/v1/setup",
    responses(
        (status = 200, body = SetupStateDto),
        (status = 410, body = crate::error::ErrorBody, description = "An administrator already exists"),
    )
)]
pub async fn get_setup(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SetupStateDto>, ApiError> {
    if !is_open(&state).await? {
        return Err(spent());
    }
    let mut configured = Vec::new();
    for provider in pagis_core::PROVIDERS {
        if state
            .keys
            .resolve(provider)
            .map_err(|error| {
                tracing::error!(%error, "the secret store did not answer");
                ApiError::internal()
            })?
            .is_some()
        {
            configured.push(provider.id().to_string());
        }
    }
    Ok(Json(SetupStateDto {
        administration_origin: state.administration.origin.clone(),
        providers: pagis_core::PROVIDERS
            .iter()
            .map(|provider| provider.id().to_string())
            .collect(),
        configured_providers: configured,
    }))
}

/// Make the installation's first Administrator and keep its provider
/// keys. It answers on the Administration Port alone. The answer signs
/// the new Administrator in, so the person who completed the flow goes
/// straight to the product.
#[utoipa::path(
    post,
    path = "/api/v1/setup",
    request_body = CompleteSetupRequest,
    responses(
        (status = 200, body = crate::user::UserDto, description = "The administrator, with a session cookie"),
        (status = 410, body = crate::error::ErrorBody, description = "An administrator already exists"),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn complete_setup(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<CompleteSetupRequest>,
) -> Result<Response, ApiError> {
    if !is_open(&state).await? {
        return Err(spent());
    }
    let Some(person) = administrator_without_a_password(&state).await? else {
        return Err(spent());
    };
    let email = request.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(ApiError::validation("that is not an email address"));
    }
    if request.password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::validation(format!(
            "a password is at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let mut keys = Vec::new();
    for (id, key) in &request.provider_keys {
        let provider = Provider::from_id(id)
            .ok_or_else(|| ApiError::validation(format!("{id} is not a model provider")))?;
        if key.trim().is_empty() {
            return Err(ApiError::validation(format!("the {id} key is empty")));
        }
        keys.push((provider, key.trim().to_string()));
    }

    let now = state.clock.now_ms();
    let hash = hash_password(&request.password)?;
    // The check above is not the claim. Two requests can both find
    // nobody who can sign in, and the store lets only one of them write
    // the first password.
    if !state
        .users
        .claim_first_administrator(&person.id, &email, &hash, now)
        .await?
    {
        return Err(spent());
    }

    // The keys land only after the claim, so a request that loses the
    // claim changes no provider key of the installation.
    for (provider, key) in keys {
        state.keys.set(provider, &key).map_err(|error| {
            tracing::error!(%error, "the provider key was not kept");
            ApiError::internal()
        })?;
        state.models.forget(provider);
    }

    if let Some(name) = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        state.users.set_name(&person.id, name, now).await?;
    }
    // The installation is a server, and this administrator has just
    // answered both of the local wizard's questions: the product
    // opens instead of the wizard, here and for every account this person
    // creates.
    crate::provisioning::onboard_for_a_server(state.workspaces.as_ref(), &person.id, now).await?;
    crate::workspace::take_first_timezone(&state, &person.id, request.timezone.as_deref()).await?;
    // The Administrator answered no model question, so their default
    // route takes the default route of the keys they typed.
    if let Some(route) = crate::model_lists::default_route(&state.keys, &state.models).await?
        && let Some(workspace) = state.workspaces.for_user(&person.id).await?
    {
        crate::model_lists::set_default_route(&state, &workspace.id, route).await?;
    }
    tracing::info!(person = %person.id, "the server setup made the first administrator");
    let secure = state.proxy.is_secure(peer, &headers);
    open_session(&state, &person.id, ClientKind::Browser, None, now, secure).await
}

/// What the environment says about the first Administrator, if
/// anything.
pub struct EnvironmentSetup {
    pub email: String,
    pub password: String,
    /// The provider keys the environment names, to keep in the secret
    /// store so they outlive the process that read them.
    pub provider_keys: Vec<(Provider, String)>,
}

/// Administrator variables that name no usable Administrator: one of
/// the two is missing, or they are not an address and a password long
/// enough.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidAdministrator;

impl std::fmt::Display for InvalidAdministrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{ADMIN_EMAIL_VAR} and {ADMIN_PASSWORD_VAR} are not an address and a password of at \
             least {MIN_PASSWORD_LEN} characters; no administrator was made"
        )
    }
}

/// Read the deployment's configuration. The provider key variables are
/// read here as well as by [`ProviderKeys`], because the resolver reads
/// them per call and a deployment that later drops the variable would
/// otherwise lose its keys.
///
/// An empty variable is an unset one: `deploy/.env.example` leaves both
/// Administrator variables empty for the deployment that finishes setup
/// on the Administration Port, and compose passes them as empty strings.
pub fn environment_setup(
    read: impl Fn(&str) -> Option<String>,
) -> Result<Option<EnvironmentSetup>, InvalidAdministrator> {
    let set = |name: &str| read(name).filter(|value| !value.trim().is_empty());
    let (email, password) = match (set(ADMIN_EMAIL_VAR), set(ADMIN_PASSWORD_VAR)) {
        (None, None) => return Ok(None),
        (Some(email), Some(password)) => (email.trim().to_lowercase(), password),
        _ => return Err(InvalidAdministrator),
    };
    if !email.contains('@') || password.chars().count() < MIN_PASSWORD_LEN {
        return Err(InvalidAdministrator);
    }
    let provider_keys = pagis_core::PROVIDERS
        .into_iter()
        .filter_map(|provider| {
            read(provider.env_var())
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
                .map(|key| (provider, key))
        })
        .collect();
    Ok(Some(EnvironmentSetup {
        email,
        password,
        provider_keys,
    }))
}

/// Give the installation its first Administrator from the environment,
/// once. It answers `true` when this call made one.
///
/// It is idempotent by the same rule the routes use: a start that finds
/// an Administrator with a password does nothing, so the variables may
/// stay in the deployment's configuration for ever and a later start
/// never overwrites a password somebody changed.
pub async fn from_environment(
    stores: &pagis_core::Stores,
    secrets: &dyn SecretStore,
    read: impl Fn(&str) -> Option<String>,
    now: pagis_core::UnixMillis,
) -> Result<bool, SetupError> {
    let setup = match environment_setup(read) {
        Ok(Some(setup)) => setup,
        Ok(None) => return Ok(false),
        Err(invalid) => {
            tracing::warn!("{invalid}");
            return Ok(false);
        }
    };
    let Some(org) = stores.orgs.list().await?.into_iter().next() else {
        return Ok(false);
    };
    let people = stores.users.list_by_org(&org.id).await?;
    if people
        .iter()
        .any(|person| person.role == UserRole::Administrator && person.password_hash.is_some())
    {
        return Ok(false);
    }
    let Some(person) = people
        .into_iter()
        .find(|person| person.role == UserRole::Administrator)
    else {
        return Ok(false);
    };
    // The Administrator answers no model question, so each well-known
    // alias names the preferred models of the first provider the
    // environment gives a key that serves it. The boot runs before any
    // model list exists, so no list confirms the default model; the
    // Models settings offer the list.
    let keyed: Vec<Provider> = setup
        .provider_keys
        .iter()
        .map(|(provider, _)| *provider)
        .collect();
    // A key that is not kept means an installation with an administrator
    // and no model route, which reads as a broken product rather than as
    // a deployment to fix. The boot stops here and names the key.
    for (provider, key) in setup.provider_keys {
        secrets
            .set(provider.secret_name(), &key)
            .map_err(|error| SetupError::ProviderKey {
                provider: provider.id(),
                reason: error.to_string(),
            })?;
    }
    let hash = argon2_hash(&setup.password).map_err(|()| SetupError::PasswordHash)?;
    if !stores
        .users
        .claim_first_administrator(&person.id, &setup.email, &hash, now)
        .await?
    {
        return Ok(false);
    }
    // The deployment named an administrator, so this installation is a
    // server and the local wizard has nothing to ask.
    crate::provisioning::onboard_for_a_server(stores.workspaces.as_ref(), &person.id, now).await?;
    if let Some(workspace) = stores.workspaces.for_user(&person.id).await? {
        for (alias, _) in crate::model_preference::PREFERENCES {
            let Some(route) = crate::model_preference::route_for(alias, &keyed) else {
                continue;
            };
            stores
                .model_aliases
                .update_candidates(&workspace.id, alias, &route, now)
                .await?;
        }
    }
    tracing::info!(
        person = %person.id,
        "the environment made the installation's first administrator"
    );
    Ok(true)
}

/// Why a setup from the environment stopped the boot.
///
/// The deployment named an administrator and the installation's provider
/// keys in one breath. A start that keeps the administrator and drops a
/// key leaves a server nobody can think on, and the person who reads the
/// logs is the person who can fix it, so the boot fails instead.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Store(#[from] pagis_core::StoreError),
    #[error("the {provider} key from the environment could not be kept: {reason}")]
    ProviderKey {
        provider: &'static str,
        reason: String,
    },
    #[error("the administrator password from the environment could not be hashed")]
    PasswordHash,
}

/// The argon2id hash, without the API error shape the route wants.
fn argon2_hash(password: &str) -> Result<String, ()> {
    use argon2::Argon2;
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ())
}

/// Whether the provider key resolver holds a key for any provider, for
/// the log line a first start writes.
pub fn any_key(keys: &ProviderKeys) -> bool {
    pagis_core::PROVIDERS
        .into_iter()
        .any(|provider| matches!(keys.resolve(provider), Ok(Some(_))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn the_environment_names_the_administrator_and_the_keys() {
        let setup = environment_setup(env(&[
            (ADMIN_EMAIL_VAR, " Ada@Example.COM "),
            (ADMIN_PASSWORD_VAR, "correct horse battery"),
            ("ANTHROPIC_API_KEY", "sk-env"),
            ("OPENAI_API_KEY", "   "),
        ]))
        .expect("the variables are valid")
        .expect("the environment is complete");

        assert_eq!(setup.email, "ada@example.com");
        assert_eq!(setup.provider_keys.len(), 1);
        assert_eq!(setup.provider_keys[0].0, Provider::Anthropic);
    }

    /// A deployment that leaves both variables empty, as
    /// `deploy/.env.example` does, has not set them: it finishes setup
    /// on the Administration Port, and nothing warns.
    #[test]
    fn empty_variables_are_unset() {
        assert!(matches!(environment_setup(env(&[])), Ok(None)));
        assert!(matches!(
            environment_setup(env(&[(ADMIN_EMAIL_VAR, ""), (ADMIN_PASSWORD_VAR, "  ")])),
            Ok(None)
        ));
    }

    /// Half the configuration makes no Administrator, and neither does
    /// a password nobody could have meant. Each is an error to warn of.
    #[test]
    fn an_incomplete_environment_makes_nobody() {
        for pairs in [
            &[(ADMIN_EMAIL_VAR, "ada@example.com")][..],
            &[
                (ADMIN_EMAIL_VAR, "ada@example.com"),
                (ADMIN_PASSWORD_VAR, ""),
            ],
            &[(ADMIN_PASSWORD_VAR, "correct horse battery")],
            &[
                (ADMIN_EMAIL_VAR, ""),
                (ADMIN_PASSWORD_VAR, "correct horse battery"),
            ],
            &[
                (ADMIN_EMAIL_VAR, "ada@example.com"),
                (ADMIN_PASSWORD_VAR, "short"),
            ],
            &[
                (ADMIN_EMAIL_VAR, "not-an-address"),
                (ADMIN_PASSWORD_VAR, "correct horse battery"),
            ],
        ] {
            assert!(
                matches!(environment_setup(env(pairs)), Err(InvalidAdministrator)),
                "{pairs:?}"
            );
        }
    }
}
