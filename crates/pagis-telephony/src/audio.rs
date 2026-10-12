//! Audio frames at 8 kHz (ADR-0020). One frame is 20 ms: 160 samples.
//! G.711 uses one byte per sample. PCM16 uses two bytes per sample,
//! signed and little-endian. The SIP media path uses G.711.

use std::time::Duration;

use bytes::Bytes;

/// Samples per second. All frame codecs run at this rate.
pub const SAMPLE_RATE: u32 = 8000;
/// How long one frame plays: `ptime=20` in the offer.
pub const FRAME_DURATION: Duration = Duration::from_millis(20);
/// The samples in one frame, and the bytes in one G.711 frame.
pub const FRAME_BYTES: usize = 160;
/// How far the RTP timestamp moves per frame.
pub const FRAME_TIMESTAMP_STEP: u32 = FRAME_BYTES as u32;

/// The frame codecs. The SIP offer carries only the two G.711 laws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Pcmu,
    Pcma,
    Pcm16,
}

impl Codec {
    pub fn payload_type(self) -> u8 {
        match self {
            Codec::Pcmu => 0,
            Codec::Pcma => 8,
            Codec::Pcm16 => panic!("PCM16 frames have no RTP payload type"),
        }
    }

    pub fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Codec::Pcmu),
            8 => Some(Codec::Pcma),
            _ => None,
        }
    }

    /// The codec name. PCM16 is a frame codec, absent from the SDP offer.
    pub fn name(self) -> &'static str {
        match self {
            Codec::Pcmu => "PCMU",
            Codec::Pcma => "PCMA",
            Codec::Pcm16 => "PCM16",
        }
    }

    /// The code of a zero sample: what silence is on the wire.
    pub fn silence_byte(self) -> u8 {
        match self {
            Codec::Pcmu => audio_codec_algorithms::encode_ulaw(0),
            Codec::Pcma => audio_codec_algorithms::encode_alaw(0),
            Codec::Pcm16 => 0,
        }
    }
}

/// One frame of audio in one codec.
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
        let bytes = if codec == Codec::Pcm16 {
            FRAME_BYTES * 2
        } else {
            FRAME_BYTES
        };
        Self::new(codec, vec![codec.silence_byte(); bytes])
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    /// Decode the frame to signed samples at 8 kHz.
    pub fn samples(&self) -> Vec<i16> {
        match self.codec {
            Codec::Pcmu => self
                .payload
                .iter()
                .map(|byte| audio_codec_algorithms::decode_ulaw(*byte))
                .collect(),
            Codec::Pcma => self
                .payload
                .iter()
                .map(|byte| audio_codec_algorithms::decode_alaw(*byte))
                .collect(),
            Codec::Pcm16 => self
                .payload
                .as_chunks::<2>()
                .0
                .iter()
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
                .collect(),
        }
    }

    /// Encode signed samples in this codec, with no resampling.
    pub fn from_samples(codec: Codec, samples: impl IntoIterator<Item = i16>) -> Self {
        let payload = match codec {
            Codec::Pcmu => samples
                .into_iter()
                .map(audio_codec_algorithms::encode_ulaw)
                .collect::<Vec<_>>(),
            Codec::Pcma => samples
                .into_iter()
                .map(audio_codec_algorithms::encode_alaw)
                .collect::<Vec<_>>(),
            Codec::Pcm16 => samples
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect::<Vec<_>>(),
        };
        Self::new(codec, payload)
    }

    /// The same audio in the requested codec. All codecs use 8 kHz.
    pub fn into_codec(self, codec: Codec) -> Self {
        if self.codec == codec {
            return self;
        }
        let convert = match (self.codec, codec) {
            (Codec::Pcma, Codec::Pcmu) => audio_codec_algorithms::convert_alaw_to_ulaw,
            (Codec::Pcmu, Codec::Pcma) => audio_codec_algorithms::convert_ulaw_to_alaw,
            _ => return Self::from_samples(codec, self.samples()),
        };
        let payload: Vec<u8> = self.payload.iter().map(|byte| convert(*byte)).collect();
        Self::new(codec, payload)
    }
}
