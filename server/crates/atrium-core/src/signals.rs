//! Shutdown signals.
//!
//! `SIGTERM` is what systemd sends; `SIGINT` is what a developer sends. Both
//! mean the same thing here, and both must leave through the same clean path
//! so the shutdown sequence is exercised during ordinary development rather
//! than only in production.
//!
//! Duplicated in `atrium-agent` for the reason given in [`crate::notify`].

use std::io;

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

/// Waits for the first shutdown signal.
///
/// # Errors
///
/// Returns an error if the signal handlers cannot be installed, which is a
/// startup failure rather than something to carry on past: a process that
/// cannot observe `SIGTERM` cannot shut down cleanly.
pub async fn wait_for_shutdown() -> io::Result<Shutdown> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = term.recv() => Ok(Shutdown::Term),
        _ = int.recv() => Ok(Shutdown::Int),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names_are_stable_for_logs() {
        assert_eq!(Shutdown::Term.as_str(), "SIGTERM");
        assert_eq!(Shutdown::Int.as_str(), "SIGINT");
    }
}
