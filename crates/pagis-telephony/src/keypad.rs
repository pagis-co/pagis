//! The keypad challenge (ADR-0021): the digits a caller enters
//! to prove a tier on a call Pagis answers.
//!
//! The challenge runs before the realtime session exists. The daemon
//! plays one fixed prompt and collects the typed keypad events of the
//! media hub, so the digits never reach the model uplink and they are
//! not in the decoded audio the recorder writes.
//!
//! The prompt is a short tone the daemon generates, and not a stored
//! clip: a clip is a file to ship, to localize and to keep, and the
//! caller who knows the code needs one signal and nothing more.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{Clock, KeypadFailureStore, WorkspaceId};
use pagis_vault::keypad::{MAX_DIGITS, MIN_DIGITS};
use tokio::sync::broadcast::error::RecvError;

use crate::audio::{Codec, FRAME_BYTES, Frame, SAMPLE_RATE};
use crate::hub::{HubEvent, MediaHub};

/// How many attempts one call allows. After the last one the call is
/// pinned to Unknown for its life. It keeps one call short. The limit
/// across calls is the failed-attempt count of the Workspace, which
/// [`Keypad::failures`] holds.
pub const MAX_ATTEMPTS: usize = 3;
/// How long one entry waits for the next digit. It debounces the
/// caller, who is holding a telephone and not a keyboard.
pub const DIGIT_GAP: Duration = Duration::from_secs(4);
/// How long the whole challenge waits. A caller with no keypad, and a
/// caller who says nothing, reaches Unknown after this.
pub const CHALLENGE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the prompt tone plays.
pub const PROMPT_DURATION: Duration = Duration::from_millis(400);
/// The pitch of the prompt tone, in hertz.
pub const PROMPT_HERTZ: f32 = 440.0;

/// Whether some digits are one Workspace's Keypad Code. The call's
/// Workspace names the code, so one person's code proves nothing on
/// another person's line. The vault implements it; telephony never
/// holds the code or its hash.
#[async_trait]
pub trait CodeCheck: Send + Sync {
    /// False when the Workspace set no code. A Workspace with no code
    /// never challenges, because no caller can prove anything.
    async fn is_set(&self, workspace_id: &WorkspaceId) -> bool;
    async fn verify(&self, workspace_id: &WorkspaceId, digits: &str) -> bool;
}

/// The check of an installation where no Workspace set a code.
pub struct NoCode;

#[async_trait]
impl CodeCheck for NoCode {
    async fn is_set(&self, _workspace_id: &WorkspaceId) -> bool {
        false
    }

    async fn verify(&self, _workspace_id: &WorkspaceId, _digits: &str) -> bool {
        false
    }
}

/// What the tier gate of an inbound call proves a tier with
/// (ADR-0021): the Keypad Code of each Workspace, the failed-attempt
/// count of each Workspace, and the clock that says whether a delay
/// runs.
#[derive(Clone)]
pub struct Keypad {
    pub code: Arc<dyn CodeCheck>,
    pub failures: Arc<dyn KeypadFailureStore>,
    pub clock: Arc<dyn Clock>,
}

/// One entry as the caller types it. `#` ends the entry, `*` clears it,
/// and an entry ends of its own at [`MAX_DIGITS`], the longest code the
/// vault accepts.
#[derive(Debug, Default)]
pub struct Entry {
    digits: String,
}

impl Entry {
    /// Take one press. `Some` is a finished entry, ready to check.
    pub fn press(&mut self, digit: char) -> Option<String> {
        match digit {
            '*' => {
                self.digits.clear();
                None
            }
            '#' => Some(self.take()),
            '0'..='9' => {
                self.digits.push(digit);
                match self.digits.len() >= MAX_DIGITS {
                    true => Some(self.take()),
                    false => None,
                }
            }
            _ => None,
        }
    }

    /// End the entry because the caller stopped typing. `None` when
    /// nothing was typed, because silence is not a wrong code.
    pub fn timed_out(&mut self) -> Option<String> {
        match self.digits.len() >= MIN_DIGITS {
            true => Some(self.take()),
            false => {
                self.digits.clear();
                None
            }
        }
    }

    fn take(&mut self) -> String {
        std::mem::take(&mut self.digits)
    }
}

/// Play the prompt: one short tone, in the codec of the call.
pub async fn play_prompt(hub: &MediaHub) {
    for frame in prompt_frames(hub.codec()) {
        hub.send_downlink(frame).await;
    }
}

/// The prompt tone as frames of G.711. It is a plain sine burst: the
/// caller needs one signal that the line waits for digits.
pub fn prompt_frames(codec: Codec) -> Vec<Frame> {
    let samples = (PROMPT_DURATION.as_millis() as usize * SAMPLE_RATE as usize) / 1000;
    let step = std::f32::consts::TAU * PROMPT_HERTZ / SAMPLE_RATE as f32;
    (0..samples / FRAME_BYTES)
        .map(|index| {
            let payload: Vec<u8> = (0..FRAME_BYTES)
                .map(|offset| {
                    let sample = ((index * FRAME_BYTES + offset) as f32 * step).sin() * 8000.0;
                    encode(codec, sample as i16)
                })
                .collect();
            Frame::new(codec, payload)
        })
        .collect()
}

fn encode(codec: Codec, sample: i16) -> u8 {
    match codec {
        Codec::Pcmu => audio_codec_algorithms::encode_ulaw(sample),
        Codec::Pcma => audio_codec_algorithms::encode_alaw(sample),
    }
}

/// What the caller typed, or why the entry ended without digits.
pub enum Typed {
    Entry(String),
    /// The caller stopped typing, or the call ended.
    Silence,
    Ended,
}

/// Read one entry from the hub's typed keypad events. It returns at the
/// first finished entry, at the digit gap, or when the call ends.
pub async fn read_entry(events: &mut tokio::sync::broadcast::Receiver<HubEvent>) -> Typed {
    let mut entry = Entry::default();
    loop {
        match tokio::time::timeout(DIGIT_GAP, events.recv()).await {
            Err(_) => {
                return match entry.timed_out() {
                    Some(digits) => Typed::Entry(digits),
                    None => Typed::Silence,
                };
            }
            Ok(Ok(HubEvent::Dtmf(digit))) => {
                if let Some(digits) = entry.press(digit) {
                    return Typed::Entry(digits);
                }
            }
            Ok(Ok(HubEvent::Ended(_))) | Ok(Err(RecvError::Closed)) => return Typed::Ended,
            Ok(Ok(_)) | Ok(Err(RecvError::Lagged(_))) => {}
        }
    }
}
