//! Device metadata a client supplies at pairing: a display name and a
//! platform. Both enter the transcript and the database, so both are
//! validated before either is used.
//!
//! **Name policy.** A display name is UTF-8 text, 1 to 64 characters and at
//! most 128 bytes, with no leading or trailing whitespace. It rejects every
//! control character, every format character that can reorder or hide text
//! (bidi overrides and isolates, zero-width characters, the byte-order
//! mark), line and paragraph separators, and the private-use and
//! noncharacter ranges. No Unicode normalization is applied: the name is
//! stored exactly as validated and never used as an identifier, so there is
//! nothing for two spellings to collide on. The rules keep a name from
//! injecting a line into a log, spoofing a column, or hiding text, and bound
//! what it can cost to store.
//!
//! **Platform** is a closed set.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Longest name, in characters.
pub const NAME_MAX_CHARS: usize = 64;
/// Longest name, in bytes.
pub const NAME_MAX_BYTES: usize = 128;

/// A validated device display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceName(String);

fn forbidden(c: char) -> bool {
    let code = u32::from(c);
    c.is_control()
        || matches!(code,
            0x00AD                  // soft hyphen
            | 0x061C                // Arabic letter mark
            | 0x115F | 0x1160       // Hangul fillers
            | 0x180E                // Mongolian vowel separator
            | 0x200B..=0x200F       // zero-width space/joiners, LRM, RLM
            | 0x2028 | 0x2029       // line and paragraph separators
            | 0x202A..=0x202E       // bidi embeddings and overrides
            | 0x2060..=0x206F       // word joiner, invisible operators, isolates
            | 0x3164                // Hangul filler
            | 0xFE00..=0xFE0F       // variation selectors
            | 0xFEFF                // byte-order mark
            | 0xFFF0..=0xFFFF       // specials and noncharacters
            | 0xE000..=0xF8FF       // private use
            | 0xF0000..=0x10FFFF    // supplementary private use
            | 0xE0000..=0xE007F     // tags
        )
        || (code & 0xFFFE) == 0xFFFE
        || (0xFDD0..=0xFDEF).contains(&code)
}

impl DeviceName {
    /// Validates a name.
    ///
    /// # Errors
    ///
    /// `Err(())` for anything outside the policy. Nothing is repaired.
    #[allow(clippy::result_unit_err)]
    pub fn parse(text: &str) -> Result<Self, ()> {
        let chars = text.chars().count();
        let valid = (1..=NAME_MAX_CHARS).contains(&chars)
            && text.len() <= NAME_MAX_BYTES
            && text.trim() == text
            && !text.chars().any(forbidden);
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(())
        }
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for DeviceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DeviceName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::parse(&text).map_err(|()| serde::de::Error::custom("invalid device name"))
    }
}

/// The platform a device says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
    /// A browser (not pairable in M1; the value exists for the schema).
    Web,
    /// Anything else.
    Other,
}

impl Platform {
    /// Every platform.
    pub const ALL: [Platform; 5] = [
        Platform::Windows,
        Platform::Macos,
        Platform::Linux,
        Platform::Web,
        Platform::Other,
    ];

    /// The stored and transmitted value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Web => "web",
            Self::Other => "other",
        }
    }

    /// Parses a stored value.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == text)
    }
}

/// A device's name and platform, together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMetadata {
    name: DeviceName,
    platform: Platform,
}

impl DeviceMetadata {
    /// Pairs a name with a platform.
    #[must_use]
    pub fn new(name: DeviceName, platform: Platform) -> Self {
        Self { name, platform }
    }

    /// The name.
    #[must_use]
    pub fn name(&self) -> &DeviceName {
        &self.name
    }

    /// The platform.
    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_pass() {
        for name in [
            "Study laptop",
            "Ölçü PC",
            "居間のテレビ",
            "a",
            &"x".repeat(64),
            "Ana's Mac",
        ] {
            assert!(DeviceName::parse(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn hostile_names_fail() {
        for name in [
            "",
            " leading",
            "trailing ",
            "line\nbreak",
            "carriage\rreturn",
            "tab\there",
            "nul\0",
            "esc\u{1b}[31m",
            "bidi\u{202e}override",
            "isolate\u{2066}x",
            "zero\u{200b}width",
            "bom\u{feff}",
            "sep\u{2028}",
            "private\u{e000}",
            "tag\u{e0041}",
            &"x".repeat(65),
            &"é".repeat(65),
        ] {
            assert!(DeviceName::parse(name).is_err(), "{name:?}");
        }
        // 64 characters but more than 128 bytes.
        assert!(DeviceName::parse(&"😀".repeat(40)).is_err());
    }

    #[test]
    fn platforms_are_closed() {
        for platform in Platform::ALL {
            assert_eq!(Platform::parse(platform.as_str()), Some(platform));
        }
        assert_eq!(Platform::parse("Windows"), None);
        assert!(serde_json::from_str::<Platform>("\"android\"").is_err());
    }
}
