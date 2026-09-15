use crate::{
    provider_auth::ProviderAuthManager, service_settings::ServiceApiAuthentication,
    service_webviews::ServiceCatalog,
};
use reqwest::{redirect::Policy, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tauri::{Url, Webview};

const MAIN_WEBVIEW_LABEL: &str = "main";
const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(5_000);
const DISCOVERY_USER_AGENT: &str = "PersonalHub/0.1 service-discovery";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CANDIDATES: usize = 500;
const MAX_SOURCE_ID_LENGTH: usize = 200;
const MAX_NAME_LENGTH: usize = 80;
const MAX_DESCRIPTION_LENGTH: usize = 160;
const MAX_URL_LENGTH: usize = 2_048;
const MAX_ICON_HINT_LENGTH: usize = 512;

pub struct ServiceDiscoveryClients {
    strict: Client,
    relaxed_local: Client,
}

impl ServiceDiscoveryClients {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            strict: build_client(false)?,
            relaxed_local: build_client(true)?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverHomarrServicesRequest {
    service_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDiscoveryCandidate {
    pub(crate) source_id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) icon_hint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDiscoveryResponse {
    pub(crate) source: &'static str,
    pub(crate) source_service_id: String,
    pub(crate) candidates: Vec<ServiceDiscoveryCandidate>,
    pub(crate) skipped_count: usize,
}

#[tauri::command]
pub async fn discover_homarr_services(
    caller: Webview,
    request: DiscoverHomarrServicesRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    authentication: tauri::State<'_, ProviderAuthManager>,
    clients: tauri::State<'_, ServiceDiscoveryClients>,
) -> Result<ServiceDiscoveryResponse, String> {
    ensure_trusted_caller(&caller)?;
    validate_service_id(&request.service_id)?;

    let target = catalog.resolve_authentication_target(&request.service_id)?;
    if !target.enabled {
        return Err("The selected Homarr service is disabled.".into());
    }
    if target.authentication.api != ServiceApiAuthentication::HomarrApiKey {
        return Err("The selected service is not configured for Homarr API authentication.".into());
    }

    let material = authentication
        .resolve_api_authentication(&target)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Homarr API authentication is not configured.".to_string())?;
    let endpoint = provider_url(&target.url, "api/apps")
        .ok_or_else(|| "The Homarr applications endpoint is invalid.".to_string())?;
    let client = if target.allow_invalid_local_certificate {
        &clients.relaxed_local
    } else {
        &clients.strict
    };
    let request_builder = material
        .apply(client.get(endpoint.clone()), &endpoint)
        .map_err(|error| error.to_string())?;
    let response = request_builder.send().await.map_err(|error| {
        if error.is_timeout() {
            "Homarr discovery timed out.".to_string()
        } else if error.is_connect() {
            "Homarr could not be reached.".to_string()
        } else {
            "Homarr discovery could not be completed.".to_string()
        }
    })?;

    if response.status() == StatusCode::UNAUTHORIZED {
        authentication.report_auth_failure(&target.service_id, material.revision());
        return Err("Homarr rejected the stored API key.".into());
    }
    if response.status() == StatusCode::FORBIDDEN {
        return Err("The Homarr API key does not have permission to list applications.".into());
    }
    if !response.status().is_success() {
        return Err(format!(
            "Homarr application discovery returned HTTP {}.",
            response.status().as_u16()
        ));
    }
    authentication.report_success(&target.service_id, material.revision());

    let body = read_bounded_body(response).await?;
    let document: Value = serde_json::from_slice(&body)
        .map_err(|_| "Homarr returned an invalid applications document.".to_string())?;
    let applications = application_array(&document)
        .ok_or_else(|| "Homarr returned an unsupported applications document.".to_string())?;

    let mut candidates = Vec::with_capacity(applications.len().min(MAX_CANDIDATES));
    let mut skipped_count = applications.len().saturating_sub(MAX_CANDIDATES);
    for (index, application) in applications.iter().take(MAX_CANDIDATES).enumerate() {
        match parse_application(application, index, &target.url) {
            Some(candidate) => candidates.push(candidate),
            None => skipped_count = skipped_count.saturating_add(1),
        }
    }

    Ok(ServiceDiscoveryResponse {
        source: "homarr",
        source_service_id: target.service_id,
        candidates,
        skipped_count,
    })
}

fn build_client(allow_invalid_local_certificate: bool) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(Policy::none())
        .http1_only()
        .user_agent(DISCOVERY_USER_AGENT)
        .tls_danger_accept_invalid_certs(allow_invalid_local_certificate)
        .build()
}

fn provider_url(base_url: &Url, suffix: &str) -> Option<Url> {
    let mut root = base_url.clone();
    root.set_query(None);
    root.set_fragment(None);
    let path = format!("{}/", root.path().trim_end_matches('/'));
    root.set_path(&path);
    root.join(suffix).ok()
}

async fn read_bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("The Homarr applications response is too large.".into());
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The Homarr applications response could not be read.".to_string())?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("The Homarr applications response is too large.".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn application_array(document: &Value) -> Option<&Vec<Value>> {
    document
        .as_array()
        .or_else(|| document.get("data")?.as_array())
        .or_else(|| document.get("apps")?.as_array())
}

fn parse_application(
    value: &Value,
    index: usize,
    homarr_url: &Url,
) -> Option<ServiceDiscoveryCandidate> {
    let object = value.as_object()?;
    let name = bounded_text(object.get("name")?, MAX_NAME_LENGTH)?;
    let source_id = object
        .get("id")
        .and_then(value_as_identifier)
        .and_then(|value| bounded_owned_text(value, MAX_SOURCE_ID_LENGTH))
        .unwrap_or_else(|| format!("homarr-app-{index}"));
    let description = object
        .get("description")
        .and_then(|value| optional_bounded_text(value, MAX_DESCRIPTION_LENGTH));
    let url = object
        .get("href")
        .and_then(Value::as_str)
        .and_then(|value| normalize_http_url(value, homarr_url));
    let icon_hint = object
        .get("iconUrl")
        .or_else(|| object.get("icon"))
        .and_then(|value| optional_bounded_text(value, MAX_ICON_HINT_LENGTH));

    Some(ServiceDiscoveryCandidate {
        source_id,
        name,
        description,
        url,
        icon_hint,
    })
}

fn value_as_identifier(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn bounded_text(value: &Value, maximum: usize) -> Option<String> {
    optional_bounded_text(value, maximum)
}

fn optional_bounded_text(value: &Value, maximum: usize) -> Option<String> {
    let value = value.as_str()?.trim();
    if value.is_empty() || value.chars().count() > maximum || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

fn bounded_owned_text(value: String, maximum: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > maximum
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn normalize_http_url(value: &str, homarr_url: &Url) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_URL_LENGTH {
        return None;
    }
    let mut url = Url::parse(value).or_else(|_| homarr_url.join(value)).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn validate_service_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("The Homarr service ID is invalid.".into());
    }
    Ok(())
}

fn ensure_trusted_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() == MAIN_WEBVIEW_LABEL {
        Ok(())
    } else {
        Err("Service discovery is available only to the trusted Personal Hub UI.".into())
    }
}
