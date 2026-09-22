//! Startup guards.
//!
//! Core must never run as root (ADR-001, `SECURITY.md` section 9). The systemd
//! unit says `User=atrium`, but a unit file is one edit away from being wrong
//! and a developer running the binary under `sudo` would not notice. The
//! process checks for itself and refuses, so the property holds wherever the
//! binary is started from.

use std::fmt;

/// A reason Core refused to start.
#[derive(Debug, PartialEq, Eq)]
pub enum StartupRefusal {
    /// Core was started with an effective uid of 0.
    RunningAsRoot,
}

impl fmt::Display for StartupRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunningAsRoot => write!(
                formatter,
                "atrium-core must not run as root: it is the unprivileged half of the \
                 Core/Agent split, and anything it needs privilege for goes through the \
                 agent's typed operations. Run it as the atrium service user."
            ),
        }
    }
}

impl std::error::Error for StartupRefusal {}

/// Refuses an effective uid of 0.
///
/// Split out from the process so it can be tested without being root.
pub fn deny_root(effective_uid: u32) -> Result<(), StartupRefusal> {
    if effective_uid == 0 {
        return Err(StartupRefusal::RunningAsRoot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_refused() {
        assert_eq!(deny_root(0), Err(StartupRefusal::RunningAsRoot));
    }

    #[test]
    fn an_ordinary_user_is_allowed() {
        assert!(deny_root(1).is_ok());
        assert!(deny_root(1000).is_ok());
        assert!(deny_root(u32::MAX).is_ok());
    }

    #[test]
    fn the_refusal_explains_itself_without_leaking_anything() {
        let message = StartupRefusal::RunningAsRoot.to_string();
        assert!(message.contains("must not run as root"));
        assert!(message.contains("atrium service user"));
    }
}
