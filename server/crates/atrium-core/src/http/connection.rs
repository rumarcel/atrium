//! What Core knows about one TLS connection, for every request on it.
//!
//! M1E's pairing needs two things from the connection a request arrived on
//! (ADR-003 §4, plan §6.4): the RFC 8446 exporter
//! `TLS-Exporter("EXPORTER-Atrium-Pairing-v1", "", 32)` of the **complete**
//! handshake, and an identity for the connection, so `begin` and `complete`
//! can be required to arrive on the same one. Both are captured here once,
//! right after the handshake, and handed to each request on that connection
//! by reference.
//!
//! The exporter is held in a [`ChannelBinding`], which:
//! - belongs to exactly one connection and is dropped (and zeroed) with it;
//! - is never global, never persisted, and never serialized — it has no
//!   `Serialize`, no `Display`, and a `Debug` that prints nothing;
//! - is readable only inside this crate, by the code that will compute the
//!   pairing proofs.
//!
//! It is captured only for TLS 1.3 connections. The pairing routes require
//! TLS 1.3 (ADR-003 §4), and a TLS 1.2 exporter is only bound to its session
//! with extended master secret, so M1 does not offer one at all rather than
//! offer a weaker one conditionally.
//!
//! **Same connection.** Core speaks HTTP/1.1 only (ALPN `http/1.1`; no
//! HTTP/2), so "the same connection" is exactly one TLS session carrying
//! sequential keep-alive requests. Session resumption is disabled, so every
//! connection has a full handshake, its own exporter and the certificate in
//! it — the pin a native client checks during pairing.

use std::fmt;

use zeroize::Zeroizing;

/// The exporter label, from ADR-003 §4.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Atrium-Pairing-v1";

/// The negotiated TLS version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsVersion {
    /// TLS 1.2 (with extended master secret; see `tls.rs`).
    Tls12,
    /// TLS 1.3.
    Tls13,
}

/// A random identifier for one connection; it never leaves Core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(u128);

impl ConnectionId {
    /// The raw value, for tests that compare connections.
    #[cfg(test)]
    pub(crate) fn as_u128(self) -> u128 {
        self.0
    }

    /// A fresh random id.
    ///
    /// # Errors
    ///
    /// When the OS random source fails; the connection is then dropped.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)?;
        Ok(Self(u128::from_le_bytes(bytes)))
    }
}

/// The exporter of one TLS 1.3 connection.
// Read by M1E's pairing handler; until then only tests read it.
#[cfg_attr(not(test), allow(dead_code))]
pub struct ChannelBinding(Zeroizing<[u8; 32]>);

impl ChannelBinding {
    pub(crate) fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The exporter bytes. Crate-private: only the pairing handler (M1E)
    /// has any business reading them, and never to print them.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn exporter(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ChannelBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChannelBinding(<redacted>)")
    }
}

/// Everything captured from one connection.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ConnectionContext {
    id: ConnectionId,
    tls: TlsVersion,
    binding: Option<ChannelBinding>,
}

impl ConnectionContext {
    pub(crate) fn new(id: ConnectionId, tls: TlsVersion, binding: Option<ChannelBinding>) -> Self {
        Self { id, tls, binding }
    }

    /// This connection's id.
    #[must_use]
    pub fn id(&self) -> ConnectionId {
        self.id
    }

    /// The negotiated version.
    #[must_use]
    pub fn tls(&self) -> TlsVersion {
        self.tls
    }

    /// The exporter, on a TLS 1.3 connection.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn binding(&self) -> Option<&ChannelBinding> {
        self.binding.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_exporter() {
        let context = ConnectionContext::new(
            ConnectionId::generate().expect("random"),
            TlsVersion::Tls13,
            Some(ChannelBinding::new([0xab; 32])),
        );
        let text = format!("{context:?}");
        // Neither as a byte list (`[171, 171, …`) nor as hex.
        assert!(!text.contains("171, 171"), "{text}");
        assert!(!text.to_lowercase().contains("abab"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        assert_eq!(context.binding().expect("binding").exporter(), &[0xab; 32]);
    }

    #[test]
    fn ids_differ() {
        assert_ne!(
            ConnectionId::generate().expect("random"),
            ConnectionId::generate().expect("random")
        );
    }
}
