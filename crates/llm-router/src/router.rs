//! The router: alias resolution and the retry/fallback loop.

use futures::StreamExt;
use rand::Rng;

use crate::config::{Candidate, ProviderConfig, RouterConfig};
use crate::error::Error;
use crate::protocol::{EventStream, Protocol, codec};
use crate::realtime::RealtimeIntent;
use crate::registry::{self, ListedModel, ModelInfo, ModelMetadata};
use crate::stream_util::with_idle_timeout;
use crate::types::{
    ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse, ImageRequest, ImageResponse,
    SpeechRequest, SpeechResponse, StreamEvent, TranscriptionRequest, TranscriptionResponse, Usage,
    VideoJob, VideoRequest, VideoStatus,
};

pub struct Router {
    http: reqwest::Client,
    config: RouterConfig,
    on_attempt: Option<AttemptHook>,
}

/// A live chat stream and the candidate that serves it.
///
/// Drop the stream to cancel the request; the HTTP connection closes.
pub struct ChatStream {
    pub provider: String,
    pub model: String,
    /// Provider calls absorbed before this stream opened.
    pub retries: u32,
    pub events: EventStream,
}

/// One attempt in the retry/fallback loop, reported to the
/// [`Router::on_attempt`] hook.
#[derive(Debug)]
pub struct AttemptInfo<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// 1-based attempt number on this candidate.
    pub attempt: u32,
    pub elapsed: std::time::Duration,
    /// `None` when the attempt succeeded.
    pub error: Option<&'a Error>,
}

type AttemptHook = std::sync::Arc<dyn Fn(AttemptInfo<'_>) + Send + Sync>;

/// Cap on buffered (non-streaming) response bodies, so a misbehaving
/// provider cannot exhaust memory.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Cap on the pages of one model list, so a provider whose cursor never
/// ends cannot hold the caller in a loop.
const MAX_MODEL_PAGES: usize = 20;

/// Cap on buffered video artifacts, which run larger than JSON bodies.
const MAX_VIDEO_BODY_BYTES: usize = 512 * 1024 * 1024;

impl Router {
    /// Build a router with its own HTTP client. Fails on invalid provider
    /// configuration (see [`ProviderConfig::base_url`]).
    pub fn new(config: RouterConfig) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .connect_timeout(config.timeouts.connect)
            .http2_keep_alive_interval(std::time::Duration::from_secs(30))
            .http2_keep_alive_while_idle(true)
            .http2_adaptive_window(true)
            .build()
            .map_err(|e| Error::InvalidConfig(format!("failed to build HTTP client: {e}")))?;
        Self::with_client(http, config)
    }

    /// Build a router on a shared [`reqwest::Client`]. `Client` clones share
    /// one connection pool, so pass clones of the same client to every router
    /// (and other HTTP users) in the process to reuse connections across
    /// them.
    pub fn with_client(http: reqwest::Client, config: RouterConfig) -> Result<Self, Error> {
        for (key, provider) in &config.providers {
            validate_base_url(key, &provider.base_url)?;
        }
        Ok(Self {
            http,
            config,
            on_attempt: None,
        })
    }

    /// Observe every attempt in the retry/fallback loop — including the
    /// failures the loop absorbs — for logging, metrics, or a "retrying…"
    /// UI. The hook runs synchronously on the request path; keep it cheap.
    pub fn on_attempt(mut self, hook: impl Fn(AttemptInfo<'_>) + Send + Sync + 'static) -> Self {
        self.on_attempt = Some(std::sync::Arc::new(hook));
        self
    }

    /// One non-streaming chat completion.
    ///
    /// Tries the alias's candidates in order, with per-candidate retries and
    /// backoff on retryable errors. A non-retryable error returns at once.
    /// When every candidate fails, returns [`Error::Exhausted`] with the last
    /// error.
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse, Error> {
        self.run(&req.model, |candidate| self.chat_once(candidate, req))
            .await
            .map(|(response, _)| response)
    }

    /// One streaming chat completion.
    ///
    /// Fallback happens only before the stream opens (an HTTP error status or
    /// a transport failure). After that, errors surface in the event stream
    /// and the router does not re-route.
    ///
    /// Drop the returned stream — or this pending future — to cancel the
    /// request and any remaining retries; the HTTP connection closes.
    /// Rebuild the assistant message from the events with
    /// [`MessageAccumulator`](crate::MessageAccumulator).
    pub async fn chat_stream(&self, req: &ChatRequest) -> Result<ChatStream, Error> {
        self.run(&req.model, |candidate| self.stream_once(candidate, req))
            .await
            .map(|(mut stream, attempts)| {
                stream.retries = attempts.saturating_sub(1);
                stream
            })
    }

    /// One embeddings request, with the same retry/fallback loop as chat.
    pub async fn embed(&self, req: &EmbeddingsRequest) -> Result<EmbeddingsResponse, Error> {
        self.run(&req.model, |candidate| self.embed_once(candidate, req))
            .await
            .map(|(response, _)| response)
    }

    /// One image-generation request, with the same retry/fallback loop as
    /// chat.
    pub async fn generate_image(&self, req: &ImageRequest) -> Result<ImageResponse, Error> {
        self.run(&req.model, |candidate| self.image_once(candidate, req))
            .await
            .map(|(response, _)| response)
    }

    /// One text-to-speech request, with the same retry/fallback loop as chat.
    /// Returns buffered audio bytes with their media type.
    pub async fn speech(&self, req: &SpeechRequest) -> Result<SpeechResponse, Error> {
        self.run(&req.model, |candidate| self.speech_once(candidate, req))
            .await
            .map(|(response, _)| response)
    }

    /// One speech-to-text request, with the same retry/fallback loop as chat.
    pub async fn transcribe(
        &self,
        req: &TranscriptionRequest,
    ) -> Result<TranscriptionResponse, Error> {
        self.run(&req.model, |candidate| self.transcribe_once(candidate, req))
            .await
            .map(|(response, _)| response)
    }

    /// Open a realtime voice session: resolve the alias, connect the
    /// provider's WebSocket with credentials, and return the socket. The
    /// same retry/fallback loop as chat runs on connect failures; after the
    /// socket opens, the router is out of the path.
    ///
    /// The wire dialect is the provider's own — OpenAI Realtime events on
    /// openai-chat providers (OpenAI, Azure, Kyutai Unmute). Providers on
    /// other protocols report realtime as unsupported.
    pub async fn realtime_connect(
        &self,
        model: &str,
    ) -> Result<crate::realtime::RealtimeConnection, Error> {
        self.realtime(model, RealtimeIntent::Conversation).await
    }

    /// Open a transcription-only realtime session for PCM16 mono audio at
    /// `sample_rate` Hz. On OpenAI it is the same socket as
    /// [`Router::realtime_connect`], opened with `intent=transcription`
    /// instead of a model, and the caller picks the transcription model
    /// and the audio format in its `session.update`. On Deepgram it is
    /// `/listen`, with the model and the audio format in the query. The
    /// returned connection's `model` is the candidate that served the
    /// alias, and its `protocol` names the dialect.
    pub async fn realtime_transcription_connect(
        &self,
        model: &str,
        sample_rate: u32,
    ) -> Result<crate::realtime::RealtimeConnection, Error> {
        self.realtime(model, RealtimeIntent::Transcription { sample_rate })
            .await
    }

    async fn realtime(
        &self,
        model: &str,
        intent: RealtimeIntent,
    ) -> Result<crate::realtime::RealtimeConnection, Error> {
        self.run(model, |candidate| async move {
            let (provider, _) = self.provider(&candidate.provider)?;
            let served = match provider.protocol {
                crate::config::ProtocolKind::OpenAiChat => true,
                crate::config::ProtocolKind::Deepgram => {
                    matches!(intent, RealtimeIntent::Transcription { .. })
                }
                _ => false,
            };
            if !served {
                return Err(Error::Unsupported {
                    provider: candidate.provider.clone(),
                    feature: "realtime",
                });
            }
            crate::realtime::connect(
                &candidate.provider,
                provider,
                &candidate.model,
                intent,
                self.config.timeouts.connect,
            )
            .await
        })
        .await
        .map(|(connection, _)| connection)
    }

    /// Submit a video-generation job, with the same retry/fallback loop as
    /// chat. The returned job carries a router-scoped id; poll it with
    /// [`Router::video_status`] and fetch the artifact with
    /// [`Router::video_content`].
    pub async fn create_video(&self, req: &VideoRequest) -> Result<VideoJob, Error> {
        self.run(&req.model, |candidate| {
            self.create_video_once(candidate, req)
        })
        .await
        .map(|(job, _)| job)
    }

    /// Poll a video job by its router-scoped id. No fallback: the id pins the
    /// provider.
    pub async fn video_status(&self, job_id: &str) -> Result<VideoJob, Error> {
        let (provider_key, native_id) = split_job_id(job_id)?;
        let (provider, protocol) = self.provider(provider_key)?;
        let request =
            protocol.build_video_status_request(&self.http, provider_key, provider, native_id)?;
        let candidate = Candidate::new(provider_key, "");
        let body = self.fetch(&candidate, protocol, request).await?;
        let mut job = protocol.parse_video_job(provider_key, "", &body)?;
        job.id = job_id.to_owned();
        Ok(job)
    }

    /// Fetch the artifact of a completed video job as buffered bytes. Polls
    /// the job first; a job without an artifact yet returns
    /// [`Error::JobNotReady`]. Signed artifact URLs (Veo) fetch with the
    /// provider's auth.
    pub async fn video_content(&self, job_id: &str) -> Result<bytes::Bytes, Error> {
        let job = self.video_status(job_id).await?;
        if job.status != VideoStatus::Completed {
            return Err(Error::JobNotReady {
                id: job_id.to_owned(),
                status: job.status,
            });
        }
        let (provider_key, native_id) = split_job_id(job_id)?;
        let (provider, protocol) = self.provider(provider_key)?;
        let request = protocol.build_video_content_request(
            &self.http,
            provider_key,
            provider,
            native_id,
            &job,
        )?;
        let response = request
            .send()
            .await
            .map_err(|e| self.transport(provider_key, e))?;
        let status = response.status();
        let body = self
            .read_body(provider_key, response, MAX_VIDEO_BODY_BYTES)
            .await?;
        if !status.is_success() {
            return Err(protocol.parse_error(provider_key, status.as_u16(), &body));
        }
        Ok(body)
    }

    async fn create_video_once(
        &self,
        candidate: Candidate,
        req: &VideoRequest,
    ) -> Result<VideoJob, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let request = protocol.build_video_create_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            req,
        )?;
        let body = self.fetch(&candidate, protocol, request).await?;
        let mut job = protocol.parse_video_job(&candidate.provider, &candidate.model, &body)?;
        job.id = format!("{}:{}", candidate.provider, job.id);
        job.model = candidate.model;
        Ok(job)
    }

    /// The models the provider's key can use, with the metadata the
    /// provider reports for each: one call to the provider's list
    /// endpoint, which generates nothing and costs nothing. No retry and
    /// no fallback: the list belongs to one provider. The list is newest
    /// first where the provider reports a release order.
    ///
    /// A protocol with no list endpoint returns [`Error::Unsupported`]. A
    /// key the provider refuses returns its [`Error::Provider`] (401 or
    /// 403) with the provider's message.
    pub async fn list_models(&self, provider_key: &str) -> Result<Vec<ListedModel>, Error> {
        let (provider, protocol) = self.provider(provider_key)?;
        let candidate = Candidate::new(provider_key, "");
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..MAX_MODEL_PAGES {
            let request = protocol.build_list_models_request(
                &self.http,
                provider_key,
                provider,
                after.as_deref(),
            )?;
            let body = self.fetch(&candidate, protocol, request).await?;
            let page = protocol.parse_list_models(provider_key, &body)?;
            models.extend(page.models);
            match page.next {
                Some(next) => after = Some(next),
                None => return Ok(models),
            }
        }
        Err(Error::InvalidResponse {
            provider: provider_key.to_owned(),
            message: format!("the model list has more than {MAX_MODEL_PAGES} pages"),
        })
    }

    /// The layered metadata for a concrete model id: `listed` (the
    /// provider's own entry for it, from [`Router::list_models`]), then
    /// the config overrides and the built-in table, then the default
    /// limits. See [`ModelMetadata::layered`].
    pub fn model_metadata(&self, model: &str, listed: Option<&ListedModel>) -> ModelMetadata {
        ModelMetadata::layered(listed, self.model_info(model))
    }

    /// Metadata for a concrete model id: config overrides first, then the
    /// built-in table, by exact id and then longest prefix.
    pub fn model_info(&self, model: &str) -> Option<&ModelInfo> {
        registry::lookup(&self.config.model_info, model)
    }

    /// The USD cost of `usage` on `model`, when the model has metadata.
    /// Use this to price streamed responses from their final `Finish` usage.
    pub fn cost_usd(&self, model: &str, usage: &Usage) -> Option<f64> {
        self.model_info(model).map(|info| info.cost(usage))
    }

    async fn run<T, F, Fut>(&self, model: &str, attempt: F) -> Result<(T, u32), Error>
    where
        F: Fn(Candidate) -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        let candidates = self.candidates(model)?;
        let retry = &self.config.retry;
        let mut last: Option<Error> = None;
        let mut attempts = 0_u32;
        for candidate in candidates {
            let mut backoff = retry.initial_backoff;
            for attempt_number in 1..=retry.max_attempts.max(1) {
                attempts += 1;
                let started = std::time::Instant::now();
                let result = attempt(candidate.clone()).await;
                if let Some(hook) = &self.on_attempt {
                    hook(AttemptInfo {
                        provider: &candidate.provider,
                        model: &candidate.model,
                        attempt: attempt_number,
                        elapsed: started.elapsed(),
                        error: result.as_ref().err(),
                    });
                }
                match result {
                    Ok(value) => return Ok((value, attempts)),
                    Err(e) if e.is_retryable() => {
                        // A deterministic rejection goes straight to the
                        // next candidate; only transient failures retry
                        // this one.
                        let retry_here = e.retries_same_candidate();
                        last = Some(e);
                        if !retry_here {
                            break;
                        }
                        if attempt_number < retry.max_attempts {
                            let jitter = if backoff.is_zero() {
                                std::time::Duration::ZERO
                            } else {
                                backoff.mul_f64(rand::rng().random_range(0.0..=0.25))
                            };
                            tokio::time::sleep(backoff.saturating_add(jitter)).await;
                            backoff = (backoff * 2).min(retry.max_backoff);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Err(Error::Exhausted {
            model: model.to_owned(),
            attempts,
            last: Box::new(last.expect("candidates is never empty")),
        })
    }

    async fn chat_once(
        &self,
        candidate: Candidate,
        req: &ChatRequest,
    ) -> Result<ChatResponse, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let req = tools_for(req, &candidate);
        let request = protocol.build_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            &req,
            false,
        )?;
        let body = self.fetch(&candidate, protocol, request).await?;
        let mut response = protocol.parse_response(&candidate.provider, &candidate.model, &body)?;
        response.cost_usd = self.cost_usd(&candidate.model, &response.usage);
        Ok(response)
    }

    async fn embed_once(
        &self,
        candidate: Candidate,
        req: &EmbeddingsRequest,
    ) -> Result<EmbeddingsResponse, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let request = protocol.build_embeddings_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            req,
        )?;
        let body = self.fetch(&candidate, protocol, request).await?;
        let mut response =
            protocol.parse_embeddings_response(&candidate.provider, &candidate.model, &body)?;
        response.cost_usd = self.cost_usd(&candidate.model, &response.usage);
        Ok(response)
    }

    async fn image_once(
        &self,
        candidate: Candidate,
        req: &ImageRequest,
    ) -> Result<ImageResponse, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let request = protocol.build_image_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            req,
        )?;
        let body = self.fetch(&candidate, protocol, request).await?;
        protocol.parse_image_response(&candidate.provider, &candidate.model, &body)
    }

    async fn speech_once(
        &self,
        candidate: Candidate,
        req: &SpeechRequest,
    ) -> Result<SpeechResponse, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let request = protocol.build_speech_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            req,
        )?;
        // The success body is raw audio; capture the media type before
        // reading it.
        let response = request
            .timeout(self.config.timeouts.request)
            .send()
            .await
            .map_err(|e| self.transport(&candidate.provider, e))?;
        let status = response.status();
        let media_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let body = self.read_capped(&candidate.provider, response).await?;
        if !status.is_success() {
            return Err(protocol.parse_error(&candidate.provider, status.as_u16(), &body));
        }
        Ok(SpeechResponse {
            provider: candidate.provider,
            model: candidate.model,
            audio: body,
            media_type,
        })
    }

    async fn transcribe_once(
        &self,
        candidate: Candidate,
        req: &TranscriptionRequest,
    ) -> Result<TranscriptionResponse, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let request = protocol.build_transcription_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            req,
        )?;
        let body = self.fetch(&candidate, protocol, request).await?;
        protocol.parse_transcription_response(&candidate.provider, &candidate.model, &body)
    }

    /// Send a non-streaming request and return the success body, or the
    /// normalized error.
    async fn fetch(
        &self,
        candidate: &Candidate,
        protocol: &'static dyn Protocol,
        request: reqwest::RequestBuilder,
    ) -> Result<bytes::Bytes, Error> {
        let response = request
            .timeout(self.config.timeouts.request)
            .send()
            .await
            .map_err(|e| self.transport(&candidate.provider, e))?;
        let status = response.status();
        let body = self.read_capped(&candidate.provider, response).await?;
        if !status.is_success() {
            return Err(protocol.parse_error(&candidate.provider, status.as_u16(), &body));
        }
        Ok(body)
    }

    /// Buffer a response body up to [`MAX_BODY_BYTES`].
    async fn read_capped(
        &self,
        provider: &str,
        response: reqwest::Response,
    ) -> Result<bytes::Bytes, Error> {
        self.read_body(provider, response, MAX_BODY_BYTES).await
    }

    /// Buffer a response body up to `cap` bytes, so a misbehaving provider
    /// cannot exhaust memory.
    async fn read_body(
        &self,
        provider: &str,
        response: reqwest::Response,
        cap: usize,
    ) -> Result<bytes::Bytes, Error> {
        let mut stream = response.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| self.transport(provider, e))?;
            if buf.len() + chunk.len() > cap {
                return Err(Error::InvalidResponse {
                    provider: provider.to_owned(),
                    message: format!("response body exceeded {cap} bytes"),
                });
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf.into())
    }

    /// Open a stream and wait for its first event. The whole open phase runs
    /// under the `first_event` timeout, so a stalled candidate falls back.
    async fn stream_once(
        &self,
        candidate: Candidate,
        req: &ChatRequest,
    ) -> Result<ChatStream, Error> {
        let (provider, protocol) = self.provider(&candidate.provider)?;
        let req = tools_for(req, &candidate);
        let request = protocol.build_request(
            &self.http,
            &candidate.provider,
            provider,
            &candidate.model,
            &req,
            true,
        )?;

        let open = async {
            let response = request
                .send()
                .await
                .map_err(|e| self.transport(&candidate.provider, e))?;
            let status = response.status();
            if !status.is_success() {
                let body = self.read_capped(&candidate.provider, response).await?;
                return Err(protocol.parse_error(&candidate.provider, status.as_u16(), &body));
            }
            let mut events =
                protocol.stream_events(&candidate.provider, response.bytes_stream().boxed());
            let first = events.next().await;
            Ok((first, events))
        };
        let (first, rest) = tokio::time::timeout(self.config.timeouts.first_event, open)
            .await
            .map_err(|_| Error::FirstEventTimeout {
                provider: candidate.provider.clone(),
            })??;

        // A stream that errors before its first shown output fails this
        // attempt, so the retry loop can try again or fall back: nothing a
        // caller acts on reached it yet. Reasoning and tool calls come
        // before it, and OpenAI's Responses stream can follow either with a
        // `server_error`, so those events are held until the first text or
        // audio, or the end of the stream. A caller acts on a tool call only
        // when the stream finishes. InvalidResponse is retryable.
        let mut events = with_idle_timeout(
            futures::stream::iter(first).chain(rest).boxed(),
            self.config.timeouts.idle,
            candidate.provider.clone(),
        );
        let mut held = Vec::new();
        loop {
            match events.next().await {
                Some(Ok(event)) if !is_shown(&event) => held.push(Ok(event)),
                // A provider error keeps its status, so a server error
                // retries the same candidate as an HTTP 500 does.
                Some(Err(error @ Error::Provider { .. })) => return Err(error),
                Some(Err(error)) => {
                    return Err(Error::InvalidResponse {
                        provider: candidate.provider.clone(),
                        message: format!("stream failed before its first output: {error}"),
                    });
                }
                next => {
                    held.extend(next);
                    break;
                }
            }
        }
        let events = futures::stream::iter(held).chain(events).boxed();
        Ok(ChatStream {
            provider: candidate.provider,
            model: candidate.model,
            retries: 0,
            events,
        })
    }

    /// Resolve a model alias, or a direct `provider/model` id, to candidates.
    fn candidates(&self, model: &str) -> Result<Vec<Candidate>, Error> {
        if let Some(candidates) = self.config.models.get(model) {
            if candidates.is_empty() {
                return Err(Error::UnknownModel(model.to_owned()));
            }
            return Ok(candidates.clone());
        }
        if let Some((provider, model_id)) = model.split_once('/')
            && self.config.providers.contains_key(provider)
        {
            return Ok(vec![Candidate::new(provider, model_id)]);
        }
        Err(Error::UnknownModel(model.to_owned()))
    }

    fn provider(&self, key: &str) -> Result<(&ProviderConfig, &'static dyn Protocol), Error> {
        let provider = self
            .config
            .providers
            .get(key)
            .ok_or_else(|| Error::UnknownProvider(key.to_owned()))?;
        Ok((provider, codec(provider.protocol)))
    }

    fn transport(&self, provider: &str, source: reqwest::Error) -> Error {
        Error::Transport {
            provider: provider.to_owned(),
            source,
        }
    }
}

/// Split a router-scoped video job id into (provider key, native id).
fn split_job_id(job_id: &str) -> Result<(&str, &str), Error> {
    job_id.split_once(':').ok_or_else(|| {
        Error::InvalidConfig(format!(
            "`{job_id}` is not a router video job id; use the id from create_video"
        ))
    })
}

/// Require `https`, or `http` to a loopback host; reject embedded
/// credentials. Plain `http` to a routable host would send the API key in
/// cleartext.
fn validate_base_url(provider: &str, base_url: &str) -> Result<(), Error> {
    let url = reqwest::Url::parse(base_url).map_err(|e| {
        Error::InvalidConfig(format!("provider `{provider}` base_url `{base_url}`: {e}"))
    })?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::InvalidConfig(format!(
            "provider `{provider}` base_url must not embed credentials"
        )));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_local_host(&url) => Ok(()),
        scheme => Err(Error::InvalidConfig(format!(
            "provider `{provider}` base_url uses `{scheme}://` to a non-local host; \
             use https, or http only for localhost"
        ))),
    }
}

pub(crate) fn is_local_host(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// The request as one candidate sees it: without the tools that belong to
/// another candidate. A request whose tools every candidate takes is not
/// copied.
fn tools_for<'a>(req: &'a ChatRequest, candidate: &Candidate) -> std::borrow::Cow<'a, ChatRequest> {
    if req
        .tools
        .iter()
        .all(|tool| tool.applies_to(&candidate.provider, &candidate.model))
    {
        return std::borrow::Cow::Borrowed(req);
    }
    let mut own = req.clone();
    own.tools
        .retain(|tool| tool.applies_to(&candidate.provider, &candidate.model));
    std::borrow::Cow::Owned(own)
}

/// Whether a caller shows a stream event as it arrives (text and audio),
/// or it is the end of the stream. A caller holds reasoning and tool
/// calls until the stream finishes, so a stream that fails before a
/// shown event can retry.
fn is_shown(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta { .. }
            | StreamEvent::AudioDelta { .. }
            | StreamEvent::AudioTranscriptDelta { .. }
            | StreamEvent::Finish { .. }
    )
}
