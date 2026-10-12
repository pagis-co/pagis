//! Listen-live (ADR-0020): the mono mix a person hears while the
//! call runs. The hub gives both directions as [`Recorded`] frames; the
//! mixer pairs them and answers one frame for each 20 ms downlink tick.
//!
//! The mix is linear: both audio frames decode to samples, the samples
//! sum with clipping, and the sum encodes back to the codec of the
//! call. A tick that carries only one direction passes that frame
//! through as it is, so nothing waits for audio that is not there.

use crate::audio::Frame;
use crate::hub::{Direction, Recorded};

/// Sum two frames into one, in the codec of `first`.
pub fn mix(first: &Frame, second: &Frame) -> Frame {
    let codec = first.codec();
    let samples = first
        .samples()
        .into_iter()
        .zip(second.samples())
        .map(|(a, b)| {
            let sum = i32::from(a) + i32::from(b);
            sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
        });
    Frame::from_samples(codec, samples)
}

/// One call's mix, frame by frame. The downlink is the clock, because
/// its pacer sends one frame every 20 ms for the full call.
#[derive(Default)]
pub struct Mixer {
    /// The uplink frame that waits for the next tick.
    pending: Option<Frame>,
}

impl Mixer {
    /// Take one frame from the hub and answer what the listener hears,
    /// or `None` while the frame waits for its tick.
    pub fn push(&mut self, recorded: Recorded) -> Option<Frame> {
        match recorded.direction {
            Direction::Downlink => Some(match self.pending.take() {
                Some(uplink) => mix(&recorded.frame, &uplink),
                None => recorded.frame,
            }),
            // The uplink runs ahead of the pacer: the frame that waited
            // goes out alone, so no audio is lost.
            Direction::Uplink => self.pending.replace(recorded.frame),
        }
    }
}
