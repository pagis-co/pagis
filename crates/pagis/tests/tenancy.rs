//! Two people in one Org reach nothing of each other.
//!
//! The harness plants one record of every kind in person A's Workspace
//! and drives every authenticated route of the daemon as person B. The
//! route list comes from `pagis_server::ROUTES`, which the server proves
//! holds every route its router builds, so a route added without a
//! tenant fails here.

use std::time::Duration;

use futures::StreamExt;
use pagis_core::{CallId, TrustTier};
use pagis_testkit::{A_PLUGIN_STDERR, TwoTenants};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn person_b_reaches_no_route_of_person_a() {
    sweep(TwoTenants::start().await).await;
}

/// The same sweep against Postgres. The tenant rule is a
/// property of the store traits, so it holds on both backends or one of
/// them is wrong. The test skips when Docker is not reachable, and says
/// so on stderr.
#[tokio::test]
async fn person_b_reaches_no_route_of_person_a_on_postgres() {
    let Some(world) = TwoTenants::start_on_postgres().await else {
        return;
    };
    sweep(world).await;
}

async fn sweep(world: TwoTenants) {
    let report = world.b_reaches_nothing_of_a().await;

    let refused = report
        .driven
        .iter()
        .filter(|answer| answer.status == 404)
        .count();
    assert!(
        report.driven.len() >= 60 && refused >= 40,
        "the sweep drove {} route calls of which {refused} answered 404; the seed \
         lost its records: {:?}",
        report.driven.len(),
        report.driven
    );
    // Every route the sweep skipped names no record of person A's, so
    // no cross-tenant read is expressible through it. The list is here
    // to be read in review, not to be asserted against.
    println!(
        "driven {} calls, {refused} answered 404, {} routes not crossable",
        report.driven.len(),
        report.not_crossable.len()
    );
}

/// The Artifact download reads the row under the Workspace of the
/// caller, so person A's Artifact is absent for person B.
#[tokio::test]
async fn person_b_gets_404_for_person_as_artifact() {
    let world = TwoTenants::start().await;
    let artifact_id = world.a_id("artifact_id").to_string();

    let as_a = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/artifacts/{artifact_id}",
            world.daemon.base_url
        ))
        .header("cookie", &world.a.cookie)
        .send()
        .await
        .expect("GET as A");
    // Person A owns the row. The blob was never written, so the daemon
    // fails on the bytes and not on the row: what matters is that it is
    // not a 404.
    assert_ne!(as_a.status(), 404, "person A owns the Artifact row");

    let as_b = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/artifacts/{artifact_id}",
            world.daemon.base_url
        ))
        .header("cookie", &world.b.cookie)
        .send()
        .await
        .expect("GET as B");

    assert_eq!(
        as_b.status(),
        404,
        "person B must not read person A's Artifact"
    );
}

/// Each person sets the Keypad Code of their own Workspace (ADR-0021).
/// One person's code does not configure, replace or clear another's.
#[tokio::test]
async fn each_person_sets_the_keypad_code_of_their_own_workspace() {
    let world = TwoTenants::start().await;
    let client = reqwest::Client::new();
    let base = world.daemon.base_url.clone();
    let configured = |cookie: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/settings/trust-list");
        async move {
            let response = client
                .get(url)
                .header("cookie", cookie)
                .send()
                .await
                .expect("GET the trust list");
            assert_eq!(response.status(), 200);
            let page: serde_json::Value = response.json().await.expect("a trust list page");
            page["keypad_code"]["configured"]
                .as_bool()
                .expect("the keypad code state")
        }
    };

    let set = client
        .put(format!("{base}/api/v1/settings/keypad-code"))
        .header("cookie", &world.a.cookie)
        .json(&serde_json::json!({ "code": "246813" }))
        .send()
        .await
        .expect("PUT the code as A");
    assert_eq!(set.status(), 200);

    assert!(configured(world.a.cookie.clone()).await);
    assert!(
        !configured(world.b.cookie.clone()).await,
        "person A's code is not person B's"
    );

    let cleared = client
        .delete(format!("{base}/api/v1/settings/keypad-code"))
        .header("cookie", &world.b.cookie)
        .send()
        .await
        .expect("DELETE the code as B");
    assert_eq!(cleared.status(), 204);
    assert!(
        configured(world.a.cookie.clone()).await,
        "person B's clear leaves person A's code"
    );
}

/// Each person reads and clears the failed-attempt count of their own
/// Workspace (ADR-0021). Wrong codes on person A's lines are not in
/// person B's Settings, and person B's clear leaves person A's delay.
#[tokio::test]
async fn each_person_clears_the_failed_attempts_of_their_own_workspace() {
    let world = TwoTenants::start().await;
    let client = reqwest::Client::new();
    let base = world.daemon.base_url.clone();
    let failed_attempts = |cookie: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/settings/trust-list");
        async move {
            let page: serde_json::Value = client
                .get(url)
                .header("cookie", cookie)
                .send()
                .await
                .expect("GET the trust list")
                .error_for_status()
                .expect("the trust list")
                .json()
                .await
                .expect("a trust list page");
            page["keypad_code"]["failed_attempts"]
                .as_u64()
                .expect("the failed-attempt count")
        }
    };
    let failures = world.daemon.stores().keypad_failures.clone();
    for _ in 0..6 {
        failures
            .record_failure(&world.a.workspace_id, pagis_core::now_ms())
            .await
            .expect("count a wrong code for A");
    }

    assert_eq!(failed_attempts(world.a.cookie.clone()).await, 6);
    assert_eq!(failed_attempts(world.b.cookie.clone()).await, 0);

    let cleared = client
        .delete(format!("{base}/api/v1/settings/keypad-code/failures"))
        .header("cookie", &world.b.cookie)
        .send()
        .await
        .expect("DELETE the failed attempts as B");
    assert_eq!(cleared.status(), 204);
    assert_eq!(
        failed_attempts(world.a.cookie.clone()).await,
        6,
        "person B's clear leaves person A's count"
    );

    let cleared = client
        .delete(format!("{base}/api/v1/settings/keypad-code/failures"))
        .header("cookie", &world.a.cookie)
        .send()
        .await
        .expect("DELETE the failed attempts as A");
    assert_eq!(cleared.status(), 204);
    assert_eq!(failed_attempts(world.a.cookie.clone()).await, 0);
}

/// One POST to a route of one Call, as one person. It answers the
/// status and the body.
async fn post_to_call(
    world: &TwoTenants,
    cookie: &str,
    call_id: &str,
    action: &str,
    body: Option<serde_json::Value>,
) -> (u16, String) {
    let request = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/calls/{call_id}/{action}",
            world.daemon.base_url
        ))
        .header("cookie", cookie);
    let request = match body {
        Some(body) => request.json(&body),
        None => request,
    };
    let response = request.send().await.expect("POST to the call");
    let status = response.status().as_u16();
    (status, response.text().await.expect("the answer body"))
}

/// A live Call is found only under its own Workspace (ADR-0023).
/// Person B knows the id of person A's live Call, and can neither hang
/// it up nor drop its tier: B gets the 404 of a Call that is not live.
/// Person A does both.
#[tokio::test]
async fn only_the_owner_hangs_up_a_live_call_or_drops_its_tier() {
    let world = TwoTenants::start().await;
    let (a, b) = (world.a.cookie.as_str(), world.b.cookie.as_str());
    let live = world.a_id("call_id");
    let not_live = CallId::generate();
    let not_live = not_live.as_str();
    let unknown = || Some(serde_json::json!({ "tier": "unknown" }));
    let call_id = CallId::from(live.to_string());
    let hub = world
        .daemon
        .live_calls
        .hub(&world.a.workspace_id, &call_id)
        .expect("person A's Call is live");
    let gate = world
        .daemon
        .live_tiers
        .gate(&world.a.workspace_id, &call_id)
        .expect("person A's Call has a tier gate");

    let crossed = post_to_call(&world, b, live, "hangup", None).await;
    assert_eq!(crossed.0, 404, "person B hung up person A's Call");
    assert_eq!(
        crossed,
        post_to_call(&world, b, not_live, "hangup", None).await,
        "person A's live Call must answer person B as a Call that is not live"
    );
    assert!(!hub.is_ended(), "person B ended person A's Call");

    let crossed = post_to_call(&world, b, live, "tier", unknown()).await;
    assert_eq!(
        crossed.0, 404,
        "person B dropped the tier of person A's Call"
    );
    assert_eq!(
        crossed,
        post_to_call(&world, b, not_live, "tier", unknown()).await,
        "person A's live Call must answer person B as a Call that is not live"
    );
    assert_eq!(gate.tier(), TrustTier::Trusted);

    let (status, body) = post_to_call(&world, a, live, "tier", unknown()).await;
    assert_eq!(status, 200, "{body}");
    let dropped: serde_json::Value = serde_json::from_str(&body).expect("a tier body");
    assert_eq!(dropped["call_id"], live);
    assert_eq!(dropped["tier"], "unknown");
    assert_eq!(gate.tier(), TrustTier::Unknown);

    let (status, body) = post_to_call(&world, a, live, "hangup", None).await;
    assert_eq!(status, 204, "{body}");
    tokio::time::timeout(
        Duration::from_secs(5),
        hub.watch_ended().wait_for(|ended| ended.is_some()),
    )
    .await
    .expect("person A's hang up ends the Call")
    .expect("the hub is alive");
}

/// The first frame of a listen socket, opened as one person.
async fn first_listen_frame(world: &TwoTenants, call_id: &str, cookie: &str) -> serde_json::Value {
    let url = format!("ws://{}/api/v1/calls/{call_id}/listen", world.daemon.addr);
    let (mut socket, _) = connect_async(world.daemon.ws_request_as(&url, cookie))
        .await
        .expect("the listen socket opens");
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("the frame is JSON");
        }
    }
}

/// Listen-Live finds a live Call only under the listener's own
/// Workspace (ADR-0023). Person B gets the `not_found` of a Call that
/// is not live for person A's live Call, and person A hears it.
#[tokio::test]
async fn only_the_owner_listens_to_a_live_call() {
    let world = TwoTenants::start().await;
    let call_id = world.a_id("call_id");

    let crossed = first_listen_frame(&world, call_id, &world.b.cookie).await;
    let absent = first_listen_frame(&world, CallId::generate().as_str(), &world.b.cookie).await;
    assert_eq!(crossed["type"], "error", "{crossed}");
    assert_eq!(crossed["code"], "not_found", "{crossed}");
    assert_eq!(crossed, absent);

    let own = first_listen_frame(&world, call_id, &world.a.cookie).await;
    assert_eq!(own["type"], "ready", "{own}");
}

/// One Plugin log read as one person, through the product route.
async fn plugin_log(world: &TwoTenants, cookie: &str) -> (u16, String) {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/plugins/{}/log",
            world.daemon.base_url,
            world.a_id("plugin_id")
        ))
        .header("cookie", cookie)
        .send()
        .await
        .expect("read the plugin log");
    let status = response.status().as_u16();
    let body: serde_json::Value = response.json().await.expect("the log answer");
    (
        status,
        body["text"].as_str().unwrap_or_default().to_string(),
    )
}

/// Each person reads the Plugin log of their own Workspace (ADR-0023).
/// The Org's Plugin runs in each Workspace's own Plugin Computer, and
/// person A's server wrote a line to stderr. Person B reads the log of
/// the same Plugin, and it does not hold A's line.
#[tokio::test]
async fn each_person_reads_the_plugin_log_of_their_own_workspace() {
    let world = TwoTenants::start().await;

    let (a_status, a_text) = plugin_log(&world, &world.a.cookie).await;
    let (b_status, b_text) = plugin_log(&world, &world.b.cookie).await;

    assert_eq!(a_status, 200);
    assert!(a_text.contains(A_PLUGIN_STDERR), "A reads {a_text:?}");
    // B's servers never ran, so B's log of the Plugin is empty, as the
    // log of a Plugin with no output is.
    assert_eq!(b_status, 200);
    assert_eq!(b_text, "");
}

/// The ids of the Push Subscriptions that `cookie` lists.
async fn push_subscription_ids(world: &TwoTenants, cookie: &str) -> Vec<String> {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/push-subscriptions",
            world.daemon.base_url
        ))
        .header("cookie", cookie)
        .send()
        .await
        .expect("list the push subscriptions");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("the list");
    body["items"]
        .as_array()
        .expect("the items")
        .iter()
        .map(|item| item["id"].as_str().expect("an id").to_string())
        .collect()
}

/// Person B neither lists nor removes person A's Push Subscription
/// (ADR-0030): B's list holds none of A's, and A's id reads as absent.
#[tokio::test]
async fn person_b_cannot_read_or_delete_person_as_push_subscription() {
    let world = TwoTenants::start().await;
    let a_subscription = world.a_id("push_subscription_id").to_string();

    assert!(
        push_subscription_ids(&world, &world.b.cookie)
            .await
            .is_empty()
    );
    let removal = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/push-subscriptions/{a_subscription}",
            world.daemon.base_url
        ))
        .header("cookie", &world.b.cookie)
        .send()
        .await
        .expect("delete as B");
    assert_eq!(removal.status(), 404);

    assert_eq!(
        push_subscription_ids(&world, &world.a.cookie).await,
        vec![a_subscription]
    );
}
