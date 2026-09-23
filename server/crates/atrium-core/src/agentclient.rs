//! Core's side of the Core-to-Agent protocol.
//!
//! One connection per call, one call per connection: `hello`, `hello_ok`,
//! `call`, result. The client is as strict as Agent is. It validates the
//! length prefix before allocating, decodes with `deny_unknown_fields`, and
//! requires the answer to match the operation it asked for.
//!
//! **There is no fallback.** When Agent cannot be reached, refuses, or
//! answers something this protocol does not define, the call fails with a
//! named [`Failure`], and the caller marks the capability unavailable. No
//! code path in Core opens a runtime socket, runs a program, or tries any
//! other privileged mechanism — Core has none (ADR-001, ADR-013), and
//! `server/ci/boundary-checks.sh` keeps it that way.
//!
//! Every call is audited by the caller, whether it succeeded or not.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atrium_protocol::frame::{self, LengthError, HEADER_BYTES};
use atrium_protocol::messages::{
    AgentFrame, AgentInfo, AgentOp, Call, CoreFrame, ErrorCode, Hello, RuntimeProbe,
};
use atrium_protocol::values::{RequestId, Version};
use atrium_protocol::AGENT_PROTOCOL_VERSION;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::capability::Reason;

/// Connecting to the socket.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// The whole call, handshake to result. Longer than Agent's own deadline, so
/// Agent's refusal arrives before Core gives up.
const CALL_TIMEOUT: Duration = Duration::from_secs(8);

/// Why a call failed. A closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The socket does not exist: Agent's socket unit is stopped.
    SocketMissing,
    /// Nothing is listening on the socket.
    ConnectionRefused,
    /// The socket's permissions refused Core.
    PermissionDenied,
    /// Connecting or the exchange took too long.
    TimedOut,
    /// Agent closed the connection without answering.
    Closed,
    /// Agent speaks another protocol version. Terminal: no retry, no
    /// negotiation.
    ProtocolMismatch {
        /// Agent's version, as it reported it.
        agent_protocol: u32,
    },
    /// Agent cannot write its journal and performed nothing.
    JournalUnavailable,
    /// Agent refused with another code.
    Refused(ErrorCode),
    /// Agent's answer was not a frame this protocol defines, or not the
    /// answer to the operation asked.
    ProtocolError,
    /// The socket failed in another way.
    Io,
}

impl Failure {
    /// Stable cause, for the audit row and the log.
    #[must_use]
    pub fn cause(self) -> &'static str {
        match self {
            Self::SocketMissing => "socket_missing",
            Self::ConnectionRefused => "connection_refused",
            Self::PermissionDenied => "permission_denied",
            Self::TimedOut => "timed_out",
            Self::Closed => "closed_by_agent",
            Self::ProtocolMismatch { .. } => "protocol_version_mismatch",
            Self::JournalUnavailable => "journal_unavailable",
            Self::Refused(code) => code.as_str(),
            Self::ProtocolError => "protocol_error",
            Self::Io => "io_error",
        }
    }

    /// The capability reason this failure maps to.
    #[must_use]
    pub fn reason(self) -> Reason {
        match self {
            Self::SocketMissing
            | Self::ConnectionRefused
            | Self::PermissionDenied
            | Self::TimedOut
            | Self::Closed
            | Self::Io => Reason::AgentUnreachable,
            Self::ProtocolMismatch { .. } => Reason::AgentProtocolMismatch,
            Self::JournalUnavailable => Reason::AgentJournalUnavailable,
            Self::Refused(_) | Self::ProtocolError => Reason::AgentProtocolError,
        }
    }

    /// Whether Agent itself answered with a refusal, as opposed to not
    /// answering at all. The audit row says `refused` for these and
    /// `failed` for the rest.
    #[must_use]
    pub fn is_refusal(self) -> bool {
        matches!(
            self,
            Self::ProtocolMismatch { .. } | Self::JournalUnavailable | Self::Refused(_)
        )
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.cause())
    }
}

/// A fresh correlation id: 8 random bytes, 16 lowercase hex characters.
///
/// # Errors
///
/// When the OS random source fails; the call is not attempted.
pub fn new_request_id() -> Result<RequestId, Failure> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| Failure::Io)?;
    RequestId::parse(&hex::encode(bytes)).map_err(|_| Failure::Io)
}

/// A client for one socket.
#[derive(Debug, Clone)]
pub struct AgentClient {
    socket: PathBuf,
    core_version: Version,
}

impl AgentClient {
    /// A client for Agent's socket at `socket`.
    ///
    /// # Errors
    ///
    /// When `core_version` is not a protocol version string, which the
    /// crate's own version always is.
    pub fn new(socket: &Path, core_version: &str) -> Result<Self, Failure> {
        Ok(Self {
            socket: socket.to_path_buf(),
            core_version: Version::parse(core_version).map_err(|_| Failure::ProtocolError)?,
        })
    }

    /// `AgentInfo`.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub async fn agent_info(&self, request_id: &RequestId) -> Result<AgentInfo, Failure> {
        match self.call(AgentOp::AgentInfo, request_id).await? {
            AgentFrame::AgentInfo(info) if info.protocol == AGENT_PROTOCOL_VERSION => Ok(info),
            _ => Err(Failure::ProtocolError),
        }
    }

    /// `RuntimeProbe`.
    ///
    /// # Errors
    ///
    /// [`Failure`].
    pub async fn runtime_probe(&self, request_id: &RequestId) -> Result<RuntimeProbe, Failure> {
        match self.call(AgentOp::RuntimeProbe, request_id).await? {
            AgentFrame::RuntimeProbe(probe) => Ok(probe),
            _ => Err(Failure::ProtocolError),
        }
    }

    /// One exchange. Returns the result frame; an error frame becomes a
    /// [`Failure`].
    async fn call(&self, op: AgentOp, request_id: &RequestId) -> Result<AgentFrame, Failure> {
        let connect = tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&self.socket));
        let mut stream = match connect.await {
            Err(_) => return Err(Failure::TimedOut),
            Ok(Err(error)) => {
                return Err(match error.kind() {
                    ErrorKind::NotFound => Failure::SocketMissing,
                    ErrorKind::ConnectionRefused => Failure::ConnectionRefused,
                    ErrorKind::PermissionDenied => Failure::PermissionDenied,
                    _ => Failure::Io,
                })
            }
            Ok(Ok(stream)) => stream,
        };
        tokio::time::timeout(CALL_TIMEOUT, self.exchange(&mut stream, op, request_id))
            .await
            .unwrap_or(Err(Failure::TimedOut))
    }

    async fn exchange(
        &self,
        stream: &mut UnixStream,
        op: AgentOp,
        request_id: &RequestId,
    ) -> Result<AgentFrame, Failure> {
        let hello = CoreFrame::Hello(Hello {
            protocol: AGENT_PROTOCOL_VERSION,
            core_version: self.core_version.clone(),
            request_id: request_id.clone(),
        });
        send(stream, &hello).await?;
        match receive(stream).await? {
            AgentFrame::HelloOk(accepted) if accepted.protocol == AGENT_PROTOCOL_VERSION => {}
            AgentFrame::HelloOk(accepted) => {
                return Err(Failure::ProtocolMismatch {
                    agent_protocol: accepted.protocol,
                })
            }
            AgentFrame::Error(error) => return Err(refusal(error.code, error.agent_protocol)),
            _ => return Err(Failure::ProtocolError),
        }
        send(stream, &CoreFrame::Call(Call { op })).await?;
        match receive(stream).await? {
            AgentFrame::Error(error) => Err(refusal(error.code, error.agent_protocol)),
            AgentFrame::HelloOk(_) => Err(Failure::ProtocolError),
            result => Ok(result),
        }
    }
}

fn refusal(code: ErrorCode, agent_protocol: u32) -> Failure {
    match code {
        ErrorCode::ProtocolVersionMismatch => Failure::ProtocolMismatch { agent_protocol },
        ErrorCode::JournalUnavailable => Failure::JournalUnavailable,
        other => Failure::Refused(other),
    }
}

async fn send(stream: &mut UnixStream, frame: &CoreFrame) -> Result<(), Failure> {
    let bytes = frame::encode(frame).map_err(|_| Failure::ProtocolError)?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| Failure::Closed)?;
    stream.flush().await.map_err(|_| Failure::Closed)
}

async fn read_exact(stream: &mut UnixStream, buffer: &mut [u8]) -> Result<(), Failure> {
    match stream.read_exact(buffer).await {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => Err(Failure::Closed),
        Err(_) => Err(Failure::Io),
    }
}

/// Reads one frame from Agent, bounded exactly as Agent bounds Core's.
async fn receive(stream: &mut UnixStream) -> Result<AgentFrame, Failure> {
    let mut header = [0_u8; HEADER_BYTES];
    read_exact(stream, &mut header).await?;
    let length = frame::body_length(header).map_err(|error| match error {
        LengthError::Empty | LengthError::TooLarge { .. } => Failure::ProtocolError,
    })?;
    let mut body = vec![0_u8; length];
    read_exact(stream, &mut body).await?;
    frame::decode_agent_frame(&body).map_err(|_| Failure::ProtocolError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_sixteen_hex_characters_and_differ() {
        let first = new_request_id().expect("random");
        let second = new_request_id().expect("random");
        assert_eq!(first.as_str().len(), 16);
        assert_ne!(first, second);
    }

    #[test]
    fn failures_map_to_the_planned_reasons() {
        for unreachable in [
            Failure::SocketMissing,
            Failure::ConnectionRefused,
            Failure::PermissionDenied,
            Failure::TimedOut,
            Failure::Closed,
            Failure::Io,
        ] {
            assert_eq!(unreachable.reason().code(), "agent_unreachable");
            assert!(!unreachable.is_refusal());
        }
        let mismatch = Failure::ProtocolMismatch { agent_protocol: 2 };
        assert_eq!(mismatch.reason().code(), "agent_protocol_mismatch");
        assert!(mismatch.is_refusal());
        assert_eq!(
            Failure::JournalUnavailable.reason().code(),
            "agent_journal_unavailable"
        );
    }

    #[tokio::test]
    async fn a_missing_socket_is_named_and_nothing_else_is_tried() {
        let client = AgentClient::new(
            Path::new("/nonexistent/atrium-test/agent.sock"),
            crate::VERSION,
        )
        .expect("client");
        let id = new_request_id().expect("id");
        assert_eq!(client.agent_info(&id).await, Err(Failure::SocketMissing));
    }
}
