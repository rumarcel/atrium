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
//! - **network** ([`http`]) — HTTPS on the configured port with the identity
//!   key: `/healthz`, a placeholder page, and in recovery the redacted
//!   diagnostics. Every route and its policy is in one literal table; `Host`
//!   is checked against the certificate's own names; cross-origin browser
//!   requests are refused; errors are RFC 9457 problems from a closed set.
//!
//! - **pairing and devices** ([`pairing`], [`devices`]) — from M1E, a
//!   console-armed secret pairs a device over TLS 1.3 bound to the
//!   connection's exporter and the server's key; devices authenticate with a
//!   256-bit bearer token of which Core keeps only `SHA-256`.
//!
//! - **system** ([`providers`], [`system`]) — from M1F, what the machine is
//!   and how it is doing, read natively from `/proc`, `/sys`, `/etc`,
//!   `getifaddrs` and `statvfs`: never a third-party monitor, and never an
//!   invented value — what cannot be read is `null` with a reason.
//!
//! Discovery arrives in M1G, in its own pass with its own tests — see
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
pub mod devices;
pub mod fsio;
pub mod guard;
pub mod http;
pub mod identity;
pub mod init;
pub mod layout;
pub mod lock;
pub mod notify;
pub mod pairing;
pub mod protect;
pub mod providers;
pub mod recovery;
pub mod redact;
pub mod rotate;
pub mod signals;
pub mod startup;
pub mod system;

#[cfg(test)]
mod testutil;

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

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
            // The listener is up before readiness is reported, and a failure
            // to serve TLS stops startup: there is never a plaintext
            // fallback and never another port (plan §16).
            let served = http::tls::Served::load(&layout, &normal.identity)?;
            let listener = TcpListener::bind(config.listen)
                .await
                .map_err(|error| format!("could not listen on {}: {error}", config.listen))?;
            let address = listener.local_addr()?;
            // The network services get their own connection to the database
            // startup has just verified and migrated.
            let observed = Observed::new();
            let services = services(&layout, &normal, &config.server_name, &observed)?;
            let app = http::App::normal(&served, address.port(), services)?;
            let listening = Listening::start(listener, Arc::clone(&app), address);
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
                listen = %listening.address,
                service_manager_notified = notified,
                "atrium-core is ready"
            );
            let signal = run_normal(&layout, &mut normal, &mut shutdown, &app, &observed).await;
            listening.close().await;
            signal
        }
        Mode::Recovery(recovery) => {
            let listening = recovery_listener(&layout, config.listen, &recovery).await;
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
                listen = listening.as_ref().map(|l| l.address.to_string()),
                service_manager_notified = notified,
                "atrium-core is ready in recovery mode"
            );
            // Recovery does nothing but wait and answer its two routes. It
            // holds no database handle, no agent client and no timer.
            let signal = shutdown.recv().await;
            if let Some(listening) = listening {
                listening.close().await;
            }
            signal
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

/// What normal mode observes in the background and the system routes read:
/// the CPU sampler, the Agent status and the recent-issue record.
struct Observed {
    sampler: Arc<providers::cpu::Sampler>,
    agent: Arc<capability::AgentStatus>,
    issues: Arc<system::Issues>,
}

impl Observed {
    fn new() -> Self {
        Self {
            sampler: providers::cpu::Sampler::new(providers::HostRoot::system()),
            agent: capability::AgentStatus::new(),
            issues: Arc::new(system::Issues::default()),
        }
    }
}

/// The Linux adapters of ADR-002's Core-side providers.
fn linux_providers(
    root: &providers::HostRoot,
    sampler: &Arc<providers::cpu::Sampler>,
) -> system::Providers {
    let (hardware, network, storage) = providers::linux::adapters(root, Arc::clone(sampler));
    system::Providers {
        hardware,
        network,
        storage,
    }
}

/// The pairing, device and system services, over a second connection to the
/// state database.
fn services(
    layout: &Layout,
    normal: &startup::Normal,
    server_name: &str,
    observed: &Observed,
) -> Result<Arc<http::Services>, Box<dyn Error + Send + Sync>> {
    let database = db::attach(layout)
        .map_err(|fault| format!("could not attach to the state database: {fault}"))?;
    let database = Arc::new(std::sync::Mutex::new(database));
    let clock = pairing::system_clock();
    Ok(Arc::new(http::Services {
        pairing: pairing::Pairing::new(
            Arc::clone(&database),
            Arc::clone(&normal.secrets),
            pairing::ServerFacts {
                server_id: normal.identity.server_id(),
                spki: normal.identity.pin(),
                name: server_name.to_owned(),
            },
            Arc::clone(&clock),
        ),
        devices: devices::Devices::new(database, normal.identity.server_id(), clock),
        system: system::System::new(
            linux_providers(&providers::HostRoot::system(), &observed.sampler),
            Arc::clone(&observed.agent),
            Arc::clone(&observed.issues),
            normal.identity.server_id(),
            system::StateFacts {
                layout: layout.clone(),
                schema_version: normal
                    .database
                    .schema_version()
                    .unwrap_or(db::SCHEMA_VERSION),
            },
        ),
    }))
}

/// The HTTPS listener task, and the switch that stops it.
struct Listening {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
    address: SocketAddr,
}

impl Listening {
    fn start(listener: TcpListener, app: Arc<http::App>, address: SocketAddr) -> Self {
        let (stop, receiver) = watch::channel(false);
        let task = tokio::spawn(http::serve(listener, app, receiver));
        Self {
            stop,
            task,
            address,
        }
    }

    /// Stops accepting, gives in-flight requests [`http::SHUTDOWN_GRACE`],
    /// and drops the rest.
    async fn close(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

/// Recovery's listener: `/healthz` and the redacted diagnostics, over TLS
/// with the verified identity key. When there is no verified key or no
/// certificate for it — which is often *why* Core is in recovery — there is
/// nothing to serve TLS with, and recovery reports through the log and the
/// service manager only. It never generates, repairs or writes anything to
/// get a listener.
async fn recovery_listener(
    layout: &Layout,
    address: SocketAddr,
    recovery: &recovery::Recovery,
) -> Option<Listening> {
    let unavailable = |reason: &str| {
        tracing::warn!(
            event = "recovery_network_unavailable",
            component = "atrium-core",
            reason = reason,
            "recovery mode is not serving over the network; see `atriumctl diagnostics`"
        );
    };
    let identity = match identity::load(layout, identity::Protection::Required) {
        Ok(identity) => identity,
        Err(fault) => {
            unavailable(recovery::RecoveryReason::from(&fault).code());
            return None;
        }
    };
    let served = match http::tls::Served::load(layout, &identity) {
        Ok(served) => served,
        Err(_) => {
            unavailable("certificate.unavailable");
            return None;
        }
    };
    let listener = match TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(_) => {
            unavailable("listen_failed");
            return None;
        }
    };
    let address = listener.local_addr().ok()?;
    let app = http::App::recovery(&served, address.port(), recovery).ok()?;
    Some(Listening::start(listener, app, address))
}

/// Normal mode until shutdown: re-checks the certificate against the current
/// addresses every [`ADDRESS_POLL`], deletes an expired pairing secret on the
/// same tick, refreshes the Agent-derived capabilities right away and then
/// every [`agentmonitor::AGENT_REFRESH`], and samples CPU usage every
/// [`providers::cpu::SAMPLE_INTERVAL`] in a task that ends with this
/// function.
async fn run_normal(
    layout: &Layout,
    normal: &mut startup::Normal,
    shutdown: &mut signals::ShutdownListener,
    app: &http::App,
    observed: &Observed,
) -> signals::Shutdown {
    // Dropping `_sampling` when this returns stops the sampler.
    let (_sampling, stop_sampling) = watch::channel(false);
    tokio::spawn(providers::cpu::run(
        Arc::clone(&observed.sampler),
        stop_sampling,
    ));
    let mut ticker = tokio::time::interval(ADDRESS_POLL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick completes immediately; startup has just checked.
    ticker.tick().await;
    // The first agent tick also completes immediately: that is the check
    // right after startup.
    let mut agent_ticker = tokio::time::interval(agentmonitor::AGENT_REFRESH);
    agent_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut monitor = match agentmonitor::Monitor::new(layout) {
        Ok(monitor) => {
            Some(monitor.publishing_to(Arc::clone(&observed.agent), Arc::clone(&observed.issues)))
        }
        Err(error) => {
            tracing::error!(
                event = "agent_client_unavailable",
                component = "atrium-core",
                reason = %error,
                "no Agent client could be built; privileged and container capabilities \
                 stay unavailable"
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
                // An expired secret's ciphertext goes even if nobody tries to
                // pair again.
                match normal.database.disarm_if_expired(now) {
                    Ok(true) => tracing::info!(
                        event = "pairing_expired",
                        component = "atrium-core",
                        "the armed pairing secret expired and was deleted"
                    ),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(
                        event = "pairing_expiry_failed",
                        component = "atrium-core",
                        reason = %error,
                        "could not delete an expired pairing secret; it is refused anyway \
                         and the next check retries"
                    ),
                }
                let addresses = current_addresses();
                match certificate::ensure(layout, &normal.identity, &addresses, now) {
                    Ok(outcome) => {
                        let before = normal.certificate.serial.clone();
                        normal.certificate = startup::settle(&normal.database, outcome, now);
                        if normal.certificate.serial != before {
                            // Same key, new names: new connections get the
                            // new certificate, and its names become the
                            // accepted hosts.
                            match http::tls::Served::load(layout, &normal.identity) {
                                Ok(served) => app.replace_certificate(&served),
                                Err(error) => tracing::warn!(
                                    event = "certificate_swap_failed",
                                    component = "atrium-core",
                                    reason = %error,
                                    "the reissued certificate could not be served; the \
                                     previous one stays in service"
                                ),
                            }
                        }
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
