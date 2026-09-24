//! `Host` validation: the DNS-rebinding defence (SECURITY §14).
//!
//! A request is served only if its `Host` names this server: one of the DNS
//! names or IP addresses in the certificate Core is currently serving (its
//! own `atrium-<short_id>.local`, `localhost`, the loopbacks and every current
//! address), **and** the port Core is listening on. The allowlist comes from
//! Core's own certificate record, never from the request, so a successful TLS
//! handshake to the right IP does not make `Host: attacker.example` acceptable.
//!
//! Parsing is strict and fails closed. There is no percent-decoding, no
//! trailing-dot folding, no IDNA, no zone ids, no short or octal IPv4 forms,
//! and no default port. A value that does not parse is not "probably fine".

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::certificate::SubjectNames;

/// A parsed host, normalised for comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostName {
    /// A DNS name, lowercased.
    Dns(String),
    /// An IPv4 or IPv6 address.
    Ip(IpAddr),
}

/// `host[:port]`, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    /// The host.
    pub host: HostName,
    /// The port, when one was given.
    pub port: Option<u16>,
}

/// Longest `Host` value considered at all.
const MAX_AUTHORITY: usize = 262;

fn dns_label(label: &str) -> bool {
    (1..=63).contains(&label.len())
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match text.parse::<u16>() {
        Ok(0) | Err(_) => None,
        Ok(port) => Some(port),
    }
}

fn parse_host(text: &str) -> Option<HostName> {
    if text.is_empty() || text.len() > 253 {
        return None;
    }
    // Digits and dots only: it must be a canonical dotted quad. Rust's parser
    // refuses leading zeros, short forms (`127.1`) and hex, so there is one
    // spelling per address.
    if text.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return text
            .parse::<Ipv4Addr>()
            .ok()
            .map(|ip| HostName::Ip(ip.into()));
    }
    if text.split('.').all(dns_label) {
        // The last label of a name is never all digits: that would be an
        // address in disguise.
        let last = text.rsplit('.').next().unwrap_or("");
        if last.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return Some(HostName::Dns(text.to_ascii_lowercase()));
    }
    None
}

/// Parses `host`, `host:port`, `[v6]` or `[v6]:port`.
///
/// # Errors
///
/// `None` for anything else, including an empty value, whitespace, a
/// userinfo part, a zone id or a trailing dot.
#[must_use]
pub fn parse_authority(text: &str) -> Option<Authority> {
    if text.is_empty() || text.len() > MAX_AUTHORITY || !text.is_ascii() {
        return None;
    }
    if let Some(rest) = text.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        if inside.contains('%') {
            return None;
        }
        let ip: Ipv6Addr = inside.parse().ok()?;
        let port = match after {
            "" => None,
            _ => Some(parse_port(after.strip_prefix(':')?)?),
        };
        return Some(Authority {
            host: HostName::Ip(IpAddr::V6(ip)),
            port,
        });
    }
    let (host, port) = match text.split_once(':') {
        None => (text, None),
        Some((host, port)) => (host, Some(parse_port(port)?)),
    };
    Some(Authority {
        host: parse_host(host)?,
        port,
    })
}

/// The names and addresses this server answers to, and its port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAllowlist {
    dns: BTreeSet<String>,
    ips: BTreeSet<IpAddr>,
    port: u16,
}

impl HostAllowlist {
    /// From the certificate Core is serving, so the names a client may use
    /// are exactly the names the certificate proves.
    #[must_use]
    pub fn from_names(names: &SubjectNames, port: u16) -> Self {
        Self {
            dns: names.dns.iter().map(|n| n.to_ascii_lowercase()).collect(),
            ips: names.ips.clone(),
            port,
        }
    }

    /// Whether `authority` names this server. The port must be given and must
    /// be Core's: Core never listens on a default port, so an absent port
    /// names some other service.
    #[must_use]
    pub fn admits(&self, authority: &Authority) -> bool {
        if authority.port != Some(self.port) {
            return false;
        }
        match &authority.host {
            HostName::Dns(name) => self.dns.contains(name),
            HostName::Ip(ip) => self.ips.contains(ip),
        }
    }
}

/// Whether an `Origin` header value is exactly this request's own origin:
/// `https://` followed by an authority equal to the validated `Host`.
#[must_use]
pub fn is_same_origin(origin: &str, host: &Authority) -> bool {
    origin
        .strip_prefix("https://")
        .and_then(parse_authority)
        .is_some_and(|origin| &origin == host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist() -> HostAllowlist {
        let names = SubjectNames {
            dns: ["atrium-0a1b2c3d.local", "localhost"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ips: ["127.0.0.1", "::1", "192.168.1.20", "fd00::20"]
                .into_iter()
                .map(|ip| ip.parse().expect("ip"))
                .collect(),
        };
        HostAllowlist::from_names(&names, 7443)
    }

    fn admitted(host: &str) -> bool {
        parse_authority(host).is_some_and(|a| allowlist().admits(&a))
    }

    #[test]
    fn approved_hosts_are_admitted() {
        for host in [
            "localhost:7443",
            "LOCALHOST:7443",
            "atrium-0a1b2c3d.local:7443",
            "Atrium-0A1B2C3D.Local:7443",
            "127.0.0.1:7443",
            "192.168.1.20:7443",
            "[::1]:7443",
            "[fd00::20]:7443",
            "[FD00:0:0:0:0:0:0:20]:7443",
        ] {
            assert!(admitted(host), "{host} must be admitted");
        }
    }

    #[test]
    fn other_names_ports_and_rebinding_hosts_are_refused() {
        for host in [
            "attacker.example:7443",
            "attacker.example",
            "localhost",
            "localhost:443",
            "localhost:8443",
            "192.168.1.21:7443",
            "atrium-0a1b2c3d.local.attacker.example:7443",
            "atrium-ffffffff.local:7443",
            "[::2]:7443",
            "[::ffff:127.0.0.1]:7443",
        ] {
            assert!(!admitted(host), "{host} must be refused");
        }
    }

    #[test]
    fn alternate_spellings_do_not_parse() {
        for host in [
            "",
            " ",
            "localhost :7443",
            "localhost:7443 ",
            "localhost.:7443",
            "localhost:",
            "localhost:07443x",
            "localhost:0",
            "localhost:65536",
            "localhost:+7443",
            "localhost:7443:7443",
            "user@localhost:7443",
            "127.1:7443",
            "0x7f.0.0.1:7443",
            "0177.0.0.1:7443",
            "127.000.000.001:7443",
            "2130706433:7443",
            "::1:7443",
            "[::1%eth0]:7443",
            "[::1%25eth0]:7443",
            "[::1]x:7443",
            "[127.0.0.1]:7443",
            "loc_alhost:7443",
            "-localhost:7443",
            "localhost-.local:7443",
            "l\u{f6}calhost:7443",
            "xn--lcalhost-7ya:7443x",
            "a..b:7443",
            "1.2.3.4.5:7443",
            "foo.123:7443",
        ] {
            assert!(
                parse_authority(host).is_none_or(|a| !allowlist().admits(&a)),
                "{host:?} must not be admitted"
            );
        }
        assert!(parse_authority(&format!("{}:7443", "a".repeat(300))).is_none());
    }

    #[test]
    fn only_the_exact_own_origin_is_same_origin() {
        let host = parse_authority("atrium-0a1b2c3d.local:7443").expect("host");
        assert!(is_same_origin("https://atrium-0a1b2c3d.local:7443", &host));
        assert!(is_same_origin("https://ATRIUM-0a1b2c3d.local:7443", &host));
        for origin in [
            "http://atrium-0a1b2c3d.local:7443",
            "https://atrium-0a1b2c3d.local",
            "https://atrium-0a1b2c3d.local:7444",
            "https://attacker.example",
            "null",
            "",
            "https://atrium-0a1b2c3d.local:7443/",
            "https://atrium-0a1b2c3d.local:7443.attacker.example",
        ] {
            assert!(!is_same_origin(origin, &host), "{origin:?}");
        }
    }
}
