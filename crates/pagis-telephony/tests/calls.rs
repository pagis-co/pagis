//! A call on the fake carrier (ADR-0020), on a paused clock: the line
//! places it and answers it, one media hub carries audio and events
//! both ways, and one call per number is the rule.

use std::sync::Arc;
use std::time::Duration;

use pagis_core::PhoneNumber;
use pagis_telephony::audio::{Codec, FRAME_BYTES, Frame};
use pagis_telephony::fake::{
    FakeCallTransport, FakeNumberDirectory, LineCall, PartyState, RemoteParty, TokioClock,
};
use pagis_telephony::hub::{HubEvent, MediaHub};
use pagis_telephony::leg::EndedReason;
use pagis_telephony::{
    CallError, EndpointTask, IncomingHub, Refusal, RegistrationState, SipCredential,
    TransportErrorCode,
};
use tokio::sync::{broadcast, mpsc};

const NUMBER: &str = "+14155550123";
const OTHER: &str = "+14155550124";
const CALLEE: &str = "+14155550199";
const CALLER: &str = "+14155550100";

struct World {
    transport: Arc<FakeCallTransport>,
    task: EndpointTask,
    incoming: mpsc::Receiver<IncomingHub>,
    /// The number an Agent holds on the line.
    number: PhoneNumber,
    /// A second number on the same line, which another Agent holds.
    other: PhoneNumber,
}

async fn registered() -> World {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let (calls, incoming) = mpsc::channel(4);
    let number = FakeNumberDirectory::held(NUMBER);
    let other = FakeNumberDirectory::held(OTHER);
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
        Arc::new(FakeNumberDirectory::with(vec![
            number.clone(),
            other.clone(),
        ])),
        calls,
    );
    let mut watch = task.watch();
    watch
        .wait_for(|state| *state == RegistrationState::Registered)
        .await
        .unwrap();
    World {
        transport,
        task,
        incoming,
        number,
        other,
    }
}

/// Wait, in paused time, for the next event of a kind.
async fn next_event(rx: &mut broadcast::Receiver<HubEvent>, wanted: impl Fn(&HubEvent) -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match rx.recv().await {
                Ok(event) if wanted(&event) => return,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("the hub closed"),
            }
        }
    })
    .await
    .expect("the event never came");
}

async fn party_reaches(party: &RemoteParty, state: PartyState) {
    let mut watch = party.watch();
    tokio::time::timeout(Duration::from_secs(30), watch.wait_for(|now| *now == state))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the party never reached {state:?}; it is at {:?}",
                party.state()
            )
        })
        .unwrap();
}

/// A frame whose first byte names it.
fn frame(byte: u8) -> Frame {
    Frame::new(Codec::Pcmu, vec![byte; FRAME_BYTES])
}

#[tokio::test(start_paused = true)]
async fn a_placed_call_carries_audio_and_events_both_ways() {
    let world = registered().await;

    let hub: Arc<MediaHub> = world.task.place_call(&world.number, CALLEE).await.unwrap();
    let mut events = hub.subscribe();
    let party = world
        .transport
        .dials()
        .pop()
        .expect("the carrier saw no INVITE");
    assert_eq!(party.to(), CALLEE);
    assert_eq!(party.from(), NUMBER);
    assert!(matches!(
        world.transport.calls().last(),
        Some(LineCall::Dial { from, to }) if from == NUMBER && to == CALLEE
    ));
    assert!(world.task.active_call(&world.number.id).is_some());

    party.ring();
    next_event(&mut events, |event| matches!(event, HubEvent::Ringing)).await;
    party.answer();
    next_event(&mut events, |event| matches!(event, HubEvent::Answered)).await;

    // The Remote Party speaks, and the model hears it in order.
    party.speak(frame(1));
    party.speak(frame(2));
    next_event(
        &mut events,
        |event| matches!(event, HubEvent::Uplink(frame) if frame.payload()[0] == 2),
    )
    .await;

    // The model speaks, and the Remote Party hears it, paced.
    hub.send_downlink(frame(7)).await;
    hub.send_downlink(frame(8)).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    let heard: Vec<u8> = party
        .heard()
        .iter()
        .map(|packet| packet.payload[0])
        .filter(|byte| *byte != 0xFF)
        .collect();
    assert_eq!(heard, vec![7, 8]);
    // Silence went out before and between: the RTP never stopped.
    assert!(party.heard().len() > 2);

    // Digits go both ways.
    party.press('7');
    next_event(&mut events, |event| matches!(event, HubEvent::Dtmf('7'))).await;
    hub.send_digits("12#").await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(party.digits(), vec!['1', '2', '#']);

    // Pagis hangs up; the far side sees it, and the number is free.
    hub.hangup().await;
    next_event(&mut events, |event| {
        matches!(event, HubEvent::Ended(EndedReason::LocalHangup))
    })
    .await;
    party_reaches(&party, PartyState::Ended(EndedReason::LocalHangup)).await;
    assert!(world.task.active_call(&world.number.id).is_none());
    world.task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_far_side_hanging_up_ends_the_call_with_remote_hangup() {
    let world = registered().await;
    let hub = world.task.place_call(&world.number, CALLEE).await.unwrap();
    let mut events = hub.subscribe();
    let party = world.transport.dials().pop().unwrap();
    party.answer();

    party.hangup();

    next_event(&mut events, |event| {
        matches!(event, HubEvent::Ended(EndedReason::RemoteHangup))
    })
    .await;
    assert_eq!(hub.ended(), Some(EndedReason::RemoteHangup));
    assert!(world.task.active_call(&world.number.id).is_none());
    world.task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_busy_callee_ends_the_call_with_busy() {
    let world = registered().await;
    let hub = world.task.place_call(&world.number, CALLEE).await.unwrap();
    let party = world.transport.dials().pop().unwrap();

    party.refuse(EndedReason::Busy);

    let mut ended = hub.watch_ended();
    ended.wait_for(|reason| reason.is_some()).await.unwrap();
    assert_eq!(hub.ended(), Some(EndedReason::Busy));
    world.task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_inbound_call_is_answered_and_handed_over() {
    let mut world = registered().await;

    let party = world.transport.ring(NUMBER, CALLER);

    let answered = tokio::time::timeout(Duration::from_secs(5), world.incoming.recv())
        .await
        .expect("no call was handed over")
        .expect("the endpoint dropped its calls");
    assert_eq!(answered.e164, NUMBER);
    assert_eq!(answered.workspace_id, world.number.workspace_id);
    assert_eq!(answered.phone_number_id, world.number.id);
    assert_eq!(answered.from_e164, CALLER);
    party_reaches(&party, PartyState::Answered).await;
    let mut events = answered.hub.subscribe();
    party.speak(frame(3));
    next_event(
        &mut events,
        |event| matches!(event, HubEvent::Uplink(frame) if frame.payload()[0] == 3),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!party.heard().is_empty());

    answered.hub.hangup().await;
    party_reaches(&party, PartyState::Ended(EndedReason::LocalHangup)).await;
    world.task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_second_inbound_call_gets_busy_here_until_the_first_ends() {
    let mut world = registered().await;
    let first = world.transport.ring(NUMBER, CALLER);
    let answered = world.incoming.recv().await.unwrap();
    party_reaches(&first, PartyState::Answered).await;

    let second = world.transport.ring(NUMBER, "+14155550101");
    party_reaches(&second, PartyState::Refused(Refusal::Busy)).await;
    assert_eq!(first.state(), PartyState::Answered);

    answered.hub.hangup().await;
    party_reaches(&first, PartyState::Ended(EndedReason::LocalHangup)).await;
    let third = world.transport.ring(NUMBER, "+14155550102");
    party_reaches(&third, PartyState::Answered).await;
    world.task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn placing_a_call_while_one_runs_is_refused_as_busy() {
    let world = registered().await;
    let hub = world.task.place_call(&world.number, CALLEE).await.unwrap();

    let second = world.task.place_call(&world.number, "+14155550198").await;

    assert!(matches!(second, Err(CallError::Busy)));
    assert_eq!(world.transport.dials().len(), 1);
    hub.hangup().await;
    let mut ended = hub.watch_ended();
    ended.wait_for(|reason| reason.is_some()).await.unwrap();
    world
        .task
        .place_call(&world.number, "+14155550198")
        .await
        .unwrap();
    assert_eq!(world.transport.dials().len(), 2);
    world.task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_inbound_call_during_an_outbound_call_is_busy() {
    let world = registered().await;
    let _hub = world.task.place_call(&world.number, CALLEE).await.unwrap();

    let party = world.transport.ring(NUMBER, CALLER);

    party_reaches(&party, PartyState::Refused(Refusal::Busy)).await;
    world.task.stop().await;
}

/// One call for each number, not for each line: a call on one number
/// leaves the other number of the line free in both directions.
#[tokio::test(start_paused = true)]
async fn a_call_on_one_number_leaves_the_other_number_of_the_line_free() {
    let mut world = registered().await;
    let _outbound = world.task.place_call(&world.number, CALLEE).await.unwrap();

    let inbound = world.transport.ring(OTHER, CALLER);
    let answered = tokio::time::timeout(Duration::from_secs(5), world.incoming.recv())
        .await
        .expect("the other number answers")
        .expect("the endpoint dropped its calls");
    assert_eq!(answered.phone_number_id, world.other.id);
    assert_eq!(answered.workspace_id, world.other.workspace_id);
    party_reaches(&inbound, PartyState::Answered).await;

    let busy = world.transport.ring(NUMBER, "+14155550101");
    party_reaches(&busy, PartyState::Refused(Refusal::Busy)).await;
    assert!(matches!(
        world.task.place_call(&world.other, "+14155550198").await,
        Err(CallError::Busy)
    ));
    assert!(matches!(
        world.task.place_call(&world.number, "+14155550198").await,
        Err(CallError::Busy)
    ));
    assert_eq!(
        world
            .transport
            .dials()
            .iter()
            .map(|party| party.from().to_string())
            .collect::<Vec<_>>(),
        vec![NUMBER.to_string()]
    );
    world.task.stop().await;
}

/// The store holds one live record for each number. A dialed number
/// that matches two records names no one Workspace, so the line refuses
/// the call before it answers.
#[tokio::test(start_paused = true)]
async fn a_dialed_number_that_matches_two_records_is_refused_with_480() {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    let (calls, mut incoming) = mpsc::channel(4);
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
        Arc::new(FakeNumberDirectory::with(vec![
            FakeNumberDirectory::held(NUMBER),
            FakeNumberDirectory::held(NUMBER),
        ])),
        calls,
    );
    task.watch()
        .wait_for(|state| *state == RegistrationState::Registered)
        .await
        .unwrap();

    let party = transport.ring(NUMBER, CALLER);

    party_reaches(&party, PartyState::Refused(Refusal::Unavailable)).await;
    assert!(incoming.try_recv().is_err(), "no call was handed over");
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_unregistered_number_places_no_call() {
    let transport = Arc::new(FakeCallTransport::new(Arc::new(TokioClock)));
    transport.fail_with(Some(TransportErrorCode::Unreachable));
    let (calls, _incoming) = mpsc::channel(4);
    let number = FakeNumberDirectory::held(NUMBER);
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        Some(SipCredential::new("robin", "secret", "sip.telnyx.com")),
        Arc::new(FakeNumberDirectory::with(vec![number.clone()])),
        calls,
    );
    let mut watch = task.watch();
    watch
        .wait_for(|state| matches!(state, RegistrationState::Failed(_)))
        .await
        .unwrap();

    let placed = task.place_call(&number, CALLEE).await;

    assert!(matches!(placed, Err(CallError::Unregistered)));
    assert!(transport.dials().is_empty());
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn stopping_the_endpoint_hangs_up_the_call() {
    let world = registered().await;
    let hub = world.task.place_call(&world.number, CALLEE).await.unwrap();
    let party = world.transport.dials().pop().unwrap();
    party.answer();
    party_reaches(&party, PartyState::Answered).await;

    world.task.stop().await;

    assert_eq!(hub.ended(), Some(EndedReason::LocalHangup));
    assert_eq!(party.state(), PartyState::Ended(EndedReason::LocalHangup));
}
