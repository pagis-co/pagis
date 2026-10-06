//! The scripted brain: deterministic turn outcomes for run
//! state-machine tests, in place of a real provider.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use pagis_agent::{Brain, BrainError, ToolInvocation, TurnDelta, TurnEnd, TurnRequest, TurnStream};

/// One scripted turn outcome, consumed in push order.
#[derive(Debug, Clone)]
pub enum Script {
    /// Stream these deltas, then finish the turn.
    Reply(Vec<String>),
    /// Stream these deltas with explicit provider accounting, then finish.
    ReplyAccounted(Vec<String>, u32, Option<pagis_agent::Usage>),
    /// Stream these deltas, then finish with a usage the provider
    /// reported and the cost the serving model's price table makes of
    /// it. The Usage Record and the Spend Cap read that number.
    ReplyCosted(Vec<String>, pagis_agent::Usage, f64),
    /// Stream these deltas, emit these tool calls, then finish the
    /// turn with `tool_calls` as the stop reason.
    ToolCalls(Vec<String>, Vec<ToolInvocation>),
    /// Stream these deltas, then fail with this error.
    FailAfter(Vec<String>, String),
    /// The provider refuses the installation's key.
    RefuseKey(String),
    /// Fail the call as a provider that answered with an error status:
    /// the status, the error body and the message.
    ProviderError(u16, serde_json::Value, String),
    /// Stream these deltas, then stay open until canceled.
    Hang(Vec<String>),
    /// Announce that the request started, wait for release, then reply.
    GateReply(
        Vec<String>,
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ),
    /// Announce that the request started, wait for release, then emit
    /// this tool call and finish the turn with `tool_calls` as the stop
    /// reason.
    GateToolCall(
        ToolInvocation,
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ),
}

impl Script {
    pub fn reply(deltas: &[&str]) -> Self {
        Script::Reply(deltas.iter().map(|d| d.to_string()).collect())
    }

    pub fn reply_with_accounting(
        deltas: &[&str],
        retries: u32,
        usage: Option<pagis_agent::Usage>,
    ) -> Self {
        Script::ReplyAccounted(
            deltas.iter().map(|d| d.to_string()).collect(),
            retries,
            usage,
        )
    }

    /// Reply, and report this usage and this cost for the turn.
    pub fn reply_costing(deltas: &[&str], usage: pagis_agent::Usage, cost_usd: f64) -> Self {
        Script::ReplyCosted(
            deltas.iter().map(|d| d.to_string()).collect(),
            usage,
            cost_usd,
        )
    }

    /// One tool call after the deltas, with the id `call_1`.
    pub fn tool_call(deltas: &[&str], name: &str, arguments: serde_json::Value) -> Self {
        Script::tool_call_as("call_1", deltas, name, arguments)
    }

    /// One tool call after the deltas, under the given call id. A tool that
    /// keeps a per-call ledger needs each call to carry its own identity.
    pub fn tool_call_as(
        id: &str,
        deltas: &[&str],
        name: &str,
        arguments: serde_json::Value,
    ) -> Self {
        Script::ToolCalls(
            deltas.iter().map(|d| d.to_string()).collect(),
            vec![ToolInvocation {
                id: id.to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
            }],
        )
    }

    pub fn fail_after(deltas: &[&str], error: &str) -> Self {
        Script::FailAfter(
            deltas.iter().map(|d| d.to_string()).collect(),
            error.to_string(),
        )
    }

    pub fn refuse_key(error: &str) -> Self {
        Script::RefuseKey(error.to_string())
    }

    pub fn provider_error(status: u16, body: serde_json::Value, message: &str) -> Self {
        Script::ProviderError(status, body, message.to_string())
    }

    pub fn hang(deltas: &[&str]) -> Self {
        Script::Hang(deltas.iter().map(|d| d.to_string()).collect())
    }

    pub fn gate_reply(
        deltas: &[&str],
        entered: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
    ) -> Self {
        Script::GateReply(
            deltas.iter().map(|d| d.to_string()).collect(),
            entered,
            release,
        )
    }

    /// One tool call with the id `call_1`, after the release. The Run
    /// holds its Capability Snapshot while the turn waits, so a test can
    /// change a Grant between the snapshot and the dispatch.
    pub fn gate_tool_call(
        name: &str,
        arguments: serde_json::Value,
        entered: std::sync::Arc<tokio::sync::Notify>,
        release: std::sync::Arc<tokio::sync::Notify>,
    ) -> Self {
        Script::GateToolCall(
            ToolInvocation {
                id: "call_1".to_string(),
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
            entered,
            release,
        )
    }
}

/// A brain that plays scripts and records every request it saw. With no
/// script queued, a turn fails ("scripted brain has no script").
///
/// Two modes: `push` feeds one shared queue, consumed in push order.
/// `push_for` feeds a per-agent queue, matched on the "You are <name>"
/// system prompt — the deterministic mode for multi-agent tests,
/// where concurrent runs would race on a shared queue. In per-agent
/// mode, reflection turns (memory tools only) stay unscripted and fail
/// like an empty queue does.
#[derive(Default)]
pub struct ScriptedBrain {
    scripts: Mutex<VecDeque<Script>>,
    named: Mutex<HashMap<String, VecDeque<Script>>>,
    requests: Mutex<Vec<TurnRequest>>,
}

impl ScriptedBrain {
    pub fn push(&self, script: Script) {
        self.scripts.lock().expect("script lock").push_back(script);
    }

    /// Queue a script for one agent's next turn (per-agent mode).
    pub fn push_for(&self, agent_name: &str, script: Script) {
        self.named
            .lock()
            .expect("named script lock")
            .entry(agent_name.to_string())
            .or_default()
            .push_back(script);
    }

    /// Every request the brain received, in order.
    pub fn requests(&self) -> Vec<TurnRequest> {
        self.requests.lock().expect("request lock").clone()
    }

    fn next_script(&self, request: &TurnRequest) -> Option<Script> {
        let mut named = self.named.lock().expect("named script lock");
        if !named.is_empty() {
            let reflection = request
                .tools
                .iter()
                .all(|tool| tool.name.starts_with("memory_") || tool.name.starts_with("schedule_"));
            if reflection {
                return None;
            }
            let name = named
                .keys()
                .find(|name| request.system.starts_with(&format!("You are {name},")))
                .cloned()?;
            return named.get_mut(&name).expect("queue exists").pop_front();
        }
        drop(named);
        self.scripts.lock().expect("script lock").pop_front()
    }
}

#[async_trait]
impl Brain for ScriptedBrain {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let script = self.next_script(&request);
        self.requests.lock().expect("request lock").push(request);
        let Some(script) = script else {
            return Err(BrainError::new("scripted brain has no script"));
        };

        let deltas = |texts: Vec<String>| {
            stream::iter(
                texts
                    .into_iter()
                    .map(|text| Ok(TurnDelta::Text(text)))
                    .collect::<Vec<_>>(),
            )
        };
        Ok(match script {
            Script::Reply(texts) => deltas(texts)
                .chain(stream::iter([Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: "stop".to_string(),
                    provider: Some("scripted".to_string()),
                    model: Some("test".to_string()),
                    usage: Some(pagis_agent::Usage {
                        input_tokens: 7,
                        output_tokens: 11,
                        ..Default::default()
                    }),
                    estimated_cost_usd: None,
                }))]))
                .boxed(),
            Script::ReplyAccounted(texts, retries, usage) => deltas(texts)
                .chain(stream::iter(
                    (retries > 0).then_some(Ok(TurnDelta::ModelRetries(retries))),
                ))
                .chain(stream::iter([Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: "stop".to_string(),
                    provider: Some("scripted".to_string()),
                    model: Some("test".to_string()),
                    usage,
                    estimated_cost_usd: None,
                }))]))
                .boxed(),
            Script::ReplyCosted(texts, usage, cost_usd) => deltas(texts)
                .chain(stream::iter([Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: "stop".to_string(),
                    provider: Some("scripted".to_string()),
                    model: Some("test".to_string()),
                    usage: Some(usage),
                    estimated_cost_usd: Some(cost_usd),
                }))]))
                .boxed(),
            Script::ToolCalls(texts, calls) => deltas(texts)
                .chain(stream::iter(
                    calls
                        .into_iter()
                        .map(|call| Ok(TurnDelta::ToolCall(call)))
                        .collect::<Vec<_>>(),
                ))
                .chain(stream::iter([Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: "tool_calls".to_string(),
                    provider: Some("scripted".to_string()),
                    model: Some("test".to_string()),
                    usage: Some(pagis_agent::Usage {
                        input_tokens: 7,
                        output_tokens: 11,
                        ..Default::default()
                    }),
                    estimated_cost_usd: None,
                }))]))
                .boxed(),
            Script::FailAfter(texts, error) => deltas(texts)
                .chain(stream::iter([Err(BrainError::new(error))]))
                .boxed(),
            Script::RefuseKey(error) => stream::iter([Err(BrainError::refused_key(error))]).boxed(),
            Script::ProviderError(status, body, message) => {
                stream::iter([Err(BrainError::provider(message, status, Some(body)))]).boxed()
            }
            Script::Hang(texts) => deltas(texts).chain(stream::pending()).boxed(),
            Script::GateReply(texts, entered, release) => stream::once(async move {
                entered.notify_one();
                release.notified().await;
                Ok(TurnDelta::Text(texts.concat()))
            })
            .chain(stream::iter([Ok(TurnDelta::Finish(TurnEnd {
                stop_reason: "stop".to_string(),
                provider: Some("scripted".to_string()),
                model: Some("test".to_string()),
                usage: Some(pagis_agent::Usage {
                    input_tokens: 7,
                    output_tokens: 11,
                    ..Default::default()
                }),
                estimated_cost_usd: None,
            }))]))
            .boxed(),
            Script::GateToolCall(call, entered, release) => stream::once(async move {
                entered.notify_one();
                release.notified().await;
                Ok(TurnDelta::ToolCall(call))
            })
            .chain(stream::iter([Ok(TurnDelta::Finish(TurnEnd {
                stop_reason: "tool_calls".to_string(),
                provider: Some("scripted".to_string()),
                model: Some("test".to_string()),
                usage: Some(pagis_agent::Usage {
                    input_tokens: 7,
                    output_tokens: 11,
                    ..Default::default()
                }),
                estimated_cost_usd: None,
            }))]))
            .boxed(),
        })
    }
}
