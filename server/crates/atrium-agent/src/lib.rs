//! Atrium Agent — M1A lifecycle skeleton.
//!
//! Agent is the privileged half of the Core/Agent split. In M1A it has no
//! privileged work to do, so it does none: it acquires its socket, accepts a
//! connection, closes it, and shuts down cleanly on `SIGTERM`.
//!
//! Where its socket comes from is decided by [`listener::resolve`], which
//! refuses an environment-chosen path when running as root: a privileged agent
//! takes its socket from `atrium-agent.socket` and nowhere else.
//!
//! What it deliberately does **not** do: there is no operation table, no
//! request parsing, no peer-credential decision, no journal, no container
//! runtime client and no probe of one. Those arrive in **M1C**, which is also
//! where the peer-uid check belongs — a check written before the thing it
//! guards is a check nobody can test.
//!
//! The one thing worth noticing about this crate today is what is absent from
//! its manifest: no HTTP crate, no TLS crate, no SQL crate. That is enforced
//! by `server/ci/boundary-checks.sh` against cargo's metadata.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

pub mod listener;
pub mod notify;
pub mod signals;

use std::error::Error;
use std::time::Duration;

use tokio::net::UnixListener;

/// Build version, reported at startup.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variable selecting the log filter, in `tracing` syntax.
pub const LOG_FILTER_ENV: &str = "ATRIUM_AGENT_LOG";

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

    let source = listener::resolve(&listener::Environment::current())?;
    let socket = bind(&source)?;

    // Before readiness, never after - see atrium_agent::signals.
    let mut shutdown = signals::listen()?;

    let notified = notify::ready()?;

    tracing::info!(
        event = "ready",
        component = "atrium-agent",
        version = VERSION,
        source = source_label(&source),
        service_manager_notified = notified,
        "atrium-agent is ready"
    );

    let signal = tokio::select! {
        signal = shutdown.recv() => signal,
        () = accept_until_shutdown(&socket) => unreachable!("the accept loop does not return"),
    };

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

/// Accepts connections and closes them immediately.
///
/// M1A defines no protocol, so there is nothing to read and nothing to answer.
/// Closing at once is the honest behaviour: a caller learns that Agent is
/// listening and learns nothing else, and no half-implemented parser exists for
/// anyone to reach.
async fn accept_until_shutdown(socket: &UnixListener) {
    loop {
        match socket.accept().await {
            Ok((stream, _address)) => {
                tracing::debug!(
                    event = "connection_closed",
                    component = "atrium-agent",
                    reason = "no_protocol_in_m1a",
                    "accepted a connection and closed it; the protocol arrives in M1C"
                );
                drop(stream);
            }
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
