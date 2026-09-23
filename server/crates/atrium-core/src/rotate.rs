//! Deliberate identity-key rotation — the mechanism behind `atriumctl
//! rotate-identity` (`docs/M1-IMPLEMENTATION-PLAN.md` section 5.5).
//!
//! It is the only code that replaces the identity key, and nothing reaches it
//! from Core or from the network: it writes into `/etc/atrium`, which only root
//! can do once installation has locked the directory down. `server_id` does not
//! change — it names the installation, not the key.
//!
//! Rotation needs `identity.json` to be readable, because that is where
//! `server_id` comes from; without it there is nothing to preserve and the
//! operator is looking at a reinstall, not a rotation. The old key, on the
//! other hand, may be missing or damaged: replacing it is the point. A crash
//! between writing the key and writing the record leaves a pin mismatch that
//! Core reports as `identity.inconsistent`, and running the rotation again
//! completes it.
//!
//! Revoking devices is not done here, and deliberately not done by anything
//! that still holds root: the state database is writable by the service user,
//! so it is data a compromised Core controls, and parsing it in a process
//! that can regain root would hand that data a path to root. The database
//! binds itself to the key instead (`db::Database::reconcile_identity`), so
//! whoever next opens it as the service user — `atriumctl` after dropping
//! privileges for good, or Core at its next start — revokes every device.

use std::fmt;

use time::OffsetDateTime;

use crate::fsio::{self, Owner, Spec};
use crate::identity::{self, DeviceKey, Identity, IdentityRecord, Protection, ServerId, SpkiPin};
use crate::layout::{Layout, IDENTITY_FILE, PRIVATE_KEY_FILE};
use crate::protect::{self, Expect};
use crate::redact::Redacted;

/// Upper bound on what is read.
const LIMIT: u64 = 4096;

/// Why rotation refused or failed.
#[derive(Debug)]
pub enum RotateError {
    /// `identity.json` is missing or unreadable, so `server_id` is unknown.
    NoRecord(String),
    /// The identity directory is not what the caller may write.
    Unprotected(String),
    /// Key generation failed.
    Generate,
    /// Writing failed; see the message for which file.
    Write(String),
}

impl fmt::Display for RotateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRecord(why) => write!(
                formatter,
                "identity.json cannot be used ({why}), so there is no server_id to preserve; \
                 rotation is not possible"
            ),
            Self::Unprotected(why) => {
                write!(formatter, "the identity directory is not safe: {why}")
            }
            Self::Generate => formatter.write_str("the new key pair could not be generated"),
            Self::Write(why) => write!(formatter, "{why}"),
        }
    }
}

impl std::error::Error for RotateError {}

/// The identity as rotation finds it.
#[derive(Debug)]
pub struct Current {
    /// The record, which supplies `server_id`.
    pub record: IdentityRecord,
    /// The pin of the key on disk, if it could be read.
    pub key_pin: Option<SpkiPin>,
}

/// Reads what rotation needs: the record, and the old key if it parses.
///
/// # Errors
///
/// [`RotateError::NoRecord`] when `identity.json` is unusable.
pub fn current(layout: &Layout) -> Result<Current, RotateError> {
    let bytes = fsio::read_regular(&layout.etc_file(IDENTITY_FILE), LIMIT)
        .map_err(|error| RotateError::NoRecord(error.to_string()))?;
    let record = IdentityRecord::parse(&bytes)
        .ok_or_else(|| RotateError::NoRecord("it is not a valid identity record".to_owned()))?;
    let key_pin = fsio::read_regular(&layout.etc_file(PRIVATE_KEY_FILE), LIMIT)
        .ok()
        .map(Redacted::new)
        .and_then(|der| DeviceKey::from_pkcs8(&der).ok())
        .map(|key| key.pin());
    Ok(Current { record, key_pin })
}

/// Where the new files go and who owns them.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    /// The owner and group for the new files; `None` keeps the caller's.
    pub owner: Option<Owner>,
    /// Their mode.
    pub mode: u32,
}

impl Target {
    /// The installed layout: `root:<group> 0640`.
    #[must_use]
    pub fn installed(service_gid: u32) -> Self {
        Self {
            owner: Some(Owner {
                uid: 0,
                gid: service_gid,
            }),
            mode: 0o640,
        }
    }
}

/// Generates a new key and writes it, then the record with the same
/// `server_id` and the new pin. Returns the new identity, read back from disk.
///
/// # Errors
///
/// A [`RotateError`]. If the key was written and the record was not, Core
/// reports `identity.inconsistent` until the rotation is run again.
pub fn replace_key(
    layout: &Layout,
    current: &Current,
    target: Target,
) -> Result<Identity, RotateError> {
    let euid = protect::euid();
    let expect = if euid == 0 {
        Expect::RootDirectory
    } else {
        Expect::PrivateDirectory
    };
    protect::check(layout.etc_dir(), expect, euid)
        .map_err(|violation| RotateError::Unprotected(violation.to_string()))?;

    let key = DeviceKey::generate().map_err(|_| RotateError::Generate)?;
    let record = IdentityRecord {
        server_id: current.record.server_id,
        created_at: current.record.created_at,
        spki: key.pin(),
    };
    let spec = Spec {
        mode: target.mode,
        owner: target.owner,
    };
    let dir = layout.etc_dir();
    fsio::replace(dir, PRIVATE_KEY_FILE, key.to_pkcs8().expose(), spec)
        .map_err(|error| RotateError::Write(format!("writing tls.key: {error}")))?;
    fsio::replace(dir, IDENTITY_FILE, &record.to_json(), spec).map_err(|error| {
        RotateError::Write(format!(
            "tls.key was replaced but identity.json could not be ({error}); run \
             rotate-identity again to complete the rotation"
        ))
    })?;

    remove_debris(layout);

    let reloaded = identity::load(layout, Protection::NotChecked).map_err(|fault| {
        RotateError::Write(format!("the rotated identity does not verify: {fault}"))
    })?;
    if reloaded.server_id() != current.record.server_id || reloaded.pin() != key.pin() {
        return Err(RotateError::Write(
            "the rotated identity read back differs from what was written".to_owned(),
        ));
    }
    Ok(reloaded)
}

/// Removes leftover temporaries from an interrupted write in the identity
/// directory. Only regular files with Atrium's temporary prefix, never
/// following a link; the caller owns the directory, and nothing else writes
/// there.
fn remove_debris(layout: &Layout) {
    let Ok(entries) = std::fs::read_dir(layout.etc_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let is_file = entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false);
        if is_file && crate::layout::is_temporary(&entry.file_name()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = fsio::sync_dir(layout.etc_dir());
}

/// The facts an audit row or a console message needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Unchanged.
    pub server_id: ServerId,
    /// The old key's pin, if it could be read.
    pub old_pin: Option<SpkiPin>,
    /// The new key's pin.
    pub new_pin: SpkiPin,
}

impl Summary {
    /// The audit detail: identifiers only.
    #[must_use]
    pub fn detail(&self) -> serde_json::Value {
        serde_json::json!({
            "server_id": self.server_id.to_string(),
            "old_spki_sha256": self.old_pin.map(|pin| pin.to_string()),
            "new_spki_sha256": self.new_pin.to_string(),
        })
    }
}

/// When the rotation happened, for callers that audit it.
#[must_use]
pub fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate::AddressSet;
    use crate::init::{self, InitOutcome};
    use crate::testutil::Fixture;

    fn initialized(tag: &str) -> (Fixture, ServerId, SpkiPin) {
        let fixture = Fixture::new(tag);
        match init::run(&fixture.layout, &AddressSet::default(), now()).expect("init") {
            InitOutcome::Created { server_id, pin, .. } => (fixture, server_id, pin),
            other => panic!("{other:?}"),
        }
    }

    fn own() -> Target {
        Target {
            owner: None,
            mode: 0o600,
        }
    }

    #[test]
    fn rotation_preserves_server_id_and_changes_spki() {
        let (fixture, server_id, pin) = initialized("rotate");
        let current = current(&fixture.layout).expect("readable");
        assert_eq!(current.key_pin, Some(pin));
        let rotated = replace_key(&fixture.layout, &current, own()).expect("rotated");
        assert_eq!(
            rotated.server_id(),
            server_id,
            "server_id names the installation"
        );
        assert_ne!(
            rotated.pin(),
            pin,
            "a rotation that kept the pin rotated nothing"
        );
        let loaded = identity::load(&fixture.layout, Protection::NotChecked).expect("consistent");
        assert_eq!(loaded.pin(), rotated.pin());
        assert_eq!(loaded.record.created_at, current.record.created_at);
    }

    #[test]
    fn rotation_completes_a_half_finished_rotation() {
        let (fixture, server_id, _) = initialized("half");
        // Simulate a crash after the key was written and before the record.
        let stray = DeviceKey::generate().expect("keygen");
        std::fs::write(
            fixture.layout.etc_file(PRIVATE_KEY_FILE),
            stray.to_pkcs8().expose(),
        )
        .expect("swap");
        assert!(identity::load(&fixture.layout, Protection::NotChecked).is_err());

        let current = current(&fixture.layout).expect("record still readable");
        let rotated = replace_key(&fixture.layout, &current, own()).expect("completes");
        assert_eq!(rotated.server_id(), server_id);
        assert!(identity::load(&fixture.layout, Protection::NotChecked).is_ok());
    }

    #[test]
    fn rotation_without_a_record_is_refused() {
        let (fixture, _, _) = initialized("norecord");
        std::fs::remove_file(fixture.layout.etc_file(IDENTITY_FILE)).expect("remove");
        let before = fixture.snapshot();
        assert!(matches!(
            current(&fixture.layout),
            Err(RotateError::NoRecord(_))
        ));
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn the_audit_detail_carries_only_identifiers() {
        let (fixture, _, _) = initialized("detail");
        let current = current(&fixture.layout).expect("readable");
        let rotated = replace_key(&fixture.layout, &current, own()).expect("rotated");
        let summary = Summary {
            server_id: rotated.server_id(),
            old_pin: current.key_pin,
            new_pin: rotated.pin(),
        };
        let detail = summary.detail();
        let keys: Vec<&String> = detail.as_object().expect("object").keys().collect();
        assert_eq!(keys, ["new_spki_sha256", "old_spki_sha256", "server_id"]);
        let key_hex = hex::encode(rotated.key.to_pkcs8().expose());
        assert!(!detail.to_string().contains(&key_hex[key_hex.len() - 32..]));
    }
}
