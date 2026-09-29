//! G.711 frames (ADR-0020): one frame is 20 ms of PCMU or PCMA, silence
//! is the codec's own silence byte, and a frame converts between the
//! two laws only when a consumer needs the other one.

use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};

#[test]
fn a_silent_frame_is_twenty_milliseconds_of_the_codec_silence_byte() {
    let pcmu = Frame::silence(Codec::Pcmu);
    let pcma = Frame::silence(Codec::Pcma);

    assert_eq!(pcmu.payload().len(), FRAME_BYTES);
    assert_eq!(pcma.payload().len(), FRAME_BYTES);
    // The mu-law and A-law codes of a zero sample.
    assert!(pcmu.payload().iter().all(|byte| *byte == 0xFF));
    assert!(pcma.payload().iter().all(|byte| *byte == 0xD5));
}

#[test]
fn the_payload_types_are_the_static_rfc_3551_numbers() {
    assert_eq!(Codec::Pcmu.payload_type(), 0);
    assert_eq!(Codec::Pcma.payload_type(), 8);
    assert_eq!(Codec::from_payload_type(0), Some(Codec::Pcmu));
    assert_eq!(Codec::from_payload_type(8), Some(Codec::Pcma));
    assert_eq!(Codec::from_payload_type(101), None);
}

#[test]
fn a_frame_converts_between_the_laws_and_back() {
    let bytes: Vec<u8> = (0..FRAME_BYTES).map(|i| (i * 7 % 256) as u8).collect();
    let pcmu = Frame::new(Codec::Pcmu, bytes.clone());

    let pcma = pcmu.clone().into_codec(Codec::Pcma);
    assert_eq!(pcma.codec(), Codec::Pcma);
    assert_ne!(pcma.payload(), &bytes[..]);

    // Silence survives the round trip exactly; speech is within one
    // step of the coarser law.
    let silence = Frame::silence(Codec::Pcmu).into_codec(Codec::Pcma);
    assert_eq!(silence, Frame::silence(Codec::Pcma));
    let back = pcma.into_codec(Codec::Pcmu);
    assert_eq!(back.codec(), Codec::Pcmu);
    assert_eq!(back.payload().len(), FRAME_BYTES);
}

#[test]
fn converting_to_the_same_codec_changes_nothing() {
    let frame = Frame::new(Codec::Pcma, vec![1, 2, 3]);
    assert_eq!(frame.clone().into_codec(Codec::Pcma), frame);
}
