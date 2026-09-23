//! Pre-migration copies of the state database.
//!
//! `/var/lib/atrium/backups/atrium.db.pre-<n>-<UTC timestamp>`, one per
//! migration run, `0600` in a `0700` directory, made with `VACUUM INTO` —
//! never a file copy of a live database (ADR-012) — and verified before the
//! migration is allowed to start. The newest five are kept.
//!
//! This module also lists them, for recovery mode's diagnostics and for
//! `atriumctl restore --list`. Listing reads directory entries only.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use time::OffsetDateTime;

use super::{clip, integrity, user_version, StateFault};
use crate::fsio;
use crate::layout::{Layout, DATABASE_FILE};
use crate::protect::{self, Expect};

/// How many backups survive pruning.
pub const KEEP: usize = 5;

/// One backup on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    /// File name, which is also what `atriumctl restore --from` accepts.
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// The schema version the migration that made it was moving *to*.
    pub before_version: u32,
    /// When it was taken, `YYYYMMDDTHHMMSSZ`.
    pub taken_at: String,
}

fn prefix() -> String {
    format!("{DATABASE_FILE}.pre-")
}

fn stamp(now: OffsetDateTime) -> String {
    let now = now.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// Parses `atrium.db.pre-<n>-<stamp>`; `None` for anything else.
fn parse_name(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix(&prefix())?;
    let (version, taken_at) = rest.split_once('-')?;
    let version = version.parse().ok()?;
    let valid = taken_at.len() == 16
        && taken_at.as_bytes()[8] == b'T'
        && taken_at.ends_with('Z')
        && taken_at
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || i == 15 || b.is_ascii_digit());
    valid.then(|| (version, taken_at.to_owned()))
}

/// True when `name` is a backup file name. The console tool accepts nothing
/// else, so `--from` cannot name a path outside the backups directory.
#[must_use]
pub fn is_backup_name(name: &str) -> bool {
    parse_name(name).is_some()
}

/// Every backup, newest first. Entries that are not regular files, or whose
/// names do not match, are ignored.
///
/// # Errors
///
/// When the directory exists but cannot be read. A missing directory is an
/// empty list.
pub fn list(layout: &Layout) -> std::io::Result<Vec<Backup>> {
    let dir = layout.backups_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((before_version, taken_at)) = parse_name(&name) else {
            continue;
        };
        let metadata = entry.metadata()?;
        if !metadata.file_type().is_file() {
            continue;
        }
        found.push(Backup {
            path: dir.join(&name),
            size: metadata.size(),
            name,
            before_version,
            taken_at,
        });
    }
    found.sort_by(|a, b| {
        (&b.taken_at, b.before_version, &b.name).cmp(&(&a.taken_at, a.before_version, &a.name))
    });
    Ok(found)
}

/// Makes sure the backups directory exists and is private.
fn prepare_dir(layout: &Layout) -> Result<PathBuf, StateFault> {
    let dir = layout.backups_dir();
    let exists = fsio::lstat(&dir)
        .map_err(|error| StateFault::BackupFailed(clip(error)))?
        .is_some();
    if !exists {
        fsio::create_dir(&dir, 0o700).map_err(|error| StateFault::BackupFailed(clip(error)))?;
    }
    protect::check(&dir, Expect::PrivateDirectory, protect::euid())
        .map_err(|violation| StateFault::BackupFailed(violation.to_string()))?;
    Ok(dir)
}

/// Takes and verifies the copy that must exist before migrating from
/// `from` to `to`.
pub(super) fn take(
    connection: &Connection,
    layout: &Layout,
    from: u32,
    to: u32,
    now: OffsetDateTime,
) -> Result<PathBuf, StateFault> {
    let failed = |message: String| StateFault::BackupFailed(message);
    let dir = prepare_dir(layout)?;
    let path = dir.join(format!("{}{to}-{}", prefix(), stamp(now)));
    if fsio::lstat(&path).map_err(|e| failed(clip(e)))?.is_some() {
        return Err(failed(format!("{} already exists", path.display())));
    }
    let target = path
        .to_str()
        .ok_or_else(|| failed("backup path is not UTF-8".to_owned()))?;

    let result = connection
        .execute("VACUUM INTO ?1", [target])
        .map_err(|error| failed(clip(error)))
        .and_then(|_| seal(&path).map_err(|error| failed(clip(error))))
        .and_then(|()| verify(&path, from));
    if let Err(fault) = result {
        // A copy that failed verification is worse than none: it would be
        // offered for restore later. Remove it; the original is untouched.
        let _ = std::fs::remove_file(&path);
        return Err(fault);
    }
    fsio::sync_dir(&dir).map_err(|error| failed(clip(error)))?;
    Ok(path)
}

/// Mode `0600` and flushed, whatever the umask was.
fn seal(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let file = std::fs::OpenOptions::new().read(true).open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.sync_all()
}

/// The copy must pass `integrity_check` and carry the schema it was taken at.
fn verify(path: &Path, expected_version: u32) -> Result<(), StateFault> {
    let failed = |message: String| StateFault::BackupFailed(message);
    let copy = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| failed(clip(error)))?;
    integrity(&copy).map_err(|message| failed(format!("backup failed verification: {message}")))?;
    let version = user_version(&copy).map_err(|error| failed(clip(error)))?;
    if version != expected_version {
        return Err(failed(format!(
            "backup has schema {version}, expected {expected_version}"
        )));
    }
    Ok(())
}

/// Removes all but the newest [`KEEP`] backups. Returns how many went.
///
/// # Errors
///
/// When listing or removing fails. Pruning is housekeeping: callers log a
/// failure and carry on.
pub fn prune(layout: &Layout) -> std::io::Result<usize> {
    let backups = list(layout)?;
    let mut removed = 0;
    for backup in backups.iter().skip(KEEP) {
        std::fs::remove_file(&backup.path)?;
        removed += 1;
    }
    if removed > 0 {
        fsio::sync_dir(&layout.backups_dir())?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Fixture;

    #[test]
    fn names_are_parsed_strictly() {
        assert_eq!(
            parse_name("atrium.db.pre-1-20260924T101500Z"),
            Some((1, "20260924T101500Z".to_owned()))
        );
        for bad in [
            "atrium.db.pre-1-20260924T101500",
            "atrium.db.pre-x-20260924T101500Z",
            "atrium.db.pre-1-2026092AT101500Z",
            "../atrium.db.pre-1-20260924T101500Z",
            "atrium.db",
            "atrium.db.corrupt-20260924T101500Z",
        ] {
            assert!(parse_name(bad).is_none(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn the_stamp_sorts_chronologically() {
        let earlier = OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time");
        let later = earlier + time::Duration::seconds(1);
        assert!(stamp(earlier) < stamp(later));
        assert_eq!(stamp(earlier).len(), 16);
    }

    #[test]
    fn pruning_keeps_the_newest_five() {
        let fixture = Fixture::new("prune");
        let dir = fixture.layout.backups_dir();
        std::fs::create_dir(&dir).expect("dir");
        let base = OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time");
        let mut names = Vec::new();
        for i in 0..8 {
            let name = format!("{}1-{}", prefix(), stamp(base + time::Duration::minutes(i)));
            std::fs::write(dir.join(&name), b"x").expect("seed");
            names.push(name);
        }
        std::fs::write(dir.join("unrelated"), b"keep me").expect("seed");

        assert_eq!(prune(&fixture.layout).expect("pruned"), 3);
        let left: Vec<String> = list(&fixture.layout)
            .expect("list")
            .into_iter()
            .map(|b| b.name)
            .collect();
        let mut expected: Vec<String> = names[3..].to_vec();
        expected.reverse();
        assert_eq!(left, expected);
        assert!(dir.join("unrelated").exists(), "only backups are pruned");
    }
}
