//! Provider Keys of the installation, on the Administration Port.
//! Connection setups use the native Connections routes.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use pagis_core::{PROVIDERS, Provider, ProviderKeys};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Administrator;
use crate::error::ApiError;
use crate::settings::{ProviderFieldDto, secret_error};

/// One value a part states about what the installation holds, or what
/// the administrator copies into the provider's console. It is never a
/// secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct SetupFactDto {
    pub label: String,
    pub value: String,
}

/// One part of a provider that the installation sets up, and where it
/// stands.
#[derive(Debug, Serialize, ToSchema)]
pub struct SetupPartDto {
    /// The path segment the routes name the part by.
    pub id: String,
    /// `connection`, `sip_credential`, `oauth_client` or `model_key`.
    pub kind: String,
    pub label: String,
    /// What the administrator reads before they type.
    pub blurb: String,
    /// What the form asks for. A secret field is masked and never read
    /// back.
    pub fields: Vec<ProviderFieldDto>,
    pub configured: bool,
    /// The Connection's status, for a Connection part that exists:
    /// `connected`, `unavailable` or `reauth_required`.
    pub status: Option<String>,
    /// The Installation Connection this part is, once it exists.
    pub connection_id: Option<String>,
    /// Whether a test proves the kept credential at the provider.
    pub testable: bool,
    pub facts: Vec<SetupFactDto>,
}

/// One provider as the installation sets it up.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderSetupDto {
    /// The catalog id or the model provider id.
    pub provider: String,
    pub label: String,
    /// `models`, `accounts`, `telephony` or `mailboxes`.
    pub group: String,
    pub parts: Vec<SetupPartDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderSetupPage {
    pub items: Vec<ProviderSetupDto>,
}

/// The values of one part's fields, by field key. A secret field passes
/// to the provider or the secret store and is never written to the
/// database. The type carries no `Debug`, so none of them reaches a log
/// line.
#[derive(Deserialize, ToSchema)]
pub struct ConfigureSetupPartRequest {
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

/// The keys for the portable read and the state for native Connection setups.
#[derive(Clone)]
pub(crate) struct ProviderSetupState {
    pub keys: Arc<ProviderKeys>,
    #[cfg(feature = "native")]
    app: Arc<AppState>,
}

impl axum::extract::FromRef<Arc<AppState>> for ProviderSetupState {
    fn from_ref(state: &Arc<AppState>) -> Self {
        Self {
            keys: state.keys.clone(),
            #[cfg(feature = "native")]
            app: state.clone(),
        }
    }
}

fn model_provider(id: &str) -> Result<Provider, ApiError> {
    Provider::from_id(id).ok_or_else(|| ApiError::not_found("provider setup"))
}

fn describe_key(keys: &ProviderKeys, provider: Provider) -> Result<ProviderSetupDto, ApiError> {
    let source = keys
        .status()
        .map_err(secret_error)?
        .into_iter()
        .find(|key| key.provider == provider)
        .and_then(|key| key.source);
    Ok(ProviderSetupDto {
        provider: provider.id().into(),
        label: provider.name().into(),
        group: "models".into(),
        parts: vec![SetupPartDto {
            id: "key".into(),
            kind: "model_key".into(),
            label: "API key".into(),
            blurb: "The key every person of this installation thinks on. It goes to the \
                    installation's secret store, and nothing reads it back."
                .into(),
            fields: vec![ProviderFieldDto {
                key: "api_key".into(),
                label: "API key".into(),
                hint: "API key".into(),
                kind: "text".into(),
                secret: true,
                default: None,
            }],
            configured: source.is_some(),
            status: None,
            connection_id: None,
            testable: false,
            facts: source
                .into_iter()
                .map(|source| SetupFactDto {
                    label: "Source".into(),
                    value: source.label(provider),
                })
                .collect(),
        }],
    })
}

#[utoipa::path(get, path = "/api/v1/administration/providers", responses(
    (status = 200, body = ProviderSetupPage),
    (status = 401, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody),
))]
/// Every provider the installation sets up, with where each part
/// stands.
pub async fn list_provider_setups(
    State(state): State<ProviderSetupState>,
    _administrator: Administrator,
) -> Result<Json<ProviderSetupPage>, ApiError> {
    let items = PROVIDERS
        .into_iter()
        .map(|provider| describe_key(&state.keys, provider))
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(feature = "native")]
    let items = {
        let mut items = items;
        items.extend(crate::connections::installation_setups(&state.app).await?);
        items
    };
    Ok(Json(ProviderSetupPage { items }))
}

pub async fn configure_provider_key(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(provider): Path<String>,
    Json(request): Json<ConfigureSetupPartRequest>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    let provider = model_provider(&provider)?;
    let key = request
        .fields
        .get("api_key")
        .map(|key| key.trim())
        .filter(|key| !key.is_empty())
        .ok_or_else(|| ApiError::validation("api_key is required"))?;
    state.keys.set(provider, key).map_err(secret_error)?;
    crate::model_lists::key_changed(&state.models, provider);
    crate::model_lists::route_unrouted_aliases(&state).await?;
    describe_key(&state.keys, provider).map(Json)
}

pub async fn remove_provider_key(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(provider): Path<String>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    let provider = model_provider(&provider)?;
    state.keys.remove(provider).map_err(secret_error)?;
    state.models.forget(provider);
    crate::model_lists::route_unrouted_aliases(&state).await?;
    describe_key(&state.keys, provider).map(Json)
}

pub async fn test_provider_key(
    _administrator: Administrator,
    Path(provider): Path<String>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    model_provider(&provider)?;
    Err(ApiError::validation(
        "API key proves itself when it is used, so there is nothing to test here",
    ))
}
