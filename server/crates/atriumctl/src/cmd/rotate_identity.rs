//! `atriumctl rotate-identity` — the only way the identity key is replaced
//! (`docs/M1-IMPLEMENTATION-PLAN.md` section 5.5).
//!
//! Order:
//!
//! 1. Take the state lock (Core must be stopped), read `server_id` from the
//!    root-owned record, and ask for the server's name.
//! 2. As root: write the new key, then the record with the same `server_id`.
//! 3. Drop to the service user **for good**, and only then open the state
//!    database: revoke every device and disarm pairing, issue a certificate
//!    for the new key, and audit the rotation.
//!
//! Step 3 never runs with a way back to root, because the database is
//! writable by the service user and is therefore data a compromised Core
//! controls. Revocation is still guaranteed if step 3 is interrupted or
//! fails: the database records the pin its devices were paired against, and
//! Core revokes every device at its next start when the pin differs
//! (`Database::reconcile_identity`). A device paired against the old key
//! cannot survive either way.
//!
//! Step 3 ends by arming a fresh pairing secret (the plan's step 3) and
//! printing it, exactly as `atriumctl pair` does, so the owner can re-pair
//! against the new key straight away.

use std::process::ExitCode;

use atrium_core::certificate::{self, AddressSet, SubjectNames};
use atrium_core::config;
use atrium_core::db::{self, Actor, AuditAction, Reconciled};
use atrium_core::lock::{self, LockError};
use atrium_core::rotate::{self, Summary, Target};
use time::OffsetDateTime;

use super::{confirm, fail, Context};

const COMMAND: &str = "rotate-identity";

const WARNING: &str = "\
This replaces the server's identity key.

Every paired device will see a different key for the same server — which is
exactly what an impersonation attempt looks like — and will refuse to connect
until it is paired again. Every device is revoked. Re-pair only with a code
read from this server's own console.
";

/// Runs the command.
pub fn run() -> ExitCode {
    let context = match Context::resolve() {
        Ok(context) => context,
        Err(error) => return fail(COMMAND, error),
    };
    let layout = &context.layout;

    // Opening the directory to lock it reads nothing inside it.
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

    // Both root-owned, in the root-owned identity directory.
    let current = match rotate::current(layout) {
        Ok(current) => current,
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

    print!("{WARNING}");
    println!("server_id {} is kept.", current.record.server_id);
    if let Err(error) = confirm(&server_name) {
        return fail(COMMAND, error);
    }

    let target = match context.account {
        Some(account) => Target::installed(account.gid.as_raw()),
        None => Target {
            owner: None,
            mode: 0o600,
        },
    };
    let rotated = match rotate::replace_key(layout, &current, target) {
        Ok(rotated) => rotated,
        Err(error) => return fail(COMMAND, error),
    };
    let summary = Summary {
        server_id: rotated.server_id(),
        old_pin: current.key_pin,
        new_pin: rotated.pin(),
    };
    println!("identity key rotated");
    println!("server_id={}", summary.server_id);
    println!("spki_sha256={}", summary.new_pin);

    // From here on: the service user, with no way back.
    if let Err(error) = context.drop_to_service() {
        return fail(
            COMMAND,
            format!(
                "the key was rotated, but privileges could not be dropped ({error}); the \
                 state was not touched. Core revokes every device at its next start"
            ),
        );
    }

    let now = OffsetDateTime::now_utc();
    let mut failures = 0;
    let mut armed = false;
    match db::open(layout, now) {
        Ok(mut opened) => {
            match opened
                .database
                .reconcile_identity(&summary.new_pin, Actor::Console, now)
            {
                Ok(Reconciled::Revoked(count)) => {
                    println!("revoked {count} device(s); pairing disarmed");
                }
                Ok(Reconciled::Recorded | Reconciled::Unchanged) => {
                    println!("no devices were bound to the previous key");
                }
                Err(error) => {
                    failures += 1;
                    eprintln!(
                        "atriumctl {COMMAND}: devices could not be revoked now ({error}); Core \
                         revokes every device at its next start"
                    );
                }
            }
            if let Err(error) = opened.database.audit(
                AuditAction::KeyRotated,
                Actor::Console,
                &summary.detail(),
                now,
            ) {
                failures += 1;
                eprintln!("atriumctl {COMMAND}: the audit row could not be written: {error}");
            }
            println!();
            match super::pair::arm_and_show(layout, &mut opened.database, &summary.server_id) {
                Ok(()) => armed = true,
                Err(error) => {
                    failures += 1;
                    eprintln!(
                        "atriumctl {COMMAND}: {error}. Arm pairing with `atriumctl pair` once \
                         Core is running"
                    );
                }
            }
            println!();
        }
        Err(fault) => {
            failures += 1;
            eprintln!(
                "atriumctl {COMMAND}: the state database could not be opened ({fault}); Core \
                 will report it, and revokes every device once it is restored"
            );
        }
    }

    let addresses = AddressSet::current().unwrap_or_default();
    let names = SubjectNames::for_server(&rotated.server_id(), &addresses);
    match certificate::issue(&rotated, &names, now).and_then(|issued| {
        certificate::write(layout, &issued)?;
        Ok(issued)
    }) {
        Ok(issued) => println!("certificate reissued (serial {})", issued.record.serial),
        Err(error) => {
            failures += 1;
            eprintln!(
                "atriumctl {COMMAND}: the certificate was not reissued ({error}); Core \
                 reissues it at its next start"
            );
        }
    }

    if armed {
        println!("No device can connect until it is paired again, with the code above.");
    } else {
        println!("No device can connect until it is paired again: run `atriumctl pair`.");
    }
    println!("Start Core: systemctl start atrium-core");
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
