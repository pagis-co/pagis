//! Voice in a Thread (ADR-0020): Dictation in, spoken replies
//! out, and the Agent Voice.
//!
//! The crate has one seam, [`VoiceProvider`], with three duties that
//! the two Workspace Model Aliases carry: `transcribe` turns a held
//! clip into text, either live over a transcription-only realtime
//! session or buffered on release, and `speak` turns one prose block
//! into audio. [`RouterVoice`] is the production implementation over
//! `llm-router`; [`fake::FakeVoice`] is the scripted one tests run on.
//!
//! Nothing here stores audio. A clip is transcribed and dropped, and
//! synthesized speech is handed to the caller and not kept.

mod deepgram;
pub mod fake;
mod router;
mod wav;

use bytes::Bytes;
use futures::stream::BoxStream;

pub use router::RouterVoice;
pub use wav::pcm16_wav;

/// The Workspace Model Alias that turns speech into text.
pub const TRANSCRIBE_ALIAS: &str = "transcribe";

/// The Workspace Model Alias that turns text into speech.
pub const SPEAK_ALIAS: &str = "speak";

/// The sample rate of every clip on the seam: PCM16, mono, 24 kHz, the
/// rate the realtime transcription session takes (ADR-0020).
pub const SAMPLE_RATE: u32 = 24_000;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    /// The alias is not in the Workspace. Settings creates it.
    #[error("model alias `{0}` not found")]
    NoAlias(String),
    /// The alias names no candidate on a provider with a key.
    #[error("model alias `{0}` has no candidate with a configured provider")]
    NoProvider(String),
    #[error(transparent)]
    Store(#[from] pagis_core::StoreError),
    #[error("{0}")]
    Keys(String),
    /// The provider refused or failed. The message is the provider's.
    #[error("{0}")]
    Provider(String),
}

/// One held clip: PCM16 little-endian mono samples at [`SAMPLE_RATE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub pcm16: Bytes,
}

/// One piece of a live transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transcript {
    /// More text of the utterance in progress.
    Delta(String),
    /// The whole utterance, after the commit. The session is over.
    Final(String),
}

/// The audio half of a live dictation: the caller appends frames while
/// the button is held and commits on release.
#[async_trait::async_trait]
pub trait DictationInput: Send {
    async fn append(&mut self, pcm16: &[u8]) -> Result<(), VoiceError>;
    async fn commit(&mut self) -> Result<(), VoiceError>;
}

/// One live dictation: audio goes in through `input`, text comes back
/// on `transcripts`, and the session ends with [`Transcript::Final`].
/// Dropping both halves closes the provider socket.
pub struct DictationSession {
    pub input: Box<dyn DictationInput>,
    pub transcripts: BoxStream<'static, Result<Transcript, VoiceError>>,
}

/// Synthesized speech for one block. Bytes only; nothing is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Speech {
    pub audio: Bytes,
    /// The media type of `audio`, e.g. `audio/mpeg`.
    pub media_type: String,
    /// The voice that spoke it.
    pub voice: String,
}

/// The voice seam. One implementation per provider path; the daemon
/// holds one.
#[async_trait::async_trait]
pub trait VoiceProvider: Send + Sync {
    /// Transcribe a whole clip. Works on every provider the
    /// `transcribe` alias names.
    ///
    /// The Workspace comes from the caller: the model aliases a
    /// voice call resolves are the asking tenant's own.
    async fn transcribe(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        clip: Clip,
    ) -> Result<String, VoiceError>;

    /// Open a live transcription session, or `None` when no candidate
    /// of the `transcribe` alias has a realtime socket. Absent is not an
    /// error (ADR-0005): the caller buffers the clip and calls
    /// [`VoiceProvider::transcribe`] on release instead.
    async fn dictate(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<Option<DictationSession>, VoiceError>;

    /// Speak one prose block in `voice`, a voice of the model that the
    /// `speak` alias serves on. The caller resolves it from the Provider
    /// Voice List of that model.
    async fn speak(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        text: &str,
        voice: &str,
    ) -> Result<Speech, VoiceError>;
}
