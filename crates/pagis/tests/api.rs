//! Full-daemon API tests: the axum app booted in-process on a temp
//! directory, driven over real HTTP via the testkit harness.

use pagis_testkit::TestDaemon;

#[tokio::test]
async fn health_endpoint_reports_ok() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::get(format!("{}/api/v1/health", daemon.base_url))
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    // The version is the shell's attach check (ADR-0025). A browser on
    // this machine signs in with a password (ADR-0028).
    assert_eq!(
        body,
        serde_json::json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "sign_in": "password",
        })
    );
}

/// What a connect-only client reads before it has a Session. It
/// compares this version with its own compatibility range, so the route
/// answers without a cookie and the version is SemVer the client can
/// range-check.
#[tokio::test]
async fn health_reports_a_semver_version_without_a_session() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/health", daemon.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    let version = body["version"].as_str().unwrap();
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "{version} is not major.minor.patch");
    for part in parts {
        assert!(
            part.split(['-', '+'])
                .next()
                .unwrap()
                .parse::<u64>()
                .is_ok(),
            "{version} is not SemVer"
        );
    }
}

#[tokio::test]
async fn root_and_client_routes_serve_the_spa_page() {
    let daemon = TestDaemon::start().await;

    for path in ["/", "/channels/01SOMECHANNEL"] {
        let response = reqwest::get(format!("{}{path}", daemon.base_url))
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "GET {path}");
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.starts_with("text/html"), "GET {path}");
    }
}

/// Text that an Agent writes can name an image at any host, and the
/// browser of each Person who reads it would send that host a request.
/// The policy of the entry page lets an image load only from the
/// daemon, from a `data:` URL or from a `blob:` URL. The daemon sends
/// it, so a Local Installation without a reverse proxy has it too.
#[tokio::test]
async fn the_product_app_page_loads_no_image_from_another_origin() {
    let daemon = TestDaemon::start().await;

    for path in ["/", "/channels/01SOMECHANNEL", "/index.html"] {
        let response = reqwest::get(format!("{}{path}", daemon.base_url))
            .await
            .unwrap();
        let policy = response
            .headers()
            .get("content-security-policy")
            .unwrap_or_else(|| panic!("GET {path} has no Content-Security-Policy"))
            .to_str()
            .unwrap()
            .to_string();
        let images: Vec<&str> = policy
            .split(';')
            .find_map(|directive| directive.trim().strip_prefix("img-src "))
            .unwrap_or_else(|| panic!("GET {path}: {policy} has no img-src"))
            .split_whitespace()
            .collect();
        assert_eq!(images, ["'self'", "data:", "blob:"], "GET {path}");
    }
}

#[tokio::test]
async fn unknown_api_route_stays_a_json_404() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::get(format!("{}/api/v1/no-such-route", daemon.base_url))
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "not_found");
}
