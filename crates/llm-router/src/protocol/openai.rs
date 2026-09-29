//! The `openai-chat` codec: OpenAI Chat Completions and compatible APIs.

use async_stream::stream;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::error::{Error, ErrorKind};
use crate::protocol::{ByteStream, EventStream, ModelPage, Protocol, model_list};
use crate::types::{
    ChatRequest, ChatResponse, ContentPart, EmbeddingsRequest, EmbeddingsResponse, FinishReason,
    GeneratedImage, ImageData, ImageInput, ImageRequest, ImageResponse, Message, Role,
    SpeechRequest, StreamEvent, ToolCall, ToolChoice, TranscriptSegment, TranscriptWord,
    TranscriptionRequest, TranscriptionResponse, Usage, VideoJob, VideoRequest, VideoStatus,
};

pub struct OpenAiChat;

impl Protocol for OpenAiChat {
    fn build_list_models_request(
        &self,
        http: &reqwest::Client,
        _provider_key: &str,
        provider: &ProviderConfig,
        _after: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Ok(get(http, provider, &provider.compat.model_list_path))
    }

    fn parse_list_models(&self, provider_key: &str, body: &[u8]) -> Result<ModelPage, Error> {
        model_list::parse_openai(provider_key, body)
    }

    fn build_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &ChatRequest,
        stream: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        // Provider-defined tools (computer use) exist on the Responses API
        // and on Anthropic, not on Chat Completions.
        if req.tools.iter().any(|t| t.kind.is_some()) {
            return Err(Error::Unsupported {
                provider: provider_key.to_owned(),
                feature: "provider-defined tools",
            });
        }
        let mut body = json!({
            "model": model,
            "messages": encode_messages(&req.messages),
        });
        let obj = body.as_object_mut().expect("body is an object");
        if let Some(t) = req.temperature {
            obj.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            obj.insert("top_p".into(), json!(p));
        }
        if let Some(m) = req.max_tokens {
            obj.insert(
                provider.compat.max_tokens_field.wire_name().into(),
                json!(m),
            );
        }
        if !req.stop.is_empty() {
            obj.insert("stop".into(), json!(req.stop));
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            obj.insert("tools".into(), json!(tools));
        }
        if let Some(choice) = &req.tool_choice {
            let value = match choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Required => json!("required"),
                ToolChoice::Tool(name) => {
                    json!({"type": "function", "function": {"name": name}})
                }
            };
            obj.insert("tool_choice".into(), value);
        }
        if let Some(reasoning) = &req.reasoning
            && provider.compat.reasoning_effort
        {
            obj.insert("reasoning_effort".into(), json!(effort_name(reasoning)));
        }
        if !req.modalities.is_empty() {
            let modalities: Vec<&str> = req.modalities.iter().map(|m| m.wire_name()).collect();
            obj.insert("modalities".into(), json!(modalities));
        }
        if let Some(audio) = &req.audio {
            obj.insert(
                "audio".into(),
                json!({"voice": audio.voice, "format": audio.format.wire_name()}),
            );
        }
        if let Some(format) = &req.output_schema {
            let mut schema = json!({
                "name": format.name,
                "schema": format.schema,
                "strict": true,
            });
            if let Some(description) = &format.description {
                schema["description"] = json!(description);
            }
            obj.insert(
                "response_format".into(),
                json!({"type": "json_schema", "json_schema": schema}),
            );
        }
        if stream {
            obj.insert("stream".into(), json!(true));
            if provider.compat.stream_options {
                obj.insert("stream_options".into(), json!({"include_usage": true}));
            }
        }
        for (key, value) in &req.extra {
            obj.insert(key.clone(), value.clone());
        }

        Ok(post(http, provider, "/chat/completions").json(&body))
    }

    fn parse_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<ChatResponse, Error> {
        let wire: WireResponse =
            serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("failed to decode chat completion: {e}"),
            })?;
        let choice = wire
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: "response has no choices".to_owned(),
            })?;

        let mut content = Vec::new();
        // Compatible servers return reasoning in `reasoning_content`
        // (DeepSeek, vLLM, llama.cpp) or `reasoning` (Groq, OpenRouter,
        // Ollama).
        if let Some(text) = choice
            .message
            .reasoning_content
            .or(choice.message.reasoning)
            && !text.is_empty()
        {
            content.push(ContentPart::Reasoning {
                text,
                signature: None,
            });
        }
        if let Some(text) = choice.message.content
            && !text.is_empty()
        {
            content.push(ContentPart::Text { text });
        }
        if let Some(audio) = choice.message.audio {
            content.push(ContentPart::OutputAudio {
                id: audio.id,
                data: audio.data,
                transcript: audio.transcript,
                expires_at: audio.expires_at,
            });
        }
        let tool_calls = choice
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|c| ToolCall {
                id: c.id.unwrap_or_default(),
                name: c.function.name.unwrap_or_default(),
                arguments: c.function.arguments.unwrap_or_default(),
            })
            .collect();

        Ok(ChatResponse {
            provider: provider_key.to_owned(),
            model: wire.model.unwrap_or_else(|| model.to_owned()),
            message: Message {
                role: Role::Assistant,
                content,
                tool_calls,
                tool_call_id: None,
            },
            finish_reason: map_finish_reason(choice.finish_reason.as_deref()),
            native_finish_reason: choice.finish_reason,
            usage: wire.usage.map(Usage::from).unwrap_or_default(),
            cost_usd: None,
        })
    }

    fn parse_error(&self, provider_key: &str, status: u16, body: &[u8]) -> Error {
        let raw: Option<Value> = serde_json::from_slice(body).ok();
        let (message, code) = extract_error_fields(raw.as_ref());
        let message = message.unwrap_or_else(|| {
            String::from_utf8_lossy(&body[..body.len().min(2048)])
                .trim()
                .to_owned()
        });
        let kind = classify(status, code.as_deref(), &message);
        Error::Provider {
            provider: provider_key.to_owned(),
            status,
            kind,
            message: crate::protocol::cap_error_text(message),
            raw,
        }
    }

    fn stream_events(&self, provider_key: &str, bytes: ByteStream) -> EventStream {
        let provider = provider_key.to_owned();
        stream! {
            let mut events = bytes.eventsource();
            let mut finish: Option<(FinishReason, Option<String>)> = None;
            let mut usage: Option<Usage> = None;
            // Tool-call correlation. Some servers (Ollama) send `index: 0`
            // for every parallel call, or omit the index; synthesize a
            // stable per-stream index from the call id instead.
            let mut synth_by_id: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            let mut synth_by_wire: std::collections::HashMap<u32, u32> =
                std::collections::HashMap::new();
            let mut next_tool_index: u32 = 0;
            // Assistant audio state, so the stream can close it with one
            // `AudioEnd` before `Finish`.
            let mut audio_seen = false;
            let mut audio_id: Option<String> = None;
            let mut audio_expires: Option<u64> = None;

            while let Some(event) = events.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        yield Err(Error::Stream {
                            provider: provider.clone(),
                            message: e.to_string(),
                        });
                        return;
                    }
                };
                if event.data.trim() == "[DONE]" {
                    if audio_seen {
                        yield Ok(StreamEvent::AudioEnd {
                            id: audio_id.take(),
                            expires_at: audio_expires.take(),
                        });
                    }
                    let (reason, native) = finish.take().unwrap_or((FinishReason::Stop, None));
                    yield Ok(StreamEvent::Finish { reason, native_reason: native, usage });
                    return;
                }
                let chunk: WireChunk = match serde_json::from_str(&event.data) {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        yield Err(Error::Stream {
                            provider: provider.clone(),
                            message: format!("failed to decode stream chunk: {e}"),
                        });
                        return;
                    }
                };
                if let Some(u) = chunk.usage {
                    usage = Some(u.into());
                }
                for choice in chunk.choices {
                    if let Some(text) = choice.delta.reasoning_content.or(choice.delta.reasoning)
                        && !text.is_empty()
                    {
                        yield Ok(StreamEvent::ReasoningDelta { text });
                    }
                    if let Some(text) = choice.delta.content
                        && !text.is_empty()
                    {
                        yield Ok(StreamEvent::TextDelta { text });
                    }
                    if let Some(audio) = choice.delta.audio {
                        audio_seen = true;
                        if audio.id.is_some() {
                            audio_id = audio.id;
                        }
                        if audio.expires_at.is_some() {
                            audio_expires = audio.expires_at;
                        }
                        if let Some(text) = audio.transcript
                            && !text.is_empty()
                        {
                            yield Ok(StreamEvent::AudioTranscriptDelta { text });
                        }
                        if let Some(data) = audio.data
                            && !data.is_empty()
                        {
                            yield Ok(StreamEvent::AudioDelta { data });
                        }
                    }
                    for call in choice.delta.tool_calls.unwrap_or_default() {
                        let function = call.function.unwrap_or_default();
                        let index = match call.id.as_deref().filter(|id| !id.is_empty()) {
                            // A chunk with an id starts a call, or continues
                            // one the server already started under that id.
                            Some(id) => match synth_by_id.get(id) {
                                Some(&index) => {
                                    synth_by_wire.insert(call.index, index);
                                    index
                                }
                                None => {
                                    let index = next_tool_index;
                                    next_tool_index += 1;
                                    synth_by_id.insert(id.to_owned(), index);
                                    synth_by_wire.insert(call.index, index);
                                    yield Ok(StreamEvent::ToolCallStart {
                                        index,
                                        id: id.to_owned(),
                                        name: function.name.clone().unwrap_or_default(),
                                    });
                                    index
                                }
                            },
                            // Without an id, the chunk continues the latest
                            // call at this wire index.
                            None => match synth_by_wire.get(&call.index) {
                                Some(&index) => index,
                                None => {
                                    let index = next_tool_index;
                                    next_tool_index += 1;
                                    synth_by_wire.insert(call.index, index);
                                    yield Ok(StreamEvent::ToolCallStart {
                                        index,
                                        id: String::new(),
                                        name: function.name.clone().unwrap_or_default(),
                                    });
                                    index
                                }
                            },
                        };
                        if let Some(arguments) = function.arguments
                            && !arguments.is_empty()
                        {
                            yield Ok(StreamEvent::ToolCallDelta { index, arguments });
                        }
                    }
                    if let Some(reason) = choice.finish_reason {
                        finish = Some((map_finish_reason(Some(&reason)), Some(reason)));
                    }
                }
            }

            // Some compatible providers close the stream without `[DONE]`.
            match finish.take() {
                Some((reason, native)) => {
                    if audio_seen {
                        yield Ok(StreamEvent::AudioEnd {
                            id: audio_id.take(),
                            expires_at: audio_expires.take(),
                        });
                    }
                    yield Ok(StreamEvent::Finish { reason, native_reason: native, usage });
                }
                None => {
                    yield Err(Error::Stream {
                        provider: provider.clone(),
                        message: "stream ended without a finish reason".to_owned(),
                    });
                }
            }
        }
        .boxed()
    }

    fn build_embeddings_request(
        &self,
        http: &reqwest::Client,
        _provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &EmbeddingsRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Ok(post(http, provider, "/embeddings").json(&json!({"model": model, "input": req.input})))
    }

    fn parse_embeddings_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<EmbeddingsResponse, Error> {
        let wire: WireEmbeddings =
            serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("failed to decode embeddings: {e}"),
            })?;
        let mut data = wire.data;
        data.sort_by_key(|d| d.index);
        Ok(EmbeddingsResponse {
            provider: provider_key.to_owned(),
            model: wire.model.unwrap_or_else(|| model.to_owned()),
            embeddings: data.into_iter().map(|d| d.embedding).collect(),
            usage: wire.usage.map(Usage::from).unwrap_or_default(),
            cost_usd: None,
        })
    }

    fn build_image_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &ImageRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let size = match &req.size {
            None => None,
            Some(size) => Some(size.pixel_string().ok_or_else(|| {
                Error::InvalidConfig(format!(
                    "provider `{provider_key}` takes pixel sizes; use SizeSpec::Pixels"
                ))
            })?),
        };
        if req.input_images.is_empty() {
            let mut body = json!({"model": model, "prompt": req.prompt});
            let obj = body.as_object_mut().expect("body is an object");
            if let Some(count) = req.count {
                obj.insert("n".into(), json!(count));
            }
            if let Some(size) = size {
                obj.insert("size".into(), json!(size));
            }
            if let Some(quality) = &req.quality {
                obj.insert("quality".into(), json!(quality));
            }
            if let Some(format) = &req.output_format {
                obj.insert("output_format".into(), json!(format));
            }
            if let Some(background) = &req.background {
                obj.insert("background".into(), json!(background));
            }
            for (key, value) in &req.extra {
                obj.insert(key.clone(), value.clone());
            }
            return Ok(post(http, provider, "/images/generations").json(&body));
        }

        // Reference images route to the multipart edits endpoint. A form
        // cannot override a repeated field the way a JSON map does, so a
        // built-in field yields to `extra` under the same name.
        let mut form = reqwest::multipart::Form::new()
            .text("model", model.to_owned())
            .text("prompt", req.prompt.clone());
        // One image sends the singular field; several send the array form.
        if let [input] = req.input_images.as_slice() {
            form = form.part("image", image_part(provider_key, input)?);
        } else {
            for input in &req.input_images {
                form = form.part("image[]", image_part(provider_key, input)?);
            }
        }
        if let Some(mask) = &req.mask {
            form = form.part("mask", image_part(provider_key, mask)?);
        }
        let mut fields: Vec<(&str, String)> = Vec::new();
        if let Some(count) = req.count {
            fields.push(("n", count.to_string()));
        }
        if let Some(size) = size {
            fields.push(("size", size));
        }
        if let Some(quality) = &req.quality {
            fields.push(("quality", quality.clone()));
        }
        if let Some(format) = &req.output_format {
            fields.push(("output_format", format.clone()));
        }
        if let Some(background) = &req.background {
            fields.push(("background", background.clone()));
        }
        for (name, value) in fields {
            if !req.extra.contains_key(name) {
                form = form.text(name, value);
            }
        }
        for (key, value) in &req.extra {
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            form = form.text(key.clone(), text);
        }
        Ok(post(http, provider, "/images/edits").multipart(form))
    }

    fn parse_image_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<ImageResponse, Error> {
        let wire: WireImages =
            serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("failed to decode image response: {e}"),
            })?;
        let images = wire
            .data
            .into_iter()
            .filter_map(|d| {
                let image = match (d.b64_json, d.url) {
                    (Some(data), _) => ImageData::B64 { data },
                    (None, Some(url)) => ImageData::Url { url },
                    (None, None) => return None,
                };
                Some(GeneratedImage {
                    image,
                    revised_prompt: d.revised_prompt,
                })
            })
            .collect();
        Ok(ImageResponse {
            provider: provider_key.to_owned(),
            model: model.to_owned(),
            images,
            mime_type: wire.output_format.map(|f| image_media_type(&f).to_owned()),
            usage: wire.usage.map(|u| Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                ..Usage::default()
            }),
            cost_usd: None,
        })
    }

    fn build_speech_request(
        &self,
        http: &reqwest::Client,
        _provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &SpeechRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let mut body = json!({
            "model": model,
            "input": req.input,
            "voice": req.voice,
        });
        let obj = body.as_object_mut().expect("body is an object");
        if let Some(format) = req.format {
            // The wire name for raw PCM is `pcm`, not `pcm16`.
            let name = match format {
                crate::types::AudioFormat::Pcm16 => "pcm",
                other => other.wire_name(),
            };
            obj.insert("response_format".into(), json!(name));
        }
        if let Some(speed) = req.speed {
            obj.insert("speed".into(), json!(speed));
        }
        if let Some(instructions) = &req.instructions {
            obj.insert("instructions".into(), json!(instructions));
        }
        for (key, value) in &req.extra {
            obj.insert(key.clone(), value.clone());
        }
        Ok(post(http, provider, "/audio/speech").json(&body))
    }

    fn build_transcription_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &TranscriptionRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let file = reqwest::multipart::Part::bytes(req.audio.to_vec())
            .file_name(format!("audio.{}", audio_extension(&req.media_type)))
            .mime_str(&req.media_type)
            .map_err(|_| {
                Error::InvalidConfig(format!(
                    "provider `{provider_key}`: `{}` is not a valid media type",
                    req.media_type
                ))
            })?;
        // A form cannot override a repeated field the way a JSON map does,
        // so a built-in field yields to `extra` under the same name.
        let mut form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("model", model.to_owned());
        if let Some(language) = &req.language
            && !req.extra.contains_key("language")
        {
            form = form.text("language", language.clone());
        }
        if let Some(prompt) = &req.prompt
            && !req.extra.contains_key("prompt")
        {
            form = form.text("prompt", prompt.clone());
        }
        if !req.extra.contains_key("response_format") {
            if req.timestamps {
                // Segment and word timestamps need `verbose_json` — on
                // OpenAI a whisper-1 feature; the gpt-4o transcribe models
                // reject it.
                form = form
                    .text("response_format", "verbose_json")
                    .text("timestamp_granularities[]", "segment")
                    .text("timestamp_granularities[]", "word");
            } else {
                form = form.text("response_format", "json");
            }
        }
        for (key, value) in &req.extra {
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            form = form.text(key.clone(), text);
        }
        Ok(post(http, provider, "/audio/transcriptions").multipart(form))
    }

    fn parse_transcription_response(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<TranscriptionResponse, Error> {
        let wire: WireTranscription =
            serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("failed to decode transcription: {e}"),
            })?;
        Ok(TranscriptionResponse {
            provider: provider_key.to_owned(),
            model: model.to_owned(),
            text: wire.text,
            segments: wire
                .segments
                .unwrap_or_default()
                .into_iter()
                .map(|s| TranscriptSegment {
                    start_s: s.start,
                    end_s: s.end,
                    text: s.text,
                })
                .collect(),
            words: wire
                .words
                .unwrap_or_default()
                .into_iter()
                .map(|w| TranscriptWord {
                    start_s: w.start,
                    end_s: w.end,
                    word: w.word,
                })
                .collect(),
            language: wire.language,
            duration_s: wire.duration,
        })
    }

    fn build_video_create_request(
        &self,
        http: &reqwest::Client,
        provider_key: &str,
        provider: &ProviderConfig,
        model: &str,
        req: &VideoRequest,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let size = match &req.size {
            None => None,
            Some(size) => Some(size.pixel_string().ok_or_else(|| {
                Error::InvalidConfig(format!(
                    "provider `{provider_key}` takes pixel sizes; use SizeSpec::Pixels"
                ))
            })?),
        };
        if let Some(input) = &req.input_image {
            // A first-frame reference makes the request multipart.
            // A form cannot override a repeated field the way a JSON map
            // does, so a built-in field yields to `extra` under the same
            // name.
            let mut form = reqwest::multipart::Form::new()
                .text("model", model.to_owned())
                .text("prompt", req.prompt.clone())
                .part("input_reference", image_part(provider_key, input)?);
            if let Some(seconds) = req.seconds
                && !req.extra.contains_key("seconds")
            {
                form = form.text("seconds", seconds.to_string());
            }
            if let Some(size) = size
                && !req.extra.contains_key("size")
            {
                form = form.text("size", size);
            }
            for (key, value) in &req.extra {
                let text = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                form = form.text(key.clone(), text);
            }
            return Ok(post(http, provider, "/videos").multipart(form));
        }
        let mut body = json!({"model": model, "prompt": req.prompt});
        let obj = body.as_object_mut().expect("body is an object");
        if let Some(seconds) = req.seconds {
            // The API takes seconds as a string enum.
            obj.insert("seconds".into(), json!(seconds.to_string()));
        }
        if let Some(size) = size {
            obj.insert("size".into(), json!(size));
        }
        for (key, value) in &req.extra {
            obj.insert(key.clone(), value.clone());
        }
        Ok(post(http, provider, "/videos").json(&body))
    }

    fn parse_video_job(
        &self,
        provider_key: &str,
        model: &str,
        body: &[u8],
    ) -> Result<VideoJob, Error> {
        let wire: WireVideoJob =
            serde_json::from_slice(body).map_err(|e| Error::InvalidResponse {
                provider: provider_key.to_owned(),
                message: format!("failed to decode video job: {e}"),
            })?;
        let status = match wire.status.as_deref() {
            Some("queued") => VideoStatus::Queued,
            Some("in_progress") => VideoStatus::InProgress,
            Some("completed") => VideoStatus::Completed,
            Some("failed") => VideoStatus::Failed,
            Some("canceled") | Some("cancelled") => VideoStatus::Canceled,
            other => {
                return Err(Error::InvalidResponse {
                    provider: provider_key.to_owned(),
                    message: format!("unknown video job status: {other:?}"),
                });
            }
        };
        Ok(VideoJob {
            id: wire.id,
            provider: provider_key.to_owned(),
            model: wire.model.unwrap_or_else(|| model.to_owned()),
            status,
            progress: wire.progress,
            created_at: wire.created_at,
            expires_at: wire.expires_at,
            video_url: None,
            error: wire.error.map(|e| e.message),
        })
    }

    fn build_video_status_request(
        &self,
        http: &reqwest::Client,
        _provider_key: &str,
        provider: &ProviderConfig,
        native_id: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        validate_video_id(native_id)?;
        Ok(get(http, provider, &format!("/videos/{native_id}")))
    }

    fn build_video_content_request(
        &self,
        http: &reqwest::Client,
        _provider_key: &str,
        provider: &ProviderConfig,
        native_id: &str,
        _job: &VideoJob,
    ) -> Result<reqwest::RequestBuilder, Error> {
        validate_video_id(native_id)?;
        Ok(get(http, provider, &format!("/videos/{native_id}/content")))
    }
}

/// Job ids are caller input and become one URL path segment; reject
/// anything that would rewrite the request path or query.
fn validate_video_id(native_id: &str) -> Result<(), Error> {
    if native_id.is_empty() || native_id.contains(['/', '?', '#']) || native_id == ".." {
        return Err(Error::InvalidConfig(format!(
            "`{native_id}` is not a valid video job id"
        )));
    }
    Ok(())
}

/// A multipart file part for an input image. Only base64 inputs work here:
/// fetching a URL is I/O, which codecs do not perform.
fn image_part(provider_key: &str, input: &ImageInput) -> Result<reqwest::multipart::Part, Error> {
    match input {
        ImageInput::B64 { data, media_type } => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| {
                    Error::InvalidConfig(format!(
                        "provider `{provider_key}`: input image is not valid base64: {e}"
                    ))
                })?;
            reqwest::multipart::Part::bytes(bytes)
                .file_name(format!("image.{}", image_extension(media_type)))
                .mime_str(media_type)
                .map_err(|_| {
                    Error::InvalidConfig(format!(
                        "provider `{provider_key}`: `{media_type}` is not a valid media type"
                    ))
                })
        }
        ImageInput::Url { .. } => Err(Error::InvalidConfig(format!(
            "provider `{provider_key}` takes input images as base64 bytes, not URLs"
        ))),
    }
}

fn image_extension(media_type: &str) -> &str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

fn image_media_type(output_format: &str) -> &str {
    match output_format {
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        other => match other.starts_with("image/") {
            true => other,
            false => "application/octet-stream",
        },
    }
}

/// The filename extension for an audio media type. Servers key format
/// detection on it (OpenAI rejects unknown extensions).
fn audio_extension(media_type: &str) -> &str {
    match media_type {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/webm" | "video/webm" => "webm",
        _ => "bin",
    }
}

/// A POST to the provider. An empty api_key sends no auth header — local
/// servers (Ollama, llama.cpp) ignore auth.
pub(super) fn post(
    http: &reqwest::Client,
    provider: &ProviderConfig,
    path: &str,
) -> reqwest::RequestBuilder {
    with_auth(http.post(format!("{}{path}", provider.base_url)), provider)
}

/// A GET to the provider, with the same auth behavior as [`post`].
pub(super) fn get(
    http: &reqwest::Client,
    provider: &ProviderConfig,
    path: &str,
) -> reqwest::RequestBuilder {
    with_auth(http.get(format!("{}{path}", provider.base_url)), provider)
}

fn with_auth(
    mut request: reqwest::RequestBuilder,
    provider: &ProviderConfig,
) -> reqwest::RequestBuilder {
    if !provider.api_key.is_empty() {
        request = request.bearer_auth(&provider.api_key);
    }
    for (name, value) in &provider.headers {
        request = request.header(name, value);
    }
    request
}

fn encode_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| match m.role {
            Role::System => json!({"role": "system", "content": m.text_content()}),
            Role::User => json!({"role": "user", "content": encode_user_content(&m.content)}),
            Role::Assistant => {
                let mut out = json!({"role": "assistant"});
                let obj = out.as_object_mut().expect("object");
                let text = m.text_content();
                if !text.is_empty() {
                    obj.insert("content".into(), json!(text));
                }
                // Prior assistant audio replays by id.
                if let Some(id) = m.content.iter().find_map(|p| match p {
                    ContentPart::OutputAudio { id: Some(id), .. } => Some(id),
                    _ => None,
                }) {
                    obj.insert("audio".into(), json!({"id": id}));
                }
                if !m.tool_calls.is_empty() {
                    let calls: Vec<Value> = m
                        .tool_calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {"name": c.name, "arguments": c.arguments},
                            })
                        })
                        .collect();
                    obj.insert("tool_calls".into(), json!(calls));
                }
                out
            }
            Role::Tool => json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id,
                "content": m.text_content(),
            }),
        })
        .collect()
}

/// Text-only user content encodes as a plain string; anything else as parts.
/// Reasoning parts have no user-content wire form and drop out.
fn encode_user_content(parts: &[ContentPart]) -> Value {
    let text_only = parts.iter().all(|p| matches!(p, ContentPart::Text { .. }));
    if text_only {
        let text: String = parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        return json!(text);
    }
    let encoded: Vec<Value> = parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text } => Some(json!({"type": "text", "text": text})),
            ContentPart::ImageUrl { url } => {
                Some(json!({"type": "image_url", "image_url": {"url": url}}))
            }
            ContentPart::InputAudio { data, format } => Some(json!({
                "type": "input_audio",
                "input_audio": {"data": data, "format": format.wire_name()},
            })),
            ContentPart::Reasoning { .. }
            | ContentPart::RedactedReasoning { .. }
            | ContentPart::OutputAudio { .. } => None,
        })
        .collect();
    json!(encoded)
}

/// The `reasoning_effort` level: an explicit effort wins; a bare token budget
/// converts by size.
pub(super) fn effort_name(reasoning: &crate::types::ReasoningConfig) -> &'static str {
    use crate::types::ReasoningEffort::*;
    let effort = reasoning.effort.unwrap_or(match reasoning.max_tokens {
        Some(0..=2048) => Low,
        Some(8193..) => High,
        _ => Medium,
    });
    match effort {
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}

fn map_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("tool_calls") | Some("function_call") => FinishReason::ToolCalls,
        Some("content_filter") => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

pub(super) fn extract_error_fields(raw: Option<&Value>) -> (Option<String>, Option<String>) {
    let Some(raw) = raw else {
        return (None, None);
    };
    let error = raw.get("error").unwrap_or(raw);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| error.get("type").and_then(Value::as_str))
        .map(str::to_owned);
    (message, code)
}

pub(super) fn classify(status: u16, code: Option<&str>, message: &str) -> ErrorKind {
    let text = format!("{} {}", code.unwrap_or(""), message).to_lowercase();
    match status {
        401 | 403 => ErrorKind::Authentication,
        429 => ErrorKind::RateLimit,
        400..=499 => {
            if text.contains("context_length") || text.contains("maximum context") {
                ErrorKind::ContextLength
            } else if text.contains("content_policy") || text.contains("content management") {
                ErrorKind::ContentFilter
            } else {
                ErrorKind::InvalidRequest
            }
        }
        503 | 529 => ErrorKind::Overloaded,
        500..=599 => ErrorKind::Server,
        _ => ErrorKind::Unknown,
    }
}

#[derive(Deserialize)]
struct WireResponse {
    model: Option<String>,
    choices: Vec<WireChoice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Option<Vec<WireToolCall>>,
    audio: Option<WireAudio>,
}

#[derive(Deserialize, Default)]
struct WireAudio {
    id: Option<String>,
    data: Option<String>,
    transcript: Option<String>,
    expires_at: Option<u64>,
}

#[derive(Deserialize)]
struct WireToolCall {
    id: Option<String>,
    function: WireFunction,
}

#[derive(Deserialize, Default)]
struct WireFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    prompt_tokens_details: Option<WirePromptDetails>,
    completion_tokens_details: Option<WireCompletionDetails>,
}

#[derive(Deserialize)]
struct WirePromptDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Deserialize)]
struct WireCompletionDetails {
    #[serde(default)]
    reasoning_tokens: u64,
}

impl From<WireUsage> for Usage {
    fn from(w: WireUsage) -> Self {
        Usage {
            input_tokens: w.prompt_tokens,
            output_tokens: w.completion_tokens,
            cache_read_input_tokens: w
                .prompt_tokens_details
                .map(|d| d.cached_tokens)
                .unwrap_or(0),
            cache_write_input_tokens: 0,
            reasoning_tokens: w
                .completion_tokens_details
                .map(|d| d.reasoning_tokens)
                .unwrap_or(0),
        }
    }
}

#[derive(Deserialize)]
struct WireEmbeddings {
    model: Option<String>,
    data: Vec<WireEmbedding>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireEmbedding {
    #[serde(default)]
    index: u32,
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct WireImages {
    data: Vec<WireImage>,
    output_format: Option<String>,
    usage: Option<WireImageUsage>,
}

#[derive(Deserialize)]
struct WireImage {
    b64_json: Option<String>,
    url: Option<String>,
    revised_prompt: Option<String>,
}

#[derive(Deserialize)]
struct WireImageUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct WireVideoJob {
    id: String,
    status: Option<String>,
    model: Option<String>,
    progress: Option<u8>,
    created_at: Option<u64>,
    expires_at: Option<u64>,
    error: Option<WireVideoError>,
}

#[derive(Deserialize)]
struct WireVideoError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct WireTranscription {
    text: String,
    language: Option<String>,
    duration: Option<f64>,
    segments: Option<Vec<WireSegment>>,
    words: Option<Vec<WireWord>>,
}

#[derive(Deserialize)]
struct WireSegment {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct WireWord {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    word: String,
}

#[derive(Deserialize)]
struct WireChunk {
    #[serde(default)]
    choices: Vec<WireChunkChoice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChunkChoice {
    delta: WireDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Option<Vec<WireDeltaToolCall>>,
    audio: Option<WireAudio>,
}

#[derive(Deserialize)]
struct WireDeltaToolCall {
    #[serde(default)]
    index: u32,
    id: Option<String>,
    function: Option<WireFunction>,
}
