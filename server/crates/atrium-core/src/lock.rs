//! One writer for the state directory.
//!
//! Core holds an exclusive `flock` on `/var/lib/atrium` for as long as it
//! runs, in normal and in recovery mode. `atriumctl restore` and
//! `rotate-identity` take the same lock without waiting, so a restore can
//! never swap a database out from under a running Core, and two Cores can
//! never share one state directory. The lock is on the directory itself, so
//! taking it creates no file — recovery mode writes nothing, not even a lock
//! file.

use std::fs::File;
use std::io;
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use crate::fsio;

/// Held for as long as it lives; released on drop or on process exit.
#[derive(Debug)]
pub struct StateLock {
    _held: Flock<File>,
}

/// Why the lock was not taken.
#[derive(Debug)]
pub enum LockError {
    /// Another process holds it.
    Busy,
    /// The directory could not be opened or locked.
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str(
                "another process holds the state directory (a running atrium-core, or an \
                 atriumctl restore in progress)",
            ),
            Self::Io(error) => write!(formatter, "the state directory cannot be locked: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Takes the lock on `state_dir` without waiting.
///
/// # Errors
///
/// [`LockError::Busy`] when it is held elsewhere.
pub fn acquire(state_dir: &Path) -> Result<StateLock, LockError> {
    let directory = fsio::open_dir(state_dir).map_err(LockError::Io)?;
    match Flock::lock(directory, FlockArg::LockExclusiveNonblock) {
        Ok(held) => Ok(StateLock { _held: held }),
        Err((_, Errno::EWOULDBLOCK)) => Err(LockError::Busy),
        Err((_, errno)) => Err(LockError::Io(io::Error::from(errno))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let fixture = Fixture::new("lock");
        let first = acquire(fixture.layout.state_dir()).expect("first");
        assert!(matches!(
            acquire(fixture.layout.state_dir()),
            Err(LockError::Busy)
        ));
        drop(first);
        assert!(acquire(fixture.layout.state_dir()).is_ok());
    }

    #[test]
    fn taking_the_lock_creates_nothing() {
        let fixture = Fixture::new("lockfree");
        let _held = acquire(fixture.layout.state_dir()).expect("lock");
        assert!(fixture.snapshot().is_empty());
    }
}
