//! The model meter of the release evaluation. It wraps the
//! model the caller supplies, counts every turn the daemon asks for,
//! including a failed one, and keeps the tool calls and the tool
//! results of conversation turns so the driver can record what a probe
//! actually did.
//!
//! The daemon keeps no tool result: a result lives in the run's own
//! transcript, which the daemon sends to the model and then drops. The
//! meter therefore reads each result from the next request of the same
//! run, where it arrives as a `Tool` message.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use pagis_agent::{Brain, BrainError, TurnDelta, TurnRequest, TurnRole, TurnStream};
use serde_json::Value;

/// How many characters of a tool's own sentence one summary line
/// carries.
const RESULT_TEXT_LIMIT: usize = 80;

/// The longest plain result the summary quotes. A longer one is a
/// document, not a sentence, so the line gives its size only.
const PLAIN_RESULT_LIMIT: usize = 200;

#[derive(Default, Debug)]
struct Counted {
    model_calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    /// Tool calls of conversation turns, in order.
    tool_calls: Vec<String>,
    /// One bounded summary of each tool result, in order.
    tool_results: Vec<String>,
    /// The kind and offered tools of each model request. It contains
    /// no message text or source content.
    requests: Vec<String>,
    /// The calls the meter has already summarized in this probe
    /// window. Every request repeats the whole transcript, so the
    /// meter must summarize each result once.
    answered: BTreeSet<String>,
}

/// What one chronology run spent on the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelSpend {
    pub model_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Where one probe starts in the meter's records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeterCursor {
    calls: usize,
    results: usize,
}

#[derive(Default)]
pub struct ModelMeter(Mutex<Counted>);

impl ModelMeter {
    pub fn spend(&self) -> ModelSpend {
        let counted = self.0.lock().expect("meter lock");
        ModelSpend {
            model_calls: counted.model_calls,
            input_tokens: counted.input_tokens,
            output_tokens: counted.output_tokens,
        }
    }

    /// Open one probe window. The driver takes the cursor before it
    /// sends the probe and reads the calls and the results made after
    /// it. A probe is one run, so the call ids of the last window
    /// cannot come back and the meter forgets them.
    pub fn probe_start(&self) -> MeterCursor {
        let mut counted = self.0.lock().expect("meter lock");
        counted.answered.clear();
        MeterCursor {
            calls: counted.tool_calls.len(),
            results: counted.tool_results.len(),
        }
    }

    pub fn tool_calls_since(&self, cursor: MeterCursor) -> Vec<String> {
        self.0.lock().expect("meter lock").tool_calls[cursor.calls..].to_vec()
    }

    pub fn tool_results_since(&self, cursor: MeterCursor) -> Vec<String> {
        self.0.lock().expect("meter lock").tool_results[cursor.results..].to_vec()
    }

    /// The newest conversation tool call of one probe window.
    /// A wait that ran out of time names it, so the report says what
    /// the daemon was doing. The window matters: a run that made no
    /// call of its own must not be described by the call of the probe
    /// before it.
    pub fn last_tool_call_since(&self, cursor: MeterCursor) -> Option<String> {
        self.0.lock().expect("meter lock").tool_calls[cursor.calls..]
            .last()
            .cloned()
    }

    /// Return bounded request metadata for the newest model calls.
    /// The lines contain no prompt or source text.
    pub fn recent_requests(&self, limit: usize) -> Vec<String> {
        let counted = self.0.lock().expect("meter lock");
        let start = counted.requests.len().saturating_sub(limit);
        counted.requests[start..].to_vec()
    }

    /// Return the newest model tool-call names without their arguments.
    pub fn recent_tool_calls(&self, limit: usize) -> Vec<String> {
        let counted = self.0.lock().expect("meter lock");
        let start = counted.tool_calls.len().saturating_sub(limit);
        counted.tool_calls[start..].to_vec()
    }
}

fn request_summary(request: &TurnRequest) -> String {
    let kind = if request.system.starts_with("Grade one evaluation probe") {
        "pre-grade"
    } else if request
        .messages
        .iter()
        .any(|message| message.text.starts_with("The run is over."))
    {
        "reflection"
    } else if request
        .messages
        .iter()
        .any(|message| message.text.contains("Schedule trigger:"))
    {
        "fired Schedule"
    } else {
        "reply"
    };
    let tools = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!("{kind} tools=[{tools}]")
}

/// One bounded line for one tool result. It carries counts and the
/// daemon's own words, never a claim body or source text.
fn summary(name: &str, _arguments: &str, text: &str) -> String {
    let Ok(Value::Object(body)) = serde_json::from_str::<Value>(text) else {
        // A result that is not a JSON object is the daemon's own
        // sentence: a refusal, a denial or a fixed acknowledgement. A
        // long one is a document and gives its size only.
        return match text.chars().count() {
            length if length <= PLAIN_RESULT_LIMIT => {
                format!("result:{name} text={}", bounded(text))
            }
            length => format!("result:{name} chars={length}"),
        };
    };
    if let Some(error) = body.get("error").and_then(Value::as_object) {
        let code = error.get("code").and_then(Value::as_str).unwrap_or("error");
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return format!(
            "result:{name} error={}",
            bounded(&format!("{code}: {message}"))
        );
    }
    format!("result:{name} chars={}", text.chars().count())
}

/// The first line of `text`, cut to the character limit.
fn bounded(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(RESULT_TEXT_LIMIT)
        .collect()
}

/// The caller's model, metered.
pub struct MeteredBrain {
    model: Arc<dyn Brain>,
    meter: Arc<ModelMeter>,
}

impl MeteredBrain {
    pub fn new(model: Arc<dyn Brain>, meter: Arc<ModelMeter>) -> Self {
        Self { model, meter }
    }

    /// Summarize every tool result of this request the meter has not
    /// summarized yet. The assistant messages of the same request name
    /// the tool each result answers.
    fn record_results(&self, request: &TurnRequest) {
        let calls: HashMap<&str, (&str, &str)> = request
            .messages
            .iter()
            .flat_map(|message| message.tool_calls.iter())
            .map(|call| {
                (
                    call.id.as_str(),
                    (call.name.as_str(), call.arguments.as_str()),
                )
            })
            .collect();
        let mut counted = self.meter.0.lock().expect("meter lock");
        for message in &request.messages {
            if message.role != TurnRole::Tool {
                continue;
            }
            let Some(id) = message.tool_call_id.clone() else {
                continue;
            };
            if !counted.answered.insert(id.clone()) {
                continue;
            }
            let (name, arguments) = calls.get(id.as_str()).copied().unwrap_or(("unknown", "{}"));
            let line = summary(name, arguments, &message.text);
            counted.tool_results.push(line);
        }
    }
}

#[async_trait]
impl Brain for MeteredBrain {
    fn ready(&self) -> bool {
        self.model.ready()
    }

    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        self.record_results(&request);
        {
            let mut counted = self.meter.0.lock().expect("meter lock");
            counted.model_calls += 1;
            counted.requests.push(request_summary(&request));
        }
        let stream = match self.model.turn(request).await {
            Ok(stream) => stream,
            Err(error) => {
                self.meter.0.lock().expect("meter lock").model_calls +=
                    u64::from(error.model_attempts().saturating_sub(1));
                return Err(error);
            }
        };
        let meter = Arc::clone(&self.meter);
        Ok(stream
            .map(move |delta| {
                let mut counted = meter.0.lock().expect("meter lock");
                match &delta {
                    Ok(TurnDelta::Finish(end)) => {
                        if let Some(usage) = end.usage {
                            counted.input_tokens += usage.input_tokens;
                            counted.output_tokens += usage.output_tokens;
                        }
                    }
                    Ok(TurnDelta::ModelRetries(retries)) => {
                        counted.model_calls += u64::from(*retries);
                    }
                    Ok(TurnDelta::ToolCall(call)) => {
                        counted.tool_calls.push(call.name.clone());
                    }
                    _ => {}
                }
                drop(counted);
                delta
            })
            .boxed())
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures::{StreamExt, stream};
    use pagis_agent::{
        Brain, BrainError, TurnDelta, TurnEnd, TurnMessage, TurnRequest, TurnStream,
    };

    use super::{MeteredBrain, ModelMeter, summary};
    use std::sync::Arc;

    #[test]
    fn an_error_result_names_its_code_and_message() {
        let error = serde_json::json!({
            "error": {"code": "temporarily_unavailable", "message": "mail is not served here"}
        })
        .to_string();

        assert_eq!(
            summary("mail__search", "{}", &error),
            "result:mail__search error=temporarily_unavailable: mail is not served here"
        );
    }

    #[test]
    fn a_plain_result_carries_the_daemon_sentence_and_a_long_one_its_size() {
        assert_eq!(
            summary("mail__search", "{}", "unknown tool: mail__search"),
            "result:mail__search text=unknown tool: mail__search"
        );
        let document = "a".repeat(400);
        assert_eq!(
            summary("skill_load", "{}", &document),
            "result:skill_load chars=400"
        );
    }

    #[test]
    fn every_other_json_result_gives_its_size_only() {
        assert_eq!(
            summary(
                "mail__search",
                "{}",
                r#"{"messages":[{"subject":"Couch"}]}"#
            ),
            "result:mail__search chars=34"
        );
    }

    struct RetriedPreGrader;

    #[async_trait]
    impl Brain for RetriedPreGrader {
        async fn turn(&self, _: TurnRequest) -> Result<TurnStream, BrainError> {
            Ok(stream::iter([
                Ok(TurnDelta::ModelRetries(2)),
                Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: "stop".into(),
                    provider: Some("scripted".into()),
                    model: Some("test".into()),
                    usage: Some(pagis_agent::Usage {
                        input_tokens: 10,
                        output_tokens: 2,
                        ..Default::default()
                    }),
                    estimated_cost_usd: None,
                })),
            ])
            .boxed())
        }
    }

    #[tokio::test]
    async fn pre_grader_retries_count_as_model_calls() {
        let meter = Arc::new(ModelMeter::default());
        let brain = MeteredBrain::new(Arc::new(RetriedPreGrader), Arc::clone(&meter));
        let request = TurnRequest {
            model_alias: "evaluation-pre-grader".into(),
            model_candidates: vec!["openai/gpt-5-mini".into()],
            system: "Grade one evaluation probe.".into(),
            messages: vec![TurnMessage::user("Grade this reply.")],
            tools: Vec::new(),
            computer: false,
            allow_tool_calls: true,
            max_output_tokens: Some(4_096),
            output_schema: None,
        };

        brain
            .turn(request)
            .await
            .expect("pre-grader turn")
            .collect::<Vec<_>>()
            .await;

        assert_eq!(meter.spend().model_calls, 3);
    }
}
