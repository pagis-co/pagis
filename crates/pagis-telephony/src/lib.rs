//! Telephony: the carrier seams and the Agent Phone Number.
//!
//! Telephony has three duties with different credentials and different
//! failure modes, so it has three seams (ADR-0020, ADR-0020).
//! [`NumberCatalog`] searches, buys and releases a number with the REST
//! API key. [`CallTransport`] is signaling and media, and uses the SIP
//! credential instead; the API key stays out of the call path.
//! [`TextTransport`] carries a text out, polls the texts that came in
//! and holds the carrier's messaging object in place; it signs with the
//! REST API key, as the catalog does, and holds a cursor per number,
//! which the catalog does not. A carrier that carries no text gets
//! [`NoTextTransport`], which declares texting absent and refuses every
//! call (ADR-0005).
//!
//! [`NumberDesk`] is the lifecycle above the catalog: buy, assign,
//! unassign and release, with the refusals and the audit trail
//! ADR-0018 states. It also holds the two emergency guards: Pagis
//! never calls an emergency number, and [`emergency`] is the predicate
//! that decides. [`Endpoints`] is the registry of the lines, one for
//! each carrier Connection, that keep the carrier registered and route
//! each inbound call by the number that was dialed.
//!
//! [`tiers`] owns the Trust Tier of one call (ADR-0021): the
//! Trust List proposes it, the [`keypad`] challenge confirms it before
//! the realtime session exists, and only the user drops it.
//! [`hub::MediaHub`] owns the media of one call: both directions of audio,
//! the typed events, and the keypad digits both ways.
//!
//! [`PhoneToolRuntime`] executes the one call tool (ADR-0020), and
//! [`InboundCalls`] runs the calls the lines answer.
//! It writes the [`CallBrief`] the model never writes, binds the
//! session tools of [`session`] only while a call is live in
//! [`LiveCalls`], and hands the call to a [`CallBridge`].

pub mod audio;
mod bridge;
mod brief;
mod call_session;
mod call_tools;
mod calls;
mod catalog;
pub mod dtmf;
pub mod emergency;
mod endpoint;
pub mod events;
pub mod fake;
pub mod hub;
mod inbound;
pub mod keypad;
pub mod leg;
pub mod listen;
mod live;
pub mod model;
pub mod model_fake;
mod model_router;
mod no_text;
mod numbers;
mod phone_tool;
mod plivo;
mod realtime_bridge;
pub mod recording;
mod reorder;
pub mod session;
pub mod sip_media;
mod sip_transport;
mod telnyx;
mod telnyx_text;
pub mod text;
pub mod tiers;
mod transport;
mod twilio;
mod twilio_text;

// The Call vocabulary is the record's, so it lives in `pagis-core` and
// is re-exported here where the telephony code reads it.
pub use pagis_core::{CallDirection, CallOutcome, Classification, TrustTier};

pub use bridge::{BridgeError, CallBridge, CallLog, CallReport, FakeCallBridge, PlacedCall};
pub use brief::{
    CLASSIFY_PROMPT, CallArguments, CallBrief, DEFAULT_DURATION_CAP, EMERGENCY_RULE,
    IVR_MODE_PROMPT, REMOTE_PARTY_SOURCE, REPORT_ANSWER, TAKE_A_MESSAGE, VoicemailPolicy,
    WRAP_UP_LEAD, report_answer_tool, tier_line, wrap_up_prompt,
};
pub use call_session::{
    AGENT_HANGUP, CallHandle, CallSession, CallSessionDeps, DAEMON_RESTART, DURATION_CAP,
    MODEL_UNAVAILABLE,
};
pub use call_tools::{BrokerCallTools, CallTools};
pub use calls::{ActiveCalls, NoActiveCalls};
pub use catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberCatalogs,
    NumberSearch, PurchasedNumber,
};
pub use emergency::EmergencyRefused;
pub use endpoint::{
    CallError, EndpointTask, Endpoints, EndpointsDeps, INITIAL_BACKOFF, IncomingHub, MAX_BACKOFF,
    NumberDirectory, REQUESTED_EXPIRY, RegistrationFailure, RegistrationState, SIP_DOMAIN_KEY,
    SIP_USERNAME_KEY, sip_identity,
};
pub use events::{
    CALL_ENDED, CALL_MATCHER, CallEndedMatcher, CallEvents, CallIngestError,
    STANDING_CALL_INSTRUCTION, STANDING_CALL_RULE_NAME, SettledCall, StandingCallRule,
    StandingRuleError, call_ended_event,
};
pub use inbound::{InboundCalls, InboundCallsDeps, InboundError, InboundRuns};
pub use keypad::{CodeCheck, Keypad, NoCode};
pub use live::{CallActive, LiveCallGuard, LiveCalls};
pub use model_router::{
    GPT_LIVE_REASONING_ALIAS, GPT_LIVE_REASONING_MODELS, KeyedModelSessions, PHONE_ALIAS,
    PHONE_CLASSIFIER_ALIAS, PHONE_CLASSIFIER_MODELS, PHONE_MODEL_SETTINGS, PHONE_MODELS,
    PhoneModelSetting, REALTIME_MODEL, RouterModelSessions,
};
pub use no_text::NoTextTransport;
pub use numbers::{NumberDesk, NumberDeskDeps, NumberError, normalize_e164};
pub use phone_tool::{
    CALL_ACTIVE, CALL_FAILED, PHONE_NOT_ASSIGNED, PhoneToolDeps, PhoneToolRuntime, RunTools,
};
pub use plivo::PlivoNumberCatalog;
pub use realtime_bridge::{RealtimeBridge, RealtimeBridgeDeps};
pub use sip_transport::{SipCallTransport, dialed_e164, register_contact};
pub use telnyx::TelnyxNumberCatalog;
pub use telnyx_text::{
    TELNYX_KV_NAMESPACE_KEY, TELNYX_MESSAGING_PROFILE_KEY, TELNYX_RELAY_URL_KEY,
    TelnyxTextTransport,
};
pub use text::{
    InboundText, Prepared, SentText, TextCapabilities, TextError, TextTransport, TextTransports,
};
pub use tiers::{LiveTiers, TierCause, TierChange, TierGate, candidate_tier, settle_inbound};
pub use transport::{
    Answer, CallTransport, IncomingCall, Line, Opened, Refusal, SipCredential,
    TransportCapabilities, TransportError, TransportErrorCode,
};
pub use twilio::TwilioNumberCatalog;
pub use twilio_text::{TWILIO_MESSAGING_SERVICE_KEY, TwilioTextTransport};

/// The carrier providers Pagis connects (ADR-0020). A Workspace
/// has one carrier Connection, of any of these, and it carries every
/// number (ADR-0018). They live in the broker because the `call.ended`
/// declaration names each of them, and a manifest cannot depend on
/// this crate.
pub use pagis_broker::{
    CARRIER_PROVIDERS, PLIVO_PROVIDER, TELNYX_PROVIDER, TWILIO_PROVIDER, is_carrier,
};

/// Where the secret store files one carrier account's API secret. The
/// secret never reaches the database, the model or a log line.
///
/// The carrier connection belongs to the installation and not to one
/// Person, so the name carries no Workspace: one Org holds one
/// carrier account and an Administrator alone configures it. Rotating
/// the key is therefore one write, not one per person.
pub fn carrier_key_secret_name(provider: &str, alias: &str) -> String {
    format!("carrier/{provider}/api_key/{alias}")
}

/// Where the secret store files one carrier account's SIP password.
/// The username and the registrar are not secret and live in the
/// Connection's `config`; the password leaves the store only to the
/// transport. The name carries no Workspace, as the carrier key does.
pub fn sip_password_secret_name(provider: &str, alias: &str) -> String {
    format!("carrier/{provider}/sip_password/{alias}")
}

/// The `config` key that holds a carrier account's id, the public half
/// of its [`CarrierKey`]. Telnyx has none, so the key is absent there.
pub const CARRIER_ACCOUNT_KEY: &str = "account";
