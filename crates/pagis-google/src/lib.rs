//! Daemon-side Google Connection provider through pinned `gog`.

pub mod gmail_events;
pub mod gmail_filter;
pub mod gmail_sync;
pub mod manifest;
pub mod oauth;

pub use gmail_events::{Collection, GmailCollector, GmailCursor};
pub use manifest::{
    BROWSE_QUERY, GOOGLE_NAMESPACE, GOOGLE_PROVIDER, INBOX_LABEL, STARRED_LABEL, UNREAD_LABEL,
    call_from_tool, capability_manifest, is_write, source_reads,
};
pub use oauth::{
    CALLBACK_PATH, GoogleOAuth, IDENTITY_SCOPES, IdentityError, OauthTokens, Pkce, WebClient,
    oauth_scopes, random_token, redirect_uri,
};

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The `gog` release distributed with Pagis and covered by these contracts.
pub use pagis_versions::GOG_VERSION;

/// One named right in an Agent's grant for a Google Connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoogleCapability {
    GmailRead,
    GmailSend,
    GmailModify,
    CalendarRead,
    CalendarWrite,
}

/// One stable model-facing tool supplied by the Google adapter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GoogleToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub capability: GoogleCapability,
}

/// The stock `gog auth add` service and scope flags for a capability set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeProfile {
    pub services: &'static str,
    pub gmail_scope: Option<&'static str>,
    pub extra_scopes: Vec<&'static str>,
    pub readonly: bool,
}

/// Return the Google tool manifest. Mail is not here: the `mail__*`
/// tools serve every mailbox, and this adapter maps them onto `gog`
/// when the call names a Gmail Connection (ADR-0019). The broker adds
/// Connection aliases, and they never multiply this list.
pub fn manifest() -> Vec<GoogleToolDefinition> {
    use GoogleCapability::*;

    [
        (
            "google__calendar_events",
            "List Google Calendar events in a time range.",
            CalendarRead,
            schema(
                &[
                    ("calendar_id", string()),
                    ("from", string()),
                    ("max", integer()),
                    ("page", string()),
                    ("query", string()),
                    ("to", string()),
                ],
                &[],
            ),
        ),
        (
            "google__calendar_create_event",
            "Create one Google Calendar event.",
            CalendarWrite,
            event_schema(true),
        ),
        (
            "google__calendar_update_event",
            "Update selected fields on one Google Calendar event.",
            CalendarWrite,
            event_patch_schema(),
        ),
        (
            "google__calendar_delete_event",
            "Delete one Google Calendar event.",
            CalendarWrite,
            schema(
                &[
                    ("calendar_id", string()),
                    ("event_id", string()),
                    (
                        "send_updates",
                        enum_string(&["all", "externalOnly", "none"]),
                    ),
                ],
                &["calendar_id", "event_id"],
            ),
        ),
        (
            "google__calendar_respond_event",
            "Respond to one Google Calendar invitation.",
            CalendarWrite,
            schema(
                &[
                    ("calendar_id", string()),
                    ("comment", string()),
                    ("event_id", string()),
                    (
                        "status",
                        enum_string(&["accepted", "declined", "tentative", "needsAction"]),
                    ),
                ],
                &["calendar_id", "event_id", "status"],
            ),
        ),
    ]
    .into_iter()
    .map(
        |(name, description, capability, parameters)| GoogleToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
            capability,
        },
    )
    .collect()
}

fn string() -> serde_json::Value {
    serde_json::json!({"type": "string"})
}

fn integer() -> serde_json::Value {
    serde_json::json!({"type": "integer", "minimum": 1})
}

fn string_array() -> serde_json::Value {
    serde_json::json!({"type": "array", "items": {"type": "string"}})
}

fn boolean() -> serde_json::Value {
    serde_json::json!({"type": "boolean"})
}

fn enum_string(values: &[&str]) -> serde_json::Value {
    serde_json::json!({"type": "string", "enum": values})
}

fn schema(properties: &[(&str, serde_json::Value)], required: &[&str]) -> serde_json::Value {
    let properties: serde_json::Map<_, _> = properties
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn event_schema(create: bool) -> serde_json::Value {
    schema(
        &[
            ("attendees", string_array()),
            ("calendar_id", string()),
            ("description", string()),
            ("from", string()),
            ("location", string()),
            (
                "send_updates",
                enum_string(&["all", "externalOnly", "none"]),
            ),
            ("summary", string()),
            ("timezone", string()),
            ("to", string()),
            ("with_meet", boolean()),
        ],
        if create {
            &["calendar_id", "from", "summary", "to"]
        } else {
            &[]
        },
    )
}

fn event_patch_schema() -> serde_json::Value {
    let mut value = event_schema(false);
    let properties = value["properties"]
        .as_object_mut()
        .expect("event properties");
    properties.insert("event_id".into(), string());
    properties.remove("with_meet");
    value["required"] = serde_json::json!(["calendar_id", "event_id"]);
    value
}

/// Select the complete stock `gog` scope profile for a reauthorization.
pub fn scope_profile(capabilities: impl IntoIterator<Item = GoogleCapability>) -> ScopeProfile {
    use GoogleCapability::*;

    let capabilities: BTreeSet<_> = capabilities.into_iter().collect();
    let gmail = capabilities
        .iter()
        .any(|capability| matches!(capability, GmailRead | GmailSend | GmailModify));
    let calendar = capabilities
        .iter()
        .any(|capability| matches!(capability, CalendarRead | CalendarWrite));
    let writes = capabilities
        .iter()
        .any(|capability| matches!(capability, GmailSend | GmailModify | CalendarWrite));

    let gmail_read = capabilities.contains(&GmailRead);
    let gmail_send = capabilities.contains(&GmailSend);
    let gmail_modify = capabilities.contains(&GmailModify);
    let gmail_scope = if gmail_modify {
        Some("readonly")
    } else if gmail_read && gmail_send {
        Some("read-send")
    } else if gmail_send {
        Some("send")
    } else if gmail_read {
        Some("readonly")
    } else {
        None
    };
    let mut extra_scopes = Vec::new();
    if gmail_modify {
        extra_scopes.push("https://www.googleapis.com/auth/gmail.modify");
    }
    if gmail_modify && gmail_send {
        extra_scopes.push("https://www.googleapis.com/auth/gmail.send");
    }

    ScopeProfile {
        services: match (gmail, calendar) {
            (true, true) => "gmail,calendar",
            (true, false) => "gmail",
            (false, true) => "calendar",
            (false, false) => "",
        },
        gmail_scope,
        extra_scopes,
        readonly: !writes,
    }
}

/// Trusted account, OAuth-client and `gog` home binding from one
/// Connection.
///
/// `home` is the `GOG_HOME` every call of this Connection runs under.
/// It carries the Workspace, so two people who both name their
/// Connection `google` get two `gog` clients called `google` in two
/// homes. In one shared store, the second client would overwrite the
/// credentials of the first.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionBinding {
    account: String,
    client: String,
    home: PathBuf,
}

impl fmt::Debug for ConnectionBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionBinding")
            .field("account", &self.account)
            .field("client", &self.client)
            .field("home", &self.home)
            .finish()
    }
}

impl ConnectionBinding {
    pub fn new(
        account: impl Into<String>,
        client: impl Into<String>,
        home: impl Into<PathBuf>,
    ) -> Result<Self, AdapterError> {
        let account = account.into();
        let client = client.into();
        let home = home.into();
        validate_trusted_value("account", &account, true)?;
        validate_trusted_value("client", &client, false)?;
        if home.as_os_str().is_empty() || !home.is_absolute() {
            return Err(AdapterError::InvalidConfiguration("gog_home"));
        }
        Ok(Self {
            account,
            client,
            home,
        })
    }

    /// The `GOG_HOME` of the Workspace this Connection belongs to.
    pub fn home(&self) -> &Path {
        &self.home
    }
}

/// The password of the file keyring under one `GOG_HOME`
/// (ADR-0012).
///
/// `gog` keeps its own tokens in the platform keyring. On macOS that is
/// the one system keychain, which `GOG_HOME` does not move: two people of
/// one installation who both name a Connection `google` would share a
/// keychain item, so one person's `byo` token would be the other person's
/// too. The daemon therefore puts `gog` on its file backend, under the
/// per-Workspace home, with a password of that home's own. The daemon
/// holds the password in its own secret store and nobody types it.
pub trait GogKeyring: Send + Sync {
    /// The password of the keyring under `home`, made on first use and
    /// the same one every time after.
    fn password(&self, home: &Path) -> Result<String, String>;
}

/// The secret-store name of that password. The `GOG_HOME` of a Workspace
/// is `<root>/<workspace_id>/gog` ([`workspace_gog_home`]), so the parent
/// directory names the tenant; a home of another shape answers its own
/// last component, which still keeps two homes apart.
pub fn keyring_secret_name(home: &Path) -> String {
    let key = home
        .parent()
        .and_then(|parent| parent.file_name())
        .or_else(|| home.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "default".to_string());
    let key: String = key
        .chars()
        .map(
            |letter| match letter.is_ascii_alphanumeric() || letter == '-' || letter == '_' {
                true => letter,
                false => '_',
            },
        )
        .collect();
    format!("google/gog-keyring/{key}")
}

/// The keyring passwords in the daemon's own secret store (ADR-0013).
pub struct SecretStoreKeyring {
    secrets: Arc<dyn pagis_core::SecretStore>,
}

impl SecretStoreKeyring {
    pub fn new(secrets: Arc<dyn pagis_core::SecretStore>) -> Self {
        Self { secrets }
    }
}

impl GogKeyring for SecretStoreKeyring {
    fn password(&self, home: &Path) -> Result<String, String> {
        let name = keyring_secret_name(home);
        if let Some(held) = self.secrets.get(&name).map_err(|error| error.to_string())? {
            return Ok(held);
        }
        let mut bytes = [0_u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
        let fresh: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        self.secrets
            .set(&name, &fresh)
            .map_err(|error| error.to_string())?;
        Ok(fresh)
    }
}

/// The `gog` home of one Workspace, under the daemon's own directory.
/// One directory per Workspace is what keeps a `gog` client name of one
/// person out of another person's store.
pub fn workspace_gog_home(root: &Path, workspace_id: &pagis_core::WorkspaceId) -> PathBuf {
    root.join(workspace_id.as_str()).join("gog")
}

fn validate_trusted_value(
    name: &'static str,
    value: &str,
    email: bool,
) -> Result<(), AdapterError> {
    let valid = !value.is_empty()
        && value.len() <= 254
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '@' | '.' | '_' | '-' | '+')
        })
        && (!email || value.contains('@'));
    if valid {
        Ok(())
    } else {
        Err(AdapterError::InvalidConfiguration(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailSearch {
    pub query: String,
    pub max: Option<u32>,
    pub page: Option<String>,
}

/// One outgoing message. A reply is a send with `in_reply_to`: there
/// is no draft and no separate reply call (ADR-0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailDraft {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    pub calendar_id: String,
    pub summary: String,
    pub from: String,
    pub to: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub attendees: Vec<String>,
    pub timezone: Option<String>,
    pub send_updates: Option<String>,
    pub with_meet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventPatch {
    pub calendar_id: String,
    pub event_id: String,
    pub summary: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub attendees: Option<Vec<String>>,
    pub timezone: Option<String>,
    pub send_updates: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoogleCall {
    GmailSearch(GmailSearch),
    GmailGetMessage {
        message_id: String,
    },
    GmailGetThread {
        thread_id: String,
    },
    CalendarEvents {
        calendar_id: Option<String>,
        from: Option<String>,
        to: Option<String>,
        query: Option<String>,
        max: Option<u32>,
        page: Option<String>,
    },
    GmailSend(EmailDraft),
    GmailModifyMessage {
        message_id: String,
        add_labels: Vec<String>,
        remove_labels: Vec<String>,
    },
    CalendarCreateEvent(CalendarEvent),
    CalendarUpdateEvent(CalendarEventPatch),
    CalendarDeleteEvent {
        calendar_id: String,
        event_id: String,
        send_updates: Option<String>,
    },
    CalendarRespondEvent {
        calendar_id: String,
        event_id: String,
        status: String,
        comment: Option<String>,
    },
}

#[derive(Clone, PartialEq, Eq)]
struct CommandInput(Vec<u8>);

impl fmt::Debug for CommandInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted stdin>")
    }
}

/// A direct `gog` process invocation. No value is interpreted by a shell.
#[derive(Clone, PartialEq, Eq)]
pub struct GogCommand {
    args: Vec<String>,
    stdin: Option<CommandInput>,
    write: bool,
    /// The `GOG_HOME` the child runs under: one directory per Workspace.
    home: PathBuf,
    /// An access token the daemon minted for this call, for a
    /// `brokered` Connection whose refresh token the daemon owns.
    /// `gog` takes it through `GOG_ACCESS_TOKEN` and makes no
    /// token call of its own. `None` leaves `gog` to resolve the token
    /// from its own store, which is the `byo` path.
    access_token: Option<CommandInput>,
}

impl fmt::Debug for GogCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GogCommand")
            .field("args", &self.args)
            .field("stdin", &self.stdin)
            .field("write", &self.write)
            .field("home", &self.home)
            .field("has_access_token", &self.access_token.is_some())
            .finish()
    }
}

impl GogCommand {
    pub fn for_call(binding: &ConnectionBinding, call: &GoogleCall) -> Result<Self, AdapterError> {
        let (exact, readonly, write, operation) = operation_args(call)?;
        let mut args = common_args(binding, Some(exact), readonly);
        args.extend(operation.into_args());
        Ok(Self {
            args,
            stdin: None,
            write,
            home: binding.home.clone(),
            access_token: None,
        })
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The `GOG_HOME` this command runs under.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Hand `gog` an access token the daemon minted, in place of the
    /// one it would resolve from its own store.
    pub fn with_access_token(mut self, token: &str) -> Self {
        self.access_token = Some(CommandInput(token.as_bytes().to_vec()));
        self
    }

    /// Whether the daemon supplies the token for this call.
    pub fn has_access_token(&self) -> bool {
        self.access_token.is_some()
    }

    /// One read-only command from arguments a caller built itself.
    pub(crate) fn read_only(args: Vec<String>, home: &Path) -> Self {
        Self {
            args,
            stdin: None,
            write: false,
            home: home.to_path_buf(),
            access_token: None,
        }
    }

    pub fn stdin(&self) -> Option<&[u8]> {
        self.stdin.as_ref().map(|input| input.0.as_slice())
    }

    pub fn is_write(&self) -> bool {
        self.write
    }
}

fn common_args(binding: &ConnectionBinding, exact: Option<&str>, readonly: bool) -> Vec<String> {
    let mut args = vec![
        "--account".into(),
        binding.account.clone(),
        "--client".into(),
        binding.client.clone(),
    ];
    if let Some(exact) = exact {
        args.extend(["--enable-commands-exact".into(), exact.into()]);
    }
    args.extend(["--no-input".into(), "--wrap-untrusted".into()]);
    if readonly {
        args.push("--readonly".into());
    }
    args.push("--json".into());
    args
}

/// The part of one `gog` argv that a tool call gives: the command, its
/// flags and its positional values.
///
/// `gog` reads an argv item that starts with `-` as an option, also after
/// a positional value. So each flag holds its value in the same item
/// (`--flag=value`), and `--` ends the options in front of the positional
/// values. A value from the Agent thus cannot start an option of its own,
/// such as a second `--client` that replaces the client of the
/// Connection.
struct Operation {
    command: &'static [&'static str],
    flags: Vec<String>,
    positionals: Vec<String>,
}

impl Operation {
    fn new(command: &'static [&'static str]) -> Self {
        Self {
            command,
            flags: Vec::new(),
            positionals: Vec::new(),
        }
    }

    /// A flag without a value, such as `--sanitize-content`.
    fn switch(&mut self, flag: &str) {
        self.flags.push(flag.to_string());
    }

    /// A flag and its value, in one item.
    fn flag(&mut self, flag: &str, value: &str) {
        self.flags.push(format!("{flag}={value}"));
    }

    fn optional(&mut self, flag: &str, value: &Option<String>) {
        if let Some(value) = value {
            self.flag(flag, value);
        }
    }

    fn number(&mut self, flag: &str, value: Option<u32>) {
        if let Some(value) = value {
            self.flag(flag, &value.to_string());
        }
    }

    /// A list flag, with its values joined by commas. An empty list gives
    /// no flag.
    fn list(&mut self, flag: &str, values: &[String]) {
        if !values.is_empty() {
            self.flag(flag, &values.join(","));
        }
    }

    fn positional(&mut self, value: &str) {
        self.positionals.push(value.to_string());
    }

    fn into_args(self) -> Vec<String> {
        let mut args = strings(self.command);
        args.extend(self.flags);
        args.push("--".into());
        args.extend(self.positionals);
        args
    }
}

fn operation_args(
    call: &GoogleCall,
) -> Result<(&'static str, bool, bool, Operation), AdapterError> {
    use GoogleCall::*;
    let contract = match call {
        GmailSearch(search) => {
            // The query keeps a leading `-`: `-label:spam` is Gmail query
            // syntax, and the query comes after `--`.
            let mut operation = Operation::new(&["gmail", "search"]);
            operation.positional(nonempty("query", &search.query)?);
            operation.number("--max", search.max);
            operation.optional("--page", &search.page);
            ("gmail.search", true, false, operation)
        }
        GmailGetMessage { message_id } => {
            let mut operation = Operation::new(&["gmail", "get"]);
            operation.positional(id("message_id", message_id)?);
            operation.switch("--sanitize-content");
            ("gmail.get", true, false, operation)
        }
        GmailGetThread { thread_id } => {
            let mut operation = Operation::new(&["gmail", "thread", "get"]);
            operation.positional(id("thread_id", thread_id)?);
            operation.switch("--sanitize-content");
            ("gmail.thread.get", true, false, operation)
        }
        CalendarEvents {
            calendar_id,
            from,
            to,
            query,
            max,
            page,
        } => {
            let mut operation = Operation::new(&["calendar", "events"]);
            if let Some(calendar_id) = calendar_id {
                operation.positional(id("calendar_id", calendar_id)?);
            }
            operation.optional("--from", from);
            operation.optional("--to", to);
            operation.optional("--query", query);
            operation.number("--max", *max);
            operation.optional("--page", page);
            ("calendar.events", true, false, operation)
        }
        GmailSend(email) => ("gmail.send", false, true, email_args(email)?),
        GmailModifyMessage {
            message_id,
            add_labels,
            remove_labels,
        } => {
            if add_labels.is_empty() && remove_labels.is_empty() {
                return Err(AdapterError::InvalidArguments("labels"));
            }
            let mut operation = Operation::new(&["gmail", "messages", "modify"]);
            operation.positional(id("message_id", message_id)?);
            operation.list("--add", add_labels);
            operation.list("--remove", remove_labels);
            ("gmail.messages.modify", false, true, operation)
        }
        CalendarCreateEvent(event) => ("calendar.create", false, true, create_event_args(event)?),
        CalendarUpdateEvent(event) => ("calendar.update", false, true, update_event_args(event)?),
        CalendarDeleteEvent {
            calendar_id,
            event_id,
            send_updates,
        } => {
            validate_send_updates(send_updates)?;
            let mut operation = Operation::new(&["calendar", "delete"]);
            operation.positional(id("calendar_id", calendar_id)?);
            operation.positional(id("event_id", event_id)?);
            // With `--no-input`, `gog` refuses a delete that has no
            // `--force`. The Broker holds a destructive call for the
            // approval of the user before the call reaches the adapter.
            operation.switch("--force");
            operation.optional("--send-updates", send_updates);
            ("calendar.delete", false, true, operation)
        }
        CalendarRespondEvent {
            calendar_id,
            event_id,
            status,
            comment,
        } => {
            if !matches!(
                status.as_str(),
                "accepted" | "declined" | "tentative" | "needsAction"
            ) {
                return Err(AdapterError::InvalidArguments("status"));
            }
            let mut operation = Operation::new(&["calendar", "respond"]);
            operation.positional(id("calendar_id", calendar_id)?);
            operation.positional(id("event_id", event_id)?);
            operation.flag("--status", status);
            operation.optional("--comment", comment);
            ("calendar.respond", false, true, operation)
        }
    };
    Ok(contract)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn nonempty<'a>(name: &'static str, value: &'a str) -> Result<&'a str, AdapterError> {
    if value.is_empty() || value.contains('\0') {
        Err(AdapterError::InvalidArguments(name))
    } else {
        Ok(value)
    }
}

/// Check the shape of one message, thread, calendar or event ID from the
/// Agent. A Google ID starts with an ASCII letter or digit and holds no
/// whitespace and no control character: `primary`,
/// `name@group.calendar.google.com` and a hex message ID have this shape.
/// An ID of another shape, such as one that starts with `-`, is refused
/// before `gog` starts.
fn id<'a>(name: &'static str, value: &'a str) -> Result<&'a str, AdapterError> {
    let valid = value.starts_with(|first: char| first.is_ascii_alphanumeric())
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control());
    if valid {
        Ok(value)
    } else {
        Err(AdapterError::InvalidArguments(name))
    }
}

fn email_args(email: &EmailDraft) -> Result<Operation, AdapterError> {
    nonempty("body", &email.body)?;
    nonempty("subject", &email.subject)?;
    if email.to.is_empty() {
        return Err(AdapterError::InvalidArguments("to"));
    }
    let mut operation = Operation::new(&["gmail", "send"]);
    operation.list("--to", &email.to);
    operation.list("--cc", &email.cc);
    operation.list("--bcc", &email.bcc);
    operation.flag("--subject", &email.subject);
    operation.flag("--body", &email.body);
    // `gog` threads a reply from the message it answers, and it sets
    // References and In-Reply-To itself.
    operation.optional("--reply-to-message-id", &email.in_reply_to);
    Ok(operation)
}

fn create_event_args(event: &CalendarEvent) -> Result<Operation, AdapterError> {
    validate_send_updates(&event.send_updates)?;
    let mut operation = Operation::new(&["calendar", "create"]);
    operation.positional(id("calendar_id", &event.calendar_id)?);
    operation.flag("--summary", nonempty("summary", &event.summary)?);
    operation.flag("--from", nonempty("from", &event.from)?);
    operation.flag("--to", nonempty("to", &event.to)?);
    operation.optional("--description", &event.description);
    operation.optional("--location", &event.location);
    operation.list("--attendees", &event.attendees);
    operation.optional("--timezone", &event.timezone);
    operation.optional("--send-updates", &event.send_updates);
    if event.with_meet {
        operation.switch("--with-meet");
    }
    Ok(operation)
}

fn update_event_args(event: &CalendarEventPatch) -> Result<Operation, AdapterError> {
    validate_send_updates(&event.send_updates)?;
    let mut operation = Operation::new(&["calendar", "update"]);
    operation.positional(id("calendar_id", &event.calendar_id)?);
    operation.positional(id("event_id", &event.event_id)?);
    operation.optional("--summary", &event.summary);
    operation.optional("--from", &event.from);
    operation.optional("--to", &event.to);
    operation.optional("--description", &event.description);
    operation.optional("--location", &event.location);
    if let Some(attendees) = &event.attendees {
        operation.list("--attendees", attendees);
    }
    // `calendar update` has no `--timezone`. It takes the time zone of each
    // end with the new time of that end: `--start-timezone` needs `--from`,
    // and `--end-timezone` needs `--to`.
    if let Some(timezone) = &event.timezone {
        if event.from.is_none() && event.to.is_none() {
            return Err(AdapterError::InvalidArguments("timezone"));
        }
        if event.from.is_some() {
            operation.flag("--start-timezone", timezone);
        }
        if event.to.is_some() {
            operation.flag("--end-timezone", timezone);
        }
    }
    operation.optional("--send-updates", &event.send_updates);
    Ok(operation)
}

fn validate_send_updates(value: &Option<String>) -> Result<(), AdapterError> {
    if value
        .as_deref()
        .is_some_and(|value| !matches!(value, "all" | "externalOnly" | "none"))
    {
        Err(AdapterError::InvalidArguments("send_updates"))
    } else {
        Ok(())
    }
}

/// Build the Desktop OAuth client document `gog` stores, from the two
/// values the Google console shows the user. The bytes go straight to
/// the provider over stdin: no credential file is ever written.
pub fn desktop_client_document(
    client_id: &str,
    client_secret: &str,
) -> Result<Vec<u8>, AdapterError> {
    let invalid = |value: &str| value.trim().is_empty() || value.contains('\0');
    if invalid(client_id) || invalid(client_secret) {
        return Err(AdapterError::InvalidConfiguration("desktop_oauth_client"));
    }
    Ok(serde_json::json!({
        "installed": {
            "client_id": client_id.trim(),
            "client_secret": client_secret.trim(),
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token",
        }
    })
    .to_string()
    .into_bytes())
}

/// Store the Desktop OAuth client through stdin, without a credential path.
pub fn install_client_command(
    binding: &ConnectionBinding,
    credentials: &[u8],
) -> Result<GogCommand, AdapterError> {
    let parsed: serde_json::Value = serde_json::from_slice(credentials)
        .map_err(|_| AdapterError::InvalidConfiguration("desktop_oauth_client"))?;
    if parsed.get("installed").is_none() {
        return Err(AdapterError::InvalidConfiguration("desktop_oauth_client"));
    }
    Ok(GogCommand {
        args: strings(&[
            "--client",
            &binding.client,
            "--no-input",
            "--json",
            "auth",
            "credentials",
            "set",
            "-",
        ]),
        stdin: Some(CommandInput(credentials.to_vec())),
        write: false,
        home: binding.home.clone(),
        access_token: None,
    })
}

/// Start stock `gog` loopback plus PKCE authorization on localhost.
pub fn authorize_command(binding: &ConnectionBinding, profile: ScopeProfile) -> GogCommand {
    let mut args = strings(&[
        "--client",
        &binding.client,
        "--json",
        "auth",
        "add",
        &binding.account,
        "--services",
        profile.services,
    ]);
    if let Some(gmail_scope) = profile.gmail_scope {
        args.extend(["--gmail-scope".into(), gmail_scope.into()]);
    }
    if !profile.extra_scopes.is_empty() {
        args.extend(["--extra-scopes".into(), profile.extra_scopes.join(",")]);
    }
    if profile.readonly {
        args.push("--readonly".into());
    }
    args.extend(["--listen-addr".into(), "127.0.0.1:0".into()]);
    GogCommand {
        args,
        stdin: None,
        write: false,
        home: binding.home.clone(),
        access_token: None,
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("invalid trusted Google configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("invalid Google tool arguments: {0}")]
    InvalidArguments(&'static str),
}

/// Process data that can cross the adapter's runner boundary. Provider stderr
/// is discarded before this value exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
}

/// Whether a failed process definitely did not start or may have dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessFailure {
    NotStarted,
    Interrupted,
    /// The provider rejected the refresh token before dispatch.
    ReauthRequired,
    /// The process wrote more than the caller allowed. It was stopped and
    /// reaped before the whole body reached memory.
    Oversized,
}

impl ProcessFailure {
    pub fn into_provider_error(self, write: bool) -> ProviderError {
        match self {
            Self::NotStarted => ProviderError::new(ProviderErrorCode::TemporarilyUnavailable, true),
            Self::ReauthRequired => ProviderError::new(ProviderErrorCode::ReauthRequired, false),
            Self::Interrupted if write => {
                ProviderError::new(ProviderErrorCode::OutcomeUnknown, false)
            }
            Self::Interrupted => {
                ProviderError::new(ProviderErrorCode::TemporarilyUnavailable, true)
            }
            // The same request gives the same oversized answer, so a retry
            // cannot succeed. A write may already have been dispatched.
            Self::Oversized if write => {
                ProviderError::new(ProviderErrorCode::OutcomeUnknown, false)
            }
            Self::Oversized => {
                ProviderError::rejected("the provider answer is larger than the transport limit")
            }
        }
    }
}

/// Stable errors returned to the broker. They contain no upstream text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCode {
    InvalidRequest,
    NotFound,
    PermissionRevoked,
    ReauthRequired,
    TemporarilyUnavailable,
    OutcomeUnknown,
}

impl ProviderErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::PermissionRevoked => "permission_revoked",
            Self::ReauthRequired => "reauth_required",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub retryable: bool,
    /// Why the request itself is wrong, in words the daemon can log and
    /// record. It holds no upstream text.
    pub detail: &'static str,
}

impl ProviderError {
    fn new(code: ProviderErrorCode, retryable: bool) -> Self {
        Self {
            code,
            retryable,
            detail: "",
        }
    }

    /// The request itself is wrong, so the same request gives the same
    /// answer. `detail` says why.
    pub fn rejected(detail: &'static str) -> Self {
        Self {
            code: ProviderErrorCode::InvalidRequest,
            retryable: false,
            detail,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.as_str())?;
        if !self.detail.is_empty() {
            write!(f, ": {}", self.detail)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderError {}

/// Parse successful JSON and map the automation exits that the pinned `gog`
/// documents.
pub fn normalize_output(
    output: ProcessOutput,
    write: bool,
) -> Result<serde_json::Value, ProviderError> {
    use ProviderErrorCode::*;

    match output.status {
        Some(0) => serde_json::from_slice(&output.stdout).map_err(|_| {
            ProviderError::new(
                if write {
                    OutcomeUnknown
                } else {
                    TemporarilyUnavailable
                },
                false,
            )
        }),
        Some(3) => {
            if output.stdout.is_empty() {
                Ok(serde_json::json!({}))
            } else {
                serde_json::from_slice(&output.stdout)
                    .map_err(|_| ProviderError::new(TemporarilyUnavailable, false))
            }
        }
        Some(2) => Err(ProviderError::new(InvalidRequest, false)),
        Some(4) => Err(ProviderError::new(ReauthRequired, false)),
        Some(5) => Err(ProviderError::new(NotFound, false)),
        Some(6) => Err(ProviderError::new(PermissionRevoked, false)),
        Some(7) => Err(ProviderError::new(TemporarilyUnavailable, true)),
        Some(8) if write => Err(ProviderError::new(OutcomeUnknown, false)),
        Some(8) => Err(ProviderError::new(TemporarilyUnavailable, true)),
        Some(_) | None if write => Err(ProviderError::new(OutcomeUnknown, false)),
        Some(_) | None => Err(ProviderError::new(TemporarilyUnavailable, true)),
    }
}

#[async_trait]
pub trait GogRunner: Send + Sync {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure>;

    /// Reads at most `max_stdout` bytes of standard output. A runner that
    /// reaches the limit stops the process instead of buffering the rest, so
    /// a caller with a byte budget never holds a larger body in memory.
    async fn run_bounded(
        &self,
        command: &GogCommand,
        max_stdout: usize,
    ) -> Result<ProcessOutput, ProcessFailure> {
        let output = self.run(command).await?;
        if output.stdout.len() > max_stdout {
            return Err(ProcessFailure::Oversized);
        }
        Ok(output)
    }
}

/// One runner serves the dispatch path and the connect flow at once, so
/// both reach `gog` the same way and a test replaces both with one fake.
#[async_trait]
impl<R: GogRunner + ?Sized> GogRunner for std::sync::Arc<R> {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        (**self).run(command).await
    }

    async fn run_bounded(
        &self,
        command: &GogCommand,
        max_stdout: usize,
    ) -> Result<ProcessOutput, ProcessFailure> {
        (**self).run_bounded(command, max_stdout).await
    }
}

/// Starts the distributed `gog` binary directly. It never invokes a shell.
#[derive(Clone)]
pub struct SystemGogRunner {
    binary: PathBuf,
    /// The password of the file keyring under each `GOG_HOME`.
    keyring: Arc<dyn GogKeyring>,
}

impl fmt::Debug for SystemGogRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemGogRunner")
            .field("binary", &self.binary)
            .finish()
    }
}

impl SystemGogRunner {
    pub fn new(binary: impl Into<PathBuf>, keyring: Arc<dyn GogKeyring>) -> Self {
        Self {
            binary: binary.into(),
            keyring,
        }
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }
}

#[async_trait]
impl GogRunner for SystemGogRunner {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        self.run_bounded(command, usize::MAX).await
    }

    async fn run_bounded(
        &self,
        command: &GogCommand,
        max_stdout: usize,
    ) -> Result<ProcessOutput, ProcessFailure> {
        let mut process = tokio::process::Command::new(&self.binary);
        process
            .args(&command.args)
            .stdin(if command.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        std::fs::create_dir_all(command.home()).map_err(|_| ProcessFailure::NotStarted)?;
        let password = self
            .keyring
            .password(command.home())
            .map_err(|_| ProcessFailure::NotStarted)?;
        let environment = gog_environment(
            std::env::vars_os().map(|(name, _)| name),
            command,
            &password,
        )?;
        for name in &environment.removed {
            process.env_remove(name);
        }
        for (name, value) in &environment.set {
            process.env(name, value);
        }

        let mut child = process.spawn().map_err(|_| ProcessFailure::NotStarted)?;
        let mut error_source = child.stderr.take().ok_or(ProcessFailure::Interrupted)?;
        // The runner retains only a small prefix for classification and
        // drains the rest so provider diagnostics cannot block the child.
        // The bytes never leave this adapter.
        let error_task = tokio::spawn(async move {
            const LIMIT: usize = 16 * 1024;
            let mut prefix = Vec::new();
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = error_source.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                let retained = LIMIT.saturating_sub(prefix.len()).min(read);
                prefix.extend_from_slice(&chunk[..retained]);
            }
            prefix
        });
        if let Some(input) = &command.stdin {
            let mut stdin = child.stdin.take().ok_or(ProcessFailure::Interrupted)?;
            stdin
                .write_all(&input.0)
                .await
                .map_err(|_| ProcessFailure::Interrupted)?;
        }
        // Standard output is read in chunks and stops at the caller's limit.
        // An oversized answer is never held whole, and the stopped process is
        // reaped before this call returns.
        let mut source = child.stdout.take().ok_or(ProcessFailure::Interrupted)?;
        let mut stdout = Vec::new();
        let mut chunk = [0_u8; 8192];
        let mut oversized = false;
        loop {
            let read = match source.read(&mut chunk).await {
                Ok(read) => read,
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(ProcessFailure::Interrupted);
                }
            };
            if read == 0 {
                break;
            }
            stdout.extend_from_slice(&chunk[..read]);
            if stdout.len() > max_stdout {
                oversized = true;
                break;
            }
        }
        drop(source);
        if oversized {
            let _ = child.kill().await;
            return Err(ProcessFailure::Oversized);
        }
        let status = child
            .wait()
            .await
            .map_err(|_| ProcessFailure::Interrupted)?;
        let stderr = error_task.await.unwrap_or_default();
        if !status.success() && expired_refresh_token(&stderr) {
            return Err(ProcessFailure::ReauthRequired);
        }
        Ok(ProcessOutput {
            status: status.code(),
            stdout,
        })
    }
}

/// What the runner changes in the environment that a `gog` child gets from
/// the daemon.
pub struct GogEnvironment {
    /// The variables of the daemon that the child does not get.
    pub removed: Vec<OsString>,
    /// The variables that the command decides, in the order the runner
    /// sets them.
    pub set: Vec<(&'static str, OsString)>,
}

/// The values of `set` are secrets, so this prints only the names.
impl fmt::Debug for GogEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GogEnvironment")
            .field("removed", &self.removed)
            .field(
                "set",
                &self.set.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// The environment of the `gog` child of `command`, from `daemon`, the
/// names of the variables of the daemon's own environment.
///
/// The command decides every setting of its call, never the daemon's own
/// environment. `gog` also reads its settings from `GOG_` variables, and a
/// new release can add one. So the child gets no `GOG_` variable of the
/// daemon, and it gets the home, the keyring and the token of the command.
/// The token travels in the environment and not in an argument, so it is
/// not in `ps`. `gog` keeps its own tokens on its file backend under that
/// home, not in the platform keyring, so two people of one macOS
/// installation do not share a keychain item (ADR-0012).
/// `keyring_password` is the password of that home, which the daemon
/// holds.
pub fn gog_environment(
    daemon: impl IntoIterator<Item = OsString>,
    command: &GogCommand,
    keyring_password: &str,
) -> Result<GogEnvironment, ProcessFailure> {
    let removed = daemon
        .into_iter()
        .filter(|name| name.as_encoded_bytes().starts_with(b"GOG_"))
        .collect();
    let mut set = vec![
        ("GOG_HOME", command.home().as_os_str().to_owned()),
        ("GOG_KEYRING_BACKEND", OsString::from("file")),
        ("GOG_KEYRING_PASSWORD", OsString::from(keyring_password)),
    ];
    if let Some(token) = &command.access_token {
        let token = String::from_utf8(token.0.clone()).map_err(|_| ProcessFailure::NotStarted)?;
        set.push(("GOG_ACCESS_TOKEN", OsString::from(token)));
    }
    Ok(GogEnvironment { removed, set })
}

fn expired_refresh_token(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("oauth2: \"invalid_grant\"")
}

/// Typed daemon adapter for one Google Connection. The broker can share one
/// instance with every Agent granted that Connection.
pub struct GoogleProvider<R> {
    binding: ConnectionBinding,
    runner: R,
    tokens: Option<std::sync::Arc<dyn AccessTokens>>,
}

impl<R: fmt::Debug> fmt::Debug for GoogleProvider<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GoogleProvider")
            .field("binding", &self.binding)
            .field("runner", &self.runner)
            .field("mints_access_tokens", &self.tokens.is_some())
            .finish()
    }
}

/// Where a `brokered` Connection's access token comes from. The
/// daemon owns the refresh token, so it mints the access token and
/// hands it to `gog` for the one call.
#[async_trait]
pub trait AccessTokens: Send + Sync {
    /// A token that is live now. The implementation refreshes when the
    /// one it holds has run out.
    async fn fresh(&self) -> Result<String, ProviderError>;
}

impl<R> GoogleProvider<R>
where
    R: GogRunner,
{
    pub fn new(binding: ConnectionBinding, runner: R) -> Self {
        Self {
            binding,
            runner,
            tokens: None,
        }
    }

    /// Mint the access token for every call instead of leaving `gog` to
    /// resolve one from its own store. This is what makes a `brokered`
    /// Connection work: the refresh token is the daemon's, sealed with
    /// the Tenant Data Key, and `gog` keeps nothing.
    pub fn with_access_tokens(mut self, tokens: std::sync::Arc<dyn AccessTokens>) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Stamp the daemon's access token on one command, when this
    /// Connection is one the daemon holds the tokens for.
    pub(crate) async fn with_access_token(
        &self,
        command: GogCommand,
    ) -> Result<GogCommand, ProviderError> {
        match &self.tokens {
            Some(tokens) => Ok(command.with_access_token(&tokens.fresh().await?)),
            None => Ok(command),
        }
    }

    pub async fn invoke(&self, call: &GoogleCall) -> Result<serde_json::Value, ProviderError> {
        let command = GogCommand::for_call(&self.binding, call)
            .map_err(|_| ProviderError::new(ProviderErrorCode::InvalidRequest, false))?;
        let command = self.with_access_token(command).await?;
        let write = command.is_write();
        let output = self
            .runner
            .run(&command)
            .await
            .map_err(|failure| failure.into_provider_error(write))?;
        normalize_output(output, write)
    }

    pub async fn install_client(
        &self,
        credentials: &[u8],
    ) -> Result<serde_json::Value, ProviderError> {
        let command = install_client_command(&self.binding, credentials)
            .map_err(|_| ProviderError::new(ProviderErrorCode::InvalidRequest, false))?;
        let output = self
            .runner
            .run(&command)
            .await
            .map_err(|failure| failure.into_provider_error(false))?;
        normalize_output(output, false)
    }

    pub async fn authorize(
        &self,
        profile: ScopeProfile,
    ) -> Result<serde_json::Value, ProviderError> {
        let command = authorize_command(&self.binding, profile);
        let output = self
            .runner
            .run(&command)
            .await
            .map_err(|failure| failure.into_provider_error(false))?;
        normalize_output(output, false)
    }
}
