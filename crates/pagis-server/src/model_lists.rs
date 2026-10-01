//! The Provider Model Lists on the product port, and the one-model
//! default route that onboarding picks from them.
//!
//! The provider keys belong to the installation and change on the
//! Administration Port. The list they unlock is read-only and names no
//! secret, so any signed-in person reads it: the Models settings offer
//! it as the choices of each provider, beside a typed id.
//!
//! The default route is one model (ADR-0025). The person picks it at
//! onboarding from the list of the provider they chose. The
//! preselection is the provider's model in the [`DEFAULT_PREFERENCE`]
//! when the list names it, else the newest listed chat model. A provider
//! whose list is not available takes its [`preferred_candidate`].
//! Fallback candidates on other providers are the person's own addition
//! in the Models settings, never the seed's.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use pagis_agent::{ModelCatalog, ModelListError};
use pagis_core::{DEFAULT_MODEL_ALIAS, PROVIDERS, Provider, ProviderKeys, WorkspaceId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One model a provider lists, in candidate form.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListedModelDto {
    /// The `provider/model` candidate, e.g. `openrouter/anthropic/claude-sonnet-4.6`.
    pub candidate: String,
    /// The context window, after the layers: the provider's report,
    /// then the built-in table, then the conservative default.
    pub context_window: u32,
    pub max_output_tokens: u32,
    /// USD per million input tokens, or `null` when no layer prices the
    /// model: its cost is unknown, never zero.
    pub input_cost: Option<f64>,
    /// USD per million output tokens, or `null` when unknown.
    pub output_cost: Option<f64>,
}

/// One provider's list, or why it is missing.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderModelsDto {
    pub provider: String,
    /// Newest first where the provider reports a release order. Empty
    /// when `error` is set.
    pub models: Vec<ListedModelDto>,
    /// The model id a default pick takes: the provider's preferred model
    /// when the list names it, else the newest listed model whose name
    /// reads as a chat model. `None` when the list has neither.
    pub preselected: Option<String>,
    /// The provider's own words when the list call failed.
    pub error: Option<String>,
}

/// The lists of every provider that holds a key.
#[derive(Debug, Serialize, ToSchema)]
pub struct ModelListsDto {
    pub providers: Vec<ProviderModelsDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetOnboardingDefaultModelRequest {
    /// `anthropic`, `openai`, or `openrouter`.
    pub provider: String,
    /// The model id as the provider lists it, without the provider
    /// prefix. `null` takes the preselection of the provider's list, or
    /// the provider's preferred model when its list is not available.
    pub model: Option<String>,
}

/// The Model Preference of the `default` alias: the models the product
/// selects for a Workspace, best first. Each provider names one. The
/// provider's own list stays the choice: a preferred model is selected
/// only where its provider holds a key and, when the list answers,
/// lists it.
pub const DEFAULT_PREFERENCE: [&str; 3] = [
    "openai/gpt-6-luna",
    "openrouter/openai/gpt-6-luna",
    "anthropic/claude-sonnet-5-5",
];

/// The provider's model in the [`DEFAULT_PREFERENCE`], as a candidate.
pub fn preferred_candidate(provider: Provider) -> &'static str {
    let prefix = format!("{}/", provider.id());
    DEFAULT_PREFERENCE
        .into_iter()
        .find(|candidate| candidate.starts_with(&prefix))
        .expect("each provider names one preferred model")
}

/// The preferred model of `providers`, as a candidate: the first one in
/// the [`DEFAULT_PREFERENCE`] whose provider is among them.
pub fn preferred_of(providers: impl IntoIterator<Item = Provider>) -> Option<&'static str> {
    let providers: Vec<Provider> = providers.into_iter().collect();
    DEFAULT_PREFERENCE
        .into_iter()
        .find(|candidate| providers.contains(&provider_of(candidate)))
}

fn provider_of(candidate: &str) -> Provider {
    candidate
        .split_once('/')
        .and_then(|(provider, _)| Provider::from_id(provider))
        .expect("a preferred candidate names a known provider")
}

/// The provider's preferred model as its list names it: the candidate
/// without the provider prefix.
fn preferred_model(provider: Provider) -> &'static str {
    &preferred_candidate(provider)[provider.id().len() + 1..]
}

/// The model id a list preselects for `provider`, without the provider
/// prefix: the preferred model when the list names it, else the newest
/// listed model whose name reads as a chat model.
fn preselected_model(provider: Provider, listed: &[llm_router::ListedModel]) -> Option<String> {
    let preferred = preferred_model(provider);
    listed
        .iter()
        .find(|model| model.id == preferred)
        .or_else(|| listed.iter().find(|model| model.looks_like_chat()))
        .map(|model| model.id.clone())
}

/// The preselected default model of one provider, as a candidate: the
/// [`preselected_model`] of its list, or its preferred model when the
/// list call fails or lists no chat model.
pub async fn preselected(models: &ModelCatalog, provider: Provider) -> String {
    match models.models(provider).await {
        Ok(list) => match preselected_model(provider, &list) {
            Some(model) => format!("{}/{model}", provider.id()),
            None => preferred_candidate(provider).to_string(),
        },
        Err(error) => {
            tracing::warn!(provider = provider.id(), %error, "the default model takes the preferred model");
            preferred_candidate(provider).to_string()
        }
    }
}

/// The default route for a Workspace that nobody picked a model for:
/// the first preferred model whose provider holds a key and lists it,
/// else the newest listed chat model of the first keyed provider whose
/// list answers, else the preferred model of the first keyed provider,
/// or `None` when no provider holds a key. Providers go in the order of
/// the [`DEFAULT_PREFERENCE`].
pub async fn default_route(
    keys: &ProviderKeys,
    models: &ModelCatalog,
) -> Result<Option<String>, ApiError> {
    let mut keyed = Vec::new();
    for candidate in DEFAULT_PREFERENCE {
        let provider = provider_of(candidate);
        if keys
            .resolve(provider)
            .map_err(crate::settings::secret_error)?
            .is_none()
        {
            continue;
        }
        let listed = match models.models(provider).await {
            Ok(list) => Some(list),
            Err(error) => {
                tracing::warn!(provider = provider.id(), %error, "the provider did not list its models");
                None
            }
        };
        keyed.push((provider, listed));
    }
    let preferred_listed = keyed.iter().find_map(|(provider, listed)| {
        let model = preferred_model(*provider);
        listed
            .as_ref()?
            .iter()
            .any(|listed| listed.id == model)
            .then(|| preferred_candidate(*provider).to_string())
    });
    let newest_listed = || {
        keyed.iter().find_map(|(provider, listed)| {
            let model = listed
                .as_ref()?
                .iter()
                .find(|model| model.looks_like_chat())?;
            Some(format!("{}/{}", provider.id(), model.id))
        })
    };
    Ok(preferred_listed.or_else(newest_listed).or_else(|| {
        keyed
            .first()
            .map(|(provider, _)| preferred_candidate(*provider).to_string())
    }))
}

/// The default route of a Person that an Administrator creates. The
/// Administrator chose a model for the installation, so the Person
/// takes the Administrator's own route, less every candidate whose
/// provider holds no key. With no such candidate the Person takes
/// [`default_route`].
pub async fn route_for_new_person(
    state: &AppState,
    administrator_workspace: &WorkspaceId,
) -> Result<Vec<String>, ApiError> {
    let chosen = state
        .model_aliases
        .get_by_alias(administrator_workspace, DEFAULT_MODEL_ALIAS)
        .await?
        .map(|alias| alias.candidates)
        .unwrap_or_default();
    let reachable = reachable_candidates(&state.keys, chosen)?;
    if !reachable.is_empty() {
        return Ok(reachable);
    }
    Ok(default_route(&state.keys, &state.models)
        .await?
        .into_iter()
        .collect())
}

/// The candidates of a route whose provider holds a key, in order.
fn reachable_candidates(
    keys: &ProviderKeys,
    candidates: Vec<String>,
) -> Result<Vec<String>, ApiError> {
    let mut reachable = Vec::new();
    for candidate in candidates {
        let provider = candidate
            .split_once('/')
            .and_then(|(provider, _)| Provider::from_id(provider));
        let Some(provider) = provider else { continue };
        if keys
            .resolve(provider)
            .map_err(crate::settings::secret_error)?
            .is_some()
        {
            reachable.push(candidate);
        }
    }
    Ok(reachable)
}

/// Give [`default_route`] to every Workspace whose default route names
/// no provider that holds a key.
///
/// A key that the setup page, the Providers view or the onboarding
/// stores or removes changes which providers answer. A route that names
/// none of them fails every message, and on a server nobody answers a
/// model question that would change it. A route that still reaches a
/// provider stays as it is, because a Person or an Administrator chose it.
pub async fn route_unrouted_workspaces(state: &AppState) -> Result<(), ApiError> {
    let mut route: Option<Option<String>> = None;
    for workspace in state.workspaces.list().await? {
        let Some(alias) = state
            .model_aliases
            .get_by_alias(&workspace.id, DEFAULT_MODEL_ALIAS)
            .await?
        else {
            continue;
        };
        if !reachable_candidates(&state.keys, alias.candidates)?.is_empty() {
            continue;
        }
        let candidate = match &route {
            Some(route) => route.clone(),
            None => route
                .insert(default_route(&state.keys, &state.models).await?)
                .clone(),
        };
        // No provider holds a key, so no route can reach one.
        let Some(candidate) = candidate else {
            return Ok(());
        };
        set_default_route(state, &workspace.id, candidate).await?;
    }
    Ok(())
}

/// Make `candidate` the whole default route of the Workspace.
pub async fn set_default_route(
    state: &AppState,
    workspace_id: &WorkspaceId,
    candidate: String,
) -> Result<(), ApiError> {
    set_default_candidates(state, workspace_id, &[candidate]).await
}

/// Make `candidates`, in order, the default route of the Workspace.
pub async fn set_default_candidates(
    state: &AppState,
    workspace_id: &WorkspaceId,
    candidates: &[String],
) -> Result<(), ApiError> {
    let updated = state
        .model_aliases
        .update_candidates(
            workspace_id,
            DEFAULT_MODEL_ALIAS,
            candidates,
            pagis_core::now_ms(),
        )
        .await?;
    if !updated {
        return Err(ApiError::not_found("model alias"));
    }
    Ok(())
}

/// A key change makes the provider's list stale: fetch it again in the
/// background, so the Models settings and the budget read the new key's
/// list.
pub fn key_changed(models: &Arc<ModelCatalog>, provider: Provider) {
    models.forget(provider);
    let models = Arc::clone(models);
    tokio::spawn(async move {
        match models.models(provider).await {
            Ok(_) | Err(ModelListError::NoKey(_)) => {}
            Err(error) => {
                tracing::warn!(provider = provider.id(), %error, "the model list of the new key was not fetched");
            }
        }
    });
}

fn provider_dto(provider: Provider, listed: &[llm_router::ListedModel]) -> ProviderModelsDto {
    ProviderModelsDto {
        provider: provider.id().to_string(),
        preselected: preselected_model(provider, listed),
        models: listed
            .iter()
            .map(|model| {
                let metadata = llm_router::model_metadata(&model.id, Some(model));
                ListedModelDto {
                    candidate: format!("{}/{}", provider.id(), model.id),
                    context_window: metadata.context_window,
                    max_output_tokens: metadata.max_output_tokens,
                    input_cost: metadata.prices.map(|prices| prices.input_cost),
                    output_cost: metadata.prices.map(|prices| prices.output_cost),
                }
            })
            .collect(),
        error: None,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/models",
    responses(
        (status = 200, body = ModelListsDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
/// The Provider Model List of every provider that holds a key, from the
/// installation's cache. A list older than the refresh interval, or
/// fetched with an older key, is fetched again first.
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
) -> Result<Json<ModelListsDto>, ApiError> {
    let mut providers = Vec::new();
    for provider in PROVIDERS {
        match state.models.models(provider).await {
            Ok(listed) => providers.push(provider_dto(provider, &listed)),
            Err(ModelListError::NoKey(_)) => {}
            Err(ModelListError::Secret(message)) => {
                tracing::error!(%message, "secret store error");
                return Err(ApiError::internal());
            }
            Err(ModelListError::Provider(error)) => providers.push(ProviderModelsDto {
                provider: provider.id().to_string(),
                models: Vec::new(),
                preselected: None,
                error: Some(error.to_string()),
            }),
        }
    }
    Ok(Json(ModelListsDto { providers }))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/onboarding/default-model",
    request_body = SetOnboardingDefaultModelRequest,
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// The model step's pick: the one model the default route names. It
/// answers while the Workspace has not finished onboarding, and `409`
/// afterwards, when the Models settings change the route.
pub async fn set_onboarding_default_model(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SetOnboardingDefaultModelRequest>,
) -> Result<StatusCode, ApiError> {
    crate::settings::refuse_after_onboarding(&state, &tenant).await?;
    let provider =
        Provider::from_id(&request.provider).ok_or_else(|| ApiError::not_found("provider"))?;
    if state
        .keys
        .resolve(provider)
        .map_err(crate::settings::secret_error)?
        .is_none()
    {
        return Err(ApiError::validation(format!(
            "no key is configured for {}",
            provider.id()
        )));
    }
    let candidate = match request.model.as_deref().map(str::trim) {
        Some("") => return Err(ApiError::validation("model must not be empty")),
        Some(model) => format!("{}/{model}", provider.id()),
        None => preselected(&state.models, provider).await,
    };
    set_default_route(&state, &tenant.workspace_id, candidate).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod preselection_tests {
    use pagis_core::{PROVIDERS, Provider};

    use super::{DEFAULT_PREFERENCE, preferred_candidate};

    /// Each provider names exactly one model in the Model Preference, so
    /// a provider with a key always has a model to preselect.
    #[test]
    fn each_provider_names_one_preferred_default_model() {
        for provider in PROVIDERS {
            let prefix = format!("{}/", provider.id());
            let named = DEFAULT_PREFERENCE
                .iter()
                .filter(|candidate| candidate.starts_with(&prefix))
                .count();
            assert_eq!(named, 1, "{} names {named} models", provider.id());
            assert!(preferred_candidate(provider).starts_with(&prefix));
        }
    }

    /// The preferred model wins over a newer listed model: OpenRouter
    /// lists small free models newest first.
    #[test]
    fn the_preselection_is_the_preferred_model_when_the_list_names_it() {
        let listed = [
            llm_router::ListedModel::new("qwen/qwen3.8-27b:free"),
            llm_router::ListedModel::new("openai/gpt-6-luna"),
        ];

        let dto = super::provider_dto(Provider::OpenRouter, &listed);

        assert_eq!(dto.preselected.as_deref(), Some("openai/gpt-6-luna"));
    }

    /// A list that does not name the preferred model preselects its
    /// newest chat model.
    #[test]
    fn the_preselection_skips_models_that_take_no_chat_turn() {
        let listed = [
            llm_router::ListedModel::new("gpt-image-2"),
            llm_router::ListedModel::new("gpt-realtime-2.1"),
            llm_router::ListedModel::new("gpt-6-sol"),
            llm_router::ListedModel::new("gpt-5.6-luna"),
        ];

        let dto = super::provider_dto(Provider::OpenAi, &listed);

        assert_eq!(dto.preselected.as_deref(), Some("gpt-6-sol"));
    }
}
