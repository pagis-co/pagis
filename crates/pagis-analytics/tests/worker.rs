//! The analytics task against a fake PostHog: what goes, when, and what
//! a failure or the System Setting does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pagis_analytics::{
    Analytics, Bucket, Features, InstallationKind, Ledger, Project, Report, Source, StorageBackend,
};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NOW: i64 = 1_800_000_000_000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

struct FakeSource {
    enabled: AtomicBool,
}

#[async_trait::async_trait]
impl Source for FakeSource {
    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    async fn report(&self) -> anyhow::Result<Report> {
        Ok(Report {
            installation: InstallationKind::Server,
            storage: StorageBackend::Postgres,
            multi_user: true,
            computers: false,
            people: Bucket::Few,
            agents: Bucket::Some,
            features: Features::default(),
        })
    }
}

struct Fixture {
    posthog: MockServer,
    source: Arc<FakeSource>,
    home: tempfile::TempDir,
}

impl Fixture {
    async fn new(status: u16) -> Self {
        let posthog = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/batch/"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&posthog)
            .await;
        Self {
            posthog,
            source: Arc::new(FakeSource {
                enabled: AtomicBool::new(true),
            }),
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn analytics(&self, release: &str) -> Analytics {
        let project = Project {
            id: "12345".to_string(),
            token: "phc_test".to_string(),
            host: self.posthog.uri(),
        };
        Analytics::new(
            project,
            Arc::clone(&self.source) as _,
            self.home.path().to_path_buf(),
            release,
        )
        .unwrap()
    }

    /// Every batch PostHog received, oldest first.
    async fn batches(&self) -> Vec<serde_json::Value> {
        self.posthog
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect()
    }
}

fn event_names(batch: &serde_json::Value) -> Vec<&str> {
    batch["batch"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn the_first_check_sends_the_created_event_and_a_report_under_the_installation_id() {
    let fixture = Fixture::new(200).await;

    assert!(fixture.analytics("0.1.0").check(NOW).await.unwrap());

    let batches = fixture.batches().await;
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    assert_eq!(batch["api_key"], "phc_test");
    assert_eq!(
        event_names(batch),
        ["installation_created", "installation_report"]
    );
    let ledger = Ledger::read(fixture.home.path()).unwrap().unwrap();
    for event in batch["batch"].as_array().unwrap() {
        assert_eq!(event["distinct_id"], ledger.installation_id.as_str());
        assert_eq!(event["properties"]["release"], "0.1.0");
        assert_eq!(event["properties"]["$process_person_profile"], false);
        assert_eq!(event["properties"]["$geoip_disable"], true);
    }
    let report = &batch["batch"][1]["properties"];
    assert_eq!(report["installation"], "server");
    assert_eq!(report["people"], "2-5");
    assert_eq!(ledger.reported_release.as_deref(), Some("0.1.0"));
    assert_eq!(ledger.last_report_at, Some(NOW));
}

#[tokio::test]
async fn nothing_goes_again_until_a_day_passes_or_the_release_changes() {
    let fixture = Fixture::new(200).await;
    fixture.analytics("0.1.0").check(NOW).await.unwrap();

    assert!(!fixture.analytics("0.1.0").check(NOW + 1).await.unwrap());
    assert!(
        fixture
            .analytics("0.1.0")
            .check(NOW + DAY_MS)
            .await
            .unwrap()
    );
    assert!(
        fixture
            .analytics("0.2.0")
            .check(NOW + DAY_MS + 1)
            .await
            .unwrap()
    );

    let batches = fixture.batches().await;
    assert_eq!(batches.len(), 3);
    assert_eq!(event_names(&batches[1]), ["installation_report"]);
    assert_eq!(event_names(&batches[2]), ["installation_upgraded"]);
    assert_eq!(
        batches[2]["batch"][0]["properties"]["from_release"],
        "0.1.0"
    );
    // One installation, one ID, across every batch.
    let ids: Vec<&serde_json::Value> = batches
        .iter()
        .flat_map(|batch| batch["batch"].as_array().unwrap())
        .map(|event| &event["distinct_id"])
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] == pair[1]), "{ids:?}");
}

#[tokio::test]
async fn with_the_setting_off_nothing_goes_and_nothing_is_written() {
    let fixture = Fixture::new(200).await;
    fixture.source.enabled.store(false, Ordering::SeqCst);

    assert!(!fixture.analytics("0.1.0").check(NOW).await.unwrap());

    assert!(fixture.batches().await.is_empty());
    assert_eq!(Ledger::read(fixture.home.path()).unwrap(), None);
}

#[tokio::test]
async fn a_batch_that_posthog_refuses_goes_again_at_the_next_check_with_the_same_id() {
    let fixture = Fixture::new(503).await;

    assert!(fixture.analytics("0.1.0").check(NOW).await.is_err());
    let ledger = Ledger::read(fixture.home.path()).unwrap().unwrap();
    assert_eq!(ledger.reported_release, None);

    fixture.posthog.reset().await;
    Mock::given(method("POST"))
        .and(path("/batch/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&fixture.posthog)
        .await;
    assert!(fixture.analytics("0.1.0").check(NOW + 1).await.unwrap());

    let batches = fixture.batches().await;
    assert_eq!(
        event_names(&batches[0]),
        ["installation_created", "installation_report"]
    );
    assert_eq!(
        batches[0]["batch"][0]["distinct_id"],
        ledger.installation_id.as_str()
    );
}

/// The task runs on its own: the first check waits, and a PostHog that
/// does not answer holds up nothing but the task.
#[tokio::test]
async fn the_spawned_task_sends_after_the_first_wait_and_stops_on_cancel() {
    let fixture = Fixture::new(200).await;
    let cancel = CancellationToken::new();

    fixture
        .analytics("0.1.0")
        .with_timing(Duration::from_millis(50), Duration::from_secs(3600))
        .spawn(cancel.clone());
    assert!(fixture.batches().await.is_empty());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while fixture.batches().await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the task sent nothing"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cancel.cancel();
    assert_eq!(fixture.batches().await.len(), 1);
}
