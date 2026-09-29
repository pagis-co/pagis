//! A carrier for tests (ADR-0020, ADR-0020), one fake per seam. The
//! number half answers from an inventory; the call half is a registrar
//! that answers from memory and a Remote Party a test can script; the
//! text half carries texts in memory. All three live beside the seams,
//! so a test of the number lifecycle, of the endpoint task, of a call
//! or of a text starts no process and reaches no network. The
//! failed-attempt count of the Keypad Code (ADR-0021) is in memory too,
//! so a test of the tier gate needs no database.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    KeypadFailureStore, KeypadFailures, PhoneNumber, StoreError, TextDeliveryStatus, UnixMillis,
    WorkspaceId,
};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::audio::{Codec, FRAME_TIMESTAMP_STEP, Frame};
use crate::catalog::{
    AvailableNumber, CarrierKey, CatalogError, CatalogErrorCode, NumberCatalog, NumberSearch,
    PurchasedNumber,
};
use crate::dtmf;
use crate::endpoint::NumberDirectory;
use crate::leg::{EndedReason, LegEvent, MediaLeg, PacketSink, RtpPacket};
use crate::text::{InboundText, Prepared, SentText, TextCapabilities, TextError, TextTransport};
use crate::transport::{
    Answer, CallTransport, IncomingCall, Line, Opened, Refusal, SipCredential,
    TransportCapabilities, TransportError, TransportErrorCode,
};

/// What the fake was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarrierCall {
    Search(NumberSearch),
    Buy {
        e164: String,
        idempotency_key: String,
    },
    FindPurchased(String),
    Release(String),
    PrepareSipConnection {
        username: String,
    },
}

struct PurchaseReplyBarrier {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

/// A carrier with an inventory and a memory. Wrap it in an `Arc` and
/// script it from the test.
#[derive(Default)]
pub struct FakeNumberCatalog {
    offered: Mutex<Vec<AvailableNumber>>,
    held: Mutex<Vec<PurchasedNumber>>,
    calls: Mutex<Vec<CarrierCall>>,
    /// The key the last request signed with.
    last_key: Mutex<Option<CarrierKey>>,
    fail_with: Mutex<Option<CatalogErrorCode>>,
    /// A purchase the carrier completed while the daemon was down: the
    /// number lands in `held` and `buy` is never answered.
    swallow_next_purchase: Mutex<bool>,
    pause_next_purchase: Mutex<Option<PurchaseReplyBarrier>>,
}

impl FakeNumberCatalog {
    /// A carrier that offers these numbers at one dollar a month.
    pub fn offering(numbers: &[&str]) -> Self {
        let catalog = Self::default();
        catalog.offer(numbers);
        catalog
    }

    pub fn offer(&self, numbers: &[&str]) {
        let mut offered = self.offered.lock().unwrap();
        for e164 in numbers {
            offered.push(AvailableNumber {
                e164: (*e164).to_string(),
                region: Some("San Francisco, CA".to_string()),
                monthly_cost: Some("1.00".to_string()),
                currency: Some("USD".to_string()),
            });
        }
    }

    /// Every later request fails with this code, until it is cleared.
    pub fn fail_with(&self, code: Option<CatalogErrorCode>) {
        *self.fail_with.lock().unwrap() = code;
    }

    /// The next `buy` sells the number and never answers, as a carrier
    /// does when the daemon dies between the order and the reply.
    pub fn swallow_next_purchase(&self) {
        *self.swallow_next_purchase.lock().unwrap() = true;
    }

    /// Hold the next purchase reply after the fake carrier has sold the number.
    pub fn pause_next_purchase(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.pause_next_purchase.lock().unwrap() = Some(PurchaseReplyBarrier {
            entered: entered_tx,
            release: release_rx,
        });
        (entered_rx, release_tx)
    }

    pub fn calls(&self) -> Vec<CarrierCall> {
        self.calls.lock().unwrap().clone()
    }

    /// The key the last request signed with, so a test sees which
    /// account the desk read from the Connection.
    pub fn last_key(&self) -> Option<CarrierKey> {
        self.last_key.lock().unwrap().clone()
    }

    /// The account already holds this number, bought outside Pagis.
    pub fn hold(&self, e164: &str) {
        self.held.lock().unwrap().push(PurchasedNumber {
            e164: e164.to_string(),
            provider_number_id: format!("carrier-{e164}"),
        });
    }

    /// The numbers the account holds now.
    pub fn held(&self) -> Vec<PurchasedNumber> {
        self.held.lock().unwrap().clone()
    }

    fn refuse(&self) -> Result<(), CatalogError> {
        match *self.fail_with.lock().unwrap() {
            Some(code) => Err(CatalogError(code)),
            None => Ok(()),
        }
    }

    fn record(&self, key: &CarrierKey, call: CarrierCall) {
        *self.last_key.lock().unwrap() = Some(key.clone());
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl NumberCatalog for FakeNumberCatalog {
    async fn search(
        &self,
        key: &CarrierKey,
        search: &NumberSearch,
    ) -> Result<Vec<AvailableNumber>, CatalogError> {
        self.record(key, CarrierCall::Search(search.clone()));
        self.refuse()?;
        let held = self.held.lock().unwrap();
        Ok(self
            .offered
            .lock()
            .unwrap()
            .iter()
            .filter(|number| !held.iter().any(|sold| sold.e164 == number.e164))
            .take(search.limit as usize)
            .cloned()
            .collect())
    }

    async fn buy(
        &self,
        key: &CarrierKey,
        e164: &str,
        idempotency_key: &str,
    ) -> Result<PurchasedNumber, CatalogError> {
        self.record(
            key,
            CarrierCall::Buy {
                e164: e164.to_string(),
                idempotency_key: idempotency_key.to_string(),
            },
        );
        let offered = self
            .offered
            .lock()
            .unwrap()
            .iter()
            .any(|number| number.e164 == e164);
        let purchased = {
            let mut held = self.held.lock().unwrap();
            if !offered || held.iter().any(|number| number.e164 == e164) {
                return Err(CatalogError(CatalogErrorCode::NumberUnavailable));
            }
            let purchased = PurchasedNumber {
                e164: e164.to_string(),
                provider_number_id: format!("carrier-{}", e164.trim_start_matches('+')),
            };
            held.push(purchased.clone());
            purchased
        };
        let swallowed = std::mem::take(&mut *self.swallow_next_purchase.lock().unwrap());
        if swallowed {
            return Err(CatalogError(CatalogErrorCode::TemporarilyUnavailable));
        }
        let pause = self.pause_next_purchase.lock().unwrap().take();
        if let Some(barrier) = pause {
            let _ = barrier.entered.send(());
            let _ = barrier.release.await;
        }
        self.refuse()?;
        Ok(purchased)
    }

    async fn find_purchased(
        &self,
        key: &CarrierKey,
        e164: &str,
    ) -> Result<Option<PurchasedNumber>, CatalogError> {
        self.record(key, CarrierCall::FindPurchased(e164.to_string()));
        self.refuse()?;
        Ok(self
            .held
            .lock()
            .unwrap()
            .iter()
            .find(|number| number.e164 == e164)
            .cloned())
    }

    async fn release(
        &self,
        key: &CarrierKey,
        provider_number_id: &str,
    ) -> Result<(), CatalogError> {
        self.record(key, CarrierCall::Release(provider_number_id.to_string()));
        self.refuse()?;
        self.held
            .lock()
            .unwrap()
            .retain(|number| number.provider_number_id != provider_number_id);
        Ok(())
    }

    async fn prepare_sip_connection(
        &self,
        key: &CarrierKey,
        username: &str,
    ) -> Result<bool, CatalogError> {
        self.record(
            key,
            CarrierCall::PrepareSipConnection {
                username: username.to_string(),
            },
        );
        self.refuse()?;
        Ok(true)
    }
}

/// What the fake reads the time from. The registrar of
/// [`FakeCallTransport`] forgets a binding by this clock, so a test on
/// paused tokio time drives an expiry without waiting for one.
pub trait Clock: Send + Sync {
    fn now(&self) -> tokio::time::Instant;
}

/// Tokio's clock, which `start_paused` tests control.
pub struct TokioClock;

impl Clock for TokioClock {
    fn now(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

/// The failed-attempt count of each Workspace, in memory. It counts by
/// the rule of [`KeypadFailures::after_failure`], as the stores do.
#[derive(Default)]
pub struct FakeKeypadFailures {
    counts: Mutex<HashMap<WorkspaceId, KeypadFailures>>,
}

#[async_trait]
impl KeypadFailureStore for FakeKeypadFailures {
    async fn get(&self, workspace_id: &WorkspaceId) -> Result<KeypadFailures, StoreError> {
        Ok(self
            .counts
            .lock()
            .expect("lock")
            .get(workspace_id)
            .copied()
            .unwrap_or_default())
    }

    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        now: UnixMillis,
    ) -> Result<KeypadFailures, StoreError> {
        let mut counts = self.counts.lock().expect("lock");
        let count = counts.entry(workspace_id.clone()).or_default();
        *count = count.after_failure(now);
        Ok(*count)
    }

    async fn clear(&self, workspace_id: &WorkspaceId) -> Result<(), StoreError> {
        self.counts.lock().expect("lock").remove(workspace_id);
        Ok(())
    }
}

/// What the fake transport was asked to do, in order. The registrar
/// knows a line by the SIP username it signs in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineCall {
    Open {
        username: String,
    },
    Register {
        username: String,
        expires: Duration,
        /// When the registrar saw it, by the fake's clock.
        at: tokio::time::Instant,
    },
    Unregister {
        username: String,
    },
    Dial {
        from: String,
        to: String,
    },
}

/// Where one Remote Party stands in its call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyState {
    /// The `INVITE` is out and nothing has answered it.
    Calling,
    Ringing,
    Answered,
    /// The daemon turned the call away before it answered.
    Refused(Refusal),
    Ended(EndedReason),
}

/// The codec every fake call settles on.
const FAKE_CODEC: Codec = Codec::Pcmu;
/// The telephone-event payload type every fake call accepts.
const FAKE_DTMF: u8 = 101;
/// The SSRC of every Remote Party's stream.
const PARTY_SSRC: u32 = 0xFA4E;
/// How many events a Remote Party may have in flight before its script
/// must wait for the hub.
const PARTY_DEPTH: usize = 256;

/// The far side of one fake call: the person or system the daemon
/// called, or the one that called it. A test scripts it and reads
/// what it heard.
pub struct RemoteParty {
    from: String,
    to: String,
    state: watch::Sender<PartyState>,
    /// Into the daemon's leg. `None` before the leg exists and after
    /// the call ended.
    feed: Mutex<Option<mpsc::Sender<LegEvent>>>,
    heard: Mutex<Vec<RtpPacket>>,
    sequence: Mutex<u16>,
    timestamp: Mutex<u32>,
}

impl RemoteParty {
    fn new(from: &str, to: &str) -> Arc<Self> {
        let (state, _) = watch::channel(PartyState::Calling);
        Arc::new(Self {
            from: from.to_string(),
            to: to.to_string(),
            state,
            feed: Mutex::new(None),
            heard: Mutex::new(Vec::new()),
            sequence: Mutex::new(1),
            timestamp: Mutex::new(0),
        })
    }

    /// The number that called.
    pub fn from(&self) -> &str {
        &self.from
    }

    /// The number that was called.
    pub fn to(&self) -> &str {
        &self.to
    }

    pub fn state(&self) -> PartyState {
        *self.state.borrow()
    }

    /// A receiver that sees every state change.
    pub fn watch(&self) -> watch::Receiver<PartyState> {
        self.state.subscribe()
    }

    /// The far side rings (a call the daemon placed).
    pub fn ring(&self) {
        self.state.send_replace(PartyState::Ringing);
        self.feed(LegEvent::Ringing);
    }

    /// The far side picks up (a call the daemon placed).
    pub fn answer(&self) {
        self.state.send_replace(PartyState::Answered);
        self.feed(LegEvent::Answered);
    }

    /// The far side turns the call down (a call the daemon placed).
    pub fn refuse(&self, reason: EndedReason) {
        self.end(reason);
    }

    /// The far side hangs up.
    pub fn hangup(&self) {
        self.end(EndedReason::RemoteHangup);
    }

    /// One frame of speech to the daemon.
    pub fn speak(&self, frame: Frame) {
        let packet = RtpPacket {
            ssrc: PARTY_SSRC,
            payload_type: frame.codec().payload_type(),
            sequence: self.next_sequence(),
            timestamp: self.next_timestamp(),
            marker: false,
            payload: frame.payload().clone(),
        };
        self.feed(LegEvent::Rtp(packet));
    }

    /// One key press to the daemon, as RFC 4733 sends it: start packets
    /// with a growing duration, then three end packets.
    pub fn press(&self, digit: char) {
        let code = dtmf::digit_to_code(digit).expect("a keypad digit");
        let timestamp = self.next_timestamp();
        let packets = [
            (false, 1),
            (false, 2),
            (false, 3),
            (true, 3),
            (true, 3),
            (true, 3),
        ];
        for (index, (end, frames)) in packets.into_iter().enumerate() {
            self.feed(LegEvent::Rtp(RtpPacket {
                ssrc: PARTY_SSRC,
                payload_type: FAKE_DTMF,
                sequence: self.next_sequence(),
                timestamp,
                marker: index == 0,
                payload: dtmf::encode(&dtmf::Event {
                    code,
                    end,
                    duration: frames * FRAME_TIMESTAMP_STEP as u16,
                })
                .to_vec()
                .into(),
            }));
        }
    }

    /// Every packet the daemon sent, in order.
    pub fn heard(&self) -> Vec<RtpPacket> {
        self.heard.lock().unwrap().clone()
    }

    /// The key presses the daemon sent, each once.
    pub fn digits(&self) -> Vec<char> {
        let mut reported = None;
        self.heard()
            .iter()
            .filter(|packet| packet.payload_type == FAKE_DTMF)
            .filter_map(|packet| {
                let event = dtmf::decode(&packet.payload)?;
                if !event.end || reported == Some(packet.timestamp) {
                    return None;
                }
                reported = Some(packet.timestamp);
                dtmf::digit_from_code(event.code)
            })
            .collect()
    }

    fn end(&self, reason: EndedReason) {
        self.state.send_replace(PartyState::Ended(reason));
        self.feed(LegEvent::Ended(reason));
        self.feed.lock().unwrap().take();
    }

    fn feed(&self, event: LegEvent) {
        let feed = self.feed.lock().unwrap().clone();
        if let Some(feed) = feed {
            feed.try_send(event)
                .expect("the daemon's leg fell more than PARTY_DEPTH events behind");
        }
    }

    fn next_sequence(&self) -> u16 {
        let mut sequence = self.sequence.lock().unwrap();
        let next = *sequence;
        *sequence = next.wrapping_add(1);
        next
    }

    fn next_timestamp(&self) -> u32 {
        let mut timestamp = self.timestamp.lock().unwrap();
        let next = *timestamp;
        *timestamp = next.wrapping_add(FRAME_TIMESTAMP_STEP);
        next
    }

    /// The daemon's leg of this call. The party feeds it from now on.
    fn attach_leg(self: &Arc<Self>) -> MediaLeg {
        let (feed, inbound) = mpsc::channel(PARTY_DEPTH);
        *self.feed.lock().unwrap() = Some(feed);
        MediaLeg {
            codec: FAKE_CODEC,
            dtmf_payload_type: Some(FAKE_DTMF),
            ssrc: rand::random::<u32>(),
            inbound,
            sink: Arc::new(FakeSink {
                party: Arc::clone(self),
            }),
        }
    }
}

/// The daemon's side of the wire to one Remote Party.
struct FakeSink {
    party: Arc<RemoteParty>,
}

#[async_trait]
impl PacketSink for FakeSink {
    async fn send(&self, packet: RtpPacket) {
        self.party.heard.lock().unwrap().push(packet);
    }

    async fn hangup(&self) {
        self.party.end(EndedReason::LocalHangup);
    }
}

/// An `INVITE` the fake carrier delivered to a line.
struct FakeAnswer {
    party: Arc<RemoteParty>,
}

#[async_trait]
impl Answer for FakeAnswer {
    async fn accept(self: Box<Self>) -> Result<MediaLeg, TransportError> {
        let leg = self.party.attach_leg();
        self.party.answer();
        Ok(leg)
    }

    async fn reject(self: Box<Self>, refusal: Refusal) {
        self.party.state.send_replace(PartyState::Refused(refusal));
    }
}

/// The registrar's memory and the script a test writes on it.
struct FakeRegistrar {
    clock: Arc<dyn Clock>,
    /// Each registered SIP username and when its binding expires.
    bindings: Mutex<HashMap<String, tokio::time::Instant>>,
    /// The line that registered last, and where it takes the calls.
    /// The account has one address of record, and this registrar keeps
    /// one contact on it, as a carrier does: every `INVITE` goes to the
    /// contact that registered last, whatever number it dials.
    contact: Mutex<Option<(u64, mpsc::Sender<IncomingCall>)>>,
    /// The id of the next line that opens.
    next_line: std::sync::atomic::AtomicU64,
    /// The calls the daemon placed, in order.
    dials: Mutex<Vec<Arc<RemoteParty>>>,
    /// The expiry the registrar grants; `None` grants what was asked.
    granted_expiry: Mutex<Option<Duration>>,
    fail_with: Mutex<Option<TransportErrorCode>>,
    calls: Mutex<Vec<LineCall>>,
}

impl FakeRegistrar {
    fn record(&self, call: LineCall) {
        self.calls.lock().unwrap().push(call);
    }

    fn refuse(&self) -> Result<(), TransportError> {
        match *self.fail_with.lock().unwrap() {
            Some(code) => Err(TransportError(code)),
            None => Ok(()),
        }
    }
}

/// A carrier whose registrar answers from memory (ADR-0020). Its
/// capability set is whatever the test declares, so a test runs against
/// a provider that has a capability and against one that does not.
pub struct FakeCallTransport {
    registrar: Arc<FakeRegistrar>,
    capabilities: Mutex<TransportCapabilities>,
}

impl FakeCallTransport {
    /// A transport shaped like Telnyx over SIP: DTMF out, no AMD, no
    /// wideband, no ingress.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            registrar: Arc::new(FakeRegistrar {
                clock,
                bindings: Mutex::new(HashMap::new()),
                contact: Mutex::new(None),
                next_line: std::sync::atomic::AtomicU64::new(1),
                dials: Mutex::new(Vec::new()),
                granted_expiry: Mutex::new(None),
                fail_with: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
            }),
            capabilities: Mutex::new(TransportCapabilities {
                send_dtmf: true,
                answering_machine_detection: false,
                wideband_audio: false,
                public_ingress_required: false,
            }),
        }
    }

    pub fn set_capabilities(&self, capabilities: TransportCapabilities) {
        *self.capabilities.lock().unwrap() = capabilities;
    }

    /// The expiry the registrar grants from now on; `None` grants what
    /// each `REGISTER` asks for.
    pub fn grant_expiry(&self, expiry: Option<Duration>) {
        *self.registrar.granted_expiry.lock().unwrap() = expiry;
    }

    /// Every later `REGISTER` and `INVITE` fails with this code, until
    /// it is cleared.
    pub fn fail_with(&self, code: Option<TransportErrorCode>) {
        *self.registrar.fail_with.lock().unwrap() = code;
    }

    /// Whether the registrar holds a binding for the SIP username now.
    /// A binding that was not refreshed in time is gone.
    pub fn is_registered(&self, username: &str) -> bool {
        self.registrar
            .bindings
            .lock()
            .unwrap()
            .get(username)
            .is_some_and(|until| *until > self.registrar.clock.now())
    }

    pub fn calls(&self) -> Vec<LineCall> {
        self.registrar.calls.lock().unwrap().clone()
    }

    /// The far side of every call the daemon placed, in order.
    pub fn dials(&self) -> Vec<Arc<RemoteParty>> {
        self.registrar.dials.lock().unwrap().clone()
    }

    /// A call from `from` to the dialed number arrives at the contact
    /// that registered last. `dialed` is the user part the carrier
    /// puts in the `INVITE`, and the line reads it the way the SIP
    /// transport does: an empty or non-E.164 value names no number.
    /// The party stands at `Calling` until the endpoint task answers
    /// or turns it away. With no registered line the call gets no
    /// answer, as at a real carrier.
    pub fn ring(&self, dialed: &str, from: &str) -> Arc<RemoteParty> {
        let party = RemoteParty::new(from, dialed);
        let line = self
            .registrar
            .contact
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, line)| line.clone());
        let delivered = line.is_some_and(|line| {
            line.try_send(IncomingCall {
                dialed_e164: pagis_core::normalize_e164(dialed),
                from_e164: from.to_string(),
                answer: Box::new(FakeAnswer {
                    party: Arc::clone(&party),
                }),
            })
            .is_ok()
        });
        if !delivered {
            party
                .state
                .send_replace(PartyState::Ended(EndedReason::NoAnswer));
        }
        party
    }
}

#[async_trait]
impl CallTransport for FakeCallTransport {
    fn capabilities(&self) -> TransportCapabilities {
        *self.capabilities.lock().unwrap()
    }

    async fn open(&self, credential: &SipCredential) -> Result<Opened, TransportError> {
        self.registrar.record(LineCall::Open {
            username: credential.username().to_string(),
        });
        let (calls, incoming) = mpsc::channel(8);
        Ok(Opened {
            line: Box::new(FakeLine {
                registrar: Arc::clone(&self.registrar),
                id: self
                    .registrar
                    .next_line
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                username: credential.username().to_string(),
                calls,
            }),
            incoming,
        })
    }
}

struct FakeLine {
    registrar: Arc<FakeRegistrar>,
    id: u64,
    username: String,
    /// Where the registrar sends the calls once this line is the
    /// contact.
    calls: mpsc::Sender<IncomingCall>,
}

impl FakeLine {
    /// Take the calls away from this line, when it is the contact.
    fn drop_contact(&self) {
        let mut contact = self.registrar.contact.lock().unwrap();
        if contact.as_ref().is_some_and(|(id, _)| *id == self.id) {
            *contact = None;
        }
    }
}

#[async_trait]
impl Line for FakeLine {
    async fn register(&mut self, expires: Duration) -> Result<Duration, TransportError> {
        let now = self.registrar.clock.now();
        self.registrar.record(LineCall::Register {
            username: self.username.clone(),
            expires,
            at: now,
        });
        self.registrar.refuse()?;
        let granted = self
            .registrar
            .granted_expiry
            .lock()
            .unwrap()
            .unwrap_or(expires);
        self.registrar
            .bindings
            .lock()
            .unwrap()
            .insert(self.username.clone(), now + granted);
        *self.registrar.contact.lock().unwrap() = Some((self.id, self.calls.clone()));
        Ok(granted)
    }

    async fn unregister(&mut self) -> Result<(), TransportError> {
        self.registrar.record(LineCall::Unregister {
            username: self.username.clone(),
        });
        self.registrar.refuse()?;
        self.registrar
            .bindings
            .lock()
            .unwrap()
            .remove(&self.username);
        self.drop_contact();
        Ok(())
    }

    async fn dial(&mut self, from_e164: &str, to_e164: &str) -> Result<MediaLeg, TransportError> {
        self.registrar.record(LineCall::Dial {
            from: from_e164.to_string(),
            to: to_e164.to_string(),
        });
        self.registrar.refuse()?;
        let party = RemoteParty::new(from_e164, to_e164);
        let leg = party.attach_leg();
        self.registrar.dials.lock().unwrap().push(party);
        Ok(leg)
    }
}

impl Drop for FakeLine {
    fn drop(&mut self) {
        // A dropped line takes no more calls; the endpoint task opens a
        // fresh one on its next attempt.
        self.drop_contact();
    }
}

/// The stored Agent Phone Numbers a line routes by, in memory
/// (ADR-0020). A test of the line on a paused clock reads its numbers
/// here instead of from a database.
#[derive(Default)]
pub struct FakeNumberDirectory {
    numbers: Mutex<Vec<PhoneNumber>>,
}

impl FakeNumberDirectory {
    /// A directory with these records.
    pub fn with(numbers: Vec<PhoneNumber>) -> Self {
        Self {
            numbers: Mutex::new(numbers),
        }
    }

    /// A record of one number that an Agent of a Workspace holds, as a
    /// purchase from the Agent's page writes it. Each call makes a new
    /// Workspace and a new Agent.
    pub fn held(e164: &str) -> PhoneNumber {
        PhoneNumber::new(
            pagis_core::PhoneNumberId::generate(),
            pagis_core::WorkspaceId::generate(),
            pagis_core::ConnectionId::generate(),
            e164.to_string(),
            format!("carrier-{e164}"),
            Some(pagis_core::AgentId::generate()),
            pagis_core::now_ms(),
        )
    }
}

#[async_trait]
impl NumberDirectory for FakeNumberDirectory {
    async fn lookup(&self, e164: &str) -> Result<Vec<PhoneNumber>, pagis_core::StoreError> {
        Ok(self
            .numbers
            .lock()
            .unwrap()
            .iter()
            .filter(|number| {
                number.e164 == e164 && number.status != pagis_core::PhoneNumberStatus::Released
            })
            .cloned()
            .collect())
    }
}

/// One text the daemon sent through the fake, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSend {
    pub from_e164: String,
    pub to_e164: String,
    pub body: String,
    /// The messaging object the number carried at the send. Twilio
    /// sends carry it beside `From`, so a test reads that it was there.
    pub messaging_object_id: Option<String>,
    /// The id the fake answered with.
    pub carrier_id: String,
    pub segments: u32,
}

/// One number the daemon asked the fake to prepare, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct TextPrepare {
    pub e164: String,
    /// The carrier Connection's `config` map as it stood.
    pub connection_config: Value,
}

/// How many characters the fake bills as one segment. A real carrier
/// counts the same way for the plain alphabet.
const FAKE_SEGMENT_CHARS: usize = 160;

/// A carrier that carries texts in memory (ADR-0020). A test pushes
/// inbound texts and delivery states into it, reads the sends out of
/// it, and sets its capabilities, so one test runs against a carrier
/// with texting and the next against one without.
///
/// The cursor is the count of the texts the collector already read, as
/// a decimal string. A cursor the fake did not give is refused, so a
/// test that loses the cursor learns it here and not at the carrier.
pub struct FakeTextTransport {
    capabilities: Mutex<TextCapabilities>,
    prepares: Mutex<Vec<TextPrepare>>,
    prepared: Mutex<Option<Prepared>>,
    sends: Mutex<Vec<TextSend>>,
    /// The inbound queue of each number, oldest first.
    inbound: Mutex<HashMap<String, Vec<InboundText>>>,
    delivery: Mutex<HashMap<String, TextDeliveryStatus>>,
    media: Mutex<HashMap<String, Vec<u8>>>,
    /// The key the last request signed with.
    last_key: Mutex<Option<CarrierKey>>,
    fail_with: Mutex<Option<TextError>>,
}

impl Default for FakeTextTransport {
    fn default() -> Self {
        Self {
            capabilities: Mutex::new(TextCapabilities {
                texting: true,
                inbound_media: true,
            }),
            prepares: Mutex::new(Vec::new()),
            prepared: Mutex::new(None),
            sends: Mutex::new(Vec::new()),
            inbound: Mutex::new(HashMap::new()),
            delivery: Mutex::new(HashMap::new()),
            media: Mutex::new(HashMap::new()),
            last_key: Mutex::new(None),
            fail_with: Mutex::new(None),
        }
    }
}

impl FakeTextTransport {
    /// A carrier that carries no text, as Plivo does.
    pub fn without_texting() -> Self {
        let transport = Self::default();
        transport.set_capabilities(TextCapabilities::ABSENT);
        transport
    }

    pub fn set_capabilities(&self, capabilities: TextCapabilities) {
        *self.capabilities.lock().unwrap() = capabilities;
    }

    /// What `prepare` answers from now on. Without one it answers a
    /// messaging object named after the number and no Connection key.
    pub fn set_prepared(&self, prepared: Prepared) {
        *self.prepared.lock().unwrap() = Some(prepared);
    }

    /// The numbers the daemon asked to prepare, in order.
    pub fn prepares(&self) -> Vec<TextPrepare> {
        self.prepares.lock().unwrap().clone()
    }

    /// The texts the daemon sent, in order.
    pub fn sends(&self) -> Vec<TextSend> {
        self.sends.lock().unwrap().clone()
    }

    /// A text arrives for the number it names. The collector reads it
    /// on its next poll.
    pub fn push_inbound(&self, text: InboundText) {
        self.inbound
            .lock()
            .unwrap()
            .entry(text.to_e164.clone())
            .or_default()
            .push(text);
    }

    /// The carrier's receipt for one sent text. A send starts at
    /// `Queued`, and this is how the carrier moves it.
    pub fn set_delivery_status(&self, carrier_id: &str, status: TextDeliveryStatus) {
        self.delivery
            .lock()
            .unwrap()
            .insert(carrier_id.to_string(), status);
    }

    /// The bytes the carrier serves at one media URL.
    pub fn push_media(&self, url: &str, bytes: Vec<u8>) {
        self.media.lock().unwrap().insert(url.to_string(), bytes);
    }

    /// Every later request fails with this error, until it is cleared.
    pub fn fail_with(&self, error: Option<TextError>) {
        *self.fail_with.lock().unwrap() = error;
    }

    /// The key the last request signed with.
    pub fn last_key(&self) -> Option<CarrierKey> {
        self.last_key.lock().unwrap().clone()
    }

    /// The two refusals every method makes first: a carrier with
    /// texting absent carries nothing, and a scripted failure stands
    /// in for the carrier's own.
    fn accept(&self, key: &CarrierKey) -> Result<(), TextError> {
        *self.last_key.lock().unwrap() = Some(key.clone());
        if !self.capabilities().texting {
            return Err(TextError::TextingAbsent);
        }
        match self.fail_with.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl TextTransport for FakeTextTransport {
    fn capabilities(&self) -> TextCapabilities {
        *self.capabilities.lock().unwrap()
    }

    async fn prepare(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        connection_config: &Value,
    ) -> Result<Prepared, TextError> {
        self.accept(key)?;
        self.prepares.lock().unwrap().push(TextPrepare {
            e164: number.e164.clone(),
            connection_config: connection_config.clone(),
        });
        Ok(self
            .prepared
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| Prepared {
                messaging_object_id: Some(format!("messaging-object-{}", number.e164)),
                connection_config: Map::new(),
            }))
    }

    async fn send(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        to_e164: &str,
        body: &str,
    ) -> Result<SentText, TextError> {
        self.accept(key)?;
        let mut sends = self.sends.lock().unwrap();
        let carrier_id = format!("fake-text-{}", sends.len() + 1);
        let segments = body.chars().count().div_ceil(FAKE_SEGMENT_CHARS).max(1) as u32;
        sends.push(TextSend {
            from_e164: number.e164.clone(),
            to_e164: to_e164.to_string(),
            body: body.to_string(),
            messaging_object_id: number.messaging_object_id.clone(),
            carrier_id: carrier_id.clone(),
            segments,
        });
        drop(sends);
        self.delivery
            .lock()
            .unwrap()
            .insert(carrier_id.clone(), TextDeliveryStatus::Queued);
        Ok(SentText {
            carrier_id,
            segments,
        })
    }

    async fn delivery_status(
        &self,
        key: &CarrierKey,
        carrier_id: &str,
    ) -> Result<TextDeliveryStatus, TextError> {
        self.accept(key)?;
        self.delivery
            .lock()
            .unwrap()
            .get(carrier_id)
            .cloned()
            .ok_or_else(|| TextError::Carrier {
                code: "not_found".to_string(),
                message: format!("this carrier sent no text {carrier_id}"),
            })
    }

    async fn poll_inbound(
        &self,
        key: &CarrierKey,
        number: &PhoneNumber,
        cursor: Option<&str>,
    ) -> Result<(Vec<InboundText>, Option<String>), TextError> {
        self.accept(key)?;
        let read = match cursor {
            None => 0,
            Some(cursor) => cursor.parse::<usize>().map_err(|_| {
                TextError::Unreachable(format!("this carrier gave no cursor {cursor}"))
            })?,
        };
        let inbound = self.inbound.lock().unwrap();
        let queue = inbound.get(&number.e164).cloned().unwrap_or_default();
        let fresh = queue.get(read..).unwrap_or_default().to_vec();
        Ok((fresh, Some(queue.len().to_string())))
    }

    async fn fetch_media(&self, key: &CarrierKey, url: &str) -> Result<Vec<u8>, TextError> {
        self.accept(key)?;
        if !self.capabilities().inbound_media {
            return Err(TextError::Carrier {
                code: "media_absent".to_string(),
                message: "this carrier brings no inbound media".to_string(),
            });
        }
        self.media
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| TextError::Carrier {
                code: "not_found".to_string(),
                message: format!("this carrier serves nothing at {url}"),
            })
    }
}

/// The Standing Call Rule the Trigger module would write (ADR-0020).
/// It records the lines it was asked to watch, so a test of the number
/// lifecycle reads the rule without the Trigger module.
#[derive(Default)]
pub struct FakeStandingCallRule {
    created: Mutex<Vec<(String, pagis_core::AgentId)>>,
    archived: Mutex<Vec<(String, pagis_core::AgentId)>>,
}

impl FakeStandingCallRule {
    /// The lines a rule was written for, oldest first.
    pub fn created(&self) -> Vec<(String, pagis_core::AgentId)> {
        self.created.lock().unwrap().clone()
    }

    /// The lines whose rule was archived, oldest first.
    pub fn archived(&self) -> Vec<(String, pagis_core::AgentId)> {
        self.archived.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::events::StandingCallRule for FakeStandingCallRule {
    async fn create(
        &self,
        number: &PhoneNumber,
        agent_id: &pagis_core::AgentId,
    ) -> Result<(), crate::events::StandingRuleError> {
        self.created
            .lock()
            .unwrap()
            .push((number.e164.clone(), agent_id.clone()));
        Ok(())
    }

    async fn archive(
        &self,
        number: &PhoneNumber,
        agent_id: &pagis_core::AgentId,
    ) -> Result<(), crate::events::StandingRuleError> {
        self.archived
            .lock()
            .unwrap()
            .push((number.e164.clone(), agent_id.clone()));
        Ok(())
    }
}
