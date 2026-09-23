//! The two capabilities that come through Agent: `privileged` and
//! `container`.
//!
//! This is the plumbing M1F's `GET /api/v1/system/capabilities` will serve
//! (plan section 9.6). Both are derived **only** from what Agent answered, or
//! from why it did not. There is no second source: when Agent cannot be
//! reached, both are unavailable, with a reason, and Core tries nothing
//! else (criteria 29, 40).

use atrium_protocol::messages::{AgentInfo, Runtime, RuntimeProbe};
use atrium_protocol::values::Version;

use crate::agentclient::Failure;

/// Why a capability is unavailable, or what it is missing. A closed set;
/// the codes are stable and match plan section 9.5, extended as recorded in
/// the M1C as-built notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Agent's socket is missing, refused the connection, timed out or
    /// closed without answering.
    AgentUnreachable,
    /// Agent speaks another protocol version.
    AgentProtocolMismatch,
    /// Agent cannot write its journal, so it performs nothing.
    AgentJournalUnavailable,
    /// Agent answered something this protocol does not define, or refused
    /// a request Core believes well-formed.
    AgentProtocolError,
    /// No candidate runtime socket exists.
    NoContainerRuntime,
    /// A runtime socket exists but did not accept Agent's connection.
    ContainerRuntimeUnreachable,
    /// M1 does not ask a runtime for its version.
    RuntimeVersionProbeNotInM1,
    /// Container inventory is not an M1 feature.
    NotEnabledInM1,
}

impl Reason {
    /// The stable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::AgentUnreachable => "agent_unreachable",
            Self::AgentProtocolMismatch => "agent_protocol_mismatch",
            Self::AgentJournalUnavailable => "agent_journal_unavailable",
            Self::AgentProtocolError => "agent_protocol_error",
            Self::NoContainerRuntime => "no_container_runtime",
            Self::ContainerRuntimeUnreachable => "container_runtime_unreachable",
            Self::RuntimeVersionProbeNotInM1 => "runtime_version_probe_not_in_m1",
            Self::NotEnabledInM1 => "not_enabled_in_m1",
        }
    }
}

/// A feature a capability lacks, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Missing {
    /// The feature; `"*"` means the whole capability.
    pub feature: &'static str,
    /// Why.
    pub reason: Reason,
}

/// The `privileged` capability: is the Agent boundary there and speaking
/// Core's protocol?
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Privileged {
    /// Agent answered `AgentInfo`.
    pub available: bool,
    /// Agent's version, when it answered.
    pub agent_version: Option<Version>,
    /// Agent's protocol version, when it answered.
    pub protocol: Option<u32>,
    /// Why not, when unavailable.
    pub missing: Vec<Missing>,
}

/// The `container` capability, always `via: agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    /// A runtime socket exists and accepted Agent's connection.
    pub available: bool,
    /// Which runtime, when one was found.
    pub provider: Option<Runtime>,
    /// What is missing, and why.
    pub missing: Vec<Missing>,
}

impl Container {
    /// Always `"agent"`: Core has no other route to a runtime.
    #[must_use]
    pub fn via(&self) -> &'static str {
        "agent"
    }
}

/// Both capabilities, as of the last refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCapabilities {
    /// `privileged`.
    pub privileged: Privileged,
    /// `container`.
    pub container: Container,
}

fn whole(reason: Reason) -> Vec<Missing> {
    vec![Missing {
        feature: "*",
        reason,
    }]
}

impl AgentCapabilities {
    /// Derives both capabilities. `probe` is `None` when it was not asked
    /// because `AgentInfo` already failed.
    #[must_use]
    pub fn derive(
        info: &Result<AgentInfo, Failure>,
        probe: Option<&Result<RuntimeProbe, Failure>>,
    ) -> Self {
        let privileged = match info {
            Ok(info) => Privileged {
                available: true,
                agent_version: Some(info.agent_version.clone()),
                protocol: Some(info.protocol),
                missing: Vec::new(),
            },
            Err(failure) => Privileged {
                available: false,
                agent_version: None,
                protocol: None,
                missing: whole(failure.reason()),
            },
        };
        let container = match (info, probe) {
            (Err(failure), _) | (Ok(_), Some(Err(failure))) => Container {
                available: false,
                provider: None,
                missing: whole(failure.reason()),
            },
            (Ok(_), None) => Container {
                available: false,
                provider: None,
                missing: whole(Reason::AgentProtocolError),
            },
            (Ok(_), Some(Ok(probe))) => match (probe.runtime, probe.reachable) {
                (None, _) => Container {
                    available: false,
                    provider: None,
                    missing: whole(Reason::NoContainerRuntime),
                },
                (Some(runtime), false) => Container {
                    available: false,
                    provider: Some(runtime),
                    missing: whole(Reason::ContainerRuntimeUnreachable),
                },
                (Some(runtime), true) => Container {
                    available: true,
                    provider: Some(runtime),
                    missing: vec![
                        Missing {
                            feature: "version",
                            reason: Reason::RuntimeVersionProbeNotInM1,
                        },
                        Missing {
                            feature: "inventory",
                            reason: Reason::NotEnabledInM1,
                        },
                    ],
                },
            },
        };
        Self {
            privileged,
            container,
        }
    }

    /// The first reason each capability is unavailable, for logs.
    #[must_use]
    pub fn summary(&self) -> (Option<Reason>, Option<Reason>) {
        let first = |available: bool, missing: &[Missing]| {
            (!available)
                .then(|| missing.first().map(|m| m.reason))
                .flatten()
        };
        (
            first(self.privileged.available, &self.privileged.missing),
            first(self.container.available, &self.container.missing),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_protocol::messages::{
        JournalInfo, NotProbed, RuntimeSocket, StateDirInfo, VersionReason,
    };
    use atrium_protocol::values::{ReportedPath, Timestamp};

    fn info() -> AgentInfo {
        AgentInfo {
            agent_version: Version::parse("0.1.0").expect("v"),
            protocol: 1,
            uid: 0,
            pid: 1,
            started_at: Timestamp::parse("2026-09-24T10:00:00Z").expect("ts"),
            boot_id: None,
            state_dir: StateDirInfo {
                path: ReportedPath::parse("/var/lib/atrium-agent").expect("path"),
                mode: None,
                uid: None,
                gid: None,
            },
            journal: JournalInfo {
                writable: true,
                last_seq: 1,
            },
            capability_bounding_set_empty: Some(true),
            no_new_privileges: Some(true),
        }
    }

    fn probe(socket: Option<RuntimeSocket>, reachable: bool) -> RuntimeProbe {
        RuntimeProbe {
            runtime: socket.map(RuntimeSocket::runtime),
            socket,
            reachable,
            version: NotProbed,
            version_reason: VersionReason::RuntimeVersionProbeNotInM1,
            also_present: Vec::new(),
        }
    }

    #[test]
    fn agent_unreachable_makes_both_unavailable() {
        let caps = AgentCapabilities::derive(&Err(Failure::SocketMissing), None);
        assert!(!caps.privileged.available);
        assert!(!caps.container.available);
        assert_eq!(
            caps.summary(),
            (
                Some(Reason::AgentUnreachable),
                Some(Reason::AgentUnreachable)
            )
        );
        assert_eq!(caps.container.via(), "agent");
    }

    #[test]
    fn a_mismatch_is_named() {
        let caps =
            AgentCapabilities::derive(&Err(Failure::ProtocolMismatch { agent_protocol: 2 }), None);
        assert_eq!(
            caps.summary(),
            (
                Some(Reason::AgentProtocolMismatch),
                Some(Reason::AgentProtocolMismatch)
            )
        );
    }

    #[test]
    fn docker_found_through_agent() {
        let caps = AgentCapabilities::derive(
            &Ok(info()),
            Some(&Ok(probe(Some(RuntimeSocket::DockerVarRun), true))),
        );
        assert!(caps.privileged.available);
        assert!(caps.container.available);
        assert_eq!(caps.container.provider, Some(Runtime::Docker));
        assert_eq!(caps.container.via(), "agent");
        assert_eq!(
            caps.container.missing[0].reason.code(),
            "runtime_version_probe_not_in_m1"
        );
    }

    #[test]
    fn no_runtime_and_unreachable_runtime_differ() {
        let none = AgentCapabilities::derive(&Ok(info()), Some(&Ok(probe(None, false))));
        assert_eq!(none.summary().1, Some(Reason::NoContainerRuntime));
        let stale = AgentCapabilities::derive(
            &Ok(info()),
            Some(&Ok(probe(Some(RuntimeSocket::PodmanRun), false))),
        );
        assert_eq!(stale.summary().1, Some(Reason::ContainerRuntimeUnreachable));
        assert!(stale.privileged.available);
    }

    #[test]
    fn codes_are_unique() {
        let all = [
            Reason::AgentUnreachable,
            Reason::AgentProtocolMismatch,
            Reason::AgentJournalUnavailable,
            Reason::AgentProtocolError,
            Reason::NoContainerRuntime,
            Reason::ContainerRuntimeUnreachable,
            Reason::RuntimeVersionProbeNotInM1,
            Reason::NotEnabledInM1,
        ];
        let codes: std::collections::HashSet<&str> = all.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), all.len());
    }
}
