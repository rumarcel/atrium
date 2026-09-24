//! TLS from the M1B identity: the persistent key and the current certificate.
//!
//! There is one key — `/etc/atrium/tls.key`, the device identity that
//! clients pin — and one certificate for it, which Core reissues when its
//! addresses change. Nothing here generates a key, and a certificate whose
//! public key is not the identity key's is refused rather than served, so the
//! SPKI a client sees is always the one it pinned.
//!
//! The certificate is chosen by Core, never by the client: the resolver
//! ignores SNI and returns the one current certificate. A reissue swaps it in
//! place for new connections; the key, and therefore the pin, never changes.
//!
//! Configuration:
//! - TLS 1.3 and 1.2 (the API floor, ADR-003 §4); 1.2 only with extended
//!   master secret (RFC 7627).
//! - ALPN `http/1.1` only.
//! - No session resumption: no tickets, no session cache. Every connection
//!   runs a full handshake, carries the certificate, and has its own exporter.
//! - No client certificates, no 0-RTT.

use std::sync::{Arc, RwLock};

use rustls::crypto::ring::sign::any_ecdsa_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, NoServerSessionStorage, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::certificate::{self, CertificateRecord};
use crate::fsio;
use crate::identity::Identity;
use crate::layout::{Layout, CERTIFICATE_FILE};

/// Largest certificate file read.
const CERTIFICATE_LIMIT: u64 = 64 * 1024;

/// Why TLS could not be set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsError {
    /// The certificate file is missing or unreadable.
    Unreadable,
    /// The file is not a single PEM certificate.
    Malformed,
    /// The certificate certifies a key other than the identity key.
    KeyMismatch,
    /// rustls refused the key or the configuration.
    Rejected,
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unreadable => "the certificate could not be read",
            Self::Malformed => "the certificate file is not a single PEM certificate",
            Self::KeyMismatch => "the certificate does not certify the identity key",
            Self::Rejected => "the TLS library refused the key or configuration",
        })
    }
}

impl std::error::Error for TlsError {}

/// A certificate ready to serve, and what it says.
#[derive(Debug)]
pub struct Served {
    certified: Arc<CertifiedKey>,
    record: CertificateRecord,
}

impl Served {
    /// Pairs `identity`'s key with the PEM certificate `pem`.
    ///
    /// # Errors
    ///
    /// [`TlsError`]; nothing is served from a certificate that fails.
    pub fn new(identity: &Identity, pem: &[u8]) -> Result<Self, TlsError> {
        let record = certificate::describe_pem(pem).ok_or(TlsError::Malformed)?;
        if record.spki != identity.pin() {
            return Err(TlsError::KeyMismatch);
        }
        let (_, block) = x509_parser::pem::parse_x509_pem(pem).map_err(|_| TlsError::Malformed)?;
        let der = CertificateDer::from(block.contents);
        let pkcs8 = identity.key.to_pkcs8();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.expose().clone()));
        let signer = any_ecdsa_type(&key).map_err(|_| TlsError::Rejected)?;
        Ok(Self {
            certified: Arc::new(CertifiedKey::new(vec![der], signer)),
            record,
        })
    }

    /// Reads `tls.crt` from the state directory and pairs it with the key.
    ///
    /// # Errors
    ///
    /// [`TlsError`].
    pub fn load(layout: &Layout, identity: &Identity) -> Result<Self, TlsError> {
        let pem = fsio::read_regular(&layout.state_file(CERTIFICATE_FILE), CERTIFICATE_LIMIT)
            .map_err(|_| TlsError::Unreadable)?;
        Self::new(identity, &pem)
    }

    /// What the certificate says.
    #[must_use]
    pub fn record(&self) -> &CertificateRecord {
        &self.record
    }
}

/// The one certificate Core serves, replaceable on reissue.
#[derive(Debug)]
pub struct CertStore {
    current: RwLock<Arc<CertifiedKey>>,
}

impl CertStore {
    /// A store serving `served`.
    #[must_use]
    pub fn new(served: &Served) -> Self {
        Self {
            current: RwLock::new(Arc::clone(&served.certified)),
        }
    }

    /// Serves `served` to every connection from now on.
    pub fn replace(&self, served: &Served) {
        if let Ok(mut current) = self.current.write() {
            *current = Arc::clone(&served.certified);
        }
    }
}

impl ResolvesServerCert for CertStore {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        // SNI and everything else in the hello are ignored on purpose: the
        // client does not choose what Core serves.
        self.current.read().ok().map(|current| Arc::clone(&current))
    }
}

/// The server configuration.
///
/// # Errors
///
/// [`TlsError::Rejected`] if rustls refuses the protocol versions.
pub fn server_config(store: Arc<CertStore>) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|_| TlsError::Rejected)?
        .with_no_client_auth()
        .with_cert_resolver(store);
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.require_ems = true;
    config.session_storage = Arc::new(NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    config.max_early_data_size = 0;
    Ok(Arc::new(config))
}
