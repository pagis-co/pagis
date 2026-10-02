//! A Tailscale that answers from memory, so the Remote Access switch runs
//! in a test and no test changes a tailnet (ADR-0028).

use std::sync::Mutex;

use pagis_server::{Port443, Tailscale, TailscaleState};
use tokio::sync::Notify;

/// The name of the machine on the fake tailnet.
pub const TAILNET_NAME: &str = "owner-mac.tail1234.ts.net";

/// The page that the fake names to turn on HTTPS and Funnel.
pub const ENABLE_URL: &str = "https://login.tailscale.com/f/funnel?node=nTEST000000CNTRL";

/// A Tailscale whose state a test sets. A turn-on serves the product port
/// at [`TAILNET_NAME`], and a turn-off removes it, as the real command
/// does. It records each change it is asked for.
pub struct FakeTailscale {
    state: Mutex<TailscaleState>,
    /// Whether a turn-on names [`ENABLE_URL`] and waits for
    /// [`FakeTailscale::approve`].
    asks_approval: bool,
    approved: Notify,
    /// Why a turn-on fails, where it fails.
    refusal: Option<String>,
    changes: Mutex<Vec<String>>,
}

impl FakeTailscale {
    fn in_state(state: TailscaleState) -> Self {
        Self {
            state: Mutex::new(state),
            asks_approval: false,
            approved: Notify::new(),
            refusal: None,
            changes: Mutex::default(),
        }
    }

    /// No `tailscale` command on the machine.
    pub fn not_installed() -> Self {
        Self::in_state(TailscaleState::NotInstalled {
            install_url: "https://tailscale.com/download".to_string(),
        })
    }

    /// Tailscale is installed, and it is signed out.
    pub fn not_running() -> Self {
        Self::in_state(TailscaleState::NotRunning { detail: None })
    }

    /// The tailnet has HTTPS and Funnel off. A turn-on names the page that
    /// turns them on and waits until the test approves.
    pub fn funnel_off() -> Self {
        Self {
            asks_approval: true,
            ..Self::in_state(TailscaleState::FunnelOff {
                port_443: Port443::Nothing,
            })
        }
    }

    /// Tailscale runs, the tailnet allows Funnel, and port 443 serves
    /// `port_443`.
    pub fn ready(port_443: Port443) -> Self {
        Self::in_state(TailscaleState::Ready {
            dns_name: TAILNET_NAME.to_string(),
            port_443,
        })
    }

    /// A turn-on fails with `reason`, as `tailscale funnel` does when it
    /// ends with a status that is not zero.
    pub fn refusing(self, reason: &str) -> Self {
        Self {
            refusal: Some(reason.to_string()),
            ..self
        }
    }

    /// The owner approves HTTPS and Funnel on the page.
    pub fn approve(&self) {
        self.approved.notify_one();
    }

    /// The changes that the switch asked for, in order: `funnel on <port>`
    /// and `funnel off <port>`.
    pub fn changes(&self) -> Vec<String> {
        self.changes.lock().expect("the changes").clone()
    }

    fn set_port_443(&self, served: Port443) {
        let mut state = self.state.lock().expect("the state");
        *state = match &*state {
            TailscaleState::FunnelOff { .. } | TailscaleState::Ready { .. } => {
                TailscaleState::Ready {
                    dns_name: TAILNET_NAME.to_string(),
                    port_443: served,
                }
            }
            other => other.clone(),
        };
    }
}

#[async_trait::async_trait]
impl Tailscale for FakeTailscale {
    async fn state(&self, _port: u16) -> TailscaleState {
        self.state.lock().expect("the state").clone()
    }

    async fn funnel_on(
        &self,
        port: u16,
        enable_url: &(dyn Fn(String) + Send + Sync),
    ) -> Result<(), String> {
        self.changes
            .lock()
            .expect("the changes")
            .push(format!("funnel on {port}"));
        if self.asks_approval {
            enable_url(ENABLE_URL.to_string());
            self.approved.notified().await;
        }
        if let Some(reason) = &self.refusal {
            return Err(reason.clone());
        }
        self.set_port_443(Port443::Pagis);
        Ok(())
    }

    async fn funnel_off(&self, port: u16) -> Result<(), String> {
        self.changes
            .lock()
            .expect("the changes")
            .push(format!("funnel off {port}"));
        let serves_pagis =
            self.state.lock().expect("the state").port_443() == Some(&Port443::Pagis);
        if serves_pagis {
            self.set_port_443(Port443::Nothing);
        }
        Ok(())
    }
}
