use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, Webview};

const DOCUMENT_FILE_NAME: &str = "desktop-integration.json";
const DOCUMENT_TEMP_FILE_NAME: &str = "desktop-integration.json.tmp";
const MAX_DOCUMENT_BYTES: u64 = 8 * 1024;
const INVALID_DOCUMENT_NOTICE: &str =
    "The saved desktop settings could not be read. Notifications were disabled until the settings are saved again.";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferences {
    pub(crate) version: u8,
    pub(crate) notify_service_outages: bool,
    pub(crate) notify_storage_pressure: bool,
    pub(crate) notify_download_completion: bool,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            notify_service_outages: false,
            notify_storage_pressure: false,
            notify_download_completion: false,
        }
    }
}

impl DesktopPreferences {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("desktop preferences.version must be 1.".into());
        }
        Ok(())
    }

    fn notifications_enabled(&self) -> bool {
        self.notify_service_outages
            || self.notify_storage_pressure
            || self.notify_download_completion
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DesktopDocument {
    version: u8,
    revision: u64,
    preferences: DesktopPreferences,
}

impl Default for DesktopDocument {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            preferences: DesktopPreferences::default(),
        }
    }
}

impl DesktopDocument {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("The desktop settings document version is unsupported.".into());
        }
        self.preferences.validate()
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopIntegrationSnapshot {
    preferences: DesktopPreferences,
    revision: String,
    startup_enabled: bool,
    startup_supported: bool,
    notifications_supported: bool,
    app_version: String,
    build_profile: &'static str,
    platform: &'static str,
    architecture: &'static str,
    signing_status: &'static str,
    recovery_notice: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDesktopIntegrationSettingsRequest {
    preferences: DesktopPreferences,
    expected_revision: String,
}

struct DesktopState {
    document: DesktopDocument,
    recovery_notice: Option<String>,
}

pub struct DesktopIntegrationSettings {
    primary_path: PathBuf,
    temporary_path: PathBuf,
    state: Mutex<DesktopState>,
    startup_operation: Mutex<()>,
}

impl DesktopIntegrationSettings {
    pub fn initialize(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&directory)
            .map_err(|_| "The desktop settings storage is unavailable.".to_string())?;
        let settings = Self {
            primary_path: directory.join(DOCUMENT_FILE_NAME),
            temporary_path: directory.join(DOCUMENT_TEMP_FILE_NAME),
            state: Mutex::new(DesktopState {
                document: DesktopDocument::default(),
                recovery_notice: None,
            }),
            startup_operation: Mutex::new(()),
        };
        settings.document_state()?;
        Ok(settings)
    }

    // Call only while holding the in-process state lock and native document
    // lock. Disk changes from another process cannot overwrite a newer save.
    fn refresh(&self, state: &mut DesktopState) {
        match read_document(&self.primary_path) {
            Ok(Some(document)) => {
                state.document = document;
                state.recovery_notice = None;
            }
            Ok(None) => {
                state.document.preferences = DesktopPreferences::default();
                state.recovery_notice = None;
            }
            Err(_) => {
                state.document.preferences = DesktopPreferences::default();
                state.recovery_notice = Some(INVALID_DOCUMENT_NOTICE.into());
            }
        }
    }

    fn document_state(&self) -> Result<(DesktopDocument, Option<String>), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "The desktop settings state is unavailable.".to_string())?;
        let _document_lock = acquire_document_write_lock()?;
        self.refresh(&mut state);
        Ok((state.document.clone(), state.recovery_notice.clone()))
    }

    pub(crate) fn preferences(&self) -> Result<DesktopPreferences, String> {
        self.document_state()
            .map(|(document, _)| document.preferences)
    }

    pub fn background_notifications_enabled(&self) -> bool {
        self.preferences()
            .map(|preferences| preferences.notifications_enabled())
            .unwrap_or(false)
    }

    fn save(&self, request: SaveDesktopIntegrationSettingsRequest) -> Result<(), String> {
        request.preferences.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "The desktop settings state is unavailable.".to_string())?;
        let _document_lock = acquire_document_write_lock()?;
        self.refresh(&mut state);
        if request.expected_revision != state.document.revision.to_string() {
            return Err(
                "Desktop settings changed in another surface. Reload them and try again.".into(),
            );
        }
        if request.preferences == state.document.preferences && state.recovery_notice.is_none() {
            return Ok(());
        }
        let next = DesktopDocument {
            version: 1,
            revision: state
                .document
                .revision
                .checked_add(1)
                .ok_or_else(|| "The desktop settings revision is exhausted.".to_string())?,
            preferences: request.preferences,
        };
        persist_document(&self.primary_path, &self.temporary_path, &next)?;
        state.document = next;
        state.recovery_notice = None;
        Ok(())
    }

    fn snapshot(&self, app: &AppHandle) -> Result<DesktopIntegrationSnapshot, String> {
        let (document, recovery_notice) = self.document_state()?;
        Ok(DesktopIntegrationSnapshot {
            preferences: document.preferences,
            revision: document.revision.to_string(),
            startup_enabled: startup_enabled(app)?,
            startup_supported: startup_supported(),
            notifications_supported: cfg!(windows),
            app_version: app.package_info().version.to_string(),
            build_profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            platform: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            // Signature verification is not performed by this module.
            signing_status: "not-verified",
            recovery_notice,
        })
    }
}

fn authorize_main(label: &str) -> Result<(), String> {
    if label != "main" {
        return Err("Desktop settings are available only to the trusted Personal Hub UI.".into());
    }
    Ok(())
}

fn startup_supported() -> bool {
    cfg!(windows) && !cfg!(debug_assertions)
}

#[cfg(windows)]
fn startup_manager() -> Result<auto_launch::AutoLaunch, String> {
    let executable = std::env::current_exe()
        .map_err(|_| "The application executable could not be located.".to_string())?;
    let path = executable
        .to_str()
        .ok_or_else(|| "The startup executable path is invalid.".to_string())?;
    // Windows Run values are command lines. Quote the executable explicitly:
    // auto-launch 0.5 writes app_path verbatim, including installation spaces.
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name("Personal Hub")
        .set_app_path(&format!("\"{path}\""))
        .build()
        .map_err(|_| "Windows startup integration is unavailable.".to_string())
}

fn startup_enabled(_app: &AppHandle) -> Result<bool, String> {
    if !startup_supported() {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        startup_manager()?
            .is_enabled()
            .map_err(|_| "Windows startup status could not be read.".into())
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[tauri::command]
pub fn get_desktop_integration_settings(
    caller: Webview,
    app: AppHandle,
    settings: tauri::State<'_, DesktopIntegrationSettings>,
) -> Result<DesktopIntegrationSnapshot, String> {
    authorize_main(caller.label())?;
    settings.snapshot(&app)
}

#[tauri::command]
pub fn save_desktop_integration_settings(
    caller: Webview,
    app: AppHandle,
    settings: tauri::State<'_, DesktopIntegrationSettings>,
    request: SaveDesktopIntegrationSettingsRequest,
) -> Result<DesktopIntegrationSnapshot, String> {
    authorize_main(caller.label())?;
    settings.save(request)?;
    settings.snapshot(&app)
}

#[tauri::command]
pub fn set_desktop_startup_enabled(
    caller: Webview,
    app: AppHandle,
    settings: tauri::State<'_, DesktopIntegrationSettings>,
    enabled: bool,
) -> Result<DesktopIntegrationSnapshot, String> {
    authorize_main(caller.label())?;
    if !startup_supported() {
        return Err("Windows startup can be changed only in a release desktop build.".into());
    }
    let _operation = settings
        .startup_operation
        .lock()
        .map_err(|_| "Another startup settings operation did not finish.".to_string())?;
    #[cfg(windows)]
    {
        let autolaunch = startup_manager()?;
        let result = if enabled {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        result.map_err(|_| "Windows startup registration could not be updated.".to_string())?;
        if startup_enabled(&app)? != enabled {
            return Err("Windows did not apply the requested startup setting.".into());
        }
    }
    #[cfg(not(windows))]
    let _ = enabled;
    settings.snapshot(&app)
}

fn read_document(path: &Path) -> Result<Option<DesktopDocument>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("The desktop settings file could not be read.".into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The desktop settings file could not be read.".to_string())?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("The desktop settings file is too large.".into());
    }
    let document: DesktopDocument = serde_json::from_slice(&bytes)
        .map_err(|_| "The desktop settings file is invalid.".to_string())?;
    document.validate()?;
    Ok(Some(document))
}

fn persist_document(
    primary_path: &Path,
    temporary_path: &Path,
    document: &DesktopDocument,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(document)
        .map_err(|_| "The desktop settings could not be encoded.".to_string())?;
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(temporary_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        atomic_replace(temporary_path, primary_path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
        return Err("The desktop settings could not be saved.".into());
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
        // SAFETY: this guard exclusively owns a successfully acquired mutex.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn acquire_document_write_lock() -> Result<DocumentWriteLock, String> {
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
    let name = "Local\\PersonalHub.DesktopIntegration.v1"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: the NUL-terminated mutex name remains valid during the call.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("The desktop settings lock is unavailable.".into());
    }
    // SAFETY: handle is a live mutex handle returned by CreateMutexW.
    let result = unsafe { WaitForSingleObject(handle, 5_000) };
    if result == 0 || result == 0x80 {
        return Ok(DocumentWriteLock(handle));
    }
    // SAFETY: no ownership transferred; only the unowned handle is closed.
    unsafe {
        let _ = CloseHandle(handle);
    }
    Err("Another Personal Hub process is updating desktop settings. Try again.".into())
}

#[cfg(not(windows))]
struct DocumentWriteLock;

#[cfg(not(windows))]
fn acquire_document_write_lock() -> Result<DocumentWriteLock, String> {
    Ok(DocumentWriteLock)
}

#[cfg(windows)]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
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
    // SAFETY: both paths are NUL-terminated and remain valid through the call.
    // REPLACE_EXISTING | WRITE_THROUGH keeps the old primary until commit.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "personal-hub-desktop-integration-{}-{sequence}",
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

    #[test]
    fn notification_preferences_are_opt_in_and_strict() {
        assert!(!DesktopPreferences::default().notifications_enabled());
        assert!(serde_json::from_str::<DesktopPreferences>(
            r#"{"version":1,"notifyServiceOutages":false,"notifyStoragePressure":false,"notifyDownloadCompletion":false,"extra":true}"#
        ).is_err());
        let unsupported = DesktopPreferences {
            version: 2,
            ..DesktopPreferences::default()
        };
        assert!(unsupported.validate().is_err());
        assert!(authorize_main("main").is_ok());
        assert!(authorize_main("widget-server").is_err());
        assert!(authorize_main("service-qbittorrent").is_err());
    }

    #[test]
    fn stale_settings_cannot_overwrite_another_save() {
        let directory = TestDirectory::new();
        let first = DesktopIntegrationSettings::initialize(directory.0.clone()).unwrap();
        let second = DesktopIntegrationSettings::initialize(directory.0.clone()).unwrap();
        first
            .save(SaveDesktopIntegrationSettingsRequest {
                preferences: DesktopPreferences {
                    notify_service_outages: true,
                    ..DesktopPreferences::default()
                },
                expected_revision: "0".into(),
            })
            .unwrap();
        assert!(second
            .save(SaveDesktopIntegrationSettingsRequest {
                preferences: DesktopPreferences::default(),
                expected_revision: "0".into(),
            })
            .unwrap_err()
            .contains("changed in another surface"));
        assert!(second.background_notifications_enabled());
        assert_eq!(second.document_state().unwrap().0.revision, 1);
        assert!(!directory.0.join(DOCUMENT_TEMP_FILE_NAME).exists());
    }

    #[test]
    fn corrupt_document_disables_notifications_and_can_be_repaired() {
        let directory = TestDirectory::new();
        let settings = DesktopIntegrationSettings::initialize(directory.0.clone()).unwrap();
        settings
            .save(SaveDesktopIntegrationSettingsRequest {
                preferences: DesktopPreferences {
                    notify_download_completion: true,
                    ..DesktopPreferences::default()
                },
                expected_revision: "0".into(),
            })
            .unwrap();
        fs::write(directory.0.join(DOCUMENT_FILE_NAME), b"broken").unwrap();
        assert!(!settings.background_notifications_enabled());
        let (document, notice) = settings.document_state().unwrap();
        assert!(notice.is_some());
        settings
            .save(SaveDesktopIntegrationSettingsRequest {
                preferences: DesktopPreferences::default(),
                expected_revision: document.revision.to_string(),
            })
            .unwrap();
        assert!(settings.document_state().unwrap().1.is_none());
        assert_eq!(
            read_document(&directory.0.join(DOCUMENT_FILE_NAME))
                .unwrap()
                .unwrap()
                .preferences,
            DesktopPreferences::default()
        );
    }
}
