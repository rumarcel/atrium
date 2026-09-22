//! Identifiers and sizes frozen by the pairing design (ADR-003).
//!
//! This crate is **pure**: no I/O, no paths, no runtime. In M1A it holds only
//! the constants the architecture fixes, plus tests that keep the arithmetic
//! honest — an earlier draft of ADR-003 said "32 bytes rendered as 26 Crockford
//! base32 characters", which is not a thing that can happen, and these tests
//! exist so that class of mistake cannot come back silently.
//!
//! # What is deliberately not here yet
//!
//! The Crockford Base32 codec, the normalization rules, the transcript builder
//! and the HMAC proofs arrive in **M1E**. M1A defines no cryptography.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

/// Binding profile used by native clients: the transcript covers the server's
/// SPKI hash and an RFC 8446 TLS exporter.
///
/// This is the only profile Alpha implements.
pub const BINDING_PROFILE_NATIVE: &str = "atrium-pair-binding/native-tls-exporter-v1";

/// Binding profile reserved for a future browser client, whose JavaScript can
/// read neither a TLS exporter nor the peer certificate.
///
/// **Not implemented.** It is reserved here so that adding it later is an
/// addition to an existing field rather than a replacement protocol, and it is
/// gated on the certificate decision in A-24 (ADR-003 section 4a).
pub const BINDING_PROFILE_WEB_PKI_RESERVED: &str = "atrium-pair-binding/web-pki-v1";

/// Entropy of the pairing secret, in bits.
pub const SECRET_ENTROPY_BITS: usize = 128;

/// Length of the pairing secret in bytes, straight from the OS CSPRNG.
pub const SECRET_BYTES: usize = 16;

/// Length of the encoded pairing secret, in Crockford Base32 symbols.
pub const SECRET_SYMBOLS: usize = 26;

/// Bits carried by one Crockford Base32 symbol.
pub const SYMBOL_BITS: usize = 5;

/// Length of the pairing transcript `T`, in bytes.
///
/// `profile(32) + serverId(16) + spki(32) + cb(32) + clientNonce(32) +
/// serverNonce(32) + deviceHash(32)`. Every field is fixed-length so the
/// transcript cannot be made ambiguous by moving bytes between neighbours.
pub const TRANSCRIPT_BYTES: usize = 208;

/// Length of a device token in bytes.
pub const DEVICE_TOKEN_BYTES: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_length_matches_its_entropy() {
        assert_eq!(SECRET_BYTES * 8, SECRET_ENTROPY_BITS);
    }

    #[test]
    fn symbol_count_is_the_ceiling_of_the_bit_count() {
        let expected = SECRET_ENTROPY_BITS.div_ceil(SYMBOL_BITS);
        assert_eq!(
            SECRET_SYMBOLS, expected,
            "26 symbols carry 130 bit positions for 128 bits of secret; \
             the two spare bits are padding and must decode as zero"
        );
    }

    #[test]
    fn symbol_count_leaves_exactly_two_padding_bits() {
        assert_eq!(SECRET_SYMBOLS * SYMBOL_BITS - SECRET_ENTROPY_BITS, 2);
    }

    #[test]
    fn transcript_is_the_sum_of_its_fields() {
        let profile = 32;
        let server_id = 16;
        let spki = 32;
        let channel_binding = 32;
        let client_nonce = 32;
        let server_nonce = 32;
        let device_hash = 32;
        assert_eq!(
            TRANSCRIPT_BYTES,
            profile
                + server_id
                + spki
                + channel_binding
                + client_nonce
                + server_nonce
                + device_hash
        );
    }

    #[test]
    fn profiles_are_distinct_and_namespaced() {
        assert_ne!(BINDING_PROFILE_NATIVE, BINDING_PROFILE_WEB_PKI_RESERVED);
        for profile in [BINDING_PROFILE_NATIVE, BINDING_PROFILE_WEB_PKI_RESERVED] {
            assert!(profile.starts_with("atrium-pair-binding/"));
        }
    }

    #[test]
    fn device_token_is_256_bits() {
        assert_eq!(DEVICE_TOKEN_BYTES * 8, 256);
    }
}
