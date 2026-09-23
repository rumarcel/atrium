//! Recovery mode.
//!
//! When the identity or the state database cannot be trusted, Core does not
//! exit (systemd would restart it into the same failure, over and over) and
//! does not repair anything. It stays up, says clearly what is wrong, and does
//! nothing else (`docs/M1-IMPLEMENTATION-PLAN.md` section 4.6):
//!
//! - no state-changing operation of any kind, local or remote;
//! - no pairing;
//! - no Agent: recovery holds no Agent client and has no code path that
//!   could construct one;
//! - no fresh identity and no fresh database;
//! - **no second authentication store.** There is no copy of the device
//!   verifiers anywhere else, so there is nothing recovery could
//!   authenticate against, and it does not try.
//!
//! Restore is a console action, `atriumctl restore`, with physical or
//! administrative access to the machine.
//!
//! M1B has no network listener, so recovery is reported through the log and
//! the service manager's status line. The payloads the HTTP API will serve in
//! recovery from M1D — `/healthz` and the redacted diagnostics — are defined
//! here, now, so their field allowlist is fixed and tested before any route
//! exists to serve them.

use serde::Serialize;
use time::OffsetDateTime;

use crate::db::StateFault;
use crate::identity::{format_time, IdentityFault};
use crate::VERSION;

/// Why Core is in recovery mode. A closed set: each has a stable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    /// No identity exists.
    IdentityMissing,
    /// The identity is partial or does not agree with itself.
    IdentityInconsistent,
    /// The identity's ownership or modes break the guarantees.
    IdentityUnprotected,
    /// An identity file cannot be read by the service user.
    IdentityUnreadable,
    /// The state directory or database has the wrong owner, mode or type.
    StateUnprotected,
    /// The state database does not exist.
    StateMissing,
    /// The state database cannot be opened or failed its integrity check.
    StateDatabaseUnreadable,
    /// The state database's schema is newer than this binary.
    StateSchemaNewer,
    /// A migration was needed and the backup before it failed.
    StateBackupFailed,
    /// A migration failed and was rolled back.
    StateMigrationFailed,
    /// No usable certificate could be produced from the identity key.
    CertificateUnavailable,
}

impl RecoveryReason {
    /// Every reason, for tests that must cover the whole set.
    pub const ALL: [Self; 11] = [
        Self::IdentityMissing,
        Self::IdentityInconsistent,
        Self::IdentityUnprotected,
        Self::IdentityUnreadable,
        Self::StateUnprotected,
        Self::StateMissing,
        Self::StateDatabaseUnreadable,
        Self::StateSchemaNewer,
        Self::StateBackupFailed,
        Self::StateMigrationFailed,
        Self::CertificateUnavailable,
    ];

    /// The stable code, as logged and as the API will report it.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::IdentityMissing => "identity.missing",
            Self::IdentityInconsistent => "identity.inconsistent",
            Self::IdentityUnprotected => "identity.unprotected",
            Self::IdentityUnreadable => "identity.unreadable",
            Self::StateUnprotected => "state.unprotected",
            Self::StateMissing => "state.missing",
            Self::StateDatabaseUnreadable => "state.database_unreadable",
            Self::StateSchemaNewer => "state.schema_newer",
            Self::StateBackupFailed => "state.backup_failed",
            Self::StateMigrationFailed => "state.migration_failed",
            Self::CertificateUnavailable => "certificate.unavailable",
        }
    }
}

impl From<&IdentityFault> for RecoveryReason {
    fn from(fault: &IdentityFault) -> Self {
        match fault {
            IdentityFault::Missing => Self::IdentityMissing,
            IdentityFault::Inconsistent(_) => Self::IdentityInconsistent,
            IdentityFault::Unprotected(_) => Self::IdentityUnprotected,
            IdentityFault::Unreadable(_) => Self::IdentityUnreadable,
        }
    }
}

impl From<&StateFault> for RecoveryReason {
    fn from(fault: &StateFault) -> Self {
        match fault {
            StateFault::Unprotected(_) => Self::StateUnprotected,
            StateFault::Missing => Self::StateMissing,
            // AlreadyExists comes only from creation, which Core never does;
            // were it ever reported here, "unreadable" is the safe reading.
            StateFault::Unreadable(_) | StateFault::AlreadyExists => Self::StateDatabaseUnreadable,
            StateFault::SchemaNewer { .. } => Self::StateSchemaNewer,
            StateFault::BackupFailed(_) => Self::StateBackupFailed,
            StateFault::MigrationFailed { .. } => Self::StateMigrationFailed,
        }
    }
}

/// Everything recovery mode knows. Deliberately small.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// Why.
    pub reason: RecoveryReason,
    /// Since when.
    pub since: OffsetDateTime,
    /// The schema version found, when it could be read.
    pub schema_version: Option<u32>,
    /// How many pre-migration backups exist.
    pub backup_count: usize,
}

/// `GET /healthz` in recovery. Unauthenticated.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct HealthReport {
    /// Always `"recovery"`.
    pub state: &'static str,
    /// The reason code.
    pub reason: &'static str,
    /// RFC 3339.
    pub since: String,
}

/// `GET /api/v1/system/diagnostics` in recovery. Unauthenticated, because it
/// cannot be authenticated, and therefore cut down to what is safe to hand
/// anyone on the LAN: no hostname, no addresses, no identifiers, no device
/// data, no audit content, no file paths, no SQLite message.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DiagnosticsReport {
    /// Always `"recovery"`.
    pub state: &'static str,
    /// The reason code.
    pub reason: &'static str,
    /// RFC 3339.
    pub since: String,
    /// Component versions.
    pub versions: Versions,
    /// The schema version found, if it could be read.
    pub schema_version: Option<u32>,
    /// Whether a restore is possible, and from how many copies.
    pub backups: BackupSummary,
}

/// Versions of the components recovery can speak for.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Versions {
    /// This binary.
    pub core: &'static str,
}

/// Backup availability, without names or paths.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct BackupSummary {
    /// At least one exists.
    pub available: bool,
    /// How many.
    pub count: usize,
}

impl Recovery {
    /// The health payload.
    #[must_use]
    pub fn health(&self) -> HealthReport {
        HealthReport {
            state: "recovery",
            reason: self.reason.code(),
            since: format_time(self.since),
        }
    }

    /// The redacted diagnostics payload.
    #[must_use]
    pub fn diagnostics(&self) -> DiagnosticsReport {
        DiagnosticsReport {
            state: "recovery",
            reason: self.reason.code(),
            since: format_time(self.since),
            versions: Versions { core: VERSION },
            schema_version: self.schema_version,
            backups: BackupSummary {
                available: self.backup_count > 0,
                count: self.backup_count,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    }

    fn sample(reason: RecoveryReason) -> Recovery {
        Recovery {
            reason,
            since: OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time"),
            schema_version: Some(1),
            backup_count: 2,
        }
    }

    #[test]
    fn reason_codes_are_stable_and_distinct() {
        let codes: Vec<&str> = RecoveryReason::ALL.iter().map(|r| r.code()).collect();
        assert_eq!(
            codes,
            [
                "identity.missing",
                "identity.inconsistent",
                "identity.unprotected",
                "identity.unreadable",
                "state.unprotected",
                "state.missing",
                "state.database_unreadable",
                "state.schema_newer",
                "state.backup_failed",
                "state.migration_failed",
                "certificate.unavailable",
            ]
        );
        let unique: BTreeSet<&str> = codes.iter().copied().collect();
        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn health_payload_is_exactly_state_reason_since() {
        let value = serde_json::to_value(sample(RecoveryReason::StateDatabaseUnreadable).health())
            .expect("serialises");
        assert_eq!(
            keys(&value),
            ["reason", "since", "state"].map(String::from).into()
        );
        assert_eq!(value["state"], "recovery");
        assert_eq!(value["reason"], "state.database_unreadable");
    }

    #[test]
    fn recovery_diagnostics_field_allowlist() {
        // A new field fails this test until someone has decided it is safe to
        // hand to an unauthenticated caller on the LAN.
        let value = serde_json::to_value(sample(RecoveryReason::IdentityMissing).diagnostics())
            .expect("serialises");
        assert_eq!(
            keys(&value),
            [
                "backups",
                "reason",
                "schema_version",
                "since",
                "state",
                "versions"
            ]
            .map(String::from)
            .into()
        );
        assert_eq!(keys(&value["versions"]), ["core"].map(String::from).into());
        assert_eq!(
            keys(&value["backups"]),
            ["available", "count"].map(String::from).into()
        );
        assert_eq!(value["backups"]["count"], 2);
        assert_eq!(value["backups"]["available"], true);

        // No identifier, address or path of any kind. The sample's own values
        // are a reason code, a timestamp and a version, none of which contain
        // these, so any hit is a leak.
        let text = value.to_string().to_lowercase();
        for forbidden in [
            "host",
            "addr",
            "ip",
            "device",
            "audit",
            "server_id",
            "spki",
            "/",
        ] {
            assert!(
                !text.contains(forbidden),
                "diagnostics mention {forbidden:?}: {text}"
            );
        }
    }

    #[test]
    fn every_fault_maps_to_a_reason() {
        use crate::identity::Inconsistency;
        assert_eq!(
            RecoveryReason::from(&IdentityFault::Missing),
            RecoveryReason::IdentityMissing
        );
        assert_eq!(
            RecoveryReason::from(&IdentityFault::Inconsistent(Inconsistency::KeyMalformed)),
            RecoveryReason::IdentityInconsistent
        );
        assert_eq!(
            RecoveryReason::from(&StateFault::Unreadable("x".into())),
            RecoveryReason::StateDatabaseUnreadable
        );
        assert_eq!(
            RecoveryReason::from(&StateFault::SchemaNewer { found: 9 }),
            RecoveryReason::StateSchemaNewer
        );
        assert_eq!(
            RecoveryReason::from(&StateFault::BackupFailed("x".into())),
            RecoveryReason::StateBackupFailed
        );
    }
}
