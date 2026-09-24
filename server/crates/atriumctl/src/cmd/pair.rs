//! `atriumctl pair` — arms pairing at the server's console (ADR-003 §3,
//! plan §6, ADR-019).
//!
//! It generates a 128-bit secret, seals it under `secrets.key` into the
//! state database for the native TLS-exporter profile only, and shows it
//! once, with its expiry. Arming replaces any secret already armed and
//! clears a failure lock; there is only ever one armed secret. Core need not
//! be stopped: it reads the armed state on every pairing request, and the
//! arming is one transaction.
//!
//! Nothing here needs root. Every file it reads — the identity record, the
//! key's public half via the record, `secrets.key`, the database — is
//! readable by the service user, so the command drops to it for good before
//! reading anything, and the database's `-wal` and `-shm` stay the
//! service's.
//!
//! The secret is printed and dropped. It is not logged, not written to any
//! file, and cannot be shown again: running the command again arms a new
//! one.

use std::process::ExitCode;

use atrium_core::db::{self, Database};
use atrium_core::identity::{self, Protection, ServerId};
use atrium_core::layout::Layout;
use atrium_core::pairing::{self, SECRET_LIFETIME};
use time::OffsetDateTime;

use super::{fail, Context};

const COMMAND: &str = "pair";

/// Runs the command.
pub fn run() -> ExitCode {
    let context = match Context::resolve() {
        Ok(context) => context,
        Err(error) => return fail(COMMAND, error),
    };
    if let Err(error) = context.drop_to_service() {
        return fail(COMMAND, error);
    }
    let layout = &context.layout;
    // Installed: the same ownership rules Core requires. An unprivileged
    // tree (tests, development) owns its files and skips them.
    let protection = if context.account.is_some() {
        Protection::Required
    } else {
        Protection::NotChecked
    };
    let identity = match identity::load(layout, protection) {
        Ok(identity) => identity,
        Err(fault) => {
            return fail(
                COMMAND,
                format!("the server identity cannot be used ({fault}); nothing was armed"),
            )
        }
    };
    let mut database = match db::attach(layout) {
        Ok(database) => database,
        Err(fault) => {
            return fail(
                COMMAND,
                format!("the state database cannot be used ({fault}); nothing was armed"),
            )
        }
    };
    match arm_and_show(layout, &mut database, &identity.server_id()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(COMMAND, error),
    }
}

/// Arms a fresh secret on `database` and prints it once. Shared with
/// `rotate-identity`, which arms right after revoking every device.
///
/// # Errors
///
/// A message saying what failed; nothing was armed.
pub fn arm_and_show(
    layout: &Layout,
    database: &mut Database,
    server_id: &ServerId,
) -> Result<(), String> {
    let secrets = identity::load_secrets_key(layout)
        .map_err(|fault| format!("secrets.key cannot be used ({fault}); nothing was armed"))?;
    let armed = pairing::arm(database, &secrets, server_id, OffsetDateTime::now_utc())
        .map_err(|error| format!("{error}; nothing was armed"))?;
    let minutes = SECRET_LIFETIME.whole_minutes();
    println!("Pairing is armed for Atrium's native clients.");
    println!();
    println!("    Pairing code:  {}", &*armed.secret.display_form());
    println!();
    println!(
        "It pairs one device, once, until {} ({minutes} minutes from now).",
        identity::format_time(armed.expires_at)
    );
    println!("Enter it in the Atrium client on the device you are pairing. Hyphens and");
    println!("case do not matter. It cannot be shown again; run `atriumctl pair` for a");
    println!("new one.");
    if armed.outcome.replaced {
        println!();
        println!("The previous pairing code was replaced and no longer works.");
    }
    if armed.outcome.unlocked {
        println!();
        println!("Pairing was locked after five failed attempts; arming unlocked it.");
    }
    Ok(())
}
