//! Full-daemon Agent Phone Number tests (ADR-0018): the user
//! connects a carrier, buys a number for an Agent, moves it to a second
//! Agent, unassigns it and releases it. The carrier is a fake, so no
//! network and no money are involved.

use std::sync::Arc;

use pagis_telephony::fake::{CarrierCall, FakeCallTransport, FakeNumberCatalog, TokioClock};
use pagis_testkit::{TestDaemon, TestDaemonOptions};

struct Desk {
    daemon: TestDaemon,
    catalog: Arc<FakeNumberCatalog>,
}

impl Desk {
    async fn start() -> Self {
        let catalog = Arc::new(FakeNumberCatalog::offering(&[
            "+14155550123",
            "+14155550124",
        ]));
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            number_catalog: Arc::clone(&catalog) as _,
            ..TestDaemonOptions::default()
        })
        .await;
        Self { daemon, catalog }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.daemon.base_url)
    }

    async fn get(&self, path: &str) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .get(self.url(path))
            .header("cookie", self.daemon.cookie())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    async fn post(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .post(self.url(path))
            .header("cookie", self.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    /// Connect the carrier the way an Administrator does: one API key
    /// on the administration port.
    async fn connect_carrier(&self) -> String {
        self.daemon
            .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
            .await
    }

    /// Keep the carrier's SIP sign-in the way an Administrator does, and
    /// answer the status and the carrier's setup.
    async fn set_sip_sign_in(&self) -> (u16, serde_json::Value) {
        self.daemon
            .set_up_provider(
                "telnyx",
                "sip",
                serde_json::json!({
                    "username": "robin",
                    "password": "sip-secret",
                    "domain": "sip.telnyx.com",
                }),
            )
            .await
    }

    /// Remove the carrier account the way an Administrator does.
    async fn remove_carrier(&self) -> u16 {
        reqwest::Client::new()
            .delete(format!(
                "{}/api/v1/administration/providers/telnyx/connection",
                self.daemon.administration_base_url
            ))
            .header("cookie", self.daemon.cookie())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    async fn create_agent(&self, name: &str) -> String {
        let (status, body) = self
            .post(
                "/api/v1/agents",
                serde_json::json!({
                    "name": name,
                    "job": "answers the phone",
                    "personality": "warm",
                }),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        body["id"].as_str().unwrap().to_string()
    }

    async fn buy(&self, e164: &str, agent_id: Option<&str>) -> (u16, serde_json::Value) {
        self.post(
            "/api/v1/settings/phone-numbers",
            serde_json::json!({ "e164": e164, "agent_id": agent_id }),
        )
        .await
    }

    async fn adopt(&self, e164: &str, agent_id: Option<&str>) -> (u16, serde_json::Value) {
        self.post(
            "/api/v1/settings/phone-numbers/adopt",
            serde_json::json!({ "e164": e164, "agent_id": agent_id }),
        )
        .await
    }
}

#[tokio::test]
async fn a_number_is_bought_moved_unassigned_and_released() {
    let desk = Desk::start().await;
    desk.connect_carrier().await;
    let robin = desk.daemon.agent_id.clone();
    let wren = desk.create_agent("Wren").await;

    // The carrier's own list, read live. Pagis keeps no pool.
    let (status, body) = desk
        .get("/api/v1/settings/phone-numbers/available?country=US&area_code=415")
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["items"][0]["e164"], "+14155550123");
    assert_eq!(body["items"][0]["monthly_cost"], "1.00");

    // Buying from Robin's page assigns the line to Robin.
    let (status, number) = desk.buy("+14155550123", Some(&robin)).await;
    assert_eq!(status, 201, "{number}");
    assert_eq!(number["status"], "assigned");
    assert_eq!(number["agent_id"], robin);
    // The carrier handle never leaves the daemon.
    assert!(number.get("provider_number_id").is_none());
    let number_id = number["id"].as_str().unwrap().to_string();

    // One Agent holds one number, so the move is two acts.
    let (status, refused) = desk
        .post(
            &format!("/api/v1/settings/phone-numbers/{number_id}/assign"),
            serde_json::json!({ "agent_id": wren }),
        )
        .await;
    assert_eq!(status, 409, "{refused}");

    let (status, unassigned) = desk
        .post(
            &format!("/api/v1/settings/phone-numbers/{number_id}/unassign"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200, "{unassigned}");
    assert_eq!(unassigned["status"], "unassigned");
    assert_eq!(unassigned["agent_id"], serde_json::Value::Null);

    let (status, moved) = desk
        .post(
            &format!("/api/v1/settings/phone-numbers/{number_id}/assign"),
            serde_json::json!({ "agent_id": wren }),
        )
        .await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["agent_id"], wren);

    let (status, released) = desk
        .post(
            &format!("/api/v1/settings/phone-numbers/{number_id}/release"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200, "{released}");
    assert_eq!(released["status"], "released");
    assert!(desk.catalog.held().is_empty());

    // The record stays, for the Calls that point at it.
    let (status, page) = desk.get("/api/v1/settings/phone-numbers").await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["items"][0]["status"], "released");
    assert_eq!(page["items"][0]["e164"], "+14155550123");
    assert_eq!(page["carrier"]["status"], "connected");
}

/// A number the account already holds is adopted, not bought:
/// the daemon asks the carrier whether the account has it, records it,
/// and orders nothing.
#[tokio::test]
async fn a_number_the_account_already_holds_is_adopted_not_bought() {
    let desk = Desk::start().await;
    desk.connect_carrier().await;
    let robin = desk.daemon.agent_id.clone();

    // A number the account does not hold is refused with the reason.
    let (status, refused) = desk.adopt("+14155550199", None).await;
    assert_eq!(status, 422, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("does not hold"),
        "{refused}"
    );

    desk.catalog.hold("+14155550199");
    let (status, spare) = desk.adopt("+1 (415) 555-0199", None).await;
    assert_eq!(status, 201, "{spare}");
    assert_eq!(spare["e164"], "+14155550199");
    assert_eq!(spare["status"], "unassigned");
    assert!(spare.get("provider_number_id").is_none());
    assert!(
        !desk
            .catalog
            .calls()
            .iter()
            .any(|call| matches!(call, CarrierCall::Buy { .. }))
    );

    // Pagis holds one live record per number.
    let (status, twice) = desk.adopt("+14155550199", None).await;
    assert_eq!(status, 409, "{twice}");

    // Adopted from an Agent's page, the line is the Agent's at once.
    desk.catalog.hold("+14155550198");
    let (status, held) = desk.adopt("+14155550198", Some(&robin)).await;
    assert_eq!(status, 201, "{held}");
    assert_eq!(held["status"], "assigned");
    assert_eq!(held["agent_id"], robin);

    let (status, page) = desk.get("/api/v1/settings/phone-numbers").await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn buying_needs_a_carrier_at_connected() {
    let desk = Desk::start().await;

    let (status, refused) = desk.buy("+14155550123", None).await;

    assert_eq!(status, 409, "{refused}");
    let (status, page) = desk.get("/api/v1/settings/phone-numbers").await;
    assert_eq!(status, 200);
    assert_eq!(page["carrier"], serde_json::Value::Null);
    assert!(desk.catalog.calls().is_empty());
}

#[tokio::test]
async fn archiving_an_agent_takes_its_line_back() {
    let desk = Desk::start().await;
    desk.connect_carrier().await;
    let wren = desk.create_agent("Wren").await;
    let (status, number) = desk.buy("+14155550123", Some(&wren)).await;
    assert_eq!(status, 201, "{number}");

    let (status, archived) = desk
        .post(
            &format!("/api/v1/agents/{wren}/archive"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200, "{archived}");

    let (_, page) = desk.get("/api/v1/settings/phone-numbers").await;
    assert_eq!(page["items"][0]["status"], "unassigned");
    assert_eq!(page["items"][0]["agent_id"], serde_json::Value::Null);
}

#[tokio::test]
async fn the_carrier_stays_while_it_carries_a_number() {
    let desk = Desk::start().await;
    desk.connect_carrier().await;
    let (status, number) = desk.buy("+14155550123", None).await;
    assert_eq!(status, 201, "{number}");
    let number_id = number["id"].as_str().unwrap().to_string();

    assert_eq!(desk.remove_carrier().await, 409);

    desk.post(
        &format!("/api/v1/settings/phone-numbers/{number_id}/release"),
        serde_json::json!({}),
    )
    .await;

    assert_eq!(desk.remove_carrier().await, 200);
}

#[tokio::test]
async fn a_purchase_that_outlived_the_daemon_is_reconciled_once() {
    let catalog = Arc::new(FakeNumberCatalog::offering(&["+14155550123"]));
    // One secret store across the restart, as the platform keychain is.
    let secrets: Arc<dyn pagis_core::SecretStore> =
        Arc::new(pagis_core::MemorySecretStore::default());
    let options = || TestDaemonOptions {
        number_catalog: Arc::clone(&catalog) as _,
        secrets: Arc::clone(&secrets),
        ..TestDaemonOptions::default()
    };
    let desk = Desk {
        daemon: TestDaemon::start_with(options()).await,
        catalog: Arc::clone(&catalog),
    };
    desk.connect_carrier().await;
    let robin = desk.daemon.agent_id.clone();
    // The carrier sells the number and the answer never arrives.
    catalog.swallow_next_purchase();
    let (status, _) = desk.buy("+14155550123", Some(&robin)).await;
    assert_eq!(status, 422);
    let (_, page) = desk.get("/api/v1/settings/phone-numbers").await;
    assert_eq!(page["items"].as_array().unwrap().len(), 0);

    // The daemon restarts on the same database.
    let daemon = desk.daemon.restart(options()).await;
    let desk = Desk {
        daemon,
        catalog: Arc::clone(&catalog),
    };

    let page = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let (_, page) = desk.get("/api/v1/settings/phone-numbers").await;
            if !page["items"].as_array().unwrap().is_empty() {
                return page;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the intent is reconciled");

    assert_eq!(page["items"][0]["e164"], "+14155550123");
    assert_eq!(page["items"][0]["agent_id"], robin);
    // The carrier was asked to sell one time, and only one.
    let orders = desk
        .catalog
        .calls()
        .into_iter()
        .filter(|call| matches!(call, pagis_telephony::fake::CarrierCall::Buy { .. }))
        .count();
    assert_eq!(orders, 1);
}

// The carrier's line (ADR-0020): one line registers the carrier's SIP
// credential for every number, and the Agent's page shows where the
// line stands.

impl Desk {
    /// The number as the page shows it once its line settles.
    async fn settled_number(&self, e164: &str) -> serde_json::Value {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let (_, page) = self.get("/api/v1/settings/phone-numbers").await;
                let number = page["items"]
                    .as_array()
                    .and_then(|items| items.iter().find(|item| item["e164"] == e164))
                    .cloned();
                if let Some(number) = number
                    && number["registration"] != "registering"
                {
                    return number;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the line settles")
    }

    /// The number once the carrier's line exists and settled. After a
    /// restart the line starts off the boot path, so the first read can
    /// be early and show no line (`unregistered`); this waits for the
    /// line itself.
    async fn number_with_line(&self, e164: &str) -> serde_json::Value {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let number = self.settled_number(e164).await;
                if number["registration"] != serde_json::Value::Null
                    && number["registration"] != "unregistered"
                {
                    return number;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the line starts")
    }
}

#[tokio::test]
async fn a_held_number_registers_once_the_carrier_has_a_sip_credential() {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let desk = Desk {
        daemon: TestDaemon::start_with(TestDaemonOptions {
            call_transport: Arc::clone(&transport) as _,
            ..TestDaemonOptions::default()
        })
        .await,
        catalog: Arc::new(FakeNumberCatalog::default()),
    };
    desk.connect_carrier().await;
    let robin = desk.daemon.agent_id.clone();
    let (status, _) = desk.buy("+14155550123", Some(&robin)).await;
    assert_eq!(status, 201);

    // No SIP credential yet: the page says so, and the carrier was
    // asked nothing.
    let number = desk.number_with_line("+14155550123").await;
    assert_eq!(number["registration"], "failed");
    assert_eq!(number["registration_failure"], "no_credential");
    assert!(transport.calls().is_empty());

    let (status, setup) = desk.set_sip_sign_in().await;
    assert_eq!(status, 200, "{setup}");
    assert_eq!(
        setup["parts"][1]["facts"][0]["value"],
        "robin@sip.telnyx.com"
    );
    assert!(!setup.to_string().contains("sip-secret"));

    let number = desk.number_with_line("+14155550123").await;
    assert_eq!(number["registration"], "registered");
    assert_eq!(number["registration_failure"], serde_json::Value::Null);
    assert!(transport.is_registered("robin"));

    // An unassigned number shows no line, and the carrier's line stays.
    let number_id = number["id"].as_str().unwrap();
    let (status, unassigned) = desk
        .post(
            &format!("/api/v1/settings/phone-numbers/{number_id}/unassign"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, 200, "{unassigned}");
    assert_eq!(unassigned["registration"], serde_json::Value::Null);
    assert!(transport.is_registered("robin"));
}

#[tokio::test]
async fn the_daemon_registers_held_numbers_when_it_starts() {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let secrets: Arc<dyn pagis_core::SecretStore> =
        Arc::new(pagis_core::MemorySecretStore::default());
    let options = || TestDaemonOptions {
        call_transport: Arc::clone(&transport) as _,
        secrets: Arc::clone(&secrets),
        ..TestDaemonOptions::default()
    };
    let desk = Desk {
        daemon: TestDaemon::start_with(options()).await,
        catalog: Arc::new(FakeNumberCatalog::default()),
    };
    desk.connect_carrier().await;
    desk.set_sip_sign_in().await;
    let robin = desk.daemon.agent_id.clone();
    desk.buy("+14155550123", Some(&robin)).await;
    desk.buy("+14155550124", None).await;
    desk.number_with_line("+14155550123").await;

    let desk = Desk {
        daemon: desk.daemon.restart(options()).await,
        catalog: Arc::new(FakeNumberCatalog::default()),
    };

    let held = desk.number_with_line("+14155550123").await;
    assert_eq!(held["registration"], "registered");
    let spare = desk.settled_number("+14155550124").await;
    assert_eq!(spare["registration"], serde_json::Value::Null);
}
