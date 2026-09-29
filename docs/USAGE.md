# LLM Router — Usage Guide

How to use the `llm-router` crate. Every code block in this guide also
compiles as part of the test suite — see
`crates/llm-router/examples/tour.rs`. For design rationale, read
`docs/DESIGN.md`.

## Contents

1. [Setup](#setup)
2. [Configure the router](#configure-the-router)
3. [Chat](#chat)
4. [Streaming](#streaming)
5. [Tool calls — the agent loop](#tool-calls--the-agent-loop)
6. [Computer use](#computer-use)
7. [Reasoning and prompt caching](#reasoning-and-prompt-caching)
8. [Images and audio in chat](#images-and-audio-in-chat)
9. [Speech and transcription](#speech-and-transcription)
10. [Image generation](#image-generation)
11. [Video generation](#video-generation)
12. [Realtime voice](#realtime-voice)
13. [Embeddings](#embeddings)
14. [Errors, retries, and fallback](#errors-retries-and-fallback)
15. [Cost and model metadata](#cost-and-model-metadata)
16. [Model lists](#model-lists)
17. [Provider quirks and escape hatches](#provider-quirks-and-escape-hatches)

## Setup

Add the crate as a workspace path dependency:

```toml
[dependencies]
llm-router = { path = "crates/llm-router" }
```

The router is async and runs on tokio. It owns one shared
`reqwest::Client`; parallel calls to the same provider multiplex over
pooled HTTP/2 connections.

## Configure the router

A provider is data: a wire protocol, a base URL, and a key. A model
alias names an ordered candidate list; the router tries candidates in
order and falls back on retryable errors.

```rust
use llm_router::{Candidate, ProviderConfig, Router, RouterConfig};

let config = RouterConfig::new()
    .provider("anthropic", ProviderConfig::anthropic(anthropic_key))
    .provider("openai", ProviderConfig::openai(openai_key))
    .model("default", [
        Candidate::new("anthropic", "claude-sonnet-4-6"),
        Candidate::new("openai", "gpt-5"),
    ]);
let router = Router::new(config)?;
```

Constructors exist for the common providers: `openai`,
`openai_responses`, `openrouter`, `anthropic`, `elevenlabs`, `deepgram`,
`veo`. The OpenRouter constructor uses its Responses API. Use
`ProviderConfig::new(protocol, base_url, api_key)` for anything else.
Groq, Mistral, xAI, Together, and compatible local servers can use
`ProtocolKind::OpenAiChat`.

Rules:

- Base URLs must be `https`, or `http` only to a loopback host.
- API keys never serialize and never print in `Debug` output. Store
  keys outside serialized config and set `api_key` after loading.
- Any request can skip aliases with a direct `provider/model` id, e.g.
  `"openai/gpt-5-mini"`.
- An OpenRouter model id includes its model provider, e.g.
  `"openrouter/openai/gpt-5.6"`.
- To share one connection pool across several routers, build them with
  `Router::with_client(client.clone(), config)`.

A local model needs one provider entry:

```rust
let config = RouterConfig::new().provider(
    "ollama",
    ProviderConfig::new(ProtocolKind::OpenAiChat, "http://localhost:11434/v1", ""),
);
```

## Chat

```rust
use llm_router::{ChatRequest, Message};

let response = router
    .chat(&ChatRequest::new("default", vec![
        Message::system("You are terse."),
        Message::user("Why is the sky blue?"),
    ]))
    .await?;
println!("{}", response.message.text_content());
println!("cost: {:?} USD", response.cost_usd);
```

`ChatRequest` carries the neutral parameters: `temperature`, `top_p`,
`max_tokens`, `stop`, `tools`, `tool_choice`, `reasoning`, `cache`,
`modalities`, `audio`, and an `extra` map for anything
provider-specific. `ChatResponse` reports which provider and concrete
model served the request, the normalized `finish_reason` (plus the
provider's raw value), and non-overlapping token usage.

## Streaming

`chat_stream` returns a `ChatStream`: the serving candidate plus a
stream of flat, tagged events. Rebuild the assistant message with
`MessageAccumulator` — it absorbs the per-protocol stream quirks.

```rust
use futures::StreamExt;
use llm_router::{MessageAccumulator, StreamEvent};

let mut stream = router.chat_stream(&request).await?;
let mut acc = MessageAccumulator::new();
while let Some(event) = stream.events.next().await {
    let event = event?;
    if let StreamEvent::TextDelta { text } = &event {
        print!("{text}");
    }
    acc.push(&event);
}
let streamed = acc.finish();
// streamed.message is ready to append to the conversation.
```

Events: `TextDelta`, `ReasoningDelta`, `ReasoningEnd`,
`RedactedReasoning`, `AudioDelta`, `AudioTranscriptDelta`, `AudioEnd`,
`ToolCallStart`, `ToolCallDelta`, `Finish` (reason + usage). Drop the
stream to cancel the request. Fallback happens only before the first
event; after that, errors surface in-band.

## Tool calls — the agent loop

Define tools with a JSON Schema. Execute the calls the model makes,
append the results, and call again until the model stops asking.

```rust
use llm_router::{FinishReason, Tool};
use serde_json::json;

let mut request = ChatRequest::new("default", vec![Message::user("Weather in Paris?")]);
request.tools = vec![Tool::function(
    "get_weather",
    "Get the current weather for a city",
    json!({
        "type": "object",
        "properties": {"city": {"type": "string"}},
        "required": ["city"],
    }),
)];

loop {
    let response = router.chat(&request).await?;
    request.messages.push(response.message.clone());
    if response.finish_reason != FinishReason::ToolCalls {
        break;
    }
    for call in &response.message.tool_calls {
        let result = run_my_tool(&call.name, &call.arguments);
        request.messages.push(Message::tool(&call.id, result));
    }
}
```

Rules:

- Append the assistant message unmodified. Providers that sign
  reasoning reject altered or missing blocks.
- A tool result can carry images: build a `Message` with
  `Role::Tool`, a `tool_call_id`, and `ContentPart::ImageUrl` parts.
- `ToolCall.arguments` always parses as JSON; zero-argument calls
  normalize to `{}`.

## Computer use

Declare the provider's computer tool with
`Tool::provider_defined(kind, name, config)`. The kind strings are the
provider's own and pass through as data — new tool revisions need no
router change.

Anthropic (`ProviderConfig::anthropic`); set the beta header on the
provider:

```rust
let mut provider = ProviderConfig::anthropic(key);
provider.headers.insert("anthropic-beta".into(), "computer-use-2025-11-24".into());

let mut config = serde_json::Map::new();
config.insert("display_width_px".into(), json!(1280));
config.insert("display_height_px".into(), json!(800));
request.tools = vec![
    Tool::provider_defined("computer_20251124", "computer", config),
    Tool::provider_defined("bash_20250124", "bash", Default::default()),
];
```

OpenAI (`ProviderConfig::openai_responses` — computer use lives on the
Responses API, not Chat Completions):

```rust
request.tools = vec![Tool::provider_defined("computer", "computer", Default::default())];
```

OpenRouter uses the same tool shape through
`ProviderConfig::openrouter`. Tool support still depends on the routed
model. No provider's model list says which models take the tool. Pagis
offers the native tool to every OpenAI model, directly or as OpenRouter's
`openai/...`. A model that does not take it answers with a 400 on `tools`
("Tool 'computer' is not supported with <model>."), and Pagis repeats the
turn with a portable `computer` function tool and keeps using it for that
model. Every other OpenRouter model gets the portable tool. That tool has
one shape for each action, under `action`, and returns the settled
screenshot with the function result. The routed model must support image
input and function tools for this loop to work.

The loop is the regular tool loop with three specifics:

```rust
for call in &response.message.tool_calls {
    // 1. Read the action with the wire wrapper removed. The action
    //    vocabulary stays the provider's own.
    let action = call.computer_action();
    let screenshot_png = execute_on_desktop(&action);
    // 2. Surface pending safety checks (openai-responses puts them in
    //    the call arguments) to the user BEFORE replying — sending the
    //    result acknowledges them.
    // 3. Reply with the screenshot as an image part.
    request.messages.push(Message {
        role: Role::Tool,
        content: vec![ContentPart::ImageUrl {
            url: format!("data:image/png;base64,{screenshot_png}"),
        }],
        tool_calls: vec![],
        tool_call_id: Some(call.id.clone()),
    });
}
```

On openai-responses, both native and portable computer calls surface with
the name `computer`. The native call arguments contain the provider's
`computer_call` wrapper. The portable call arguments contain one action
under `action`.

## Reasoning and prompt caching

```rust
use llm_router::{CachePolicy, ReasoningConfig, ReasoningEffort};

request.reasoning = Some(ReasoningConfig {
    effort: Some(ReasoningEffort::High),
    max_tokens: None, // or a thinking-token budget; the codec converts
});
request.cache = CachePolicy::Auto;
```

`reasoning` maps to `reasoning_effort` (openai protocols) or a thinking
budget (anthropic). Reasoning comes back as `ContentPart::Reasoning`
parts and `ReasoningDelta` events; pass them back unmodified in tool
loops. `CachePolicy::Auto` places cache hints at the stable prefix
boundaries on Anthropic — the biggest cost lever for agent loops;
providers with implicit caching ignore it.

## Images and audio in chat

Image input takes an `https` URL or a `data:` URI (local servers need
`data:` URIs):

```rust
Message {
    role: Role::User,
    content: vec![
        ContentPart::Text { text: "What is in this picture?".into() },
        ContentPart::ImageUrl { url: "data:image/png;base64,...".into() },
    ],
    tool_calls: vec![],
    tool_call_id: None,
}
```

Audio in and out (openai-chat protocol; anthropic reports audio as
unsupported):

```rust
use llm_router::{AudioFormat, AudioOut, Modality};

request.modalities = vec![Modality::Text, Modality::Audio];
request.audio = Some(AudioOut { voice: "alloy".into(), format: AudioFormat::Mp3 });
request.messages.push(Message {
    role: Role::User,
    content: vec![ContentPart::InputAudio {
        data: base64_wav_bytes,
        format: AudioFormat::Wav,
    }],
    tool_calls: vec![],
    tool_call_id: None,
});
```

The reply carries a `ContentPart::OutputAudio` with base64 audio, a
transcript, and an id. Pass the assistant message back unmodified —
the provider replays the audio by id in later turns. Streamed audio
arrives as `AudioDelta`/`AudioTranscriptDelta` events; the accumulator
rebuilds the part. When you stream, set
`format: AudioFormat::Pcm16` — OpenAI streams chat audio as raw PCM
only (`wav` works non-streaming only).

## Speech and transcription

Text to speech returns buffered bytes with their media type. The
openai-chat protocol covers OpenAI, Groq, and compatible servers;
`ProviderConfig::elevenlabs` adds ElevenLabs (the voice field is the
ElevenLabs voice id).

```rust
use llm_router::SpeechRequest;

let mut req = SpeechRequest::new("tts", "Hello there.", "alloy");
req.format = Some(AudioFormat::Mp3);
let speech = router.speech(&req).await?;
std::fs::write("hello.mp3", &speech.audio)?;
```

Speech to text takes raw bytes plus their media type.
`ProviderConfig::deepgram` adds Deepgram.

```rust
use llm_router::TranscriptionRequest;

let mut req = TranscriptionRequest::new("stt", audio_bytes, "audio/wav");
req.timestamps = true; // OpenAI: whisper-1 only; Deepgram: words come anyway
let transcript = router.transcribe(&req).await?;
println!("{}", transcript.text);
```

## Image generation

```rust
use llm_router::{ImageData, ImageInput, ImageRequest, SizeSpec};

let mut req = ImageRequest::new("image", "a lighthouse at dawn");
req.size = Some(SizeSpec::pixels(1024, 1024));
req.quality = Some("high".into());
let response = router.generate_image(&req).await?;
for image in &response.images {
    if let ImageData::B64 { data } = &image.image {
        // decode and save
    }
}
```

Set `input_images` (base64) and optionally `mask` to edit or draw from
reference images — the router routes to the provider's edits endpoint.
Sizes are pixels on OpenAI-family providers and aspect ratio + tier
(`SizeSpec::Aspect`) on Gemini-family ones; the codec rejects the wrong
convention with a clear error instead of guessing.

## Video generation

Video is an async job: submit, poll, fetch.

```rust
use llm_router::{VideoRequest, VideoStatus};

let mut req = VideoRequest::new("video", "a paper boat in the rain");
req.seconds = Some(8);
let job = router.create_video(&req).await?;

let job = loop {
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let job = router.video_status(&job.id).await?;
    match job.status {
        VideoStatus::Queued | VideoStatus::InProgress => continue,
        _ => break job,
    }
};
if job.status == VideoStatus::Completed {
    let bytes = router.video_content(&job.id).await?;
    std::fs::write("clip.mp4", &bytes)?;
}
```

The job id is router-scoped and opaque; store it as-is. Sora
(`ProviderConfig::openai`, pixel sizes) and Veo (`ProviderConfig::veo`,
aspect sizes) sit behind the same three calls. `video_content` on a job
without an artifact returns `Error::JobNotReady`.

## Realtime voice

Realtime voice is an authenticated passthrough: the router resolves the alias,
opens the provider's WebSocket with credentials and fallback, and hands
you the socket. The wire dialect is the provider's own OpenAI Realtime
events (OpenAI and Kyutai Unmute).

```rust
use llm_router::RealtimeMessage;

let mut session = router.realtime_connect("voice").await?;
session
    .send(RealtimeMessage::text(r#"{"type":"session.update","session":{}}"#))
    .await?;
while let Some(frame) = session.next().await {
    let frame = frame?;
    // parse the OpenAI Realtime event JSON
}
```

For full-duplex audio, split the public `socket` field with
`futures::StreamExt::split` and pump both directions concurrently.
After the socket opens, the router is out of the path.

## Embeddings

```rust
use llm_router::EmbeddingsRequest;

let response = router
    .embed(&EmbeddingsRequest::new("embed", vec!["hello".to_owned()]))
    .await?;
let vector: &Vec<f32> = &response.embeddings[0];
```

## Errors, retries, and fallback

Every method runs the same loop: try the alias's candidates in order,
retry transient failures per candidate with doubling backoff, and fall
through on retryable errors. Deterministic rejections — an unsupported
feature, bad credentials — skip to the next candidate at once, so an
alias can mix providers with different capabilities. Non-retryable
errors (invalid request, content filter) return immediately. When all
candidates fail, you get `Error::Exhausted` wrapping the last error.

```rust
use llm_router::{Error, ErrorKind};

match router.chat(&request).await {
    Ok(response) => { /* ... */ }
    Err(Error::Provider { kind: ErrorKind::ContextLength, .. }) => { /* compact */ }
    Err(Error::Exhausted { last, .. }) => { /* every candidate failed */ }
    Err(other) => { /* config errors, transport, ... */ }
}
```

Observe the loop (logging, a "retrying…" UI) with a hook:

```rust
let router = Router::new(config)?.on_attempt(|info| {
    if let Some(error) = info.error {
        eprintln!("attempt {} on {} failed: {error}", info.attempt, info.provider);
    }
});
```

Timeouts (`RouterConfig::timeouts`): `connect`, `request`
(non-streaming total), `first_event` (streaming; triggers fallback),
`idle` (streaming; in-band error). Retry policy is
`RouterConfig::retry`.

## Cost and model metadata

Non-streaming responses carry `cost_usd` when the concrete model is in
the built-in registry (curated, price-verified) or in
`RouterConfig::model_info` overrides. Price a streamed response from
its final usage:

```rust
if let Some(usage) = streamed.usage {
    let cost = router.cost_usd(&stream.model, &usage);
}
```

`router.model_info("claude-sonnet-4-6")` exposes context window, max
output tokens, and per-Mtok prices (`info.prices`).

## Model lists

`list_models` asks one provider for the models its key can use. It is
one call to the provider's list endpoint: it generates nothing and costs
nothing, so it also proves a key. A refused key returns
`Error::Provider` (401 or 403) with the provider's message; a protocol
with no list endpoint returns `Error::Unsupported`.

```rust
let listed = router.list_models("openrouter").await?;
for model in &listed {
    // The provider's report, then the table, then the default limits.
    let metadata = router.model_metadata(&model.id, Some(model));
    println!(
        "{}: {} tokens, {:?} USD per Mtok in",
        model.id,
        metadata.context_window,
        metadata.prices.map(|prices| prices.input_cost),
    );
}
```

Rules:

- The list is newest first where the provider reports a release order.
- Each `ListedModel` field the provider does not report is `None`.
- `metadata.prices` is `None` when no layer prices the model: the cost
  is unknown, never zero.
- The table (`models.json`) describes models; it does not limit them. A
  model the table does not know gets `DEFAULT_CONTEXT_WINDOW` and
  `DEFAULT_MAX_OUTPUT_TOKENS`.

## Provider quirks and escape hatches

- `ChatRequest.extra` merges provider-specific params into the wire
  body last, so it also overrides encoded fields. The media request
  types have the same field.
- `ProviderConfig.headers` adds per-provider headers (beta flags,
  gateway attribution, proxy auth).
- `ProviderConfig.compat` holds wire-level knobs for strict
  OpenAI-compatible servers (`stream_options`, `max_tokens_field`,
  `reasoning_effort`).

Known per-server deviations (Ollama, Mistral, xAI, api.openai.com) are
listed in `docs/DESIGN.md` → "Provider compatibility notes".
