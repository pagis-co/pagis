//! What a signed-in browser does with an Artifact URL and with the
//! raw Widget URL. The author of hostile content steers an Agent to
//! link to such a file, and the Person clicks the link in their own
//! conversation. The browser must save the file and never run it as a
//! page at the Product App origin, where its script would send requests
//! with the Person's Session. Only the sandbox proxy, on the second
//! loopback origin, runs Widget HTML (ADR-0016).

use pagis_testkit::TestDaemon;
use pagis_testkit::browser::{Browser, Outcome, PROBE};

use crate::artifacts::upload;
use crate::widgets::seed_page;

/// The script of each hostile file: it reports that it runs, then reads
/// the Workspace with the Session of the Person who opens it.
fn hostile_script(name: &str) -> String {
    format!("if (window.{PROBE}) {PROBE}('{name}'); fetch('/api/v1/requests');")
}

fn hostile_html() -> String {
    format!(
        "<!doctype html><title>Invoice</title><script>{}</script>",
        hostile_script("html")
    )
}

fn hostile_svg() -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\">\
         <script>{}</script></svg>",
        hostile_script("svg")
    )
}

/// A Widget page that acts as the hostile one does when a browser runs
/// it at the Product App origin, and speaks the MCP Apps protocol when
/// it runs in the sandbox proxy. The Widget's own policy blocks
/// `fetch`, so the page reads through an image, which `img-src 'self'`
/// allows.
fn hostile_widget() -> String {
    format!(
        "<!doctype html><title>Forecast</title><p>The forecast.</p><script>\
         if (window.{PROBE}) {PROBE}('widget');\
         new Image().src = '/api/v1/requests';\
         parent.postMessage({{ jsonrpc: '2.0', id: 1, method: 'ui/initialize', params: {{}} }}, '*');\
         </script>"
    )
}

/// The Artifact URL of one uploaded file.
async fn artifact_url(daemon: &TestDaemon, filename: &str, mime: &str, body: String) -> String {
    let artifact: serde_json::Value = upload(daemon, filename, mime, body.into_bytes())
        .await
        .json()
        .await
        .expect("the Artifact");
    format!(
        "{}/api/v1/artifacts/{}",
        daemon.base_url,
        artifact["id"].as_str().expect("an Artifact id")
    )
}

#[tokio::test]
async fn a_signed_in_browser_runs_no_script_from_an_artifact_or_widget_url() {
    let daemon = TestDaemon::start().await;
    let html = artifact_url(&daemon, "invoice.html", "text/html", hostile_html()).await;
    let svg = artifact_url(&daemon, "chart.svg", "image/svg+xml", hostile_svg()).await;
    seed_page(&daemon, &hostile_widget()).await;
    let widget = format!(
        "{}/api/v1/widgets/weather/v1/forecast-card",
        daemon.base_url
    );
    let api = format!("{}/api/", daemon.base_url);

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;

    for url in [html, svg, widget] {
        let opened = tab.follow(&url).await;

        assert!(
            opened.probes.is_empty(),
            "a script runs from {url}: {:?}",
            opened.probes
        );
        let to_the_api: Vec<&str> = opened
            .requests
            .iter()
            .map(|request| request.url.as_str())
            .filter(|sent| sent.starts_with(&api))
            .collect();
        assert_eq!(
            to_the_api,
            [url.as_str()],
            "the link is the one request to the API"
        );
        assert_eq!(
            opened.outcome,
            Outcome::Downloaded,
            "the browser saves {url} as a file"
        );
    }
}
