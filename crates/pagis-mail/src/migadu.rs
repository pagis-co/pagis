//! Migadu, the mailbox host with an API (ADR-0019). One `POST` makes a
//! mailbox with the password the daemon chose, and the mailbox logs in
//! over IMAP and SMTP at once. Authentication is HTTP Basic with the
//! account email address and an API key.
//!
//! This client holds the API key and never reads or sends mail: the
//! mailbox password is the transport's, in the mail path.
//!
//! Migadu answers a success with `200` and every failure with `400`,
//! so the failure body, not the status, names the
//! reason.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::host::{
    Deletion, HostAccount, HostCapabilities, HostError, HostErrorCode, HostedMailbox, MailboxHost,
    MailboxPassword, local_part_on,
};

const API_BASE: &str = "https://api.migadu.com/v1";
/// How long the client waits for one Migadu request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct MigaduHost {
    http: reqwest::Client,
    base_url: String,
}

impl MigaduHost {
    pub fn new() -> Result<Self, HostError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|_| HostError(HostErrorCode::TemporarilyUnavailable))?,
            base_url: API_BASE.to_string(),
        })
    }

    /// Point the client at another base URL. The contract tests use it;
    /// production uses [`MigaduHost::new`].
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    fn mailboxes_url(&self, account: &HostAccount) -> String {
        format!("{}/domains/{}/mailboxes", self.base_url, account.domain())
    }

    fn mailbox_url(&self, account: &HostAccount, local_part: &str) -> String {
        format!("{}/{local_part}", self.mailboxes_url(account))
    }

    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
        account: &HostAccount,
    ) -> reqwest::RequestBuilder {
        request.basic_auth(account.account(), Some(account.expose_api_key()))
    }
}

/// One mailbox as Migadu answers it. Only the fields Pagis reads are
/// named; the host can add its own without breaking this client.
#[derive(Debug, Deserialize)]
struct MailboxDto {
    local_part: String,
    domain_name: String,
}

#[derive(Debug, Deserialize)]
struct MailboxListDto {
    mailboxes: Vec<MailboxDto>,
}

impl From<MailboxDto> for HostedMailbox {
    fn from(dto: MailboxDto) -> Self {
        HostedMailbox::new(dto.local_part, dto.domain_name)
    }
}

#[async_trait]
impl MailboxHost for MigaduHost {
    fn capabilities(&self) -> HostCapabilities {
        HostCapabilities {
            // Migadu caps sends per account and per plan, and its
            // per-mailbox limits are absent on the small plans.
            // The daemon enforces the Outgoing Cap.
            outgoing_cap: false,
            delete_mailbox: true,
            reset_password: true,
        }
    }

    async fn create(
        &self,
        account: &HostAccount,
        local_part: &str,
        password: &MailboxPassword,
        _outgoing_cap: u32,
    ) -> Result<HostedMailbox, HostError> {
        let request = self
            .http
            .post(self.mailboxes_url(account))
            .json(&serde_json::json!({
                "local_part": local_part,
                "password": password.expose(),
                // The other method mails an invitation link, which an
                // Agent cannot open.
                "password_method": "password",
            }));
        let response = self
            .authorize(request, account)
            .send()
            .await
            .map_err(transport_error)?;
        let created: MailboxDto = read(response).await?;
        Ok(created.into())
    }

    async fn delete(&self, account: &HostAccount, address: &str) -> Result<Deletion, HostError> {
        let local_part = local_part_on(account, address)?;
        let request = self.http.delete(self.mailbox_url(account, &local_part));
        let response = self
            .authorize(request, account)
            .send()
            .await
            .map_err(transport_error)?;
        // A mailbox the host does not hold is deleted already.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Deletion::Removed);
        }
        let _: serde_json::Value = read(response).await?;
        Ok(Deletion::Removed)
    }

    async fn reset_password(
        &self,
        account: &HostAccount,
        address: &str,
        password: &MailboxPassword,
    ) -> Result<(), HostError> {
        let local_part = local_part_on(account, address)?;
        let request = self
            .http
            .put(self.mailbox_url(account, &local_part))
            .json(&serde_json::json!({ "password": password.expose() }));
        let response = self
            .authorize(request, account)
            .send()
            .await
            .map_err(transport_error)?;
        let _: serde_json::Value = read(response).await?;
        Ok(())
    }

    async fn list(&self, account: &HostAccount) -> Result<Vec<HostedMailbox>, HostError> {
        let request = self.http.get(self.mailboxes_url(account));
        let response = self
            .authorize(request, account)
            .send()
            .await
            .map_err(transport_error)?;
        let found: MailboxListDto = read(response).await?;
        Ok(found
            .mailboxes
            .into_iter()
            .map(HostedMailbox::from)
            .collect())
    }
}

/// One answer, turned into either a document or a stable code. No
/// upstream text reaches the caller.
async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, HostError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(HostError(failure_code(status, &body)));
    }
    let body = response.text().await.map_err(transport_error)?;
    serde_json::from_str(&body).map_err(|_| HostError(HostErrorCode::Unreadable))
}

/// The reason behind one failure. Migadu answers most failures with
/// `400`, so the message the body carries decides.
fn failure_code(status: reqwest::StatusCode, body: &str) -> HostErrorCode {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            return HostErrorCode::Unauthorized;
        }
        reqwest::StatusCode::NOT_FOUND => return HostErrorCode::MailboxUnknown,
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            return HostErrorCode::TemporarilyUnavailable;
        }
        _ if status.is_server_error() => return HostErrorCode::TemporarilyUnavailable,
        _ => {}
    }
    let message = failure_message(body).to_ascii_lowercase();
    if message.contains("taken") || message.contains("already exists") {
        HostErrorCode::AddressTaken
    } else if message.contains("limit")
        || message.contains("quota")
        || message.contains("maximum")
        || message.contains("exceeded")
    {
        HostErrorCode::CapReached
    } else {
        HostErrorCode::Refused
    }
}

/// The text of a failure body, in either shape Migadu uses: one
/// `error` string, or an `errors` object of fields and their
/// complaints.
fn failure_message(body: &str) -> String {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    if let Some(error) = document.get("error").and_then(|error| error.as_str()) {
        return error.to_string();
    }
    match document.get("errors") {
        Some(errors) => errors.to_string(),
        None => body.to_string(),
    }
}

/// A request that never reached the host, or an answer that never
/// arrived.
fn transport_error(_error: reqwest::Error) -> HostError {
    HostError(HostErrorCode::TemporarilyUnavailable)
}
