//! A wrapper for secret material.
//!
//! [`Redacted`] holds a private key, a `secrets.key`, or anything else that
//! must never reach a log line, an error, a diagnostics payload or a test
//! snapshot. It has no `Display`, no `Serialize`, and a `Debug` that prints
//! only that something was withheld. Getting at the bytes needs an explicit
//! [`Redacted::expose`], which is easy to find in review.
//!
//! The contents are zeroed on drop. That is hygiene, not a security boundary:
//! the TLS stack keeps its own copy of the key for as long as it serves
//! connections, and nothing here pretends otherwise.

use std::fmt;

use zeroize::{Zeroize, Zeroizing};

/// Secret material that cannot be printed.
pub struct Redacted<T: Zeroize>(Zeroizing<T>);

impl<T: Zeroize> Redacted<T> {
    /// Wraps a secret.
    pub fn new(value: T) -> Self {
        Self(Zeroizing::new(value))
    }

    /// The secret itself. Every call site is a place secret material flows,
    /// and should read like one.
    #[must_use]
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize> fmt::Debug for Redacted<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Redacted(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_contents() {
        let secret = Redacted::new(vec![0xde_u8, 0xad, 0xbe, 0xef]);
        let shown = format!("{secret:?} {secret:#?}");
        assert!(!shown.contains("173"), "{shown}");
        assert!(!shown.contains("222"), "{shown}");
        assert_eq!(format!("{secret:?}"), "Redacted(..)");
    }

    #[test]
    fn expose_returns_the_contents() {
        let secret = Redacted::new(vec![1_u8, 2, 3]);
        assert_eq!(secret.expose(), &vec![1, 2, 3]);
    }
}
