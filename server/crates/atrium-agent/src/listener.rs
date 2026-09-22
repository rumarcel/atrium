//! Where Agent's socket comes from.
//!
//! In production it comes from systemd: `atrium-agent.socket` creates
//! `/run/atrium/agent.sock` with `0660 root:atrium` and hands it over as file
//! descriptor 3. Declaring the permissions in the socket unit rather than in
//! code is deliberate — they are then visible to anyone reading the unit, and
//! the process never has to `chmod` anything.
//!
//! Outside systemd, a path may be given explicitly so the lifecycle can be
//! tested without a service manager. That path is validated and never guessed.

use std::fmt;
use std::path::PathBuf;

/// First descriptor systemd passes, per the socket-activation protocol.
const SD_LISTEN_FDS_START: i32 = 3;

/// Longest socket path accepted, well inside `sun_path`.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Environment variable naming a socket path, used only when there is no
/// service manager. Never consulted when systemd has passed a descriptor.
pub const SOCKET_PATH_ENV: &str = "ATRIUM_AGENT_SOCKET";

/// Where the listening socket will come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerSource {
    /// A descriptor inherited from the service manager.
    Inherited {
        /// The descriptor number.
        fd: i32,
    },
    /// A path Agent binds itself.
    Path(PathBuf),
}

/// Why Agent could not work out what to listen on.
#[derive(Debug, PartialEq, Eq)]
pub enum ListenerError {
    /// `LISTEN_PID` was absent, unparseable, or named another process.
    ActivationNotForThisProcess,
    /// `LISTEN_FDS` did not say exactly one descriptor.
    ActivationWrongDescriptorCount(String),
    /// Neither socket activation nor an explicit path was available.
    NoSocketConfigured,
    /// Agent is running as root without socket activation.
    RootWithoutSocketActivation,
    /// The explicit path was not usable.
    PathRejected(&'static str),
}

impl fmt::Display for ListenerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActivationNotForThisProcess => write!(
                formatter,
                "LISTEN_FDS is set but LISTEN_PID does not name this process; \
                 refusing to adopt a descriptor that was not handed to us"
            ),
            Self::ActivationWrongDescriptorCount(count) => write!(
                formatter,
                "expected exactly one socket from the service manager, LISTEN_FDS says {count}"
            ),
            Self::NoSocketConfigured => write!(
                formatter,
                "no socket to listen on: start atrium-agent through atrium-agent.socket, \
                 or set {SOCKET_PATH_ENV} to an absolute path for local testing"
            ),
            Self::RootWithoutSocketActivation => write!(
                formatter,
                "refusing to bind a socket path chosen by the environment while running as                  root: a privileged agent takes its socket from atrium-agent.socket and                  nowhere else. {SOCKET_PATH_ENV} is a development affordance for                  unprivileged runs only"
            ),
            Self::PathRejected(why) => write!(formatter, "{SOCKET_PATH_ENV} is unusable: {why}"),
        }
    }
}

impl std::error::Error for ListenerError {}

/// The environment inputs that decide the listener, isolated so the decision
/// is a pure function and can be tested exhaustively.
#[derive(Debug, Default, Clone)]
pub struct Environment {
    /// Value of `LISTEN_PID`, if set.
    pub listen_pid: Option<String>,
    /// Value of `LISTEN_FDS`, if set.
    pub listen_fds: Option<String>,
    /// Value of [`SOCKET_PATH_ENV`], if set.
    pub socket_path: Option<String>,
    /// This process's id.
    pub pid: u32,
    /// This process's effective user id.
    pub euid: u32,
}

impl Environment {
    /// Reads the real environment.
    #[must_use]
    pub fn current() -> Self {
        Self {
            listen_pid: std::env::var("LISTEN_PID").ok(),
            listen_fds: std::env::var("LISTEN_FDS").ok(),
            socket_path: std::env::var(SOCKET_PATH_ENV).ok(),
            pid: std::process::id(),
            euid: nix::unistd::geteuid().as_raw(),
        }
    }
}

/// Decides where the listening socket comes from.
///
/// Socket activation wins when present; an explicit path is only a fallback,
/// so a stray environment variable cannot redirect a systemd-managed Agent.
///
/// # Errors
///
/// Returns [`ListenerError`] when the inputs are absent or inconsistent. Every
/// branch fails closed: Agent never invents a path and never adopts a
/// descriptor it cannot account for.
pub fn resolve(environment: &Environment) -> Result<ListenerSource, ListenerError> {
    if let Some(count) = environment.listen_fds.as_deref() {
        let claimed_pid = environment
            .listen_pid
            .as_deref()
            .and_then(|value| value.parse::<u32>().ok());
        if claimed_pid != Some(environment.pid) {
            return Err(ListenerError::ActivationNotForThisProcess);
        }
        if count != "1" {
            return Err(ListenerError::ActivationWrongDescriptorCount(
                count.to_owned(),
            ));
        }
        return Ok(ListenerSource::Inherited {
            fd: SD_LISTEN_FDS_START,
        });
    }

    match environment.socket_path.as_deref() {
        None => Err(ListenerError::NoSocketConfigured),
        // The environment may name a socket only for an unprivileged run. In
        // production Agent is root and socket-activated, so this branch is
        // unreachable there; refusing it outright means an environment
        // variable can never steer a root process into creating a socket
        // somewhere of an attacker's choosing.
        Some(_) if environment.euid == 0 => Err(ListenerError::RootWithoutSocketActivation),
        Some(path) => validate_path(path).map(ListenerSource::Path),
    }
}

/// Validates an explicitly configured socket path.
///
/// # Errors
///
/// Rejects anything that is not a plain, absolute, reasonably short path. This
/// is a `bind()` target, not a command, so the risk is confusion rather than
/// execution — but a component that runs as root should not accept a sloppy
/// path just because it happens to work.
pub fn validate_path(path: &str) -> Result<PathBuf, ListenerError> {
    if path.is_empty() {
        return Err(ListenerError::PathRejected("it is empty"));
    }
    if !path.starts_with('/') {
        return Err(ListenerError::PathRejected("it is not absolute"));
    }
    if path.len() > MAX_SOCKET_PATH_BYTES {
        return Err(ListenerError::PathRejected(
            "it is longer than a Unix socket path may be",
        ));
    }
    if path.contains('\0') {
        return Err(ListenerError::PathRejected("it contains a NUL byte"));
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err(ListenerError::PathRejected("it contains a '..' segment"));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> Environment {
        Environment {
            pid: 4242,
            euid: 1000,
            ..Environment::default()
        }
    }

    #[test]
    fn socket_activation_is_adopted_when_it_names_this_process() {
        let mut env = environment();
        env.listen_pid = Some("4242".into());
        env.listen_fds = Some("1".into());
        assert_eq!(resolve(&env), Ok(ListenerSource::Inherited { fd: 3 }));
    }

    #[test]
    fn a_descriptor_meant_for_another_process_is_refused() {
        let mut env = environment();
        env.listen_pid = Some("9999".into());
        env.listen_fds = Some("1".into());
        assert_eq!(
            resolve(&env),
            Err(ListenerError::ActivationNotForThisProcess)
        );
    }

    #[test]
    fn activation_without_a_pid_is_refused() {
        let mut env = environment();
        env.listen_fds = Some("1".into());
        assert_eq!(
            resolve(&env),
            Err(ListenerError::ActivationNotForThisProcess)
        );
    }

    #[test]
    fn more_than_one_descriptor_is_refused() {
        let mut env = environment();
        env.listen_pid = Some("4242".into());
        env.listen_fds = Some("2".into());
        assert!(matches!(
            resolve(&env),
            Err(ListenerError::ActivationWrongDescriptorCount(_))
        ));
    }

    #[test]
    fn activation_takes_precedence_over_an_explicit_path() {
        let mut env = environment();
        env.listen_pid = Some("4242".into());
        env.listen_fds = Some("1".into());
        env.socket_path = Some("/tmp/somewhere-else.sock".into());
        assert_eq!(resolve(&env), Ok(ListenerSource::Inherited { fd: 3 }));
    }

    #[test]
    fn no_inputs_at_all_is_refused() {
        assert_eq!(
            resolve(&environment()),
            Err(ListenerError::NoSocketConfigured)
        );
    }

    #[test]
    fn an_absolute_path_is_accepted_as_a_fallback() {
        let mut env = environment();
        env.socket_path = Some("/run/atrium/agent.sock".into());
        assert_eq!(
            resolve(&env),
            Ok(ListenerSource::Path(PathBuf::from(
                "/run/atrium/agent.sock"
            )))
        );
    }

    #[test]
    fn sloppy_paths_are_refused() {
        for bad in [
            "",
            "relative.sock",
            "./relative.sock",
            "/run/atrium/../../etc/passwd",
            "/run/atrium/has\0nul.sock",
        ] {
            let mut env = environment();
            env.socket_path = Some(bad.into());
            assert!(
                matches!(resolve(&env), Err(ListenerError::PathRejected(_))),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn an_overlong_path_is_refused() {
        let mut env = environment();
        env.socket_path = Some(format!("/{}", "a".repeat(MAX_SOCKET_PATH_BYTES)));
        assert!(matches!(resolve(&env), Err(ListenerError::PathRejected(_))));
    }

    #[test]
    fn root_may_not_take_its_socket_from_the_environment() {
        let mut env = environment();
        env.euid = 0;
        env.socket_path = Some("/run/atrium/agent.sock".into());
        assert_eq!(
            resolve(&env),
            Err(ListenerError::RootWithoutSocketActivation)
        );
    }

    #[test]
    fn root_still_accepts_socket_activation() {
        let mut env = environment();
        env.euid = 0;
        env.listen_pid = Some("4242".into());
        env.listen_fds = Some("1".into());
        assert_eq!(resolve(&env), Ok(ListenerSource::Inherited { fd: 3 }));
    }

    #[test]
    fn errors_explain_themselves() {
        assert!(ListenerError::NoSocketConfigured
            .to_string()
            .contains("atrium-agent.socket"));
    }
}
