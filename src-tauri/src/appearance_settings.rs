use serde::{Deserialize, Deserializer, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
};
use tauri::{AppHandle, Emitter, Manager, Theme, Webview};

const DOCUMENT_VERSION: u8 = 1;
const PREFERENCES_VERSION: u8 = 1;
const THEME_PACK_VERSION: u8 = 1;
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024;
const DOCUMENT_FILE_NAME: &str = "appearance-settings.json";
const DOCUMENT_TEMP_FILE_NAME: &str = "appearance-settings.json.tmp";
const INVALID_DOCUMENT_NOTICE: &str =
    "The saved appearance settings were invalid. Safe defaults were restored.";
const MAIN_WEBVIEW_LABEL: &str = "main";
const TRUSTED_APPEARANCE_WEBVIEW_LABELS: [&str; 4] = [
    MAIN_WEBVIEW_LABEL,
    "widget-server",
    "widget-storage",
    "widget-services",
];
const EVENT_APPEARANCE_CHANGED: &str = "personal-hub://appearance-changed";

const MIN_RADIUS_PX: u8 = 0;
const MAX_RADIUS_PX: u8 = 32;
const MIN_DENSITY: f64 = 0.75;
const MAX_DENSITY: f64 = 1.25;
const MIN_TYPE_SCALE: f64 = 0.80;
const MAX_TYPE_SCALE: f64 = 1.30;
const MIN_SHADOW: f64 = 0.0;
const MAX_SHADOW: f64 = 1.0;
const MIN_TRANSLUCENCY: f64 = 0.0;
const MAX_TRANSLUCENCY: f64 = 0.85;
const MIN_BLUR_PX: f64 = 0.0;
const MAX_BLUR_PX: f64 = 40.0;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeId {
    #[default]
    Default,
    Code,
    Translucent,
    Minimal,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ColorMode {
    System,
    // Personal Hub was dark-only before this contract. Keeping Dark as the
    // first-run value guarantees that adopting appearance persistence does
    // not change an existing installation's visual language.
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    System,
    Tr,
    En,
}

/// User-editable theme values. All fields are declarative primitives: no CSS,
/// font URL, markup, script, native path or command can enter the theme pack.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SafeThemeTokens {
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    background: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    surface: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    surface_elevated: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    surface_hover: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    border: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    border_strong: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    text_primary: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    text_secondary: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    text_tertiary: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    accent: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    accent_hover: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    online: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    offline: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    warning: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    radius_small: Option<u8>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    radius_medium: Option<u8>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    radius_large: Option<u8>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    density: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    type_scale: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    shadow: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    translucency: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    blur: Option<f64>,
}

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl SafeThemeTokens {
    fn normalized(&self) -> Result<Self, String> {
        let normalized = Self {
            background: normalize_color(&self.background, "background")?,
            surface: normalize_color(&self.surface, "surface")?,
            surface_elevated: normalize_color(&self.surface_elevated, "surfaceElevated")?,
            surface_hover: normalize_color(&self.surface_hover, "surfaceHover")?,
            border: normalize_color(&self.border, "border")?,
            border_strong: normalize_color(&self.border_strong, "borderStrong")?,
            text_primary: normalize_color(&self.text_primary, "textPrimary")?,
            text_secondary: normalize_color(&self.text_secondary, "textSecondary")?,
            text_tertiary: normalize_color(&self.text_tertiary, "textTertiary")?,
            accent: normalize_color(&self.accent, "accent")?,
            accent_hover: normalize_color(&self.accent_hover, "accentHover")?,
            online: normalize_color(&self.online, "online")?,
            offline: normalize_color(&self.offline, "offline")?,
            warning: normalize_color(&self.warning, "warning")?,
            radius_small: bounded_radius(self.radius_small, "radiusSmall")?,
            radius_medium: bounded_radius(self.radius_medium, "radiusMedium")?,
            radius_large: bounded_radius(self.radius_large, "radiusLarge")?,
            density: bounded_number(self.density, MIN_DENSITY, MAX_DENSITY, "density")?,
            type_scale: bounded_number(
                self.type_scale,
                MIN_TYPE_SCALE,
                MAX_TYPE_SCALE,
                "typeScale",
            )?,
            shadow: bounded_number(self.shadow, MIN_SHADOW, MAX_SHADOW, "shadow")?,
            translucency: bounded_number(
                self.translucency,
                MIN_TRANSLUCENCY,
                MAX_TRANSLUCENCY,
                "translucency",
            )?,
            blur: bounded_number(self.blur, MIN_BLUR_PX, MAX_BLUR_PX, "blur")?,
        };

        validate_radius_order(
            normalized.radius_small,
            normalized.radius_medium,
            "radiusSmall",
            "radiusMedium",
        )?;
        validate_radius_order(
            normalized.radius_medium,
            normalized.radius_large,
            "radiusMedium",
            "radiusLarge",
        )?;
        validate_radius_order(
            normalized.radius_small,
            normalized.radius_large,
            "radiusSmall",
            "radiusLarge",
        )?;
        Ok(normalized)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeOverrides {
    dark: SafeThemeTokens,
    light: SafeThemeTokens,
}

impl ThemeOverrides {
    fn normalized(&self) -> Result<Self, String> {
        Ok(Self {
            dark: self.dark.normalized()?,
            light: self.light.normalized()?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearancePreferences {
    version: u8,
    color_mode: ColorMode,
    theme_id: ThemeId,
    language: Language,
    overrides: ThemeOverrides,
}

impl Default for AppearancePreferences {
    fn default() -> Self {
        Self {
            version: PREFERENCES_VERSION,
            color_mode: ColorMode::default(),
            theme_id: ThemeId::default(),
            language: Language::default(),
            overrides: ThemeOverrides::default(),
        }
    }
}

impl AppearancePreferences {
    fn normalized(&self) -> Result<Self, String> {
        if self.version != PREFERENCES_VERSION {
            return Err("appearance preferences.version must be 1.".into());
        }

        let overrides = self.overrides.normalized()?;
        validate_resolved_theme_radii(self.theme_id, &overrides)?;

        Ok(Self {
            version: PREFERENCES_VERSION,
            color_mode: self.color_mode,
            theme_id: self.theme_id,
            language: self.language,
            overrides,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppearanceDocument {
    version: u8,
    revision: u64,
    preferences: AppearancePreferences,
}

impl Default for AppearanceDocument {
    fn default() -> Self {
        Self {
            version: DOCUMENT_VERSION,
            revision: 0,
            preferences: AppearancePreferences::default(),
        }
    }
}

impl AppearanceDocument {
    fn normalized(&self) -> Result<Self, String> {
        if self.version != DOCUMENT_VERSION {
            return Err("The appearance settings document version is unsupported.".into());
        }

        Ok(Self {
            version: DOCUMENT_VERSION,
            revision: self.revision,
            preferences: self.preferences.normalized()?,
        })
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSnapshot {
    preferences: AppearancePreferences,
    revision: String,
    recovery_notice: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveAppearancePreferencesRequest {
    preferences: AppearancePreferences,
    expected_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetAppearancePreferencesRequest {
    expected_revision: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ThemePackKind {
    PersonalHubAppearance,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemePackDocument {
    kind: ThemePackKind,
    version: u8,
    preferences: AppearancePreferences,
}

impl ThemePackDocument {
    fn from_preferences(preferences: AppearancePreferences) -> Self {
        Self {
            kind: ThemePackKind::PersonalHubAppearance,
            version: THEME_PACK_VERSION,
            preferences,
        }
    }

    fn normalized_preferences(&self) -> Result<AppearancePreferences, String> {
        if self.kind != ThemePackKind::PersonalHubAppearance || self.version != THEME_PACK_VERSION {
            return Err("The appearance import format is unsupported.".into());
        }
        self.preferences.normalized()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportAppearancePreferencesRequest {
    document: ThemePackDocument,
    expected_revision: String,
}

pub struct AppearanceSettings {
    primary_path: PathBuf,
    temporary_path: PathBuf,
    document: RwLock<AppearanceDocument>,
    recovery_notice: RwLock<Option<String>>,
    operation: Mutex<()>,
    runtime_operation: Mutex<()>,
}

impl AppearanceSettings {
    pub fn initialize(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&directory)
            .map_err(|_| "The appearance settings storage is unavailable.".to_string())?;
        let primary_path = directory.join(DOCUMENT_FILE_NAME);
        let temporary_path = directory.join(DOCUMENT_TEMP_FILE_NAME);
        let (document, recovery_notice) = if primary_path.exists() {
            match read_document(&primary_path) {
                Ok(document) => (document, None),
                Err(_) => (
                    AppearanceDocument::default(),
                    Some(INVALID_DOCUMENT_NOTICE.to_string()),
                ),
            }
        } else {
            (AppearanceDocument::default(), None)
        };

        Ok(Self {
            primary_path,
            temporary_path,
            document: RwLock::new(document),
            recovery_notice: RwLock::new(recovery_notice),
            operation: Mutex::new(()),
            runtime_operation: Mutex::new(()),
        })
    }

    fn document(&self) -> Result<AppearanceDocument, String> {
        self.document
            .read()
            .map(|document| document.clone())
            .map_err(|_| "The appearance settings state is unavailable.".to_string())
    }

    fn recovery_notice(&self) -> Result<Option<String>, String> {
        self.recovery_notice
            .read()
            .map(|notice| notice.clone())
            .map_err(|_| "The appearance recovery state is unavailable.".to_string())
    }

    /// Refreshes the in-memory document while the caller owns both the local
    /// operation mutex and the cross-process document mutex. Keeping reads on
    /// the same boundary as writes prevents a second Personal Hub process
    /// from leaving this process with a stale snapshot or export.
    fn refresh_document_from_disk(&self) -> Result<AppearanceDocument, String> {
        let cached = self.document()?;
        if !self.primary_path.exists() {
            return Ok(cached);
        }

        match read_document(&self.primary_path) {
            Ok(document) => {
                if document != cached {
                    *self.document.write().map_err(|_| {
                        "The appearance settings state is unavailable.".to_string()
                    })? = document.clone();
                }
                *self
                    .recovery_notice
                    .write()
                    .map_err(|_| "The appearance recovery state is unavailable.".to_string())? =
                    None;
                Ok(document)
            }
            Err(_) if self.recovery_notice()?.is_some() => Ok(cached),
            Err(error) => Err(error),
        }
    }

    fn snapshot(&self) -> Result<AppearanceSnapshot, String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "Another appearance settings operation did not finish.".to_string())?;
        let _document_lock = acquire_document_write_lock()?;
        let document = self.refresh_document_from_disk()?;
        Ok(AppearanceSnapshot {
            preferences: document.preferences,
            revision: document.revision.to_string(),
            recovery_notice: self.recovery_notice()?,
        })
    }

    fn mutate(
        &self,
        expected_revision: String,
        preferences: AppearancePreferences,
    ) -> Result<AppearanceDocument, String> {
        let preferences = preferences.normalized()?;
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "Another appearance settings operation did not finish.".to_string())?;
        let _document_write_lock = acquire_document_write_lock()?;
        let previous = self.refresh_document_from_disk()?;

        if expected_revision != previous.revision.to_string() {
            return Err(
                "Appearance settings changed in another surface. Reload them and try again.".into(),
            );
        }

        let needs_recovery_write = self.recovery_notice()?.is_some();
        if preferences == previous.preferences && !needs_recovery_write {
            return Ok(previous);
        }

        let next = AppearanceDocument {
            version: DOCUMENT_VERSION,
            revision: previous
                .revision
                .checked_add(1)
                .ok_or_else(|| "The appearance settings revision is exhausted.".to_string())?,
            preferences,
        };
        persist_document(&self.primary_path, &self.temporary_path, &next)?;
        *self
            .document
            .write()
            .map_err(|_| "The appearance settings state is unavailable.".to_string())? =
            next.clone();
        *self
            .recovery_notice
            .write()
            .map_err(|_| "The appearance recovery state is unavailable.".to_string())? = None;
        Ok(next)
    }
}

#[tauri::command]
pub fn get_appearance_settings(
    caller: Webview,
    settings: tauri::State<'_, AppearanceSettings>,
) -> Result<AppearanceSnapshot, String> {
    authorize_reader(caller.label())?;
    settings.snapshot()
}

#[tauri::command]
pub fn save_appearance_preferences(
    caller: Webview,
    app: AppHandle,
    request: SaveAppearancePreferencesRequest,
    settings: tauri::State<'_, AppearanceSettings>,
) -> Result<AppearanceSnapshot, String> {
    authorize_mutation(caller.label())?;
    settings.mutate(request.expected_revision, request.preferences)?;
    finish_appearance_change(&app, &settings)
}

#[tauri::command]
pub fn reset_appearance_preferences(
    caller: Webview,
    app: AppHandle,
    request: ResetAppearancePreferencesRequest,
    settings: tauri::State<'_, AppearanceSettings>,
) -> Result<AppearanceSnapshot, String> {
    authorize_mutation(caller.label())?;
    settings.mutate(request.expected_revision, AppearancePreferences::default())?;
    finish_appearance_change(&app, &settings)
}

#[tauri::command]
pub fn import_appearance_preferences(
    caller: Webview,
    app: AppHandle,
    request: ImportAppearancePreferencesRequest,
    settings: tauri::State<'_, AppearanceSettings>,
) -> Result<AppearanceSnapshot, String> {
    authorize_mutation(caller.label())?;
    let preferences = request.document.normalized_preferences()?;
    settings.mutate(request.expected_revision, preferences)?;
    finish_appearance_change(&app, &settings)
}

#[tauri::command]
pub fn export_appearance_preferences(
    caller: Webview,
    settings: tauri::State<'_, AppearanceSettings>,
) -> Result<ThemePackDocument, String> {
    authorize_mutation(caller.label())?;
    Ok(ThemePackDocument::from_preferences(
        settings.snapshot()?.preferences,
    ))
}

/// Applies the persisted native color mode during setup. CSS theme tokens are
/// consumed from `get_appearance_settings` by each trusted frontend surface.
pub(crate) fn apply_current_appearance(app: &AppHandle) -> Result<(), String> {
    let settings = app.state::<AppearanceSettings>();
    let _runtime_operation = settings
        .runtime_operation
        .lock()
        .map_err(|_| "The appearance runtime is unavailable.".to_string())?;
    let snapshot = settings.snapshot()?;
    apply_native_theme(app, snapshot.preferences.color_mode);
    emit_appearance_changed(app, &snapshot);
    Ok(())
}

pub(crate) fn current_native_theme(app: &AppHandle) -> Result<Option<Theme>, String> {
    let color_mode = app
        .state::<AppearanceSettings>()
        .snapshot()?
        .preferences
        .color_mode;
    Ok(native_theme(color_mode))
}

pub(crate) fn notification_language_is_turkish(app: &AppHandle) -> bool {
    let language = app
        .state::<AppearanceSettings>()
        .snapshot()
        .map(|snapshot| snapshot.preferences.language)
        .unwrap_or_default();
    match language {
        Language::Tr => true,
        Language::En => false,
        Language::System => {
            #[cfg(windows)]
            {
                #[link(name = "Kernel32")]
                extern "system" {
                    fn GetUserDefaultUILanguage() -> u16;
                }
                // SAFETY: this Windows function takes no pointers or handles.
                unsafe { GetUserDefaultUILanguage() & 0x03ff == 0x001f }
            }
            #[cfg(not(windows))]
            {
                false
            }
        }
    }
}

fn finish_appearance_change(
    app: &AppHandle,
    settings: &AppearanceSettings,
) -> Result<AppearanceSnapshot, String> {
    let _runtime_operation = settings
        .runtime_operation
        .lock()
        .map_err(|_| "The appearance runtime is unavailable.".to_string())?;
    let snapshot = settings.snapshot()?;
    apply_native_theme(app, snapshot.preferences.color_mode);
    emit_appearance_changed(app, &snapshot);
    Ok(snapshot)
}

fn apply_native_theme(app: &AppHandle, color_mode: ColorMode) {
    let theme = native_theme(color_mode);

    for label in TRUSTED_APPEARANCE_WEBVIEW_LABELS {
        if let Some(window) = app.get_webview_window(label) {
            // A window can disappear between lookup and set_theme. Appearance
            // persistence remains authoritative and will be applied when that
            // trusted surface is created again.
            let _ = window.set_theme(theme);
        }
    }
}

fn native_theme(color_mode: ColorMode) -> Option<Theme> {
    match color_mode {
        ColorMode::System => None,
        ColorMode::Dark => Some(Theme::Dark),
        ColorMode::Light => Some(Theme::Light),
    }
}

fn emit_appearance_changed(app: &AppHandle, snapshot: &AppearanceSnapshot) {
    for label in TRUSTED_APPEARANCE_WEBVIEW_LABELS {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.emit(EVENT_APPEARANCE_CHANGED, snapshot);
        }
    }
}

fn authorize_reader(caller_label: &str) -> Result<(), String> {
    if TRUSTED_APPEARANCE_WEBVIEW_LABELS.contains(&caller_label) {
        Ok(())
    } else {
        Err("Appearance settings are available only to a trusted Personal Hub window.".into())
    }
}

fn authorize_mutation(caller_label: &str) -> Result<(), String> {
    if caller_label == MAIN_WEBVIEW_LABEL {
        Ok(())
    } else {
        Err("Appearance settings can be changed only by the trusted main window.".into())
    }
}

fn normalize_color(value: &Option<String>, field: &str) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes = value.as_bytes();
    if !matches!(bytes.len(), 7 | 9)
        || bytes.first() != Some(&b'#')
        || !bytes[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(format!(
            "appearance overrides.{field} must be #RRGGBB or #RRGGBBAA."
        ));
    }
    Ok(Some(value.to_ascii_uppercase()))
}

fn bounded_radius(value: Option<u8>, field: &str) -> Result<Option<u8>, String> {
    if value.is_some_and(|value| !(MIN_RADIUS_PX..=MAX_RADIUS_PX).contains(&value)) {
        Err(format!(
            "appearance overrides.{field} must be between {MIN_RADIUS_PX} and {MAX_RADIUS_PX}."
        ))
    } else {
        Ok(value)
    }
}

fn bounded_number(
    value: Option<f64>,
    minimum: f64,
    maximum: f64,
    field: &str,
) -> Result<Option<f64>, String> {
    match value {
        Some(value) if !value.is_finite() || value < minimum || value > maximum => Err(format!(
            "appearance overrides.{field} must be between {minimum} and {maximum}."
        )),
        Some(value) => {
            let rounded = (value * 1_000.0).round() / 1_000.0;
            Ok(Some(if rounded == 0.0 { 0.0 } else { rounded }))
        }
        None => Ok(None),
    }
}

fn validate_resolved_theme_radii(
    theme_id: ThemeId,
    overrides: &ThemeOverrides,
) -> Result<(), String> {
    let base = match theme_id {
        ThemeId::Default | ThemeId::Custom => (10, 14, 18),
        ThemeId::Code => (4, 6, 8),
        ThemeId::Translucent => (12, 18, 24),
        ThemeId::Minimal => (3, 4, 6),
    };

    for (mode, tokens) in [("dark", &overrides.dark), ("light", &overrides.light)] {
        let small = tokens.radius_small.unwrap_or(base.0);
        let medium = tokens.radius_medium.unwrap_or(base.1);
        let large = tokens.radius_large.unwrap_or(base.2);
        if small > medium || medium > large {
            return Err(format!(
                "appearance overrides.{mode} must resolve to ordered small, medium and large radii."
            ));
        }
    }
    Ok(())
}

fn validate_radius_order(
    smaller: Option<u8>,
    larger: Option<u8>,
    smaller_name: &str,
    larger_name: &str,
) -> Result<(), String> {
    if matches!((smaller, larger), (Some(smaller), Some(larger)) if smaller > larger) {
        Err(format!(
            "appearance overrides.{smaller_name} must not exceed {larger_name}."
        ))
    } else {
        Ok(())
    }
}

fn read_document(path: &Path) -> Result<AppearanceDocument, String> {
    let file = fs::File::open(path)
        .map_err(|_| "The appearance settings file is unavailable.".to_string())?;
    let mut contents = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| "The appearance settings file could not be read.".to_string())?;
    if contents.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("The appearance settings file is too large.".into());
    }
    let document: AppearanceDocument = serde_json::from_slice(&contents)
        .map_err(|_| "The appearance settings file has an invalid schema.".to_string())?;
    document.normalized()
}

fn persist_document(
    destination: &Path,
    temporary: &Path,
    document: &AppearanceDocument,
) -> Result<(), String> {
    let mut contents = serde_json::to_vec_pretty(document)
        .map_err(|_| "The appearance settings could not be serialized.".to_string())?;
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

    if write_result.is_err() {
        let _ = fs::remove_file(temporary);
        return Err("The appearance settings could not be saved.".into());
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

        // SAFETY: this guard exclusively owns a successfully acquired mutex
        // handle. Both native calls are best-effort cleanup during Drop.
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

    let name = "Local\\PersonalHub.AppearanceSettings.v1"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: the mutex name is NUL-terminated and remains alive for the call.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("The appearance settings lock is unavailable.".into());
    }

    // SAFETY: handle is a valid mutex handle returned by CreateMutexW.
    let wait_result = unsafe { WaitForSingleObject(handle, DOCUMENT_LOCK_TIMEOUT_MS) };
    if wait_result == WAIT_OBJECT_0 || wait_result == WAIT_ABANDONED {
        return Ok(DocumentWriteLock(handle));
    }

    // SAFETY: the wait did not transfer ownership; only the handle is closed.
    unsafe {
        let _ = CloseHandle(handle);
    }
    if wait_result == WAIT_TIMEOUT {
        Err("Another Personal Hub process is updating appearance settings. Try again.".into())
    } else {
        Err("The appearance settings lock is unavailable.".into())
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
    // SAFETY: both path buffers are valid NUL-terminated UTF-16 strings for
    // the duration of the call.
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
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "personal-hub-appearance-settings-{}-{sequence}",
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

    fn preferences_with_theme(theme_id: ThemeId) -> AppearancePreferences {
        AppearancePreferences {
            theme_id,
            ..AppearancePreferences::default()
        }
    }

    #[test]
    fn first_run_preserves_the_existing_dark_default_appearance() {
        let directory = TestDirectory::new();
        let settings = AppearanceSettings::initialize(directory.0.clone()).unwrap();
        let snapshot = settings.snapshot().unwrap();

        assert_eq!(snapshot.revision, "0");
        assert_eq!(snapshot.recovery_notice, None);
        assert_eq!(snapshot.preferences.theme_id, ThemeId::Default);
        assert_eq!(snapshot.preferences.color_mode, ColorMode::Dark);
        assert_eq!(snapshot.preferences.language, Language::System);
        assert_eq!(snapshot.preferences.overrides, ThemeOverrides::default());
        assert!(matches!(
            native_theme(snapshot.preferences.color_mode),
            Some(Theme::Dark)
        ));
        assert!(matches!(native_theme(ColorMode::Light), Some(Theme::Light)));
        assert!(native_theme(ColorMode::System).is_none());
    }

    #[test]
    fn public_dtos_do_not_expose_storage_or_runtime_details() {
        let snapshot = AppearanceSnapshot {
            preferences: AppearancePreferences::default(),
            revision: "3".into(),
            recovery_notice: None,
        };
        let snapshot = serde_json::to_value(snapshot).unwrap();
        let snapshot_fields = snapshot.as_object().unwrap();
        assert_eq!(snapshot_fields.len(), 3);
        for key in ["preferences", "recoveryNotice", "revision"] {
            assert!(snapshot_fields.contains_key(key));
        }

        let pack = serde_json::to_value(ThemePackDocument::from_preferences(
            AppearancePreferences::default(),
        ))
        .unwrap();
        let pack_fields = pack.as_object().unwrap();
        assert_eq!(pack_fields.len(), 3);
        for key in ["kind", "preferences", "version"] {
            assert!(pack_fields.contains_key(key));
        }
        let serialized = serde_json::to_string(&pack).unwrap();
        assert!(!serialized.contains(DOCUMENT_FILE_NAME));
        assert!(!serialized.contains("credential"));
    }

    #[test]
    fn schemas_are_exact_and_versioned() {
        let mut value = serde_json::to_value(AppearancePreferences::default()).unwrap();
        value["unexpected"] = json!(true);
        assert!(serde_json::from_value::<AppearancePreferences>(value).is_err());

        let mut value = serde_json::to_value(AppearancePreferences::default()).unwrap();
        value["version"] = json!(2);
        let parsed: AppearancePreferences = serde_json::from_value(value).unwrap();
        assert!(parsed.normalized().unwrap_err().contains("version"));

        let mut document = serde_json::to_value(ThemePackDocument::from_preferences(
            AppearancePreferences::default(),
        ))
        .unwrap();
        document["extra"] = json!("rejected");
        assert!(serde_json::from_value::<ThemePackDocument>(document).is_err());
    }

    #[test]
    fn token_colors_are_canonical_and_cannot_contain_css() {
        let tokens: SafeThemeTokens = serde_json::from_value(json!({
            "background": "#0a0b0c",
            "border": "#FfFfFf14"
        }))
        .unwrap();
        let normalized = tokens.normalized().unwrap();
        assert_eq!(normalized.background.as_deref(), Some("#0A0B0C"));
        assert_eq!(normalized.border.as_deref(), Some("#FFFFFF14"));

        for invalid in [
            "red",
            "#fff",
            "#12345g",
            "#000000; background:url(file:///secret)",
        ] {
            let tokens: SafeThemeTokens =
                serde_json::from_value(json!({ "accent": invalid })).unwrap();
            assert!(tokens.normalized().is_err());
        }

        // Optional means omitted, not an alternate null value. This keeps the
        // native decoder identical to the published JSON Schema.
        assert!(serde_json::from_value::<SafeThemeTokens>(json!({
            "accent": null
        }))
        .is_err());
        assert!(serde_json::from_value::<SafeThemeTokens>(json!({
            "density": null
        }))
        .is_err());
    }

    #[test]
    fn numeric_tokens_are_tightly_bounded_and_radii_are_ordered() {
        let valid: SafeThemeTokens = serde_json::from_value(json!({
            "radiusSmall": 8,
            "radiusMedium": 12,
            "radiusLarge": 20,
            "density": 0.75,
            "typeScale": 1.3,
            "shadow": 0.0,
            "translucency": 0.85,
            "blur": 40.0
        }))
        .unwrap();
        assert!(valid.normalized().is_ok());

        for invalid in [
            json!({ "radiusLarge": 33 }),
            json!({ "radiusSmall": 18, "radiusMedium": 10 }),
            json!({ "density": 0.74 }),
            json!({ "typeScale": 1.31 }),
            json!({ "shadow": -0.01 }),
            json!({ "translucency": 0.86 }),
            json!({ "blur": 40.01 }),
        ] {
            let tokens: SafeThemeTokens = serde_json::from_value(invalid).unwrap();
            assert!(tokens.normalized().is_err());
        }
    }

    #[test]
    fn partial_radius_overrides_are_checked_against_each_bundled_base() {
        for (theme_id, mode, overrides) in [
            (
                ThemeId::Default,
                "dark",
                json!({ "dark": { "radiusSmall": 20 }, "light": {} }),
            ),
            (
                ThemeId::Code,
                "dark",
                json!({ "dark": { "radiusMedium": 3 }, "light": {} }),
            ),
            (
                ThemeId::Translucent,
                "light",
                json!({ "dark": {}, "light": { "radiusLarge": 17 } }),
            ),
            (
                ThemeId::Minimal,
                "light",
                json!({ "dark": {}, "light": { "radiusSmall": 5 } }),
            ),
            (
                ThemeId::Custom,
                "dark",
                json!({ "dark": { "radiusSmall": 20 }, "light": {} }),
            ),
        ] {
            let mut preferences = preferences_with_theme(theme_id);
            preferences.overrides = serde_json::from_value(overrides).unwrap();
            assert!(preferences.normalized().unwrap_err().contains(mode));
        }

        let mut valid_code = preferences_with_theme(ThemeId::Code);
        valid_code.overrides = serde_json::from_value(json!({
            "dark": { "radiusSmall": 5 },
            "light": { "radiusLarge": 9 }
        }))
        .unwrap();
        assert!(valid_code.normalized().is_ok());
    }

    #[test]
    fn numeric_tokens_use_the_frontend_three_decimal_contract() {
        let tokens: SafeThemeTokens = serde_json::from_value(json!({
            "density": 0.75149,
            "typeScale": 1.23456,
            "shadow": -0.0
        }))
        .unwrap();
        let normalized = tokens.normalized().unwrap();
        assert_eq!(normalized.density, Some(0.751));
        assert_eq!(normalized.type_scale, Some(1.235));
        assert_eq!(normalized.shadow, Some(0.0));
        assert!(!normalized.shadow.unwrap().is_sign_negative());
    }

    #[test]
    fn changes_are_atomic_and_revision_checked() {
        let directory = TestDirectory::new();
        let settings = AppearanceSettings::initialize(directory.0.clone()).unwrap();
        let preferences = preferences_with_theme(ThemeId::Code);
        settings.mutate("0".into(), preferences.clone()).unwrap();

        let saved = read_document(&directory.0.join(DOCUMENT_FILE_NAME)).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.preferences, preferences);
        assert!(!directory.0.join(DOCUMENT_TEMP_FILE_NAME).exists());
        assert!(settings
            .mutate("0".into(), preferences_with_theme(ThemeId::Translucent))
            .unwrap_err()
            .contains("changed in another surface"));
    }

    #[test]
    fn disk_revision_is_rechecked_inside_the_cross_process_boundary() {
        let directory = TestDirectory::new();
        let settings = AppearanceSettings::initialize(directory.0.clone()).unwrap();
        let external = AppearanceDocument {
            revision: 7,
            preferences: preferences_with_theme(ThemeId::Minimal),
            ..AppearanceDocument::default()
        };
        persist_document(
            &directory.0.join(DOCUMENT_FILE_NAME),
            &directory.0.join(DOCUMENT_TEMP_FILE_NAME),
            &external,
        )
        .unwrap();

        let error = settings
            .mutate("0".into(), AppearancePreferences::default())
            .unwrap_err();
        assert!(error.contains("changed in another surface"));
        assert_eq!(settings.document().unwrap(), external);
    }

    #[test]
    fn snapshots_refresh_changes_written_by_another_process_instance() {
        let directory = TestDirectory::new();
        let first = AppearanceSettings::initialize(directory.0.clone()).unwrap();
        let second = AppearanceSettings::initialize(directory.0.clone()).unwrap();

        let mut preferences = preferences_with_theme(ThemeId::Code);
        preferences.overrides = serde_json::from_value(json!({
            "dark": { "accent": "#abcdef" },
            "light": { "accent": "#a1B2c3D4" }
        }))
        .unwrap();
        second.mutate("0".into(), preferences).unwrap();

        let snapshot = first.snapshot().unwrap();
        assert_eq!(snapshot.revision, "1");
        assert_eq!(snapshot.preferences.theme_id, ThemeId::Code);
        assert_eq!(
            snapshot.preferences.overrides.dark.accent.as_deref(),
            Some("#ABCDEF")
        );
        assert_eq!(
            snapshot.preferences.overrides.light.accent.as_deref(),
            Some("#A1B2C3D4")
        );

        let persisted = read_document(&directory.0.join(DOCUMENT_FILE_NAME)).unwrap();
        assert_eq!(persisted.preferences, snapshot.preferences);
        let persisted_json = fs::read_to_string(directory.0.join(DOCUMENT_FILE_NAME)).unwrap();
        assert!(persisted_json.contains("#ABCDEF"));
        assert!(persisted_json.contains("#A1B2C3D4"));
        assert!(!persisted_json.contains("#abcdef"));
        assert!(!persisted_json.contains("#a1B2c3D4"));
    }

    #[test]
    fn invalid_documents_fall_back_and_a_save_repairs_them() {
        let directory = TestDirectory::new();
        fs::write(
            directory.0.join(DOCUMENT_FILE_NAME),
            br#"{"version":1,"revision":9,"preferences":{"version":1,"colorMode":"dark","themeId":"default","language":"system","overrides":{"dark":{},"light":{}}},"unknown":true}"#,
        )
        .unwrap();

        let settings = AppearanceSettings::initialize(directory.0.clone()).unwrap();
        assert_eq!(
            settings.recovery_notice().unwrap().as_deref(),
            Some(INVALID_DOCUMENT_NOTICE)
        );
        assert_eq!(settings.document().unwrap(), AppearanceDocument::default());

        settings
            .mutate("0".into(), AppearancePreferences::default())
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
    fn reset_and_theme_pack_round_trip_use_the_same_safe_contract() {
        let directory = TestDirectory::new();
        let settings = AppearanceSettings::initialize(directory.0.clone()).unwrap();
        let custom: AppearancePreferences = serde_json::from_value(json!({
            "version": 1,
            "colorMode": "system",
            "themeId": "custom",
            "language": "tr",
            "overrides": {
                "dark": { "accent": "#FF00AA", "density": 0.9 },
                "light": { "accent": "#AA0066", "density": 1.0 }
            }
        }))
        .unwrap();
        let custom = custom.normalized().unwrap();
        let pack = ThemePackDocument::from_preferences(custom.clone());
        let encoded = serde_json::to_vec(&pack).unwrap();
        let decoded: ThemePackDocument = serde_json::from_slice(&encoded).unwrap();

        settings
            .mutate("0".into(), decoded.normalized_preferences().unwrap())
            .unwrap();
        assert_eq!(settings.document().unwrap().preferences, custom);
        settings
            .mutate("1".into(), AppearancePreferences::default())
            .unwrap();
        assert_eq!(
            settings.document().unwrap().preferences,
            AppearancePreferences::default()
        );
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
    fn reader_and_mutation_authorization_are_exact() {
        for label in TRUSTED_APPEARANCE_WEBVIEW_LABELS {
            assert!(authorize_reader(label).is_ok());
        }
        assert!(authorize_reader("service-jellyfin").is_err());
        assert!(authorize_reader("widget-server-extra").is_err());
        assert!(authorize_mutation(MAIN_WEBVIEW_LABEL).is_ok());
        assert!(authorize_mutation("widget-server").is_err());
    }
}
