//! The listener against real TLS clients and hostile raw requests.
//!
//! Each test starts the real server on `127.0.0.1:0` with a freshly generated
//! identity and a certificate issued for it by M1B's own code, and talks to it
//! over real TCP and real TLS. The client pins the identity's SPKI, the way an
//! Atrium client does, so every successful handshake also proves which key
//! was served.

use std::io::{Read as _, Write as _};
use std::net::{IpAddr, TcpStream as StdTcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::Digest;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use super::connection::EXPORTER_LABEL;
use super::tls::Served;
use super::{serve, App, HEADER_TIMEOUT, MAX_CONNECTIONS};
use crate::certificate::{self, AddressSet, SubjectNames};
use crate::identity::{DeviceKey, Identity, IdentityRecord, ServerId, SpkiPin};
use crate::recovery::{Recovery, RecoveryReason};

// ---------------------------------------------------------------------------
// Fixture

fn identity() -> Identity {
    let key = DeviceKey::generate().expect("key");
    Identity {
        record: IdentityRecord {
            server_id: ServerId::generate().expect("id"),
            created_at: OffsetDateTime::now_utc(),
            spki: key.pin(),
        },
        key,
    }
}

fn served(identity: &Identity, extra: &[&str]) -> Served {
    let addresses = AddressSet::new(extra.iter().map(|ip| ip.parse::<IpAddr>().expect("ip")));
    let names = SubjectNames::for_server(&identity.server_id(), &addresses);
    let issued = certificate::issue(identity, &names, OffsetDateTime::now_utc()).expect("issue");
    Served::new(identity, issued.pem.as_bytes()).expect("served")
}

struct Server {
    port: u16,
    pin: SpkiPin,
    app: Arc<App>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    identity: Identity,
}

impl Server {
    async fn start_with(build: impl FnOnce(&Served, u16) -> Arc<App>) -> Self {
        let identity = identity();
        let served = served(&identity, &["192.168.1.20"]);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let app = build(&served, port);
        let (stop, receiver) = watch::channel(false);
        let task = tokio::spawn(serve(listener, Arc::clone(&app), receiver));
        Self {
            port,
            pin: identity.pin(),
            app,
            stop,
            task,
            identity,
        }
    }

    async fn normal() -> Self {
        Self::start_with(|served, port| App::normal(served, port).expect("app")).await
    }

    async fn test_routes() -> Self {
        Self::start_with(|served, port| App::with_test_routes(served, port).expect("app")).await
    }

    async fn recovery() -> Self {
        let recovery = Recovery {
            reason: RecoveryReason::StateDatabaseUnreadable,
            since: OffsetDateTime::now_utc(),
            schema_version: Some(1),
            backup_count: 2,
        };
        Self::start_with(move |served, port| App::recovery(served, port, &recovery).expect("app"))
            .await
    }

    fn host(&self) -> String {
        format!("localhost:{}", self.port)
    }

    async fn connect(&self) -> TlsStream<TcpStream> {
        connect(self.port, self.pin, &[&rustls::version::TLS13]).await
    }

    /// Stops the server and returns how long it took.
    async fn stop(self) -> Duration {
        let started = Instant::now();
        self.stop.send(true).expect("stop");
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("the server stops")
            .expect("the server task does not panic");
        started.elapsed()
    }
}

/// Accepts exactly one SPKI, like an Atrium client with a pin.
#[derive(Debug)]
struct PinVerifier(SpkiPin);

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let (_, certificate) = x509_parser::parse_x509_certificate(end_entity)
            .map_err(|_| rustls::Error::General("unparseable".into()))?;
        if SpkiPin::of(certificate.public_key().raw) == self.0 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("wrong key".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

async fn try_connect(
    port: u16,
    pin: SpkiPin,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> std::io::Result<TlsStream<TcpStream>> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(versions)
    .expect("versions")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(PinVerifier(pin)))
    .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    connector
        .connect(ServerName::try_from("localhost").expect("name"), tcp)
        .await
}

async fn connect(
    port: u16,
    pin: SpkiPin,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> TlsStream<TcpStream> {
    try_connect(port, pin, versions)
        .await
        .expect("TLS handshake")
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("a JSON body")
    }

    fn code(&self) -> String {
        self.json()["code"].as_str().unwrap_or_default().to_owned()
    }
}

/// Sends raw bytes and reads one response (`Content-Length` bodies only,
/// which is all Core sends).
async fn exchange(stream: &mut TlsStream<TcpStream>, raw: &[u8]) -> Reply {
    stream.write_all(raw).await.expect("write");
    stream.flush().await.expect("flush");
    read_reply(stream).await.expect("a response")
}

async fn read_reply(stream: &mut TlsStream<TcpStream>) -> Option<Reply> {
    let mut buffer = Vec::new();
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let mut chunk = [0_u8; 4096];
        let read = tokio::time::timeout(Duration::from_secs(15), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8(buffer[..head_end].to_vec()).expect("ascii head");
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some(Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn get(path: &str, host: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()
}

async fn one(server: &Server, raw: &[u8]) -> Reply {
    let mut stream = server.connect().await;
    exchange(&mut stream, raw).await
}

const COMMON_HEADERS: [(&str, &str); 7] = [
    ("x-atrium-api", "1"),
    ("cache-control", "no-store"),
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    ("referrer-policy", "no-referrer"),
    ("cross-origin-resource-policy", "same-origin"),
    (
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    ),
];

fn assert_common_headers(reply: &Reply) {
    for (name, value) in COMMON_HEADERS {
        assert_eq!(reply.header(name), Some(value), "{name} on {reply:?}");
    }
    assert_eq!(reply.header("x-atrium-version"), Some(crate::VERSION));
    assert!(reply.header("x-request-id").is_some_and(|v| !v.is_empty()));
    assert!(
        reply
            .headers
            .iter()
            .all(|(n, _)| !n.to_ascii_lowercase().starts_with("access-control-")),
        "no CORS header is ever sent: {reply:?}"
    );
}

fn assert_problem(reply: &Reply, status: u16, code: &str) {
    assert_eq!(reply.status, status, "{reply:?}");
    assert_eq!(
        reply.header("content-type"),
        Some("application/problem+json"),
        "{reply:?}"
    );
    assert_eq!(reply.code(), code, "{reply:?}");
    assert_common_headers(reply);
}

// ---------------------------------------------------------------------------
// TLS

#[tokio::test]
async fn valid_tls_succeeds_and_serves_the_identity_key() {
    let server = Server::normal().await;
    let reply = one(&server, &get("/healthz", &server.host())).await;
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.json(),
        serde_json::json!({"status":"ok","state":"normal","api":1,"version":crate::VERSION})
    );
    assert_common_headers(&reply);

    // A client pinning any other key refuses the handshake.
    let other = identity();
    assert!(
        try_connect(server.port, other.pin(), &[&rustls::version::TLS13])
            .await
            .is_err()
    );
    server.stop().await;
}

#[tokio::test]
async fn tls12_with_ems_is_the_floor_and_every_connection_is_a_full_handshake() {
    let server = Server::normal().await;
    let mut stream = connect(server.port, server.pin, &[&rustls::version::TLS12]).await;
    assert_eq!(
        stream.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_2)
    );
    assert_eq!(
        exchange(&mut stream, &get("/healthz", &server.host()))
            .await
            .status,
        200
    );

    // No resumption: a second connection from a client that would resume
    // still performs a full handshake and receives the certificate.
    for _ in 0..2 {
        let stream = server.connect().await;
        assert_eq!(
            stream.get_ref().1.handshake_kind(),
            Some(rustls::HandshakeKind::Full)
        );
        assert!(stream.get_ref().1.peer_certificates().is_some());
    }
    server.stop().await;
}

#[tokio::test]
async fn plaintext_http_gets_no_http_response() {
    let server = Server::normal().await;
    let port = server.port;
    let received = tokio::task::spawn_blocking(move || {
        let mut tcp = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
        tcp.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        tcp.write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes(),
        )
        .expect("write");
        let mut received = Vec::new();
        let _ = tcp.read_to_end(&mut received);
        received
    })
    .await
    .expect("join");
    let text = String::from_utf8_lossy(&received).to_lowercase();
    assert!(!text.contains("http/"), "{text:?}");
    for leak in ["atrium", "healthz", "normal", crate::VERSION] {
        assert!(!text.contains(leak), "{leak} leaked: {text:?}");
    }
    server.stop().await;
}

#[tokio::test]
async fn a_reissued_certificate_keeps_the_pin_and_moves_the_host_allowlist() {
    let server = Server::normal().await;
    let before = server.connect().await;
    let serial_before = before.get_ref().1.peer_certificates().expect("cert")[0].to_vec();

    // Reissue for the same key with a different address set.
    let reissued = served(&server.identity, &["10.0.0.7"]);
    server.app.replace_certificate(&reissued);

    let mut after = server.connect().await; // the pin still matches
    let serial_after = after.get_ref().1.peer_certificates().expect("cert")[0].to_vec();
    assert_ne!(serial_before, serial_after, "a new certificate is served");
    assert_eq!(
        exchange(
            &mut after,
            &get("/healthz", &format!("10.0.0.7:{}", server.port))
        )
        .await
        .status,
        200
    );
    let old = exchange(
        &mut after,
        &get("/healthz", &format!("192.168.1.20:{}", server.port)),
    )
    .await;
    assert_problem(&old, 421, "request.host_rejected");
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Exporter / connection context

async fn binding(stream: &mut TlsStream<TcpStream>, host: &str) -> (String, String) {
    let reply = exchange(stream, &get("/test/binding", host)).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let json = reply.json();
    (
        json["connection"].as_str().expect("id").to_owned(),
        json["bindingDigest"].as_str().expect("digest").to_owned(),
    )
}

fn client_digest(stream: &TlsStream<TcpStream>) -> String {
    let exporter = stream
        .get_ref()
        .1
        .export_keying_material([0_u8; 32], EXPORTER_LABEL, Some(b""))
        .expect("exporter");
    let mut digest = sha2::Sha256::new();
    digest.update(b"atrium-test-binding");
    digest.update(exporter);
    hex::encode(digest.finalize())
}

#[tokio::test]
async fn the_exporter_belongs_to_the_connection_and_matches_the_client() {
    let server = Server::test_routes().await;
    let host = server.host();

    let mut first = server.connect().await;
    let (id_a, digest_a) = binding(&mut first, &host).await;
    let (id_b, digest_b) = binding(&mut first, &host).await;
    assert_eq!(id_a, id_b, "one connection, one context");
    assert_eq!(digest_a, digest_b);
    assert_eq!(
        digest_a,
        client_digest(&first),
        "the server holds this connection's exporter"
    );

    let mut second = server.connect().await;
    let (id_c, digest_c) = binding(&mut second, &host).await;
    assert_ne!(id_a, id_c, "another connection, another context");
    assert_ne!(digest_a, digest_c, "another connection, another exporter");
    assert_eq!(digest_c, client_digest(&second));
    server.stop().await;
}

#[tokio::test]
async fn a_tls13_route_refuses_tls12_and_tls12_has_no_binding() {
    let server = Server::test_routes().await;
    let mut stream = connect(server.port, server.pin, &[&rustls::version::TLS12]).await;
    let reply = exchange(&mut stream, &get("/test/binding", &server.host())).await;
    assert_problem(&reply, 403, "request.tls_version_required");
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Host

#[tokio::test]
async fn host_header_allowlist() {
    let server = Server::normal().await;
    let port = server.port;
    let mut stream = server.connect().await;
    for host in [
        format!("localhost:{port}"),
        format!("LocalHost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        format!("192.168.1.20:{port}"),
    ] {
        let reply = exchange(&mut stream, &get("/healthz", &host)).await;
        assert_eq!(reply.status, 200, "{host}: {reply:?}");
    }
    for host in [
        format!("attacker.example:{port}"),
        "attacker.example".to_owned(),
        "localhost".to_owned(),
        format!("localhost:{}", port.wrapping_add(1)),
        format!("localhost.:{port}"),
        format!("127.1:{port}"),
        format!("[::1%lo]:{port}"),
        format!("192.168.1.21:{port}"),
        format!("[::ffff:127.0.0.1]:{port}"),
        format!("user@localhost:{port}"),
    ] {
        let reply = exchange(&mut stream, &get("/healthz", &host)).await;
        assert_problem(&reply, 421, "request.host_rejected");
        assert!(!reply.body.contains("attacker"), "never reflected");
    }
    server.stop().await;
}

#[tokio::test]
async fn missing_duplicate_and_mismatched_hosts_are_refused() {
    let server = Server::normal().await;
    let port = server.port;
    let host = server.host();
    let cases: Vec<Vec<u8>> = vec![
        b"GET /healthz HTTP/1.1\r\n\r\n".to_vec(),
        format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nHost: attacker.example:{port}\r\n\r\n")
            .into_bytes(),
        format!("GET https://attacker.example:{port}/healthz HTTP/1.1\r\nHost: {host}\r\n\r\n")
            .into_bytes(),
        b"GET /healthz HTTP/1.0\r\n\r\n".to_vec(),
    ];
    for raw in cases {
        let reply = one(&server, &raw).await;
        // hyper itself answers 400 to some malformed heads; either way no
        // handler ran and nothing identifying came back.
        assert!(
            reply.status == 421 || reply.status == 400,
            "{}: {reply:?}",
            String::from_utf8_lossy(&raw)
        );
        assert!(
            !reply.body.contains("\"state\""),
            "no handler ran: {reply:?}"
        );
    }
    // The same absolute form, naming this server, is fine.
    let raw = format!("GET https://{host}/healthz HTTP/1.1\r\nHost: {host}\r\n\r\n");
    assert_eq!(one(&server, raw.as_bytes()).await.status, 200);
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Origin / CORS

#[tokio::test]
async fn cross_origin_browser_requests_are_refused_and_nothing_advertises_cors() {
    let server = Server::normal().await;
    let host = server.host();
    let with = |path: &str, method: &str, extra: &str| {
        format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n").into_bytes()
    };
    let mut stream = server.connect().await;

    let native = exchange(&mut stream, &with("/healthz", "GET", "")).await;
    assert_eq!(native.status, 200);

    for extra in [
        "Origin: https://attacker.example\r\n",
        "Origin: null\r\n",
        &format!("Origin: https://{host}\r\n"), // /healthz allows no browser at all
        "Sec-Fetch-Site: cross-site\r\n",
    ] {
        let reply = exchange(&mut stream, &with("/healthz", "GET", extra)).await;
        assert_problem(&reply, 403, "request.origin_rejected");
    }

    // The placeholder page allows its own origin and nothing else.
    let own = exchange(
        &mut stream,
        &with("/", "GET", &format!("Origin: https://{host}\r\n")),
    )
    .await;
    assert_eq!(own.status, 200);
    let foreign = exchange(
        &mut stream,
        &with("/", "GET", "Origin: https://attacker.example\r\n"),
    )
    .await;
    assert_problem(&foreign, 403, "request.origin_rejected");

    // A preflight names no future capability and grants nothing.
    for path in ["/healthz", "/api/v1/pair/begin", "/api/v1/devices"] {
        let preflight = exchange(
            &mut stream,
            &with(
                path,
                "OPTIONS",
                "Origin: https://attacker.example\r\nAccess-Control-Request-Method: POST\r\n\
                 Access-Control-Request-Headers: authorization\r\n",
            ),
        )
        .await;
        assert_problem(&preflight, 403, "request.origin_rejected");
    }
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Routing, errors, body

#[tokio::test]
async fn routing_is_exact_and_fails_closed() {
    let server = Server::normal().await;
    let host = server.host();
    let mut stream = server.connect().await;
    for (path, status, code) in [
        (
            "/api/v2/system/diagnostics",
            404,
            "request.unsupported_api_version",
        ),
        (
            "/api/V1/system/diagnostics",
            404,
            "request.unsupported_api_version",
        ),
        (
            "/api/%76%31/system/diagnostics",
            404,
            "request.unsupported_api_version",
        ),
        ("/api", 404, "request.unsupported_api_version"),
        ("/api/v1/system/diagnostics", 401, "auth.unauthorized"),
        ("/api/v1/devices", 401, "auth.unauthorized"),
        ("/api/v1/pair/info", 401, "auth.unauthorized"),
        ("/api/v1/../../healthz", 401, "auth.unauthorized"),
        ("/api/v1", 401, "auth.unauthorized"),
        ("/nothing", 404, "request.not_found"),
        ("/healthz/", 404, "request.not_found"),
        ("/HEALTHZ", 404, "request.not_found"),
        ("//healthz", 404, "request.not_found"),
    ] {
        let reply = exchange(&mut stream, &get(path, &host)).await;
        assert_problem(&reply, status, code);
    }

    let wrong = exchange(
        &mut stream,
        format!("POST /healthz HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\n\r\n").as_bytes(),
    )
    .await;
    assert_problem(&wrong, 405, "request.method_not_allowed");
    assert_eq!(wrong.header("allow"), Some("GET"));

    let device = exchange(
        &mut stream,
        format!("DELETE /api/v1/system/diagnostics HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes(),
    )
    .await;
    assert_problem(&device, 401, "auth.unauthorized");
    assert_eq!(device.header("www-authenticate"), Some("Bearer"));

    // A bearer token changes nothing in M1D: nothing is verified yet.
    let bearer = exchange(
        &mut stream,
        format!(
            "GET /api/v1/system/diagnostics HTTP/1.1\r\nHost: {host}\r\n\
             Authorization: Bearer AAAA\r\n\r\n"
        )
        .as_bytes(),
    )
    .await;
    assert_problem(&bearer, 401, "auth.unauthorized");
    server.stop().await;
}

#[tokio::test]
async fn error_response_leaks_nothing() {
    let server = Server::normal().await;
    let host = server.host();
    let server_id = server.identity.server_id().to_string();
    let short = server.identity.server_id().short_id();
    let mut stream = server.connect().await;
    for raw in [
        get("/api/v1/system/diagnostics", &host),
        get("/nothing/../../etc/passwd", &host),
        get("/healthz", "attacker.example:1"),
        get("/api/v9/x", &host),
    ] {
        let reply = exchange(&mut stream, &raw).await;
        let lowered = reply.body.to_lowercase();
        for leak in [
            server_id.as_str(),
            short.as_str(),
            "localhost",
            "/etc",
            "/var",
            "passwd",
            "attacker",
            ".rs",
            "panicked",
            "error(",
            crate::VERSION,
        ] {
            assert!(
                !lowered.contains(&leak.to_lowercase()),
                "{leak} in {reply:?}"
            );
        }
    }
    server.stop().await;
}

#[tokio::test]
async fn bodies_are_refused_or_bounded_before_parsing() {
    let server = Server::test_routes().await;
    let host = server.host();

    // A body where none is accepted is refused unread.
    let reply = one(
        &server,
        format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nContent-Length: 5\r\n\r\nhello")
            .as_bytes(),
    )
    .await;
    assert_problem(&reply, 400, "validation.body_not_accepted");

    let post = |body: &str| {
        format!(
            "POST /test/json HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    assert_eq!(
        one(&server, post(r#"{"name":"x"}"#).as_bytes())
            .await
            .status,
        200
    );
    assert_problem(
        &one(&server, post(r#"{"name":"x","admin":true}"#).as_bytes()).await,
        400,
        "validation.unknown_field",
    );
    assert_problem(
        &one(&server, post("{not json").as_bytes()).await,
        400,
        "validation.invalid_body",
    );

    // Declared too large: refused on the header, without waiting for bytes
    // that are never sent.
    let started = Instant::now();
    let declared = one(
        &server,
        format!("POST /test/json HTTP/1.1\r\nHost: {host}\r\nContent-Length: 100000000\r\n\r\n")
            .as_bytes(),
    )
    .await;
    assert_problem(&declared, 413, "validation.too_large");
    assert!(started.elapsed() < Duration::from_secs(2));

    // Chunked and too large: refused once the limit is crossed.
    let big = "a".repeat(200);
    let chunked = format!(
        "POST /test/json HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\n\r\n\
         {:x}\r\n{big}\r\n0\r\n\r\n",
        big.len()
    );
    assert_problem(
        &one(&server, chunked.as_bytes()).await,
        413,
        "validation.too_large",
    );
    server.stop().await;
}

#[tokio::test]
async fn oversized_and_malformed_heads_get_no_product_response() {
    let server = Server::normal().await;
    let host = server.host();
    let huge = format!(
        "GET /healthz HTTP/1.1\r\nHost: {host}\r\nX-Filler: {}\r\n\r\n",
        "a".repeat(40_000)
    );
    for raw in [
        huge.into_bytes(),
        b"GARBAGE\r\n\r\n".to_vec(),
        b"GET /healthz HTTP/9.9\r\n\r\n".to_vec(),
        b"GET healthz HTTP/1.1\r\n\r\n".to_vec(),
    ] {
        let mut stream = server.connect().await;
        let _ = stream.write_all(&raw).await;
        if let Some(reply) = read_reply(&mut stream).await {
            assert!(reply.status >= 400, "{reply:?}");
            assert!(
                !reply.body.contains("\"state\""),
                "no handler ran: {reply:?}"
            );
        }
    }
    // The server is still healthy afterwards.
    assert_eq!(one(&server, &get("/healthz", &host)).await.status, 200);
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Recovery

#[tokio::test]
async fn recovery_serves_only_health_and_redacted_diagnostics() {
    let server = Server::recovery().await;
    let host = server.host();
    let mut stream = server.connect().await;

    let health = exchange(&mut stream, &get("/healthz", &host)).await;
    assert_eq!(health.status, 200);
    let health = health.json();
    assert_eq!(health["state"], "recovery");
    assert_eq!(health["reason"], "state.database_unreadable");

    let diagnostics = exchange(&mut stream, &get("/api/v1/system/diagnostics", &host)).await;
    assert_eq!(diagnostics.status, 200);
    let mut keys: Vec<String> = diagnostics
        .json()
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "backups",
            "reason",
            "schema_version",
            "since",
            "state",
            "versions"
        ]
    );
    for leak in ["localhost", "192.168", "127.0.0.1", "atrium-"] {
        assert!(!diagnostics.body.contains(leak), "{leak}");
    }

    for raw in [
        get("/", &host),
        get("/api/v1/pair/info", &host),
        get("/api/v1/devices", &host),
        get("/nothing", &host),
        get("/api/v2/x", &host),
        format!(
            "POST /api/v1/pair/begin HTTP/1.1\r\nHost: {host}\r\nContent-Length: 2\r\n\r\n{{}}"
        )
        .into_bytes(),
        format!("POST /api/v1/restore HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\n\r\n")
            .into_bytes(),
        format!("DELETE /api/v1/devices/x HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes(),
    ] {
        let mut stream = server.connect().await;
        let reply = exchange(&mut stream, &raw).await;
        assert_problem(&reply, 503, "internal.recovery_mode");
    }
    // Host and Origin still apply in recovery.
    assert_problem(
        &exchange(&mut stream, &get("/healthz", "attacker.example:1")).await,
        421,
        "request.host_rejected",
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Lifecycle and limits

#[tokio::test]
async fn stalled_clients_do_not_delay_shutdown() {
    let server = Server::normal().await;
    let port = server.port;
    // Stalled before TLS, mid-handshake, and mid-request-head.
    let _silent = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp");
    let mut half_hello = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp");
    half_hello
        .write_all(&[0x16, 0x03, 0x01, 0x02, 0x00, 0x01])
        .await
        .expect("partial hello");
    let mut half_request = server.connect().await;
    half_request
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: ")
        .await
        .expect("partial head");
    let mut idle = server.connect().await;
    assert_eq!(
        exchange(&mut idle, &get("/healthz", &server.host()))
            .await
            .status,
        200
    );
    tokio::time::sleep(Duration::from_millis(200)).await;

    let took = server.stop().await;
    assert!(took < Duration::from_secs(4), "shutdown took {took:?}");
}

#[tokio::test]
async fn connections_over_the_cap_are_closed_before_tls() {
    let server = Server::normal().await;
    let port = server.port;
    let mut held = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        held.push(TcpStream::connect(("127.0.0.1", port)).await.expect("tcp"));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut extra = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp");
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(3), extra.read(&mut byte))
        .await
        .expect("closed promptly, not left hanging");
    assert!(matches!(read, Ok(0) | Err(_)), "closed without a byte");

    drop(held);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        one(&server, &get("/healthz", &server.host())).await.status,
        200
    );
    server.stop().await;
}

#[tokio::test]
async fn an_idle_keep_alive_connection_is_closed_after_the_header_timeout() {
    let server = Server::normal().await;
    let mut stream = server.connect().await;
    assert_eq!(
        exchange(&mut stream, &get("/healthz", &server.host()))
            .await
            .status,
        200
    );
    let started = Instant::now();
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(HEADER_TIMEOUT * 2, stream.read(&mut byte))
        .await
        .expect("an idle connection is not kept forever");
    assert!(matches!(read, Ok(0) | Err(_)));
    assert!(started.elapsed() >= HEADER_TIMEOUT - Duration::from_secs(1));
    server.stop().await;
}
