//! The Google side of a test daemon (ADR-0012).
//!
//! A Google Connection reaches Google with an access token that the
//! daemon mints from the refresh token it holds sealed. A test daemon
//! therefore needs the Installation OAuth Client, a sealed refresh
//! token on the row, and a token endpoint that answers the refresh. No
//! test reaches Google: [`FakeTokenEndpoint`] answers on loopback.

use std::sync::Arc;

use axum::Router;
use axum::routing::post;
use pagis_core::{Connection, SecretStore, TenantKeys};

/// The client id of the Installation OAuth Client that
/// [`crate::TestDaemon::plant_google_connection`] registers.
pub const CLIENT_ID: &str = "test-installation.apps.googleusercontent.com";

/// The refresh token that a planted Connection holds.
pub const REFRESH_TOKEN: &str = "1//test-refresh-token";

/// The access token that the fake endpoint mints for each refresh.
pub const ACCESS_TOKEN: &str = "ya29.test-access-token";

/// A Google token endpoint on loopback that answers every refresh with
/// [`ACCESS_TOKEN`]. A test that drives the code exchange names its own
/// endpoint through `TestDaemonOptions::google_oauth`.
pub struct FakeTokenEndpoint {
    pub oauth: Arc<pagis_google::GoogleOAuth>,
}

impl FakeTokenEndpoint {
    pub async fn start() -> Self {
        let app = Router::new().route(
            "/token",
            post(|| async {
                axum::Json(serde_json::json!({
                    "access_token": ACCESS_TOKEN,
                    "expires_in": 3599,
                    "token_type": "Bearer",
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind the fake Google token endpoint");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self {
            oauth: Arc::new(pagis_google::GoogleOAuth::with_endpoints(
                "https://accounts.example.test/authorize",
                &format!("http://{addr}/token"),
            )),
        }
    }
}

/// Write `connection` as the connect flow leaves it: the Org holds the
/// Installation OAuth Client, and the row holds [`REFRESH_TOKEN`] sealed
/// with the Tenant Data Key of its Workspace.
pub(crate) async fn plant(
    stores: &pagis_core::Stores,
    secrets: &Arc<dyn SecretStore>,
    connection: &Connection,
) {
    let client = pagis_connect::OrgWebClient::new(stores.orgs.clone(), Arc::clone(secrets));
    if client
        .registered_client_id()
        .await
        .expect("read the Installation OAuth Client")
        .is_none()
    {
        client
            .register(CLIENT_ID, "GOCSPX-test-installation")
            .await
            .expect("register the Installation OAuth Client");
    }
    stores
        .connections
        .create(connection)
        .await
        .expect("write the Google Connection");
    let sealed = TenantKeys::new(Arc::clone(secrets))
        .of(&connection.workspace_id)
        .and_then(|key| key.seal(REFRESH_TOKEN))
        .expect("seal the refresh token");
    assert!(
        stores
            .connections
            .set_refresh_token(&connection.workspace_id, &connection.id, Some(&sealed))
            .await
            .expect("write the refresh token"),
        "the planted Connection is gone"
    );
}
