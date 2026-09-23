//! `atriumctl diagnostics` — what Core would find, from the console.
//!
//! Read-only, and run as the service user so it sees exactly what Core sees.
//! It shows identifiers (the server id, the pin, the certificate serial),
//! which are public; it never reads the private key's bytes into its output
//! and never prints `secrets.key`. Unlike the recovery payload Core will serve
//! on the network, this runs at the console, so it may name paths and give
//! SQLite's own message.

use std::process::ExitCode;

use atrium_core::certificate;
use atrium_core::config;
use atrium_core::db::{self, backups};
use atrium_core::fsio;
use atrium_core::identity::{self, Protection};
use atrium_core::layout::CERTIFICATE_FILE;
use atrium_core::recovery::RecoveryReason;

use super::{fail, Context};

/// Runs the command.
pub fn run() -> ExitCode {
    let context = match Context::resolve() {
        Ok(context) => context,
        Err(error) => return fail("diagnostics", error),
    };
    if let Err(error) = context.drop_to_service() {
        return fail("diagnostics", error);
    }
    let layout = &context.layout;

    println!("atriumctl {}", env!("CARGO_PKG_VERSION"));
    println!("identity directory: {}", layout.etc_dir().display());
    println!("state directory:    {}", layout.state_dir().display());

    match config::load(layout) {
        Ok(config) => println!("config:      ok (server_name = {:?})", config.server_name),
        Err(error) => println!("config:      error: {error}"),
    }

    // Core refuses an identity whose files are not protected, and so does
    // this report; but the identifiers are still worth showing next to the
    // finding, so a key that loads without the protection check is described.
    let identity = match identity::load(layout, Protection::Required) {
        Ok(identity) => {
            println!(
                "identity:    ok (server_id = {}, spki_sha256 = {})",
                identity.server_id(),
                identity.pin()
            );
            Some(identity)
        }
        Err(fault) => {
            println!(
                "identity:    {}: {fault}",
                RecoveryReason::from(&fault).code()
            );
            let unchecked = identity::load(layout, Protection::NotChecked).ok();
            if let Some(identity) = &unchecked {
                println!(
                    "             found server_id = {}, spki_sha256 = {}",
                    identity.server_id(),
                    identity.pin()
                );
            }
            unchecked
        }
    };

    match fsio::read_regular(&layout.state_file(CERTIFICATE_FILE), 64 * 1024) {
        Ok(pem) => match certificate::describe_pem(&pem) {
            Some(record) => {
                let matches = identity
                    .as_ref()
                    .is_some_and(|identity| identity.pin() == record.spki);
                println!(
                    "certificate: serial {}, valid until {}, {} names, {} addresses{}",
                    record.serial,
                    identity::format_time(record.not_after),
                    record.names.dns.len(),
                    record.names.ips.len(),
                    if matches {
                        ""
                    } else {
                        " (does not match the identity key)"
                    }
                );
            }
            None => println!("certificate: present but not a valid certificate"),
        },
        Err(error) => println!("certificate: {error}"),
    }

    match db::inspect(layout) {
        Ok(version) => println!("state:       ok (schema {version})"),
        Err(fault) => println!(
            "state:       {}: {fault}",
            RecoveryReason::from(&fault).code()
        ),
    }

    match backups::list(layout) {
        Ok(list) if list.is_empty() => println!("backups:     none"),
        Ok(list) => println!(
            "backups:     {} (newest {}); see `atriumctl restore --list`",
            list.len(),
            list[0].name
        ),
        Err(error) => println!("backups:     cannot be listed: {error}"),
    }

    ExitCode::SUCCESS
}
