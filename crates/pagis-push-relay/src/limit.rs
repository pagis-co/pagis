//! The limit on new registrations from one client address.
//!
//! The limit counts in process, in a fixed window for each address, as
//! the daemon's sign-in limit does. A restart of the relay forgets the
//! counts.
//!
//! An IPv6 client counts by its /64 network, not by its full address. One
//! site usually gets a /64, and a client there can use any address in it.
//! An IPv4 client, and an IPv4-mapped IPv6 address, counts by its full
//! IPv4 address.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How many registrations one address may make in one window.
const MAX_REGISTRATIONS: u32 = 20;
/// The window that the registrations are counted in.
const WINDOW: Duration = Duration::from_secs(60 * 60);
/// The bits of an IPv6 address that name its /64 network.
const IPV6_NETWORK: u128 = !0 << 64;

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
        let window = windows.entry(key(address)).or_insert(Window {
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

/// The address that the window of `address` is kept under: the /64
/// network of an IPv6 address, and the IPv4 address otherwise.
fn key(address: IpAddr) -> IpAddr {
    match address.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & IPV6_NETWORK)),
        v4 => v4,
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

    fn v6(text: &str) -> IpAddr {
        text.parse().expect("an IPv6 address")
    }

    /// Fill the window of `address`, so that the next registration from
    /// the same budget is refused.
    fn fill(limit: &RegistrationLimit, address: IpAddr, now: Instant) {
        for _ in 0..MAX_REGISTRATIONS {
            assert_eq!(limit.admit(address, now), Ok(()));
        }
    }

    #[test]
    fn two_addresses_in_one_ipv6_64_share_one_window() {
        let limit = RegistrationLimit::default();
        let now = Instant::now();
        fill(&limit, v6("2001:db8:1:2::1"), now);

        assert_eq!(
            limit.admit(v6("2001:db8:1:2:ffff:ffff:ffff:ffff"), now),
            Err(WINDOW)
        );
    }

    #[test]
    fn addresses_in_different_ipv6_64s_have_their_own_windows() {
        let limit = RegistrationLimit::default();
        let now = Instant::now();
        fill(&limit, v6("2001:db8:1:2::1"), now);

        assert_eq!(limit.admit(v6("2001:db8:1:3::1"), now), Ok(()));
    }

    #[test]
    fn an_ipv4_mapped_ipv6_address_counts_as_its_ipv4_address() {
        let limit = RegistrationLimit::default();
        let now = Instant::now();
        fill(&limit, ONE, now);

        assert_eq!(limit.admit(v6("::ffff:203.0.113.1"), now), Err(WINDOW));
        assert_eq!(limit.admit(v6("::ffff:203.0.113.2"), now), Ok(()));
    }
}
