//! The permission requests and questions of a harness, against the fake
//! harness over `tokio::io::duplex`. The test `AskHandler` hands each ask
//! to the test, and the test answers it through a channel.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_coding::fake::{self, Ask, Script, Turn, acp};
use pagis_coding::{
    AcpSession, AskHandler, PermissionAnswer, PermissionAsk, PermissionOptionKind, QuestionAnswer,
    QuestionAsk, SessionEvent, StopReason, ToolKind,
};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

const WAIT: Duration = Duration::from_secs(10);

/// One ask that reached the handler, with the sender of its answer.
enum Asked {
    Permission(PermissionAsk, oneshot::Sender<PermissionAnswer>),
    Question(QuestionAsk, oneshot::Sender<QuestionAnswer>),
}

/// Hands each ask to the test and waits for the test's answer.
struct ChannelAsks(UnboundedSender<Asked>);

#[async_trait]
impl AskHandler for ChannelAsks {
    async fn permission(&self, ask: PermissionAsk) -> PermissionAnswer {
        let (answer, answered) = oneshot::channel();
        self.0
            .send(Asked::Permission(ask, answer))
            .expect("the test reads the asks");
        answered.await.expect("the test answers the ask")
    }

    async fn question(&self, ask: QuestionAsk) -> QuestionAnswer {
        let (answer, answered) = oneshot::channel();
        self.0
            .send(Asked::Question(ask, answer))
            .expect("the test reads the asks");
        answered.await.expect("the test answers the ask")
    }
}

struct Opened {
    harness: fake::FakeHarness,
    session: AcpSession,
    events: UnboundedReceiver<SessionEvent>,
    asks: UnboundedReceiver<Asked>,
}

async fn open(script: Script) -> Opened {
    let (asks_tx, asks) = mpsc::unbounded_channel();
    let (harness, opened) = fake::pair(
        script,
        fake::new_session("/work/repo"),
        Arc::new(ChannelAsks(asks_tx)),
    )
    .await;
    let (session, events) = opened.expect("the session opens");
    Opened {
        harness,
        session,
        events,
        asks,
    }
}

async fn next(events: &mut UnboundedReceiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("an event comes in time")
        .expect("the event channel is open")
}

async fn next_ask(asks: &mut UnboundedReceiver<Asked>) -> Asked {
    tokio::time::timeout(WAIT, asks.recv())
        .await
        .expect("an ask comes in time")
        .expect("the ask channel is open")
}

async fn next_permission(
    asks: &mut UnboundedReceiver<Asked>,
) -> (PermissionAsk, oneshot::Sender<PermissionAnswer>) {
    match next_ask(asks).await {
        Asked::Permission(ask, answer) => (ask, answer),
        Asked::Question(..) => panic!("a permission request comes, not a question"),
    }
}

async fn next_question(
    asks: &mut UnboundedReceiver<Asked>,
) -> (QuestionAsk, oneshot::Sender<QuestionAnswer>) {
    match next_ask(asks).await {
        Asked::Question(ask, answer) => (ask, answer),
        Asked::Permission(..) => panic!("a question comes, not a permission request"),
    }
}

fn ended(stop_reason: StopReason) -> SessionEvent {
    SessionEvent::TurnEnded { stop_reason }
}

fn run_tests() -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new(
        "call-1",
        acp::ToolCallUpdateFields::new()
            .title("Run cargo test")
            .kind(acp::ToolKind::Execute)
            .locations(vec![acp::ToolCallLocation::new("/work/repo")])
            .raw_input(json!({ "command": "cargo test" })),
    )
}

fn option(id: &str, kind: acp::PermissionOptionKind) -> acp::PermissionOption {
    acp::PermissionOption::new(id.to_owned(), id.to_owned(), kind)
}

fn every_option() -> Vec<acp::PermissionOption> {
    vec![
        option("allow-always", acp::PermissionOptionKind::AllowAlways),
        option("allow-once", acp::PermissionOptionKind::AllowOnce),
        option("reject-always", acp::PermissionOptionKind::RejectAlways),
        option("reject-once", acp::PermissionOptionKind::RejectOnce),
    ]
}

fn asks_to_run_tests() -> Ask {
    Ask::permission(run_tests(), every_option())
}

fn selected(option_id: &str) -> Value {
    json!({ "outcome": { "outcome": "selected", "optionId": option_id } })
}

fn cancelled() -> Value {
    json!({ "outcome": { "outcome": "cancelled" } })
}

fn branch_schema() -> acp::ElicitationSchema {
    acp::ElicitationSchema::new()
        .string("branch", true)
        .boolean("push", false)
}

fn asks_for_the_branch() -> Ask {
    Ask::form("Which branch?", branch_schema())
}

fn values(value: Value) -> Map<String, Value> {
    let Value::Object(values) = value else {
        panic!("the values are a JSON object");
    };
    values
}

/// The answers that the fake got, each as JSON or as its error code.
fn answers(harness: &fake::FakeHarness) -> Vec<Result<Value, acp::ErrorCode>> {
    harness
        .answers()
        .into_iter()
        .map(|answer| answer.map_err(|error| error.code))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_declares_form_elicitation_and_no_file_system_or_terminal() {
    let Opened { harness, .. } = open(Script::default()).await;

    assert_eq!(
        harness.params("initialize")[0]["clientCapabilities"],
        json!({
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
            "auth": { "terminal": true },
            "elicitation": { "form": {} },
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_request_reaches_the_handler_and_its_answer_selects_the_once_option() {
    let script = Script::default()
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_to_run_tests()))
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_to_run_tests()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (ask, answer) = next_permission(&mut asks).await;
    assert!(!ask.ask_id.is_empty());
    assert_eq!(
        ask,
        PermissionAsk {
            ask_id: ask.ask_id.clone(),
            tool_call_id: "call-1".to_owned(),
            title: Some("Run cargo test".to_owned()),
            kind: ToolKind::Execute,
            locations: vec![PathBuf::from("/work/repo")],
            raw_input: Some(json!({ "command": "cargo test" })),
            options: vec![
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::RejectAlways,
                PermissionOptionKind::RejectOnce,
            ],
        }
    );
    answer
        .send(PermissionAnswer::AllowOnce)
        .expect("the handler waits");
    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));

    session
        .prompt("Run them again")
        .expect("the prompt is sent");
    let (_ask, answer) = next_permission(&mut asks).await;
    answer
        .send(PermissionAnswer::RejectOnce)
        .expect("the handler waits");
    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));

    assert_eq!(
        answers(&harness),
        [Ok(selected("allow-once")), Ok(selected("reject-once"))]
    );
}

/// The Codex adapter reports a command in a `tool_call` update, and then
/// asks for it with a tool call that holds only its id.
#[tokio::test(flavor = "multi_thread")]
async fn a_permission_takes_each_field_that_it_leaves_out_from_the_reported_tool_call() {
    let reported = acp::SessionUpdate::ToolCall(
        acp::ToolCall::new("call-1", "Run cargo test")
            .kind(acp::ToolKind::Execute)
            .locations(vec![acp::ToolCallLocation::new("/work/repo")])
            .raw_input(json!({ "command": "cargo test" })),
    );
    let other = acp::SessionUpdate::ToolCall(
        acp::ToolCall::new("call-2", "Read the notes").kind(acp::ToolKind::Read),
    );
    let ask = Ask::permission(
        acp::ToolCallUpdate::new(
            "call-1",
            acp::ToolCallUpdateFields::new().title("Run cargo test --all"),
        ),
        every_option(),
    )
    .after_updates();
    let script = Script::default()
        .turn(Turn::new(vec![reported, other], acp::StopReason::EndTurn).asks(ask));
    let Opened {
        session, mut asks, ..
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (ask, answer) = next_permission(&mut asks).await;

    assert_eq!(ask.tool_call_id, "call-1");
    assert_eq!(ask.kind, ToolKind::Execute);
    assert_eq!(ask.title.as_deref(), Some("Run cargo test --all"));
    assert_eq!(ask.locations, [PathBuf::from("/work/repo")]);
    assert_eq!(ask.raw_input, Some(json!({ "command": "cargo test" })));
    answer
        .send(PermissionAnswer::AllowOnce)
        .expect("the handler waits");
}

/// A tool call that the harness reported as finished gets no permission,
/// so a request with its id has only its own fields.
#[tokio::test(flavor = "multi_thread")]
async fn a_permission_for_a_finished_tool_call_has_only_its_own_fields() {
    let reported = acp::SessionUpdate::ToolCall(
        acp::ToolCall::new("call-1", "Run cargo test").kind(acp::ToolKind::Execute),
    );
    let finished = acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        "call-1",
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    ));
    let ask = Ask::permission(
        acp::ToolCallUpdate::new("call-1", acp::ToolCallUpdateFields::new()),
        every_option(),
    )
    .after_updates();
    let script = Script::default()
        .turn(Turn::new(vec![reported, finished], acp::StopReason::EndTurn).asks(ask));
    let Opened {
        session, mut asks, ..
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (ask, answer) = next_permission(&mut asks).await;

    assert_eq!(ask.kind, ToolKind::Other);
    assert_eq!(ask.title, None);
    answer
        .send(PermissionAnswer::AllowOnce)
        .expect("the handler waits");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_answer_selects_no_option() {
    let script = Script::default()
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_to_run_tests()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (_ask, answer) = next_permission(&mut asks).await;
    answer
        .send(PermissionAnswer::Cancel)
        .expect("the handler waits");

    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
    assert_eq!(answers(&harness), [Ok(cancelled())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_with_no_offered_option_of_its_kind_is_cancelled() {
    let ask = Ask::permission(
        run_tests(),
        vec![
            option("allow-always", acp::PermissionOptionKind::AllowAlways),
            option("reject-once", acp::PermissionOptionKind::RejectOnce),
        ],
    );
    let script = Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (_ask, answer) = next_permission(&mut asks).await;
    answer
        .send(PermissionAnswer::AllowOnce)
        .expect("the handler waits");

    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
    assert_eq!(answers(&harness), [Ok(cancelled())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn updates_arrive_while_the_handler_waits() {
    let still_working = acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new("still working")),
    ));
    let script = Script::default()
        .turn(Turn::new(vec![still_working], acp::StopReason::EndTurn).asks(asks_to_run_tests()));
    let Opened {
        session,
        mut events,
        mut asks,
        ..
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (_ask, answer) = next_permission(&mut asks).await;

    assert!(matches!(
        next(&mut events).await,
        SessionEvent::AgentMessage { text, .. } if text == "still working"
    ));
    answer
        .send(PermissionAnswer::AllowOnce)
        .expect("the handler waits");
    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_answers_a_pending_permission_request_cancelled_and_withdraws_it() {
    let script = Script::default().turn(Turn::until_cancel(vec![]).asks(asks_to_run_tests()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (ask, answer) = next_permission(&mut asks).await;
    session.cancel().expect("the cancel is sent");

    assert_eq!(
        next(&mut events).await,
        SessionEvent::AskWithdrawn { ask_id: ask.ask_id }
    );
    assert_eq!(next(&mut events).await, ended(StopReason::Cancelled));
    assert_eq!(answers(&harness), [Ok(cancelled())]);
    assert!(
        answer.send(PermissionAnswer::AllowOnce).is_err(),
        "the crate dropped the handler's wait"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_the_harness_withdraws_gets_no_answer_from_the_handler() {
    let script = Script::default()
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_to_run_tests().withdrawn()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Run the tests").expect("the prompt is sent");
    let (ask, answer) = next_permission(&mut asks).await;
    harness.withdraw_ask();

    assert_eq!(
        next(&mut events).await,
        SessionEvent::AskWithdrawn { ask_id: ask.ask_id }
    );
    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
    assert!(
        answer.send(PermissionAnswer::AllowOnce).is_err(),
        "the crate dropped the handler's wait"
    );
    assert_eq!(answers(&harness), [Err(acp::ErrorCode::RequestCancelled)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_form_question_reaches_the_handler_and_each_answer_reaches_the_harness() {
    let script = Script::default()
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_for_the_branch()))
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_for_the_branch()))
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_for_the_branch()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Push the fix").expect("the prompt is sent");
    let (ask, answer) = next_question(&mut asks).await;
    assert!(!ask.ask_id.is_empty());
    assert_eq!(ask.message, "Which branch?");
    assert_eq!(
        ask.schema,
        serde_json::to_value(branch_schema()).expect("a schema converts to JSON")
    );
    assert_eq!(ask.schema["properties"]["branch"]["type"], json!("string"));
    answer
        .send(QuestionAnswer::Accept(values(
            json!({ "branch": "main", "push": true }),
        )))
        .expect("the handler waits");
    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));

    for answer_with in [QuestionAnswer::Decline, QuestionAnswer::Cancel] {
        session.prompt("Push the fix").expect("the prompt is sent");
        let (_ask, answer) = next_question(&mut asks).await;
        answer.send(answer_with).expect("the handler waits");
        assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
    }

    assert_eq!(
        answers(&harness),
        [
            Ok(json!({ "action": "accept", "content": { "branch": "main", "push": true } })),
            Ok(json!({ "action": "decline" })),
            Ok(json!({ "action": "cancel" })),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_accepted_value_that_acp_cannot_carry_answers_cancel() {
    let script = Script::default()
        .turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(asks_for_the_branch()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Push the fix").expect("the prompt is sent");
    let (_ask, answer) = next_question(&mut asks).await;
    answer
        .send(QuestionAnswer::Accept(values(
            json!({ "branch": { "name": "main" } }),
        )))
        .expect("the handler waits");

    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
    assert_eq!(answers(&harness), [Ok(json!({ "action": "cancel" }))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_answers_a_pending_question_cancel_and_withdraws_it() {
    let script = Script::default().turn(Turn::until_cancel(vec![]).asks(asks_for_the_branch()));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Push the fix").expect("the prompt is sent");
    let (ask, _answer) = next_question(&mut asks).await;
    session.cancel().expect("the cancel is sent");

    assert_eq!(
        next(&mut events).await,
        SessionEvent::AskWithdrawn { ask_id: ask.ask_id }
    );
    assert_eq!(next(&mut events).await, ended(StopReason::Cancelled));
    assert_eq!(answers(&harness), [Ok(json!({ "action": "cancel" }))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_url_question_is_declined_and_the_handler_sees_no_call() {
    let ask = Ask::url("Sign in to continue", "https://example.com/login");
    let script = Script::default().turn(Turn::new(vec![], acp::StopReason::EndTurn).asks(ask));
    let Opened {
        harness,
        session,
        mut events,
        mut asks,
    } = open(script).await;

    session.prompt("Deploy").expect("the prompt is sent");

    assert_eq!(next(&mut events).await, ended(StopReason::EndTurn));
    assert_eq!(answers(&harness), [Ok(json!({ "action": "decline" }))]);
    assert!(asks.try_recv().is_err(), "the handler saw no call");
}
