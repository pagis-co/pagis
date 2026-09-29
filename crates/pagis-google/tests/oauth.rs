//! The server-side authorization-code flow, driven against a
//! fake Google token endpoint. No test here reaches Google.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::routing::post;
use pagis_google::{GoogleCapability, GoogleOAuth, ProviderErrorCode, WebClient};

/// One fake token endpoint. It records the form it was posted and
/// answers what the test told it to.
#[derive(Default)]
struct FakeGoogle {
    posted: Mutex<Vec<String>>,
    answer: Mutex<(u16, String)>,
}

async fn token(
    State(google): State<Arc<FakeGoogle>>,
    body: String,
) -> (axum::http::StatusCode, String) {
    google.posted.lock().unwrap().push(body);
    let (status, answer) = google.answer.lock().unwrap().clone();
    (axum::http::StatusCode::from_u16(status).unwrap(), answer)
}

async fn serve(google: Arc<FakeGoogle>) -> SocketAddr {
    let app = Router::new()
        .route("/token", post(token))
        .with_state(google);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn client() -> WebClient {
    WebClient::new("installation-client-id", "installation-client-secret").unwrap()
}

/// An ID token as the token endpoint of Google answers it. The daemon
/// checks no signature, so the signature is a placeholder.
fn id_token(claims: serde_json::Value) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string()),
        URL_SAFE_NO_PAD.encode("not-a-signature"),
    )
}

/// The exchange answers the granted scopes and the ID token of the
/// account that consented, next to the tokens.
#[tokio::test]
async fn the_code_is_traded_for_a_refresh_token_with_the_pkce_verifier() {
    let google = Arc::new(FakeGoogle::default());
    *google.answer.lock().unwrap() = (
        200,
        serde_json::json!({
            "access_token": "ya29.access",
            "refresh_token": "1//refresh",
            "expires_in": 3599,
            "scope": "openid https://www.googleapis.com/auth/userinfo.email \
                      https://www.googleapis.com/auth/gmail.readonly",
            "token_type": "Bearer",
            "id_token": id_token(serde_json::json!({
                "aud": "installation-client-id",
                "email": "alice@example.com",
                "email_verified": true,
            })),
        })
        .to_string(),
    );
    let addr = serve(Arc::clone(&google)).await;
    let oauth = GoogleOAuth::with_endpoints(
        "https://accounts.example.test/authorize",
        &format!("http://{addr}/token"),
    );

    let tokens = oauth
        .exchange_code(
            &client(),
            "https://pagis.example.net/api/v1/connections/google/callback",
            "the-code",
            "the-verifier",
        )
        .await
        .expect("the fake endpoint answers");

    assert_eq!(tokens.access_token, "ya29.access");
    assert_eq!(tokens.refresh_token.as_deref(), Some("1//refresh"));
    assert_eq!(tokens.expires_in, 3599);
    assert_eq!(
        tokens.granted(&[GoogleCapability::GmailRead, GoogleCapability::GmailSend]),
        vec![GoogleCapability::GmailRead]
    );
    assert_eq!(
        tokens.verified_account(&client()).as_deref(),
        Ok("alice@example.com")
    );
    let posted = google.posted.lock().unwrap()[0].clone();
    assert!(posted.contains("grant_type=authorization_code"), "{posted}");
    assert!(posted.contains("code=the-code"), "{posted}");
    assert!(posted.contains("code_verifier=the-verifier"), "{posted}");
    assert!(
        posted.contains("client_secret=installation-client-secret"),
        "{posted}"
    );
}

#[tokio::test]
async fn a_refresh_mints_an_access_token_and_keeps_the_refresh_token() {
    let google = Arc::new(FakeGoogle::default());
    // Google answers a refresh with no refresh token of its own: the
    // daemon keeps the one it holds.
    *google.answer.lock().unwrap() = (
        200,
        r#"{"access_token":"ya29.fresh","expires_in":3599}"#.to_string(),
    );
    let addr = serve(Arc::clone(&google)).await;
    let oauth = GoogleOAuth::with_endpoints(
        "https://accounts.example.test/authorize",
        &format!("http://{addr}/token"),
    );

    let tokens = oauth.refresh(&client(), "1//refresh").await.unwrap();

    assert_eq!(tokens.access_token, "ya29.fresh");
    assert_eq!(tokens.refresh_token, None);
    let posted = google.posted.lock().unwrap()[0].clone();
    assert!(posted.contains("grant_type=refresh_token"), "{posted}");
    assert!(posted.contains("refresh_token=1%2F%2Frefresh"), "{posted}");
}

/// A grant the person withdrew answers 400 `invalid_grant`. Only a new
/// consent repairs it, so the code is `reauth_required` and not a
/// fault the daemon retries around.
#[tokio::test]
async fn a_withdrawn_grant_reads_as_reauth_required() {
    let google = Arc::new(FakeGoogle::default());
    *google.answer.lock().unwrap() = (400, r#"{"error":"invalid_grant"}"#.to_string());
    let addr = serve(Arc::clone(&google)).await;
    let oauth = GoogleOAuth::with_endpoints(
        "https://accounts.example.test/authorize",
        &format!("http://{addr}/token"),
    );

    let error = oauth.refresh(&client(), "1//withdrawn").await.unwrap_err();

    assert_eq!(error.code, ProviderErrorCode::ReauthRequired);
}

#[tokio::test]
async fn google_being_down_is_temporary() {
    let google = Arc::new(FakeGoogle::default());
    *google.answer.lock().unwrap() = (503, "upstream down".to_string());
    let addr = serve(Arc::clone(&google)).await;
    let oauth = GoogleOAuth::with_endpoints(
        "https://accounts.example.test/authorize",
        &format!("http://{addr}/token"),
    );

    let error = oauth.refresh(&client(), "1//refresh").await.unwrap_err();

    assert_eq!(error.code, ProviderErrorCode::TemporarilyUnavailable);
}
