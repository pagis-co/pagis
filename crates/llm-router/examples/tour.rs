//! Compile-checked mirror of the code in `docs/USAGE.md`.
//!
//! Each function matches one section of the guide. The example never
//! calls the network; `cargo build --examples` verifies that the guide's
//! code uses the real API. Keep this file and the guide in sync.

use futures::StreamExt;
use llm_router::{
    AudioFormat, AudioOut, CachePolicy, Candidate, ChatRequest, ChatStream, ContentPart,
    EmbeddingsRequest, Error, ErrorKind, FinishReason, ImageData, ImageRequest, Message,
    MessageAccumulator, Modality, ProtocolKind, ProviderConfig, RealtimeMessage, ReasoningConfig,
    ReasoningEffort, Role, Router, RouterConfig, SizeSpec, SpeechRequest, StreamEvent,
    StreamedMessage, Tool, TranscriptionRequest, VideoRequest, VideoStatus,
};
use serde_json::json;

// ## Configure the router

fn configure_the_router(anthropic_key: String, openai_key: String) -> Result<Router, Error> {
    let config = RouterConfig::new()
        .provider("anthropic", ProviderConfig::anthropic(anthropic_key))
        .provider("openai", ProviderConfig::openai(openai_key))
        .model(
            "default",
            [
                Candidate::new("anthropic", "claude-sonnet-4-6"),
                Candidate::new("openai", "gpt-5"),
            ],
        );
    Router::new(config)
}

fn local_provider() -> RouterConfig {
    RouterConfig::new().provider(
        "ollama",
        ProviderConfig::new(ProtocolKind::OpenAiChat, "http://localhost:11434/v1", ""),
    )
}

// ## Chat

async fn chat(router: &Router) -> Result<(), Error> {
    let response = router
        .chat(&ChatRequest::new(
            "default",
            vec![
                Message::system("You are terse."),
                Message::user("Why is the sky blue?"),
            ],
        ))
        .await?;
    println!("{}", response.message.text_content());
    println!("cost: {:?} USD", response.cost_usd);
    Ok(())
}

// ## Streaming

async fn streaming(router: &Router, request: &ChatRequest) -> Result<StreamedMessage, Error> {
    let mut stream: ChatStream = router.chat_stream(request).await?;
    let mut acc = MessageAccumulator::new();
    while let Some(event) = stream.events.next().await {
        let event = event?;
        if let StreamEvent::TextDelta { text } = &event {
            print!("{text}");
        }
        acc.push(&event);
    }
    Ok(acc.finish())
}

// ## Tool calls — the agent loop

fn run_my_tool(_name: &str, _arguments: &str) -> String {
    "sunny".to_owned()
}

async fn agent_loop(router: &Router) -> Result<(), Error> {
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
    Ok(())
}

// ## Computer use

fn computer_use_tools(key: String) -> (ProviderConfig, Vec<Tool>, Vec<Tool>) {
    let mut provider = ProviderConfig::anthropic(key);
    provider
        .headers
        .insert("anthropic-beta".into(), "computer-use-2025-11-24".into());

    let mut config = serde_json::Map::new();
    config.insert("display_width_px".into(), json!(1280));
    config.insert("display_height_px".into(), json!(800));
    let anthropic_tools = vec![
        Tool::provider_defined("computer_20251124", "computer", config),
        Tool::provider_defined("bash_20250124", "bash", Default::default()),
    ];
    let responses_tools = vec![Tool::provider_defined(
        "computer",
        "computer",
        Default::default(),
    )];
    (provider, anthropic_tools, responses_tools)
}

fn execute_on_desktop(_action: &serde_json::Value) -> String {
    "iVBORw0KGgo=".to_owned()
}

async fn computer_use_loop(router: &Router, request: &mut ChatRequest) -> Result<(), Error> {
    let response = router.chat(request).await?;
    request.messages.push(response.message.clone());
    for call in &response.message.tool_calls {
        let action = call.computer_action();
        let screenshot_png = execute_on_desktop(&action);
        request.messages.push(Message {
            role: Role::Tool,
            content: vec![ContentPart::ImageUrl {
                url: format!("data:image/png;base64,{screenshot_png}"),
            }],
            tool_calls: vec![],
            tool_call_id: Some(call.id.clone()),
        });
    }
    Ok(())
}

// ## Reasoning and prompt caching

fn reasoning_and_caching(request: &mut ChatRequest) {
    request.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::High),
        max_tokens: None,
    });
    request.cache = CachePolicy::Auto;
}

// ## Images and audio in chat

fn multimodal_input(request: &mut ChatRequest, base64_wav_bytes: String) {
    request.messages.push(Message {
        role: Role::User,
        content: vec![
            ContentPart::Text {
                text: "What is in this picture?".into(),
            },
            ContentPart::ImageUrl {
                url: "data:image/png;base64,...".into(),
            },
        ],
        tool_calls: vec![],
        tool_call_id: None,
    });

    request.modalities = vec![Modality::Text, Modality::Audio];
    request.audio = Some(AudioOut {
        voice: "alloy".into(),
        format: AudioFormat::Mp3,
    });
    request.messages.push(Message {
        role: Role::User,
        content: vec![ContentPart::InputAudio {
            data: base64_wav_bytes,
            format: AudioFormat::Wav,
        }],
        tool_calls: vec![],
        tool_call_id: None,
    });
}

// ## Speech and transcription

async fn speech_and_transcription(
    router: &Router,
    audio_bytes: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut req = SpeechRequest::new("tts", "Hello there.", "alloy");
    req.format = Some(AudioFormat::Mp3);
    let speech = router.speech(&req).await?;
    std::fs::write("hello.mp3", &speech.audio)?;

    let mut req = TranscriptionRequest::new("stt", audio_bytes, "audio/wav");
    req.timestamps = true;
    let transcript = router.transcribe(&req).await?;
    println!("{}", transcript.text);
    Ok(())
}

// ## Image generation

async fn image_generation(router: &Router) -> Result<(), Error> {
    let mut req = ImageRequest::new("image", "a lighthouse at dawn");
    req.size = Some(SizeSpec::pixels(1024, 1024));
    req.quality = Some("high".into());
    let response = router.generate_image(&req).await?;
    for image in &response.images {
        if let ImageData::B64 { data } = &image.image {
            let _ = data;
        }
    }
    Ok(())
}

// ## Video generation

async fn video_generation(router: &Router) -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}

// ## Realtime voice

async fn realtime(router: &Router) -> Result<(), Error> {
    let mut session = router.realtime_connect("voice").await?;
    session
        .send(RealtimeMessage::text(
            r#"{"type":"session.update","session":{}}"#,
        ))
        .await?;
    while let Some(frame) = session.next().await {
        let _frame = frame?;
    }
    Ok(())
}

// ## Embeddings

async fn embeddings(router: &Router) -> Result<(), Error> {
    let response = router
        .embed(&EmbeddingsRequest::new("embed", vec!["hello".to_owned()]))
        .await?;
    let _vector: &Vec<f32> = &response.embeddings[0];
    Ok(())
}

// ## Errors, retries, and fallback

async fn error_handling(router: &Router, request: &ChatRequest) {
    match router.chat(request).await {
        Ok(_response) => {}
        Err(Error::Provider {
            kind: ErrorKind::ContextLength,
            ..
        }) => { /* compact the conversation */ }
        Err(Error::Exhausted { .. }) => { /* every candidate failed */ }
        Err(_other) => {}
    }
}

fn attempt_hook(config: RouterConfig) -> Result<Router, Error> {
    Ok(Router::new(config)?.on_attempt(|info| {
        if let Some(error) = info.error {
            eprintln!(
                "attempt {} on {} failed: {error}",
                info.attempt, info.provider
            );
        }
    }))
}

// ## Cost and model metadata

fn cost(router: &Router, stream: &ChatStream, streamed: &StreamedMessage) -> Option<f64> {
    let info = router.model_info("claude-sonnet-4-6");
    let _ = info;
    streamed
        .usage
        .as_ref()
        .and_then(|usage| router.cost_usd(&stream.model, usage))
}

// ## Model lists

async fn model_lists(router: &Router) -> Result<(), Error> {
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
    Ok(())
}

fn main() {
    // The example exists to compile the guide's code, not to run it.
    let _ = (
        configure_the_router as fn(_, _) -> _,
        local_provider as fn() -> _,
        chat as fn(_) -> _,
        streaming as fn(_, _) -> _,
        agent_loop as fn(_) -> _,
        computer_use_tools as fn(_) -> _,
        computer_use_loop as fn(_, _) -> _,
        reasoning_and_caching as fn(_),
        multimodal_input as fn(_, _),
        speech_and_transcription as fn(_, _) -> _,
        image_generation as fn(_) -> _,
        video_generation as fn(_) -> _,
        realtime as fn(_) -> _,
        embeddings as fn(_) -> _,
        error_handling as fn(_, _) -> _,
        attempt_hook as fn(_) -> _,
        cost as fn(_, _, _) -> _,
        model_lists as fn(_) -> _,
    );
    println!("The tour compiles. Read docs/USAGE.md alongside this file.");
}
