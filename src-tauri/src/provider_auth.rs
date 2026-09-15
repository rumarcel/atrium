use crate::{
    credential_vault::{
        credential_revision, inspect_origin_bound_credential, read_origin_bound_credential,
        CredentialBindingState, CredentialKind, CredentialReadResult, OriginBoundCredential,
        SensitiveString,
    },
    service_settings::{ServiceApiAuthentication, ServiceBrowserAuthentication},
    service_webviews::{ServiceCatalog, TrustedAuthenticationTarget},
};
use futures_util::future::{BoxFuture, FutureExt, Shared};
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE, COOKIE, SET_COOKIE},
    redirect::Policy,
    Client, RequestBuilder, StatusCode,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Url, Webview};

const MAIN_WEBVIEW_LABEL: &str = "main";
const AUTH_TIMEOUT: Duration = Duration::from_millis(3_000);
const AUTH_USER_AGENT: &str = "PersonalHub/0.1 provider-auth";
const MAX_HOMARR_INFO_BYTES: usize = 64 * 1024;
const MAX_HOMARR_VERSION_LENGTH: usize = 128;
const MAX_QBITTORRENT_LOGIN_BYTES: usize = 4 * 1024;
const MAX_QBITTORRENT_VERSION_BYTES: usize = 4 * 1024;
const MAX_QBITTORRENT_COOKIE_PAIR_BYTES: usize = 1024;
const BACKOFF_STEPS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(5 * 60),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderAuthErrorKind {
    MissingCredential,
    EndpointChanged,
    VaultUnavailable,
    InsecureTransport,
    InvalidData,
    Backoff,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderAuthError {
    kind: ProviderAuthErrorKind,
    message: &'static str,
    retry_after_ms: Option<u64>,
}

impl ProviderAuthError {
    fn new(kind: ProviderAuthErrorKind, message: &'static str) -> Self {
        Self {
            kind,
            message,
            retry_after_ms: None,
        }
    }

    fn backoff(retry_after_ms: u64) -> Self {
        Self {
            kind: ProviderAuthErrorKind::Backoff,
            message: "Provider authentication is temporarily backing off.",
            retry_after_ms: Some(retry_after_ms),
        }
    }

    pub(crate) fn kind(&self) -> ProviderAuthErrorKind {
        self.kind
    }

    pub(crate) fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }
}

impl fmt::Display for ProviderAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for ProviderAuthError {}

#[derive(Clone)]
enum ApiCredential {
    HomarrApiKey(Arc<OriginBoundCredential>),
    GlancesHttpBasic(Arc<OriginBoundCredential>),
    GlancesBearer(Arc<OriginBoundCredential>),
    QbittorrentUsernamePassword(Arc<OriginBoundCredential>),
}

/// Native-only request material. Every plaintext credential is held behind an
/// `Arc` whose final drop zeroizes its backing strings in `credential_vault`.
#[derive(Clone)]
pub(crate) struct ProviderAuthMaterial {
    expected_origin: String,
    revision: String,
    api: Option<ApiCredential>,
    browser_basic: Option<Arc<OriginBoundCredential>>,
}

impl fmt::Debug for ProviderAuthMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderAuthMaterial")
            .field("revision", &self.revision)
            .field("has_api_credential", &self.api.is_some())
            .field("has_browser_basic", &self.browser_basic.is_some())
            .finish()
    }
}

impl ProviderAuthMaterial {
    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn apply(
        &self,
        mut request: RequestBuilder,
        request_url: &Url,
    ) -> Result<RequestBuilder, ProviderAuthError> {
        self.ensure_exact_origin(request_url)?;

        match &self.api {
            Some(ApiCredential::HomarrApiKey(credential)) => {
                let mut value = HeaderValue::from_str(credential.secret()).map_err(|_| {
                    ProviderAuthError::new(
                        ProviderAuthErrorKind::InvalidData,
                        "The stored API key cannot be used as an HTTP header.",
                    )
                })?;
                value.set_sensitive(true);
                request = request.header("ApiKey", value);
            }
            Some(ApiCredential::GlancesHttpBasic(credential)) => {
                request = apply_basic(request, credential)?;
            }
            Some(ApiCredential::GlancesBearer(credential)) => {
                if self.browser_basic.is_some() {
                    return Err(ProviderAuthError::new(
                        ProviderAuthErrorKind::InvalidData,
                        "Bearer and HTTP Basic authentication cannot share one Authorization header.",
                    ));
                }
                request = request.bearer_auth(credential.secret());
            }
            Some(ApiCredential::QbittorrentUsernamePassword(_)) => {}
            None => {}
        }

        if let Some(credential) = &self.browser_basic {
            if !matches!(self.api, Some(ApiCredential::GlancesHttpBasic(_))) {
                request = apply_basic(request, credential)?;
            }
        }

        Ok(request)
    }

    pub(crate) fn basic_credentials_for(
        &self,
        request_url: &Url,
    ) -> Result<(&str, &str), ProviderAuthError> {
        self.ensure_exact_origin(request_url)?;
        let credential = self.browser_basic.as_ref().ok_or_else(|| {
            ProviderAuthError::new(
                ProviderAuthErrorKind::InvalidData,
                "No browser HTTP Basic credential is configured.",
            )
        })?;
        let username = credential.username().ok_or_else(|| {
            ProviderAuthError::new(
                ProviderAuthErrorKind::InvalidData,
                "The stored HTTP Basic credential is invalid.",
            )
        })?;
        Ok((username, credential.secret()))
    }

    pub(crate) fn qbittorrent_credentials_for(
        &self,
        request_url: &Url,
    ) -> Result<(&str, &str), ProviderAuthError> {
        self.ensure_exact_origin(request_url)?;
        let Some(ApiCredential::QbittorrentUsernamePassword(credential)) = &self.api else {
            return Err(ProviderAuthError::new(
                ProviderAuthErrorKind::InvalidData,
                "No qBittorrent Web API credential is configured.",
            ));
        };
        let username = credential.username().ok_or_else(|| {
            ProviderAuthError::new(
                ProviderAuthErrorKind::InvalidData,
                "The stored qBittorrent username/password credential is invalid.",
            )
        })?;
        Ok((username, credential.secret()))
    }

    fn ensure_exact_origin(&self, request_url: &Url) -> Result<(), ProviderAuthError> {
        if canonical_origin(request_url).as_deref() == Some(self.expected_origin.as_str()) {
            Ok(())
        } else {
            Err(ProviderAuthError::new(
                ProviderAuthErrorKind::EndpointChanged,
                "Provider credentials are restricted to the configured service origin.",
            ))
        }
    }
}

fn apply_basic(
    request: RequestBuilder,
    credential: &OriginBoundCredential,
) -> Result<RequestBuilder, ProviderAuthError> {
    let username = credential.username().ok_or_else(|| {
        ProviderAuthError::new(
            ProviderAuthErrorKind::InvalidData,
            "The stored HTTP Basic credential is invalid.",
        )
    })?;
    Ok(request.basic_auth(username, Some(credential.secret())))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SnapshotCredentialState {
    NotRequired,
    Missing,
    Stored,
    NeedsRebind,
    VaultUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SnapshotValidationState {
    Unsupported,
    NotValidated,
    Validating,
    Valid,
    Invalid,
    TemporarilyUnavailable,
    Backoff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SnapshotReasonCode {
    MissingCredential,
    EndpointChanged,
    Unauthorized,
    Forbidden,
    RateLimited,
    Timeout,
    Tls,
    Connection,
    ApiUnavailable,
    InvalidData,
    InsecureTransport,
    VaultUnavailable,
    ValidationInProgress,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAuthenticationSnapshot {
    service_id: String,
    revision: String,
    api_adapter: ServiceApiAuthentication,
    browser_adapter: ServiceBrowserAuthentication,
    required_credential_kinds: Vec<CredentialKind>,
    credential_state: SnapshotCredentialState,
    validation_state: SnapshotValidationState,
    can_validate: bool,
    can_clear_session: bool,
    reason_code: Option<SnapshotReasonCode>,
    retry_after_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidateServiceAuthenticationRequest {
    service_id: String,
    expected_revision: Option<String>,
}

trait CredentialSource: Send + Sync {
    fn revision(&self, service_id: &str) -> u64;
    fn inspect(
        &self,
        service_id: &str,
        kind: CredentialKind,
        origin: &str,
    ) -> Result<CredentialBindingState, String>;
    fn read(
        &self,
        service_id: &str,
        kind: CredentialKind,
        origin: &str,
    ) -> Result<CredentialReadResult, String>;
}

struct SystemCredentialSource;

impl CredentialSource for SystemCredentialSource {
    fn revision(&self, service_id: &str) -> u64 {
        credential_revision(service_id)
    }

    fn inspect(
        &self,
        service_id: &str,
        kind: CredentialKind,
        origin: &str,
    ) -> Result<CredentialBindingState, String> {
        inspect_origin_bound_credential(service_id, kind, origin)
    }

    fn read(
        &self,
        service_id: &str,
        kind: CredentialKind,
        origin: &str,
    ) -> Result<CredentialReadResult, String> {
        read_origin_bound_credential(service_id, kind, origin)
    }
}

#[derive(Clone)]
struct ValidationClients {
    strict: Client,
    relaxed_local: Client,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ValidationRecord {
    state: SnapshotValidationState,
    reason: Option<SnapshotReasonCode>,
}

impl Default for ValidationRecord {
    fn default() -> Self {
        Self {
            state: SnapshotValidationState::NotValidated,
            reason: None,
        }
    }
}

#[derive(Clone, Debug)]
struct ValidationCompletion {
    revision: String,
    record: ValidationRecord,
}

type SharedValidation = Shared<BoxFuture<'static, ValidationCompletion>>;

#[derive(Clone)]
struct InFlightValidation {
    id: u64,
    future: SharedValidation,
}

struct RuntimeEntry {
    revision: String,
    record: ValidationRecord,
    failures: usize,
    retry_at: Option<Instant>,
    in_flight: Option<InFlightValidation>,
}

impl RuntimeEntry {
    fn new(revision: String) -> Self {
        Self {
            revision,
            record: ValidationRecord::default(),
            failures: 0,
            retry_at: None,
            in_flight: None,
        }
    }
}

struct ProviderAuthInner {
    clients: ValidationClients,
    credentials: Arc<dyn CredentialSource>,
    runtime: Mutex<HashMap<String, RuntimeEntry>>,
    next_flight_id: std::sync::atomic::AtomicU64,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
}

#[derive(Clone)]
pub(crate) struct ProviderAuthManager {
    inner: Arc<ProviderAuthInner>,
}

impl ProviderAuthManager {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        Self::with_dependencies(Arc::new(SystemCredentialSource), Arc::new(Instant::now))
    }

    fn with_dependencies(
        credentials: Arc<dyn CredentialSource>,
        clock: Arc<dyn Fn() -> Instant + Send + Sync>,
    ) -> Result<Self, reqwest::Error> {
        Ok(Self {
            inner: Arc::new(ProviderAuthInner {
                clients: ValidationClients {
                    strict: build_client(false)?,
                    relaxed_local: build_client(true)?,
                },
                credentials,
                runtime: Mutex::new(HashMap::new()),
                next_flight_id: std::sync::atomic::AtomicU64::new(1),
                clock,
            }),
        })
    }

    pub(crate) fn resolve_api_authentication(
        &self,
        target: &TrustedAuthenticationTarget,
    ) -> Result<Option<ProviderAuthMaterial>, ProviderAuthError> {
        if target.authentication.api == ServiceApiAuthentication::None {
            return Ok(None);
        }
        self.ensure_transport_allowed(target)?;
        let revision = self.revision_for(target);
        self.ensure_not_backing_off(&target.service_id, &revision)?;
        self.load_material(target, true, revision).map(Some)
    }

    pub(crate) fn resolve_browser_authentication(
        &self,
        target: &TrustedAuthenticationTarget,
    ) -> Result<Option<ProviderAuthMaterial>, ProviderAuthError> {
        if target.authentication.browser == ServiceBrowserAuthentication::None {
            return Ok(None);
        }
        self.ensure_transport_allowed(target)?;
        let revision = self.revision_for(target);
        // API validation backoff must not disable a separate WebView2 Basic
        // challenge. Browser submissions have their own per-view attempt
        // budget and remain independent from provider API polling.
        self.load_material(target, false, revision).map(Some)
    }

    pub(crate) fn revision_for_target(&self, target: &TrustedAuthenticationTarget) -> String {
        self.revision_for(target)
    }

    pub(crate) fn retry_after_ms_for_target(
        &self,
        target: &TrustedAuthenticationTarget,
    ) -> Option<u64> {
        let revision = self.revision_for(target);
        let (_, _, retry_after_ms) =
            self.runtime_snapshot(&target.service_id, &revision, (self.inner.clock)());
        retry_after_ms
    }

    pub(crate) fn report_success(&self, service_id: &str, revision: &str) {
        let mut runtime = self.lock_runtime();
        let Some(entry) = runtime.get_mut(service_id) else {
            return;
        };
        if entry.revision != revision {
            return;
        }
        entry.failures = 0;
        entry.retry_at = None;
        entry.record = ValidationRecord {
            state: SnapshotValidationState::Valid,
            reason: None,
        };
    }

    pub(crate) fn report_auth_failure(&self, service_id: &str, revision: &str) {
        self.record_failure(
            service_id,
            revision,
            ValidationRecord {
                state: SnapshotValidationState::Invalid,
                reason: Some(SnapshotReasonCode::Unauthorized),
            },
        );
    }

    pub(crate) fn report_rate_limited(&self, service_id: &str, revision: &str) {
        self.record_failure(
            service_id,
            revision,
            ValidationRecord {
                state: SnapshotValidationState::TemporarilyUnavailable,
                reason: Some(SnapshotReasonCode::RateLimited),
            },
        );
    }

    fn revision_for(&self, target: &TrustedAuthenticationTarget) -> String {
        format!(
            "v1:{}:{}",
            target.catalog_revision,
            self.inner.credentials.revision(&target.service_id)
        )
    }

    fn ensure_transport_allowed(
        &self,
        target: &TrustedAuthenticationTarget,
    ) -> Result<(), ProviderAuthError> {
        if target.url.scheme() == "https" || target.authentication.allow_insecure_local_http {
            Ok(())
        } else {
            Err(ProviderAuthError::new(
                ProviderAuthErrorKind::InsecureTransport,
                "Provider credentials cannot be sent over insecure HTTP without explicit local opt-in.",
            ))
        }
    }

    fn ensure_not_backing_off(
        &self,
        service_id: &str,
        revision: &str,
    ) -> Result<(), ProviderAuthError> {
        let now = (self.inner.clock)();
        let mut runtime = self.lock_runtime();
        let entry = runtime
            .entry(service_id.to_string())
            .or_insert_with(|| RuntimeEntry::new(revision.to_string()));
        if entry.revision != revision {
            *entry = RuntimeEntry::new(revision.to_string());
        }
        clear_expired_backoff(entry, now);
        if let Some(retry_at) = entry.retry_at.filter(|retry_at| *retry_at > now) {
            return Err(ProviderAuthError::backoff(remaining_ms(now, retry_at)));
        }
        Ok(())
    }

    fn load_material(
        &self,
        target: &TrustedAuthenticationTarget,
        include_api: bool,
        revision: String,
    ) -> Result<ProviderAuthMaterial, ProviderAuthError> {
        let mut http_basic = None;
        let api = if include_api {
            match target.authentication.api {
                ServiceApiAuthentication::None => None,
                ServiceApiAuthentication::HomarrApiKey => Some(ApiCredential::HomarrApiKey(
                    self.read_credential(target, CredentialKind::ApiKey)?,
                )),
                ServiceApiAuthentication::GlancesHttpBasic => {
                    let credential = self.read_credential(target, CredentialKind::HttpBasic)?;
                    http_basic = Some(Arc::clone(&credential));
                    Some(ApiCredential::GlancesHttpBasic(credential))
                }
                ServiceApiAuthentication::GlancesBearer => Some(ApiCredential::GlancesBearer(
                    self.read_credential(target, CredentialKind::BearerToken)?,
                )),
                ServiceApiAuthentication::QbittorrentWebApi => {
                    Some(ApiCredential::QbittorrentUsernamePassword(
                        self.read_credential(target, CredentialKind::UsernamePassword)?,
                    ))
                }
            }
        } else {
            None
        };

        let browser_basic =
            if target.authentication.browser == ServiceBrowserAuthentication::HttpBasic {
                match http_basic {
                    Some(credential) => Some(credential),
                    None => Some(self.read_credential(target, CredentialKind::HttpBasic)?),
                }
            } else {
                None
            };

        Ok(ProviderAuthMaterial {
            expected_origin: target.canonical_origin.clone(),
            revision,
            api,
            browser_basic,
        })
    }

    fn read_credential(
        &self,
        target: &TrustedAuthenticationTarget,
        kind: CredentialKind,
    ) -> Result<Arc<OriginBoundCredential>, ProviderAuthError> {
        match self
            .inner
            .credentials
            .read(&target.service_id, kind, &target.canonical_origin)
            .map_err(|_| {
                ProviderAuthError::new(
                    ProviderAuthErrorKind::VaultUnavailable,
                    "The Windows credential vault is unavailable.",
                )
            })? {
            CredentialReadResult::Missing => Err(ProviderAuthError::new(
                ProviderAuthErrorKind::MissingCredential,
                "A required provider credential is missing.",
            )),
            CredentialReadResult::NeedsRebind => Err(ProviderAuthError::new(
                ProviderAuthErrorKind::EndpointChanged,
                "A required credential must be saved again for the current service origin.",
            )),
            CredentialReadResult::Available(credential) => Ok(Arc::new(credential)),
        }
    }

    fn lock_runtime(&self) -> std::sync::MutexGuard<'_, HashMap<String, RuntimeEntry>> {
        self.inner
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn record_failure(&self, service_id: &str, revision: &str, record: ValidationRecord) {
        let now = (self.inner.clock)();
        let mut runtime = self.lock_runtime();
        let entry = runtime
            .entry(service_id.to_string())
            .or_insert_with(|| RuntimeEntry::new(revision.to_string()));
        if entry.revision != revision {
            *entry = RuntimeEntry::new(revision.to_string());
        }
        entry.record = record;
        entry.failures = entry.failures.saturating_add(1);
        entry.retry_at = Some(now + backoff_duration(entry.failures));
    }
}

#[tauri::command]
pub fn get_service_authentication_status(
    caller: Webview,
    service_id: String,
    catalog: tauri::State<'_, ServiceCatalog>,
    manager: tauri::State<'_, ProviderAuthManager>,
) -> Result<ServiceAuthenticationSnapshot, String> {
    ensure_trusted_caller(&caller)?;
    let target = catalog.resolve_authentication_target(&service_id)?;
    manager.snapshot(&target, None)
}

#[tauri::command]
pub async fn validate_service_authentication(
    caller: Webview,
    request: ValidateServiceAuthenticationRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    manager: tauri::State<'_, ProviderAuthManager>,
) -> Result<ServiceAuthenticationSnapshot, String> {
    ensure_trusted_caller(&caller)?;
    let target = catalog.resolve_authentication_target(&request.service_id)?;
    let current = manager.snapshot(&target, None)?;
    if request
        .expected_revision
        .as_deref()
        .is_some_and(|expected| expected != current.revision)
        || !current.can_validate
    {
        return Ok(current);
    }

    let completion = manager.validate_singleflight(target.clone()).await;
    let latest_target = catalog.resolve_authentication_target(&request.service_id)?;
    manager.snapshot(&latest_target, Some(&completion))
}

impl ProviderAuthManager {
    fn snapshot(
        &self,
        target: &TrustedAuthenticationTarget,
        completion: Option<&ValidationCompletion>,
    ) -> Result<ServiceAuthenticationSnapshot, String> {
        let revision = self.revision_for(target);
        let required_credential_kinds = required_credential_kinds(target);
        let credential_state =
            self.inspect_required_credentials(target, &required_credential_kinds);
        let now = (self.inner.clock)();
        let (mut validation_state, mut reason_code, mut retry_after_ms) =
            self.runtime_snapshot(&target.service_id, &revision, now);

        if let Some(completion) = completion.filter(|completion| completion.revision == revision) {
            validation_state = completion.record.state;
            reason_code = completion.record.reason;
            // A just-completed failure is shown as invalid/unavailable. Its
            // retry delay becomes visible only on the following status read.
            if completion.record.state != SnapshotValidationState::Backoff {
                retry_after_ms = None;
            }
        }

        if required_credential_kinds.is_empty() {
            validation_state = SnapshotValidationState::Unsupported;
            reason_code = None;
            retry_after_ms = None;
        } else {
            match credential_state {
                SnapshotCredentialState::Missing => {
                    validation_state = SnapshotValidationState::NotValidated;
                    reason_code = Some(SnapshotReasonCode::MissingCredential);
                    retry_after_ms = None;
                }
                SnapshotCredentialState::NeedsRebind => {
                    validation_state = SnapshotValidationState::NotValidated;
                    reason_code = Some(SnapshotReasonCode::EndpointChanged);
                    retry_after_ms = None;
                }
                SnapshotCredentialState::VaultUnavailable => {
                    validation_state = SnapshotValidationState::TemporarilyUnavailable;
                    reason_code = Some(SnapshotReasonCode::VaultUnavailable);
                    retry_after_ms = None;
                }
                SnapshotCredentialState::Stored if !target.enabled => {
                    validation_state = SnapshotValidationState::TemporarilyUnavailable;
                    reason_code = Some(SnapshotReasonCode::ApiUnavailable);
                    retry_after_ms = None;
                }
                SnapshotCredentialState::Stored
                    if target.url.scheme() != "https"
                        && !target.authentication.allow_insecure_local_http =>
                {
                    validation_state = SnapshotValidationState::TemporarilyUnavailable;
                    reason_code = Some(SnapshotReasonCode::InsecureTransport);
                    retry_after_ms = None;
                }
                SnapshotCredentialState::NotRequired | SnapshotCredentialState::Stored => {}
            }
        }

        let can_validate = target.enabled
            && credential_state == SnapshotCredentialState::Stored
            && validation_state != SnapshotValidationState::Validating
            && validation_state != SnapshotValidationState::Backoff
            && (target.url.scheme() == "https" || target.authentication.allow_insecure_local_http);

        Ok(ServiceAuthenticationSnapshot {
            service_id: target.service_id.clone(),
            revision,
            api_adapter: target.authentication.api,
            browser_adapter: target.authentication.browser,
            required_credential_kinds,
            credential_state,
            validation_state,
            can_validate,
            // Phase 7.1 clears a cached browser session automatically during
            // credential rotation. It does not expose a standalone clear
            // command yet, so advertising this capability would be misleading.
            can_clear_session: false,
            reason_code,
            retry_after_ms,
        })
    }

    fn inspect_required_credentials(
        &self,
        target: &TrustedAuthenticationTarget,
        required: &[CredentialKind],
    ) -> SnapshotCredentialState {
        if required.is_empty() {
            return SnapshotCredentialState::NotRequired;
        }

        let mut result = SnapshotCredentialState::Stored;
        for kind in required {
            match self.inner.credentials.inspect(
                &target.service_id,
                *kind,
                &target.canonical_origin,
            ) {
                Ok(CredentialBindingState::Available) => {}
                Ok(CredentialBindingState::Missing) => return SnapshotCredentialState::Missing,
                Ok(CredentialBindingState::NeedsRebind) => {
                    result = SnapshotCredentialState::NeedsRebind;
                }
                Err(_) => return SnapshotCredentialState::VaultUnavailable,
            }
        }
        result
    }

    fn runtime_snapshot(
        &self,
        service_id: &str,
        revision: &str,
        now: Instant,
    ) -> (
        SnapshotValidationState,
        Option<SnapshotReasonCode>,
        Option<u64>,
    ) {
        let mut runtime = self.lock_runtime();
        let entry = runtime
            .entry(service_id.to_string())
            .or_insert_with(|| RuntimeEntry::new(revision.to_string()));
        if entry.revision != revision {
            *entry = RuntimeEntry::new(revision.to_string());
        }
        clear_expired_backoff(entry, now);
        if entry.in_flight.is_some() {
            return (
                SnapshotValidationState::Validating,
                Some(SnapshotReasonCode::ValidationInProgress),
                None,
            );
        }
        if let Some(retry_at) = entry.retry_at.filter(|retry_at| *retry_at > now) {
            return (
                SnapshotValidationState::Backoff,
                entry.record.reason,
                Some(remaining_ms(now, retry_at)),
            );
        }
        (entry.record.state, entry.record.reason, None)
    }

    async fn validate_singleflight(
        &self,
        target: TrustedAuthenticationTarget,
    ) -> ValidationCompletion {
        let revision = self.revision_for(&target);
        let (flight_id, future) = {
            let mut runtime = self.lock_runtime();
            let entry = runtime
                .entry(target.service_id.clone())
                .or_insert_with(|| RuntimeEntry::new(revision.clone()));
            if entry.revision != revision {
                *entry = RuntimeEntry::new(revision.clone());
            }
            if let Some(in_flight) = &entry.in_flight {
                (in_flight.id, in_flight.future.clone())
            } else {
                let flight_id = self
                    .inner
                    .next_flight_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let manager = self.clone();
                let target = target.clone();
                let expected_revision = revision.clone();
                let future =
                    async move { manager.perform_validation(target, expected_revision).await }
                        .boxed()
                        .shared();
                entry.in_flight = Some(InFlightValidation {
                    id: flight_id,
                    future: future.clone(),
                });
                (flight_id, future)
            }
        };

        let completion = future.await;
        let mut runtime = self.lock_runtime();
        let entry = runtime
            .entry(target.service_id.clone())
            .or_insert_with(|| RuntimeEntry::new(revision.clone()));
        if entry.revision == revision
            && entry
                .in_flight
                .as_ref()
                .is_some_and(|flight| flight.id == flight_id)
        {
            entry.in_flight = None;
            entry.record = completion.record;
            if completion.record.state == SnapshotValidationState::Valid {
                entry.failures = 0;
                entry.retry_at = None;
            } else if completion.record.state == SnapshotValidationState::Invalid
                || completion.record.reason == Some(SnapshotReasonCode::RateLimited)
            {
                entry.failures = entry.failures.saturating_add(1);
                entry.retry_at = Some((self.inner.clock)() + backoff_duration(entry.failures));
            } else {
                entry.retry_at = None;
            }
        }
        completion
    }

    async fn perform_validation(
        &self,
        target: TrustedAuthenticationTarget,
        revision: String,
    ) -> ValidationCompletion {
        let record = if self.revision_for(&target) != revision {
            ValidationRecord {
                state: SnapshotValidationState::TemporarilyUnavailable,
                reason: Some(SnapshotReasonCode::EndpointChanged),
            }
        } else {
            match self.ensure_transport_allowed(&target) {
                Err(error) => validation_record_for_provider_error(error),
                Ok(()) => match self.load_material(&target, true, revision.clone()) {
                    Err(error) => validation_record_for_provider_error(error),
                    Ok(material) => self.validate_with_material(&target, &material).await,
                },
            }
        };
        ValidationCompletion { revision, record }
    }

    async fn validate_with_material(
        &self,
        target: &TrustedAuthenticationTarget,
        material: &ProviderAuthMaterial,
    ) -> ValidationRecord {
        let client = if target.allow_invalid_local_certificate {
            &self.inner.clients.relaxed_local
        } else {
            &self.inner.clients.strict
        };

        match target.authentication.api {
            ServiceApiAuthentication::HomarrApiKey => {
                let url = provider_url(&target.url, "api/info");
                validate_homarr_info(client, material, url).await
            }
            ServiceApiAuthentication::GlancesHttpBasic
            | ServiceApiAuthentication::GlancesBearer => {
                let v4 = provider_url(&target.url, "api/4/status");
                let first = validate_url(client, material, v4).await;
                if first.reason == Some(SnapshotReasonCode::ApiUnavailable) {
                    let v3 = provider_url(&target.url, "api/3/status");
                    validate_url(client, material, v3).await
                } else {
                    first
                }
            }
            ServiceApiAuthentication::QbittorrentWebApi => {
                let login_url = provider_url(&target.url, "api/v2/auth/login");
                let version_url = provider_url(&target.url, "api/v2/app/webapiVersion");
                let logout_url = provider_url(&target.url, "api/v2/auth/logout");
                validate_qbittorrent_login(client, material, login_url, version_url, logout_url)
                    .await
            }
            ServiceApiAuthentication::None => {
                validate_url(client, material, Some(target.url.clone())).await
            }
        }
    }
}

async fn validate_url(
    client: &Client,
    material: &ProviderAuthMaterial,
    url: Option<Url>,
) -> ValidationRecord {
    let Some(url) = url else {
        return ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        };
    };
    let request = match material.apply(client.get(url.clone()), &url) {
        Ok(request) => request,
        Err(error) => return validation_record_for_provider_error(error),
    };
    match request.send().await {
        Ok(response) => validation_record_for_status(response.status()),
        Err(error) => validation_record_for_request_error(&error),
    }
}

#[derive(Deserialize)]
struct HomarrInfo {
    version: String,
}

async fn validate_homarr_info(
    client: &Client,
    material: &ProviderAuthMaterial,
    url: Option<Url>,
) -> ValidationRecord {
    let Some(url) = url else {
        return ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        };
    };
    let request = match material.apply(client.get(url.clone()), &url) {
        Ok(request) => request,
        Err(error) => return validation_record_for_provider_error(error),
    };
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) => return validation_record_for_request_error(&error),
    };
    if !response.status().is_success() {
        return validation_record_for_status(response.status());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HOMARR_INFO_BYTES as u64)
    {
        return ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        };
    }

    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > MAX_HOMARR_INFO_BYTES {
                    body.fill(0);
                    return ValidationRecord {
                        state: SnapshotValidationState::TemporarilyUnavailable,
                        reason: Some(SnapshotReasonCode::InvalidData),
                    };
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => return validation_record_for_request_error(&error),
        }
    }

    let valid = serde_json::from_slice::<HomarrInfo>(&body)
        .ok()
        .is_some_and(|info| {
            !info.version.trim().is_empty()
                && info.version == info.version.trim()
                && info.version.chars().count() <= MAX_HOMARR_VERSION_LENGTH
                && !info.version.chars().any(char::is_control)
        });
    body.fill(0);
    if valid {
        ValidationRecord {
            state: SnapshotValidationState::Valid,
            reason: None,
        }
    } else {
        ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        }
    }
}

async fn validate_qbittorrent_login(
    client: &Client,
    material: &ProviderAuthMaterial,
    login_url: Option<Url>,
    version_url: Option<Url>,
    logout_url: Option<Url>,
) -> ValidationRecord {
    let (Some(login_url), Some(version_url), Some(logout_url)) =
        (login_url, version_url, logout_url)
    else {
        return ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        };
    };
    let (username, password) = match material.qbittorrent_credentials_for(&login_url) {
        Ok(credentials) => credentials,
        Err(error) => return validation_record_for_provider_error(error),
    };
    let referer = format!("{}/", material.expected_origin.trim_end_matches('/'));
    let request = match material.apply(client.post(login_url.clone()), &login_url) {
        Ok(request) => request,
        Err(error) => return validation_record_for_provider_error(error),
    };
    let login_body = qbittorrent_login_form(username, password);
    let request = request
        .header("Origin", material.expected_origin.as_str())
        .header("Referer", referer.as_str())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(login_body.expose().to_owned());
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => return validation_record_for_request_error(&error),
    };
    let session_cookie = extract_qbittorrent_session_cookie(response.headers());
    let record = validate_qbittorrent_login_response(
        client,
        material,
        response,
        &session_cookie,
        &version_url,
        &referer,
    )
    .await;

    // Validation owns a temporary session. Release it even when the login
    // body or subsequent version check is rejected, truncated, or unavailable.
    if let Ok(session_cookie) = &session_cookie {
        if let Ok(cookie_header) = qbittorrent_cookie_header(session_cookie) {
            if let Ok(logout_request) = material.apply(client.post(logout_url.clone()), &logout_url)
            {
                let _ = logout_request
                    .header("Origin", material.expected_origin.as_str())
                    .header("Referer", referer.as_str())
                    .header(COOKIE, cookie_header)
                    .send()
                    .await;
            }
        }
    }
    record
}

async fn validate_qbittorrent_login_response(
    client: &Client,
    material: &ProviderAuthMaterial,
    mut response: reqwest::Response,
    session_cookie: &Result<SensitiveString, ProviderAuthError>,
    version_url: &Url,
    referer: &str,
) -> ValidationRecord {
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::NO_CONTENT {
        return validation_record_for_status(status);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_QBITTORRENT_LOGIN_BYTES as u64)
    {
        return ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        };
    }

    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > MAX_QBITTORRENT_LOGIN_BYTES {
                    body.fill(0);
                    return ValidationRecord {
                        state: SnapshotValidationState::TemporarilyUnavailable,
                        reason: Some(SnapshotReasonCode::InvalidData),
                    };
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => {
                body.fill(0);
                return validation_record_for_request_error(&error);
            }
        }
    }

    let rejected = status == StatusCode::OK && trim_ascii_whitespace(&body) == b"Fails.";
    body.fill(0);
    if rejected {
        return ValidationRecord {
            state: SnapshotValidationState::Invalid,
            reason: Some(SnapshotReasonCode::Unauthorized),
        };
    }

    let session_cookie = match session_cookie {
        Ok(cookie) => cookie,
        Err(error) => return validation_record_for_provider_error(error.clone()),
    };

    let cookie_header = match qbittorrent_cookie_header(session_cookie) {
        Ok(header) => header,
        Err(error) => return validation_record_for_provider_error(error),
    };
    let version_request = match material.apply(client.get(version_url.clone()), version_url) {
        Ok(request) => request,
        Err(error) => return validation_record_for_provider_error(error),
    };
    let mut response = match version_request
        .header("Origin", material.expected_origin.as_str())
        .header("Referer", referer)
        .header(COOKIE, cookie_header)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return validation_record_for_request_error(&error),
    };
    if response.status() != StatusCode::OK {
        return validation_record_for_status(response.status());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_QBITTORRENT_VERSION_BYTES as u64)
    {
        return ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        };
    }
    let mut version = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if version.len().saturating_add(chunk.len()) > MAX_QBITTORRENT_VERSION_BYTES {
                    version.fill(0);
                    return ValidationRecord {
                        state: SnapshotValidationState::TemporarilyUnavailable,
                        reason: Some(SnapshotReasonCode::InvalidData),
                    };
                }
                version.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => {
                version.fill(0);
                return validation_record_for_request_error(&error);
            }
        }
    }
    let valid_version = std::str::from_utf8(trim_ascii_whitespace(&version))
        .ok()
        .is_some_and(|value| {
            !value.is_empty()
                && value.chars().count() <= 128
                && !value.chars().any(char::is_control)
        });
    version.fill(0);
    if valid_version {
        ValidationRecord {
            state: SnapshotValidationState::Valid,
            reason: None,
        }
    } else {
        ValidationRecord {
            state: SnapshotValidationState::TemporarilyUnavailable,
            reason: Some(SnapshotReasonCode::InvalidData),
        }
    }
}

pub(crate) fn extract_qbittorrent_session_cookie(
    headers: &HeaderMap,
) -> Result<SensitiveString, ProviderAuthError> {
    let mut fallback = None;
    let mut fallback_is_ambiguous = false;

    for value in headers.get_all(SET_COOKIE) {
        let raw = value.to_str().map_err(|_| {
            ProviderAuthError::new(
                ProviderAuthErrorKind::InvalidData,
                "qBittorrent returned an invalid session cookie.",
            )
        })?;
        let Some(pair) = parse_cookie_pair(raw) else {
            continue;
        };
        let cookie_name = pair.split_once('=').map(|(name, _)| name).unwrap_or("");
        if cookie_name == "SID" || cookie_name.starts_with("QBT_SID_") {
            return Ok(SensitiveString::new(pair));
        }

        if fallback.is_some() {
            fallback_is_ambiguous = true;
        } else {
            fallback = Some(pair);
        }
    }

    fallback
        .filter(|_| !fallback_is_ambiguous)
        .map(SensitiveString::new)
        .ok_or_else(|| {
            ProviderAuthError::new(
                ProviderAuthErrorKind::InvalidData,
                "qBittorrent did not return an unambiguous session cookie.",
            )
        })
}

pub(crate) fn qbittorrent_cookie_header(
    cookie: &SensitiveString,
) -> Result<HeaderValue, ProviderAuthError> {
    let mut value = HeaderValue::from_str(cookie.expose()).map_err(|_| {
        ProviderAuthError::new(
            ProviderAuthErrorKind::InvalidData,
            "The qBittorrent session cookie is invalid.",
        )
    })?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn qbittorrent_login_form(username: &str, password: &str) -> SensitiveString {
    let mut body = String::with_capacity(username.len().saturating_add(password.len() + 19));
    body.push_str("username=");
    append_form_component(&mut body, username);
    body.push_str("&password=");
    append_form_component(&mut body, password);
    SensitiveString::new(body)
}

fn append_form_component(output: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                output.push(char::from(byte));
            }
            b' ' => output.push('+'),
            _ => {
                output.push('%');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
}

fn parse_cookie_pair(value: &str) -> Option<String> {
    let pair = value.split(';').next()?.trim();
    if pair.is_empty() || pair.len() > MAX_QBITTORRENT_COOKIE_PAIR_BYTES {
        return None;
    }
    let (name, cookie_value) = pair.split_once('=')?;
    if name.is_empty()
        || cookie_value.is_empty()
        || !name.bytes().all(is_cookie_token_byte)
        || cookie_value
            .bytes()
            .any(|byte| byte <= 0x20 || byte >= 0x7f || byte == b';' || byte == b',')
    {
        return None;
    }
    Some(format!("{name}={cookie_value}"))
}

fn is_cookie_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
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

fn validation_record_for_status(status: StatusCode) -> ValidationRecord {
    let (state, reason) = if status.is_success() {
        (SnapshotValidationState::Valid, None)
    } else {
        match status {
            StatusCode::UNAUTHORIZED => (
                SnapshotValidationState::Invalid,
                Some(SnapshotReasonCode::Unauthorized),
            ),
            StatusCode::FORBIDDEN => (
                SnapshotValidationState::Invalid,
                Some(SnapshotReasonCode::Forbidden),
            ),
            StatusCode::TOO_MANY_REQUESTS => (
                SnapshotValidationState::Backoff,
                Some(SnapshotReasonCode::RateLimited),
            ),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => (
                SnapshotValidationState::TemporarilyUnavailable,
                Some(SnapshotReasonCode::InvalidData),
            ),
            _ => (
                SnapshotValidationState::TemporarilyUnavailable,
                Some(SnapshotReasonCode::ApiUnavailable),
            ),
        }
    };
    ValidationRecord { state, reason }
}

fn validation_record_for_provider_error(error: ProviderAuthError) -> ValidationRecord {
    let reason = match error.kind() {
        ProviderAuthErrorKind::MissingCredential => SnapshotReasonCode::MissingCredential,
        ProviderAuthErrorKind::EndpointChanged => SnapshotReasonCode::EndpointChanged,
        ProviderAuthErrorKind::VaultUnavailable => SnapshotReasonCode::VaultUnavailable,
        ProviderAuthErrorKind::InsecureTransport => SnapshotReasonCode::InsecureTransport,
        ProviderAuthErrorKind::InvalidData => SnapshotReasonCode::InvalidData,
        ProviderAuthErrorKind::Backoff => SnapshotReasonCode::ApiUnavailable,
    };
    ValidationRecord {
        state: SnapshotValidationState::TemporarilyUnavailable,
        reason: Some(reason),
    }
}

fn validation_record_for_request_error(error: &reqwest::Error) -> ValidationRecord {
    let reason = if error.is_timeout() {
        SnapshotReasonCode::Timeout
    } else if error_chain_mentions_tls(error) {
        SnapshotReasonCode::Tls
    } else if error.is_connect() {
        SnapshotReasonCode::Connection
    } else {
        SnapshotReasonCode::ApiUnavailable
    };
    ValidationRecord {
        state: SnapshotValidationState::TemporarilyUnavailable,
        reason: Some(reason),
    }
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

fn required_credential_kinds(target: &TrustedAuthenticationTarget) -> Vec<CredentialKind> {
    CredentialKind::ALL
        .into_iter()
        .filter(|kind| match kind {
            CredentialKind::ApiKey => {
                target.authentication.api == ServiceApiAuthentication::HomarrApiKey
            }
            CredentialKind::BearerToken => {
                target.authentication.api == ServiceApiAuthentication::GlancesBearer
            }
            CredentialKind::HttpBasic => {
                target.authentication.api == ServiceApiAuthentication::GlancesHttpBasic
                    || target.authentication.browser == ServiceBrowserAuthentication::HttpBasic
            }
            CredentialKind::UsernamePassword => {
                target.authentication.api == ServiceApiAuthentication::QbittorrentWebApi
            }
        })
        .collect()
}

fn provider_url(base_url: &Url, suffix: &str) -> Option<Url> {
    let mut root = base_url.clone();
    root.set_query(None);
    root.set_fragment(None);
    let path = format!("{}/", root.path().trim_end_matches('/'));
    root.set_path(&path);
    root.join(suffix).ok()
}

fn canonical_origin(url: &Url) -> Option<String> {
    let scheme = url.scheme();
    if !matches!(scheme, "http" | "https") || !url.username().is_empty() || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.');
    let host = if host.contains(':') {
        format!("[{host}]").to_ascii_lowercase()
    } else {
        host.trim_end_matches('.').to_ascii_lowercase()
    };
    Some(format!("{scheme}://{host}:{port}"))
}

fn build_client(allow_invalid_local_certificate: bool) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(AUTH_TIMEOUT)
        .redirect(Policy::none())
        .http1_only()
        .user_agent(AUTH_USER_AGENT)
        .tls_danger_accept_invalid_certs(allow_invalid_local_certificate)
        .build()
}

fn backoff_duration(failures: usize) -> Duration {
    BACKOFF_STEPS[failures.saturating_sub(1).min(BACKOFF_STEPS.len() - 1)]
}

fn clear_expired_backoff(entry: &mut RuntimeEntry, now: Instant) {
    if entry.retry_at.is_some_and(|retry_at| retry_at <= now) {
        entry.retry_at = None;
        // Rate limiting is represented as Backoff only while a positive retry
        // delay exists. Once that delay expires, return to a valid retryable
        // state instead of emitting an impossible `backoff` snapshot.
        if entry.record.state == SnapshotValidationState::Backoff {
            entry.record = ValidationRecord::default();
        }
    }
}

fn remaining_ms(now: Instant, deadline: Instant) -> u64 {
    let millis = deadline.saturating_duration_since(now).as_millis();
    millis.clamp(1, u64::MAX as u128) as u64
}

fn ensure_trusted_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() == MAIN_WEBVIEW_LABEL {
        Ok(())
    } else {
        Err("Provider authentication is available only to the trusted Atrium UI.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_settings::ServiceAuthentication;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicU64, AtomicUsize, Ordering},
            mpsc, Arc,
        },
        thread,
    };

    #[derive(Clone)]
    enum FakeCredential {
        Missing,
        NeedsRebind,
        Available {
            username: Option<String>,
            secret: String,
        },
        VaultUnavailable,
    }

    struct FakeCredentialSource {
        revision: AtomicU64,
        entries: Mutex<HashMap<CredentialKind, FakeCredential>>,
        reads: AtomicUsize,
        inspections: AtomicUsize,
    }

    impl FakeCredentialSource {
        fn new(entries: impl IntoIterator<Item = (CredentialKind, FakeCredential)>) -> Self {
            Self {
                revision: AtomicU64::new(1),
                entries: Mutex::new(entries.into_iter().collect()),
                reads: AtomicUsize::new(0),
                inspections: AtomicUsize::new(0),
            }
        }

        fn rotate(&self) {
            self.revision.fetch_add(1, Ordering::SeqCst);
        }

        fn entry(&self, kind: CredentialKind) -> FakeCredential {
            self.entries
                .lock()
                .unwrap()
                .get(&kind)
                .cloned()
                .unwrap_or(FakeCredential::Missing)
        }
    }

    impl CredentialSource for FakeCredentialSource {
        fn revision(&self, _service_id: &str) -> u64 {
            self.revision.load(Ordering::SeqCst)
        }

        fn inspect(
            &self,
            _service_id: &str,
            kind: CredentialKind,
            _origin: &str,
        ) -> Result<CredentialBindingState, String> {
            self.inspections.fetch_add(1, Ordering::SeqCst);
            match self.entry(kind) {
                FakeCredential::Missing => Ok(CredentialBindingState::Missing),
                FakeCredential::NeedsRebind => Ok(CredentialBindingState::NeedsRebind),
                FakeCredential::Available { .. } => Ok(CredentialBindingState::Available),
                FakeCredential::VaultUnavailable => Err("injected vault failure".into()),
            }
        }

        fn read(
            &self,
            _service_id: &str,
            kind: CredentialKind,
            _origin: &str,
        ) -> Result<CredentialReadResult, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match self.entry(kind) {
                FakeCredential::Missing => Ok(CredentialReadResult::Missing),
                FakeCredential::NeedsRebind => Ok(CredentialReadResult::NeedsRebind),
                FakeCredential::Available { username, secret } => {
                    Ok(CredentialReadResult::Available(
                        OriginBoundCredential::for_test(username.as_deref(), &secret),
                    ))
                }
                FakeCredential::VaultUnavailable => Err("injected vault failure".into()),
            }
        }
    }

    fn available(username: Option<&str>, secret: &str) -> FakeCredential {
        FakeCredential::Available {
            username: username.map(str::to_string),
            secret: secret.to_string(),
        }
    }

    fn target(
        service_id: &str,
        url: &str,
        api: ServiceApiAuthentication,
        browser: ServiceBrowserAuthentication,
        allow_insecure_local_http: bool,
    ) -> TrustedAuthenticationTarget {
        let url = Url::parse(url).unwrap();
        TrustedAuthenticationTarget {
            service_id: service_id.to_string(),
            enabled: true,
            canonical_origin: canonical_origin(&url).unwrap(),
            url,
            allow_invalid_local_certificate: false,
            authentication: ServiceAuthentication {
                api,
                browser,
                allow_insecure_local_http,
            },
            catalog_revision: 7,
        }
    }

    fn manager_with_source(source: Arc<FakeCredentialSource>) -> ProviderAuthManager {
        ProviderAuthManager::with_dependencies(source, Arc::new(Instant::now)).unwrap()
    }

    fn manager_with_clock(
        source: Arc<FakeCredentialSource>,
        now: Arc<Mutex<Instant>>,
    ) -> ProviderAuthManager {
        let clock_now = Arc::clone(&now);
        ProviderAuthManager::with_dependencies(source, Arc::new(move || *clock_now.lock().unwrap()))
            .unwrap()
    }

    #[test]
    fn snapshot_uses_the_exact_safe_wire_contract_and_canonical_kind_order() {
        let source = Arc::new(FakeCredentialSource::new([
            (CredentialKind::ApiKey, available(None, "homarr-secret")),
            (
                CredentialKind::HttpBasic,
                available(Some("proxy-user"), "proxy-password"),
            ),
        ]));
        let manager = manager_with_source(source);
        let target = target(
            "homarr",
            "https://homarr.test/dashboard",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::HttpBasic,
            false,
        );

        let value = serde_json::to_value(manager.snapshot(&target, None).unwrap()).unwrap();

        assert_eq!(
            value,
            json!({
                "serviceId": "homarr",
                "revision": "v1:7:1",
                "apiAdapter": "homarr-api-key",
                "browserAdapter": "http-basic",
                "requiredCredentialKinds": ["api-key", "http-basic"],
                "credentialState": "stored",
                "validationState": "not-validated",
                "canValidate": true,
                "canClearSession": false,
                "reasonCode": null,
                "retryAfterMs": null
            })
        );
        assert_eq!(value.as_object().unwrap().len(), 11);
        let document = value.to_string();
        assert!(!document.contains("homarr-secret"));
        assert!(!document.contains("proxy-password"));
        assert!(!document.contains("proxy-user"));
    }

    #[test]
    fn snapshot_reports_missing_rebind_and_vault_failures_without_reading_secrets() {
        for (entry, state, reason) in [
            (FakeCredential::Missing, "missing", "missing-credential"),
            (
                FakeCredential::NeedsRebind,
                "needs-rebind",
                "endpoint-changed",
            ),
            (
                FakeCredential::VaultUnavailable,
                "vault-unavailable",
                "vault-unavailable",
            ),
        ] {
            let source = Arc::new(FakeCredentialSource::new([(CredentialKind::ApiKey, entry)]));
            let manager = manager_with_source(Arc::clone(&source));
            let target = target(
                "homarr",
                "https://homarr.test",
                ServiceApiAuthentication::HomarrApiKey,
                ServiceBrowserAuthentication::None,
                false,
            );
            let value = serde_json::to_value(manager.snapshot(&target, None).unwrap()).unwrap();

            assert_eq!(value["credentialState"], state);
            assert_eq!(value["reasonCode"], reason);
            assert_eq!(value["canValidate"], false);
            assert_eq!(source.reads.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn disabled_services_still_report_missing_credentials_consistently() {
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            FakeCredential::Missing,
        )]));
        let manager = manager_with_source(source);
        let mut target = target(
            "homarr",
            "https://homarr.test",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            false,
        );
        target.enabled = false;

        let value = serde_json::to_value(manager.snapshot(&target, None).unwrap()).unwrap();
        assert_eq!(value["credentialState"], "missing");
        assert_eq!(value["validationState"], "not-validated");
        assert_eq!(value["reasonCode"], "missing-credential");
        assert_eq!(value["canValidate"], false);
    }

    #[test]
    fn combined_homarr_and_reverse_proxy_material_sets_both_headers() {
        let source = Arc::new(FakeCredentialSource::new([
            (CredentialKind::ApiKey, available(None, "key-id.token")),
            (CredentialKind::HttpBasic, available(Some("proxy"), "pass")),
        ]));
        let manager = manager_with_source(source);
        let target = target(
            "homarr",
            "https://Example.Test./base",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::HttpBasic,
            false,
        );
        let material = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();
        let request_url = Url::parse("https://example.test:443/base/api/info").unwrap();
        let request = material
            .apply(Client::new().get(request_url.clone()), &request_url)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(request.headers()["apikey"], "key-id.token");
        assert_eq!(request.headers()["authorization"], "Basic cHJveHk6cGFzcw==");
        let debug = format!("{material:?}");
        assert!(!debug.contains("key-id.token"));
        assert!(!debug.contains("proxy"));
        assert!(!debug.contains("pass"));
    }

    #[test]
    fn request_material_is_restricted_to_the_exact_normalized_origin() {
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            available(None, "private-key"),
        )]));
        let manager = manager_with_source(source);
        let target = target(
            "homarr",
            "https://Example.Test./base",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            false,
        );
        let material = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();

        for rejected in [
            "https://example.test:444/base/api/info",
            "https://sub.example.test/base/api/info",
            "http://example.test:80/base/api/info",
        ] {
            let url = Url::parse(rejected).unwrap();
            let error = material
                .apply(Client::new().get(url.clone()), &url)
                .unwrap_err();
            assert_eq!(error.kind(), ProviderAuthErrorKind::EndpointChanged);
            assert!(!error.to_string().contains("private-key"));
        }

        let accepted = Url::parse("https://example.test/base/elsewhere").unwrap();
        assert!(material
            .apply(Client::new().get(accepted.clone()), &accepted)
            .is_ok());
    }

    #[test]
    fn canonical_origins_normalize_domain_dots_ports_and_ipv6_brackets() {
        assert_eq!(
            canonical_origin(&Url::parse("https://Example.Test./path").unwrap()).unwrap(),
            "https://example.test:443"
        );
        assert_eq!(
            canonical_origin(&Url::parse("http://[2001:db8::1]/").unwrap()).unwrap(),
            "http://[2001:db8::1]:80"
        );
        assert!(canonical_origin(&Url::parse("ftp://example.test/file").unwrap()).is_none());
        assert!(canonical_origin(&Url::parse("https://user@example.test/").unwrap()).is_none());
    }

    #[test]
    fn qbittorrent_login_form_uses_standard_form_encoding() {
        let body = qbittorrent_login_form("user name+ü", "p&=word");
        assert_eq!(
            body.expose(),
            "username=user+name%2B%C3%BC&password=p%26%3Dword"
        );
    }

    #[test]
    fn qbittorrent_cookie_selection_prefers_known_names_and_rejects_ambiguity() {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_static("proxy=one; HttpOnly"));
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("QBT_SID_8080=secret; HttpOnly; SameSite=Strict"),
        );
        assert_eq!(
            extract_qbittorrent_session_cookie(&headers)
                .unwrap()
                .expose(),
            "QBT_SID_8080=secret"
        );

        let mut ambiguous = HeaderMap::new();
        ambiguous.append(SET_COOKIE, HeaderValue::from_static("custom_a=one"));
        ambiguous.append(SET_COOKIE, HeaderValue::from_static("custom_b=two"));
        assert!(extract_qbittorrent_session_cookie(&ambiguous).is_err());
    }

    #[test]
    fn insecure_transport_requires_explicit_local_opt_in() {
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            available(None, "private-key"),
        )]));
        let manager = manager_with_source(source);
        let blocked = target(
            "homarr",
            "http://127.0.0.1:7575",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            false,
        );
        let allowed = target(
            "homarr",
            "http://127.0.0.1:7575",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            true,
        );

        assert_eq!(
            manager
                .resolve_api_authentication(&blocked)
                .unwrap_err()
                .kind(),
            ProviderAuthErrorKind::InsecureTransport
        );
        assert!(manager.resolve_api_authentication(&allowed).is_ok());
    }

    #[test]
    fn backoff_is_bounded_and_credential_rotation_clears_it_by_revision() {
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            available(None, "private-key"),
        )]));
        let now = Arc::new(Mutex::new(Instant::now()));
        let manager = manager_with_clock(Arc::clone(&source), Arc::clone(&now));
        let target = target(
            "homarr",
            "https://homarr.test",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            false,
        );
        let first = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();
        manager.report_auth_failure("homarr", first.revision());
        let error = manager.resolve_api_authentication(&target).unwrap_err();
        assert_eq!(error.kind(), ProviderAuthErrorKind::Backoff);
        assert_eq!(error.retry_after_ms(), Some(5_000));

        *now.lock().unwrap() += Duration::from_secs(5);
        let second = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();
        manager.report_auth_failure("homarr", second.revision());
        assert_eq!(
            manager
                .resolve_api_authentication(&target)
                .unwrap_err()
                .retry_after_ms(),
            Some(30_000)
        );

        *now.lock().unwrap() += Duration::from_secs(30);
        let third = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();
        manager.report_auth_failure("homarr", third.revision());
        assert_eq!(
            manager
                .resolve_api_authentication(&target)
                .unwrap_err()
                .retry_after_ms(),
            Some(300_000)
        );

        source.rotate();
        let rotated = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();
        assert_ne!(rotated.revision(), third.revision());
    }

    #[test]
    fn api_backoff_does_not_block_separate_browser_basic_challenges() {
        let source = Arc::new(FakeCredentialSource::new([
            (CredentialKind::ApiKey, available(None, "private-key")),
            (
                CredentialKind::HttpBasic,
                available(Some("proxy-user"), "proxy-password"),
            ),
        ]));
        let manager = manager_with_source(source);
        let target = target(
            "homarr",
            "https://homarr.test",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::HttpBasic,
            false,
        );
        let api = manager
            .resolve_api_authentication(&target)
            .unwrap()
            .unwrap();
        manager.report_auth_failure("homarr", api.revision());

        assert_eq!(
            manager
                .resolve_api_authentication(&target)
                .unwrap_err()
                .kind(),
            ProviderAuthErrorKind::Backoff
        );
        assert!(manager
            .resolve_browser_authentication(&target)
            .unwrap()
            .is_some());
    }

    #[test]
    fn expired_rate_limit_backoff_returns_to_a_retryable_snapshot() {
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            available(None, "private-key"),
        )]));
        let now = Arc::new(Mutex::new(Instant::now()));
        let manager = manager_with_clock(source, Arc::clone(&now));
        let target = target(
            "homarr",
            "https://homarr.test",
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            false,
        );
        let revision = manager.revision_for(&target);
        manager.record_failure(
            "homarr",
            &revision,
            ValidationRecord {
                state: SnapshotValidationState::Backoff,
                reason: Some(SnapshotReasonCode::RateLimited),
            },
        );

        let waiting = manager.snapshot(&target, None).unwrap();
        assert_eq!(waiting.validation_state, SnapshotValidationState::Backoff);
        assert_eq!(waiting.retry_after_ms, Some(5_000));

        *now.lock().unwrap() += Duration::from_secs(5);
        let retryable = manager.snapshot(&target, None).unwrap();
        assert_eq!(
            retryable.validation_state,
            SnapshotValidationState::NotValidated
        );
        assert_eq!(retryable.reason_code, None);
        assert_eq!(retryable.retry_after_ms, None);
        assert!(retryable.can_validate);
    }

    fn serve_once(
        status: &str,
        extra_headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let headers = extra_headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect::<Vec<_>>();
        let (request_sender, request_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            request_sender
                .send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();

            let mut response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (name, value) in headers {
                response.push_str(&format!("{name}: {value}\r\n"));
            }
            response.push_str("\r\n");
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        (format!("http://{address}/homarr"), request_receiver, handle)
    }

    fn validate_homarr_once(
        status: &str,
        extra_headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (ValidationCompletion, String) {
        let (base_url, request_receiver, server) = serve_once(status, extra_headers, body);
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            available(None, "test-api-key"),
        )]));
        let manager = manager_with_source(source);
        let target = target(
            "homarr",
            &base_url,
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            true,
        );
        let completion = tauri::async_runtime::block_on(manager.validate_singleflight(target));
        let request = request_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        server.join().unwrap();
        (completion, request)
    }

    #[test]
    fn homarr_validation_uses_api_info_and_checks_the_bounded_json_shape() {
        let (valid, request) =
            validate_homarr_once("200 OK", &[], br#"{"version":"1.70.0"}"#.to_vec());
        assert_eq!(valid.record.state, SnapshotValidationState::Valid);
        assert!(request.starts_with("GET /homarr/api/info HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("apikey: test-api-key\r\n"));

        let (invalid, _) = validate_homarr_once("200 OK", &[], br#"{"healthy":true}"#.to_vec());
        assert_eq!(invalid.record.reason, Some(SnapshotReasonCode::InvalidData));

        let oversized = vec![b'x'; MAX_HOMARR_INFO_BYTES + 1];
        let (too_large, _) = validate_homarr_once("200 OK", &[], oversized);
        assert_eq!(
            too_large.record.reason,
            Some(SnapshotReasonCode::InvalidData)
        );
    }

    #[test]
    fn homarr_validation_does_not_follow_redirects() {
        let (completion, _) = validate_homarr_once(
            "302 Found",
            &[("Location", "http://127.0.0.1:9/credential-sink")],
            Vec::new(),
        );
        assert_eq!(
            completion.record.reason,
            Some(SnapshotReasonCode::ApiUnavailable)
        );
    }

    #[test]
    fn concurrent_validation_calls_share_one_network_flight() {
        let (base_url, request_receiver, server) =
            serve_once("200 OK", &[], br#"{"version":"1.70.0"}"#.to_vec());
        let source = Arc::new(FakeCredentialSource::new([(
            CredentialKind::ApiKey,
            available(None, "test-api-key"),
        )]));
        let manager = manager_with_source(Arc::clone(&source));
        let target = target(
            "homarr",
            &base_url,
            ServiceApiAuthentication::HomarrApiKey,
            ServiceBrowserAuthentication::None,
            true,
        );

        let (first, second) = tauri::async_runtime::block_on(async {
            futures_util::join!(
                manager.validate_singleflight(target.clone()),
                manager.validate_singleflight(target.clone())
            )
        });
        let _ = request_receiver
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        server.join().unwrap();

        assert_eq!(first.record.state, SnapshotValidationState::Valid);
        assert_eq!(second.record.state, SnapshotValidationState::Valid);
        assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    }
}
