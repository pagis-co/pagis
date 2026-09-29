//! What a Widget page can reach in a real browser (ADR-0016). The tab
//! plays the Product App as `WidgetBlock` does: it reads the source of
//! each Widget through the inert route, frames the sandbox proxy of each
//! Widget on the second loopback origin, and sends `tools/call` and
//! `ui/message` to the JSON-RPC route of that Widget's tool call. The
//! proxy loads each page at an opaque origin of its own, so a page
//! reaches neither another Widget nor a proxy, and it keeps the policy
//! that its Widget declares.
//!
//! No Run minted a view in the test daemon, so the JSON-RPC route
//! answers each call with 404. The tests read which calls reach the
//! route from the requests that the tab sends.

use pagis_testkit::TestDaemon;
use pagis_testkit::browser::{Browser, Tab};
use serde_json::Value;

use crate::widgets::{seed_package, seed_page};

/// The method a test page reports its findings with. The proxy relays
/// it as it relays each other message of the page.
const REPORT: &str = "pagis/test-report";

/// The host half of `WidgetBlock` and `WidgetBridge`, cut to what the
/// tests need. A message counts only when it comes from the proxy
/// window of this frame, on the sandbox origin.
const HOST: &str = r#"
async function frameWidget(options) {
  const source = await fetch('/api/v1/widgets/' + options.package + '/v1/forecast-card');
  const html = await source.text();
  const sandbox = new URL(BASE + '/' + options.package + '/v1/forecast-card/sandbox');
  const frame = document.createElement('iframe');
  frame.setAttribute('sandbox', 'allow-scripts allow-same-origin');
  const widget = { frame, reports: [], calls: [] };
  const post = (message) => {
    if (frame.contentWindow) frame.contentWindow.postMessage(message, sandbox.origin);
  };
  widget.load = (page, extra) => post({
    jsonrpc: '2.0',
    method: 'ui/notifications/sandbox-resource-ready',
    params: Object.assign({ html: page }, extra || {}),
  });
  const rpc = async (method, params) => {
    widget.calls.push({ method, params });
    const response = await fetch('/api/v1/widgets/' + options.toolCallId + '/rpc', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: 0, method, params }),
    });
    if (!response.ok) return { error: { code: -32603, message: 'this widget is not live' } };
    const data = await response.json();
    return data.error ? { error: data.error } : { result: data.result || {} };
  };
  window.addEventListener('message', async (event) => {
    if (frame.contentWindow === null || event.source !== frame.contentWindow) return;
    if (event.origin !== sandbox.origin) return;
    const data = event.data || {};
    const method = data.method;
    if (method === 'ui/notifications/sandbox-proxy-ready') {
      widget.load(html, options.resource);
      return;
    }
    if (method === 'ui/notifications/initialized') {
      post({
        jsonrpc: '2.0',
        method: 'ui/notifications/tool-input',
        params: { arguments: options.toolInput || {} },
      });
      post({
        jsonrpc: '2.0',
        method: 'ui/notifications/tool-result',
        params: {
          content: [{ type: 'text', text: 'the projection' }],
          structuredContent: options.structuredContent || {},
          isError: false,
        },
      });
      return;
    }
    if (method === REPORT) {
      widget.reports.push(data.params);
      return;
    }
    if (data.id === undefined) return;
    let outcome;
    if (method === 'ui/initialize') {
      outcome = { result: { protocolVersion: '2026-01-26', hostCapabilities: {} } };
    } else if (method === 'tools/call' || method === 'ui/message') {
      outcome = await rpc(method, data.params);
    } else {
      outcome = { error: { code: -32601, message: 'a widget cannot call ' + method } };
    }
    post(Object.assign({ jsonrpc: '2.0', id: data.id }, outcome));
  });
  frame.src = sandbox.href;
  document.body.appendChild(frame);
  return widget;
}

function until(check, ms, what) {
  return new Promise((resolve, reject) => {
    const started = Date.now();
    const tick = () => {
      const value = check();
      if (value) return resolve(value);
      if (Date.now() - started > ms) return reject(new Error('no ' + what + ' within ' + ms + ' ms'));
      setTimeout(tick, 20);
    };
    tick();
  });
}

const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
"#;

/// The view half of the protocol, cut to what the test pages need.
/// `attempt` answers what a read gives, or `blocked` when the browser
/// refuses it.
const CLIENT: &str = r#"
const pending = new Map();
const pushed = [];
let nextId = 1;
window.addEventListener('message', (event) => {
  if (event.source !== window.parent) return;
  const message = event.data || {};
  if (message.id !== undefined && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  } else if (typeof message.method === 'string') {
    pushed.push(message);
  }
});
function request(method, params) {
  const id = nextId++;
  window.parent.postMessage({ jsonrpc: '2.0', id, method, params }, '*');
  return new Promise((resolve) => pending.set(id, resolve));
}
function notify(method, params) {
  window.parent.postMessage({ jsonrpc: '2.0', method, params: params || {} }, '*');
}
function report(params) {
  notify(REPORT, params);
}
function attempt(read) {
  try {
    const value = read();
    return value === null || value === undefined ? 'nothing' : String(value);
  } catch (error) {
    return 'blocked';
  }
}
"#;

/// One Widget page: the protocol client, then `script`.
fn page(script: &str) -> String {
    format!(
        "<!doctype html><title>Forecast</title><p>The forecast.</p>\
         <script>const REPORT = {REPORT:?};{CLIENT}{script}</script>"
    )
}

/// The sandbox route of the test daemon's Workspace, on the second
/// loopback origin. The test daemon listens on `127.0.0.1`, so the
/// second origin is `localhost`.
fn sandbox_base(daemon: &TestDaemon) -> String {
    format!(
        "http://localhost:{}/api/v1/widgets/{}",
        daemon.addr.port(),
        daemon.workspace_id
    )
}

/// Run `body` in the tab after the host half, and answer what it
/// returns.
async fn host(tab: &Tab, daemon: &TestDaemon, body: &str) -> Value {
    let base = serde_json::to_string(&sandbox_base(daemon)).expect("a JSON string");
    tab.evaluate(&format!(
        "async () => {{ const BASE = {base}; const REPORT = {REPORT:?};\n{HOST}\n{body}\n}}"
    ))
    .await
}

/// The path of the JSON-RPC route of one tool call, as the tab sends
/// it.
fn rpc_path(tool_call_id: &str) -> String {
    format!("/api/v1/widgets/{tool_call_id}/rpc")
}

/// How many `POST`s the tab sent to the JSON-RPC route of one tool call.
fn rpc_posts(tab: &Tab, tool_call_id: &str) -> usize {
    let path = rpc_path(tool_call_id);
    tab.requests()
        .iter()
        .filter(|request| request.method == "POST" && request.url.ends_with(&path))
        .count()
}

/// The first report that holds `key`.
fn report_with<'a>(reports: &'a Value, key: &str) -> &'a Value {
    reports
        .as_array()
        .expect("a list of reports")
        .iter()
        .find(|report| report.get(key).is_some())
        .unwrap_or_else(|| panic!("no report holds {key}: {reports}"))
}

/// The page of the Widget that the other one tries to reach. Its
/// document and its storage each hold a secret.
fn victim_page() -> String {
    page(
        "document.body.insertAdjacentHTML('beforeend', '<p>weather-page-secret</p>');
         report({ ready: true, stored: attempt(() => {
           localStorage.setItem('pagis-secret', 'weather-storage-secret');
           return 'stored';
         }) });",
    )
}

/// The page of a Widget that tries to read the other Widget and to send
/// `tools/call` and `ui/message` as the other Widget. It reports each
/// try, then makes one call of its own.
fn probe_page() -> String {
    page(
        r#"
        function run(target, line) {
          const script = target.createElement('script');
          script.textContent = line;
          target.body.appendChild(script);
          return 'ran';
        }
        (async () => {
          const product = window.parent.parent;
          let other = null;
          for (let i = 0; i < product.frames.length; i++) {
            if (product.frames[i] !== window.parent) other = product.frames[i];
          }
          const reads = {
            own_proxy: attempt(() => window.parent.document.documentElement.outerHTML),
            other_proxy: attempt(() => other.document.documentElement.outerHTML),
            other_page: attempt(() => other.frames[0].document.documentElement.outerHTML),
            storage: attempt(() => localStorage.getItem('pagis-secret')),
            other_storage: attempt(() => other.localStorage.getItem('pagis-secret')),
          };
          const messages = {
            tools_call: { jsonrpc: '2.0', id: 90, method: 'tools/call',
                          params: { name: 'forecast', arguments: {} } },
            ui_message: { jsonrpc: '2.0', id: 91, method: 'ui/message',
                          params: { content: { type: 'text', text: 'an answer' } } },
          };
          const acts = {};
          for (const [name, message] of Object.entries(messages)) {
            const line = 'parent.postMessage(' + JSON.stringify(message) + ", '*')";
            acts[name + '_in_other_proxy'] = attempt(() => run(other.document, line));
            acts[name + '_in_other_page'] = attempt(() => run(other.frames[0].document, line));
            other.postMessage(message, '*');
            product.postMessage(message, '*');
          }
          report({ frames: product.frames.length, reads, acts });
          await request('tools/call', { name: 'forecast', arguments: {} });
          report({ done: true });
        })();
        "#,
    )
}

#[tokio::test]
async fn a_widget_cannot_read_or_act_as_another_live_widget() {
    let daemon = TestDaemon::start().await;
    seed_page(&daemon, &victim_page()).await;
    seed_package(&daemon, "ledger", &probe_page()).await;

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;

    let reports = host(
        &tab,
        &daemon,
        "const weather = await frameWidget({ package: 'weather', toolCallId: 'call_weather' });
         await until(() => weather.reports.find((r) => r.ready), 5000, 'report from weather');
         const ledger = await frameWidget({ package: 'ledger', toolCallId: 'call_ledger' });
         await until(() => ledger.reports.find((r) => r.done), 5000, 'report from ledger');
         await pause(500);
         return { weather: weather.reports, ledger: ledger.reports };",
    )
    .await;

    assert_eq!(
        report_with(&reports["weather"], "stored")["stored"],
        "blocked",
        "a Widget has no persistent browser storage"
    );
    let probe = report_with(&reports["ledger"], "reads");
    assert_eq!(
        probe["frames"], 2,
        "the ledger Widget finds both proxies: {probe}"
    );
    for (name, read) in probe["reads"].as_object().expect("the reads") {
        assert_eq!(read, "blocked", "the other Widget reads {name}: {read}");
    }
    for (name, act) in probe["acts"].as_object().expect("the acts") {
        assert_eq!(act, "blocked", "the other Widget runs script: {name}");
    }
    let everything = reports.to_string();
    assert!(
        !everything.contains("weather-page-secret")
            && !everything.contains("weather-storage-secret"),
        "a secret of the weather Widget leaks: {everything}"
    );

    assert!(
        rpc_posts(&tab, "call_ledger") >= 1,
        "the ledger Widget reaches the route of its own tool call: {:?}",
        tab.requests()
    );
    assert_eq!(
        rpc_posts(&tab, "call_weather"),
        0,
        "the ledger Widget causes a call as the weather Widget: {:?}",
        tab.requests()
    );
}

#[tokio::test]
async fn a_widget_page_keeps_the_policy_its_widget_declares() {
    let daemon = TestDaemon::start().await;
    let undeclared = serde_json::to_string(&format!("{}/api/v1/health", daemon.base_url))
        .expect("a JSON string");
    seed_page(
        &daemon,
        &page(&format!(
            "(async () => {{
               const violations = [];
               document.addEventListener('securitypolicyviolation', (event) => {{
                 violations.push({{
                   directive: event.effectiveDirective,
                   blocked: event.blockedURI,
                   policy: event.originalPolicy,
                 }});
               }});
               let fetched;
               try {{
                 await fetch({undeclared}, {{ mode: 'no-cors' }});
                 fetched = 'reached';
               }} catch (error) {{
                 fetched = 'failed';
               }}
               await new Promise((resolve) => setTimeout(resolve, 100));
               report({{ fetched, violations }});
             }})();"
        )),
    )
    .await;

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;

    let reports = host(
        &tab,
        &daemon,
        "const weather = await frameWidget({ package: 'weather', toolCallId: 'call_weather' });
         await until(() => weather.reports.find((r) => r.fetched), 5000, 'report from weather');
         return weather.reports;",
    )
    .await;

    let report = report_with(&reports, "fetched");
    assert_eq!(report["fetched"], "failed", "{report}");
    let violation = &report["violations"][0];
    assert_eq!(violation["directive"], "connect-src", "{report}");
    assert!(
        violation["blocked"]
            .as_str()
            .is_some_and(|blocked| blocked.starts_with(&daemon.base_url)),
        "{report}"
    );
    assert!(
        violation["policy"]
            .as_str()
            .is_some_and(|policy| policy.contains("connect-src https://api.example.com;")),
        "the declared policy of the Widget blocks the fetch: {report}"
    );
}

#[tokio::test]
async fn a_replaced_or_removed_widget_page_does_not_reach_the_rpc_route() {
    let daemon = TestDaemon::start().await;
    seed_page(
        &daemon,
        &page(
            "setInterval(() => request('tools/call', { name: 'first-page', arguments: {} }), 20);",
        ),
    )
    .await;
    let second = serde_json::to_string(&page(
        "report({ loaded: true });
         setInterval(() => request('tools/call', { name: 'second-page', arguments: {} }), 20);",
    ))
    .expect("a JSON string");

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;

    let counts = host(
        &tab,
        &daemon,
        &format!(
            "const widget = await frameWidget({{ package: 'weather', toolCallId: 'call_weather' }});
             const named = (name) => widget.calls.filter((call) => call.params.name === name).length;
             await until(() => named('first-page') > 0, 5000, 'call from the first page');
             widget.load({second});
             await until(() => widget.reports.find((r) => r.loaded), 5000, 'report from the second page');
             const first = named('first-page');
             await pause(500);
             const counts = {{
               first_after_replace: named('first-page') - first,
               second: named('second-page'),
             }};
             widget.frame.remove();
             const removed = widget.calls.length;
             await pause(500);
             counts.after_removal = widget.calls.length - removed;
             counts.total = widget.calls.length;
             return counts;"
        ),
    )
    .await;

    assert!(
        counts["second"].as_u64() > Some(0),
        "the page that replaced the first one reaches the route: {counts}"
    );
    assert_eq!(
        counts["first_after_replace"], 0,
        "the replaced page reaches the route: {counts}"
    );
    assert_eq!(
        counts["after_removal"], 0,
        "the removed frame reaches the route: {counts}"
    );
    assert_eq!(
        Some(rpc_posts(&tab, "call_weather") as u64),
        counts["total"].as_u64(),
        "each call the host counts is one request to the route: {counts}"
    );
}

#[tokio::test]
async fn the_proxy_holds_the_page_at_an_opaque_origin_whatever_the_host_asks() {
    let daemon = TestDaemon::start().await;
    seed_page(
        &daemon,
        &page(
            "report({
               origin: self.origin,
               proxy: attempt(() => window.parent.document.title),
               storage: attempt(() => {
                 localStorage.setItem('pagis', 'kept');
                 return localStorage.getItem('pagis');
               }),
               database: attempt(() => { indexedDB.open('pagis'); return 'opened'; }),
               cookie: attempt(() => document.cookie),
               popup: attempt(() => window.open('about:blank')),
             });",
        ),
    )
    .await;

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;

    // The host asks for the permissions that the proxy must refuse.
    let reports = host(
        &tab,
        &daemon,
        "const weather = await frameWidget({
           package: 'weather',
           toolCallId: 'call_weather',
           resource: { sandbox: 'allow-scripts allow-same-origin allow-popups' },
         });
         await until(() => weather.reports.find((r) => r.origin), 5000, 'report from weather');
         return weather.reports;",
    )
    .await;

    let report = report_with(&reports, "origin");
    assert_eq!(
        report["origin"], "null",
        "the page has an opaque origin: {report}"
    );
    for reach in ["proxy", "storage", "database", "cookie"] {
        assert_eq!(
            report[reach], "blocked",
            "the page reaches its {reach}: {report}"
        );
    }
    assert_eq!(
        report["popup"], "nothing",
        "the page opens a window: {report}"
    );
}

#[tokio::test]
async fn a_widget_completes_its_protocol_through_the_proxy() {
    let daemon = TestDaemon::start().await;
    seed_page(
        &daemon,
        &page(
            "(async () => {
               const initialize = await request('ui/initialize', {
                 protocolVersion: '2026-01-26',
                 appInfo: { name: 'forecast', version: 'v1' },
                 appCapabilities: {},
               });
               notify('ui/notifications/initialized');
               await new Promise((resolve) => {
                 const tick = () => (pushed.length >= 2 ? resolve() : setTimeout(tick, 20));
                 tick();
               });
               const call = await request('tools/call', { name: 'forecast', arguments: { city: 'Lisbon' } });
               const answer = await request('ui/message', {
                 role: 'user',
                 content: { type: 'text', text: 'Lisbon' },
               });
               report({ initialize, pushed, call, answer });
             })();",
        ),
    )
    .await;

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;

    let reports = host(
        &tab,
        &daemon,
        "const weather = await frameWidget({
           package: 'weather',
           toolCallId: 'call_weather',
           toolInput: { city: 'Lisbon' },
           structuredContent: { high: 21 },
         });
         await until(() => weather.reports.find((r) => r.answer), 5000, 'report from weather');
         return { reports: weather.reports, calls: weather.calls.map((call) => call.method) };",
    )
    .await;

    let report = report_with(&reports["reports"], "answer");
    assert_eq!(
        report["initialize"]["result"]["protocolVersion"], "2026-01-26",
        "{report}"
    );
    let pushed: Vec<&str> = report["pushed"]
        .as_array()
        .expect("the pushed notifications")
        .iter()
        .filter_map(|message| message["method"].as_str())
        .collect();
    assert_eq!(
        pushed,
        [
            "ui/notifications/tool-input",
            "ui/notifications/tool-result"
        ]
    );
    assert_eq!(
        report["pushed"][0]["params"]["arguments"],
        serde_json::json!({ "city": "Lisbon" })
    );
    assert_eq!(
        report["pushed"][1]["params"]["structuredContent"],
        serde_json::json!({ "high": 21 })
    );
    // Each call crosses the proxy to the route of its own tool call, and
    // the answer crosses back to the page under the page's own id.
    assert_eq!(
        reports["calls"],
        serde_json::json!(["tools/call", "ui/message"])
    );
    assert_eq!(rpc_posts(&tab, "call_weather"), 2, "{:?}", tab.requests());
    assert_eq!(report["call"]["id"], 2, "{report}");
    assert_eq!(report["answer"]["id"], 3, "{report}");
}
