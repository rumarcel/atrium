//! Native client for the Atrium API.
//!
//! This crate is the one the Windows, macOS and Linux desktop applications will
//! depend on, so it is **platform-neutral by policy**: it must keep building on
//! Windows, and it must never pull a Linux-only or server-only dependency into
//! a desktop build. `server/ci/boundary-checks.sh` fails the build if it grows
//! a dependency on `nix`, `rusqlite`, `axum`, `atrium-core` or `atrium-agent`.
//!
//! # What is deliberately not here yet
//!
//! SPKI pinning, the pairing exchange, the TLS transport and the typed API
//! calls arrive in **M1G**. M1A adds no dependency on this crate from
//! `../../src-tauri` either: the desktop integration is M1G's work, and until
//! then the Windows build is untouched.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

/// The API constants this client speaks, re-exported so a consumer needs one
/// dependency rather than two.
pub use atrium_api_types as api;

/// Version of this client crate, for user agents and diagnostics.
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_speaks_api_v1() {
        assert_eq!(api::API_VERSION, 1);
    }

    #[test]
    fn version_is_populated() {
        assert!(!CLIENT_VERSION.is_empty());
    }
}
