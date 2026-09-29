//! The Trust Tier of one call (ADR-0021).
//!
//! Direction decides what caller ID is worth. Outbound, Pagis dialed
//! the number, so the listed tier holds with no further proof. Inbound,
//! caller ID selects a candidate only, and the Keypad Code confirms it:
//! the tier is `min(candidate, proved)`, so a Trusted-listed number
//! that enters the code becomes Trusted and never Owner.
//!
//! A tier rises once and falls only by hand. The gate below owns that
//! decision for one call: the challenge before the session exists, a
//! late code while the call runs, and the user's drop to Unknown. No
//! tool reaches any of the three, because a tool that raised a tier
//! would be a lever the model can be talked into pulling.
//!
//! The code belongs to the Workspace, so its guessing limit does too.
//! Each wrong code adds to the failed-attempt count of the Workspace,
//! across all its calls and all its lines. While the count shows a
//! delay, the gate plays no prompt and checks no digit, and the call
//! stays at Unknown. The per-call limit of [`MAX_ATTEMPTS`] keeps one
//! call short.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pagis_core::{
    AgentId, CallId, StoreError, TrustListStore, TrustSubject, TrustTier, WorkspaceId,
};

use crate::bridge::CallLog;

use crate::hub::MediaHub;
use crate::keypad::{self, CHALLENGE_TIMEOUT, Entry, Keypad, MAX_ATTEMPTS, Typed};

/// The tier caller ID proposes for one number on one Agent's line.
/// A number on no list proposes Unknown, and it is never challenged.
pub async fn candidate_tier(
    lists: &dyn TrustListStore,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    e164: &str,
) -> Result<TrustTier, StoreError> {
    Ok(lists
        .candidate(workspace_id, agent_id, TrustSubject::Number, e164)
        .await?
        .unwrap_or(TrustTier::Unknown))
}

/// Why a tier moved. It reaches the audit log and never the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierCause {
    /// The caller entered the Keypad Code.
    Code,
    /// The user dropped the tier from the live Call surface.
    UserRevoked,
}

impl TierCause {
    pub fn as_str(self) -> &'static str {
        match self {
            TierCause::Code => "code",
            TierCause::UserRevoked => "user_revoked",
        }
    }
}

/// One tier change, as the audit log and the bridge read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierChange {
    pub from: TrustTier,
    pub to: TrustTier,
    pub cause: TierCause,
}

struct State {
    tier: TrustTier,
    attempts: usize,
    entry: Entry,
}

/// What one entry of the caller came to.
enum Attempt {
    /// The code was right, and the call rises to this tier.
    Rose(TrustTier),
    /// The code was wrong, and the caller may try again.
    Wrong,
    /// The gate checks no further entry: keypad elevation is suspended
    /// for the Workspace, or this call can prove nothing.
    Closed,
}

/// The tier of one call, and the only three ways it moves.
pub struct TierGate {
    /// What caller ID proposes. It caps the tier for the whole call.
    candidate: TrustTier,
    /// The code a caller proves a tier with. An outbound call has none,
    /// because the dialing established the destination.
    code: Option<WorkspaceCode>,
    state: Mutex<State>,
}

/// The Keypad Code of the Workspace the call reached, and the count of
/// the wrong codes its callers entered. One person's code proves
/// nothing on another person's line.
struct WorkspaceCode {
    workspace_id: WorkspaceId,
    keypad: Keypad,
    /// Where each wrong code of this call is audited.
    log: CallLog,
}

impl WorkspaceCode {
    async fn is_set(&self) -> bool {
        self.keypad.code.is_set(&self.workspace_id).await
    }

    async fn verify(&self, digits: &str) -> bool {
        self.keypad.code.verify(&self.workspace_id, digits).await
    }

    /// True while a delay runs for the Workspace. A count the daemon
    /// cannot read suspends elevation too, because a limit that is not
    /// read limits nothing.
    async fn suspended(&self) -> bool {
        match self.keypad.failures.get(&self.workspace_id).await {
            Ok(failures) => failures.suspended_at(self.keypad.clock.now_ms()),
            Err(error) => {
                tracing::error!(%error, workspace_id = %self.workspace_id, "the keypad failures were not read");
                true
            }
        }
    }

    /// Count one wrong code for the Workspace, and audit it. True when
    /// the gate checks no further entry: the failure started a delay,
    /// or it was not counted.
    async fn failed(&self, candidate: TrustTier) -> bool {
        let now = self.keypad.clock.now_ms();
        let failures = match self
            .keypad
            .failures
            .record_failure(&self.workspace_id, now)
            .await
        {
            Ok(failures) => failures,
            Err(error) => {
                tracing::error!(%error, workspace_id = %self.workspace_id, "a wrong keypad code was not counted");
                return true;
            }
        };
        if let Err(error) = self.log.keypad_failed(candidate, &failures).await {
            tracing::error!(%error, "the wrong keypad code did not reach the audit log");
        }
        failures.suspended_at(now)
    }

    /// A correct code clears the count of the Workspace.
    async fn succeeded(&self) {
        if let Err(error) = self.keypad.failures.clear(&self.workspace_id).await {
            tracing::error!(%error, workspace_id = %self.workspace_id, "the keypad failures were not cleared");
        }
    }
}

impl TierGate {
    /// A gate on a call the Agent answered. The tier starts at Unknown
    /// and rises only when the code arrives.
    pub fn inbound(
        candidate: TrustTier,
        workspace_id: WorkspaceId,
        keypad: Keypad,
        log: CallLog,
    ) -> Self {
        Self {
            candidate,
            code: Some(WorkspaceCode {
                workspace_id,
                keypad,
                log,
            }),
            state: Mutex::new(State {
                tier: TrustTier::Unknown,
                attempts: 0,
                entry: Entry::default(),
            }),
        }
    }

    /// A gate on a call Pagis placed. The dialed number established the
    /// destination, so the listed tier holds and there is no challenge.
    pub fn outbound(tier: TrustTier) -> Self {
        Self {
            candidate: tier,
            code: None,
            state: Mutex::new(State {
                tier,
                attempts: 0,
                entry: Entry::default(),
            }),
        }
    }

    pub fn tier(&self) -> TrustTier {
        self.state.lock().expect("lock").tier
    }

    /// True when the call has no attempts left. It is pinned to Unknown
    /// for its life.
    pub fn pinned(&self) -> bool {
        self.state.lock().expect("lock").attempts >= MAX_ATTEMPTS
    }

    /// Run the challenge, before the realtime session exists. It
    /// answers with the tier the session binds.
    ///
    /// A caller whose number is on no list is never challenged, and a
    /// Workspace with no code never challenges. While a delay runs for
    /// the Workspace, no prompt plays. A caller who enters nothing,
    /// enters a wrong code, or has no keypad reaches Unknown.
    pub async fn challenge(&self, hub: &MediaHub) -> TrustTier {
        let Some(code) = self.challengeable().await else {
            return TrustTier::Unknown;
        };
        if code.suspended().await {
            return TrustTier::Unknown;
        }
        let mut events = hub.subscribe();
        keypad::play_prompt(hub).await;
        let _ = tokio::time::timeout(CHALLENGE_TIMEOUT, async {
            while !self.pinned() {
                match keypad::read_entry(&mut events).await {
                    Typed::Entry(digits) => match self.check(&digits).await {
                        Attempt::Wrong => {}
                        Attempt::Rose(_) | Attempt::Closed => return,
                    },
                    Typed::Silence | Typed::Ended => return,
                }
            }
        })
        .await;
        self.tier()
    }

    /// One keypad press that arrived after the session started. The
    /// bridge calls this for each `HubEvent::Dtmf` and applies
    /// the answer with one `session.update`.
    pub async fn late_digit(&self, digit: char) -> Option<TierChange> {
        let entry = {
            let mut state = self.state.lock().expect("lock");
            if state.tier != TrustTier::Unknown || state.attempts >= MAX_ATTEMPTS {
                return None;
            }
            state.entry.press(digit)
        };
        match self.check(&entry?).await {
            Attempt::Rose(tier) => Some(TierChange {
                from: TrustTier::Unknown,
                to: tier,
                cause: TierCause::Code,
            }),
            Attempt::Wrong | Attempt::Closed => None,
        }
    }

    /// The user drops the tier, from the live Call surface. It is the
    /// only fall there is: a tier never falls on a heuristic. `None`
    /// when the call is at Unknown already.
    pub fn drop_to_unknown(&self) -> Option<TierChange> {
        let mut state = self.state.lock().expect("lock");
        if state.tier == TrustTier::Unknown {
            return None;
        }
        let from = state.tier;
        state.tier = TrustTier::Unknown;
        // The user's drop is final for this call: no further code
        // raises it again.
        state.attempts = MAX_ATTEMPTS;
        Some(TierChange {
            from,
            to: TrustTier::Unknown,
            cause: TierCause::UserRevoked,
        })
    }

    /// The code of a call that can prove a tier: an inbound call from a
    /// listed number, to a Workspace that set a code. `None` for any
    /// other call, which is never challenged and checks no digit.
    async fn challengeable(&self) -> Option<&WorkspaceCode> {
        let code = self.code.as_ref()?;
        match self.candidate != TrustTier::Unknown && code.is_set().await {
            true => Some(code),
            false => None,
        }
    }

    /// One entry. Only an entry that is checked counts: one per call
    /// against [`MAX_ATTEMPTS`], and a wrong one against the count of
    /// the Workspace.
    async fn check(&self, digits: &str) -> Attempt {
        let Some(code) = self.challengeable().await else {
            return Attempt::Closed;
        };
        if code.suspended().await {
            return Attempt::Closed;
        }
        let verified = code.verify(digits).await;
        self.state.lock().expect("lock").attempts += 1;
        if !verified {
            return match code.failed(self.candidate).await {
                true => Attempt::Closed,
                false => Attempt::Wrong,
            };
        }
        // The code proves the caller is on the list, and caller ID caps
        // how far the list reaches: the call rises to the candidate.
        let tier = self.candidate;
        self.state.lock().expect("lock").tier = tier;
        code.succeeded().await;
        Attempt::Rose(tier)
    }
}

/// Settle the tier of a call the Agent answered, before the realtime
/// session exists (ADR-0021): read the candidate from the Trust List,
/// run the keypad challenge on the answered call, and hand back the
/// gate the session is bound under.
///
/// The gate stays alive for the whole call. The bridge keeps it,
/// feeds it a late digit through [`TierGate::late_digit`] and applies
/// the answer with one `session.update`, and the daemon's drop
/// endpoint calls [`TierGate::drop_to_unknown`].
pub async fn settle_inbound(
    lists: &dyn TrustListStore,
    keypad: &Keypad,
    hub: &MediaHub,
    log: &CallLog,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    from_e164: &str,
) -> Result<Arc<TierGate>, StoreError> {
    let candidate = candidate_tier(lists, workspace_id, agent_id, from_e164).await?;
    let gate = Arc::new(TierGate::inbound(
        candidate,
        workspace_id.clone(),
        keypad.clone(),
        log.clone(),
    ));
    gate.challenge(hub).await;
    Ok(gate)
}

/// The tier gates of the calls that run right now. The daemon's drop
/// endpoint reaches a live call through this registry, because the drop
/// is a user act on the live Call surface and not a tool.
///
/// The key holds the Workspace and the Call, so a lookup finds a gate
/// only under the Workspace of its call (ADR-0023). A Person of another
/// Workspace who knows the Call id finds no gate.
#[derive(Default)]
pub struct LiveTiers {
    gates: Mutex<HashMap<(WorkspaceId, CallId), LiveTier>>,
}

/// The gate of one live call, and where its tier changes are audited.
struct LiveTier {
    gate: Arc<TierGate>,
    log: Arc<CallLog>,
}

impl LiveTiers {
    /// Hold the gate of one call. The bridge binds it when the
    /// call starts and releases it when the call ends.
    pub fn bind(
        &self,
        workspace_id: &WorkspaceId,
        call_id: &CallId,
        gate: Arc<TierGate>,
        log: Arc<CallLog>,
    ) {
        self.gates.lock().expect("lock").insert(
            (workspace_id.clone(), call_id.clone()),
            LiveTier { gate, log },
        );
    }

    pub fn release(&self, workspace_id: &WorkspaceId, call_id: &CallId) {
        self.gates
            .lock()
            .expect("lock")
            .remove(&(workspace_id.clone(), call_id.clone()));
    }

    pub fn gate(&self, workspace_id: &WorkspaceId, call_id: &CallId) -> Option<Arc<TierGate>> {
        self.gates
            .lock()
            .expect("lock")
            .get(&(workspace_id.clone(), call_id.clone()))
            .map(|live| Arc::clone(&live.gate))
    }

    /// The user drops one live call of the Workspace to Unknown, and the
    /// audit log keeps the change. `None` when no such call is live in
    /// that Workspace; `Some(None)` when the call is at Unknown already.
    pub async fn drop_to_unknown(
        &self,
        workspace_id: &WorkspaceId,
        call_id: &CallId,
    ) -> Option<Result<Option<TierChange>, StoreError>> {
        let (gate, log) = {
            let gates = self.gates.lock().expect("lock");
            let live = gates.get(&(workspace_id.clone(), call_id.clone()))?;
            (Arc::clone(&live.gate), Arc::clone(&live.log))
        };
        let Some(change) = gate.drop_to_unknown() else {
            return Some(Ok(None));
        };
        Some(log.tier_changed(change).await.map(|()| Some(change)))
    }
}
