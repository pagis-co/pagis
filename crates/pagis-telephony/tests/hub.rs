//! The media hub (ADR-0020) on a paused clock: it owns both
//! directions of one call. The uplink reorders by sequence number in a
//! 40 ms window and drops late packets; the downlink paces one packet
//! every 20 ms and never runs deeper than 60 ms; subscribers are lossy;
//! the recorder has a bounded queue; typed events keep their order with
//! the audio.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_telephony::audio::{Codec, FRAME_BYTES, FRAME_TIMESTAMP_STEP, Frame};
use pagis_telephony::dtmf;
use pagis_telephony::dtmf::{END_PACKETS, PRESS_FRAMES};
use pagis_telephony::hub::{
    DOWNLINK_DEPTH, Direction, DtmfAbsent, DtmfPresence, HubEvent, INTER_DIGIT_FRAMES,
    MEDIA_TIMEOUT, MediaHub, REORDER_WINDOW, Recorded, RecordingSink,
};
use pagis_telephony::leg::{EndedReason, LegEvent, MediaLeg, PacketSink, RtpPacket};
use tokio::sync::{broadcast, mpsc};

const PT_DTMF: u8 = 101;
const SSRC: u32 = 0x1234;

/// The far side of a leg, as a test holds it: what the hub sent, and a
/// way to feed it.
struct FarSide {
    sent: Mutex<Vec<RtpPacket>>,
    hangups: Mutex<u32>,
    /// `None` once the far side went away.
    feed: Mutex<Option<mpsc::Sender<LegEvent>>>,
}

#[async_trait]
impl PacketSink for FarSide {
    async fn send(&self, packet: RtpPacket) {
        self.sent.lock().unwrap().push(packet);
    }

    async fn hangup(&self) {
        *self.hangups.lock().unwrap() += 1;
        let feed = self.feed.lock().unwrap().clone();
        if let Some(feed) = feed {
            let _ = feed.send(LegEvent::Ended(EndedReason::LocalHangup)).await;
        }
    }
}

impl FarSide {
    fn sent(&self) -> Vec<RtpPacket> {
        self.sent.lock().unwrap().clone()
    }

    fn audio_sent(&self) -> Vec<RtpPacket> {
        self.sent()
            .into_iter()
            .filter(|packet| packet.payload_type != PT_DTMF)
            .collect()
    }

    async fn feed(&self, event: LegEvent) {
        let feed = self
            .feed
            .lock()
            .unwrap()
            .clone()
            .expect("the far side is gone");
        feed.send(event).await.unwrap();
    }

    /// The far side goes away without a word: no `BYE`, no packets.
    fn disconnect(&self) {
        self.feed.lock().unwrap().take();
    }

    async fn packet(&self, sequence: u16, payload: u8) {
        self.feed(LegEvent::Rtp(RtpPacket {
            ssrc: 0xABCD,
            payload_type: Codec::Pcmu.payload_type(),
            sequence,
            timestamp: u32::from(sequence) * 160,
            marker: false,
            payload: vec![payload; FRAME_BYTES].into(),
        }))
        .await;
    }
}

fn start() -> (Arc<MediaHub>, Arc<FarSide>) {
    start_with_dtmf(Some(PT_DTMF))
}

/// A hub whose leg negotiated the given telephone-event payload type,
/// or none at all.
fn start_with_dtmf(dtmf_payload_type: Option<u8>) -> (Arc<MediaHub>, Arc<FarSide>) {
    let (feed, inbound) = mpsc::channel(64);
    let far = Arc::new(FarSide {
        sent: Mutex::new(Vec::new()),
        hangups: Mutex::new(0),
        feed: Mutex::new(Some(feed)),
    });
    let leg = MediaLeg {
        codec: Codec::Pcmu,
        dtmf_payload_type,
        ssrc: SSRC,
        inbound,
        sink: Arc::clone(&far) as _,
    };
    (MediaHub::start(leg), far)
}

/// Drain what a subscriber has now, without waiting.
fn drain(rx: &mut broadcast::Receiver<HubEvent>) -> Vec<HubEvent> {
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(event) => events.push(event),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => return events,
        }
    }
}

fn uplink_bytes(events: &[HubEvent]) -> Vec<u8> {
    events
        .iter()
        .filter_map(|event| match event {
            HubEvent::Uplink(frame) => Some(frame.payload()[0]),
            _ => None,
        })
        .collect()
}

/// Let the hub's tasks run without moving the clock.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn uplink_packets_reach_a_subscriber_in_sequence_order() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    far.packet(1, 1).await;
    far.packet(3, 3).await;
    far.packet(2, 2).await;
    far.packet(4, 4).await;
    settle().await;

    let events = drain(&mut rx);
    assert!(matches!(events[0], HubEvent::Answered));
    assert_eq!(uplink_bytes(&events), vec![1, 2, 3, 4]);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_gap_holds_the_uplink_for_the_window_and_no_longer() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    far.packet(1, 1).await;
    far.packet(3, 3).await;
    settle().await;
    assert_eq!(uplink_bytes(&drain(&mut rx)), vec![1]);

    tokio::time::sleep(REORDER_WINDOW - Duration::from_millis(1)).await;
    assert_eq!(uplink_bytes(&drain(&mut rx)), Vec::<u8>::new());
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert_eq!(uplink_bytes(&drain(&mut rx)), vec![3]);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_late_packet_is_dropped_and_there_is_no_playout_buffer() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    far.packet(1, 1).await;
    far.packet(2, 2).await;
    settle().await;
    // Packet 2 was released the moment it arrived: nothing waited.
    assert_eq!(uplink_bytes(&drain(&mut rx)), vec![1, 2]);

    far.packet(1, 9).await;
    far.packet(3, 3).await;
    settle().await;
    assert_eq!(uplink_bytes(&drain(&mut rx)), vec![3]);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn the_sequence_number_wraps() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    far.packet(65534, 1).await;
    far.packet(65535, 2).await;
    far.packet(0, 3).await;
    far.packet(1, 4).await;
    settle().await;

    assert_eq!(uplink_bytes(&drain(&mut rx)), vec![1, 2, 3, 4]);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn the_downlink_sends_silence_every_twenty_milliseconds_from_the_start() {
    let (hub, far) = start();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let sent = far.audio_sent();
    assert!(
        (9..=11).contains(&sent.len()),
        "expected about 10 packets, got {}",
        sent.len()
    );
    assert!(sent.iter().all(|packet| packet.ssrc == SSRC));
    assert!(sent.iter().all(|packet| packet.payload_type == 0));
    assert!(
        sent.iter()
            .all(|packet| packet.payload[..] == Frame::silence(Codec::Pcmu).payload()[..])
    );
    assert!(sent[0].marker);
    for pair in sent.windows(2) {
        assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1));
        assert_eq!(pair[1].timestamp, pair[0].timestamp.wrapping_add(160));
        assert!(!pair[1].marker);
    }
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn the_downlink_paces_queued_frames_and_holds_at_most_sixty_milliseconds() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;
    settle().await;

    for byte in 1..=DOWNLINK_DEPTH as u8 {
        hub.send_downlink(Frame::new(Codec::Pcmu, vec![byte; FRAME_BYTES]))
            .await;
    }
    // The queue is full: one more waits until the pacer takes one.
    let overflow = tokio::time::timeout(
        Duration::from_millis(5),
        hub.send_downlink(Frame::new(Codec::Pcmu, vec![9; FRAME_BYTES])),
    )
    .await;
    assert!(overflow.is_err(), "the fourth frame did not wait");

    tokio::time::sleep(Duration::from_millis(100)).await;
    hub.send_downlink(Frame::new(Codec::Pcmu, vec![4; FRAME_BYTES]))
        .await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    let bytes: Vec<u8> = far
        .audio_sent()
        .iter()
        .map(|packet| packet.payload[0])
        .filter(|byte| *byte != 0xFF)
        .collect();
    assert_eq!(bytes, vec![1, 2, 3, 4]);
    // One tick, one packet: the queued frames went out 20 ms apart.
    let stamps: Vec<u32> = far
        .audio_sent()
        .iter()
        .filter(|packet| packet.payload[0] != 0xFF)
        .map(|packet| packet.timestamp)
        .collect();
    assert_eq!(stamps[1] - stamps[0], 160);
    assert_eq!(stamps[2] - stamps[1], 160);
    // The model heard nothing of its own voice.
    assert!(uplink_bytes(&drain(&mut rx)).is_empty());
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_downlink_frame_in_the_other_law_is_converted_to_the_call_codec() {
    let (hub, far) = start();
    let frame = Frame::new(Codec::Pcma, vec![0x55; FRAME_BYTES]);
    hub.send_downlink(frame.clone()).await;
    tokio::time::sleep(Duration::from_millis(30)).await;

    let converted = frame.into_codec(Codec::Pcmu);
    let sent = far.audio_sent();
    assert!(sent.iter().all(|packet| packet.payload_type == 0));
    assert!(
        sent.iter()
            .any(|packet| packet.payload[..] == converted.payload()[..]),
        "no packet carried the converted frame"
    );
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn clearing_the_downlink_drops_what_was_not_sent_and_reports_what_was() {
    let (hub, far) = start();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let sent_before = far.audio_sent().len() as u64;
    for byte in 1..=3 {
        hub.send_downlink(Frame::new(Codec::Pcmu, vec![byte; FRAME_BYTES]))
            .await;
    }

    let dropped = hub.clear_downlink();

    assert_eq!(dropped, 3);
    assert_eq!(hub.downlink_packets_sent(), sent_before);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        far.audio_sent()
            .iter()
            .all(|packet| packet.payload[0] == 0xFF)
    );
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_slow_subscriber_loses_frames_and_the_downlink_keeps_its_pace() {
    let (hub, far) = start();
    let mut slow = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    for sequence in 1..=600u16 {
        far.packet(sequence, 1).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The slow subscriber never read: it lagged, and the newest frames
    // are what it can still read.
    assert!(matches!(
        slow.try_recv(),
        Err(broadcast::error::TryRecvError::Lagged(_))
    ));
    let sent = far.audio_sent();
    assert!(
        sent.len() >= 590,
        "downlink stalled at {} packets",
        sent.len()
    );
    hub.hangup().await;
}

struct MemorySink {
    frames: Arc<Mutex<Vec<Recorded>>>,
    hold: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl RecordingSink for MemorySink {
    async fn write(&self, recorded: Recorded) {
        let _permit = self.hold.acquire().await.unwrap();
        self.frames.lock().unwrap().push(recorded);
    }
}

#[tokio::test(start_paused = true)]
async fn the_recorder_gets_both_directions_through_its_own_writer_task() {
    let (hub, far) = start();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let hold = Arc::new(tokio::sync::Semaphore::new(1));
    hub.record(Box::new(MemorySink {
        frames: Arc::clone(&frames),
        hold: Arc::clone(&hold),
    }));
    far.feed(LegEvent::Answered).await;

    far.packet(1, 7).await;
    hub.send_downlink(Frame::new(Codec::Pcmu, vec![8; FRAME_BYTES]))
        .await;
    tokio::time::sleep(Duration::from_millis(25)).await;

    let recorded = frames.lock().unwrap().clone();
    assert!(
        recorded
            .iter()
            .any(|item| { item.direction == Direction::Uplink && item.frame.payload()[0] == 7 })
    );
    assert!(
        recorded
            .iter()
            .any(|item| { item.direction == Direction::Downlink && item.frame.payload()[0] == 8 })
    );

    // A writer that blocks does not stall the RTP path: the queue fills,
    // frames are dropped, and the pacer keeps sending.
    let permit = hold.acquire().await.unwrap();
    let before = far.audio_sent().len();
    for sequence in 2..=40u16 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        far.packet(sequence, 1).await;
    }
    assert!(far.audio_sent().len() - before >= 970);
    assert!(hub.recorder_dropped() > 0);
    assert_eq!(hub.ended(), None);
    drop(permit);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn events_keep_their_order_with_the_audio() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();

    far.feed(LegEvent::Ringing).await;
    far.feed(LegEvent::Answered).await;
    far.packet(1, 1).await;
    far.feed(LegEvent::Ended(EndedReason::RemoteHangup)).await;
    settle().await;

    let events = drain(&mut rx);
    assert!(matches!(events[0], HubEvent::Ringing));
    assert!(matches!(events[1], HubEvent::Answered));
    assert!(matches!(&events[2], HubEvent::Uplink(frame) if frame.payload()[0] == 1));
    assert!(matches!(
        events[3],
        HubEvent::Ended(EndedReason::RemoteHangup)
    ));
    assert_eq!(hub.ended(), Some(EndedReason::RemoteHangup));
}

#[tokio::test(start_paused = true)]
async fn the_end_flushes_what_the_reorder_window_holds() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    far.packet(1, 1).await;
    far.packet(3, 3).await;
    far.feed(LegEvent::Ended(EndedReason::RemoteHangup)).await;
    settle().await;

    let events = drain(&mut rx);
    assert_eq!(uplink_bytes(&events), vec![1, 3]);
    assert!(matches!(events.last(), Some(HubEvent::Ended(_))));
}

#[tokio::test(start_paused = true)]
async fn after_the_end_the_downlink_stops() {
    let (hub, far) = start();
    far.feed(LegEvent::Ended(EndedReason::RemoteHangup)).await;
    settle().await;
    let sent = far.audio_sent().len();

    tokio::time::sleep(Duration::from_secs(1)).await;

    assert_eq!(far.audio_sent().len(), sent);
    assert_eq!(hub.ended(), Some(EndedReason::RemoteHangup));
}

#[tokio::test(start_paused = true)]
async fn a_hangup_from_the_hub_reaches_the_leg_and_ends_the_call() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    hub.hangup().await;
    settle().await;

    assert_eq!(*far.hangups.lock().unwrap(), 1);
    assert_eq!(hub.ended(), Some(EndedReason::LocalHangup));
    assert!(matches!(
        drain(&mut rx).last(),
        Some(HubEvent::Ended(EndedReason::LocalHangup))
    ));
}

#[tokio::test(start_paused = true)]
async fn a_keypad_press_arrives_once_as_a_typed_event() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    // Three start packets, then three end packets, as RFC 4733 sends
    // them. The digit is reported once, at the first end packet.
    let start_timestamp = 8000;
    let packets = [
        (false, 160),
        (false, 320),
        (false, 480),
        (true, 640),
        (true, 640),
        (true, 640),
    ];
    for (index, (end, duration)) in packets.into_iter().enumerate() {
        far.feed(LegEvent::Rtp(RtpPacket {
            ssrc: 0xABCD,
            payload_type: PT_DTMF,
            sequence: 10 + index as u16,
            timestamp: start_timestamp,
            marker: index == 0,
            payload: dtmf::encode(&dtmf::Event {
                code: 5,
                end,
                duration,
            })
            .to_vec()
            .into(),
        }))
        .await;
    }
    settle().await;

    let digits: Vec<char> = drain(&mut rx)
        .into_iter()
        .filter_map(|event| match event {
            HubEvent::Dtmf(digit) => Some(digit),
            _ => None,
        })
        .collect();
    assert_eq!(digits, vec!['5']);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn sending_a_digit_writes_the_event_packets_in_the_audio_timeline() {
    let (hub, far) = start();
    far.feed(LegEvent::Answered).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    hub.send_dtmf('#').await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let sent = far.sent();
    let events: Vec<(usize, dtmf::Event)> = sent
        .iter()
        .enumerate()
        .filter(|(_, packet)| packet.payload_type == PT_DTMF)
        .map(|(index, packet)| (index, dtmf::decode(&packet.payload).unwrap()))
        .collect();
    assert!(!events.is_empty());
    assert!(events.iter().all(|(_, event)| event.code == 11));
    let ends = events.iter().filter(|(_, event)| event.end).count();
    assert_eq!(ends, 3);
    let (first, first_event) = &events[0];
    assert!(sent[*first].marker);
    assert_eq!(first_event.duration, 160);
    // The event packets share one timestamp and keep the sequence
    // numbers of the audio they replace.
    assert!(
        events
            .iter()
            .all(|(index, _)| sent[*index].timestamp == sent[*first].timestamp)
    );
    for pair in sent.windows(2) {
        assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1));
    }
    // Audio resumes after the event, with the timeline moved on.
    let last = events.last().unwrap().0;
    assert!(sent.len() > last + 1);
    assert_eq!(sent[last + 1].payload_type, 0);
    assert!(sent[last + 1].timestamp > sent[*first].timestamp);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn ten_seconds_without_inbound_rtp_ends_the_call_with_media_timeout() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;
    far.packet(1, 1).await;

    tokio::time::sleep(MEDIA_TIMEOUT - Duration::from_millis(100)).await;
    far.packet(2, 2).await;
    tokio::time::sleep(MEDIA_TIMEOUT - Duration::from_millis(100)).await;
    assert_eq!(hub.ended(), None);

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(hub.ended(), Some(EndedReason::MediaTimeout));
    assert_eq!(*far.hangups.lock().unwrap(), 1);
    assert!(matches!(
        drain(&mut rx).last(),
        Some(HubEvent::Ended(EndedReason::MediaTimeout))
    ));
}

#[tokio::test(start_paused = true)]
async fn the_media_timeout_does_not_run_before_the_answer() {
    let (hub, far) = start();
    far.feed(LegEvent::Ringing).await;

    tokio::time::sleep(MEDIA_TIMEOUT * 3).await;

    assert_eq!(hub.ended(), None);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_leg_that_goes_away_ends_the_call_with_transport_lost() {
    let (hub, far) = start();
    far.feed(LegEvent::Answered).await;

    far.disconnect();
    settle().await;

    assert_eq!(hub.ended(), Some(EndedReason::TransportLost));
}

#[tokio::test(start_paused = true)]
async fn a_listener_hears_both_directions() {
    let (hub, far) = start();
    let mut rx = hub.listen();
    far.feed(LegEvent::Answered).await;
    hub.send_downlink(Frame::new(Codec::Pcmu, vec![7u8; FRAME_BYTES]))
        .await;
    far.packet(1, 5).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    settle().await;

    let heard = drain_recorded(&mut rx);
    assert!(
        heard
            .iter()
            .any(|r| r.direction == Direction::Uplink && r.frame.payload()[0] == 5),
        "the uplink frame is absent: {heard:?}"
    );
    assert!(
        heard
            .iter()
            .any(|r| r.direction == Direction::Downlink && r.frame.payload()[0] == 7),
        "the downlink frame is absent: {heard:?}"
    );
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_listener_that_never_reads_does_not_stop_the_hub() {
    let (hub, far) = start();
    // The listener never reads: its channel fills and stays full.
    let _slow = hub.listen();
    let frames = Arc::new(Mutex::new(Vec::new()));
    hub.record(Box::new(MemorySink {
        frames: Arc::clone(&frames),
        hold: Arc::new(tokio::sync::Semaphore::new(1)),
    }));
    far.feed(LegEvent::Answered).await;

    tokio::time::sleep(Duration::from_secs(4)).await;
    settle().await;

    // The downlink pacer and the recorder both went on.
    assert!(far.audio_sent().len() > 100, "the downlink stopped");
    assert!(frames.lock().unwrap().len() > 100, "the recorder stopped");
    hub.hangup().await;
}

/// Drain what a listener has now, without waiting.
fn drain_recorded(rx: &mut broadcast::Receiver<Recorded>) -> Vec<Recorded> {
    let mut heard = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(recorded) => heard.push(recorded),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => return heard,
        }
    }
}
/// One telephone-event packet from the far side.
fn event_packet(sequence: u16, timestamp: u32, code: u8, end: bool, duration: u16) -> RtpPacket {
    RtpPacket {
        ssrc: 0xABCD,
        payload_type: PT_DTMF,
        sequence,
        timestamp,
        marker: !end && duration == FRAME_TIMESTAMP_STEP as u16,
        payload: dtmf::encode(&dtmf::Event {
            code,
            end,
            duration,
        })
        .to_vec()
        .into(),
    }
}

/// The digits of a subscriber's events, in the order they arrived.
fn digits(events: &[HubEvent]) -> Vec<char> {
    events
        .iter()
        .filter_map(|event| match event {
            HubEvent::Dtmf(digit) => Some(*digit),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_press_between_two_audio_frames_arrives_between_them() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    far.packet(1, 1).await;
    for (index, (end, duration)) in [(false, 160), (true, 160), (true, 160)]
        .into_iter()
        .enumerate()
    {
        far.feed(LegEvent::Rtp(event_packet(
            2 + index as u16,
            8000,
            7,
            end,
            duration,
        )))
        .await;
    }
    far.packet(5, 2).await;
    settle().await;

    let events = drain(&mut rx);
    let order: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            HubEvent::Uplink(frame) => Some(format!("audio {}", frame.payload()[0])),
            HubEvent::Dtmf(digit) => Some(format!("digit {digit}")),
            _ => None,
        })
        .collect();
    assert_eq!(order, vec!["audio 1", "digit 7", "audio 2"]);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_press_whose_start_packets_were_lost_still_arrives_once() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    // Only the three retransmitted end packets reach the daemon.
    for index in 0..3 {
        far.feed(LegEvent::Rtp(event_packet(1 + index, 8000, 3, true, 640)))
            .await;
    }
    settle().await;

    assert_eq!(digits(&drain(&mut rx)), vec!['3']);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn two_presses_of_one_digit_are_two_events_by_their_timestamps() {
    let (hub, far) = start();
    let mut rx = hub.subscribe();
    far.feed(LegEvent::Answered).await;

    let mut sequence = 1;
    for timestamp in [8000, 9600] {
        for (end, duration) in [(false, 160), (false, 320), (true, 320), (true, 320)] {
            far.feed(LegEvent::Rtp(event_packet(
                sequence, timestamp, 4, end, duration,
            )))
            .await;
            sequence += 1;
        }
    }
    settle().await;

    assert_eq!(digits(&drain(&mut rx)), vec!['4', '4']);
    hub.hangup().await;
}

/// The presses of the sent packets, each as its digit and the index of
/// its first packet.
fn presses_sent(far: &FarSide) -> Vec<(usize, char)> {
    let mut presses = Vec::new();
    let mut open: Option<u32> = None;
    for (index, packet) in far.sent().iter().enumerate() {
        if packet.payload_type != PT_DTMF {
            continue;
        }
        if open == Some(packet.timestamp) {
            continue;
        }
        open = Some(packet.timestamp);
        let event = dtmf::decode(&packet.payload).unwrap();
        presses.push((index, dtmf::digit_from_code(event.code).unwrap()));
    }
    presses
}

#[tokio::test(start_paused = true)]
async fn send_digits_presses_every_digit_in_order_with_a_gap_between_them() {
    let (hub, far) = start();
    far.feed(LegEvent::Answered).await;
    hub.send_digits("12#").await.unwrap();

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let presses = presses_sent(&far);
    let digits: Vec<char> = presses.iter().map(|(_, digit)| *digit).collect();
    assert_eq!(digits, vec!['1', '2', '#']);
    // Every press is the full burst, and audio fills the gap between
    // two presses.
    let sent = far.sent();
    for pair in presses.windows(2) {
        let (first, _) = pair[0];
        let (second, _) = pair[1];
        let burst = (first..second)
            .filter(|index| sent[*index].payload_type == PT_DTMF)
            .count();
        assert_eq!(burst as u16, PRESS_FRAMES + END_PACKETS as u16);
        assert_eq!(second - first - burst, INTER_DIGIT_FRAMES as usize);
    }
    for pair in sent.windows(2) {
        assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1));
    }
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn every_end_packet_of_a_press_repeats_the_final_duration() {
    let (hub, far) = start();
    far.feed(LegEvent::Answered).await;
    hub.send_dtmf('5').await;

    tokio::time::sleep(Duration::from_millis(400)).await;

    let events: Vec<dtmf::Event> = far
        .sent()
        .iter()
        .filter(|packet| packet.payload_type == PT_DTMF)
        .map(|packet| dtmf::decode(&packet.payload).unwrap())
        .collect();
    let starts: Vec<u16> = events
        .iter()
        .filter(|event| !event.end)
        .map(|event| event.duration)
        .collect();
    assert_eq!(starts, vec![160, 320, 480, 640, 800]);
    let ends: Vec<u16> = events
        .iter()
        .filter(|event| event.end)
        .map(|event| event.duration)
        .collect();
    assert_eq!(ends, vec![800; END_PACKETS]);
    hub.hangup().await;
}

#[tokio::test(start_paused = true)]
async fn a_leg_with_no_telephone_event_declares_both_directions_absent() {
    let (hub, far) = start_with_dtmf(None);
    far.feed(LegEvent::Answered).await;

    assert_eq!(
        hub.dtmf(),
        DtmfPresence {
            send_dtmf: false,
            receive_dtmf: false
        }
    );
    assert_eq!(hub.send_digits("1").await, Err(DtmfAbsent));

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Nothing was emulated: only audio went out.
    assert!(far.sent().iter().all(|packet| packet.payload_type == 0));
    hub.hangup().await;
}
