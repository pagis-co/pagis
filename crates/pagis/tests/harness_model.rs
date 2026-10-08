//! The Harness Model Endpoint (ADR-0033).
//!
//! The harness of a Coding Session in a Computer sends its model
//! requests to the daemon with a token of its session. The daemon sends
//! each one to the provider with the Org's key, under the Spend Cap of
//! the session's Workspace, and keeps a Usage Record of the Run that
//! started the session. The provider is a fake at `provider_base_url`.

use std::time::Duration;

use pagis_core::{
    AgentId, ChannelId, CodingSession, CodingSessionState, RunState, SHELL_CAPABILITY, UsagePeriod,
    UsageRecord, WorkspaceId, now_ms,
};
use pagis_server::HarnessModelTokens;
use pagis_testkit::{TestDaemon, TestDaemonOptions, TwoTenants, fixture, test_provider_keys};
use reqwest::StatusCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The Org's keys, which the providers must receive and the harness must
/// never hold.
const ORG_KEY: &str = "sk-ant-org-key";
const OPENAI_KEY: &str = "sk-openai-org-key";
const OPENROUTER_KEY: &str = "sk-or-org-key";

/// A Messages request as Claude Code writes it, with spacing that a
/// decode and an encode would not keep.
const BODY: &str = concat!(
    r#"{"model":"claude-sonnet-4-5", "max_tokens":32000,"#,
    r#""messages":[{"role":"user","content":[{"type":"text","text":"hi","#,
    r#""cache_control":{"type":"ephemeral"}}]}],"stream":true}"#,
);

fn stream_body() -> String {
    [
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",",
        "\"model\":\"claude-sonnet-4-5-20250929\",\"usage\":{\"input_tokens\":25,",
        "\"cache_read_input_tokens\":10,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ]
    .concat()
}

/// A Responses request as Codex writes it, with spacing that a decode and
/// an encode would not keep.
const RESPONSES_BODY: &str = concat!(
    r#"{"model":"gpt-5", "instructions":"You are Codex.","#,
    r#""input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}],"#,
    r#""store":false,"stream":true}"#,
);

fn responses_stream() -> String {
    [
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",",
        "\"model\":\"gpt-5-2025-08-07\",\"status\":\"in_progress\",\"usage\":null}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",",
        "\"output_index\":0,\"content_index\":0,\"delta\":\"Hi\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",",
        "\"model\":\"gpt-5-2025-08-07\",\"status\":\"completed\",\"usage\":{",
        "\"input_tokens\":40,\"input_tokens_details\":{\"cached_tokens\":10},",
        "\"output_tokens\":7,\"output_tokens_details\":{\"reasoning_tokens\":3},",
        "\"total_tokens\":47}}}\n\n",
    ]
    .concat()
}

/// A Chat Completions request as OpenCode writes it for OpenRouter. It
/// does not ask for the usage chunk.
const CHAT_BODY: &str =
    r#"{"model":"openai/gpt-5", "messages":[{"role":"user","content":"hi"}],"stream":true}"#;

fn chat_stream() -> String {
    [
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"openai/gpt-5\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hi\"},",
        "\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"openai/gpt-5\",",
        "\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"openai/gpt-5\",",
        "\"choices\":[],\"usage\":{\"prompt_tokens\":40,\"completion_tokens\":7,",
        "\"prompt_tokens_details\":{\"cached_tokens\":10}}}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat()
}

/// A fake provider at every provider's base URL: an Anthropic stream for
/// `/messages`, a count for `/messages/count_tokens`, a Responses stream
/// for `/responses` and a Chat Completions stream for
/// `/chat/completions`.
async fn provider() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("request-id", "req_1")
                .set_body_raw(stream_body(), "text/event-stream"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/messages/count_tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "input_tokens": 12
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-request-id", "req_openai")
                .set_body_raw(responses_stream(), "text/event-stream"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(chat_stream(), "text/event-stream"))
        .mount(&server)
        .await;
    server
}

/// The model requests that reached the provider. The daemon also asks
/// the same address for its model lists, which are no model requests.
async fn model_requests(provider: &MockServer) -> Vec<wiremock::Request> {
    provider
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| !request.url.path().starts_with("/models"))
        .collect()
}

async fn daemon(provider: &MockServer) -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        keys: test_provider_keys(vec![
            ("ANTHROPIC_API_KEY", ORG_KEY),
            ("OPENAI_API_KEY", OPENAI_KEY),
            ("OPENROUTER_API_KEY", OPENROUTER_KEY),
        ]),
        provider_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await
}

/// A Coding Session of the seeded Agent in `idle`, with the Run that
/// started it, and a token of the endpoint.
async fn running_session(
    daemon: &TestDaemon,
    workspace_id: &WorkspaceId,
) -> (CodingSession, String) {
    let stores = daemon.stores();
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());
    let run = pagis_core::Run {
        state: RunState::Completed,
        ..fixture::queued_run(workspace_id, &agent_id, &channel_id)
    };
    stores.runs.create(&run).await.expect("write the Run");
    let host = stores
        .hosts
        .register(
            workspace_id,
            "Air",
            "macos",
            &[SHELL_CAPABILITY.to_string()],
            now_ms(),
        )
        .await
        .expect("register the Host");
    let session = fixture::coding_session(workspace_id, &agent_id, &run.id, &channel_id, &host.id);
    stores
        .coding_sessions
        .insert(&session)
        .await
        .expect("write the session");
    let token = HarnessModelTokens::new(stores.coding_sessions.clone())
        .mint(workspace_id, &session.id)
        .await
        .expect("mint a token")
        .expect("the Workspace holds the session");
    (session, token)
}

fn url(daemon: &TestDaemon, path: &str) -> String {
    format!("http://{}{path}", daemon.model_addr)
}

/// A request as Claude Code sends it with `ANTHROPIC_AUTH_TOKEN`.
fn harness_request(daemon: &TestDaemon, path: &str, token: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(url(daemon, path))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "interleaved-thinking-2025-05-14")
        .header("x-claude-code-session-id", "harness-own-id")
}

/// A request as Codex or OpenCode sends it to an OpenAI API.
fn openai_request(daemon: &TestDaemon, path: &str, token: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(url(daemon, path))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("originator", "codex_cli_rs")
}

/// The one model request that reached the provider, with its checks of
/// the credential: the Org's key as a bearer, and no token or harness
/// header.
async fn sent_with_bearer(provider: &MockServer, key: &str, token: &str) -> wiremock::Request {
    let mut received = model_requests(provider).await;
    assert_eq!(received.len(), 1);
    let sent = received.remove(0);
    assert_eq!(sent.headers["authorization"], format!("Bearer {key}"));
    assert_eq!(sent.headers["accept"], "text/event-stream");
    assert!(sent.headers.get("x-api-key").is_none());
    assert!(sent.headers.get("originator").is_none());
    assert!(
        sent.headers
            .values()
            .all(|value| !value.to_str().unwrap_or_default().contains(token)),
        "the session token reaches no provider"
    );
    sent
}

fn month() -> UsagePeriod {
    UsagePeriod::calendar_month(now_ms(), "UTC")
}

/// The Usage Records of a Workspace, once the meter wrote at least one.
async fn wait_for_usage(daemon: &TestDaemon, workspace_id: &WorkspaceId) -> pagis_core::UsageTotal {
    wait_for_calls(daemon, workspace_id, 1).await
}

/// The Usage Records of a Workspace, once the meter wrote `calls` of
/// them.
async fn wait_for_calls(
    daemon: &TestDaemon,
    workspace_id: &WorkspaceId,
    calls: i64,
) -> pagis_core::UsageTotal {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let total = daemon
            .stores()
            .usage
            .total_for_workspace(workspace_id, month())
            .await
            .unwrap();
        if total.calls >= calls {
            return total;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the endpoint wrote no Usage Record"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn usage_column(daemon: &TestDaemon, column: &str, workspace_id: &WorkspaceId) -> String {
    daemon
        .rows()
        .text(
            &format!("SELECT {column} FROM usage WHERE workspace_id = ?"),
            &[workspace_id.as_str().into()],
        )
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a Usage Record with a {column}"))
}

async fn error_of(response: reqwest::Response) -> (StatusCode, Option<String>, serde_json::Value) {
    let status = response.status();
    let retry = response
        .headers()
        .get("x-should-retry")
        .map(|value| value.to_str().unwrap().to_string());
    (status, retry, response.json().await.unwrap())
}

/// The Person's monthly Spend Cap, set as an Administrator sets it.
async fn cap(daemon: &TestDaemon, dollars: f64) {
    let capped = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/administration/people/{}/spend-cap",
            daemon.administration_base_url, daemon.user_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "monthly_spend_cap_usd": dollars }))
        .send()
        .await
        .unwrap();
    assert_eq!(capped.status(), StatusCode::OK);
}

/// The Person's monthly Spend Cap at $1, and a spend of $5 this month.
async fn spend_past_the_cap(daemon: &TestDaemon, session: &CodingSession) {
    cap(daemon, 1.0).await;
    daemon
        .stores()
        .usage
        .record(&UsageRecord {
            id: pagis_core::UsageId::generate(),
            workspace_id: daemon.workspace_id.clone(),
            run_id: session.run_id.clone(),
            provider: Some("anthropic".to_string()),
            model: Some("claude-sonnet-4-5".to_string()),
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: Some(5.0),
            created_at: now_ms(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_streamed_request_reaches_the_provider_with_the_org_key_and_is_recorded() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;

    let response = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body(BODY)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/event-stream",
        "the provider's headers come back"
    );
    assert_eq!(response.headers()["request-id"], "req_1");
    assert_eq!(response.text().await.unwrap(), stream_body());

    let received = model_requests(&provider).await;
    assert_eq!(received.len(), 1);
    let sent = &received[0];
    assert_eq!(sent.url.path(), "/messages");
    assert_eq!(sent.body, BODY.as_bytes(), "the body goes unchanged");
    assert_eq!(sent.headers["x-api-key"], ORG_KEY);
    assert_eq!(sent.headers["anthropic-version"], "2023-06-01");
    assert_eq!(
        sent.headers["anthropic-beta"], "interleaved-thinking-2025-05-14",
        "the harness's own beta header, and not the one of the computer tool"
    );
    assert!(sent.headers.get("authorization").is_none());
    assert!(sent.headers.get("x-claude-code-session-id").is_none());
    assert!(
        sent.headers
            .values()
            .all(|value| !value.to_str().unwrap_or_default().contains(&token)),
        "the session token reaches no provider"
    );

    let total = wait_for_usage(&daemon, &daemon.workspace_id).await;
    assert_eq!(total.calls, 1);
    assert_eq!(total.input_tokens, 40);
    assert_eq!(total.output_tokens, 7);
    assert_eq!(total.cache_read_tokens, 10);
    assert_eq!(total.cache_write_tokens, 5);
    // claude-sonnet-4-5: 25 input at $3, 10 cache reads at $0.30, 5 cache
    // writes at $3.75 and 7 output at $15, per million tokens.
    assert!((total.cost_usd - 0.000_201_75).abs() < 1e-12, "{total:?}");
    let workspace = &daemon.workspace_id;
    assert_eq!(
        usage_column(&daemon, "run_id", workspace).await,
        session.run_id.as_str()
    );
    assert_eq!(
        usage_column(&daemon, "provider", workspace).await,
        "anthropic"
    );
    assert_eq!(
        usage_column(&daemon, "model", workspace).await,
        "claude-sonnet-4-5-20250929"
    );
}

#[tokio::test]
async fn a_token_count_passes_and_is_not_recorded() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;
    // A Person at the cap still counts tokens: a count costs nothing.
    spend_past_the_cap(&daemon, &session).await;

    let response = reqwest::Client::new()
        .post(url(&daemon, "/anthropic/v1/messages/count_tokens"))
        .header("x-api-key", &token)
        .header("content-type", "application/json")
        .body(BODY)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({ "input_tokens": 12 })
    );
    let received = model_requests(&provider).await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].url.path(), "/messages/count_tokens");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let total = daemon
        .stores()
        .usage
        .total_for_workspace(&daemon.workspace_id, month())
        .await
        .unwrap();
    assert_eq!(total.calls, 1, "the spend of the cap alone");
}

#[tokio::test]
async fn a_wrong_token_and_the_token_of_a_closed_session_are_refused() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;

    let wrong = harness_request(&daemon, "/anthropic/v1/messages", "not-a-token")
        .body(BODY)
        .send()
        .await
        .unwrap();
    let (status, retry, body) = error_of(wrong).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");

    let closed = CodingSession {
        state: CodingSessionState::Closed,
        end_reason: Some("closed_by_agent".to_string()),
        ended_at: Some(now_ms()),
        ..session
    };
    assert!(
        daemon
            .stores()
            .coding_sessions
            .update(&closed)
            .await
            .unwrap()
    );
    let after_close = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body(BODY)
        .send()
        .await
        .unwrap();
    let (status, _, body) = error_of(after_close).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["type"], "authentication_error");

    assert!(model_requests(&provider).await.is_empty());
}

#[tokio::test]
async fn a_person_at_the_spend_cap_is_refused_before_the_provider() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;
    spend_past_the_cap(&daemon, &session).await;

    let response = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body(BODY)
        .send()
        .await
        .unwrap();

    let (status, retry, body) = error_of(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(body["error"]["type"], "permission_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("spend cap"), "{message}");
    assert!(!message.contains("sprite"), "{message}");
    assert!(model_requests(&provider).await.is_empty());
}

#[tokio::test]
async fn an_unpriced_model_under_a_cap_is_refused_before_the_provider() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (_, token) = running_session(&daemon, &daemon.workspace_id).await;
    cap(&daemon, 10.0).await;

    let response = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body(r#"{"model":"unlisted-model-1","max_tokens":10,"messages":[]}"#)
        .send()
        .await
        .unwrap();

    let (status, retry, body) = error_of(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(body["error"]["type"], "permission_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("anthropic/unlisted-model-1"), "{message}");
    assert!(message.contains("no known price"), "{message}");
    assert!(model_requests(&provider).await.is_empty());
}

#[tokio::test]
async fn an_installation_with_no_anthropic_key_refuses() {
    let provider = provider().await;
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        provider_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;
    let (_, token) = running_session(&daemon, &daemon.workspace_id).await;

    let response = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body(BODY)
        .send()
        .await
        .unwrap();

    let (status, retry, body) = error_of(response).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(body["error"]["type"], "permission_error");
    assert!(model_requests(&provider).await.is_empty());
}

#[tokio::test]
async fn the_endpoint_refuses_what_it_does_not_serve_in_the_error_shape() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (_, token) = running_session(&daemon, &daemon.workspace_id).await;
    let client = reqwest::Client::new();

    let other_path = client
        .post(url(&daemon, "/anthropic/v1/complete"))
        .header("x-api-key", &token)
        .send()
        .await
        .unwrap();
    let (status, retry, body) = error_of(other_path).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(body["error"]["type"], "not_found_error");

    let other_method = client
        .get(url(&daemon, "/anthropic/v1/messages"))
        .header("x-api-key", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(other_method.status(), StatusCode::NOT_FOUND);

    let not_json = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body("model=claude")
        .send()
        .await
        .unwrap();
    let (status, _, body) = error_of(not_json).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "invalid_request_error");

    let mut large = String::from(r#"{"model":"claude-sonnet-4-5","pad":""#);
    large.push_str(&"a".repeat(pagis_server::harness_model::MAX_REQUEST_BYTES));
    large.push_str(r#""}"#);
    let too_large = harness_request(&daemon, "/anthropic/v1/messages", &token)
        .body(large)
        .send()
        .await
        .unwrap();
    let (status, _, body) = error_of(too_large).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"]["type"], "request_too_large");

    assert!(model_requests(&provider).await.is_empty());
}

#[tokio::test]
async fn the_spend_of_a_session_lands_in_its_own_workspace() {
    let provider = provider().await;
    let tenants = TwoTenants::on(daemon(&provider).await).await;
    let daemon = &tenants.daemon;
    let (session, token) = running_session(daemon, &tenants.a.workspace_id).await;

    let anthropic = harness_request(daemon, "/anthropic/v1/messages", &token)
        .body(BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(anthropic.status(), StatusCode::OK);
    anthropic.text().await.unwrap();
    let openai = openai_request(daemon, "/openai/v1/responses", &token)
        .body(RESPONSES_BODY)
        .send()
        .await
        .unwrap();
    assert_eq!(openai.status(), StatusCode::OK);
    openai.text().await.unwrap();

    let a_total = wait_for_calls(daemon, &tenants.a.workspace_id, 2).await;
    assert_eq!(a_total.calls, 2);
    let of_the_run = daemon
        .rows()
        .count(
            "SELECT COUNT(*) FROM usage WHERE workspace_id = ? AND run_id = ?",
            &[
                tenants.a.workspace_id.as_str().into(),
                session.run_id.as_str().into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(of_the_run, 2, "both records are of the Run of the session");
    let b_usage: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/usage", daemon.base_url))
        .header("cookie", &tenants.b.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b_usage["total"]["calls"], 0, "{b_usage}");
    assert_eq!(b_usage["total"]["cost_usd"], 0.0, "{b_usage}");
    let b_total = daemon
        .stores()
        .usage
        .total_for_workspace(&tenants.b.workspace_id, month())
        .await
        .unwrap();
    assert_eq!(b_total, pagis_core::UsageTotal::default());
}

#[tokio::test]
async fn a_streamed_responses_call_passes_byte_for_byte_and_is_recorded() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;

    let response = openai_request(&daemon, "/openai/v1/responses", &token)
        .body(RESPONSES_BODY)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-request-id"], "req_openai");
    assert_eq!(response.text().await.unwrap(), responses_stream());
    let sent = sent_with_bearer(&provider, OPENAI_KEY, &token).await;
    assert_eq!(sent.url.path(), "/responses");
    assert_eq!(
        sent.body,
        RESPONSES_BODY.as_bytes(),
        "the body goes unchanged"
    );

    let total = wait_for_usage(&daemon, &daemon.workspace_id).await;
    assert_eq!(total.calls, 1);
    assert_eq!(total.input_tokens, 40);
    assert_eq!(total.output_tokens, 7);
    assert_eq!(total.cache_read_tokens, 10);
    assert_eq!(total.cache_write_tokens, 0);
    // gpt-5: 30 input at $1.25, 10 cache reads at $0.125 and 7 output at
    // $10, per million tokens.
    assert!((total.cost_usd - 0.000_108_75).abs() < 1e-12, "{total:?}");
    let workspace = &daemon.workspace_id;
    assert_eq!(
        usage_column(&daemon, "run_id", workspace).await,
        session.run_id.as_str()
    );
    assert_eq!(usage_column(&daemon, "provider", workspace).await, "openai");
    assert_eq!(
        usage_column(&daemon, "model", workspace).await,
        "gpt-5-2025-08-07"
    );
}

#[tokio::test]
async fn a_streamed_chat_completions_call_asks_for_its_usage_and_is_recorded() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;

    let response = openai_request(&daemon, "/openrouter/v1/chat/completions", &token)
        .body(CHAT_BODY)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), chat_stream());
    let sent = sent_with_bearer(&provider, OPENROUTER_KEY, &token).await;
    assert_eq!(sent.url.path(), "/chat/completions");
    let mut asked: serde_json::Value = serde_json::from_str(CHAT_BODY).unwrap();
    asked["stream_options"] = serde_json::json!({ "include_usage": true });
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&sent.body).unwrap(),
        asked,
        "the body asks for the usage chunk and changes no other field"
    );

    let total = wait_for_usage(&daemon, &daemon.workspace_id).await;
    assert_eq!(total.calls, 1);
    assert_eq!(total.input_tokens, 40);
    assert_eq!(total.output_tokens, 7);
    assert_eq!(total.cache_read_tokens, 10);
    // openai/gpt-5 has the price of gpt-5.
    assert!((total.cost_usd - 0.000_108_75).abs() < 1e-12, "{total:?}");
    let workspace = &daemon.workspace_id;
    assert_eq!(
        usage_column(&daemon, "run_id", workspace).await,
        session.run_id.as_str()
    );
    assert_eq!(
        usage_column(&daemon, "provider", workspace).await,
        "openrouter"
    );
    assert_eq!(
        usage_column(&daemon, "model", workspace).await,
        "openai/gpt-5"
    );
}

#[tokio::test]
async fn the_openai_routes_refuse_in_the_openai_error_shape() {
    let provider = provider().await;
    let daemon = daemon(&provider).await;
    let (session, token) = running_session(&daemon, &daemon.workspace_id).await;

    let wrong = openai_request(&daemon, "/openai/v1/responses", "not-a-token")
        .body(RESPONSES_BODY)
        .send()
        .await
        .unwrap();
    let (status, retry, body) = error_of(wrong).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(retry.as_deref(), Some("false"));
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(body["error"]["code"], "invalid_api_key");
    assert!(body["error"]["message"].is_string(), "{body}");

    let models = reqwest::Client::new()
        .get(url(&daemon, "/openai/v1/models"))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let (status, _, body) = error_of(models).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], "invalid_request_error");

    spend_past_the_cap(&daemon, &session).await;
    for path in ["/openai/v1/responses", "/openrouter/v1/chat/completions"] {
        let capped = openai_request(&daemon, path, &token)
            .body(CHAT_BODY)
            .send()
            .await
            .unwrap();
        let (status, retry, body) = error_of(capped).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(retry.as_deref(), Some("false"), "{path}");
        assert_eq!(body["error"]["type"], "insufficient_quota", "{path}");
        assert_eq!(body["error"]["code"], "insufficient_quota", "{path}");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("spend cap"), "{path}: {message}");
        assert!(body.get("type").is_none(), "{path}: {body}");
    }

    assert!(model_requests(&provider).await.is_empty());
}
