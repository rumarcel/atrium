//! One request, start to finish, in one fixed order.
//!
//! ```text
//! request id      X-Request-Id if 1–64 lowercase hex, else a fresh one
//! Host            exactly one, parsed strictly, in the allowlist      → 421
//! lookup          exact method and path in this mode's table
//! browser         Origin / Sec-Fetch-Site against the route's policy  → 403
//! absent          recovery → 503; /api/v1 → 401, or 404 to a device;
//!                 /api/x → 404; else 404
//! wrong method    device route → 401, or 405 to a device; otherwise 405
//! TLS             route needs 1.3 and this is 1.2                     → 403
//! auth            Device: the one token check                          → 401
//!                 Pairing: the source's and the global budget          → 403
//! body            none allowed and one sent → 400; not JSON → 415;
//!                 over limit → 413
//! handler
//! headers         version, request id and security headers on every response
//! ```
//!
//! No step can be skipped by a route, and nothing before `handler` reads the
//! request body. Device authentication happens in exactly one function,
//! [`authenticate`], which every `Auth::Device` route and every "does this
//! exist?" answer under `/api/v1` goes through; no handler parses a token. Nothing reflects request text into a response or a log:
//! logs carry the request id, a fixed route label, the method if standard,
//! and the status.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body as _, Incoming};
use zeroize::Zeroizing;

use atrium_pairing::wire::Id;

use super::connection::{ConnectionContext, TlsVersion};
use super::error::ApiError;
use super::host::{self, Authority};
use super::routes::{self, Auth, Body, Browser, Endpoint, Lookup, Namespace, ServingMode, Tls};
use super::{App, Services};
use crate::devices::Authenticated;
use crate::pairing::{Arrival, Refused};

/// Longest a handler, including reading a request body, may take.
const HANDLER_TIMEOUT: Duration = Duration::from_secs(10);

/// The placeholder page. No script, no style, no remote resource, nothing
/// that identifies the machine (plan §2.4).
const INDEX: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Atrium</title></head><body><h1>Atrium</h1>\
<p>Atrium Core is running. Its interface arrives in a later release; until then \
use an Atrium client.</p></body></html>\n";

fn request_id(headers: &HeaderMap) -> String {
    let supplied = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            (1..=64).contains(&v.len()) && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        });
    match supplied {
        Some(value) => value.to_owned(),
        None => {
            let mut bytes = [0_u8; 8];
            // A request id is a correlation hint, not a secret; a failed
            // random read falls back to a fixed value rather than failing
            // the request.
            let _ = getrandom::fill(&mut bytes);
            hex::encode(bytes)
        }
    }
}

/// The request's `Host`, validated against the allowlist.
fn validated_host(app: &App, request: &Request<Incoming>) -> Result<Authority, ApiError> {
    let mut values = request.headers().get_all(header::HOST).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(ApiError::HostRejected);
    };
    let authority = value
        .to_str()
        .ok()
        .and_then(host::parse_authority)
        .ok_or(ApiError::HostRejected)?;
    // An absolute-form request line names a host too; it must be the same.
    if let Some(target) = request.uri().authority() {
        if host::parse_authority(target.as_str()).as_ref() != Some(&authority) {
            return Err(ApiError::HostRejected);
        }
    }
    if !app.hosts_admit(&authority) {
        return Err(ApiError::HostRejected);
    }
    Ok(authority)
}

/// Whether a browser context is allowed by `policy`. Native clients send
/// neither header and are unaffected.
fn browser_allowed(policy: Browser, headers: &HeaderMap, host: &Authority) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin = origins.next();
    if origins.next().is_some() {
        return false;
    }
    let cross_site = headers
        .get("sec-fetch-site")
        .is_some_and(|v| v.as_bytes() != b"same-origin" && v.as_bytes() != b"none");
    if cross_site {
        return false;
    }
    match policy {
        Browser::Deny => origin.is_none(),
        Browser::SameOrigin => origin.is_none_or(|value| {
            value
                .to_str()
                .is_ok_and(|value| host::is_same_origin(value, host))
        }),
    }
}

fn json(value: &serde_json::Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(
        serde_json::to_vec(value).unwrap_or_default(),
    )));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

fn serialized<T: serde::Serialize>(value: &T) -> Result<Response<Full<Bytes>>, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ApiError::Internal)?;
    let mut response = Response::new(Full::new(Bytes::from(bytes)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    Ok(response)
}

fn stored_json(bytes: &Bytes) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(bytes.clone()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

/// Reads a JSON body of at most `limit` bytes.
async fn read_body(body: Incoming, limit: usize) -> Result<Bytes, ApiError> {
    let declared = body.size_hint().upper().or(Some(body.size_hint().lower()));
    if declared.is_some_and(|n| n > limit as u64) {
        return Err(ApiError::TooLarge);
    }
    match Limited::new(body, limit).collect().await {
        Ok(collected) => Ok(collected.to_bytes()),
        Err(error) if error.is::<http_body_util::LengthLimitError>() => Err(ApiError::TooLarge),
        Err(_) => Err(ApiError::InvalidBody),
    }
}

/// Whether the request declares exactly one `Content-Type`, and it is
/// `application/json`, optionally with `charset=utf-8`.
fn declares_json(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(header::CONTENT_TYPE).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let compact: String = text
        .chars()
        .filter(|c| *c != ' ' && *c != '\t')
        .collect::<String>()
        .to_ascii_lowercase();
    compact == "application/json" || compact == "application/json;charset=utf-8"
}

/// Parses a strict JSON body, telling an unknown field from other errors.
/// Duplicate fields are refused by the derived deserializers.
fn parse_strict<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|error| {
        if error.to_string().starts_with("unknown field") {
            ApiError::UnknownField
        } else {
            ApiError::InvalidBody
        }
    })
}

/// The central device check: the request's single `Authorization` value,
/// looked up by `crate::devices`. `Ok(None)` for every way of not being a
/// device.
async fn authenticate(app: &App, headers: &HeaderMap) -> Result<Option<Authenticated>, ApiError> {
    let Some(services) = app.services.clone() else {
        // Recovery: nothing can authenticate.
        return Ok(None);
    };
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Ok(None);
    };
    let value = Zeroizing::new(value.as_bytes().to_vec());
    tokio::task::spawn_blocking(move || services.devices.authenticate(Some(&value)))
        .await
        .map_err(|_| ApiError::Internal)?
        .map_err(|_| ApiError::Internal)
}

fn with_allow(mut response: Response<Full<Bytes>>, allow: &[Method]) -> Response<Full<Bytes>> {
    let listed: Vec<&str> = allow.iter().map(Method::as_str).collect();
    if let Ok(value) = HeaderValue::from_str(&listed.join(", ")) {
        response.headers_mut().insert(header::ALLOW, value);
    }
    response
}

async fn handle(
    app: &App,
    connection: &ConnectionContext,
    request: Request<Incoming>,
    request_id: &str,
) -> (Result<Response<Full<Bytes>>, ApiError>, &'static str) {
    let host = match validated_host(app, &request) {
        Ok(host) => host,
        Err(error) => return (Err(error), "host"),
    };
    let mode = app.serving_mode();
    let lookup = routes::lookup(app.routes, mode, request.method(), request.uri().path());
    let browser = match &lookup {
        Lookup::Found(_, policy, _) => policy.browser(),
        _ => Browser::Deny,
    };
    if !browser_allowed(browser, request.headers(), &host) {
        return (Err(ApiError::OriginRejected), "origin");
    }
    let (endpoint, policy, parameter) = match lookup {
        Lookup::Found(endpoint, policy, parameter) => (endpoint, policy, parameter),
        Lookup::Absent(_) if mode == ServingMode::Recovery => {
            return (Err(ApiError::RecoveryMode), "recovery")
        }
        // Whether a path exists under /api/v1 is itself device-only
        // information (criterion 26).
        Lookup::Absent(Namespace::ApiV1) => {
            return match authenticate(app, request.headers()).await {
                Ok(Some(_)) => (Err(ApiError::NotFound), "api"),
                Ok(None) => (Err(ApiError::Unauthorized), "api"),
                Err(error) => (Err(error), "api"),
            };
        }
        Lookup::Absent(Namespace::ApiOther) => {
            return (Err(ApiError::UnsupportedApiVersion), "api-version")
        }
        Lookup::Absent(Namespace::Other) => return (Err(ApiError::NotFound), "absent"),
        Lookup::WrongMethod {
            device: true,
            allow,
        } => {
            return match authenticate(app, request.headers()).await {
                Ok(Some(_)) => (
                    Ok(with_allow(
                        ApiError::MethodNotAllowed.response(request_id),
                        &allow,
                    )),
                    "method",
                ),
                Ok(None) => (Err(ApiError::Unauthorized), "method"),
                Err(error) => (Err(error), "method"),
            };
        }
        Lookup::WrongMethod { allow, .. } => {
            return (
                Ok(with_allow(
                    ApiError::MethodNotAllowed.response(request_id),
                    &allow,
                )),
                "method",
            );
        }
    };
    let label = endpoint_label(endpoint);

    if policy.tls() == Tls::Tls13 && connection.tls() != TlsVersion::Tls13 {
        return (Err(ApiError::TlsVersionRequired), label);
    }
    let device = match policy.auth() {
        Auth::None => None,
        Auth::Device => match authenticate(app, request.headers()).await {
            Ok(Some(device)) => Some(device),
            Ok(None) => return (Err(ApiError::Unauthorized), label),
            Err(error) => return (Err(error), label),
        },
        Auth::Pairing => {
            // Before the body is read or anything is parsed.
            if let Err(wait) = app.pairing_rates.check(connection.source(), Instant::now()) {
                let mut response = ApiError::PairingRejected.response(request_id);
                if let Ok(value) = HeaderValue::from_str(&wait.as_secs().to_string()) {
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
                return (Ok(response), label);
            }
            None
        }
    };
    let body = match policy.body() {
        Body::None => {
            if !request.body().is_end_stream() {
                return (Err(ApiError::BodyNotAccepted), label);
            }
            Bytes::new()
        }
        Body::Json { limit } => {
            if !declares_json(request.headers()) {
                return (Err(ApiError::UnsupportedMediaType), label);
            }
            match tokio::time::timeout(HANDLER_TIMEOUT, read_body(request.into_body(), limit)).await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(error)) => return (Err(error), label),
                Err(_) => return (Err(ApiError::Timeout), label),
            }
        }
    };
    let call = Call {
        connection,
        endpoint,
        parameter,
        device,
        body,
    };
    (endpoint_response(app, call).await, label)
}

fn endpoint_label(endpoint: Endpoint) -> &'static str {
    match endpoint {
        Endpoint::Healthz => "healthz",
        Endpoint::Index => "index",
        Endpoint::SystemDiagnostics => "system-diagnostics",
        Endpoint::PairInfo => "pair-info",
        Endpoint::PairBegin => "pair-begin",
        Endpoint::PairComplete => "pair-complete",
        Endpoint::Me => "me",
        Endpoint::Devices => "devices",
        Endpoint::RevokeDevice => "revoke-device",
        #[cfg(test)]
        Endpoint::TestBinding => "test-binding",
        #[cfg(test)]
        Endpoint::TestJson => "test-json",
    }
}

/// Everything a handler receives, after every check has passed.
struct Call<'a> {
    connection: &'a ConnectionContext,
    endpoint: Endpoint,
    parameter: Option<Id>,
    device: Option<Authenticated>,
    body: Bytes,
}

fn services(app: &App) -> Result<Arc<Services>, ApiError> {
    app.services.clone().ok_or(ApiError::Internal)
}

fn refused(refusal: Refused) -> ApiError {
    match refusal {
        Refused::Rejected => ApiError::PairingRejected,
        Refused::Internal => ApiError::Internal,
    }
}

/// Runs `work` on the blocking pool: every service call touches SQLite.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::Internal)
}

/// What a pairing handler knows about the connection, owned so it can move
/// to the blocking pool. The exporter copy is zeroed on drop.
struct OwnedArrival {
    connection: super::connection::ConnectionId,
    source: super::limits::SourceKey,
    exporter: Option<Zeroizing<[u8; 32]>>,
}

impl OwnedArrival {
    fn of(connection: &ConnectionContext) -> Self {
        Self {
            connection: connection.id(),
            source: connection.source(),
            exporter: connection
                .binding()
                .map(|binding| Zeroizing::new(*binding.exporter())),
        }
    }

    fn view(&self) -> Arrival<'_> {
        Arrival {
            connection: self.connection,
            source: self.source,
            exporter: self.exporter.as_deref(),
        }
    }
}

async fn endpoint_response(app: &App, call: Call<'_>) -> Result<Response<Full<Bytes>>, ApiError> {
    let Call {
        connection,
        endpoint,
        parameter,
        device,
        body,
    } = call;
    match (endpoint, &app.mode) {
        (Endpoint::Healthz, super::AppMode::Normal) => Ok(json(&serde_json::json!({
            "status": "ok",
            "state": "normal",
            "api": atrium_api_types::API_VERSION,
            "version": crate::VERSION,
        }))),
        (Endpoint::Healthz, super::AppMode::Recovery { health, .. }) => Ok(stored_json(health)),
        (Endpoint::Index, super::AppMode::Normal) => {
            let mut response = Response::new(Full::new(Bytes::from_static(INDEX.as_bytes())));
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            Ok(response)
        }
        (Endpoint::SystemDiagnostics, super::AppMode::Recovery { diagnostics, .. }) => {
            Ok(stored_json(diagnostics))
        }
        // Normal-mode diagnostics is a device route whose handler is M1F's;
        // until then an authenticated device finds nothing there.
        (Endpoint::SystemDiagnostics, super::AppMode::Normal) => Err(ApiError::NotFound),
        // The pairing and device routes do not exist in recovery, so these
        // arms are reached in normal mode only.
        (Endpoint::PairInfo, _) => {
            let services = services(app)?;
            let info = blocking(move || services.pairing.info())
                .await?
                .map_err(refused)?;
            serialized(&info)
        }
        (Endpoint::PairBegin, _) => {
            let request: atrium_pairing::wire::BeginRequest = parse_strict(&body)?;
            let services = services(app)?;
            let arrival = OwnedArrival::of(connection);
            let begun = blocking(move || services.pairing.begin(&arrival.view(), request))
                .await?
                .map_err(refused)?;
            serialized(&begun)
        }
        (Endpoint::PairComplete, _) => {
            let request: atrium_pairing::wire::CompleteRequest = parse_strict(&body)?;
            let services = services(app)?;
            let arrival = OwnedArrival::of(connection);
            let completed = blocking(move || services.pairing.complete(&arrival.view(), &request))
                .await?
                .map_err(refused)?;
            serialized(&completed)
        }
        (Endpoint::Me, _) => {
            let device = device.ok_or(ApiError::Unauthorized)?;
            serialized(&services(app)?.devices.me(&device))
        }
        (Endpoint::Devices, _) => {
            let device = device.ok_or(ApiError::Unauthorized)?;
            let services = services(app)?;
            let list = blocking(move || services.devices.list(&device))
                .await?
                .map_err(|_| ApiError::Internal)?;
            serialized(&list)
        }
        (Endpoint::RevokeDevice, _) => {
            let device = device.ok_or(ApiError::Unauthorized)?;
            let target = parameter.ok_or(ApiError::NotFound)?.to_hex();
            let services = services(app)?;
            let revoked = blocking(move || services.devices.revoke(&device, &target))
                .await?
                .map_err(|_| ApiError::Internal)?;
            if !revoked {
                return Err(ApiError::NotFound);
            }
            let mut response = Response::new(Full::new(Bytes::new()));
            *response.status_mut() = StatusCode::NO_CONTENT;
            Ok(response)
        }
        // A recovery-only index cannot be reached.
        (Endpoint::Index, _) => Err(ApiError::NotFound),
        #[cfg(test)]
        (Endpoint::TestBinding, _) => {
            use sha2::Digest;
            let binding = connection.binding().ok_or(ApiError::Internal)?;
            let mut digest = sha2::Sha256::new();
            digest.update(b"atrium-test-binding");
            digest.update(binding.exporter());
            Ok(json(&serde_json::json!({
                "connection": format!("{:x}", connection.id().as_u128()),
                "bindingDigest": hex::encode(digest.finalize()),
            })))
        }
        #[cfg(test)]
        (Endpoint::TestJson, _) => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Echo {
                name: String,
            }
            let echo: Echo = parse_strict(&body)?;
            Ok(json(&serde_json::json!({ "name": echo.name })))
        }
    }
}

/// Headers every response carries, success or error.
fn finish(response: &mut Response<Full<Bytes>>, request_id: &str) {
    let headers = response.headers_mut();
    let fixed: [(&str, &'static str); 8] = [
        ("x-atrium-api", "1"),
        ("x-atrium-version", crate::VERSION),
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
        ("cross-origin-resource-policy", "same-origin"),
    ];
    for (name, value) in fixed {
        headers.insert(name, HeaderValue::from_static(value));
    }
    if let Ok(value) = HeaderValue::from_str(request_id) {
        headers.insert("x-request-id", value);
    }
}

fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::OPTIONS => "OPTIONS",
        _ => "other",
    }
}

/// Serves one request.
pub(super) async fn dispatch(
    app: &App,
    connection: &ConnectionContext,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    let request_id = request_id(request.headers());
    let method = method_label(request.method());
    let (result, route) = handle(app, connection, request, &request_id).await;
    let mut response = match result {
        Ok(response) => response,
        Err(error) => error.response(&request_id),
    };
    finish(&mut response, &request_id);
    tracing::debug!(
        event = "http_request",
        component = "atrium-core",
        request_id = %request_id,
        method = method,
        route = route,
        status = response.status().as_u16(),
        "served a request"
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_accepted_only_in_the_planned_shape() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("3f1c"));
        assert_eq!(request_id(&headers), "3f1c");
        for bad in ["3F1C", "3f1c-", "<script>", ""] {
            let mut headers = HeaderMap::new();
            headers.insert("x-request-id", HeaderValue::from_str(bad).expect("header"));
            let id = request_id(&headers);
            assert_ne!(id, bad);
            assert_eq!(id.len(), 16);
        }
    }

    #[test]
    fn browser_policy() {
        let host = host::parse_authority("localhost:7443").expect("host");
        let with = |pairs: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.append(*name, HeaderValue::from_static(value));
            }
            headers
        };
        let native = with(&[]);
        assert!(browser_allowed(Browser::Deny, &native, &host));
        assert!(browser_allowed(Browser::SameOrigin, &native, &host));

        let own = with(&[("origin", "https://localhost:7443")]);
        assert!(!browser_allowed(Browser::Deny, &own, &host));
        assert!(browser_allowed(Browser::SameOrigin, &own, &host));

        for hostile in [
            with(&[("origin", "https://attacker.example")]),
            with(&[("origin", "null")]),
            with(&[("origin", "http://localhost:7443")]),
            with(&[("sec-fetch-site", "cross-site")]),
            with(&[("sec-fetch-site", "same-site")]),
            with(&[
                ("origin", "https://localhost:7443"),
                ("origin", "https://attacker.example"),
            ]),
        ] {
            assert!(!browser_allowed(Browser::Deny, &hostile, &host));
            assert!(!browser_allowed(Browser::SameOrigin, &hostile, &host));
        }
        let navigation = with(&[("sec-fetch-site", "none")]);
        assert!(browser_allowed(Browser::SameOrigin, &navigation, &host));
    }

    #[test]
    fn the_placeholder_identifies_nothing() {
        assert!(!INDEX.contains(crate::VERSION));
        assert!(!INDEX.contains("<script"));
        assert!(!INDEX.contains("http"));
    }
}
