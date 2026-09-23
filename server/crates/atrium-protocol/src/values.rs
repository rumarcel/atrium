//! Validated values that cross the boundary.
//!
//! Every string in the protocol is one of these newtypes. Each has a private
//! field and exactly one constructor, `parse`, which checks the value against
//! a closed character set and a length bound. Deserialization goes through
//! `parse`, so a value that exists in memory has already been validated. There
//! is no `From<String>`, no `new_unchecked` and no public field.
//!
//! Nothing here is a path, a command, a key, or anything else Agent could act
//! on. They are correlation hints and descriptive facts, and every one of them
//! is safe to write into Agent's journal once it has been parsed.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why a value was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid(&'static str);

impl Invalid {
    /// What the value was supposed to be.
    #[must_use]
    pub fn expected(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid value: expected {}", self.0)
    }
}

impl std::error::Error for Invalid {}

/// Defines a string newtype whose only constructor is `parse`.
macro_rules! validated_string {
    ($(#[$doc:meta])* $name:ident, $expected:literal, $check:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Validates `text`.
            ///
            /// # Errors
            ///
            /// [`Invalid`] when the text is outside the value's character set
            /// or length bound. Nothing is normalised: an invalid value is
            /// refused, never repaired.
            pub fn parse(text: &str) -> Result<Self, Invalid> {
                let check: fn(&str) -> bool = $check;
                if check(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(Invalid($expected))
                }
            }

            /// The validated text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                Self::parse(&text).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn lower_hex(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.len())
        && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

validated_string!(
    /// A correlation hint chosen by Core: `[0-9a-f]{1,64}`.
    ///
    /// Plan section 3.4: Agent validates it before journaling it, so Core
    /// cannot shape Agent's audit trail, even within JSON escaping. Upper
    /// case is refused rather than folded; a value is never normalised into
    /// something loggable.
    RequestId,
    "1 to 64 lowercase hexadecimal characters",
    |text| lower_hex(text, 1, 64)
);

validated_string!(
    /// A build version such as `0.1.0` or `0.1.0-alpha.1+abc`:
    /// 1 to 32 characters from `[0-9A-Za-z.+-]`, starting with a digit.
    Version,
    "a version of 1 to 32 characters from [0-9A-Za-z.+-], starting with a digit",
    |text| {
        (1..=32).contains(&text.len())
            && text.as_bytes()[0].is_ascii_digit()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
    }
);

validated_string!(
    /// A UTC instant in RFC 3339 form: `YYYY-MM-DDTHH:MM:SSZ`, optionally with
    /// a fraction of 1 to 9 digits before the `Z`.
    Timestamp,
    "an RFC 3339 UTC timestamp such as 2026-09-24T10:15:00.123Z",
    |text| {
        let bytes = text.as_bytes();
        if bytes.len() < 20 || bytes.len() > 30 || bytes[bytes.len() - 1] != b'Z' {
            return false;
        }
        let shape = b"dddd-dd-ddTdd:dd:dd";
        let fixed = bytes[..19].iter().zip(shape).all(|(b, s)| match s {
            b'd' => b.is_ascii_digit(),
            other => b == other,
        });
        let rest = &bytes[19..bytes.len() - 1];
        let fraction = rest.is_empty()
            || (rest[0] == b'.'
                && (2..=10).contains(&rest.len())
                && rest[1..].iter().all(u8::is_ascii_digit));
        fixed && fraction
    }
);

validated_string!(
    /// The kernel's boot id: a lowercase UUID, 36 characters.
    BootId,
    "a lowercase UUID",
    |text| {
        text.len() == 36
            && text.bytes().enumerate().all(|(i, b)| match i {
                8 | 13 | 18 | 23 => b == b'-',
                _ => matches!(b, b'0'..=b'9' | b'a'..=b'f'),
            })
    }
);

validated_string!(
    /// A path Agent *reports*, for display: absolute, at most 128 bytes of
    /// printable ASCII, no `.` or `..` segment.
    ///
    /// It only ever travels from Agent to Core. No operation accepts one,
    /// and the enumeration test in this crate keeps it that way.
    ReportedPath,
    "an absolute printable-ASCII path of at most 128 bytes with no . or .. segment",
    |text| {
        (2..=128).contains(&text.len())
            && text.starts_with('/')
            && text.bytes().all(|b| (0x21..=0x7e).contains(&b))
            && text.split('/').skip(1).all(|s| !s.is_empty() && s != "." && s != "..")
    }
);

/// Unix permission bits, `0` to `0o7777`, written as four octal digits
/// (`"0700"`) on the wire so a journal line reads the way `stat` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMode(u32);

impl FileMode {
    /// Keeps the permission bits of `mode`.
    #[must_use]
    pub fn from_bits(mode: u32) -> Self {
        Self(mode & 0o7777)
    }

    /// The permission bits.
    #[must_use]
    pub fn bits(self) -> u32 {
        self.0
    }
}

impl fmt::Display for FileMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:04o}", self.0)
    }
}

impl Serialize for FileMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for FileMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        let valid = text.len() == 4 && text.bytes().all(|b| matches!(b, b'0'..=b'7'));
        if !valid {
            return Err(serde::de::Error::custom(Invalid("four octal digits")));
        }
        u32::from_str_radix(&text, 8)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_lowercase_hex_of_bounded_length() {
        for good in ["0", "3f1c", &"a".repeat(64), "0123456789abcdef"] {
            assert!(RequestId::parse(good).is_ok(), "{good:?}");
        }
        for bad in [
            "",
            &"a".repeat(65),
            "3F1C",
            "3f1g",
            "3f 1c",
            "3f1c\n",
            "3f1c\"",
            "../x",
            "\u{0661}",
            "3f1c\u{0}",
        ] {
            assert!(RequestId::parse(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_request_id_is_never_repaired() {
        // Upper case is refused, not folded: the value Agent journals is
        // exactly the value Core sent, or nothing.
        assert!(RequestId::parse("ABC").is_err());
        assert!(RequestId::parse(" abc").is_err());
    }

    #[test]
    fn versions() {
        for good in ["0.1.0", "1", "0.1.0-alpha.1+abc", &"1".repeat(32)] {
            assert!(Version::parse(good).is_ok(), "{good:?}");
        }
        for bad in [
            "",
            "v0.1.0",
            "0.1.0 ",
            "0.1.0\n",
            "0.1/0",
            &"1".repeat(33),
            "é",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn timestamps() {
        for good in [
            "2026-09-24T10:15:00Z",
            "2026-09-24T10:15:00.1Z",
            "2026-09-24T10:15:00.123456789Z",
        ] {
            assert!(Timestamp::parse(good).is_ok(), "{good:?}");
        }
        for bad in [
            "2026-09-24T10:15:00",
            "2026-09-24 10:15:00Z",
            "2026-09-24T10:15:00+00:00",
            "2026-09-24T10:15:00.Z",
            "2026-09-24T10:15:00.1234567890Z",
            "2026-9-24T10:15:00Z",
        ] {
            assert!(Timestamp::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn boot_ids() {
        assert!(BootId::parse("0f5e3c1a-2b4d-4e6f-8a9b-0c1d2e3f4a5b").is_ok());
        assert!(BootId::parse("0F5E3C1A-2B4D-4E6F-8A9B-0C1D2E3F4A5B").is_err());
        assert!(BootId::parse("0f5e3c1a2b4d4e6f8a9b0c1d2e3f4a5b").is_err());
    }

    #[test]
    fn reported_paths() {
        assert!(ReportedPath::parse("/var/lib/atrium-agent").is_ok());
        for bad in [
            "",
            "/",
            "relative",
            "/a/../b",
            "/a/./b",
            "/a//b",
            "/a b",
            "/a\nb",
            &format!("/{}", "a".repeat(128)),
        ] {
            assert!(ReportedPath::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn file_modes_round_trip_as_four_octal_digits() {
        let mode = FileMode::from_bits(0o40700);
        assert_eq!(mode.bits(), 0o700);
        let text = serde_json::to_string(&mode).expect("serialize");
        assert_eq!(text, "\"0700\"");
        let back: FileMode = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, mode);
        for bad in ["\"700\"", "\"0800\"", "\"00700\"", "448"] {
            assert!(serde_json::from_str::<FileMode>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn deserialization_goes_through_parse() {
        assert!(serde_json::from_str::<RequestId>("\"abc\"").is_ok());
        assert!(serde_json::from_str::<RequestId>("\"ABC\"").is_err());
        // An escaped control character is still a control character.
        assert!(serde_json::from_str::<RequestId>("\"ab\\u000a\"").is_err());
    }
}
