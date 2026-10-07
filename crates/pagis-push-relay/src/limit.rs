//! The limit on new registrations from one client address.
//!
//! The limit counts in process, in a fixed window for each address, as
//! the daemon's sign-in limit does. A restart of the relay forgets the
//! counts.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many registrations one address may make in one window.
const MAX_REGISTRATIONS: u32 = 20;
/// The window that the registrations are counted in.
const WINDOW: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Default)]
pub(crate) struct RegistrationLimit {
    windows: Mutex<HashMap<IpAddr, Window>>,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    count: u32,
}

impl RegistrationLimit {
    /// Count one registration from `address` at `now`. Above the limit,
    /// answer how long until the window of the address ends.
    pub(crate) fn admit(&self, address: IpAddr, now: Instant) -> Result<(), Duration> {
        let mut windows = self.windows.lock().expect("registration limit lock");
        windows.retain(|_, window| now.duration_since(window.started) < WINDOW);
        let window = windows.entry(address).or_insert(Window {
            started: now,
            count: 0,
        });
        if window.count >= MAX_REGISTRATIONS {
            return Err(WINDOW - now.duration_since(window.started));
        }
        window.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 1));
    const TWO: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 2));

    #[test]
    fn the_window_refuses_above_the_limit_and_opens_again_when_it_ends() {
        let limit = RegistrationLimit::default();
        let start = Instant::now();
        for _ in 0..MAX_REGISTRATIONS {
            assert_eq!(limit.admit(ONE, start), Ok(()));
        }

        let later = start + Duration::from_secs(600);
        assert_eq!(
            limit.admit(ONE, later),
            Err(WINDOW - Duration::from_secs(600))
        );
        assert_eq!(limit.admit(TWO, later), Ok(()));
        assert_eq!(limit.admit(ONE, start + WINDOW), Ok(()));
    }
}
