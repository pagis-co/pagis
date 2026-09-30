//! A headless Chrome that a test drives over the Chrome DevTools
//! Protocol.
//!
//! A test opens a tab that is signed in to a [`TestDaemon`], as a
//! Person is in the Product App, and follows links from it. The tab
//! records what the browser does: each request that the tab sends, each
//! file that the tab saves as a download, and each call of [`PROBE`]
//! that a script makes when it runs.
//!
//! The test needs Chrome or Chromium on the machine. When the launch
//! finds none, the test fails and says so: a browser test never passes
//! without a browser.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::browser::{
    EventDownloadWillBegin, SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};
use chromiumoxide::cdp::browser_protocol::network::{
    CookieParam, CookieSameSite, EventRequestWillBeSent,
};
use chromiumoxide::cdp::browser_protocol::page::{EventFrameNavigated, EventLifecycleEvent};
use chromiumoxide::cdp::js_protocol::runtime::{AddBindingParams, EventBindingCalled};
use chromiumoxide::{BrowserConfig, Page};
use futures::StreamExt as _;
use serde::de::DeserializeOwned;
use tempfile::TempDir;

use crate::TestDaemon;

/// The function that a test page calls when its script runs. Each tab
/// adds it to each document that it opens, so a call proves that the
/// script of that document ran.
pub const PROBE: &str = "pagisProbe";

/// How long the browser has to show or save one link.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// The lifecycle event of a document that has sent its requests: the
/// document loaded, and at most two requests stayed open for 500 ms.
/// Headless Chrome does not always send the stricter `networkIdle`.
const NETWORK_ALMOST_IDLE: &str = "networkAlmostIdle";

/// One headless Chrome with a profile of its own. Parallel tests do not
/// share a profile. Dropping the value stops Chrome and deletes the
/// profile.
pub struct Browser {
    chrome: chromiumoxide::Browser,
    handler: tokio::task::JoinHandle<()>,
    /// The profile and the download folder of this Chrome.
    _profile: TempDir,
}

impl Browser {
    /// Launch headless Chrome. The launch finds Chrome the way
    /// `chromiumoxide` does: the `CHROME` variable, the program names
    /// on the `PATH`, then the usual install folders.
    pub async fn launch() -> Self {
        let profile = tempfile::tempdir().expect("a folder for the browser profile");
        let config = BrowserConfig::builder()
            .new_headless_mode()
            .user_data_dir(profile.path().join("profile"))
            .build()
            .unwrap_or_else(|error| {
                panic!(
                    "the browser tests need Chrome or Chromium, and none is found ({error}). \
                     Install Google Chrome, or set CHROME to the path of a Chrome program."
                )
            });
        let (chrome, mut events) = chromiumoxide::Browser::launch(config)
            .await
            .unwrap_or_else(|error| panic!("Chrome does not start: {error}"));
        let handler = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                // Only a broken connection to Chrome is an error here.
                if event.is_err() {
                    break;
                }
            }
        });
        // A download goes into the profile, never into the Downloads
        // folder of the machine, and the browser reports it.
        chrome
            .execute(
                SetDownloadBehaviorParams::builder()
                    .behavior(SetDownloadBehaviorBehavior::AllowAndName)
                    .download_path(profile.path().join("downloads").display().to_string())
                    .events_enabled(true)
                    .build()
                    .expect("a complete download behavior"),
            )
            .await
            .expect("Chrome accepts the download behavior");
        Self {
            chrome,
            handler,
            _profile: profile,
        }
    }

    /// Open a tab that is signed in to `daemon` as its seeded Person.
    ///
    /// The tab holds the Session cookie with the attributes the daemon
    /// sets, and it starts on a page at the daemon's origin that runs no
    /// script of its own. A link that the test follows from there is a
    /// same-origin navigation, as a click in the Product App is, so the
    /// browser sends the `SameSite=Strict` cookie with it.
    pub async fn signed_in(&self, daemon: &TestDaemon) -> Tab {
        let page = self
            .chrome
            .new_page("about:blank")
            .await
            .expect("Chrome opens a tab");
        let (name, value) = daemon
            .cookie()
            .split_once('=')
            .expect("the Session cookie is name=value");
        page.set_cookie(
            CookieParam::builder()
                .name(name)
                .value(value)
                .url(daemon.base_url.as_str())
                .path("/")
                .http_only(true)
                .same_site(CookieSameSite::Strict)
                .build()
                .expect("a complete cookie"),
        )
        .await
        .expect("Chrome keeps the Session cookie");
        page.execute(AddBindingParams::new(PROBE))
            .await
            .expect("Chrome adds the probe");

        let tab = Tab::record(page, &self.chrome).await;
        tab.page
            .goto(format!("{}/api/v1/health", daemon.base_url))
            .await
            .expect("the tab opens the daemon's origin");
        tab
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.handler.abort();
    }
}

/// What the browser does with a link that a tab follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The browser shows the response as a document in the tab.
    Shown,
    /// The browser saves the response as a file and the tab stays on
    /// its page.
    Downloaded,
}

/// One request that a tab sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentRequest {
    pub method: String,
    pub url: String,
}

/// What one followed link causes.
#[derive(Debug)]
pub struct Opened {
    pub outcome: Outcome,
    /// Each request that the tab sends from the click on, in order.
    /// The first one is the navigation to the link. The browser's own
    /// requests, such as the one for `/favicon.ico`, are in the list too.
    pub requests: Vec<SentRequest>,
    /// The argument of each [`PROBE`] call from the click on.
    pub probes: Vec<String>,
}

/// One tab of a [`Browser`], and the record of what it does.
pub struct Tab {
    page: Page,
    record: Arc<Mutex<Record>>,
    recorder: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Record {
    requests: Vec<SentRequest>,
    probes: Vec<String>,
    /// The URL of each document of the main frame, by its loader.
    documents: HashMap<String, String>,
    /// The URL of each document of the main frame that has sent its
    /// requests.
    settled: Vec<String>,
    /// The URL of each download that the main frame begins.
    downloads: Vec<String>,
}

impl Tab {
    /// Start to record the events of `page`, before it opens anything.
    async fn record(page: Page, chrome: &chromiumoxide::Browser) -> Self {
        let main_frame = page
            .mainframe()
            .await
            .expect("the tab has a main frame")
            .expect("the tab has a main frame");
        let mut requests = page
            .event_listener::<EventRequestWillBeSent>()
            .await
            .expect("listen to requests");
        let mut probes = page
            .event_listener::<EventBindingCalled>()
            .await
            .expect("listen to probe calls");
        let mut navigations = page
            .event_listener::<EventFrameNavigated>()
            .await
            .expect("listen to navigations");
        let mut lifecycle = page
            .event_listener::<EventLifecycleEvent>()
            .await
            .expect("listen to lifecycle events");
        let mut downloads = chrome
            .event_listener::<EventDownloadWillBegin>()
            .await
            .expect("listen to downloads");

        let record = Arc::new(Mutex::new(Record::default()));
        let into = Arc::clone(&record);
        let recorder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = requests.next() => {
                        into.lock().expect("the record").requests.push(SentRequest {
                            method: event.request.method.clone(),
                            url: event.request.url.clone(),
                        });
                    }
                    Some(event) = probes.next() => {
                        if event.name == PROBE {
                            into.lock().expect("the record").probes.push(event.payload.clone());
                        }
                    }
                    Some(event) = navigations.next() => {
                        if event.frame.parent_id.is_none() {
                            into.lock().expect("the record").documents.insert(
                                event.frame.loader_id.inner().clone(),
                                event.frame.url.clone(),
                            );
                        }
                    }
                    Some(event) = lifecycle.next() => {
                        if event.frame_id == main_frame && event.name == NETWORK_ALMOST_IDLE {
                            let mut record = into.lock().expect("the record");
                            if let Some(url) = record.documents.get(event.loader_id.inner()).cloned() {
                                record.settled.push(url);
                            }
                        }
                    }
                    Some(event) = downloads.next() => {
                        if event.frame_id == main_frame {
                            into.lock().expect("the record").downloads.push(event.url.clone());
                        }
                    }
                    else => break,
                }
            }
        });
        Self {
            page,
            record,
            recorder,
        }
    }

    /// Follow a link to `url` from the tab's page, as a Person who
    /// clicks it does. Wait until the browser shows the response and the
    /// document has sent its requests, or until the browser saves the
    /// response as a download.
    pub async fn follow(&self, url: &str) -> Opened {
        let (requests, probes, settled, downloads) = {
            let record = self.record.lock().expect("the record");
            (
                record.requests.len(),
                record.probes.len(),
                record.settled.len(),
                record.downloads.len(),
            )
        };
        let link = serde_json::to_string(url).expect("a URL is a JSON string");
        self.page
            .evaluate_expression(format!("location.href = {link}"))
            .await
            .unwrap_or_else(|error| panic!("the tab does not follow {url}: {error}"));

        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            {
                let record = self.record.lock().expect("the record");
                let outcome = if record.downloads[downloads..].iter().any(|d| d == url) {
                    Some(Outcome::Downloaded)
                } else if record.settled[settled..].iter().any(|s| s == url) {
                    Some(Outcome::Shown)
                } else {
                    None
                };
                if let Some(outcome) = outcome {
                    return Opened {
                        outcome,
                        requests: record.requests[requests..].to_vec(),
                        probes: record.probes[probes..].to_vec(),
                    };
                }
            }
            if Instant::now() >= deadline {
                let record = self.record.lock().expect("the record");
                panic!(
                    "the browser neither shows nor saves {url} within {STEP_TIMEOUT:?}. \
                     Requests: {:?}. Probe calls: {:?}. Documents: {:?}. Settled: {:?}. \
                     Downloads: {:?}.",
                    &record.requests[requests..],
                    &record.probes[probes..],
                    record.documents,
                    &record.settled[settled..],
                    &record.downloads[downloads..],
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Each request that the tab sent so far, in order.
    pub fn requests(&self) -> Vec<SentRequest> {
        self.record.lock().expect("the record").requests.clone()
    }

    /// Run `function`, a JavaScript function, in the tab's page and
    /// answer its value. The tab waits for a promise that it returns.
    pub async fn evaluate<T: DeserializeOwned>(&self, function: &str) -> T {
        self.page
            .evaluate_function(function)
            .await
            .unwrap_or_else(|error| panic!("the script fails in the tab: {error}"))
            .into_value()
            .unwrap_or_else(|error| panic!("the script answers an unexpected value: {error}"))
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.recorder.abort();
    }
}
