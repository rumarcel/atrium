//! One request, start to finish, in one fixed order.
//!
//! ```text
//! request id      X-Request-Id if 1–64 lowercase hex, else a fresh one
//! Host            exactly one, parsed strictly, in the allowlist      → 421
//! lookup          exact method and path in this mode's table
//! browser         Origin / Sec-Fetch-Site against the route's policy  → 403
//! absent          recovery → 503; /api/v1 → 401; /api/x → 404; else 404
//! wrong method    device route → 401; otherwise 405 with Allow
//! TLS             route needs 1.3 and this is 1.2                     → 403
//! auth            Device → 401 (no device can authenticate in M1D)
//! body            none allowed and one sent → 400; JSON over limit → 413
//! handler
//! headers         version, request id and security headers on every response
//! ```
//!
//! No step can be skipped by a route, and nothing before `handler` reads the
//! request body. Nothing reflects request text into a response or a log:
//! logs carry the request id, a fixed route label, the method if standard,
//! and the status.

use std::time::Duration;

use bytes::Bytes;
use http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body as _, Incoming};

use super::connection::{ConnectionContext, TlsVersion};
use super::error::ApiError;
use super::host::{self, Authority};
use super::routes::{self, Auth, Body, Browser, Endpoint, Lookup, Namespace, ServingMode, Tls};
use super::App;

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

/// Parses a strict JSON body, telling an unknown field from other errors.
#[cfg(test)]
fn parse_strict<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|error| {
        if error.to_string().starts_with("unknown field") {
            ApiError::UnknownField
        } else {
            ApiError::InvalidBody
        }
    })
}

async fn handle(
    app: &App,
    connection: &ConnectionContext,
    request: Request<Incoming>,
) -> (Result<Response<Full<Bytes>>, ApiError>, &'static str) {
    let host = match validated_host(app, &request) {
        Ok(host) => host,
        Err(error) => return (Err(error), "host"),
    };
    let mode = app.serving_mode();
    let lookup = routes::lookup(app.routes, mode, request.method(), request.uri().path());
    let browser = match &lookup {
        Lookup::Found(_, policy) => policy.browser(),
        _ => Browser::Deny,
    };
    if !browser_allowed(browser, request.headers(), &host) {
        return (Err(ApiError::OriginRejected), "origin");
    }
    let (endpoint, policy) = match lookup {
        Lookup::Found(endpoint, policy) => (endpoint, policy),
        Lookup::Absent(_) if mode == ServingMode::Recovery => {
            return (Err(ApiError::RecoveryMode), "recovery")
        }
        Lookup::Absent(Namespace::ApiV1) => return (Err(ApiError::Unauthorized), "api"),
        Lookup::Absent(Namespace::ApiOther) => {
            return (Err(ApiError::UnsupportedApiVersion), "api-version")
        }
        Lookup::Absent(Namespace::Other) => return (Err(ApiError::NotFound), "absent"),
        Lookup::WrongMethod { device: true, .. } => return (Err(ApiError::Unauthorized), "method"),
        Lookup::WrongMethod { allow, .. } => {
            let mut response = ApiError::MethodNotAllowed.response("");
            let listed: Vec<&str> = allow.iter().map(Method::as_str).collect();
            if let Ok(value) = HeaderValue::from_str(&listed.join(", ")) {
                response.headers_mut().insert(header::ALLOW, value);
            }
            return (Ok(response), "method");
        }
    };
    let label = endpoint_label(endpoint);

    if policy.tls() == Tls::Tls13 && connection.tls() != TlsVersion::Tls13 {
        return (Err(ApiError::TlsVersionRequired), label);
    }
    if policy.auth() == Auth::Device {
        // M1D: no device can authenticate. M1E verifies the token here.
        return (Err(ApiError::Unauthorized), label);
    }
    let body = match policy.body() {
        Body::None => {
            if !request.body().is_end_stream() {
                return (Err(ApiError::BodyNotAccepted), label);
            }
            Bytes::new()
        }
        Body::Json { limit } => {
            match tokio::time::timeout(HANDLER_TIMEOUT, read_body(request.into_body(), limit)).await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(error)) => return (Err(error), label),
                Err(_) => return (Err(ApiError::Timeout), label),
            }
        }
    };
    (endpoint_response(app, connection, endpoint, &body), label)
}

fn endpoint_label(endpoint: Endpoint) -> &'static str {
    match endpoint {
        Endpoint::Healthz => "healthz",
        Endpoint::Index => "index",
        Endpoint::SystemDiagnostics => "system-diagnostics",
        #[cfg(test)]
        Endpoint::TestBinding => "test-binding",
        #[cfg(test)]
        Endpoint::TestJson => "test-json",
    }
}

#[allow(clippy::unnecessary_wraps)]
fn endpoint_response(
    app: &App,
    connection: &ConnectionContext,
    endpoint: Endpoint,
    body: &Bytes,
) -> Result<Response<Full<Bytes>>, ApiError> {
    let _ = (connection, body);
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
        // Normal-mode diagnostics is a device route (M1F); the policy has
        // already refused it. A recovery-only index cannot be reached either.
        (Endpoint::SystemDiagnostics | Endpoint::Index, _) => Err(ApiError::Unauthorized),
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
            let echo: Echo = parse_strict(body)?;
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
    let (result, route) = handle(app, connection, request).await;
    let mut response = match result {
        Ok(response) => response,
        Err(error) => error.response(&request_id),
    };
    if response.status() == StatusCode::METHOD_NOT_ALLOWED {
        // Built before the id was known to the error; rebuild the body with it.
        let allow = response.headers().get(header::ALLOW).cloned();
        response = ApiError::MethodNotAllowed.response(&request_id);
        if let Some(allow) = allow {
            response.headers_mut().insert(header::ALLOW, allow);
        }
    }
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
