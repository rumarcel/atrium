//! Where Core's files live.
//!
//! The paths are fixed by `docs/M1-IMPLEMENTATION-PLAN.md` section 4.2. They
//! are not configurable in production: a configurable identity path is a
//! configurable identity.
//!
//! One override exists, [`ROOT_ENV`], and it moves the *whole* tree under a
//! prefix — `<root>/etc/atrium` and `<root>/var/lib/atrium` — so the test suite
//! can build a complete installation in a temporary directory. It cannot point
//! the identity at one place and the state at another, and it changes nothing
//! about the permission checks, which are applied to whatever tree is in use.
//! It grants nothing: the process still runs as whoever started it, and every
//! file it could reach through the override it could reach without it.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Environment variable that relocates the whole tree under a prefix.
pub const ROOT_ENV: &str = "ATRIUM_ROOT";

/// Identity directory, relative to the root.
const ETC: &str = "etc/atrium";
/// State directory, relative to the root.
const STATE: &str = "var/lib/atrium";

/// Bootstrap configuration.
pub const CONFIG_FILE: &str = "core.toml";
/// Immutable identity facts: `server_id`, creation time, key fingerprint.
pub const IDENTITY_FILE: &str = "identity.json";
/// The device identity private key, PKCS#8 DER.
pub const PRIVATE_KEY_FILE: &str = "tls.key";
/// Thirty-two random bytes; generated in M1, first used in M3.
pub const SECRETS_KEY_FILE: &str = "secrets.key";
/// The current public certificate, PEM.
pub const CERTIFICATE_FILE: &str = "tls.crt";
/// The record of the current certificate: SANs, serial, validity.
pub const CERTIFICATE_STATE_FILE: &str = "identity-state.json";
/// The state database.
pub const DATABASE_FILE: &str = "atrium.db";
/// Pre-migration copies of the state database.
pub const BACKUPS_DIR: &str = "backups";

/// The three files that together are the server's identity. All three exist,
/// or none do; anything in between is an inconsistency, never a prompt to
/// generate what is missing.
pub const IDENTITY_FILES: [&str; 3] = [PRIVATE_KEY_FILE, SECRETS_KEY_FILE, IDENTITY_FILE];

/// Prefix of every temporary file Atrium creates. A leftover one is evidence
/// of an interrupted write and is treated as such.
pub const TEMP_PREFIX: &str = ".atrium-tmp-";

/// The file tree Core works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: Option<PathBuf>,
    etc: PathBuf,
    state: PathBuf,
}

/// Why [`ROOT_ENV`] was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// The prefix is not an absolute, normalised path.
    NotAbsolute,
    /// The prefix is not valid UTF-8, which the rest of the tree assumes.
    NotUtf8,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute => write!(
                formatter,
                "{ROOT_ENV} must be an absolute path with no '.' or '..' components"
            ),
            Self::NotUtf8 => write!(formatter, "{ROOT_ENV} must be valid UTF-8"),
        }
    }
}

impl std::error::Error for LayoutError {}

impl Layout {
    /// The production tree: `/etc/atrium` and `/var/lib/atrium`.
    #[must_use]
    pub fn production() -> Self {
        Self::build(None)
    }

    /// The same tree, relocated under `root`.
    ///
    /// # Errors
    ///
    /// Refuses a relative path or one containing `.` or `..`, so the tree
    /// cannot be steered anywhere by path arithmetic.
    pub fn under(root: &Path) -> Result<Self, LayoutError> {
        if root.to_str().is_none() {
            return Err(LayoutError::NotUtf8);
        }
        let normal = root.is_absolute()
            && root
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
        if !normal {
            return Err(LayoutError::NotAbsolute);
        }
        Ok(Self::build(Some(root.to_path_buf())))
    }

    /// Reads [`ROOT_ENV`]; the production tree when it is unset.
    ///
    /// # Errors
    ///
    /// See [`Layout::under`].
    pub fn from_env() -> Result<Self, LayoutError> {
        match std::env::var_os(ROOT_ENV) {
            None => Ok(Self::production()),
            Some(value) if value.is_empty() => Ok(Self::production()),
            Some(value) => Self::under(Path::new(&value)),
        }
    }

    fn build(root: Option<PathBuf>) -> Self {
        let base = root.clone().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            etc: base.join(ETC),
            state: base.join(STATE),
            root,
        }
    }

    /// The relocation prefix, when one is in use.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// `/etc/atrium`.
    #[must_use]
    pub fn etc_dir(&self) -> &Path {
        &self.etc
    }

    /// `/var/lib/atrium`.
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    /// `/etc/atrium/<name>`.
    #[must_use]
    pub fn etc_file(&self, name: &str) -> PathBuf {
        self.etc.join(name)
    }

    /// `/var/lib/atrium/<name>`.
    #[must_use]
    pub fn state_file(&self, name: &str) -> PathBuf {
        self.state.join(name)
    }

    /// `/etc/atrium/core.toml`.
    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.etc_file(CONFIG_FILE)
    }

    /// `/var/lib/atrium/atrium.db`.
    #[must_use]
    pub fn database(&self) -> PathBuf {
        self.state_file(DATABASE_FILE)
    }

    /// `/var/lib/atrium/backups`.
    #[must_use]
    pub fn backups_dir(&self) -> PathBuf {
        self.state_file(BACKUPS_DIR)
    }
}

/// True for a file name Atrium uses as a temporary.
#[must_use]
pub fn is_temporary(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(TEMP_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_paths_are_the_documented_ones() {
        let layout = Layout::production();
        assert_eq!(layout.etc_dir(), Path::new("/etc/atrium"));
        assert_eq!(layout.state_dir(), Path::new("/var/lib/atrium"));
        assert_eq!(layout.config(), Path::new("/etc/atrium/core.toml"));
        assert_eq!(
            layout.etc_file(PRIVATE_KEY_FILE),
            Path::new("/etc/atrium/tls.key")
        );
        assert_eq!(
            layout.state_file(CERTIFICATE_FILE),
            Path::new("/var/lib/atrium/tls.crt")
        );
        assert_eq!(layout.database(), Path::new("/var/lib/atrium/atrium.db"));
        assert_eq!(layout.backups_dir(), Path::new("/var/lib/atrium/backups"));
        assert!(layout.root().is_none());
    }

    #[test]
    fn a_relocated_tree_moves_both_halves_together() {
        let layout = Layout::under(Path::new("/tmp/fixture")).expect("valid prefix");
        assert_eq!(layout.etc_dir(), Path::new("/tmp/fixture/etc/atrium"));
        assert_eq!(layout.state_dir(), Path::new("/tmp/fixture/var/lib/atrium"));
    }

    #[test]
    fn a_relative_or_dotted_prefix_is_refused() {
        for bad in ["relative", "/tmp/../etc", "/tmp/./x", ""] {
            assert_eq!(
                Layout::under(Path::new(bad)),
                Err(LayoutError::NotAbsolute),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn temporary_names_are_recognised() {
        assert!(is_temporary(OsStr::new(".atrium-tmp-tls.key-0011")));
        assert!(!is_temporary(OsStr::new("tls.key")));
    }
}
