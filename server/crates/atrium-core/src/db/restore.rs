//! Installing a pre-migration backup in place of the live database.
//!
//! This is the mechanism behind `atriumctl restore --from`; the console tool
//! adds what a mechanism should not decide for itself — privilege, the lock,
//! and the operator typing the server's name. It runs as the service user,
//! never as root, so everything it creates belongs to the service.
//!
//! Nothing is deleted. The current database, with its `-wal`, `-shm` and
//! `-journal` companions, is renamed aside to `atrium.db.corrupt-<stamp>`, so
//! it can still be examined or handed to someone who can repair it. Moving the
//! companions matters as much as moving the database: SQLite would otherwise
//! replay the old write-ahead log into the restored copy.
//!
//! The chosen backup is copied with `VACUUM INTO` to a temporary name,
//! verified, and only then linked into place — so there is never a moment when
//! `atrium.db` exists and is not a complete, verified copy.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use time::OffsetDateTime;

use super::backups::{self, Backup};
use super::{clip, integrity, user_version, SCHEMA_VERSION};
use crate::fsio;
use crate::layout::{Layout, DATABASE_FILE, TEMP_PREFIX};
use crate::protect::{self, Expect};

/// The database companions that move with it.
const COMPANIONS: [&str; 4] = ["", "-wal", "-shm", "-journal"];

/// Why a restore did not happen. In every case the live database is as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreError {
    /// The name is not a backup file name.
    NotABackupName,
    /// No such backup.
    NoSuchBackup,
    /// The backup is a link, not ours, or readable by others.
    BackupUnprotected(String),
    /// The backup itself failed verification.
    BackupInvalid(String),
    /// The backup's schema is newer than this binary.
    BackupTooNew(u32),
    /// The state directory is not ours.
    StateUnprotected(String),
    /// A file operation failed.
    Io(String),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotABackupName => formatter.write_str(
                "that is not a backup name; use a name exactly as `atriumctl restore --list` prints it",
            ),
            Self::NoSuchBackup => formatter.write_str("no backup by that name"),
            Self::BackupUnprotected(why) => write!(formatter, "the backup is not safe to use: {why}"),
            Self::BackupInvalid(why) => write!(formatter, "the backup failed verification: {why}"),
            Self::BackupTooNew(version) => write!(
                formatter,
                "the backup has schema {version}, newer than this binary's {SCHEMA_VERSION}"
            ),
            Self::StateUnprotected(why) => write!(formatter, "the state directory is not safe: {why}"),
            Self::Io(why) => write!(formatter, "{why}"),
        }
    }
}

impl std::error::Error for RestoreError {}

/// What a completed restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The backup installed.
    pub backup: String,
    /// Its schema version.
    pub schema_version: u32,
    /// Files moved aside, by their new names.
    pub moved_aside: Vec<String>,
}

/// Verifies a backup file: integrity and schema, opened read-only.
///
/// # Errors
///
/// [`RestoreError::BackupInvalid`] or [`RestoreError::BackupTooNew`].
pub fn verify(path: &Path) -> Result<u32, RestoreError> {
    let copy = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|error| RestoreError::BackupInvalid(clip(error)))?;
    integrity(&copy).map_err(RestoreError::BackupInvalid)?;
    let version = user_version(&copy).map_err(|error| RestoreError::BackupInvalid(clip(error)))?;
    if version > SCHEMA_VERSION {
        return Err(RestoreError::BackupTooNew(version));
    }
    Ok(version)
}

/// Finds a backup by exact name, refusing anything that is not one.
///
/// # Errors
///
/// [`RestoreError::NotABackupName`], [`RestoreError::NoSuchBackup`] or
/// [`RestoreError::BackupUnprotected`].
pub fn find(layout: &Layout, name: &str) -> Result<Backup, RestoreError> {
    if !backups::is_backup_name(name) {
        return Err(RestoreError::NotABackupName);
    }
    let backup = backups::list(layout)
        .map_err(|error| RestoreError::Io(error.to_string()))?
        .into_iter()
        .find(|backup| backup.name == name)
        .ok_or(RestoreError::NoSuchBackup)?;
    protect::check(&backup.path, Expect::PrivateFile, protect::euid())
        .map_err(|violation| RestoreError::BackupUnprotected(violation.to_string()))?;
    Ok(backup)
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

fn io(what: &str) -> impl Fn(std::io::Error) -> RestoreError + '_ {
    move |error| RestoreError::Io(format!("{what}: {error}"))
}

/// Replaces the live database with the named backup.
///
/// The caller must hold the state lock ([`crate::lock`]) so no Core is
/// running.
///
/// # Errors
///
/// A [`RestoreError`]; the live database is untouched unless the error says
/// otherwise, and the backup is never modified.
pub fn restore(layout: &Layout, name: &str, now: OffsetDateTime) -> Result<Restored, RestoreError> {
    let euid = protect::euid();
    protect::check(layout.state_dir(), Expect::PrivateDirectory, euid)
        .map_err(|violation| RestoreError::StateUnprotected(violation.to_string()))?;
    let backup = find(layout, name)?;
    let schema_version = verify(&backup.path)?;

    // Stage the copy first. If this fails, nothing live has moved.
    let dir = layout.state_dir();
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| RestoreError::Io("no randomness".to_owned()))?;
    let staged = dir.join(format!(
        "{TEMP_PREFIX}{DATABASE_FILE}-{}",
        hex::encode(random)
    ));
    let staged_text = staged
        .to_str()
        .ok_or_else(|| RestoreError::Io("state path is not UTF-8".to_owned()))?
        .to_owned();
    let cleanup = Staged(staged.clone());
    {
        let source = Connection::open_with_flags(
            &backup.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|error| RestoreError::BackupInvalid(clip(error)))?;
        source
            .execute("VACUUM INTO ?1", [&staged_text])
            .map_err(|error| RestoreError::Io(format!("copying the backup: {}", clip(error))))?;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let file = std::fs::File::open(&staged).map_err(io("opening the copy"))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io("protecting the copy"))?;
        file.sync_all().map_err(io("flushing the copy"))?;
    }
    if verify(&staged)? != schema_version {
        return Err(RestoreError::BackupInvalid(
            "the copy does not match its source".to_owned(),
        ));
    }

    // Move the live database and its companions aside — renamed, never deleted.
    let aside_stamp = stamp(now);
    let mut moved_aside = Vec::new();
    for suffix in COMPANIONS {
        let current = dir.join(format!("{DATABASE_FILE}{suffix}"));
        if fsio::lstat(&current)
            .map_err(io("inspecting the database"))?
            .is_none()
        {
            continue;
        }
        let aside_name = format!("{DATABASE_FILE}.corrupt-{aside_stamp}{suffix}");
        let aside = dir.join(&aside_name);
        if fsio::lstat(&aside)
            .map_err(io("inspecting the destination"))?
            .is_some()
        {
            return Err(RestoreError::Io(format!("{aside_name} already exists")));
        }
        std::fs::rename(&current, &aside).map_err(io("moving the database aside"))?;
        moved_aside.push(aside_name);
    }
    fsio::sync_dir(dir).map_err(io("flushing the state directory"))?;

    // Link, not rename: fails rather than replaces if a database reappeared.
    std::fs::hard_link(&staged, layout.database()).map_err(io("installing the copy"))?;
    drop(cleanup);
    fsio::sync_dir(dir).map_err(io("flushing the state directory"))?;

    Ok(Restored {
        backup: backup.name,
        schema_version,
        moved_aside,
    })
}

/// Removes the staged copy on every path out of [`restore`].
struct Staged(PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, Actor, AuditAction};
    use crate::testutil::{chmod, Fixture};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time")
    }

    /// A version-0 database with a marker row, migrated once so a backup of
    /// it exists, then marked again so the live copy differs from the backup.
    fn fixture_with_backup(tag: &str) -> (Fixture, String) {
        let fixture = Fixture::new(tag);
        let path = fixture.layout.database();
        {
            let connection = Connection::open(&path).expect("open");
            connection
                .execute_batch(
                    "CREATE TABLE marker (v TEXT); INSERT INTO marker VALUES ('before');",
                )
                .expect("seed");
        }
        chmod(&path, 0o600);
        let opened = db::open(&fixture.layout, now()).expect("migrates");
        opened
            .database
            .connection()
            .execute("UPDATE marker SET v = 'after'", ())
            .expect("change");
        let name = opened
            .migrated
            .as_ref()
            .expect("migrated")
            .backup
            .file_name()
            .and_then(|n| n.to_str())
            .expect("name")
            .to_owned();
        drop(opened);
        (fixture, name)
    }

    fn marker(path: &Path) -> String {
        Connection::open(path)
            .expect("open")
            .query_row("SELECT v FROM marker", (), |row| row.get(0))
            .expect("marker")
    }

    #[test]
    fn restore_installs_the_backup_and_moves_the_live_database_aside() {
        let (fixture, name) = fixture_with_backup("restore");
        let live_before = std::fs::read(fixture.layout.database()).expect("read");

        let restored = restore(&fixture.layout, &name, now()).expect("restores");
        assert_eq!(restored.backup, name);
        assert_eq!(restored.schema_version, 0);
        assert_eq!(marker(&fixture.layout.database()), "before");

        let aside = fixture.layout.state_file(&restored.moved_aside[0]);
        assert!(restored.moved_aside[0].starts_with("atrium.db.corrupt-"));
        assert_eq!(
            std::fs::read(&aside).expect("kept"),
            live_before,
            "preserved, not deleted"
        );

        let metadata = std::fs::metadata(fixture.layout.database()).expect("installed");
        assert_eq!(fsio::mode_of(&metadata), 0o600);

        // Normal service resumes: the restored database opens and migrates.
        let reopened =
            db::open(&fixture.layout, now() + time::Duration::seconds(5)).expect("opens");
        reopened
            .database
            .audit(
                AuditAction::StateRestored,
                Actor::Console,
                &serde_json::json!({}),
                now(),
            )
            .expect("writable");
    }

    #[test]
    fn restore_moves_the_corrupt_database_aside() {
        let (fixture, name) = fixture_with_backup("corruptaside");
        std::fs::write(fixture.layout.database(), vec![0x5a_u8; 4096]).expect("corrupt");
        let restored = restore(&fixture.layout, &name, now()).expect("restores");
        let aside = fixture.layout.state_file(&restored.moved_aside[0]);
        assert_eq!(std::fs::read(aside).expect("kept"), vec![0x5a_u8; 4096]);
        assert_eq!(marker(&fixture.layout.database()), "before");
    }

    #[test]
    fn restore_works_when_the_database_is_gone() {
        let (fixture, name) = fixture_with_backup("gone");
        std::fs::remove_file(fixture.layout.database()).expect("remove");
        let restored = restore(&fixture.layout, &name, now()).expect("restores");
        assert!(restored.moved_aside.is_empty());
        assert_eq!(marker(&fixture.layout.database()), "before");
    }

    #[test]
    fn names_that_are_not_backups_are_refused_before_anything_moves() {
        let (fixture, _) = fixture_with_backup("names");
        let before = fixture.snapshot();
        for bad in [
            "../../etc/passwd",
            "/etc/shadow",
            "atrium.db",
            "atrium.db.pre-1-20260924T101500Z/../../x",
            "",
        ] {
            assert_eq!(
                restore(&fixture.layout, bad, now()),
                Err(RestoreError::NotABackupName),
                "{bad:?}"
            );
        }
        assert_eq!(
            restore(&fixture.layout, "atrium.db.pre-1-20000101T000000Z", now()),
            Err(RestoreError::NoSuchBackup)
        );
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn an_invalid_backup_is_refused_and_nothing_moves() {
        let (fixture, name) = fixture_with_backup("invalid");
        std::fs::write(fixture.layout.backups_dir().join(&name), vec![0_u8; 4096]).expect("damage");
        let before = fixture.snapshot();
        assert!(matches!(
            restore(&fixture.layout, &name, now()),
            Err(RestoreError::BackupInvalid(_))
        ));
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_symlinked_backup_is_refused() {
        let (fixture, name) = fixture_with_backup("backuplink");
        let path = fixture.layout.backups_dir().join(&name);
        let elsewhere = fixture.layout.state_file("elsewhere");
        std::fs::rename(&path, &elsewhere).expect("move");
        std::os::unix::fs::symlink(&elsewhere, &path).expect("link");
        // A link is not listed as a backup at all.
        assert_eq!(
            restore(&fixture.layout, &name, now()),
            Err(RestoreError::NoSuchBackup)
        );
    }
}
