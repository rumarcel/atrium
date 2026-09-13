use crate::provider_auth::ProviderAuthManager;
#[cfg(test)]
use crate::service_settings::BUNDLED_SERVICE_CONFIG;
use crate::service_settings::{
    ServiceAuthentication, ServiceBrowserAuthentication, ServiceConfiguration, ServiceDefinition,
    ServiceTlsPolicy,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Mutex, RwLock,
    },
};
use tauri::{
    webview::{NewWindowResponse, WebviewBuilder},
    AppHandle, LogicalPosition, LogicalSize, Manager, Rect, Url, Webview, WebviewUrl,
};

const MAX_SERVICE_ID_LENGTH: usize = 64;
const MAX_SERVICE_NAME_LENGTH: usize = 80;
const MAX_URL_LENGTH: usize = 2_048;
const MAX_CANONICAL_ORIGIN_UTF16_UNITS: usize = 512;
const MAX_WEBVIEW_COORDINATE: f64 = 65_536.0;
const MAX_LIVE_SERVICE_WEBVIEWS: usize = 6;
const MAX_BASIC_AUTH_SUBMISSIONS: u8 = 2;
const MAIN_WEBVIEW_LABEL: &str = "main";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum TlsPolicy {
    #[default]
    Strict,
    AllowInvalidLocalCertificate,
}

impl TlsPolicy {
    fn profile_suffix(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::AllowInvalidLocalCertificate => "local-certificate",
        }
    }
}

#[derive(Clone, Debug)]
struct TrustedService {
    id: String,
    name: String,
    url_text: String,
    url: Url,
    canonical_origin: String,
    tls_policy: TlsPolicy,
    authentication: ServiceAuthentication,
    enabled: bool,
}

pub struct ServiceCatalog {
    services: RwLock<HashMap<String, TrustedService>>,
    revision: AtomicU64,
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedServiceEndpoint {
    pub url: Url,
    pub allow_invalid_local_certificate: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedHealthTarget {
    pub id: String,
    pub name: String,
    pub endpoint: TrustedServiceEndpoint,
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedAuthenticationTarget {
    pub(crate) service_id: String,
    pub(crate) enabled: bool,
    pub(crate) url: Url,
    pub(crate) canonical_origin: String,
    pub(crate) allow_invalid_local_certificate: bool,
    pub(crate) authentication: ServiceAuthentication,
    pub(crate) catalog_revision: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedDownloadProvider {
    pub(crate) name: String,
    pub(crate) target: TrustedAuthenticationTarget,
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedServiceIdentity {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BrowserAuthenticationFingerprint {
    method: ServiceBrowserAuthentication,
    allow_insecure_local_http: bool,
}

impl From<ServiceAuthentication> for BrowserAuthenticationFingerprint {
    fn from(authentication: ServiceAuthentication) -> Self {
        Self {
            method: authentication.browser,
            allow_insecure_local_http: authentication.browser
                == ServiceBrowserAuthentication::HttpBasic
                && authentication.allow_insecure_local_http,
        }
    }
}

impl ServiceCatalog {
    #[cfg(test)]
    pub fn from_bundled_config() -> Result<Self, String> {
        let configuration = ServiceConfiguration::parse_document(BUNDLED_SERVICE_CONFIG)
            .map_err(|error| format!("services.json is invalid: {error}"))?;
        Self::from_configuration(&configuration)
    }

    pub fn from_configuration(configuration: &ServiceConfiguration) -> Result<Self, String> {
        Ok(Self {
            services: RwLock::new(Self::trusted_services(configuration)?),
            revision: AtomicU64::new(1),
        })
    }

    pub fn replace_configuration(
        &self,
        configuration: &ServiceConfiguration,
    ) -> Result<(), String> {
        let services = Self::trusted_services(configuration)?;
        let mut current = self
            .services
            .write()
            .map_err(|_| "The trusted service catalog is unavailable.".to_string())?;
        *current = services;
        let _ = self
            .revision
            .fetch_update(Ordering::Release, Ordering::Relaxed, |revision| {
                Some(revision.saturating_add(1))
            });
        Ok(())
    }

    fn trusted_services(
        configuration: &ServiceConfiguration,
    ) -> Result<HashMap<String, TrustedService>, String> {
        let configuration = configuration.normalized()?;
        let mut services = HashMap::with_capacity(configuration.services().len());

        for configured in configuration.services() {
            let trusted = TrustedService::try_from(configured)?;
            let service_id = trusted.id.clone();

            if services.insert(service_id.clone(), trusted).is_some() {
                return Err(format!(
                    "services.json contains the duplicate id \"{service_id}\"."
                ));
            }
        }

        Ok(services)
    }

    fn resolve_enabled(&self, service_id: &str) -> Result<TrustedService, String> {
        let services = self
            .services
            .read()
            .map_err(|_| "The trusted service catalog is unavailable.".to_string())?;
        let service = services
            .get(service_id)
            .ok_or_else(|| "The requested service is not in the trusted catalog.".to_string())?;

        if !service.enabled {
            return Err("The requested service is disabled.".into());
        }

        Ok(service.clone())
    }

    pub(crate) fn resolve_endpoint(
        &self,
        service_id: &str,
    ) -> Result<TrustedServiceEndpoint, String> {
        let service = self.resolve_enabled(service_id)?;

        Ok(TrustedServiceEndpoint {
            url: service.url.clone(),
            allow_invalid_local_certificate: service.tls_policy
                == TlsPolicy::AllowInvalidLocalCertificate,
        })
    }

    pub(crate) fn resolve_authentication_target(
        &self,
        service_id: &str,
    ) -> Result<TrustedAuthenticationTarget, String> {
        let services = self
            .services
            .read()
            .map_err(|_| "The trusted service catalog is unavailable.".to_string())?;
        let service = services
            .get(service_id)
            .ok_or_else(|| "The requested service is not in the trusted catalog.".to_string())?;

        Ok(TrustedAuthenticationTarget {
            service_id: service.id.clone(),
            enabled: service.enabled,
            url: service.url.clone(),
            canonical_origin: service.canonical_origin.clone(),
            allow_invalid_local_certificate: service.tls_policy
                == TlsPolicy::AllowInvalidLocalCertificate,
            authentication: service.authentication,
            // The read guard prevents replacement until this target and its
            // revision have been captured as one coherent catalog snapshot.
            catalog_revision: self.revision.load(Ordering::Acquire),
        })
    }

    pub(crate) fn find_download_provider(
        &self,
        adapter: crate::service_settings::ServiceApiAuthentication,
    ) -> Result<Option<TrustedDownloadProvider>, String> {
        let services = self
            .services
            .read()
            .map_err(|_| "The trusted service catalog is unavailable.".to_string())?;
        let Some(service) = services
            .values()
            .filter(|service| service.authentication.api == adapter)
            .min_by(|left, right| {
                // An enabled provider wins over a disabled duplicate. The ID
                // tie-break keeps selection deterministic across HashMap runs.
                (!left.enabled, left.id.as_str()).cmp(&(!right.enabled, right.id.as_str()))
            })
        else {
            return Ok(None);
        };

        Ok(Some(TrustedDownloadProvider {
            name: service.name.clone(),
            target: TrustedAuthenticationTarget {
                service_id: service.id.clone(),
                enabled: service.enabled,
                url: service.url.clone(),
                canonical_origin: service.canonical_origin.clone(),
                allow_invalid_local_certificate: service.tls_policy
                    == TlsPolicy::AllowInvalidLocalCertificate,
                authentication: service.authentication,
                catalog_revision: self.revision.load(Ordering::Acquire),
            },
        }))
    }

    pub(crate) fn find_enabled_service_identity(
        &self,
        conventional_id: &str,
        conventional_name: &str,
    ) -> Result<Option<TrustedServiceIdentity>, String> {
        let services = self
            .services
            .read()
            .map_err(|_| "The trusted service catalog is unavailable.".to_string())?;
        let service = services
            .values()
            .filter(|service| {
                service.enabled
                    && (service.id == conventional_id
                        || service.name.eq_ignore_ascii_case(conventional_name))
            })
            .min_by(|left, right| {
                (left.id != conventional_id, left.id.as_str())
                    .cmp(&(right.id != conventional_id, right.id.as_str()))
            });

        Ok(service.map(|service| TrustedServiceIdentity {
            id: service.id.clone(),
            name: service.name.clone(),
        }))
    }

    pub(crate) fn canonical_origin_for_service(&self, service_id: &str) -> Result<String, String> {
        self.services
            .read()
            .map_err(|_| "The trusted service catalog is unavailable.".to_string())?
            .get(service_id)
            .map(|service| service.canonical_origin.clone())
            .ok_or_else(|| "The requested service is not in the trusted catalog.".to_string())
    }

    pub(crate) fn has_enabled_service(&self, service_id: &str) -> bool {
        self.services
            .read()
            .ok()
            .and_then(|services| services.get(service_id).map(|service| service.enabled))
            .unwrap_or(false)
    }

    pub(crate) fn contains_service(&self, service_id: &str) -> bool {
        self.services
            .read()
            .ok()
            .is_some_and(|services| services.contains_key(service_id))
    }

    pub(crate) fn enabled_health_targets(&self) -> Vec<TrustedHealthTarget> {
        let Ok(services) = self.services.read() else {
            return Vec::new();
        };
        let mut targets = services
            .values()
            .filter(|service| service.enabled)
            .map(|service| TrustedHealthTarget {
                id: service.id.clone(),
                name: service.name.clone(),
                endpoint: TrustedServiceEndpoint {
                    url: service.url.clone(),
                    allow_invalid_local_certificate: service.tls_policy
                        == TlsPolicy::AllowInvalidLocalCertificate,
                },
            })
            .collect::<Vec<_>>();

        targets.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        targets
    }

    fn matches_registered_configuration(
        &self,
        service_id: &str,
        url: &str,
        tls_policy: TlsPolicy,
        browser_authentication: BrowserAuthenticationFingerprint,
    ) -> bool {
        self.services.read().ok().is_some_and(|services| {
            services.get(service_id).is_some_and(|service| {
                service.enabled
                    && service.url_text == url
                    && service.tls_policy == tls_policy
                    && BrowserAuthenticationFingerprint::from(service.authentication)
                        == browser_authentication
            })
        })
    }
}

impl TryFrom<&ServiceDefinition> for TrustedService {
    type Error = String;

    fn try_from(service: &ServiceDefinition) -> Result<Self, Self::Error> {
        if !is_valid_service_id(&service.id) {
            return Err(format!("Service id \"{}\" is invalid.", service.id));
        }

        if service.name.trim().is_empty()
            || service.name.trim() != service.name
            || service.name.chars().count() > MAX_SERVICE_NAME_LENGTH
        {
            return Err(format!("Service name for \"{}\" is invalid.", service.id));
        }

        if service.url.trim() != service.url {
            return Err(format!(
                "Service URL for \"{}\" must not contain edge whitespace.",
                service.id
            ));
        }

        let url = validate_http_url(&service.url)?;
        let canonical_origin = canonical_origin(&url)?;

        if service.tls_policy == ServiceTlsPolicy::AllowInvalidLocalCertificate {
            validate_local_tls_exception(&url)?;
        }

        Ok(Self {
            id: service.id.clone(),
            name: service.name.clone(),
            url_text: service.url.clone(),
            url,
            canonical_origin,
            tls_policy: service.tls_policy.into(),
            authentication: service.authentication,
            enabled: service.enabled,
        })
    }
}

impl From<ServiceTlsPolicy> for TlsPolicy {
    fn from(policy: ServiceTlsPolicy) -> Self {
        match policy {
            ServiceTlsPolicy::Strict => Self::Strict,
            ServiceTlsPolicy::AllowInvalidLocalCertificate => Self::AllowInvalidLocalCertificate,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl ChildBounds {
    fn validate(self) -> Result<Self, String> {
        let values = [self.x, self.y, self.width, self.height];

        if values.iter().any(|value| !value.is_finite()) {
            return Err("The service webview bounds must be finite numbers.".into());
        }

        if self.x < 0.0 || self.y < 0.0 || self.width < 1.0 || self.height < 1.0 {
            return Err("The service webview bounds must be inside the main window.".into());
        }

        if values.iter().any(|value| *value > MAX_WEBVIEW_COORDINATE) {
            return Err("The service webview bounds are outside the supported range.".into());
        }

        Ok(self)
    }

    fn position(self) -> LogicalPosition<f64> {
        LogicalPosition::new(self.x, self.y)
    }

    fn size(self) -> LogicalSize<f64> {
        LogicalSize::new(self.width, self.height)
    }

    fn rect(self) -> Rect {
        Rect {
            position: self.position().into(),
            size: self.size().into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenServiceWebviewRequest {
    service_id: String,
    bounds: ChildBounds,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivateServiceWebviewRequest {
    service_id: String,
    bounds: ChildBounds,
    #[serde(default)]
    focus: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemBrowserRequest {
    service_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconcileServiceWebviewsRequest {
    enabled_service_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenServiceWebviewResult {
    created: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileServiceWebviewsResult {
    registered_service_ids: Vec<String>,
    message: Option<String>,
}

#[derive(Clone, Debug)]
struct RegistryEntry {
    label: String,
    url: String,
    tls_policy: TlsPolicy,
    browser_authentication: BrowserAuthenticationFingerprint,
    ready: bool,
    attached_to_tab: bool,
    last_used: u64,
}

#[derive(Default)]
struct RegistryInner {
    entries: HashMap<String, RegistryEntry>,
    active_service_id: Option<String>,
    use_sequence: u64,
    /// Prevents a hidden main window from recreating service renderers after
    /// the native close-to-tray boundary has released them.
    background_suspended: bool,
}

#[derive(Default)]
pub struct ServiceWebviewRegistry {
    inner: Mutex<RegistryInner>,
}

pub(crate) fn suspend_and_close_all_service_webviews(
    app: &AppHandle,
    registry: &ServiceWebviewRegistry,
) -> Result<(), String> {
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    inner.background_suspended = true;
    inner.active_service_id = None;

    let entries = inner
        .entries
        .iter()
        .map(|(service_id, entry)| (service_id.clone(), entry.label.clone()))
        .collect::<Vec<_>>();
    let mut failures = Vec::new();

    for (service_id, label) in entries {
        let close_result: Result<(), String> = app.get_webview(&label).map_or(Ok(()), |view| {
            match view.close() {
                Ok(()) => Ok(()),
                Err(first_error) => match view.close() {
                    Ok(()) => Ok(()),
                    Err(retry_error) => {
                        // A failed view is surfaced instead of hidden: hiding
                        // it could recreate the exact invisible-audio failure
                        // this boundary is designed to prevent.
                        let _ = view.show();
                        Err(format!(
                            "Could not close a service webview before backgrounding. First attempt: {first_error}. Retry: {retry_error}"
                        ))
                    }
                },
            }
        });

        if let Err(error) = finalize_registry_close(&mut inner, &service_id, close_result) {
            set_entry_attached(&mut inner, &service_id, true);
            inner.active_service_id = Some(service_id);
            failures.push(error);
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Personal Hub stayed open because {} service {} could not be released: {}",
            failures.len(),
            if failures.len() == 1 { "view" } else { "views" },
            failures.join(" ")
        ))
    }
}

pub(crate) fn resume_service_webviews(registry: &ServiceWebviewRegistry) -> Result<(), String> {
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    inner.background_suspended = false;
    Ok(())
}

fn ensure_service_webviews_active(inner: &RegistryInner) -> Result<(), String> {
    if inner.background_suspended {
        Err("Service views are paused while Personal Hub is running in the background.".into())
    } else {
        Ok(())
    }
}

/// Runs a browser-credential mutation while the service registry remains
/// exclusively locked after revoking the old view. An in-flight open uses the
/// same lock, so it cannot recreate a session with the old credential between
/// revocation and the vault write/delete.
pub(crate) fn with_service_webview_revoked<T>(
    app: &AppHandle,
    registry: &ServiceWebviewRegistry,
    service_id: &str,
    mutation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if !is_valid_service_id(service_id) {
        return Err("The service id is invalid.".into());
    }

    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    revoke_service_webview_locked(app, &mut inner, service_id)?;
    mutation()
}

fn revoke_service_webview_locked(
    app: &AppHandle,
    inner: &mut RegistryInner,
    service_id: &str,
) -> Result<bool, String> {
    let registered_entry = inner.entries.get(service_id).cloned();
    let label = registered_entry
        .as_ref()
        .map(|entry| entry.label.clone())
        .unwrap_or_else(|| service_webview_label(service_id));
    let Some(view) = app.get_webview(&label) else {
        if registered_entry.is_some() {
            remove_registry_entry(inner, service_id);
            return Ok(true);
        }
        return Ok(false);
    };

    if let Err(error) = view.close() {
        // Never leave a view that may hold rotated credentials visible. Keep
        // the entry for an explicit cleanup retry if native close failed.
        let _ = view.hide();
        set_entry_attached(inner, service_id, false);
        if inner.active_service_id.as_deref() == Some(service_id) {
            inner.active_service_id = None;
        }
        return Err(webview_error(
            "close the credential-revoked service webview",
        )(error));
    }

    remove_registry_entry(inner, service_id);
    Ok(true)
}

/// Releases native views whose registered endpoint is no longer trusted by the
/// current catalog.  Call this immediately after replacing the catalog, while
/// the settings operation is still serialized.
///
/// The registry lock also serializes this work with an in-flight open. An open
/// which resolved its service before a catalog replacement re-resolves it once
/// it owns this same lock, so it cannot recreate an endpoint this function has
/// just revoked.
pub(crate) fn revoke_stale_service_webviews(
    app: &AppHandle,
    catalog: &ServiceCatalog,
    registry: &ServiceWebviewRegistry,
) -> Result<usize, String> {
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    let stale_service_ids = stale_catalog_entry_ids(&inner, catalog);
    let mut released_count = 0_usize;
    let mut failures = Vec::new();

    for service_id in stale_service_ids {
        let Some(entry) = inner.entries.get(&service_id).cloned() else {
            continue;
        };

        let close_result = match app.get_webview(&entry.label) {
            Some(view) => match view.close() {
                Ok(()) => Ok(()),
                Err(error) => {
                    // A failed close must not leave a revoked view visible.
                    // Keep its registry record for an explicit later retry.
                    let _ = view.hide();
                    Err(webview_error("close the revoked service webview")(error))
                }
            },
            None => Ok(()),
        };

        match finalize_registry_close(&mut inner, &service_id, close_result) {
            Ok(()) => released_count += 1,
            Err(error) => {
                // `close` failed, so retain the entry for cleanup retry but
                // detach it from the active tab after the best-effort hide.
                set_entry_attached(&mut inner, &service_id, false);
                if inner.active_service_id.as_deref() == Some(service_id.as_str()) {
                    inner.active_service_id = None;
                }
                failures.push(error);
            }
        }
    }

    if failures.is_empty() {
        Ok(released_count)
    } else {
        Err(format!(
            "Could not release {} revoked native service {}: {}",
            failures.len(),
            if failures.len() == 1 { "view" } else { "views" },
            failures.join(" ")
        ))
    }
}

#[tauri::command]
pub async fn open_service_webview(
    caller: Webview,
    app: AppHandle,
    request: OpenServiceWebviewRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
    authentication: tauri::State<'_, ProviderAuthManager>,
) -> Result<OpenServiceWebviewResult, String> {
    ensure_trusted_caller(&caller)?;
    let bounds = request.bounds.validate()?;
    // Keep the inexpensive early validation, then resolve again once opening
    // owns the registry sequence below. The second resolution prevents an old
    // pre-commit catalog snapshot from creating a view after revocation.
    catalog.resolve_enabled(&request.service_id)?;
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    ensure_service_webviews_active(&inner)?;
    let service = catalog.resolve_enabled(&request.service_id)?;
    let authentication_target = catalog.resolve_authentication_target(&request.service_id)?;
    if !authentication_target.enabled
        || authentication_target.url != service.url
        || authentication_target.canonical_origin != service.canonical_origin
        || authentication_target.authentication != service.authentication
        || authentication_target.allow_invalid_local_certificate
            != (service.tls_policy == TlsPolicy::AllowInvalidLocalCertificate)
    {
        return Err(
            "The service configuration changed while its webview was being opened; try again."
                .into(),
        );
    }
    let label = service_webview_label(&service.id);
    let browser_authentication = BrowserAuthenticationFingerprint::from(service.authentication);

    hide_registered_webviews(&app, &inner, Some(&service.id))?;
    inner.active_service_id = None;

    if let Some(entry) = inner.entries.get(&service.id).cloned() {
        let configuration_matches = entry.url == service.url_text
            && entry.tls_policy == service.tls_policy
            && entry.browser_authentication == browser_authentication;

        if configuration_matches && entry.ready {
            if let Some(existing) = app.get_webview(&entry.label) {
                let activation_result = (|| {
                    if service.tls_policy == TlsPolicy::AllowInvalidLocalCertificate {
                        clear_cached_certificate_decisions(&existing)?;
                    }

                    existing
                        .set_bounds(bounds.rect())
                        .map_err(webview_error("resize the existing service webview"))?;
                    existing
                        .show()
                        .map_err(webview_error("show the existing service webview"))?;
                    existing
                        .set_focus()
                        .map_err(webview_error("focus the existing service webview"))?;
                    Ok::<(), String>(())
                })();

                if let Err(error) = activation_result {
                    let _ = existing.hide();
                    return Err(error);
                }

                mark_entry_attached(&mut inner, &service.id, true);
                inner.active_service_id = Some(service.id);
                return Ok(OpenServiceWebviewResult { created: false });
            }
        }

        if let Some(stale) = app.get_webview(&entry.label) {
            stale
                .close()
                .map_err(webview_error("close the stale service webview"))?;
        }

        inner.entries.remove(&service.id);
    } else if let Some(orphaned) = app.get_webview(&label) {
        if let Err(error) = orphaned.close() {
            let _ = orphaned.hide();
            let last_used = next_registry_sequence(&mut inner);
            inner.entries.insert(
                service.id.clone(),
                RegistryEntry {
                    label,
                    url: service.url_text,
                    tls_policy: service.tls_policy,
                    browser_authentication,
                    ready: false,
                    attached_to_tab: false,
                    last_used,
                },
            );
            return Err(webview_error("close the orphaned service webview")(error));
        }
    }

    let profile_directory = service_profile_directory(&app, &service)?;
    std::fs::create_dir_all(&profile_directory)
        .map_err(|error| format!("The service webview profile could not be created: {error}"))?;

    let allowed_origin = AllowedOrigin::from_url(&service.url)?;
    let navigation_origin = allowed_origin.clone();
    let blank_url = Url::parse("about:blank")
        .map_err(|error| format!("The blank webview URL could not be parsed: {error}"))?;

    let builder = WebviewBuilder::new(label.clone(), WebviewUrl::External(blank_url))
        .data_directory(profile_directory)
        .focused(false)
        .devtools(false)
        .on_navigation(move |next| {
            next.as_str() == "about:blank" || navigation_origin.matches(next)
        })
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_download(|_, _| false);

    // Prepare and validate everything that cannot mutate the native view tree
    // before releasing an older warm session to make room.
    trim_service_webview_pool(&app, &mut inner, Some(&service.id))?;

    let window = caller.window();
    let child = window
        .add_child(builder, bounds.position(), bounds.size())
        .map_err(webview_error("create the service webview"))?;

    let last_used = next_registry_sequence(&mut inner);
    inner.entries.insert(
        service.id.clone(),
        RegistryEntry {
            label,
            url: service.url_text,
            tls_policy: service.tls_policy,
            browser_authentication,
            ready: false,
            attached_to_tab: true,
            last_used,
        },
    );

    let setup_result = (|| {
        if browser_authentication.method == ServiceBrowserAuthentication::HttpBasic {
            install_basic_authentication_handler(
                &child,
                authentication_target,
                authentication.inner().clone(),
            )?;
        }

        if service.tls_policy == TlsPolicy::AllowInvalidLocalCertificate {
            install_local_certificate_exception(&child, allowed_origin)?;
            clear_cached_certificate_decisions(&child)?;
        }

        child
            .navigate(service.url.clone())
            .map_err(|error| format!("Could not navigate the service webview: {error}"))?;
        child
            .set_bounds(bounds.rect())
            .map_err(webview_error("set the service webview bounds"))?;
        child
            .show()
            .map_err(webview_error("show the service webview"))?;
        child
            .set_focus()
            .map_err(webview_error("focus the service webview"))?;
        Ok::<(), String>(())
    })();

    if let Err(error) = setup_result {
        inner.active_service_id = None;

        match child.close() {
            Ok(()) => {
                inner.entries.remove(&service.id);
                return Err(error);
            }
            Err(cleanup_error) => {
                let _ = child.hide();
                set_entry_attached(&mut inner, &service.id, false);
                return Err(format!(
                    "{error} Cleanup also failed; the service view remains registered for a later retry: {cleanup_error}"
                ));
            }
        }
    }

    if let Some(entry) = inner.entries.get_mut(&service.id) {
        entry.ready = true;
    }
    mark_entry_attached(&mut inner, &service.id, true);
    inner.active_service_id = Some(service.id);

    Ok(OpenServiceWebviewResult { created: true })
}

#[tauri::command]
pub async fn activate_service_webview(
    caller: Webview,
    app: AppHandle,
    request: ActivateServiceWebviewRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
) -> Result<(), String> {
    ensure_trusted_caller(&caller)?;
    catalog.resolve_enabled(&request.service_id)?;
    let bounds = request.bounds.validate()?;
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    ensure_service_webviews_active(&inner)?;
    let entry = inner
        .entries
        .get(&request.service_id)
        .cloned()
        .ok_or_else(|| {
            "The service webview must be opened before it can be activated.".to_string()
        })?;

    if !catalog.matches_registered_configuration(
        &request.service_id,
        &entry.url,
        entry.tls_policy,
        entry.browser_authentication,
    ) {
        return Err(
            "The service configuration changed; the service webview must be reopened.".into(),
        );
    }

    if !entry.ready {
        return Err("The service webview did not finish opening and must be recreated.".into());
    }

    let Some(view) = app.get_webview(&entry.label) else {
        inner.entries.remove(&request.service_id);
        if inner.active_service_id.as_deref() == Some(request.service_id.as_str()) {
            inner.active_service_id = None;
        }
        return Err("The service webview is no longer available and must be reopened.".into());
    };

    let is_geometry_update =
        !request.focus && inner.active_service_id.as_deref() == Some(request.service_id.as_str());
    if is_geometry_update {
        view.set_bounds(bounds.rect())
            .map_err(webview_error("resize the service webview"))?;
        return Ok(());
    }

    hide_registered_webviews(&app, &inner, Some(&request.service_id))?;
    inner.active_service_id = None;

    if entry.tls_policy == TlsPolicy::AllowInvalidLocalCertificate {
        clear_cached_certificate_decisions(&view)?;
    }

    view.set_bounds(bounds.rect())
        .map_err(webview_error("resize the service webview"))?;
    view.show()
        .map_err(webview_error("show the service webview"))?;

    if request.focus {
        view.set_focus()
            .map_err(webview_error("focus the service webview"))?;
    }

    mark_entry_attached(&mut inner, &request.service_id, true);
    inner.active_service_id = Some(request.service_id);
    Ok(())
}

#[tauri::command]
pub async fn hide_service_webviews(
    caller: Webview,
    app: AppHandle,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
) -> Result<(), String> {
    ensure_trusted_caller(&caller)?;
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    hide_registered_webviews(&app, &inner, None)?;
    inner.active_service_id = None;
    Ok(())
}

#[tauri::command]
pub async fn reconcile_service_webviews(
    caller: Webview,
    app: AppHandle,
    request: ReconcileServiceWebviewsRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
) -> Result<ReconcileServiceWebviewsResult, String> {
    ensure_trusted_caller(&caller)?;
    let enabled_service_ids = validate_service_id_set(request.enabled_service_ids)?;
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;
    let entries = inner
        .entries
        .iter()
        .map(|(service_id, entry)| (service_id.clone(), entry.clone()))
        .collect::<Vec<_>>();
    let mut failed_close_count = 0_usize;

    for (service_id, entry) in entries {
        let Some(view) = app.get_webview(&entry.label) else {
            finalize_registry_close(&mut inner, &service_id, Ok(()))?;
            continue;
        };

        if entry.ready
            && enabled_service_ids.contains(&service_id)
            && catalog.matches_registered_configuration(
                &service_id,
                &entry.url,
                entry.tls_policy,
                entry.browser_authentication,
            )
        {
            continue;
        }

        let close_result = view
            .close()
            .map_err(webview_error("close a reconciled service webview"));
        if finalize_registry_close(&mut inner, &service_id, close_result).is_err() {
            failed_close_count += 1;
        }
    }

    if inner
        .active_service_id
        .as_ref()
        .is_some_and(|service_id| !inner.entries.contains_key(service_id))
    {
        inner.active_service_id = None;
    }

    let mut registered_service_ids = inner.entries.keys().cloned().collect::<Vec<_>>();
    registered_service_ids.sort();

    Ok(ReconcileServiceWebviewsResult {
        registered_service_ids,
        message: (failed_close_count > 0).then(|| {
            format!(
                "Could not release {failed_close_count} native service {}.",
                if failed_close_count == 1 {
                    "view"
                } else {
                    "views"
                }
            )
        }),
    })
}

#[tauri::command]
pub async fn close_service_webview(
    caller: Webview,
    app: AppHandle,
    service_id: String,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
) -> Result<(), String> {
    ensure_trusted_caller(&caller)?;

    if !is_valid_service_id(&service_id) {
        return Err("The service id is invalid.".into());
    }

    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;

    let close_result = inner
        .entries
        .get(&service_id)
        .and_then(|entry| app.get_webview(&entry.label))
        .map_or(Ok(()), |view| {
            view.close()
                .map_err(webview_error("close the service webview"))
        });

    finalize_registry_close(&mut inner, &service_id, close_result)
}

#[tauri::command]
pub async fn open_service_in_system_browser(
    caller: Webview,
    request: SystemBrowserRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
) -> Result<(), String> {
    ensure_trusted_caller(&caller)?;
    let service = catalog.resolve_enabled(&request.service_id)?.clone();

    #[cfg(windows)]
    {
        Command::new("explorer.exe")
            .arg(service.url.as_str())
            .spawn()
            .map_err(|error| format!("The system browser could not be opened: {error}"))?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = service;
        Err("Opening the system browser is currently supported on Windows only.".into())
    }
}

fn hide_registered_webviews(
    app: &AppHandle,
    registry: &RegistryInner,
    except_service_id: Option<&str>,
) -> Result<(), String> {
    for (service_id, entry) in &registry.entries {
        if except_service_id == Some(service_id.as_str()) {
            continue;
        }

        if let Some(view) = app.get_webview(&entry.label) {
            view.hide()
                .map_err(webview_error("hide an inactive service webview"))?;
        }
    }

    Ok(())
}

fn next_registry_sequence(registry: &mut RegistryInner) -> u64 {
    registry.use_sequence = registry.use_sequence.saturating_add(1);
    registry.use_sequence
}

fn mark_entry_attached(registry: &mut RegistryInner, service_id: &str, attached: bool) {
    let last_used = next_registry_sequence(registry);

    if let Some(entry) = registry.entries.get_mut(service_id) {
        entry.attached_to_tab = attached;
        entry.last_used = last_used;
    }
}

fn set_entry_attached(registry: &mut RegistryInner, service_id: &str, attached: bool) {
    if let Some(entry) = registry.entries.get_mut(service_id) {
        entry.attached_to_tab = attached;
    }
}

fn remove_registry_entry(registry: &mut RegistryInner, service_id: &str) {
    registry.entries.remove(service_id);

    if registry.active_service_id.as_deref() == Some(service_id) {
        registry.active_service_id = None;
    }
}

fn finalize_registry_close(
    registry: &mut RegistryInner,
    service_id: &str,
    close_result: Result<(), String>,
) -> Result<(), String> {
    close_result?;
    remove_registry_entry(registry, service_id);
    Ok(())
}

fn stale_catalog_entry_ids(registry: &RegistryInner, catalog: &ServiceCatalog) -> Vec<String> {
    let mut service_ids = registry
        .entries
        .iter()
        .filter(|(service_id, entry)| {
            !catalog.matches_registered_configuration(
                service_id,
                &entry.url,
                entry.tls_policy,
                entry.browser_authentication,
            )
        })
        .map(|(service_id, _)| service_id.clone())
        .collect::<Vec<_>>();
    service_ids.sort();
    service_ids
}

fn pool_eviction_candidates(
    registry: &RegistryInner,
    reserved_service_id: Option<&str>,
) -> Vec<String> {
    let mut candidates = registry
        .entries
        .iter()
        .filter(|(service_id, _)| {
            registry.active_service_id.as_deref() != Some(service_id.as_str())
                && reserved_service_id != Some(service_id.as_str())
        })
        .map(|(service_id, entry)| (service_id.clone(), entry.attached_to_tab, entry.last_used))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, attached_to_tab, last_used)| (*attached_to_tab, *last_used));
    candidates
        .into_iter()
        .map(|(service_id, _, _)| service_id)
        .collect()
}

fn trim_service_webview_pool(
    app: &AppHandle,
    registry: &mut RegistryInner,
    reserved_service_id: Option<&str>,
) -> Result<(), String> {
    let maximum_existing =
        MAX_LIVE_SERVICE_WEBVIEWS.saturating_sub(usize::from(reserved_service_id.is_some()));

    while registry.entries.len() > maximum_existing {
        let mut evicted = false;

        for service_id in pool_eviction_candidates(registry, reserved_service_id) {
            let Some(entry) = registry.entries.get(&service_id).cloned() else {
                continue;
            };

            let closed = app
                .get_webview(&entry.label)
                .is_none_or(|view| view.close().is_ok());

            if closed {
                registry.entries.remove(&service_id);
                evicted = true;
                break;
            }
        }

        if !evicted {
            return Err(format!(
                "The service view pool is full ({MAX_LIVE_SERVICE_WEBVIEWS}) and no inactive view could be released."
            ));
        }
    }

    Ok(())
}

fn ensure_trusted_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() != MAIN_WEBVIEW_LABEL {
        return Err("This command is available only to the trusted Personal Hub UI.".into());
    }

    Ok(())
}

fn service_webview_label(service_id: &str) -> String {
    format!("service-{service_id}")
}

fn service_profile_directory(app: &AppHandle, service: &TrustedService) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("The local app-data directory is unavailable: {error}"))?;

    Ok(root.join("service-webviews").join(format!(
        "{}-{}",
        service.id,
        service.tls_policy.profile_suffix()
    )))
}

fn validate_http_url(value: &str) -> Result<Url, String> {
    if value.is_empty() || value.chars().count() > MAX_URL_LENGTH {
        return Err("The service URL length is invalid.".into());
    }

    let url = Url::parse(value).map_err(|_| "The service URL is invalid.".to_string())?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err("Service webviews only support HTTP and HTTPS URLs.".into());
    }

    if url.host_str().is_none() {
        return Err("The service URL must include a hostname.".into());
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err("The service URL must not contain embedded credentials.".into());
    }

    Ok(url)
}

fn canonical_origin(url: &Url) -> Result<String, String> {
    let raw_host = url
        .host_str()
        .ok_or_else(|| "The service URL must include a hostname.".to_string())?;
    let host = raw_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err("The service URL hostname is invalid.".into());
    }

    let rendered_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "The service URL port is invalid.".to_string())?;
    let origin = format!(
        "{}://{rendered_host}:{port}",
        url.scheme().to_ascii_lowercase()
    );

    if origin.encode_utf16().count() > MAX_CANONICAL_ORIGIN_UTF16_UNITS {
        return Err("The service authentication origin is too long.".into());
    }

    Ok(origin)
}

fn validate_local_tls_exception(url: &Url) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err("A local certificate exception requires an HTTPS URL.".into());
    }

    if !is_local_target(url) {
        return Err(
            "A local certificate exception is limited to loopback and private-network targets."
                .into(),
        );
    }

    Ok(())
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

fn validate_service_id_set(service_ids: Vec<String>) -> Result<HashSet<String>, String> {
    if service_ids.len() > 500 {
        return Err("The enabled service list is too large.".into());
    }

    let mut validated = HashSet::with_capacity(service_ids.len());

    for service_id in service_ids {
        if !is_valid_service_id(&service_id) {
            return Err("The enabled service list contains an invalid id.".into());
        }

        if !validated.insert(service_id) {
            return Err("The enabled service list contains a duplicate id.".into());
        }
    }

    Ok(validated)
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct AllowedOrigin {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl AllowedOrigin {
    fn from_url(url: &Url) -> Result<Self, String> {
        let host = url
            .host_str()
            .ok_or_else(|| "The service URL must include a hostname.".to_string())?;

        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host.trim_end_matches('.').to_ascii_lowercase(),
            port: url.port_or_known_default(),
        })
    }

    fn matches(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };

        url.username().is_empty()
            && url.password().is_none()
            && url.scheme().eq_ignore_ascii_case(&self.scheme)
            && host.trim_end_matches('.').eq_ignore_ascii_case(&self.host)
            && url.port_or_known_default() == self.port
    }
}

fn webview_error(action: &'static str) -> impl FnOnce(tauri::Error) -> String {
    move |error| format!("Could not {action}: {error}")
}

fn reserve_basic_auth_submission(submissions: &AtomicU8) -> bool {
    submissions
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |attempts| {
            (attempts < MAX_BASIC_AUTH_SUBMISSIONS).then_some(attempts + 1)
        })
        .is_ok()
}

#[cfg(windows)]
fn install_basic_authentication_handler(
    webview: &Webview,
    target: TrustedAuthenticationTarget,
    authentication: ProviderAuthManager,
) -> Result<(), String> {
    use std::{sync::mpsc, time::Duration};

    let (sender, receiver) = mpsc::sync_channel(1);
    webview
        .with_webview(move |platform_webview| {
            let result =
                register_basic_authentication_handler(platform_webview, target, authentication);
            let _ = sender.send(result);
        })
        .map_err(webview_error("access the native WebView2 controller"))?;

    receiver
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "Timed out while configuring HTTP Basic authentication.".to_string())?
}

#[cfg(not(windows))]
fn install_basic_authentication_handler(
    _webview: &Webview,
    _target: TrustedAuthenticationTarget,
    _authentication: ProviderAuthManager,
) -> Result<(), String> {
    Err("HTTP Basic service-webview authentication is supported on Windows only.".into())
}

#[cfg(windows)]
fn register_basic_authentication_handler(
    platform_webview: tauri::webview::PlatformWebview,
    target: TrustedAuthenticationTarget,
    authentication: ProviderAuthManager,
) -> Result<(), String> {
    use webview2_com::{
        take_pwstr, BasicAuthenticationRequestedEventHandler,
        Microsoft::Web::WebView2::Win32::ICoreWebView2_10,
    };
    use windows::core::{Interface, PCWSTR, PWSTR};

    let allowed_origin = AllowedOrigin::from_url(&target.url)?;
    let submissions = AtomicU8::new(0);
    let controller = platform_webview.controller();
    let core = unsafe { controller.CoreWebView2() }
        .map_err(|error| format!("WebView2 core is unavailable: {error}"))?;
    let core_10: ICoreWebView2_10 = core.cast().map_err(|error| {
        format!("This WebView2 runtime lacks HTTP Basic authentication controls: {error}")
    })?;
    let mut token = 0_i64;

    let handler =
        BasicAuthenticationRequestedEventHandler::create(Box::new(move |_sender, arguments| {
            let Some(arguments) = arguments else {
                return Ok(());
            };

            // Every challenge starts cancelled. Only an exact-origin request,
            // an origin-bound credential and a remaining submission budget can
            // turn cancellation off.
            unsafe {
                arguments.SetCancel(true)?;
            }

            let mut request_uri = PWSTR::null();
            unsafe {
                arguments.Uri(&mut request_uri)?;
            }
            let request_uri = take_pwstr(request_uri);
            let Ok(request_url) = Url::parse(&request_uri) else {
                return Ok(());
            };
            if !allowed_origin.matches(&request_url) {
                return Ok(());
            }

            let material = match authentication.resolve_browser_authentication(&target) {
                Ok(Some(material)) => material,
                Ok(None) | Err(_) => return Ok(()),
            };
            let (username, password) = match material.basic_credentials_for(&request_url) {
                Ok(credentials) => credentials,
                Err(_) => return Ok(()),
            };
            if username.contains('\0')
                || password.contains('\0')
                || !reserve_basic_auth_submission(&submissions)
            {
                return Ok(());
            }

            let mut username_utf16 = username
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let mut password_utf16 = password
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let set_result = (|| {
                let response = unsafe { arguments.Response()? };
                unsafe {
                    response.SetUserName(PCWSTR::from_raw(username_utf16.as_ptr()))?;
                    response.SetPassword(PCWSTR::from_raw(password_utf16.as_ptr()))?;
                }
                Ok::<(), windows::core::Error>(())
            })();
            username_utf16.fill(0);
            password_utf16.fill(0);
            set_result?;

            unsafe {
                arguments.SetCancel(false)?;
            }
            Ok(())
        }));

    unsafe {
        core_10
            .add_BasicAuthenticationRequested(&handler, &mut token)
            .map_err(|error| {
                format!(
                    "The WebView2 HTTP Basic authentication policy could not be installed: {error}"
                )
            })?;
    }

    Ok(())
}

#[cfg(windows)]
fn install_local_certificate_exception(
    webview: &Webview,
    allowed_origin: AllowedOrigin,
) -> Result<(), String> {
    use std::{sync::mpsc, time::Duration};

    let (sender, receiver) = mpsc::sync_channel(1);
    webview
        .with_webview(move |platform_webview| {
            let result = register_certificate_handler(platform_webview, allowed_origin);
            let _ = sender.send(result);
        })
        .map_err(webview_error("access the native WebView2 controller"))?;

    receiver
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "Timed out while configuring the local certificate policy.".to_string())?
}

#[cfg(not(windows))]
fn install_local_certificate_exception(
    _webview: &Webview,
    _allowed_origin: AllowedOrigin,
) -> Result<(), String> {
    Err("Local certificate exceptions are currently supported on Windows only.".into())
}

#[cfg(windows)]
fn clear_cached_certificate_decisions(webview: &Webview) -> Result<(), String> {
    use std::{sync::mpsc, time::Duration};
    use webview2_com::{
        ClearServerCertificateErrorActionsCompletedHandler,
        Microsoft::Web::WebView2::Win32::ICoreWebView2_14,
    };
    use windows::core::Interface;

    let (sender, receiver) = mpsc::sync_channel(1);
    webview
        .with_webview(move |platform_webview| {
            let callback_sender = sender.clone();
            let result = (|| {
                let controller = platform_webview.controller();
                let core = unsafe { controller.CoreWebView2() }
                    .map_err(|error| format!("WebView2 core is unavailable: {error}"))?;
                let core_14: ICoreWebView2_14 = core.cast().map_err(|error| {
                    format!("This WebView2 runtime lacks certificate controls: {error}")
                })?;
                let completed = ClearServerCertificateErrorActionsCompletedHandler::create(
                    Box::new(move |completion_result| {
                        let result = completion_result.map_err(|error| {
                            format!(
                                "The cached WebView2 certificate decisions could not be cleared: {error}"
                            )
                        });
                        let _ = callback_sender.send(result);
                        Ok(())
                    }),
                );

                unsafe {
                    core_14
                        .ClearServerCertificateErrorActions(&completed)
                        .map_err(|error| {
                            format!(
                                "The cached WebView2 certificate decisions could not be cleared: {error}"
                            )
                        })?;
                }

                Ok::<(), String>(())
            })();

            if let Err(error) = result {
                let _ = sender.send(Err(error));
            }
        })
        .map_err(webview_error("access the native WebView2 controller"))?;

    receiver
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| "Timed out while refreshing the local certificate policy.".to_string())?
}

#[cfg(not(windows))]
fn clear_cached_certificate_decisions(_webview: &Webview) -> Result<(), String> {
    Err("Local certificate exceptions are currently supported on Windows only.".into())
}

#[cfg(windows)]
fn register_certificate_handler(
    platform_webview: tauri::webview::PlatformWebview,
    allowed_origin: AllowedOrigin,
) -> Result<(), String> {
    use webview2_com::{
        take_pwstr, Microsoft::Web::WebView2::Win32::*, ServerCertificateErrorDetectedEventHandler,
    };
    use windows::core::{Interface, PWSTR};

    let controller = platform_webview.controller();
    let core = unsafe { controller.CoreWebView2() }
        .map_err(|error| format!("WebView2 core is unavailable: {error}"))?;
    let core_14: ICoreWebView2_14 = core
        .cast()
        .map_err(|error| format!("This WebView2 runtime lacks certificate controls: {error}"))?;
    let mut token = 0_i64;

    let handler =
        ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_sender, arguments| {
            let Some(arguments) = arguments else {
                return Ok(());
            };

            // Fail closed first. Only explicitly configured private HTTPS
            // requests and narrowly accepted local-certificate errors relax it.
            // WebView2 caches an allow by host + certificate for the session;
            // the cache is cleared on creation and every tab activation.
            unsafe {
                arguments.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)?;
            }

            let mut request_uri = PWSTR::null();
            let mut error_status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
            unsafe {
                arguments.RequestUri(&mut request_uri)?;
                arguments.ErrorStatus(&mut error_status)?;
            }

            let request_uri = take_pwstr(request_uri);
            let request_url = Url::parse(&request_uri).ok();
            let allowed_error = matches!(
                error_status,
                COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID
                    | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
            );
            let allowed_request = request_url.as_ref().is_some_and(|url| {
                allowed_origin.matches(url) && url.scheme() == "https" && is_local_target(url)
            });

            if allowed_error && allowed_request {
                unsafe {
                    arguments
                        .SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)?;
                }
            }

            Ok(())
        }));

    unsafe {
        core_14
            .add_ServerCertificateErrorDetected(&handler, &mut token)
            .map_err(|error| {
                format!("The WebView2 certificate policy could not be installed: {error}")
            })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_service(name: &str, url: &str) -> ServiceDefinition {
        let mut configuration =
            ServiceConfiguration::parse_document(BUNDLED_SERVICE_CONFIG).unwrap();
        let mut service = configuration.services.remove(0);
        service.id = "test-service".into();
        service.name = name.into();
        service.url = url.into();
        service.tls_policy = ServiceTlsPolicy::Strict;
        service
    }

    fn registry_entry(attached_to_tab: bool, last_used: u64) -> RegistryEntry {
        RegistryEntry {
            label: format!("service-test-{last_used}"),
            url: "http://192.168.1.10:8080".into(),
            tls_policy: TlsPolicy::Strict,
            browser_authentication: ServiceAuthentication::default().into(),
            ready: true,
            attached_to_tab,
            last_used,
        }
    }

    #[test]
    fn background_suspension_blocks_open_and_activate_paths() {
        let mut registry = RegistryInner::default();
        assert!(ensure_service_webviews_active(&registry).is_ok());

        registry.background_suspended = true;
        assert!(ensure_service_webviews_active(&registry)
            .unwrap_err()
            .contains("running in the background"));
    }

    #[test]
    fn bundled_catalog_is_valid_and_contains_enabled_services() {
        let catalog = ServiceCatalog::from_bundled_config().unwrap();
        assert!(catalog.services.read().unwrap().len() >= 12);
        assert!(catalog.resolve_enabled("jellyfin").is_ok());
        assert!(catalog.has_enabled_service("glances"));
        assert!(!catalog.has_enabled_service("missing-provider"));
    }

    #[test]
    fn trusted_catalog_rejects_edge_whitespace() {
        assert!(TrustedService::try_from(&configured_service(
            "Test Service ",
            "http://192.168.1.10:8080"
        ))
        .is_err());
        assert!(TrustedService::try_from(&configured_service(
            "Test Service",
            "http://192.168.1.10:8080 "
        ))
        .is_err());
    }

    #[test]
    fn catalog_replacement_updates_endpoints_and_invalidates_old_registry_metadata() {
        let mut configuration =
            ServiceConfiguration::parse_document(BUNDLED_SERVICE_CONFIG).unwrap();
        configuration
            .services
            .retain(|service| service.id == "jellyfin");
        let original_url_text = configuration.services[0].url.clone();
        let catalog = ServiceCatalog::from_configuration(&configuration).unwrap();

        assert!(catalog.matches_registered_configuration(
            "jellyfin",
            &original_url_text,
            TlsPolicy::Strict,
            ServiceAuthentication::default().into(),
        ));

        configuration.services[0].url = "http://192.168.0.50:8096".into();
        catalog.replace_configuration(&configuration).unwrap();

        assert_eq!(
            catalog.resolve_endpoint("jellyfin").unwrap().url.as_str(),
            "http://192.168.0.50:8096/"
        );
        assert!(!catalog.matches_registered_configuration(
            "jellyfin",
            &original_url_text,
            TlsPolicy::Strict,
            ServiceAuthentication::default().into(),
        ));

        configuration.services[0].url = "https://192.168.0.50:8096".into();
        configuration.services[0].tls_policy = ServiceTlsPolicy::AllowInvalidLocalCertificate;
        catalog.replace_configuration(&configuration).unwrap();
        assert!(!catalog.matches_registered_configuration(
            "jellyfin",
            "https://192.168.0.50:8096",
            TlsPolicy::Strict,
            ServiceAuthentication::default().into(),
        ));
        assert!(catalog.matches_registered_configuration(
            "jellyfin",
            "https://192.168.0.50:8096",
            TlsPolicy::AllowInvalidLocalCertificate,
            ServiceAuthentication::default().into(),
        ));

        configuration.services[0].enabled = false;
        catalog.replace_configuration(&configuration).unwrap();
        assert!(!catalog.has_enabled_service("jellyfin"));
    }

    #[test]
    fn catalog_revocation_selects_only_removed_disabled_or_changed_entries() {
        let mut configuration =
            ServiceConfiguration::parse_document(BUNDLED_SERVICE_CONFIG).unwrap();
        configuration.services.truncate(3);
        let unchanged = configuration.services[0].clone();
        let mut disabled = configuration.services[1].clone();
        let changed = configuration.services[2].clone();

        let mut registry = RegistryInner::default();
        for service in [&unchanged, &disabled, &changed] {
            registry.entries.insert(
                service.id.clone(),
                RegistryEntry {
                    label: service_webview_label(&service.id),
                    url: service.url.clone(),
                    tls_policy: service.tls_policy.into(),
                    browser_authentication: service.authentication.into(),
                    // A pending, unchanged open is a warm session too and
                    // must not be selected for revocation.
                    ready: service.id != unchanged.id,
                    attached_to_tab: false,
                    last_used: 0,
                },
            );
        }
        registry.entries.insert(
            "removed-service".into(),
            RegistryEntry {
                label: service_webview_label("removed-service"),
                url: unchanged.url.clone(),
                tls_policy: unchanged.tls_policy.into(),
                browser_authentication: unchanged.authentication.into(),
                ready: true,
                attached_to_tab: false,
                last_used: 0,
            },
        );

        disabled.enabled = false;
        configuration.services[1] = disabled.clone();
        configuration.services[2].url = "http://192.168.0.42:8888".into();
        let catalog = ServiceCatalog::from_configuration(&configuration).unwrap();

        let mut expected = vec![disabled.id, changed.id, "removed-service".into()];
        expected.sort();
        assert_eq!(stale_catalog_entry_ids(&registry, &catalog), expected);
        assert!(!stale_catalog_entry_ids(&registry, &catalog).contains(&unchanged.id));
    }

    #[test]
    fn navigation_origin_rejects_lookalikes_credentials_and_wrong_ports() {
        let origin =
            AllowedOrigin::from_url(&Url::parse("https://192.168.1.10:8443/app").unwrap()).unwrap();

        assert!(origin.matches(&Url::parse("https://192.168.1.10:8443/next").unwrap()));
        assert!(!origin.matches(&Url::parse("https://192.168.1.10:9443/next").unwrap()));
        assert!(!origin.matches(&Url::parse("http://192.168.1.10:8443/next").unwrap()));
        assert!(!origin.matches(&Url::parse("https://192.168.1.10.evil.test:8443/next").unwrap()));
        assert!(!origin.matches(&Url::parse("https://192.168.1.10:8443@evil.test/next").unwrap()));
    }

    #[test]
    fn authentication_origins_are_canonical_and_always_include_the_effective_port() {
        assert_eq!(
            canonical_origin(&Url::parse("https://Example.COM./path").unwrap()).unwrap(),
            "https://example.com:443"
        );
        assert_eq!(
            canonical_origin(&Url::parse("http://[::1]/status").unwrap()).unwrap(),
            "http://[::1]:80"
        );
        assert_eq!(
            canonical_origin(&Url::parse("https://192.168.1.10:9443/").unwrap()).unwrap(),
            "https://192.168.1.10:9443"
        );

        let oversized_host = std::iter::repeat_n("a", 260).collect::<Vec<_>>().join(".");
        let oversized_url = Url::parse(&format!("https://{oversized_host}/")).unwrap();
        assert!(canonical_origin(&oversized_url).is_err());
    }

    #[test]
    fn authentication_target_is_available_for_disabled_services_and_is_revisioned() {
        let mut configuration =
            ServiceConfiguration::parse_document(BUNDLED_SERVICE_CONFIG).unwrap();
        configuration
            .services
            .retain(|service| service.id == "jellyfin");
        configuration.services[0].enabled = false;
        let catalog = ServiceCatalog::from_configuration(&configuration).unwrap();

        let first = catalog.resolve_authentication_target("jellyfin").unwrap();
        assert!(!first.enabled);
        assert_eq!(
            first.canonical_origin,
            catalog.canonical_origin_for_service("jellyfin").unwrap()
        );

        configuration.services[0].enabled = true;
        catalog.replace_configuration(&configuration).unwrap();
        let second = catalog.resolve_authentication_target("jellyfin").unwrap();
        assert!(second.enabled);
        assert!(second.catalog_revision > first.catalog_revision);
    }

    #[test]
    fn browser_authentication_changes_invalidate_registered_views() {
        let mut configuration =
            ServiceConfiguration::parse_document(BUNDLED_SERVICE_CONFIG).unwrap();
        configuration
            .services
            .retain(|service| service.id == "jellyfin");
        let original = configuration.services[0].clone();
        let catalog = ServiceCatalog::from_configuration(&configuration).unwrap();
        let old_fingerprint = BrowserAuthenticationFingerprint::from(original.authentication);

        configuration.services[0].authentication.browser = ServiceBrowserAuthentication::HttpBasic;
        configuration.services[0]
            .authentication
            .allow_insecure_local_http = true;
        catalog.replace_configuration(&configuration).unwrap();

        assert!(!catalog.matches_registered_configuration(
            &original.id,
            &original.url,
            original.tls_policy.into(),
            old_fingerprint,
        ));
    }

    #[test]
    fn api_only_plaintext_opt_in_is_not_part_of_the_browser_fingerprint() {
        let baseline = BrowserAuthenticationFingerprint::from(ServiceAuthentication::default());
        let api_only = ServiceAuthentication {
            api: crate::service_settings::ServiceApiAuthentication::HomarrApiKey,
            browser: ServiceBrowserAuthentication::None,
            allow_insecure_local_http: true,
        };

        assert_eq!(BrowserAuthenticationFingerprint::from(api_only), baseline);
    }

    #[test]
    fn browser_basic_submission_budget_is_bounded() {
        let submissions = AtomicU8::new(0);
        assert!(reserve_basic_auth_submission(&submissions));
        assert!(reserve_basic_auth_submission(&submissions));
        assert!(!reserve_basic_auth_submission(&submissions));
        assert_eq!(submissions.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn local_certificate_policy_rejects_public_expired_scope() {
        for url in [
            "https://example.com",
            "https://8.8.8.8",
            "http://192.168.1.10",
            "https://0.0.0.0",
        ] {
            assert!(validate_local_tls_exception(&Url::parse(url).unwrap()).is_err());
        }

        assert!(
            validate_local_tls_exception(&Url::parse("https://192.168.1.10:9443").unwrap()).is_ok()
        );
    }

    #[test]
    fn webview_bounds_reject_invalid_geometry() {
        let valid = ChildBounds {
            x: 0.0,
            y: 100.0,
            width: 1_000.0,
            height: 700.0,
        };
        assert!(valid.validate().is_ok());
        assert!(ChildBounds {
            width: 0.0,
            ..valid
        }
        .validate()
        .is_err());
        assert!(ChildBounds { x: -1.0, ..valid }.validate().is_err());
        assert!(ChildBounds {
            height: f64::NAN,
            ..valid
        }
        .validate()
        .is_err());
    }

    #[test]
    fn pool_eviction_prefers_dormant_oldest_and_protects_active_and_reserved() {
        let mut registry = RegistryInner {
            active_service_id: Some("active".into()),
            ..RegistryInner::default()
        };
        registry
            .entries
            .insert("active".into(), registry_entry(true, 1));
        registry
            .entries
            .insert("reserved".into(), registry_entry(false, 2));
        registry
            .entries
            .insert("attached-old".into(), registry_entry(true, 3));
        registry
            .entries
            .insert("dormant-old".into(), registry_entry(false, 4));
        registry
            .entries
            .insert("dormant-new".into(), registry_entry(false, 6));
        registry
            .entries
            .insert("attached-new".into(), registry_entry(true, 5));

        assert_eq!(
            pool_eviction_candidates(&registry, Some("reserved")),
            vec!["dormant-old", "dormant-new", "attached-old", "attached-new"]
        );
    }

    #[test]
    fn explicit_close_forgets_the_warm_view_and_clears_active_state() {
        let mut registry = RegistryInner {
            active_service_id: Some("jellyfin".into()),
            ..RegistryInner::default()
        };
        registry
            .entries
            .insert("jellyfin".into(), registry_entry(true, 1));
        registry
            .entries
            .insert("sonarr".into(), registry_entry(true, 2));

        finalize_registry_close(&mut registry, "jellyfin", Ok(())).unwrap();

        assert!(!registry.entries.contains_key("jellyfin"));
        assert!(registry.entries.contains_key("sonarr"));
        assert_eq!(registry.active_service_id, None);
    }

    #[test]
    fn closing_an_inactive_view_preserves_the_active_warm_view() {
        let mut registry = RegistryInner {
            active_service_id: Some("jellyfin".into()),
            ..RegistryInner::default()
        };
        registry
            .entries
            .insert("jellyfin".into(), registry_entry(true, 1));
        registry
            .entries
            .insert("sonarr".into(), registry_entry(true, 2));

        finalize_registry_close(&mut registry, "sonarr", Ok(())).unwrap();

        assert!(registry.entries.contains_key("jellyfin"));
        assert!(!registry.entries.contains_key("sonarr"));
        assert_eq!(registry.active_service_id.as_deref(), Some("jellyfin"));
    }

    #[test]
    fn failed_native_close_preserves_the_registry_and_active_state() {
        let mut registry = RegistryInner {
            active_service_id: Some("jellyfin".into()),
            ..RegistryInner::default()
        };
        registry
            .entries
            .insert("jellyfin".into(), registry_entry(true, 1));

        let error =
            finalize_registry_close(&mut registry, "jellyfin", Err("native close failed".into()))
                .unwrap_err();

        assert_eq!(error, "native close failed");
        assert!(registry.entries.contains_key("jellyfin"));
        assert_eq!(registry.active_service_id.as_deref(), Some("jellyfin"));
    }

    #[test]
    fn registry_reconcile_ids_are_valid_and_unique() {
        assert_eq!(
            validate_service_id_set(vec!["jellyfin".into(), "server-firefox".into()])
                .unwrap()
                .len(),
            2
        );
        assert!(validate_service_id_set(vec!["jellyfin".into(), "jellyfin".into()]).is_err());
        assert!(validate_service_id_set(vec!["Not Valid".into()]).is_err());
    }
}
