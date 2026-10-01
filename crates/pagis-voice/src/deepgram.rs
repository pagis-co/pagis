//! Live dictation over Deepgram's `/listen` socket (ADR-0020).
//!
//! The audio goes up as binary PCM16 frames in the format the socket was
//! opened with. Deepgram answers with `Results`: an interim result guesses
//! the words in progress, and a final result fixes a stretch of speech.
//! Each final result adds to the draft as a [`Transcript::Delta`], so the
//! deltas together are the draft. The release sends `CloseStream`:
//! Deepgram transcribes the rest of the audio, sends its last final
//! results and a `Metadata` message, and closes. The whole draft is then
//! the [`Transcript::Final`].

use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use llm_router::{RealtimeConnection, RealtimeMessage};

use crate::{DictationInput, DictationSession, Transcript, VoiceError};

type Socket = llm_router::RealtimeSocket;

pub(crate) fn open_listen_session(connection: RealtimeConnection) -> DictationSession {
    let provider = connection.provider.clone();
    let (sink, stream) = connection.socket.split();
    let transcripts = futures::stream::unfold(
        (stream, String::new(), false),
        move |(mut stream, mut draft, done)| {
            let provider = provider.clone();
            async move {
                if done {
                    return None;
                }
                loop {
                    let piece = match stream.next().await {
                        Some(Ok(RealtimeMessage::Text(text))) => match event(&text) {
                            Event::Final(words) if !words.is_empty() => {
                                let delta = if draft.is_empty() {
                                    words
                                } else {
                                    format!(" {words}")
                                };
                                draft.push_str(&delta);
                                Ok(Transcript::Delta(delta))
                            }
                            Event::Final(_) | Event::Other => continue,
                            Event::Closed => Ok(Transcript::Final(draft.clone())),
                            Event::Error(message) => Err(VoiceError::Provider(message)),
                        },
                        Some(Ok(RealtimeMessage::Close(_))) | None => {
                            Ok(Transcript::Final(draft.clone()))
                        }
                        Some(Ok(_)) => continue,
                        Some(Err(error)) => {
                            Err(VoiceError::Provider(format!("{provider}: {error}")))
                        }
                    };
                    let done = !matches!(piece, Ok(Transcript::Delta(_)));
                    return Some((piece, (stream, draft, done)));
                }
            }
        },
    )
    .boxed();
    DictationSession {
        input: Box::new(ListenInput { sink }),
        transcripts,
    }
}

/// What one server message means to the draft.
enum Event {
    /// A final result: the words of one stretch of speech.
    Final(String),
    /// The metadata that ends the stream after `CloseStream`.
    Closed,
    Error(String),
    /// An interim result, a speech-start event, or anything else.
    Other,
}

fn event(text: &str) -> Event {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(text) else {
        return Event::Other;
    };
    match message["type"].as_str() {
        Some("Results") if message["is_final"] == true => Event::Final(
            message
                .pointer("/channel/alternatives/0/transcript")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
        ),
        Some("Metadata") => Event::Closed,
        Some("Error") => Event::Error(
            message["description"]
                .as_str()
                .or_else(|| message["message"].as_str())
                .unwrap_or("the live transcription failed")
                .to_string(),
        ),
        _ => Event::Other,
    }
}

struct ListenInput {
    sink: SplitSink<Socket, RealtimeMessage>,
}

#[async_trait::async_trait]
impl DictationInput for ListenInput {
    async fn append(&mut self, pcm16: &[u8]) -> Result<(), VoiceError> {
        self.sink
            .send(RealtimeMessage::binary(pcm16.to_vec()))
            .await
            .map_err(|error| VoiceError::Provider(error.to_string()))
    }

    async fn commit(&mut self) -> Result<(), VoiceError> {
        self.sink
            .send(RealtimeMessage::text(
                serde_json::json!({ "type": "CloseStream" }).to_string(),
            ))
            .await
            .map_err(|error| VoiceError::Provider(error.to_string()))
    }
}
