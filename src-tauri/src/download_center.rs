use crate::{
    credential_vault::SensitiveString,
    provider_auth::{
        extract_qbittorrent_session_cookie, qbittorrent_cookie_header, qbittorrent_login_form,
        ProviderAuthError, ProviderAuthErrorKind, ProviderAuthManager, ProviderAuthMaterial,
    },
    service_settings::ServiceApiAuthentication,
    service_webviews::{ServiceCatalog, TrustedAuthenticationTarget, TrustedServiceIdentity},
};
use futures_util::lock::Mutex as AsyncMutex;
use reqwest::{
    header::{CONTENT_TYPE, COOKIE},
    redirect::Policy,
    Client, Response, StatusCode,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    error::Error,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{Url, Webview};

const MAIN_WEBVIEW_LABEL: &str = "main";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_millis(5_000);
const DOWNLOAD_USER_AGENT: &str = "PersonalHub/0.1 download-center";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_LOGIN_RESPONSE_BYTES: usize = 4 * 1024;
const MAX_SNAPSHOT_ITEMS: usize = 200;
const MAX_ID_LENGTH: usize = 128;
const MAX_NAME_LENGTH: usize = 256;
const MAX_STATE_LENGTH: usize = 64;
const MAX_CATEGORY_LENGTH: usize = 80;
const MAX_TAG_DOCUMENT_LENGTH: usize = 1024;
const MAX_TAG_LENGTH: usize = 64;
const MAX_TAGS: usize = 16;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const QBITTORRENT_INFINITE_ETA_SECONDS: u64 = 8_640_000;

#[derive(Clone)]
pub struct DownloadCenterClients {
    strict: Client,
    relaxed_local: Client,
    sessions: Arc<AsyncMutex<HashMap<String, CachedSession>>>,
}

impl DownloadCenterClients {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            strict: build_client(false)?,
            relaxed_local: build_client(true)?,
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
        })
    }

    pub(crate) fn clear_sessions(&self) {
        if let Some(mut sessions) = self.sessions.try_lock() {
            sessions.clear();
            return;
        }

        let sessions = Arc::clone(&self.sessions);
        tauri::async_runtime::spawn(async move {
            sessions.lock().await.clear();
        });
    }

    pub(crate) fn clear_service_session(&self, service_id: &str) {
        if let Some(mut sessions) = self.sessions.try_lock() {
            sessions.remove(service_id);
            return;
        }

        let sessions = Arc::clone(&self.sessions);
        let service_id = service_id.to_string();
        tauri::async_runtime::spawn(async move {
            sessions.lock().await.remove(&service_id);
        });
    }
}

struct CachedSession {
    revision: String,
    cookie: SensitiveString,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DownloadCenterStatus {
    Online,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DownloadProviderState {
    Configured,
    NotConfigured,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DownloadReason {
    Authentication,
    Timeout,
    Tls,
    Connection,
    ApiUnavailable,
    InvalidData,
    Backoff,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProviderSnapshot {
    service_id: String,
    name: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DownloadSourceKind {
    Sonarr,
    Radarr,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadSourceSnapshot {
    kind: DownloadSourceKind,
    service_id: String,
    label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadItemSnapshot {
    id: String,
    name: String,
    progress_percent: f64,
    download_speed_bytes_per_second: u64,
    eta_seconds: Option<u64>,
    state: &'static str,
    category: Option<String>,
    tags: Vec<String>,
    source: Option<DownloadSourceSnapshot>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCenterSnapshot {
    status: DownloadCenterStatus,
    provider_state: DownloadProviderState,
    reason: Option<DownloadReason>,
    sampled_at: u64,
    total_download_speed_bytes_per_second: u64,
    retry_after_ms: Option<u64>,
    provider: Option<DownloadProviderSnapshot>,
    items: Vec<DownloadItemSnapshot>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QbittorrentTorrent {
    hash: String,
    name: String,
    progress: f64,
    #[serde(default)]
    dlspeed: u64,
    #[serde(default)]
    eta: u64,
    state: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    tags: String,
}

struct SourceServices {
    sonarr: Option<TrustedServiceIdentity>,
    radarr: Option<TrustedServiceIdentity>,
}

#[derive(Debug)]
struct DownloadFailure {
    reason: DownloadReason,
    retry_after_ms: Option<u64>,
    message: &'static str,
    session_rejected: bool,
}

impl DownloadFailure {
    fn new(reason: DownloadReason, message: &'static str) -> Self {
        Self {
            reason,
            retry_after_ms: None,
            message,
            session_rejected: false,
        }
    }

    fn session_rejected() -> Self {
        Self {
            reason: DownloadReason::Authentication,
            retry_after_ms: None,
            message: "qBittorrent rejected its current session.",
            session_rejected: true,
        }
    }

    fn backoff(retry_after_ms: u64, message: &'static str) -> Self {
        Self {
            reason: DownloadReason::Backoff,
            retry_after_ms: Some(retry_after_ms.max(1)),
            message,
            session_rejected: false,
        }
    }

    fn from_provider_auth(error: ProviderAuthError) -> Self {
        let reason = match error.kind() {
            ProviderAuthErrorKind::Backoff => DownloadReason::Backoff,
            ProviderAuthErrorKind::InvalidData => DownloadReason::InvalidData,
            ProviderAuthErrorKind::MissingCredential
            | ProviderAuthErrorKind::EndpointChanged
            | ProviderAuthErrorKind::VaultUnavailable
            | ProviderAuthErrorKind::InsecureTransport => DownloadReason::Authentication,
        };
        Self {
            reason,
            retry_after_ms: error.retry_after_ms(),
            message: "qBittorrent authentication is unavailable.",
            session_rejected: false,
        }
    }
}

#[tauri::command]
pub async fn get_download_center_snapshot(
    caller: Webview,
    catalog: tauri::State<'_, ServiceCatalog>,
    authentication: tauri::State<'_, ProviderAuthManager>,
    clients: tauri::State<'_, DownloadCenterClients>,
) -> Result<DownloadCenterSnapshot, String> {
    ensure_trusted_caller(&caller)?;
    collect_download_snapshot(&catalog, &authentication, &clients, &TorrentQuery::Active).await
}

enum TorrentQuery<'a> {
    Active,
    Hashes(&'a [String]),
}

/// Native-only observation. Hashes stay in memory and never enter a toast.
pub(crate) struct DownloadNotificationSample {
    pub(crate) provider_key: String,
    pub(crate) incomplete_hashes: Vec<String>,
    pub(crate) confirmed_completed: usize,
}

pub(crate) fn notification_provider_key(
    catalog: &ServiceCatalog,
    authentication: &ProviderAuthManager,
) -> Option<String> {
    let provider = catalog
        .find_download_provider(ServiceApiAuthentication::QbittorrentWebApi)
        .ok()??;
    provider.target.enabled.then(|| {
        format!(
            "{}:{}",
            provider.target.service_id,
            authentication.revision_for_target(&provider.target)
        )
    })
}

pub(crate) async fn collect_download_notification_sample(
    catalog: &ServiceCatalog,
    authentication: &ProviderAuthManager,
    clients: &DownloadCenterClients,
    previous_provider: Option<&str>,
    previous_hashes: &[String],
) -> Result<Option<DownloadNotificationSample>, String> {
    let Some(provider_key) = notification_provider_key(catalog, authentication) else {
        return Ok(None);
    };
    let active =
        collect_download_snapshot(catalog, authentication, clients, &TorrentQuery::Active).await?;
    if !matches!(active.status, DownloadCenterStatus::Online) {
        return Err("Download notifications are temporarily unavailable.".into());
    }
    let incomplete_hashes = active
        .items
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    let mut confirmed_completed = 0;
    if previous_provider == Some(provider_key.as_str()) {
        let disappeared = previous_hashes
            .iter()
            .take(MAX_SNAPSHOT_ITEMS)
            .filter(|hash| !incomplete_hashes.contains(hash))
            .cloned()
            .collect::<Vec<_>>();
        // Keep URL sizes bounded even for long hashes. A deleted torrent returns
        // no record and therefore cannot be mistaken for a completed download.
        for hashes in disappeared.chunks(50) {
            let confirmed = collect_download_snapshot(
                catalog,
                authentication,
                clients,
                &TorrentQuery::Hashes(hashes),
            )
            .await?;
            if !matches!(confirmed.status, DownloadCenterStatus::Online) {
                return Err("Download completion could not be confirmed.".into());
            }
            confirmed_completed += confirmed
                .items
                .iter()
                .filter(|item| item.progress_percent == 100.0)
                .count();
        }
    }
    if notification_provider_key(catalog, authentication).as_deref() != Some(provider_key.as_str())
    {
        return Err("The download provider changed during notification polling.".into());
    }
    Ok(Some(DownloadNotificationSample {
        provider_key,
        incomplete_hashes,
        confirmed_completed,
    }))
}

async fn collect_download_snapshot(
    catalog: &ServiceCatalog,
    authentication: &ProviderAuthManager,
    clients: &DownloadCenterClients,
    query: &TorrentQuery<'_>,
) -> Result<DownloadCenterSnapshot, String> {
    // Resolve credentials only after queued polls have obtained their turn.
    // A settings/credential change while waiting must not reuse an old target.
    let mut sessions = clients.sessions.lock().await;
    let sampled_at = unix_time_ms();
    let Some(selected) =
        catalog.find_download_provider(ServiceApiAuthentication::QbittorrentWebApi)?
    else {
        return Ok(DownloadCenterSnapshot {
            status: DownloadCenterStatus::Unavailable,
            provider_state: DownloadProviderState::NotConfigured,
            reason: None,
            sampled_at,
            total_download_speed_bytes_per_second: 0,
            retry_after_ms: None,
            provider: None,
            items: Vec::new(),
            message: Some("No qBittorrent Web API provider is configured.".into()),
        });
    };

    let provider = DownloadProviderSnapshot {
        service_id: selected.target.service_id.clone(),
        name: selected.name.clone(),
    };
    if !selected.target.enabled {
        return Ok(unavailable_snapshot(
            sampled_at,
            provider,
            DownloadFailure::new(
                DownloadReason::ApiUnavailable,
                "The configured qBittorrent provider is disabled.",
            ),
        ));
    }

    let source_services = SourceServices {
        sonarr: catalog.find_enabled_service_identity("sonarr", "Sonarr")?,
        radarr: catalog.find_enabled_service_identity("radarr", "Radarr")?,
    };
    let material = match authentication.resolve_api_authentication(&selected.target) {
        Ok(Some(material)) => material,
        Ok(None) => {
            return Ok(unavailable_snapshot(
                sampled_at,
                provider,
                DownloadFailure::new(
                    DownloadReason::Authentication,
                    "qBittorrent authentication is not configured.",
                ),
            ))
        }
        Err(error) => {
            return Ok(unavailable_snapshot(
                sampled_at,
                provider,
                DownloadFailure::from_provider_auth(error),
            ))
        }
    };
    let revision = material.revision().to_string();
    let client = if selected.target.allow_invalid_local_certificate {
        &clients.relaxed_local
    } else {
        &clients.strict
    };
    let mut session = sessions
        .remove(&selected.target.service_id)
        .filter(|session| session.revision == revision);
    let had_cached_session = session.is_some();

    if session.is_none() {
        match login(client, &selected.target, authentication, &material, catalog).await {
            Ok(cookie) => {
                session = Some(CachedSession {
                    revision: revision.clone(),
                    cookie,
                });
            }
            Err(error) => return Ok(unavailable_snapshot(sampled_at, provider, error)),
        }
    }

    let mut result = read_torrents(
        client,
        &selected.target,
        &material,
        &session.as_ref().expect("a login created a session").cookie,
        &source_services,
        authentication,
        catalog,
        query,
    )
    .await;

    if had_cached_session
        && result
            .as_ref()
            .err()
            .is_some_and(|error| error.session_rejected)
    {
        drop(session.take());
        match login(client, &selected.target, authentication, &material, catalog).await {
            Ok(cookie) => {
                session = Some(CachedSession {
                    revision: revision.clone(),
                    cookie,
                });
                result = read_torrents(
                    client,
                    &selected.target,
                    &material,
                    &session.as_ref().expect("a login created a session").cookie,
                    &source_services,
                    authentication,
                    catalog,
                    query,
                )
                .await;
            }
            Err(error) => return Ok(unavailable_snapshot(sampled_at, provider, error)),
        }
    }

    if let Err(error) =
        ensure_current_provider(catalog, authentication, &selected.target, &revision)
    {
        return Ok(unavailable_snapshot(sampled_at, provider, error));
    }

    match result {
        Ok(items) => {
            authentication.report_success(&selected.target.service_id, &revision);
            if let Some(session) = session {
                sessions.insert(selected.target.service_id.clone(), session);
            }
            let total_download_speed_bytes_per_second = items.iter().fold(0_u64, |total, item| {
                total
                    .saturating_add(item.download_speed_bytes_per_second)
                    .min(MAX_SAFE_INTEGER)
            });
            Ok(DownloadCenterSnapshot {
                status: DownloadCenterStatus::Online,
                provider_state: DownloadProviderState::Configured,
                reason: None,
                sampled_at,
                total_download_speed_bytes_per_second,
                retry_after_ms: None,
                provider: Some(provider),
                items,
                message: None,
            })
        }
        Err(error) => {
            if !error.session_rejected {
                if let Some(session) = session {
                    sessions.insert(selected.target.service_id.clone(), session);
                }
            } else {
                authentication.report_auth_failure(&selected.target.service_id, &revision);
            }
            Ok(unavailable_snapshot(sampled_at, provider, error))
        }
    }
}

async fn login(
    client: &Client,
    target: &TrustedAuthenticationTarget,
    authentication: &ProviderAuthManager,
    material: &ProviderAuthMaterial,
    catalog: &ServiceCatalog,
) -> Result<SensitiveString, DownloadFailure> {
    let endpoint = provider_url(&target.url, "api/v2/auth/login").ok_or_else(|| {
        DownloadFailure::new(
            DownloadReason::InvalidData,
            "The qBittorrent login endpoint is invalid.",
        )
    })?;
    let (username, password) = material
        .qbittorrent_credentials_for(&endpoint)
        .map_err(DownloadFailure::from_provider_auth)?;
    let referer = same_origin_referer(target);
    let request = material
        .apply(client.post(endpoint.clone()), &endpoint)
        .map_err(DownloadFailure::from_provider_auth)?;
    let login_body = qbittorrent_login_form(username, password);
    ensure_current_provider(catalog, authentication, target, material.revision())?;
    let response = request
        .header("Origin", target.canonical_origin.as_str())
        .header("Referer", referer.as_str())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(login_body.expose().to_owned())
        .send()
        .await
        .map_err(classify_request_error)?;
    ensure_current_provider(catalog, authentication, target, material.revision())?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        authentication.report_auth_failure(&target.service_id, material.revision());
        return Err(DownloadFailure::new(
            DownloadReason::Authentication,
            "qBittorrent rejected the stored username or password.",
        ));
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        authentication.report_rate_limited(&target.service_id, material.revision());
        let retry_after_ms = authentication
            .retry_after_ms_for_target(target)
            .unwrap_or(1_000);
        return Err(DownloadFailure::backoff(
            retry_after_ms,
            "qBittorrent temporarily refused another login attempt.",
        ));
    }
    if status != StatusCode::OK && status != StatusCode::NO_CONTENT {
        return Err(DownloadFailure::new(
            DownloadReason::ApiUnavailable,
            "qBittorrent login returned an unexpected status.",
        ));
    }

    let cookie = extract_qbittorrent_session_cookie(response.headers());
    let mut body = read_bounded_body(response, MAX_LOGIN_RESPONSE_BYTES).await?;
    let rejected = status == StatusCode::OK && trim_ascii_whitespace(&body) == b"Fails.";
    body.fill(0);
    if rejected {
        authentication.report_auth_failure(&target.service_id, material.revision());
        return Err(DownloadFailure::new(
            DownloadReason::Authentication,
            "qBittorrent rejected the stored username or password.",
        ));
    }
    let cookie = cookie.map_err(DownloadFailure::from_provider_auth)?;

    ensure_current_provider(catalog, authentication, target, material.revision())?;
    authentication.report_success(&target.service_id, material.revision());
    Ok(cookie)
}

// Each parameter is a distinct borrowed capability (transport, trusted target,
// credential material, session cookie, attribution sources, vault and catalog).
// Bundling them into one struct would hand this request path a single handle
// that outlives the call, so they stay explicit.
#[allow(clippy::too_many_arguments)]
async fn read_torrents(
    client: &Client,
    target: &TrustedAuthenticationTarget,
    material: &ProviderAuthMaterial,
    cookie: &SensitiveString,
    source_services: &SourceServices,
    authentication: &ProviderAuthManager,
    catalog: &ServiceCatalog,
    query: &TorrentQuery<'_>,
) -> Result<Vec<DownloadItemSnapshot>, DownloadFailure> {
    let mut endpoint = provider_url(&target.url, "api/v2/torrents/info").ok_or_else(|| {
        DownloadFailure::new(
            DownloadReason::InvalidData,
            "The qBittorrent torrents endpoint is invalid.",
        )
    })?;
    let cookie = qbittorrent_cookie_header(cookie).map_err(DownloadFailure::from_provider_auth)?;
    let referer = same_origin_referer(target);
    endpoint
        .query_pairs_mut()
        // Sort incomplete records before the completed archive on the server.
        // `downloading` alone excludes errored/moving incomplete torrents.
        .append_pair("filter", "all")
        .append_pair("sort", "progress")
        .append_pair("limit", &MAX_SNAPSHOT_ITEMS.to_string());
    if let TorrentQuery::Hashes(hashes) = query {
        if hashes.is_empty()
            || hashes.len() > 50
            || hashes.iter().any(|hash| {
                hash.is_empty()
                    || hash.len() > MAX_ID_LENGTH
                    || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(DownloadFailure::new(
                DownloadReason::InvalidData,
                "Invalid completion identifiers.",
            ));
        }
        endpoint
            .query_pairs_mut()
            .append_pair("hashes", &hashes.join("|"));
    }
    let request = material
        .apply(client.get(endpoint.clone()), &endpoint)
        .map_err(DownloadFailure::from_provider_auth)?;
    ensure_current_provider(catalog, authentication, target, material.revision())?;
    let response = request
        .header("Origin", target.canonical_origin.as_str())
        .header("Referer", referer.as_str())
        .header(COOKIE, cookie)
        .send()
        .await
        .map_err(classify_request_error)?;
    ensure_current_provider(catalog, authentication, target, material.revision())?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err(DownloadFailure::session_rejected())
        }
        StatusCode::TOO_MANY_REQUESTS => {
            authentication.report_rate_limited(&target.service_id, material.revision());
            let retry_after_ms = authentication
                .retry_after_ms_for_target(target)
                .unwrap_or(1_000);
            return Err(DownloadFailure::backoff(
                retry_after_ms,
                "qBittorrent temporarily refused the downloads request.",
            ));
        }
        _ => {
            return Err(DownloadFailure::new(
                DownloadReason::ApiUnavailable,
                "qBittorrent downloads are temporarily unavailable.",
            ))
        }
    }

    let body = read_bounded_body(response, MAX_RESPONSE_BYTES).await?;
    let torrents: Vec<QbittorrentTorrent> = serde_json::from_slice(&body).map_err(|_| {
        DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned an invalid downloads response.",
        )
    })?;
    if torrents.len() > MAX_SNAPSHOT_ITEMS {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned too many downloads.",
        ));
    }

    let mut items = Vec::new();
    let mut seen_hashes = HashSet::new();
    for torrent in torrents {
        match query {
            TorrentQuery::Active if !should_include_torrent(&torrent) => continue,
            TorrentQuery::Hashes(hashes)
                if (!hashes.contains(&torrent.hash)
                    || !torrent.progress.is_finite()
                    || !(0.0..=1.0).contains(&torrent.progress)
                    || !seen_hashes.insert(torrent.hash.clone())) =>
            {
                return Err(DownloadFailure::new(
                    DownloadReason::InvalidData,
                    "Invalid completion response.",
                ));
            }
            _ => {}
        }
        items.push(normalize_torrent(torrent, source_services)?);
        if items.len() == MAX_SNAPSHOT_ITEMS {
            break;
        }
    }
    Ok(items)
}

async fn read_bounded_body(
    mut response: Response,
    maximum: usize,
) -> Result<Vec<u8>, DownloadFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "The qBittorrent response is too large.",
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify_request_error)? {
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(DownloadFailure::new(
                DownloadReason::InvalidData,
                "The qBittorrent response is too large.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn normalize_torrent(
    torrent: QbittorrentTorrent,
    source_services: &SourceServices,
) -> Result<DownloadItemSnapshot, DownloadFailure> {
    let id = bounded_text(torrent.hash, MAX_ID_LENGTH, "download identifier")?;
    if !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned an invalid download identifier.",
        ));
    }
    let name = bounded_text(torrent.name, MAX_NAME_LENGTH, "download name")?;
    let raw_state = bounded_text(torrent.state, MAX_STATE_LENGTH, "download state")?;
    let category = optional_bounded_text(torrent.category, MAX_CATEGORY_LENGTH, "category")?;
    let tags = normalize_tags(torrent.tags)?;
    if !torrent.progress.is_finite()
        || torrent.dlspeed > MAX_SAFE_INTEGER
        || torrent.eta > MAX_SAFE_INTEGER
    {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned invalid numeric download data.",
        ));
    }
    let source = source_for(
        category.as_deref().unwrap_or_default(),
        &tags,
        source_services,
    );

    Ok(DownloadItemSnapshot {
        id,
        name,
        progress_percent: torrent.progress.clamp(0.0, 1.0) * 100.0,
        download_speed_bytes_per_second: torrent.dlspeed,
        eta_seconds: (torrent.eta < QBITTORRENT_INFINITE_ETA_SECONDS).then_some(torrent.eta),
        state: normalize_state(&raw_state, torrent.progress),
        category,
        tags,
        source,
    })
}

fn should_include_torrent(torrent: &QbittorrentTorrent) -> bool {
    torrent.progress < 1.0
}

fn ensure_current_provider(
    catalog: &ServiceCatalog,
    authentication: &ProviderAuthManager,
    target: &TrustedAuthenticationTarget,
    revision: &str,
) -> Result<(), DownloadFailure> {
    let latest = catalog
        .find_download_provider(ServiceApiAuthentication::QbittorrentWebApi)
        .ok()
        .flatten();
    if latest.is_some_and(|latest| {
        latest.target.enabled
            && latest.target.service_id == target.service_id
            && authentication.revision_for_target(&latest.target) == revision
    }) {
        Ok(())
    } else {
        Err(DownloadFailure::new(
            DownloadReason::Authentication,
            "The qBittorrent provider or credential changed while downloads were loading.",
        ))
    }
}

fn normalize_tags(value: String) -> Result<Vec<String>, DownloadFailure> {
    if value.chars().count() > MAX_TAG_DOCUMENT_LENGTH || value.chars().any(char::is_control) {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned invalid download tags.",
        ));
    }
    let mut tags = Vec::new();
    for tag in value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        if !tags
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tag))
        {
            tags.push(tag.to_string());
        }
    }
    if tags.len() > MAX_TAGS || tags.iter().any(|tag| tag.chars().count() > MAX_TAG_LENGTH) {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned too many or oversized download tags.",
        ));
    }
    Ok(tags)
}

fn source_for(
    category: &str,
    tags: &[String],
    sources: &SourceServices,
) -> Option<DownloadSourceSnapshot> {
    let category_source = source_token(category);
    let tag_source = if category_source.is_none() {
        let has_sonarr = tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("sonarr") || tag.eq_ignore_ascii_case("tv-sonarr"));
        let has_radarr = tags.iter().any(|tag| tag.eq_ignore_ascii_case("radarr"));
        match (has_sonarr, has_radarr) {
            (true, false) => Some(DownloadSourceKind::Sonarr),
            (false, true) => Some(DownloadSourceKind::Radarr),
            _ => None,
        }
    } else {
        None
    };

    match category_source.or(tag_source)? {
        DownloadSourceKind::Sonarr => Some(source_snapshot(
            DownloadSourceKind::Sonarr,
            sources.sonarr.as_ref(),
            "sonarr",
            "Sonarr",
        )),
        DownloadSourceKind::Radarr => Some(source_snapshot(
            DownloadSourceKind::Radarr,
            sources.radarr.as_ref(),
            "radarr",
            "Radarr",
        )),
    }
}

fn source_token(value: &str) -> Option<DownloadSourceKind> {
    if value.eq_ignore_ascii_case("sonarr") || value.eq_ignore_ascii_case("tv-sonarr") {
        Some(DownloadSourceKind::Sonarr)
    } else if value.eq_ignore_ascii_case("radarr") {
        Some(DownloadSourceKind::Radarr)
    } else {
        None
    }
}

fn source_snapshot(
    kind: DownloadSourceKind,
    service: Option<&TrustedServiceIdentity>,
    fallback_id: &str,
    fallback_name: &str,
) -> DownloadSourceSnapshot {
    DownloadSourceSnapshot {
        kind,
        service_id: service
            .map(|service| service.id.clone())
            .unwrap_or_else(|| fallback_id.to_string()),
        label: service
            .map(|service| service.name.clone())
            .unwrap_or_else(|| fallback_name.to_string()),
    }
}

fn normalize_state(value: &str, progress: f64) -> &'static str {
    match value {
        "downloading" | "forcedDL" => "downloading",
        "metaDL" | "forcedMetaDL" => "metadata",
        "pausedDL" | "stoppedDL" => "paused",
        "stalledDL" => "stalled",
        "queuedDL" => "queued",
        "checkingDL" | "checkingResumeData" | "allocating" | "moving" => "checking",
        "error" | "missingFiles" => "error",
        "uploading" | "stalledUP" | "queuedUP" | "checkingUP" | "forcedUP" | "pausedUP"
        | "stoppedUP" => "other",
        _ if progress >= 1.0 => "other",
        _ => "other",
    }
}

fn bounded_text(
    value: String,
    maximum: usize,
    field: &'static str,
) -> Result<String, DownloadFailure> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > maximum || value.chars().any(char::is_control) {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            match field {
                "download identifier" => "qBittorrent returned an invalid download identifier.",
                "download name" => "qBittorrent returned an invalid download name.",
                _ => "qBittorrent returned an invalid download state.",
            },
        ));
    }
    Ok(value.to_string())
}

fn optional_bounded_text(
    value: String,
    maximum: usize,
    _field: &'static str,
) -> Result<Option<String>, DownloadFailure> {
    let value = value.trim();
    if value.chars().count() > maximum || value.chars().any(char::is_control) {
        return Err(DownloadFailure::new(
            DownloadReason::InvalidData,
            "qBittorrent returned an invalid download category.",
        ));
    }
    Ok((!value.is_empty()).then(|| value.to_string()))
}

fn unavailable_snapshot(
    sampled_at: u64,
    provider: DownloadProviderSnapshot,
    error: DownloadFailure,
) -> DownloadCenterSnapshot {
    DownloadCenterSnapshot {
        status: DownloadCenterStatus::Unavailable,
        provider_state: DownloadProviderState::Configured,
        reason: Some(error.reason),
        sampled_at,
        total_download_speed_bytes_per_second: 0,
        retry_after_ms: error.retry_after_ms,
        provider: Some(provider),
        items: Vec::new(),
        message: Some(error.message.into()),
    }
}

fn classify_request_error(error: reqwest::Error) -> DownloadFailure {
    let reason = if error.is_timeout() {
        DownloadReason::Timeout
    } else if error_chain_mentions_tls(&error) {
        DownloadReason::Tls
    } else if error.is_connect() {
        DownloadReason::Connection
    } else {
        DownloadReason::ApiUnavailable
    };
    DownloadFailure::new(reason, "qBittorrent could not be reached.")
}

fn error_chain_mentions_tls(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(source) = current {
        let text = source.to_string().to_ascii_lowercase();
        if text.contains("tls") || text.contains("certificate") {
            return true;
        }
        current = source.source();
    }
    false
}

fn provider_url(base_url: &Url, suffix: &str) -> Option<Url> {
    let mut root = base_url.clone();
    root.set_query(None);
    root.set_fragment(None);
    let path = format!("{}/", root.path().trim_end_matches('/'));
    root.set_path(&path);
    root.join(suffix).ok()
}

fn same_origin_referer(target: &TrustedAuthenticationTarget) -> String {
    format!("{}/", target.canonical_origin.trim_end_matches('/'))
}

fn build_client(allow_invalid_local_certificate: bool) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(Policy::none())
        .http1_only()
        .user_agent(DOWNLOAD_USER_AGENT)
        .tls_danger_accept_invalid_certs(allow_invalid_local_certificate)
        .build()
}

fn trim_ascii_whitespace(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .clamp(0, u64::MAX as u128) as u64
}

fn ensure_trusted_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() == MAIN_WEBVIEW_LABEL {
        Ok(())
    } else {
        Err("The download center is available only to the trusted Personal Hub UI.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_downloads_normalize_old_and_new_states_without_private_fields() {
        let sources = SourceServices {
            sonarr: None,
            radarr: None,
        };
        for (raw_state, expected) in [
            ("pausedDL", "paused"),
            ("stoppedDL", "paused"),
            ("error", "error"),
        ] {
            let torrent: QbittorrentTorrent = serde_json::from_value(serde_json::json!({
                "hash": "abcdef0123456789", "name": "Example ISO", "progress": 0.42,
                "dlspeed": 0, "eta": QBITTORRENT_INFINITE_ETA_SECONDS,
                "state": raw_state, "category": "", "tags": "sonarr, SONARR",
                "save_path": "private path", "tracker": "private tracker"
            }))
            .unwrap();
            assert!(should_include_torrent(&torrent));
            let item = normalize_torrent(torrent, &sources).unwrap();
            assert_eq!(item.state, expected);
            assert_eq!(item.progress_percent, 42.0);
            assert!(item.category.is_none());
            assert!(item.eta_seconds.is_none());
            assert_eq!(item.tags, ["sonarr"]);
            let wire = serde_json::to_value(item).unwrap();
            assert!(wire["category"].is_null());
            assert!(wire.get("save_path").is_none());
            assert!(wire.get("tracker").is_none());
        }
        let completed: QbittorrentTorrent = serde_json::from_value(serde_json::json!({
            "hash": "abcdef", "name": "Completed ISO", "progress": 1.0, "state": "moving"
        }))
        .unwrap();
        assert!(!should_include_torrent(&completed));
    }

    #[test]
    fn attribution_uses_exact_category_before_tags() {
        let sources = SourceServices {
            sonarr: None,
            radarr: None,
        };
        let source = source_for("tv-sonarr", &["radarr".into()], &sources).unwrap();
        assert_eq!(source.service_id, "sonarr");
        assert!(source_for("movies", &["sonarr-radarr".into()], &sources).is_none());
    }

    #[test]
    fn provider_url_preserves_reverse_proxy_prefix() {
        let base = Url::parse("https://server.local/qbit/").unwrap();
        assert_eq!(
            provider_url(&base, "api/v2/torrents/info")
                .unwrap()
                .as_str(),
            "https://server.local/qbit/api/v2/torrents/info"
        );
    }
}
