//! Deciding, at startup, whether Core runs normally or in recovery.
//!
//! The order is fixed: identity, then state, then certificate. Each step
//! either succeeds or produces a recovery reason, and the first reason wins.
//! Nothing in this module generates an identity or creates a database; the
//! only thing it may write is what it is allowed to regenerate — the public
//! certificate and its record — plus the audit row saying it did, and a
//! forward schema migration behind a verified backup.

use time::OffsetDateTime;

use crate::certificate::{self, AddressSet, CertificateRecord, Outcome};
use crate::db::{self, backups, Actor, AuditAction, Database, Reconciled, StateFault};
use crate::identity::{self, Identity, Protection};
use crate::layout::Layout;
use crate::recovery::{Recovery, RecoveryReason};

/// Core's running state in normal mode.
#[derive(Debug)]
pub struct Normal {
    /// The verified identity.
    pub identity: Identity,
    /// The open state database.
    pub database: Database,
    /// The certificate currently on disk.
    pub certificate: CertificateRecord,
}

/// What Core does after startup.
#[derive(Debug)]
pub enum Mode {
    /// Everything verified.
    Normal(Box<Normal>),
    /// Something could not be trusted; report and refuse.
    Recovery(Recovery),
}

fn enter_recovery(
    layout: &Layout,
    reason: RecoveryReason,
    detail: &str,
    schema_version: Option<u32>,
    now: OffsetDateTime,
) -> Mode {
    let backup_count = backups::list(layout).map(|list| list.len()).unwrap_or(0);
    tracing::error!(
        event = "recovery_entered",
        component = "atrium-core",
        reason = reason.code(),
        detail = detail,
        backups = backup_count,
        "atrium-core is entering recovery mode; nothing will be changed. Restore with \
         `atriumctl restore` at the server console"
    );
    Mode::Recovery(Recovery {
        reason,
        since: now,
        schema_version,
        backup_count,
    })
}

/// Verifies identity and state and decides the mode.
///
/// `addresses` is called only when a certificate check is reached.
pub fn prepare(
    layout: &Layout,
    now: OffsetDateTime,
    addresses: impl FnOnce() -> AddressSet,
) -> Mode {
    let identity = match identity::load(layout, Protection::Required) {
        Ok(identity) => identity,
        Err(fault) => {
            return enter_recovery(layout, (&fault).into(), &fault.to_string(), None, now);
        }
    };
    tracing::info!(
        event = "identity_loaded",
        component = "atrium-core",
        server_id = %identity.server_id(),
        spki_sha256 = %identity.pin(),
        "server identity verified"
    );

    let opened = match db::open(layout, now) {
        Ok(opened) => opened,
        Err(fault) => {
            if let StateFault::Unreadable(message) = &fault {
                // The raw SQLite message goes to the log and nowhere else.
                tracing::error!(
                    event = "state_database_unreadable",
                    component = "atrium-core",
                    sqlite_message = %message,
                    "the state database could not be opened or failed its integrity check; \
                     it has not been modified"
                );
            }
            let schema = match fault {
                StateFault::SchemaNewer { found } => Some(found),
                _ => None,
            };
            return enter_recovery(layout, (&fault).into(), &fault.to_string(), schema, now);
        }
    };
    if let Some(migrated) = &opened.migrated {
        tracing::info!(
            event = "state_migrated",
            component = "atrium-core",
            from = migrated.from,
            to = migrated.to,
            backup = %migrated.backup.display(),
            pruned = opened.pruned,
            "the state database was migrated after a verified backup"
        );
    }
    let mut database = opened.database;

    // Devices paired against another key never survive into this one.
    match database.reconcile_identity(&identity.pin(), Actor::System, now) {
        Ok(Reconciled::Revoked(devices)) => tracing::warn!(
            event = "devices_revoked",
            component = "atrium-core",
            devices = devices,
            reason = "identity_key_changed",
            "the identity key differs from the one the state was paired against; every              device was revoked and pairing disarmed"
        ),
        Ok(Reconciled::Recorded | Reconciled::Unchanged) => {}
        Err(error) => {
            return enter_recovery(
                layout,
                RecoveryReason::StateDatabaseUnreadable,
                &format!("the identity could not be reconciled with the state: {error}"),
                database.schema_version().ok(),
                now,
            );
        }
    }

    if identity::inventory(layout).is_ok_and(|inventory| inventory.leftovers) {
        tracing::warn!(
            event = "identity_directory_debris",
            component = "atrium-core",
            "a leftover temporary file is in the identity directory; the identity itself              verified. Remove it from the console (it cannot be removed by Core)"
        );
    }

    let addresses = addresses();
    let certificate = match certificate::ensure(layout, &identity, &addresses, now) {
        Ok(outcome) => settle(&database, outcome, now),
        Err(error) => {
            return enter_recovery(
                layout,
                RecoveryReason::CertificateUnavailable,
                &error.to_string(),
                database.schema_version().ok(),
                now,
            );
        }
    };

    Mode::Normal(Box::new(Normal {
        identity,
        database,
        certificate,
    }))
}

/// Logs and audits a certificate outcome, returning what is now on disk.
pub fn settle(database: &Database, outcome: Outcome, now: OffsetDateTime) -> CertificateRecord {
    match outcome {
        Outcome::Kept(record) => record,
        Outcome::Reissued { record, cause } => {
            tracing::info!(
                event = "certificate_reissued",
                component = "atrium-core",
                cause = cause.as_str(),
                serial = %record.serial,
                not_after = %identity::format_time(record.not_after),
                spki_sha256 = %record.spki,
                addresses = record.names.ips.len(),
                "issued a new certificate for the existing key; the pin is unchanged"
            );
            let detail = serde_json::json!({
                "cause": cause.as_str(),
                "serial": record.serial,
                "not_after": identity::format_time(record.not_after),
            });
            if let Err(error) = database.audit(
                AuditAction::CertificateReissued,
                Actor::System,
                &detail,
                now,
            ) {
                tracing::warn!(
                    event = "audit_write_failed",
                    component = "atrium-core",
                    action = AuditAction::CertificateReissued.as_str(),
                    reason = %error,
                    "could not record the reissue in the audit log"
                );
            }
            record
        }
        Outcome::KeptAfterFailure {
            record,
            cause,
            error,
        } => {
            tracing::warn!(
                event = "certificate_reissue_failed",
                component = "atrium-core",
                cause = cause.as_str(),
                reason = %error,
                serial = %record.serial,
                "could not reissue the certificate; the current one is still valid and stays \
                 in service"
            );
            record
        }
    }
}
