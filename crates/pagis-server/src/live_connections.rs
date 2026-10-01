//! The live connections of each Session, and the signal that ends them.
//!
//! A socket and a Media Relay path check the Session once, when they
//! open, and then live on. The Session can end while they are open: the
//! Person signs out or removes it from their Sessions list; an
//! Administrator disables the Person, resets their password or sets a
//! way in for them; or the Session reaches its expiry. A use of the
//! Session moves its expiry, and the expiry here moves with it. Each
//! Session that holds a live connection has one signal
//! here. Every socket and every Media Relay path of that Session waits
//! on the signal and closes when it fires. The OWASP WebSocket Security
//! Cheat Sheet recommends this: authority is checked during the
//! connection, and the connection closes when the session ends.
//!
//! A socket closes with 1008, policy violation, and not with a silent
//! drop, so a client tells the end of its Session from a network fault
//! and shows the sign-in page instead of reconnecting.
//!
//! The signal is in the daemon's memory and is not stored. Presence and
//! the sockets are in memory too, and a restart of the daemon ends them
//! all.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::extract::ws::{CloseFrame, Message as WsMessage, WebSocket, close_code};
use axum::http::request::Parts;
use pagis_core::{Clock, SessionId, UnixMillis, UserId};
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// How long a socket that the daemon closes waits for the Close of the
/// client.
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// The signal of each Session that holds a live connection, by Session
/// and by Person.
pub struct LiveConnections {
    clock: Arc<dyn Clock>,
    sessions: Mutex<HashMap<SessionId, LiveSession>>,
}

/// The signal of one Session, the Person the Session belongs to, and
/// when it expires.
struct LiveSession {
    user_id: UserId,
    ended: CancellationToken,
    expires_at: Arc<AtomicI64>,
}

impl LiveConnections {
    /// The registry of one daemon. `clock` is the clock `auth::resolve`
    /// reads, so a socket obeys the same expiry as a request.
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// The signal that fires when the Session of `tenant` ends. The
    /// first live connection of a Session makes the signal, and a timer
    /// that fires it at the Session's expiry.
    pub fn bind(self: &Arc<Self>, tenant: &Tenant) -> CancellationToken {
        let (ended, expires_at) = {
            let mut sessions = self.sessions.lock().expect("live connections lock");
            if let Some(session) = sessions.get(&tenant.session_id) {
                session
                    .expires_at
                    .fetch_max(tenant.session_expires_at, Ordering::SeqCst);
                return session.ended.child_token();
            }
            let ended = CancellationToken::new();
            let expires_at = Arc::new(AtomicI64::new(tenant.session_expires_at));
            sessions.insert(
                tenant.session_id.clone(),
                LiveSession {
                    user_id: tenant.user_id.clone(),
                    ended: ended.clone(),
                    expires_at: Arc::clone(&expires_at),
                },
            );
            (ended, expires_at)
        };
        let connections = Arc::clone(self);
        let session_id = tenant.session_id.clone();
        let signal = ended.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = signal.cancelled() => {}
                () = reach_expiry(connections.clock.as_ref(), &expires_at) => {
                    connections.end_session(&session_id);
                }
            }
        });
        ended.child_token()
    }

    /// Move the expiry of one Session that holds a live connection: a
    /// request used the Session. An expiry never moves back.
    pub fn extend(&self, session_id: &SessionId, expires_at: UnixMillis) {
        if let Some(session) = self
            .sessions
            .lock()
            .expect("live connections lock")
            .get(session_id)
        {
            session.expires_at.fetch_max(expires_at, Ordering::SeqCst);
        }
    }

    /// End one Session: the Person signed out of it.
    pub fn end_session(&self, session_id: &SessionId) {
        let ended = self
            .sessions
            .lock()
            .expect("live connections lock")
            .remove(session_id);
        if let Some(session) = ended {
            session.ended.cancel();
        }
    }

    /// End every Session of one Person: an Administrator disabled the
    /// Person, reset their password or set a way in for them.
    pub fn end_person(&self, user_id: &UserId) {
        self.sessions
            .lock()
            .expect("live connections lock")
            .retain(|_, session| {
                if session.user_id != *user_id {
                    return true;
                }
                session.ended.cancel();
                false
            });
    }
}

/// Complete when `clock` reaches the expiry. An expiry that moved while
/// the wait ran is waited for again.
async fn reach_expiry(clock: &dyn Clock, expires_at: &AtomicI64) {
    loop {
        let at = expires_at.load(Ordering::SeqCst);
        reach(clock, at).await;
        if expires_at.load(Ordering::SeqCst) <= at {
            return;
        }
    }
}

/// Complete when `clock` reaches `at`. A logical clock that moves wakes
/// the wait at once.
async fn reach(clock: &dyn Clock, at: UnixMillis) {
    loop {
        // The wait on a change starts before the read of the time, so a
        // change between the two is not lost.
        let changed = clock.changed();
        let now = clock.now_ms();
        if now >= at {
            return;
        }
        let left = Duration::from_millis(u64::try_from(at - now).unwrap_or(0));
        tokio::select! {
            () = tokio::time::sleep(left) => {}
            () = changed => {}
        }
    }
}

/// The Tenant of a request that opens a live connection, and the signal
/// that fires when its Session ends.
pub struct LiveTenant {
    pub tenant: Tenant,
    pub session_ended: CancellationToken,
}

impl FromRequestParts<Arc<AppState>> for LiveTenant {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let tenant = Tenant::from_request_parts(parts, state).await?;
        let session_ended = state.live_connections.bind(&tenant);
        // The middleware read the Session before the signal existed. An
        // end deletes the Session record first and fires the signal
        // after, so an end between the two reads is found here: the
        // record is gone.
        let again = crate::auth::resolve(state, &parts.headers).await?;
        if again.is_none_or(|again| again.session_id != tenant.session_id) {
            state.live_connections.end_session(&tenant.session_id);
            return Err(ApiError::unauthorized());
        }
        Ok(Self {
            tenant,
            session_ended,
        })
    }
}

/// Close a socket whose Session ended, with 1008.
pub(crate) async fn close_for_ended_session(socket: &mut WebSocket) -> anyhow::Result<()> {
    socket
        .send(WsMessage::Close(Some(CloseFrame {
            code: close_code::POLICY,
            reason: "the session ended".into(),
        })))
        .await?;
    // The client answers with a Close of its own. The daemon reads it
    // before the connection ends, so the client reads the Close and not
    // a reset connection.
    let _ = tokio::time::timeout(CLOSE_GRACE, async {
        while let Some(Ok(_)) = socket.recv().await {}
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use pagis_core::{UserRole, WorkspaceId};

    use super::*;

    fn tenant(user_id: &UserId, expires_at: UnixMillis) -> Tenant {
        Tenant {
            workspace_id: WorkspaceId::generate(),
            user_id: user_id.clone(),
            role: UserRole::Member,
            session_id: SessionId::generate(),
            session_expires_at: expires_at,
        }
    }

    fn later() -> UnixMillis {
        pagis_core::now_ms() + 60 * 60 * 1_000
    }

    #[tokio::test]
    async fn the_end_of_a_session_fires_its_signal_and_no_other() {
        let connections = LiveConnections::new(Arc::new(pagis_core::SystemClock));
        let person = UserId::generate();
        let signing_out = tenant(&person, later());
        let other = tenant(&person, later());
        let first = connections.bind(&signing_out);
        let second = connections.bind(&signing_out);
        let stays = connections.bind(&other);

        connections.end_session(&signing_out.session_id);

        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        assert!(!stays.is_cancelled());
    }

    #[tokio::test]
    async fn the_end_of_a_person_fires_the_signal_of_each_of_their_sessions() {
        let connections = LiveConnections::new(Arc::new(pagis_core::SystemClock));
        let person = UserId::generate();
        let laptop = connections.bind(&tenant(&person, later()));
        let phone = connections.bind(&tenant(&person, later()));
        let someone_else = connections.bind(&tenant(&UserId::generate(), later()));

        connections.end_person(&person);

        assert!(laptop.is_cancelled());
        assert!(phone.is_cancelled());
        assert!(!someone_else.is_cancelled());
    }

    /// A use of the Session moves its expiry, and the signal waits for
    /// the new one.
    #[tokio::test]
    async fn a_signal_waits_for_an_expiry_that_moved() {
        let connections = LiveConnections::new(Arc::new(pagis_core::SystemClock));
        let session = tenant(&UserId::generate(), pagis_core::now_ms() + 50);
        let expiring = connections.bind(&session);

        connections.extend(&session.session_id, later());
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !expiring.is_cancelled(),
            "the signal fired at the expiry the Session had before its use"
        );

        connections.end_session(&session.session_id);
        assert!(expiring.is_cancelled());
    }

    #[tokio::test]
    async fn a_signal_fires_at_the_expiry_of_its_session() {
        let connections = LiveConnections::new(Arc::new(pagis_core::SystemClock));
        let expiring = connections.bind(&tenant(&UserId::generate(), pagis_core::now_ms() + 50));

        tokio::time::timeout(Duration::from_secs(5), expiring.cancelled())
            .await
            .expect("the signal fires at the expiry");
    }
}
