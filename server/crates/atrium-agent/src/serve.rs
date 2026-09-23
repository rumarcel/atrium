//! One connection, start to finish.
//!
//! ```text
//! accept
//!   rate limit        over the limit: close unread, count it, summarise later
//!   SO_PEERCRED       not Core's uid: journal, close — nothing read, nothing sent
//!   read hello        length checked before allocation; strict decode
//!   version           not Agent's: journal, answer protocol_version_mismatch, close
//!   send hello_ok
//!   read call         strict decode; exactly one of the two M1 operations
//!   perform           AgentInfo | RuntimeProbe — both read-only
//!   journal           cannot journal: answer journal_unavailable, nothing else
//!   send the result, close
//! ```
//!
//! The whole exchange has one deadline. Connections are served one at a time:
//! Core makes a handful of calls a minute, and serial handling keeps the
//! journal's order and Agent's reasoning simple. Only Core's uid ever gets
//! past `SO_PEERCRED`, so a peer that holds a connection open can delay only
//! Core.
//!
//! Nothing here falls back to anything. A refusal is final for the connection,
//! and there is no second mechanism to try.

use std::time::{Duration, Instant, SystemTime};

use atrium_protocol::frame::{self, DecodeError, LengthError, HEADER_BYTES};
use atrium_protocol::messages::{AgentError, AgentFrame, AgentOp, CoreFrame, ErrorCode, HelloOk};
use atrium_protocol::values::{RequestId, Version};
use atrium_protocol::AGENT_PROTOCOL_VERSION;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::clock;
use crate::info::{self, Startup};
use crate::journal::{Journal, Outcome, Peer, Reason, Record};
use crate::peer::PeerPolicy;
use crate::probe;

/// The whole exchange, from the first byte read to the result sent.
pub const EXCHANGE_DEADLINE: Duration = Duration::from_secs(5);
/// Sending one error frame on the way out.
const REFUSAL_WRITE_DEADLINE: Duration = Duration::from_secs(1);

/// Connections admitted in a burst before the limit applies.
pub const RATE_BURST: u32 = 32;
/// Connections admitted per second, sustained.
pub const RATE_PER_SECOND: u32 = 8;

/// A token bucket over all connections.
///
/// Every connection costs a token, whoever the peer is. Only Core's uid and
/// root can reach the socket at all, so this bounds how fast a compromised
/// Core can make Agent write its journal, which bounds how quickly it can
/// push old lines out through rotation. It does not remove that possibility;
/// see `docs/M1-IMPLEMENTATION-PLAN.md`, M1C as built.
#[derive(Debug)]
pub struct Limiter {
    tokens: f64,
    last: Instant,
}

impl Limiter {
    /// A full bucket.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            tokens: f64::from(RATE_BURST),
            last: now,
        }
    }

    /// Takes a token if one is available.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens =
            (self.tokens + elapsed * f64::from(RATE_PER_SECOND)).min(f64::from(RATE_BURST));
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// What the exchange decided.
enum Step {
    /// Refuse, journal `reason`, and answer `reply` if there is anyone left
    /// to answer.
    Refuse {
        reason: Reason,
        reply: Option<ErrorCode>,
    },
    /// Perform this operation.
    Perform(AgentOp),
}

impl Step {
    fn refuse(reason: Reason, reply: Option<ErrorCode>) -> Self {
        Self::Refuse { reason, reply }
    }
}

/// What the exchange learned, kept outside the deadline so a timeout still
/// journals what was known by then. Every field is a validated type.
#[derive(Default)]
struct Learned {
    request_id: Option<RequestId>,
    core_version: Option<Version>,
    found_protocol: Option<u32>,
}

/// Why a frame could not be read.
enum ReadError {
    /// The peer closed before sending a byte of this frame.
    Closed,
    /// The peer closed partway through.
    Truncated,
    /// A zero length prefix.
    Empty,
    /// A length prefix over the limit.
    TooLarge,
    /// The socket failed.
    Io,
}

async fn read_exact_or(stream: &mut UnixStream, buffer: &mut [u8]) -> Result<(), ReadError> {
    let mut filled = 0;
    while filled < buffer.len() {
        match stream.read(&mut buffer[filled..]).await {
            Ok(0) if filled == 0 => return Err(ReadError::Closed),
            Ok(0) => return Err(ReadError::Truncated),
            Ok(read) => filled += read,
            Err(_) => return Err(ReadError::Io),
        }
    }
    Ok(())
}

/// Reads one frame. The length is validated on the four header bytes and
/// only then is exactly that much memory allocated.
async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, ReadError> {
    let mut header = [0_u8; HEADER_BYTES];
    read_exact_or(stream, &mut header).await?;
    let length = frame::body_length(header).map_err(|error| match error {
        LengthError::Empty => ReadError::Empty,
        LengthError::TooLarge { .. } => ReadError::TooLarge,
    })?;
    let mut body = vec![0_u8; length];
    read_exact_or(stream, &mut body)
        .await
        .map_err(|error| match error {
            ReadError::Closed => ReadError::Truncated,
            other => other,
        })?;
    Ok(body)
}

async fn send(stream: &mut UnixStream, reply: &AgentFrame) -> std::io::Result<()> {
    let bytes = frame::encode(reply).map_err(std::io::Error::other)?;
    stream.write_all(&bytes).await?;
    stream.flush().await
}

fn refusal(error: ErrorCode) -> AgentFrame {
    AgentFrame::Error(AgentError {
        code: error,
        agent_protocol: AGENT_PROTOCOL_VERSION,
    })
}

fn refuse_read(error: ReadError) -> Step {
    match error {
        ReadError::Closed => Step::refuse(Reason::NoRequest, None),
        ReadError::Truncated => Step::refuse(Reason::TruncatedFrame, None),
        ReadError::Empty => Step::refuse(Reason::EmptyFrame, Some(ErrorCode::EmptyFrame)),
        ReadError::TooLarge => Step::refuse(Reason::FrameTooLarge, Some(ErrorCode::FrameTooLarge)),
        ReadError::Io => Step::refuse(Reason::PeerDisconnected, None),
    }
}

fn refuse_decode(error: DecodeError, learned: &mut Learned) -> Step {
    match error {
        DecodeError::Malformed => {
            Step::refuse(Reason::MalformedFrame, Some(ErrorCode::MalformedFrame))
        }
        DecodeError::ProtocolMismatch { found } => {
            learned.found_protocol = Some(found);
            Step::refuse(
                Reason::ProtocolVersionMismatch,
                Some(ErrorCode::ProtocolVersionMismatch),
            )
        }
        DecodeError::InvalidRequestId => {
            Step::refuse(Reason::InvalidRequestId, Some(ErrorCode::InvalidRequestId))
        }
        DecodeError::InvalidCoreVersion => Step::refuse(
            Reason::InvalidCoreVersion,
            Some(ErrorCode::InvalidCoreVersion),
        ),
        DecodeError::UnknownOperation => {
            Step::refuse(Reason::UnknownOperation, Some(ErrorCode::UnknownOperation))
        }
    }
}

fn unexpected() -> Step {
    Step::refuse(Reason::UnexpectedFrame, Some(ErrorCode::UnexpectedFrame))
}

/// Handshake and request. Returns what to do; performs nothing.
async fn negotiate(stream: &mut UnixStream, learned: &mut Learned, version: &Version) -> Step {
    let body = match read_frame(stream).await {
        Ok(body) => body,
        Err(error) => return refuse_read(error),
    };
    let hello = match frame::decode_core_frame(&body) {
        Ok(CoreFrame::Hello(hello)) => hello,
        Ok(CoreFrame::Call(_)) => return unexpected(),
        Err(error) => return refuse_decode(error, learned),
    };
    // Both values are validated types by now, so they are safe to journal
    // even when the version is refused.
    learned.request_id = Some(hello.request_id);
    learned.core_version = Some(hello.core_version);
    if hello.protocol != AGENT_PROTOCOL_VERSION {
        learned.found_protocol = Some(hello.protocol);
        return Step::refuse(
            Reason::ProtocolVersionMismatch,
            Some(ErrorCode::ProtocolVersionMismatch),
        );
    }

    let accepted = AgentFrame::HelloOk(HelloOk {
        protocol: AGENT_PROTOCOL_VERSION,
        agent_version: version.clone(),
    });
    if send(stream, &accepted).await.is_err() {
        return Step::refuse(Reason::PeerDisconnected, None);
    }

    let body = match read_frame(stream).await {
        Ok(body) => body,
        Err(error) => return refuse_read(error),
    };
    match frame::decode_core_frame(&body) {
        Ok(CoreFrame::Call(call)) => Step::Perform(call.op),
        Ok(CoreFrame::Hello(_)) => unexpected(),
        Err(error) => refuse_decode(error, learned),
    }
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Agent's serving state.
pub struct Server {
    policy: PeerPolicy,
    journal: Journal,
    startup: Startup,
    limiter: Limiter,
    suppressed: u64,
}

impl Server {
    /// A server that answers `policy`'s uid and journals into `journal`.
    #[must_use]
    pub fn new(policy: PeerPolicy, journal: Journal, startup: Startup) -> Self {
        Self {
            policy,
            journal,
            startup,
            limiter: Limiter::new(Instant::now()),
            suppressed: 0,
        }
    }

    /// Opens the journal, reporting a problem at startup rather than at the
    /// first call. A failure is not fatal: every call retries, and until one
    /// succeeds Agent performs nothing.
    pub fn open_journal(&mut self) {
        match self.journal.open(SystemTime::now()) {
            Ok(()) => tracing::info!(
                event = "journal_open",
                component = "atrium-agent",
                last_seq = self.journal.last_seq(),
                "the journal is open"
            ),
            Err(error) => tracing::error!(
                event = "journal_unavailable",
                component = "atrium-agent",
                reason = %error,
                "the journal cannot be written; every operation will be refused until it can"
            ),
        }
    }

    fn journal(&mut self, record: &Record) -> bool {
        match self.journal.append(record, SystemTime::now()) {
            Ok(_) => true,
            Err(error) => {
                tracing::error!(
                    event = "journal_write_failed",
                    component = "atrium-agent",
                    reason = %error,
                    "could not write the journal"
                );
                false
            }
        }
    }

    /// Writes a summary line for connections closed by the rate limit.
    pub fn flush_suppressed(&mut self) {
        if self.suppressed == 0 {
            return;
        }
        let mut record = Record::refused(
            clock::timestamp(SystemTime::now()),
            None,
            Reason::RateLimited,
        );
        record.suppressed = Some(self.suppressed);
        if self.journal(&record) {
            self.suppressed = 0;
        }
    }

    /// Serves one connection.
    pub async fn handle(&mut self, mut stream: UnixStream) {
        let began = Instant::now();
        if !self.limiter.take(began) {
            self.suppressed = self.suppressed.saturating_add(1);
            return;
        }
        self.flush_suppressed();
        let now = || clock::timestamp(SystemTime::now());

        let peer = match stream.peer_cred() {
            Ok(credentials) => Peer {
                uid: credentials.uid(),
                gid: credentials.gid(),
                pid: credentials.pid(),
            },
            Err(_) => {
                self.journal(&Record::refused(
                    now(),
                    None,
                    Reason::PeerCredentialsUnavailable,
                ));
                return;
            }
        };
        if !self.policy.admits(peer.uid) {
            tracing::warn!(
                event = "peer_rejected",
                component = "atrium-agent",
                peer_uid = peer.uid,
                peer_gid = peer.gid,
                peer_pid = peer.pid,
                "refused a connection from a uid that is not Core's"
            );
            self.journal(&Record::refused(now(), Some(peer), Reason::PeerUidRejected));
            return;
        }

        let mut learned = Learned::default();
        let version = self.startup.version.clone();
        let step = tokio::time::timeout(
            EXCHANGE_DEADLINE,
            negotiate(&mut stream, &mut learned, &version),
        )
        .await
        .unwrap_or(Step::refuse(Reason::TimedOut, None));

        let mut record = Record::refused(now(), Some(peer), Reason::MalformedFrame);
        record.request_id = learned.request_id;
        record.core_version = learned.core_version;
        record.found_protocol = learned.found_protocol;

        match step {
            Step::Refuse { reason, reply } => {
                record.reason = Some(reason);
                record.duration_ms = elapsed_ms(began);
                tracing::warn!(
                    event = "request_refused",
                    component = "atrium-agent",
                    peer_uid = peer.uid,
                    reason = ?reason,
                    "refused a request"
                );
                self.journal(&record);
                if let Some(code) = reply {
                    let _ = tokio::time::timeout(
                        REFUSAL_WRITE_DEADLINE,
                        send(&mut stream, &refusal(code)),
                    )
                    .await;
                }
            }
            Step::Perform(op) => {
                let result = match op {
                    AgentOp::AgentInfo => {
                        AgentFrame::AgentInfo(info::collect(&self.startup, &self.journal))
                    }
                    AgentOp::RuntimeProbe => AgentFrame::RuntimeProbe(probe::probe().await),
                };
                record.accepted = true;
                record.op = Some(op);
                record.outcome = Outcome::Ok;
                record.reason = None;
                record.duration_ms = elapsed_ms(began);
                // The result is sent only once it is on disk: Core never
                // holds an answer Agent has not recorded.
                let reply = if self.journal(&record) {
                    result
                } else {
                    refusal(ErrorCode::JournalUnavailable)
                };
                let _ =
                    tokio::time::timeout(REFUSAL_WRITE_DEADLINE, send(&mut stream, &reply)).await;
            }
        }
        let _ = stream.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limiter_admits_a_burst_then_the_sustained_rate() {
        let start = Instant::now();
        let mut limiter = Limiter::new(start);
        for _ in 0..RATE_BURST {
            assert!(limiter.take(start));
        }
        assert!(!limiter.take(start), "the burst is spent");
        let later = start + Duration::from_millis(1000 / u64::from(RATE_PER_SECOND) + 1);
        assert!(limiter.take(later), "one token refills");
        assert!(!limiter.take(later));
    }

    #[test]
    fn the_limiter_never_exceeds_its_burst() {
        let start = Instant::now();
        let mut limiter = Limiter::new(start);
        let much_later = start + Duration::from_secs(3600);
        let mut admitted = 0;
        while limiter.take(much_later) {
            admitted += 1;
        }
        assert_eq!(admitted, RATE_BURST);
    }
}
