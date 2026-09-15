use crate::service_webviews::TrustedHealthTarget;
use reqwest::{redirect::Policy, Client, Url};
use serde::{Deserialize, Serialize};
use std::{
    error::Error as StdError,
    net::IpAddr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_millis(2_500);
const MAX_SERVICE_ID_LENGTH: usize = 64;
const MAX_URL_LENGTH: usize = 2_048;
const HEALTH_CHECK_USER_AGENT: &str = "PersonalHub/0.1 health-check";

pub struct HealthClients {
    strict: Client,
    relaxed_local: Client,
}

impl HealthClients {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            strict: build_client(false)?,
            relaxed_local: build_client(true)?,
        })
    }

    fn select(&self, tls_exception_used: bool) -> Client {
        if tls_exception_used {
            self.relaxed_local.clone()
        } else {
            self.strict.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsPolicy {
    #[default]
    Strict,
    AllowInvalidLocalCertificate,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthCheckRequest {
    service_id: String,
    url: String,
    #[serde(default)]
    tls_policy: TlsPolicy,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HealthStatus {
    Online,
    Offline,
    Warning,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HealthReason {
    Timeout,
    Connection,
    Tls,
    TlsException,
    HttpStatus,
    InvalidRequest,
    Request,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckResult {
    pub(crate) service_id: String,
    pub(crate) status: HealthStatus,
    pub(crate) status_code: Option<u16>,
    pub(crate) latency_ms: u64,
    pub(crate) checked_at_unix_ms: u64,
    pub(crate) reason: Option<HealthReason>,
    pub(crate) message: Option<String>,
    pub(crate) tls_exception_used: bool,
}

#[tauri::command]
pub async fn check_service_health(
    caller: tauri::Webview,
    request: HealthCheckRequest,
    clients: tauri::State<'_, HealthClients>,
) -> Result<HealthCheckResult, String> {
    if caller.label() != "main" {
        return Err("Health checks are available only to the trusted Atrium UI.".into());
    }

    let started_at = Instant::now();
    let checked_at_unix_ms = unix_time_ms();
    let service_id = request.service_id.clone();

    let (url, tls_exception_used) = match validate_request(&request) {
        Ok(validated) => validated,
        Err(message) => {
            return Ok(HealthCheckResult {
                service_id,
                status: HealthStatus::Offline,
                status_code: None,
                latency_ms: elapsed_ms(started_at),
                checked_at_unix_ms,
                reason: Some(HealthReason::InvalidRequest),
                message: Some(message),
                tls_exception_used: false,
            });
        }
    };

    Ok(perform_health_check(service_id, url, tls_exception_used, &clients).await)
}

pub(crate) async fn check_trusted_service_health(
    target: TrustedHealthTarget,
    clients: &HealthClients,
) -> HealthCheckResult {
    perform_health_check(
        target.id,
        target.endpoint.url,
        target.endpoint.allow_invalid_local_certificate,
        clients,
    )
    .await
}

async fn perform_health_check(
    service_id: String,
    url: Url,
    tls_exception_used: bool,
    clients: &HealthClients,
) -> HealthCheckResult {
    let started_at = Instant::now();
    let checked_at_unix_ms = unix_time_ms();
    let client = clients.select(tls_exception_used);

    match client.get(url).send().await {
        Ok(response) => {
            let status_code = response.status().as_u16();
            let is_online = (200..=399).contains(&status_code);

            if is_online && tls_exception_used {
                HealthCheckResult {
                    service_id,
                    status: HealthStatus::Warning,
                    status_code: Some(status_code),
                    latency_ms: elapsed_ms(started_at),
                    checked_at_unix_ms,
                    reason: Some(HealthReason::TlsException),
                    message: Some(
                        "Online using the explicitly allowed local certificate exception.".into(),
                    ),
                    tls_exception_used,
                }
            } else if is_online {
                HealthCheckResult {
                    service_id,
                    status: HealthStatus::Online,
                    status_code: Some(status_code),
                    latency_ms: elapsed_ms(started_at),
                    checked_at_unix_ms,
                    reason: None,
                    message: None,
                    tls_exception_used,
                }
            } else {
                HealthCheckResult {
                    service_id,
                    status: HealthStatus::Offline,
                    status_code: Some(status_code),
                    latency_ms: elapsed_ms(started_at),
                    checked_at_unix_ms,
                    reason: Some(HealthReason::HttpStatus),
                    message: Some(format!("The service returned HTTP {status_code}.")),
                    tls_exception_used,
                }
            }
        }
        Err(error) => {
            let (status, reason, message) = classify_request_error(&error);

            HealthCheckResult {
                service_id,
                status,
                status_code: None,
                latency_ms: elapsed_ms(started_at),
                checked_at_unix_ms,
                reason: Some(reason),
                message: Some(message.into()),
                tls_exception_used,
            }
        }
    }
}

fn build_client(tls_exception_used: bool) -> Result<Client, reqwest::Error> {
    let mut builder = Client::builder()
        .timeout(HEALTH_CHECK_TIMEOUT)
        .redirect(Policy::none())
        .http1_only()
        .user_agent(HEALTH_CHECK_USER_AGENT)
        .tls_backend_rustls();

    if tls_exception_used {
        builder = builder.tls_danger_accept_invalid_certs(true);
    }

    builder.build()
}

fn validate_request(request: &HealthCheckRequest) -> Result<(Url, bool), String> {
    if !is_valid_service_id(&request.service_id) {
        return Err("The service id is invalid.".into());
    }

    if request.url.len() > MAX_URL_LENGTH {
        return Err("The service URL is too long.".into());
    }

    let url = Url::parse(&request.url).map_err(|_| "The service URL is invalid.".to_string())?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err("Health checks only support HTTP and HTTPS URLs.".into());
    }

    if url.host_str().is_none() {
        return Err("The service URL must include a hostname.".into());
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err("The service URL must not contain embedded credentials.".into());
    }

    let tls_exception_used = request.tls_policy == TlsPolicy::AllowInvalidLocalCertificate;

    if tls_exception_used {
        if url.scheme() != "https" {
            return Err("A local certificate exception requires an HTTPS URL.".into());
        }

        if !is_local_target(&url) {
            return Err(
                "A local certificate exception is limited to loopback and private-network targets."
                    .into(),
            );
        }
    }

    Ok((url, tls_exception_used))
}

fn is_valid_service_id(service_id: &str) -> bool {
    !service_id.is_empty()
        && service_id.len() <= MAX_SERVICE_ID_LENGTH
        && service_id.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

fn is_local_target(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };

    let normalized_host = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']');

    if normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host.to_ascii_lowercase().ends_with(".localhost")
    {
        return true;
    }

    match normalized_host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) if !address.is_unspecified() => {
            address.is_private() || address.is_loopback() || address.is_link_local()
        }
        Ok(IpAddr::V6(address)) if !address.is_unspecified() => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
        Ok(_) | Err(_) => false,
    }
}

fn classify_request_error(error: &reqwest::Error) -> (HealthStatus, HealthReason, &'static str) {
    if error.is_timeout() {
        return (
            HealthStatus::Offline,
            HealthReason::Timeout,
            "The health check timed out after 2.5 seconds.",
        );
    }

    if error_chain_mentions_tls(error) {
        return (
            HealthStatus::Warning,
            HealthReason::Tls,
            "TLS certificate validation failed.",
        );
    }

    if error.is_connect() {
        return (
            HealthStatus::Offline,
            HealthReason::Connection,
            "The service could not be reached.",
        );
    }

    (
        HealthStatus::Offline,
        HealthReason::Request,
        "The health-check request failed.",
    )
}

fn error_chain_mentions_tls(error: &reqwest::Error) -> bool {
    let mut messages = error.to_string().to_ascii_lowercase();
    let mut source = error.source();

    while let Some(cause) = source {
        messages.push(' ');
        messages.push_str(&cause.to_string().to_ascii_lowercase());
        source = cause.source();
    }

    [
        "certificate",
        "unknown issuer",
        "unknownissuer",
        "invalid peer",
        "invalidpeer",
        "rustls",
        "tls handshake",
    ]
    .iter()
    .any(|needle| messages.contains(needle))
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, tls_policy: TlsPolicy) -> HealthCheckRequest {
        HealthCheckRequest {
            service_id: "test-service".into(),
            url: url.into(),
            tls_policy,
        }
    }

    #[test]
    fn relaxed_tls_accepts_private_and_loopback_https_targets() {
        for url in [
            "https://192.168.1.10:9443",
            "https://10.0.0.2",
            "https://172.16.2.3",
            "https://127.0.0.1",
            "https://[::1]",
            "https://[fd00::1]",
            "https://localhost",
            "https://app.localhost",
        ] {
            assert!(
                validate_request(&request(url, TlsPolicy::AllowInvalidLocalCertificate)).is_ok()
            );
        }
    }

    #[test]
    fn relaxed_tls_rejects_public_non_https_and_credentialed_targets() {
        for url in [
            "https://8.8.8.8",
            "https://example.com",
            "https://0.0.0.0",
            "https://[::]",
            "http://192.168.1.10",
            "https://user:secret@192.168.1.10",
        ] {
            assert!(
                validate_request(&request(url, TlsPolicy::AllowInvalidLocalCertificate)).is_err()
            );
        }
    }

    #[test]
    fn strict_tls_allows_regular_http_and_https_targets() {
        for url in ["http://192.168.1.10:8096", "https://example.com"] {
            let (_, tls_exception_used) =
                validate_request(&request(url, TlsPolicy::Strict)).unwrap();
            assert!(!tls_exception_used);
        }
    }

    #[test]
    fn service_id_validation_matches_the_frontend_contract() {
        assert!(is_valid_service_id("crafty-controller"));
        assert!(is_valid_service_id("service2"));
        assert!(!is_valid_service_id("Crafty"));
        assert!(!is_valid_service_id("double--hyphen"));
        assert!(!is_valid_service_id("contains_space"));
    }
}
