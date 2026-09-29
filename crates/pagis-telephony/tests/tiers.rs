//! The Trust Tier of one call (ADR-0021), on the fake carrier and
//! a paused clock: a scripted inbound call reaches Unknown with no
//! code, its listed tier with the code, and Unknown after three wrong
//! attempts. A number on no list is never challenged.
//!
//! The wrong codes of one Workspace add to one count across its Calls
//! and its lines. After six, keypad elevation is suspended for a delay
//! that doubles with each further failure. The delay reads a clock the
//! test moves by hand, so no test waits for it.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    AgentId, CallDirection, CallId, Event, EventBus, EventId, EventScope, EventStream,
    KeypadFailureStore, KeypadFailures, NewEvent, PhoneNumberId, RunId, StoreError, TrustEntry,
    TrustEntryId, TrustListStore, TrustSubject, TrustTier, WorkspaceId, now_ms,
};
use pagis_telephony::fake::{
    FakeCallTransport, FakeKeypadFailures, FakeNumberDirectory, RemoteParty, TokioClock,
};
use pagis_telephony::hub::MediaHub;
use pagis_telephony::keypad::{CHALLENGE_TIMEOUT, CodeCheck, MAX_ATTEMPTS};
use pagis_telephony::{
    CLASSIFY_PROMPT, CallBrief, CallLog, DEFAULT_DURATION_CAP, EMERGENCY_RULE, EndpointTask,
    IVR_MODE_PROMPT, IncomingHub, Keypad, LiveTiers, PlacedCall, RegistrationState, SipCredential,
    TierCause, TierGate, VoicemailPolicy, settle_inbound,
};
use serde_json::Value;
use tokio::sync::mpsc;

const NUMBER: &str = "+14155550123";
/// The Agent Phone Number of a second Agent of the same Workspace.
const OTHER_NUMBER: &str = "+14155550124";
const CALLER: &str = "+14155550100";
const CODE: &str = "246813";
/// A code no Workspace of these tests holds.
const WRONG: &str = "975319";
const MINUTE: i64 = 60 * 1_000;
/// Where the clock of each test starts.
const START: i64 = 1_790_305_200_000;

/// The Trust List as a test writes it.
#[derive(Default)]
struct Lists {
    rows: std::sync::Mutex<Vec<TrustEntry>>,
}

impl Lists {
    fn listing(workspace_id: &WorkspaceId, e164: &str, tier: TrustTier) -> Self {
        let lists = Self::default();
        lists.rows.lock().unwrap().push(TrustEntry {
            id: TrustEntryId::generate(),
            workspace_id: workspace_id.clone(),
            agent_id: None,
            subject: TrustSubject::Number,
            value: e164.to_string(),
            tier,
            label: "Home".to_string(),
            created_at: 1,
        });
        lists
    }
}

#[async_trait]
impl TrustListStore for Lists {
    async fn upsert(&self, row: &TrustEntry) -> Result<(), StoreError> {
        self.rows.lock().unwrap().push(row.clone());
        Ok(())
    }

    async fn list(&self, _workspace_id: &WorkspaceId) -> Result<Vec<TrustEntry>, StoreError> {
        Ok(self.rows.lock().unwrap().clone())
    }

    async fn delete(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &TrustEntryId,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn candidate(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        subject: TrustSubject,
        value: &str,
    ) -> Result<Option<TrustTier>, StoreError> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.subject == subject && row.value == value)
            .map(|row| row.tier)
            .max())
    }
}

/// The Keypad Code of one Workspace, and how many times it was asked.
/// Another Workspace holds no code here.
struct Code {
    workspace_id: WorkspaceId,
    digits: Option<String>,
    checks: AtomicUsize,
}

impl Code {
    fn set(digits: &str) -> Arc<Self> {
        Arc::new(Self {
            workspace_id: WorkspaceId::generate(),
            digits: Some(digits.to_string()),
            checks: AtomicUsize::new(0),
        })
    }

    fn none() -> Arc<Self> {
        Arc::new(Self {
            workspace_id: WorkspaceId::generate(),
            digits: None,
            checks: AtomicUsize::new(0),
        })
    }

    fn digits_of(&self, workspace_id: &WorkspaceId) -> Option<&str> {
        match workspace_id == &self.workspace_id {
            true => self.digits.as_deref(),
            false => None,
        }
    }

    fn checks(&self) -> usize {
        self.checks.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl CodeCheck for Code {
    async fn is_set(&self, workspace_id: &WorkspaceId) -> bool {
        self.digits_of(workspace_id).is_some()
    }

    async fn verify(&self, workspace_id: &WorkspaceId, digits: &str) -> bool {
        self.checks.fetch_add(1, Ordering::Relaxed);
        self.digits_of(workspace_id) == Some(digits)
    }
}

/// A clock the test moves by hand.
struct ManualClock {
    now: AtomicI64,
}

impl ManualClock {
    fn at(millis: i64) -> Arc<Self> {
        Arc::new(Self {
            now: AtomicI64::new(millis),
        })
    }

    fn set(&self, millis: i64) {
        self.now.store(millis, Ordering::SeqCst);
    }

    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }
}

impl pagis_core::Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now()
    }
}

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

/// The keypad of one Workspace: its code, its failed-attempt count,
/// the clock its delay reads, and the bus its Call events reach.
struct Guard {
    code: Arc<Code>,
    failures: Arc<FakeKeypadFailures>,
    clock: Arc<ManualClock>,
    bus: Arc<RecordingBus>,
}

impl Guard {
    fn new(code: Arc<Code>) -> Self {
        Self::sharing(
            code,
            Arc::new(FakeKeypadFailures::default()),
            ManualClock::at(START),
        )
    }

    /// A Workspace whose count sits in the same store as another's, on
    /// the same clock, as every Workspace of an installation does.
    fn sharing(
        code: Arc<Code>,
        failures: Arc<FakeKeypadFailures>,
        clock: Arc<ManualClock>,
    ) -> Self {
        Self {
            code,
            failures,
            clock,
            bus: Arc::new(RecordingBus::default()),
        }
    }

    fn workspace_id(&self) -> WorkspaceId {
        self.code.workspace_id.clone()
    }

    fn keypad(&self) -> Keypad {
        Keypad {
            code: Arc::clone(&self.code) as Arc<dyn CodeCheck>,
            failures: Arc::clone(&self.failures) as Arc<dyn KeypadFailureStore>,
            clock: Arc::clone(&self.clock) as Arc<dyn pagis_core::Clock>,
        }
    }

    /// The audit trail of one Call to this Workspace, on its bus.
    fn log(&self, own_e164: &str) -> CallLog {
        let call = PlacedCall {
            id: CallId::generate(),
            workspace_id: self.workspace_id(),
            run_id: RunId::generate(),
            brief: CallBrief {
                direction: CallDirection::Inbound,
                agent_id: AgentId::generate(),
                agent_name: "Robin".to_string(),
                voice: None,
                phone_number_id: PhoneNumberId::generate(),
                own_e164: own_e164.to_string(),
                remote_e164: CALLER.to_string(),
                tier: TrustTier::Unknown,
                purpose: "Take a message".to_string(),
                success_criteria: None,
                voicemail: VoicemailPolicy::HangUp,
                duration_cap: DEFAULT_DURATION_CAP,
                tools: Vec::new(),
                ivr_mode_prompt: IVR_MODE_PROMPT,
                classify_prompt: CLASSIFY_PROMPT,
                emergency_rule: EMERGENCY_RULE,
            },
            tools: Vec::new(),
        };
        CallLog::new(Arc::clone(&self.bus) as Arc<dyn EventBus>, &call)
    }

    /// A gate on a Call already answered, as the bridge holds it after
    /// the challenge, for the digits a caller types later.
    fn gate(&self, candidate: TrustTier) -> TierGate {
        TierGate::inbound(
            candidate,
            self.workspace_id(),
            self.keypad(),
            self.log(NUMBER),
        )
    }

    async fn count(&self) -> KeypadFailures {
        self.failures.get(&self.workspace_id()).await.unwrap()
    }

    /// Count wrong codes straight into the store, at the time on the
    /// clock.
    async fn fail(&self, failures: u32) {
        for _ in 0..failures {
            self.failures
                .record_failure(&self.workspace_id(), self.clock.now())
                .await
                .unwrap();
        }
    }
}

/// One answered inbound call on the fake carrier.
struct Answered {
    party: Arc<RemoteParty>,
    hub: Arc<MediaHub>,
    own_e164: &'static str,
    /// The task holds the line for the length of the test.
    _task: EndpointTask,
}

async fn answered_call() -> Answered {
    answered_call_on(NUMBER).await
}

/// A call to one Agent Phone Number, answered.
async fn answered_call_on(number: &'static str) -> Answered {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let (calls, mut incoming) = mpsc::channel::<IncomingHub>(4);
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
        Arc::new(FakeNumberDirectory::with(vec![FakeNumberDirectory::held(
            number,
        )])),
        calls,
    );
    task.watch()
        .wait_for(|state| *state == RegistrationState::Registered)
        .await
        .unwrap();
    let party = transport.ring(number, CALLER);
    let answered = tokio::time::timeout(Duration::from_secs(30), incoming.recv())
        .await
        .expect("the call was never answered")
        .expect("the endpoint answered");
    Answered {
        party,
        hub: answered.hub,
        own_e164: number,
        _task: task,
    }
}

/// The presses of one entry: the digits, then `#`.
fn entry(digits: &str) -> Vec<char> {
    digits.chars().chain(['#']).collect()
}

/// The presses of `count` wrong entries.
fn wrong_entries(count: usize) -> Vec<char> {
    (0..count).flat_map(|_| entry(WRONG)).collect()
}

/// Run the challenge while the caller presses what the script says,
/// on a call to the Workspace that holds the code.
async fn challenged(
    lists: &Lists,
    guard: &Guard,
    call: &Answered,
    presses: &[char],
) -> Arc<TierGate> {
    challenged_in(&guard.workspace_id(), lists, guard, call, presses).await
}

/// Run the challenge on a call to one Workspace.
async fn challenged_in(
    workspace_id: &WorkspaceId,
    lists: &Lists,
    guard: &Guard,
    call: &Answered,
    presses: &[char],
) -> Arc<TierGate> {
    let agent_id = AgentId::generate();
    let party = Arc::clone(&call.party);
    let script: Vec<char> = presses.to_vec();
    let typing = tokio::spawn(async move {
        for digit in script {
            tokio::time::sleep(Duration::from_millis(200)).await;
            party.press(digit);
        }
    });
    let gate = settle_inbound(
        lists,
        &guard.keypad(),
        &call.hub,
        &guard.log(call.own_e164),
        workspace_id,
        &agent_id,
        CALLER,
    )
    .await
    .unwrap();
    typing.await.unwrap();
    gate
}

/// Type one entry into a gate after the session started. `Some` is the
/// tier the entry raised the call to.
async fn late_entry(gate: &TierGate, digits: &str) -> Option<TrustTier> {
    let mut raised = None;
    for digit in entry(digits) {
        if let Some(change) = gate.late_digit(digit).await {
            raised = Some(change.to);
        }
    }
    raised
}

/// True when the caller heard nothing but silence: no prompt played.
fn heard_only_silence(call: &Answered) -> bool {
    let silence = call.hub.codec().silence_byte();
    call.party
        .heard()
        .iter()
        .all(|packet| packet.payload.iter().all(|byte| *byte == silence))
}

#[tokio::test(start_paused = true)]
async fn a_listed_caller_with_no_code_reaches_unknown() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Trusted);
    let guard = Guard::new(Code::set(CODE));

    let gate = challenged(&lists, &guard, &call, &[]).await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert_eq!(guard.code.checks(), 0, "silence is not a wrong code");
    assert_eq!(guard.count().await, KeypadFailures::default());
}

#[tokio::test(start_paused = true)]
async fn a_trusted_listed_caller_with_the_code_reaches_trusted() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Trusted);
    let guard = Guard::new(Code::set(CODE));

    let gate = challenged(&lists, &guard, &call, &entry(CODE)).await;

    assert_eq!(gate.tier(), TrustTier::Trusted);
}

#[tokio::test(start_paused = true)]
async fn an_owner_listed_caller_with_the_code_reaches_owner() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));

    let gate = challenged(&lists, &guard, &call, &entry(CODE)).await;

    assert_eq!(gate.tier(), TrustTier::Owner);
}

#[tokio::test(start_paused = true)]
async fn three_wrong_attempts_pin_the_call_to_unknown() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));

    let gate = challenged(&lists, &guard, &call, &wrong_entries(MAX_ATTEMPTS + 1)).await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert!(gate.pinned());
    assert_eq!(
        guard.code.checks(),
        MAX_ATTEMPTS,
        "the fourth try is not read"
    );
    assert!(
        gate.late_digit('2').await.is_none(),
        "a pinned call does not rise later"
    );
}

/// The count belongs to the Workspace and outlives the Call, so a
/// spoofed caller cannot guess three codes in each of many Calls.
#[tokio::test(start_paused = true)]
async fn a_new_call_does_not_reset_the_failed_attempts_of_the_workspace() {
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));

    let first = answered_call().await;
    challenged(&lists, &guard, &first, &wrong_entries(MAX_ATTEMPTS)).await;
    assert_eq!(guard.count().await.failed_attempts, 3);

    let second = answered_call().await;
    let gate = challenged(&lists, &guard, &second, &wrong_entries(MAX_ATTEMPTS)).await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
    let count = guard.count().await;
    assert_eq!(
        count.failed_attempts, 6,
        "the second Call counts on from the first"
    );
    assert!(count.suspended_at(guard.clock.now()));
}

/// A Workspace can hold several Agents, each with an Agent Phone
/// Number. Guesses on all of those lines, at the same time, add to the
/// one count of the Workspace.
#[tokio::test(start_paused = true)]
async fn failures_on_two_agent_phone_numbers_add_to_one_count() {
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));
    let first = answered_call_on(NUMBER).await;
    let second = answered_call_on(OTHER_NUMBER).await;
    let presses = wrong_entries(MAX_ATTEMPTS);

    let (on_first, on_second) = tokio::join!(
        challenged(&lists, &guard, &first, &presses),
        challenged(&lists, &guard, &second, &presses),
    );

    assert_eq!(on_first.tier(), TrustTier::Unknown);
    assert_eq!(on_second.tier(), TrustTier::Unknown);
    let count = guard.count().await;
    assert_eq!(count.failed_attempts, 6, "both lines count to one total");
    assert!(count.suspended_at(guard.clock.now()));
}

/// The count of one Workspace is its own. Guesses on Ada's lines start
/// a delay for Ada, and Grace's caller still proves a tier.
#[tokio::test(start_paused = true)]
async fn failures_in_one_workspace_leave_another_alone() {
    let failures = Arc::new(FakeKeypadFailures::default());
    let clock = ManualClock::at(START);
    let ada = Guard::sharing(Code::set(CODE), Arc::clone(&failures), Arc::clone(&clock));
    let grace = Guard::sharing(
        Code::set("135792"),
        Arc::clone(&failures),
        Arc::clone(&clock),
    );
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);

    for _ in 0..2 {
        let call = answered_call().await;
        challenged(&lists, &ada, &call, &wrong_entries(MAX_ATTEMPTS)).await;
    }
    assert!(ada.count().await.suspended_at(clock.now()));
    assert_eq!(grace.count().await, KeypadFailures::default());

    let call = answered_call().await;
    let gate = challenged(&lists, &grace, &call, &entry("135792")).await;

    assert_eq!(gate.tier(), TrustTier::Owner);
    assert_eq!(ada.count().await.failed_attempts, 6);
}

/// During a delay no prompt plays and no digit is checked, at the
/// challenge or later in the Call, and those digits do not count. The
/// Call is still answered, at Unknown.
#[tokio::test(start_paused = true)]
async fn during_a_delay_the_code_raises_nothing_and_the_call_is_unknown() {
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));
    guard.fail(6).await;
    let delayed = guard.count().await;
    assert!(delayed.suspended_at(guard.clock.now()));

    let call = answered_call().await;
    let gate = challenged(&lists, &guard, &call, &entry(CODE)).await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert!(heard_only_silence(&call), "a prompt played during a delay");
    assert_eq!(late_entry(&gate, CODE).await, None);
    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert_eq!(guard.code.checks(), 0, "a digit was checked during a delay");
    assert_eq!(guard.count().await, delayed, "the digits of a delay count");
}

/// A delay starts at the sixth failure, doubles with each failure after
/// a delay ends, and stops at 24 hours. While each delay runs, the right
/// code raises nothing; when it ends, the next wrong code is counted.
#[tokio::test(start_paused = true)]
async fn the_delay_starts_after_six_failures_doubles_and_stops_at_a_day() {
    let guard = Guard::new(Code::set(CODE));
    // The delay in minutes after each failure.
    let delays: [Option<i64>; 18] = [
        None,
        None,
        None,
        None,
        None,
        Some(1),
        Some(2),
        Some(4),
        Some(8),
        Some(16),
        Some(32),
        Some(64),
        Some(128),
        Some(256),
        Some(512),
        Some(1_024),
        Some(24 * 60),
        Some(24 * 60),
    ];

    for (index, delay) in delays.iter().enumerate() {
        let failed_at = guard.clock.now();
        assert_eq!(late_entry(&guard.gate(TrustTier::Owner), WRONG).await, None);

        let count = guard.count().await;
        assert_eq!(count.failed_attempts as usize, index + 1);
        assert_eq!(
            count.suspended_until,
            delay.map(|minutes| failed_at + minutes * MINUTE),
            "the delay after failure {}",
            index + 1
        );
        match count.suspended_until {
            Some(until) => {
                guard.clock.set(until - 1);
                assert_eq!(
                    late_entry(&guard.gate(TrustTier::Owner), CODE).await,
                    None,
                    "the code worked during the delay after failure {}",
                    index + 1
                );
                assert_eq!(guard.count().await, count);
                guard.clock.set(until);
            }
            None => guard.clock.set(failed_at + 1_000),
        }
    }
}

/// A correct code outside a delay clears the count: after five failures
/// with no delay, and after a delay has ended.
#[tokio::test(start_paused = true)]
async fn a_correct_code_outside_a_delay_clears_the_count() {
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));
    guard.fail(5).await;

    let call = answered_call().await;
    let gate = challenged(&lists, &guard, &call, &entry(CODE)).await;

    assert_eq!(gate.tier(), TrustTier::Owner);
    assert_eq!(guard.count().await, KeypadFailures::default());

    guard.fail(6).await;
    let until = guard.count().await.suspended_until.expect("a delay runs");
    guard.clock.set(until);

    assert_eq!(
        late_entry(&guard.gate(TrustTier::Trusted), CODE).await,
        Some(TrustTier::Trusted)
    );
    assert_eq!(guard.count().await, KeypadFailures::default());
}

/// The Person clears the count in Settings, and the next correct code
/// raises the tier at once, although the delay had not ended.
#[tokio::test(start_paused = true)]
async fn a_cleared_count_lets_the_code_raise_the_tier_at_once() {
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));
    guard.fail(7).await;
    assert!(guard.count().await.suspended_at(guard.clock.now()));

    guard.failures.clear(&guard.workspace_id()).await.unwrap();

    let call = answered_call().await;
    let gate = challenged(&lists, &guard, &call, &entry(CODE)).await;
    assert_eq!(gate.tier(), TrustTier::Owner);
}

#[tokio::test(start_paused = true)]
async fn a_caller_on_no_list_is_never_challenged() {
    let call = answered_call().await;
    let lists = Lists::default();
    let guard = Guard::new(Code::set(CODE));

    let started = tokio::time::Instant::now();
    let gate = challenged(&lists, &guard, &call, &[]).await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert!(
        started.elapsed() < CHALLENGE_TIMEOUT,
        "the challenge waited for a caller it must not challenge"
    );
    assert!(
        heard_only_silence(&call),
        "a prompt played for a caller on no list"
    );

    // The digits the caller types later are not checked either, so a
    // stranger cannot run up the count of the Workspace.
    assert_eq!(late_entry(&gate, WRONG).await, None);
    assert_eq!(late_entry(&gate, CODE).await, None);
    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert_eq!(guard.code.checks(), 0);
    assert_eq!(guard.count().await, KeypadFailures::default());
}

/// Each wrong code writes `call.keypad_failed` with the tier caller ID
/// proposed and the new count of the Workspace. It never holds the
/// digits.
#[tokio::test(start_paused = true)]
async fn each_failed_attempt_writes_an_event_without_the_digits() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));

    challenged(&lists, &guard, &call, &wrong_entries(2)).await;

    let failed = guard.bus.payloads_of("call.keypad_failed");
    assert_eq!(failed.len(), 2, "{failed:?}");
    for (index, payload) in failed.iter().enumerate() {
        assert_eq!(payload["candidate_tier"], "owner");
        assert_eq!(payload["failed_attempts"], index + 1);
        assert_eq!(payload["suspended_until"], Value::Null);
        let mut keys: Vec<&str> = payload
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "call_id",
                "candidate_tier",
                "failed_attempts",
                "from",
                "suspended_until",
                "to"
            ]
        );
        assert!(
            !payload.to_string().contains(WRONG),
            "the event held the digits: {payload}"
        );
    }
}

/// The failure that starts a delay says when the delay ends.
#[tokio::test(start_paused = true)]
async fn the_failure_that_starts_a_delay_names_its_end() {
    let guard = Guard::new(Code::set(CODE));
    guard.fail(5).await;

    late_entry(&guard.gate(TrustTier::Trusted), WRONG).await;

    let failed = guard.bus.payloads_of("call.keypad_failed");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["candidate_tier"], "trusted");
    assert_eq!(failed[0]["failed_attempts"], 6);
    assert_eq!(failed[0]["suspended_until"], START + MINUTE);
}

/// A code proves a tier only on a call to the Workspace that holds
/// it: another person's code confirms nothing on this person's line.
#[tokio::test(start_paused = true)]
async fn the_code_of_another_workspace_proves_nothing() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::set(CODE));

    let gate = challenged_in(
        &WorkspaceId::generate(),
        &lists,
        &guard,
        &call,
        &entry(CODE),
    )
    .await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
}

#[tokio::test(start_paused = true)]
async fn a_workspace_with_no_code_never_challenges() {
    let call = answered_call().await;
    let lists = Lists::listing(&WorkspaceId::generate(), CALLER, TrustTier::Owner);
    let guard = Guard::new(Code::none());

    let gate = challenged(&lists, &guard, &call, &[]).await;

    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert_eq!(late_entry(&gate, WRONG).await, None);
    assert_eq!(
        guard.count().await,
        KeypadFailures::default(),
        "a Workspace with no code counts no failure"
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_code_raises_the_tier_once() {
    let guard = Guard::new(Code::set(CODE));
    let gate = guard.gate(TrustTier::Trusted);

    let mut change = None;
    for digit in entry(CODE) {
        change = gate.late_digit(digit).await.or(change);
    }

    let change = change.expect("the late code raised the tier");
    assert_eq!(change.from, TrustTier::Unknown);
    assert_eq!(change.to, TrustTier::Trusted);
    assert_eq!(change.cause, TierCause::Code);
    assert_eq!(gate.tier(), TrustTier::Trusted);
    assert!(
        gate.late_digit('1').await.is_none(),
        "a tier rises once and no more"
    );
}

#[tokio::test(start_paused = true)]
async fn only_the_user_drops_the_tier_and_the_call_stays_down() {
    let gate = TierGate::outbound(TrustTier::Owner);

    let change = gate.drop_to_unknown().expect("the user dropped the tier");

    assert_eq!(change.from, TrustTier::Owner);
    assert_eq!(change.to, TrustTier::Unknown);
    assert_eq!(change.cause, TierCause::UserRevoked);
    assert_eq!(gate.tier(), TrustTier::Unknown);
    assert!(gate.drop_to_unknown().is_none());
    assert!(
        gate.late_digit('2').await.is_none(),
        "a dropped tier does not rise again on this call"
    );
}

/// The tier gate of a live call is found only under the Workspace of
/// the call (ADR-0023). Another Workspace that names the Call id finds
/// no gate and drops nothing, and the call keeps its tier.
#[tokio::test]
async fn a_live_tier_is_found_only_under_the_workspace_of_its_call() {
    let guard = Guard::new(Code::none());
    let owner = guard.workspace_id();
    let other = WorkspaceId::generate();
    let call_id = CallId::generate();
    let gate = Arc::new(TierGate::outbound(TrustTier::Trusted));
    let live = LiveTiers::default();
    live.bind(
        &owner,
        &call_id,
        Arc::clone(&gate),
        Arc::new(guard.log(NUMBER)),
    );

    assert!(live.gate(&other, &call_id).is_none());
    assert!(live.drop_to_unknown(&other, &call_id).await.is_none());
    assert_eq!(gate.tier(), TrustTier::Trusted);
    assert!(guard.bus.payloads_of("call.tier_changed").is_empty());

    let own = live.gate(&owner, &call_id).expect("the gate of the call");
    assert!(Arc::ptr_eq(&own, &gate));
    let change = live
        .drop_to_unknown(&owner, &call_id)
        .await
        .expect("the call is live")
        .expect("the audit log keeps the change")
        .expect("the tier falls");
    assert_eq!(change.to, TrustTier::Unknown);
    assert_eq!(gate.tier(), TrustTier::Unknown);

    live.release(&owner, &call_id);
    assert!(live.gate(&owner, &call_id).is_none());
}

#[tokio::test(start_paused = true)]
async fn an_outbound_call_holds_the_listed_tier_with_no_challenge() {
    let gate = TierGate::outbound(TrustTier::Trusted);

    assert_eq!(gate.tier(), TrustTier::Trusted);
}
