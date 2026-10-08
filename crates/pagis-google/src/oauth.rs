//! The server-side authorization-code flow for Google.
//!
//! `gog` runs the loopback flow of RFC 8252 on the machine it is
//! started on. A server has no browser and the person is somewhere
//! else, so the daemon runs the exchange itself against one Web OAuth
//! client the Org holds: it sends the person to Google, Google redirects
//! the browser to the daemon's own `public_origin`, and the daemon
//! trades the code for tokens.
//!
//! `gog` still makes every API call. It takes a caller-supplied access
//! token through `GOG_ACCESS_TOKEN`, which bypasses its own token store
//! (`gog --help`: "Use provided access token directly (bypasses stored
//! refresh tokens)"), so the daemon owns the refresh token and the
//! refresh, and `gog` keeps nothing.
//!
//! Nothing here writes a token down. The caller seals what it keeps
//! with the Tenant Data Key of the person it belongs to.
//!
//! Each authorization also asks for `openid email`, so the answer of the
//! exchange names the Google account that consented. The caller keeps a
//! token only for the account of its Connection, and records only the
//! capabilities that it asked for and Google granted.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::{GoogleCapability, ProviderError, ProviderErrorCode};

/// Where Google sends the person, and where the daemon trades the code.
pub const GOOGLE_AUTHORIZE_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// The one Web OAuth client of an installation. The
/// administrator registers it once; every person consents against it.
///
/// The type carries no `Debug`, so the secret never reaches a log line.
#[derive(Clone, PartialEq, Eq)]
pub struct WebClient {
    client_id: String,
    client_secret: String,
}

impl WebClient {
    /// A Web client from the two values the Google console shows.
    pub fn new(client_id: &str, client_secret: &str) -> Result<Self, crate::AdapterError> {
        let client_id = client_id.trim().to_string();
        let client_secret = client_secret.trim().to_string();
        let unusable = |value: &str| {
            value.is_empty() || value.len() > 512 || value.contains(|c: char| c.is_control())
        };
        if unusable(&client_id) || unusable(&client_secret) {
            return Err(crate::AdapterError::InvalidConfiguration(
                "web_oauth_client",
            ));
        }
        Ok(Self {
            client_id,
            client_secret,
        })
    }

    /// The client id. It is not a secret: it rides in the address bar
    /// of the consent screen, and the settings page shows it back.
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

/// One PKCE pair (RFC 7636). The verifier stays with the daemon and the
/// challenge goes to Google, so a code taken out of the redirect is
/// worth nothing without the daemon's half.
pub struct Pkce {
    verifier: String,
    challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce").finish_non_exhaustive()
    }
}

impl Pkce {
    pub fn generate() -> Self {
        let verifier = random_token();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

/// A 256-bit URL-safe random value: a PKCE verifier, or the `state`
/// that names one pending authorization.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// What Google answers a token call with. The refresh token is absent
/// when Google decides the person already granted this client offline
/// access, which is why the authorization asks for consent every time.
pub struct OauthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds the access token stays usable, as Google reports it.
    pub expires_in: u64,
    /// The scopes Google granted, from the `scope` value of the answer.
    /// A person can clear a box on the consent screen, and
    /// `include_granted_scopes=true` adds what the account granted this
    /// client before, so this list is not the list the daemon asked for.
    pub granted_scopes: Vec<String>,
    /// The OpenID Connect ID token of a code exchange. It names the
    /// person, so the `Debug` form leaves it out.
    pub id_token: Option<String>,
}

impl fmt::Debug for OauthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OauthTokens")
            .field("expires_in", &self.expires_in)
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("granted_scopes", &self.granted_scopes)
            .field("has_id_token", &self.id_token.is_some())
            .finish()
    }
}

impl OauthTokens {
    /// The capabilities of `requested` whose scope Google granted, in
    /// the order of `requested`. A granted scope that nobody requested
    /// adds no capability.
    pub fn granted(&self, requested: &[GoogleCapability]) -> Vec<GoogleCapability> {
        requested
            .iter()
            .copied()
            .filter(|capability| {
                let scope = capability_scope(*capability);
                self.granted_scopes.iter().any(|granted| granted == scope)
            })
            .collect()
    }

    /// The Google account that consented, from the ID token of a code
    /// exchange.
    ///
    /// The daemon takes the ID token directly from the token endpoint
    /// over TLS, so it checks the claims and not a signature (OpenID
    /// Connect Core 1.0, section 3.1.3.7). The audience must be this
    /// client, and Google must have verified the address. The caller
    /// compares the address with the account of its Connection.
    pub fn verified_account(&self, client: &WebClient) -> Result<String, IdentityError> {
        let token = self.id_token.as_deref().ok_or(IdentityError::Missing)?;
        let mut segments = token.split('.');
        let (Some(_header), Some(payload), Some(_signature), None) = (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ) else {
            return Err(IdentityError::Unreadable);
        };
        let claims: IdTokenClaims = URL_SAFE_NO_PAD
            .decode(payload)
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .ok_or(IdentityError::Unreadable)?;
        if !claims.aud.names(client.client_id()) {
            return Err(IdentityError::OtherAudience);
        }
        match claims.email {
            Some(email) if claims.email_verified && !email.trim().is_empty() => Ok(email),
            _ => Err(IdentityError::Unverified),
        }
    }
}

/// Why the answer of an exchange proves no Google account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("Google returned no ID token")]
    Missing,
    #[error("the ID token is not a readable JWT")]
    Unreadable,
    #[error("the ID token is for another OAuth client")]
    OtherAudience,
    #[error("the ID token names no address that Google verified")]
    Unverified,
}

/// The claims of an ID token that the daemon reads.
#[derive(serde::Deserialize)]
struct IdTokenClaims {
    aud: Audience,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
}

/// The `aud` claim: one client id, or a list of them (OpenID Connect
/// Core 1.0, section 2).
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn names(&self, client_id: &str) -> bool {
        match self {
            Audience::One(audience) => audience == client_id,
            Audience::Many(audiences) => audiences.iter().any(|audience| audience == client_id),
        }
    }
}

/// The scopes every authorization asks for next to the capability
/// scopes. They make the exchange answer an ID token that names the
/// Google account that consented.
pub const IDENTITY_SCOPES: [&str; 2] = ["openid", "email"];

/// The Google scope of one capability. `gog` names the same rights
/// through `--services` and `--gmail-scope`; the web flow asks Google for
/// them directly, because there is no `gog` in it.
fn capability_scope(capability: GoogleCapability) -> &'static str {
    use GoogleCapability::*;

    match capability {
        GmailRead => "https://www.googleapis.com/auth/gmail.readonly",
        GmailSend => "https://www.googleapis.com/auth/gmail.send",
        GmailModify => "https://www.googleapis.com/auth/gmail.modify",
        CalendarRead => "https://www.googleapis.com/auth/calendar.readonly",
        CalendarWrite => "https://www.googleapis.com/auth/calendar",
    }
}

/// The Google scopes one capability set consents to, each one time.
pub fn oauth_scopes(capabilities: impl IntoIterator<Item = GoogleCapability>) -> Vec<&'static str> {
    let mut scopes: Vec<&'static str> = Vec::new();
    for scope in capabilities.into_iter().map(capability_scope) {
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes
}

/// The redirect URI this installation registers with Google, derived
/// from the origin a browser reaches it at. Google matches it
/// character for character against the console entry, so it is built in
/// one place and shown to the administrator to paste.
pub fn redirect_uri(public_origin: &str) -> String {
    format!("{}{}", public_origin.trim_end_matches('/'), CALLBACK_PATH)
}

/// The path Google redirects the browser to. It is public: the browser
/// arrives from Google with no Session cookie.
pub const CALLBACK_PATH: &str = "/api/v1/connections/google/callback";

/// The daemon's half of the authorization-code flow.
///
/// The endpoints are injected so a test drives the exchange against a
/// fake token endpoint and never against Google.
pub struct GoogleOAuth {
    http: reqwest::Client,
    authorize_endpoint: String,
    token_endpoint: String,
}

impl Default for GoogleOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl GoogleOAuth {
    pub fn new() -> Self {
        Self::with_endpoints(GOOGLE_AUTHORIZE_ENDPOINT, GOOGLE_TOKEN_ENDPOINT)
    }

    pub fn with_endpoints(authorize_endpoint: &str, token_endpoint: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            authorize_endpoint: authorize_endpoint.to_string(),
            token_endpoint: token_endpoint.to_string(),
        }
    }

    /// The address the person opens to consent.
    ///
    /// `access_type=offline` with `prompt=consent` is what makes Google
    /// return a refresh token: without the prompt Google returns one
    /// only the first time a person grants this client, and the second
    /// person of an installation would reach `connected` with no way to
    /// refresh. The scope starts with [`IDENTITY_SCOPES`], so the answer
    /// names the account that consented.
    ///
    /// `account` is the account of the Connection, when it has one. With
    /// none, the prompt adds `select_account`, and the person picks the
    /// account in Google's account chooser.
    pub fn authorization_url(
        &self,
        client: &WebClient,
        redirect_uri: &str,
        account: Option<&str>,
        scopes: &[&str],
        state: &str,
        pkce: &Pkce,
    ) -> String {
        let scope = IDENTITY_SCOPES
            .iter()
            .chain(scopes)
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let mut pairs = vec![
            ("client_id", client.client_id()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", &scope),
            ("access_type", "offline"),
            (
                "prompt",
                match account {
                    Some(_) => "consent",
                    None => "select_account consent",
                },
            ),
            ("include_granted_scopes", "true"),
        ];
        if let Some(account) = account {
            pairs.push(("login_hint", account));
        }
        pairs.extend([
            ("state", state),
            ("code_challenge", pkce.challenge()),
            ("code_challenge_method", "S256"),
        ]);
        format!("{}?{}", self.authorize_endpoint, form_encode(&pairs))
    }

    /// Trade the code the redirect carried for tokens.
    pub async fn exchange_code(
        &self,
        client: &WebClient,
        redirect_uri: &str,
        code: &str,
        verifier: &str,
    ) -> Result<OauthTokens, ProviderError> {
        self.token_call(&[
            ("client_id", client.client_id()),
            ("client_secret", client.client_secret()),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
        ])
        .await
    }

    /// Mint a fresh access token from the refresh token the daemon
    /// holds. A refusal here is `reauth_required`: the person revoked
    /// the grant or Google expired the token, and only a new consent
    /// repairs it.
    pub async fn refresh(
        &self,
        client: &WebClient,
        refresh_token: &str,
    ) -> Result<OauthTokens, ProviderError> {
        self.token_call(&[
            ("client_id", client.client_id()),
            ("client_secret", client.client_secret()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .await
    }

    async fn token_call(&self, form: &[(&str, &str)]) -> Result<OauthTokens, ProviderError> {
        let body = form_encode(form);
        let response = self
            .http
            .post(&self.token_endpoint)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|_| {
                provider_error(
                    ProviderErrorCode::TemporarilyUnavailable,
                    "google_unreachable",
                )
            })?;
        let status = response.status();
        let text = response.text().await.map_err(|_| {
            provider_error(
                ProviderErrorCode::TemporarilyUnavailable,
                "google_unreachable",
            )
        })?;
        if !status.is_success() {
            // Google answers a dead or withdrawn grant with 400 and
            // `invalid_grant`. Every 4xx here is the person's grant, not
            // a fault the daemon can retry around.
            let code = if status.is_client_error() {
                ProviderErrorCode::ReauthRequired
            } else {
                ProviderErrorCode::TemporarilyUnavailable
            };
            return Err(provider_error(code, "google_refused_the_token_call"));
        }
        let parsed: TokenResponse = serde_json::from_str(&text).map_err(|_| {
            provider_error(
                ProviderErrorCode::TemporarilyUnavailable,
                "google_token_response_unreadable",
            )
        })?;
        if parsed.access_token.trim().is_empty() {
            return Err(provider_error(
                ProviderErrorCode::TemporarilyUnavailable,
                "google_returned_no_access_token",
            ));
        }
        Ok(OauthTokens {
            access_token: parsed.access_token,
            refresh_token: parsed
                .refresh_token
                .filter(|token| !token.trim().is_empty()),
            expires_in: parsed.expires_in.unwrap_or(3600),
            // An answer without `scope` grants nothing the daemon can
            // record: the daemon does not assume the scopes it asked for.
            granted_scopes: parsed
                .scope
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            id_token: parsed.id_token.filter(|token| !token.trim().is_empty()),
        })
    }
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    id_token: Option<String>,
}

fn provider_error(code: ProviderErrorCode, detail: &'static str) -> ProviderError {
    ProviderError {
        code,
        retryable: false,
        detail,
    }
}

/// `application/x-www-form-urlencoded`, which is also the query string
/// of the authorization address.
fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode everything outside the unreserved set of RFC 3986.
/// A space becomes `%20`, which every OAuth server reads, and not `+`,
/// which only some do.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> WebClient {
        WebClient::new("client-id.apps.googleusercontent.com", "client-secret").unwrap()
    }

    #[test]
    fn the_authorization_url_carries_the_state_the_pkce_challenge_and_offline_access() {
        let pkce = Pkce::generate();
        let url = GoogleOAuth::new().authorization_url(
            &client(),
            "https://pagis.example.net/api/v1/connections/google/callback",
            Some("alice@example.com"),
            &["https://www.googleapis.com/auth/gmail.readonly"],
            "state-value",
            &pkce,
        );

        assert!(url.starts_with(GOOGLE_AUTHORIZE_ENDPOINT), "{url}");
        assert!(url.contains("state=state-value"), "{url}");
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(
            url.contains(&format!(
                "code_challenge={}",
                percent_encode(pkce.challenge())
            )),
            "{url}"
        );
        assert!(url.contains("access_type=offline"), "{url}");
        assert!(url.contains("prompt=consent&"), "{url}");
        assert!(url.contains("login_hint=alice%40example.com"), "{url}");
        // `openid email` comes first, so the answer names the account.
        assert!(
            url.contains(
                "scope=openid%20email%20https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fgmail.readonly&"
            ),
            "{url}"
        );
        assert!(
            url.contains(
                "redirect_uri=https%3A%2F%2Fpagis.example.net%2Fapi%2Fv1%2Fconnections%2Fgoogle%2Fcallback"
            ),
            "{url}"
        );
        // The verifier is the daemon's half and never leaves it.
        assert!(!url.contains(pkce.verifier()), "{url}");
    }

    /// The first authorization of a Connection names no account: the
    /// person picks one in Google's account chooser, which Google shows
    /// even to a browser with one Google session.
    #[test]
    fn with_no_account_the_person_picks_one_at_google() {
        let url = GoogleOAuth::new().authorization_url(
            &client(),
            "https://pagis.example.net/api/v1/connections/google/callback",
            None,
            &["https://www.googleapis.com/auth/gmail.readonly"],
            "state-value",
            &Pkce::generate(),
        );

        assert!(url.contains("prompt=select_account%20consent&"), "{url}");
        assert!(!url.contains("login_hint"), "{url}");
        assert!(url.contains("access_type=offline"), "{url}");
    }

    /// The challenge is the SHA-256 of the verifier, URL-safe and
    /// unpadded, which is what `S256` means.
    #[test]
    fn the_pkce_challenge_hashes_its_verifier() {
        let pkce = Pkce::generate();

        assert_eq!(
            pkce.challenge(),
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier().as_bytes()))
        );
        assert_ne!(Pkce::generate().verifier(), pkce.verifier());
    }

    #[test]
    fn a_capability_set_becomes_the_google_scopes_that_cover_it() {
        use GoogleCapability::*;

        assert_eq!(
            oauth_scopes([GmailRead, CalendarRead]),
            vec![
                "https://www.googleapis.com/auth/gmail.readonly",
                "https://www.googleapis.com/auth/calendar.readonly",
            ]
        );
        assert_eq!(
            oauth_scopes([GmailModify, CalendarWrite]),
            vec![
                "https://www.googleapis.com/auth/gmail.modify",
                "https://www.googleapis.com/auth/calendar",
            ]
        );
        assert!(oauth_scopes([]).is_empty());
    }

    #[test]
    fn the_redirect_uri_hangs_off_the_public_origin_once() {
        assert_eq!(
            redirect_uri("https://pagis.example.net/"),
            "https://pagis.example.net/api/v1/connections/google/callback"
        );
        assert_eq!(
            redirect_uri("http://127.0.0.1:7777"),
            "http://127.0.0.1:7777/api/v1/connections/google/callback"
        );
    }

    /// An answer of the token endpoint with this ID token and this
    /// `scope` value.
    fn answer(id_token: Option<String>, scope: &str) -> OauthTokens {
        OauthTokens {
            access_token: "ya29.access".to_string(),
            refresh_token: Some("1//refresh".to_string()),
            expires_in: 3599,
            granted_scopes: scope.split_whitespace().map(str::to_string).collect(),
            id_token,
        }
    }

    /// An ID token with these claims. The daemon checks no signature, so
    /// the signature is a placeholder.
    fn id_token(claims: serde_json::Value) -> Option<String> {
        let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
        Some(format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string()),
            URL_SAFE_NO_PAD.encode("not-a-signature"),
        ))
    }

    /// A capability counts only when the Connection asked for it and its
    /// scope is in the `scope` value of the answer. A scope that the
    /// account granted earlier, which `include_granted_scopes=true`
    /// returns, does not count.
    #[test]
    fn a_capability_is_granted_when_it_was_requested_and_its_scope_came_back() {
        use GoogleCapability::*;

        let tokens = answer(
            None,
            "openid https://www.googleapis.com/auth/userinfo.email \
             https://www.googleapis.com/auth/gmail.readonly \
             https://www.googleapis.com/auth/gmail.send",
        );

        assert_eq!(tokens.granted(&[GmailRead, CalendarRead]), vec![GmailRead]);
        assert_eq!(tokens.granted(&[GmailModify]), Vec::new());
        assert_eq!(answer(None, "").granted(&[GmailRead]), Vec::new());
    }

    /// The ID token names the Google account that consented, when its
    /// audience is this client and Google verified its address.
    #[test]
    fn the_id_token_names_the_verified_account_for_this_client() {
        let tokens = answer(
            id_token(serde_json::json!({
                "iss": "https://accounts.google.com",
                "aud": "client-id.apps.googleusercontent.com",
                "email": "Alice@Example.com",
                "email_verified": true,
            })),
            "openid",
        );

        assert_eq!(
            tokens.verified_account(&client()).as_deref(),
            Ok("Alice@Example.com")
        );
    }

    /// OpenID Connect lets `aud` be a list. The client must be in it.
    #[test]
    fn an_audience_list_must_name_this_client() {
        let listed = |audience: serde_json::Value| {
            answer(
                id_token(serde_json::json!({
                    "aud": audience,
                    "email": "alice@example.com",
                    "email_verified": true,
                })),
                "openid",
            )
            .verified_account(&client())
        };

        assert!(listed(serde_json::json!(["client-id.apps.googleusercontent.com"])).is_ok());
        assert_eq!(
            listed(serde_json::json!(["another-client"])),
            Err(IdentityError::OtherAudience)
        );
    }

    #[test]
    fn an_id_token_for_another_client_proves_no_account() {
        let tokens = answer(
            id_token(serde_json::json!({
                "aud": "another-client.apps.googleusercontent.com",
                "email": "alice@example.com",
                "email_verified": true,
            })),
            "openid",
        );

        assert_eq!(
            tokens.verified_account(&client()),
            Err(IdentityError::OtherAudience)
        );
    }

    #[test]
    fn an_address_google_did_not_verify_proves_no_account() {
        for claims in [
            serde_json::json!({
                "aud": "client-id.apps.googleusercontent.com",
                "email": "alice@example.com",
                "email_verified": false,
            }),
            serde_json::json!({
                "aud": "client-id.apps.googleusercontent.com",
                "email": "alice@example.com",
            }),
            serde_json::json!({
                "aud": "client-id.apps.googleusercontent.com",
                "email_verified": true,
            }),
        ] {
            assert_eq!(
                answer(id_token(claims.clone()), "openid").verified_account(&client()),
                Err(IdentityError::Unverified),
                "{claims}"
            );
        }
    }

    #[test]
    fn a_missing_or_unreadable_id_token_proves_no_account() {
        assert_eq!(
            answer(None, "openid").verified_account(&client()),
            Err(IdentityError::Missing)
        );
        for token in [
            "not-a-jwt",
            "a.b",
            "a.%%%.c",
            "a.b.c.d",
            &format!("a.{}.c", URL_SAFE_NO_PAD.encode("not json")),
        ] {
            assert_eq!(
                answer(Some(token.to_string()), "openid").verified_account(&client()),
                Err(IdentityError::Unreadable),
                "{token}"
            );
        }
    }

    /// The Debug form of an answer shows no token and no claim of the ID
    /// token, which names the person.
    #[test]
    fn the_debug_form_of_an_answer_shows_no_token() {
        let tokens = answer(
            id_token(serde_json::json!({ "email": "alice@example.com" })),
            "openid",
        );

        let shown = format!("{tokens:?}");

        assert!(!shown.contains("ya29"), "{shown}");
        assert!(!shown.contains("1//refresh"), "{shown}");
        assert!(
            !shown.contains(tokens.id_token.as_deref().unwrap()),
            "{shown}"
        );
    }

    /// The client secret has no accessor outside this module and no
    /// `Debug`, so it cannot reach a log line by accident.
    #[test]
    fn a_web_client_needs_both_values() {
        assert!(WebClient::new("  ", "secret").is_err());
        assert!(WebClient::new("id", "").is_err());
        assert_eq!(
            WebClient::new(" id ", " secret ").unwrap().client_id(),
            "id"
        );
    }
}
