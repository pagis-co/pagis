//! Remote Access (ADR-0028): how an installation at home serves the
//! owner's other machines and the other People of the installation, at a
//! public `https://` name of the owner's Tailscale Funnel.
//!
//! On a Local Installation the daemon drives the `tailscale` command of
//! its machine through the [`Tailscale`] seam. One switch in the
//! Administration Interface turns Remote Access on: the daemon turns on
//! Funnel on port 443 to the product port on loopback, writes the Public
//! Origin `https://<machine>.<tailnet>.ts.net` and the Trusted Proxy
//! `127.0.0.1`, keeps the Bind Address on loopback, and the page asks for
//! the reserved restart. Turning it off removes the Funnel and clears the
//! three settings.
//!
//! `tailscale funnel` can wait for the owner: where the tailnet has HTTPS
//! or Funnel off, it names a page that turns them on and waits for the
//! approval. So a turn-on runs in the background, and the switch reads the
//! page and the result here until it ends.
//!
//! A Local Installation serves other machines only while Remote Access is
//! on ([`serves_this_machine_only`]). With Remote Access on, another
//! machine signs in with a Sign-In Link and never with a password.
//!
//! A Server is not switched here: its deployment sets
//! `PAGIS_REMOTE_ACCESS`, and the routes answer `409`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Administrator;
use crate::error::ApiError;
use crate::system::{SystemConfigFile, is_local};

/// The address the Funnel of this machine reaches the daemon from, which
/// Remote Access writes as the Trusted Proxy.
pub const FUNNEL_PROXY: &str = "127.0.0.1";

/// Why a turn-on fails where `tailscale funnel` ends with no error and
/// Funnel still does not serve Pagis: Tailscale does that where the
/// tailnet does not let this owner turn Funnel on from its page.
const FUNNEL_DOES_NOT_SERVE: &str = "`tailscale funnel` ended, but Funnel does not serve Pagis \
     on port 443; an Admin of the tailnet may need to turn on HTTPS and Funnel in the \
     Tailscale admin console";

/// What port 443 of the Funnel of this machine serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "serves", rename_all = "snake_case")]
pub enum Port443 {
    /// Funnel publishes nothing on port 443.
    Nothing,
    /// Funnel publishes the product port of this Pagis on port 443.
    Pagis,
    /// Port 443 serves something else, which Pagis does not replace.
    Other {
        /// What port 443 forwards to, as Tailscale names it.
        target: String,
    },
}

/// Tailscale on the machine of the installation, as the Remote Access
/// switch reads it. Each state has one thing for the owner to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TailscaleState {
    /// No `tailscale` command on this machine.
    NotInstalled {
        /// Where the owner gets Tailscale for this system.
        install_url: String,
    },
    /// Tailscale is installed, but it does not run or it is signed out.
    NotRunning {
        /// What the `tailscale` command said, where it said why.
        detail: Option<String>,
    },
    /// The tailnet has HTTPS or Funnel off. A turn-on shows the page that
    /// Tailscale names to turn them on.
    FunnelOff { port_443: Port443 },
    /// Tailscale runs and the tailnet allows Funnel.
    Ready {
        /// The name of this machine on the tailnet, with no trailing dot.
        dns_name: String,
        port_443: Port443,
    },
}

impl TailscaleState {
    /// What port 443 serves, where Tailscale runs.
    pub fn port_443(&self) -> Option<&Port443> {
        match self {
            TailscaleState::FunnelOff { port_443 } | TailscaleState::Ready { port_443, .. } => {
                Some(port_443)
            }
            TailscaleState::NotInstalled { .. } | TailscaleState::NotRunning { .. } => None,
        }
    }
}

/// The `tailscale` command of this machine. The daemon runs it as the
/// owner, as it runs `git`; a test injects a fake, so no test changes a
/// tailnet.
#[async_trait::async_trait]
pub trait Tailscale: Send + Sync {
    /// Read the state of Tailscale, and what port 443 of its Funnel
    /// serves, against the product port `port` on loopback.
    async fn state(&self, port: u16) -> TailscaleState;

    /// Turn on Funnel on port 443 to the product port `port` on loopback.
    /// Where the tailnet has HTTPS or Funnel off, Tailscale names a page
    /// that turns them on: the call gives each such page to `enable_url`
    /// and waits until the owner approves.
    async fn funnel_on(
        &self,
        port: u16,
        enable_url: &(dyn Fn(String) + Send + Sync),
    ) -> Result<(), String>;

    /// Remove the Funnel on port 443 where it serves the product port
    /// `port`. Anything else that port 443 serves stays.
    async fn funnel_off(&self, port: u16) -> Result<(), String>;
}

/// The turn-on that runs now, or how the last one ended.
#[derive(Debug, Default)]
enum TurnOn {
    #[default]
    Idle,
    Waiting {
        /// The last page that Tailscale named to turn on HTTPS or Funnel
        /// for the tailnet.
        enable_url: Option<String>,
        task: tokio::task::AbortHandle,
    },
    Failed(String),
}

#[derive(Debug, Default)]
struct Slot {
    /// Each turn-on and each turn-off takes the next number, so a
    /// turn-on that a turn-off overtook writes nothing.
    generation: u64,
    turn_on: TurnOn,
}

/// The Remote Access switch of a Local Installation: the Tailscale of
/// this machine, and the turn-on that waits for it.
pub struct RemoteAccessSwitch {
    tailscale: Arc<dyn Tailscale>,
    slot: Arc<Mutex<Slot>>,
}

impl RemoteAccessSwitch {
    pub fn new(tailscale: Arc<dyn Tailscale>) -> Self {
        Self {
            tailscale,
            slot: Arc::default(),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start a turn-on in the background, unless one waits already.
    fn start(&self, system: Arc<dyn SystemConfigFile>, port: u16) {
        let mut slot = self.slot();
        if matches!(slot.turn_on, TurnOn::Waiting { .. }) {
            return;
        }
        slot.generation += 1;
        let generation = slot.generation;
        let task = tokio::spawn(turn_on(
            Arc::clone(&self.tailscale),
            system,
            port,
            Arc::clone(&self.slot),
            generation,
        ));
        slot.turn_on = TurnOn::Waiting {
            enable_url: None,
            task: task.abort_handle(),
        };
    }

    /// Stop a turn-on that waits. Its command stops, and it writes
    /// nothing.
    fn stop(&self) {
        let mut slot = self.slot();
        slot.generation += 1;
        if let TurnOn::Waiting { task, .. } = &slot.turn_on {
            task.abort();
        }
        slot.turn_on = TurnOn::Idle;
    }

    /// The turn-on that waits, and why the last one failed.
    fn progress(&self) -> (Option<TurningOnDto>, Option<String>) {
        match &self.slot().turn_on {
            TurnOn::Idle => (None, None),
            TurnOn::Waiting { enable_url, .. } => (
                Some(TurningOnDto {
                    enable_url: enable_url.clone(),
                }),
                None,
            ),
            TurnOn::Failed(reason) => (None, Some(reason.clone())),
        }
    }
}

/// One turn-on: Funnel first, then the settings. It writes the settings
/// under the lock of the slot, so a turn-off that comes after it clears
/// them, and a turn-off that comes before it stops it from writing.
async fn turn_on(
    tailscale: Arc<dyn Tailscale>,
    system: Arc<dyn SystemConfigFile>,
    port: u16,
    slot: Arc<Mutex<Slot>>,
    generation: u64,
) {
    let lock = || slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let show = |url: String| {
        let mut slot = lock();
        if slot.generation != generation {
            return;
        }
        if let TurnOn::Waiting { enable_url, .. } = &mut slot.turn_on {
            tracing::info!(%url, "Tailscale asks the owner to turn on HTTPS and Funnel");
            *enable_url = Some(url);
        }
    };
    let funnel = match tailscale.funnel_on(port, &show).await {
        Ok(()) => match tailscale.state(port).await {
            TailscaleState::Ready {
                dns_name,
                port_443: Port443::Pagis,
            } => public_origin_of(&dns_name),
            _ => Err(FUNNEL_DOES_NOT_SERVE.to_string()),
        },
        Err(reason) => Err(reason),
    };
    let mut slot = lock();
    if slot.generation != generation {
        return;
    }
    let written = funnel.and_then(|origin| {
        system.set_remote_access(Some(&origin)).map_err(|error| {
            tracing::error!(%error, "cannot write the config file");
            "Pagis could not write config.toml".to_string()
        })?;
        Ok(origin)
    });
    slot.turn_on = match written {
        Ok(origin) => {
            tracing::info!(public_origin = %origin, "the administrator turned on Remote Access");
            TurnOn::Idle
        }
        Err(reason) => {
            tracing::warn!(%reason, "Remote Access did not turn on");
            TurnOn::Failed(reason)
        }
    };
}

/// The Public Origin of a machine on the tailnet: `https://` and its
/// name. A name that is not a host gives no origin.
fn public_origin_of(dns_name: &str) -> Result<String, String> {
    url::Url::parse(&format!("https://{dns_name}"))
        .ok()
        .filter(|url| matches!(url.host(), Some(url::Host::Domain(_))) && url.path() == "/")
        .map(|url| url.origin().ascii_serialization())
        .ok_or_else(|| format!("Tailscale names this machine {dns_name:?}, which is not a host"))
}

/// Whether the running daemon serves this machine alone: a Local
/// Installation (ADR-0025) with Remote Access off. It answers only
/// programs of its own machine, and it has one Person who signs in.
///
/// A switch of Remote Access writes `config.toml` and takes effect at the
/// next start, so the answer holds for the life of the process. The guard
/// that refuses other machines and the start route of a Google
/// authorization both read it here. A Server serves its network whatever
/// Remote Access says.
pub fn serves_this_machine_only(state: &AppState) -> bool {
    is_local(state) && !state.remote_access
}

/// How a client at the address that asks signs in (ADR-0028). The
/// sign-in page of the Product App shows the form that it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    /// An address and a password.
    Password,
    /// A Sign-In Link. In Remote Access the daemon takes no password from
    /// another machine, because the public name faces the internet.
    Link,
}

/// How the client of one request signs in: with a Sign-In Link where
/// Remote Access is on and the request did not come from this machine,
/// and with a password everywhere else.
pub(crate) fn sign_in_method(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> SignInMethod {
    if state.remote_access && !crate::forwarded::is_from_this_machine(peer, headers) {
        SignInMethod::Link
    } else {
        SignInMethod::Password
    }
}

/// A turn-on that waits for Tailscale.
#[derive(Debug, Serialize, ToSchema)]
pub struct TurningOnDto {
    /// The page that Tailscale names to turn on HTTPS and Funnel for the
    /// tailnet. The turn-on waits until the owner approves there. Null
    /// while Tailscale names no page.
    pub enable_url: Option<String>,
}

/// Remote Access as the Settings view shows it (ADR-0028).
#[derive(Debug, Serialize, ToSchema)]
pub struct RemoteAccessDto {
    /// Whether Remote Access is on. A Local Installation shows what
    /// `config.toml` says, which a restart puts in effect. A Server shows
    /// what it runs with.
    pub enabled: bool,
    /// Whether the running daemon differs from `enabled` until a restart.
    pub restart_required: bool,
    /// The public `https://` name that other machines open, while Remote
    /// Access is on.
    pub public_origin: Option<String>,
    /// Whether an Administrator switches Remote Access here. Only a Local
    /// Installation does: a Server's deployment sets
    /// `PAGIS_REMOTE_ACCESS`.
    pub switchable: bool,
    /// Tailscale on this machine. Null on a Server, whose deployment runs
    /// Tailscale beside the daemon.
    pub tailscale: Option<TailscaleState>,
    /// The turn-on that waits for Tailscale, or null.
    pub turning_on: Option<TurningOnDto>,
    /// Why the last turn-on failed, or null.
    pub failure: Option<String>,
}

/// The Public Origin that the running daemon serves in Remote Access.
fn running_public_origin(state: &AppState) -> Option<&str> {
    state.remote_access.then_some(state.public_origin.as_str())
}

async fn remote_access_dto(state: &AppState) -> Result<RemoteAccessDto, ApiError> {
    if !is_local(state) {
        return Ok(RemoteAccessDto {
            enabled: state.remote_access,
            restart_required: false,
            public_origin: running_public_origin(state).map(str::to_string),
            switchable: false,
            tailscale: None,
            turning_on: None,
            failure: None,
        });
    }
    let public_origin = state.system.remote_access().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    let tailscale = state
        .remote_access_switch
        .tailscale
        .state(state.runtime_port)
        .await;
    let (turning_on, failure) = state.remote_access_switch.progress();
    Ok(RemoteAccessDto {
        enabled: public_origin.is_some(),
        restart_required: public_origin.as_deref() != running_public_origin(state),
        public_origin,
        switchable: true,
        tailscale: Some(tailscale),
        turning_on,
        failure,
    })
}

/// Refuse a switch on a Server. Its deployment turns Remote Access on.
fn require_local(state: &AppState) -> Result<(), ApiError> {
    if is_local(state) {
        return Ok(());
    }
    Err(ApiError::conflict(
        "this Pagis is a server; its deployment turns Remote Access on with \
         PAGIS_REMOTE_ACCESS and names the Public Origin in PAGIS_PUBLIC_ORIGIN",
    ))
}

/// Why Tailscale cannot turn Remote Access on now, in words that say
/// what the owner does.
fn not_ready(state: &TailscaleState) -> Option<String> {
    match state {
        TailscaleState::NotInstalled { install_url } => Some(format!(
            "Tailscale is not installed on this computer; install it from {install_url}, sign \
             in, and turn on Remote Access again"
        )),
        TailscaleState::NotRunning { .. } => Some(
            "Tailscale does not run on this computer, or it is signed out; open Tailscale, sign \
             in, and turn on Remote Access again"
                .to_string(),
        ),
        TailscaleState::FunnelOff { .. } | TailscaleState::Ready { .. } => match state.port_443() {
            Some(Port443::Other { target }) => Some(format!(
                "port 443 of Tailscale Funnel on this computer serves {target}, and Pagis \
                     does not replace it; remove it with `tailscale funnel --https=443 off`, \
                     then turn on Remote Access again"
            )),
            _ => None,
        },
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/system/remote-access",
    responses(
        (status = 200, body = RemoteAccessDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
/// Remote Access and the Tailscale of this machine (ADR-0028).
pub async fn get_remote_access(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<RemoteAccessDto>, ApiError> {
    Ok(Json(remote_access_dto(&state).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/system/remote-access",
    responses(
        (status = 202, body = RemoteAccessDto, description = "The turn-on runs; `turning_on` shows it until it ends"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "A Server, or Tailscale that cannot turn Remote Access on now"),
    )
)]
/// Turn on Remote Access (ADR-0028). The daemon turns on Funnel on port
/// 443 to the product port on loopback, in the background, because
/// Tailscale can wait for the owner to turn on HTTPS and Funnel for the
/// tailnet. Then it writes the Public Origin and the Trusted Proxy and
/// keeps the Bind Address on loopback. The change takes effect on the
/// next start.
pub async fn turn_on_remote_access(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<(StatusCode, Json<RemoteAccessDto>), ApiError> {
    require_local(&state)?;
    let tailscale = state
        .remote_access_switch
        .tailscale
        .state(state.runtime_port)
        .await;
    if let Some(reason) = not_ready(&tailscale) {
        return Err(ApiError::conflict(reason));
    }
    state
        .remote_access_switch
        .start(Arc::clone(&state.system), state.runtime_port);
    Ok((StatusCode::ACCEPTED, Json(remote_access_dto(&state).await?)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/settings/system/remote-access",
    responses(
        (status = 200, body = RemoteAccessDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "A Server, whose deployment turns Remote Access on"),
    )
)]
/// Turn off Remote Access (ADR-0028), or stop a turn-on that waits. The
/// daemon removes the Funnel of the product port and clears the Public
/// Origin, the Trusted Proxy and Remote Access, and binds loopback. People
/// who signed in from other machines keep their accounts and their
/// Sessions, and reach nothing until Remote Access is on again. The change
/// takes effect on the next start.
pub async fn turn_off_remote_access(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<RemoteAccessDto>, ApiError> {
    require_local(&state)?;
    state.remote_access_switch.stop();
    // A Funnel that stays serves a daemon that refuses other machines once
    // it runs with Remote Access off, so the settings clear all the same,
    // and the switch then says that the Funnel still serves Pagis.
    if let Err(reason) = state
        .remote_access_switch
        .tailscale
        .funnel_off(state.runtime_port)
        .await
    {
        tracing::warn!(%reason, "Tailscale did not remove the Funnel of Pagis");
    }
    state.system.set_remote_access(None).map_err(|error| {
        tracing::error!(%error, "cannot write the config file");
        ApiError::internal()
    })?;
    tracing::info!("the administrator turned off Remote Access");
    Ok(Json(remote_access_dto(&state).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_origin_is_https_and_the_name_of_the_machine() {
        assert_eq!(
            public_origin_of("owner-mac.tail1234.ts.net").unwrap(),
            "https://owner-mac.tail1234.ts.net"
        );
        assert_eq!(
            public_origin_of("Owner-Mac.Tail1234.ts.net").unwrap(),
            "https://owner-mac.tail1234.ts.net"
        );
        for name in ["", "owner mac", "127.0.0.1", "owner-mac.ts.net/path"] {
            assert!(public_origin_of(name).is_err(), "{name:?}");
        }
    }

    /// Each state that cannot turn Remote Access on says what to do, and
    /// so does a port 443 that serves something else.
    #[test]
    fn a_state_that_cannot_turn_on_says_what_to_do() {
        let other = Port443::Other {
            target: "http://127.0.0.1:3000".to_string(),
        };
        for (state, says) in [
            (
                TailscaleState::NotInstalled {
                    install_url: "https://tailscale.com/download".to_string(),
                },
                "https://tailscale.com/download",
            ),
            (
                TailscaleState::NotRunning { detail: None },
                "open Tailscale, sign in",
            ),
            (
                TailscaleState::FunnelOff {
                    port_443: other.clone(),
                },
                "http://127.0.0.1:3000",
            ),
            (
                TailscaleState::Ready {
                    dns_name: "owner-mac.tail1234.ts.net".to_string(),
                    port_443: other,
                },
                "tailscale funnel --https=443 off",
            ),
        ] {
            let reason = not_ready(&state).expect("a reason");
            assert!(reason.contains(says), "{reason}");
        }
        for port_443 in [Port443::Nothing, Port443::Pagis] {
            assert_eq!(
                not_ready(&TailscaleState::FunnelOff {
                    port_443: port_443.clone()
                }),
                None
            );
            assert_eq!(
                not_ready(&TailscaleState::Ready {
                    dns_name: "owner-mac.tail1234.ts.net".to_string(),
                    port_443,
                }),
                None
            );
        }
    }
}
