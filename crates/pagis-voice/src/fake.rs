//! A scripted voice provider for tests (ADR-0020). It lives beside the
//! seam, so a test of dictation and speaking reaches no provider.

use std::sync::Mutex;

use bytes::Bytes;
use futures::StreamExt;
use futures::channel::mpsc;

use crate::{
    Clip, DictationInput, DictationSession, Speech, Transcript, VoiceError, VoiceProvider,
};

/// What the fake was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceCall {
    /// A buffered transcription of this many PCM16 bytes.
    Transcribe {
        bytes: usize,
    },
    /// A live session was opened.
    Dictate,
    Speak {
        text: String,
        voice: Option<String>,
    },
}

/// A provider that hears one scripted transcript and speaks bytes that
/// name the voice and the text. Wrap it in an `Arc` and script it from
/// the test.
pub struct FakeVoice {
    transcript: Mutex<String>,
    live: Mutex<bool>,
    calls: Mutex<Vec<VoiceCall>>,
    fail_with: Mutex<Option<String>>,
}

impl Default for FakeVoice {
    fn default() -> Self {
        Self::hearing("hello from the fake")
    }
}

impl FakeVoice {
    /// A provider with no realtime socket that hears `transcript` in
    /// every clip.
    pub fn hearing(transcript: &str) -> Self {
        Self {
            transcript: Mutex::new(transcript.to_string()),
            live: Mutex::new(false),
            calls: Mutex::new(Vec::new()),
            fail_with: Mutex::new(None),
        }
    }

    /// Give the provider a realtime socket: [`VoiceProvider::dictate`]
    /// opens a session that emits one word of the transcript for every
    /// appended frame and the whole transcript on commit.
    pub fn with_live_transcription(self) -> Self {
        *self.live.lock().unwrap() = true;
        self
    }

    pub fn set_transcript(&self, transcript: &str) {
        *self.transcript.lock().unwrap() = transcript.to_string();
    }

    /// Every later request fails with this message, until it is cleared.
    pub fn fail_with(&self, message: Option<&str>) {
        *self.fail_with.lock().unwrap() = message.map(str::to_string);
    }

    pub fn calls(&self) -> Vec<VoiceCall> {
        self.calls.lock().unwrap().clone()
    }

    fn refuse(&self) -> Result<(), VoiceError> {
        match self.fail_with.lock().unwrap().as_ref() {
            Some(message) => Err(VoiceError::Provider(message.clone())),
            None => Ok(()),
        }
    }

    fn record(&self, call: VoiceCall) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait::async_trait]
impl VoiceProvider for FakeVoice {
    async fn transcribe(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        clip: Clip,
    ) -> Result<String, VoiceError> {
        self.record(VoiceCall::Transcribe {
            bytes: clip.pcm16.len(),
        });
        self.refuse()?;
        Ok(self.transcript.lock().unwrap().clone())
    }

    async fn dictate(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<Option<DictationSession>, VoiceError> {
        if !*self.live.lock().unwrap() {
            return Ok(None);
        }
        self.record(VoiceCall::Dictate);
        self.refuse()?;
        let transcript = self.transcript.lock().unwrap().clone();
        let (sender, receiver) = mpsc::unbounded();
        Ok(Some(DictationSession {
            input: Box::new(FakeInput {
                words: transcript.split_whitespace().map(str::to_string).collect(),
                next_word: 0,
                transcript,
                sender,
            }),
            transcripts: receiver.boxed(),
        }))
    }

    async fn speak(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        text: &str,
        voice: Option<&str>,
    ) -> Result<Speech, VoiceError> {
        self.record(VoiceCall::Speak {
            text: text.to_string(),
            voice: voice.map(str::to_string),
        });
        self.refuse()?;
        let voice = voice.unwrap_or("default").to_string();
        Ok(Speech {
            audio: Bytes::from(format!("speech:{voice}:{text}")),
            media_type: "audio/mpeg".to_string(),
            voice,
        })
    }
}

struct FakeInput {
    words: Vec<String>,
    next_word: usize,
    transcript: String,
    sender: mpsc::UnboundedSender<Result<Transcript, VoiceError>>,
}

#[async_trait::async_trait]
impl DictationInput for FakeInput {
    async fn append(&mut self, pcm16: &[u8]) -> Result<(), VoiceError> {
        // A frame that is not whole samples is a caller bug worth
        // surfacing in a test.
        if !pcm16.len().is_multiple_of(2) {
            return Err(VoiceError::Provider(format!(
                "a PCM16 frame has an odd byte count ({})",
                pcm16.len()
            )));
        }
        if let Some(word) = self.words.get(self.next_word) {
            let delta = if self.next_word == 0 {
                word.clone()
            } else {
                format!(" {word}")
            };
            self.next_word += 1;
            let _ = self.sender.unbounded_send(Ok(Transcript::Delta(delta)));
        }
        Ok(())
    }

    async fn commit(&mut self) -> Result<(), VoiceError> {
        let _ = self
            .sender
            .unbounded_send(Ok(Transcript::Final(self.transcript.clone())));
        self.sender.close_channel();
        Ok(())
    }
}
