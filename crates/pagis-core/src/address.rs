//! Which network addresses the daemon reaches for a party outside the
//! installation (ADR-0029, ADR-0030).

use std::net::IpAddr;

/// Whether `ip` is a public unicast address: one that a connection of a
/// Computer may reach from the server, and one that a Web Push may go to.
/// An IPv4-mapped IPv6 address is checked as its IPv4 address. IPv6 passes only in the global unicast
/// block 2000::/3, without the documentation blocks, the IETF protocol
/// assignments (2001::/23, Teredo among them) and 6to4 (2002::/16), which
/// carries an IPv4 address that can be a private one.
pub fn is_public_unicast(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [first, second, third, _] = ip.octets();
            let refused = first == 0
                || first == 10
                || first == 127
                || (first == 100 && (64..128).contains(&second))
                || (first == 169 && second == 254)
                || (first == 172 && (16..32).contains(&second))
                || (first == 192 && second == 168)
                || (first == 192 && second == 0 && third == 0)
                || (first == 192 && second == 0 && third == 2)
                || (first == 198 && (18..20).contains(&second))
                || (first == 198 && second == 51 && third == 100)
                || (first == 203 && second == 0 && third == 113)
                // Multicast, the reserved block and the broadcast address.
                || first >= 224;
            !refused
        }
        IpAddr::V6(ip) => {
            let [first, second, ..] = ip.segments();
            (first & 0xe000) == 0x2000
                && !(first == 0x2001 && second < 0x0200)
                && !(first == 0x2001 && second == 0x0db8)
                && first != 0x2002
                && !(first == 0x3fff && second < 0x1000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The public unicast addresses, which the daemon reaches, and the
    /// addresses that it refuses.
    #[test]
    fn the_public_unicast_addresses() {
        for public in [
            "93.184.215.14",
            "1.1.1.1",
            "172.32.0.1",
            "100.128.0.1",
            "198.20.0.1",
            "223.255.255.254",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "2a00:1450:4001:82a::200e",
            "::ffff:93.184.215.14",
        ] {
            let ip: IpAddr = public.parse().expect("an address");
            assert!(is_public_unicast(ip), "{public} is refused");
        }
        for refused in [
            "0.1.2.3",
            "10.255.255.254",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.0.1",
            "172.31.255.254",
            "192.0.0.8",
            "192.0.2.10",
            "192.168.0.1",
            "198.18.0.1",
            "198.19.255.254",
            "198.51.100.10",
            "203.0.113.10",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:192.168.0.1",
            "64:ff9b::a00:1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::fb",
            "2001::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "3fff::1",
        ] {
            let ip: IpAddr = refused.parse().expect("an address");
            assert!(!is_public_unicast(ip), "{refused} passes");
        }
    }
}
