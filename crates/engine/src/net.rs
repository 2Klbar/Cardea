//! What the firewall references are on this host now: gateways, connected subnets and the host's own addresses
//! from `/proc/net`, the resolvers and the host alias from the sandbox's own `/etc/resolv.conf` and `/etc/hosts`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use cardea_core::firewall::{Net, Resolved};

/// Names Podman gives the host in a container's `/etc/hosts`.
const HOST_ALIASES: [&str; 2] = ["host.containers.internal", "host.docker.internal"];

/// Resolves from the host's `/proc/net` and `view`: the sandbox's `/etc/resolv.conf` and `/etc/hosts`, one after
/// the other.
pub fn resolve(view: &str) -> Resolved {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let (mut gateway, mut lan) = routes4(&read("/proc/net/route"));
    let (gw6, lan6) = routes6(&read("/proc/net/ipv6_route"));
    gateway.extend(gw6);
    lan.extend(lan6);
    let (dns, alias) = sandbox_view(view);
    let mut host = alias;
    host.extend(local4(&read("/proc/net/fib_trie")));
    host.extend(local6(&read("/proc/net/if_inet6")));
    for v in [&mut host, &mut gateway] {
        v.sort();
        v.dedup();
    }
    Resolved {
        host,
        gateway,
        dns,
        lan,
    }
}

/// IPv4 default gateways and connected subnets from `/proc/net/route` (addresses printed as native-endian
/// numbers of their network-order bytes).
fn routes4(text: &str) -> (Vec<IpAddr>, Vec<Net>) {
    let (mut gateways, mut subnets) = (Vec::new(), Vec::new());
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let hex = |i: usize| f.get(i).and_then(|s| u32::from_str_radix(s, 16).ok());
        let (Some(iface), Some(dest), Some(gw), Some(mask)) = (f.first(), hex(1), hex(2), hex(7))
        else {
            continue;
        };
        if *iface == "lo" {
            continue;
        }
        if dest == 0 && mask == 0 && gw != 0 {
            gateways.push(IpAddr::V4(Ipv4Addr::from(gw.to_ne_bytes())));
        } else if gw == 0 && mask != 0 {
            subnets.push(Net {
                addr: IpAddr::V4(Ipv4Addr::from(dest.to_ne_bytes())),
                prefix: mask.count_ones() as u8,
            });
        }
    }
    (gateways, subnets)
}

/// IPv6 default gateways and connected subnets from `/proc/net/ipv6_route`.
fn routes6(text: &str) -> (Vec<IpAddr>, Vec<Net>) {
    let (mut gateways, mut subnets) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || f[9] == "lo" {
            continue;
        }
        let (Some(dest), Ok(prefix), Some(hop)) =
            (v6(f[0]), u8::from_str_radix(f[1], 16), v6(f[4]))
        else {
            continue;
        };
        if prefix == 0 && !hop.is_unspecified() {
            gateways.push(IpAddr::V6(hop));
        } else if hop.is_unspecified() && prefix > 0 && prefix < 128 && !dest.is_multicast() {
            subnets.push(Net {
                addr: IpAddr::V6(dest),
                prefix,
            });
        }
    }
    (gateways, subnets)
}

/// The host's own IPv4 addresses: `/32 host LOCAL` leaves of `/proc/net/fib_trie`.
fn local4(text: &str) -> Vec<IpAddr> {
    let mut out = Vec::new();
    let mut last: Option<Ipv4Addr> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(a) = t.strip_prefix("|-- ") {
            last = a.parse().ok();
        } else if t.starts_with("/32 host LOCAL")
            && let Some(a) = last
            && !a.is_loopback()
        {
            out.push(IpAddr::V4(a));
        }
    }
    out
}

/// The host's own IPv6 addresses from `/proc/net/if_inet6`.
fn local6(text: &str) -> Vec<IpAddr> {
    text.lines()
        .filter_map(|l| v6(l.split_whitespace().next()?))
        .filter(|a| !a.is_loopback())
        .map(IpAddr::V6)
        .collect()
}

fn v6(hex: &str) -> Option<Ipv6Addr> {
    (hex.len() == 32).then(|| u128::from_str_radix(hex, 16).ok().map(Ipv6Addr::from))?
}

/// Resolvers and host alias addresses from the sandbox's `/etc/resolv.conf` and `/etc/hosts`.
fn sandbox_view(text: &str) -> (Vec<IpAddr>, Vec<IpAddr>) {
    let (mut dns, mut host) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let mut f = line.split('#').next().unwrap_or("").split_whitespace();
        let (Some(first), rest) = (f.next(), f.collect::<Vec<_>>()) else {
            continue;
        };
        if first == "nameserver" {
            dns.extend(rest.first().and_then(|a| a.parse::<IpAddr>().ok()));
        } else if let Ok(a) = first.parse::<IpAddr>()
            && rest.iter().any(|n| HOST_ALIASES.contains(n))
        {
            host.push(a);
        }
    }
    (dns, host)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        enp4s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        enp4s0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
        wg0\t0000080A\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";
    const ROUTE6: &str = "fe800000000000000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001   enp4s0\n\
        00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003   enp4s0\n\
        00000000000000000000000000000001 80 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000002 00000000 80200001       lo\n\
        ff000000000000000000000000000000 08 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001   enp4s0\n";
    const TRIE: &str = "Main:\n  +-- 0.0.0.0/0 3 0 5\n     |-- 127.0.0.1\n        /32 host LOCAL\n     +-- 192.168.1.0/24 2 0 1\n        |-- 192.168.1.0\n           /24 link UNICAST\n        |-- 192.168.1.103\n           /32 host LOCAL\n        |-- 192.168.1.255\n           /32 link BROADCAST\n";
    const INET6: &str = "00000000000000000000000000000001 01 80 10 80       lo\nfe800000000000009caa6ffffe6d2ef3 02 40 20 80   enp4s0\n";

    #[test]
    fn routes_and_addresses() {
        let (gw, lan) = routes4(ROUTE);
        assert_eq!(gw, ["192.168.1.1".parse::<IpAddr>().unwrap()]);
        let lan: Vec<String> = lan.iter().map(ToString::to_string).collect();
        assert_eq!(lan, ["192.168.1.0/24", "10.8.0.0/16"]);
        let (gw6, lan6) = routes6(ROUTE6);
        assert_eq!(gw6, ["fe80::1".parse::<IpAddr>().unwrap()]);
        assert_eq!(
            lan6.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["fe80::/64"]
        );
        assert_eq!(local4(TRIE), ["192.168.1.103".parse::<IpAddr>().unwrap()]);
        assert_eq!(
            local6(INET6),
            ["fe80::9caa:6fff:fe6d:2ef3".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn sandbox_files() {
        let view = "# Generated by Podman\nsearch home\nnameserver 169.254.1.1\nnameserver fd00::1 # v6\n\
                    127.0.0.1 localhost\n169.254.1.2 host.containers.internal host.docker.internal\n10.0.0.9 box\n";
        let (dns, host) = sandbox_view(view);
        assert_eq!(
            dns,
            [
                "169.254.1.1".parse::<IpAddr>().unwrap(),
                "fd00::1".parse().unwrap()
            ]
        );
        assert_eq!(host, ["169.254.1.2".parse::<IpAddr>().unwrap()]);
    }

    #[test]
    fn this_host_resolves_without_panicking() {
        let r = resolve("");
        assert!(r.host.iter().all(|a| !a.is_loopback()));
    }
}
