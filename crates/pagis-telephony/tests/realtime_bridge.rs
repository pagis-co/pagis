//! The realtime bridge (ADR-0020) on a paused clock: the classify
//! phase, the tool set the tier binds, barge-in, the duration cap and
//! every ended reason. The carrier is the fake transport and the model
//! server is the fake peer, so no socket opens and no call is placed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_broker::{ToolCall, ToolDef, ToolResult};
use pagis_core::{
    AgentId, CallDirection, CallId, CallOutcome, Classification, Event, EventBus, EventId,
    EventScope, EventStream, NewEvent, PhoneNumberId, RunId, Speaker, StoreError, SystemClock,
    TrustTier, WorkspaceId, now_ms, render_transcript,
};
use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};
use pagis_telephony::fake::{
    FakeCallTransport, FakeKeypadFailures, FakeNumberDirectory, RemoteParty, TokioClock,
};
use pagis_telephony::hub::MediaHub;
use pagis_telephony::keypad::CodeCheck;
use pagis_telephony::model_fake::{FakeModelSessions, ModelPeer};
use pagis_telephony::session::{HANG_UP, SEND_DIGITS};
use pagis_telephony::{
    AGENT_HANGUP, CallBrief, CallHandle, CallLog, CallReport, CallSession, CallSessionDeps,
    CallTools, CallTransport, DAEMON_RESTART, DEFAULT_DURATION_CAP, DURATION_CAP, EMERGENCY_RULE,
    EndpointTask, IVR_MODE_PROMPT, IncomingHub, Keypad, MODEL_UNAVAILABLE, PlacedCall,
    REPORT_ANSWER, RegistrationState, SipCredential, TierGate, TransportCapabilities,
    VoicemailPolicy, WRAP_UP_LEAD, wrap_up_prompt,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+14155550199";

/// A bus that keeps what it was told and wakes nobody.
#[derive(Default)]
struct RecordingBus {
    events: Mutex<Vec<NewEvent>>,
}

impl RecordingBus {
    /// The payloads of the events of one type, in order.
    fn payloads_of(&self, event_type: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.event_type == event_type)
            .map(|event| event.payload.clone())
            .collect()
    }

    fn types(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|event| event.event_type.clone())
            .collect()
    }

    /// The text of each `call.transcript` line, in order.
    fn transcript_texts(&self) -> Vec<String> {
        self.payloads_of("call.transcript")
            .iter()
            .map(|payload| payload["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// True when the text holds no digit. A record of the call never shows
/// what the caller typed at the keypad (ADR-0021).
fn holds_no_digit(text: &str) -> bool {
    !text.chars().any(|character| character.is_ascii_digit())
}

#[async_trait]
impl EventBus for RecordingBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        let mut events = self.events.lock().unwrap();
        events.push(event.clone());
        Ok(Event {
            id: EventId::generate(),
            seq: events.len() as i64,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        })
    }

    async fn subscribe(&self, _scope: EventScope, _after_seq: Option<i64>) -> EventStream {
        Box::pin(futures::stream::empty())
    }
}

/// A broker that answers every tool call from a script and keeps what
/// it was asked.
#[derive(Default)]
struct RecordingTools {
    calls: Mutex<Vec<ToolCall>>,
}

#[async_trait]
impl CallTools for RecordingTools {
    async fn invoke(
        &self,
        _workspace: &WorkspaceId,
        _agent: &AgentId,
        _run: &RunId,
        call: ToolCall,
    ) -> ToolResult {
        self.calls.lock().unwrap().push(call);
        ToolResult::success("done")
    }
}

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: name.to_string(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn brief(direction: CallDirection, tier: TrustTier) -> CallBrief {
    CallBrief {
        direction,
        agent_id: AgentId::generate(),
        agent_name: "Robin".to_string(),
        voice: Some("marin".to_string()),
        phone_number_id: PhoneNumberId::generate(),
        own_e164: OWN.to_string(),
        remote_e164: REMOTE.to_string(),
        tier,
        purpose: "book a table for four".to_string(),
        success_criteria: Some("a table is booked".to_string()),
        voicemail: VoicemailPolicy::HangUp,
        duration_cap: DEFAULT_DURATION_CAP,
        tools: vec![tool("memory_read")],
        ivr_mode_prompt: IVR_MODE_PROMPT,
        classify_prompt: pagis_telephony::CLASSIFY_PROMPT,
        emergency_rule: EMERGENCY_RULE,
    }
}

fn placed(brief: CallBrief) -> PlacedCall {
    PlacedCall {
        id: CallId::generate(),
        workspace_id: WorkspaceId::generate(),
        run_id: RunId::generate(),
        brief,
        tools: Vec::new(),
    }
}

/// The carrier side of one call: the Remote Party and the hub the
/// endpoint task handed over.
struct Line {
    transport: Arc<FakeCallTransport>,
    party: Arc<RemoteParty>,
    hub: Arc<MediaHub>,
    task: EndpointTask,
}

impl Line {
    /// Register the line. A caller `from` rings it and the task
    /// answers; with no caller, the line dials out.
    async fn open(from: Option<&str>) -> Self {
        let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
        let (answered, mut incoming) = tokio::sync::mpsc::channel(1);
        let own = FakeNumberDirectory::held(OWN);
        let task = EndpointTask::spawn(
            Arc::clone(&transport) as _,
            Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
            Arc::new(FakeNumberDirectory::with(vec![own.clone()])),
            answered,
        );
        let mut watch = task.watch();
        watch
            .wait_for(|state| *state == RegistrationState::Registered)
            .await
            .expect("the line registers");

        let (hub, party) = match from {
            Some(from) => {
                let party = transport.ring(OWN, from);
                let hub: IncomingHub = incoming.recv().await.expect("the task answers the call");
                (hub.hub, party)
            }
            None => {
                let hub = task.place_call(&own, REMOTE).await.expect("the line dials");
                (hub, transport.dials()[0].clone())
            }
        };
        Self {
            transport,
            party,
            hub,
            task,
        }
    }
}

/// One call under test: the media of the fake carrier, the fake model
/// server, and the pump between them.
struct Call {
    party: Arc<RemoteParty>,
    sessions: Arc<FakeModelSessions>,
    tools: Arc<RecordingTools>,
    bus: Arc<RecordingBus>,
    cancel: CancellationToken,
    handle: CallHandle,
    report: tokio::task::JoinHandle<CallReport>,
    /// Held so the line stays open for the length of the call.
    _task: EndpointTask,
}

impl Call {
    /// A call the Agent placed, pumped from the moment the model
    /// session opened.
    async fn outbound(brief: CallBrief) -> Self {
        Self::start(brief, None, None).await
    }

    /// A call that arrived on the Agent's line and was answered.
    async fn inbound(brief: CallBrief) -> Self {
        Self::start(brief, Some(REMOTE), None).await
    }

    /// An inbound call the bridge bound a tier gate to, so a digit that
    /// arrives during the call raises the tier (ADR-0021).
    async fn inbound_gated(brief: CallBrief, gate: Arc<TierGate>) -> Self {
        Self::start(brief, Some(REMOTE), Some(gate)).await
    }

    /// An inbound call whose caller types `presses` at the keypad
    /// challenge, before the session exists. The session opens at the
    /// tier the challenge proved, as the bridge opens it (ADR-0021).
    async fn challenged(mut brief: CallBrief, gate: Arc<TierGate>, presses: &str) -> Self {
        let line = Line::open(Some(REMOTE)).await;
        let party = Arc::clone(&line.party);
        let presses = presses.to_string();
        let typing = tokio::spawn(async move {
            for digit in presses.chars() {
                tokio::time::sleep(Duration::from_millis(200)).await;
                party.press(digit);
            }
        });
        brief.tier = gate.challenge(&line.hub).await;
        typing.await.expect("the caller typed");
        Self::open_session(line, brief, Some(gate)).await
    }

    async fn start(brief: CallBrief, from: Option<&str>, gate: Option<Arc<TierGate>>) -> Self {
        Self::open_session(Line::open(from).await, brief, gate).await
    }

    /// Open the model session on the line and pump the call.
    async fn open_session(line: Line, brief: CallBrief, gate: Option<Arc<TierGate>>) -> Self {
        let Line {
            transport,
            party,
            hub,
            task,
        } = line;
        let sessions = Arc::new(FakeModelSessions::new());
        let tools = Arc::new(RecordingTools::default());
        let bus = Arc::new(RecordingBus::default());
        let cancel = CancellationToken::new();
        let call = placed(brief);
        let log = CallLog::new(Arc::clone(&bus) as _, &call);
        let session = CallSession::open(CallSessionDeps {
            workspace_id: call.workspace_id.clone(),
            brief: call.brief.clone(),
            gate,
            run_id: call.run_id.clone(),
            capabilities: transport.capabilities(),
            sessions: Arc::clone(&sessions) as _,
            tools: Arc::clone(&tools) as _,
            cancel: cancel.clone(),
        })
        .await
        .expect("the model session opens");
        let handle = session.handle();
        let report = tokio::spawn(async move { session.run(hub, &log).await });
        Self {
            party,
            sessions,
            tools,
            bus,
            cancel,
            handle,
            report,
            _task: task,
        }
    }

    /// Speak G.711 silence for the length of the call, so the media
    /// timeout of the hub does not end a test about something else.
    fn keep_the_media_alive(&self) {
        let party = Arc::clone(&self.party);
        tokio::spawn(async move {
            loop {
                party.speak(Frame::silence(Codec::Pcmu));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }

    fn peer(&self) -> Arc<ModelPeer> {
        self.sessions.peer()
    }

    async fn finished(self) -> CallReport {
        tokio::time::timeout(Duration::from_secs(30), self.report)
            .await
            .expect("the call ends")
            .expect("the pump did not panic")
    }
}

/// The tool names one `session.update` binds.
fn bound_tools(update: &Value) -> Vec<String> {
    update["session"]["tools"]
        .as_array()
        .expect("a tool array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn instructions(update: &Value) -> String {
    update["session"]["instructions"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The text of every system message the bridge sent, in order.
fn system_messages(peer: &ModelPeer) -> Vec<String> {
    peer.sent_of("conversation.item.create")
        .iter()
        .filter(|event| event["item"]["role"] == "system")
        .map(|event| {
            event["item"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Yield until `done` holds, or panic after a bounded number of turns.
/// The pump answers an event in a few turns, so a test on a paused
/// clock never sleeps for one.
async fn eventually(what: &str, done: impl Fn() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{what}");
}

#[tokio::test(start_paused = true)]
async fn an_outbound_session_classifies_before_it_listens() {
    let call = Call::outbound(brief(CallDirection::Outbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    let first = peer.wait_for("session.update").await;

    // The classify phase binds one tool and nothing else.
    assert_eq!(bound_tools(&first), vec![REPORT_ANSWER.to_string()]);
    assert_eq!(
        first["session"]["audio"]["input"]["format"]["type"],
        "audio/pcmu"
    );
    assert_eq!(
        first["session"]["audio"]["output"]["format"]["type"],
        "audio/pcmu"
    );
    assert_eq!(first["session"]["audio"]["output"]["voice"], "marin");
    assert_eq!(
        first["session"]["audio"]["input"]["turn_detection"]["type"],
        "server_vad"
    );

    // The line rings, and no audio reaches the model until it answers.
    call.party.ring();
    call.party.speak(Frame::silence(Codec::Pcmu));
    tokio::task::yield_now().await;
    assert!(peer.sent_of("input_audio_buffer.append").is_empty());

    call.party.answer();
    call.party.speak(Frame::silence(Codec::Pcmu));
    peer.wait_for("input_audio_buffer.append").await;

    // The verdict swaps the instructions and the real tool set in.
    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "human"}),
    );
    tokio::task::yield_now().await;
    let updates = peer.sent_of("session.update");
    assert_eq!(updates.len(), 2, "one swap on the verdict");
    let bound = bound_tools(&updates[1]);
    assert_eq!(bound, vec![HANG_UP, SEND_DIGITS, "memory_read"]);
    assert!(instructions(&updates[1]).contains("book a table for four"));

    call.party.hangup();
    let bus = Arc::clone(&call.bus);
    let report = call.finished().await;
    assert_eq!(report.classification, Some(Classification::Human));
    assert_eq!(report.outcome, CallOutcome::Answered);
    assert_eq!(report.ended_reason, "remote_hangup");
    assert!(bus.types().contains(&"call.answered".to_string()));
}

#[tokio::test(start_paused = true)]
async fn an_inbound_call_skips_the_classify_phase() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    let first = peer.wait_for("session.update").await;

    assert_eq!(
        bound_tools(&first),
        vec![HANG_UP, SEND_DIGITS, "memory_read"]
    );
    peer.caller_transcript("is that the vet?");
    peer.agent_transcript("no, this is Robin.");
    call.party.hangup();

    let report = call.finished().await;
    assert_eq!(report.classification, None);
    assert_eq!(report.outcome, CallOutcome::Answered);
    assert!(render_transcript(&report.transcript).contains("caller: is that the vet?"));
    assert!(render_transcript(&report.transcript).contains("agent: no, this is Robin."));
    assert!(report.answered_at.is_some());
}

#[tokio::test(start_paused = true)]
async fn the_unknown_tier_binds_no_brief_tool_and_a_raised_tier_binds_them() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Unknown)).await;
    let peer = call.peer();
    let first = peer.wait_for("session.update").await;
    assert_eq!(bound_tools(&first), vec![HANG_UP, SEND_DIGITS]);
    assert!(instructions(&first).contains("not identified"));

    // The keypad code arrived: the tier rises in place.
    call.handle.set_tier(TrustTier::Trusted).await;
    tokio::task::yield_now().await;
    let updates = peer.sent_of("session.update");
    assert_eq!(updates.len(), 2);
    assert_eq!(
        bound_tools(&updates[1]),
        vec![HANG_UP, SEND_DIGITS, "memory_read"]
    );

    call.party.hangup();
    let report = call.finished().await;
    assert!(render_transcript(&report.transcript).contains("[tier: unknown]"));
    assert!(render_transcript(&report.transcript).contains("[tier: trusted]"));
}

#[tokio::test(start_paused = true)]
async fn barge_in_clears_the_downlink_and_reports_what_was_heard() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;

    // The model speaks a full second: the hub holds 60 ms and paces the
    // rest, so most of it is still queued.
    peer.speak("item-1", &vec![0xFF; FRAME_BYTES * 50]);
    tokio::time::sleep(Duration::from_millis(60)).await;
    peer.speech_started();
    let truncate = peer.wait_for("conversation.item.truncate").await;

    assert_eq!(truncate["item_id"], "item-1");
    let heard = truncate["audio_end_ms"].as_u64().expect("a number");
    assert!(heard < 1000, "the caller heard {heard} ms of one second");
    assert_eq!(heard % 20, 0, "one packet is 20 ms");

    call.party.hangup();
    call.finished().await;
}

#[tokio::test(start_paused = true)]
async fn a_session_tool_acts_on_the_call_and_never_on_the_broker() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;

    peer.function_call("c1", SEND_DIGITS, serde_json::json!({"digits": "12"}));
    let output = peer.wait_for("conversation.item.create").await;
    assert_eq!(output["item"]["type"], "function_call_output");
    assert!(!peer.sent_of("response.create").is_empty());

    // The presses go out one RTP packet at a time, so the call waits.
    tokio::time::sleep(Duration::from_millis(500)).await;
    peer.function_call("c2", HANG_UP, serde_json::json!({"reason": "done"}));
    let party = Arc::clone(&call.party);
    let tools = Arc::clone(&call.tools);
    let report = call.finished().await;

    assert_eq!(party.digits(), vec!['1', '2']);
    assert!(tools.calls.lock().unwrap().is_empty());
    assert_eq!(report.ended_reason, AGENT_HANGUP);
}

#[tokio::test(start_paused = true)]
async fn every_other_tool_goes_through_the_broker() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;

    peer.function_call("c1", "memory_read", serde_json::json!({"path": "notes.md"}));
    peer.wait_for("conversation.item.create").await;

    let calls = call.tools.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "memory_read");

    call.party.hangup();
    call.finished().await;
}

#[tokio::test(start_paused = true)]
async fn a_number_that_is_not_in_service_ends_the_call() {
    let call = Call::outbound(brief(CallDirection::Outbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    call.party.answer();
    call.party.speak(Frame::silence(Codec::Pcmu));
    peer.wait_for("input_audio_buffer.append").await;

    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "machine-unavailable"}),
    );
    let report = call.finished().await;

    assert_eq!(
        report.classification,
        Some(Classification::MachineUnavailable)
    );
    assert_eq!(report.outcome, CallOutcome::Failed);
    assert!(!report.message_left);
}

#[tokio::test(start_paused = true)]
async fn a_voicemail_greeting_is_left_a_message_when_the_brief_says_so() {
    let mut brief = brief(CallDirection::Outbound, TrustTier::Trusted);
    brief.voicemail = VoicemailPolicy::LeaveMessage;
    let call = Call::outbound(brief).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    call.party.answer();
    call.party.speak(Frame::silence(Codec::Pcmu));
    peer.wait_for("input_audio_buffer.append").await;

    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "machine-vm"}),
    );
    tokio::task::yield_now().await;
    assert_eq!(peer.sent_of("session.update").len(), 2);

    call.party.hangup();
    let report = call.finished().await;
    assert_eq!(report.outcome, CallOutcome::Voicemail);
    assert!(report.message_left);
}

#[tokio::test(start_paused = true)]
async fn a_phone_tree_starts_ivr_mode_from_the_verdict() {
    let call = Call::outbound(brief(CallDirection::Outbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    call.party.answer();
    call.party.speak(Frame::silence(Codec::Pcmu));
    peer.wait_for("input_audio_buffer.append").await;

    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "machine-ivr"}),
    );
    tokio::task::yield_now().await;
    let messages = peer.sent_of("conversation.item.create");
    assert!(
        messages
            .iter()
            .any(|event| event["item"]["content"][0]["text"] == IVR_MODE_PROMPT),
        "IVR mode starts from the verdict"
    );

    call.party.hangup();
    call.finished().await;
}

#[tokio::test(start_paused = true)]
async fn the_duration_cap_warns_and_then_ends_the_call() {
    let mut brief = brief(CallDirection::Inbound, TrustTier::Trusted);
    brief.duration_cap = Duration::from_secs(60);
    let call = Call::inbound(brief).await;
    call.keep_the_media_alive();
    let peer = call.peer();
    peer.wait_for("session.update").await;

    tokio::time::sleep(Duration::from_secs(60) - WRAP_UP_LEAD + Duration::from_secs(1)).await;
    let messages = peer.sent_of("conversation.item.create");
    assert!(
        messages
            .iter()
            .any(|event| event["item"]["content"][0]["text"] == wrap_up_prompt(WRAP_UP_LEAD)),
        "the wrap-up goes in 30 s before the cap"
    );

    let report = call.finished().await;
    assert_eq!(report.ended_reason, DURATION_CAP);
    assert!(report.duration >= Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn a_verdict_before_any_audio_is_refused() {
    let call = Call::outbound(brief(CallDirection::Outbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    call.party.answer();
    tokio::task::yield_now().await;

    // Nothing has been heard: the verdict is a guess, and it is refused.
    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "human"}),
    );
    let output = peer.wait_for("conversation.item.create").await;
    assert_eq!(output["item"]["type"], "function_call_output");
    let text = output["item"]["output"].as_str().unwrap_or_default();
    assert!(text.contains("nothing"), "{text}");
    assert_eq!(
        peer.sent_of("session.update").len(),
        1,
        "no swap on a guess"
    );
    assert!(
        peer.sent_of("response.create").is_empty(),
        "no answer is forced while there is nothing to hear"
    );

    // Audio arrives, and the same verdict is taken.
    call.party.speak(Frame::silence(Codec::Pcmu));
    peer.wait_for("input_audio_buffer.append").await;
    peer.function_call(
        "c2",
        REPORT_ANSWER,
        serde_json::json!({"category": "human"}),
    );
    eventually("the verdict swaps the session", || {
        peer.sent_of("session.update").len() == 2
    })
    .await;

    call.party.hangup();
    let report = call.finished().await;
    assert_eq!(report.classification, Some(Classification::Human));
}

#[tokio::test(start_paused = true)]
async fn the_cap_counts_from_the_answer_and_not_from_the_ring() {
    let mut brief = brief(CallDirection::Outbound, TrustTier::Trusted);
    brief.duration_cap = Duration::from_secs(60);
    let call = Call::outbound(brief).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    call.party.ring();

    // A long ring eats nothing of the cap.
    tokio::time::sleep(Duration::from_secs(50)).await;
    assert!(
        system_messages(&peer).is_empty(),
        "no wrap-up while it rings"
    );

    call.party.answer();
    call.keep_the_media_alive();
    peer.wait_for("input_audio_buffer.append").await;
    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "human"}),
    );
    eventually("the verdict swaps the session", || {
        peer.sent_of("session.update").len() == 2
    })
    .await;

    tokio::time::sleep(Duration::from_secs(29)).await;
    assert!(
        system_messages(&peer).is_empty(),
        "no wrap-up before 30 s from the answer"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(system_messages(&peer), vec![wrap_up_prompt(WRAP_UP_LEAD)]);

    let report = call.finished().await;
    assert_eq!(report.ended_reason, DURATION_CAP);
    assert!(
        report.duration >= Duration::from_secs(110),
        "{:?}",
        report.duration
    );
}

#[tokio::test(start_paused = true)]
async fn a_short_cap_warns_at_half_of_it() {
    let mut brief = brief(CallDirection::Inbound, TrustTier::Trusted);
    brief.duration_cap = Duration::from_secs(20);
    let call = Call::inbound(brief).await;
    call.keep_the_media_alive();
    let peer = call.peer();
    peer.wait_for("session.update").await;

    tokio::time::sleep(Duration::from_secs(9)).await;
    assert!(system_messages(&peer).is_empty(), "no wrap-up at the start");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let lead = Duration::from_secs(10);
    assert_eq!(system_messages(&peer), vec![wrap_up_prompt(lead)]);
    assert!(wrap_up_prompt(lead).contains("10 seconds"));
    assert_eq!(
        peer.sent_of("response.create").len(),
        1,
        "the wrap-up asks for an answer"
    );

    let report = call.finished().await;
    assert_eq!(report.ended_reason, DURATION_CAP);
}

#[tokio::test(start_paused = true)]
async fn the_wrap_up_waits_for_the_verdict() {
    let mut brief = brief(CallDirection::Outbound, TrustTier::Trusted);
    brief.duration_cap = Duration::from_secs(30);
    let call = Call::outbound(brief).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    call.party.answer();
    call.keep_the_media_alive();
    peer.wait_for("input_audio_buffer.append").await;

    // The wrap-up point passes while the call is still classified.
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert!(
        system_messages(&peer).is_empty(),
        "no wrap-up in the classify phase"
    );
    assert!(peer.sent_of("response.create").is_empty());

    peer.function_call(
        "c1",
        REPORT_ANSWER,
        serde_json::json!({"category": "human"}),
    );
    eventually("the wrap-up follows the verdict", || {
        !system_messages(&peer).is_empty()
    })
    .await;
    assert_eq!(
        peer.sent_of("session.update").len(),
        2,
        "the swap comes first"
    );
    assert_eq!(
        system_messages(&peer),
        vec![wrap_up_prompt(Duration::from_secs(15))]
    );

    let report = call.finished().await;
    assert_eq!(report.ended_reason, DURATION_CAP);
}

#[tokio::test(start_paused = true)]
async fn ten_seconds_with_no_inbound_rtp_ends_the_call() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    call.peer().wait_for("session.update").await;

    let report = call.finished().await;
    assert_eq!(report.ended_reason, "media_timeout");
    assert_eq!(report.outcome, CallOutcome::Answered);
}

#[tokio::test(start_paused = true)]
async fn a_dropped_model_socket_reconnects_with_the_call_so_far() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let first = call.peer();
    first.wait_for("session.update").await;
    first.caller_transcript("my name is Sam");
    tokio::task::yield_now().await;
    first.drop_socket();

    // The second session opens with the same instructions and is seeded
    // with the call so far.
    tokio::task::yield_now().await;
    let second = call.sessions.peers()[1].clone();
    second.wait_for("session.update").await;
    let seed = second.wait_for("conversation.item.create").await;
    assert!(
        seed["item"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("caller: my name is Sam")
    );

    call.party.hangup();
    let report = call.finished().await;
    assert_eq!(report.ended_reason, "remote_hangup");
}

/// A reconnected session reads the call so far as instructions. A
/// keypad press is in them, and its digit is not (ADR-0021).
#[tokio::test(start_paused = true)]
async fn a_reconnected_session_reads_no_keypad_digit() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let first = call.peer();
    first.wait_for("session.update").await;
    let bus = Arc::clone(&call.bus);
    call.party.press('7');
    eventually("the press reaches the transcript", || {
        bus.transcript_texts().len() == 2
    })
    .await;
    first.drop_socket();

    let sessions = Arc::clone(&call.sessions);
    eventually("the session reconnects", || sessions.peers().len() == 2).await;
    let second = sessions.peers()[1].clone();
    second.wait_for("conversation.item.create").await;
    let seeded = system_messages(&second);
    for text in &seeded {
        assert!(holds_no_digit(text), "the new session read {text}");
    }
    assert!(
        seeded
            .iter()
            .any(|text| text.contains("daemon: [the caller used the keypad]")),
        "{seeded:?}"
    );

    call.party.hangup();
    call.finished().await;
}

#[tokio::test(start_paused = true)]
async fn a_second_dropped_socket_ends_the_call_as_model_unavailable() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    call.peer().wait_for("session.update").await;
    call.peer().drop_socket();
    tokio::task::yield_now().await;

    let second = call.sessions.peers()[1].clone();
    second.wait_for("session.update").await;
    second.drop_socket();

    let report = call.finished().await;
    assert_eq!(report.ended_reason, MODEL_UNAVAILABLE);
}

#[tokio::test(start_paused = true)]
async fn a_stopping_daemon_ends_the_call_with_its_reason() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    call.peer().wait_for("session.update").await;

    call.cancel.cancel();
    let report = call.finished().await;
    assert_eq!(report.ended_reason, DAEMON_RESTART);
}

#[tokio::test(start_paused = true)]
async fn a_call_nobody_answers_reports_no_answer() {
    let call = Call::outbound(brief(CallDirection::Outbound, TrustTier::Trusted)).await;
    call.peer().wait_for("session.update").await;
    call.party.ring();
    call.party
        .refuse(pagis_telephony::leg::EndedReason::NoAnswer);

    let report = call.finished().await;
    assert_eq!(report.outcome, CallOutcome::NoAnswer);
    assert_eq!(report.ended_reason, "no_answer");
    assert!(report.ringing_at.is_some());
    assert!(report.answered_at.is_none());
}

/// A transport that says it cannot send digits: `send_digits` is then
/// absent, not present and failing (ADR-0005).
#[tokio::test(start_paused = true)]
async fn a_carrier_without_dtmf_binds_no_send_digits() {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    transport.set_capabilities(TransportCapabilities {
        send_dtmf: false,
        answering_machine_detection: false,
        wideband_audio: false,
        public_ingress_required: false,
    });
    let sessions = Arc::new(FakeModelSessions::new());
    let call = placed(brief(CallDirection::Inbound, TrustTier::Trusted));
    let session = CallSession::open(CallSessionDeps {
        workspace_id: call.workspace_id.clone(),
        brief: call.brief.clone(),
        gate: None,
        run_id: call.run_id.clone(),
        capabilities: transport.capabilities(),
        sessions: Arc::clone(&sessions) as _,
        tools: Arc::new(RecordingTools::default()),
        cancel: CancellationToken::new(),
    })
    .await
    .expect("the model session opens");
    drop(session);

    let update = sessions.peer().wait_for("session.update").await;
    assert_eq!(bound_tools(&update), vec![HANG_UP, "memory_read"]);
}

#[tokio::test(start_paused = true)]
async fn a_call_whose_model_session_does_not_open_is_never_placed() {
    let sessions = Arc::new(FakeModelSessions::new());
    sessions.fail_next(1);
    let call = placed(brief(CallDirection::Outbound, TrustTier::Trusted));

    let opened = CallSession::open(CallSessionDeps {
        workspace_id: call.workspace_id.clone(),
        brief: call.brief.clone(),
        gate: None,
        run_id: call.run_id.clone(),
        capabilities: TransportCapabilities {
            send_dtmf: true,
            answering_machine_detection: false,
            wideband_audio: false,
            public_ingress_required: false,
        },
        sessions: Arc::clone(&sessions) as _,
        tools: Arc::new(RecordingTools::default()),
        cancel: CancellationToken::new(),
    })
    .await
    .err();

    assert!(
        opened
            .expect("no session, no call")
            .0
            .contains("unavailable"),
        "the reason reaches the Agent"
    );
}

#[tokio::test(start_paused = true)]
async fn the_report_carries_the_transcript_the_run_reads() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    peer.agent_transcript("hello");
    call.party.press('5');
    tokio::task::yield_now().await;
    call.party.hangup();

    let report: CallReport = call.finished().await;
    let rendered = render_transcript(&report.transcript);
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines[0], "daemon: [tier: trusted]");
    assert!(lines.contains(&"agent: hello"));
    // The Run reads that the caller used the keypad, and never the
    // digit (ADR-0021).
    assert!(lines.contains(&"daemon: [the caller used the keypad]"));
    assert!(holds_no_digit(&rendered), "{rendered}");
    // Every line carries the time it was said.
    assert!(report.transcript.iter().all(|line| line.at > 0));
    assert_eq!(report.transcript[0].speaker, Speaker::Daemon);
}

/// The live `call` block in the Thread reads the same lines the settled
/// record keeps, so each line goes on the bus as it is said.
#[tokio::test(start_paused = true)]
async fn each_transcript_line_reaches_the_bus_as_it_is_said() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    peer.caller_transcript("is that Robin?");
    peer.agent_transcript("it is.");
    tokio::task::yield_now().await;
    call.party.hangup();

    let bus = Arc::clone(&call.bus);
    let report = call.finished().await;
    let said: Vec<(String, String)> = bus
        .payloads_of("call.transcript")
        .into_iter()
        .map(|payload| {
            (
                payload["speaker"].as_str().unwrap_or_default().to_string(),
                payload["text"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(said.contains(&("caller".to_string(), "is that Robin?".to_string())));
    assert!(said.contains(&("agent".to_string(), "it is.".to_string())));
    assert_eq!(said.len(), report.transcript.len());
}

/// What a response cost goes to the router's metering, as the
/// `turn.completed` every Run reports its usage with.
#[tokio::test(start_paused = true)]
async fn a_completed_response_meters_what_it_cost() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;
    peer.response_done(120, 45);
    peer.response_done(200, 60);
    tokio::task::yield_now().await;
    call.party.hangup();

    let bus = Arc::clone(&call.bus);
    call.finished().await;
    let metered = bus.payloads_of("turn.completed");
    assert_eq!(metered.len(), 2);
    assert_eq!(metered[0]["turn"], 1);
    assert_eq!(metered[0]["input_tokens"], 120);
    assert_eq!(metered[0]["output_tokens"], 45);
    assert_eq!(metered[1]["turn"], 2);
    assert_eq!(metered[1]["input_tokens"], 200);
}

/// The one code the gate of this test accepts.
const CODE: &str = "246813";

/// Every Workspace's Keypad Code is [`CODE`].
struct TestCode;

#[async_trait]
impl CodeCheck for TestCode {
    async fn is_set(&self, _workspace_id: &WorkspaceId) -> bool {
        true
    }

    async fn verify(&self, _workspace_id: &WorkspaceId, digits: &str) -> bool {
        digits == CODE
    }
}

/// The gate of an inbound call from a Trusted-listed number. Every
/// Workspace's code is [`CODE`], and no Workspace has a failed attempt.
/// The keypad events of the gate go to a bus of its own.
fn trusted_gate() -> Arc<TierGate> {
    let call = placed(brief(CallDirection::Inbound, TrustTier::Unknown));
    Arc::new(TierGate::inbound(
        TrustTier::Trusted,
        call.workspace_id.clone(),
        Keypad {
            code: Arc::new(TestCode),
            failures: Arc::new(FakeKeypadFailures::default()),
            clock: Arc::new(SystemClock),
        },
        CallLog::new(Arc::new(RecordingBus::default()), &call),
    ))
}

/// A digit that arrives after the session started reaches the gate
/// (ADR-0021): the tier rises with one `session.update`, and the
/// audit log keeps `call.tier_changed`. The digits of the code stay out
/// of the transcript and out of the bus, and one line says that the
/// caller used the keypad.
#[tokio::test(start_paused = true)]
async fn a_late_code_raises_the_tier_of_a_live_call() {
    let gate = trusted_gate();
    let call = Call::inbound_gated(
        brief(CallDirection::Inbound, TrustTier::Unknown),
        Arc::clone(&gate),
    )
    .await;
    let peer = call.peer();
    let first = peer.wait_for("session.update").await;
    assert_eq!(bound_tools(&first), vec![HANG_UP, SEND_DIGITS]);

    // The caller types the Keypad Code during the call.
    for digit in CODE.chars().chain(std::iter::once('#')) {
        call.party.press(digit);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(gate.tier(), TrustTier::Trusted);
    let updates = peer.sent_of("session.update");
    assert_eq!(updates.len(), 2);
    assert_eq!(
        bound_tools(&updates[1]),
        vec![HANG_UP, SEND_DIGITS, "memory_read"]
    );

    let bus = Arc::clone(&call.bus);
    call.party.hangup();
    let report = call.finished().await;
    let rendered = render_transcript(&report.transcript);
    assert!(rendered.contains("[tier: trusted]"));
    assert!(bus.types().contains(&"call.tier_changed".to_string()));

    assert!(holds_no_digit(&rendered), "the report kept {rendered}");
    for text in bus.transcript_texts() {
        assert!(holds_no_digit(&text), "the bus carried {text}");
    }
    let keypad_lines = report
        .transcript
        .iter()
        .filter(|line| {
            line.speaker == Speaker::Daemon && line.text == "[the caller used the keypad]"
        })
        .count();
    assert_eq!(
        keypad_lines, 1,
        "one run of presses is one line: {rendered}"
    );
}

/// A run of presses is one keypad line, and any other line ends the
/// run. A line for each press would show the length of the code
/// (ADR-0021).
#[tokio::test(start_paused = true)]
async fn another_line_ends_a_run_of_presses() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;

    for digit in "123".chars() {
        call.party.press(digit);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    peer.caller_transcript("did that work?");
    tokio::time::sleep(Duration::from_millis(200)).await;
    for digit in "45".chars() {
        call.party.press(digit);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    call.party.hangup();

    let report = call.finished().await;
    let rendered = render_transcript(&report.transcript);
    assert_eq!(
        rendered.lines().collect::<Vec<_>>(),
        vec![
            "daemon: [tier: trusted]",
            "daemon: [the caller used the keypad]",
            "caller: did that work?",
            "daemon: [the caller used the keypad]",
        ]
    );
}

/// The challenge reads the code before the session exists (ADR-0021),
/// so the session never sees a digit of it and no transcript line
/// holds one.
#[tokio::test(start_paused = true)]
async fn the_digits_of_the_challenge_reach_no_transcript_line() {
    let gate = trusted_gate();
    let call = Call::challenged(
        brief(CallDirection::Inbound, TrustTier::Unknown),
        Arc::clone(&gate),
        &format!("{CODE}#"),
    )
    .await;
    assert_eq!(
        gate.tier(),
        TrustTier::Trusted,
        "the challenge took the code"
    );
    let peer = call.peer();
    let first = peer.wait_for("session.update").await;
    assert_eq!(
        bound_tools(&first),
        vec![HANG_UP, SEND_DIGITS, "memory_read"]
    );

    let bus = Arc::clone(&call.bus);
    call.party.hangup();
    let report = call.finished().await;
    let rendered = render_transcript(&report.transcript);
    assert!(
        rendered.starts_with("daemon: [tier: trusted]"),
        "{rendered}"
    );
    assert!(holds_no_digit(&rendered), "the report kept {rendered}");
    for text in bus.transcript_texts() {
        assert!(holds_no_digit(&text), "the bus carried {text}");
    }
}

/// The endpoint answers an inbound call before the bridge runs the
/// session. A leg that dies in that gap publishes its end before the
/// session subscribes, and the session must still settle the call.
#[tokio::test]
async fn a_hub_that_ended_before_the_session_ran_settles_the_call() {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let (answered, mut incoming) = tokio::sync::mpsc::channel(1);
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
        Arc::new(FakeNumberDirectory::with(vec![FakeNumberDirectory::held(
            OWN,
        )])),
        answered,
    );
    let mut watch = task.watch();
    watch
        .wait_for(|state| *state == RegistrationState::Registered)
        .await
        .expect("the line registers");
    let party = transport.ring(OWN, REMOTE);
    let hub = incoming
        .recv()
        .await
        .expect("the task answers the call")
        .hub;
    party.hangup();
    eventually("the hub ends", || hub.is_ended()).await;

    let sessions = Arc::new(FakeModelSessions::new());
    let bus = Arc::new(RecordingBus::default());
    let call = placed(brief(CallDirection::Inbound, TrustTier::Trusted));
    let log = CallLog::new(Arc::clone(&bus) as _, &call);
    let session = CallSession::open(CallSessionDeps {
        workspace_id: call.workspace_id.clone(),
        brief: call.brief.clone(),
        gate: None,
        run_id: call.run_id.clone(),
        capabilities: transport.capabilities(),
        sessions: Arc::clone(&sessions) as _,
        tools: Arc::new(RecordingTools::default()) as _,
        cancel: CancellationToken::new(),
    })
    .await
    .expect("the model session opens");

    let report = tokio::time::timeout(Duration::from_secs(5), session.run(hub, &log))
        .await
        .expect("the session settles the call at once");
    assert_eq!(report.ended_reason, "remote_hangup");
}

/// The model says goodbye and calls hang_up in one response. The
/// goodbye is still on the 20 ms pacer when the tool arrives, so the
/// hang-up waits until the Remote Party heard it.
#[tokio::test(start_paused = true)]
async fn the_agent_hang_up_waits_for_its_last_words_to_be_heard() {
    let call = Call::inbound(brief(CallDirection::Inbound, TrustTier::Trusted)).await;
    let peer = call.peer();
    peer.wait_for("session.update").await;

    peer.speak("item-1", &vec![0x55; FRAME_BYTES * 50]);
    peer.function_call("c1", HANG_UP, serde_json::json!({"reason": "done"}));
    let party = Arc::clone(&call.party);
    let report = call.finished().await;

    assert_eq!(report.ended_reason, AGENT_HANGUP);
    let goodbye = party
        .heard()
        .iter()
        .filter(|packet| packet.payload.iter().all(|byte| *byte == 0x55))
        .count();
    assert_eq!(goodbye, 50, "the caller heard every frame of the goodbye");
}
