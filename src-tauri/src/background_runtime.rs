use crate::{
    appearance_settings,
    desktop_widgets::{DesktopWidgetBroker, DesktopWidgetKind},
    service_webviews::{
        resume_service_webviews, suspend_and_close_all_service_webviews, ServiceCatalog,
        ServiceWebviewRegistry,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, RwLock,
    },
    time::Duration,
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::Color,
    App, AppHandle, Emitter, Manager, Webview, WebviewUrl, WebviewWindowBuilder,
};

const DOCUMENT_VERSION: u8 = 1;
const PREFERENCES_VERSION: u8 = 1;
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024;
const DOCUMENT_FILE_NAME: &str = "background-runtime.json";
const DOCUMENT_TEMP_FILE_NAME: &str = "background-runtime.json.tmp";
const INVALID_DOCUMENT_NOTICE: &str =
    "The saved background settings were invalid and were safely disabled.";
const MAIN_WEBVIEW_LABEL: &str = "main";
const TRAY_ID: &str = "personal-hub-runtime";
const EVENT_RUNTIME_CHANGED: &str = "personal-hub://background-runtime-changed";
const EVENT_OPEN_SETTINGS: &str = "personal-hub://open-settings";
const EVENT_MAIN_RESUMED: &str = "personal-hub://main-resumed";

const MENU_OPEN: &str = "runtime-open";
const MENU_SETTINGS: &str = "runtime-settings";
const MENU_EXPERIMENTAL: &str = "runtime-experimental";
const MENU_SERVER: &str = "runtime-card-server";
const MENU_STORAGE: &str = "runtime-card-storage";
const MENU_SERVICES: &str = "runtime-card-services";
const MENU_RESET_GEOMETRY: &str = "runtime-reset-geometry";
const MENU_QUIT: &str = "runtime-quit";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopCardPreferences {
    server: bool,
    storage: bool,
    services: bool,
}

impl Default for DesktopCardPreferences {
    fn default() -> Self {
        Self {
            server: true,
            storage: true,
            services: true,
        }
    }
}

impl DesktopCardPreferences {
    fn selected(&self, kind: DesktopWidgetKind) -> bool {
        match kind {
            DesktopWidgetKind::Server => self.server,
            DesktopWidgetKind::Storage => self.storage,
            DesktopWidgetKind::Services => self.services,
        }
    }

    fn set_selected(&mut self, kind: DesktopWidgetKind, selected: bool) {
        match kind {
            DesktopWidgetKind::Server => self.server = selected,
            DesktopWidgetKind::Storage => self.storage = selected,
            DesktopWidgetKind::Services => self.services = selected,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackgroundRuntimePreferences {
    version: u8,
    experimental_desktop_cards: bool,
    close_to_tray: bool,
    cards: DesktopCardPreferences,
}

impl Default for BackgroundRuntimePreferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            experimental_desktop_cards: false,
            close_to_tray: true,
            cards: DesktopCardPreferences::default(),
        }
    }
}

impl BackgroundRuntimePreferences {
    fn normalized(&self) -> Result<Self, String> {
        if self.version != PREFERENCES_VERSION {
            return Err("background runtime preferences.version must be 1.".into());
        }

        Ok(self.clone())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeometryRevisions {
    server: u64,
    storage: u64,
    services: u64,
}

impl GeometryRevisions {
    fn get(&self, kind: DesktopWidgetKind) -> u64 {
        match kind {
            DesktopWidgetKind::Server => self.server,
            DesktopWidgetKind::Storage => self.storage,
            DesktopWidgetKind::Services => self.services,
        }
    }

    fn increment_all(&mut self) -> Result<(), String> {
        self.server = self
            .server
            .checked_add(1)
            .ok_or_else(|| "The Server card geometry revision is exhausted.".to_string())?;
        self.storage = self
            .storage
            .checked_add(1)
            .ok_or_else(|| "The Storage card geometry revision is exhausted.".to_string())?;
        self.services = self.services.checked_add(1).ok_or_else(|| {
            "The Service-attention card geometry revision is exhausted.".to_string()
        })?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackgroundRuntimeDocument {
    version: u8,
    revision: u64,
    preferences: BackgroundRuntimePreferences,
    geometry_revisions: GeometryRevisions,
}

impl Default for BackgroundRuntimeDocument {
    fn default() -> Self {
        Self {
            version: DOCUMENT_VERSION,
            revision: 0,
            preferences: BackgroundRuntimePreferences::default(),
            geometry_revisions: GeometryRevisions::default(),
        }
    }
}

impl BackgroundRuntimeDocument {
    fn normalized(&self) -> Result<Self, String> {
        if self.version != DOCUMENT_VERSION {
            return Err("The background runtime document version is unsupported.".into());
        }

        Ok(Self {
            version: DOCUMENT_VERSION,
            revision: self.revision,
            preferences: self.preferences.normalized()?,
            geometry_revisions: self.geometry_revisions.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopCardAvailability {
    Available,
    GlancesNotConfigured,
    NoHealthTargets,
}

impl DesktopCardAvailability {
    fn is_available(self) -> bool {
        self == Self::Available
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCardAvailabilityMap {
    server: DesktopCardAvailability,
    storage: DesktopCardAvailability,
    services: DesktopCardAvailability,
}

impl DesktopCardAvailabilityMap {
    fn from_catalog(catalog: &ServiceCatalog) -> Self {
        let glances = if catalog.resolve_endpoint("glances").is_ok() {
            DesktopCardAvailability::Available
        } else {
            DesktopCardAvailability::GlancesNotConfigured
        };
        let services = if catalog.enabled_health_targets().is_empty() {
            DesktopCardAvailability::NoHealthTargets
        } else {
            DesktopCardAvailability::Available
        };

        Self {
            server: glances,
            storage: glances,
            services,
        }
    }

    fn get(&self, kind: DesktopWidgetKind) -> DesktopCardAvailability {
        match kind {
            DesktopWidgetKind::Server => self.server,
            DesktopWidgetKind::Storage => self.storage,
            DesktopWidgetKind::Services => self.services,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundRuntimeSnapshot {
    preferences: BackgroundRuntimePreferences,
    revision: String,
    availability: DesktopCardAvailabilityMap,
    tray_available: bool,
    recovery_notice: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveBackgroundRuntimePreferencesRequest {
    preferences: BackgroundRuntimePreferences,
    expected_revision: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWidgetRuntimeState {
    geometry_revision: String,
}

pub struct BackgroundRuntimeSettings {
    primary_path: PathBuf,
    temporary_path: PathBuf,
    document: RwLock<BackgroundRuntimeDocument>,
    recovery_notice: RwLock<Option<String>>,
    operation: Mutex<()>,
    runtime_operation: Mutex<()>,
    close_to_tray_in_progress: AtomicBool,
    tray_available: AtomicBool,
}

impl BackgroundRuntimeSettings {
    pub fn initialize(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&directory).map_err(|error| {
            format!("The background settings directory is unavailable: {error}")
        })?;
        let primary_path = directory.join(DOCUMENT_FILE_NAME);
        let temporary_path = directory.join(DOCUMENT_TEMP_FILE_NAME);
        let (document, recovery_notice) = if primary_path.exists() {
            match read_document(&primary_path) {
                Ok(document) => (document, None),
                Err(_) => (
                    BackgroundRuntimeDocument::default(),
                    Some(INVALID_DOCUMENT_NOTICE.to_string()),
                ),
            }
        } else {
            (BackgroundRuntimeDocument::default(), None)
        };

        Ok(Self {
            primary_path,
            temporary_path,
            document: RwLock::new(document),
            recovery_notice: RwLock::new(recovery_notice),
            operation: Mutex::new(()),
            runtime_operation: Mutex::new(()),
            close_to_tray_in_progress: AtomicBool::new(false),
            tray_available: AtomicBool::new(false),
        })
    }

    fn document(&self) -> Result<BackgroundRuntimeDocument, String> {
        self.document
            .read()
            .map(|document| document.clone())
            .map_err(|_| "The background settings state is unavailable.".to_string())
    }

    fn recovery_notice(&self) -> Result<Option<String>, String> {
        self.recovery_notice
            .read()
            .map(|notice| notice.clone())
            .map_err(|_| "The background settings recovery state is unavailable.".to_string())
    }

    fn mutate(
        &self,
        mutation: impl FnOnce(&mut BackgroundRuntimeDocument) -> Result<(), String>,
    ) -> Result<(BackgroundRuntimeDocument, BackgroundRuntimeDocument), String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "Another background settings operation did not finish.".to_string())?;
        let _document_write_lock = acquire_document_write_lock()?;
        let cached = self.document()?;
        let previous = if self.primary_path.exists() {
            match read_document(&self.primary_path) {
                Ok(document) => {
                    if document != cached {
                        *self.document.write().map_err(|_| {
                            "The background settings state is unavailable.".to_string()
                        })? = document.clone();
                    }
                    *self.recovery_notice.write().map_err(|_| {
                        "The background settings recovery state is unavailable.".to_string()
                    })? = None;
                    document
                }
                Err(_) if self.recovery_notice()?.is_some() => cached,
                Err(error) => return Err(error),
            }
        } else {
            cached
        };
        let mut next = previous.clone();
        mutation(&mut next)?;
        next = next.normalized()?;

        let is_unchanged = next.preferences == previous.preferences
            && next.geometry_revisions == previous.geometry_revisions;
        let needs_recovery_write = self.recovery_notice()?.is_some();
        if is_unchanged && !needs_recovery_write {
            return Ok((previous.clone(), previous));
        }

        next.revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| "The background settings revision is exhausted.".to_string())?;
        persist_document(&self.primary_path, &self.temporary_path, &next)?;
        *self
            .document
            .write()
            .map_err(|_| "The background settings state is unavailable.".to_string())? =
            next.clone();
        *self
            .recovery_notice
            .write()
            .map_err(|_| "The background settings recovery state is unavailable.".to_string())? =
            None;
        Ok((previous, next))
    }

    fn save_preferences(
        &self,
        request: SaveBackgroundRuntimePreferencesRequest,
    ) -> Result<(BackgroundRuntimeDocument, BackgroundRuntimeDocument), String> {
        let expected_revision = request.expected_revision;
        let preferences = request.preferences.normalized()?;
        self.mutate(move |document| {
            if expected_revision != document.revision.to_string() {
                return Err(
                    "Background settings changed in another surface. Reload them and try again."
                        .into(),
                );
            }
            document.preferences = preferences;
            Ok(())
        })
    }

    fn begin_close_to_tray(&self) -> bool {
        self.close_to_tray_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn end_close_to_tray(&self) {
        self.close_to_tray_in_progress
            .store(false, Ordering::Release);
    }

    fn close_to_tray_in_progress(&self) -> bool {
        self.close_to_tray_in_progress.load(Ordering::Acquire)
    }

    fn set_tray_available(&self, available: bool) {
        self.tray_available.store(available, Ordering::Release);
    }

    fn tray_available(&self) -> bool {
        self.tray_available.load(Ordering::Acquire)
    }

    pub(crate) fn should_close_to_tray(&self, catalog: &ServiceCatalog) -> Result<bool, String> {
        let document = self.document()?;
        let availability = DesktopCardAvailabilityMap::from_catalog(catalog);
        Ok(self.tray_available()
            && document.preferences.close_to_tray
            && DesktopWidgetKind::all()
                .into_iter()
                .any(|kind| effective_card_enabled(&document.preferences, &availability, kind)))
    }
}

impl DesktopWidgetKind {
    fn all() -> [Self; 3] {
        [Self::Server, Self::Storage, Self::Services]
    }
}

fn effective_card_enabled(
    preferences: &BackgroundRuntimePreferences,
    availability: &DesktopCardAvailabilityMap,
    kind: DesktopWidgetKind,
) -> bool {
    preferences.experimental_desktop_cards
        && preferences.cards.selected(kind)
        && availability.get(kind).is_available()
}

fn snapshot(
    settings: &BackgroundRuntimeSettings,
    catalog: &ServiceCatalog,
) -> Result<BackgroundRuntimeSnapshot, String> {
    let document = settings.document()?;
    Ok(BackgroundRuntimeSnapshot {
        preferences: document.preferences,
        revision: document.revision.to_string(),
        availability: DesktopCardAvailabilityMap::from_catalog(catalog),
        tray_available: settings.tray_available(),
        recovery_notice: settings.recovery_notice()?,
    })
}

#[tauri::command]
pub fn get_background_runtime_preferences(
    caller: Webview,
    settings: tauri::State<'_, BackgroundRuntimeSettings>,
    catalog: tauri::State<'_, ServiceCatalog>,
) -> Result<BackgroundRuntimeSnapshot, String> {
    authorize_main(caller.label())?;
    snapshot(&settings, &catalog)
}

#[tauri::command]
pub async fn save_background_runtime_preferences(
    caller: Webview,
    app: AppHandle,
    request: SaveBackgroundRuntimePreferencesRequest,
    settings: tauri::State<'_, BackgroundRuntimeSettings>,
) -> Result<BackgroundRuntimeSnapshot, String> {
    authorize_main(caller.label())?;
    let (previous, next) = settings.save_preferences(request)?;
    let force_recreate = runtime_recreate_changes(&previous, &next);
    finish_runtime_change(&app, &force_recreate)
}

#[tauri::command]
pub fn get_desktop_widget_runtime_state(
    caller: Webview,
    kind: DesktopWidgetKind,
    settings: tauri::State<'_, BackgroundRuntimeSettings>,
) -> Result<DesktopWidgetRuntimeState, String> {
    authorize_widget(caller.label(), kind)?;
    let revision = settings.document()?.geometry_revisions.get(kind);
    Ok(DesktopWidgetRuntimeState {
        geometry_revision: revision.to_string(),
    })
}

#[tauri::command]
pub async fn disable_desktop_widget(
    caller: Webview,
    kind: DesktopWidgetKind,
    app: AppHandle,
    settings: tauri::State<'_, BackgroundRuntimeSettings>,
    broker: tauri::State<'_, DesktopWidgetBroker>,
    catalog: tauri::State<'_, ServiceCatalog>,
) -> Result<(), String> {
    authorize_widget(caller.label(), kind)?;
    settings.mutate(|document| {
        document.preferences.cards.set_selected(kind, false);
        Ok(())
    })?;
    broker.deactivate(kind)?;
    let menu_result = refresh_tray_menu(&app);
    let response = snapshot(&settings, &catalog)?;
    emit_runtime_changed(&app, &response);

    // Let the command response reach the card before its own WebView is
    // destroyed. The broker claim is already gone, so this brief grace period
    // performs no monitoring work.
    let close_app = app.clone();
    let spawn_result = std::thread::Builder::new()
        .name(format!("desktop-card-disable-{}", kind.slug()))
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            // Re-read the latest persisted state under the runtime-operation
            // lock instead of destroying whichever window currently owns this
            // label. The card may have been re-enabled from Settings or the
            // tray while this command response was in flight.
            if let Err(error) = reconcile_card_windows(&close_app, &[]) {
                eprintln!("The disabled desktop card could not be reconciled: {error}");
            }
        });
    if let Err(error) = spawn_result {
        if let Some(window) = app.get_webview_window(kind.window_label()) {
            let _ = window.hide();
        }
        return Err(format!(
            "The disabled desktop card could not be released: {error}"
        ));
    }

    menu_result?;
    Ok(())
}

fn authorize_main(caller_label: &str) -> Result<(), String> {
    if caller_label == MAIN_WEBVIEW_LABEL {
        Ok(())
    } else {
        Err("Background settings are available only to the trusted main window.".into())
    }
}

fn authorize_widget(caller_label: &str, kind: DesktopWidgetKind) -> Result<(), String> {
    if caller_label == kind.window_label() {
        Ok(())
    } else {
        Err("A desktop card can change only its own trusted runtime state.".into())
    }
}

fn finish_runtime_change(
    app: &AppHandle,
    force_recreate: &[DesktopWidgetKind],
) -> Result<BackgroundRuntimeSnapshot, String> {
    let runtime_result = reconcile_card_windows(app, force_recreate);
    let menu_result = refresh_tray_menu(app);
    let settings = app.state::<BackgroundRuntimeSettings>();
    let catalog = app.state::<ServiceCatalog>();
    let response = snapshot(&settings, &catalog)?;
    emit_runtime_changed(app, &response);
    runtime_result?;
    menu_result?;
    Ok(response)
}

fn emit_runtime_changed(app: &AppHandle, response: &BackgroundRuntimeSnapshot) {
    if let Some(main) = app.get_webview_window(MAIN_WEBVIEW_LABEL) {
        let _ = main.emit(EVENT_RUNTIME_CHANGED, response);
    }
}

fn geometry_changes(
    previous: &BackgroundRuntimeDocument,
    next: &BackgroundRuntimeDocument,
) -> Vec<DesktopWidgetKind> {
    DesktopWidgetKind::all()
        .into_iter()
        .filter(|kind| previous.geometry_revisions.get(*kind) != next.geometry_revisions.get(*kind))
        .collect()
}

fn runtime_recreate_changes(
    previous: &BackgroundRuntimeDocument,
    next: &BackgroundRuntimeDocument,
) -> Vec<DesktopWidgetKind> {
    let mut changes = geometry_changes(previous, next);
    for kind in DesktopWidgetKind::all() {
        let was_selected = previous.preferences.experimental_desktop_cards
            && previous.preferences.cards.selected(kind);
        let is_selected =
            next.preferences.experimental_desktop_cards && next.preferences.cards.selected(kind);
        if !was_selected && is_selected && !changes.contains(&kind) {
            // A card can still own its native window during the short
            // self-disable response grace period. Recreating it on a rapid
            // re-enable guarantees that its frontend remounts and reclaims
            // the polling broker instead of leaving a frozen visible card.
            changes.push(kind);
        }
    }
    changes
}

fn reconcile_card_windows(
    app: &AppHandle,
    force_recreate: &[DesktopWidgetKind],
) -> Result<(), String> {
    let settings = app.state::<BackgroundRuntimeSettings>();
    let _runtime_operation = settings
        .runtime_operation
        .lock()
        .map_err(|_| "The desktop card window manager is unavailable.".to_string())?;
    let document = settings.document()?;
    let catalog = app.state::<ServiceCatalog>();
    let availability = DesktopCardAvailabilityMap::from_catalog(&catalog);
    let broker = app.state::<DesktopWidgetBroker>();
    let mut failures = Vec::new();

    for kind in DesktopWidgetKind::all() {
        let desired = effective_card_enabled(&document.preferences, &availability, kind);
        let recreate = force_recreate.contains(&kind);
        let existing = app.get_webview_window(kind.window_label());

        if !desired || recreate {
            if let Err(error) = broker.deactivate(kind) {
                failures.push(error);
            }
            if let Some(window) = existing {
                if let Err(error) = window.destroy() {
                    let _ = window.hide();
                    failures.push(format!(
                        "The {} desktop card could not be closed: {error}",
                        kind.slug()
                    ));
                    continue;
                }
            }
        }

        if desired && app.get_webview_window(kind.window_label()).is_none() {
            if let Err(error) = create_card_window(app, kind) {
                failures.push(error);
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join(" "))
    }
}

struct CardWindowSpec {
    title: &'static str,
    width: f64,
    height: f64,
    min_width: f64,
    min_height: f64,
}

fn card_window_spec(kind: DesktopWidgetKind) -> CardWindowSpec {
    match kind {
        DesktopWidgetKind::Server => CardWindowSpec {
            title: "Personal Hub — Server",
            width: 420.0,
            height: 380.0,
            min_width: 320.0,
            min_height: 300.0,
        },
        DesktopWidgetKind::Storage => CardWindowSpec {
            title: "Personal Hub — Storage",
            width: 400.0,
            height: 320.0,
            min_width: 320.0,
            min_height: 240.0,
        },
        DesktopWidgetKind::Services => CardWindowSpec {
            title: "Personal Hub — Service alerts",
            width: 400.0,
            height: 340.0,
            min_width: 320.0,
            min_height: 260.0,
        },
    }
}

fn create_card_window(app: &AppHandle, kind: DesktopWidgetKind) -> Result<(), String> {
    let spec = card_window_spec(kind);
    let url = format!("index.html#desktop-widget/{}", kind.slug());
    let native_theme = appearance_settings::current_native_theme(app)?;
    let window = WebviewWindowBuilder::new(app, kind.window_label(), WebviewUrl::App(url.into()))
        .title(spec.title)
        .inner_size(spec.width, spec.height)
        .min_inner_size(spec.min_width, spec.min_height)
        .visible(false)
        .focused(false)
        .focusable(true)
        .resizable(true)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(false)
        .always_on_bottom(true)
        .skip_taskbar(true)
        .prevent_overflow()
        .disable_drag_drop_handler()
        .theme(native_theme)
        .background_color(Color(0, 0, 0, 0))
        .build()
        .map_err(|error| {
            format!(
                "The {} desktop card could not be created: {error}",
                kind.slug()
            )
        })?;

    if let Err(error) = appearance_settings::apply_current_appearance(app) {
        let _ = window.destroy();
        return Err(format!(
            "The {} desktop card appearance could not be applied: {error}",
            kind.slug()
        ));
    }

    let cleanup_app = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = cleanup_app.state::<DesktopWidgetBroker>().deactivate(kind);
            // Window destruction is asynchronous on Tauri. A newer Settings,
            // tray or catalog mutation may already want this card back by the
            // time the old native window actually exits. Reconcile from the
            // Destroyed boundary so that latest persisted state wins.
            schedule_catalog_reconcile(cleanup_app.clone());
        }
    });
    Ok(())
}

fn build_tray_menu(app: &AppHandle) -> Result<Menu<tauri::Wry>, String> {
    let settings = app.state::<BackgroundRuntimeSettings>();
    let document = settings.document()?;
    let catalog = app.state::<ServiceCatalog>();
    let availability = DesktopCardAvailabilityMap::from_catalog(&catalog);
    let master = document.preferences.experimental_desktop_cards;

    let open = MenuItem::with_id(app, MENU_OPEN, "Open Personal Hub", true, None::<&str>)
        .map_err(menu_error)?;
    let open_settings = MenuItem::with_id(app, MENU_SETTINGS, "Settings", true, None::<&str>)
        .map_err(menu_error)?;
    let separator_one = PredefinedMenuItem::separator(app).map_err(menu_error)?;
    let experimental = CheckMenuItem::with_id(
        app,
        MENU_EXPERIMENTAL,
        "Experimental desktop cards",
        true,
        master,
        None::<&str>,
    )
    .map_err(menu_error)?;
    let server = CheckMenuItem::with_id(
        app,
        MENU_SERVER,
        "Server card",
        master && availability.server.is_available(),
        master && document.preferences.cards.server,
        None::<&str>,
    )
    .map_err(menu_error)?;
    let storage = CheckMenuItem::with_id(
        app,
        MENU_STORAGE,
        "Storage card",
        master && availability.storage.is_available(),
        master && document.preferences.cards.storage,
        None::<&str>,
    )
    .map_err(menu_error)?;
    let services = CheckMenuItem::with_id(
        app,
        MENU_SERVICES,
        "Service-attention card",
        master && availability.services.is_available(),
        master && document.preferences.cards.services,
        None::<&str>,
    )
    .map_err(menu_error)?;
    let reset_geometry = MenuItem::with_id(
        app,
        MENU_RESET_GEOMETRY,
        "Reset card positions",
        master,
        None::<&str>,
    )
    .map_err(menu_error)?;
    let separator_two = PredefinedMenuItem::separator(app).map_err(menu_error)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>).map_err(menu_error)?;

    Menu::with_items(
        app,
        &[
            &open,
            &open_settings,
            &separator_one,
            &experimental,
            &server,
            &storage,
            &services,
            &reset_geometry,
            &separator_two,
            &quit,
        ],
    )
    .map_err(menu_error)
}

fn menu_error(error: tauri::Error) -> String {
    format!("The Personal Hub tray menu is unavailable: {error}")
}

fn refresh_tray_menu(app: &AppHandle) -> Result<(), String> {
    if !app.state::<BackgroundRuntimeSettings>().tray_available() {
        return Ok(());
    }
    let menu = build_tray_menu(app)?;
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| "The Personal Hub tray icon is unavailable.".to_string())?;
    tray.set_menu(Some(menu)).map_err(menu_error)
}

pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let tray_result = (|| -> Result<(), String> {
        let menu = build_tray_menu(app.handle())?;
        let mut tray = TrayIconBuilder::with_id(TRAY_ID)
            .menu(&menu)
            .tooltip("Personal Hub")
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| handle_tray_menu(app.clone(), event.id().as_ref()))
            .on_tray_icon_event(|tray, event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    let _ = show_main_window(tray.app_handle(), false);
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.build(app).map_err(menu_error)?;
        Ok(())
    })();
    let runtime = app.state::<BackgroundRuntimeSettings>();
    runtime.set_tray_available(tray_result.is_ok());
    if let Err(error) = tray_result {
        // A tray failure must not make the core dashboard unavailable. With
        // this flag cleared the close policy exits normally, so the app can
        // never become an unreachable hidden process.
        eprintln!("Personal Hub started without its tray integration: {error}");
    }
    if let Err(error) = reconcile_card_windows(app.handle(), &[]) {
        // Experimental card failures must not prevent the main application
        // from starting; the desired state remains persisted for a later
        // Settings or startup retry.
        eprintln!("Desktop cards could not be restored during startup: {error}");
    }
    Ok(())
}

fn handle_tray_menu(app: AppHandle, menu_id: &str) {
    match menu_id {
        MENU_OPEN => {
            let _ = show_main_window(&app, false);
        }
        MENU_SETTINGS => {
            let _ = show_main_window(&app, true);
        }
        MENU_QUIT => {
            let quit_app_handle = app.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("personal-hub-tray-quit".into())
                .spawn(move || quit_app(&quit_app_handle))
            {
                eprintln!("Personal Hub could not start its quit worker: {error}");
                app.exit(0);
            }
        }
        MENU_EXPERIMENTAL | MENU_SERVER | MENU_STORAGE | MENU_SERVICES | MENU_RESET_GEOMETRY => {
            let menu_id = menu_id.to_string();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = mutate_from_tray(&app, &menu_id) {
                    eprintln!("Personal Hub tray action failed: {error}");
                }
            });
        }
        _ => {}
    }
}

fn mutate_from_tray(app: &AppHandle, menu_id: &str) -> Result<(), String> {
    let settings = app.state::<BackgroundRuntimeSettings>();
    let (previous, next) = settings.mutate(|document| {
        match menu_id {
            MENU_EXPERIMENTAL => {
                document.preferences.experimental_desktop_cards =
                    !document.preferences.experimental_desktop_cards;
            }
            MENU_SERVER => {
                document.preferences.cards.server = !document.preferences.cards.server;
            }
            MENU_STORAGE => {
                document.preferences.cards.storage = !document.preferences.cards.storage;
            }
            MENU_SERVICES => {
                document.preferences.cards.services = !document.preferences.cards.services;
            }
            MENU_RESET_GEOMETRY => document.geometry_revisions.increment_all()?,
            _ => return Err("The tray action is not recognized.".into()),
        }
        Ok(())
    })?;
    let force_recreate = runtime_recreate_changes(&previous, &next);
    finish_runtime_change(app, &force_recreate).map(|_| ())
}

fn show_main_window(app: &AppHandle, open_settings: bool) -> Result<(), String> {
    let resume_app = app.clone();
    std::thread::Builder::new()
        .name("personal-hub-tray-open".into())
        .spawn(move || {
            while resume_app
                .state::<BackgroundRuntimeSettings>()
                .close_to_tray_in_progress()
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Err(error) = show_main_window_now(&resume_app, open_settings) {
                eprintln!("Personal Hub could not reopen from the tray: {error}");
            }
        })
        .map_err(|error| format!("The tray open action could not be scheduled: {error}"))?;
    Ok(())
}

fn show_main_window_now(app: &AppHandle, open_settings: bool) -> Result<(), String> {
    let main = app
        .get_webview_window(MAIN_WEBVIEW_LABEL)
        .ok_or_else(|| "The Personal Hub main window is unavailable.".to_string())?;
    if main.is_minimized().unwrap_or(false) {
        main.unminimize()
            .map_err(|error| format!("The Personal Hub window could not be restored: {error}"))?;
    }
    main.show()
        .map_err(|error| format!("The Personal Hub window could not be shown: {error}"))?;
    let registry = app.state::<ServiceWebviewRegistry>();
    resume_service_webviews(&registry)?;
    let _ = main.emit(EVENT_MAIN_RESUMED, ());
    if open_settings {
        let _ = main.emit(EVENT_OPEN_SETTINGS, ());
    }
    main.set_focus()
        .map_err(|error| format!("The Personal Hub window could not be focused: {error}"))?;
    Ok(())
}

fn quit_app(app: &AppHandle) {
    let registry = app.state::<ServiceWebviewRegistry>();
    let _ = suspend_and_close_all_service_webviews(app, &registry);
    app.exit(0);
}

pub(crate) fn schedule_catalog_reconcile(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = reconcile_card_windows(&app, &[]) {
            eprintln!("Desktop cards could not be reconciled after a catalog change: {error}");
        }
        if let Err(error) = refresh_tray_menu(&app) {
            eprintln!("The tray menu could not be refreshed after a catalog change: {error}");
        }
        let settings = app.state::<BackgroundRuntimeSettings>();
        let catalog = app.state::<ServiceCatalog>();
        if let Ok(response) = snapshot(&settings, &catalog) {
            emit_runtime_changed(&app, &response);
        }
    });
}

pub(crate) fn begin_close_to_tray(settings: &BackgroundRuntimeSettings) -> bool {
    settings.begin_close_to_tray()
}

fn read_document(path: &Path) -> Result<BackgroundRuntimeDocument, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("The background settings file is unavailable: {error}"))?;
    let mut contents = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| format!("The background settings file could not be read: {error}"))?;
    if contents.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("The background settings file is too large.".into());
    }
    let document: BackgroundRuntimeDocument = serde_json::from_slice(&contents)
        .map_err(|_| "The background settings file has an invalid schema.".to_string())?;
    document.normalized()
}

fn persist_document(
    destination: &Path,
    temporary: &Path,
    document: &BackgroundRuntimeDocument,
) -> Result<(), String> {
    let mut contents = serde_json::to_vec_pretty(document)
        .map_err(|_| "The background settings could not be serialized.".to_string())?;
    contents.push(b'\n');
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(temporary)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        drop(file);
        atomic_replace(temporary, destination)
    })();

    if let Err(error) = write_result {
        let _ = fs::remove_file(temporary);
        return Err(format!(
            "The background settings could not be saved: {error}"
        ));
    }
    Ok(())
}

#[cfg(windows)]
struct DocumentWriteLock(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for DocumentWriteLock {
    fn drop(&mut self) {
        #[link(name = "Kernel32")]
        extern "system" {
            fn ReleaseMutex(mutex: *mut std::ffi::c_void) -> i32;
            fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
        }

        // SAFETY: the handle was returned by CreateMutexW and this guard is
        // the sole owner after a successful wait. Both calls are best-effort
        // cleanup during Drop.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn acquire_document_write_lock() -> Result<DocumentWriteLock, String> {
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_ABANDONED: u32 = 0x80;
    const WAIT_TIMEOUT: u32 = 0x102;
    const DOCUMENT_LOCK_TIMEOUT_MS: u32 = 5_000;

    #[link(name = "Kernel32")]
    extern "system" {
        fn CreateMutexW(
            attributes: *const std::ffi::c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
        fn CloseHandle(object: *mut std::ffi::c_void) -> i32;
    }

    let name = "Local\\PersonalHub.BackgroundRuntime.v1"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: the name is NUL-terminated and remains alive for the call;
    // default security attributes are requested and ownership is acquired by
    // the explicit bounded wait below.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(format!(
            "The background settings lock is unavailable: {}",
            io::Error::last_os_error()
        ));
    }

    // SAFETY: handle is a valid mutex handle returned above.
    let wait_result = unsafe { WaitForSingleObject(handle, DOCUMENT_LOCK_TIMEOUT_MS) };
    if wait_result == WAIT_OBJECT_0 || wait_result == WAIT_ABANDONED {
        return Ok(DocumentWriteLock(handle));
    }

    // SAFETY: the wait did not transfer mutex ownership, so only the handle
    // itself must be closed.
    unsafe {
        let _ = CloseHandle(handle);
    }
    if wait_result == WAIT_TIMEOUT {
        Err("Another Personal Hub process is updating background settings. Try again.".into())
    } else {
        Err(format!(
            "The background settings lock failed with native status {wait_result}."
        ))
    }
}

#[cfg(not(windows))]
struct DocumentWriteLock;

#[cfg(not(windows))]
fn acquire_document_write_lock() -> Result<DocumentWriteLock, String> {
    Ok(DocumentWriteLock)
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "personal-hub-background-runtime-{}-{sequence}",
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

    fn available() -> DesktopCardAvailabilityMap {
        DesktopCardAvailabilityMap {
            server: DesktopCardAvailability::Available,
            storage: DesktopCardAvailability::Available,
            services: DesktopCardAvailability::Available,
        }
    }

    #[test]
    fn first_run_is_opt_in_but_preselects_all_cards() {
        let directory = TestDirectory::new();
        let settings = BackgroundRuntimeSettings::initialize(directory.0.clone()).unwrap();
        let document = settings.document().unwrap();

        assert!(!document.preferences.experimental_desktop_cards);
        assert!(document.preferences.close_to_tray);
        assert!(document.preferences.cards.server);
        assert!(document.preferences.cards.storage);
        assert!(document.preferences.cards.services);
        assert!(DesktopWidgetKind::all()
            .into_iter()
            .all(|kind| { !effective_card_enabled(&document.preferences, &available(), kind) }));
    }

    #[test]
    fn unavailable_providers_never_become_effective() {
        let preferences = BackgroundRuntimePreferences {
            experimental_desktop_cards: true,
            ..BackgroundRuntimePreferences::default()
        };
        let availability = DesktopCardAvailabilityMap {
            server: DesktopCardAvailability::GlancesNotConfigured,
            storage: DesktopCardAvailability::GlancesNotConfigured,
            services: DesktopCardAvailability::Available,
        };

        assert!(!effective_card_enabled(
            &preferences,
            &availability,
            DesktopWidgetKind::Server
        ));
        assert!(!effective_card_enabled(
            &preferences,
            &availability,
            DesktopWidgetKind::Storage
        ));
        assert!(effective_card_enabled(
            &preferences,
            &availability,
            DesktopWidgetKind::Services
        ));
    }

    #[test]
    fn persisted_changes_are_atomic_and_revision_checked() {
        let directory = TestDirectory::new();
        let settings = BackgroundRuntimeSettings::initialize(directory.0.clone()).unwrap();
        let preferences = BackgroundRuntimePreferences {
            experimental_desktop_cards: true,
            ..BackgroundRuntimePreferences::default()
        };
        settings
            .save_preferences(SaveBackgroundRuntimePreferencesRequest {
                preferences: preferences.clone(),
                expected_revision: "0".into(),
            })
            .unwrap();

        let saved = read_document(&directory.0.join(DOCUMENT_FILE_NAME)).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.preferences, preferences);
        assert!(settings
            .save_preferences(SaveBackgroundRuntimePreferencesRequest {
                preferences: BackgroundRuntimePreferences::default(),
                expected_revision: "0".into(),
            })
            .unwrap_err()
            .contains("changed in another surface"));
    }

    #[test]
    fn disk_revision_is_rechecked_inside_the_cross_process_write_boundary() {
        let directory = TestDirectory::new();
        let settings = BackgroundRuntimeSettings::initialize(directory.0.clone()).unwrap();
        let external = BackgroundRuntimeDocument {
            revision: 4,
            ..BackgroundRuntimeDocument::default()
        };
        persist_document(
            &directory.0.join(DOCUMENT_FILE_NAME),
            &directory.0.join(DOCUMENT_TEMP_FILE_NAME),
            &external,
        )
        .unwrap();

        let error = settings
            .save_preferences(SaveBackgroundRuntimePreferencesRequest {
                preferences: BackgroundRuntimePreferences::default(),
                expected_revision: "0".into(),
            })
            .unwrap_err();
        assert!(error.contains("changed in another surface"));
        assert_eq!(settings.document().unwrap().revision, 4);
    }

    #[test]
    fn corrupt_or_unknown_documents_fail_closed() {
        let directory = TestDirectory::new();
        fs::write(
            directory.0.join(DOCUMENT_FILE_NAME),
            br#"{"version":1,"revision":3,"preferences":{"version":1,"experimentalDesktopCards":true,"closeToTray":true,"cards":{"server":true,"storage":true,"services":true}},"geometryRevisions":{"server":0,"storage":0,"services":0},"unexpected":true}"#,
        )
        .unwrap();

        let settings = BackgroundRuntimeSettings::initialize(directory.0.clone()).unwrap();
        assert!(
            !settings
                .document()
                .unwrap()
                .preferences
                .experimental_desktop_cards
        );
        assert_eq!(
            settings.recovery_notice().unwrap().as_deref(),
            Some(INVALID_DOCUMENT_NOTICE)
        );

        // Saving the displayed safe defaults must repair the persisted file,
        // even though the preferences themselves did not change in memory.
        settings
            .save_preferences(SaveBackgroundRuntimePreferencesRequest {
                preferences: BackgroundRuntimePreferences::default(),
                expected_revision: "0".into(),
            })
            .unwrap();
        assert_eq!(
            read_document(&directory.0.join(DOCUMENT_FILE_NAME))
                .unwrap()
                .revision,
            1
        );
        assert_eq!(settings.recovery_notice().unwrap(), None);
    }

    #[test]
    fn oversized_documents_are_rejected_with_a_bounded_read() {
        let directory = TestDirectory::new();
        fs::write(
            directory.0.join(DOCUMENT_FILE_NAME),
            vec![b' '; (MAX_DOCUMENT_BYTES + 1) as usize],
        )
        .unwrap();

        assert!(read_document(&directory.0.join(DOCUMENT_FILE_NAME))
            .unwrap_err()
            .contains("too large"));
    }

    #[test]
    fn geometry_reset_changes_every_card_without_enabling_it() {
        let directory = TestDirectory::new();
        let settings = BackgroundRuntimeSettings::initialize(directory.0.clone()).unwrap();
        let (previous, next) = settings
            .mutate(|document| document.geometry_revisions.increment_all())
            .unwrap();

        assert_eq!(geometry_changes(&previous, &next), DesktopWidgetKind::all());
        assert!(!next.preferences.experimental_desktop_cards);
    }

    #[test]
    fn enabling_cards_forces_a_clean_runtime_remount() {
        let previous = BackgroundRuntimeDocument::default();
        let mut next = previous.clone();
        next.preferences.experimental_desktop_cards = true;
        assert_eq!(
            runtime_recreate_changes(&previous, &next),
            DesktopWidgetKind::all()
        );

        let mut previous = next;
        previous.preferences.cards.server = false;
        let mut next = previous.clone();
        next.preferences.cards.server = true;
        assert_eq!(
            runtime_recreate_changes(&previous, &next),
            vec![DesktopWidgetKind::Server]
        );
    }

    #[test]
    fn widget_authorization_is_exact() {
        assert!(authorize_widget("widget-server", DesktopWidgetKind::Server).is_ok());
        assert!(authorize_widget("widget-storage", DesktopWidgetKind::Server).is_err());
        assert!(authorize_main("main").is_ok());
        assert!(authorize_main("widget-services").is_err());
    }
}
