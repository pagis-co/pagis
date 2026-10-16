use std::time::Duration;

use pagis_broker::ToolDef;
use pagis_core::{
    AgentId, CallDirection, CallId, CallOutcome, Classification, FileId, PhoneNumberId, RunId,
    Speaker, TranscriptLine, TrustTier, WorkspaceId, WorkspaceKind,
};
use pagis_telephony::{BridgeError, CallBrief, CallReport, PlacedCall, VoicemailPolicy};
use serde_json::json;

fn brief() -> CallBrief {
    CallBrief {
        direction: CallDirection::Outbound,
        agent_id: AgentId::generate(),
        agent_name: "Robin".into(),
        person_name: "Sam".into(),
        workspace_kind: WorkspaceKind::Private,
        personality: "warm".into(),
        voice: Some("marin".into()),
        phone_number_id: PhoneNumberId::generate(),
        own_e164: "+14155550123".into(),
        remote_e164: "+14155550199".into(),
        tier: TrustTier::Trusted,
        purpose: "Book a table".into(),
        success_criteria: Some("A table is booked".into()),
        voicemail: VoicemailPolicy::LeaveMessage,
        duration_cap: Duration::from_secs(600),
        tools: vec![tool()],
        ivr_mode_prompt: "Choose one digit".into(),
        classify_prompt: "Identify the answer".into(),
        emergency_rule: "Use a telephone for emergency services".into(),
    }
}

fn tool() -> ToolDef {
    ToolDef {
        name: "memory_read".into(),
        description: "Read memory".into(),
        parameters: json!({"type": "object"}),
    }
}

#[test]
fn call_brief_round_trips_json() {
    let brief = brief();
    let value = serde_json::to_value(&brief).unwrap();
    assert_eq!(value["duration_cap"].as_u64(), Some(600));
    assert_eq!(value["voicemail"], "leave_message");
    assert_eq!(serde_json::from_value::<CallBrief>(value).unwrap(), brief);
}

#[test]
fn placed_call_round_trips_json() {
    let call = PlacedCall {
        id: CallId::generate(),
        workspace_id: WorkspaceId::generate(),
        run_id: RunId::generate(),
        brief: brief(),
        tools: vec![tool()],
    };
    let value = serde_json::to_value(&call).unwrap();
    assert_eq!(value["brief"]["duration_cap"].as_u64(), Some(600));
    assert_eq!(serde_json::from_value::<PlacedCall>(value).unwrap(), call);
}

#[test]
fn call_report_round_trips_json() {
    let report = CallReport {
        outcome: CallOutcome::Answered,
        tier: TrustTier::Trusted,
        ended_reason: "hangup".into(),
        classification: Some(Classification::Human),
        message_left: false,
        duration: Duration::from_secs(42),
        ringing_at: Some(1000),
        answered_at: Some(2000),
        transcript: vec![TranscriptLine::new(3000, Speaker::Caller, "Hello")],
        recording_file_id: Some(FileId::generate()),
    };
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["duration"].as_u64(), Some(42));
    assert_eq!(serde_json::from_value::<CallReport>(value).unwrap(), report);
}

#[test]
fn bridge_error_round_trips_json() {
    let error = BridgeError("The model is unavailable".into());
    let value = serde_json::to_value(&error).unwrap();
    assert_eq!(value, "The model is unavailable");
    assert_eq!(serde_json::from_value::<BridgeError>(value).unwrap(), error);
}

#[test]
fn duration_cap_serializes_as_whole_seconds() {
    let mut brief = brief();
    brief.duration_cap = Duration::from_millis(1500);
    let value = serde_json::to_value(&brief).unwrap();
    assert_eq!(value["duration_cap"].as_u64(), Some(1));
    let decoded: CallBrief = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.duration_cap, Duration::from_secs(1));
}
