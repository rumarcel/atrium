//! Keeps the Agent-derived capabilities current, and audits every call.
//!
//! In normal mode Core asks Agent `AgentInfo` and, if that succeeded,
//! `RuntimeProbe`, once right after startup and then every
//! [`AGENT_REFRESH`]. Each call gets a fresh `request_id`, which appears in
//! Core's `agent.call` audit row and in Agent's journal line, so the two
//! records can be correlated (criterion 33).
//!
//! When `AgentInfo` fails, `RuntimeProbe` is not attempted: the container
//! capability takes the same reason. Nothing is retried in a loop, nothing
//! falls back, and the log carries one line per *change* of state rather than
//! one per refresh (plan section 16).
//!
//! Recovery mode never constructs a monitor, so it never contacts Agent.

use std::future::Future;
use std::time::Duration;

use atrium_protocol::messages::AgentOp;
use atrium_protocol::values::RequestId;
use time::OffsetDateTime;

use crate::agentclient::{self, AgentClient, Failure};
use crate::capability::AgentCapabilities;
use crate::db::{AgentCallOutcome, Database};
use crate::layout::Layout;

/// How often the capabilities are refreshed after the first time.
///
/// Every call writes an audit row, so this is deliberately slow; the M1F
/// capabilities endpoint may refresh on demand, under its own rate limit.
pub const AGENT_REFRESH: Duration = Duration::from_secs(300);

/// The monitor.
#[derive(Debug)]
pub struct Monitor {
    client: AgentClient,
    last: Option<AgentCapabilities>,
}

impl Monitor {
    /// A monitor for the Agent socket in `layout`.
    ///
    /// # Errors
    ///
    /// Only when Core's own version is not a valid protocol version string.
    pub fn new(layout: &Layout) -> Result<Self, Failure> {
        Ok(Self {
            client: AgentClient::new(layout.agent_socket(), crate::VERSION)?,
            last: None,
        })
    }

    /// The capabilities as of the last refresh.
    #[must_use]
    pub fn current(&self) -> Option<&AgentCapabilities> {
        self.last.as_ref()
    }

    /// Asks Agent again, audits each call, and logs a change of state.
    pub async fn refresh(&mut self, database: &Database) -> &AgentCapabilities {
        let info = audited(database, AgentOp::AgentInfo, |id| {
            let client = self.client.clone();
            async move { client.agent_info(&id).await }
        })
        .await;
        let probe = match &info {
            Ok(_) => Some(
                audited(database, AgentOp::RuntimeProbe, |id| {
                    let client = self.client.clone();
                    async move { client.runtime_probe(&id).await }
                })
                .await,
            ),
            Err(_) => None,
        };
        let capabilities = AgentCapabilities::derive(&info, probe.as_ref());

        if self.last.as_ref() != Some(&capabilities) {
            let (privileged_reason, container_reason) = capabilities.summary();
            let cause = info
                .as_ref()
                .err()
                .or_else(|| probe.as_ref().and_then(|p| p.as_ref().err()))
                .map(|failure| failure.cause());
            let privileged_version = capabilities
                .privileged
                .agent_version
                .as_ref()
                .map(ToString::to_string);
            let provider = capabilities.container.provider.map(|p| p.as_str());
            if capabilities.privileged.available {
                tracing::info!(
                    event = "agent_status",
                    component = "atrium-core",
                    privileged_available = true,
                    agent_version = privileged_version,
                    agent_protocol = capabilities.privileged.protocol,
                    container_available = capabilities.container.available,
                    container_via = capabilities.container.via(),
                    container_provider = provider,
                    container_reason = container_reason.map(|r| r.code()),
                    cause = cause,
                    "the Agent boundary is up"
                );
            } else {
                tracing::warn!(
                    event = "agent_status",
                    component = "atrium-core",
                    privileged_available = false,
                    privileged_reason = privileged_reason.map(|r| r.code()),
                    container_available = false,
                    container_via = capabilities.container.via(),
                    container_reason = container_reason.map(|r| r.code()),
                    cause = cause,
                    "Agent is unavailable; privileged and container capabilities are off, \
                     every read keeps working, and nothing else is tried"
                );
            }
        }
        self.last.insert(capabilities)
    }
}

/// Runs one call with a fresh id and writes its audit row.
async fn audited<T, F, Fut>(database: &Database, op: AgentOp, call: F) -> Result<T, Failure>
where
    F: FnOnce(RequestId) -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    let id = agentclient::new_request_id()?;
    let result = call(id.clone()).await;
    let (outcome, code) = match &result {
        Ok(_) => (AgentCallOutcome::Ok, None),
        Err(failure) if failure.is_refusal() => (AgentCallOutcome::Refused, Some(failure.cause())),
        Err(failure) => (AgentCallOutcome::Failed, Some(failure.cause())),
    };
    tracing::debug!(
        event = "agent_call",
        component = "atrium-core",
        request_id = %id,
        op = op.as_str(),
        outcome = outcome.as_str(),
        cause = code,
        "called Agent"
    );
    if let Err(error) = database.audit_agent_call(&id, op, outcome, code, OffsetDateTime::now_utc())
    {
        tracing::warn!(
            event = "audit_write_failed",
            component = "atrium-core",
            action = "agent.call",
            request_id = %id,
            reason = %error,
            "could not record the Agent call in the audit log"
        );
    }
    result
}
