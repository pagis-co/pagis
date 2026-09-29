//! Live smoke test against real providers.
//!
//! Opt-in: it spends real money (well under $0.10 on current OpenAI
//! prices; the video job is excluded because Sora bills per clip). Run
//! with the provider key in the environment:
//!
//! ```sh
//! OPENAI_API_KEY=... cargo run --example smoke
//! ```
//!
//! Each step prints PASS, FAIL, or SKIP (feature not enabled for the
//! account). The process exits non-zero when any step fails.

use futures::StreamExt;
use llm_router::{
    AudioFormat, AudioOut, ChatRequest, ContentPart, EmbeddingsRequest, FinishReason, ImageData,
    ImageRequest, Message, MessageAccumulator, Modality, ProviderConfig, ReasoningConfig,
    ReasoningEffort, Role, Router, RouterConfig, SizeSpec, SpeechRequest, StreamEvent, Tool,
    TranscriptionRequest,
};
use serde_json::json;

const CHAT_MODEL: &str = "openai/gpt-5-mini";
const RESPONSES_CHAT_MODEL: &str = "openai-responses/gpt-5-mini";
const AUDIO_MODELS: &[&str] = &["openai/gpt-audio", "openai/gpt-4o-audio-preview"];
const TTS_MODEL: &str = "openai/gpt-4o-mini-tts";
const STT_MODEL: &str = "openai/gpt-4o-mini-transcribe";
const STT_TIMESTAMP_MODEL: &str = "openai/whisper-1";
const EMBED_MODEL: &str = "openai/text-embedding-3-small";
const IMAGE_MODEL: &str = "openai/gpt-image-1";
const REALTIME_MODELS: &[&str] = &["openai/gpt-realtime-2.1-mini", "openai/gpt-realtime-2.1"];
const CUA_MODEL: &str = "openai-responses/computer-use-preview";

struct Report {
    passed: u32,
    failed: u32,
    skipped: u32,
}

impl Report {
    fn record(&mut self, name: &str, outcome: Result<String, String>) {
        match outcome {
            Ok(detail) => {
                self.passed += 1;
                println!("PASS  {name}: {detail}");
            }
            Err(detail) if is_unavailable(&detail) => {
                self.skipped += 1;
                println!("SKIP  {name}: {detail}");
            }
            Err(detail) => {
                self.failed += 1;
                println!("FAIL  {name}: {detail}");
            }
        }
    }
}

/// Access errors mean the account lacks the model or feature, not that
/// the router is wrong.
fn is_unavailable(detail: &str) -> bool {
    let lower = detail.to_lowercase();
    [
        "model_not_found",
        "model not found",
        "does not exist",
        "not allowed",
        "must be verified",
        "unsupported_model",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn err(e: impl std::fmt::Display) -> String {
    let text = e.to_string();
    text.chars().take(300).collect()
}

#[tokio::main]
async fn main() {
    let Ok(openai_key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("OPENAI_API_KEY is not set");
        std::process::exit(2);
    };
    let config = RouterConfig::new()
        .provider("openai", ProviderConfig::openai(openai_key.clone()))
        .provider(
            "openai-responses",
            ProviderConfig::openai_responses(openai_key),
        );
    let router = Router::new(config).expect("valid config");

    let mut report = Report {
        passed: 0,
        failed: 0,
        skipped: 0,
    };

    report.record("chat", chat(&router).await);
    report.record("streaming", streaming(&router).await);
    report.record(
        "tool loop (chat completions)",
        tool_loop(&router, CHAT_MODEL).await,
    );
    report.record(
        "tool loop (responses, reasoning replay)",
        tool_loop(&router, RESPONSES_CHAT_MODEL).await,
    );
    let tts_audio = match tts(&router).await {
        Ok((detail, audio)) => {
            report.record("speech (tts)", Ok(detail));
            Some(audio)
        }
        Err(detail) => {
            report.record("speech (tts)", Err(detail));
            None
        }
    };
    if let Some(audio) = tts_audio {
        report.record(
            "transcription",
            stt(&router, STT_MODEL, false, audio.clone()).await,
        );
        report.record(
            "transcription with timestamps (whisper-1)",
            stt(&router, STT_TIMESTAMP_MODEL, true, audio.clone()).await,
        );
        report.record("audio chat", audio_chat(&router, audio.clone()).await);
        report.record(
            "audio chat streaming",
            audio_chat_stream(&router, audio).await,
        );
    }
    report.record("embeddings", embeddings(&router).await);
    report.record("image generation", image(&router).await);
    report.record("realtime", realtime(&router).await);
    report.record(
        "computer use (responses preview)",
        computer_use(&router).await,
    );
    println!("skipped by design: video generation (Sora bills per clip)");

    println!(
        "\n{} passed, {} failed, {} skipped",
        report.passed, report.failed, report.skipped
    );
    if report.failed > 0 {
        std::process::exit(1);
    }
}

async fn chat(router: &Router) -> Result<String, String> {
    let mut request = ChatRequest::new(
        CHAT_MODEL,
        vec![Message::user("Reply with exactly the word: pong")],
    );
    request.max_tokens = Some(2000);
    request.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::Low),
        max_tokens: None,
    });
    let response = router.chat(&request).await.map_err(err)?;
    let text = response.message.text_content();
    if !text.to_lowercase().contains("pong") {
        return Err(format!("unexpected reply: {text:?}"));
    }
    Ok(format!(
        "reply {:?}, usage {}in/{}out, cost {:?}",
        text.trim(),
        response.usage.input_tokens,
        response.usage.output_tokens,
        response.cost_usd
    ))
}

async fn streaming(router: &Router) -> Result<String, String> {
    let mut request = ChatRequest::new(
        CHAT_MODEL,
        vec![Message::user("Count from 1 to 5, digits only.")],
    );
    request.max_tokens = Some(2000);
    let mut stream = router.chat_stream(&request).await.map_err(err)?;
    let mut acc = MessageAccumulator::new();
    let mut deltas = 0u32;
    while let Some(event) = stream.events.next().await {
        let event = event.map_err(err)?;
        if matches!(event, StreamEvent::TextDelta { .. }) {
            deltas += 1;
        }
        acc.push(&event);
    }
    let streamed = acc.finish();
    if streamed.usage.is_none() {
        return Err("stream ended without usage".into());
    }
    if streamed.message.text_content().is_empty() {
        return Err("stream produced no text".into());
    }
    Ok(format!(
        "{deltas} text deltas, finish {:?}, rebuilt {:?}",
        streamed.finish_reason,
        streamed.message.text_content().trim()
    ))
}

/// Two turns with a function tool. On the responses protocol this also
/// proves stateless reasoning replay: turn 2 resends the encrypted
/// reasoning item and the function call.
async fn tool_loop(router: &Router, model: &str) -> Result<String, String> {
    let mut request = ChatRequest::new(
        model,
        vec![Message::user(
            "Use the get_weather tool for Paris, then state the result in one short sentence.",
        )],
    );
    request.max_tokens = Some(4000);
    request.tools = vec![Tool::function(
        "get_weather",
        "Get the current weather for a city",
        json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
        }),
    )];

    let first = router.chat(&request).await.map_err(err)?;
    if first.finish_reason != FinishReason::ToolCalls || first.message.tool_calls.is_empty() {
        return Err(format!(
            "expected a tool call, got finish {:?} with text {:?}",
            first.finish_reason,
            first.message.text_content()
        ));
    }
    let call = first.message.tool_calls[0].clone();
    serde_json::from_str::<serde_json::Value>(&call.arguments)
        .map_err(|e| format!("tool arguments are not JSON: {e}"))?;
    request.messages.push(first.message.clone());
    request
        .messages
        .push(Message::tool(&call.id, "22C and sunny"));

    let second = router.chat(&request).await.map_err(err)?;
    let text = second.message.text_content();
    if text.is_empty() {
        return Err("no final answer after the tool result".into());
    }
    Ok(format!("call {} -> {:?}", call.name, text.trim()))
}

async fn tts(router: &Router) -> Result<(String, bytes::Bytes), String> {
    let mut request = SpeechRequest::new(
        TTS_MODEL,
        "The quick brown fox jumps over the lazy dog.",
        "alloy",
    );
    request.format = Some(AudioFormat::Wav);
    let response = router.speech(&request).await.map_err(err)?;
    if response.audio.len() < 1000 {
        return Err(format!(
            "suspiciously small audio: {} bytes",
            response.audio.len()
        ));
    }
    Ok((
        format!("{} bytes of {}", response.audio.len(), response.media_type),
        response.audio,
    ))
}

async fn stt(
    router: &Router,
    model: &str,
    timestamps: bool,
    audio: bytes::Bytes,
) -> Result<String, String> {
    let mut request = TranscriptionRequest::new(model, audio, "audio/wav");
    request.timestamps = timestamps;
    let response = router.transcribe(&request).await.map_err(err)?;
    if !response.text.to_lowercase().contains("fox") {
        return Err(format!("transcript missed the text: {:?}", response.text));
    }
    if timestamps && response.segments.is_empty() && response.words.is_empty() {
        return Err("asked for timestamps but got none".into());
    }
    Ok(format!(
        "{:?} ({} segments, {} words)",
        response.text.trim(),
        response.segments.len(),
        response.words.len()
    ))
}

fn audio_request(model: &str, wav: &bytes::Bytes, out_format: AudioFormat) -> ChatRequest {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(wav);
    let mut request = ChatRequest::new(
        model,
        vec![Message {
            role: Role::User,
            content: vec![
                ContentPart::Text {
                    text: "Repeat the sentence you hear.".into(),
                },
                ContentPart::InputAudio {
                    data: b64,
                    format: AudioFormat::Wav,
                },
            ],
            tool_calls: vec![],
            tool_call_id: None,
        }],
    );
    request.modalities = vec![Modality::Text, Modality::Audio];
    request.audio = Some(AudioOut {
        voice: "alloy".into(),
        format: out_format,
    });
    request.max_tokens = Some(4000);
    request
}

async fn audio_chat(router: &Router, wav: bytes::Bytes) -> Result<String, String> {
    let mut errors: Vec<String> = Vec::new();
    for model in AUDIO_MODELS {
        match router
            .chat(&audio_request(model, &wav, AudioFormat::Wav))
            .await
        {
            Ok(response) => {
                let audio = response.message.content.iter().find_map(|p| match p {
                    ContentPart::OutputAudio {
                        data, transcript, ..
                    } => Some((data.clone(), transcript.clone())),
                    _ => None,
                });
                let Some((data, transcript)) = audio else {
                    return Err("no OutputAudio part in the reply".into());
                };
                let bytes = decode_b64(data.as_deref().unwrap_or(""))?;
                return Ok(format!(
                    "{model}: {} audio bytes, transcript {:?}",
                    bytes,
                    transcript.unwrap_or_default().trim()
                ));
            }
            Err(e) => errors.push(format!("{model}: {}", err(e))),
        }
    }
    Err(errors.join(" | "))
}

/// Streamed assistant audio must rebuild into decodable base64 — the
/// accumulator decodes and re-joins the chunks.
async fn audio_chat_stream(router: &Router, wav: bytes::Bytes) -> Result<String, String> {
    let mut errors: Vec<String> = Vec::new();
    for model in AUDIO_MODELS {
        // Streamed chat audio comes as raw PCM only; wav is
        // non-streaming only.
        let stream = match router
            .chat_stream(&audio_request(model, &wav, AudioFormat::Pcm16))
            .await
        {
            Ok(stream) => stream,
            Err(e) => {
                errors.push(format!("{model}: {}", err(e)));
                continue;
            }
        };
        let mut events = stream.events;
        let mut acc = MessageAccumulator::new();
        let mut audio_deltas = 0u32;
        while let Some(event) = events.next().await {
            let event = event.map_err(err)?;
            if matches!(event, StreamEvent::AudioDelta { .. }) {
                audio_deltas += 1;
            }
            acc.push(&event);
        }
        let streamed = acc.finish();
        let Some(ContentPart::OutputAudio {
            data: Some(data), ..
        }) = streamed
            .message
            .content
            .iter()
            .find(|p| matches!(p, ContentPart::OutputAudio { .. }))
        else {
            return Err(format!("{model}: stream rebuilt no audio part"));
        };
        let bytes = decode_b64(data)?;
        return Ok(format!(
            "{model}: {audio_deltas} audio deltas rebuilt into {bytes} decodable bytes"
        ));
    }
    Err(errors.join(" | "))
}

fn decode_b64(data: &str) -> Result<usize, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map(|b| b.len())
        .map_err(|e| format!("accumulated audio is not decodable base64: {e}"))
}

async fn embeddings(router: &Router) -> Result<String, String> {
    let response = router
        .embed(&EmbeddingsRequest::new(
            EMBED_MODEL,
            vec!["hello".to_owned(), "world".to_owned()],
        ))
        .await
        .map_err(err)?;
    if response.embeddings.len() != 2 || response.embeddings[0].is_empty() {
        return Err("wrong embedding shape".into());
    }
    Ok(format!(
        "2 vectors of {} dims, cost {:?}",
        response.embeddings[0].len(),
        response.cost_usd
    ))
}

async fn image(router: &Router) -> Result<String, String> {
    let mut request = ImageRequest::new(IMAGE_MODEL, "a single red triangle on white");
    request.size = Some(SizeSpec::pixels(1024, 1024));
    request.quality = Some("low".into());
    let response = router.generate_image(&request).await.map_err(err)?;
    let Some(image) = response.images.first() else {
        return Err("no image in the response".into());
    };
    let ImageData::B64 { data } = &image.image else {
        return Err("expected base64 image data".into());
    };
    let bytes = decode_b64(data)?;
    Ok(format!(
        "{bytes} image bytes, mime {:?}, usage {:?}",
        response.mime_type,
        response.usage.map(|u| u.output_tokens)
    ))
}

async fn realtime(router: &Router) -> Result<String, String> {
    let mut last = String::from("no realtime model tried");
    for model in REALTIME_MODELS {
        match router.realtime_connect(model).await {
            Ok(mut session) => {
                let Some(frame) = session.next().await else {
                    return Err("socket closed before the first event".into());
                };
                let frame = frame.map_err(err)?;
                let text = frame.into_text().map_err(err)?;
                if !text.contains("session.created") {
                    return Err(format!(
                        "unexpected first event: {}",
                        &text[..text.len().min(120)]
                    ));
                }
                session.close().await.map_err(err)?;
                return Ok(format!("{model}: received session.created"));
            }
            Err(e) => last = err(e),
        }
    }
    Err(last)
}

async fn computer_use(router: &Router) -> Result<String, String> {
    let mut config = serde_json::Map::new();
    config.insert("display_width".into(), json!(1024));
    config.insert("display_height".into(), json!(768));
    config.insert("environment".into(), json!("linux"));
    let mut request = ChatRequest::new(
        CUA_MODEL,
        vec![Message::user("Take a screenshot of the current screen.")],
    );
    request.tools = vec![Tool::provider_defined(
        "computer_use_preview",
        "computer",
        config,
    )];
    request.max_tokens = Some(2000);
    let response = router.chat(&request).await.map_err(err)?;
    let Some(call) = response.message.tool_calls.first() else {
        return Err(format!(
            "no computer call; finish {:?}, text {:?}",
            response.finish_reason,
            response.message.text_content()
        ));
    };
    let action = call.computer_action();
    if action.is_null() {
        return Err("computer call carried no action".into());
    }
    Ok(format!("call {} with action {}", call.name, action))
}
