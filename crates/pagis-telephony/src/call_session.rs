//! The realtime bridge (ADR-0020): the task that pumps one call.
//!
//! One [`CallSession`] joins a [`MediaHub`] to a [`ModelSession`] and
//! stays on the call until it ends. G.711 crosses unchanged in both
//! directions for the OpenAI adapters, so nothing resamples. A provider
//! can report a speech-start event. When it does, the pending downlink is
//! dropped and the model is told how much of its speech the Remote
//! Party heard.
//!
//! The session opens before the call is dialed, with the classify
//! instructions and one tool. Audio starts only when the carrier
//! reports the call answered, because ringback makes the verdict wrong.
//! On the verdict, the model-session adapter opens or updates the
//! conversation model. An inbound call skips the phase.
//!
//! Every call ends with a reason. Four of them are the bridge's own:
//! `model_unavailable` after one failed reconnect, `duration_cap`,
//! `daemon_restart`, and the hub's `media_timeout`.

use std::sync::Arc;
use std::time::Duration;

use pagis_broker::{ToolCall, ToolDef};
use pagis_core::WorkspaceId;
use pagis_core::{
    CallOutcome, Classification, RunId, Speaker, TranscriptLine, TrustTier, UnixMillis, now_ms,
    render_transcript,
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::audio::{Codec, Frame};
use crate::bridge::{BridgeError, CallLog, CallReport};
use crate::brief::{
    CallBrief, REPORT_ANSWER, VoicemailPolicy, report_answer_tool, wrap_up_lead, wrap_up_prompt,
};
use crate::call_tools::CallTools;
use crate::hub::{HubEvent, MediaHub};
use crate::leg::EndedReason;
use crate::model::{ClientCommand, ModelSession, ModelSessions, ServerEvent, SessionConfig};
use crate::session::{HANG_UP, SEND_DIGITS, session_tools};
use crate::transport::TransportCapabilities;

/// The model socket went away and did not come back after one
/// reconnect.
pub const MODEL_UNAVAILABLE: &str = "model_unavailable";
/// The call reached the workspace duration cap.
pub const DURATION_CAP: &str = "duration_cap";
/// The daemon stopped while the call ran.
pub const DAEMON_RESTART: &str = "daemon_restart";
/// The model asked to end the call.
pub const AGENT_HANGUP: &str = "agent_hangup";
/// The model's speech goes to the Remote Party in this codec.
const MODEL_CODEC: Codec = Codec::Pcmu;
/// The transcript line for one run of keypad presses by the Remote
/// Party. It has no digit and no count: the digits show the Keypad
/// Code, and the count shows its length (ADR-0021).
const KEYPAD_USED: &str = "[the caller used the keypad]";

/// Everything one call session reaches.
pub struct CallSessionDeps {
    /// The Workspace the call belongs to. Every store read of the
    /// session names it.
    pub workspace_id: WorkspaceId,
    pub brief: CallBrief,
    /// The tier gate of this call (ADR-0021). A digit that
    /// arrives during the call reaches it, and nothing else raises the
    /// tier. An outbound call has none.
    pub gate: Option<Arc<crate::tiers::TierGate>>,
    pub run_id: RunId,
    /// What the carrier can do. It decides whether `send_digits` is
    /// bound at all (ADR-0005).
    pub capabilities: TransportCapabilities,
    pub sessions: Arc<dyn ModelSessions>,
    pub tools: Arc<dyn CallTools>,
    /// Cancelled when the daemon stops.
    pub cancel: CancellationToken,
}

/// What a call session is told from outside while it runs.
enum Control {
    /// The keypad code arrived and the tier rises (ADR-0021).
    SetTier(TrustTier),
}

/// A handle on a running call. The tier rises in place through it, and
/// the provider adapter binds the new tier's tools.
#[derive(Clone)]
pub struct CallHandle {
    control: mpsc::Sender<Control>,
}

impl CallHandle {
    /// Bind the tools of a new tier, without a new session. A tier
    /// never rises inside the model loop (ADR-0021), so the caller is
    /// the daemon and never a tool.
    pub async fn set_tier(&self, tier: TrustTier) {
        let _ = self.control.send(Control::SetTier(tier)).await;
    }
}

/// One call, from the open socket to the settled report.
pub struct CallSession {
    deps: CallSessionDeps,
    model: Box<dyn ModelSession>,
    control: mpsc::Receiver<Control>,
    handle: CallHandle,
    /// The phase the instructions are in. An inbound call starts in the
    /// conversation, because it skips the classify phase.
    classifying: bool,
}

impl CallSession {
    /// Open the model session. An outbound call does this before it
    /// dials: if the session does not open, no call is placed.
    pub async fn open(deps: CallSessionDeps) -> Result<Self, BridgeError> {
        let mut model = deps
            .sessions
            .open(&deps.workspace_id)
            .await
            .map_err(|error| BridgeError(error.to_string()))?;
        let classifying = deps.brief.direction == pagis_core::CallDirection::Outbound;
        let update = if classifying {
            SessionConfig::new(
                deps.brief.classify_instructions(),
                vec![report_answer_tool()],
                deps.brief.voice.clone(),
            )
        } else {
            SessionConfig::new(
                deps.brief.conversation_instructions(),
                session_tools(&deps.brief, deps.capabilities, None),
                deps.brief.voice.clone(),
            )
        };
        model
            .send(ClientCommand::Configure(update))
            .await
            .map_err(|error| BridgeError(error.to_string()))?;
        let (control, control_rx) = mpsc::channel(4);
        Ok(Self {
            deps,
            model,
            control: control_rx,
            handle: CallHandle { control },
            classifying,
        })
    }

    /// The handle the daemon raises the tier through.
    pub fn handle(&self) -> CallHandle {
        self.handle.clone()
    }

    /// Pump the call until it ends, and report how it went.
    pub async fn run(self, hub: Arc<MediaHub>, log: &CallLog) -> CallReport {
        Pump::new(self, hub, log).run().await
    }
}

/// One frame on its way to the Remote Party, or the order to drop what
/// waits. The queue is drained by its own task, so the pump never waits
/// on the 20 ms pacer and reads a barge-in the moment it arrives.
enum Downlink {
    Frame(Frame),
    Clear,
    /// Answer once every frame queued before it went to the hub.
    Drained(tokio::sync::oneshot::Sender<()>),
}

/// How long a hang-up waits for the agent's last words to go out.
const LAST_WORDS_CAP: Duration = Duration::from_secs(15);
/// What the hub still holds once the queue is drained: its pacer depth.
const PACER_DEPTH: Duration = Duration::from_millis(80);

struct Pump<'a> {
    deps: CallSessionDeps,
    /// The audit trail. Each transcript line and each response's usage
    /// goes on it while the call runs.
    log: &'a CallLog,
    model: Box<dyn ModelSession>,
    control: mpsc::Receiver<Control>,
    hub: Arc<MediaHub>,
    downlink: mpsc::UnboundedSender<Downlink>,
    writer: tokio::task::JoinHandle<()>,
    classifying: bool,
    /// The model may hear the Remote Party from here on.
    answered: bool,
    /// When the call was answered on this clock. The cap and the
    /// wrap-up count from here, so a long ring eats none of the cap.
    answered_since: Option<Instant>,
    /// At least one frame of the Remote Party reached the model. A
    /// verdict before that is a guess, and it is refused.
    heard_audio: bool,
    /// The wrap-up point passed in the classify phase, so the prompt
    /// goes in with the verdict and not before.
    wrap_up_pending: bool,
    transcript: Vec<TranscriptLine>,
    /// How many responses the realtime session has completed. It
    /// numbers the metering events.
    turns: u32,
    classification: Option<Classification>,
    message_left: bool,
    ringing_at: Option<UnixMillis>,
    answered_at: Option<UnixMillis>,
    /// Why the bridge ended the call, when it did. It wins over the
    /// hub's own reason.
    ended_reason: Option<String>,
    /// The item the model is speaking now, and how much of the downlink
    /// had gone out when it started.
    speaking: Option<(String, u64)>,
    /// One reconnect is allowed per call.
    reconnected: bool,
}

impl<'a> Pump<'a> {
    fn new(session: CallSession, hub: Arc<MediaHub>, log: &'a CallLog) -> Self {
        let (downlink, queue) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write_downlink(Arc::clone(&hub), queue));
        let inbound = session.deps.brief.direction == pagis_core::CallDirection::Inbound;
        Self {
            deps: session.deps,
            log,
            model: session.model,
            control: session.control,
            hub,
            downlink,
            writer,
            classifying: session.classifying,
            // The endpoint task answered the inbound call before it
            // handed the hub over, so its audio starts at once.
            answered: inbound,
            answered_since: inbound.then(Instant::now),
            heard_audio: false,
            wrap_up_pending: false,
            transcript: Vec::new(),
            turns: 0,
            classification: None,
            message_left: false,
            ringing_at: None,
            answered_at: inbound.then(now_ms),
            ended_reason: None,
            speaking: None,
            reconnected: false,
        }
    }

    async fn run(mut self) -> CallReport {
        self.note(
            Speaker::Daemon,
            format!("[tier: {}]", self.deps.brief.tier.as_str()),
        )
        .await;
        let started = Instant::now();
        let cap = self.deps.brief.duration_cap;
        let lead = wrap_up_lead(cap);
        let mut wrapped_up = false;
        let mut capped = false;
        let mut events = self.hub.subscribe();
        // The hub may have ended before this subscription existed: an
        // inbound leg is answered before the session runs, and its end
        // is published once. The hub's state says so.
        let already_ended = self.hub.ended();
        let ended = loop {
            if let Some(reason) = already_ended {
                break reason;
            }
            let wrap_up_at = self.answered_since.map(|answered| answered + cap - lead);
            let cap_at = self.answered_since.map(|answered| answered + cap);
            tokio::select! {
                biased;
                () = self.deps.cancel.cancelled(), if self.ended_reason.is_none() => {
                    self.end(DAEMON_RESTART).await;
                }
                event = events.recv() => match event {
                    Ok(event) => {
                        if let Some(reason) = self.on_hub(event).await {
                            break reason;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break EndedReason::TransportLost;
                    }
                },
                incoming = self.model.next() => match incoming {
                    Some(Ok(event)) => self.on_model(event).await,
                    Some(Err(error)) => {
                        tracing::warn!(%error, "the model socket reported an error");
                    }
                    None => self.reconnect().await,
                },
                Some(control) = self.control.recv() => match control {
                    Control::SetTier(tier) => self.set_tier(tier).await,
                },
                () = sleep_until_or_never(wrap_up_at), if !wrapped_up => {
                    wrapped_up = true;
                    self.wrap_up(lead).await;
                }
                () = sleep_until_or_never(cap_at), if !capped => {
                    capped = true;
                    self.end(DURATION_CAP).await;
                }
            }
        };
        self.finish(ended, started.elapsed()).await
    }

    /// What arrived from the Remote Party. `Some` ends the call.
    async fn on_hub(&mut self, event: HubEvent) -> Option<EndedReason> {
        match event {
            HubEvent::Uplink(frame) => {
                if self.answered {
                    self.heard_audio = true;
                    let frame = frame.into_codec(MODEL_CODEC);
                    self.send(ClientCommand::AppendAudio(frame.payload().clone()))
                        .await;
                }
                None
            }
            HubEvent::Ringing => {
                self.ringing_at = Some(now_ms());
                None
            }
            HubEvent::Answered => {
                self.answered = true;
                self.answered_since = Some(Instant::now());
                self.answered_at = Some(now_ms());
                if let Err(error) = self.log.answered().await {
                    tracing::error!(%error, "the answered event did not reach the audit log");
                }
                None
            }
            // The keypad of the Remote Party is the daemon's (ADR-0021)
            // and never the model's: the tones stay out of the uplink.
            // The digit goes only to the tier gate. It stays out of the
            // transcript, and so out of every record made from it: the
            // audit log, the Call record, the transcript Artifact and
            // what both models read. The transcript gets one line for
            // each run of presses.
            HubEvent::Dtmf(digit) => {
                if !self.in_keypad_run() {
                    self.note(Speaker::Daemon, KEYPAD_USED).await;
                }
                if let Some(gate) = self.deps.gate.clone()
                    && let Some(change) = gate.late_digit(digit).await
                {
                    self.set_tier(change.to).await;
                    if let Err(error) = self.log.tier_changed(change).await {
                        tracing::error!(%error, "the tier change did not reach the audit log");
                    }
                }
                None
            }
            HubEvent::Ended(reason) => Some(reason),
        }
    }

    async fn on_model(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::SpeechStarted => self.barge_in().await,
            ServerEvent::AudioDelta { item_id, audio } => {
                if let Some(item_id) = item_id
                    && self.speaking.as_ref().is_none_or(|(id, _)| id != &item_id)
                {
                    self.speaking = Some((item_id, self.hub.downlink_packets_sent()));
                }
                for chunk in audio.chunks(crate::audio::FRAME_BYTES) {
                    let _ = self
                        .downlink
                        .send(Downlink::Frame(Frame::new(MODEL_CODEC, chunk.to_vec())));
                }
            }
            ServerEvent::AgentTranscript(text) => self.note(Speaker::Agent, text).await,
            ServerEvent::CallerTranscript(text) => self.note(Speaker::Caller, text).await,
            ServerEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => self.on_tool(&call_id, &name, &arguments).await,
            ServerEvent::Usage {
                input_tokens,
                output_tokens,
            } => self.meter(input_tokens, output_tokens).await,
            ServerEvent::Error(message) => {
                tracing::warn!(%message, "the model reported an error");
            }
            ServerEvent::Other => {}
        }
    }

    /// Barge-in. The pending downlink is dropped, and the model is told
    /// how much of the item it is speaking the Remote Party heard: the
    /// packets that went out since the item started, one per 20 ms.
    async fn barge_in(&mut self) {
        let _ = self.downlink.send(Downlink::Clear);
        let Some((item_id, started_at)) = self.speaking.take() else {
            return;
        };
        let played = self.hub.downlink_packets_sent().saturating_sub(started_at);
        self.send(ClientCommand::Truncate {
            item_id,
            packets_played: played,
        })
        .await;
    }

    /// One tool the model called. The session tools act on the call
    /// itself and need no approval; every other tool goes to the
    /// broker.
    async fn on_tool(&mut self, call_id: &str, name: &str, arguments: &str) {
        let mut respond = true;
        let output = match name {
            REPORT_ANSWER => {
                let (output, accepted) = self.on_verdict(arguments).await;
                // A refused verdict asks for no answer: the model has
                // nothing new to say until it hears something.
                respond = accepted;
                output
            }
            HANG_UP => {
                self.note(
                    Speaker::Daemon,
                    format!("[the agent ended the call: {arguments}]"),
                )
                .await;
                self.let_last_words_out().await;
                self.end(AGENT_HANGUP).await;
                "the call is ending".to_string()
            }
            SEND_DIGITS => self.send_digits(arguments).await,
            _ => {
                let result = self
                    .deps
                    .tools
                    .invoke(
                        &self.deps.workspace_id,
                        &self.deps.brief.agent_id,
                        &self.deps.run_id,
                        ToolCall::new(name, arguments),
                    )
                    .await;
                result.content
            }
        };
        self.send(ClientCommand::FunctionCallOutput {
            call_id: call_id.to_string(),
            output,
        })
        .await;
        if respond {
            self.send(ClientCommand::ContinueResponse).await;
        }
    }

    /// The classify verdict (ADR-0020). One `session.update` swaps the
    /// instructions and the tool set, and the verdict that ends the
    /// call ends it here. Returns the tool output and whether the
    /// verdict was taken.
    async fn on_verdict(&mut self, arguments: &str) -> (String, bool) {
        if !self.classifying {
            return ("the call is classified already".to_string(), true);
        }
        if !self.heard_audio {
            return (
                "nothing has been heard yet: wait until the far side is audible, then report"
                    .to_string(),
                false,
            );
        }
        let category = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("category")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .and_then(|category| category.parse::<Classification>().ok());
        let Some(classification) = category else {
            return ("that is not one of the five categories".to_string(), false);
        };
        self.classifying = false;
        self.classification = Some(classification);
        self.note(
            Speaker::Daemon,
            format!("[what answered: {}]", classification.as_str()),
        )
        .await;
        match classification {
            Classification::MachineUnavailable => {
                self.end(EndedReason::LocalHangup.as_str()).await;
                return (
                    "the number is not in service; the call ends".to_string(),
                    true,
                );
            }
            Classification::MachineVm if self.deps.brief.voicemail == VoicemailPolicy::HangUp => {
                self.end(EndedReason::LocalHangup.as_str()).await;
                return (
                    "a machine answered and no message is left; the call ends".to_string(),
                    true,
                );
            }
            _ => {}
        }
        self.swap_session().await;
        if self.wrap_up_pending {
            self.wrap_up_pending = false;
            self.send(ClientCommand::AddInstructions(wrap_up_prompt(
                wrap_up_lead(self.deps.brief.duration_cap),
            )))
            .await;
        }
        if classification == Classification::MachineIvr {
            self.send(ClientCommand::AddInstructions(
                self.deps.brief.ivr_mode_prompt.to_string(),
            ))
            .await;
        }
        if classification == Classification::MachineVm {
            self.message_left = true;
            self.send(ClientCommand::AddInstructions(
                "A voicemail greeting answered. Leave the message after the tone, then end the \
                 call."
                    .to_string(),
            ))
            .await;
        }
        ("start the conversation".to_string(), true)
    }

    async fn send_digits(&mut self, arguments: &str) -> String {
        let digits = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("digits")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if digits.is_empty() {
            return "no digits were given".to_string();
        }
        for digit in digits.chars() {
            self.hub.send_dtmf(digit).await;
        }
        self.note(Speaker::Daemon, format!("[the agent pressed {digits}]"))
            .await;
        format!("pressed {digits}")
    }

    /// The tier rose (ADR-0021): the provider adapter binds the
    /// new tier's tools, and the transcript records the change.
    async fn set_tier(&mut self, tier: TrustTier) {
        if tier == self.deps.brief.tier {
            return;
        }
        self.deps.brief.tier = tier;
        self.note(Speaker::Daemon, format!("[tier: {}]", tier.as_str()))
            .await;
        if !self.classifying {
            self.swap_session().await;
        }
    }

    /// Bind the conversational instructions and the tools of the tier
    /// the call stands at now.
    async fn swap_session(&mut self) {
        let update = SessionConfig::new(
            self.deps.brief.conversation_instructions(),
            session_tools(
                &self.deps.brief,
                self.deps.capabilities,
                Some(self.hub.dtmf()),
            ),
            self.deps.brief.voice.clone(),
        );
        self.send(ClientCommand::Configure(update)).await;
    }

    /// The wrap-up, `lead` before the cap. In the classify phase it
    /// waits for the verdict: a forced answer with nothing heard would
    /// be a guessed verdict and a speech into a line nobody is on.
    async fn wrap_up(&mut self, lead: Duration) {
        if self.classifying {
            self.wrap_up_pending = true;
            return;
        }
        self.send(ClientCommand::AddInstructions(wrap_up_prompt(lead)))
            .await;
        self.send(ClientCommand::ContinueResponse).await;
    }

    /// The model socket went away. One reconnect is allowed, and the
    /// new session is seeded with the transcript so far.
    async fn reconnect(&mut self) {
        if self.reconnected {
            self.end(MODEL_UNAVAILABLE).await;
            return;
        }
        self.reconnected = true;
        let Ok(model) = self.deps.sessions.open(&self.deps.workspace_id).await else {
            self.end(MODEL_UNAVAILABLE).await;
            return;
        };
        self.model = model;
        self.speaking = None;
        let _ = self.downlink.send(Downlink::Clear);
        let tools: Vec<ToolDef> = if self.classifying {
            vec![report_answer_tool()]
        } else {
            session_tools(
                &self.deps.brief,
                self.deps.capabilities,
                Some(self.hub.dtmf()),
            )
        };
        let instructions = if self.classifying {
            self.deps.brief.classify_instructions()
        } else {
            self.deps.brief.conversation_instructions()
        };
        self.send(ClientCommand::Configure(SessionConfig::new(
            instructions,
            tools,
            self.deps.brief.voice.clone(),
        )))
        .await;
        self.send(ClientCommand::AddInstructions(format!(
            "The connection dropped and the call goes on. This is the call so far:\n{}",
            render_transcript(&self.transcript)
        )))
        .await;
    }

    /// End the call from this side, with the bridge's own reason. The
    /// hub answers with its `Ended` event, which stops the pump.
    /// Wait for what the model said before it hung up: the goodbye is
    /// on the 20 ms pacer when the tool arrives. A Remote Party that
    /// hangs up first, or the cap, ends the wait.
    async fn let_last_words_out(&mut self) {
        let (drained, wait) = tokio::sync::oneshot::channel();
        if self.downlink.send(Downlink::Drained(drained)).is_err() {
            return;
        }
        let ended = self.hub.watch_ended();
        tokio::select! {
            _ = wait => tokio::time::sleep(PACER_DEPTH).await,
            () = hub_ended(ended) => {}
            () = tokio::time::sleep(LAST_WORDS_CAP) => {}
        }
    }

    async fn end(&mut self, reason: &str) {
        if self.ended_reason.is_some() {
            return;
        }
        self.ended_reason = Some(reason.to_string());
        self.hub.hangup().await;
    }

    async fn send(&mut self, command: ClientCommand) {
        if let Err(error) = self.model.send(command).await {
            tracing::warn!(%error, "the model socket did not take the event");
        }
    }

    /// Write one line of the conversation down. The line goes on the
    /// bus at once, because the live `call` block in the Thread reads
    /// the same lines the settled record keeps.
    async fn note(&mut self, speaker: Speaker, text: impl Into<String>) {
        let line = TranscriptLine::new(now_ms(), speaker, text);
        if let Err(error) = self.log.transcript(&line).await {
            tracing::error!(%error, "a transcript line did not reach the audit log");
        }
        self.transcript.push(line);
    }

    /// True when the last transcript line is the keypad line. A press
    /// then continues the run, and any other line ends it.
    fn in_keypad_run(&self) -> bool {
        self.transcript
            .last()
            .is_some_and(|line| line.speaker == Speaker::Daemon && line.text == KEYPAD_USED)
    }

    /// One response completed: its usage goes to the router's metering
    /// as the `turn.completed` of this Run.
    async fn meter(&mut self, input_tokens: u64, output_tokens: u64) {
        self.turns += 1;
        if let Err(error) = self
            .log
            .usage(self.turns, input_tokens, output_tokens)
            .await
        {
            tracing::error!(%error, "the call usage did not reach the audit log");
        }
    }

    async fn finish(self, ended: EndedReason, duration: Duration) -> CallReport {
        self.writer.abort();
        self.model.close().await;
        let ended_reason = self
            .ended_reason
            .clone()
            .unwrap_or_else(|| ended.as_str().to_string());
        CallReport {
            outcome: outcome_of(self.answered_at.is_some(), self.classification, ended),
            // The brief carries the tier the call ends at: the
            // challenge settles it before the session opens, and a
            // rise or a drop writes it here in place (ADR-0021).
            tier: self.deps.brief.tier,
            ended_reason,
            classification: self.classification,
            message_left: self.message_left,
            duration,
            ringing_at: self.ringing_at,
            answered_at: self.answered_at,
            transcript: self.transcript,
            recording_artifact_id: None,
        }
    }
}

/// Whether the call did what it was for (ADR-0020). A verdict that
/// settles the outcome wins; otherwise the answer decides, and a call
/// nobody answered reports why.
fn outcome_of(
    answered: bool,
    classification: Option<Classification>,
    ended: EndedReason,
) -> CallOutcome {
    match classification {
        Some(Classification::MachineVm) => return CallOutcome::Voicemail,
        Some(Classification::MachineUnavailable) => return CallOutcome::Failed,
        _ => {}
    }
    if answered {
        return CallOutcome::Answered;
    }
    match ended {
        EndedReason::Busy => CallOutcome::Busy,
        EndedReason::NoAnswer => CallOutcome::NoAnswer,
        _ => CallOutcome::Failed,
    }
}

/// The downlink writer. The hub holds 60 ms at most, so this task waits
/// on the pacer and the pump does not.
async fn write_downlink(hub: Arc<MediaHub>, mut queue: mpsc::UnboundedReceiver<Downlink>) {
    while let Some(command) = queue.recv().await {
        match command {
            Downlink::Frame(frame) => hub.send_downlink(frame).await,
            Downlink::Clear => {
                // What is still in this queue was never heard either.
                while let Ok(Downlink::Frame(_)) = queue.try_recv() {}
                hub.clear_downlink();
            }
            Downlink::Drained(answer) => {
                let _ = answer.send(());
            }
        }
    }
}

/// Resolve when the hub ends. The borrow never crosses an await, so
/// the future is `Send`.
async fn hub_ended(mut ended: tokio::sync::watch::Receiver<Option<EndedReason>>) {
    loop {
        if ended.borrow().is_some() {
            return;
        }
        if ended.changed().await.is_err() {
            return;
        }
    }
}

async fn sleep_until_or_never(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
