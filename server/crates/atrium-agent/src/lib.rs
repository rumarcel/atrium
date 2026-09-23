//! Atrium Agent — the privileged half of the Core/Agent split.
//!
//! As of M1C, Agent serves the typed Core-to-Agent protocol
//! (`atrium-protocol`) on `/run/atrium/agent.sock`:
//!
//! - **who** ([`peer`]) — exactly one uid, Core's, taken from the kernel
//!   with `SO_PEERCRED` and compared with the `atrium` user resolved at
//!   startup; nothing a peer sends can change it;
//! - **what** ([`serve`]) — one handshake and one of exactly two
//!   parameterless, read-only operations, [`info`] and [`probe`];
//! - **evidence** ([`journal`]) — one line per connection, accepted or not,
//!   in a root-only directory the `atrium` user cannot read or change.
//!
//! Where its socket comes from is decided by [`listener::resolve`], which
//! refuses an environment-chosen path when running as root: a privileged agent
//! takes its socket from `atrium-agent.socket` and nowhere else.
//!
//! **M1's operation table contains no mutating operation of any kind.** Agent
//! executes nothing, writes nothing but its own journal, and sends no byte to
//! a container runtime.
//!
//! What is absent from its manifest matters as much: no HTTP crate, no TLS
//! crate, no SQL crate, no container-runtime client. That is enforced by
//! `server/ci/boundary-checks.sh` against cargo's metadata.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

pub mod clock;
pub mod info;
pub mod journal;
pub mod listener;
pub mod notify;
pub mod peer;
pub mod probe;
pub mod serve;
pub mod signals;

use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use atrium_protocol::values::Version;
use tokio::net::UnixListener;

use crate::info::Startup;
use crate::journal::Journal;
use crate::serve::Server;

/// Build version, reported at startup.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable selecting the log filter, in `tracing` syntax.
pub const LOG_FILTER_ENV: &str = "ATRIUM_AGENT_LOG";

/// Environment variable relocating Agent's state directory, for tests.
///
/// Honoured in every mode, because what it can select is narrow: the
/// directory must already exist, be a real directory owned by Agent's uid and
/// have no group or other access, or the journal is unavailable and Agent
/// performs nothing. Only root could create such a directory for a root
/// Agent, and Core cannot set Agent's environment at all.
pub const STATE_DIR_ENV: &str = "ATRIUM_AGENT_STATE_DIR";

/// Pause after a failed `accept` so a persistent error cannot become a hot loop.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(200);

/// Installs the structured logger.
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

/// Runs Agent until a shutdown signal arrives.
///
/// # Errors
///
/// Returns an error when the listening socket cannot be resolved or bound.
/// Agent never falls back to a guessed path: no socket means no start.
pub async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let pid = std::process::id();
    let uid = nix::unistd::geteuid().as_raw();

    tracing::info!(
        event = "starting",
        component = "atrium-agent",
        version = VERSION,
        pid = pid,
        euid = uid,
        "atrium-agent is starting"
    );

    let started = SystemTime::now();
    let policy = peer::resolve(uid, peer::system_lookup)?;
    let state_dir = state_dir()?;
    let source = listener::resolve(&listener::Environment::current())?;
    let socket = bind(&source)?;

    // Before readiness, never after - see atrium_agent::signals.
    let mut shutdown = signals::listen()?;

    let mut server = Server::new(
        policy,
        Journal::new(state_dir.clone(), uid),
        Startup {
            started_at: clock::timestamp(started),
            version: Version::parse(VERSION)?,
        },
    );
    server.open_journal();

    let notified = notify::ready()?;

    tracing::info!(
        event = "ready",
        component = "atrium-agent",
        version = VERSION,
        protocol = atrium_protocol::AGENT_PROTOCOL_VERSION,
        source = source_label(&source),
        core_uid = policy.core_uid(),
        peer_policy = policy.source().as_str(),
        state_dir = %state_dir.display(),
        service_manager_notified = notified,
        "atrium-agent is ready"
    );

    let signal = tokio::select! {
        signal = shutdown.recv() => signal,
        () = serve_until_shutdown(&socket, &mut server) => unreachable!("the accept loop does not return"),
    };

    server.flush_suppressed();

    tracing::info!(
        event = "stopping",
        component = "atrium-agent",
        signal = signal.as_str(),
        "atrium-agent is shutting down"
    );

    notify::stopping()?;

    tracing::info!(
        event = "stopped",
        component = "atrium-agent",
        "atrium-agent stopped cleanly"
    );

    Ok(())
}

/// The state directory: [`STATE_DIR_ENV`] if set and well-formed, else the
/// production constant.
fn state_dir() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    match std::env::var(STATE_DIR_ENV) {
        Ok(path) => listener::validate_path(&path).map_err(|error| match error {
            listener::ListenerError::PathRejected(why) => {
                format!("{STATE_DIR_ENV} is unusable: {why}").into()
            }
            other => other.to_string().into(),
        }),
        Err(_) => Ok(PathBuf::from(atrium_protocol::AGENT_STATE_DIR)),
    }
}

/// Accepts connections and serves each one to completion.
async fn serve_until_shutdown(socket: &UnixListener, server: &mut Server) {
    loop {
        match socket.accept().await {
            Ok((stream, _address)) => server.handle(stream).await,
            Err(error) => {
                tracing::warn!(
                    event = "accept_failed",
                    component = "atrium-agent",
                    reason = %error,
                    "could not accept a connection"
                );
                tokio::time::sleep(ACCEPT_BACKOFF).await;
            }
        }
    }
}

fn source_label(source: &listener::ListenerSource) -> &'static str {
    match source {
        listener::ListenerSource::Inherited { .. } => "socket_activation",
        listener::ListenerSource::Path(_) => "explicit_path",
    }
}

/// Turns a resolved source into a listening socket.
fn bind(source: &listener::ListenerSource) -> Result<UnixListener, Box<dyn Error + Send + Sync>> {
    use std::os::unix::net::UnixListener as StdUnixListener;

    let std_socket = match source {
        listener::ListenerSource::Inherited { fd } => {
            use std::os::fd::FromRawFd;
            // SAFETY: the service manager passed this descriptor to us and
            // `listener::resolve` has already confirmed that LISTEN_PID names
            // this process and that LISTEN_FDS is exactly one, so descriptor 3
            // is ours and is the only one. Nothing else in the process has
            // taken ownership of it. This is the only `unsafe` block in the
            // server workspace, and `server/ci/boundary-checks.sh` fails the
            // build if another appears.
            #[allow(unsafe_code)]
            unsafe {
                StdUnixListener::from_raw_fd(*fd)
            }
        }
        listener::ListenerSource::Path(path) => {
            if path.exists() {
                return Err(format!(
                    "{} already exists; refusing to unlink a path we did not create",
                    path.display()
                )
                .into());
            }
            StdUnixListener::bind(path)?
        }
    };

    std_socket.set_nonblocking(true)?;
    Ok(UnixListener::from_std(std_socket)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_labels_are_stable_for_logs() {
        assert_eq!(
            source_label(&listener::ListenerSource::Inherited { fd: 3 }),
            "socket_activation"
        );
        assert_eq!(
            source_label(&listener::ListenerSource::Path(
                "/run/atrium/agent.sock".into()
            )),
            "explicit_path"
        );
    }

    #[test]
    fn binding_refuses_an_existing_path() {
        // Refusing rather than unlinking keeps Agent from removing a file it
        // did not create, which matters for a process running as root.
        let error = bind(&listener::ListenerSource::Path("/".into()))
            .expect_err("binding over an existing path must fail");
        assert!(error.to_string().contains("already exists"));
    }
}
