//! The three pairing messages, as JSON (API.md §3).
//!
//! Every type denies unknown fields and duplicate fields (serde refuses a
//! repeated field of a struct), every string has a bounded, validated form,
//! and every binary field is canonical unpadded base64url of a fixed length.
//! These types describe the messages. They do **not** define the
//! cryptographic transcript, which is assembled byte by byte in
//! [`crate::transcript`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::device::{DeviceName, Platform};
use crate::token;

/// A fixed-length binary field on the wire.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bytes<const N: usize>(pub [u8; N]);

impl<const N: usize> std::fmt::Debug for Bytes<N> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Bytes<{N}>(..)")
    }
}

impl<const N: usize> Serialize for Bytes<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&token::encode(&self.0))
    }
}

impl<'de, const N: usize> Deserialize<'de> for Bytes<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        token::decode::<N>(&text)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("not a canonical fixed-length field"))
    }
}

/// A 16-byte identifier as 32 lowercase hex characters (`serverId`,
/// `pairingId`, `deviceId`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub [u8; 16]);

impl Id {
    /// Lowercase hex.
    #[must_use]
    pub fn to_hex(self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parses exactly 32 lowercase hex characters.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut out = [0_u8; 16];
        for (index, pair) in text.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(pair).ok()?;
            out[index] = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl std::fmt::Debug for Id {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Id({})", self.to_hex())
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom("not a 32-hex identifier"))
    }
}

/// `GET /api/v1/pair/info`: display material, never trust material. The
/// client derives the pin from the handshake and compares; a match grants
/// nothing (plan §7.1a).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InfoResponse {
    /// The server's identifier.
    pub server_id: Id,
    /// The server's display name.
    pub name: String,
    /// Core's version.
    pub version: String,
    /// `SHA-256(SPKI)`, lowercase hex, for display and comparison only.
    pub spki: String,
    /// Whether a device has ever been paired.
    pub claimed: bool,
    /// Whether a secret is armed, unlocked and unexpired.
    pub pairing_open: bool,
}

/// `POST /api/v1/pair/begin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BeginRequest {
    /// The device's display name.
    pub device_name: DeviceName,
    /// The device's platform.
    pub platform: Platform,
    /// 32 random bytes from the client.
    pub client_nonce: Bytes<32>,
    /// The binding profile the client computes under. Checked against the
    /// armed state; never negotiated.
    pub binding_profile: ProfileField,
}

/// A binding-profile identifier, bounded before it is compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileField(pub String);

impl<'de> Deserialize<'de> for ProfileField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        let bounded =
            !text.is_empty() && text.len() <= 64 && text.bytes().all(|b| b.is_ascii_graphic());
        if bounded {
            Ok(Self(text.into_owned()))
        } else {
            Err(serde::de::Error::custom("not a profile identifier"))
        }
    }
}

/// The answer to `begin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BeginResponse {
    /// Names this attempt; single use.
    pub pairing_id: Id,
    /// 32 random bytes from the server.
    pub server_nonce: Bytes<32>,
    /// When this attempt stops being completable (RFC 3339, UTC).
    pub expires_at: String,
}

/// `POST /api/v1/pair/complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompleteRequest {
    /// From `begin`.
    pub pairing_id: Id,
    /// `HMAC-SHA256(K, "atrium-pair-v1:client" ‖ T)`.
    pub proof_c: Bytes<32>,
}

/// The server, as named in `complete`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ServerRef {
    /// The server's identifier.
    pub id: Id,
    /// Its display name.
    pub name: String,
}

/// The answer to `complete`. The client verifies `proof_s` **before** it
/// keeps anything else from this message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompleteResponse {
    /// `HMAC-SHA256(K, "atrium-pair-v1:server" ‖ T)`.
    pub proof_s: Bytes<32>,
    /// The new device's identifier.
    pub device_id: Id,
    /// The device token, returned this once.
    pub device_token: String,
    /// Always `owner` in M1.
    pub role: String,
    /// The server.
    pub server: ServerRef,
}

impl std::fmt::Debug for CompleteResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompleteResponse")
            .field("device_id", &self.device_id)
            .field("device_token", &"<redacted>")
            .field("role", &self.role)
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_is_strict() {
        let good = r#"{"deviceName":"Study laptop","platform":"windows",
            "clientNonce":"MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM",
            "bindingProfile":"atrium-pair-binding/native-tls-exporter-v1"}"#;
        assert!(serde_json::from_str::<BeginRequest>(good).is_ok());
        for bad in [
            // unknown field
            r#"{"deviceName":"a","platform":"linux","clientNonce":"MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM","bindingProfile":"p","admin":true}"#,
            // duplicate field
            r#"{"deviceName":"a","deviceName":"b","platform":"linux","clientNonce":"MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM","bindingProfile":"p"}"#,
            // short nonce
            r#"{"deviceName":"a","platform":"linux","clientNonce":"MzMz","bindingProfile":"p"}"#,
            // name with a newline
            r#"{"deviceName":"a\nb","platform":"linux","clientNonce":"MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM","bindingProfile":"p"}"#,
            // unknown platform
            r#"{"deviceName":"a","platform":"android","clientNonce":"MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM","bindingProfile":"p"}"#,
            // oversized profile
            &format!(
                r#"{{"deviceName":"a","platform":"linux","clientNonce":"MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM","bindingProfile":"{}"}}"#,
                "p".repeat(65)
            ),
        ] {
            assert!(serde_json::from_str::<BeginRequest>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ids_are_exact() {
        assert!(Id::parse("00112233445566778899aabbccddeeff").is_some());
        for bad in [
            "00112233445566778899AABBCCDDEEFF",
            "00112233445566778899aabbccddeef",
            "00112233445566778899aabbccddeeff0",
            "0011223344556677889ga9bbccddeeff",
        ] {
            assert!(Id::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn complete_response_redacts_the_token_in_debug() {
        let response = CompleteResponse {
            proof_s: Bytes([0; 32]),
            device_id: Id([1; 16]),
            device_token: "SECRET".to_owned(),
            role: "owner".to_owned(),
            server: ServerRef {
                id: Id([2; 16]),
                name: "n".to_owned(),
            },
        };
        assert!(!format!("{response:?}").contains("SECRET"));
    }
}
