//! The Provider Catalog (ADR-0012): the one list of providers
//! Pagis connects.
//!
//! Each entry declares how a provider is set up: an OAuth exchange
//! finished in the browser, or a field list the picker draws as a
//! form. The entry also carries what a Connection of the provider
//! gives an Agent, how many of them a Workspace may hold, and the
//! provider copy the cards show. The client holds no provider list:
//! a new provider is one entry here and one arm in the parser below.
//!
//! Each entry also declares its **installation setup**: the parts of
//! the provider that the installation sets up one time for everybody,
//! such as the carrier account, its SIP credential or the Installation
//! OAuth Client. The Administration Interface sets up those parts
//! through one set of routes for every provider. What a person does
//! with the provider stays on the product port: a person connects their
//! own Google account, buys an Agent Phone Number or makes an Agent
//! Mailbox. A model provider has one installation part, its key, and
//! [`installation_setups`] lists it beside the catalog entries.

use std::collections::BTreeMap;

use pagis_core::{PROVIDERS, Provider};
use pagis_mail::{Endpoint, MANUAL_PROVIDER, MIGADU_PROVIDER};
use pagis_telephony::{PLIVO_PROVIDER, TELNYX_PROVIDER, TWILIO_PROVIDER};

use crate::{ConnectError, NewCredentials};

/// Agents read and send the account's own mail through the `mail__*`
/// tools, under a Grant.
pub const MAIL: &str = "mail";
/// Agents read and write the account's calendar, under a Grant.
pub const CALENDAR: &str = "calendar";
/// Agents get mailboxes of their own on the provider's domain
/// (ADR-0019).
pub const MAILBOXES: &str = "mailboxes";
/// Agents hold phone numbers bought through the provider (ADR-0018).
pub const TELEPHONY: &str = "telephony";
/// Agents send and receive texts on those numbers (ADR-0020). A
/// carrier that does not carry a text lists it as absent instead, so
/// the user reads it before a number is bought.
pub const TEXTING: &str = "texting";

/// How a provider is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// The user finishes at the provider in the browser (ADR-0012).
    Oauth,
    /// The user types the declared fields and the daemon proves them.
    Fields,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Oauth => "oauth",
            ProviderKind::Fields => "fields",
        }
    }
}

/// What the picker draws for one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Number,
}

impl FieldKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Number => "number",
        }
    }
}

/// One value the user types. `key` is the name the create request
/// carries it under, and the name the parser reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderField {
    pub key: &'static str,
    pub label: &'static str,
    /// The placeholder, with an example value.
    pub hint: &'static str,
    pub kind: FieldKind,
    /// The picker masks it, and it goes to the secret store, never to
    /// the database.
    pub secret: bool,
    pub default: Option<&'static str>,
}

/// What one part of an installation setup is, and so what the
/// Administration Interface does when it configures, tests or removes
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupKind {
    /// The Installation Connection itself: the carrier account or the
    /// mail domain. Its fields are the entry's own fields.
    Connection,
    /// The carrier's SIP credential, which every Agent Phone Number
    /// registers with (ADR-0020). It needs the Connection first.
    SipCredential,
    /// The Installation OAuth Client (ADR-0012).
    OauthClient,
    /// A model provider's key.
    ModelKey,
}

impl SetupKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SetupKind::Connection => "connection",
            SetupKind::SipCredential => "sip_credential",
            SetupKind::OauthClient => "oauth_client",
            SetupKind::ModelKey => "model_key",
        }
    }

    /// Whether the daemon can prove the kept credential again at the
    /// provider. A Connection proves its key the same way the connect
    /// step did. A SIP credential proves itself when a number
    /// registers, and an OAuth client when a person consents, so a test
    /// of either would prove nothing.
    pub fn testable(&self) -> bool {
        matches!(self, SetupKind::Connection)
    }
}

/// One part of a provider that the installation sets up one time for
/// everybody. `id` is the path segment the administration routes name
/// it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupPart {
    pub id: &'static str,
    pub kind: SetupKind,
    pub label: &'static str,
    /// What the administrator reads before they type.
    pub blurb: &'static str,
    pub fields: &'static [ProviderField],
}

/// One provider as the Administration Interface sets it up: a catalog
/// entry with installation parts, or a model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallationSetup {
    /// The catalog id or the model provider id.
    pub provider: &'static str,
    pub label: &'static str,
    /// `models`, `accounts`, `telephony` or `mailboxes`: the group the
    /// Administration Interface shows the provider in.
    pub group: &'static str,
    pub parts: &'static [SetupPart],
}

/// One provider Pagis connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEntry {
    /// The `provider` a Connection records.
    pub id: &'static str,
    pub label: &'static str,
    /// What the user reads before they type: where the values come
    /// from, and what Pagis does with them.
    pub blurb: &'static str,
    pub kind: ProviderKind,
    pub fields: &'static [ProviderField],
    /// What a Connection of this provider gives an Agent.
    pub capabilities: &'static [&'static str],
    /// What this provider does not do, and a sibling provider does
    /// (ADR-0005, ADR-0020). The card states each one, so the user
    /// reads it before they commit to the provider.
    pub absent_capabilities: &'static [&'static str],
    /// How many Connections of this provider one Workspace holds, or
    /// `None` for as many as the user makes.
    pub max_instances: Option<u32>,
    pub default_display_name: &'static str,
    pub default_alias: &'static str,
    /// Where the user finds the values, e.g. `the Telnyx portal`.
    pub portal: Option<&'static str>,
    /// The parts of this provider the installation sets up for
    /// everybody. A provider with a [`SetupKind::Connection`] part is an
    /// Installation Connection: a person never creates one.
    pub installation: &'static [SetupPart],
}

const fn text(key: &'static str, label: &'static str, hint: &'static str) -> ProviderField {
    ProviderField {
        key,
        label,
        hint,
        kind: FieldKind::Text,
        secret: false,
        default: None,
    }
}

const fn secret(key: &'static str, label: &'static str, hint: &'static str) -> ProviderField {
    ProviderField {
        secret: true,
        ..text(key, label, hint)
    }
}

const fn number(key: &'static str, label: &'static str, default: &'static str) -> ProviderField {
    ProviderField {
        kind: FieldKind::Number,
        default: Some(default),
        ..text(key, label, label)
    }
}

const GOOGLE_FIELDS: &[ProviderField] = &[
    text(
        "account",
        "Google account",
        "Google account, e.g. alice@example.com",
    ),
    text("client_id", "Client ID", "Client ID"),
    secret("client_secret", "Client secret", "Client secret"),
];

/// The Google form of a brokered installation: the account and
/// nothing else. The installation's own Web OAuth client is the one
/// every person consents against, so there is nothing to paste.
const GOOGLE_BROKERED_FIELDS: &[ProviderField] = &[text(
    "account",
    "Google account",
    "Google account, e.g. alice@example.com",
)];

const GOOGLE_BROKERED_BLURB: &str = "Pagis signs in with this installation's own Google client. Type the \
     account you want to connect, then finish at Google in your browser. \
     Pagis holds the sign-in for this account and nobody else's.";

const TELNYX_FIELDS: &[ProviderField] = &[secret("api_key", "Carrier API key", "Carrier API key")];

const TWILIO_FIELDS: &[ProviderField] = &[
    text("account_sid", "Account SID", "Account SID, e.g. AC..."),
    secret("auth_token", "Auth token", "Auth token"),
];

const PLIVO_FIELDS: &[ProviderField] = &[
    text("auth_id", "Auth ID", "Auth ID"),
    secret("auth_token", "Auth Token", "Auth Token"),
];

const MIGADU_FIELDS: &[ProviderField] = &[
    text(
        "account",
        "Migadu account",
        "Migadu account, e.g. owner@example.com",
    ),
    secret("api_key", "Migadu API key", "Migadu API key"),
    text("domain", "Mail domain", "Mail domain, e.g. example.com"),
];

const MANUAL_FIELDS: &[ProviderField] = &[
    text("domain", "Mail domain", "Mail domain, e.g. example.com"),
    text("imap_host", "IMAP host", "IMAP host, e.g. imap.example.com"),
    number("imap_port", "IMAP port", "993"),
    text("smtp_host", "SMTP host", "SMTP host, e.g. smtp.example.com"),
    number("smtp_port", "SMTP port", "465"),
];

const fn sip_fields(registrar: Option<&'static str>) -> [ProviderField; 3] {
    [
        text("username", "SIP username", "SIP username"),
        secret("password", "SIP password", "SIP password"),
        ProviderField {
            default: registrar,
            ..text("domain", "SIP server", "SIP server, e.g. sip.example.com")
        },
    ]
}

const TELNYX_SIP_FIELDS: &[ProviderField] = &sip_fields(Some("sip.telnyx.com"));
// A Twilio SIP Domain is the user's own, `<name>.sip.twilio.com`, so
// the server comes with the SIP credential and has no default.
const TWILIO_SIP_FIELDS: &[ProviderField] = &sip_fields(None);
const PLIVO_SIP_FIELDS: &[ProviderField] = &sip_fields(Some("phone.plivo.com"));

const CARRIER_ACCOUNT_BLURB: &str = "The carrier account every Agent Phone Number of this installation \
     is bought on. The daemon proves the key at the carrier, then keeps it in the \
     installation's secret store and never in the database.";

const SIP_BLURB: &str = "The SIP sign-in you made at the carrier. Every held number \
     registers with it, and without it incoming calls do not reach an Agent. The \
     password goes to the installation's secret store.";

const fn carrier_setup(
    fields: &'static [ProviderField],
    sip: &'static [ProviderField],
) -> [SetupPart; 2] {
    [
        SetupPart {
            id: "connection",
            kind: SetupKind::Connection,
            label: "Carrier account",
            blurb: CARRIER_ACCOUNT_BLURB,
            fields,
        },
        SetupPart {
            id: "sip",
            kind: SetupKind::SipCredential,
            label: "SIP sign-in",
            blurb: SIP_BLURB,
            fields: sip,
        },
    ]
}

const TELNYX_SETUP: &[SetupPart] = &carrier_setup(TELNYX_FIELDS, TELNYX_SIP_FIELDS);
const TWILIO_SETUP: &[SetupPart] = &carrier_setup(TWILIO_FIELDS, TWILIO_SIP_FIELDS);
const PLIVO_SETUP: &[SetupPart] = &carrier_setup(PLIVO_FIELDS, PLIVO_SIP_FIELDS);

const fn mail_domain_setup(fields: &'static [ProviderField]) -> [SetupPart; 1] {
    [SetupPart {
        id: "connection",
        kind: SetupKind::Connection,
        label: "Mail domain",
        blurb: "The domain every Agent Mailbox of this installation lives on. \
                A person makes the mailboxes; the installation holds the domain.",
        fields,
    }]
}

const MIGADU_SETUP: &[SetupPart] = &mail_domain_setup(MIGADU_FIELDS);
const MANUAL_SETUP: &[SetupPart] = &mail_domain_setup(MANUAL_FIELDS);

/// The Installation OAuth Client (ADR-0012): the Web client every
/// person of the installation consents against. A person still
/// connects their own Google account on the product port.
const GOOGLE_SETUP: &[SetupPart] = &[SetupPart {
    id: "oauth-client",
    kind: SetupKind::OauthClient,
    label: "Google OAuth client",
    blurb: "The Web OAuth client every person of this installation signs in to \
            Google with. Create it in the Google Cloud console with the redirect URI \
            below. Without one, each person on a local installation brings their own \
            Desktop client, and a server takes no Google connection.",
    fields: &[
        text("client_id", "Client ID", "Web client ID"),
        secret("client_secret", "Client secret", "Web client secret"),
    ],
}];

/// A model provider's key: the one key every person of the
/// installation thinks on.
const MODEL_KEY_SETUP: &[SetupPart] = &[SetupPart {
    id: "key",
    kind: SetupKind::ModelKey,
    label: "API key",
    blurb: "The key every person of this installation thinks on. It goes to the \
            installation's secret store, and nothing reads it back.",
    fields: &[secret("api_key", "API key", "API key")],
}];

const CATALOG: &[ProviderEntry] = &[
    ProviderEntry {
        id: pagis_google::GOOGLE_PROVIDER,
        label: "Google account",
        blurb: "Pagis signs in with an OAuth client you own. In the Google Cloud \
                console, create a project, enable the Gmail and Calendar APIs, then \
                create an OAuth client of type Desktop app and paste its two values \
                here. They reach Google once and are never stored by Pagis.",
        kind: ProviderKind::Oauth,
        fields: GOOGLE_FIELDS,
        capabilities: &[MAIL, CALENDAR],
        absent_capabilities: &[],
        max_instances: None,
        default_display_name: "Google",
        default_alias: "",
        portal: Some("the Google Cloud console"),
        installation: GOOGLE_SETUP,
    },
    ProviderEntry {
        id: TELNYX_PROVIDER,
        label: "Telnyx",
        blurb: "The installation's phone numbers are bought on its own Telnyx \
                account. Paste an API key from the Telnyx portal.",
        kind: ProviderKind::Fields,
        fields: TELNYX_FIELDS,
        capabilities: &[TELEPHONY, TEXTING],
        absent_capabilities: &[],
        // One carrier account carries every number of the Workspace
        // (ADR-0018).
        max_instances: Some(1),
        default_display_name: "Telnyx",
        default_alias: "carrier",
        portal: Some("the Telnyx portal"),
        installation: TELNYX_SETUP,
    },
    ProviderEntry {
        id: TWILIO_PROVIDER,
        label: "Twilio",
        blurb: "The installation's phone numbers are bought on its own Twilio account. \
                Paste the Account SID and an auth token from the Twilio console. \
                At Twilio, make a SIP Domain with a Credential List, and a TwiML \
                Bin that holds <Dial><Sip> on the number and <Dial> on the domain, \
                so calls route over SIP both ways and Pagis needs no public URL. \
                The SIP Domain is your own, so you type it with the SIP \
                credential. Twilio expects a public address in the media \
                description, and media behind NAT is not established, so a call \
                can be one-way audio here.",
        kind: ProviderKind::Fields,
        fields: TWILIO_FIELDS,
        capabilities: &[TELEPHONY, TEXTING],
        absent_capabilities: &[],
        // One carrier account carries every number of the Workspace
        // (ADR-0018).
        max_instances: Some(1),
        default_display_name: "Twilio",
        default_alias: "carrier",
        portal: Some("the Twilio console"),
        installation: TWILIO_SETUP,
    },
    ProviderEntry {
        id: PLIVO_PROVIDER,
        label: "Plivo",
        blurb: "The installation's phone numbers are bought on its own Plivo account. \
                Paste the Auth ID and Auth Token from the Plivo console. Plivo \
                needs an answer URL it can reach for every call, in both \
                directions, so a call on a Plivo number needs a public HTTPS \
                endpoint you run. Buying and holding numbers works without one.",
        kind: ProviderKind::Fields,
        fields: PLIVO_FIELDS,
        capabilities: &[TELEPHONY],
        // Plivo never returns the body of an inbound text, so Pagis
        // does not text on Plivo at all (ADR-0020).
        absent_capabilities: &[TEXTING],
        // One carrier account carries every number of the Workspace
        // (ADR-0018).
        max_instances: Some(1),
        default_display_name: "Plivo",
        default_alias: "carrier",
        portal: Some("the Plivo console"),
        installation: PLIVO_SETUP,
    },
    ProviderEntry {
        id: MIGADU_PROVIDER,
        label: "Migadu",
        blurb: "The installation's Agent Mailboxes are made on its own Migadu \
                account. Paste the admin account and an API key from the Migadu \
                console, and the domain the mailboxes live on. The key reaches Migadu \
                to prove it, then goes to the installation's secret store, and never \
                to the database.",
        kind: ProviderKind::Fields,
        fields: MIGADU_FIELDS,
        capabilities: &[MAILBOXES],
        absent_capabilities: &[],
        max_instances: None,
        default_display_name: "Agent mail",
        default_alias: "mail",
        portal: Some("the Migadu console"),
        installation: MIGADU_SETUP,
    },
    ProviderEntry {
        id: MANUAL_PROVIDER,
        label: "Manual mail host",
        blurb: "This host has no API, so Pagis makes no mailbox on it. Each person \
                makes their mailbox at the host, then gives it to an Agent with its \
                address and password. Pagis reads and sends over IMAP and SMTP.",
        kind: ProviderKind::Fields,
        fields: MANUAL_FIELDS,
        capabilities: &[MAILBOXES],
        absent_capabilities: &[],
        max_instances: None,
        default_display_name: "Mail host",
        default_alias: "mail-host",
        portal: None,
        installation: MANUAL_SETUP,
    },
];

/// Every provider Pagis connects, in the order the picker shows them.
pub fn catalog() -> &'static [ProviderEntry] {
    CATALOG
}

/// The catalog a person picks from on the product port: every
/// provider a person connects on their own, as one installation shows
/// it. An Installation Connection is not in it, because an
/// administrator sets one up in the Administration Interface.
///
/// `google_brokered` is true where the Org holds an Installation OAuth
/// Client. The Google entry then asks for the account alone and says
/// so, because the client is the installation's.
pub fn person_catalog(google_brokered: bool) -> Vec<ProviderEntry> {
    CATALOG
        .iter()
        .filter(|entry| !is_installation_provider(entry.id))
        .map(
            |entry| match google_brokered && entry.id == pagis_google::GOOGLE_PROVIDER {
                true => ProviderEntry {
                    fields: GOOGLE_BROKERED_FIELDS,
                    blurb: GOOGLE_BROKERED_BLURB,
                    ..*entry
                },
                false => *entry,
            },
        )
        .collect()
}

/// The entry of one provider, by the `provider` a Connection records.
pub fn entry(provider: &str) -> Option<&'static ProviderEntry> {
    CATALOG.iter().find(|entry| entry.id == provider)
}

/// Whether a Connection of this provider belongs to the Org: its entry
/// declares the Connection itself as an installation part.
///
/// The carrier connection and the mail domain are the installation's:
/// an Administrator alone sets up either in the Administration
/// Interface, and the key is filed under an installation secret name,
/// so rotating it is one write and not one per person. Every other
/// provider, a person's Google account above all, stays the Person's
/// own.
pub fn is_installation_provider(provider: &str) -> bool {
    entry(provider).is_some_and(|entry| {
        entry
            .installation
            .iter()
            .any(|part| part.kind == SetupKind::Connection)
    })
}

/// The group the Administration Interface shows one entry in.
fn group_of(entry: &ProviderEntry) -> &'static str {
    if entry.capabilities.contains(&TELEPHONY) {
        TELEPHONY
    } else if entry.capabilities.contains(&MAILBOXES) {
        MAILBOXES
    } else {
        "accounts"
    }
}

/// The label of a model provider, as the Administration Interface
/// names it.
fn model_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "Anthropic",
        Provider::OpenAi => "OpenAI",
        Provider::OpenRouter => "OpenRouter",
    }
}

/// Every provider the installation sets up, in the order the
/// Administration Interface shows them: the model providers first, then
/// each catalog entry that declares an installation part.
pub fn installation_setups() -> Vec<InstallationSetup> {
    PROVIDERS
        .into_iter()
        .map(|provider| InstallationSetup {
            provider: provider.id(),
            label: model_label(provider),
            group: "models",
            parts: MODEL_KEY_SETUP,
        })
        .chain(
            CATALOG
                .iter()
                .filter(|entry| !entry.installation.is_empty())
                .map(|entry| InstallationSetup {
                    provider: entry.id,
                    label: entry.label,
                    group: group_of(entry),
                    parts: entry.installation,
                }),
        )
        .collect()
}

/// The installation setup of one provider, or `None` where the
/// installation sets up nothing of it.
pub fn installation_setup(provider: &str) -> Option<InstallationSetup> {
    installation_setups()
        .into_iter()
        .find(|setup| setup.provider == provider)
}

/// The capabilities a Connection of this provider gives an Agent. A
/// provider the catalog does not list gives none.
pub fn capabilities(provider: &str) -> Vec<String> {
    entry(provider)
        .map(|entry| {
            entry
                .capabilities
                .iter()
                .map(|capability| capability.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// What a Connection of this provider does not do, and a sibling
/// provider does (ADR-0005, ADR-0020).
pub fn absent_capabilities(provider: &str) -> Vec<String> {
    entry(provider)
        .map(|entry| {
            entry
                .absent_capabilities
                .iter()
                .map(|capability| capability.to_string())
                .collect()
        })
        .unwrap_or_default()
}

impl NewCredentials {
    /// The secrets one provider needs, read from the fields its entry
    /// declares. A request that names a provider and brings another's
    /// values is refused here, before anything is written.
    pub fn from_fields(
        provider: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<NewCredentials, ConnectError> {
        let field = |key: &str| -> Result<String, ConnectError> {
            fields
                .get(key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    ConnectError::Validation(format!("{key} is required for this provider"))
                })
        };
        let port = |key: &str| -> Result<u16, ConnectError> {
            field(key)?
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .ok_or_else(|| ConnectError::Validation(format!("{key} is a port number")))
        };
        match provider {
            // The Desktop client fields are absent on a brokered
            // installation: the entry the picker drew has no place to
            // type them, because the Org's Web client is the one every
            // person consents against.
            pagis_google::GOOGLE_PROVIDER => Ok(NewCredentials::Google {
                account: field("account")?,
                client: match (fields.get("client_id"), fields.get("client_secret")) {
                    (None, None) => None,
                    _ => Some(crate::DesktopClient {
                        client_id: field("client_id")?,
                        client_secret: field("client_secret")?,
                    }),
                },
            }),
            // A Telnyx key is a bearer token: the secret is the whole
            // credential, and the account is empty.
            TELNYX_PROVIDER => Ok(NewCredentials::Carrier {
                provider: TELNYX_PROVIDER.to_string(),
                account: String::new(),
                secret: field("api_key")?,
            }),
            // Twilio signs with HTTP Basic: the Account SID is the
            // account and names it in the path, and the auth token is
            // the secret.
            TWILIO_PROVIDER => Ok(NewCredentials::Carrier {
                provider: TWILIO_PROVIDER.to_string(),
                account: field("account_sid")?,
                secret: field("auth_token")?,
            }),
            // Plivo signs with the Auth ID and the Auth Token, and the
            // Auth ID also names the account in every request path.
            PLIVO_PROVIDER => Ok(NewCredentials::Carrier {
                provider: PLIVO_PROVIDER.to_string(),
                account: field("auth_id")?,
                secret: field("auth_token")?,
            }),
            MIGADU_PROVIDER => Ok(NewCredentials::Migadu {
                account: field("account")?,
                api_key: field("api_key")?,
                domain: field("domain")?,
            }),
            MANUAL_PROVIDER => Ok(NewCredentials::Manual {
                domain: field("domain")?,
                imap: Endpoint::new(field("imap_host")?, port("imap_port")?),
                smtp: Endpoint::new(field("smtp_host")?, port("smtp_port")?),
            }),
            other => Err(ConnectError::Validation(format!(
                "{other} is not a provider Pagis connects"
            ))),
        }
    }
}

#[cfg(test)]
mod installation_tests {
    use super::*;

    /// The carrier connections and the mail domains are the
    /// installation's, and nothing else is. The rule is the entry's own
    /// declaration, so each provider that gives numbers or mailboxes
    /// must declare it.
    #[test]
    fn every_provider_of_numbers_or_mailboxes_is_an_installation_connection() {
        for entry in CATALOG {
            let installation =
                entry.capabilities.contains(&TELEPHONY) || entry.capabilities.contains(&MAILBOXES);
            assert_eq!(
                is_installation_provider(entry.id),
                installation,
                "{} gives {:?}",
                entry.id,
                entry.capabilities
            );
        }
        assert!(!is_installation_provider("no-such-provider"));
    }

    /// The Connection part of an entry takes the entry's own fields, so
    /// the parser reads what the Administration Interface draws.
    #[test]
    fn a_connection_part_takes_the_fields_of_its_entry() {
        for entry in CATALOG {
            for part in entry.installation {
                if part.kind == SetupKind::Connection {
                    assert_eq!(part.fields, entry.fields, "{}", entry.id);
                }
            }
        }
    }
}
