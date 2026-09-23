//! The messages, in both directions.
//!
//! One connection carries exactly one exchange, in this order:
//!
//! ```text
//! Core  -> Agent   {"hello":{"protocol":1,"core_version":"0.1.0","request_id":"3f1c"}}
//! Agent -> Core    {"hello_ok":{"protocol":1,"agent_version":"0.1.0"}}
//! Core  -> Agent   {"call":{"op":"agent_info"}}
//! Agent -> Core    {"agent_info":{...}}          or {"runtime_probe":{...}}
//! ```
//!
//! Anything that goes wrong makes Agent answer `{"error":{...}}` and close:
//! a protocol version mismatch, a malformed or oversized frame, a frame out
//! of order, an unknown operation, or a journal it cannot write to. A peer
//! that is not Core is closed before a single byte is read or written.
//!
//! Every type denies unknown fields, and every string is a validated newtype
//! from [`crate::values`]. The request side is deliberately tiny: a version
//! number, a version string, a correlation id and one parameterless enum.

use serde::{Deserialize, Serialize};

use crate::values::{BootId, FileMode, ReportedPath, RequestId, Timestamp, Version};

// ---------------------------------------------------------------------------
// Core -> Agent

/// The first frame on every connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// Must equal [`crate::AGENT_PROTOCOL_VERSION`] exactly. There is no
    /// range, no minimum and no negotiation.
    pub protocol: u32,
    /// Core's build version, journaled by Agent next to the call.
    pub core_version: Version,
    /// Core's correlation id for this call; it appears in Core's audit row
    /// and in Agent's journal line.
    pub request_id: RequestId,
}

/// The M1 operation table — complete.
///
/// Two variants, and neither takes a parameter. That is the correct M1
/// surface, because M1 performs no privileged action. Adding a variant is a
/// security change (ADR-004): it needs a validated parameter type, a test
/// proving the parameter cannot escape its domain, and review. The
/// enumeration test below and the source gate in
/// `server/ci/boundary-checks.sh` both fail until the addition is deliberate.
///
/// On the wire an operation is its bare name and nothing else. The
/// deserializer is written by hand so that the one spelling is the only
/// spelling: serde's derived form would also accept `{"agent_info":null}`,
/// which carries nothing but is a second way to say the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOp {
    /// Agent's own identity and state. No parameters.
    AgentInfo,
    /// Read-only check for a container runtime socket at a compiled-in list
    /// of paths. No parameters: Core cannot name a socket.
    RuntimeProbe,
}

impl AgentOp {
    /// Every operation, in declaration order.
    pub const ALL: [AgentOp; 2] = [AgentOp::AgentInfo, AgentOp::RuntimeProbe];

    /// The wire and journal name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentInfo => "agent_info",
            Self::RuntimeProbe => "runtime_probe",
        }
    }
}

impl<'de> Deserialize<'de> for AgentOp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        AgentOp::ALL
            .into_iter()
            .find(|op| op.as_str() == name)
            .ok_or_else(|| serde::de::Error::custom("unknown operation"))
    }
}

/// The second frame: which operation to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    /// The operation.
    pub op: AgentOp,
}

/// Everything Core may send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CoreFrame {
    /// Opens the exchange.
    Hello(Hello),
    /// Asks for one operation, after `hello_ok`.
    Call(Call),
}

// ---------------------------------------------------------------------------
// Agent -> Core

/// Agent accepted the handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloOk {
    /// Agent's protocol version; equal to Core's, or there would be no
    /// `hello_ok`.
    pub protocol: u32,
    /// Agent's build version.
    pub agent_version: Version,
}

/// Why Agent refused. A closed set, stable on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// `hello.protocol` is not Agent's version. Terminal: Core marks the
    /// privileged capability unavailable and tries nothing else.
    ProtocolVersionMismatch,
    /// Not a frame this protocol defines: bad JSON, not UTF-8, a missing or
    /// unknown field, a wrong type.
    MalformedFrame,
    /// A length prefix of zero.
    EmptyFrame,
    /// A length prefix over [`crate::MAX_FRAME_BYTES`]; the body is not read.
    FrameTooLarge,
    /// `hello.request_id` is not 1 to 64 lowercase hex characters.
    InvalidRequestId,
    /// `hello.core_version` is not a version.
    InvalidCoreVersion,
    /// A valid frame in the wrong place, such as a `call` before `hello`.
    UnexpectedFrame,
    /// A `call` naming an operation this protocol does not define.
    UnknownOperation,
    /// Agent cannot write its journal, so it performs nothing and returns
    /// nothing it has not recorded.
    JournalUnavailable,
}

impl ErrorCode {
    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolVersionMismatch => "protocol_version_mismatch",
            Self::MalformedFrame => "malformed_frame",
            Self::EmptyFrame => "empty_frame",
            Self::FrameTooLarge => "frame_too_large",
            Self::InvalidRequestId => "invalid_request_id",
            Self::InvalidCoreVersion => "invalid_core_version",
            Self::UnexpectedFrame => "unexpected_frame",
            Self::UnknownOperation => "unknown_operation",
            Self::JournalUnavailable => "journal_unavailable",
        }
    }
}

/// Agent refused; the connection closes after this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentError {
    /// What was wrong.
    pub code: ErrorCode,
    /// Agent's protocol version, so a mismatch explains itself.
    pub agent_protocol: u32,
}

/// Agent's state directory as `lstat` sees it. A field is `null` when it
/// could not be read, never a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateDirInfo {
    /// The directory Agent is using.
    pub path: ReportedPath,
    /// Permission bits.
    pub mode: Option<FileMode>,
    /// Owner.
    pub uid: Option<u32>,
    /// Group.
    pub gid: Option<u32>,
}

/// Agent's journal, as Agent sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalInfo {
    /// Whether the last attempt to open or append succeeded.
    pub writable: bool,
    /// The sequence number of the last line written, `0` when none has been.
    pub last_seq: u64,
}

/// The answer to [`AgentOp::AgentInfo`]. Every value is read from the
/// running process or the kernel; one that cannot be read is `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInfo {
    /// Agent's build version.
    pub agent_version: Version,
    /// Agent's protocol version.
    pub protocol: u32,
    /// Agent's effective uid.
    pub uid: u32,
    /// Agent's process id.
    pub pid: u32,
    /// When this Agent process started.
    pub started_at: Timestamp,
    /// `/proc/sys/kernel/random/boot_id`. `null` under the production unit,
    /// whose `ProcSubset=pid` hides `/proc/sys`; Core reads the boot id
    /// itself (M1F).
    pub boot_id: Option<BootId>,
    /// The state directory.
    pub state_dir: StateDirInfo,
    /// The journal.
    pub journal: JournalInfo,
    /// `CapBnd` in `/proc/self/status` is zero.
    pub capability_bounding_set_empty: Option<bool>,
    /// `NoNewPrivs` in `/proc/self/status` is one.
    pub no_new_privileges: Option<bool>,
}

/// A container runtime Agent knows how to recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// Docker Engine.
    Docker,
    /// Podman.
    Podman,
}

impl Runtime {
    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

/// One of the compiled-in candidate runtime sockets, by name.
///
/// The paths themselves live in Agent's probe module and nowhere else. This
/// crate, which Core links, names candidates without knowing where they are,
/// so Core carries no runtime socket path in any form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSocket {
    /// Docker's socket under `/var/run`.
    DockerVarRun,
    /// Docker's socket under `/run`.
    DockerRun,
    /// Podman's rootful socket.
    PodmanRun,
}

impl RuntimeSocket {
    /// Every candidate, in the order Agent probes them.
    pub const ALL: [RuntimeSocket; 3] = [
        RuntimeSocket::DockerVarRun,
        RuntimeSocket::DockerRun,
        RuntimeSocket::PodmanRun,
    ];

    /// Which runtime a candidate belongs to.
    #[must_use]
    pub fn runtime(self) -> Runtime {
        match self {
            Self::DockerVarRun | Self::DockerRun => Runtime::Docker,
            Self::PodmanRun => Runtime::Podman,
        }
    }

    /// The wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DockerVarRun => "docker_var_run",
            Self::DockerRun => "docker_run",
            Self::PodmanRun => "podman_run",
        }
    }
}

/// Why `version` is `null`. M1 never queries a runtime's API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionReason {
    /// Plan section 3.4: M1 sends no byte to a runtime.
    RuntimeVersionProbeNotInM1,
}

/// Why `liveness` is `null`. The M1 probe is passive: it never connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LivenessReason {
    /// The probe inspects filesystem metadata only. Connecting could wake a
    /// socket-activated runtime, so M1 never does, and therefore cannot say
    /// whether a runtime is running, healthy or able to answer.
    PassiveProbeInM1,
}

/// A value that is always `null` on the wire: M1 does not know it and does
/// not pretend to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotProbed;

/// The answer to [`AgentOp::RuntimeProbe`]: which known runtime sockets
/// **exist**. Presence only — never reachability, health or version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbe {
    /// The runtime of the selected candidate; `null` when no candidate
    /// exists as a socket.
    pub runtime: Option<Runtime>,
    /// The selected candidate: the first, in probe order, that exists as a
    /// socket.
    pub socket: Option<RuntimeSocket>,
    /// Other candidates that also exist as distinct sockets, in probe order.
    /// Two paths that name the same socket (such as `/var/run` linked to
    /// `/run`) count once.
    pub also_present: Vec<RuntimeSocket>,
    /// Always `null` in M1: nothing is known about whether the runtime is
    /// running or would answer.
    pub liveness: NotProbed,
    /// Why `liveness` is `null`.
    pub liveness_reason: LivenessReason,
    /// Always `null` in M1.
    pub version: NotProbed,
    /// Why `version` is `null`.
    pub version_reason: VersionReason,
}

/// Everything Agent may send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum AgentFrame {
    /// Handshake accepted.
    HelloOk(HelloOk),
    /// Refused; the connection closes.
    Error(AgentError),
    /// Result of [`AgentOp::AgentInfo`].
    AgentInfo(AgentInfo),
    /// Result of [`AgentOp::RuntimeProbe`].
    RuntimeProbe(RuntimeProbe),
}
