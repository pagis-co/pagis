//! Live dictation over ElevenLabs Scribe's realtime socket (ADR-0020).
//!
//! The audio goes up as JSON chunks of base64 PCM16 at the sample rate the
//! socket was opened with. The session commits by hand: the release sends
//! a last chunk with `commit`, and Scribe answers the committed transcript,
//! which is the draft. A partial transcript is a guess that a later one
//! replaces, so it adds nothing to the draft.

use base64::Engine;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use llm_router::{RealtimeConnection, RealtimeMessage};

use crate::{DictationInput, DictationSession, SAMPLE_RATE, Transcript, VoiceError};

type Socket = llm_router::RealtimeSocket;

/// The message types Scribe sends when the session fails. It closes the
/// socket after each.
const ERRORS: &[&str] = &[
    "error",
    "auth_error",
    "quota_exceeded",
    "commit_throttled",
    "rate_limited",
    "queue_overflow",
    "resource_exhausted",
    "session_time_limit_exceeded",
    "input_error",
    "invalid_request",
    "chunk_size_exceeded",
    "insufficient_audio_activity",
    "transcriber_error",
    "unaccepted_terms",
];

pub(crate) fn open_scribe_session(connection: RealtimeConnection) -> DictationSession {
    let provider = connection.provider.clone();
    let (sink, stream) = connection.socket.split();
    let transcripts = futures::stream::unfold(Some(stream), move |stream| {
        let provider = provider.clone();
        async move {
            let mut stream = stream?;
            loop {
                let piece = match stream.next().await {
                    Some(Ok(RealtimeMessage::Text(text))) => match event(&text) {
                        Some(piece) => piece,
                        None => continue,
                    },
                    Some(Ok(RealtimeMessage::Close(_))) | None => Err(VoiceError::Provider(
                        format!("{provider}: the session closed with no committed transcript"),
                    )),
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => Err(VoiceError::Provider(format!("{provider}: {error}"))),
                };
                // A committed transcript or a failure ends the session.
                return Some((piece, None));
            }
        }
    })
    .boxed();
    DictationSession {
        input: Box::new(ScribeInput { sink }),
        transcripts,
    }
}

/// The piece one server message gives, or `None` for a message that ends
/// nothing: the session start, a partial transcript, and the like.
fn event(text: &str) -> Option<Result<Transcript, VoiceError>> {
    let message: serde_json::Value = serde_json::from_str(text).ok()?;
    let kind = message["message_type"].as_str()?;
    if kind == "committed_transcript" {
        return Some(Ok(Transcript::Final(
            message["text"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string(),
        )));
    }
    ERRORS.contains(&kind).then(|| {
        Err(VoiceError::Provider(
            message["error"]
                .as_str()
                .or_else(|| message["message"].as_str())
                .unwrap_or(kind)
                .to_string(),
        ))
    })
}

struct ScribeInput {
    sink: SplitSink<Socket, RealtimeMessage>,
}

impl ScribeInput {
    async fn send(&mut self, pcm16: &[u8], commit: bool) -> Result<(), VoiceError> {
        let chunk = serde_json::json!({
            "message_type": "input_audio_chunk",
            "audio_base_64": base64::engine::general_purpose::STANDARD.encode(pcm16),
            "commit": commit,
            "sample_rate": SAMPLE_RATE,
        });
        self.sink
            .send(RealtimeMessage::text(chunk.to_string()))
            .await
            .map_err(|error| VoiceError::Provider(error.to_string()))
    }
}

#[async_trait::async_trait]
impl DictationInput for ScribeInput {
    async fn append(&mut self, pcm16: &[u8]) -> Result<(), VoiceError> {
        self.send(pcm16, false).await
    }

    async fn commit(&mut self) -> Result<(), VoiceError> {
        self.send(&[], true).await
    }
}
