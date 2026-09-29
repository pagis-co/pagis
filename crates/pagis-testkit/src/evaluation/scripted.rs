//! The scripted model of a release-evaluation smoke run.
//!
//! It answers conversation, reflection, and grading turns.
//! The daemon retries a turn the model refuses while the run lasts, so
//! a refused turn kind makes the metered model calls grow with the
//! machine's load and breaks the manifest's per-repeat cap.
//!
//! A run on this model is unscored by definition: it shows that the
//! driver reaches the shipped path and says nothing about behavior.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use pagis_agent::{Brain, BrainError, TurnRequest, TurnRole, TurnStream};
use serde_json::json;

use crate::{Script, ScriptedBrain};

/// The system prompt of one model pre-grader call.
const PRE_GRADER: &str = "Grade one evaluation probe";

/// Answers every conversation turn with the prompt it received, so the
/// recorded output is deterministic and traceable to the probe.
#[derive(Default)]
pub struct ScriptedModel {
    /// The conversation prompt this model answers with a gated tool
    /// call instead of prose.
    park_on: Option<String>,
    /// Whether the gated call was made. The call is made once: a broker
    /// that refuses it answers the model, and the next turn replies in
    /// prose, as a real model does.
    parked: AtomicBool,
}

impl ScriptedModel {
    /// A model that answers the conversation prompt `prompt` with one
    /// `schedule_create` call. The broker gates that tool: a run that has
    /// not read sourced knowledge shows an approval card and waits for
    /// the owner, and a knowledge-bearing run refuses the call, after
    /// which this model replies in prose. Every other turn keeps the
    /// default answer.
    pub fn parking_on(prompt: &str) -> Self {
        Self {
            park_on: Some(prompt.to_string()),
            ..Self::default()
        }
    }

    /// The tool call that waits for the owner's decision.
    fn parked_call() -> Script {
        Script::tool_call(
            &[],
            "schedule_create",
            json!({
                "kind": "one_shot",
                "name": "Compare the couch offer",
                "instruction": "Compare the couch offer for the owner",
                "local_time": "2026-01-02T10:00:00",
                "timezone": "Etc/UTC",
            }),
        )
    }

    /// The first line of the newest owner message: the prompt itself,
    /// without the time the driver appends.
    fn asked(request: &TurnRequest) -> String {
        request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == TurnRole::User)
            .map(|message| message.text.lines().next().unwrap_or_default().to_string())
            .unwrap_or_default()
    }

    fn answer(request: &TurnRequest) -> String {
        format!("Scripted answer: {}", Self::asked(request))
    }
}

#[async_trait]
impl Brain for ScriptedModel {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let scripted = ScriptedBrain::default();
        if request.system.starts_with(PRE_GRADER) {
            scripted.push(Script::Reply(vec![
                json!({
                    "grade": "pass",
                    "reason": "The scripted answer follows the rubric."
                })
                .to_string(),
            ]));
        } else if self
            .park_on
            .as_deref()
            .is_some_and(|prompt| Self::asked(&request).ends_with(prompt))
            && !self.parked.swap(true, Ordering::Relaxed)
        {
            scripted.push(Self::parked_call());
        } else {
            scripted.push(Script::Reply(vec![Self::answer(&request)]));
        }
        scripted.turn(request).await
    }
}
