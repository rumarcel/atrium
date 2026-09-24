//! The server's persistent identity.
//!
//! Identity is three files in `/etc/atrium`, generated once at installation by
//! [`crate::init`] and never by the running service:
//!
//! - `identity.json` — `server_id` (128 random bits, not derived from anything
//!   about the machine), creation time, and the SHA-256 of the key's
//!   SubjectPublicKeyInfo. Written last, so its presence means generation
//!   completed.
//! - `tls.key` — the device identity key, ECDSA P-256, PKCS#8 DER.
//! - `secrets.key` — 32 random bytes. From M1E it is read by Core and by
//!   `atriumctl pair` to seal and open an armed pairing secret (ADR-019), and
//!   by nothing else until M3.
//!
//! [`load`] is what Core does at startup. It reads, verifies that the three
//! files agree with each other, and reports a [`IdentityFault`] for anything
//! else. It has no code path that writes, and no fault leads to generation:
//! a missing or damaged identity is reported and left exactly as found, because
//! a new identity would silently break every client's pin.
//!
//! Identity is not the certificate. The certificate is public, lives in the
//! state directory, and is reissued freely from this key (see
//! [`crate::certificate`]).

use std::fmt;
use std::os::unix::fs::MetadataExt;

use rcgen::{KeyPair, PublicKeyData, PKCS_ECDSA_P256_SHA256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::fsio::{self, ReadError};
use crate::layout::{self, Layout, IDENTITY_FILE, PRIVATE_KEY_FILE, SECRETS_KEY_FILE};
use crate::protect::{self, Expect, Violation};
use crate::redact::Redacted;

/// The one key algorithm M1 uses. P-256 rather than Ed25519 because browsers
/// are the reference client and do not accept Ed25519 certificates.
pub const KEY_ALGORITHM: &str = "ecdsa-p256-sha256";

/// `identity.json` schema version.
const RECORD_FORMAT: u32 = 1;

/// Upper bounds on what is read. Real files are a few hundred bytes.
const RECORD_LIMIT: u64 = 4096;
const KEY_LIMIT: u64 = 4096;

/// Length of `secrets.key`.
pub const SECRETS_KEY_LEN: usize = 32;

/// The installation's identifier: 128 bits from the operating system's CSPRNG.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServerId([u8; 16]);

impl ServerId {
    /// A new random identifier.
    ///
    /// # Errors
    ///
    /// When the operating system cannot supply randomness. There is no
    /// fallback: an identifier from a weak source is worse than none.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Parses exactly 32 lowercase hexadecimal characters.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut bytes = [0_u8; 16];
        hex::decode_to_slice(text, &mut bytes).ok()?;
        Some(Self(bytes))
    }

    /// The 16 bytes, for the pairing transcript and the sealed secret's
    /// associated data.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The first eight hex characters, used in the certificate's names and
    /// the mDNS instance name.
    #[must_use]
    pub fn short_id(&self) -> String {
        hex::encode(&self.0[..4])
    }
}

impl fmt::Display for ServerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for ServerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ServerId({self})")
    }
}

/// `SHA-256(SubjectPublicKeyInfo)`: what a client pins. A property of the key
/// pair, not of any certificate, which is why reissue does not change it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpkiPin([u8; 32]);

impl SpkiPin {
    /// The pin of a DER-encoded SubjectPublicKeyInfo.
    #[must_use]
    pub fn of(spki_der: &[u8]) -> Self {
        Self(Sha256::digest(spki_der).into())
    }

    /// The 32 bytes, for the pairing transcript.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parses 64 lowercase hexadecimal characters.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut bytes = [0_u8; 32];
        hex::decode_to_slice(text, &mut bytes).ok()?;
        Some(Self(bytes))
    }
}

impl fmt::Display for SpkiPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for SpkiPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SpkiPin({self})")
    }
}

/// Why a key could not be produced or accepted. Deliberately carries no
/// detail from the parser: nothing derived from key bytes goes into an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Key generation failed.
    Generate,
    /// The bytes are not a PKCS#8 private key.
    Malformed,
    /// A valid key, but not ECDSA P-256.
    WrongAlgorithm,
}

impl fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Generate => "the key pair could not be generated",
            Self::Malformed => "the private key is not valid PKCS#8",
            Self::WrongAlgorithm => "the private key is not ECDSA P-256",
        })
    }
}

impl std::error::Error for KeyError {}

/// The device identity key pair.
pub struct DeviceKey {
    pair: KeyPair,
    spki_der: Vec<u8>,
    pin: SpkiPin,
}

impl DeviceKey {
    /// A new ECDSA P-256 key pair.
    ///
    /// # Errors
    ///
    /// [`KeyError::Generate`].
    pub fn generate() -> Result<Self, KeyError> {
        let pair =
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|_| KeyError::Generate)?;
        Ok(Self::wrap(pair))
    }

    /// Parses a PKCS#8 DER private key.
    ///
    /// # Errors
    ///
    /// [`KeyError::Malformed`] or [`KeyError::WrongAlgorithm`].
    pub fn from_pkcs8(der: &Redacted<Vec<u8>>) -> Result<Self, KeyError> {
        let pair = KeyPair::try_from(der.expose().as_slice()).map_err(|_| KeyError::Malformed)?;
        if pair.algorithm() != &PKCS_ECDSA_P256_SHA256 {
            return Err(KeyError::WrongAlgorithm);
        }
        Ok(Self::wrap(pair))
    }

    fn wrap(pair: KeyPair) -> Self {
        let spki_der = pair.subject_public_key_info();
        let pin = SpkiPin::of(&spki_der);
        Self {
            pair,
            spki_der,
            pin,
        }
    }

    /// The private key, PKCS#8 DER, for writing to disk.
    #[must_use]
    pub fn to_pkcs8(&self) -> Redacted<Vec<u8>> {
        Redacted::new(self.pair.serialize_der())
    }

    /// The public half, DER SubjectPublicKeyInfo.
    #[must_use]
    pub fn spki_der(&self) -> &[u8] {
        &self.spki_der
    }

    /// `SHA-256(SubjectPublicKeyInfo)`.
    #[must_use]
    pub fn pin(&self) -> SpkiPin {
        self.pin
    }

    /// The signing key, for certificate issuance.
    pub(crate) fn pair(&self) -> &KeyPair {
        &self.pair
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceKey")
            .field("algorithm", &KEY_ALGORITHM)
            .field("spki_sha256", &self.pin)
            .finish_non_exhaustive()
    }
}

/// `identity.json` on disk.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordFile {
    format: u32,
    server_id: String,
    created_at: String,
    key_algorithm: String,
    spki_sha256: String,
}

/// The immutable facts about this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRecord {
    /// The installation's identifier.
    pub server_id: ServerId,
    /// When the identity was generated.
    pub created_at: OffsetDateTime,
    /// The pin of the key generated with it.
    pub spki: SpkiPin,
}

impl IdentityRecord {
    /// Serialises to the on-disk form.
    ///
    /// # Panics
    ///
    /// Never in practice: the fields are plain strings and an integer.
    #[must_use]
    pub fn to_json(&self) -> Vec<u8> {
        let file = RecordFile {
            format: RECORD_FORMAT,
            server_id: self.server_id.to_string(),
            created_at: format_time(self.created_at),
            key_algorithm: KEY_ALGORITHM.to_owned(),
            spki_sha256: self.spki.to_string(),
        };
        let mut json = serde_json::to_vec_pretty(&file).expect("a flat record serialises");
        json.push(b'\n');
        json
    }

    /// Parses the on-disk form strictly: unknown fields, another format
    /// version, another algorithm or a malformed value are all refusals.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let file: RecordFile = serde_json::from_slice(bytes).ok()?;
        if file.format != RECORD_FORMAT || file.key_algorithm != KEY_ALGORITHM {
            return None;
        }
        Some(Self {
            server_id: ServerId::parse(&file.server_id)?,
            created_at: OffsetDateTime::parse(&file.created_at, &Rfc3339).ok()?,
            spki: SpkiPin::parse(&file.spki_sha256)?,
        })
    }
}

/// RFC 3339, whole seconds, UTC.
#[must_use]
pub fn format_time(time: OffsetDateTime) -> String {
    time.to_offset(time::UtcOffset::UTC)
        .replace_nanosecond(0)
        .unwrap_or(time)
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

/// A loaded, internally consistent identity.
#[derive(Debug)]
pub struct Identity {
    /// The facts from `identity.json`.
    pub record: IdentityRecord,
    /// The key from `tls.key`, verified to match the record's pin.
    pub key: DeviceKey,
}

impl Identity {
    /// The installation's identifier.
    #[must_use]
    pub fn server_id(&self) -> ServerId {
        self.record.server_id
    }

    /// What clients pin.
    #[must_use]
    pub fn pin(&self) -> SpkiPin {
        self.key.pin()
    }
}

/// Which of the ways the three files can disagree was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inconsistency {
    /// Some identity files exist and others do not.
    Partial {
        /// Files that exist.
        present: Vec<&'static str>,
        /// Files that do not.
        absent: Vec<&'static str>,
    },
    /// A temporary file from an interrupted write is in the directory.
    LeftoverTemporary,
    /// `identity.json` cannot be parsed or has the wrong shape.
    RecordMalformed,
    /// `tls.key` is not an ECDSA P-256 PKCS#8 key.
    KeyMalformed,
    /// The key's pin is not the one recorded in `identity.json` — the key was
    /// replaced, or the record was.
    KeyDoesNotMatchRecord,
    /// `secrets.key` is not exactly 32 bytes.
    SecretsKeyMalformed,
    /// An identity path is a link, a directory or a device.
    NotRegular(&'static str),
}

impl Inconsistency {
    /// Stable, loggable name.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Partial { .. } => "partial",
            Self::LeftoverTemporary => "leftover_temporary_file",
            Self::RecordMalformed => "record_malformed",
            Self::KeyMalformed => "key_malformed",
            Self::KeyDoesNotMatchRecord => "key_does_not_match_record",
            Self::SecretsKeyMalformed => "secrets_key_malformed",
            Self::NotRegular(_) => "not_a_regular_file",
        }
    }
}

impl fmt::Display for Inconsistency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Partial { present, absent } => write!(
                formatter,
                "partial identity: present [{}], absent [{}]",
                present.join(", "),
                absent.join(", ")
            ),
            Self::NotRegular(file) => write!(formatter, "{file} is not a regular file"),
            other => formatter.write_str(other.as_str()),
        }
    }
}

/// Why Core cannot use the identity it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityFault {
    /// No identity at all.
    Missing,
    /// An identity that is incomplete or does not agree with itself.
    Inconsistent(Inconsistency),
    /// An identity whose ownership or mode breaks the guarantees.
    Unprotected(Violation),
    /// An identity file exists but this process is not allowed to read it.
    Unreadable(&'static str),
}

impl fmt::Display for IdentityFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("no server identity exists"),
            Self::Inconsistent(detail) => {
                write!(formatter, "server identity is inconsistent: {detail}")
            }
            Self::Unprotected(violation) => {
                write!(formatter, "server identity is not protected: {violation}")
            }
            Self::Unreadable(file) => write!(formatter, "{file} exists but cannot be read"),
        }
    }
}

/// Whether [`load`] checks ownership and modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Require the installed ownership: root-owned, not writable by Core.
    /// The only setting the service uses.
    Required,
    /// Skip the ownership checks. For the installer's own consistency check,
    /// which runs before lockdown, and for unprivileged tests.
    NotChecked,
}

/// Which identity files exist, and whether a temporary is lying around.
#[derive(Debug)]
pub struct Inventory {
    /// Identity files present.
    pub present: Vec<&'static str>,
    /// Identity files absent.
    pub absent: Vec<&'static str>,
    /// Whether a leftover temporary file is in the directory.
    pub leftovers: bool,
}

impl Inventory {
    /// Nothing at all: no identity file and no leftover.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.present.is_empty() && !self.leftovers
    }
}

/// Lists what is in the identity directory, reading names only.
///
/// # Errors
///
/// [`IdentityFault::Unreadable`] when the directory cannot be listed.
pub fn inventory(layout: &Layout) -> Result<Inventory, IdentityFault> {
    let mut present = Vec::new();
    let mut absent = Vec::new();
    let mut leftovers = false;

    match fsio::lstat(layout.etc_dir()) {
        Ok(None) => {
            return Ok(Inventory {
                present,
                absent: layout::IDENTITY_FILES.to_vec(),
                leftovers,
            })
        }
        Ok(Some(_)) => {}
        Err(_) => return Err(IdentityFault::Unreadable("identity directory")),
    }

    for name in layout::IDENTITY_FILES {
        match fsio::lstat(&layout.etc_file(name)) {
            Ok(Some(_)) => present.push(name),
            Ok(None) => absent.push(name),
            Err(_) => return Err(IdentityFault::Unreadable(name)),
        }
    }

    let entries = std::fs::read_dir(layout.etc_dir())
        .map_err(|_| IdentityFault::Unreadable("identity directory"))?;
    for entry in entries {
        let entry = entry.map_err(|_| IdentityFault::Unreadable("identity directory"))?;
        if layout::is_temporary(&entry.file_name()) {
            leftovers = true;
        }
    }

    Ok(Inventory {
        present,
        absent,
        leftovers,
    })
}

/// Checks the installed ownership of the identity tree, including `core.toml`.
fn check_protection(layout: &Layout) -> Result<(), IdentityFault> {
    let euid = protect::euid();
    protect::check(layout.etc_dir(), Expect::RootDirectory, euid)
        .map_err(IdentityFault::Unprotected)?;
    for name in layout::IDENTITY_FILES {
        protect::check(&layout.etc_file(name), Expect::RootFile, euid)
            .map_err(IdentityFault::Unprotected)?;
    }
    protect::check(&layout.config(), Expect::RootFile, euid).map_err(IdentityFault::Unprotected)
}

fn read(layout: &Layout, name: &'static str, limit: u64) -> Result<Vec<u8>, IdentityFault> {
    fsio::read_regular(&layout.etc_file(name), limit).map_err(|error| match error {
        ReadError::Missing => IdentityFault::Inconsistent(Inconsistency::Partial {
            present: Vec::new(),
            absent: vec![name],
        }),
        ReadError::NotRegular => IdentityFault::Inconsistent(Inconsistency::NotRegular(name)),
        ReadError::TooLarge => IdentityFault::Inconsistent(if name == PRIVATE_KEY_FILE {
            Inconsistency::KeyMalformed
        } else {
            Inconsistency::RecordMalformed
        }),
        error if error.is_permission_denied() => IdentityFault::Unreadable(name),
        ReadError::Io(_) => IdentityFault::Unreadable(name),
    })
}

/// `secrets.key`, in memory: zeroed on drop, never printed.
pub struct SecretsKey(Redacted<[u8; SECRETS_KEY_LEN]>);

impl SecretsKey {
    /// The 32 bytes, for key derivation only.
    #[must_use]
    pub fn expose(&self) -> &[u8; SECRETS_KEY_LEN] {
        self.0.expose()
    }

    /// From bytes; for tests and for the caller that just read the file.
    #[must_use]
    pub fn from_bytes(bytes: [u8; SECRETS_KEY_LEN]) -> Self {
        Self(Redacted::new(bytes))
    }
}

impl fmt::Debug for SecretsKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretsKey(<redacted>)")
    }
}

/// Reads `secrets.key`. Only its size is checked here; its ownership and mode
/// are checked with the rest of the identity by [`load`].
///
/// # Errors
///
/// [`IdentityFault`]: missing, unreadable, or not exactly 32 bytes.
pub fn load_secrets_key(layout: &Layout) -> Result<SecretsKey, IdentityFault> {
    let bytes = Redacted::new(read(layout, SECRETS_KEY_FILE, SECRETS_KEY_LEN as u64 + 1)?);
    let array: [u8; SECRETS_KEY_LEN] = bytes
        .expose()
        .as_slice()
        .try_into()
        .map_err(|_| IdentityFault::Inconsistent(Inconsistency::SecretsKeyMalformed))?;
    Ok(SecretsKey::from_bytes(array))
}

/// Loads and verifies the identity. Reads only; never writes, never generates.
///
/// # Errors
///
/// An [`IdentityFault`] describing the first problem found. The files are
/// left exactly as they were.
pub fn load(layout: &Layout, protection: Protection) -> Result<Identity, IdentityFault> {
    let inventory = inventory(layout)?;
    if inventory.present.is_empty() {
        return Err(if inventory.leftovers {
            IdentityFault::Inconsistent(Inconsistency::LeftoverTemporary)
        } else {
            IdentityFault::Missing
        });
    }
    if !inventory.absent.is_empty() {
        return Err(IdentityFault::Inconsistent(Inconsistency::Partial {
            present: inventory.present,
            absent: inventory.absent,
        }));
    }
    // A leftover temporary beside a complete identity is debris from a write
    // that finished (the name was linked, the temporary not yet removed) or
    // one that never got as far as replacing anything. The three files are
    // verified against each other below, which is what matters, and startup
    // reports the debris. Beside an incomplete identity it is part of the
    // evidence, and was reported above.
    if protection == Protection::Required {
        check_protection(layout)?;
    }

    let record = IdentityRecord::parse(&read(layout, IDENTITY_FILE, RECORD_LIMIT)?)
        .ok_or(IdentityFault::Inconsistent(Inconsistency::RecordMalformed))?;

    let key_der = Redacted::new(read(layout, PRIVATE_KEY_FILE, KEY_LIMIT)?);
    let key = DeviceKey::from_pkcs8(&key_der)
        .map_err(|_| IdentityFault::Inconsistent(Inconsistency::KeyMalformed))?;
    if key.pin() != record.spki {
        return Err(IdentityFault::Inconsistent(
            Inconsistency::KeyDoesNotMatchRecord,
        ));
    }

    // secrets.key is read on its own by `load_secrets_key` (ADR-019); its
    // presence and size are part of a consistent identity.
    match fsio::lstat(&layout.etc_file(SECRETS_KEY_FILE)) {
        Ok(Some(metadata)) if metadata.file_type().is_file() => {
            if metadata.size() != SECRETS_KEY_LEN as u64 {
                return Err(IdentityFault::Inconsistent(
                    Inconsistency::SecretsKeyMalformed,
                ));
            }
        }
        Ok(Some(_)) => {
            return Err(IdentityFault::Inconsistent(Inconsistency::NotRegular(
                SECRETS_KEY_FILE,
            )))
        }
        Ok(None) => {
            return Err(IdentityFault::Inconsistent(Inconsistency::Partial {
                present: vec![IDENTITY_FILE, PRIVATE_KEY_FILE],
                absent: vec![SECRETS_KEY_FILE],
            }))
        }
        Err(_) => return Err(IdentityFault::Unreadable(SECRETS_KEY_FILE)),
    }

    Ok(Identity { record, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_ids_are_random_128_bit_lowercase_hex() {
        let first = ServerId::generate().expect("randomness");
        let second = ServerId::generate().expect("randomness");
        assert_ne!(first, second, "two draws from the CSPRNG must differ");

        let text = first.to_string();
        assert_eq!(text.len(), 32);
        assert!(text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(ServerId::parse(&text), Some(first));
        assert_eq!(first.short_id(), text[..8]);
    }

    #[test]
    fn server_id_parsing_is_strict() {
        let valid = "00112233445566778899aabbccddeeff";
        assert!(ServerId::parse(valid).is_some());
        for bad in [
            "00112233445566778899AABBCCDDEEFF",
            "00112233445566778899aabbccddee",
            "00112233445566778899aabbccddeeff00",
            "00112233445566778899aabbccddeefg",
            "",
        ] {
            assert!(ServerId::parse(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_key_round_trips_through_pkcs8_with_the_same_pin() {
        let key = DeviceKey::generate().expect("keygen");
        let reloaded = DeviceKey::from_pkcs8(&key.to_pkcs8()).expect("parse");
        assert_eq!(key.pin(), reloaded.pin());
        assert_eq!(key.spki_der(), reloaded.spki_der());
        assert_eq!(key.pin(), SpkiPin::of(key.spki_der()));
    }

    #[test]
    fn garbage_is_not_a_key() {
        let garbage = Redacted::new(vec![0x30_u8, 0x03, 0x02, 0x01, 0x00]);
        assert_eq!(
            DeviceKey::from_pkcs8(&garbage).map(|_| ()),
            Err(KeyError::Malformed)
        );
    }

    #[test]
    fn debug_output_of_a_key_contains_no_private_material() {
        let key = DeviceKey::generate().expect("keygen");
        let der = key.to_pkcs8();
        let shown = format!("{key:?} {key:#?} {der:?} {der:#?}");

        // PKCS#8 embeds the public key as well, so only windows of the
        // encoding that do not also occur in the public half are evidence.
        let private_hex = hex::encode(der.expose());
        let public = format!("{}{}", hex::encode(key.spki_der()), key.pin());
        for start in 0..=private_hex.len() - 16 {
            let needle = &private_hex[start..start + 16];
            if !public.contains(needle) {
                assert!(
                    !shown.contains(needle),
                    "debug output leaks {needle}: {shown}"
                );
            }
        }
        let decimal = format!("{:?}", &der.expose()[der.expose().len() - 8..]);
        assert!(!shown.contains(decimal.trim_matches(['[', ']'])), "{shown}");
    }

    #[test]
    fn the_record_round_trips_and_is_strict() {
        let key = DeviceKey::generate().expect("keygen");
        let record = IdentityRecord {
            server_id: ServerId::generate().expect("randomness"),
            created_at: OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time"),
            spki: key.pin(),
        };
        let json = record.to_json();
        assert_eq!(IdentityRecord::parse(&json), Some(record.clone()));

        let text = String::from_utf8(json).expect("utf-8");
        let extra = text.replacen('{', "{\"extra\": 1,", 1);
        assert!(IdentityRecord::parse(extra.as_bytes()).is_none());
        let other_alg = text.replace(KEY_ALGORITHM, "ed25519");
        assert!(IdentityRecord::parse(other_alg.as_bytes()).is_none());
        let other_format = text.replace("\"format\": 1", "\"format\": 2");
        assert!(IdentityRecord::parse(other_format.as_bytes()).is_none());
        assert!(IdentityRecord::parse(b"").is_none());
        assert!(IdentityRecord::parse(b"{}").is_none());
    }

    #[test]
    fn faults_never_carry_key_material() {
        // Every fault is built from closed enums and file names. This pins
        // that down: the Display of each variant is a fixed shape.
        let faults = [
            IdentityFault::Missing,
            IdentityFault::Inconsistent(Inconsistency::KeyMalformed),
            IdentityFault::Inconsistent(Inconsistency::KeyDoesNotMatchRecord),
            IdentityFault::Unreadable(PRIVATE_KEY_FILE),
        ];
        for fault in faults {
            let text = fault.to_string();
            assert!(text.len() < 120, "unexpectedly long fault text: {text}");
            assert!(!text.contains("BEGIN"), "{text}");
        }
    }
}
