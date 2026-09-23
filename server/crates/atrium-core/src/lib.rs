//! Atrium Core — the unprivileged server component.
//!
//! As of M1B, Core starts, verifies its persistent identity and state, and
//! either runs normally or enters recovery mode:
//!
//! - **identity** ([`identity`]) — `server_id`, the device identity key and
//!   `secrets.key`, created once at installation by `atrium-core
//!   init-identity` ([`init`]) and only ever *read* by the running service;
//! - **certificate** ([`certificate`]) — public, reissued from the same key
//!   when addresses change or expiry nears, so a client's pin never moves;
//! - **state** ([`db`]) — the SQLite database, integrity-checked at every
//!   start and migrated only behind a verified backup;
//! - **recovery** ([`recovery`]) — when any of that cannot be trusted: stay
//!   up, report, change nothing.
//!
//! - **agent** ([`agentclient`], [`agentmonitor`], [`capability`]) — in
//!   normal mode only, Core asks Agent `AgentInfo` and `RuntimeProbe` over
//!   the typed Unix-socket protocol, audits every call, and derives the
//!   `privileged` and `container` capabilities from the answers. When Agent
//!   is unreachable they are unavailable, with a reason, and nothing else is
//!   tried.
//!
//! What it deliberately does **not** do yet: it opens no TCP port, serves no
//! HTTP, pairs no device and reports no system metrics.
//! Each of those arrives in its own pass with its own tests — see
//! `docs/M1-IMPLEMENTATION-PLAN.md` section 17. There is no placeholder
//! endpoint and no invented data anywhere in this crate.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

pub mod agentclient;
pub mod agentmonitor;
pub mod capability;
pub mod certificate;
pub mod config;
pub mod db;
pub mod fsio;
pub mod guard;
pub mod identity;
pub mod init;
pub mod layout;
pub mod lock;
pub mod notify;
pub mod protect;
pub mod recovery;
pub mod redact;
pub mod rotate;
pub mod signals;
pub mod startup;

#[cfg(test)]
mod testutil;

use std::error::Error;
use std::time::Duration;

use time::OffsetDateTime;

use crate::certificate::AddressSet;
use crate::config::{Config, ConfigError, LogLevel};
use crate::layout::{Layout, LayoutError};
use crate::startup::Mode;

/// Build version, reported at startup and in diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable selecting the log filter, in `tracing` syntax. It
/// overrides `log_level` in `core.toml`.
pub const LOG_FILTER_ENV: &str = "ATRIUM_CORE_LOG";

/// How often the interface set is checked for address changes. Plan section
/// 5.3: adequate for M1; netlink monitoring is a later optimisation.
pub const ADDRESS_POLL: Duration = Duration::from_secs(30);

/// Installs the structured logger.
///
/// JSON to stderr, so journald captures it verbatim and a diagnostics bundle
/// can read it back without a parser of its own.
pub fn init_logging(level: Option<LogLevel>) {
    let default = level.unwrap_or(LogLevel::Info).as_str();
    let filter = tracing_subscriber::EnvFilter::try_from_env(LOG_FILTER_ENV)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(std::io::stderr)
        .init();
}

/// The machine's addresses, or none when they cannot be read. An empty set is
/// safe: the certificate still names `localhost` and both loopbacks, and the
/// next poll reissues it once addresses can be read.
#[must_use]
pub fn current_addresses() -> AddressSet {
    AddressSet::current().unwrap_or_else(|error| {
        tracing::warn!(
            event = "addresses_unavailable",
            component = "atrium-core",
            reason = %error,
            "could not list interface addresses"
        );
        AddressSet::default()
    })
}

/// Runs Core until a shutdown signal arrives.
///
/// `layout` and `config` are resolved by the caller before logging starts,
/// because the configuration chooses the log level.
///
/// # Errors
///
/// Returns an error when a startup guard refuses — running as root, an
/// invalid layout override or configuration, or another process holding the
/// state directory. The caller turns that into a non-zero exit so systemd
/// reports a failure rather than a clean stop. A broken identity or database
/// is **not** an error: it is recovery mode, which runs until stopped, so that
/// systemd does not restart Core into the same failure in a loop.
pub async fn run(
    layout: Result<Layout, LayoutError>,
    config: Option<Result<Config, ConfigError>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let process = ProcessIdentity::current();

    guard::deny_root(process.effective_uid)?;

    tracing::info!(
        event = "starting",
        component = "atrium-core",
        version = VERSION,
        pid = process.pid,
        uid = process.real_uid,
        euid = process.effective_uid,
        gid = process.real_gid,
        egid = process.effective_gid,
        "atrium-core is starting"
    );

    let layout = layout?;
    if let Some(root) = layout.root() {
        tracing::warn!(
            event = "layout_relocated",
            component = "atrium-core",
            root = %root.display(),
            "the file tree is relocated by {}; this is for tests and development",
            layout::ROOT_ENV
        );
    }
    let config = config.ok_or("configuration was not loaded")??;

    // Before readiness, never after: a stop request that arrives between
    // READY=1 and the handler being installed would hit the default
    // disposition and kill the process uncleanly.
    let mut shutdown = signals::listen()?;

    // Held for the life of the process, in either mode.
    let now = OffsetDateTime::now_utc();
    let (lock, mode) = match lock::acquire(layout.state_dir()) {
        Ok(lock) => (
            Some(lock),
            startup::prepare(&layout, now, current_addresses),
        ),
        Err(lock::LockError::Busy) => return Err(Box::new(lock::LockError::Busy)),
        Err(error) => {
            // No lock means no safe way to write state, and the state checks
            // would fail for the same reason; report it directly.
            tracing::error!(
                event = "state_lock_unavailable",
                component = "atrium-core",
                reason = %error,
                "could not lock the state directory"
            );
            let missing = matches!(
                &error,
                lock::LockError::Io(io) if io.kind() == std::io::ErrorKind::NotFound
            );
            let reason = if missing {
                recovery::RecoveryReason::StateMissing
            } else {
                recovery::RecoveryReason::StateUnprotected
            };
            (
                None,
                Mode::Recovery(recovery::Recovery {
                    reason,
                    since: now,
                    schema_version: None,
                    backup_count: 0,
                }),
            )
        }
    };

    let signal = match mode {
        Mode::Normal(mut normal) => {
            let notified = notify::ready_with_status("running")?;
            tracing::info!(
                event = "ready",
                component = "atrium-core",
                version = VERSION,
                mode = "normal",
                server_name = %config.server_name,
                server_id = %normal.identity.server_id(),
                spki_sha256 = %normal.identity.pin(),
                schema_version = normal.database.schema_version().unwrap_or(0),
                certificate_serial = %normal.certificate.serial,
                certificate_not_after = %identity::format_time(normal.certificate.not_after),
                service_manager_notified = notified,
                "atrium-core is ready"
            );
            run_normal(&layout, &mut normal, &mut shutdown).await
        }
        Mode::Recovery(recovery) => {
            let health = recovery.health();
            let notified = notify::ready_with_status(&format!(
                "recovery mode: {}; see `atriumctl diagnostics`",
                health.reason
            ))?;
            tracing::info!(
                event = "ready",
                component = "atrium-core",
                version = VERSION,
                mode = "recovery",
                reason = health.reason,
                diagnostics = %serde_json::to_string(&recovery.diagnostics()).unwrap_or_default(),
                service_manager_notified = notified,
                "atrium-core is ready in recovery mode"
            );
            // Recovery does nothing but wait. It holds no database handle, no
            // agent client and no timer; there is nothing here to act on.
            shutdown.recv().await
        }
    };

    tracing::info!(
        event = "stopping",
        component = "atrium-core",
        signal = signal.as_str(),
        "atrium-core is shutting down"
    );

    notify::stopping()?;
    drop(lock);

    tracing::info!(
        event = "stopped",
        component = "atrium-core",
        "atrium-core stopped cleanly"
    );

    Ok(())
}

/// Normal mode until shutdown: re-checks the certificate against the current
/// addresses every [`ADDRESS_POLL`], and refreshes the Agent-derived
/// capabilities right away and then every [`agentmonitor::AGENT_REFRESH`].
async fn run_normal(
    layout: &Layout,
    normal: &mut startup::Normal,
    shutdown: &mut signals::ShutdownListener,
) -> signals::Shutdown {
    let mut ticker = tokio::time::interval(ADDRESS_POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick completes immediately; startup has just checked.
    ticker.tick().await;
    // The first agent tick also completes immediately: that is the check
    // right after startup.
    let mut agent_ticker = tokio::time::interval(agentmonitor::AGENT_REFRESH);
    agent_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut monitor = match agentmonitor::Monitor::new(layout) {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            tracing::error!(
                event = "agent_client_unavailable",
                component = "atrium-core",
                reason = %error,
                "no Agent client could be built; privileged and container capabilities                  stay unavailable"
            );
            None
        }
    };
    loop {
        tokio::select! {
            signal = shutdown.recv() => return signal,
            _ = agent_ticker.tick(), if monitor.is_some() => {
                if let Some(monitor) = monitor.as_mut() {
                    // A stalled Agent can hold a call for its full timeout;
                    // a stop request must not wait behind it. A call cut
                    // short here may be in Agent's journal without a
                    // matching audit row, which is the honest record of a
                    // call whose result Core never received.
                    tokio::select! {
                        signal = shutdown.recv() => return signal,
                        _ = monitor.refresh(&normal.database) => {}
                    }
                }
            }
            _ = ticker.tick() => {
                let now = OffsetDateTime::now_utc();
                let addresses = current_addresses();
                match certificate::ensure(layout, &normal.identity, &addresses, now) {
                    Ok(outcome) => {
                        normal.certificate = startup::settle(&normal.database, outcome, now);
                    }
                    Err(error) => tracing::error!(
                        event = "certificate_reissue_failed",
                        component = "atrium-core",
                        reason = %error,
                        "no usable certificate could be written; the previous one is kept \
                         and the next check retries"
                    ),
                }
            }
        }
    }
}

/// Who this process is, as the kernel sees it.
///
/// Logged at startup so "Core runs as `atrium`, not root" is visible in the
/// journal and not only in a unit file (criterion 5).
#[derive(Debug, Clone, Copy)]
pub struct ProcessIdentity {
    /// Process id.
    pub pid: u32,
    /// Real user id.
    pub real_uid: u32,
    /// Effective user id — the one the guard checks.
    pub effective_uid: u32,
    /// Real group id.
    pub real_gid: u32,
    /// Effective group id.
    pub effective_gid: u32,
}

impl ProcessIdentity {
    /// Reads the current process's identity.
    #[must_use]
    pub fn current() -> Self {
        Self {
            pid: std::process::id(),
            real_uid: nix::unistd::getuid().as_raw(),
            effective_uid: nix::unistd::geteuid().as_raw(),
            real_gid: nix::unistd::getgid().as_raw(),
            effective_gid: nix::unistd::getegid().as_raw(),
        }
    }
}
