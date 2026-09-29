//! The fixture source of the release evaluation: a deterministic
//! clock and a Gmail provider that serves fixture evidence through the
//! shipped `gog` seam. One clock decides both the time the source
//! stamps and the moment an item starts to exist, so no evidence is
//! visible before its acquisition point.

use std::sync::{
    Arc,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

use base64::Engine;
use chrono::TimeZone;
use pagis_evaluation::Evidence;
use pagis_google::{GogCommand, GogRunner, ProcessFailure, ProcessOutput};
use serde_json::{Value, json};

/// The mail form of one fixture evidence item.
#[derive(Clone, Debug)]
pub struct MailItem {
    /// The Gmail message and thread id.
    pub id: String,
    /// The declared source version, served as the Gmail history id.
    pub version: String,
    /// The declared source identity.
    pub source_id: String,
    /// The declared source reference.
    pub source_ref: String,
    /// The declared occurrence time, served as `internalDate`.
    pub occurred_at: i64,
    /// The declared valid time, served in a header.
    pub valid_at: i64,
    /// The acquisition point. The source serves nothing before it.
    pub acquired_at: i64,
    /// The declared civil zone of the chronology.
    pub zone: String,
    pub text: String,
}

/// The clock the driver advances. It never moves back: a fixture whose
/// probe time precedes its own evidence keeps the later point, because
/// no daemon runs in the past.
#[derive(Debug)]
struct FixtureTime {
    now: AtomicI64,
    changed: tokio::sync::Notify,
}

#[derive(Clone, Debug)]
pub struct FixtureClock(Arc<FixtureTime>);

impl FixtureClock {
    pub fn at(millis: i64) -> Self {
        Self(Arc::new(FixtureTime {
            now: AtomicI64::new(millis),
            changed: tokio::sync::Notify::new(),
        }))
    }

    pub fn now_ms(&self) -> i64 {
        self.0.now.load(Ordering::SeqCst)
    }

    pub fn advance_to(&self, millis: i64) {
        if self.0.now.fetch_max(millis, Ordering::SeqCst) < millis {
            self.0.changed.notify_one();
        }
    }
}

/// The daemon reads the same clock as the source, so the
/// acquisition and Schedule passes decide inside the
/// chronology instead of eight months after it.
impl pagis_core::Clock for FixtureClock {
    fn now_ms(&self) -> i64 {
        self.now_ms()
    }

    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.0.changed.notified())
    }
}

/// A Gmail provider over fixture evidence. It answers only the five
/// read methods the shipped sync uses, and it counts every read so the
/// driver can charge source reads to the evaluation ledger.
pub struct FixtureSource {
    clock: FixtureClock,
    items: Vec<MailItem>,
    reads: AtomicU64,
}

impl FixtureSource {
    pub fn new(clock: FixtureClock, items: Vec<MailItem>) -> Self {
        Self {
            clock,
            items,
            reads: AtomicU64::new(0),
        }
    }

    /// Every read this source served.
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }

    /// The items acquired at or before the current fixture time.
    pub fn visible(&self) -> Vec<&MailItem> {
        let now = self.clock.now_ms();
        self.items
            .iter()
            .filter(|item| item.acquired_at <= now)
            .collect()
    }

    fn message(&self, item: &MailItem) -> Value {
        let date = |millis: i64| match item.zone.parse::<chrono_tz::Tz>() {
            Ok(zone) => zone
                .timestamp_millis_opt(millis)
                .single()
                .map(|at| at.to_rfc2822())
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        json!({
            "id": item.id,
            "threadId": item.id,
            "historyId": item.version,
            "internalDate": item.occurred_at.to_string(),
            // The default filter reflects mail Gmail marks important.
            "labelIds": ["INBOX", "IMPORTANT"],
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "From", "value": format!("{} <fixture@fixture.invalid>", item.source_id)},
                    {"name": "To", "value": "owner@fixture.invalid"},
                    {"name": "Subject", "value": item.source_ref},
                    {"name": "Date", "value": date(item.occurred_at)},
                    {"name": "X-Fixture-Source-Version", "value": item.version},
                    {"name": "X-Fixture-Valid-At", "value": date(item.valid_at)},
                    {"name": "X-Fixture-Zone", "value": item.zone},
                ],
                "body": {"data": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&item.text)},
            }
        })
    }

    /// One provider read, counted. The `gog` seam and a direct caller
    /// take the same path, so a test proves the rule the daemon meets.
    pub fn read(&self, method: &str, params: &Value) -> Result<Value, String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let visible = self.visible();
        let highest = visible
            .iter()
            .filter_map(|item| item.version.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        match method {
            "users.getProfile" => Ok(json!({"historyId": highest.to_string()})),
            "users.messages.list" => Ok(json!({
                "messages": visible
                    .iter()
                    .map(|item| json!({"id": item.id, "threadId": item.id}))
                    .collect::<Vec<_>>()
            })),
            "users.messages.get" => {
                let id = params["id"].as_str().unwrap_or_default();
                match visible.iter().find(|item| item.id == id) {
                    Some(item) => Ok(self.message(item)),
                    None => Err(format!("{id} is not acquired yet")),
                }
            }
            "users.threads.get" => {
                let id = params["id"].as_str().unwrap_or_default();
                match visible.iter().find(|item| item.id == id) {
                    Some(item) => Ok(json!({"messages": [self.message(item)]})),
                    None => Err(format!("{id} is not acquired yet")),
                }
            }
            "users.history.list" => {
                let start = params["startHistoryId"]
                    .as_str()
                    .and_then(|id| id.parse::<u64>().ok())
                    .unwrap_or(0);
                let added = visible
                    .iter()
                    .filter(|item| {
                        item.version
                            .parse::<u64>()
                            .is_ok_and(|version| version > start)
                    })
                    .map(|item| {
                        json!({
                            "id": item.version,
                            "messagesAdded": [{"message": {"id": item.id, "threadId": item.id}}]
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(json!({"historyId": highest.max(start).to_string(), "history": added}))
            }
            other => Err(format!("{other} is not a fixture read")),
        }
    }
}

/// The Gmail method and parameters of one `gog api call`.
fn call(command: &GogCommand) -> Option<(String, Value)> {
    let args = command.args();
    let params = args
        .iter()
        .position(|arg| arg == "--params")
        .and_then(|at| args.get(at + 1))
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    let method = args.iter().find(|arg| arg.starts_with("users.")).cloned()?;
    Some((method, params))
}

#[async_trait::async_trait]
impl GogRunner for FixtureSource {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        assert!(
            !command.is_write(),
            "an evaluation never writes to a source"
        );
        let Some((method, params)) = call(command) else {
            self.reads.fetch_add(1, Ordering::SeqCst);
            return Ok(ProcessOutput {
                status: Some(1),
                stdout: b"the fixture source serves Gmail reads only".to_vec(),
            });
        };
        Ok(match self.read(&method, &params) {
            // Status 1 is the provider's temporary failure, which is
            // what an item that does not exist yet must look like.
            Err(reason) => ProcessOutput {
                status: Some(1),
                stdout: reason.into_bytes(),
            },
            Ok(response) => ProcessOutput {
                status: Some(0),
                stdout: response.to_string().into_bytes(),
            },
        })
    }
}

/// The mail items of one chronology, in acquisition order.
///
/// The Gmail history protocol reads versions as increasing history ids,
/// so a fixture whose mail versions are not increasing integers cannot
/// be served through the shipped path. The caller reports that as a
/// missing capability instead of serving a different version.
pub fn mail_items(evidence: &[(usize, &Evidence)], zone: &str) -> Result<Vec<MailItem>, String> {
    let mut items: Vec<MailItem> = Vec::new();
    for (index, item) in evidence {
        let version = item.source_version.parse::<u64>().map_err(|_| {
            format!(
                "integer source versions for the mail history protocol (item {index} has {})",
                item.source_version
            )
        })?;
        if items
            .last()
            .and_then(|last| last.version.parse::<u64>().ok())
            .is_some_and(|last| last >= version)
        {
            return Err(format!(
                "increasing source versions in acquisition order (item {index} has {version})"
            ));
        }
        items.push(MailItem {
            id: format!("fixture-{index}"),
            version: version.to_string(),
            source_id: item.source_id.clone(),
            source_ref: item.source_ref.clone(),
            occurred_at: millis(&item.occurred_at)?,
            valid_at: millis(&item.valid_at)?,
            acquired_at: millis(&item.acquired_at)?,
            zone: zone.to_string(),
            text: item.text.clone(),
        });
    }
    Ok(items)
}

/// One fixture timestamp as unix milliseconds.
pub fn millis(stamp: &str) -> Result<i64, String> {
    chrono::DateTime::parse_from_rfc3339(stamp)
        .map(|at| at.timestamp_millis())
        .map_err(|_| format!("an RFC 3339 fixture time (found {stamp})"))
}
