//! Network interfaces (plan §9.3).
//!
//! The answer is a **list**, sorted by interface name for a stable order and
//! for nothing else. There is no primary interface, no "the IP", and no code
//! path that takes `addresses[0]` (criterion 23).
//!
//! Interfaces come from `/sys/class/net`; each field there degrades alone.
//! Addresses come from `getifaddrs(3)` — IPv4 and IPv6, every scope — and are
//! printed canonically (RFC 5952 for IPv6). An interface with no address has
//! an empty list; an interface that disappears while being read is left out.
//! These are host-interface counters, not a measure of Internet speed, and
//! M1 reports cumulative byte counts, not rates.

use std::net::IpAddr;

use serde::Serialize;

use super::{read_line, Absences, HostRoot, ReadFailure, Reason, Sourced, Unavailable};

/// Interfaces examined at most.
const MAX_INTERFACES: usize = 1024;
/// Addresses kept per interface at most.
const MAX_ADDRESSES: usize = 128;
/// `IFF_LOOPBACK`.
const IFF_LOOPBACK: u64 = 0x8;

/// One address on an interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    /// `ipv4` or `ipv6`.
    pub family: &'static str,
    /// Canonical text.
    pub address: String,
    /// From the netmask; `null` when the mask is absent or not contiguous.
    pub prefix_length: Option<u8>,
    /// `host` (loopback), `link` (link-local) or `global`, as the kernel
    /// scopes addresses.
    pub scope: &'static str,
}

/// Cumulative byte counters since the interface was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    /// Bytes received.
    pub rx_bytes: Option<u64>,
    /// Bytes sent.
    pub tx_bytes: Option<u64>,
}

/// One interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interface {
    /// The kernel's name for it.
    pub name: String,
    /// RFC 2863 operational state as the kernel words it.
    pub oper_state: Option<String>,
    /// Link carrier.
    pub carrier: Option<bool>,
    /// Link speed in megabits per second.
    pub speed_mbps: Option<u32>,
    /// MTU in bytes.
    pub mtu: Option<u32>,
    /// Hardware address.
    pub mac_address: Option<String>,
    /// Not backed by a device (bridges, veths, tunnels, loopback).
    #[serde(rename = "virtual")]
    pub is_virtual: Option<bool>,
    /// The loopback interface (`IFF_LOOPBACK`).
    pub loopback: Option<bool>,
    /// Every address, IPv4 first.
    pub addresses: Vec<Address>,
    /// Counters.
    pub statistics: Statistics,
    /// What could not be read, and why.
    pub unavailable: Vec<Unavailable>,
}

/// `GET /api/v1/network/interfaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Interfaces {
    /// Every interface.
    pub interfaces: Vec<Interface>,
    /// What could not be read for the whole list, and why.
    pub unavailable: Vec<Unavailable>,
}

/// An address as `getifaddrs` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAddress {
    /// Interface name.
    pub interface: String,
    /// The address.
    pub address: IpAddr,
    /// The netmask, of the same family.
    pub netmask: Option<IpAddr>,
}

/// Every IPv4 and IPv6 address on the machine.
///
/// # Errors
///
/// `not_reported` when `getifaddrs` fails.
pub fn system_addresses() -> Sourced<Vec<RawAddress>> {
    let addresses = nix::ifaddrs::getifaddrs().map_err(|_| Reason::NotReported)?;
    let ip = |storage: &nix::sys::socket::SockaddrStorage| -> Option<IpAddr> {
        if let Some(v4) = storage.as_sockaddr_in() {
            return Some(IpAddr::V4(v4.ip()));
        }
        storage.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
    };
    Ok(addresses
        .filter_map(|entry| {
            let address = ip(entry.address.as_ref()?)?;
            Some(RawAddress {
                interface: entry.interface_name,
                address,
                netmask: entry.netmask.as_ref().and_then(ip),
            })
        })
        .collect())
}

fn prefix_length(netmask: Option<IpAddr>) -> Option<u8> {
    let (bits, width) = match netmask? {
        IpAddr::V4(mask) => (u128::from(u32::from(mask)) << 96, 32),
        IpAddr::V6(mask) => (u128::from(mask), 128),
    };
    let ones = bits.leading_ones();
    // Contiguous: every set bit is at the top.
    (bits.count_ones() == ones && ones <= width).then(|| u8::try_from(ones).ok())?
}

fn scope(address: IpAddr) -> &'static str {
    match address {
        IpAddr::V4(v4) if v4.is_loopback() => "host",
        IpAddr::V4(v4) if v4.is_link_local() => "link",
        IpAddr::V6(v6) if v6.is_loopback() => "host",
        IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => "link",
        _ => "global",
    }
}

fn to_address(raw: &RawAddress) -> Option<Address> {
    let (family, netmask) = match (raw.address, raw.netmask) {
        (IpAddr::V4(_), mask @ (None | Some(IpAddr::V4(_))))
        | (IpAddr::V6(_), mask @ (None | Some(IpAddr::V6(_)))) => (
            if raw.address.is_ipv4() {
                "ipv4"
            } else {
                "ipv6"
            },
            mask,
        ),
        _ => return None,
    };
    Some(Address {
        family,
        address: raw.address.to_string(),
        prefix_length: prefix_length(netmask),
        scope: scope(raw.address),
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'/' && b != b':')
}

const OPER_STATES: [&str; 7] = [
    "unknown",
    "notpresent",
    "down",
    "lowerlayerdown",
    "testing",
    "dormant",
    "up",
];

fn line(root: &HostRoot, name: &str, file: &str) -> Result<String, ReadFailure> {
    read_line(&root.path(&format!("sys/class/net/{name}/{file}")))
}

fn number<T: std::str::FromStr>(root: &HostRoot, name: &str, file: &str) -> Sourced<T> {
    match line(root, name, file) {
        Ok(text) => text.trim().parse().map_err(|_| Reason::SourceMalformed),
        Err(ReadFailure::Malformed) => Err(Reason::SourceMalformed),
        Err(_) => Err(Reason::NotReported),
    }
}

fn mac(text: &str) -> Option<String> {
    let groups: Vec<&str> = text.trim().split(':').collect();
    let valid = (1..=32).contains(&groups.len())
        && groups
            .iter()
            .all(|g| g.len() == 2 && g.bytes().all(|b| b.is_ascii_hexdigit()));
    valid.then(|| text.trim().to_ascii_lowercase())
}

fn interface(root: &HostRoot, name: &str, addresses: &[RawAddress]) -> Option<Interface> {
    let mut absences = Absences::default();
    let oper_state = match line(root, name, "operstate") {
        Ok(state) if OPER_STATES.contains(&state.trim()) => Some(state.trim().to_owned()),
        Ok(_) | Err(ReadFailure::Malformed) => absences.none("operState", Reason::SourceMalformed),
        // Gone between listing and reading.
        Err(ReadFailure::Missing) if !root.path(&format!("sys/class/net/{name}")).exists() => {
            return None
        }
        Err(_) => absences.none("operState", Reason::NotReported),
    };
    // Reading `carrier` or `speed` on a down link fails with EINVAL.
    let carrier = match number::<u8>(root, name, "carrier") {
        Ok(0) => Some(false),
        Ok(1) => Some(true),
        Ok(_) => absences.none("carrier", Reason::SourceMalformed),
        Err(reason) => absences.none("carrier", reason),
    };
    let speed_mbps = match number::<i64>(root, name, "speed") {
        Ok(speed) if speed > 0 => u32::try_from(speed)
            .ok()
            .or_else(|| absences.none("speedMbps", Reason::SourceMalformed)),
        // -1 is the kernel's SPEED_UNKNOWN.
        Ok(_) | Err(Reason::NotReported) => absences.none("speedMbps", Reason::NotReportedByDriver),
        Err(reason) => absences.none("speedMbps", reason),
    };
    let mtu = absences.take("mtu", number::<u32>(root, name, "mtu"));
    let mac_address = match line(root, name, "address") {
        Ok(text) => mac(&text).or_else(|| absences.none("macAddress", Reason::SourceMalformed)),
        Err(_) => absences.none("macAddress", Reason::NotReported),
    };
    let is_virtual =
        match std::fs::symlink_metadata(root.path(&format!("sys/class/net/{name}/device"))) {
            Ok(_) => Some(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
            Err(_) => absences.none("virtual", Reason::NotReported),
        };
    let loopback = match line(root, name, "flags") {
        Ok(text) => u64::from_str_radix(text.trim().trim_start_matches("0x"), 16)
            .ok()
            .map(|flags| flags & IFF_LOOPBACK != 0)
            .or_else(|| absences.none("loopback", Reason::SourceMalformed)),
        Err(_) => absences.none("loopback", Reason::NotReported),
    };
    let statistics = Statistics {
        rx_bytes: absences.take(
            "statistics.rxBytes",
            number(root, name, "statistics/rx_bytes"),
        ),
        tx_bytes: absences.take(
            "statistics.txBytes",
            number(root, name, "statistics/tx_bytes"),
        ),
    };
    let mut own: Vec<(IpAddr, Address)> = addresses
        .iter()
        .filter(|raw| raw.interface == name)
        .filter_map(|raw| Some((raw.address, to_address(raw)?)))
        .collect();
    own.sort_by_key(|a| (a.0.is_ipv6(), a.0));
    own.dedup_by(|a, b| a.1 == b.1);
    own.truncate(MAX_ADDRESSES);
    Some(Interface {
        name: name.to_owned(),
        oper_state,
        carrier,
        speed_mbps,
        mtu,
        mac_address,
        is_virtual,
        loopback,
        addresses: own.into_iter().map(|(_, address)| address).collect(),
        statistics,
        unavailable: absences.into_vec(),
    })
}

/// Builds the list from `/sys/class/net` under `root` and the given
/// addresses (`Err` when they could not be listed).
#[must_use]
pub fn assemble(root: &HostRoot, addresses: Sourced<Vec<RawAddress>>) -> Interfaces {
    let mut absences = Absences::default();
    let addresses = match addresses {
        Ok(addresses) => addresses,
        Err(reason) => {
            absences.none::<()>("addresses", reason);
            Vec::new()
        }
    };
    let names = match std::fs::read_dir(root.path("sys/class/net")) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| valid_name(name))
                .collect();
            names.sort();
            names.truncate(MAX_INTERFACES);
            names
        }
        Err(_) => {
            absences.none::<()>("interfaces", Reason::SysfsUnreadable);
            Vec::new()
        }
    };
    Interfaces {
        interfaces: names
            .iter()
            .filter_map(|name| interface(root, name, &addresses))
            .collect(),
        unavailable: absences.into_vec(),
    }
}

/// The machine's interfaces.
#[must_use]
pub fn read(root: &HostRoot) -> Interfaces {
    assemble(root, system_addresses())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fixture::Tree;

    fn raw(interface: &str, address: &str, netmask: Option<&str>) -> RawAddress {
        RawAddress {
            interface: interface.into(),
            address: address.parse().expect("address"),
            netmask: netmask.map(|m| m.parse().expect("mask")),
        }
    }

    fn nic(tree: &Tree, name: &str, physical: bool) {
        nic_without_carrier(tree, name, physical);
        tree.write(&format!("sys/class/net/{name}/carrier"), "1\n");
    }

    fn nic_without_carrier(tree: &Tree, name: &str, physical: bool) {
        let base = format!("sys/class/net/{name}");
        tree.write(&format!("{base}/operstate"), "up\n");
        tree.write(&format!("{base}/speed"), "1000\n");
        tree.write(&format!("{base}/mtu"), "1500\n");
        tree.write(&format!("{base}/address"), "52:54:00:AB:cd:01\n");
        tree.write(&format!("{base}/flags"), "0x1003\n");
        tree.write(&format!("{base}/statistics/rx_bytes"), "123456\n");
        tree.write(&format!("{base}/statistics/tx_bytes"), "654321\n");
        if physical {
            tree.mkdir(&format!("{base}/device"));
        }
    }

    #[test]
    fn two_nics_every_address_both_families_no_primary() {
        let tree = Tree::new();
        nic(&tree, "enp1s0", true);
        nic(&tree, "enp2s0", true);
        let base = "sys/class/net/lo";
        tree.write(&format!("{base}/operstate"), "unknown\n");
        tree.write(&format!("{base}/mtu"), "65536\n");
        tree.write(&format!("{base}/address"), "00:00:00:00:00:00\n");
        tree.write(&format!("{base}/flags"), "0x9\n");
        let addresses = vec![
            raw(
                "enp2s0",
                "fe80::5054:ff:feab:cd02",
                Some("ffff:ffff:ffff:ffff::"),
            ),
            raw("enp2s0", "10.0.0.5", Some("255.0.0.0")),
            raw("enp1s0", "192.168.1.20", Some("255.255.255.0")),
            raw(
                "enp1s0",
                "2001:db8:0:0:0:0:0:20",
                Some("ffff:ffff:ffff:ffff::"),
            ),
            raw("enp1s0", "192.168.1.21", Some("255.255.255.0")),
            raw("lo", "127.0.0.1", Some("255.0.0.0")),
            raw("lo", "::1", Some("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff")),
        ];
        let view = assemble(&tree.root(), Ok(addresses));
        assert!(view.unavailable.is_empty());
        let names: Vec<&str> = view.interfaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["enp1s0", "enp2s0", "lo"]);
        let first = &view.interfaces[0];
        let printed: Vec<(&str, &str, Option<u8>, &str)> = first
            .addresses
            .iter()
            .map(|a| (a.family, a.address.as_str(), a.prefix_length, a.scope))
            .collect();
        assert_eq!(
            printed,
            [
                ("ipv4", "192.168.1.20", Some(24), "global"),
                ("ipv4", "192.168.1.21", Some(24), "global"),
                ("ipv6", "2001:db8::20", Some(64), "global"),
            ]
        );
        assert_eq!(first.speed_mbps, Some(1000));
        assert_eq!(first.mac_address.as_deref(), Some("52:54:00:ab:cd:01"));
        assert_eq!(
            (first.is_virtual, first.loopback),
            (Some(false), Some(false))
        );
        let second = &view.interfaces[1];
        assert_eq!(second.addresses[1].scope, "link");
        let lo = &view.interfaces[2];
        assert_eq!((lo.is_virtual, lo.loopback), (Some(true), Some(true)));
        assert_eq!(lo.addresses[0].scope, "host");
        // lo reports no carrier, speed or counters here: null with reasons.
        assert_eq!(lo.speed_mbps, None);
        assert!(lo.unavailable.contains(&Unavailable {
            field: "speedMbps".into(),
            reason: Reason::NotReportedByDriver
        }));
    }

    #[test]
    fn missing_speed_or_carrier_is_never_zero() {
        let tree = Tree::new();
        nic_without_carrier(&tree, "wlan0", true);
        tree.write("sys/class/net/wlan0/speed", "-1\n");
        let view = assemble(&tree.root(), Ok(Vec::new()));
        let wlan = &view.interfaces[0];
        assert_eq!(wlan.speed_mbps, None);
        assert_eq!(wlan.carrier, None);
        assert!(wlan.addresses.is_empty(), "no address is an empty list");
        assert!(wlan.unavailable.contains(&Unavailable {
            field: "speedMbps".into(),
            reason: Reason::NotReportedByDriver
        }));
        assert!(wlan.unavailable.contains(&Unavailable {
            field: "carrier".into(),
            reason: Reason::NotReported
        }));
    }

    #[test]
    fn prefixes_are_exact_and_odd_masks_are_null() {
        assert_eq!(
            prefix_length(Some("255.255.255.0".parse().unwrap())),
            Some(24)
        );
        assert_eq!(prefix_length(Some("0.0.0.0".parse().unwrap())), Some(0));
        assert_eq!(prefix_length(Some("255.0.255.0".parse().unwrap())), None);
        assert_eq!(
            prefix_length(Some("ffff:ffff::".parse().unwrap())),
            Some(32)
        );
        assert_eq!(prefix_length(None), None);
    }

    #[test]
    fn hostile_names_and_malformed_values_are_refused() {
        let tree = Tree::new();
        nic(&tree, "eth0", false);
        tree.write("sys/class/net/eth0/operstate", "sideways\n");
        tree.write("sys/class/net/eth0/mtu", "big\n");
        tree.write("sys/class/net/eth0/address", "not-a-mac\n");
        tree.mkdir("sys/class/net/averyveryverylongname");
        let view = assemble(&tree.root(), Err(Reason::NotReported));
        assert_eq!(
            view.interfaces.len(),
            1,
            "an over-long name is not an interface"
        );
        let eth = &view.interfaces[0];
        assert_eq!(
            (eth.oper_state.as_ref(), eth.mtu, eth.mac_address.as_ref()),
            (None, None, None)
        );
        for field in ["operState", "mtu", "macAddress"] {
            assert!(
                eth.unavailable.contains(&Unavailable {
                    field: field.into(),
                    reason: Reason::SourceMalformed
                }),
                "{field}"
            );
        }
        assert_eq!(view.unavailable[0].field, "addresses");
    }

    #[test]
    fn no_sysfs_is_an_empty_list_with_a_reason() {
        let tree = Tree::new();
        let view = assemble(&tree.root(), Ok(Vec::new()));
        assert!(view.interfaces.is_empty());
        assert_eq!(view.unavailable[0].reason, Reason::SysfsUnreadable);
    }
}
