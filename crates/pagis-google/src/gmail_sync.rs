//! Gmail's full/history protocol through the pinned authenticated client.
//! Checkpoints contain no message content or credentials.

use base64::Engine;
use pagis_core::knowledge::{SourceChange, SourceOperation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{GogCommand, GogRunner, GoogleProvider, ProviderError, ProviderErrorCode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailPage {
    pub checkpoint: Value,
    pub caught_up: bool,
    pub changes: Vec<SourceChange>,
    pub historical: bool,
    pub arrivals: Vec<pagis_core::NormalizedEvent>,
}

fn invalid() -> ProviderError {
    ProviderError::new(ProviderErrorCode::TemporarilyUnavailable, false)
}

/// One thread body with the source bytes it charges. Decoded content that
/// the text projection discards stays charged; it was read all the same.
#[derive(Debug, Clone)]
pub struct BoundedSourceContent {
    pub content: pagis_core::knowledge::SourceContent,
    pub source_bytes: u32,
}

/// Base64 and JSON envelope make the transport larger than the content it
/// carries. The allowance keeps the read bounded without rejecting a body
/// that fits the caller's budget.
fn transport_cap(max_bytes: usize) -> usize {
    max_bytes.saturating_mul(4).saturating_add(16 * 1024)
}

const THREAD_LIMIT: usize = 128 * 1024;

/// The full scan never lists the mail outside the mailbox, because
/// `users.messages.list` omits it unless it is asked for it.
/// `users.history.list` has no such switch and reports every change of
/// the account, so acquisition holds both phases to the same mailbox.
use crate::gmail_filter::OUTSIDE_THE_MAILBOX;

/// The change that records an id the mailbox does not hold. It carries
/// no metadata, so the id contributes no Page Signals (ADR-0011).
fn outside(id: &str, version: &str) -> SourceChange {
    SourceChange {
        id: id.into(),
        version: version.into(),
        kind: "mail".into(),
        parent: None,
        source_at: None,
        operation: SourceOperation::Deleted,
        metadata: Value::Null,
    }
}

fn label_ids(message: &Value) -> Vec<String> {
    message["labelIds"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl<R: GogRunner> GoogleProvider<R> {
    pub async fn knowledge_thread(
        &self,
        thread: &str,
    ) -> Result<pagis_core::knowledge::SourceContent, ProviderError> {
        let read = self
            .read_thread(thread, transport_cap(THREAD_LIMIT))
            .await?;
        if read
            .content
            .contents
            .iter()
            .map(|item| item.text.len())
            .sum::<usize>()
            > THREAD_LIMIT
        {
            return Err(ProviderError::rejected(
                "the thread text is larger than the thread limit",
            ));
        }
        Ok(read.content)
    }

    /// Reads one thread under a source-byte budget. The transport stops at a
    /// bounded multiple of the budget, so an oversized thread never reaches
    /// memory whole, and a body whose charge exceeds `max_bytes` is refused
    /// instead of silently truncated.
    pub async fn knowledge_thread_bounded(
        &self,
        thread: &str,
        max_bytes: usize,
    ) -> Result<BoundedSourceContent, ProviderError> {
        let read = self.read_thread(thread, transport_cap(max_bytes)).await?;
        if read.source_bytes as usize > max_bytes {
            return Err(ProviderError::rejected(
                "the thread is larger than the byte budget",
            ));
        }
        Ok(read)
    }

    async fn read_thread(
        &self,
        thread: &str,
        max_stdout: usize,
    ) -> Result<BoundedSourceContent, ProviderError> {
        let response = self
            .gmail_read_bounded(
                "users.threads.get",
                json!({"userId":"me", "id":thread, "format":"full"}),
                max_stdout,
            )
            .await?;
        let messages = response["messages"].as_array().ok_or_else(invalid)?;
        if messages.len() > 100 {
            return Err(ProviderError::rejected(
                "the thread has more than 100 messages",
            ));
        }
        let mut items = Vec::new();
        let mut contents = Vec::new();
        let mut source_bytes = 0_usize;
        for message in messages {
            let id = message["id"].as_str().ok_or_else(invalid)?;
            let source_at = message["internalDate"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .ok_or_else(invalid)?;
            items.push(SourceChange {
                id: id.into(),
                version: message["historyId"].as_str().ok_or_else(invalid)?.into(),
                kind: "mail".into(),
                parent: Some(thread.into()),
                source_at: Some(source_at),
                operation: SourceOperation::Upsert,
                // A read is transient. Only acquisition stores metadata.
                metadata: Value::Null,
            });
            let mut body = String::new();
            let mut decoded = 0_usize;
            message_text(&message["payload"], &mut body, 0, &mut decoded)?;
            let text = serde_json::to_string(
                &json!({"source_at":source_at,"headers":message["payload"]["headers"],"body":body}),
            )
            .map_err(|_| invalid())?;
            source_bytes = source_bytes
                .saturating_add(text.len())
                .saturating_add(decoded.saturating_sub(body.len()));
            contents.push(pagis_core::knowledge::SourceText {
                id: id.into(),
                version: message["historyId"].as_str().ok_or_else(invalid)?.into(),
                text,
                native_episode: None,
            });
        }
        Ok(BoundedSourceContent {
            content: pagis_core::knowledge::SourceContent { items, contents },
            source_bytes: source_bytes.min(u32::MAX as usize) as u32,
        })
    }

    /// Only these fixed read methods are reachable from sync.
    pub(crate) async fn gmail_read(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        self.gmail_read_bounded(method, params, usize::MAX).await
    }

    async fn gmail_read_bounded(
        &self,
        method: &str,
        params: Value,
        max_stdout: usize,
    ) -> Result<Value, ProviderError> {
        if !matches!(
            method,
            "users.getProfile"
                | "users.messages.list"
                | "users.history.list"
                | "users.threads.get"
                | "users.messages.get"
                | "users.labels.list"
        ) {
            return Err(invalid());
        }
        let policy = format!("api.call,api.gmail.{}", method.to_ascii_lowercase());
        let mut args = crate::common_args(&self.binding, Some(&policy), true);
        // The worker wraps provider content once, at the model seam.
        args.retain(|arg| arg != "--wrap-untrusted");
        args.extend([
            "api".into(),
            "call".into(),
            "gmail".into(),
            "v1".into(),
            method.into(),
            "--params".into(),
            params.to_string(),
            "--scope".into(),
            "https://www.googleapis.com/auth/gmail.readonly".into(),
        ]);
        let command = self
            .with_access_token(GogCommand::read_only(args, self.binding.home()))
            .await?;
        let output = self
            .runner
            .run_bounded(&command, max_stdout)
            .await
            .map_err(|e| e.into_provider_error(false))?;
        crate::normalize_output(output, false)
    }

    pub async fn sync_gmail(
        &self,
        checkpoint: &Value,
        since: i64,
    ) -> Result<GmailPage, ProviderError> {
        let mut state = checkpoint.clone();
        if state.is_null() {
            let profile = self
                .gmail_read("users.getProfile", json!({"userId":"me"}))
                .await?;
            let history = profile["historyId"].as_str().ok_or_else(invalid)?;
            state = json!({"phase":"full", "history":history, "page":null});
        }
        if state["phase"] == "history" {
            return self.gmail_history(&state).await;
        }
        if state["phase"] != "full" {
            return Err(invalid());
        }
        let history = state["history"].as_str().ok_or_else(invalid)?;
        let mut params =
            json!({"userId":"me", "maxResults":100, "q": format!("after:{}", since / 1000)});
        if let Some(page) = state["page"].as_str() {
            params["pageToken"] = page.into();
        }
        let response = self.gmail_read("users.messages.list", params).await?;
        let mut changes = Vec::new();
        let mut arrivals = Vec::new();
        if let Some(messages) = response.get("messages") {
            for message in messages.as_array().ok_or_else(invalid)? {
                let id = message["id"].as_str().ok_or_else(invalid)?;
                let (change, event) = self.acquire_message(id, history).await?;
                changes.push(change);
                if let Some(event) = event {
                    arrivals.push(event);
                }
            }
        }
        let page = response
            .get("nextPageToken")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(GmailPage {
            historical: true,
            arrivals,
            checkpoint: json!({"phase": if page.is_null() { "history" } else { "full" }, "history":history, "page":page}),
            caught_up: false,
            changes,
        })
    }

    /// The metadata of one acquired message, for a version the store
    /// holds without it (ADR-0011).
    pub async fn gmail_message_metadata(
        &self,
        id: &str,
        version: &str,
    ) -> Result<Value, ProviderError> {
        Ok(self.acquire_message(id, version).await?.0.metadata)
    }

    /// Normalize metadata before committing a page. No body enters the durable stream.
    async fn acquire_message(
        &self,
        id: &str,
        missing_version: &str,
    ) -> Result<(SourceChange, Option<pagis_core::NormalizedEvent>), ProviderError> {
        let message = match self
            .gmail_read(
                "users.messages.get",
                json!({
                    "userId":"me", "id":id, "format":"full"
                }),
            )
            .await
        {
            Ok(message) => message,
            Err(error) if error.code == ProviderErrorCode::NotFound => {
                return Ok((outside(id, missing_version), None));
            }
            Err(error) => return Err(error),
        };
        let labels = label_ids(&message);
        // Spam and trash reach the model through no seam: they raise no
        // arrival and they leave no signals behind for a filter rule to
        // have to remove.
        if labels
            .iter()
            .any(|label| OUTSIDE_THE_MAILBOX.contains(&label.as_str()))
        {
            let version = message["historyId"].as_str().unwrap_or(missing_version);
            return Ok((outside(id, version), None));
        }
        let source_at = message["internalDate"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(invalid)?;
        let mut source = SourceChange {
            id: message["id"]
                .as_str()
                .filter(|found| *found == id)
                .ok_or_else(invalid)?
                .into(),
            version: message["historyId"].as_str().ok_or_else(invalid)?.into(),
            kind: "mail".into(),
            parent: message["threadId"].as_str().map(str::to_string),
            source_at: Some(source_at),
            operation: SourceOperation::Upsert,
            metadata: Value::Null,
        };
        let header = |name: &str| {
            message["payload"]["headers"]
                .as_array()
                .and_then(|headers| {
                    headers.iter().find(|h| {
                        h["name"]
                            .as_str()
                            .is_some_and(|n| n.eq_ignore_ascii_case(name))
                    })
                })
                .and_then(|h| h["value"].as_str())
                .unwrap_or_default()
                .to_string()
        };
        let from = header("From");
        let mut metadata = json!({"mailbox":"", "message_id":id, "to":header("To").split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>(), "received_at":source_at,
            "has_attachments":has_attachment(&message["payload"]), "labels":labels});
        for (name, value) in [
            ("from", from.clone()),
            ("subject", header("Subject")),
            ("in_reply_to", header("In-Reply-To")),
        ] {
            if !value.is_empty() {
                metadata[name] = value.into();
            }
        }
        if let Some(domain) = crate::gmail_events::domain_of(&from) {
            metadata["from_domain"] = domain.into();
        }
        if let Some(thread) = &source.parent {
            metadata["thread_id"] = thread.clone().into();
        }
        // Every acquired version keeps its metadata, so Page Signals read
        // the owner's own mail and a label change alike (ADR-0011).
        source.metadata = metadata.clone();
        // Sent mail and drafts are acquired for learning but are not inbound arrivals.
        let outbound = labels
            .iter()
            .any(|label| label == "SENT" || label == "DRAFT");
        let event = (!outbound).then_some(pagis_core::NormalizedEvent {
            provider_event_id: id.into(),
            metadata,
            occurred_at: source_at,
            landing: None,
        });
        Ok((source, event))
    }

    async fn gmail_history(&self, state: &Value) -> Result<GmailPage, ProviderError> {
        let history = state["history"].as_str().ok_or_else(invalid)?;
        let mut params = json!({"userId":"me", "maxResults":100, "startHistoryId":history});
        if let Some(page) = state["page"].as_str() {
            params["pageToken"] = page.into();
        }
        let response = self.gmail_read("users.history.list", params).await?;
        let next_history = response["historyId"].as_str().ok_or_else(invalid)?;
        let mut changes = Vec::new();
        let mut added = std::collections::BTreeSet::new();
        if let Some(records) = response.get("history") {
            for record in records.as_array().ok_or_else(invalid)? {
                let version = record["id"].as_str().ok_or_else(invalid)?;
                for (field, operation) in [
                    ("messagesAdded", SourceOperation::Upsert),
                    ("labelsAdded", SourceOperation::Upsert),
                    ("labelsRemoved", SourceOperation::Upsert),
                    ("messagesDeleted", SourceOperation::Deleted),
                ] {
                    if let Some(items) = record.get(field) {
                        for item in items.as_array().ok_or_else(invalid)? {
                            let message = &item["message"];
                            let id = message["id"].as_str().ok_or_else(invalid)?;
                            if field == "messagesAdded" {
                                added.insert(id.to_string());
                            }
                            changes.push(SourceChange {
                                id: id.into(),
                                version: version.into(),
                                kind: "mail".into(),
                                parent: message["threadId"].as_str().map(str::to_string),
                                source_at: None,
                                operation,
                                metadata: Value::Null,
                            });
                        }
                    }
                }
            }
        }
        let page = response
            .get("nextPageToken")
            .cloned()
            .unwrap_or(Value::Null);
        let mut acquired = Vec::new();
        let mut arrivals = Vec::new();
        let mut hydrated = std::collections::BTreeSet::new();
        for change in changes {
            if change.operation == SourceOperation::Deleted {
                hydrated.remove(&change.id);
                acquired.push(change);
                continue;
            }
            if !hydrated.insert(change.id.clone()) {
                continue;
            }
            let (current, event) = self.acquire_message(&change.id, &change.version).await?;
            if added.contains(&change.id)
                && let Some(event) = event
            {
                arrivals.push(event);
            }
            acquired.push(current);
        }
        Ok(GmailPage {
            historical: false,
            arrivals,
            caught_up: page.is_null(),
            checkpoint: json!({"phase":"history", "history":if page.is_null() { next_history } else { history }, "page":page}),
            changes: acquired,
        })
    }
}

fn message_text(
    part: &Value,
    text: &mut String,
    depth: u32,
    decoded_bytes: &mut usize,
) -> Result<(), ProviderError> {
    if depth > 20 || text.len() > 128 * 1024 {
        return Err(invalid());
    }
    let mime = part["mimeType"].as_str().unwrap_or_default();
    if matches!(mime, "text/plain" | "text/html")
        && let Some(data) = part["body"]["data"].as_str()
    {
        if data.len() > 256 * 1024 {
            return Err(invalid());
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(data.trim_end_matches('='))
            .map_err(|_| invalid())?;
        *decoded_bytes = decoded_bytes.saturating_add(bytes.len());
        let decoded = String::from_utf8_lossy(&bytes);
        if mime == "text/html" {
            text.push_str(&mail_parser::decoders::html::html_to_text(&decoded));
        } else {
            text.push_str(&decoded);
        }
        text.push('\n');
    }
    if let Some(parts) = part["parts"].as_array() {
        // Prefer the plain representation of a multipart alternative.
        if let Some(plain) = parts.iter().find(|p| p["mimeType"] == "text/plain") {
            message_text(plain, text, depth + 1, decoded_bytes)?;
        } else {
            for child in parts {
                message_text(child, text, depth + 1, decoded_bytes)?;
            }
        }
    }
    Ok(())
}

fn has_attachment(part: &Value) -> bool {
    part["filename"]
        .as_str()
        .is_some_and(|name| !name.is_empty())
        || part["parts"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(has_attachment))
}
