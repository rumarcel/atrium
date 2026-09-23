//! The Core-to-Agent protocol: constants, message types and the framing
//! codec.
//!
//! This crate is deliberately **pure**: it performs no I/O, opens no file,
//! takes no path and links no runtime. Its only dependencies are `serde` and
//! `serde_json`, which describe and parse data. That rule is enforced
//! mechanically by `server/ci/boundary-checks.sh`, because it is what lets the
//! Core-to-Agent contract be reviewed by reading one small crate.
//!
//! - [`values`] — the validated strings that cross the boundary;
//! - [`messages`] — every frame, in both directions, and the complete M1
//!   operation table, [`messages::AgentOp`];
//! - [`frame`] — length-prefixed framing and strict decoding.
//!
//! The transport — the socket, the peer-credential check, the journal —
//! belongs to the two processes, not here.
//!
//! # Versioning
//!
//! [`AGENT_PROTOCOL_VERSION`] is independent of the public HTTP API version
//! (ADR-008). A mismatch fails closed: Agent refuses, Core marks the
//! privileged capability unavailable, and neither side negotiates, downgrades
//! or falls back to anything.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

pub mod frame;
pub mod messages;
pub mod values;

/// Version of the Core-to-Agent protocol.
///
/// Versioned **separately** from the public HTTP API (ADR-008): a mismatch here
/// fails closed and disables the privileged capability, and is never negotiated
/// down.
pub const AGENT_PROTOCOL_VERSION: u32 = 1;

/// Largest protocol frame body either side will accept, checked on the length
/// prefix before any allocation.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// The Unix socket Agent serves, created by `atrium-agent.socket`.
///
/// Mode `0660`, owner `root`, group `atrium`. Core connects; Agent accepts
/// only Core's uid, whoever else can reach the file.
pub const AGENT_SOCKET_PATH: &str = "/run/atrium/agent.sock";

/// Agent's root-only state directory. Unreadable by the `atrium` user.
pub const AGENT_STATE_DIR: &str = "/var/lib/atrium-agent";

#[cfg(test)]
mod tests {
    use super::messages::AgentOp;
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

    /// Criterion 38 and 46, protocol half. The operation table is enumerated
    /// by an exhaustive `match` with no wildcard, so adding a variant does not
    /// compile until this test is revisited — and the revisit must keep the
    /// assertions below true.
    #[test]
    fn operation_table_is_exactly_the_m1_table() {
        for op in AgentOp::ALL {
            let parameters: &[&str] = match op {
                AgentOp::AgentInfo => &[],
                AgentOp::RuntimeProbe => &[],
            };
            assert!(
                parameters.is_empty(),
                "{} must take no parameter in M1",
                op.as_str()
            );
        }
        let names: Vec<&str> = AgentOp::ALL.iter().map(|op| op.as_str()).collect();
        assert_eq!(names, ["agent_info", "runtime_probe"]);
        for op in AgentOp::ALL {
            assert_eq!(
                serde_json::to_string(&op).expect("encode"),
                format!("\"{}\"", op.as_str()),
                "a unit variant serializes as its bare name: no payload"
            );
        }
    }

    /// The type of every operation is a unit variant, so no parameter —
    /// a String, a PathBuf, a Vec<u8>, a key, a flag — can be expressed.
    /// serde confirms it: offering any payload to either variant is refused.
    #[test]
    fn no_operation_accepts_any_payload() {
        for op in AgentOp::ALL {
            for payload in [
                "\"/etc/shadow\"",
                "[\"sh\",\"-c\",\"id\"]",
                "{\"path\":\"/\"}",
                "{\"key\":\"AAAA\"}",
                "{\"skip_verify\":true}",
                "{}",
                "null",
            ] {
                let text = format!("{{\"{}\":{payload}}}", op.as_str());
                assert!(
                    serde_json::from_str::<AgentOp>(&text).is_err(),
                    "{text} must not decode"
                );
            }
        }
    }
}
