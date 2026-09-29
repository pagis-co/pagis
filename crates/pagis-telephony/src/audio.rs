//! G.711 audio (ADR-0020). The media path is G.711 at 8 kHz from the
//! carrier to the model and back, so a frame is bytes that nothing
//! decodes. One frame is 20 ms: 160 samples, one byte each.

use std::time::Duration;

use bytes::Bytes;

/// Samples per second. Both laws run at this rate.
pub const SAMPLE_RATE: u32 = 8000;
/// How long one frame plays: `ptime=20` in the offer.
pub const FRAME_DURATION: Duration = Duration::from_millis(20);
/// The samples, and so the bytes, in one frame.
pub const FRAME_BYTES: usize = 160;
/// How far the RTP timestamp moves per frame.
pub const FRAME_TIMESTAMP_STEP: u32 = FRAME_BYTES as u32;

/// The two codecs the offer carries, by their RFC 3551 numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Pcmu,
    Pcma,
}

impl Codec {
    pub fn payload_type(self) -> u8 {
        match self {
            Codec::Pcmu => 0,
            Codec::Pcma => 8,
        }
    }

    pub fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Codec::Pcmu),
            8 => Some(Codec::Pcma),
            _ => None,
        }
    }

    /// The SDP encoding name.
    pub fn name(self) -> &'static str {
        match self {
            Codec::Pcmu => "PCMU",
            Codec::Pcma => "PCMA",
        }
    }

    /// The code of a zero sample: what silence is on the wire.
    pub fn silence_byte(self) -> u8 {
        match self {
            Codec::Pcmu => audio_codec_algorithms::encode_ulaw(0),
            Codec::Pcma => audio_codec_algorithms::encode_alaw(0),
        }
    }
}

/// One packet's worth of G.711 audio in one codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    codec: Codec,
    payload: Bytes,
}

impl Frame {
    pub fn new(codec: Codec, payload: impl Into<Bytes>) -> Self {
        Self {
            codec,
            payload: payload.into(),
        }
    }

    /// 20 ms of silence in this codec.
    pub fn silence(codec: Codec) -> Self {
        Self::new(codec, vec![codec.silence_byte(); FRAME_BYTES])
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    /// The same audio in the other law, or this frame unchanged when it
    /// is in that law already. Nothing resamples: both laws are one byte
    /// per sample at the same rate.
    pub fn into_codec(self, codec: Codec) -> Self {
        if self.codec == codec {
            return self;
        }
        let convert = match codec {
            Codec::Pcmu => audio_codec_algorithms::convert_alaw_to_ulaw,
            Codec::Pcma => audio_codec_algorithms::convert_ulaw_to_alaw,
        };
        let payload: Vec<u8> = self.payload.iter().map(|byte| convert(*byte)).collect();
        Self::new(codec, payload)
    }
}
