//! The state database: `/var/lib/atrium/atrium.db` (ADR-012).
//!
//! One SQLite file in WAL mode. M1 creates only this one; `secrets.db` and
//! `metrics.db` belong to the passes that first need them.
//!
//! Opening it is where persistence fails safe:
//!
//! - A missing database is a fault, not an invitation to create one. The
//!   database is created exactly once, by `init-identity` at installation;
//!   after that its absence means something was lost, and a fresh empty
//!   database would silently discard every paired device.
//! - `PRAGMA integrity_check` runs before anything else. A database that
//!   fails it is left byte-for-byte as found — not deleted, not truncated,
//!   not "repaired" — and Core enters recovery mode.
//! - A schema newer than this binary understands is refused, never
//!   downgraded.
//! - Before any migration the database is copied with `VACUUM INTO`, and
//!   the copy is verified. No copy, no migration.
//! - Each migration runs in one transaction together with the
//!   `user_version` bump, so a failure leaves the previous schema intact.

pub mod backups;
pub mod restore;

use std::fmt;
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use time::OffsetDateTime;

use crate::fsio;
use crate::identity::{format_time, SpkiPin};
use crate::layout::{Layout, DATABASE_FILE};
use crate::protect::{self, Expect, Violation};

/// The schema this binary writes and understands.
pub const SCHEMA_VERSION: u32 = 1;

/// How long a connection waits for another writer.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest SQLite message kept in a fault. They go to the log, never to a
/// response, but an unbounded string is never a good idea.
const MESSAGE_LIMIT: usize = 512;

/// One forward-only schema step.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// The `user_version` after this step.
    pub version: u32,
    /// Short name, for logs.
    pub name: &'static str,
    /// The statements.
    pub sql: &'static str,
}

/// Every migration, in order, starting at 1 with no gaps.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial",
    sql: include_str!("migrations/0001_initial.sql"),
}];

/// Why the state database cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateFault {
    /// The state directory or a database file has the wrong owner, mode or type.
    Unprotected(Violation),
    /// There is no database. After installation that means it was lost.
    Missing,
    /// SQLite could not open it, or `integrity_check` did not say `ok`. The
    /// message is SQLite's own, for the log only.
    Unreadable(String),
    /// The schema is newer than this binary. Refused rather than downgraded.
    SchemaNewer {
        /// The `user_version` found.
        found: u32,
    },
    /// The pre-migration copy could not be made or verified, so nothing was
    /// migrated.
    BackupFailed(String),
    /// A migration failed and was rolled back.
    MigrationFailed {
        /// The version that was being applied.
        version: u32,
        /// SQLite's message, for the log only.
        message: String,
    },
    /// `create` found a database already there.
    AlreadyExists,
}

impl fmt::Display for StateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unprotected(violation) => {
                write!(formatter, "state is not protected: {violation}")
            }
            Self::Missing => formatter.write_str("the state database does not exist"),
            Self::Unreadable(message) => {
                write!(formatter, "the state database is unreadable: {message}")
            }
            Self::SchemaNewer { found } => write!(
                formatter,
                "the state database has schema {found}, newer than this binary's {SCHEMA_VERSION}"
            ),
            Self::BackupFailed(message) => write!(
                formatter,
                "the pre-migration backup failed, so nothing was migrated: {message}"
            ),
            Self::MigrationFailed { version, message } => write!(
                formatter,
                "migration {version} failed and was rolled back: {message}"
            ),
            Self::AlreadyExists => formatter.write_str("a state database already exists"),
        }
    }
}

fn clip(message: impl fmt::Display) -> String {
    let mut text = message.to_string();
    if text.len() > MESSAGE_LIMIT {
        let mut end = MESSAGE_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// The `settings` key holding the pin of the key the devices were paired
/// against. See [`Database::reconcile_identity`].
pub const IDENTITY_PIN_SETTING: &str = "identity.spki_sha256";

/// What [`Database::reconcile_identity`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciled {
    /// No pin was recorded; the current one now is.
    Recorded,
    /// The recorded pin is the current one.
    Unchanged,
    /// The key changed; this many devices were revoked.
    Revoked(usize),
}

/// Audit actions Core writes. A closed set: the column is not free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    /// The schema was migrated forward.
    StateMigrated,
    /// A backup was restored from the console.
    StateRestored,
    /// A new certificate was issued for the same key.
    CertificateReissued,
    /// Every device was revoked and pairing disarmed because the identity key
    /// changed.
    DevicesRevoked,
    /// The identity key was replaced from the console.
    KeyRotated,
    /// Core called Agent (M1C), whatever the outcome.
    AgentCall,
}

impl AuditAction {
    /// The value stored in `audit.action`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StateMigrated => "state.migrated",
            Self::StateRestored => "state.restored",
            Self::CertificateReissued => "identity.certificate_reissued",
            Self::DevicesRevoked => "identity.devices_revoked",
            Self::KeyRotated => "identity.key_rotated",
            Self::AgentCall => "agent.call",
        }
    }
}

/// How a call to Agent ended, as `audit.outcome` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCallOutcome {
    /// Agent answered with a result.
    Ok,
    /// Agent answered with a refusal.
    Refused,
    /// Agent did not answer: unreachable, timed out, or not speaking the
    /// protocol.
    Failed,
}

impl AgentCallOutcome {
    /// The value stored in `audit.outcome`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }
}

/// Who performed an audited action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// Core itself.
    System,
    /// An operator at the server console, through `atriumctl`.
    Console,
}

impl Actor {
    fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Console => "console",
        }
    }
}

/// An open, verified, current state database.
pub struct Database {
    connection: Connection,
    path: PathBuf,
}

impl fmt::Debug for Database {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Database")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Database {
    /// The applied schema version.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn schema_version(&self) -> rusqlite::Result<u32> {
        user_version(&self.connection)
    }

    /// Appends an audit row. `detail` must be built from values that are safe
    /// to keep forever: identifiers and reason codes, never secrets.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn audit(
        &self,
        action: AuditAction,
        actor: Actor,
        detail: &serde_json::Value,
        now: OffsetDateTime,
    ) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO audit (ts, actor_role, action, outcome, detail) \
             VALUES (?1, ?2, ?3, 'ok', ?4)",
            (
                format_time(now),
                actor.as_str(),
                action.as_str(),
                detail.to_string(),
            ),
        )?;
        Ok(())
    }

    /// Records one call to Agent: the correlation id Agent journals too,
    /// the operation, and how it ended. `error_code` is a stable cause from
    /// the closed set in `agentclient::Failure`, never free text.
    ///
    /// This row is Core's record. It is useful history and it is **not**
    /// tamper-evident: a compromised Core can rewrite its own database. The
    /// independent record of what Agent was asked is Agent's journal.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn audit_agent_call(
        &self,
        request_id: &atrium_protocol::values::RequestId,
        op: atrium_protocol::messages::AgentOp,
        outcome: AgentCallOutcome,
        error_code: Option<&'static str>,
        now: OffsetDateTime,
    ) -> rusqlite::Result<()> {
        self.connection.execute(
            "INSERT INTO audit (ts, request_id, actor_role, action, target, outcome, error_code) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                format_time(now),
                request_id.as_str(),
                Actor::System.as_str(),
                AuditAction::AgentCall.as_str(),
                op.as_str(),
                outcome.as_str(),
                error_code,
            ),
        )?;
        Ok(())
    }

    /// Binds the state to the identity key it was built against.
    ///
    /// The database records the pin of the key its devices were paired
    /// against (`settings.identity.spki_sha256`). When the identity's pin
    /// differs — the key was deliberately rotated, or a backup from before a
    /// rotation was restored — every device is revoked and pairing disarmed,
    /// in one transaction with its audit row, before anything else happens.
    /// A device paired against one key never survives into state served under
    /// another.
    ///
    /// This is what makes rotation safe to interrupt: whichever of the key
    /// write and the revocation happens first, the next normal start finishes
    /// the job. It is also why `atriumctl rotate-identity` never has to process
    /// the service-writable database while it can still regain root.
    ///
    /// # Errors
    ///
    /// SQLite's error; nothing is changed.
    pub fn reconcile_identity(
        &mut self,
        pin: &SpkiPin,
        actor: Actor,
        now: OffsetDateTime,
    ) -> rusqlite::Result<Reconciled> {
        let pin = pin.to_string();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let recorded: Option<String> = transaction
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [IDENTITY_PIN_SETTING],
                |row| row.get(0),
            )
            .optional()?;
        let outcome = match recorded {
            Some(existing) if existing == pin => Reconciled::Unchanged,
            None => {
                transaction.execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                    (IDENTITY_PIN_SETTING, &pin),
                )?;
                Reconciled::Recorded
            }
            Some(_) => {
                let revoked = transaction.execute("DELETE FROM devices", ())?;
                transaction.execute(
                    "UPDATE pairing_state SET armed = 0, secret_digest = NULL, armed_at = NULL, \
                     expires_at = NULL, failures = 0, locked = 0",
                    (),
                )?;
                transaction.execute(
                    "UPDATE settings SET value = ?2 WHERE key = ?1",
                    (IDENTITY_PIN_SETTING, &pin),
                )?;
                transaction.execute(
                    "INSERT INTO audit (ts, actor_role, action, outcome, detail) \
                     VALUES (?1, ?2, ?3, 'ok', ?4)",
                    (
                        format_time(now),
                        actor.as_str(),
                        AuditAction::DevicesRevoked.as_str(),
                        serde_json::json!({
                            "devices": revoked,
                            "reason": "identity_key_changed",
                        })
                        .to_string(),
                    ),
                )?;
                Reconciled::Revoked(revoked)
            }
        };
        transaction.commit()?;
        Ok(outcome)
    }

    /// The underlying connection, for tests and for the console tool's
    /// read-only listings.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

/// A migration that happened while opening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migrated {
    /// Schema before.
    pub from: u32,
    /// Schema after.
    pub to: u32,
    /// The verified copy taken first.
    pub backup: PathBuf,
}

/// The result of [`open`].
#[derive(Debug)]
pub struct Opened {
    /// The database, at [`SCHEMA_VERSION`].
    pub database: Database,
    /// Set when a migration ran.
    pub migrated: Option<Migrated>,
    /// Backups removed by pruning.
    pub pruned: usize,
}

fn user_version(connection: &Connection) -> rusqlite::Result<u32> {
    connection.pragma_query_value(None, "user_version", |row| row.get(0))
}

/// Checks the state directory and every database file that exists.
fn check_state_tree(layout: &Layout) -> Result<bool, StateFault> {
    let euid = protect::euid();
    protect::check(layout.state_dir(), Expect::PrivateDirectory, euid)
        .map_err(StateFault::Unprotected)?;
    let database = layout.database();
    let exists = fsio::lstat(&database)
        .map_err(|error| StateFault::Unreadable(clip(error)))?
        .is_some();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let path = PathBuf::from(format!("{}{suffix}", database.display()));
        if fsio::lstat(&path)
            .map_err(|error| StateFault::Unreadable(clip(error)))?
            .is_some()
        {
            protect::check(&path, Expect::PrivateFile, euid).map_err(StateFault::Unprotected)?;
        }
    }
    Ok(exists)
}

fn connect(path: &Path) -> Result<Connection, StateFault> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(path, flags)
        .map_err(|error| StateFault::Unreadable(clip(error)))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| StateFault::Unreadable(clip(error)))?;
    Ok(connection)
}

/// `PRAGMA integrity_check`, requiring exactly `ok`.
fn integrity(connection: &Connection) -> Result<(), String> {
    let mut statement = connection.prepare("PRAGMA integrity_check").map_err(clip)?;
    let rows = statement
        .query_map((), |row| row.get::<_, String>(0))
        .map_err(clip)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(clip)?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        Err(clip(rows.join("; ")))
    }
}

/// The per-connection settings from plan section 4.3.
fn configure(connection: &Connection) -> rusqlite::Result<()> {
    let mode: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

fn apply(
    connection: &mut Connection,
    from: u32,
    migrations: &[Migration],
) -> Result<(), StateFault> {
    for migration in migrations.iter().filter(|m| m.version > from) {
        let failed = |error: rusqlite::Error| StateFault::MigrationFailed {
            version: migration.version,
            message: clip(error),
        };
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(failed)?;
        transaction.execute_batch(migration.sql).map_err(failed)?;
        transaction
            .pragma_update(None, "user_version", migration.version)
            .map_err(failed)?;
        transaction.commit().map_err(failed)?;
        tracing::info!(
            event = "state_migration_applied",
            component = "atrium-core",
            version = migration.version,
            name = migration.name,
            "applied a schema migration"
        );
    }
    Ok(())
}

/// Opens, verifies and, if needed, migrates the state database.
///
/// # Errors
///
/// A [`StateFault`]. On every fault the database is left as it was found.
pub fn open(layout: &Layout, now: OffsetDateTime) -> Result<Opened, StateFault> {
    open_with(layout, MIGRATIONS, now)
}

/// [`open`] with an explicit migration list, so tests can exercise a
/// migration that fails.
pub(crate) fn open_with(
    layout: &Layout,
    migrations: &[Migration],
    now: OffsetDateTime,
) -> Result<Opened, StateFault> {
    let target = migrations.last().map_or(0, |m| m.version);
    if !check_state_tree(layout)? {
        return Err(StateFault::Missing);
    }
    let path = layout.database();
    let mut connection = connect(&path)?;
    integrity(&connection).map_err(StateFault::Unreadable)?;
    let found = user_version(&connection).map_err(|error| StateFault::Unreadable(clip(error)))?;
    if found > target {
        return Err(StateFault::SchemaNewer { found });
    }

    // The copy comes before anything that writes, including the journal-mode
    // setting, so a failed backup leaves the database exactly as it was.
    let backup = if found < target {
        Some(backups::take(&connection, layout, found, target, now)?)
    } else {
        None
    };
    configure(&connection).map_err(|error| StateFault::Unreadable(clip(error)))?;

    let mut migrated = None;
    let mut pruned = 0;
    if let Some(backup) = backup {
        apply(&mut connection, found, migrations)?;
        let applied =
            user_version(&connection).map_err(|error| StateFault::Unreadable(clip(error)))?;
        if applied != target {
            return Err(StateFault::MigrationFailed {
                version: target,
                message: format!("schema is {applied} after migrating"),
            });
        }
        pruned = backups::prune(layout).unwrap_or(0);
        migrated = Some(Migrated {
            from: found,
            to: target,
            backup,
        });
    }

    let database = Database { connection, path };
    if let Some(step) = &migrated {
        database
            .audit(
                AuditAction::StateMigrated,
                Actor::System,
                &serde_json::json!({
                    "from": step.from,
                    "to": step.to,
                    "backup": step.backup.file_name().map(|n| n.to_string_lossy().into_owned()),
                }),
                now,
            )
            .map_err(|error| StateFault::Unreadable(clip(error)))?;
    }
    Ok(Opened {
        database,
        migrated,
        pruned,
    })
}

/// Creates the state database at installation, at [`SCHEMA_VERSION`].
///
/// # Errors
///
/// [`StateFault::AlreadyExists`] if any database file is present — an
/// existing database is never replaced — or the fault that stopped creation.
pub fn create(layout: &Layout) -> Result<Database, StateFault> {
    create_with(layout, MIGRATIONS)
}

pub(crate) fn create_with(
    layout: &Layout,
    migrations: &[Migration],
) -> Result<Database, StateFault> {
    if check_state_tree(layout)? {
        return Err(StateFault::AlreadyExists);
    }
    let path = layout.database();
    for suffix in ["-wal", "-shm", "-journal"] {
        if fsio::lstat(Path::new(&format!("{}{suffix}", path.display())))
            .map_err(|error| StateFault::Unreadable(clip(error)))?
            .is_some()
        {
            return Err(StateFault::AlreadyExists);
        }
    }

    // Created here, exclusively and with its final mode, so SQLite opens an
    // existing empty file rather than creating one under whatever umask the
    // installer happens to have. SQLite gives -wal and -shm the same mode.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                StateFault::AlreadyExists
            } else {
                StateFault::Unreadable(clip(error))
            }
        })?;
    file.sync_all()
        .map_err(|error| StateFault::Unreadable(clip(error)))?;
    drop(file);
    fsio::sync_dir(layout.state_dir()).map_err(|error| StateFault::Unreadable(clip(error)))?;

    let mut connection = connect(&path)?;
    configure(&connection).map_err(|error| StateFault::Unreadable(clip(error)))?;
    // Nothing to back up: the file was empty a moment ago.
    apply(&mut connection, 0, migrations)?;
    Ok(Database { connection, path })
}

/// Reports the state database's schema version without migrating or
/// configuring anything, for `atriumctl diagnostics`.
///
/// # Errors
///
/// The [`StateFault`] Core would report.
pub fn inspect(layout: &Layout) -> Result<u32, StateFault> {
    if !check_state_tree(layout)? {
        return Err(StateFault::Missing);
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let connection = Connection::open_with_flags(layout.database(), flags)
        .map_err(|error| StateFault::Unreadable(clip(error)))?;
    integrity(&connection).map_err(StateFault::Unreadable)?;
    let found = user_version(&connection).map_err(|error| StateFault::Unreadable(clip(error)))?;
    if found > SCHEMA_VERSION {
        return Err(StateFault::SchemaNewer { found });
    }
    Ok(found)
}

/// The file name used for a database, for the console tool's messages.
#[must_use]
pub fn file_name() -> &'static str {
    DATABASE_FILE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{chmod, Fixture};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time")
    }

    fn tables(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare");
        statement
            .query_map((), |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    fn columns(connection: &Connection, table: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!(
                "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
            ))
            .expect("prepare");
        statement
            .query_map((), |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    /// A database at schema 0 with one table of its own, standing in for a
    /// database written by an older release.
    fn seed_version_zero(fixture: &Fixture) {
        let path = fixture.layout.database();
        let connection = Connection::open(&path).expect("open");
        connection
            .execute_batch("CREATE TABLE legacy (x INTEGER); INSERT INTO legacy VALUES (42);")
            .expect("seed");
        drop(connection);
        chmod(&path, 0o600);
    }

    #[test]
    fn migrations_are_numbered_from_one_without_gaps() {
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version as usize, index + 1);
        }
        assert_eq!(MIGRATIONS.last().map(|m| m.version), Some(SCHEMA_VERSION));
    }

    #[test]
    fn migration_1_creates_exactly_the_planned_schema() {
        let fixture = Fixture::new("schema");
        let database = create(&fixture.layout).expect("created");
        let connection = database.connection();

        assert_eq!(
            tables(connection),
            [
                "audit",
                "devices",
                "pairing_state",
                "settings",
                "sqlite_sequence"
            ]
        );
        assert_eq!(
            columns(connection, "devices"),
            [
                "device_id",
                "name",
                "platform",
                "role",
                "token_digest",
                "created_at",
                "last_seen_at"
            ]
        );
        assert_eq!(
            columns(connection, "pairing_state"),
            [
                "id",
                "claimed",
                "armed",
                "secret_digest",
                "armed_at",
                "expires_at",
                "failures",
                "locked"
            ]
        );
        assert_eq!(
            columns(connection, "audit"),
            [
                "id",
                "ts",
                "request_id",
                "actor_device",
                "actor_role",
                "action",
                "target",
                "outcome",
                "error_code",
                "detail"
            ]
        );
        assert_eq!(columns(connection, "settings"), ["key", "value"]);
        assert_eq!(database.schema_version().expect("version"), SCHEMA_VERSION);

        let mode: String = connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode");
        assert_eq!(mode, "wal");
        let foreign_keys: i64 = connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("foreign keys");
        assert_eq!(foreign_keys, 1);
    }

    #[test]
    fn a_created_database_is_private() {
        let fixture = Fixture::new("private");
        let database = create(&fixture.layout).expect("created");
        database
            .audit(
                AuditAction::StateMigrated,
                Actor::System,
                &serde_json::json!({}),
                now(),
            )
            .expect("write");
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", fixture.layout.database().display()));
            if let Ok(metadata) = std::fs::metadata(&path) {
                assert_eq!(fsio::mode_of(&metadata), 0o600, "{}", path.display());
            }
        }
    }

    #[test]
    fn create_never_replaces_an_existing_database() {
        let fixture = Fixture::new("nocreate");
        seed_version_zero(&fixture);
        let before = fixture.snapshot();
        assert_eq!(
            create(&fixture.layout).map(|_| ()),
            Err(StateFault::AlreadyExists)
        );
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn a_missing_database_is_a_fault_and_is_not_created() {
        let fixture = Fixture::new("missing");
        assert_eq!(
            open(&fixture.layout, now()).map(|_| ()),
            Err(StateFault::Missing)
        );
        assert!(
            !fixture.layout.database().exists(),
            "no fresh database may appear"
        );
        assert!(fixture.snapshot().is_empty());
    }

    #[test]
    fn opening_a_current_database_changes_nothing_structural() {
        let fixture = Fixture::new("current");
        drop(create(&fixture.layout).expect("created"));
        let opened = open(&fixture.layout, now()).expect("opens");
        assert!(opened.migrated.is_none());
        assert!(
            !fixture.layout.backups_dir().exists(),
            "no backup without a migration"
        );
    }

    #[test]
    fn a_corrupt_database_is_reported_and_left_untouched() {
        let fixture = Fixture::new("corrupt");
        drop(create(&fixture.layout).expect("created"));
        let path = fixture.layout.database();
        // Checkpoint so the file holds the schema, then damage it mid-page.
        {
            let connection = Connection::open(&path).expect("open");
            connection
                .pragma_update(None, "wal_checkpoint", "TRUNCATE")
                .ok();
        }
        let mut bytes = std::fs::read(&path).expect("read");
        assert!(bytes.len() > 4096, "the schema occupies more than one page");
        bytes.truncate(4096 + 100);
        std::fs::write(&path, &bytes).expect("damage");
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let before = std::fs::read(&path).expect("read");

        let fault = open(&fixture.layout, now()).expect_err("must not open");
        assert!(matches!(fault, StateFault::Unreadable(_)), "{fault:?}");
        assert_eq!(
            std::fs::read(&path).expect("read"),
            before,
            "not repaired, not truncated"
        );
    }

    #[test]
    fn garbage_is_unreadable_not_replaced() {
        let fixture = Fixture::new("garbage");
        let path = fixture.layout.database();
        std::fs::write(&path, vec![0x5a_u8; 8192]).expect("garbage");
        chmod(&path, 0o600);
        let fault = open(&fixture.layout, now()).expect_err("must not open");
        assert!(matches!(fault, StateFault::Unreadable(_)), "{fault:?}");
        assert_eq!(std::fs::read(&path).expect("read"), vec![0x5a_u8; 8192]);
    }

    #[test]
    fn a_newer_schema_is_refused_not_downgraded() {
        let fixture = Fixture::new("newer");
        {
            let database = create(&fixture.layout).expect("created");
            database
                .connection()
                .pragma_update(None, "user_version", 99)
                .expect("bump");
        }
        assert_eq!(
            open(&fixture.layout, now()).map(|_| ()),
            Err(StateFault::SchemaNewer { found: 99 })
        );
        let connection = Connection::open(fixture.layout.database()).expect("open");
        assert_eq!(user_version(&connection).expect("version"), 99);
    }

    #[test]
    fn migration_takes_a_verified_private_backup_first() {
        let fixture = Fixture::new("backup");
        seed_version_zero(&fixture);
        let opened = open(&fixture.layout, now()).expect("migrates");
        let migrated = opened.migrated.expect("a migration ran");
        assert_eq!((migrated.from, migrated.to), (0, SCHEMA_VERSION));

        let name = migrated
            .backup
            .file_name()
            .and_then(|n| n.to_str())
            .expect("name");
        assert!(name.starts_with("atrium.db.pre-1-"), "{name}");
        let metadata = std::fs::metadata(&migrated.backup).expect("backup exists");
        assert_eq!(fsio::mode_of(&metadata), 0o600);
        let dir = std::fs::metadata(fixture.layout.backups_dir()).expect("dir");
        assert_eq!(fsio::mode_of(&dir), 0o700);

        let copy = Connection::open(&migrated.backup).expect("open backup");
        assert_eq!(user_version(&copy).expect("version"), 0);
        let kept: i64 = copy
            .query_row("SELECT x FROM legacy", (), |row| row.get(0))
            .expect("pre-migration data");
        assert_eq!(kept, 42);

        let action: String = opened
            .database
            .connection()
            .query_row(
                "SELECT action FROM audit ORDER BY id DESC LIMIT 1",
                (),
                |row| row.get(0),
            )
            .expect("audit row");
        assert_eq!(action, "state.migrated");
    }

    #[test]
    fn migration_backup_failure_aborts_migration() {
        let fixture = Fixture::new("nobackup");
        seed_version_zero(&fixture);
        // A file where the backup directory belongs: no backup can be written,
        // for root or anyone else.
        std::fs::write(fixture.layout.backups_dir(), b"in the way").expect("block");

        let fault = open(&fixture.layout, now()).expect_err("must not migrate");
        assert!(matches!(fault, StateFault::BackupFailed(_)), "{fault:?}");

        let connection = Connection::open(fixture.layout.database()).expect("open");
        assert_eq!(
            user_version(&connection).expect("version"),
            0,
            "not migrated"
        );
        assert_eq!(tables(&connection), ["legacy"], "schema untouched");
    }

    #[test]
    fn a_failed_migration_rolls_back_completely() {
        const BROKEN: &[Migration] = &[
            MIGRATIONS[0],
            Migration {
                version: 2,
                name: "broken",
                sql: "CREATE TABLE half_done (x INTEGER); THIS IS NOT SQL;",
            },
        ];
        let fixture = Fixture::new("rollback");
        drop(create(&fixture.layout).expect("created"));

        let fault = open_with(&fixture.layout, BROKEN, now()).expect_err("must fail");
        assert!(
            matches!(fault, StateFault::MigrationFailed { version: 2, .. }),
            "{fault:?}"
        );
        let connection = Connection::open(fixture.layout.database()).expect("open");
        assert_eq!(user_version(&connection).expect("version"), 1);
        assert!(!tables(&connection).contains(&"half_done".to_owned()));
        assert_eq!(
            backups::list(&fixture.layout).expect("list").len(),
            1,
            "backup kept"
        );

        // And the unchanged binary still opens it normally afterwards.
        assert!(open(&fixture.layout, now()).is_ok());
    }

    #[test]
    fn an_exposed_state_directory_is_refused() {
        let fixture = Fixture::new("exposed");
        drop(create(&fixture.layout).expect("created"));
        chmod(fixture.layout.state_dir(), 0o755);
        let fault = open(&fixture.layout, now()).expect_err("refused");
        assert!(matches!(fault, StateFault::Unprotected(_)), "{fault:?}");
    }

    #[test]
    fn a_symlinked_database_is_refused() {
        let fixture = Fixture::new("dblink");
        let elsewhere = fixture.layout.state_file("elsewhere.db");
        drop(Connection::open(&elsewhere).expect("create"));
        chmod(&elsewhere, 0o600);
        std::os::unix::fs::symlink(&elsewhere, fixture.layout.database()).expect("symlink");
        let fault = open(&fixture.layout, now()).expect_err("refused");
        assert!(matches!(fault, StateFault::Unprotected(_)), "{fault:?}");
    }

    #[test]
    fn a_changed_identity_key_revokes_every_device_in_one_audited_transaction() {
        use crate::identity::DeviceKey;
        let fixture = Fixture::new("reconcile");
        let mut database = create(&fixture.layout).expect("created");
        let first = DeviceKey::generate().expect("keygen").pin();
        let second = DeviceKey::generate().expect("keygen").pin();

        let reconcile = |database: &mut Database, pin| {
            database
                .reconcile_identity(pin, Actor::System, now())
                .expect("reconcile")
        };
        assert_eq!(reconcile(&mut database, &first), Reconciled::Recorded);
        database
            .connection()
            .execute(
                "INSERT INTO devices (device_id, name, platform, role, token_digest, created_at)                  VALUES ('00', 'laptop', 'linux', 'owner', x'00', '2026-01-01T00:00:00Z')",
                (),
            )
            .expect("seed device");
        assert_eq!(
            reconcile(&mut database, &first),
            Reconciled::Unchanged,
            "the same key revokes nothing"
        );
        assert_eq!(reconcile(&mut database, &second), Reconciled::Revoked(1));
        let remaining: i64 = database
            .connection()
            .query_row("SELECT count(*) FROM devices", (), |row| row.get(0))
            .expect("count");
        assert_eq!(remaining, 0);
        let action: String = database
            .connection()
            .query_row(
                "SELECT action FROM audit ORDER BY id DESC LIMIT 1",
                (),
                |row| row.get(0),
            )
            .expect("audit");
        assert_eq!(action, "identity.devices_revoked");
        assert_eq!(
            reconcile(&mut database, &second),
            Reconciled::Unchanged,
            "once reconciled, stays reconciled"
        );
    }
}
