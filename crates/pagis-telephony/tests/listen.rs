//! The listen-live mix (ADR-0020): both directions of a call as
//! one mono G.711 stream. The mix is linear, the sum clips, and a tick
//! that carries only one direction passes that frame through.

use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};
use pagis_telephony::hub::{Direction, Recorded};
use pagis_telephony::listen::{Mixer, mix};

fn tone(codec: Codec, sample: i16) -> Frame {
    let byte = match codec {
        Codec::Pcmu => audio_codec_algorithms::encode_ulaw(sample),
        Codec::Pcma => audio_codec_algorithms::encode_alaw(sample),
        Codec::Pcm16 => return Frame::from_samples(codec, vec![sample; FRAME_BYTES]),
    };
    Frame::new(codec, vec![byte; FRAME_BYTES])
}

fn first_sample(frame: &Frame) -> i16 {
    match frame.codec() {
        Codec::Pcmu => audio_codec_algorithms::decode_ulaw(frame.payload()[0]),
        Codec::Pcma => audio_codec_algorithms::decode_alaw(frame.payload()[0]),
        Codec::Pcm16 => i16::from_le_bytes([frame.payload()[0], frame.payload()[1]]),
    }
}

fn recorded(direction: Direction, frame: Frame) -> Recorded {
    Recorded { direction, frame }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_mix_sums_the_two_directions_in_the_linear_domain() {
    let mixed = mix(&tone(Codec::Pcmu, 4000), &tone(Codec::Pcmu, 2000));

    assert_eq!(mixed.codec(), Codec::Pcmu);
    assert_eq!(mixed.payload().len(), FRAME_BYTES);
    let sample = first_sample(&mixed);
    assert!((5500..=6500).contains(&sample), "the sum is {sample}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_mix_clips_instead_of_wrapping() {
    let mixed = mix(&tone(Codec::Pcmu, 30000), &tone(Codec::Pcmu, 30000));

    assert!(first_sample(&mixed) > 30000, "the sum wrapped");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_mix_takes_the_law_of_the_call() {
    let mixed = mix(&tone(Codec::Pcma, 1000), &tone(Codec::Pcmu, 1000));

    assert_eq!(mixed.codec(), Codec::Pcma);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn the_mixer_answers_one_frame_for_each_downlink_tick() {
    let mut mixer = Mixer::default();

    assert!(
        mixer
            .push(recorded(Direction::Uplink, tone(Codec::Pcmu, 4000)))
            .is_none(),
        "the uplink frame waits for its tick"
    );
    let frame = mixer
        .push(recorded(Direction::Downlink, tone(Codec::Pcmu, 2000)))
        .expect("the tick answers one frame");
    let sample = first_sample(&frame);
    assert!((5500..=6500).contains(&sample), "the sum is {sample}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn one_direction_alone_passes_through() {
    let mut mixer = Mixer::default();

    let downlink = mixer
        .push(recorded(Direction::Downlink, tone(Codec::Pcmu, 2000)))
        .expect("a tick with no uplink answers the downlink frame");
    assert_eq!(
        first_sample(&downlink),
        first_sample(&tone(Codec::Pcmu, 2000))
    );

    // The uplink runs ahead of the pacer: the older frame goes out as
    // it is, so no audio is lost.
    assert!(
        mixer
            .push(recorded(Direction::Uplink, tone(Codec::Pcmu, 1000)))
            .is_none()
    );
    let uplink = mixer
        .push(recorded(Direction::Uplink, tone(Codec::Pcmu, 3000)))
        .expect("the first uplink frame goes out alone");
    assert_eq!(
        first_sample(&uplink),
        first_sample(&tone(Codec::Pcmu, 1000))
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn pcm16_mix_sums_samples_with_a_g711_frame_and_clips() {
    let pcm = Frame::new(
        Codec::Pcm16,
        [1000i16, 32000]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let law = Frame::new(
        Codec::Pcmu,
        vec![audio_codec_algorithms::encode_ulaw(2000); 2],
    );
    let mixed = mix(&pcm, &law);
    assert_eq!(mixed.codec(), Codec::Pcm16);
    let samples = mixed
        .payload()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    assert_eq!(
        samples,
        vec![
            1000 + audio_codec_algorithms::decode_ulaw(law.payload()[0]),
            i16::MAX
        ]
    );
    let mixed = mix(&law, &pcm);
    assert_eq!(mixed.codec(), Codec::Pcmu);
    assert_eq!(mixed.payload().len(), 2);
}
