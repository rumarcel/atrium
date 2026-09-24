//! M1E over the real listener: pairing, devices and the unauthenticated
//! surface's limits, driven by a harness client built on `atrium-pairing`.
//!
//! Every test runs the real server on loopback with a real database in a
//! private temporary tree, arms pairing the way `atriumctl pair` does
//! (through its own database connection), and pairs over real TLS 1.3 with
//! the exporter and certificate taken from the client's own handshake.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use time::Duration as Span;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::client::TlsStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use atrium_pairing::client::{ClientPairing, Paired, Untrusted};
use atrium_pairing::device::{DeviceMetadata, DeviceName, Platform};
use atrium_pairing::secret::PairingSecret;
use atrium_pairing::wire::{
    BeginResponse, Bytes, CompleteRequest, CompleteResponse, Id, InfoResponse, ProfileField,
    ServerRef,
};

use super::connection::EXPORTER_LABEL;
use super::limits::{PairingRates, Rate};
use super::tests::{
    assert_common_headers, assert_problem, connect, exchange, get, handshake, identity, served,
    tcp_from, Reply, Server,
};
use super::tls::{self, CertStore};
use super::Limits;
use crate::identity::SpkiPin;
use crate::pairing::{ATTEMPT_WINDOW, SECRET_LIFETIME};

// ---------------------------------------------------------------------------
// Harness client

/// Limits loose enough that a test's own requests never trip them; the
/// limit tests use the real ones.
const RELAXED: Limits = Limits {
    connections_per_source: 32,
    pairing: PairingRates {
        per_source: Rate::per_minute(10_000, 10_000),
        global: Rate::per_minute(10_000, 10_000),
    },
};

async fn server() -> Server {
    Server::normal_with(RELAXED).await
}

/// A client with no pin yet: during pairing the candidate pin is whatever
/// this handshake presents, and only the proofs decide whether to keep it.
#[derive(Debug)]
struct FirstContact;

impl ServerCertVerifier for FirstContact {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
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

async fn first_contact(port: u16) -> TlsStream<TcpStream> {
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("versions")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(FirstContact))
    .with_no_client_auth();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp");
    TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("localhost").expect("name"), tcp)
        .await
        .expect("TLS 1.3")
}

/// `SHA-256(SPKI)` and the exporter, from the client's side of `stream`.
fn handshake_material<S>(stream: &TlsStream<S>) -> ([u8; 32], [u8; 32]) {
    let (_, connection) = stream.get_ref();
    let certificate = &connection.peer_certificates().expect("certificate")[0];
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate).expect("x509");
    let spki = SpkiPin::of(parsed.public_key().raw);
    let exporter = connection
        .export_keying_material([0_u8; 32], EXPORTER_LABEL, Some(b""))
        .expect("exporter");
    (*spki.as_bytes(), exporter)
}

fn copy(secret: &PairingSecret) -> PairingSecret {
    PairingSecret::from_bytes(*secret.expose())
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).expect("random");
    bytes
}

fn device(name: &str) -> DeviceMetadata {
    DeviceMetadata::new(DeviceName::parse(name).expect("name"), Platform::Linux)
}

fn post(path: &str, host: &str, body: &str) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn authorized(method: &str, path: &str, host: &str, token: &str) -> Vec<u8> {
    format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n\r\n")
        .into_bytes()
}

/// A client attempt on `stream`, for the server `server_id`.
fn attempt<S>(
    stream: &TlsStream<S>,
    server_id: [u8; 16],
    secret: &PairingSecret,
    name: &str,
) -> ClientPairing {
    let (spki, exporter) = handshake_material(stream);
    ClientPairing::new(
        copy(secret),
        device(name),
        server_id,
        spki,
        exporter,
        random(),
    )
}

async fn begin<S>(stream: &mut TlsStream<S>, host: &str, pairing: &ClientPairing) -> Reply
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let body = serde_json::to_string(&pairing.begin_request()).expect("json");
    exchange(stream, &post("/api/v1/pair/begin", host, &body)).await
}

async fn complete<S>(stream: &mut TlsStream<S>, host: &str, request: &CompleteRequest) -> Reply
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let body = serde_json::to_string(request).expect("json");
    exchange(stream, &post("/api/v1/pair/complete", host, &body)).await
}

async fn info<S>(stream: &mut TlsStream<S>, host: &str) -> InfoResponse
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let reply = exchange(stream, &get("/api/v1/pair/info", host)).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    serde_json::from_str(&reply.body).expect("exactly the info fields")
}

/// Everything a client does, on one connection: info, begin, prove,
/// complete, verify `proofS`. `Err` carries the refusal.
async fn pair_on<S>(
    stream: &mut TlsStream<S>,
    host: &str,
    secret: &PairingSecret,
    name: &str,
) -> Result<Paired, Reply>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let server_id = info(stream, host).await.server_id.0;
    let pairing = attempt(stream, server_id, secret, name);
    let begun = begin(stream, host, &pairing).await;
    if begun.status != 200 {
        return Err(begun);
    }
    let begun: BeginResponse = serde_json::from_str(&begun.body).expect("begin response");
    let (request, awaiting) = pairing.prove(&begun);
    let completed = complete(stream, host, &request).await;
    if completed.status != 200 {
        return Err(completed);
    }
    let response: CompleteResponse = serde_json::from_str(&completed.body).expect("complete");
    Ok(awaiting.finish(response).expect("proofS verifies"))
}

async fn pair(server: &Server, secret: &PairingSecret, name: &str) -> Result<Paired, Reply> {
    let mut stream = first_contact(server.port).await;
    pair_on(&mut stream, &server.host(), secret, name).await
}

/// The problem body without its request id, for byte-comparing refusals.
fn generic(reply: &Reply) -> String {
    let mut value = reply.json();
    value["requestId"] = serde_json::Value::Null;
    value.to_string()
}

fn assert_rejected(reply: &Reply) {
    assert_problem(reply, 403, "pairing.rejected");
    assert!(reply.header("retry-after").is_none(), "{reply:?}");
}

fn token_text(paired: &Paired) -> String {
    paired.token.encode().to_string()
}

// ---------------------------------------------------------------------------
// Database inspection

fn pairing_columns(server: &Server) -> (bool, bool, bool, u32, bool) {
    let database = server.state().database();
    let row: (i64, i64, i64, u32, Option<Vec<u8>>) = database
        .connection()
        .query_row(
            "SELECT claimed, armed, locked, failures, secret_ciphertext FROM pairing_state",
            (),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("row");
    (row.0 == 1, row.1 == 1, row.2 == 1, row.3, row.4.is_some())
}

fn device_count(server: &Server) -> i64 {
    server
        .state()
        .database()
        .connection()
        .query_row("SELECT count(*) FROM devices", (), |r| r.get(0))
        .expect("count")
}

fn state_files(server: &Server) -> Vec<(PathBuf, Vec<u8>)> {
    let dir = server.state().fixture.layout.state_dir().to_path_buf();
    let mut files = Vec::new();
    collect(&dir, &mut files);
    files
}

fn collect(dir: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).expect("dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Every textual and binary form a value could be stored or logged in.
fn encodings(bytes: &[u8]) -> Vec<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    use base64::Engine as _;
    vec![
        bytes.to_vec(),
        hex::encode(bytes).into_bytes(),
        hex::encode_upper(bytes).into_bytes(),
        STANDARD.encode(bytes).into_bytes(),
        STANDARD_NO_PAD.encode(bytes).into_bytes(),
        URL_SAFE_NO_PAD.encode(bytes).into_bytes(),
    ]
}

fn secret_forms(secret: &PairingSecret) -> Vec<Vec<u8>> {
    let mut forms = encodings(secret.expose());
    forms.push(secret.canonical().as_bytes().to_vec());
    forms.push(secret.display_form().as_bytes().to_vec());
    forms
}

fn assert_absent(files: &[(PathBuf, Vec<u8>)], forms: &[Vec<u8>], what: &str) {
    for (path, bytes) in files {
        for form in forms {
            assert!(
                !contains(bytes, form),
                "{what} found in {} as {:?}",
                path.display(),
                String::from_utf8_lossy(form)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pairing

#[tokio::test]
async fn happy_path_pairs_and_returns_token() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let mut stream = first_contact(server.port).await;

    let before = info(&mut stream, &host).await;
    assert_eq!(before.server_id.0, *server.identity.server_id().as_bytes());
    assert_eq!(before.spki, server.pin.to_string());
    assert_eq!(before.name, "test-server");
    assert_eq!(before.version, crate::VERSION);
    assert!(!before.claimed);
    assert!(before.pairing_open);

    let paired = pair_on(&mut stream, &host, &secret, "Study laptop")
        .await
        .expect("paired");
    // The pin the client keeps is the handshake's, and it is the server's.
    assert_eq!(paired.spki, *server.pin.as_bytes());
    assert_eq!(paired.server_id, *server.identity.server_id().as_bytes());
    let token = token_text(&paired);
    assert_eq!(token.len(), 43, "256 bits, unpadded base64url");

    let me = exchange(&mut stream, &authorized("GET", "/api/v1/me", &host, &token)).await;
    assert_eq!(me.status, 200, "{me:?}");
    assert_common_headers(&me);
    assert_eq!(
        me.json(),
        serde_json::json!({
            "deviceId": paired.device_id.to_hex(),
            "name": "Study laptop",
            "platform": "linux",
            "role": "owner",
            "serverId": server.identity.server_id().to_string(),
        })
    );
    server.stop().await;
}

#[tokio::test]
async fn proof_s_is_verifiable_by_the_client_and_a_wrong_one_is_refused() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let mut stream = first_contact(server.port).await;
    let pairing = attempt(
        &stream,
        *server.identity.server_id().as_bytes(),
        &secret,
        "Laptop",
    );
    let begun: BeginResponse =
        serde_json::from_str(&begin(&mut stream, &host, &pairing).await.body).expect("begin");
    let (request, awaiting) = pairing.prove(&begun);
    let reply = complete(&mut stream, &host, &request).await;
    let mut response: CompleteResponse = serde_json::from_str(&reply.body).expect("complete");
    // The same response with one bit of proofS flipped is untrusted.
    response.proof_s.0[0] ^= 1;
    assert_eq!(awaiting.finish(response).err(), Some(Untrusted));
    server.stop().await;
}

#[tokio::test]
async fn first_pairing_claims_the_server_and_a_second_needs_a_fresh_secret() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let first = pair(&server, &secret, "First").await.expect("first");

    let mut stream = first_contact(server.port).await;
    let after = info(&mut stream, &host).await;
    assert!(after.claimed);
    assert!(!after.pairing_open, "the secret was consumed");
    assert_eq!(pairing_columns(&server), (true, false, false, 0, false));

    // Without a fresh secret: refused, even with the old one.
    let refused = pair_on(&mut stream, &host, &secret, "Second")
        .await
        .expect_err("no armed secret");
    assert_rejected(&refused);

    // A console re-arm pairs a second owner device; the claim stays.
    let fresh = server.state().arm();
    let second = pair(&server, &fresh, "Second").await.expect("second");
    assert_ne!(first.device_id, second.device_id);
    assert_eq!(device_count(&server), 2);
    assert_eq!(pairing_columns(&server), (true, false, false, 0, false));
    server.stop().await;
}

#[tokio::test]
async fn wrong_secret_returns_generic_rejection_identical_to_every_other_refusal() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let wrong = PairingSecret::from_bytes(random());
    let mut stream = first_contact(server.port).await;

    let wrong_secret = pair_on(&mut stream, &host, &wrong, "X")
        .await
        .expect_err("wrong secret");
    assert_rejected(&wrong_secret);

    // An unknown attempt.
    let unknown = complete(
        &mut stream,
        &host,
        &CompleteRequest {
            pairing_id: Id(random()),
            proof_c: Bytes(random()),
        },
    )
    .await;
    // The reserved web profile, and an unknown one.
    let pairing = attempt(
        &stream,
        *server.identity.server_id().as_bytes(),
        &secret,
        "X",
    );
    let mut web = pairing.begin_request();
    web.binding_profile = ProfileField(atrium_pairing::BINDING_PROFILE_WEB_PKI_RESERVED.into());
    let web_reply = exchange(
        &mut stream,
        &post(
            "/api/v1/pair/begin",
            &host,
            &serde_json::to_string(&web).expect("json"),
        ),
    )
    .await;
    let mut other = pairing.begin_request();
    other.binding_profile = ProfileField("atrium-pair-binding/native-tls-exporter-v2".into());
    let other_reply = exchange(
        &mut stream,
        &post(
            "/api/v1/pair/begin",
            &host,
            &serde_json::to_string(&other).expect("json"),
        ),
    )
    .await;
    // An expired secret.
    server.state().advance(SECRET_LIFETIME);
    let expired = pair_on(&mut stream, &host, &secret, "X")
        .await
        .expect_err("expired");

    for reply in [&unknown, &web_reply, &other_reply, &expired] {
        assert_rejected(reply);
        assert_eq!(generic(reply), generic(&wrong_secret), "{reply:?}");
    }
    // Nothing in the refusal names a reason.
    let lowered = wrong_secret.body.to_lowercase();
    for word in [
        "expired",
        "profile",
        "proof",
        "connection",
        "locked",
        "remaining",
    ] {
        assert!(!lowered.contains(word), "{word} in {lowered}");
    }
    server.stop().await;
}

#[tokio::test]
async fn five_failures_lock_pairing_until_the_console_re_arms() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let wrong = PairingSecret::from_bytes(random());
    for n in 1..=5 {
        let refused = pair(&server, &wrong, "Guess").await.expect_err("wrong");
        assert_rejected(&refused);
        let (_, armed, locked, failures, ciphertext) = pairing_columns(&server);
        if n < 5 {
            assert!(armed && !locked && ciphertext);
            assert_eq!(failures, n);
        } else {
            // The fifth destroys the secret and locks.
            assert!(!armed && locked && !ciphertext);
            assert_eq!(failures, 5);
        }
    }
    // The sixth, with the correct secret, is refused like any other.
    let sixth = pair(&server, &secret, "Owner").await.expect_err("locked");
    assert_rejected(&sixth);
    let mut stream = first_contact(server.port).await;
    assert!(!info(&mut stream, &host).await.pairing_open);

    // `atriumctl pair` clears the lock with a new secret.
    let fresh = server.state().arm();
    assert_eq!(pairing_columns(&server), (false, true, false, 0, true));
    pair(&server, &fresh, "Owner")
        .await
        .expect("paired after re-arm");
    server.stop().await;
}

#[tokio::test]
async fn used_secret_and_replayed_complete_are_refused() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let mut stream = first_contact(server.port).await;
    let pairing = attempt(
        &stream,
        *server.identity.server_id().as_bytes(),
        &secret,
        "Once",
    );
    let begun: BeginResponse =
        serde_json::from_str(&begin(&mut stream, &host, &pairing).await.body).expect("begin");
    let (request, _) = pairing.prove(&begun);
    assert_eq!(complete(&mut stream, &host, &request).await.status, 200);
    // The identical complete again, on the same connection.
    assert_rejected(&complete(&mut stream, &host, &request).await);
    // The same secret, a new attempt.
    assert_rejected(
        &pair_on(&mut stream, &host, &secret, "Twice")
            .await
            .expect_err("used"),
    );
    assert_eq!(device_count(&server), 1);
    server.stop().await;
}

#[tokio::test]
async fn expired_secret_is_refused_and_its_ciphertext_deleted() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    server.state().advance(SECRET_LIFETIME - Span::seconds(1));
    let mut stream = first_contact(server.port).await;
    assert!(info(&mut stream, &host).await.pairing_open);
    server.state().advance(Span::seconds(1));
    assert!(!info(&mut stream, &host).await.pairing_open);
    assert_rejected(
        &pair_on(&mut stream, &host, &secret, "Late")
            .await
            .expect_err("expired"),
    );
    // The refused begin deleted the expired ciphertext.
    assert_eq!(pairing_columns(&server), (false, false, false, 0, false));
    server.stop().await;
}

#[tokio::test]
async fn begin_complete_window_expires_after_two_minutes() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let mut stream = first_contact(server.port).await;
    let pairing = attempt(
        &stream,
        *server.identity.server_id().as_bytes(),
        &secret,
        "Slow",
    );
    let begun_reply = begin(&mut stream, &host, &pairing).await;
    let begun: BeginResponse = serde_json::from_str(&begun_reply.body).expect("begin");
    let (request, _) = pairing.prove(&begun);
    server.state().advance(ATTEMPT_WINDOW);
    assert_rejected(&complete(&mut stream, &host, &request).await);
    // Counted against the arming, which is still usable for a new attempt.
    assert_eq!(pairing_columns(&server).3, 1);
    pair_on(&mut stream, &host, &secret, "Prompt")
        .await
        .expect("a fresh attempt within the secret's lifetime");
    server.stop().await;
}

#[tokio::test]
async fn complete_on_a_different_connection_is_refused() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let mut first = first_contact(server.port).await;
    let mut second = first_contact(server.port).await;
    let server_id = *server.identity.server_id().as_bytes();

    // Proof computed for the first connection, sent on the second.
    let pairing = attempt(&first, server_id, &secret, "Moved");
    let begun: BeginResponse =
        serde_json::from_str(&begin(&mut first, &host, &pairing).await.body).expect("begin");
    let (request, _) = pairing.prove(&begun);
    assert_rejected(&complete(&mut second, &host, &request).await);
    // And the attempt is gone: not even its own connection can finish it.
    assert_rejected(&complete(&mut first, &host, &request).await);

    // Proof computed for the second connection's exporter, begun on the
    // first and completed on the second: still refused.
    let pairing = attempt(&second, server_id, &secret, "Moved");
    let begun: BeginResponse =
        serde_json::from_str(&begin(&mut first, &host, &pairing).await.body).expect("begin");
    let (request, _) = pairing.prove(&begun);
    assert_rejected(&complete(&mut second, &host, &request).await);
    assert_eq!(device_count(&server), 0);
    server.stop().await;
}

#[tokio::test]
async fn pairing_routes_require_tls13_and_refuse_before_processing() {
    let server = server().await;
    let host = server.host();
    server.state().arm();
    let mut stream = connect(server.port, server.pin, &[&rustls::version::TLS12]).await;
    let info = exchange(&mut stream, &get("/api/v1/pair/info", &host)).await;
    assert_problem(&info, 403, "request.tls_version_required");
    // A body that would not even parse: the TLS refusal comes first.
    let begin = exchange(&mut stream, &post("/api/v1/pair/begin", &host, "{not json")).await;
    assert_problem(&begin, 403, "request.tls_version_required");
    let complete = exchange(&mut stream, &post("/api/v1/pair/complete", &host, "{}")).await;
    assert_problem(&complete, 403, "request.tls_version_required");
    // The rest of the API keeps its TLS 1.2 floor.
    assert_eq!(
        exchange(&mut stream, &get("/healthz", &host)).await.status,
        200
    );
    server.stop().await;
}

#[tokio::test]
async fn pairing_bodies_are_strict_bounded_and_json_only() {
    let server = server().await;
    let host = server.host();
    server.state().arm();
    let mut stream = first_contact(server.port).await;
    let nonce = "MzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM";
    let profile = atrium_pairing::BINDING_PROFILE_NATIVE;
    for (body, code) in [
        (
            format!(
                r#"{{"deviceName":"a","platform":"linux","clientNonce":"{nonce}","bindingProfile":"{profile}","role":"admin"}}"#
            ),
            "validation.unknown_field",
        ),
        (
            format!(
                r#"{{"deviceName":"a","deviceName":"b","platform":"linux","clientNonce":"{nonce}","bindingProfile":"{profile}"}}"#
            ),
            "validation.invalid_body",
        ),
        (
            format!(
                r#"{{"deviceName":"a\u202eb","platform":"linux","clientNonce":"{nonce}","bindingProfile":"{profile}"}}"#
            ),
            "validation.invalid_body",
        ),
        (
            format!(
                r#"{{"deviceName":"{}","platform":"linux","clientNonce":"{nonce}","bindingProfile":"{profile}"}}"#,
                "n".repeat(65)
            ),
            "validation.invalid_body",
        ),
        (
            format!(
                r#"{{"deviceName":"a","platform":"linux","clientNonce":"{nonce}=","bindingProfile":"{profile}"}}"#
            ),
            "validation.invalid_body",
        ),
    ] {
        let reply = exchange(&mut stream, &post("/api/v1/pair/begin", &host, &body)).await;
        assert_problem(&reply, 400, code);
    }
    let oversized = format!(r#"{{"deviceName":"{}"}}"#, "a".repeat(2000));
    assert_problem(
        &exchange(&mut stream, &post("/api/v1/pair/begin", &host, &oversized)).await,
        413,
        "validation.too_large",
    );
    let plain = format!(
        "POST /api/v1/pair/complete HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/plain\r\n\
         Content-Length: 2\r\n\r\n{{}}"
    );
    assert_problem(
        &exchange(&mut stream, plain.as_bytes()).await,
        415,
        "validation.unsupported_media_type",
    );
    // None of that created an attempt or counted a failure.
    assert_eq!(pairing_columns(&server).3, 0);
    server.stop().await;
}

#[tokio::test]
async fn pairing_keeps_the_host_and_origin_policy() {
    let server = server().await;
    let secret = server.state().arm();
    let mut stream = first_contact(server.port).await;
    let rebinding = exchange(&mut stream, &get("/api/v1/pair/info", "attacker.example")).await;
    assert_problem(&rebinding, 421, "request.host_rejected");
    let pairing = attempt(
        &stream,
        *server.identity.server_id().as_bytes(),
        &secret,
        "X",
    );
    let body = serde_json::to_string(&pairing.begin_request()).expect("json");
    let raw = format!(
        "POST /api/v1/pair/begin HTTP/1.1\r\nHost: {}\r\nOrigin: https://{}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        server.host(),
        server.host(),
        body.len()
    );
    assert_problem(
        &exchange(&mut stream, raw.as_bytes()).await,
        403,
        "request.origin_rejected",
    );
    server.stop().await;
}

#[tokio::test]
async fn recovery_has_no_pairing_route_on_any_method() {
    let server = Server::recovery().await;
    let host = server.host();
    let mut stream = server.connect().await;
    for path in [
        "/api/v1/pair/info",
        "/api/v1/pair/begin",
        "/api/v1/pair/complete",
        "/api/v1/me",
        "/api/v1/devices",
        "/api/v1/devices/00112233445566778899aabbccddeeff",
    ] {
        for method in ["GET", "POST", "DELETE", "PUT"] {
            let raw =
                format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\n\r\n");
            let reply = exchange(&mut stream, raw.as_bytes()).await;
            assert_problem(&reply, 503, "internal.recovery_mode");
        }
    }
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Attempts and limits

#[tokio::test]
async fn attempts_are_bounded_one_per_source() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let server_id = *server.identity.server_id().as_bytes();
    let mut begun = Vec::new();
    for n in 1..=4_u8 {
        let tcp = tcp_from([127, 0, 0, n], server.port).await;
        let mut stream = handshake(tcp, server.pin, &[&rustls::version::TLS13])
            .await
            .expect("tls");
        let pairing = attempt(&stream, server_id, &secret, "Busy");
        let reply = begin(&mut stream, &host, &pairing).await;
        assert_eq!(reply.status, 200);
        let response: BeginResponse = serde_json::from_str(&reply.body).expect("begin");
        let (request, _) = pairing.prove(&response);
        begun.push((stream, request));
    }
    // A fifth source finds every slot taken.
    let tcp = tcp_from([127, 0, 0, 5], server.port).await;
    let mut fifth = handshake(tcp, server.pin, &[&rustls::version::TLS13])
        .await
        .expect("tls");
    let pairing = attempt(&fifth, server_id, &secret, "Late");
    assert_rejected(&begin(&mut fifth, &host, &pairing).await);

    // A source that begins again replaces its own attempt rather than
    // taking another slot; the replaced attempt is gone.
    let (stream, replaced) = &mut begun[0];
    let pairing = attempt(stream, server_id, &secret, "Again");
    let reply = begin(stream, &host, &pairing).await;
    assert_eq!(reply.status, 200, "same source, same slot");
    assert_rejected(&complete(stream, &host, replaced).await);
    let response: BeginResponse = serde_json::from_str(&reply.body).expect("begin");
    let (request, _) = pairing.prove(&response);
    assert_eq!(complete(stream, &host, &request).await.status, 200);

    // The arming is consumed: every other attempt for it is forgotten, even
    // once the console arms a new secret.
    let _fresh = server.state().arm();
    let (stream, stale) = &mut begun[1];
    assert_rejected(&complete(stream, &host, stale).await);
    assert_eq!(device_count(&server), 1);
    server.stop().await;
}

#[tokio::test]
async fn pairing_requests_are_rate_limited_per_source_and_headers_do_not_help() {
    let server = Server::normal().await;
    let host = server.host();
    let tcp = tcp_from([127, 0, 0, 1], server.port).await;
    let mut stream = handshake(tcp, server.pin, &[&rustls::version::TLS13])
        .await
        .expect("tls");
    let burst = PairingRates::DEFAULT.per_source.burst;
    for _ in 0..burst {
        assert_eq!(
            exchange(&mut stream, &get("/api/v1/pair/info", &host))
                .await
                .status,
            200
        );
    }
    let limited = exchange(&mut stream, &get("/api/v1/pair/info", &host)).await;
    assert_problem(&limited, 403, "pairing.rejected");
    let wait: u64 = limited
        .header("retry-after")
        .expect("retry-after")
        .parse()
        .expect("seconds");
    assert!((1..=60).contains(&wait));
    assert!(!limited.body.contains("remaining") && !limited.body.contains("limit"));

    // Claiming to be someone else changes nothing: the source is the socket.
    for header in [
        "X-Forwarded-For: 10.9.8.7",
        "Forwarded: for=10.9.8.7",
        "X-Real-IP: 10.9.8.7",
    ] {
        let raw = format!("GET /api/v1/pair/info HTTP/1.1\r\nHost: {host}\r\n{header}\r\n\r\n");
        assert_problem(
            &exchange(&mut stream, raw.as_bytes()).await,
            403,
            "pairing.rejected",
        );
    }
    // Refused before the body is read: a declared huge body is not a 413.
    let raw = format!(
        "POST /api/v1/pair/begin HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Content-Length: 99999999\r\n\r\n"
    );
    assert_problem(
        &exchange(&mut stream, raw.as_bytes()).await,
        403,
        "pairing.rejected",
    );
    // Another source has its own budget, and the rest of the API is not
    // limited by it.
    let other = tcp_from([127, 0, 0, 2], server.port).await;
    let mut other = handshake(other, server.pin, &[&rustls::version::TLS13])
        .await
        .expect("tls");
    assert_eq!(
        exchange(&mut other, &get("/api/v1/pair/info", &host))
            .await
            .status,
        200
    );
    let again = tcp_from([127, 0, 0, 1], server.port).await;
    let mut again = handshake(again, server.pin, &[&rustls::version::TLS13])
        .await
        .expect("tls");
    assert_eq!(
        exchange(&mut again, &get("/healthz", &host)).await.status,
        200
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Tokens and devices

#[tokio::test]
async fn devices_authenticate_list_safely_and_revoke_immediately() {
    let server = server().await;
    let host = server.host();
    let first = pair(&server, &server.state().arm(), "Desk")
        .await
        .expect("first");
    let second = pair(&server, &server.state().arm(), "Phone")
        .await
        .expect("second");
    let (one_token, two_token) = (token_text(&first), token_text(&second));
    let mut stream = server.connect().await;

    let list = exchange(
        &mut stream,
        &authorized("GET", "/api/v1/devices", &host, &one_token),
    )
    .await;
    assert_eq!(list.status, 200, "{list:?}");
    let value = list.json();
    let items = value["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert!(value["nextCursor"].is_null());
    for item in items {
        let mut keys: Vec<&str> = item
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "createdAt",
                "current",
                "deviceId",
                "lastSeenAt",
                "name",
                "platform",
                "role"
            ]
        );
        assert_eq!(
            item["current"],
            item["deviceId"] == first.device_id.to_hex().as_str()
        );
    }
    // No verifier material in any form.
    for token in [&first.token, &second.token] {
        let mut forms = encodings(token.digest().as_bytes());
        forms.extend(encodings(token.encode().as_bytes()));
        for form in &forms {
            assert!(!contains(list.body.as_bytes(), form));
        }
    }

    // The first revokes the second; the second's very next request fails,
    // and so do the next hundred.
    let revoke = exchange(
        &mut stream,
        &authorized(
            "DELETE",
            &format!("/api/v1/devices/{}", second.device_id.to_hex()),
            &host,
            &one_token,
        ),
    )
    .await;
    assert_eq!(revoke.status, 204, "{revoke:?}");
    assert!(revoke.body.is_empty());
    for _ in 0..100 {
        let reply = exchange(
            &mut stream,
            &authorized("GET", "/api/v1/me", &host, &two_token),
        )
        .await;
        assert_problem(&reply, 401, "auth.unauthorized");
    }
    // Revoking it again, or anything unknown, is not found.
    let again = exchange(
        &mut stream,
        &authorized(
            "DELETE",
            &format!("/api/v1/devices/{}", second.device_id.to_hex()),
            &host,
            &one_token,
        ),
    )
    .await;
    assert_problem(&again, 404, "request.not_found");
    // The survivor still works and can revoke itself.
    assert_eq!(
        exchange(
            &mut stream,
            &authorized("GET", "/api/v1/me", &host, &one_token)
        )
        .await
        .status,
        200
    );
    let own = exchange(
        &mut stream,
        &authorized(
            "DELETE",
            &format!("/api/v1/devices/{}", first.device_id.to_hex()),
            &host,
            &one_token,
        ),
    )
    .await;
    assert_eq!(own.status, 204);
    assert_problem(
        &exchange(
            &mut stream,
            &authorized("GET", "/api/v1/me", &host, &one_token),
        )
        .await,
        401,
        "auth.unauthorized",
    );
    assert_eq!(device_count(&server), 0);
    server.stop().await;
}

#[tokio::test]
async fn revoked_unknown_and_malformed_tokens_are_indistinguishable() {
    let server = server().await;
    let host = server.host();
    let paired = pair(&server, &server.state().arm(), "Gone")
        .await
        .expect("paired");
    let token = token_text(&paired);
    let database = server.state().database();
    database
        .connection()
        .execute("DELETE FROM devices", ())
        .expect("revoke");
    let mut stream = server.connect().await;
    let unknown = atrium_pairing::token::DeviceToken::from_bytes(random()).encode();
    let replies = [
        exchange(&mut stream, &authorized("GET", "/api/v1/me", &host, &token)).await,
        exchange(
            &mut stream,
            &authorized("GET", "/api/v1/me", &host, &unknown),
        )
        .await,
        exchange(&mut stream, &authorized("GET", "/api/v1/me", &host, "AAAA")).await,
        exchange(&mut stream, &get("/api/v1/me", &host)).await,
    ];
    for reply in &replies {
        assert_problem(reply, 401, "auth.unauthorized");
        assert_eq!(generic(reply), generic(&replies[0]));
        let mut names: Vec<String> = reply
            .headers
            .iter()
            .map(|(n, _)| n.to_ascii_lowercase())
            .filter(|n| n != "date")
            .collect();
        names.sort();
        let mut first: Vec<String> = replies[0]
            .headers
            .iter()
            .map(|(n, _)| n.to_ascii_lowercase())
            .filter(|n| n != "date")
            .collect();
        first.sort();
        assert_eq!(names, first);
    }
    server.stop().await;
}

#[tokio::test]
async fn existence_under_api_v1_is_device_only_information() {
    let server = server().await;
    let host = server.host();
    let token = token_text(
        &pair(&server, &server.state().arm(), "Probe")
            .await
            .expect("paired"),
    );
    let mut stream = server.connect().await;
    for (method, path) in [
        ("GET", "/api/v1/nothing"),
        ("DELETE", "/api/v1/devices/not-an-id"),
        ("DELETE", "/api/v1/devices/00112233445566778899aabbccddeeff"),
    ] {
        let raw = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n");
        assert_problem(
            &exchange(&mut stream, raw.as_bytes()).await,
            401,
            "auth.unauthorized",
        );
        assert_problem(
            &exchange(&mut stream, &authorized(method, path, &host, &token)).await,
            404,
            "request.not_found",
        );
    }
    let wrong = exchange(
        &mut stream,
        &authorized("POST", "/api/v1/devices", &host, &token),
    )
    .await;
    assert_problem(&wrong, 405, "request.method_not_allowed");
    assert_eq!(wrong.header("allow"), Some("GET"));
    // A device with Origin set is refused like any browser.
    let raw = format!(
        "GET /api/v1/me HTTP/1.1\r\nHost: {host}\r\nOrigin: https://{host}\r\n\
         Authorization: Bearer {token}\r\n\r\n"
    );
    assert_problem(
        &exchange(&mut stream, raw.as_bytes()).await,
        403,
        "request.origin_rejected",
    );
    server.stop().await;
}

#[tokio::test]
async fn database_contains_no_raw_token_and_no_secret() {
    let server = server().await;
    let secret = server.state().arm();
    // Armed: only ciphertext on disk.
    assert_absent(&state_files(&server), &secret_forms(&secret), "the secret");
    let paired = pair(&server, &secret, "Scan").await.expect("paired");
    let files = state_files(&server);
    assert_absent(&files, &secret_forms(&secret), "the secret");
    assert_absent(
        &files,
        &encodings(paired.token.encode().as_bytes()),
        "the token text",
    );
    // The raw 32 bytes, in every encoding.
    let bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(paired.token.encode().as_bytes())
            .expect("base64")
    };
    assert_absent(&files, &encodings(&bytes), "the raw token");
    // The verifier is there, as the one 32-byte blob.
    let stored: Vec<u8> = server
        .state()
        .database()
        .connection()
        .query_row("SELECT token_digest FROM devices", (), |r| r.get(0))
        .expect("digest");
    assert_eq!(stored, paired.token.digest().as_bytes().to_vec());
    server.stop().await;
}

// ---------------------------------------------------------------------------
// MITM (criterion 15)

/// Terminates TLS with its own key and relays application bytes to Core,
/// the way an attacker on the path would. Core's `Host` is kept, as with
/// transparent interception.
async fn relay(core_port: u16, core_pin: SpkiPin) -> (u16, SpkiPin) {
    let attacker = identity();
    let attacker_pin = attacker.pin();
    let served = served(&attacker, &[]);
    let config = tls::server_config(Arc::new(CertStore::new(&served))).expect("config");
    let acceptor = TlsAcceptor::from(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut victim) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut upstream = connect(core_port, core_pin, &[&rustls::version::TLS13]).await;
                let _ = tokio::io::copy_bidirectional(&mut victim, &mut upstream).await;
            });
        }
    });
    (port, attacker_pin)
}

#[tokio::test]
async fn a_relaying_mitm_with_its_own_key_cannot_complete_pairing() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let server_id = *server.identity.server_id().as_bytes();
    let (mitm_port, mitm_pin) = relay(server.port, server.pin).await;

    // The victim has the right secret and reaches the attacker, whose
    // certificate it takes as the server's.
    let mut victim = first_contact(mitm_port).await;
    let (seen_spki, _) = handshake_material(&victim);
    assert_eq!(seen_spki, *mitm_pin.as_bytes());
    assert_ne!(seen_spki, *server.pin.as_bytes());
    let refused = pair_on(&mut victim, &host, &secret, "Victim")
        .await
        .expect_err("relayed proof refused upstream");
    assert_rejected(&refused);
    assert_eq!(device_count(&server), 0);

    // An attacker who knows the secret and rewrites proofC upstream still
    // needs Core's SPKI *and* its own upstream exporter. Substituting only
    // one of them fails.
    let mut upstream = first_contact(server.port).await;
    let (core_spki, upstream_exporter) = handshake_material(&upstream);
    let (_, victim_exporter) = handshake_material(&victim);
    for (spki, exporter, case) in [
        (*mitm_pin.as_bytes(), upstream_exporter, "attacker SPKI"),
        (core_spki, victim_exporter, "victim-side exporter"),
    ] {
        let pairing = ClientPairing::new(
            copy(&secret),
            device("Attacker"),
            server_id,
            spki,
            exporter,
            random(),
        );
        let begun: BeginResponse =
            serde_json::from_str(&begin(&mut upstream, &host, &pairing).await.body).expect("begin");
        let (request, _) = pairing.prove(&begun);
        let reply = complete(&mut upstream, &host, &request).await;
        assert_rejected(&reply);
        assert_eq!(device_count(&server), 0, "{case}");
    }
    // Three refusals were counted; the arming is intact for the owner, and
    // the same secret on an honest connection pairs — so what failed above
    // was the binding, not the secret.
    assert_eq!(pairing_columns(&server).3, 3);
    let honest = pair(&server, &secret, "Owner").await.expect("honest");
    assert_eq!(honest.spki, *server.pin.as_bytes());
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Hostile server (criterion 16)

/// A client's durable state: a pin and a token, written only for a pairing
/// the server proved.
struct ClientStore {
    fixture: crate::testutil::Fixture,
}

impl ClientStore {
    fn new() -> Self {
        Self {
            fixture: crate::testutil::Fixture::new("client-store"),
        }
    }

    fn dir(&self) -> &Path {
        self.fixture.layout.state_dir()
    }

    fn keep(&self, paired: &Paired) {
        std::fs::write(self.dir().join("pin"), hex::encode(paired.spki)).expect("pin");
        std::fs::write(self.dir().join("token"), paired.token.encode().as_bytes()).expect("token");
    }

    fn files(&self) -> usize {
        std::fs::read_dir(self.dir()).expect("dir").count()
    }
}

/// What the harness client does end to end: pair, verify the server, and
/// keep the result only if the server proved itself.
async fn pair_and_keep(port: u16, host: &str, secret: &PairingSecret, store: &ClientStore) -> bool {
    let mut stream = first_contact(port).await;
    let server_id = info(&mut stream, host).await.server_id.0;
    let pairing = attempt(&stream, server_id, secret, "Harness");
    let begun = begin(&mut stream, host, &pairing).await;
    let Ok(begun) = serde_json::from_str::<BeginResponse>(&begun.body) else {
        return false;
    };
    let (request, awaiting) = pairing.prove(&begun);
    let completed = complete(&mut stream, host, &request).await;
    let Ok(response) = serde_json::from_str::<CompleteResponse>(&completed.body) else {
        return false;
    };
    match awaiting.finish(response) {
        Ok(paired) => {
            store.keep(&paired);
            true
        }
        Err(Untrusted) => false,
    }
}

/// Completes TLS, answers `info` and `begin` well, and answers `complete`
/// with a well-formed response whose `proofS` is wrong.
async fn hostile_server() -> u16 {
    let impostor = identity();
    let served = served(&impostor, &[]);
    let config = tls::server_config(Arc::new(CertStore::new(&served))).expect("config");
    let acceptor = TlsAcceptor::from(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let server_id = *impostor.server_id().as_bytes();
    let pin = impostor.pin().to_string();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let Ok(mut stream) = acceptor.accept(tcp).await else {
                continue;
            };
            let pin = pin.clone();
            tokio::spawn(async move {
                loop {
                    let Some(path) = read_request(&mut stream).await else {
                        return;
                    };
                    let body = match path.as_str() {
                        "/api/v1/pair/info" => serde_json::to_string(&InfoResponse {
                            server_id: Id(server_id),
                            name: "atrium".into(),
                            version: crate::VERSION.into(),
                            spki: pin.clone(),
                            claimed: false,
                            pairing_open: true,
                        }),
                        "/api/v1/pair/begin" => serde_json::to_string(&BeginResponse {
                            pairing_id: Id(random()),
                            server_nonce: Bytes(random()),
                            expires_at: "2099-01-01T00:00:00Z".into(),
                        }),
                        _ => serde_json::to_string(&CompleteResponse {
                            proof_s: Bytes(random()),
                            device_id: Id(random()),
                            device_token: atrium_pairing::token::DeviceToken::from_bytes(random())
                                .encode()
                                .to_string(),
                            role: "owner".into(),
                            server: ServerRef {
                                id: Id(server_id),
                                name: "atrium".into(),
                            },
                        }),
                    }
                    .expect("json");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if stream.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// Reads one request head and its `Content-Length` body; returns the path.
async fn read_request<S>(stream: &mut S) -> Option<String>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let path = head.split(' ').nth(1)?.to_owned();
    let length: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().to_owned())
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buffer.len() - (end + 4);
    while body < length {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        body += read;
    }
    Some(path)
}

#[tokio::test]
async fn a_hostile_server_with_a_wrong_proof_s_leaves_the_client_with_nothing() {
    let port = hostile_server().await;
    let store = ClientStore::new();
    let secret = PairingSecret::from_bytes(random());
    let kept = pair_and_keep(port, &format!("localhost:{port}"), &secret, &store).await;
    assert!(!kept, "the client rejects the server");
    assert_eq!(store.files(), 0, "no pin, no token, no paired state");

    // The same harness against the real server keeps exactly a pin and a
    // token, so the empty store above is the harness refusing, not failing.
    let server = server().await;
    let secret = server.state().arm();
    assert!(pair_and_keep(server.port, &server.host(), &secret, &store).await);
    assert_eq!(store.files(), 2);
    let pin = std::fs::read_to_string(store.dir().join("pin")).expect("pin");
    assert_eq!(pin, server.pin.to_string());
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Secrecy

#[tokio::test]
async fn nothing_secret_reaches_the_audit_or_any_response() {
    let server = server().await;
    let host = server.host();
    let secret = server.state().arm();
    let mut stream = first_contact(server.port).await;
    let (_, exporter) = handshake_material(&stream);
    let server_id = info(&mut stream, &host).await.server_id.0;
    // One failed attempt and one successful one, on one connection.
    let wrong = attempt(
        &stream,
        server_id,
        &PairingSecret::from_bytes(random()),
        "Bad",
    );
    let begun: BeginResponse =
        serde_json::from_str(&begin(&mut stream, &host, &wrong).await.body).expect("begin");
    let (bad_request, _) = wrong.prove(&begun);
    let refused = complete(&mut stream, &host, &bad_request).await;
    let pairing = attempt(&stream, server_id, &secret, "Good");
    let begun_reply = begin(&mut stream, &host, &pairing).await;
    let begun: BeginResponse = serde_json::from_str(&begun_reply.body).expect("begin");
    let (request, awaiting) = pairing.prove(&begun);
    let completed = complete(&mut stream, &host, &request).await;
    let response: CompleteResponse = serde_json::from_str(&completed.body).expect("complete");
    let proof_s = response.proof_s.0;
    let paired = awaiting.finish(response).expect("paired");
    let list = exchange(
        &mut stream,
        &authorized("GET", "/api/v1/devices", &host, &token_text(&paired)),
    )
    .await;

    let mut forbidden = secret_forms(&secret);
    forbidden.extend(encodings(&request.proof_c.0));
    forbidden.extend(encodings(&bad_request.proof_c.0));
    forbidden.extend(encodings(&proof_s));
    forbidden.extend(encodings(&exporter));
    forbidden.extend(encodings(paired.token.digest().as_bytes()));
    let token_bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(paired.token.encode().as_bytes())
            .expect("token")
    };
    forbidden.extend(encodings(&token_bytes));

    // Every audit row, whole.
    let database = server.state().database();
    let mut statement = database
        .connection()
        .prepare("SELECT action, outcome, coalesce(target,''), coalesce(detail,'') FROM audit")
        .expect("audit");
    let rows: Vec<String> = statement
        .query_map((), |r| {
            Ok(format!(
                "{} {} {} {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?
            ))
        })
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("rows");
    let audit = rows.join("\n");
    for action in [
        "pairing.armed",
        "pairing.failed",
        "pairing.consumed",
        "device.created",
    ] {
        assert!(audit.contains(action), "{action} audited: {audit}");
    }
    // The name the client chose is not copied into the audit either.
    assert!(!audit.contains("Good") && !audit.contains("Bad"));
    for form in &forbidden {
        assert!(
            !contains(audit.as_bytes(), form),
            "secret material in the audit"
        );
        assert!(!contains(refused.body.as_bytes(), form));
        assert!(!contains(list.body.as_bytes(), form));
        assert!(!contains(begun_reply.body.as_bytes(), form));
    }
    server.stop().await;
}
