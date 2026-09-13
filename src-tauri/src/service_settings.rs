use crate::{
    background_runtime::schedule_catalog_reconcile,
    credential_vault::delete_all_service_credentials,
    desktop_widgets::{refresh_after_catalog_change, DesktopWidgetBroker},
    download_center::DownloadCenterClients,
    service_webviews::{revoke_stale_service_webviews, ServiceCatalog, ServiceWebviewRegistry},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
};
use tauri::{AppHandle, Manager, Url, Webview};

pub const BUNDLED_SERVICE_CONFIG: &str = include_str!("../../public/config/services.json");

const CONFIGURATION_VERSION: u8 = 1;
const MAX_SERVICE_COUNT: usize = 500;
const MAX_SERVICE_ID_LENGTH: usize = 64;
const MAX_SERVICE_NAME_LENGTH: usize = 80;
const MAX_SERVICE_DESCRIPTION_LENGTH: usize = 160;
const MAX_SERVICE_ICON_LENGTH: usize = 100;
const MAX_SERVICE_CATEGORY_LENGTH: usize = 40;
const MAX_URL_LENGTH: usize = 2_048;
const MAX_CONFIGURATION_FILE_BYTES: u64 = 2 * 1024 * 1024;
const CONFIGURATION_FILE_NAME: &str = "services.json";
const CONFIGURATION_BACKUP_FILE_NAME: &str = "services.json.bak";
const CONFIGURATION_TEMP_FILE_NAME: &str = "services.json.tmp";
const CONFIGURATION_BACKUP_TEMP_FILE_NAME: &str = "services.json.bak.tmp";
const PENDING_CREDENTIAL_CLEANUP_FILE_NAME: &str = "services.credentials-cleanup.json";
const PENDING_CREDENTIAL_CLEANUP_TEMP_FILE_NAME: &str = "services.credentials-cleanup.json.tmp";
const PENDING_CREDENTIAL_CLEANUP_VERSION: u8 = 1;
const MAX_PENDING_CREDENTIAL_CLEANUP_IDS: usize = MAX_SERVICE_COUNT;
const MAX_PENDING_CREDENTIAL_CLEANUP_FILE_BYTES: u64 = 64 * 1024;
const INVALID_CONFIGURATION_NOTICE: &str =
    "The saved service configuration was invalid and was replaced with the bundled defaults.";
const MAIN_WEBVIEW_LABEL: &str = "main";
const PENDING_CREDENTIAL_CLEANUP_NOTICE: &str =
    "One or more credentials for removed services could not be deleted from Windows Credential Manager. Cleanup will be retried.";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ServiceTlsPolicy {
    #[default]
    Strict,
    AllowInvalidLocalCertificate,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ServiceApiAuthentication {
    #[default]
    None,
    HomarrApiKey,
    GlancesHttpBasic,
    GlancesBearer,
    QbittorrentWebApi,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ServiceBrowserAuthentication {
    #[default]
    None,
    HttpBasic,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ServiceAuthentication {
    #[serde(default)]
    pub(crate) api: ServiceApiAuthentication,
    #[serde(default)]
    pub(crate) browser: ServiceBrowserAuthentication,
    #[serde(default)]
    pub(crate) allow_insecure_local_http: bool,
}

impl ServiceAuthentication {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.api != ServiceApiAuthentication::None
            || self.browser != ServiceBrowserAuthentication::None
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ServiceAccent {
    Violet,
    Amber,
    Blue,
    Cyan,
    Green,
    Orange,
    Red,
    #[default]
    Slate,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default = "default_service_description")]
    pub(crate) description: String,
    pub(crate) url: String,
    pub(crate) icon: String,
    pub(crate) category: String,
    pub(crate) enabled: bool,
    #[serde(default)]
    accent: ServiceAccent,
    #[serde(default)]
    pub(crate) tls_policy: ServiceTlsPolicy,
    #[serde(default, skip_serializing_if = "ServiceAuthentication::is_default")]
    pub(crate) authentication: ServiceAuthentication,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfiguration {
    pub(crate) version: u8,
    pub(crate) services: Vec<ServiceDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceConfigurationDocument {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    version: u8,
    services: Vec<ServiceDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveServiceConfigurationRequest {
    configuration: ServiceConfiguration,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConfigurationResponse {
    configuration: ServiceConfiguration,
    recovery_notice: Option<String>,
    backup_available: bool,
}

/// This journal deliberately contains only service identifiers. It is written
/// before a configuration which removes services becomes durable, so a crash
/// cannot leave their Credential Manager entries without a retry record.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingCredentialCleanupJournal {
    version: u8,
    pending_service_ids: Vec<String>,
}

#[derive(Debug)]
struct PendingCleanupResult {
    pending_service_ids: BTreeSet<String>,
    deletion_failed: bool,
}

pub struct ServiceSettings {
    paths: ConfigurationPaths,
    bundled_seed: ServiceConfiguration,
    configuration: RwLock<ServiceConfiguration>,
    recovery_notice: RwLock<Option<String>>,
    operation: Mutex<()>,
}

#[derive(Clone, Debug)]
struct ConfigurationPaths {
    primary: PathBuf,
    backup: PathBuf,
    primary_temp: PathBuf,
    backup_temp: PathBuf,
    pending_credential_cleanup: PathBuf,
    pending_credential_cleanup_temp: PathBuf,
}

#[derive(Debug)]
enum ConfigurationReadError {
    Io(String),
    Invalid,
}

impl ServiceConfiguration {
    pub(crate) fn services(&self) -> &[ServiceDefinition] {
        &self.services
    }

    pub(crate) fn service_ids(&self) -> HashSet<String> {
        self.services
            .iter()
            .map(|service| service.id.clone())
            .collect()
    }

    pub(crate) fn normalized(&self) -> Result<Self, String> {
        if self.version != CONFIGURATION_VERSION {
            return Err("configuration.version must be 1 for this application version.".into());
        }

        if self.services.len() > MAX_SERVICE_COUNT {
            return Err(format!(
                "configuration.services cannot contain more than {MAX_SERVICE_COUNT} services."
            ));
        }

        let mut ids = HashSet::with_capacity(self.services.len());
        let mut services = Vec::with_capacity(self.services.len());

        for (index, service) in self.services.iter().enumerate() {
            let normalized = service.normalized(index)?;

            if !ids.insert(normalized.id.clone()) {
                return Err(format!(
                    "configuration.services contains the duplicate id \"{}\".",
                    normalized.id
                ));
            }

            services.push(normalized);
        }

        Ok(Self {
            version: CONFIGURATION_VERSION,
            services,
        })
    }

    pub(crate) fn parse_document(document: &str) -> Result<Self, String> {
        if document.len() as u64 > MAX_CONFIGURATION_FILE_BYTES {
            return Err("The service configuration is too large.".into());
        }

        let parsed: ServiceConfigurationDocument =
            serde_json::from_str(document).map_err(|_| {
                "The service configuration does not match the supported schema.".to_string()
            })?;
        let _ = parsed.schema;

        Self {
            version: parsed.version,
            services: parsed.services,
        }
        .normalized()
    }
}

impl ServiceDefinition {
    fn normalized(&self, index: usize) -> Result<Self, String> {
        let path = format!("configuration.services[{index}]");
        let id = normalize_text(&self.id, &format!("{path}.id"), MAX_SERVICE_ID_LENGTH)?;

        if !is_valid_service_id(&id) {
            return Err(format!(
                "{path}.id must use lowercase letters, numbers and single hyphens."
            ));
        }

        let name = normalize_text(&self.name, &format!("{path}.name"), MAX_SERVICE_NAME_LENGTH)?;
        let description = normalize_text(
            &self.description,
            &format!("{path}.description"),
            MAX_SERVICE_DESCRIPTION_LENGTH,
        )?;
        let url_text = normalize_text(&self.url, &format!("{path}.url"), MAX_URL_LENGTH)?;
        let url = validate_http_url(&url_text, &format!("{path}.url"))?;
        let icon = normalize_text(&self.icon, &format!("{path}.icon"), MAX_SERVICE_ICON_LENGTH)?;
        let category = normalize_text(
            &self.category,
            &format!("{path}.category"),
            MAX_SERVICE_CATEGORY_LENGTH,
        )?;

        if self.tls_policy == ServiceTlsPolicy::AllowInvalidLocalCertificate {
            validate_local_tls_exception(&url, &format!("{path}.tlsPolicy"))?;
        }

        let mut authentication = self.authentication;
        if authentication.api == ServiceApiAuthentication::GlancesBearer
            && authentication.browser == ServiceBrowserAuthentication::HttpBasic
        {
            return Err(format!(
                "{path}.authentication cannot combine Glances bearer and HTTP Basic because both require the Authorization header."
            ));
        }

        if authentication.allow_insecure_local_http {
            if url.scheme() == "https" {
                authentication.allow_insecure_local_http = false;
            } else if !authentication.is_configured() {
                return Err(format!(
                    "{path}.authentication.allowInsecureLocalHttp requires an authentication adapter."
                ));
            } else if !is_local_target(&url) {
                return Err(format!(
                    "{path}.authentication.allowInsecureLocalHttp is limited to loopback and private-network targets."
                ));
            }
        }

        Ok(Self {
            id,
            name,
            description,
            url: url_text,
            icon,
            category,
            enabled: self.enabled,
            accent: self.accent,
            tls_policy: self.tls_policy,
            authentication,
        })
    }
}

impl ServiceSettings {
    pub fn initialize(
        config_directory: impl AsRef<Path>,
        bundled_seed: &str,
    ) -> Result<Self, String> {
        let mut delete_credentials = delete_all_service_credentials;
        Self::initialize_with_cleanup(config_directory, bundled_seed, &mut delete_credentials)
    }

    fn initialize_with_cleanup(
        config_directory: impl AsRef<Path>,
        bundled_seed: &str,
        delete_credentials: &mut impl FnMut(&str) -> Result<(), String>,
    ) -> Result<Self, String> {
        let directory = config_directory.as_ref();
        fs::create_dir_all(directory).map_err(|error| {
            format!("The application configuration directory could not be created: {error}")
        })?;

        let paths = ConfigurationPaths::new(directory);
        let seed = ServiceConfiguration::parse_document(bundled_seed)
            .map_err(|error| format!("The bundled service configuration is invalid: {error}"))?;
        let (configuration, recovery_notice) = if paths.primary.exists() {
            match read_configuration(&paths.primary) {
                Ok(configuration) => (configuration, None),
                Err(ConfigurationReadError::Invalid) => {
                    persist_configuration(&paths, &seed, false)?;
                    (seed.clone(), Some(INVALID_CONFIGURATION_NOTICE.to_string()))
                }
                Err(ConfigurationReadError::Io(error)) => return Err(error),
            }
        } else {
            persist_configuration(&paths, &seed, false)?;
            (seed.clone(), None)
        };

        let settings = Self {
            paths,
            bundled_seed: seed,
            configuration: RwLock::new(configuration),
            recovery_notice: RwLock::new(recovery_notice),
            operation: Mutex::new(()),
        };

        let cleanup = settings.retry_pending_credential_cleanup_with(delete_credentials)?;
        if cleanup.deletion_failed {
            settings.set_pending_credential_cleanup_notice()?;
        }

        Ok(settings)
    }

    pub fn configuration(&self) -> Result<ServiceConfiguration, String> {
        self.configuration
            .read()
            .map(|configuration| configuration.clone())
            .map_err(|_| "The service configuration state is unavailable.".to_string())
    }

    pub(crate) fn lock_operation(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.operation
            .lock()
            .map_err(|_| "Service settings operations are unavailable.".to_string())
    }

    fn response(&self) -> Result<ServiceConfigurationResponse, String> {
        let configuration = self.configuration()?;
        let recovery_notice = self
            .recovery_notice
            .read()
            .map(|notice| notice.clone())
            .map_err(|_| "The service recovery state is unavailable.".to_string())?;

        Ok(ServiceConfigurationResponse {
            configuration,
            recovery_notice,
            backup_available: valid_backup_available(&self.paths),
        })
    }

    #[cfg(test)]
    fn save(
        &self,
        configuration: ServiceConfiguration,
        catalog: &ServiceCatalog,
    ) -> Result<ServiceConfigurationResponse, String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "Service configuration updates are unavailable.".to_string())?;
        let configuration = configuration.normalized()?;
        self.commit(configuration, catalog, true)
    }

    #[cfg(test)]
    fn restore_backup(
        &self,
        catalog: &ServiceCatalog,
    ) -> Result<ServiceConfigurationResponse, String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "Service configuration updates are unavailable.".to_string())?;

        if !self.paths.backup.is_file() {
            return Err("No valid service configuration backup is available.".into());
        }

        let configuration = match read_configuration(&self.paths.backup) {
            Ok(configuration) => configuration,
            Err(ConfigurationReadError::Invalid) => {
                return Err("No valid service configuration backup is available.".into())
            }
            Err(ConfigurationReadError::Io(error)) => return Err(error),
        };

        self.commit(configuration, catalog, true)
    }

    #[cfg(test)]
    fn commit(
        &self,
        configuration: ServiceConfiguration,
        catalog: &ServiceCatalog,
        create_backup: bool,
    ) -> Result<ServiceConfigurationResponse, String> {
        let mut delete_credentials = delete_all_service_credentials;
        self.commit_with_cleanup_and_catalog_change(
            configuration,
            catalog,
            create_backup,
            &mut delete_credentials,
            Vec::new,
        )
    }

    fn commit_live(
        &self,
        configuration: ServiceConfiguration,
        catalog: &ServiceCatalog,
        create_backup: bool,
        app: &AppHandle,
        broker: &DesktopWidgetBroker,
    ) -> Result<ServiceConfigurationResponse, String> {
        let mut delete_credentials = delete_all_service_credentials;
        self.commit_with_cleanup_and_catalog_change(
            configuration,
            catalog,
            create_backup,
            &mut delete_credentials,
            || reconcile_catalog_change(app, catalog, broker),
        )
    }

    #[cfg(test)]
    fn commit_with_cleanup(
        &self,
        configuration: ServiceConfiguration,
        catalog: &ServiceCatalog,
        create_backup: bool,
        delete_credentials: &mut impl FnMut(&str) -> Result<(), String>,
    ) -> Result<ServiceConfigurationResponse, String> {
        self.commit_with_cleanup_and_catalog_change(
            configuration,
            catalog,
            create_backup,
            delete_credentials,
            Vec::new,
        )
    }

    fn commit_with_cleanup_and_catalog_change(
        &self,
        configuration: ServiceConfiguration,
        catalog: &ServiceCatalog,
        create_backup: bool,
        delete_credentials: &mut impl FnMut(&str) -> Result<(), String>,
        on_catalog_changed: impl FnOnce() -> Vec<String>,
    ) -> Result<ServiceConfigurationResponse, String> {
        let previous_service_ids = self.configuration()?.service_ids();
        let mut pending_cleanup = self.retry_pending_credential_cleanup_with(delete_credentials)?;
        let current_service_ids = configuration.service_ids();

        if pending_cleanup
            .pending_service_ids
            .iter()
            .any(|service_id| current_service_ids.contains(service_id))
        {
            return Err(
                "A removed service cannot be re-added until its pending credential cleanup completes."
                    .into(),
            );
        }

        let removed_service_ids = previous_service_ids
            .difference(&current_service_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !removed_service_ids.is_empty() {
            pending_cleanup
                .pending_service_ids
                .extend(removed_service_ids);
            persist_pending_credential_cleanup_journal(
                &self.paths,
                &pending_cleanup.pending_service_ids,
            )?;
        }

        persist_configuration(&self.paths, &configuration, create_backup)?;
        catalog.replace_configuration(&configuration)?;
        let catalog_change_notices = on_catalog_changed();

        *self
            .configuration
            .write()
            .map_err(|_| "The service configuration state is unavailable.".to_string())? =
            configuration;
        *self
            .recovery_notice
            .write()
            .map_err(|_| "The service recovery state is unavailable.".to_string())? = None;

        let cleanup_incomplete =
            match self.retry_pending_credential_cleanup_with(delete_credentials) {
                Ok(cleanup) => cleanup.deletion_failed,
                Err(_) => true,
            };
        if cleanup_incomplete {
            self.set_pending_credential_cleanup_notice()?;
        }

        let mut response = self.response()?;
        append_recovery_notices(&mut response, catalog_change_notices);
        Ok(response)
    }

    fn retry_pending_credential_cleanup_with(
        &self,
        delete_credentials: &mut impl FnMut(&str) -> Result<(), String>,
    ) -> Result<PendingCleanupResult, String> {
        let current_service_ids = self.configuration()?.service_ids();
        retry_pending_credential_cleanup(&self.paths, &current_service_ids, delete_credentials)
    }

    fn set_pending_credential_cleanup_notice(&self) -> Result<(), String> {
        let mut recovery_notice = self
            .recovery_notice
            .write()
            .map_err(|_| "The service recovery state is unavailable.".to_string())?;
        match recovery_notice.as_mut() {
            Some(notice) => {
                notice.push(' ');
                notice.push_str(PENDING_CREDENTIAL_CLEANUP_NOTICE);
            }
            None => *recovery_notice = Some(PENDING_CREDENTIAL_CLEANUP_NOTICE.to_string()),
        }
        Ok(())
    }
}

impl ConfigurationPaths {
    fn new(directory: &Path) -> Self {
        Self {
            primary: directory.join(CONFIGURATION_FILE_NAME),
            backup: directory.join(CONFIGURATION_BACKUP_FILE_NAME),
            primary_temp: directory.join(CONFIGURATION_TEMP_FILE_NAME),
            backup_temp: directory.join(CONFIGURATION_BACKUP_TEMP_FILE_NAME),
            pending_credential_cleanup: directory.join(PENDING_CREDENTIAL_CLEANUP_FILE_NAME),
            pending_credential_cleanup_temp: directory
                .join(PENDING_CREDENTIAL_CLEANUP_TEMP_FILE_NAME),
        }
    }
}

#[tauri::command]
pub fn get_service_configuration(
    caller: Webview,
    settings: tauri::State<'_, ServiceSettings>,
) -> Result<ServiceConfigurationResponse, String> {
    ensure_trusted_caller(&caller)?;
    let _operation = settings
        .operation
        .lock()
        .map_err(|_| "Service configuration updates are unavailable.".to_string())?;
    settings.response()
}

#[tauri::command]
pub fn save_service_configuration(
    caller: Webview,
    app: AppHandle,
    request: SaveServiceConfigurationRequest,
    settings: tauri::State<'_, ServiceSettings>,
    catalog: tauri::State<'_, ServiceCatalog>,
    broker: tauri::State<'_, DesktopWidgetBroker>,
) -> Result<ServiceConfigurationResponse, String> {
    ensure_trusted_caller(&caller)?;
    let _operation = settings.lock_operation()?;
    let configuration = request.configuration.normalized()?;
    settings.commit_live(configuration, &catalog, true, &app, &broker)
}

#[tauri::command]
pub fn reset_service_configuration(
    caller: Webview,
    app: AppHandle,
    settings: tauri::State<'_, ServiceSettings>,
    catalog: tauri::State<'_, ServiceCatalog>,
    broker: tauri::State<'_, DesktopWidgetBroker>,
) -> Result<ServiceConfigurationResponse, String> {
    ensure_trusted_caller(&caller)?;
    let _operation = settings.lock_operation()?;
    settings.commit_live(settings.bundled_seed.clone(), &catalog, true, &app, &broker)
}

#[tauri::command]
pub fn restore_service_configuration_backup(
    caller: Webview,
    app: AppHandle,
    settings: tauri::State<'_, ServiceSettings>,
    catalog: tauri::State<'_, ServiceCatalog>,
    broker: tauri::State<'_, DesktopWidgetBroker>,
) -> Result<ServiceConfigurationResponse, String> {
    ensure_trusted_caller(&caller)?;
    let _operation = settings.lock_operation()?;

    if !settings.paths.backup.is_file() {
        return Err("No valid service configuration backup is available.".into());
    }

    let configuration = match read_configuration(&settings.paths.backup) {
        Ok(configuration) => configuration,
        Err(ConfigurationReadError::Invalid) => {
            return Err("No valid service configuration backup is available.".into())
        }
        Err(ConfigurationReadError::Io(error)) => return Err(error),
    };
    settings.commit_live(configuration, &catalog, true, &app, &broker)
}

fn reconcile_catalog_change(
    app: &AppHandle,
    catalog: &ServiceCatalog,
    broker: &DesktopWidgetBroker,
) -> Vec<String> {
    let mut notices = Vec::new();

    app.state::<DownloadCenterClients>().clear_sessions();

    let registry = app.state::<ServiceWebviewRegistry>();
    if revoke_stale_service_webviews(app, catalog, &registry).is_err() {
        notices.push("One or more removed service windows could not be closed.".to_string());
    }

    if refresh_after_catalog_change(app, catalog, broker).is_err() {
        notices.push(
            "Desktop-card monitoring could not be refreshed after the catalog changed.".to_string(),
        );
    }

    schedule_catalog_reconcile(app.clone());

    notices
}

fn append_recovery_notices(response: &mut ServiceConfigurationResponse, notices: Vec<String>) {
    if notices.is_empty() {
        return;
    }

    let mut all_notices = response
        .recovery_notice
        .take()
        .into_iter()
        .collect::<Vec<_>>();
    all_notices.extend(notices);
    response.recovery_notice = Some(all_notices.join(" "));
}

fn ensure_trusted_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() != MAIN_WEBVIEW_LABEL {
        return Err("This command is available only to the trusted Personal Hub UI.".into());
    }

    Ok(())
}

fn default_service_description() -> String {
    "Local service".into()
}

fn normalize_text(value: &str, path: &str, max_length: usize) -> Result<String, String> {
    let normalized = value.trim();
    let length = normalized.chars().count();

    if length == 0 || length > max_length {
        return Err(format!(
            "{path} must contain between 1 and {max_length} characters."
        ));
    }

    Ok(normalized.to_string())
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

fn validate_http_url(value: &str, path: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| format!("{path} must be a valid URL."))?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{path} must use HTTP or HTTPS."));
    }

    if url.host_str().is_none() {
        return Err(format!("{path} must include a hostname."));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{path} must not contain embedded credentials."));
    }

    Ok(url)
}

fn validate_local_tls_exception(url: &Url, path: &str) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err(format!(
            "{path} can only relax certificate checks for HTTPS URLs."
        ));
    }

    if !is_local_target(url) {
        return Err(format!(
            "{path} is limited to loopback and private-network targets."
        ));
    }

    Ok(())
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

fn read_configuration(path: &Path) -> Result<ServiceConfiguration, ConfigurationReadError> {
    let file = File::open(path).map_err(|error| {
        ConfigurationReadError::Io(format!(
            "The saved service configuration could not be opened: {error}"
        ))
    })?;
    let metadata = file.metadata().map_err(|error| {
        ConfigurationReadError::Io(format!(
            "The saved service configuration could not be inspected: {error}"
        ))
    })?;

    if metadata.len() > MAX_CONFIGURATION_FILE_BYTES {
        return Err(ConfigurationReadError::Invalid);
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CONFIGURATION_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ConfigurationReadError::Io(format!(
                "The saved service configuration could not be read: {error}"
            ))
        })?;

    if bytes.len() as u64 > MAX_CONFIGURATION_FILE_BYTES {
        return Err(ConfigurationReadError::Invalid);
    }

    let document = std::str::from_utf8(&bytes).map_err(|_| ConfigurationReadError::Invalid)?;
    ServiceConfiguration::parse_document(document).map_err(|_| ConfigurationReadError::Invalid)
}

fn serialize_configuration(configuration: &ServiceConfiguration) -> Result<Vec<u8>, String> {
    let mut document = serde_json::to_vec_pretty(configuration)
        .map_err(|_| "The service configuration could not be serialized.".to_string())?;
    document.push(b'\n');
    Ok(document)
}

fn read_pending_credential_cleanup_journal(
    paths: &ConfigurationPaths,
) -> Result<BTreeSet<String>, String> {
    if !paths.pending_credential_cleanup.exists() {
        return Ok(BTreeSet::new());
    }

    let file = File::open(&paths.pending_credential_cleanup)
        .map_err(|_| "The pending credential cleanup journal could not be opened.".to_string())?;
    let metadata = file.metadata().map_err(|_| {
        "The pending credential cleanup journal could not be inspected.".to_string()
    })?;
    if metadata.len() > MAX_PENDING_CREDENTIAL_CLEANUP_FILE_BYTES {
        return Err("The pending credential cleanup journal is invalid.".into());
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PENDING_CREDENTIAL_CLEANUP_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The pending credential cleanup journal could not be read.".to_string())?;
    if bytes.len() as u64 > MAX_PENDING_CREDENTIAL_CLEANUP_FILE_BYTES {
        return Err("The pending credential cleanup journal is invalid.".into());
    }

    let journal: PendingCredentialCleanupJournal = serde_json::from_slice(&bytes)
        .map_err(|_| "The pending credential cleanup journal is invalid.".to_string())?;
    if journal.version != PENDING_CREDENTIAL_CLEANUP_VERSION
        || journal.pending_service_ids.len() > MAX_PENDING_CREDENTIAL_CLEANUP_IDS
    {
        return Err("The pending credential cleanup journal is invalid.".into());
    }

    let journal_id_count = journal.pending_service_ids.len();
    let pending_service_ids = journal
        .pending_service_ids
        .into_iter()
        .collect::<BTreeSet<_>>();
    if pending_service_ids.len() != journal_id_count
        || pending_service_ids
            .iter()
            .any(|id| !is_valid_service_id(id))
    {
        return Err("The pending credential cleanup journal is invalid.".into());
    }

    Ok(pending_service_ids)
}

fn persist_pending_credential_cleanup_journal(
    paths: &ConfigurationPaths,
    pending_service_ids: &BTreeSet<String>,
) -> Result<(), String> {
    if pending_service_ids.len() > MAX_PENDING_CREDENTIAL_CLEANUP_IDS
        || pending_service_ids
            .iter()
            .any(|id| !is_valid_service_id(id))
    {
        return Err("The pending credential cleanup journal is invalid.".into());
    }

    let journal = PendingCredentialCleanupJournal {
        version: PENDING_CREDENTIAL_CLEANUP_VERSION,
        pending_service_ids: pending_service_ids.iter().cloned().collect(),
    };
    let mut document = serde_json::to_vec_pretty(&journal).map_err(|_| {
        "The pending credential cleanup journal could not be serialized.".to_string()
    })?;
    document.push(b'\n');

    write_atomic(
        &paths.pending_credential_cleanup,
        &paths.pending_credential_cleanup_temp,
        &document,
    )
    .map_err(|_| "The pending credential cleanup journal could not be written.".to_string())
}

fn retry_pending_credential_cleanup(
    paths: &ConfigurationPaths,
    current_service_ids: &HashSet<String>,
    delete_credentials: &mut impl FnMut(&str) -> Result<(), String>,
) -> Result<PendingCleanupResult, String> {
    let pending_service_ids = read_pending_credential_cleanup_journal(paths)?;
    let mut retained_service_ids = BTreeSet::new();
    let mut deletion_failed = false;

    for service_id in pending_service_ids {
        // A prepared journal entry may survive a failed configuration write. In
        // that case the service is still active and must never be deleted.
        if current_service_ids.contains(&service_id) {
            continue;
        }

        if delete_credentials(&service_id).is_ok() {
            continue;
        }

        deletion_failed = true;
        retained_service_ids.insert(service_id);
    }

    persist_pending_credential_cleanup_journal(paths, &retained_service_ids)?;
    Ok(PendingCleanupResult {
        pending_service_ids: retained_service_ids,
        deletion_failed,
    })
}

fn persist_configuration(
    paths: &ConfigurationPaths,
    configuration: &ServiceConfiguration,
    create_backup: bool,
) -> Result<(), String> {
    let document = serialize_configuration(configuration)?;

    if create_backup && paths.primary.exists() {
        match read_configuration(&paths.primary) {
            Ok(previous) => {
                let backup_document = serialize_configuration(&previous)?;
                write_atomic(&paths.backup, &paths.backup_temp, &backup_document).map_err(
                    |error| {
                        format!("The service configuration backup could not be written: {error}")
                    },
                )?;
            }
            // Never replace a known-good backup with a corrupt externally edited file.
            Err(ConfigurationReadError::Invalid) => {}
            Err(ConfigurationReadError::Io(error)) => return Err(error),
        }
    }

    write_atomic(&paths.primary, &paths.primary_temp, &document)
        .map_err(|error| format!("The service configuration could not be written: {error}"))
}

fn valid_backup_available(paths: &ConfigurationPaths) -> bool {
    read_configuration(&paths.backup).is_ok()
}

fn write_atomic(destination: &Path, temporary: &Path, contents: &[u8]) -> io::Result<()> {
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        atomic_replace(temporary, destination)
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(temporary);
    }

    write_result
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    };

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    const TEST_SEED: &str = r#"{
      "version": 1,
      "services": [{
        "id": "jellyfin",
        "name": "Jellyfin",
        "url": "http://192.168.1.10:8096",
        "icon": "jellyfin",
        "category": "Media",
        "enabled": true,
        "accent": "violet"
      }]
    }"#;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "personal-hub-service-settings-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn configured_name(configuration: &ServiceConfiguration) -> &str {
        &configuration.services[0].name
    }

    fn renamed_configuration(name: &str) -> ServiceConfiguration {
        let mut configuration = ServiceConfiguration::parse_document(TEST_SEED).unwrap();
        configuration.services[0].name = name.into();
        configuration
    }

    fn empty_configuration() -> ServiceConfiguration {
        ServiceConfiguration {
            version: CONFIGURATION_VERSION,
            services: Vec::new(),
        }
    }

    #[test]
    fn first_run_persists_the_bundled_seed_without_creating_a_backup() {
        let directory = TestDirectory::new();
        let settings = ServiceSettings::initialize(&directory.0, TEST_SEED).unwrap();
        let paths = ConfigurationPaths::new(&directory.0);

        assert_eq!(
            configured_name(&settings.configuration().unwrap()),
            "Jellyfin"
        );
        assert!(paths.primary.is_file());
        assert!(!paths.backup.exists());
        assert_eq!(settings.response().unwrap().recovery_notice, None);
        assert!(!settings.response().unwrap().backup_available);
    }

    #[test]
    fn invalid_saved_configuration_recovers_to_seed_and_preserves_valid_backup() {
        let directory = TestDirectory::new();
        let paths = ConfigurationPaths::new(&directory.0);
        fs::write(&paths.primary, b"{ definitely not json").unwrap();
        fs::write(&paths.backup, TEST_SEED.as_bytes()).unwrap();

        let settings = ServiceSettings::initialize(&directory.0, TEST_SEED).unwrap();
        let response = settings.response().unwrap();

        assert_eq!(configured_name(&response.configuration), "Jellyfin");
        assert_eq!(
            response.recovery_notice.as_deref(),
            Some(INVALID_CONFIGURATION_NOTICE)
        );
        assert!(response.backup_available);
        assert!(read_configuration(&paths.primary).is_ok());
    }

    #[test]
    fn backup_restore_swaps_the_saved_and_previous_working_configurations() {
        let directory = TestDirectory::new();
        let settings = ServiceSettings::initialize(&directory.0, TEST_SEED).unwrap();
        let catalog =
            ServiceCatalog::from_configuration(&settings.configuration().unwrap()).unwrap();

        settings
            .save(renamed_configuration("Media One"), &catalog)
            .unwrap();
        settings
            .save(renamed_configuration("Media Two"), &catalog)
            .unwrap();
        let restored = settings.restore_backup(&catalog).unwrap();

        assert_eq!(configured_name(&restored.configuration), "Media One");
        assert_eq!(
            configured_name(&read_configuration(&settings.paths.backup).unwrap()),
            "Media Two"
        );
    }

    #[test]
    fn validation_rejects_unknown_secret_fields_credentials_and_unsafe_tls_scope() {
        let secret_field = TEST_SEED.replace(
            "\"enabled\": true",
            "\"enabled\": true, \"apiKey\": \"must-not-be-stored\"",
        );
        assert!(ServiceConfiguration::parse_document(&secret_field).is_err());

        let credential_url = TEST_SEED.replace(
            "http://192.168.1.10:8096",
            "http://admin:secret@192.168.1.10:8096",
        );
        assert!(ServiceConfiguration::parse_document(&credential_url).is_err());

        let public_tls_exception = TEST_SEED
            .replace("http://192.168.1.10:8096", "https://example.com")
            .replace(
                "\"accent\": \"violet\"",
                "\"accent\": \"violet\", \"tlsPolicy\": \"allow-invalid-local-certificate\"",
            );
        assert!(ServiceConfiguration::parse_document(&public_tls_exception).is_err());
    }

    #[test]
    fn validation_normalizes_optional_defaults_and_edge_whitespace() {
        let document = TEST_SEED
            .replace("\"Jellyfin\"", "\"  Jellyfin  \"")
            .replace(",\n        \"accent\": \"violet\"", "");
        let configuration = ServiceConfiguration::parse_document(&document).unwrap();
        let service = &configuration.services[0];

        assert_eq!(service.name, "Jellyfin");
        assert_eq!(service.description, "Local service");
        assert_eq!(service.accent, ServiceAccent::Slate);
        assert_eq!(service.tls_policy, ServiceTlsPolicy::Strict);
        assert_eq!(service.authentication, ServiceAuthentication::default());
    }

    #[test]
    fn authentication_is_explicit_allowlisted_and_private_http_requires_opt_in() {
        let private_http = TEST_SEED.replace(
            "\"accent\": \"violet\"",
            "\"accent\": \"violet\", \"authentication\": { \"api\": \"homarr-api-key\", \"allowInsecureLocalHttp\": true }",
        );
        let configuration = ServiceConfiguration::parse_document(&private_http).unwrap();
        assert_eq!(
            configuration.services[0].authentication,
            ServiceAuthentication {
                api: ServiceApiAuthentication::HomarrApiKey,
                browser: ServiceBrowserAuthentication::None,
                allow_insecure_local_http: true,
            }
        );

        let unknown_adapter = private_http.replace("homarr-api-key", "arbitrary-header");
        assert!(ServiceConfiguration::parse_document(&unknown_adapter).is_err());

        let public_http = private_http.replace("192.168.1.10", "example.com");
        assert!(ServiceConfiguration::parse_document(&public_http).is_err());

        let conflicting_authorization = TEST_SEED.replace(
            "\"accent\": \"violet\"",
            "\"accent\": \"violet\", \"authentication\": { \"api\": \"glances-bearer\", \"browser\": \"http-basic\" }",
        );
        assert!(ServiceConfiguration::parse_document(&conflicting_authorization).is_err());
    }

    #[test]
    fn https_authentication_drops_an_irrelevant_plaintext_opt_in() {
        let document = TEST_SEED
            .replace("http://192.168.1.10:8096", "https://192.168.1.10:8096")
            .replace(
                "\"accent\": \"violet\"",
                "\"accent\": \"violet\", \"authentication\": { \"browser\": \"http-basic\", \"allowInsecureLocalHttp\": true }",
            );
        let configuration = ServiceConfiguration::parse_document(&document).unwrap();

        assert_eq!(
            configuration.services[0].authentication.browser,
            ServiceBrowserAuthentication::HttpBasic
        );
        assert!(
            !configuration.services[0]
                .authentication
                .allow_insecure_local_http
        );
    }

    #[test]
    fn response_uses_the_safe_camel_case_command_contract() {
        let response = ServiceConfigurationResponse {
            configuration: ServiceConfiguration::parse_document(TEST_SEED).unwrap(),
            recovery_notice: Some("Recovered defaults".into()),
            backup_available: true,
        };
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["configuration"]["version"], 1);
        assert!(value["configuration"].get("$schema").is_none());
        assert_eq!(value["configuration"]["services"][0]["tlsPolicy"], "strict");
        assert!(value["configuration"]["services"][0]
            .get("authentication")
            .is_none());
        assert_eq!(value["recoveryNotice"], "Recovered defaults");
        assert_eq!(value["backupAvailable"], true);
    }

    #[test]
    fn removal_journal_is_retried_after_commit_without_using_the_real_vault() {
        let directory = TestDirectory::new();
        let mut startup_delete = |_: &str| Ok(());
        let settings =
            ServiceSettings::initialize_with_cleanup(&directory.0, TEST_SEED, &mut startup_delete)
                .unwrap();
        let catalog =
            ServiceCatalog::from_configuration(&settings.configuration().unwrap()).unwrap();
        let mut deleted_ids = Vec::new();
        let mut delete = |service_id: &str| {
            deleted_ids.push(service_id.to_string());
            Ok(())
        };

        let response = settings
            .commit_with_cleanup(empty_configuration(), &catalog, true, &mut delete)
            .unwrap();

        assert_eq!(deleted_ids, ["jellyfin"]);
        assert!(response.configuration.services.is_empty());
        assert_eq!(response.recovery_notice, None);
        assert!(read_pending_credential_cleanup_journal(&settings.paths)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn catalog_change_is_finalized_before_failed_post_commit_cleanup() {
        let directory = TestDirectory::new();
        let mut startup_delete = |_: &str| Ok(());
        let settings =
            ServiceSettings::initialize_with_cleanup(&directory.0, TEST_SEED, &mut startup_delete)
                .unwrap();
        let catalog =
            ServiceCatalog::from_configuration(&settings.configuration().unwrap()).unwrap();
        let finalized = Arc::new(AtomicBool::new(false));
        let cleanup_observed_finalized = Arc::clone(&finalized);
        let mut delete = move |_: &str| {
            assert!(cleanup_observed_finalized.load(Ordering::SeqCst));
            Err("injected deletion failure".to_string())
        };
        let finalizer_flag = Arc::clone(&finalized);

        let response = settings
            .commit_with_cleanup_and_catalog_change(
                empty_configuration(),
                &catalog,
                true,
                &mut delete,
                move || {
                    finalizer_flag.store(true, Ordering::SeqCst);
                    vec!["Catalog consumers reconciled.".to_string()]
                },
            )
            .unwrap();

        assert!(finalized.load(Ordering::SeqCst));
        assert!(response.configuration.services.is_empty());
        let notice = response.recovery_notice.unwrap();
        assert!(notice.contains(PENDING_CREDENTIAL_CLEANUP_NOTICE));
        assert!(notice.contains("Catalog consumers reconciled."));
        assert_eq!(
            read_pending_credential_cleanup_journal(&settings.paths).unwrap(),
            ["jellyfin".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn active_service_drops_prepared_journal_entry_without_deleting_its_credentials() {
        let directory = TestDirectory::new();
        let paths = ConfigurationPaths::new(&directory.0);
        persist_configuration(
            &paths,
            &ServiceConfiguration::parse_document(TEST_SEED).unwrap(),
            false,
        )
        .unwrap();
        persist_pending_credential_cleanup_journal(
            &paths,
            &["jellyfin".to_string()].into_iter().collect(),
        )
        .unwrap();
        let mut attempted = Vec::new();
        let mut delete = |service_id: &str| {
            attempted.push(service_id.to_string());
            Err("injected deletion failure".into())
        };

        let settings =
            ServiceSettings::initialize_with_cleanup(&directory.0, TEST_SEED, &mut delete).unwrap();

        assert!(attempted.is_empty());
        assert!(read_pending_credential_cleanup_journal(&settings.paths)
            .unwrap()
            .is_empty());
        assert_eq!(settings.response().unwrap().recovery_notice, None);
    }

    #[test]
    fn pending_cleanup_failure_blocks_readding_the_same_service() {
        let directory = TestDirectory::new();
        let paths = ConfigurationPaths::new(&directory.0);
        persist_configuration(&paths, &empty_configuration(), false).unwrap();
        persist_pending_credential_cleanup_journal(
            &paths,
            &["jellyfin".to_string()].into_iter().collect(),
        )
        .unwrap();
        let mut failed_attempts = Vec::new();
        let mut always_fail = |service_id: &str| {
            failed_attempts.push(service_id.to_string());
            Err("injected deletion failure".into())
        };
        let settings = ServiceSettings::initialize_with_cleanup(
            &directory.0,
            r#"{ "version": 1, "services": [] }"#,
            &mut always_fail,
        )
        .unwrap();
        let catalog =
            ServiceCatalog::from_configuration(&settings.configuration().unwrap()).unwrap();

        let error = settings
            .commit_with_cleanup(
                ServiceConfiguration::parse_document(TEST_SEED).unwrap(),
                &catalog,
                true,
                &mut always_fail,
            )
            .unwrap_err();

        assert!(error.contains("cannot be re-added"));
        assert_eq!(settings.configuration().unwrap(), empty_configuration());
        assert_eq!(failed_attempts, ["jellyfin", "jellyfin"]);
        assert_eq!(
            read_pending_credential_cleanup_journal(&settings.paths).unwrap(),
            ["jellyfin".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn pending_journal_rejects_malformed_or_unsafe_service_ids() {
        let directory = TestDirectory::new();
        let paths = ConfigurationPaths::new(&directory.0);
        fs::write(
            &paths.pending_credential_cleanup,
            br#"{ "version": 1, "pendingServiceIds": ["Not-safe"] }"#,
        )
        .unwrap();

        assert!(read_pending_credential_cleanup_journal(&paths).is_err());
    }
}
