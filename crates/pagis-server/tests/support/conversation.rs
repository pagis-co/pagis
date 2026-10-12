//! A model script and an executor for the conversation routes.

use std::collections::VecDeque;
use std::sync::Mutex;

use futures::{StreamExt, stream};
use pagis_agent::{Brain, BrainError, ModelCallGate, TurnDelta, TurnEnd, TurnRequest, TurnStream};
use pagis_broker::{AuthorizedCall, ToolExecutor, ToolResult};
use pagis_core::Run;

#[derive(Default)]
pub struct Model {
    turns: Mutex<VecDeque<TurnStream>>,
    pub requests: Mutex<Vec<TurnRequest>>,
    pub calls: Mutex<Vec<AuthorizedCall>>,
}

impl Model {
    pub fn tool(&self, name: &str, arguments: serde_json::Value) {
        self.turns.lock().unwrap().push_back(
            stream::iter([
                Ok(TurnDelta::ToolCall(pagis_agent::ToolInvocation {
                    id: "call_1".into(),
                    name: name.into(),
                    arguments: arguments.to_string(),
                })),
                Ok(TurnDelta::Finish(Self::end("tool_calls"))),
            ])
            .boxed(),
        );
    }

    pub fn reply(&self, text: &str) {
        self.turns.lock().unwrap().push_back(
            stream::iter([
                Ok(TurnDelta::Text(text.into())),
                Ok(TurnDelta::Finish(Self::end("stop"))),
            ])
            .boxed(),
        );
    }

    pub fn hang(&self) {
        self.turns
            .lock()
            .unwrap()
            .push_back(stream::pending().boxed());
    }

    fn end(reason: &str) -> TurnEnd {
        TurnEnd {
            stop_reason: reason.into(),
            provider: Some("scripted".into()),
            model: Some("test".into()),
            usage: Some(pagis_agent::Usage {
                input_tokens: 7,
                output_tokens: 11,
                ..Default::default()
            }),
            estimated_cost_usd: None,
        }
    }
}

#[async_trait::async_trait]
impl Brain for Model {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("the model has a script"))
    }
}

#[async_trait::async_trait]
impl ModelCallGate for Model {
    async fn before_call(&self, _: &Run, _: &[String]) -> Result<(), pagis_agent::GateStop> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToolExecutor for Model {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        assert_eq!(call.tool_name, "mail__send");
        self.calls.lock().unwrap().push(call);
        ToolResult::success("The mail is sent.")
    }
}
