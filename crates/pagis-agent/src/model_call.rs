//! One logical model request of a Run, from its summary to its answer.
//!
//! A request publishes `model.requested` before the call and
//! `model.completed` after it, with the same phase and request number
//! (ADR-0010). The answer also writes the Usage Record of the call and,
//! while the Model Request Capture setting is on, the capture of the
//! request and its answer (ADR-0030). Every exit path of a call goes
//! through [`ModelCall::finish`], so none of the three is left out.

use std::time::Instant;

use pagis_core::{ModelRequestCapture, ModelRequestCaptureId, Run};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::brain::{BrainError, TurnEnd, TurnMessage, TurnRequest, TurnRole};
use crate::context_budget::RequestSummary;
use crate::system::AgentDeps;

/// How one model request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The budget check refused the request before the call.
    Rejected,
    Canceled,
    Failed,
    Completed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Rejected => "rejected",
            Outcome::Canceled => "canceled",
            Outcome::Failed => "failed",
            Outcome::Completed => "completed",
        }
    }
}

pub(crate) struct ModelCall<'a> {
    deps: &'a AgentDeps,
    run: &'a Run,
    phase: &'a str,
    phase_request: usize,
    request: &'a TurnRequest,
    started: Instant,
}

impl<'a> ModelCall<'a> {
    /// Publish the summary of the request, before the call.
    pub async fn start(
        deps: &'a AgentDeps,
        run: &'a Run,
        phase: &'a str,
        phase_request: usize,
        request: &'a TurnRequest,
    ) -> Self {
        let mut payload = serde_json::to_value(RequestSummary::of(request, &deps.models))
            .expect("a request summary serializes");
        payload["phase"] = phase.into();
        payload["phase_request"] = phase_request.into();
        crate::run::publish(deps, run, "model.requested", payload).await;
        Self {
            deps,
            run,
            phase,
            phase_request,
            request,
            started: Instant::now(),
        }
    }

    /// Record how the request ended: the Usage Record, the capture while
    /// the setting is on, and the `model.completed` event.
    pub async fn finish(
        &self,
        outcome: Outcome,
        router_attempts: Option<u32>,
        end: Option<&TurnEnd>,
        error: Option<&BrainError>,
    ) {
        let duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let usage = end.and_then(|end| end.usage);
        // One Usage Record per model call. This is the one place that
        // holds the Run and the router's own accounting of the call, so
        // it is where the record is written.
        if let Some(end) = end {
            crate::spend::record(self.deps, self.run, end).await;
        }
        if self.deps.capture.is_enabled() {
            self.capture(outcome, end, error).await;
        }
        crate::run::publish(
            self.deps,
            self.run,
            "model.completed",
            json!({
                "phase": self.phase,
                "phase_request": self.phase_request,
                "outcome": outcome.as_str(),
                "logical_requests": 1,
                "router_attempts": router_attempts,
                "retries": router_attempts.map(|attempts| attempts.saturating_sub(1)),
                "duration_ms": duration_ms,
                "provider": end.and_then(|end| end.provider.as_deref()),
                "model": end.and_then(|end| end.model.as_deref()),
                "usage": usage,
                "estimated_final_attempt_cost_usd": end.and_then(|end| end.estimated_cost_usd),
                "estimated_total_cost_usd": match router_attempts {
                    Some(1) => end.and_then(|end| end.estimated_cost_usd),
                    _ => None,
                },
                "error": error.map(|error| error.message.as_str()),
            }),
        )
        .await;
    }

    /// A capture that is not kept is logged and never fails the Run: it
    /// is a debug copy, and the answer the Person asked for matters more.
    async fn capture(&self, outcome: Outcome, end: Option<&TurnEnd>, error: Option<&BrainError>) {
        let capture = ModelRequestCapture {
            id: ModelRequestCaptureId::generate(),
            workspace_id: self.run.workspace_id.clone(),
            run_id: self.run.id.clone(),
            phase: self.phase.to_string(),
            phase_request: i64::try_from(self.phase_request).unwrap_or(i64::MAX),
            request: request_json(self.request),
            answer: answer_json(outcome, end, error),
            created_at: self.deps.clock.now_ms(),
        };
        if let Err(error) = self.deps.model_request_captures.record(&capture).await {
            tracing::error!(%error, run_id = %self.run.id, "the model request capture was not kept");
        }
    }
}

/// The request as the Agent sends it, with each image replaced by its
/// media type, its sizes and its SHA-256.
pub(crate) fn request_json(request: &TurnRequest) -> Value {
    json!({
        "model_alias": request.model_alias,
        "model_candidates": request.model_candidates,
        "system": request.system,
        "messages": request.messages.iter().map(message_json).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(|tool| json!({
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        })).collect::<Vec<_>>(),
        "computer": request.computer,
        "allow_tool_calls": request.allow_tool_calls,
        "max_output_tokens": request.max_output_tokens,
        "output_schema": request.output_schema,
    })
}

fn message_json(message: &TurnMessage) -> Value {
    let role = match message.role {
        TurnRole::User => "user",
        TurnRole::Assistant => "assistant",
        TurnRole::Tool => "tool",
    };
    let mut value = json!({ "role": role, "text": message.text });
    if !message.images.is_empty() {
        value["images"] = message
            .images
            .iter()
            .map(|image| image_json(image))
            .collect();
    }
    if !message.tool_calls.is_empty() {
        value["tool_calls"] = message
            .tool_calls
            .iter()
            .map(|call| json!({"id": call.id, "name": call.name, "arguments": call.arguments}))
            .collect();
    }
    if let Some(call_id) = &message.tool_call_id {
        value["tool_call_id"] = call_id.as_str().into();
    }
    value
}

/// An image as its media type, its size in bytes and in pixels, and the
/// SHA-256 of its bytes. The bytes stay out of the capture.
fn image_json(data_uri: &str) -> Value {
    use base64::Engine as _;
    let (media_type, bytes) = data_uri
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,"))
        .and_then(|(media_type, encoded)| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
                .map(|bytes| (media_type.to_string(), bytes))
        })
        .unwrap_or_else(|| (String::new(), data_uri.as_bytes().to_vec()));
    let pixels = crate::image_tokens::dimensions(data_uri);
    json!({
        "media_type": (!media_type.is_empty()).then_some(media_type),
        "bytes": bytes.len(),
        "width": pixels.map(|(width, _)| width),
        "height": pixels.map(|(_, height)| height),
        "sha256": format!("{:x}", Sha256::digest(&bytes)),
    })
}

/// The provider's answer: what a completed request returned, or the
/// status, the message and the error body of a failed one.
pub(crate) fn answer_json(
    outcome: Outcome,
    end: Option<&TurnEnd>,
    error: Option<&BrainError>,
) -> Value {
    let mut answer = json!({ "outcome": outcome.as_str() });
    if let Some(end) = end {
        answer["provider"] = json!(end.provider);
        answer["model"] = json!(end.model);
        answer["stop_reason"] = json!(end.stop_reason);
        answer["usage"] = json!(end.usage);
    }
    if let Some(error) = error {
        answer["error"] = json!(error.message);
        if let Some(status) = error.provider_status() {
            answer["status"] = json!(status);
        }
        if let Some(body) = error.provider_body() {
            answer["body"] = body.clone();
        }
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 transparent PNG.
    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    #[test]
    fn an_image_becomes_its_type_its_sizes_and_its_hash() {
        let image = image_json(PNG);

        assert_eq!(image["media_type"], "image/png");
        assert_eq!(image["width"], 1);
        assert_eq!(image["height"], 1);
        assert!(image["bytes"].as_u64().is_some_and(|bytes| bytes > 0));
        assert_eq!(image["sha256"].as_str().map(str::len), Some(64));
        assert!(!image.to_string().contains("iVBORw0KGgo"));
    }

    #[test]
    fn a_request_keeps_its_text_and_drops_the_bytes_of_its_images() {
        let mut message = TurnMessage::user("look at this");
        message.images.push(PNG.to_string());
        let request = TurnRequest {
            model_alias: "default".into(),
            model_candidates: vec!["openai/gpt-6-luna".into()],
            system: "Follow the user.".into(),
            messages: vec![message],
            tools: vec![],
            computer: false,
            allow_tool_calls: true,
            max_output_tokens: Some(16_384),
            output_schema: None,
        };

        let captured = request_json(&request);

        assert_eq!(captured["system"], "Follow the user.");
        assert_eq!(captured["messages"][0]["text"], "look at this");
        assert_eq!(
            captured["messages"][0]["images"][0]["media_type"],
            "image/png"
        );
        assert_eq!(captured["max_output_tokens"], 16_384);
        assert!(!captured.to_string().contains("iVBORw0KGgo"));
    }
}
