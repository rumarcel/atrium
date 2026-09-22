//! Minimal `sd_notify` support.
//!
//! systemd's readiness protocol is a datagram on the socket named by
//! `NOTIFY_SOCKET` — that is the whole protocol. ADR-007 says to implement it
//! directly rather than take a dependency for forty lines, so this module is
//! those forty lines.
//!
//! This file is duplicated, deliberately, in `atrium-agent`. The alternative
//! would be a shared crate, and the three shared crates are I/O-free by policy
//! (`server/ci/boundary-checks.sh` enforces it) precisely so that the
//! Core-to-Agent contract stays reviewable. Duplicating a small, stable,
//! well-understood module is the cheaper of the two costs.

use std::io;

/// Sends one state line to the service manager.
///
/// Returns `Ok(false)` when there is no service manager to talk to, which is
/// the normal case outside systemd — during tests, and when a developer runs
/// the binary by hand. A missing manager is not an error.
pub fn notify(state: &str) -> io::Result<bool> {
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else {
        return Ok(false);
    };
    send(socket.as_os_str(), state.as_bytes())
}

/// Signals that startup is complete.
pub fn ready() -> io::Result<bool> {
    notify("READY=1\n")
}

/// Signals that shutdown has begun.
pub fn stopping() -> io::Result<bool> {
    notify("STOPPING=1\n")
}

#[cfg(unix)]
fn send(socket: &std::ffi::OsStr, payload: &[u8]) -> io::Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixDatagram;

    let bytes = socket.as_bytes();
    if bytes.is_empty() {
        return Ok(false);
    }

    let datagram = UnixDatagram::unbound()?;

    // A leading '@' names the abstract namespace; anything else is a path.
    if bytes[0] == b'@' {
        #[cfg(target_os = "linux")]
        {
            use std::os::linux::net::SocketAddrExt;
            let address = std::os::unix::net::SocketAddr::from_abstract_name(&bytes[1..])?;
            datagram.send_to_addr(payload, &address)?;
            return Ok(true);
        }
        // Abstract sockets are a Linux concept. Atrium's server components run
        // on Linux; elsewhere there is nothing to notify.
        #[cfg(not(target_os = "linux"))]
        return Ok(false);
    }

    datagram.send_to(payload, std::path::Path::new(socket))?;
    Ok(true)
}

#[cfg(not(unix))]
fn send(_socket: &std::ffi::OsStr, _payload: &[u8]) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_manager_is_not_an_error() {
        // The test process does not run under systemd; NOTIFY_SOCKET is unset,
        // and notify() must report "nobody listening" rather than fail.
        if std::env::var_os("NOTIFY_SOCKET").is_none() {
            assert!(!notify("READY=1\n").expect("absent manager must not error"));
        }
    }
}
