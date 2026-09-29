//! The uplink reorder window (ADR-0020): about 40 ms keyed on the
//! sequence number, and no playout buffer. A packet that arrives in
//! order leaves at once. A packet that arrives early waits for the gap
//! before it, and no longer than the window. A packet that arrives
//! after its place was passed is dropped.

use std::time::Duration;

use tokio::time::Instant;

use crate::leg::RtpPacket;

pub struct Reorder {
    window: Duration,
    /// The sequence number the uplink expects next.
    next: Option<u16>,
    /// Packets that wait for a gap, with when they arrived.
    pending: Vec<(Instant, RtpPacket)>,
}

impl Reorder {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            next: None,
            pending: Vec::new(),
        }
    }

    /// Take one packet in. Returns the packets the uplink may pass on
    /// now, in order.
    pub fn push(&mut self, now: Instant, packet: RtpPacket) -> Vec<RtpPacket> {
        let next = *self.next.get_or_insert(packet.sequence);
        if distance(next, packet.sequence) < 0 {
            tracing::trace!(sequence = packet.sequence, "dropped a late packet");
            return Vec::new();
        }
        if self
            .pending
            .iter()
            .any(|(_, pending)| pending.sequence == packet.sequence)
        {
            return Vec::new();
        }
        self.pending.push((now, packet));
        self.pending
            .sort_by_key(|(_, packet)| distance(next, packet.sequence));
        self.release_in_order()
    }

    /// When the oldest waiting packet has waited long enough, or `None`
    /// when nothing waits.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|(arrived, _)| *arrived + self.window)
            .min()
    }

    /// Give up on the gap before a packet that has waited the window.
    pub fn expire(&mut self, now: Instant) -> Vec<RtpPacket> {
        let mut released = Vec::new();
        while self
            .pending
            .first()
            .is_some_and(|(arrived, _)| *arrived + self.window <= now)
        {
            self.next = Some(self.pending[0].1.sequence);
            released.extend(self.release_in_order());
        }
        released
    }

    /// Everything that waits, in order: the call ended, and nothing
    /// more will fill a gap.
    pub fn flush(&mut self) -> Vec<RtpPacket> {
        let pending = std::mem::take(&mut self.pending);
        if let Some((_, last)) = pending.last() {
            self.next = Some(last.sequence.wrapping_add(1));
        }
        pending.into_iter().map(|(_, packet)| packet).collect()
    }

    fn release_in_order(&mut self) -> Vec<RtpPacket> {
        let mut released = Vec::new();
        while let Some(next) = self.next {
            if self
                .pending
                .first()
                .is_some_and(|(_, packet)| packet.sequence == next)
            {
                let (_, packet) = self.pending.remove(0);
                released.push(packet);
                self.next = Some(next.wrapping_add(1));
            } else {
                break;
            }
        }
        released
    }
}

/// How far `sequence` is past `from`, with the wrap of a 16-bit number.
fn distance(from: u16, sequence: u16) -> i16 {
    sequence.wrapping_sub(from) as i16
}
