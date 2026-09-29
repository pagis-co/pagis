//! Full-daemon credential-grant tests (ADR-0013): the credential
//! approval card writes a domain rule on "Always allow", a later action
//! on that domain needs no card, a domain outside the rules still does,
//! the cap is enforced instead of silently trimmed, and revoking
//! returns the agent to per-action.
//!
//! These tests drive the request row that the `vault__*` tools write,
//! so the grant machinery is covered without a tool call.

use pagis_broker::{CredentialAction, CredentialActionKind, domain_allowed};
use pagis_core::{
    AgentId, AgentStore, Grant, GrantStore, Request, RequestId, RequestState, RequestStore,
    WorkspaceId, now_ms,
};
use pagis_storage_sqlite::{SqliteGrantStore, SqliteRequestStore};
use pagis_testkit::TestDaemon;

fn action(domain: &str) -> CredentialAction {
    CredentialAction {
        action: CredentialActionKind::Fill,
        domain: domain.to_string(),
        username: "alice@example.com".to_string(),
        login_url: format!("https://{domain}/login"),
    }
}

/// Write the request row a vault action parks on, exactly as the
/// daemon writes it: every field derived from the Credential record.
async fn pending_credential_request(daemon: &TestDaemon, domain: &str) -> String {
    let requests = SqliteRequestStore::new(daemon.pool().clone());
    let request = Request {
        id: RequestId::generate(),
        workspace_id: workspace_id(daemon).await,
        agent_id: AgentId::from(daemon.agent_id.clone()),
        run_id: None,
        kind: Request::CREDENTIAL_ACTION_KIND.to_string(),
        payload: action(domain).request_payload(),
        state: RequestState::Pending,
        values: None,
        decided_at: None,
        created_at: now_ms(),
    };
    requests.create(&request).await.unwrap();
    request.id.to_string()
}

async fn workspace_id(daemon: &TestDaemon) -> WorkspaceId {
    let agents = pagis_storage_sqlite::SqliteAgentStore::new(daemon.pool().clone());
    agents
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .workspace_id
}

async fn decide(
    daemon: &TestDaemon,
    request_id: &str,
    decision: &str,
    scope: Option<&str>,
) -> reqwest::Response {
    let mut body = serde_json::json!({ "decision": decision });
    if let Some(scope) = scope {
        body["scope"] = serde_json::json!(scope);
    }
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{request_id}/decision",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn list_grants(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    reqwest::Client::new()
        .get(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .clone()
}

async fn put_rules(daemon: &TestDaemon, grant_id: &str, allow: &[&str]) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/grants/{grant_id}/rules",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "allow": allow }))
        .send()
        .await
        .unwrap()
}

/// The live credential grant's allow rules, straight from the store.
async fn live_domains(daemon: &TestDaemon) -> Vec<String> {
    SqliteGrantStore::new(daemon.pool().clone())
        .live_grant(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
            Grant::CREDENTIAL_KIND,
        )
        .await
        .unwrap()
        .map(|grant| grant.allow_rules())
        .unwrap_or_default()
}

#[tokio::test]
async fn always_allow_writes_the_records_domain_and_settings_lists_it() {
    let daemon = TestDaemon::start().await;
    let request_id = pending_credential_request(&daemon, "example.com").await;

    assert_eq!(
        decide(&daemon, &request_id, "approved", Some("always"))
            .await
            .status(),
        200
    );

    assert_eq!(live_domains(&daemon).await, vec!["example.com".to_string()]);
    let grants = list_grants(&daemon).await;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["resource_kind"], "credential");
    assert_eq!(grants[0]["allow"], serde_json::json!(["example.com"]));

    // The next action on that domain runs free; another domain does not.
    let rules = live_domains(&daemon).await;
    assert!(domain_allowed("example.com", &rules));
    assert!(!domain_allowed("other.test", &rules));
}

#[tokio::test]
async fn approve_once_leaves_the_agent_in_per_action_mode() {
    let daemon = TestDaemon::start().await;
    let request_id = pending_credential_request(&daemon, "example.com").await;

    assert_eq!(
        decide(&daemon, &request_id, "approved", None)
            .await
            .status(),
        200
    );

    // The grant exists, so settings can show it, but it allows nothing.
    assert_eq!(live_domains(&daemon).await, Vec::<String>::new());
    assert!(!domain_allowed("example.com", &live_domains(&daemon).await));
}

#[tokio::test]
async fn a_denial_writes_no_grant() {
    let daemon = TestDaemon::start().await;
    let request_id = pending_credential_request(&daemon, "example.com").await;

    assert_eq!(
        decide(&daemon, &request_id, "denied", None).await.status(),
        200
    );

    assert!(list_grants(&daemon).await.is_empty());
}

#[tokio::test]
async fn the_sixth_domain_is_refused_rather_than_trimmed() {
    let daemon = TestDaemon::start().await;
    let five = ["a.test", "b.test", "c.test", "d.test", "e.test"];
    for domain in five {
        let request_id = pending_credential_request(&daemon, domain).await;
        assert_eq!(
            decide(&daemon, &request_id, "approved", Some("always"))
                .await
                .status(),
            200
        );
    }
    assert_eq!(live_domains(&daemon).await.len(), 5);

    let request_id = pending_credential_request(&daemon, "f.test").await;
    let refused = decide(&daemon, &request_id, "approved", Some("always")).await;

    assert_eq!(refused.status(), 422);
    assert_eq!(
        live_domains(&daemon).await.len(),
        5,
        "the rules are unchanged"
    );
    // The refusal comes before the decision is stored, so the card still
    // waits and the person can approve it once.
    let request = SqliteRequestStore::new(daemon.pool().clone())
        .get(
            &workspace_id(&daemon).await,
            &RequestId::from(request_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.state, RequestState::Pending);
    assert_eq!(
        decide(&daemon, &request_id, "approved", None)
            .await
            .status(),
        200
    );
    assert_eq!(live_domains(&daemon).await.len(), 5);
}

#[tokio::test]
async fn a_credential_rule_must_read_as_a_domain() {
    let daemon = TestDaemon::start().await;
    let request_id = pending_credential_request(&daemon, "example.com").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    let grant_id = list_grants(&daemon).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A URL is not a rule: the match is exact on the domain.
    let refused = put_rules(&daemon, &grant_id, &["https://example.com/login"]).await;
    assert_eq!(refused.status(), 422);

    // Editing keeps the domains, case-folded.
    let edited = put_rules(&daemon, &grant_id, &["Example.COM", "other.test"]).await;
    assert_eq!(edited.status(), 200);
    assert_eq!(
        live_domains(&daemon).await,
        vec!["example.com".to_string(), "other.test".to_string()]
    );
}

#[tokio::test]
async fn revoking_the_grant_returns_the_agent_to_per_action() {
    let daemon = TestDaemon::start().await;
    let request_id = pending_credential_request(&daemon, "example.com").await;
    decide(&daemon, &request_id, "approved", Some("always")).await;
    let grant_id = list_grants(&daemon).await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let revoked = reqwest::Client::new()
        .delete(format!("{}/api/v1/grants/{grant_id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();

    assert_eq!(revoked.status(), 204);
    assert!(list_grants(&daemon).await.is_empty());
    assert!(!domain_allowed("example.com", &live_domains(&daemon).await));
}
