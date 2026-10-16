//! Session tools and live media stay within the Call's Workspace.
use pagis_broker::ToolDef;
use pagis_core::{
    Agent, AgentStatus, CallId, ConnectionId, PhoneNumber, PhoneNumberId, TrustTier, WorkspaceId,
    now_ms,
};
use pagis_telephony::fake::{FakeCallTransport, TokioClock};
use pagis_telephony::hub::{DtmfPresence, MediaHub};
use pagis_telephony::session::{HANG_UP, SEND_DIGITS};
use pagis_telephony::{CallBrief, CallTransport, LiveCalls, SipCredential, TransportCapabilities};
use std::sync::Arc;
const OWN: &str = "+14155550123";
const REMOTE: &str = "+16505550100";
fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: name.into(),
        parameters: serde_json::json!({"type":"object","properties":{}}),
    }
}
fn names(tools: &[ToolDef]) -> Vec<&str> {
    tools.iter().map(|tool| tool.name.as_str()).collect()
}

#[tokio::test]
async fn a_session_tool_exists_only_while_the_call_is_live() {
    let live = Arc::new(LiveCalls::default());
    let number_id = PhoneNumberId::generate();
    assert!(live.session_tools(&number_id).is_none());
    let workspace_id = WorkspaceId::generate();
    let guard = live
        .begin(
            &workspace_id,
            &number_id,
            &CallId::generate(),
            vec![tool(HANG_UP)],
        )
        .unwrap();
    assert_eq!(
        names(&live.session_tools(&number_id).unwrap()),
        vec![HANG_UP]
    );
    // One active call per number (ADR-0020).
    let second = live.begin(&workspace_id, &number_id, &CallId::generate(), Vec::new());
    assert!(second.is_err());
    drop(guard);
    assert!(live.session_tools(&number_id).is_none());
}

/// The media hub of a live call is found only under the Workspace of
/// the call (ADR-0023). Another Workspace that names the Call id finds
/// no hub, so it can neither listen to the call nor hang it up.
#[tokio::test]
async fn a_live_hub_is_found_only_under_the_workspace_of_its_call() {
    let transport = FakeCallTransport::new(Arc::new(TokioClock));
    let mut opened = transport
        .open(&SipCredential::new("robin", "secret", "sip.telnyx.com"))
        .await
        .unwrap();
    let hub = MediaHub::start(opened.line.dial(OWN, REMOTE).await.unwrap());
    let _party = transport.dials().pop().expect("one dialed party");
    let live = Arc::new(LiveCalls::default());
    let owner = WorkspaceId::generate();
    let other = WorkspaceId::generate();
    let call_id = CallId::generate();
    let guard = live
        .begin(&owner, &PhoneNumberId::generate(), &call_id, Vec::new())
        .unwrap();
    live.attach_hub(&owner, &call_id, Arc::clone(&hub));

    assert!(live.hub(&other, &call_id).is_none());
    let own = live.hub(&owner, &call_id).expect("the hub of the call");
    assert!(Arc::ptr_eq(&own, &hub));
    assert!(!hub.is_ended());

    // The line frees when the call ends, and the hub goes with it.
    drop(guard);
    assert!(live.hub(&owner, &call_id).is_none());
}

/// A brief of an inbound call, for the session-tool tests.
fn a_brief() -> CallBrief {
    let agent = Agent {
        id: pagis_core::AgentId::generate(),
        workspace_id: WorkspaceId::generate(),
        name: "Robin".to_string(),
        job: "assistant".to_string(),
        description: String::new(),
        personality: "warm".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    let number = PhoneNumber::new(
        PhoneNumberId::generate(),
        agent.workspace_id.clone(),
        ConnectionId::generate(),
        OWN.to_string(),
        "carrier".to_string(),
        now_ms(),
    );
    CallBrief::inbound(
        &agent,
        "Sam",
        &number,
        REMOTE,
        TrustTier::Unknown,
        vec![tool("memory_read")],
    )
}

#[tokio::test]
async fn send_digits_is_absent_when_the_carrier_cannot_send_dtmf() {
    let brief = a_brief();
    let no_dtmf = TransportCapabilities {
        send_dtmf: false,
        answering_machine_detection: false,
        wideband_audio: false,
        public_ingress_required: false,
    };
    let tools = pagis_telephony::session::session_tools(&brief, no_dtmf, None);
    assert_eq!(names(&tools), vec![HANG_UP]);
}

#[tokio::test]
async fn send_digits_is_absent_when_the_negotiated_leg_has_no_telephone_event() {
    let brief = a_brief();
    let can_send = TransportCapabilities {
        send_dtmf: true,
        answering_machine_detection: false,
        wideband_audio: false,
        public_ingress_required: false,
    };
    // The transport can send digits, but this call's leg negotiated no
    // telephone-event payload type (ADR-0005).
    let absent = DtmfPresence {
        send_dtmf: false,
        receive_dtmf: false,
    };
    let tools = pagis_telephony::session::session_tools(&brief, can_send, Some(absent));
    assert_eq!(names(&tools), vec![HANG_UP]);

    let present = DtmfPresence {
        send_dtmf: true,
        receive_dtmf: true,
    };
    let tools = pagis_telephony::session::session_tools(&brief, can_send, Some(present));
    assert_eq!(names(&tools), vec![HANG_UP, SEND_DIGITS]);
}

#[tokio::test]
async fn the_answered_call_rebinds_the_tools_of_its_negotiated_leg() {
    let live = Arc::new(LiveCalls::default());
    let number_id = PhoneNumberId::generate();
    assert!(!live.rebind(&number_id, vec![tool(HANG_UP)]));
    let _guard = live
        .begin(
            &WorkspaceId::generate(),
            &number_id,
            &CallId::generate(),
            vec![tool(HANG_UP), tool(SEND_DIGITS)],
        )
        .unwrap();

    assert!(live.rebind(&number_id, vec![tool(HANG_UP)]));

    assert_eq!(
        names(&live.session_tools(&number_id).unwrap()),
        vec![HANG_UP]
    );
}
