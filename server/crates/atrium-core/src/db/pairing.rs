//! Pairing and device rows: every read and write M1E makes, each one
//! transactional where it must be.
//!
//! The authoritative transition — consume the armed secret, create the
//! device, mark the server claimed, write the audit rows — happens in one
//! `IMMEDIATE` transaction ([`PairingTx::consume`] then [`PairingTx::commit`]).
//! A crash before the commit leaves the arming untouched and no device; a
//! crash after it leaves the secret gone and the device present. There is no
//! state in which a device exists and its secret is still usable, or the
//! server is claimed without the device that claimed it. The schema's CHECKs
//! (migrations 2 and 3) make the half-states unrepresentable as well.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::{Actor, AuditAction, Database};
use crate::identity::format_time;

/// The only profile M1 arms.
pub const NATIVE_PROFILE: &str = "atrium-pair-binding/native-tls-exporter-v1";

/// An armed secret as stored: sealed, never plaintext.
#[derive(Clone)]
pub struct ArmedRow {
    /// Names this arming; part of the seal's associated data.
    pub arming_id: [u8; 16],
    /// Always [`NATIVE_PROFILE`] in M1 (the schema refuses anything else).
    pub profile: String,
    /// XChaCha20-Poly1305 nonce.
    pub nonce: [u8; 24],
    /// Ciphertext and tag, 32 bytes.
    pub ciphertext: Vec<u8>,
    /// When it was armed.
    pub armed_at: OffsetDateTime,
    /// When it stops being usable.
    pub expires_at: OffsetDateTime,
}

impl std::fmt::Debug for ArmedRow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArmedRow")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// `pairing_state`.
#[derive(Debug, Clone)]
pub struct PairingRow {
    /// A device has been paired at least once.
    pub claimed: bool,
    /// Failed `complete`s against the current arming: telemetry for the
    /// audit row that ends the arming, never a decision input (ADR-020).
    pub failed_attempts: u32,
    /// The armed secret, if any.
    pub armed: Option<ArmedRow>,
}

/// A device about to be created.
pub struct NewDevice<'a> {
    /// 32 lowercase hex.
    pub device_id: &'a str,
    /// Validated display name.
    pub name: &'a str,
    /// Closed platform value.
    pub platform: &'a str,
    /// `SHA-256(token)`.
    pub token_digest: &'a [u8; 32],
}

/// A device as stored, without its verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// 32 lowercase hex.
    pub device_id: String,
    /// Display name.
    pub name: String,
    /// Platform.
    pub platform: String,
    /// Always `owner`.
    pub role: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339, updated at most once a minute.
    pub last_seen_at: Option<String>,
}

fn parse_time(text: &str) -> rusqlite::Result<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn blob<const N: usize>(bytes: Vec<u8>) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn read_row(connection: &rusqlite::Connection) -> rusqlite::Result<PairingRow> {
    connection.query_row(
        "SELECT claimed, failed_attempts, armed, arming_id, profile, secret_nonce, \
         secret_ciphertext, armed_at, expires_at FROM pairing_state WHERE id = 1",
        (),
        |row| {
            let armed: bool = row.get(2)?;
            let armed = if armed {
                Some(ArmedRow {
                    arming_id: blob(row.get(3)?)?,
                    profile: row.get(4)?,
                    nonce: blob(row.get(5)?)?,
                    ciphertext: row.get(6)?,
                    armed_at: parse_time(&row.get::<_, String>(7)?)?,
                    expires_at: parse_time(&row.get::<_, String>(8)?)?,
                })
            } else {
                None
            };
            Ok(PairingRow {
                claimed: row.get(0)?,
                failed_attempts: row.get(1)?,
                armed,
            })
        },
    )
}

const DISARM: &str = "UPDATE pairing_state SET armed = 0, arming_id = NULL, profile = NULL, \
     secret_nonce = NULL, secret_ciphertext = NULL, armed_at = NULL, expires_at = NULL, \
     failed_attempts = 0";

/// One audit row written inside a pairing or device transaction.
struct Entry<'a> {
    action: AuditAction,
    actor: &'a str,
    actor_device: Option<&'a str>,
    target: Option<&'a str>,
    outcome: &'a str,
    detail: serde_json::Value,
}

fn audit(
    transaction: &Transaction<'_>,
    entry: &Entry<'_>,
    now: OffsetDateTime,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO audit (ts, actor_role, actor_device, action, target, outcome, detail) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            format_time(now),
            entry.actor,
            entry.actor_device,
            entry.action.as_str(),
            entry.target,
            entry.outcome,
            entry.detail.to_string(),
        ),
    )?;
    Ok(())
}

/// How arming changed the state it replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Armed {
    /// An earlier, unexpired secret was replaced and is now useless.
    pub replaced: bool,
}

impl Database {
    /// Reads `pairing_state`.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn pairing_row(&self) -> rusqlite::Result<PairingRow> {
        read_row(&self.connection)
    }

    /// Arms pairing with a sealed secret, replacing whatever was armed, in
    /// one transaction with its audit row. The row records how many failed
    /// attempts the replaced arming had seen.
    ///
    /// # Errors
    ///
    /// SQLite's error; nothing is changed.
    pub fn arm_pairing(
        &mut self,
        armed: &ArmedRow,
        actor: Actor,
        now: OffsetDateTime,
    ) -> rusqlite::Result<Armed> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = read_row(&transaction)?;
        transaction.execute(
            "UPDATE pairing_state SET armed = 1, arming_id = ?1, profile = ?2, \
             secret_nonce = ?3, secret_ciphertext = ?4, armed_at = ?5, expires_at = ?6, \
             failed_attempts = 0 WHERE id = 1",
            (
                armed.arming_id.as_slice(),
                armed.profile.as_str(),
                armed.nonce.as_slice(),
                armed.ciphertext.as_slice(),
                format_time(armed.armed_at),
                format_time(armed.expires_at),
            ),
        )?;
        let outcome = Armed {
            replaced: before.armed.as_ref().is_some_and(|a| a.expires_at > now),
        };
        audit(
            &transaction,
            &Entry {
                action: AuditAction::PairingArmed,
                actor: actor.as_str(),
                actor_device: None,
                target: None,
                outcome: "ok",
                detail: serde_json::json!({
                    "profile": armed.profile,
                    "expires_at": format_time(armed.expires_at),
                    "replaced": outcome.replaced,
                    "replaced_failed_attempts": before.failed_attempts,
                }),
            },
            now,
        )?;
        transaction.commit()?;
        Ok(outcome)
    }

    /// Opens the pairing transaction `complete` runs in.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn pairing_transaction(&mut self) -> rusqlite::Result<PairingTx<'_>> {
        Ok(PairingTx(self.connection.transaction_with_behavior(
            TransactionBehavior::Immediate,
        )?))
    }

    /// Disarms an arming that has expired, deleting its ciphertext. Returns
    /// whether anything changed.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn disarm_if_expired(&mut self, now: OffsetDateTime) -> rusqlite::Result<bool> {
        let transaction = PairingTx(
            self.connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?,
        );
        let expired = transaction.disarm_if_expired(now)?;
        transaction.commit()?;
        Ok(expired)
    }

    /// The device whose verifier is `digest`, if any.
    ///
    /// The lookup uses the unique index; the digest found is then compared
    /// with the presented one in constant time, so equality is never decided
    /// by a short-circuiting comparison in Core.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn device_by_digest(
        &self,
        digest: &atrium_pairing::token::TokenDigest,
    ) -> rusqlite::Result<Option<DeviceRow>> {
        let found = self
            .connection
            .query_row(
                "SELECT device_id, name, platform, role, created_at, last_seen_at, token_digest \
                 FROM devices WHERE token_digest = ?1",
                [digest.as_bytes().as_slice()],
                |row| {
                    Ok((
                        DeviceRow {
                            device_id: row.get(0)?,
                            name: row.get(1)?,
                            platform: row.get(2)?,
                            role: row.get(3)?,
                            created_at: row.get(4)?,
                            last_seen_at: row.get(5)?,
                        },
                        row.get::<_, Vec<u8>>(6)?,
                    ))
                },
            )
            .optional()?;
        Ok(found.and_then(|(device, stored)| {
            let stored: [u8; 32] = stored.try_into().ok()?;
            atrium_pairing::token::TokenDigest::from_bytes(stored)
                .ct_eq(digest)
                .then_some(device)
        }))
    }

    /// Records use, at most once a minute per device.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn touch_device(&self, device_id: &str, now: OffsetDateTime) -> rusqlite::Result<()> {
        let cutoff = format_time(now - time::Duration::minutes(1));
        self.connection.execute(
            "UPDATE devices SET last_seen_at = ?2 WHERE device_id = ?1 \
             AND (last_seen_at IS NULL OR last_seen_at < ?3)",
            (device_id, format_time(now), cutoff),
        )?;
        Ok(())
    }

    /// Every device, oldest first, without verifiers.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn list_devices(&self) -> rusqlite::Result<Vec<DeviceRow>> {
        let mut statement = self.connection.prepare(
            "SELECT device_id, name, platform, role, created_at, last_seen_at FROM devices \
             ORDER BY created_at, device_id",
        )?;
        let rows = statement.query_map((), |row| {
            Ok(DeviceRow {
                device_id: row.get(0)?,
                name: row.get(1)?,
                platform: row.get(2)?,
                role: row.get(3)?,
                created_at: row.get(4)?,
                last_seen_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Revokes one device: deletes its row, verifier included, and audits
    /// it, in one transaction. Returns whether the device existed. There is
    /// no cache to invalidate; the row's absence is the revocation.
    ///
    /// # Errors
    ///
    /// SQLite's error; nothing is changed.
    pub fn revoke_device(
        &mut self,
        device_id: &str,
        actor_device: &str,
        now: OffsetDateTime,
    ) -> rusqlite::Result<bool> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed =
            transaction.execute("DELETE FROM devices WHERE device_id = ?1", [device_id])?;
        if removed == 1 {
            audit(
                &transaction,
                &Entry {
                    action: AuditAction::DeviceRevoked,
                    actor: "owner",
                    actor_device: Some(actor_device),
                    target: Some(device_id),
                    outcome: "ok",
                    detail: serde_json::json!({ "self": device_id == actor_device }),
                },
                now,
            )?;
        }
        transaction.commit()?;
        Ok(removed == 1)
    }
}

/// The transaction `pair/complete` decides in.
pub struct PairingTx<'a>(Transaction<'a>);

impl PairingTx<'_> {
    /// The state as this transaction sees it.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn row(&self) -> rusqlite::Result<PairingRow> {
        read_row(&self.0)
    }

    /// Deletes an expired arming's ciphertext. Returns whether it did.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn disarm_if_expired(&self, now: OffsetDateTime) -> rusqlite::Result<bool> {
        let row = self.row()?;
        match row.armed {
            Some(armed) if armed.expires_at <= now => {
                self.0.execute(DISARM, ())?;
                audit(
                    &self.0,
                    &Entry {
                        action: AuditAction::PairingExpired,
                        actor: Actor::System.as_str(),
                        actor_device: None,
                        target: None,
                        outcome: "ok",
                        detail: serde_json::json!({
                            "expires_at": format_time(armed.expires_at),
                            "failed_attempts": row.failed_attempts,
                        }),
                    },
                    now,
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Counts a failed `complete` against the current arming (ADR-020): the
    /// count is telemetry for the audit row that ends the arming and decides
    /// nothing. The armed secret is untouched. Returns the new count; zero
    /// when nothing is armed.
    ///
    /// # Errors
    ///
    /// SQLite's error.
    pub fn record_failure(&self) -> rusqlite::Result<u32> {
        let updated = self.0.execute(
            "UPDATE pairing_state SET failed_attempts = failed_attempts + 1 \
             WHERE id = 1 AND armed = 1",
            (),
        )?;
        if updated == 0 {
            return Ok(0);
        }
        Ok(self.row()?.failed_attempts)
    }

    /// Consumes the armed secret and creates the device, claiming the server
    /// if it was unclaimed. Returns whether this device claimed it.
    ///
    /// # Errors
    ///
    /// SQLite's error; with the transaction dropped, nothing happened.
    pub fn consume(&self, device: &NewDevice<'_>, now: OffsetDateTime) -> rusqlite::Result<bool> {
        let row = self.row()?;
        let first = !row.claimed;
        let failed_attempts = row.failed_attempts;
        self.0.execute(
            "INSERT INTO devices (device_id, name, platform, role, token_digest, created_at) \
             VALUES (?1, ?2, ?3, 'owner', ?4, ?5)",
            (
                device.device_id,
                device.name,
                device.platform,
                device.token_digest.as_slice(),
                format_time(now),
            ),
        )?;
        self.0.execute(DISARM, ())?;
        self.0
            .execute("UPDATE pairing_state SET claimed = 1 WHERE id = 1", ())?;
        audit(
            &self.0,
            &Entry {
                action: AuditAction::PairingConsumed,
                actor: Actor::System.as_str(),
                actor_device: None,
                target: Some(device.device_id),
                outcome: "ok",
                detail: serde_json::json!({
                    "first_claim": first,
                    "failed_attempts": failed_attempts,
                }),
            },
            now,
        )?;
        audit(
            &self.0,
            &Entry {
                action: AuditAction::DeviceCreated,
                actor: Actor::System.as_str(),
                actor_device: None,
                target: Some(device.device_id),
                outcome: "ok",
                detail: serde_json::json!({ "platform": device.platform, "role": "owner" }),
            },
            now,
        )?;
        Ok(first)
    }

    /// Commits.
    ///
    /// # Errors
    ///
    /// SQLite's error; nothing is changed.
    pub fn commit(self) -> rusqlite::Result<()> {
        self.0.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{SecretsKey, ServerId, SpkiPin};
    use crate::testutil::Fixture;
    use atrium_pairing::token::TokenDigest;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_900_000_000).expect("time")
    }

    fn setup(tag: &str) -> (Fixture, Database) {
        let fixture = Fixture::new(tag);
        let database = super::super::create(&fixture.layout).expect("database");
        (fixture, database)
    }

    fn device<'a>(id: &'a str, digest: &'a [u8; 32]) -> NewDevice<'a> {
        NewDevice {
            device_id: id,
            name: "Desk",
            platform: "linux",
            token_digest: digest,
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// ADR-019: the copy taken before the next migration holds only the
    /// ciphertext, like the database it copies.
    #[test]
    fn a_backup_of_an_armed_database_holds_no_plaintext_secret() {
        let (fixture, mut database) = setup("backup-secret");
        let secrets = SecretsKey::from_bytes([3; 32]);
        let server_id = ServerId::parse("00112233445566778899aabbccddeeff").expect("id");
        let armed = crate::pairing::arm(&mut database, &secrets, &server_id, now()).expect("arm");
        let path = super::super::backups::take(
            &database.connection,
            &fixture.layout,
            super::super::SCHEMA_VERSION,
            super::super::SCHEMA_VERSION + 1,
            now(),
        )
        .expect("backup");
        let copy = std::fs::read(&path).expect("read backup");
        let ciphertext: Vec<u8> = database
            .connection
            .query_row("SELECT secret_ciphertext FROM pairing_state", (), |r| {
                r.get(0)
            })
            .expect("ciphertext");
        assert!(
            contains(&copy, &ciphertext),
            "the backup is of the armed state"
        );
        let secret = &armed.secret;
        for form in [
            secret.expose().to_vec(),
            hex::encode(secret.expose()).into_bytes(),
            hex::encode_upper(secret.expose()).into_bytes(),
            secret.canonical().as_bytes().to_vec(),
            secret.display_form().as_bytes().to_vec(),
        ] {
            assert!(!contains(&copy, &form));
        }
    }

    /// M1B's rotation rule holds for M1E's tables: a new key revokes every
    /// device and destroys any armed secret, in one transaction.
    #[test]
    fn an_identity_key_change_revokes_every_device_and_the_armed_secret() {
        let (_fixture, mut database) = setup("rotation");
        let old = SpkiPin::of(b"old key");
        database
            .reconcile_identity(&old, Actor::System, now())
            .expect("record");
        for (id, digest) in [
            ("00000000000000000000000000000001", [1_u8; 32]),
            ("00000000000000000000000000000002", [2_u8; 32]),
        ] {
            let transaction = database.pairing_transaction().expect("tx");
            transaction
                .consume(&device(id, &digest), now())
                .expect("consume");
            transaction.commit().expect("commit");
        }
        let secrets = SecretsKey::from_bytes([3; 32]);
        let server_id = ServerId::parse("00112233445566778899aabbccddeeff").expect("id");
        crate::pairing::arm(&mut database, &secrets, &server_id, now()).expect("arm");

        let outcome = database
            .reconcile_identity(&SpkiPin::of(b"new key"), Actor::System, now())
            .expect("reconcile");
        assert_eq!(outcome, super::super::Reconciled::Revoked(2));
        assert!(database.list_devices().expect("list").is_empty());
        let row = database.pairing_row().expect("row");
        assert!(row.armed.is_none());
        assert_eq!(row.failed_attempts, 0);
        assert!(database
            .device_by_digest(&TokenDigest::from_bytes([1; 32]))
            .expect("lookup")
            .is_none());
    }

    #[test]
    fn revocation_deletes_the_verifier_and_audits_in_one_transaction() {
        let (_fixture, mut database) = setup("revoke");
        let id = "00000000000000000000000000000001";
        let transaction = database.pairing_transaction().expect("tx");
        transaction
            .consume(&device(id, &[5; 32]), now())
            .expect("consume");
        transaction.commit().expect("commit");
        assert!(database
            .device_by_digest(&TokenDigest::from_bytes([5; 32]))
            .expect("lookup")
            .is_some());

        assert!(database.revoke_device(id, id, now()).expect("revoke"));
        assert!(database
            .device_by_digest(&TokenDigest::from_bytes([5; 32]))
            .expect("lookup")
            .is_none());
        let audited: (String, String, String) = database
            .connection
            .query_row(
                "SELECT action, target, detail FROM audit WHERE action = 'device.revoked'",
                (),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("audit row");
        assert_eq!(audited.1, id);
        assert_eq!(audited.2, r#"{"self":true}"#);
        // Unknown: nothing changes, nothing is audited.
        assert!(!database.revoke_device(id, id, now()).expect("again"));
        let rows: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'device.revoked'",
                (),
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(rows, 1);
    }

    #[test]
    fn last_seen_is_written_at_most_once_a_minute() {
        let (_fixture, mut database) = setup("touch");
        let id = "00000000000000000000000000000001";
        let transaction = database.pairing_transaction().expect("tx");
        transaction
            .consume(&device(id, &[6; 32]), now())
            .expect("consume");
        transaction.commit().expect("commit");
        let seen = |database: &Database| {
            database.list_devices().expect("list")[0]
                .last_seen_at
                .clone()
        };
        database.touch_device(id, now()).expect("touch");
        let first = seen(&database);
        assert_eq!(first, Some(format_time(now())));
        database
            .touch_device(id, now() + time::Duration::seconds(59))
            .expect("touch");
        assert_eq!(seen(&database), first, "within the minute: no write");
        database
            .touch_device(id, now() + time::Duration::seconds(61))
            .expect("touch");
        assert_eq!(
            seen(&database),
            Some(format_time(now() + time::Duration::seconds(61)))
        );
    }

    #[test]
    fn the_armed_row_never_prints_its_ciphertext() {
        let row = ArmedRow {
            arming_id: [0xab; 16],
            profile: NATIVE_PROFILE.to_owned(),
            nonce: [0xcd; 24],
            ciphertext: vec![0xef; 32],
            armed_at: now(),
            expires_at: now(),
        };
        let text = format!("{row:?}");
        assert!(!text.contains("171") && !text.contains("205") && !text.contains("239"));
    }
}
