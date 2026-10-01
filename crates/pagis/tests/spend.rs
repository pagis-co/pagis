//! What a Run spends, and the cap that stops one.
//!
//! The router computes a cost per model call. These tests prove the
//! daemon keeps it, attributes it to the Workspace and the Run, serves
//! it to the person and to the Administrator, and stops a Run whose
//! Person is at their monthly Spend Cap with a sentence in the
//! conversation.

use std::sync::Arc;
use std::time::Duration;

use pagis_agent::Usage;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

const PASSWORD: &str = "correct horse battery";

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn options(brain: &Arc<ScriptedBrain>) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    }
}

fn usage() -> Usage {
    Usage {
        input_tokens: 1_200,
        output_tokens: 300,
        cache_read_input_tokens: 200,
        cache_write_input_tokens: 100,
        reasoning_tokens: 0,
    }
}

/// Send one message into the seeded person's DM.
async fn say(daemon: &TestDaemon, cookie: &str, channel_id: &str, pending_id: &str, text: &str) {
    let response = client()
        .post(format!(
            "{}/api/v1/channels/{channel_id}/messages",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED, "the message landed");
}

async fn get(daemon: &TestDaemon, cookie: &str, path: &str) -> serde_json::Value {
    // The installation's spend answers on the Administration Port, and
    // a person's own spend on the product port.
    let base = match path.starts_with("/api/v1/administration/") {
        true => &daemon.administration_base_url,
        false => &daemon.base_url,
    };
    let response = client()
        .get(format!("{base}{path}"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    response.json().await.unwrap()
}

/// Poll the person's own usage read until it holds at least one call.
async fn wait_for_usage(daemon: &TestDaemon, cookie: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let usage = get(daemon, cookie, "/api/v1/usage").await;
        if usage["total"]["calls"].as_i64().unwrap_or_default() > 0 {
            return usage;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the run recorded no usage: {usage}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll the person's runs until one reaches a state.
async fn wait_for_run(daemon: &TestDaemon, cookie: &str, state: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let page = get(daemon, cookie, "/api/v1/runs").await;
        if let Some(run) = page["items"]
            .as_array()
            .and_then(|items| items.iter().find(|run| run["state"] == state))
        {
            return run.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no run reached {state}: {page}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The messages of a channel, newest last.
async fn messages(daemon: &TestDaemon, cookie: &str, channel_id: &str) -> Vec<serde_json::Value> {
    let page = get(
        daemon,
        cookie,
        &format!("/api/v1/channels/{channel_id}/messages"),
    )
    .await;
    page["items"].as_array().expect("items").clone()
}

/// The tokens and the cost of one Run are stored, attributed, and read
/// back by the person who spent them.
#[tokio::test]
async fn a_run_records_what_it_spent_and_the_person_reads_it() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply_costing(&["Done."], usage(), 0.5));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();

    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;

    let mine = wait_for_usage(&daemon, daemon.cookie()).await;
    assert_eq!(mine["total"]["input_tokens"], 1_200);
    assert_eq!(mine["total"]["output_tokens"], 300);
    assert_eq!(mine["total"]["cache_read_tokens"], 200);
    assert_eq!(mine["total"]["cache_write_tokens"], 100);
    assert_eq!(mine["total"]["cost_usd"], 0.5);
    assert!(mine["monthly_spend_cap_usd"].is_null(), "{mine}");

    // The spend is attributed to the Run that asked for it.
    let runs = mine["runs"].as_array().expect("runs");
    assert_eq!(runs.len(), 1, "{mine}");
    assert_eq!(runs[0]["total"]["cost_usd"], 0.5);
    let recorded = runs[0]["run_id"].as_str().expect("a run id");
    let page = get(&daemon, daemon.cookie(), "/api/v1/runs").await;
    assert!(
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|run| run["id"] == recorded),
        "the usage names a run of this workspace: {page}"
    );
}

/// The Administrator reads spend per person for a period, and one
/// person's spend never lands on another's line.
#[tokio::test]
async fn the_administrator_reads_spend_per_person() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply_costing(&["Done."], usage(), 0.75));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();

    // A second person, who spends nothing.
    let created: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "name": "Grace",
            "password": PASSWORD,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let person = &created["person"];

    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;
    wait_for_usage(&daemon, daemon.cookie()).await;

    let usage = get(&daemon, daemon.cookie(), "/api/v1/administration/usage").await;
    assert_eq!(usage["total"]["cost_usd"], 0.75);
    let items = usage["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{usage}");
    // The biggest spender first.
    assert_eq!(items[0]["person"]["id"], daemon.user_id.to_string());
    assert_eq!(items[0]["total"]["cost_usd"], 0.75);
    assert_eq!(items[0]["cap_reached"], false);
    assert_eq!(items[1]["person"]["id"], person["id"]);
    assert_eq!(items[1]["total"]["cost_usd"], 0.0);
    assert_eq!(items[1]["total"]["calls"], 0);

    // A period before anything happened holds nothing.
    let empty = get(
        &daemon,
        daemon.cookie(),
        "/api/v1/administration/usage?from=0&to=1",
    )
    .await;
    assert_eq!(empty["total"]["cost_usd"], 0.0);
}

/// A person at their monthly Spend Cap is told so in the conversation,
/// and the Run ends with a failure kind a client can name. A
/// silent stop would leave a sprite that does nothing and no reason for
/// it.
#[tokio::test]
async fn a_person_at_their_spend_cap_is_told_so_in_the_conversation() {
    let brain = Arc::new(ScriptedBrain::default());
    // The first turn spends past the cap; nothing scripts a second,
    // because the second Run must never reach a model at all.
    brain.push(Script::reply_costing(&["Done."], usage(), 5.0));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();

    let capped: serde_json::Value = client()
        .put(format!(
            "{}/api/v1/administration/people/{}/spend-cap",
            daemon.administration_base_url, daemon.user_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "monthly_spend_cap_usd": 1.0 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(capped["monthly_spend_cap_usd"], 1.0);

    // The first run is under the cap when it starts, and it runs.
    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;
    let spent = wait_for_usage(&daemon, daemon.cookie()).await;
    assert_eq!(spent["total"]["cost_usd"], 5.0);
    assert_eq!(spent["monthly_spend_cap_usd"], 1.0);

    // The second is over it, and it stops before it asks a model
    // anything.
    say(&daemon, daemon.cookie(), &channel, "p2", "Do another thing").await;
    let failed = wait_for_run(&daemon, daemon.cookie(), "failed").await;
    assert_eq!(failed["failure_kind"], "spend_cap_reached");

    let said = messages(&daemon, daemon.cookie(), &channel).await;
    let note = said
        .iter()
        .find(|message| {
            message["author_kind"] == "system"
                && message["text_content"]
                    .as_str()
                    .is_some_and(|text| text.contains("spend cap"))
        })
        .unwrap_or_else(|| panic!("the conversation says the cap was reached: {said:?}"));
    assert_eq!(note["run_id"], failed["id"]);

    // The administrator's roster read says who is stopped.
    let usage = get(&daemon, daemon.cookie(), "/api/v1/administration/usage").await;
    let line = usage["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["person"]["id"] == daemon.user_id.to_string())
        .expect("the administrator's own line");
    assert_eq!(line["cap_reached"], true);
}

/// A cap cannot count a cost it does not know. A capped person's Run on
/// a model that no layer prices stops before it asks the model, and the
/// note names the model.
#[tokio::test]
async fn a_capped_person_does_not_start_on_a_model_without_a_price() {
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();
    let routed = client()
        .put(format!(
            "{}/api/v1/settings/model-aliases/default",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidates": ["openai/gpt-unlisted"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(routed.status(), StatusCode::OK);
    let capped = client()
        .put(format!(
            "{}/api/v1/administration/people/{}/spend-cap",
            daemon.administration_base_url, daemon.user_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "monthly_spend_cap_usd": 10.0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(capped.status(), StatusCode::OK);

    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;

    let failed = wait_for_run(&daemon, daemon.cookie(), "failed").await;
    assert_eq!(failed["failure_kind"], "spend_cap_reached");
    assert!(brain.requests().is_empty(), "the run asked a model");
    let said = messages(&daemon, daemon.cookie(), &channel).await;
    assert!(
        said.iter().any(|message| {
            message["author_kind"] == "system"
                && message["text_content"].as_str().is_some_and(|text| {
                    text.contains("openai/gpt-unlisted") && text.contains("no known price")
                })
        }),
        "the conversation names the model: {said:?}"
    );
}

/// A call whose cost is unknown is kept as unknown: it counts as a call,
/// adds no money, and the read says how many calls have no price.
#[tokio::test]
async fn an_unpriced_call_reads_as_unknown_not_as_free() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply_with_accounting(&["Done."], 0, Some(usage())));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();

    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;
    let mine = wait_for_usage(&daemon, daemon.cookie()).await;

    assert_eq!(mine["total"]["calls"], 1);
    assert_eq!(mine["total"]["unpriced_calls"], 1);
    assert_eq!(mine["total"]["cost_usd"], 0.0);
}

/// A person with no cap is never stopped, which is what a local
/// installation holds: the one person has no cap and every run of theirs
/// starts.
#[tokio::test]
async fn a_person_with_no_cap_is_never_stopped() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply_costing(&["One."], usage(), 1_000.0));
    brain.push(Script::reply_costing(&["Two."], usage(), 1_000.0));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();

    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;
    wait_for_usage(&daemon, daemon.cookie()).await;
    say(&daemon, daemon.cookie(), &channel, "p2", "Do another thing").await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let mine = get(&daemon, daemon.cookie(), "/api/v1/usage").await;
        if mine["total"]["calls"].as_i64().unwrap_or_default() >= 2 {
            assert_eq!(mine["total"]["cost_usd"], 2_000.0);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the second run did not spend: {mine}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let page = get(&daemon, daemon.cookie(), "/api/v1/runs").await;
    assert!(
        !page["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|run| run["failure_kind"] == "spend_cap_reached"),
        "a person with no cap was stopped: {page}"
    );
}

/// A Member reads their own spend and nobody else's.
#[tokio::test]
async fn a_member_reads_their_own_spend_and_nothing_of_the_installations() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply_costing(&["Done."], usage(), 0.25));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();
    client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "name": "Grace",
            "password": PASSWORD,
        }))
        .send()
        .await
        .unwrap();
    let response = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": PASSWORD,
        }))
        .send()
        .await
        .unwrap();
    let cookie = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("pagis_session="))
        .and_then(|value| value.split(';').next())
        .expect("the answer sets the cookie")
        .to_string();

    // The administrator spends; the member's own read stays at zero.
    say(&daemon, daemon.cookie(), &channel, "p1", "Do a thing").await;
    wait_for_usage(&daemon, daemon.cookie()).await;

    let theirs = get(&daemon, &cookie, "/api/v1/usage").await;
    assert_eq!(theirs["total"]["cost_usd"], 0.0);
    assert_eq!(theirs["total"]["calls"], 0);
    assert!(theirs["runs"].as_array().expect("runs").is_empty());

    // And the installation's read is refused.
    let refused = client()
        .get(format!(
            "{}/api/v1/administration/usage",
            daemon.administration_base_url
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}

/// A provider that refuses the installation's key fails the Run, and the
/// conversation says who fixes it and where: the person who sent the
/// message may be a Member who cannot see the provider setup.
#[tokio::test]
async fn a_refused_key_says_an_administrator_sets_up_the_providers() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::refuse_key(
        "all candidates for model `default` failed: provider `anthropic` returned status 401 \
         (Authentication): invalid x-api-key",
    ));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let channel = daemon.dm_channel_id.clone();

    say(&daemon, daemon.cookie(), &channel, "p1", "Hello").await;
    let failed = wait_for_run(&daemon, daemon.cookie(), "failed").await;
    assert_eq!(failed["failure_kind"], "model_failed");

    let said = messages(&daemon, daemon.cookie(), &channel).await;
    let note = said
        .iter()
        .find(|message| message["author_kind"] == "system")
        .unwrap_or_else(|| panic!("the conversation says why: {said:?}"));
    let text = note["text_content"].as_str().unwrap();
    assert!(
        text.contains("refused this installation's API key"),
        "{text}"
    );
    assert!(
        text.contains("Administration Interface, under Providers"),
        "{text}"
    );
    assert_eq!(note["run_id"], failed["id"]);
}
