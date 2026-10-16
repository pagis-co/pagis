//! A carrier gives the Media Hub frames and typed keypad events.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};
use pagis_telephony::hub::{Direction, HubEvent, MediaHub, Recorded, RecordingSink};
use pagis_telephony::keypad::{self, Typed};
use pagis_telephony::leg::{EndedReason, FrameLeg, LegEvent};

struct Recording(Arc<Mutex<Vec<Recorded>>>);

#[async_trait]
impl RecordingSink for Recording {
    async fn write(&self, frame: Recorded) {
        self.0.lock().unwrap().push(frame);
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn carrier_frames_reach_the_hub_and_keypad_and_answer_frames_return() {
    pagis_rt::timeout(Duration::from_secs(3), async {
        let (leg, mut carrier) = FrameLeg::pair(Codec::Pcm16, 32);
        let hub = MediaHub::start(leg);
        let mut events = hub.subscribe();
        let mut keypad_events = hub.subscribe();
        let mut listener = hub.listen();
        let recording = Arc::new(Mutex::new(Vec::new()));
        let writer = hub.record(Box::new(Recording(Arc::clone(&recording))));
        assert!(hub.dtmf().receive_dtmf);
        assert!(!hub.dtmf().send_dtmf);

        carrier.inbound.send(LegEvent::Answered).await.unwrap();
        let audio = Frame::from_samples(Codec::Pcmu, vec![1234; FRAME_BYTES]);
        carrier
            .inbound
            .send(LegEvent::Frame(audio.clone()))
            .await
            .unwrap();
        for digit in "123456#".chars() {
            carrier.inbound.send(LegEvent::Dtmf(digit)).await.unwrap();
        }
        assert_eq!(events.recv().await.unwrap(), HubEvent::Answered);
        assert_eq!(
            events.recv().await.unwrap(),
            HubEvent::Uplink(audio.clone())
        );
        assert!(matches!(keypad::read_entry(&mut keypad_events).await,
            Typed::Entry(digits) if digits == "123456"));
        loop {
            let recorded = listener.recv().await.unwrap();
            if recorded.direction == Direction::Uplink {
                assert_eq!(recorded.frame, audio);
                break;
            }
        }

        let answer = Frame::from_samples(Codec::Pcma, vec![4321; FRAME_BYTES]);
        hub.send_downlink(answer.clone()).await;
        loop {
            let frame = carrier.outbound.recv().await.unwrap();
            if frame != Frame::silence(Codec::Pcm16) {
                assert_eq!(frame, answer.into_codec(Codec::Pcm16));
                break;
            }
        }
        hub.hangup().await;
        let mut ended = hub.watch_ended();
        ended.wait_for(|reason| reason.is_some()).await.unwrap();
        assert_eq!(hub.ended(), Some(EndedReason::LocalHangup));
        writer.await.unwrap();
        let recording = recording.lock().unwrap();
        let uplink: Vec<_> = recording
            .iter()
            .filter(|frame| frame.direction == Direction::Uplink)
            .collect();
        assert_eq!(uplink.len(), 1);
        assert_eq!(uplink[0].frame, audio);
        assert!(
            recording
                .iter()
                .any(|frame| frame.direction == Direction::Downlink)
        );
    })
    .await
    .expect("the frame Call completes");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_lost_frame_carrier_ends_the_call() {
    pagis_rt::timeout(Duration::from_secs(3), async {
        let (leg, carrier) = FrameLeg::pair(Codec::Pcm16, 1);
        let hub = MediaHub::start(leg);
        drop(carrier);
        let mut ended = hub.watch_ended();
        ended.wait_for(|reason| reason.is_some()).await.unwrap();
        assert_eq!(hub.ended(), Some(EndedReason::TransportLost));
    })
    .await
    .expect("the lost carrier ends the Call");
}
