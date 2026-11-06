//! Address classes for outbound connections the daemon makes on someone else's behalf (Web
//! Push endpoints, MCP servers, card images, web fetches): one table, so every caller refuses
//! the same internal and special-purpose ranges.

use std::net::{IpAddr, Ipv6Addr};

/// Globally routable unicast. False for this host, its LAN and every other IANA
/// special-purpose range that is not globally reachable, for transition prefixes whose far
/// side would reach an embedded IPv4 address on our behalf (6to4, Teredo, NAT64), and for
/// cloud metadata endpoints outside link-local. IPv4-mapped addresses are judged as IPv4.
pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..128).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..32).contains(&b))
                // 192.0.0.0/24 IETF assignments, 192.0.2.0/24 documentation, 192.88.99.0/24
                // retired 6to4 relay anycast.
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 192 && b == 88 && c == 99)
                || (a == 192 && b == 168)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                // Multicast, 240.0.0.0/4 reserved and the limited broadcast address.
                || a >= 224
                // Azure's platform endpoint answers metadata and agent requests.
                || ip.octets() == [168, 63, 129, 16])
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only 2000::/3 is global unicast; ::/8 (loopback, unspecified, IPv4-compatible),
            // 64:ff9b::/96 NAT64, 100::/64 discard, fc00::/7, fe80::/10, fec0::/10 and ff00::/8
            // all fall outside it.
            (s[0] & 0xe000) == 0x2000
                // 2001::/23 IETF protocol assignments (Teredo, benchmarking, ORCHID, ...).
                && !(s[0] == 0x2001 && s[1] < 0x0200)
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
                // 6to4 embeds an IPv4 address.
                && s[0] != 0x2002
                // 3ffe::/16 returned 6bone test space, 3fff::/20 documentation.
                && s[0] != 0x3ffe
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

/// Private (RFC 1918), shared (CGNAT, which Tailscale uses) and unique-local addresses, minus
/// the metadata services clouds place inside them. Link-local stays out: it is where most
/// cloud metadata services answer. Expects a canonical address.
pub(crate) fn private_network_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            (ip.is_private() || (a == 100 && (64..128).contains(&b)))
                // Alibaba Cloud metadata.
                && ip.octets() != [100, 100, 100, 200]
        }
        IpAddr::V6(ip) => {
            (ip.segments()[0] & 0xfe00) == 0xfc00
                // AWS instance metadata over IPv6.
                && ip != Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{private_network_ip, public_ip};

    #[test]
    fn only_globally_routable_addresses_are_public() {
        for ip in [
            "0.0.0.0",
            "0.1.2.3",
            "10.1.2.3",
            "100.64.0.1",
            "100.100.100.200",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.0.3",
            "192.0.0.8",
            "192.0.0.9",
            "192.0.0.10",
            "192.0.0.170",
            "192.0.0.255",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "224.1.1.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "168.63.129.16",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:2::1",
            "2001:10::1",
            "2001:1ff::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "2002:7f00:1::",
            "3ffe::1",
            "3ffe:ffff::1",
            "3fff::1",
            "3fff:fff::1",
            "5f00::1",
            "fc00::1",
            "fd00:ec2::254",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.0.3.1",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.254",
            "::ffff:8.8.8.8",
            "2001:200::1",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "2a00:1450::1",
            "3fff:1000::1",
        ] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn private_networks_exclude_link_local_and_metadata() {
        for ip in [
            "10.0.0.5",
            "172.20.1.1",
            "192.168.1.2",
            "100.101.102.103",
            "fd7a:115c:a1e0::1",
        ] {
            assert!(private_network_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "100.100.100.200",
            "fd00:ec2::254",
            "169.254.169.254",
            "fe80::1",
            "fec0::1",
            "8.8.8.8",
            "127.0.0.1",
        ] {
            assert!(!private_network_ip(ip.parse().unwrap()), "{ip}");
        }
    }
}
