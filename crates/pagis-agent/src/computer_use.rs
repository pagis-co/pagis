//! Computer-use call handling: classify a model `computer`
//! call into its provider vocabulary, surface OpenAI safety checks,
//! and execute the normalized actions against the agent's computer —
//! sequentially, stopping at the first failure. Screenshots come back
//! as display-space PNGs the caller stores as artifacts.

use std::sync::Arc;

use pagis_computer::exec::{self, Action, DisplayFrame};
use pagis_computer::{ComputerError, ComputerManager};
use pagis_core::AgentId;
use serde_json::Value;

/// Which provider's loop the call belongs to; decides the tool-result
/// shape (per-action for Anthropic, one final screenshot for OpenAI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vocabulary {
    Anthropic,
    OpenAi,
    Portable,
}

/// One parsed computer call: its normalized actions and any safety
/// checks the user must approve before execution.
pub(crate) struct ComputerCall {
    pub vocabulary: Vocabulary,
    pub actions: Vec<Action>,
    /// OpenAI `pending_safety_checks`, verbatim.
    pub safety_checks: Vec<Value>,
}

/// Parse one `computer` tool call's arguments. Anthropic sends the
/// action object itself; OpenAI sends the whole `computer_call` item;
/// the portable function wraps one OpenAI-style action object in
/// `action`.
pub(crate) fn parse_call(arguments: &str) -> Result<ComputerCall, String> {
    let parsed: Value =
        serde_json::from_str(arguments).map_err(|err| format!("bad computer call: {err}"))?;
    if parsed.get("type").and_then(Value::as_str) == Some("computer_call") {
        let raw_actions = parsed
            .get("actions")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| parsed.get("action").map(|action| vec![action.clone()]))
            .unwrap_or_default();
        let actions = raw_actions
            .iter()
            .map(exec::parse_openai)
            .collect::<Result<Vec<_>, _>>()?;
        let safety_checks = parsed
            .get("pending_safety_checks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return Ok(ComputerCall {
            vocabulary: Vocabulary::OpenAi,
            actions,
            safety_checks,
        });
    }
    if let Some(action) = parsed.get("action").filter(|action| action.is_object()) {
        return Ok(ComputerCall {
            vocabulary: Vocabulary::Portable,
            actions: vec![exec::parse_openai(action)?],
            safety_checks: Vec::new(),
        });
    }
    Ok(ComputerCall {
        vocabulary: Vocabulary::Anthropic,
        actions: vec![exec::parse_anthropic(&parsed)?],
        safety_checks: Vec::new(),
    })
}

/// The run's computer context: the coordinate scale of the last
/// screenshot the model saw, and the current turn's batch failure.
pub(crate) struct ComputerCtx {
    pub scale_x: f64,
    pub scale_y: f64,
    /// Set when an action failed this turn: later computer calls in
    /// the same batch are skipped (stop at first failure).
    pub batch_failed: bool,
    /// The screen lease, held for the turn's batch.
    pub lease: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Set when the run resumed after a takeover: the next
    /// computer tool result carries the "screen may have changed" note.
    pub resumed_after_takeover: bool,
    /// Set when an input reached the screen after the last screenshot:
    /// the next screenshot waits for the screen to settle.
    pub input_since_screenshot: bool,
}

impl Default for ComputerCtx {
    fn default() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            batch_failed: false,
            lease: None,
            resumed_after_takeover: false,
            input_since_screenshot: false,
        }
    }
}

/// One executed batch: what happened, plus every screenshot taken.
pub(crate) struct BatchOutcome {
    /// `Ok(())` or the first failure's message.
    pub result: Result<(), String>,
    /// Screenshots in execution order (the last one is the reply
    /// image for OpenAI calls).
    pub screenshots: Vec<DisplayFrame>,
}

/// Execute the actions sequentially, stopping at the first failure.
/// `ctx.scale_*` update from every screenshot taken.
pub(crate) async fn execute_actions(
    computer: &Arc<ComputerManager>,
    agent_id: &AgentId,
    actions: &[Action],
    ctx: &mut ComputerCtx,
) -> BatchOutcome {
    let mut screenshots = Vec::new();
    if let Err(error) = computer.ensure_awake(agent_id).await {
        return BatchOutcome {
            result: Err(computer_error(error)),
            screenshots,
        };
    }
    for action in actions {
        let step = match action {
            Action::Screenshot => match screenshot(computer, agent_id, ctx).await {
                Ok(frame) => {
                    screenshots.push(frame);
                    Ok(())
                }
                Err(error) => Err(error),
            },
            Action::Wait { ms } => {
                tokio::time::sleep(std::time::Duration::from_millis((*ms).min(30_000))).await;
                Ok(())
            }
            action => {
                let ops = exec::input_ops(action, ctx.scale_x, ctx.scale_y);
                ctx.input_since_screenshot = true;
                computer.input(agent_id, &ops).await.map_err(computer_error)
            }
        };
        if let Err(error) = step {
            return BatchOutcome {
                result: Err(error),
                screenshots,
            };
        }
    }
    BatchOutcome {
        result: Ok(()),
        screenshots,
    }
}

/// One display-space screenshot; updates the coordinate scale. After
/// an input it waits for the screen to settle, so the model sees the
/// outcome of its action.
pub(crate) async fn screenshot(
    computer: &Arc<ComputerManager>,
    agent_id: &AgentId,
    ctx: &mut ComputerCtx,
) -> Result<DisplayFrame, String> {
    let png = if ctx.input_since_screenshot {
        computer.settled_frame(agent_id).await
    } else {
        computer.live_frame(agent_id).await
    }
    .map_err(computer_error)?;
    ctx.input_since_screenshot = false;
    let frame = exec::fit_display(&png)?;
    ctx.scale_x = frame.scale_x;
    ctx.scale_y = frame.scale_y;
    Ok(frame)
}

fn computer_error(error: ComputerError) -> String {
    format!("computer error: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_anthropic_action_object_parses_as_one_action() {
        let call = parse_call(r#"{"action": "screenshot"}"#).unwrap();
        assert_eq!(call.vocabulary, Vocabulary::Anthropic);
        assert_eq!(call.actions, vec![Action::Screenshot]);
        assert!(call.safety_checks.is_empty());
    }

    #[test]
    fn an_openai_computer_call_item_parses_as_a_batch_with_checks() {
        let item = json!({
            "type": "computer_call", "call_id": "call_1",
            "actions": [
                {"type": "click", "button": "left", "x": 1, "y": 2},
                {"type": "screenshot"}
            ],
            "pending_safety_checks": [{"id": "sc_1", "code": "x", "message": "m"}]
        });
        let call = parse_call(&item.to_string()).unwrap();
        assert_eq!(call.vocabulary, Vocabulary::OpenAi);
        assert_eq!(call.actions.len(), 2);
        assert_eq!(call.safety_checks.len(), 1);
    }

    #[test]
    fn a_portable_action_object_parses_with_openai_coordinates() {
        let call =
            parse_call(r#"{"action":{"type":"click","button":"left","x":100,"y":200}}"#).unwrap();
        assert_eq!(call.vocabulary, Vocabulary::Portable);
        assert_eq!(call.actions.len(), 1);
        assert!(call.safety_checks.is_empty());
    }

    #[test]
    fn bad_actions_are_errors() {
        assert!(parse_call("not json").is_err());
        assert!(parse_call(r#"{"action": "levitate"}"#).is_err());
    }
}
