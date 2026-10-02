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
    /// Whether the next read of the state stops until
    /// [`FakeTailscale::release_held_read`].
    hold_next_read: Mutex<bool>,
    read_held: Notify,
    read_released: Notify,
}

impl FakeTailscale {
    fn in_state(state: TailscaleState) -> Self {
        Self {
            state: Mutex::new(state),
            asks_approval: false,
            approved: Notify::new(),
            refusal: None,
            changes: Mutex::default(),
            hold_next_read: Mutex::new(false),
            read_held: Notify::new(),
            read_released: Notify::new(),
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

    /// Stop the next read of the state until the test releases it, as a
    /// slow `tailscale status` does.
    pub fn hold_next_state_read(&self) {
        *self.hold_next_read.lock().expect("the hold") = true;
    }

    /// Wait until a read of the state stops at the hold.
    pub async fn read_is_held(&self) {
        self.read_held.notified().await;
    }

    /// Let the read that stops at the hold go on.
    pub fn release_held_read(&self) {
        self.read_released.notify_one();
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
        let held = std::mem::take(&mut *self.hold_next_read.lock().expect("the hold"));
        if held {
            self.read_held.notify_one();
            self.read_released.notified().await;
        }
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
