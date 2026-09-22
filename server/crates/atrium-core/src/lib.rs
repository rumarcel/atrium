//! Atrium Core — M1A lifecycle skeleton.
//!
//! What this binary does today: start deterministically, refuse to run as root,
//! log who and what it is, signal readiness to systemd, wait, and shut down
//! cleanly on `SIGTERM`.
//!
//! What it deliberately does **not** do, so that nobody mistakes a skeleton for
//! a product: it opens no TCP port, serves no HTTP, reads no configuration
//! file, touches no database, generates no identity, speaks to no agent and
//! reports no system metrics. Each of those arrives in its own pass with its
//! own tests — see `docs/M1-IMPLEMENTATION-PLAN.md` section 17. There is no
//! placeholder endpoint and no invented data anywhere in this crate.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

pub mod guard;
pub mod notify;
pub mod signals;

use std::error::Error;

/// Build version, reported at startup and in diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable selecting the log filter, in `tracing` syntax.
pub const LOG_FILTER_ENV: &str = "ATRIUM_CORE_LOG";

/// Installs the structured logger.
///
/// JSON to stderr, so journald captures it verbatim and a diagnostics bundle
/// can read it back without a parser of its own.
pub fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env(LOG_FILTER_ENV)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(std::io::stderr)
        .init();
}

/// Runs Core until a shutdown signal arrives.
///
/// # Errors
///
/// Returns an error when a startup guard refuses — currently, running as root.
/// The caller turns that into a non-zero exit so systemd reports a failure
/// rather than a clean stop.
pub async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let identity = ProcessIdentity::current();

    guard::deny_root(identity.effective_uid)?;

    tracing::info!(
        event = "starting",
        component = "atrium-core",
        version = VERSION,
        pid = identity.pid,
        uid = identity.real_uid,
        euid = identity.effective_uid,
        gid = identity.real_gid,
        egid = identity.effective_gid,
        "atrium-core is starting"
    );

    // Before readiness, never after: a stop request that arrives between
    // READY=1 and the handler being installed would hit the default
    // disposition and kill the process uncleanly.
    let mut shutdown = signals::listen()?;

    let notified = notify::ready()?;

    tracing::info!(
        event = "ready",
        component = "atrium-core",
        version = VERSION,
        service_manager_notified = notified,
        "atrium-core is ready"
    );

    let signal = shutdown.recv().await;

    tracing::info!(
        event = "stopping",
        component = "atrium-core",
        signal = signal.as_str(),
        "atrium-core is shutting down"
    );

    notify::stopping()?;

    tracing::info!(
        event = "stopped",
        component = "atrium-core",
        "atrium-core stopped cleanly"
    );

    Ok(())
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
