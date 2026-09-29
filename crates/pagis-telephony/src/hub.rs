//! The media hub (ADR-0020): one per call. It owns the frames of
//! both directions and fans them out to the model uplink, the recorder
//! and the listen-live subscribers.
//!
//! Subscribers are lossy, so a slow browser cannot stall an RTP write.
//! The recorder has a bounded queue and its own writer task. Typed
//! events travel with the audio and keep their order. The uplink has a
//! reorder window of about 40 ms and no playout buffer. The downlink
//! has a 20 ms pacer and stays 60 ms deep or less. RTP goes out for the
//! full call, and silence is G.711 silence, so the NAT binding and the
//! RTP latch stay alive.
//!
//! Keypad digits go both ways as RFC 4733 telephone events.
//! One press takes the place of the audio for its length: the packets
//! keep one timestamp, the duration grows, and three end packets
//! repeat the final duration. Two presses of one [`MediaHub::send_digits`]
//! have a gap between them. A leg that negotiated no telephone-event
//! payload type declares the digits absent in both directions
//! (ADR-0005): Pagis sends no in-band tone and detects none.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Semaphore, broadcast, mpsc, watch};
use tokio::time::Instant;

use crate::audio::{Codec, FRAME_TIMESTAMP_STEP, Frame};
use crate::dtmf;
use crate::leg::{EndedReason, LegEvent, MediaLeg, PacketSink, RtpPacket};
use crate::reorder::Reorder;

/// How long an early packet waits for the gap before it.
pub const REORDER_WINDOW: Duration = Duration::from_millis(40);
/// How many frames of gap go between two presses of one
/// [`MediaHub::send_digits`]. RFC 4733 asks for a pause between two
/// presses, so a phone tree does not read them as one press. Two
/// frames is 40 ms, which every carrier accepts.
pub const INTER_DIGIT_FRAMES: u16 = 2;
/// How many frames the downlink holds: three, so 60 ms.
pub const DOWNLINK_DEPTH: usize = 3;
/// Inbound RTP silent for this long with no `BYE` ends the call.
pub const MEDIA_TIMEOUT: Duration = Duration::from_secs(10);
/// How far a subscriber may fall behind before it loses frames.
const SUBSCRIBER_DEPTH: usize = 64;
/// How many frames the recorder queue holds: five seconds.
const RECORDER_DEPTH: usize = 250;
const PACER_TICK: Duration = crate::audio::FRAME_DURATION;

/// What a subscriber hears: the uplink audio, and the events in their
/// place among it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubEvent {
    /// One frame from the Remote Party, in the codec it came in.
    Uplink(Frame),
    Ringing,
    Answered,
    /// One keypad press by the Remote Party.
    Dtmf(char),
    Ended(EndedReason),
}

/// What the negotiated leg does with keypad digits. A leg that
/// negotiated no telephone-event payload type declares both directions
/// absent, as ADR-0005 asks. Pagis never emulates a digit with an
/// in-band tone, and it detects no in-band tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtmfPresence {
    pub send_dtmf: bool,
    pub receive_dtmf: bool,
}

/// The leg negotiated no telephone events, so no digit can go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the negotiated leg has no telephone-event payload type")]
pub struct DtmfAbsent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the Remote Party.
    Uplink,
    /// To the Remote Party.
    Downlink,
}

/// One frame as the recorder gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub direction: Direction,
    pub frame: Frame,
}

/// Where the recorder writes. The writer runs on its own task, so a
/// slow disk does not reach the RTP path.
#[async_trait]
pub trait RecordingSink: Send + Sync {
    async fn write(&self, recorded: Recorded);
}

struct Shared {
    codec: Codec,
    dtmf_payload_type: Option<u8>,
    ssrc: u32,
    sink: Arc<dyn PacketSink>,
    events: broadcast::Sender<HubEvent>,
    /// Both directions, for the listeners. Lossy, like the
    /// other subscribers: a slow browser cannot stall an RTP write.
    listeners: broadcast::Sender<Recorded>,
    recorders: Mutex<Vec<mpsc::Sender<Recorded>>>,
    recorder_dropped: AtomicU64,
    downlink: Mutex<VecDeque<Frame>>,
    /// One permit per free slot in the downlink queue.
    downlink_space: Semaphore,
    /// Presses waiting to go out, as event codes.
    presses: Mutex<VecDeque<u8>>,
    downlink_sent: AtomicU64,
    ended: watch::Sender<Option<EndedReason>>,
}

impl Shared {
    fn publish(&self, event: HubEvent) {
        // No receiver is not an error: nothing listens yet.
        let _ = self.events.send(event);
    }

    fn record(&self, recorded: Recorded) {
        // No listener is not an error: nobody listens yet.
        let _ = self.listeners.send(recorded.clone());
        let mut recorders = self.recorders.lock().expect("lock");
        recorders.retain(|recorder| match recorder.try_send(recorded.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.recorder_dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// End once. A second reason is ignored: the first one is why the
    /// audio stopped.
    fn end(&self, reason: EndedReason) -> bool {
        let first = self.ended.send_if_modified(|ended| {
            if ended.is_some() {
                return false;
            }
            *ended = Some(reason);
            true
        });
        if first {
            self.publish(HubEvent::Ended(reason));
            self.recorders.lock().expect("lock").clear();
        }
        first
    }

    fn is_ended(&self) -> bool {
        self.ended.borrow().is_some()
    }
}

pub struct MediaHub {
    shared: Arc<Shared>,
    uplink: tokio::task::JoinHandle<()>,
    downlink: tokio::task::JoinHandle<()>,
}

impl MediaHub {
    /// Take the leg and start both directions. The downlink sends from
    /// now on; the uplink passes on what arrives.
    pub fn start(leg: MediaLeg) -> Arc<Self> {
        let (events, _) = broadcast::channel(SUBSCRIBER_DEPTH);
        let (listeners, _) = broadcast::channel(SUBSCRIBER_DEPTH);
        let (ended, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            codec: leg.codec,
            dtmf_payload_type: leg.dtmf_payload_type,
            ssrc: leg.ssrc,
            sink: leg.sink,
            events,
            listeners,
            recorders: Mutex::new(Vec::new()),
            recorder_dropped: AtomicU64::new(0),
            downlink: Mutex::new(VecDeque::with_capacity(DOWNLINK_DEPTH)),
            downlink_space: Semaphore::new(DOWNLINK_DEPTH),
            presses: Mutex::new(VecDeque::new()),
            downlink_sent: AtomicU64::new(0),
            ended,
        });
        let uplink = tokio::spawn(run_uplink(Arc::clone(&shared), leg.inbound));
        let downlink = tokio::spawn(run_downlink(Arc::clone(&shared)));
        Arc::new(Self {
            shared,
            uplink,
            downlink,
        })
    }

    pub fn codec(&self) -> Codec {
        self.shared.codec
    }

    /// A lossy view of the uplink and the events. A receiver that falls
    /// behind skips to the newest frames.
    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.shared.events.subscribe()
    }

    /// A lossy view of both directions, for Listen-Live. A
    /// listener that falls behind loses the frames it did not read.
    pub fn listen(&self) -> broadcast::Receiver<Recorded> {
        self.shared.listeners.subscribe()
    }

    /// Start a recorder. Frames of both directions queue for its writer
    /// task; when the queue is full, frames are dropped and counted.
    pub fn record(&self, sink: Box<dyn RecordingSink>) -> tokio::task::JoinHandle<()> {
        let (tx, mut rx) = mpsc::channel(RECORDER_DEPTH);
        self.shared.recorders.lock().expect("lock").push(tx);
        tokio::spawn(async move {
            while let Some(recorded) = rx.recv().await {
                sink.write(recorded).await;
            }
        })
    }

    /// How many frames the recorders lost to a full queue.
    pub fn recorder_dropped(&self) -> u64 {
        self.shared.recorder_dropped.load(Ordering::Relaxed)
    }

    /// Queue one frame for the Remote Party. Waits while the queue holds
    /// [`DOWNLINK_DEPTH`] frames, so a caller that runs ahead of the
    /// pacer stops at 60 ms.
    pub async fn send_downlink(&self, frame: Frame) {
        let Ok(permit) = self.shared.downlink_space.acquire().await else {
            return;
        };
        permit.forget();
        self.shared
            .downlink
            .lock()
            .expect("lock")
            .push_back(frame.into_codec(self.shared.codec));
    }

    /// Drop what waits in the downlink queue, for barge-in. Returns how
    /// many frames were dropped; [`downlink_packets_sent`] then says how
    /// much the Remote Party heard.
    ///
    /// [`downlink_packets_sent`]: Self::downlink_packets_sent
    pub fn clear_downlink(&self) -> usize {
        let dropped = {
            let mut downlink = self.shared.downlink.lock().expect("lock");
            let dropped = downlink.len();
            downlink.clear();
            dropped
        };
        self.shared.downlink_space.add_permits(dropped);
        dropped
    }

    /// How many audio packets went out. Times 20 ms, this is how much
    /// the Remote Party heard.
    pub fn downlink_packets_sent(&self) -> u64 {
        self.shared.downlink_sent.load(Ordering::Relaxed)
    }

    /// What this call does with keypad digits. The fact belongs to the
    /// negotiated leg, not to the transport.
    pub fn dtmf(&self) -> DtmfPresence {
        let negotiated = self.shared.dtmf_payload_type.is_some();
        DtmfPresence {
            send_dtmf: negotiated,
            receive_dtmf: negotiated,
        }
    }

    /// Press one key on the Remote Party's side. A character that is
    /// not a key, or a leg that negotiated no telephone events, sends
    /// nothing.
    pub async fn send_dtmf(&self, digit: char) {
        let mut digits = [0u8; 4];
        let _ = self.send_digits(digit.encode_utf8(&mut digits)).await;
    }

    /// Press these keys in order, with a gap of [`INTER_DIGIT_FRAMES`]
    /// between two presses. A character that is not a key is skipped.
    /// A leg that negotiated no telephone events refuses: the digit is
    /// absent, and no tone takes its place (ADR-0005).
    pub async fn send_digits(&self, digits: &str) -> Result<(), DtmfAbsent> {
        if self.shared.dtmf_payload_type.is_none() {
            return Err(DtmfAbsent);
        }
        let mut presses = self.shared.presses.lock().expect("lock");
        for digit in digits.chars() {
            match dtmf::digit_to_code(digit) {
                Some(code) => presses.push_back(code),
                None => tracing::warn!(%digit, "not a keypad digit"),
            }
        }
        Ok(())
    }

    /// End the call from this side.
    pub async fn hangup(&self) {
        if self.shared.is_ended() {
            return;
        }
        self.shared.sink.hangup().await;
    }

    /// Why the call ended, or `None` while it runs.
    pub fn ended(&self) -> Option<EndedReason> {
        *self.shared.ended.borrow()
    }

    pub fn is_ended(&self) -> bool {
        self.shared.is_ended()
    }

    /// A receiver that wakes when the call ends.
    pub fn watch_ended(&self) -> watch::Receiver<Option<EndedReason>> {
        self.shared.ended.subscribe()
    }
}

impl Drop for MediaHub {
    fn drop(&mut self) {
        self.uplink.abort();
        self.downlink.abort();
    }
}

/// The uplink: reorder, pass on, and keep the events in their place.
async fn run_uplink(shared: Arc<Shared>, mut inbound: mpsc::Receiver<LegEvent>) {
    let mut reorder = Reorder::new(REORDER_WINDOW);
    let mut dtmf = DtmfReader::default();
    let mut answered_at: Option<Instant> = None;
    let mut last_rtp = Instant::now();
    loop {
        let reorder_deadline = reorder.deadline();
        let media_deadline = answered_at.map(|_| last_rtp + MEDIA_TIMEOUT);
        tokio::select! {
            event = inbound.recv() => match event {
                Some(LegEvent::Rtp(packet)) => {
                    last_rtp = Instant::now();
                    for packet in reorder.push(last_rtp, packet) {
                        deliver(&shared, &mut dtmf, packet);
                    }
                }
                Some(other) => {
                    for packet in reorder.flush() {
                        deliver(&shared, &mut dtmf, packet);
                    }
                    match other {
                        LegEvent::Ringing => shared.publish(HubEvent::Ringing),
                        LegEvent::Answered => {
                            let now = Instant::now();
                            answered_at = Some(now);
                            last_rtp = now;
                            shared.publish(HubEvent::Answered);
                        }
                        LegEvent::Ended(reason) => {
                            shared.end(reason);
                            return;
                        }
                        LegEvent::Rtp(_) => unreachable!("handled above"),
                    }
                }
                None => {
                    for packet in reorder.flush() {
                        deliver(&shared, &mut dtmf, packet);
                    }
                    shared.end(EndedReason::TransportLost);
                    return;
                }
            },
            () = sleep_until_or_never(reorder_deadline) => {
                for packet in reorder.expire(Instant::now()) {
                    deliver(&shared, &mut dtmf, packet);
                }
            }
            () = sleep_until_or_never(media_deadline) => {
                tracing::warn!("no inbound RTP for {MEDIA_TIMEOUT:?}; ending the call");
                if shared.end(EndedReason::MediaTimeout) {
                    shared.sink.hangup().await;
                }
                return;
            }
        }
    }
}

async fn sleep_until_or_never(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Reports each press once, at its first end packet.
#[derive(Default)]
struct DtmfReader {
    reported: Option<u32>,
}

impl DtmfReader {
    fn read(&mut self, packet: &RtpPacket) -> Option<char> {
        let event = dtmf::decode(&packet.payload)?;
        if !event.end || self.reported == Some(packet.timestamp) {
            return None;
        }
        self.reported = Some(packet.timestamp);
        dtmf::digit_from_code(event.code)
    }
}

fn deliver(shared: &Shared, dtmf: &mut DtmfReader, packet: RtpPacket) {
    if Some(packet.payload_type) == shared.dtmf_payload_type {
        if let Some(digit) = dtmf.read(&packet) {
            shared.publish(HubEvent::Dtmf(digit));
        }
        return;
    }
    let Some(codec) = Codec::from_payload_type(packet.payload_type) else {
        tracing::trace!(
            payload_type = packet.payload_type,
            "dropped a packet of an unknown type"
        );
        return;
    };
    let frame = Frame::new(codec, packet.payload);
    shared.record(Recorded {
        direction: Direction::Uplink,
        frame: frame.clone(),
    });
    shared.publish(HubEvent::Uplink(frame));
}

/// One press on its way out: start packets, then the end packets.
struct Press {
    code: u8,
    timestamp: u32,
    frames_sent: u16,
    ends_sent: usize,
}

/// The downlink: one packet every 20 ms, from the queue or silence,
/// with a press taking the place of the audio while it lasts.
async fn run_downlink(shared: Arc<Shared>) {
    let mut ticks = tokio::time::interval(PACER_TICK);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ended = shared.ended.subscribe();
    let mut sequence: u16 = rand_start() as u16;
    let mut timestamp: u32 = rand_start();
    let mut first = true;
    let mut press: Option<Press> = None;
    // How many frames of gap are still owed to the press before.
    let mut gap: u16 = 0;
    loop {
        tokio::select! {
            _ = ended.changed() => return,
            _ = ticks.tick() => {}
        }
        if shared.is_ended() {
            return;
        }
        if press.is_none() && gap > 0 {
            gap -= 1;
        } else if press.is_none() {
            press = shared
                .presses
                .lock()
                .expect("lock")
                .pop_front()
                .map(|code| Press {
                    code,
                    timestamp,
                    frames_sent: 0,
                    ends_sent: 0,
                });
        }
        let packet = match press.as_mut() {
            Some(active) => {
                let (packet, done) = press_packet(&shared, active, sequence);
                if done {
                    press = None;
                    gap = INTER_DIGIT_FRAMES;
                }
                shared.record(Recorded {
                    direction: Direction::Downlink,
                    frame: Frame::silence(shared.codec),
                });
                packet
            }
            None => {
                let frame = shared
                    .downlink
                    .lock()
                    .expect("lock")
                    .pop_front()
                    .inspect(|_| shared.downlink_space.add_permits(1))
                    .unwrap_or_else(|| Frame::silence(shared.codec));
                shared.record(Recorded {
                    direction: Direction::Downlink,
                    frame: frame.clone(),
                });
                shared.downlink_sent.fetch_add(1, Ordering::Relaxed);
                RtpPacket {
                    ssrc: shared.ssrc,
                    payload_type: shared.codec.payload_type(),
                    sequence,
                    timestamp,
                    marker: first,
                    payload: frame.payload().clone(),
                }
            }
        };
        first = false;
        sequence = sequence.wrapping_add(1);
        timestamp = timestamp.wrapping_add(FRAME_TIMESTAMP_STEP);
        shared.sink.send(packet).await;
    }
}

/// The next packet of a press, and whether it was the last one.
fn press_packet(shared: &Shared, press: &mut Press, sequence: u16) -> (RtpPacket, bool) {
    let payload_type = shared
        .dtmf_payload_type
        .expect("a press is queued only when the far side accepts events");
    let marker = press.frames_sent == 0 && press.ends_sent == 0;
    let end = press.frames_sent >= dtmf::PRESS_FRAMES;
    if end {
        press.ends_sent += 1;
    } else {
        press.frames_sent += 1;
    }
    let event = dtmf::Event {
        code: press.code,
        end,
        duration: press.frames_sent * FRAME_TIMESTAMP_STEP as u16,
    };
    let packet = RtpPacket {
        ssrc: shared.ssrc,
        payload_type,
        sequence,
        timestamp: press.timestamp,
        marker,
        payload: dtmf::encode(&event).to_vec().into(),
    };
    (packet, press.ends_sent >= dtmf::END_PACKETS)
}

/// A random start for the sequence and the timestamp, as RFC 3550 asks.
fn rand_start() -> u32 {
    rand::random::<u32>() & 0x7FFF_FFFF
}
