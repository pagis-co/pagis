# LLM Router — Design

This document states the design of the LLM router. The design follows the patterns
of LiteLLM, Traceloop Hub, OpenCode, OpenRouter, and Rust projects (TensorZero,
Helicone, genai, rig).

## Goals

- A lightweight, high-performance Rust library. The Pagis daemon calls it directly.
  No HTTP server in the core.
- One neutral interface over many providers and modalities: text, agentic (tool
  calls), multi-modal input, speech, image generation, video and embeddings.
- Routing policy: model aliases, ordered fallbacks, and error classification.

## Decisions and their sources

1. **A library, not a service.** TensorZero, Helicone, and Hub are standalone
   binaries. The consumer is a Rust daemon, so the core is a library crate.
2. **Protocol codecs, not per-provider clients** (OpenCode native runtime,
   TensorZero). A small set of wire protocols covers almost all providers:
   - `openai-chat` (OpenAI-compatible): OpenAI, Groq, DeepSeek, xAI, Together,
     Mistral, Ollama, vLLM, Gemini-compat, and most aggregators.
   - `anthropic-messages`: Anthropic.
   - `openai-responses`: the OpenAI Responses API and OpenRouter.
   The media protocols are `elevenlabs`, `deepgram` and `veo`. A provider entry
   is data: protocol + base URL + auth + model ids.
3. **Neutral internal types, OpenAI-shaped semantics** (LiteLLM, OpenRouter, Hub).
   Requests use one `ChatRequest` type whose fields map 1:1 to OpenAI chat
   semantics. Codecs translate to and from each wire format in code, not config,
   because provider quirks (system-message hoisting, tool-name rules) do not fit
   a declarative mapping.
4. **A flat, tagged event enum is the streaming contract** (OpenCode `LLMEvent`).
   Agent loops consume `TextDelta` / `ToolCall*` / `Finish` events, not raw
   OpenAI chunks. Codecs parse provider SSE with `eventsource-stream` and emit
   this enum.
5. **Normalize both, preserve raw** (OpenRouter). Finish reasons and errors map to
   a closed taxonomy; the raw provider value stays available for debugging.
   Errors classify as retryable or not; that classification drives fallback.
6. **Normalized, non-overlapping usage buckets** (OpenCode): `input_tokens` =
   non-cached + cache-read + cache-write; `reasoning_tokens` ⊂ `output_tokens`.
   Cost accounting across providers needs buckets that do not overlap.
7. **Model alias → ordered candidate list** (LiteLLM model groups, OpenRouter
   `models[]`). The caller names an alias; the router tries candidates in order
   and falls through on retryable errors before the first token. After the first
   streamed token, errors surface in-band; the router does not re-route
   (OpenRouter rule).
8. **Nothing on the hot path but the request** (TensorZero, LiteLLM).
   No synchronous accounting, logging, or storage in the request path.
9. **The provider's list names the models; the table only describes them**
   (Open WebUI, LibreChat, Zed). `Router::list_models` reads each chat
   protocol's list endpoint (`GET {base_url}/models`; OpenRouter
   `/models/user`, because its public list answers any key) and answers
   `ListedModel`s: the id, and the context window, output limit and
   prices where the provider reports them. A protocol with no list
   endpoint answers `Unsupported`. `ModelMetadata::layered` combines the
   layers field by field: the provider's report, then `models.json` (and
   config overrides), then a conservative default of a 128,000-token
   context window and a 4,096-token output limit. `models.json` is the
   fallback and the price source, not the list of allowed models, so a
   model that a provider adds after a Pagis release still runs. A price has no default:
   no layer means an unknown cost (`None`), never zero (the LiteLLM rule).
   A successful list call also proves a key without a generation.

## Shape

```
crates/llm-router/
  src/
    lib.rs
    error.rs        # Error taxonomy + retryable classification
    types.rs        # ChatRequest, Message, ContentPart, Tool, ChatResponse,
                    # Usage, FinishReason, StreamEvent, the media types
    config.rs       # RouterConfig: providers (protocol, base_url, api_key),
                    # models (alias -> ordered candidates)
    router.rs       # Router::chat / Router::chat_stream, fallback loop
    accumulator.rs  # MessageAccumulator: a stream back into a message
    registry.rs     # the model metadata table (models.json)
    realtime.rs     # realtime WebSocket routing
    protocol/
      mod.rs        # Protocol trait (sync encode/decode; router owns async I/O)
      openai.rs     # openai-chat codec
      anthropic.rs  # anthropic-messages codec
      responses.rs  # openai-responses codec
      model_list.rs # the model list of each protocol
      elevenlabs.rs, deepgram.rs, veo.rs  # the media codecs
```

The `Protocol` trait is sync: it builds a `reqwest` request, parses a response
body, and maps a byte stream to a `StreamEvent` stream. The router owns the shared
`reqwest::Client`, timeouts, and the candidate loop.

## What the router carries

- **Text and tools.** The `openai-chat` and `anthropic-messages` codecs,
  non-streaming and streaming, with tool calls and multi-modal image
  input. Aliases resolve to ordered candidates and fall through on a
  retryable error.
- **Retries and timeouts.** Per-candidate retries with doubling backoff,
  a request timeout for a non-streaming call, a first-event timeout that
  falls through, and an idle timeout that surfaces in band.
- **Reasoning.** One `reasoning` parameter maps to `reasoning_effort` on
  openai-chat and to a thinking budget on anthropic-messages. Reasoning
  parts and stream events round-trip their thinking signatures.
  `CachePolicy::Auto` hints at the system message, the last tool and the
  last message.
- **Cost.** A model metadata registry (`models.json`, curated and
  price-verified, plus config overrides) puts `cost_usd` on a response
  and behind `Router::cost_usd` for a stream. `Router::model_metadata`
  layers a provider's listed entry over it.
- **Model lists.** `Router::list_models` reads the provider's model list
  with its metadata: `max_input_tokens` and `max_tokens` on Anthropic
  (cursor pages, newest first); `id` and `created` on OpenAI, ordered
  newest first; `context_length`, `top_provider.max_completion_tokens`
  and per-token `pricing` on OpenRouter; and `context_window` (Groq),
  `max_context_length` (Mistral) and `max_model_len` (vLLM) on other
  compatible servers. A zero or negative value reads as unreported.
- **Computer use.** `Tool::provider_defined(kind, name, config)` passes a
  provider-defined tool through as data. On anthropic-messages this
  reaches `computer_20251124`, `bash_20250124` and
  `text_editor_20250728`; set the matching `anthropic-beta` header
  through `ProviderConfig::headers`, and a screenshot returns as an image
  part in the tool result. The `openai-responses` protocol
  (`ProviderConfig::openai_responses`) reaches OpenAI computer use:
  stateless `POST /responses`, a `computer_call` item becomes a tool call
  named `computer` that carries the whole item (an item with no action
  gets `actions: [{"type": "screenshot"}]`), a screenshot result
  encodes as `computer_call_output`, and an encrypted reasoning item
  round-trips through a `RedactedReasoning` part. A Chat Completions
  provider rejects a typed tool with `Unsupported`, so a mixed alias
  falls through. A tool-type version string is caller data; the router
  hardcodes none.
- **The other modalities.** Audio in chat, speech synthesis and
  transcription through the `elevenlabs` and `deepgram` codecs, image
  generation with edits and reference inputs, video generation jobs
  through the `veo` codec, and realtime WebSocket endpoint routing.
  `docs/MODALITIES.md` states the rules these follow.

## Not built

- **An OpenAI-compatible server crate.** The only consumer is the Rust
  daemon, which calls the library.
- **A `gemini` codec** for the native Google API, with Gemini TTS and STT
  and chat-bridge image output.
- **Gemini Live, xAI Voice and raw Moshi realtime adapters**, whose event
  and audio contracts differ from the adapters telephony puts behind its
  `ModelSession` seam.
- Load-balancing across equal candidates, `Retry-After` handling, hot
  config reload, streaming TTS and partial-image streaming.

## Provider compatibility notes (local and open-weight models)

The `openai-chat` codec covers OpenAI-compatible servers. The known deviations
and how to handle them:

- **Connection sharing.** One `Router` holds one `reqwest::Client`; parallel
  calls to a provider multiplex over pooled HTTP/2 connections. Use
  `Router::with_client` to share one pool across several routers.
- **Ollama** (`http://localhost:11434/v1`): set any placeholder `api_key`
  (or empty: the codec then sends no auth header). Images must be `data:`
  URIs; Ollama does not fetch URLs. The server ignores `tool_choice`, so do
  not rely on forced tool calls. Parallel tool calls stream with a
  repeated index; the codec re-correlates them by call id.
- **Mistral**: rejects unknown params with 422. Set
  `compat.stream_options: false` and `compat.reasoning_effort: false`.
- **api.openai.com**: requires `max_completion_tokens` on reasoning models;
  `ProviderConfig::openai()` sets `compat.max_tokens_field` accordingly.
- **xAI grok-4**: rejects `reasoning_effort`; set
  `compat.reasoning_effort: false`.
- **Reasoning fields**: the codec reads `reasoning_content` (DeepSeek, vLLM,
  llama.cpp) and `reasoning` (Groq, OpenRouter, Ollama). For open-weight
  reasoning models (DeepSeek-R1, QwQ) served locally, enable the server's
  reasoning parser (vLLM `--reasoning-parser`, llama.cpp
  `--reasoning-format`, default parsers on Ollama/Groq); the router does not
  parse literal `<think>` tags out of content.
- **Escape hatches**: `ChatRequest.extra` merges any provider param into the
  request body last; `ProviderConfig.headers` adds per-provider headers
  (`anthropic-beta`, gateway attribution, proxy auth).
