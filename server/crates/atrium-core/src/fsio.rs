//! File access with the properties identity and state depend on.
//!
//! Every read refuses to follow a symbolic link, refuses anything that is not
//! a regular file, and has a size limit. Every write goes to a temporary file
//! created exclusively with mode `0600`, is flushed to disk, and only then
//! becomes visible under its real name — either by an atomic `rename` that
//! replaces the old version, or by a `link` that fails if the name is already
//! taken. The directory is flushed afterwards so the new name survives a power
//! cut. A reader therefore sees the old file or the new one, never half of
//! either, and a temporary is never readable by anyone but its owner.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nix::fcntl::OFlag;

use crate::layout::TEMP_PREFIX;

/// Why a file could not be read.
#[derive(Debug)]
pub enum ReadError {
    /// It does not exist.
    Missing,
    /// It is a symbolic link, a directory, a device or a FIFO.
    NotRegular,
    /// It is larger than the caller will accept.
    TooLarge,
    /// The kernel refused, or the read failed.
    Io(io::Error),
}

impl ReadError {
    /// True when the failure was a permission refusal.
    #[must_use]
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, Self::Io(error) if error.kind() == io::ErrorKind::PermissionDenied)
    }
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("does not exist"),
            Self::NotRegular => formatter.write_str("is not a regular file"),
            Self::TooLarge => formatter.write_str("is larger than allowed"),
            Self::Io(error) => write!(formatter, "could not be read: {}", error.kind()),
        }
    }
}

fn no_follow() -> i32 {
    (OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC).bits()
}

/// Reads a whole regular file, refusing links and anything over `limit` bytes.
///
/// # Errors
///
/// See [`ReadError`]. A symbolic link at `path` is [`ReadError::NotRegular`],
/// never followed.
pub fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>, ReadError> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(no_follow() | OFlag::O_NONBLOCK.bits())
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(ReadError::Missing),
        // O_NOFOLLOW on a symbolic link fails with ELOOP.
        Err(error) if error.raw_os_error() == Some(nix::libc::ELOOP) => {
            return Err(ReadError::NotRegular)
        }
        Err(error) => return Err(ReadError::Io(error)),
    };
    let metadata = file.metadata().map_err(ReadError::Io)?;
    if !metadata.file_type().is_file() {
        return Err(ReadError::NotRegular);
    }
    if metadata.len() > limit {
        return Err(ReadError::TooLarge);
    }
    let mut buffer = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(limit + 1)
        .read_to_end(&mut buffer)
        .map_err(ReadError::Io)?;
    if buffer.len() as u64 > limit {
        return Err(ReadError::TooLarge);
    }
    Ok(buffer)
}

/// `lstat`, with "does not exist" as `None` rather than an error.
///
/// # Errors
///
/// Any failure other than absence.
pub fn lstat(path: &Path) -> io::Result<Option<Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Opens a directory without following a link, for `fsync` and for locking.
///
/// # Errors
///
/// Fails if `path` is not a directory or is a symbolic link.
pub fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(no_follow() | OFlag::O_DIRECTORY.bits())
        .open(path)
}

/// Flushes a directory, so that names created or renamed in it are durable.
///
/// # Errors
///
/// Propagates the open or `fsync` failure.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    open_dir(path)?.sync_all()
}

/// Ownership to apply to a new file, for callers running as root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    /// User id.
    pub uid: u32,
    /// Group id.
    pub gid: u32,
}

/// How a new file should look once it is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    /// Permission bits, applied exactly, independent of the umask.
    pub mode: u32,
    /// Ownership, when the caller must set it; `None` keeps the creator's.
    pub owner: Option<Owner>,
}

impl Spec {
    /// A file with `mode`, owned by whoever creates it.
    #[must_use]
    pub const fn mode(mode: u32) -> Self {
        Self { mode, owner: None }
    }
}

/// A temporary file that removes itself unless it was promoted.
struct Temporary {
    path: PathBuf,
    keep: bool,
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Random hex for a temporary file name.
fn suffix() -> io::Result<String> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(hex::encode(bytes))
}

/// Writes `bytes` to a new temporary file in `dir`, fully flushed, with the
/// final mode and ownership already applied.
fn stage(dir: &Path, name: &str, bytes: &[u8], spec: Spec) -> io::Result<Temporary> {
    let path = dir.join(format!("{TEMP_PREFIX}{name}-{}", suffix()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(no_follow())
        .open(&path)?;
    let temporary = Temporary { path, keep: false };
    if let Some(owner) = spec.owner {
        std::os::unix::fs::fchown(&file, Some(owner.uid), Some(owner.gid))?;
    }
    file.write_all(bytes)?;
    // Mode last: a file that is about to become world-readable is only so
    // once its contents are complete, and a secret one never is.
    file.set_permissions(std::fs::Permissions::from_mode(spec.mode))?;
    file.sync_all()?;
    Ok(temporary)
}

/// Creates `dir/name` with `bytes`, failing if the name already exists.
///
/// The file appears complete or not at all: it is written and flushed under a
/// temporary name, then hard-linked into place, which fails rather than
/// replaces when the name is taken.
///
/// # Errors
///
/// [`io::ErrorKind::AlreadyExists`] if `dir/name` exists, whatever it is —
/// including a dangling symbolic link.
pub fn create_new(dir: &Path, name: &str, bytes: &[u8], spec: Spec) -> io::Result<()> {
    let temporary = stage(dir, name, bytes, spec)?;
    std::fs::hard_link(&temporary.path, dir.join(name))?;
    drop(temporary);
    sync_dir(dir)
}

/// Atomically replaces `dir/name` with `bytes`.
///
/// # Errors
///
/// Any failure leaves the previous file in place and no temporary behind.
pub fn replace(dir: &Path, name: &str, bytes: &[u8], spec: Spec) -> io::Result<()> {
    let mut temporary = stage(dir, name, bytes, spec)?;
    std::fs::rename(&temporary.path, dir.join(name))?;
    temporary.keep = true;
    sync_dir(dir)
}

/// Creates a directory with exactly `mode`, failing if it exists.
///
/// # Errors
///
/// Propagates the `mkdir` or `chmod` failure.
pub fn create_dir(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::DirBuilder::new().mode(mode).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    if let Some(parent) = path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

/// Permission bits of a file, without the type bits.
#[must_use]
pub fn mode_of(metadata: &Metadata) -> u32 {
    metadata.mode() & 0o7777
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "atrium-fsio-{tag}-{}-{}",
            std::process::id(),
            suffix().expect("random suffix")
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("listable")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8")
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn create_new_writes_the_exact_mode_and_leaves_no_temporary() {
        let dir = scratch("create");
        create_new(&dir, "a", b"hello", Spec::mode(0o640)).expect("created");
        let metadata = std::fs::metadata(dir.join("a")).expect("exists");
        assert_eq!(mode_of(&metadata), 0o640);
        assert_eq!(std::fs::read(dir.join("a")).expect("readable"), b"hello");
        assert_eq!(entries(&dir), vec!["a".to_owned()]);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn create_new_never_overwrites() {
        let dir = scratch("nooverwrite");
        std::fs::write(dir.join("a"), b"original").expect("seed");
        let error = create_new(&dir, "a", b"replacement", Spec::mode(0o600))
            .expect_err("an existing name must not be replaced");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(dir.join("a")).expect("readable"), b"original");
        assert_eq!(
            entries(&dir),
            vec!["a".to_owned()],
            "no temporary left behind"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn create_new_refuses_a_dangling_symlink_rather_than_writing_through_it() {
        let dir = scratch("dangling");
        let target = dir.join("elsewhere");
        std::os::unix::fs::symlink(&target, dir.join("a")).expect("symlink");
        let error = create_new(&dir, "a", b"x", Spec::mode(0o600)).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(!target.exists(), "nothing may be written through the link");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn replace_swaps_atomically_and_applies_the_mode() {
        let dir = scratch("replace");
        std::fs::write(dir.join("a"), b"old").expect("seed");
        replace(&dir, "a", b"new", Spec::mode(0o644)).expect("replaced");
        assert_eq!(std::fs::read(dir.join("a")).expect("readable"), b"new");
        assert_eq!(
            mode_of(&std::fs::metadata(dir.join("a")).expect("exists")),
            0o644
        );
        assert_eq!(entries(&dir), vec!["a".to_owned()]);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn replace_replaces_a_symlink_instead_of_following_it() {
        let dir = scratch("replacelink");
        let target = dir.join("target");
        std::fs::write(&target, b"victim").expect("seed");
        std::os::unix::fs::symlink(&target, dir.join("a")).expect("symlink");
        replace(&dir, "a", b"new", Spec::mode(0o600)).expect("replaced");
        assert_eq!(std::fs::read(&target).expect("readable"), b"victim");
        assert!(std::fs::symlink_metadata(dir.join("a"))
            .expect("exists")
            .file_type()
            .is_file());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn read_regular_refuses_links_directories_and_oversized_files() {
        let dir = scratch("read");
        std::fs::write(dir.join("file"), b"12345").expect("seed");
        std::os::unix::fs::symlink(dir.join("file"), dir.join("link")).expect("symlink");
        std::fs::create_dir(dir.join("sub")).expect("subdir");

        assert_eq!(
            read_regular(&dir.join("file"), 5).expect("readable"),
            b"12345"
        );
        assert!(matches!(
            read_regular(&dir.join("file"), 4),
            Err(ReadError::TooLarge)
        ));
        assert!(matches!(
            read_regular(&dir.join("link"), 64),
            Err(ReadError::NotRegular)
        ));
        assert!(matches!(
            read_regular(&dir.join("sub"), 64),
            Err(ReadError::NotRegular)
        ));
        assert!(matches!(
            read_regular(&dir.join("absent"), 64),
            Err(ReadError::Missing)
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn read_regular_refuses_a_fifo_without_blocking() {
        let dir = scratch("fifo");
        let fifo = dir.join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::from_bits_truncate(0o600))
            .expect("mkfifo");
        assert!(matches!(
            read_regular(&fifo, 64),
            Err(ReadError::NotRegular)
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
