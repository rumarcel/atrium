//! Shutdown signals.
//!
//! `SIGTERM` is what systemd sends; `SIGINT` is what a developer sends. Both
//! mean the same thing here, and both must leave through the same clean path
//! so the shutdown sequence is exercised during ordinary development rather
//! than only in production.
//!
//! **Handlers are installed before readiness is signalled.** A process that
//! says `READY=1` and only then starts listening for `SIGTERM` has a window in
//! which the service manager's stop request hits the default disposition and
//! kills it. systemd closes that window in milliseconds during a fast
//! stop-after-start, so the ordering here is a correctness requirement, not a
//! style preference. [`listen`] exists to make it impossible to get wrong:
//! acquiring the listener is a separate step from awaiting it.
//!
//! Duplicated from `atrium-core` for the reason given in [`crate::notify`].

use std::io;

use tokio::signal::unix::{signal, Signal, SignalKind};

/// Which signal ended the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    /// `SIGTERM`, the service manager's stop request.
    Term,
    /// `SIGINT`, an interactive interrupt.
    Int,
}

impl Shutdown {
    /// Name for structured logs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Term => "SIGTERM",
            Self::Int => "SIGINT",
        }
    }
}

/// Installed handlers, ready to be awaited.
pub struct ShutdownListener {
    term: Signal,
    int: Signal,
}

impl ShutdownListener {
    /// Waits for the first shutdown signal.
    ///
    /// Signals delivered before this is awaited are not lost: the handlers were
    /// installed when the listener was created, and tokio holds the
    /// notification until it is received.
    pub async fn recv(&mut self) -> Shutdown {
        tokio::select! {
            _ = self.term.recv() => Shutdown::Term,
            _ = self.int.recv() => Shutdown::Int,
        }
    }
}

/// Installs the shutdown handlers.
///
/// Call this **before** signalling readiness.
///
/// # Errors
///
/// Returns an error if the handlers cannot be installed, which is a startup
/// failure rather than something to carry on past: a process that cannot
/// observe `SIGTERM` cannot shut down cleanly.
pub fn listen() -> io::Result<ShutdownListener> {
    Ok(ShutdownListener {
        term: signal(SignalKind::terminate())?,
        int: signal(SignalKind::interrupt())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names_are_stable_for_logs() {
        assert_eq!(Shutdown::Term.as_str(), "SIGTERM");
        assert_eq!(Shutdown::Int.as_str(), "SIGINT");
    }

    #[tokio::test]
    async fn a_signal_arriving_before_the_await_is_not_lost() {
        // This is the property the ordering exists for: install, then let the
        // signal arrive, then await it. The notification must still be there.
        let mut listener = listen().expect("handlers must install");
        nix::sys::signal::raise(nix::sys::signal::Signal::SIGTERM).expect("raise must succeed");
        assert_eq!(listener.recv().await, Shutdown::Term);
    }
}
