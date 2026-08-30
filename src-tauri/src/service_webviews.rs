use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::PathBuf,
    process::Command,
    sync::Mutex,
};
use tauri::{
    webview::{NewWindowResponse, WebviewBuilder},
    AppHandle, LogicalPosition, LogicalSize, Manager, Rect, Url, Webview, WebviewUrl,
};

const BUNDLED_SERVICE_CONFIG: &str = include_str!("../../public/config/services.json");
const MAX_SERVICE_ID_LENGTH: usize = 64;
const MAX_SERVICE_NAME_LENGTH: usize = 80;
const MAX_URL_LENGTH: usize = 2_048;
const MAX_WEBVIEW_COORDINATE: f64 = 65_536.0;
const MAX_LIVE_SERVICE_WEBVIEWS: usize = 6;
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundledServiceConfiguration {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    version: u8,
    services: Vec<BundledService>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundledService {
    id: String,
    name: String,
    description: Option<String>,
    url: String,
    icon: String,
    category: String,
    enabled: bool,
    accent: Option<String>,
    #[serde(default)]
    tls_policy: TlsPolicy,
}

#[derive(Clone, Debug)]
struct TrustedService {
    id: String,
    url_text: String,
    url: Url,
    tls_policy: TlsPolicy,
    enabled: bool,
}

pub struct ServiceCatalog {
    services: HashMap<String, TrustedService>,
}

#[derive(Clone, Debug)]
pub(crate) struct TrustedServiceEndpoint {
    pub url: Url,
    pub allow_invalid_local_certificate: bool,
}

impl ServiceCatalog {
    pub fn from_bundled_config() -> Result<Self, String> {
        let configuration: BundledServiceConfiguration =
            serde_json::from_str(BUNDLED_SERVICE_CONFIG)
                .map_err(|error| format!("services.json is invalid: {error}"))?;

        if configuration.version != 1 {
            return Err("services.json uses an unsupported version.".into());
        }

        if configuration.services.len() > 500 {
            return Err("services.json contains too many services.".into());
        }

        let mut services = HashMap::with_capacity(configuration.services.len());

        for bundled in configuration.services {
            let trusted = TrustedService::try_from(bundled)?;
            let service_id = trusted.id.clone();

            if services.insert(service_id.clone(), trusted).is_some() {
                return Err(format!(
                    "services.json contains the duplicate id \"{service_id}\"."
                ));
            }
        }

        let _ = configuration.schema;
        Ok(Self { services })
    }

    fn resolve_enabled(&self, service_id: &str) -> Result<&TrustedService, String> {
        let service = self
            .services
            .get(service_id)
            .ok_or_else(|| "The requested service is not in the trusted catalog.".to_string())?;

        if !service.enabled {
            return Err("The requested service is disabled.".into());
        }

        Ok(service)
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
}

impl TryFrom<BundledService> for TrustedService {
    type Error = String;

    fn try_from(service: BundledService) -> Result<Self, Self::Error> {
        if !is_valid_service_id(&service.id) {
            return Err(format!("Service id \"{}\" is invalid.", service.id));
        }

        if service.name.trim().is_empty()
            || service.name.trim() != service.name
            || service.name.len() > MAX_SERVICE_NAME_LENGTH
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

        if service.tls_policy == TlsPolicy::AllowInvalidLocalCertificate {
            validate_local_tls_exception(&url)?;
        }

        // These fields are parsed to keep Rust's trust source as strict as the
        // frontend configuration loader, even though Phase 5 does not render them.
        let _ = (
            service.description,
            service.icon,
            service.category,
            service.accent,
        );

        Ok(Self {
            id: service.id,
            url_text: service.url,
            url,
            tls_policy: service.tls_policy,
            enabled: service.enabled,
        })
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

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ParkServiceWebviewResult {
    Retained,
    Closed,
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
    ready: bool,
    attached_to_tab: bool,
    last_used: u64,
}

#[derive(Default)]
struct RegistryInner {
    entries: HashMap<String, RegistryEntry>,
    active_service_id: Option<String>,
    use_sequence: u64,
}

#[derive(Default)]
pub struct ServiceWebviewRegistry {
    inner: Mutex<RegistryInner>,
}

#[tauri::command]
pub async fn open_service_webview(
    caller: Webview,
    app: AppHandle,
    request: OpenServiceWebviewRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
) -> Result<OpenServiceWebviewResult, String> {
    ensure_trusted_caller(&caller)?;
    let bounds = request.bounds.validate()?;
    let service = catalog.resolve_enabled(&request.service_id)?.clone();
    let label = service_webview_label(&service.id);
    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;

    hide_registered_webviews(&app, &inner, Some(&service.id))?;
    inner.active_service_id = None;

    if let Some(entry) = inner.entries.get(&service.id).cloned() {
        let configuration_matches =
            entry.url == service.url_text && entry.tls_policy == service.tls_policy;

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
            ready: false,
            attached_to_tab: true,
            last_used,
        },
    );

    let setup_result = (|| {
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
    let entry = inner
        .entries
        .get(&request.service_id)
        .cloned()
        .ok_or_else(|| {
            "The service webview must be opened before it can be activated.".to_string()
        })?;

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
            inner.entries.remove(&service_id);
            continue;
        };

        if entry.ready && enabled_service_ids.contains(&service_id) {
            continue;
        }

        match view.close() {
            Ok(()) => {
                inner.entries.remove(&service_id);
            }
            Err(_) => {
                failed_close_count += 1;
            }
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
pub async fn park_service_webview(
    caller: Webview,
    app: AppHandle,
    service_id: String,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
) -> Result<ParkServiceWebviewResult, String> {
    ensure_trusted_caller(&caller)?;

    if !is_valid_service_id(&service_id) {
        return Err("The service id is invalid.".into());
    }

    let mut inner = registry
        .inner
        .lock()
        .map_err(|_| "The service webview registry is unavailable.".to_string())?;

    let mut result = ParkServiceWebviewResult::Closed;

    if let Some(entry) = inner.entries.get(&service_id).cloned() {
        if let Some(view) = app.get_webview(&entry.label) {
            if let Err(hide_error) = view.hide() {
                if let Err(close_error) = view.close() {
                    return Err(format!(
                        "Could not hide the service view: {hide_error}. Closing it also failed: {close_error}"
                    ));
                }

                inner.entries.remove(&service_id);
                if inner.active_service_id.as_deref() == Some(service_id.as_str()) {
                    inner.active_service_id = None;
                }
                trim_service_webview_pool(&app, &mut inner, None)?;
                return Ok(ParkServiceWebviewResult::Closed);
            }

            set_entry_attached(&mut inner, &service_id, false);
            result = ParkServiceWebviewResult::Retained;
        } else {
            inner.entries.remove(&service_id);
        }
    }

    if inner.active_service_id.as_deref() == Some(service_id.as_str()) {
        inner.active_service_id = None;
    }

    trim_service_webview_pool(&app, &mut inner, None)?;
    Ok(result)
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

    if let Some(entry) = inner.entries.get(&service_id).cloned() {
        if let Some(view) = app.get_webview(&entry.label) {
            view.close()
                .map_err(webview_error("close the service webview"))?;
        }

        inner.entries.remove(&service_id);
    }

    if inner.active_service_id.as_deref() == Some(service_id.as_str()) {
        inner.active_service_id = None;
    }

    Ok(())
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
    if value.is_empty() || value.len() > MAX_URL_LENGTH {
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

    fn bundled_service(name: &str, url: &str) -> BundledService {
        BundledService {
            id: "test-service".into(),
            name: name.into(),
            description: None,
            url: url.into(),
            icon: "server".into(),
            category: "System".into(),
            enabled: true,
            accent: None,
            tls_policy: TlsPolicy::Strict,
        }
    }

    fn registry_entry(attached_to_tab: bool, last_used: u64) -> RegistryEntry {
        RegistryEntry {
            label: format!("service-test-{last_used}"),
            url: "http://192.168.1.10:8080".into(),
            tls_policy: TlsPolicy::Strict,
            ready: true,
            attached_to_tab,
            last_used,
        }
    }

    #[test]
    fn bundled_catalog_is_valid_and_contains_enabled_services() {
        let catalog = ServiceCatalog::from_bundled_config().unwrap();
        assert!(catalog.services.len() >= 12);
        assert!(catalog.resolve_enabled("jellyfin").is_ok());
    }

    #[test]
    fn trusted_catalog_rejects_edge_whitespace() {
        assert!(TrustedService::try_from(bundled_service(
            "Test Service ",
            "http://192.168.1.10:8080"
        ))
        .is_err());
        assert!(TrustedService::try_from(bundled_service(
            "Test Service",
            "http://192.168.1.10:8080 "
        ))
        .is_err());
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
