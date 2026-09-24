//! The armed pairing secret at rest (ADR-019).
//!
//! Core has to recompute `proofC` from the secret itself, so it keeps the
//! secret in a form it can recover. The form is XChaCha20-Poly1305
//! authenticated encryption (the RustCrypto `chacha20poly1305` crate) under a
//! key derived from `/etc/atrium/secrets.key`:
//!
//! ```text
//! key   = HKDF-SHA256(ikm = secrets.key, salt = none,
//!                     info = "atrium-pairing-secret-v1/key", L = 32)
//! nonce = 24 bytes from the OS CSPRNG, new for every arming
//! aad   = "atrium-pairing-secret-v1" ‖ serverId(16) ‖ armingId(16)
//!         ‖ SHA-256(profile id)(32) ‖ armedAt(i64 BE unix s) ‖ expiresAt(i64 BE unix s)
//! row   = nonce(24) ‖ ciphertext(16) ‖ tag(16)      stored as secret_nonce + secret_ciphertext
//! ```
//!
//! - `secrets.key` never enters the database, so a copy of the database or
//!   of a backup alone yields nothing but ciphertext.
//! - The associated data ties a ciphertext to one server, one arming, one
//!   profile and one validity window: a row copied from another server,
//!   moved to another arming, relabelled with another profile or given a
//!   longer expiry does not open.
//! - A 24-byte random nonce makes reuse under one key negligible without any
//!   counter to persist.
//! - Opening failure is final: [`open`] returns `None` and the caller refuses
//!   the attempt. Nothing here generates, re-arms or repairs.
//! - The plaintext exists only as a [`PairingSecret`], which zeroes itself on
//!   drop and cannot be printed.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use atrium_pairing::secret::PairingSecret;
use atrium_pairing::SECRET_BYTES;

use crate::identity::SecretsKey;

/// Domain label for the key and the associated data.
const LABEL: &[u8] = b"atrium-pairing-secret-v1";
/// HKDF `info` for the sealing key.
const KEY_INFO: &[u8] = b"atrium-pairing-secret-v1/key";
/// XChaCha20-Poly1305 nonce length.
pub const NONCE_BYTES: usize = 24;
/// Sealed length: the 16-byte secret and a 16-byte tag.
pub const SEALED_BYTES: usize = SECRET_BYTES + 16;

/// What the ciphertext is bound to.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The installation.
    pub server_id: &'a [u8; 16],
    /// This arming.
    pub arming_id: &'a [u8; 16],
    /// The one profile this arming permits.
    pub profile: &'a str,
    /// When it was armed.
    pub armed_at: OffsetDateTime,
    /// When it stops being usable.
    pub expires_at: OffsetDateTime,
}

impl Context<'_> {
    fn aad(&self) -> Vec<u8> {
        let mut aad = Vec::with_capacity(LABEL.len() + 16 + 16 + 32 + 8 + 8);
        aad.extend_from_slice(LABEL);
        aad.extend_from_slice(self.server_id);
        aad.extend_from_slice(self.arming_id);
        aad.extend_from_slice(&Sha256::digest(self.profile.as_bytes()));
        aad.extend_from_slice(&self.armed_at.unix_timestamp().to_be_bytes());
        aad.extend_from_slice(&self.expires_at.unix_timestamp().to_be_bytes());
        aad
    }
}

/// A sealed secret, as stored.
pub struct Sealed {
    /// The nonce.
    pub nonce: [u8; NONCE_BYTES],
    /// Ciphertext and tag.
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Sealed(..)")
    }
}

fn cipher(secrets: &SecretsKey) -> XChaCha20Poly1305 {
    let hkdf = Hkdf::<Sha256>::new(None, secrets.expose());
    let mut key = Zeroizing::new([0_u8; 32]);
    // 32 bytes is far below HKDF-SHA256's output limit.
    let _ = hkdf.expand(KEY_INFO, &mut *key);
    XChaCha20Poly1305::new(key.as_slice().into())
}

/// Seals `secret` for `context` under a fresh nonce.
///
/// # Errors
///
/// When the OS random source fails. Nothing is sealed with a weaker nonce.
pub fn seal(
    secrets: &SecretsKey,
    context: &Context<'_>,
    secret: &PairingSecret,
) -> Result<Sealed, getrandom::Error> {
    let mut nonce = [0_u8; NONCE_BYTES];
    getrandom::fill(&mut nonce)?;
    seal_with_nonce(secrets, context, secret, nonce)
}

fn seal_with_nonce(
    secrets: &SecretsKey,
    context: &Context<'_>,
    secret: &PairingSecret,
    nonce: [u8; NONCE_BYTES],
) -> Result<Sealed, getrandom::Error> {
    let aad = context.aad();
    let ciphertext = cipher(secrets)
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: secret.expose(),
                aad: &aad,
            },
        )
        // Encryption of 16 bytes cannot exceed the cipher's length limit.
        .unwrap_or_default();
    Ok(Sealed { nonce, ciphertext })
}

/// Opens a sealed secret. `None` for anything that does not authenticate:
/// another `secrets.key`, another server, arming, profile or window, or a
/// modified nonce or ciphertext.
#[must_use]
pub fn open(
    secrets: &SecretsKey,
    context: &Context<'_>,
    nonce: &[u8; NONCE_BYTES],
    ciphertext: &[u8],
) -> Option<PairingSecret> {
    if ciphertext.len() != SEALED_BYTES {
        return None;
    }
    let aad = context.aad();
    let plain = Zeroizing::new(
        cipher(secrets)
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .ok()?,
    );
    let bytes: [u8; SECRET_BYTES] = plain.as_slice().try_into().ok()?;
    Some(PairingSecret::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets(fill: u8) -> SecretsKey {
        SecretsKey::from_bytes([fill; 32])
    }

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("time")
    }

    const SERVER: [u8; 16] = [0x5e; 16];
    const ARMING: [u8; 16] = [0xa1; 16];

    fn context() -> Context<'static> {
        Context {
            server_id: &SERVER,
            arming_id: &ARMING,
            profile: atrium_pairing::BINDING_PROFILE_NATIVE,
            armed_at: at(1_800_000_000),
            expires_at: at(1_800_000_900),
        }
    }

    fn secret() -> PairingSecret {
        PairingSecret::from_bytes(std::array::from_fn(|i| u8::try_from(i).expect("small")))
    }

    #[test]
    fn a_sealed_secret_opens_to_the_same_bytes_and_hides_them() {
        let sealed = seal(&secrets(7), &context(), &secret()).expect("seal");
        assert_eq!(sealed.ciphertext.len(), SEALED_BYTES);
        assert!(!sealed
            .ciphertext
            .windows(SECRET_BYTES)
            .any(|w| w == secret().expose()));
        let opened =
            open(&secrets(7), &context(), &sealed.nonce, &sealed.ciphertext).expect("open");
        assert!(opened.ct_eq(&secret()));
    }

    #[test]
    fn every_arming_gets_a_fresh_nonce() {
        let a = seal(&secrets(7), &context(), &secret()).expect("seal");
        let b = seal(&secrets(7), &context(), &secret()).expect("seal");
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn another_secrets_key_cannot_open_it() {
        let sealed = seal(&secrets(7), &context(), &secret()).expect("seal");
        assert!(open(&secrets(8), &context(), &sealed.nonce, &sealed.ciphertext).is_none());
    }

    #[test]
    fn tampering_with_ciphertext_nonce_or_any_context_field_is_rejected() {
        let sealed = seal(&secrets(7), &context(), &secret()).expect("seal");
        for index in 0..sealed.ciphertext.len() {
            let mut bad = sealed.ciphertext.clone();
            bad[index] ^= 1;
            assert!(open(&secrets(7), &context(), &sealed.nonce, &bad).is_none());
        }
        let mut nonce = sealed.nonce;
        nonce[0] ^= 1;
        assert!(open(&secrets(7), &context(), &nonce, &sealed.ciphertext).is_none());
        assert!(open(
            &secrets(7),
            &context(),
            &sealed.nonce,
            &sealed.ciphertext[..SEALED_BYTES - 1]
        )
        .is_none());

        let other_server = [0x5f; 16];
        let other_arming = [0xa2; 16];
        let variants = [
            Context {
                server_id: &other_server,
                ..context()
            },
            Context {
                arming_id: &other_arming,
                ..context()
            },
            Context {
                profile: atrium_pairing::BINDING_PROFILE_WEB_PKI_RESERVED,
                ..context()
            },
            Context {
                armed_at: at(1_800_000_001),
                ..context()
            },
            Context {
                expires_at: at(1_800_000_901),
                ..context()
            },
        ];
        for variant in &variants {
            assert!(open(&secrets(7), variant, &sealed.nonce, &sealed.ciphertext).is_none());
        }
    }

    /// Fixed inputs, fixed output: a refactor cannot silently change the
    /// stored format. Computed independently in Python: HKDF from `hmac`,
    /// and a from-scratch HChaCha20 + ChaCha20-Poly1305 checked against the
    /// RFC 8439 §2.8.2 AEAD vector and the XChaCha draft's HChaCha20 vector.
    #[test]
    fn sealing_known_vector() {
        let sealed =
            seal_with_nonce(&secrets(7), &context(), &secret(), [0x24; NONCE_BYTES]).expect("seal");
        assert_eq!(hex::encode(&sealed.ciphertext), SEALED_VECTOR);
    }

    const SEALED_VECTOR: &str = "4b96ace2fb7eafdf913928b37a547a116bda4be5e9ec9bb68ffeae529162cdbf";

    #[test]
    fn nothing_prints_the_sealed_bytes() {
        let sealed = seal(&secrets(7), &context(), &secret()).expect("seal");
        assert_eq!(format!("{sealed:?}"), "Sealed(..)");
    }
}
