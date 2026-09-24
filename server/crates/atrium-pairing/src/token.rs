//! Device tokens and the binary fields of the pairing wire format.
//!
//! A device token is 32 bytes (256 bits) from the OS CSPRNG, sent as
//! unpadded base64url (43 characters). The server keeps only
//! `SHA-256(token)` ([`TokenDigest`]), compares digests in constant time,
//! and caches nothing (ADR-003 §7). Neither [`DeviceToken`] nor
//! [`TokenDigest`] implements `PartialEq`, `Display` or `Serialize`, and
//! both redact themselves in `Debug`.
//!
//! Every binary field on the wire (nonces, proofs, the token) uses the same
//! encoding: base64url without padding, decoded strictly. Trailing bits must
//! be zero and no padding or alternate alphabet is accepted, so one value has
//! exactly one text.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::DEVICE_TOKEN_BYTES;

/// Encodes bytes as unpadded base64url.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes exactly `N` bytes of canonical unpadded base64url.
#[must_use]
pub fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    // ceil(N * 4 / 3) characters, no padding.
    if text.len() != (N * 4).div_ceil(3) {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    let array: [u8; N] = bytes.try_into().ok()?;
    // The engine rejects non-zero trailing bits; re-encoding confirms the
    // text was the one canonical form.
    (encode(&array) == text).then_some(array)
}

/// A raw device token. Exists on the server only between generation and the
/// one response that returns it.
pub struct DeviceToken(Zeroizing<[u8; DEVICE_TOKEN_BYTES]>);

impl DeviceToken {
    /// Wraps 32 bytes the caller drew from the OS CSPRNG.
    #[must_use]
    pub fn from_bytes(bytes: [u8; DEVICE_TOKEN_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Parses the bearer form: exactly 43 characters of canonical base64url.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        decode::<DEVICE_TOKEN_BYTES>(text).map(Self::from_bytes)
    }

    /// The bearer form, returned to the client exactly once.
    #[must_use]
    pub fn encode(&self) -> Zeroizing<String> {
        Zeroizing::new(encode(&*self.0))
    }

    /// The verifier the server stores.
    #[must_use]
    pub fn digest(&self) -> TokenDigest {
        TokenDigest(Sha256::digest(*self.0).into())
    }
}

impl fmt::Debug for DeviceToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeviceToken(<redacted>)")
    }
}

/// `SHA-256(token)`, the only thing the server keeps.
#[derive(Clone, Copy)]
pub struct TokenDigest([u8; 32]);

impl TokenDigest {
    /// From the 32 stored bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 bytes, for storage and lookup.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Constant-time equality. There is no `==`.
    #[must_use]
    pub fn ct_eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl fmt::Debug for TokenDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TokenDigest(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counting() -> [u8; 32] {
        std::array::from_fn(|i| u8::try_from(i).expect("small"))
    }

    /// Committed literals, computed independently (Python `hashlib`,
    /// `base64.urlsafe_b64encode`).
    #[test]
    fn token_digest_known_vector() {
        let token = DeviceToken::from_bytes(counting());
        assert_eq!(
            &*token.encode(),
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
        );
        let hex: String = token
            .digest()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "630dcd2966c4336691125448bbb25b4ff412a49c732db2c8abc1b8581bd710dd"
        );
    }

    #[test]
    fn a_token_is_43_characters_and_parses_back() {
        let token = DeviceToken::from_bytes(counting());
        let text = token.encode();
        assert_eq!(text.len(), 43);
        let back = DeviceToken::parse(&text).expect("parse");
        assert!(back.digest().ct_eq(&token.digest()));
    }

    #[test]
    fn only_the_canonical_text_parses() {
        let text = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
        for bad in [
            "",
            &format!("{text}="),
            &text[..42],
            &format!("{text}A"),
            // Same bytes, non-zero trailing bits.
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9",
            // Standard alphabet characters.
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdH+8",
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdH/8",
            &format!(" {}", &text[1..]),
        ] {
            assert!(DeviceToken::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn nothing_prints_the_secret_values() {
        let token = DeviceToken::from_bytes(counting());
        assert_eq!(format!("{token:?}"), "DeviceToken(<redacted>)");
        assert_eq!(format!("{:?}", token.digest()), "TokenDigest(<redacted>)");
    }

    #[test]
    fn digests_compare_in_constant_time_only() {
        let a = DeviceToken::from_bytes(counting()).digest();
        let b = DeviceToken::from_bytes([0; 32]).digest();
        assert!(a.ct_eq(&a));
        assert!(!a.ct_eq(&b));
    }
}
