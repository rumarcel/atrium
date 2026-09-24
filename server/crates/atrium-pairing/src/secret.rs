//! The pairing secret: 16 bytes, 26 Crockford Base32 symbols (ADR-003 §3).
//!
//! [`PairingSecret`] holds the decoded bytes. It has no `Display`, no
//! `PartialEq` and no `Serialize`: it cannot be printed by accident, compared
//! with `==`, or written into a JSON document. Its `Debug` prints
//! `<redacted>`, and its bytes are zeroed when it is dropped. The one way to
//! compare two secrets is [`PairingSecret::ct_eq`], on the bytes, in
//! constant time. The text is never compared.
//!
//! Encoding and decoding follow ADR-003 §3 exactly, in this order:
//! 1. strip every ASCII and Unicode whitespace character and every `-`;
//! 2. uppercase with an **ASCII-only** mapping (never locale-dependent);
//! 3. map `O` → `0`, `I` → `1`, `L` → `1`;
//! 4. reject anything outside `0123456789ABCDEFGHJKMNPQRSTVWXYZ`;
//! 5. require exactly 26 symbols;
//! 6. reject a final symbol whose two low (padding) bits are not zero;
//! 7. produce exactly 16 bytes.
//!
//! Step 6 makes the encoding canonical: one value, one accepted text.

use std::fmt;

use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::{SECRET_BYTES, SECRET_SYMBOLS};

/// The Crockford Base32 alphabet: no `I`, `L`, `O` or `U`.
pub const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The decoded pairing secret.
pub struct PairingSecret(Zeroizing<[u8; SECRET_BYTES]>);

/// Why a text is not a pairing secret. The variants exist for tests and for
/// the console tool's message; the server never tells a client which one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// A character outside the alphabet after normalization.
    InvalidCharacter,
    /// Fewer than 26 symbols.
    TooShort,
    /// More than 26 symbols.
    TooLong,
    /// The last symbol's padding bits are not zero.
    NonCanonical,
}

impl PairingSecret {
    /// Wraps 16 bytes the caller drew from the OS CSPRNG.
    #[must_use]
    pub fn from_bytes(bytes: [u8; SECRET_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The bytes, for key derivation and sealing.
    #[must_use]
    pub fn expose(&self) -> &[u8; SECRET_BYTES] {
        &self.0
    }

    /// Constant-time comparison of the decoded bytes.
    #[must_use]
    pub fn ct_eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&*other.0).into()
    }

    /// The canonical 26 symbols, no hyphens: also the QR payload.
    #[must_use]
    pub fn canonical(&self) -> Zeroizing<String> {
        // 128 bits followed by two zero padding bits: 130 bits, 26 symbols.
        let value = u128::from_be_bytes(*self.0);
        let mut text = String::with_capacity(SECRET_SYMBOLS);
        for index in 0..SECRET_SYMBOLS {
            let shift = 125 - 5 * index;
            // Position `shift` counts from the least-significant end of the
            // 130-bit value `value << 2`.
            let symbol = if shift >= 2 {
                (value >> (shift - 2)) & 0x1f
            } else {
                (value << (2 - shift)) & 0x1f
            };
            text.push(char::from(ALPHABET[usize::try_from(symbol).unwrap_or(0)]));
        }
        Zeroizing::new(text)
    }

    /// `XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XX`, for the console.
    #[must_use]
    pub fn display_form(&self) -> Zeroizing<String> {
        let canonical = self.canonical();
        let groups: Vec<&str> = canonical
            .as_bytes()
            .chunks(4)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
            .collect();
        Zeroizing::new(groups.join("-"))
    }

    /// Decodes a secret typed, pasted or scanned by a person.
    ///
    /// # Errors
    ///
    /// [`DecodeError`]; nothing is repaired beyond the rules above.
    pub fn decode(text: &str) -> Result<Self, DecodeError> {
        let mut value: u128 = 0;
        let mut padding: u8 = 0;
        let mut count = 0_usize;
        for character in text.chars() {
            if character.is_whitespace() || character == '-' {
                continue;
            }
            if !character.is_ascii() {
                return Err(DecodeError::InvalidCharacter);
            }
            let upper = match character.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            let symbol = ALPHABET
                .iter()
                .position(|b| char::from(*b) == upper)
                .ok_or(DecodeError::InvalidCharacter)?;
            let symbol = u8::try_from(symbol).map_err(|_| DecodeError::InvalidCharacter)?;
            count += 1;
            if count > SECRET_SYMBOLS {
                return Err(DecodeError::TooLong);
            }
            if count == SECRET_SYMBOLS {
                // Three significant bits and two padding bits.
                value = (value << 3) | u128::from(symbol >> 2);
                padding = symbol & 0b11;
            } else {
                value = (value << 5) | u128::from(symbol);
            }
        }
        if count < SECRET_SYMBOLS {
            return Err(DecodeError::TooShort);
        }
        if padding != 0 {
            return Err(DecodeError::NonCanonical);
        }
        Ok(Self::from_bytes(value.to_be_bytes()))
    }
}

impl fmt::Debug for PairingSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingSecret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(bytes: [u8; 16]) -> PairingSecret {
        PairingSecret::from_bytes(bytes)
    }

    fn counting() -> [u8; 16] {
        std::array::from_fn(|i| u8::try_from(i).expect("small"))
    }

    // Vectors computed independently (Python, big-integer arithmetic).
    #[test]
    fn frozen_vectors() {
        assert_eq!(
            &*secret(counting()).canonical(),
            "000G40R40M30E209185GR38E1W"
        );
        assert_eq!(
            &*secret([0xff; 16]).canonical(),
            "ZZZZZZZZZZZZZZZZZZZZZZZZZW"
        );
        assert_eq!(&*secret([0; 16]).canonical(), "00000000000000000000000000");
        assert_eq!(
            &*secret(counting()).display_form(),
            "000G-40R4-0M30-E209-185G-R38E-1W"
        );
    }

    #[test]
    fn encoder_emits_exactly_26_symbols_from_the_alphabet() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..2_000 {
            let mut bytes = [0_u8; 16];
            for byte in &mut bytes {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = u8::try_from(state & 0xff).expect("byte");
            }
            let text = secret(bytes).canonical();
            assert_eq!(text.len(), 26);
            assert!(text.bytes().all(|b| ALPHABET.contains(&b)));
            for confusable in ['I', 'L', 'O', 'U'] {
                assert!(!text.contains(confusable));
            }
            let back = PairingSecret::decode(&text).expect("round trip");
            assert!(back.ct_eq(&secret(bytes)), "128-bit round trip");
        }
    }

    #[test]
    fn hyphens_and_whitespace_are_ignored() {
        let reference = secret(counting());
        for text in [
            "000G40R40M30E209185GR38E1W",
            "000G-40R4-0M30-E209-185G-R38E-1W",
            " 000G 40R4\t0M30\nE209 185G-R38E-1W ",
            "000G\u{00a0}40R4\u{2003}0M30E209185GR38E1W",
        ] {
            assert!(PairingSecret::decode(text).expect(text).ct_eq(&reference));
        }
    }

    #[test]
    fn crockford_confusables_map_in_both_cases() {
        let reference = PairingSecret::decode("0000000000000000000000001W").expect("ref");
        for text in [
            "OOOOOOOOOOOOOOOOOOOOOOOOIW",
            "oooooooooooooooooooooooo1w",
            "0000000000000000000000001w",
            "000000000000000000000000Lw",
            "000000000000000000000000lW",
            "000000000000000000000000iw",
        ] {
            assert!(
                PairingSecret::decode(text).expect(text).ct_eq(&reference),
                "{text}"
            );
        }
    }

    #[test]
    fn rejects_25_and_27_symbols() {
        assert_eq!(
            PairingSecret::decode("000G40R40M30E209185GR38E1").map(|_| ()),
            Err(DecodeError::TooShort)
        );
        assert_eq!(
            PairingSecret::decode("000G40R40M30E209185GR38E1W0").map(|_| ()),
            Err(DecodeError::TooLong)
        );
        assert_eq!(
            PairingSecret::decode("").map(|_| ()),
            Err(DecodeError::TooShort)
        );
    }

    #[test]
    fn rejects_non_canonical_padding() {
        // The last symbol carries three significant bits. Of the four symbols
        // that share them, exactly one — the one with zero padding — decodes.
        let base = "000G40R40M30E209185GR38E1";
        let accepted: Vec<char> = ['W', 'X', 'Y', 'Z']
            .into_iter()
            .filter(|last| PairingSecret::decode(&format!("{base}{last}")).is_ok())
            .collect();
        assert_eq!(accepted, ['W']);
        assert_eq!(
            PairingSecret::decode(&format!("{base}X")).map(|_| ()),
            Err(DecodeError::NonCanonical)
        );
    }

    #[test]
    fn rejects_out_of_alphabet() {
        for bad in ['U', 'u', '!', '\0', '_', '+', 'Ö', 'İ', 'ı', '😀'] {
            let text = format!("000G40R40M30E209185GR38E1{bad}");
            assert!(
                matches!(
                    PairingSecret::decode(&text).map(|_| ()),
                    Err(DecodeError::InvalidCharacter)
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn uppercasing_is_ascii_only() {
        // The Turkish dotted and dotless i are not the ASCII letters and are
        // refused, never folded; ASCII `i` maps to `1` whatever the locale.
        assert!(PairingSecret::decode("000g40r40m30e209185gr38e1w").is_ok());
        assert!(PairingSecret::decode("İ00G40R40M30E209185GR38E1W").is_err());
        assert!(PairingSecret::decode("ı00G40R40M30E209185GR38E1W").is_err());
        let with_i = PairingSecret::decode("i00G40R40M30E209185GR38E1W").expect("i");
        let with_one = PairingSecret::decode("100G40R40M30E209185GR38E1W").expect("1");
        assert!(with_i.ct_eq(&with_one));
    }

    #[test]
    fn qr_payload_decodes_identically() {
        let reference = secret(counting());
        let qr = reference.canonical();
        assert!(!qr.contains('-'));
        let display = reference.display_form();
        assert!(PairingSecret::decode(&qr)
            .expect("qr")
            .ct_eq(&PairingSecret::decode(&display).expect("display")));
    }

    #[test]
    fn secret_type_has_no_display_impl_and_redacts_debug() {
        let value = secret(counting());
        assert_eq!(format!("{value:?}"), "PairingSecret(<redacted>)");
        // `Display`, `PartialEq` and `Serialize` are deliberately absent; the
        // boundary gate fails the build if a derive adds one.
    }

    #[test]
    fn comparison_is_on_decoded_bytes() {
        let a = PairingSecret::decode("000G-40R4-0M30-E209-185G-R38E-1W").expect("a");
        let b = PairingSecret::decode("000g40r40m30e209185gr38e1w").expect("b");
        assert!(a.ct_eq(&b));
        assert!(!a.ct_eq(&secret([0; 16])));
    }
}
