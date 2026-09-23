//! Ownership and mode checks for the identity and state trees.
//!
//! The guarantee that a compromised Core cannot replace the server's identity
//! rests on discretionary access control (`docs/M1-IMPLEMENTATION-PLAN.md`
//! section 4.2), not on anything Core does. What Core *can* do is refuse to run
//! on a tree where that guarantee visibly does not hold: an identity directory
//! it could write to, a key the whole machine could read, a state directory
//! other users can list. Starting anyway would turn a deployment mistake into
//! a silent security property failure, so these checks fail closed into
//! recovery mode.
//!
//! The rules are stated as the property they protect rather than as one exact
//! mode, so that a stricter tree passes and a looser one does not:
//!
//! - identity directory: a real directory, owned by root, writable by nobody
//!   else;
//! - identity files: regular files, owned by root, writable by nobody else,
//!   and not readable by "other";
//! - state directory and database files: owned by the service user, with no
//!   group or other access at all.

use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::fsio;

/// What was wrong with one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// The path does not exist.
    Missing,
    /// The path is a symbolic link; nothing here is ever reached through one.
    SymbolicLink,
    /// A file where a directory belongs, or the reverse, or a device or FIFO.
    WrongType,
    /// Owned by someone other than root, where root ownership is required.
    OwnerNotRoot,
    /// Owned by someone other than the service user, where that is required.
    OwnerNotServiceUser,
    /// Writable by the group or by others.
    WritableByOthers,
    /// Readable by "other" — every account on the machine.
    ReadableByEveryone,
    /// Any group or other access to private state.
    AccessibleByOthers,
    /// `lstat` itself failed.
    Uninspectable,
}

impl Problem {
    /// Stable, loggable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::SymbolicLink => "symbolic_link",
            Self::WrongType => "wrong_type",
            Self::OwnerNotRoot => "owner_not_root",
            Self::OwnerNotServiceUser => "owner_not_service_user",
            Self::WritableByOthers => "writable_by_others",
            Self::ReadableByEveryone => "readable_by_everyone",
            Self::AccessibleByOthers => "accessible_by_others",
            Self::Uninspectable => "uninspectable",
        }
    }
}

/// A path that failed its check, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The offending path.
    pub path: PathBuf,
    /// What was wrong with it.
    pub problem: Problem,
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {}",
            self.path.display(),
            self.problem.as_str()
        )
    }
}

/// What a path is expected to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// `/etc/atrium`: root-owned directory, no group or other write.
    RootDirectory,
    /// A file in `/etc/atrium`: root-owned, no group or other write, no other read.
    RootFile,
    /// `/var/lib/atrium` and `backups/`: the service user's, nobody else's.
    PrivateDirectory,
    /// A state file: the service user's, nobody else's.
    PrivateFile,
}

/// Checks one path against `expect`, as seen by effective uid `euid`.
///
/// # Errors
///
/// The first property that does not hold.
pub fn check(path: &Path, expect: Expect, euid: u32) -> Result<(), Violation> {
    let violation = |problem| Violation {
        path: path.to_path_buf(),
        problem,
    };
    let metadata = match fsio::lstat(path) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return Err(violation(Problem::Missing)),
        Err(_) => return Err(violation(Problem::Uninspectable)),
    };
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(violation(Problem::SymbolicLink));
    }
    let directory = matches!(expect, Expect::RootDirectory | Expect::PrivateDirectory);
    if (directory && !kind.is_dir()) || (!directory && !kind.is_file()) {
        return Err(violation(Problem::WrongType));
    }
    let mode = fsio::mode_of(&metadata);
    match expect {
        Expect::RootDirectory | Expect::RootFile => {
            if metadata.uid() != 0 {
                return Err(violation(Problem::OwnerNotRoot));
            }
            if mode & 0o022 != 0 {
                return Err(violation(Problem::WritableByOthers));
            }
            if expect == Expect::RootFile && mode & 0o007 != 0 {
                return Err(violation(Problem::ReadableByEveryone));
            }
        }
        Expect::PrivateDirectory | Expect::PrivateFile => {
            if metadata.uid() != euid {
                return Err(violation(Problem::OwnerNotServiceUser));
            }
            if mode & 0o077 != 0 {
                return Err(violation(Problem::AccessibleByOthers));
            }
        }
    }
    Ok(())
}

/// The effective uid of this process.
#[must_use]
pub fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("atrium-protect-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[test]
    fn private_state_must_belong_to_us_and_nobody_else() {
        let dir = scratch("private");
        chmod(&dir, 0o700);
        assert_eq!(check(&dir, Expect::PrivateDirectory, euid()), Ok(()));

        chmod(&dir, 0o750);
        assert_eq!(
            check(&dir, Expect::PrivateDirectory, euid()).map_err(|v| v.problem),
            Err(Problem::AccessibleByOthers)
        );
        chmod(&dir, 0o700);

        let file = dir.join("atrium.db");
        std::fs::write(&file, b"").expect("seed");
        chmod(&file, 0o600);
        assert_eq!(check(&file, Expect::PrivateFile, euid()), Ok(()));
        chmod(&file, 0o604);
        assert_eq!(
            check(&file, Expect::PrivateFile, euid()).map_err(|v| v.problem),
            Err(Problem::AccessibleByOthers)
        );

        // Owned by us, but checked as if we were someone else.
        assert_eq!(
            check(&file, Expect::PrivateFile, euid().wrapping_add(1)).map_err(|v| v.problem),
            Err(Problem::OwnerNotServiceUser)
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn identity_files_owned_by_the_service_user_are_refused() {
        // In an unprivileged test every file is owned by the test user, which
        // is exactly the misconfiguration this check exists to catch: identity
        // material the running service could rewrite.
        if euid() == 0 {
            return;
        }
        let dir = scratch("root");
        chmod(&dir, 0o750);
        let file = dir.join("tls.key");
        std::fs::write(&file, b"").expect("seed");
        chmod(&file, 0o640);
        assert_eq!(
            check(&dir, Expect::RootDirectory, euid()).map_err(|v| v.problem),
            Err(Problem::OwnerNotRoot)
        );
        assert_eq!(
            check(&file, Expect::RootFile, euid()).map_err(|v| v.problem),
            Err(Problem::OwnerNotRoot)
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn symbolic_links_and_wrong_types_are_refused() {
        let dir = scratch("types");
        let file = dir.join("file");
        std::fs::write(&file, b"").expect("seed");
        chmod(&file, 0o600);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&file, &link).expect("symlink");

        assert_eq!(
            check(&link, Expect::PrivateFile, euid()).map_err(|v| v.problem),
            Err(Problem::SymbolicLink)
        );
        assert_eq!(
            check(&file, Expect::PrivateDirectory, euid()).map_err(|v| v.problem),
            Err(Problem::WrongType)
        );
        assert_eq!(
            check(&dir.join("absent"), Expect::PrivateFile, euid()).map_err(|v| v.problem),
            Err(Problem::Missing)
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
