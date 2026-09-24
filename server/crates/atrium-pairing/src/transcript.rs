//! The pairing key schedule, transcript and proofs, byte-exact (ADR-003 §4).
//!
//! ```text
//! cb     = TLS-Exporter("EXPORTER-Atrium-Pairing-v1", "", 32)
//! K      = HKDF-SHA256(ikm = secret16, salt = serverNonce,
//!                      info = "atrium-pair-v1" ‖ serverId, L = 32)
//! prof   = SHA-256(binding profile id)
//! device = SHA-256(UTF-8(deviceName) ‖ 0x00 ‖ ASCII(platform))
//! T      = prof ‖ serverId ‖ spki ‖ cb ‖ clientNonce ‖ serverNonce ‖ device
//!          32      16         32     32   32            32            32  = 208
//! proofC = HMAC-SHA256(K, "atrium-pair-v1:client" ‖ T)
//! proofS = HMAC-SHA256(K, "atrium-pair-v1:server" ‖ T)
//! ```
//!
//! Every field of `T` has a fixed length, so bytes cannot move between
//! neighbours. `device` separates the name from the platform with a NUL,
//! which a valid device name cannot contain (see [`crate::device`]); that is
//! how ADR-003's `H(deviceName ‖ platform)` is encoded, as plan §6.4 wrote
//! it. No serializer defines any of these bytes: they are assembled here,
//! and the frozen vectors below pin them. Changing any byte is a protocol
//! change.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::device::DeviceMetadata;
use crate::secret::PairingSecret;
use crate::{BINDING_PROFILE_NATIVE, TRANSCRIPT_BYTES};

/// The TLS exporter label (RFC 8446 §7.5, RFC 9266).
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Atrium-Pairing-v1";
/// The HKDF `info` prefix.
pub const KEY_INFO: &[u8] = b"atrium-pair-v1";
/// The label `proofC` is computed under.
pub const CLIENT_LABEL: &[u8] = b"atrium-pair-v1:client";
/// The label `proofS` is computed under.
pub const SERVER_LABEL: &[u8] = b"atrium-pair-v1:server";

/// A binding profile. M1 has exactly one; `web-pki-v1` is a reserved
/// identifier and deliberately not a variant, so nothing can reach a
/// zero-binding code path (ADR-003 §4a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// `atrium-pair-binding/native-tls-exporter-v1`.
    NativeTlsExporterV1,
}

impl Profile {
    /// The identifier on the wire and in the database.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::NativeTlsExporterV1 => BINDING_PROFILE_NATIVE,
        }
    }

    /// Parses an identifier. Only implemented profiles parse; the reserved
    /// web profile and anything unknown do not.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        (id == BINDING_PROFILE_NATIVE).then_some(Self::NativeTlsExporterV1)
    }

    /// `SHA-256(id)`, the transcript's first field.
    #[must_use]
    pub fn hash(self) -> [u8; 32] {
        profile_hash(self.id())
    }
}

/// `SHA-256(id)` for any identifier; tests use it to show two profiles give
/// different proofs.
#[must_use]
pub fn profile_hash(id: &str) -> [u8; 32] {
    Sha256::digest(id.as_bytes()).into()
}

/// The fixed-length inputs to the transcript.
#[derive(Clone, Copy)]
pub struct TranscriptInputs {
    /// `SHA-256(profile id)`.
    pub profile_hash: [u8; 32],
    /// The server's 16-byte identifier.
    pub server_id: [u8; 16],
    /// `SHA-256` of the SubjectPublicKeyInfo seen in the TLS handshake.
    pub spki: [u8; 32],
    /// The TLS exporter of this connection.
    pub channel_binding: [u8; 32],
    /// The client's nonce.
    pub client_nonce: [u8; 32],
    /// The server's nonce.
    pub server_nonce: [u8; 32],
    /// `SHA-256(name ‖ 0x00 ‖ platform)`.
    pub device_hash: [u8; 32],
}

impl std::fmt::Debug for TranscriptInputs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The channel binding is connection secret material; nothing here is
        // printed.
        formatter.write_str("TranscriptInputs(<redacted>)")
    }
}

/// Assembles `T`, 208 bytes.
#[must_use]
pub fn transcript(inputs: &TranscriptInputs) -> [u8; TRANSCRIPT_BYTES] {
    let mut out = [0_u8; TRANSCRIPT_BYTES];
    let fields: [&[u8]; 7] = [
        &inputs.profile_hash,
        &inputs.server_id,
        &inputs.spki,
        &inputs.channel_binding,
        &inputs.client_nonce,
        &inputs.server_nonce,
        &inputs.device_hash,
    ];
    let mut at = 0;
    for field in fields {
        out[at..at + field.len()].copy_from_slice(field);
        at += field.len();
    }
    out
}

/// `K`.
#[must_use]
pub fn pairing_key(
    secret: &PairingSecret,
    server_nonce: &[u8; 32],
    server_id: &[u8; 16],
) -> Zeroizing<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(server_nonce), secret.expose());
    let mut info = [0_u8; KEY_INFO.len() + 16];
    info[..KEY_INFO.len()].copy_from_slice(KEY_INFO);
    info[KEY_INFO.len()..].copy_from_slice(server_id);
    let mut key = Zeroizing::new([0_u8; 32]);
    // 32 bytes is far below HKDF-SHA256's 8160-byte limit.
    let _ = hkdf.expand(&info, &mut *key);
    key
}

fn proof(key: &[u8; 32], label: &[u8], transcript: &[u8; TRANSCRIPT_BYTES]) -> [u8; 32] {
    // HMAC accepts a key of any length.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(label);
    mac.update(transcript);
    mac.finalize().into_bytes().into()
}

/// `proofC`.
#[must_use]
pub fn client_proof(key: &[u8; 32], transcript: &[u8; TRANSCRIPT_BYTES]) -> [u8; 32] {
    proof(key, CLIENT_LABEL, transcript)
}

/// `proofS`.
#[must_use]
pub fn server_proof(key: &[u8; 32], transcript: &[u8; TRANSCRIPT_BYTES]) -> [u8; 32] {
    proof(key, SERVER_LABEL, transcript)
}

/// Constant-time equality of two proofs.
#[must_use]
pub fn proofs_match(expected: &[u8; 32], presented: &[u8; 32]) -> bool {
    expected.ct_eq(presented).into()
}

/// Builds the inputs from a device's metadata.
#[must_use]
pub fn device_hash(device: &DeviceMetadata) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(device.name().as_str().as_bytes());
    hasher.update([0_u8]);
    hasher.update(device.platform().as_str().as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceName, Platform};

    fn counting() -> PairingSecret {
        PairingSecret::from_bytes(std::array::from_fn(|i| u8::try_from(i).expect("small")))
    }

    fn inputs() -> TranscriptInputs {
        let device = DeviceMetadata::new(
            DeviceName::parse("Study laptop").expect("name"),
            Platform::Windows,
        );
        TranscriptInputs {
            profile_hash: Profile::NativeTlsExporterV1.hash(),
            server_id: [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ],
            spki: [0x11; 32],
            channel_binding: [0x22; 32],
            client_nonce: [0x33; 32],
            server_nonce: [0x44; 32],
            device_hash: device_hash(&device),
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Frozen vectors, computed independently (Python `hashlib`/`hmac`, HKDF
    /// written out from RFC 5869). A refactor that changes any byte fails
    /// here.
    #[test]
    fn proof_vectors() {
        let inputs = inputs();
        assert_eq!(
            hex(&inputs.profile_hash),
            "08a61df4ea57e372c62beaca7082b07beb49c52ecce4b0f42eba377488a28894"
        );
        assert_eq!(
            hex(&inputs.device_hash),
            "0aa1c05eda4194d6cc542b8aff3196c6ab3b73a17100fc782e768737df1bf376"
        );
        let key = pairing_key(&counting(), &inputs.server_nonce, &inputs.server_id);
        assert_eq!(
            hex(&*key),
            "7649237bab3bb6e8516d84db3269bbdd15f33736274cafeb9c209ed4eff85172"
        );
        let t = transcript(&inputs);
        assert_eq!(
            hex(&client_proof(&key, &t)),
            "2f3cbccaf0bb08fca21c3837177dd80fb6bb3a00974cc59967677c614caf93f3"
        );
        assert_eq!(
            hex(&server_proof(&key, &t)),
            "694a8a881b7f8ac4ac2aa2cbeb9676c9ef0a7147aab71a16a04611269eebe714"
        );
    }

    #[test]
    fn transcript_is_fixed_length_with_every_offset_asserted() {
        let t = transcript(&inputs());
        assert_eq!(t.len(), 208);
        let i = inputs();
        let expected: [(&[u8], std::ops::Range<usize>); 7] = [
            (&i.profile_hash, 0..32),
            (&i.server_id, 32..48),
            (&i.spki, 48..80),
            (&i.channel_binding, 80..112),
            (&i.client_nonce, 112..144),
            (&i.server_nonce, 144..176),
            (&i.device_hash, 176..208),
        ];
        for (field, range) in expected {
            assert_eq!(&t[range], field);
        }
    }

    #[test]
    fn client_and_server_labels_differ() {
        let key = pairing_key(&counting(), &[0x44; 32], &inputs().server_id);
        let t = transcript(&inputs());
        assert_ne!(client_proof(&key, &t), server_proof(&key, &t));
    }

    #[test]
    fn changing_any_field_changes_the_proof() {
        let secret = counting();
        let base = inputs();
        let proof_of = |i: &TranscriptInputs| {
            let key = pairing_key(&secret, &i.server_nonce, &i.server_id);
            client_proof(&key, &transcript(i))
        };
        let reference = proof_of(&base);
        let mutations: [fn(&mut TranscriptInputs); 7] = [
            |i| i.profile_hash[0] ^= 1,
            |i| i.server_id[0] ^= 1,
            |i| i.spki[0] ^= 1,
            |i| i.channel_binding[0] ^= 1,
            |i| i.client_nonce[0] ^= 1,
            |i| i.server_nonce[0] ^= 1,
            |i| i.device_hash[0] ^= 1,
        ];
        for mutate in mutations {
            let mut changed = base;
            mutate(&mut changed);
            assert_ne!(proof_of(&changed), reference);
        }
        let other_secret = PairingSecret::from_bytes([9; 16]);
        let key = pairing_key(&other_secret, &base.server_nonce, &base.server_id);
        assert_ne!(client_proof(&key, &transcript(&base)), reference);
    }

    #[test]
    fn profile_is_in_the_transcript() {
        let mut web = inputs();
        web.profile_hash = profile_hash(crate::BINDING_PROFILE_WEB_PKI_RESERVED);
        let key = pairing_key(&counting(), &web.server_nonce, &web.server_id);
        assert_ne!(
            client_proof(&key, &transcript(&web)),
            client_proof(&key, &transcript(&inputs()))
        );
    }

    #[test]
    fn web_profile_is_not_implemented_in_m1() {
        assert_eq!(
            Profile::parse(crate::BINDING_PROFILE_WEB_PKI_RESERVED),
            None
        );
        assert_eq!(Profile::parse(""), None);
        assert_eq!(
            Profile::parse("atrium-pair-binding/native-tls-exporter-v1"),
            Some(Profile::NativeTlsExporterV1)
        );
        assert_eq!(
            Profile::parse("atrium-pair-binding/native-tls-exporter-v1 "),
            None
        );
    }

    #[test]
    fn proof_comparison_is_constant_time_equality() {
        assert!(proofs_match(&[7; 32], &[7; 32]));
        assert!(!proofs_match(&[7; 32], &[8; 32]));
    }
}
