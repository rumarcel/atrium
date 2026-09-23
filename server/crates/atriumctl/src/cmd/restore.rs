//! `atriumctl restore` — the local, console-only way out of recovery mode.
//!
//! There is deliberately no network equivalent (`docs/SECURITY.md` section
//! 19): an unauthenticated remote restore would let anyone on the LAN roll the
//! server back, and an authenticated one would need a second copy of the
//! device verifiers outside the database. This command needs root at the
//! console instead, drops to the service user before it touches anything,
//! refuses while Core holds the state directory, and asks for the server's
//! name before it changes a thing.

use std::process::ExitCode;

use atrium_core::config;
use atrium_core::db::{self, backups, restore, Actor, AuditAction};
use atrium_core::lock::{self, LockError};
use time::OffsetDateTime;

use super::{confirm, fail, Context};

const COMMAND: &str = "restore";

fn prepare() -> Result<Context, String> {
    let context = Context::resolve()?;
    context.drop_to_service()?;
    Ok(context)
}

/// `restore --list`.
pub fn list() -> ExitCode {
    let context = match prepare() {
        Ok(context) => context,
        Err(error) => return fail(COMMAND, error),
    };
    let list = match backups::list(&context.layout) {
        Ok(list) => list,
        Err(error) => return fail(COMMAND, format!("cannot list backups: {error}")),
    };
    if list.is_empty() {
        println!("no backups in {}", context.layout.backups_dir().display());
        return ExitCode::SUCCESS;
    }
    println!("{:<40} {:>12}  {:<8} status", "name", "bytes", "schema");
    for backup in list {
        let (schema, status) = match restore::verify(&backup.path) {
            Ok(version) => (version.to_string(), "ok".to_owned()),
            Err(error) => ("?".to_owned(), error.to_string()),
        };
        println!(
            "{:<40} {:>12}  {:<8} {status}",
            backup.name, backup.size, schema
        );
    }
    ExitCode::SUCCESS
}

/// `restore --from <name>`.
pub fn from(name: &str) -> ExitCode {
    let context = match prepare() {
        Ok(context) => context,
        Err(error) => return fail(COMMAND, error),
    };
    let layout = &context.layout;

    let _lock = match lock::acquire(layout.state_dir()) {
        Ok(lock) => lock,
        Err(LockError::Busy) => {
            return fail(
                COMMAND,
                "atrium-core is running (it holds the state directory). Stop it first: \
                 systemctl stop atrium-core",
            )
        }
        Err(error) => return fail(COMMAND, error),
    };

    let server_name = match config::load(layout) {
        Ok(config) => config.server_name,
        Err(error) => {
            return fail(
                COMMAND,
                format!("the server name is needed to confirm, and {error}"),
            )
        }
    };

    let backup = match restore::find(layout, name) {
        Ok(backup) => backup,
        Err(error) => return fail(COMMAND, error),
    };
    let schema = match restore::verify(&backup.path) {
        Ok(schema) => schema,
        Err(error) => return fail(COMMAND, error),
    };

    println!("This replaces the state database with {}", backup.name);
    println!(
        "  taken {} at schema {schema}, {} bytes",
        backup.taken_at, backup.size
    );
    println!("The current database is kept, renamed to atrium.db.corrupt-<time>.");
    println!("Anything recorded after that backup — devices paired since, audit rows — is lost.");
    if let Err(error) = confirm(&server_name) {
        return fail(COMMAND, error);
    }

    let now = OffsetDateTime::now_utc();
    let restored = match restore::restore(layout, name, now) {
        Ok(restored) => restored,
        Err(error) => return fail(COMMAND, error),
    };
    for moved in &restored.moved_aside {
        println!("moved aside: {moved}");
    }

    // Open it the way Core will — which migrates it, behind a fresh backup,
    // if it predates this binary — and record what happened.
    match db::open(layout, now) {
        Ok(opened) => {
            let detail = serde_json::json!({
                "backup": restored.backup,
                "schema_version": restored.schema_version,
                "moved_aside": restored.moved_aside,
            });
            if let Err(error) =
                opened
                    .database
                    .audit(AuditAction::StateRestored, Actor::Console, &detail, now)
            {
                eprintln!("atriumctl {COMMAND}: restored, but the audit row failed: {error}");
            }
        }
        Err(fault) => {
            return fail(
                COMMAND,
                format!("the backup was installed but does not open: {fault}"),
            )
        }
    }

    println!(
        "restored {}. Start Core: systemctl start atrium-core",
        restored.backup
    );
    ExitCode::SUCCESS
}
