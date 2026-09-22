//! Constants shared by Atrium Core and Atrium Agent.
//!
//! This crate is deliberately **pure**: it performs no I/O, opens no file,
//! takes no path and links no runtime. That rule is enforced mechanically by
//! `server/ci/boundary-checks.sh`, because it is what lets the Core-to-Agent
//! contract be reviewed by reading one small crate.
//!
//! # What is here in M1A
//!
//! The identifiers that are frozen by the architecture: the protocol version,
//! the framing bound and the socket path. Nothing else.
//!
//! # What is deliberately not here yet
//!
//! The typed operation enum, the request/response envelopes and the framing
//! codec arrive in **M1C**, with their own tests and their own review. M1A does
//! not define them, because an operation table that exists before it is needed
//! is an operation table nobody reviewed.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

/// Version of the Core-to-Agent protocol.
///
/// Versioned **separately** from the public HTTP API (ADR-008): a mismatch here
/// fails closed and disables the privileged capability, and is never negotiated
/// down.
pub const AGENT_PROTOCOL_VERSION: u32 = 1;

/// Largest protocol frame Agent will accept, checked before any allocation.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// The Unix socket Agent serves, created by `atrium-agent.socket`.
///
/// Mode `0660`, owner `root`, group `atrium`. Core connects; nothing else can.
pub const AGENT_SOCKET_PATH: &str = "/run/atrium/agent.sock";

/// Agent's root-only state directory. Unreadable by the `atrium` user.
pub const AGENT_STATE_DIR: &str = "/var/lib/atrium-agent";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_is_in_the_atrium_runtime_namespace() {
        // ADR-015 reserves this namespace so the canary cannot collide with the
        // prototype's agent.
        assert!(AGENT_SOCKET_PATH.starts_with("/run/atrium/"));
        assert!(!AGENT_SOCKET_PATH.contains("personal-hub"));
    }

    #[test]
    fn frame_bound_is_small_enough_to_allocate_eagerly() {
        assert_eq!(MAX_FRAME_BYTES, 65_536);
    }
}
