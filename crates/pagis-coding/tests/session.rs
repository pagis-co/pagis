//! An `AcpSession` against the fake harness, over `tokio::io::duplex`.
//! Each test runs on the multi-thread runtime with no `LocalSet`, as the
//! daemon does.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_coding::fake::{self, NEW_SESSION_ID, Script, Turn, acp};
use pagis_coding::{
    AcpSession, AskHandler, CodingError, Cost, Location, Opening, PermissionAnswer, PermissionAsk,
    PlanEntry, PlanPriority, PlanStatus, QuestionAnswer, QuestionAsk, SessionEvent, SessionModes,
    SignInMethod, StopReason, ToolKind, ToolStatus,
};
use pagis_core::HarnessModeInfo;
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;

const WAIT: Duration = Duration::from_secs(10);

/// The scripts of these tests send no permission request and no question.
struct NoAsks;

#[async_trait]
impl AskHandler for NoAsks {
    async fn permission(&self, ask: PermissionAsk) -> PermissionAnswer {
        panic!("the script asks no permission, and {ask:?} came");
    }

    async fn question(&self, ask: QuestionAsk) -> QuestionAnswer {
        panic!("the script asks no question, and {ask:?} came");
    }
}

async fn pair(
    script: Script,
    opening: Opening,
) -> (
    fake::FakeHarness,
    Result<(AcpSession, UnboundedReceiver<SessionEvent>), CodingError>,
) {
    fake::pair(script, opening, Arc::new(NoAsks)).await
}

async fn open(
    script: Script,
    opening: Opening,
) -> (
    fake::FakeHarness,
    AcpSession,
    UnboundedReceiver<SessionEvent>,
) {
    let (harness, opened) = pair(script, opening).await;
    let (session, events) = opened.expect("the session opens");
    (harness, session, events)
}

async fn next(events: &mut UnboundedReceiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("an event comes in time")
        .expect("the event channel is open")
}

fn message(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    )))
}

fn restore(acp_session_id: &str) -> Opening {
    Opening::Restore {
        acp_session_id: acp_session_id.to_owned(),
        cwd: PathBuf::from("/work/repo"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_names_the_client_and_session_new_carries_the_cwd() {
    let (harness, session, _events) =
        open(Script::default(), fake::new_session("/work/repo")).await;

    assert_eq!(session.acp_session_id(), NEW_SESSION_ID);
    let methods: Vec<String> = harness.received().into_iter().map(|r| r.method).collect();
    assert_eq!(methods, ["initialize", "session/new"]);

    let initialize = &harness.params("initialize")[0];
    assert_eq!(initialize["protocolVersion"], json!(1));
    assert_eq!(initialize["clientInfo"]["name"], json!("pagis"));
    assert_eq!(
        initialize["clientInfo"]["version"],
        json!(env!("CARGO_PKG_VERSION"))
    );

    let new = &harness.params("session/new")[0];
    assert_eq!(new["cwd"], json!("/work/repo"));
    assert_eq!(new["mcpServers"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn harness_info_keeps_the_terminal_sign_in_methods_only() {
    let script = Script::default()
        .agent("claude-agent-acp", "0.87.0")
        .auth_method(acp::AuthMethod::Terminal(
            acp::AuthMethodTerminal::new("claude-ai-login", "Claude subscription")
                .args(vec!["--login".to_owned()])
                .env(HashMap::from([("CLAUDE_LOGIN".to_owned(), "1".to_owned())])),
        ))
        .auth_method(acp::AuthMethod::Agent(acp::AuthMethodAgent::new(
            "api-key", "API key",
        )));
    let (_harness, session, _events) = open(script, fake::new_session("/work/repo")).await;

    let harness = session.harness();
    assert_eq!(harness.name.as_deref(), Some("claude-agent-acp"));
    assert_eq!(harness.version.as_deref(), Some("0.87.0"));
    assert!(!harness.can_resume);
    assert!(!harness.can_load);
    assert_eq!(
        harness.sign_in_methods,
        [SignInMethod {
            id: "claude-ai-login".to_owned(),
            name: "Claude subscription".to_owned(),
            args: vec!["--login".to_owned()],
            env: BTreeMap::from([("CLAUDE_LOGIN".to_owned(), "1".to_owned())]),
        }]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_gives_its_updates_in_order_then_the_turn_end() {
    let updates = vec![
        acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new("I read the file first."),
        ))),
        acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new("Hello")))
                .message_id(acp::MessageId::new("m1")),
        ),
        acp::SessionUpdate::ToolCall(
            acp::ToolCall::new("call-1", "Edit main.rs")
                .kind(acp::ToolKind::Edit)
                .status(acp::ToolCallStatus::InProgress)
                .locations(vec![
                    acp::ToolCallLocation::new("/work/repo/main.rs").line(3),
                ]),
        ),
        acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
            "call-1",
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
        )),
        acp::SessionUpdate::Plan(acp::Plan::new(vec![acp::PlanEntry::new(
            "Fix the bug",
            acp::PlanEntryPriority::High,
            acp::PlanEntryStatus::InProgress,
        )])),
        acp::SessionUpdate::UsageUpdate(
            acp::UsageUpdate::new(1200, 200_000).cost(acp::Cost::new(0.25, "USD")),
        ),
        acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new("plan")),
        // The crate ignores the updates that Pagis does not show.
        acp::SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title("Bug fix")),
    ];
    let script = Script::default().turn(Turn::new(updates, acp::StopReason::EndTurn));
    let (harness, session, mut events) = open(script, fake::new_session("/work/repo")).await;

    session.prompt("Fix the bug").expect("the prompt is sent");

    assert_eq!(
        next(&mut events).await,
        SessionEvent::Thought {
            text: "I read the file first.".to_owned()
        }
    );
    assert_eq!(
        next(&mut events).await,
        SessionEvent::AgentMessage {
            message_id: Some("m1".to_owned()),
            text: "Hello".to_owned(),
        }
    );
    let SessionEvent::ToolCall {
        id,
        title,
        tool_kind,
        status,
        locations,
        raw,
    } = next(&mut events).await
    else {
        panic!("a tool call comes third");
    };
    assert_eq!(id, "call-1");
    assert_eq!(title, "Edit main.rs");
    assert_eq!(tool_kind, ToolKind::Edit);
    assert_eq!(status, ToolStatus::InProgress);
    assert_eq!(
        locations,
        [Location {
            path: PathBuf::from("/work/repo/main.rs"),
            line: Some(3),
        }]
    );
    assert_eq!(raw["toolCallId"], json!("call-1"));
    assert_eq!(raw["kind"], json!("edit"));
    let SessionEvent::ToolCallUpdate {
        id,
        title,
        tool_kind,
        status,
        locations,
        raw,
    } = next(&mut events).await
    else {
        panic!("a tool call update comes fourth");
    };
    assert_eq!(id, "call-1");
    assert_eq!((title, tool_kind, locations), (None, None, None));
    assert_eq!(status, Some(ToolStatus::Completed));
    assert_eq!(raw["status"], json!("completed"));
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Plan {
            entries: vec![PlanEntry {
                content: "Fix the bug".to_owned(),
                priority: PlanPriority::High,
                status: PlanStatus::InProgress,
            }],
        }
    );
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Usage {
            used: 1200,
            size: 200_000,
            cost: Some(Cost {
                amount: 0.25,
                currency: "USD".to_owned(),
            }),
        }
    );
    assert_eq!(
        next(&mut events).await,
        SessionEvent::ModeChanged {
            mode_id: "plan".to_owned()
        }
    );
    assert_eq!(
        next(&mut events).await,
        SessionEvent::TurnEnded {
            stop_reason: StopReason::EndTurn
        }
    );

    let prompt = &harness.params("session/prompt")[0];
    assert_eq!(prompt["sessionId"], json!(NEW_SESSION_ID));
    assert_eq!(
        prompt["prompt"],
        json!([{ "type": "text", "text": "Fix the bug" }])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_takes_a_new_prompt_after_the_turn_end() {
    let script = Script::default()
        .turn(Turn::new(vec![message("one")], acp::StopReason::EndTurn))
        .turn(Turn::new(vec![message("two")], acp::StopReason::MaxTokens));
    let (_harness, session, mut events) = open(script, fake::new_session("/work/repo")).await;

    session.prompt("first").expect("the first prompt is sent");
    next(&mut events).await;
    next(&mut events).await;
    session.prompt("second").expect("the second prompt is sent");

    assert!(matches!(
        next(&mut events).await,
        SessionEvent::AgentMessage { text, .. } if text == "two"
    ));
    assert_eq!(
        next(&mut events).await,
        SessionEvent::TurnEnded {
            stop_reason: StopReason::MaxTokens
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_reaches_the_harness_and_the_turn_ends_cancelled() {
    let script = Script::default().turn(Turn::until_cancel(vec![message("working")]));
    let (harness, session, mut events) = open(script, fake::new_session("/work/repo")).await;

    session.prompt("Refactor").expect("the prompt is sent");
    assert!(matches!(
        next(&mut events).await,
        SessionEvent::AgentMessage { .. }
    ));
    session.cancel().expect("the cancel is sent");

    assert_eq!(
        next(&mut events).await,
        SessionEvent::TurnEnded {
            stop_reason: StopReason::Cancelled
        }
    );
    assert_eq!(
        harness.params("session/cancel"),
        [json!({ "sessionId": NEW_SESSION_ID })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_prompt_during_a_turn_is_busy() {
    let script = Script::default().turn(Turn::until_cancel(vec![message("working")]));
    let (harness, session, mut events) = open(script, fake::new_session("/work/repo")).await;

    session.prompt("first").expect("the first prompt is sent");
    assert_eq!(session.prompt("second"), Err(CodingError::Busy));

    next(&mut events).await;
    session.cancel().expect("the cancel is sent");
    next(&mut events).await;
    assert_eq!(harness.params("session/prompt").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_resumes_when_the_harness_declares_resume() {
    let script = Script::default().resume().load();
    let (harness, session, _events) = open(script, restore("harness-session-7")).await;

    assert_eq!(session.acp_session_id(), "harness-session-7");
    assert!(session.harness().can_resume);
    assert_eq!(
        harness.params("session/load"),
        Vec::<serde_json::Value>::new()
    );
    let resume = &harness.params("session/resume")[0];
    assert_eq!(resume["sessionId"], json!("harness-session-7"));
    assert_eq!(resume["cwd"], json!("/work/repo"));
    // `session/resume` leaves an empty server list out.
    assert!(resume.get("mcpServers").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_loads_when_the_harness_declares_load_only_and_drops_the_replay() {
    let script = Script::default()
        .load()
        .history(vec![
            message("old answer"),
            acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new("plan")),
            message("older answer"),
        ])
        .turn(Turn::new(
            vec![message("new answer")],
            acp::StopReason::EndTurn,
        ));
    let (harness, session, mut events) = open(script, restore("harness-session-7")).await;

    assert_eq!(session.acp_session_id(), "harness-session-7");
    assert!(session.harness().can_load);
    assert_eq!(
        harness.params("session/resume"),
        Vec::<serde_json::Value>::new()
    );
    let load = &harness.params("session/load")[0];
    assert_eq!(load["sessionId"], json!("harness-session-7"));
    assert_eq!(load["cwd"], json!("/work/repo"));
    assert_eq!(load["mcpServers"], json!([]));

    session.prompt("Continue").expect("the prompt is sent");
    assert!(matches!(
        next(&mut events).await,
        SessionEvent::AgentMessage { text, .. } if text == "new answer"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_fails_when_the_harness_declares_neither() {
    let (harness, opened) = pair(Script::default(), restore("harness-session-7")).await;

    assert!(matches!(opened, Err(CodingError::CannotRestore)));
    let methods: Vec<String> = harness.received().into_iter().map(|r| r.method).collect();
    assert_eq!(methods, ["initialize"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_new_that_needs_a_sign_in_is_auth_required() {
    let script = Script::default().new_session_auth_required();
    let (_harness, opened) = pair(script, fake::new_session("/work/repo")).await;

    assert!(matches!(opened, Err(CodingError::AuthRequired)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_that_needs_a_sign_in_ends_the_turn_with_sign_in_required() {
    let script = Script::default().turn(Turn::auth_required());
    let (_harness, session, mut events) = open(script, fake::new_session("/work/repo")).await;

    session.prompt("Refactor").expect("the prompt is sent");

    assert_eq!(next(&mut events).await, SessionEvent::SignInRequired);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_protocol_version_is_a_protocol_error() {
    let script = Script::default().protocol_version(2);
    let (harness, opened) = pair(script, fake::new_session("/work/repo")).await;

    assert!(matches!(opened, Err(CodingError::Protocol(_))));
    assert_eq!(
        harness.params("session/new"),
        Vec::<serde_json::Value>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_stream_gives_closed_and_a_prompt_is_closed() {
    let (harness, session, mut events) =
        open(Script::default(), fake::new_session("/work/repo")).await;

    harness.close_stream();

    assert_eq!(next(&mut events).await, SessionEvent::Closed);
    assert_eq!(session.prompt("Anyone?"), Err(CodingError::Closed));
    assert_eq!(session.cancel(), Err(CodingError::Closed));
    assert_eq!(session.set_mode("plan").await, Err(CodingError::Closed));
}

#[tokio::test(flavor = "multi_thread")]
async fn close_ends_the_connection() {
    let (_harness, session, mut events) =
        open(Script::default(), fake::new_session("/work/repo")).await;

    session.close();

    assert_eq!(next(&mut events).await, SessionEvent::Closed);
    assert!(
        tokio::time::timeout(WAIT, events.recv())
            .await
            .expect("the channel ends in time")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_block_that_is_not_text_becomes_its_marker_line() {
    let chunk = |block| acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(block));
    let updates = vec![
        chunk(acp::ContentBlock::Image(acp::ImageContent::new(
            "aGVsbG8=",
            "image/png",
        ))),
        chunk(acp::ContentBlock::Audio(acp::AudioContent::new(
            "aGVsbG8=",
            "audio/wav",
        ))),
        chunk(acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
            "main.rs",
            "file:///work/repo/main.rs",
        ))),
        chunk(acp::ContentBlock::Resource(acp::EmbeddedResource::new(
            acp::EmbeddedResourceResource::TextResourceContents(acp::TextResourceContents::new(
                "fn main() {}",
                "file:///work/repo/lib.rs",
            )),
        ))),
        acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Image(
            acp::ImageContent::new("aGVsbG8=", "image/png"),
        ))),
    ];
    let script = Script::default().turn(Turn::new(updates, acp::StopReason::EndTurn));
    let (_harness, session, mut events) = open(script, fake::new_session("/work/repo")).await;

    session.prompt("Show me").expect("the prompt is sent");

    let mut texts = Vec::new();
    for _ in 0..4 {
        let SessionEvent::AgentMessage { text, .. } = next(&mut events).await else {
            panic!("an agent message comes");
        };
        texts.push(text);
    }
    assert_eq!(
        texts,
        [
            "[image]",
            "[audio]",
            "[resource file:///work/repo/main.rs]",
            "[resource file:///work/repo/lib.rs]",
        ]
    );
    assert_eq!(
        next(&mut events).await,
        SessionEvent::Thought {
            text: "[image]".to_owned()
        }
    );
}

#[test]
fn an_event_serializes_with_its_kind() {
    let event = SessionEvent::TurnEnded {
        stop_reason: StopReason::MaxTurnRequests,
    };

    assert_eq!(
        serde_json::to_value(&event).expect("an event serializes"),
        json!({ "kind": "turn_ended", "stop_reason": "max_turn_requests" })
    );
}

/// The modes of Claude Code, as the fake offers them.
fn claude_modes() -> Script {
    Script::default().modes(
        "default",
        &[
            ("default", "Manual"),
            ("plan", "Plan"),
            ("bypassPermissions", "Bypass permissions"),
        ],
    )
}

fn claude_mode_state(current: &str) -> SessionModes {
    let mode = |id: &str, name: &str| HarnessModeInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        description: None,
    };
    SessionModes {
        current: current.to_owned(),
        available: vec![
            mode("default", "Manual"),
            mode("plan", "Plan"),
            mode("bypassPermissions", "Bypass permissions"),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_modes_are_those_of_the_answer_of_session_new() {
    let (_harness, session, _events) = open(claude_modes(), fake::new_session("/work/repo")).await;

    assert_eq!(session.modes(), Some(&claude_mode_state("default")));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_modes_are_those_of_the_answer_of_session_resume() {
    let script = claude_modes().resume();
    let (harness, session, _events) = open(script, restore("harness-session-7")).await;

    assert_eq!(harness.params("session/resume").len(), 1);
    assert_eq!(session.modes(), Some(&claude_mode_state("default")));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_modes_are_those_of_the_answer_of_session_load() {
    let script = claude_modes().load();
    let (harness, session, _events) = open(script, restore("harness-session-7")).await;

    assert_eq!(harness.params("session/load").len(), 1);
    assert_eq!(session.modes(), Some(&claude_mode_state("default")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_harness_that_answers_no_modes_has_none() {
    let (_harness, session, _events) =
        open(Script::default(), fake::new_session("/work/repo")).await;

    assert_eq!(session.modes(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_mode_sends_the_session_id_and_the_mode_id() {
    let (harness, session, _events) = open(claude_modes(), fake::new_session("/work/repo")).await;

    session
        .set_mode("plan")
        .await
        .expect("the harness offers plan");

    assert_eq!(
        harness.params("session/set_mode"),
        [json!({ "sessionId": NEW_SESSION_ID, "modeId": "plan" })]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_mode_to_a_mode_that_the_harness_does_not_offer_is_its_error() {
    let (harness, session, _events) = open(claude_modes(), fake::new_session("/work/repo")).await;

    let error = session
        .set_mode("dontAsk")
        .await
        .expect_err("the harness does not offer dontAsk");

    assert!(matches!(error, CodingError::Protocol(_)), "{error:?}");
    assert_eq!(harness.params("session/set_mode").len(), 1);
}
