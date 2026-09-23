//! Who may talk to Agent.
//!
//! Exactly one uid: Core's. It is resolved once, at startup, from the
//! `atrium` system user and cached, then compared with the uid the kernel
//! reports for each connection (`SO_PEERCRED`). Nothing the peer sends can
//! change the answer: not a field in a frame, not an environment variable,
//! not a token. The socket's `0660 root:atrium` mode decides who can
//! *connect*; this decides who is *answered*, and group membership alone is
//! never enough.

use std::fmt;

/// The service account Core runs as.
pub const CORE_USER: &str = "atrium";

/// How the admitted uid was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    /// Agent is root and admits the `atrium` user's uid. Production.
    ServiceUser,
    /// Agent is not root and admits only its own uid. A development and
    /// test affordance: an unprivileged Agent holds no privilege to protect,
    /// and a uid is still compared, never assumed.
    SameUser,
}

impl PolicySource {
    /// Name for structured logs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ServiceUser => "service_user",
            Self::SameUser => "same_user_unprivileged",
        }
    }
}

/// The one uid Agent answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerPolicy {
    core_uid: u32,
    source: PolicySource,
}

impl PeerPolicy {
    /// Whether a connection from `uid` is Core.
    #[must_use]
    pub fn admits(self, uid: u32) -> bool {
        uid == self.core_uid
    }

    /// The admitted uid.
    #[must_use]
    pub fn core_uid(self) -> u32 {
        self.core_uid
    }

    /// How it was decided.
    #[must_use]
    pub fn source(self) -> PolicySource {
        self.source
    }
}

/// Why no policy could be formed. Each one stops Agent from starting: an
/// Agent that does not know who Core is must not guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerError {
    /// Running as root and there is no `atrium` user.
    NoServiceUser,
    /// The `atrium` user resolves to uid 0, which would admit root.
    ServiceUserIsRoot,
    /// The user database could not be read.
    Lookup(String),
}

impl fmt::Display for PeerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoServiceUser => write!(
                formatter,
                "there is no `{CORE_USER}` user, so Agent cannot tell which peer is Core; \
                 refusing to start"
            ),
            Self::ServiceUserIsRoot => write!(
                formatter,
                "the `{CORE_USER}` user has uid 0; Core must never be root, and Agent will \
                 not admit uid 0 as Core"
            ),
            Self::Lookup(error) => {
                write!(
                    formatter,
                    "could not look up the `{CORE_USER}` user: {error}"
                )
            }
        }
    }
}

impl std::error::Error for PeerError {}

/// Decides the policy. `lookup` resolves a user name to a uid; it is a
/// parameter so the decision is a pure function with exhaustive tests.
///
/// # Errors
///
/// [`PeerError`] when running as root and the `atrium` user is missing,
/// unreadable or root.
pub fn resolve(
    euid: u32,
    lookup: impl FnOnce(&str) -> Result<Option<u32>, String>,
) -> Result<PeerPolicy, PeerError> {
    if euid != 0 {
        return Ok(PeerPolicy {
            core_uid: euid,
            source: PolicySource::SameUser,
        });
    }
    match lookup(CORE_USER) {
        Ok(Some(0)) => Err(PeerError::ServiceUserIsRoot),
        Ok(Some(uid)) => Ok(PeerPolicy {
            core_uid: uid,
            source: PolicySource::ServiceUser,
        }),
        Ok(None) => Err(PeerError::NoServiceUser),
        Err(error) => Err(PeerError::Lookup(error)),
    }
}

/// Resolves a user name against the system user database.
///
/// # Errors
///
/// The lookup's own error, as text.
pub fn system_lookup(name: &str) -> Result<Option<u32>, String> {
    nix::unistd::User::from_name(name)
        .map(|user| user.map(|user| user.uid.as_raw()))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_admits_the_atrium_user_and_nobody_else() {
        let policy = resolve(0, |name| {
            assert_eq!(name, "atrium");
            Ok(Some(990))
        })
        .expect("policy");
        assert_eq!(policy.source(), PolicySource::ServiceUser);
        assert!(policy.admits(990));
        for other in [0, 65_534, 991, 1000, u32::MAX] {
            assert!(!policy.admits(other), "uid {other} must be refused");
        }
    }

    #[test]
    fn root_without_an_atrium_user_refuses_to_start() {
        assert_eq!(resolve(0, |_| Ok(None)), Err(PeerError::NoServiceUser));
    }

    #[test]
    fn an_atrium_user_with_uid_zero_is_refused() {
        assert_eq!(
            resolve(0, |_| Ok(Some(0))),
            Err(PeerError::ServiceUserIsRoot)
        );
    }

    #[test]
    fn a_failed_lookup_is_refused() {
        assert!(matches!(
            resolve(0, |_| Err("nss down".into())),
            Err(PeerError::Lookup(_))
        ));
    }

    #[test]
    fn unprivileged_agent_admits_only_its_own_uid() {
        let policy = resolve(1000, |_| panic!("no lookup when unprivileged")).expect("policy");
        assert_eq!(policy.source(), PolicySource::SameUser);
        assert!(policy.admits(1000));
        assert!(!policy.admits(0));
        assert!(!policy.admits(1001));
    }
}
