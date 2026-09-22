//! Constants of the public HTTP API surface.
//!
//! This crate is **pure**: no I/O, no runtime, no HTTP framework. Core depends
//! on it; so does `atrium-client`; neither depends on the other. That direction
//! is what lets a client be written without the server's source.
//!
//! # What is deliberately not here yet
//!
//! The request and response DTOs, the error-code enum, the capability document
//! and the problem+json shape arrive in **M1D**. M1A defines no routes and no
//! payloads, because M1A serves no HTTP.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]

/// Major version of the public API, echoed in every response.
pub const API_VERSION: u32 = 1;

/// Path prefix for every versioned route.
pub const API_BASE_PATH: &str = "/api/v1";

/// Default TCP port for the Core API.
///
/// Reserved by ADR-015 so the canary cannot collide with the prototype's agent
/// on 9473 or with any service the owner already runs.
pub const DEFAULT_API_PORT: u16 = 7443;

/// Response header carrying [`API_VERSION`].
pub const HEADER_API_VERSION: &str = "x-atrium-api";

/// Response header carrying the build version.
pub const HEADER_BUILD_VERSION: &str = "x-atrium-version";

/// Request and response header carrying the correlation id.
pub const HEADER_REQUEST_ID: &str = "x-request-id";

/// DNS-SD service type Core advertises.
pub const MDNS_SERVICE_TYPE: &str = "_atrium._tcp";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_does_not_collide_with_reserved_prototype_ports() {
        // ADR-015: 9473 is the prototype agent; the rest are the owner's
        // existing services. The canary must bind none of them.
        const PROTOTYPE_PORTS: [u16; 13] = [
            9473, 8096, 7878, 8989, 6767, 8080, 8443, 9443, 9090, 49494, 8081, 3001, 61208,
        ];
        assert!(!PROTOTYPE_PORTS.contains(&DEFAULT_API_PORT));
    }

    #[test]
    fn headers_are_lowercase_for_direct_comparison() {
        for header in [HEADER_API_VERSION, HEADER_BUILD_VERSION, HEADER_REQUEST_ID] {
            assert_eq!(header, header.to_ascii_lowercase());
        }
    }

    #[test]
    fn base_path_carries_the_major_version() {
        assert_eq!(API_BASE_PATH, format!("/api/v{API_VERSION}"));
    }
}
