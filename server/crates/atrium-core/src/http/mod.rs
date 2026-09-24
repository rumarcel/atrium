//! The HTTPS listener (M1D): TLS from the server identity, HTTP/1.1, and a
//! fixed route table.
//!
//! - [`tls`] — the identity key and the current certificate; no resumption.
//! - [`connection`] — per-connection context: id, TLS version, source and
//!   the exporter pairing binds to.
//! - [`limits`] — per-source connection shares and pairing rate limits.
//! - [`host`] — the `Host` allowlist, from the certificate's own names.
//! - [`routes`] — every route and its policy, literally.
//! - [`error`] — the closed set of RFC 9457 problems.
//! - `dispatch` — one request, every check, one order.
//!
//! There is no plaintext listener. A client speaking plain HTTP to the port
//! fails the TLS handshake and gets no HTTP response at all.
//!
//! Limits: at most [`MAX_CONNECTIONS`] connections at once, and at most
//! [`limits::MAX_CONNECTIONS_PER_SOURCE`] from one source (the rest are
//! closed on accept, before TLS); [`HANDSHAKE_TIMEOUT`] to finish TLS;
//! [`HEADER_TIMEOUT`] to send a request's headers, which also bounds how
//! long an idle keep-alive connection is kept; [`MAX_HEADER_BYTES`] of
//! request head; [`CONNECTION_LIFETIME`] per connection; and on shutdown
//! [`SHUTDOWN_GRACE`] for in-flight requests before every connection is
//! dropped. A stalled client can hold one connection slot for a bounded
//! time, and can never hold up shutdown.
//!
//! Nothing here reaches Agent, the database, or the identity files. The
//! pairing and device handlers call [`Services`] — `crate::pairing` and
//! `crate::devices` — which own every database access; this module has no
//! database handle of its own.

pub mod connection;
mod dispatch;
pub mod error;
pub mod host;
pub mod limits;
pub mod routes;
pub mod tls;

#[cfg(test)]
mod pairing_tests;
#[cfg(test)]
mod tests;

use std::convert::Infallible;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bytes::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use self::connection::{
    ChannelBinding, ConnectionContext, ConnectionId, TlsVersion, EXPORTER_LABEL,
};
use self::host::{Authority, HostAllowlist};
use self::limits::{ConnectionLimiter, PairingRates, RateLimiter, SourceKey};
use self::routes::{Route, ServingMode, ROUTES};
use self::tls::{CertStore, Served, TlsError};
use crate::recovery::Recovery;

/// Concurrent connections (plan §8.2). One source may hold at most
/// [`limits::MAX_CONNECTIONS_PER_SOURCE`] of them.
pub const MAX_CONNECTIONS: usize = 32;
/// Time to complete the TLS handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time to send a complete request head; also the idle keep-alive bound.
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request head, and hyper's read buffer.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Longest a single connection is kept.
pub const CONNECTION_LIFETIME: Duration = Duration::from_secs(300);
/// On shutdown, time given to in-flight requests before connections drop.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// Pause after a failed `accept`.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// What Core serves.
#[derive(Debug)]
pub enum AppMode {
    /// Normal mode.
    Normal,
    /// Recovery mode: the two payloads, fixed at startup.
    Recovery {
        /// `GET /healthz`.
        health: Bytes,
        /// `GET /api/v1/system/diagnostics`, redacted.
        diagnostics: Bytes,
    },
}

/// The state-owning services the normal-mode handlers call. Recovery has
/// none.
#[derive(Debug)]
pub struct Services {
    /// Pairing.
    pub pairing: crate::pairing::Pairing,
    /// Device authentication and management.
    pub devices: crate::devices::Devices,
}

/// The per-source limits an [`App`] enforces.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Concurrent connections from one source.
    pub connections_per_source: usize,
    /// Pairing request budgets.
    pub pairing: PairingRates,
}

impl Limits {
    /// The production values.
    pub const DEFAULT: Self = Self {
        connections_per_source: limits::MAX_CONNECTIONS_PER_SOURCE,
        pairing: PairingRates::DEFAULT,
    };
}

/// Everything the listener needs, shared by every connection.
#[derive(Debug)]
pub struct App {
    mode: AppMode,
    routes: &'static [Route],
    hosts: RwLock<HostAllowlist>,
    port: u16,
    certificates: Arc<CertStore>,
    tls: Arc<rustls::ServerConfig>,
    services: Option<Arc<Services>>,
    connections: Arc<ConnectionLimiter>,
    pairing_rates: RateLimiter,
}

impl App {
    fn build(
        mode: AppMode,
        routes: &'static [Route],
        served: &Served,
        port: u16,
        services: Option<Arc<Services>>,
        limits: Limits,
    ) -> Result<Arc<Self>, TlsError> {
        let certificates = Arc::new(CertStore::new(served));
        let tls = tls::server_config(Arc::clone(&certificates))?;
        Ok(Arc::new(Self {
            mode,
            routes,
            hosts: RwLock::new(HostAllowlist::from_names(&served.record().names, port)),
            port,
            certificates,
            tls,
            services,
            connections: ConnectionLimiter::new(limits.connections_per_source),
            pairing_rates: RateLimiter::new(limits.pairing, std::time::Instant::now()),
        }))
    }

    /// Normal mode, serving `served` on `port`, with `services` behind the
    /// pairing and device routes.
    ///
    /// # Errors
    ///
    /// [`TlsError`] if rustls refuses the configuration.
    pub fn normal(
        served: &Served,
        port: u16,
        services: Arc<Services>,
    ) -> Result<Arc<Self>, TlsError> {
        Self::build(
            AppMode::Normal,
            ROUTES,
            served,
            port,
            Some(services),
            Limits::DEFAULT,
        )
    }

    /// Normal mode with the real routes and chosen limits, for tests that
    /// drive many requests from one loopback address.
    #[doc(hidden)]
    pub fn normal_with_limits(
        served: &Served,
        port: u16,
        services: Arc<Services>,
        limits: Limits,
    ) -> Result<Arc<Self>, TlsError> {
        Self::build(
            AppMode::Normal,
            ROUTES,
            served,
            port,
            Some(services),
            limits,
        )
    }

    /// Recovery mode: `/healthz` and the redacted diagnostics only.
    ///
    /// # Errors
    ///
    /// [`TlsError`] if rustls refuses the configuration.
    pub fn recovery(
        served: &Served,
        port: u16,
        recovery: &Recovery,
    ) -> Result<Arc<Self>, TlsError> {
        let health = serde_json::to_vec(&recovery.health()).unwrap_or_default();
        let diagnostics = serde_json::to_vec(&recovery.diagnostics()).unwrap_or_default();
        Self::build(
            AppMode::Recovery {
                health: Bytes::from(health),
                diagnostics: Bytes::from(diagnostics),
            },
            ROUTES,
            served,
            port,
            None,
            Limits::DEFAULT,
        )
    }

    /// Normal mode with the test route table.
    #[cfg(test)]
    pub(crate) fn with_test_routes(
        served: &Served,
        port: u16,
        limits: Limits,
    ) -> Result<Arc<Self>, TlsError> {
        Self::build(
            AppMode::Normal,
            routes::TEST_ROUTES,
            served,
            port,
            None,
            limits,
        )
    }

    /// Serves a reissued certificate — same key, new names — to every new
    /// connection, and accepts exactly its names as `Host` from now on.
    pub fn replace_certificate(&self, served: &Served) {
        self.certificates.replace(served);
        if let Ok(mut hosts) = self.hosts.write() {
            *hosts = HostAllowlist::from_names(&served.record().names, self.port);
        }
    }

    fn serving_mode(&self) -> ServingMode {
        match self.mode {
            AppMode::Normal => ServingMode::Normal,
            AppMode::Recovery { .. } => ServingMode::Recovery,
        }
    }

    fn hosts_admit(&self, authority: &Authority) -> bool {
        self.hosts.read().is_ok_and(|hosts| hosts.admits(authority))
    }
}

/// Serves until `stop` becomes `true`, then drains for at most
/// [`SHUTDOWN_GRACE`] and drops whatever is left.
pub async fn serve(listener: TcpListener, app: Arc<App>, mut stop: watch::Receiver<bool>) {
    let acceptor = TlsAcceptor::from(Arc::clone(&app.tls));
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut tasks = JoinSet::new();
    while !*stop.borrow() {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    // The source is the socket's peer and nothing else.
                    let source = SourceKey::of(peer.ip());
                    // Over either cap: closed before any TLS work is done.
                    let Some(slot) = app.connections.try_acquire(source) else {
                        drop(stream);
                        continue;
                    };
                    let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let acceptor = acceptor.clone();
                    let app = Arc::clone(&app);
                    let stop = stop.clone();
                    tasks.spawn(async move {
                        serve_connection(stream, source, acceptor, app, stop).await;
                        drop(permit);
                        drop(slot);
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        event = "accept_failed",
                        component = "atrium-core",
                        reason = %error,
                        "could not accept a connection"
                    );
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    drop(listener);
    let drained = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    source: SourceKey,
    acceptor: TlsAcceptor,
    app: Arc<App>,
    mut stop: watch::Receiver<bool>,
) {
    let _ = stream.set_nodelay(true);
    let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream));
    let tls = tokio::select! {
        result = handshake => match result {
            Ok(Ok(tls)) => tls,
            _ => return,
        },
        _ = stop.changed() => return,
    };

    let context = {
        let (_, session) = tls.get_ref();
        let version = match session.protocol_version() {
            Some(rustls::ProtocolVersion::TLSv1_3) => TlsVersion::Tls13,
            Some(rustls::ProtocolVersion::TLSv1_2) => TlsVersion::Tls12,
            _ => return,
        };
        // The exporter of the complete handshake, for TLS 1.3 only.
        let binding = match version {
            TlsVersion::Tls13 => session
                .export_keying_material([0_u8; 32], EXPORTER_LABEL, Some(b""))
                .ok()
                .map(ChannelBinding::new),
            TlsVersion::Tls12 => None,
        };
        let Ok(id) = ConnectionId::generate() else {
            return;
        };
        Arc::new(ConnectionContext::new(id, version, binding, source))
    };

    let service = service_fn(move |request| {
        let app = Arc::clone(&app);
        let context = Arc::clone(&context);
        async move { Ok::<_, Infallible>(dispatch::dispatch(&app, &context, request).await) }
    });
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .keep_alive(true)
        .max_buf_size(MAX_HEADER_BYTES)
        .serve_connection(TokioIo::new(tls), service);
    tokio::pin!(connection);
    let lifetime = tokio::time::sleep(CONNECTION_LIFETIME);
    tokio::pin!(lifetime);
    tokio::select! {
        _ = connection.as_mut() => return,
        _ = stop.changed() => {}
        () = &mut lifetime => {}
    }
    connection.as_mut().graceful_shutdown();
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, connection).await;
}
