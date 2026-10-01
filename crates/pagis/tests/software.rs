//! The Software destination reads (ADR-0022): the list, one
//! package in full, and one Contribution with its patch. The desk only
//! reads, so there is no mutating path to test.

use pagis_core::{
    AgentStore, Contribution, ContributionId, ContributionStatus, ContributionStore, Run, RunId,
    RunState, RunStore, SoftwarePackage, SoftwarePackageId, SoftwareStore, SoftwareVersion,
    TriggerKind, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteContributionStore, SqliteRunStore, SqliteSoftwareStore,
    SqliteWorkspaceStore,
};
use pagis_testkit::TestDaemon;

/// The manifest of one Version, as the store holds it.
fn manifest(package: &str) -> serde_json::Value {
    serde_json::json!({
        "manifest": {
            "package": {
                "name": package,
                "description": "The forecast package.",
                "keywords": ["weather"],
                "setup": null,
            },
            "tool": [{
                "name": "forecast",
                "description": "The forecast of one city.",
                "entry": "bin/forecast.py",
                "schema": "schemas/forecast.json",
                "timeout_s": null,
            }],
        },
        "schemas": {},
        "widget_schemas": {},
    })
}

async fn get(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
        .error_for_status()
        .expect("success")
        .json()
        .await
        .expect("JSON")
}

async fn status(daemon: &TestDaemon, path: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
        .status()
        .as_u16()
}

/// One origin package with two Versions, one Fork of it, and one open
/// Contribution from the Fork. The Agent tools write these rows in the
/// daemon; the desk only reads them.
async fn seed(daemon: &TestDaemon) -> ContributionId {
    let pool = daemon.pool().clone();
    let workspace = SqliteWorkspaceStore::new(pool.clone())
        .list()
        .await
        .expect("workspaces")
        .remove(0);
    let author = SqliteAgentStore::new(pool.clone())
        .list_by_workspace(&workspace.id)
        .await
        .expect("agents")
        .remove(0);
    let packages = SqliteSoftwareStore::new(pool.clone());
    // Every Version and Contribution names the Run that wrote it.
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: author.id.clone(),
        channel_id: None,
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
        hop_count: 0,
        origin: None,
        state: RunState::Completed,
        failure_kind: None,
        dismissed_at: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: now_ms(),
    };
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .expect("the run");
    let contributions = SqliteContributionStore::new(pool.clone());
    let now = now_ms();

    let origin = SoftwarePackage {
        id: SoftwarePackageId::generate(),
        workspace_id: workspace.id.clone(),
        name: "weather".to_string(),
        author_agent_id: author.id.clone(),
        description: "The forecast package.".to_string(),
        keywords: vec!["weather".to_string()],
        latest_version: "v1".to_string(),
        origin_package_id: None,
        origin_version: None,
        created_at: now,
        updated_at: now,
    };
    packages.create_package(&origin).await.expect("the origin");
    for version in ["v1", "v2"] {
        let held = SoftwarePackage {
            latest_version: version.to_string(),
            ..origin.clone()
        };
        packages
            .add_version(
                &held,
                &SoftwareVersion {
                    package_id: origin.id.clone(),
                    version: version.to_string(),
                    notes: format!("{version} notes"),
                    commit_id: format!("commit-{version}"),
                    manifest: manifest("weather"),
                    published_at: now,
                    run_id: run.id.clone(),
                },
            )
            .await
            .expect("the version");
    }

    let fork = SoftwarePackage {
        id: SoftwarePackageId::generate(),
        name: "weather-bo".to_string(),
        origin_package_id: Some(origin.id.clone()),
        origin_version: Some("v1".to_string()),
        ..origin.clone()
    };
    packages.create_package(&fork).await.expect("the fork");
    packages
        .add_version(
            &fork,
            &SoftwareVersion {
                package_id: fork.id.clone(),
                version: "v1".to_string(),
                notes: "the fork".to_string(),
                commit_id: "commit-fork".to_string(),
                manifest: manifest("weather-bo"),
                published_at: now,
                run_id: run.id.clone(),
            },
        )
        .await
        .expect("the fork version");

    let contribution = Contribution {
        id: ContributionId::generate(),
        workspace_id: workspace.id.clone(),
        package_id: origin.id.clone(),
        base_version: "v1".to_string(),
        latest_at_open: "v2".to_string(),
        fork_package_id: fork.id.clone(),
        fork_version: "v1".to_string(),
        patch: "--- a/bin/forecast.py\n+++ b/bin/forecast.py\n-print(1)\n+print(2)\n".to_string(),
        summary: "print two, not one".to_string(),
        status: ContributionStatus::Open,
        outcome_reason: None,
        created_at: now,
        closed_at: None,
        run_id: run.id.clone(),
    };
    contributions
        .create(&contribution)
        .await
        .expect("the contribution");
    contribution.id
}

#[tokio::test]
async fn the_list_carries_what_one_row_shows() {
    let daemon = TestDaemon::start().await;
    seed(&daemon).await;

    let page = get(&daemon, "/api/v1/software").await;
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    let weather = items
        .iter()
        .find(|item| item["name"] == "weather")
        .expect("the origin package");
    assert_eq!(weather["latest_version"], "v2");
    assert_eq!(weather["author_name"], "Pixie");
    assert_eq!(weather["keywords"][0], "weather");
    assert_eq!(weather["tool_count"], 1);
    assert_eq!(weather["open_contributions"], 1);
    let fork = items
        .iter()
        .find(|item| item["name"] == "weather-bo")
        .expect("the fork");
    assert_eq!(fork["open_contributions"], 0, "the Fork holds none");
}

#[tokio::test]
async fn one_package_reads_its_tools_versions_and_contributions() {
    let daemon = TestDaemon::start().await;
    let contribution_id = seed(&daemon).await;

    let package = get(&daemon, "/api/v1/software/weather").await;
    assert_eq!(package["description"], "The forecast package.");
    assert_eq!(package["tools"][0]["name"], "forecast");
    assert_eq!(
        package["tools"][0]["description"],
        "The forecast of one city."
    );
    let versions = package["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["version"], "v2", "the newest Version first");
    assert_eq!(versions[0]["notes"], "v2 notes");
    let contributions = package["contributions"].as_array().expect("contributions");
    assert_eq!(contributions.len(), 1);
    assert_eq!(contributions[0]["id"], contribution_id.as_str());
    assert_eq!(contributions[0]["status"], "open");
    assert_eq!(contributions[0]["fork_package"], "weather-bo");
    assert_eq!(contributions[0]["summary"], "print two, not one");
    assert!(
        contributions[0].get("patch").is_none(),
        "the list carries no patch"
    );

    // A Fork names where it comes from.
    let fork = get(&daemon, "/api/v1/software/weather-bo").await;
    assert_eq!(fork["origin_package"], "weather");
    assert_eq!(fork["origin_version"], "v1");
    assert!(fork["contributions"].as_array().expect("none").is_empty());

    assert_eq!(status(&daemon, "/api/v1/software/nothing").await, 404);
}

#[tokio::test]
async fn one_contribution_reads_with_its_patch() {
    let daemon = TestDaemon::start().await;
    let contribution_id = seed(&daemon).await;

    let record = get(
        &daemon,
        &format!("/api/v1/software/weather/contributions/{contribution_id}"),
    )
    .await;
    assert_eq!(record["id"], contribution_id.as_str());
    assert_eq!(record["package"], "weather");
    assert_eq!(record["base_version"], "v1");
    assert_eq!(record["fork_version"], "v1");
    assert!(
        record["patch"]
            .as_str()
            .expect("the patch")
            .contains("+print(2)"),
        "{record}"
    );

    // A Contribution reads only under the package it belongs to.
    assert_eq!(
        status(
            &daemon,
            &format!("/api/v1/software/weather-bo/contributions/{contribution_id}")
        )
        .await,
        404
    );
}

/// The sandbox proxy answers with the policy of the tenant its path names.
///
/// The route reads no Session, because a sandboxed frame sends no cookie,
/// so the Workspace is in the path. A route that resolved the first
/// Workspace of the installation would give a member's Widget the closed
/// default, and anybody could probe the administrator's installed package
/// triples through it. Each tenant gets its own declaration, and a triple that names nothing installed in the
/// Workspace it names answers under the closed default whoever asks.
#[tokio::test]
async fn the_sandbox_policy_is_the_named_tenants() {
    let tenants = pagis_testkit::tenancy::TwoTenants::start().await;
    let daemon = &tenants.daemon;

    // Person A publishes `weather`, which declares one connect origin.
    // Person B publishes `almanac`, which declares another.
    seed_widget_version(
        daemon,
        &tenants.a.workspace_id,
        "weather",
        "https://a.example",
    )
    .await;
    seed_widget_version(
        daemon,
        &tenants.b.workspace_id,
        "almanac",
        "https://b.example",
    )
    .await;

    let policy = |workspace: &str, package: &str| {
        let base = daemon.base_url.clone();
        let workspace = workspace.to_string();
        let package = package.to_string();
        async move {
            let response = reqwest::Client::new()
                .get(format!(
                    "{base}/api/v1/widgets/{workspace}/{package}/v1/forecast-card/sandbox"
                ))
                .send()
                .await
                .expect("GET");
            assert_eq!(response.status(), 200, "the proxy always answers");
            response.headers()["content-security-policy"]
                .to_str()
                .expect("the policy is text")
                .to_string()
        }
    };

    // Each tenant's own Widget gets its own declaration.
    assert!(
        policy(tenants.a.workspace_id.as_str(), "weather")
            .await
            .contains("connect-src https://a.example;"),
    );
    let b_policy = policy(tenants.b.workspace_id.as_str(), "almanac").await;
    assert!(
        b_policy.contains("connect-src https://b.example;"),
        "B's widget gets B's policy: {b_policy}"
    );

    // And A's package triple is not probeable through B's path: the
    // answer is the closed default, exactly as an unknown Widget's is.
    let probed = policy(tenants.b.workspace_id.as_str(), "weather").await;
    assert!(
        probed.contains("connect-src 'none';"),
        "A's declaration leaked through B's path: {probed}"
    );
    let other_way = policy(tenants.a.workspace_id.as_str(), "almanac").await;
    assert!(other_way.contains("connect-src 'none';"), "{other_way}");
}

/// One published Version of one Workspace with one Widget, whose
/// Content-Security-Policy names `connect`. The Version's own author is
/// that Workspace's first Agent.
async fn seed_widget_version(
    daemon: &TestDaemon,
    workspace_id: &pagis_core::WorkspaceId,
    package: &str,
    connect: &str,
) {
    let pool = daemon.pool().clone();
    let author = match SqliteAgentStore::new(pool.clone())
        .list_by_workspace(workspace_id)
        .await
        .expect("agents")
        .into_iter()
        .next()
    {
        Some(agent) => agent,
        // Person B's Workspace is empty, so the Version's author is an
        // Agent this test writes: a published package always has one.
        None => {
            let agent = pagis_core::Agent {
                id: pagis_core::AgentId::generate(),
                workspace_id: workspace_id.clone(),
                name: "Bo".to_string(),
                job: String::new(),
                description: String::new(),
                personality: String::new(),
                model_alias: "default".to_string(),
                avatar: Default::default(),
                voice: None,
                standing_brief: None,
                status: pagis_core::AgentStatus::Active,
                created_at: now_ms(),
                updated_at: now_ms(),
            };
            SqliteAgentStore::new(pool.clone())
                .create(&agent)
                .await
                .expect("the author");
            agent
        }
    };
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: author.id.clone(),
        channel_id: None,
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
        hop_count: 0,
        origin: None,
        state: RunState::Completed,
        failure_kind: None,
        dismissed_at: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: now_ms(),
    };
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .expect("the run");

    let page = "<!doctype html><p>The forecast.</p>";
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in [
        ("widgets/forecast.html", page),
        ("widgets/forecast.json", "{\"type\":\"object\"}"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body.as_bytes())
            .expect("the tar entry");
    }
    let tar = builder.into_inner().expect("the tar");

    let git = pagis_software::SoftwareGitStore::new(daemon.booted.home.join("software"));
    let commit_id = git
        .commit_version(
            workspace_id,
            pagis_software::NewVersion {
                package,
                version: "v1",
                previous: None,
                notes: "The first Version.",
                author: &pagis_software::PublishAuthor::of_agent(&author.name, &author.id),
                tar,
            },
        )
        .await
        .expect("the version tree");

    let now = now_ms();
    let record = SoftwarePackage {
        id: SoftwarePackageId::generate(),
        workspace_id: workspace_id.clone(),
        name: package.to_string(),
        author_agent_id: author.id.clone(),
        description: "The forecast package.".to_string(),
        keywords: vec!["weather".to_string()],
        latest_version: "v1".to_string(),
        origin_package_id: None,
        origin_version: None,
        created_at: now,
        updated_at: now,
    };
    let packages = SqliteSoftwareStore::new(pool.clone());
    packages.create_package(&record).await.expect("the package");
    packages
        .add_version(
            &record,
            &SoftwareVersion {
                package_id: record.id.clone(),
                version: "v1".to_string(),
                notes: "The first Version.".to_string(),
                commit_id,
                manifest: widget_manifest(package, connect),
                published_at: now,
                run_id: run.id.clone(),
            },
        )
        .await
        .expect("the version record");
}

/// The stored manifest of a Version with one Widget and one declared
/// connect origin.
fn widget_manifest(package: &str, connect: &str) -> serde_json::Value {
    serde_json::json!({
        "manifest": {
            "package": {
                "name": package,
                "description": "The forecast package.",
                "keywords": ["weather"],
                "setup": null,
            },
            "tool": [{
                "name": "forecast",
                "description": "The forecast of one city.",
                "entry": "bin/forecast.py",
                "schema": "schemas/forecast.json",
                "timeout_s": null,
                "widget": "forecast-card",
            }],
            "widget": [{
                "name": "forecast-card",
                "html": "widgets/forecast.html",
                "schema": "widgets/forecast.json",
                "csp": { "connect": [connect], "resource": [] },
            }],
        },
        "schemas": {},
        "widget_schemas": { "forecast-card": { "type": "object" } },
    })
}
