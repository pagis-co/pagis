//! The line of a carrier credential (ADR-0020) on a paused clock:
//! registration, refresh at half the expiry, expiry at the registrar,
//! failure, and backoff to the cap. The transport is a fake, so no
//! socket opens.

use std::sync::Arc;
use std::time::Duration;

use pagis_telephony::fake::{FakeCallTransport, FakeNumberDirectory, LineCall, TokioClock};
use pagis_telephony::{
    EndpointTask, INITIAL_BACKOFF, MAX_BACKOFF, REQUESTED_EXPIRY, RegistrationFailure,
    RegistrationState, SipCredential, TransportErrorCode,
};
use tokio::time::Instant;

const USERNAME: &str = "robin";

fn credential() -> SipCredential {
    SipCredential::new(USERNAME, "secret", "sip.telnyx.com")
}

fn transport() -> Arc<FakeCallTransport> {
    Arc::new(FakeCallTransport::new(Arc::new(TokioClock)))
}

/// No call arrives in these tests, so nobody takes answered calls.
fn nobody() -> tokio::sync::mpsc::Sender<pagis_telephony::IncomingHub> {
    tokio::sync::mpsc::channel(1).0
}

fn start(transport: &Arc<FakeCallTransport>) -> EndpointTask {
    EndpointTask::spawn(
        Arc::clone(transport) as _,
        Some(credential()),
        Arc::new(FakeNumberDirectory::default()),
        nobody(),
    )
}

/// Wait, in paused time, until the task reports this state.
async fn settled(task: &EndpointTask, state: RegistrationState) {
    let mut watch = task.watch();
    tokio::time::timeout(Duration::from_secs(60), watch.wait_for(|now| *now == state))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the line never reached {state:?}; it is at {:?}",
                task.state()
            )
        })
        .expect("the task dropped its state");
}

/// When each `REGISTER` reached the fake registrar.
fn register_times(transport: &FakeCallTransport) -> Vec<Instant> {
    transport
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            LineCall::Register { at, .. } => Some(at),
            _ => None,
        })
        .collect()
}

fn gaps(times: &[Instant]) -> Vec<Duration> {
    times.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

#[tokio::test(start_paused = true)]
async fn a_started_line_registers_and_refreshes_at_half_the_expiry() {
    let transport = transport();
    let task = start(&transport);

    settled(&task, RegistrationState::Registered).await;
    assert!(transport.is_registered(USERNAME));
    let calls = transport.calls();
    assert!(matches!(&calls[0], LineCall::Open { username } if username == USERNAME));
    assert!(matches!(
        &calls[1],
        LineCall::Register { expires, .. } if *expires == REQUESTED_EXPIRY
    ));

    tokio::time::sleep(REQUESTED_EXPIRY / 2 - Duration::from_secs(1)).await;
    assert_eq!(register_times(&transport).len(), 1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let times = register_times(&transport);
    assert_eq!(times.len(), 2);
    assert_eq!(gaps(&times), vec![REQUESTED_EXPIRY / 2]);
    assert_eq!(task.state(), RegistrationState::Registered);
    // A refresh runs on the socket that registered; nothing reopened.
    assert_eq!(
        transport
            .calls()
            .iter()
            .filter(|call| matches!(call, LineCall::Open { .. }))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn the_refresh_follows_the_expiry_the_registrar_granted() {
    let transport = transport();
    transport.grant_expiry(Some(Duration::from_secs(60)));
    let task = start(&transport);

    settled(&task, RegistrationState::Registered).await;
    tokio::time::sleep(Duration::from_secs(61)).await;

    let times = register_times(&transport);
    assert_eq!(times.len(), 3);
    assert_eq!(
        gaps(&times),
        vec![Duration::from_secs(30), Duration::from_secs(30)]
    );
}

#[tokio::test(start_paused = true)]
async fn a_refresh_that_fails_marks_the_line_failed_and_the_registrar_forgets_it() {
    let transport = transport();
    transport.grant_expiry(Some(Duration::from_secs(60)));
    let task = start(&transport);
    settled(&task, RegistrationState::Registered).await;

    transport.fail_with(Some(TransportErrorCode::Unreachable));
    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(
        task.state(),
        RegistrationState::Failed(RegistrationFailure::Unreachable)
    );
    // The registrar still holds the binding until the expiry it
    // granted, and then it does not.
    assert!(transport.is_registered(USERNAME));
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(!transport.is_registered(USERNAME));

    // Once the registrar answers again, a retry registers afresh.
    transport.fail_with(None);
    settled(&task, RegistrationState::Registered).await;
    assert!(transport.is_registered(USERNAME));
}

#[tokio::test(start_paused = true)]
async fn a_failed_registration_backs_off_to_the_cap() {
    let transport = transport();
    transport.fail_with(Some(TransportErrorCode::Unreachable));
    let task = start(&transport);

    tokio::time::sleep(Duration::from_secs(200)).await;

    assert_eq!(
        task.state(),
        RegistrationState::Failed(RegistrationFailure::Unreachable)
    );
    let gaps = gaps(&register_times(&transport));
    assert_eq!(
        &gaps[..7],
        &[
            INITIAL_BACKOFF,
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(8),
            Duration::from_secs(16),
            MAX_BACKOFF,
            MAX_BACKOFF,
        ]
    );
    assert!(gaps.iter().all(|gap| *gap <= MAX_BACKOFF));
}

#[tokio::test(start_paused = true)]
async fn a_success_resets_the_backoff() {
    let transport = transport();
    transport.fail_with(Some(TransportErrorCode::Unreachable));
    let task = start(&transport);

    // Attempts at 0, 1 and 3 s fail; the one at 7 s succeeds.
    tokio::time::sleep(Duration::from_secs(4)).await;
    transport.fail_with(None);
    settled(&task, RegistrationState::Registered).await;
    assert_eq!(register_times(&transport).len(), 4);

    // The refresh fails, and the retry after it waits the initial
    // backoff again, not the one the earlier failures had reached.
    transport.fail_with(Some(TransportErrorCode::Unreachable));
    tokio::time::sleep(REQUESTED_EXPIRY / 2 + Duration::from_millis(1500)).await;
    let times = register_times(&transport);
    assert_eq!(times.len(), 6);
    assert_eq!(times[5] - times[4], INITIAL_BACKOFF);
}

#[tokio::test(start_paused = true)]
async fn a_credential_the_registrar_refuses_reports_unauthorized() {
    let transport = transport();
    transport.fail_with(Some(TransportErrorCode::Unauthorized));
    let task = start(&transport);

    settled(
        &task,
        RegistrationState::Failed(RegistrationFailure::Unauthorized),
    )
    .await;
    assert!(!transport.is_registered(USERNAME));
}

#[tokio::test(start_paused = true)]
async fn a_line_with_no_credential_asks_the_carrier_nothing() {
    let transport = transport();
    let task = EndpointTask::spawn(
        Arc::clone(&transport) as _,
        None,
        Arc::new(FakeNumberDirectory::default()),
        nobody(),
    );

    settled(
        &task,
        RegistrationState::Failed(RegistrationFailure::NoCredential),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert!(transport.calls().is_empty());

    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn stopping_a_line_unregisters_it() {
    let transport = transport();
    let task = start(&transport);
    settled(&task, RegistrationState::Registered).await;
    let watch = task.watch();

    task.stop().await;

    assert_eq!(*watch.borrow(), RegistrationState::Unregistered);
    assert!(!transport.is_registered(USERNAME));
    assert!(matches!(
        transport.calls().last(),
        Some(LineCall::Unregister { username }) if username == USERNAME
    ));
    let before = transport.calls().len();
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(transport.calls().len(), before);
}
