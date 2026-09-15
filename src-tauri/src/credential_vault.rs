use crate::{
    download_center::DownloadCenterClients,
    service_settings::{ServiceBrowserAuthentication, ServiceSettings},
    service_webviews::{with_service_webview_revoked, ServiceCatalog, ServiceWebviewRegistry},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    sync::{Mutex, OnceLock},
};
use tauri::{AppHandle, Manager, Webview};

const MAIN_WEBVIEW_LABEL: &str = "main";
const CREDENTIAL_TARGET_PREFIX: &str = "PersonalHub/credentials/v1";
// This privileged credential deliberately lives outside the editable service
// catalog namespace. Service credential IPC and service-removal cleanup cannot
// name, replace, or delete it.
// Its vault key and origin binding are completed with the enrolled server
// address, so re-pointing the enrollment cannot read, overwrite or silently
// reuse the previous server's token.
const SERVER_CONTROL_TARGET_PREFIX: &str = "PersonalHub/server-control/v1/";
const MAX_SERVICE_ID_LENGTH: usize = 64;
const MAX_SECRET_BYTES: usize = 2_560;
const MAX_USERNAME_UTF16_UNITS: usize = 513;
const MAX_CANONICAL_ORIGIN_UTF16_UNITS: usize = 512;
// CRED_MAX_STRING_LENGTH from wincred.h. Credential comments are limited to
// this many UTF-16 code units, excluding their terminating NUL.
const MAX_CREDENTIAL_COMMENT_UTF16_UNITS: usize = 256;
const ORIGIN_METADATA_PREFIX: &str = "PersonalHub-Origin-v1:";

static CREDENTIAL_REVISIONS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static NEXT_CREDENTIAL_REVISION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    ApiKey,
    BearerToken,
    HttpBasic,
    UsernamePassword,
}

impl CredentialKind {
    pub(crate) const ALL: [Self; 4] = [
        Self::ApiKey,
        Self::BearerToken,
        Self::HttpBasic,
        Self::UsernamePassword,
    ];

    pub(crate) const fn target_suffix(self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::BearerToken => "bearer-token",
            Self::HttpBasic => "http-basic",
            Self::UsernamePassword => "username-password",
        }
    }

    const fn requires_username(self) -> bool {
        matches!(self, Self::HttpBasic | Self::UsernamePassword)
    }
}

/// Non-secret result of checking whether a credential can be consumed for an
/// exact service origin. `NeedsRebind` includes Phase 7.0/legacy entries that
/// have no origin metadata as well as entries saved for a previous origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CredentialBindingState {
    Missing,
    NeedsRebind,
    Available,
}

/// A plaintext value whose backing allocation is overwritten on drop.
///
/// This type is intentionally neither serializable nor cloneable. Provider
/// request material shares a whole credential through `Arc` instead of making
/// additional plaintext copies.
pub(crate) struct SensitiveString(String);

impl SensitiveString {
    fn from_bytes(bytes: Vec<u8>) -> Result<Self, String> {
        match String::from_utf8(bytes) {
            Ok(value) => Ok(Self(value)),
            Err(error) => {
                let mut bytes = error.into_bytes();
                bytes.fill(0);
                Err("The stored credential is invalid.".into())
            }
        }
    }

    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    fn zeroize(&mut self) {
        // SAFETY: replacing every byte with zero preserves UTF-8 validity and
        // keeps the String allocation/layout unchanged.
        unsafe { self.0.as_bytes_mut() }.fill(0);
    }
}

impl Drop for SensitiveString {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl fmt::Debug for SensitiveString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveString([REDACTED])")
    }
}

/// Credential material returned only to the native provider layer.
///
/// The canonical origin itself is deliberately not repeated here: the caller
/// supplies the expected origin to the read and the vault refuses every
/// missing, malformed, legacy, or non-matching binding before constructing
/// this value.
pub(crate) struct OriginBoundCredential {
    username: Option<SensitiveString>,
    secret: SensitiveString,
}

impl OriginBoundCredential {
    #[cfg(test)]
    fn new(username: Option<String>, secret: SensitiveString) -> Self {
        Self::from_sensitive(username.map(SensitiveString::new), secret)
    }

    fn from_sensitive(username: Option<SensitiveString>, secret: SensitiveString) -> Self {
        Self { username, secret }
    }

    pub(crate) fn username(&self) -> Option<&str> {
        self.username.as_ref().map(SensitiveString::expose)
    }

    pub(crate) fn secret(&self) -> &str {
        self.secret.expose()
    }

    #[cfg(test)]
    pub(crate) fn for_test(username: Option<&str>, secret: &str) -> Self {
        Self::new(
            username.map(str::to_string),
            SensitiveString::new(secret.to_string()),
        )
    }
}

impl fmt::Debug for OriginBoundCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OriginBoundCredential")
            .field("username", &self.username.as_ref().map(|_| "[REDACTED]"))
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

pub(crate) enum CredentialReadResult {
    Missing,
    NeedsRebind,
    Available(OriginBoundCredential),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    kind: CredentialKind,
    exists: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetServiceCredentialRequest {
    service_id: String,
    kind: CredentialKind,
    username: Option<String>,
    secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteServiceCredentialRequest {
    service_id: String,
    kind: CredentialKind,
}

#[tauri::command]
pub fn get_service_credential_statuses(
    caller: Webview,
    service_id: String,
    catalog: tauri::State<'_, ServiceCatalog>,
    settings: tauri::State<'_, ServiceSettings>,
) -> Result<Vec<CredentialStatus>, String> {
    ensure_trusted_caller(&caller)?;
    validate_service_id(&service_id)?;
    let _operation = settings.lock_operation()?;
    ensure_known_service(&catalog, &service_id)?;

    CredentialKind::ALL
        .into_iter()
        .map(|kind| {
            platform::credential_exists(&credential_target(&service_id, kind))
                .map(|exists| CredentialStatus { kind, exists })
        })
        .collect()
}

#[tauri::command]
pub fn set_service_credential(
    caller: Webview,
    request: SetServiceCredentialRequest,
    app: AppHandle,
    catalog: tauri::State<'_, ServiceCatalog>,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
    settings: tauri::State<'_, ServiceSettings>,
) -> Result<CredentialStatus, String> {
    ensure_trusted_caller(&caller)?;

    let SetServiceCredentialRequest {
        service_id,
        kind,
        username,
        secret,
    } = request;

    validate_service_id(&service_id)?;
    let _operation = settings.lock_operation()?;
    ensure_known_service(&catalog, &service_id)?;
    validate_credential(kind, username.as_deref(), &secret)?;
    let authentication_target = catalog.resolve_authentication_target(&service_id)?;
    let canonical_origin = catalog.canonical_origin_for_service(&service_id)?;

    let mutation = || {
        platform::write_credential(
            credential_target(&service_id, kind),
            username,
            secret,
            &canonical_origin,
        )?;
        bump_credential_revision(&service_id);
        Ok(())
    };
    if kind == CredentialKind::HttpBasic
        && authentication_target.authentication.browser == ServiceBrowserAuthentication::HttpBasic
    {
        with_service_webview_revoked(&app, &registry, &service_id, mutation)?;
    } else {
        mutation()?;
    }
    app.state::<DownloadCenterClients>()
        .clear_service_session(&service_id);
    Ok(CredentialStatus { kind, exists: true })
}

#[tauri::command]
pub fn delete_service_credential(
    caller: Webview,
    request: DeleteServiceCredentialRequest,
    app: AppHandle,
    catalog: tauri::State<'_, ServiceCatalog>,
    registry: tauri::State<'_, ServiceWebviewRegistry>,
    settings: tauri::State<'_, ServiceSettings>,
) -> Result<CredentialStatus, String> {
    ensure_trusted_caller(&caller)?;
    validate_service_id(&request.service_id)?;
    let _operation = settings.lock_operation()?;
    ensure_known_service(&catalog, &request.service_id)?;

    let authentication_target = catalog.resolve_authentication_target(&request.service_id)?;
    let mutation = || {
        platform::delete_credential(&credential_target(&request.service_id, request.kind))?;
        bump_credential_revision(&request.service_id);
        Ok(())
    };
    if request.kind == CredentialKind::HttpBasic
        && authentication_target.authentication.browser == ServiceBrowserAuthentication::HttpBasic
    {
        with_service_webview_revoked(&app, &registry, &request.service_id, mutation)?;
    } else {
        mutation()?;
    }
    app.state::<DownloadCenterClients>()
        .clear_service_session(&request.service_id);
    Ok(CredentialStatus {
        kind: request.kind,
        exists: false,
    })
}

/// Removes every credential shape owned by one service.
///
/// This helper intentionally has no catalog-membership requirement so callers
/// can use it after removing a service from configuration. Every kind is
/// attempted even if an earlier deletion fails, preventing one inaccessible
/// entry from stranding the other credentials.
pub(crate) fn delete_all_service_credentials(service_id: &str) -> Result<(), String> {
    let result = delete_all_service_credentials_with(service_id, platform::delete_credential);
    bump_credential_revision(service_id);
    result
}

fn delete_all_service_credentials_with(
    service_id: &str,
    mut delete: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    validate_service_id(service_id)?;
    let mut deletion_failed = false;

    for kind in CredentialKind::ALL {
        if delete(&credential_target(service_id, kind)).is_err() {
            deletion_failed = true;
        }
    }

    if deletion_failed {
        Err("One or more service credentials could not be deleted.".into())
    } else {
        Ok(())
    }
}

fn ensure_trusted_caller(caller: &Webview) -> Result<(), String> {
    if caller.label() != MAIN_WEBVIEW_LABEL {
        return Err("This command is available only to the trusted Personal Hub UI.".into());
    }

    Ok(())
}

fn ensure_known_service(catalog: &ServiceCatalog, service_id: &str) -> Result<(), String> {
    if catalog.contains_service(service_id) {
        Ok(())
    } else {
        Err("The requested credential service is not in the service catalog.".into())
    }
}

fn validate_service_id(service_id: &str) -> Result<(), String> {
    let valid = !service_id.is_empty()
        && service_id.len() <= MAX_SERVICE_ID_LENGTH
        && service_id.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        });

    if valid {
        Ok(())
    } else {
        Err("The credential service id is invalid.".into())
    }
}

fn validate_credential(
    kind: CredentialKind,
    username: Option<&str>,
    secret: &str,
) -> Result<(), String> {
    if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
        return Err("The credential secret length is invalid.".into());
    }

    match (kind.requires_username(), username) {
        (false, None) => Ok(()),
        (false, Some(_)) => Err("This credential kind does not accept a username.".into()),
        (true, None) => Err("This credential kind requires a username.".into()),
        (true, Some(username)) => validate_username(username),
    }
}

fn validate_username(username: &str) -> Result<(), String> {
    let utf16_length = username.encode_utf16().count();

    if username.is_empty() || username.contains('\0') || utf16_length > MAX_USERNAME_UTF16_UNITS {
        return Err("The credential username is invalid.".into());
    }

    Ok(())
}

fn credential_target(service_id: &str, kind: CredentialKind) -> String {
    format!(
        "{CREDENTIAL_TARGET_PREFIX}/{service_id}/{}",
        kind.target_suffix()
    )
}

pub(crate) fn credential_revision(service_id: &str) -> u64 {
    credential_revisions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(service_id)
        .copied()
        .unwrap_or(0)
}

fn bump_credential_revision(service_id: &str) -> u64 {
    let revision = NEXT_CREDENTIAL_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut revisions = credential_revisions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = revisions.entry(service_id.to_string()).or_default();
    *current = (*current).max(revision);
    *current
}

fn credential_revisions() -> &'static Mutex<HashMap<String, u64>> {
    CREDENTIAL_REVISIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn inspect_origin_bound_credential(
    service_id: &str,
    kind: CredentialKind,
    expected_origin: &str,
) -> Result<CredentialBindingState, String> {
    validate_service_id(service_id)?;
    validate_origin_metadata(expected_origin)?;
    platform::inspect_origin_binding(&credential_target(service_id, kind), expected_origin)
}

pub(crate) fn read_origin_bound_credential(
    service_id: &str,
    kind: CredentialKind,
    expected_origin: &str,
) -> Result<CredentialReadResult, String> {
    validate_service_id(service_id)?;
    validate_origin_metadata(expected_origin)?;
    platform::read_origin_bound_credential(&credential_target(service_id, kind), expected_origin)
}

/// The vault key and origin binding of the enrolled server's token. `address`
/// is already a validated private literal and `port` is the agent's fixed port,
/// so neither can introduce a separator, path segment or foreign origin here.
pub(crate) fn server_control_credential_target(address: &str, port: u16) -> String {
    format!("{SERVER_CONTROL_TARGET_PREFIX}{address}:{port}")
}

pub(crate) fn server_control_origin(address: &str, port: u16) -> String {
    format!("https://{address}:{port}")
}

/// Returns presence only; privileged token material never crosses IPC.
pub(crate) fn inspect_server_control_token(target: &str, origin: &str) -> Result<bool, String> {
    match platform::inspect_origin_binding(target, origin)? {
        CredentialBindingState::Missing => Ok(false),
        CredentialBindingState::Available => Ok(true),
        CredentialBindingState::NeedsRebind => {
            Err("The server control token has an invalid origin binding. Save it again.".into())
        }
    }
}

pub(crate) fn read_server_control_token(
    target: &str,
    origin: &str,
) -> Result<OriginBoundCredential, String> {
    match platform::read_origin_bound_credential(target, origin)? {
        CredentialReadResult::Missing => Err("No server control token is stored.".into()),
        CredentialReadResult::NeedsRebind => {
            Err("The server control token has an invalid origin binding. Save it again.".into())
        }
        CredentialReadResult::Available(credential) => {
            validate_server_control_token(credential.secret())?;
            if credential.username().is_some() {
                return Err("The stored server control token is invalid.".into());
            }
            Ok(credential)
        }
    }
}

pub(crate) fn write_server_control_token(
    target: &str,
    origin: &str,
    mut secret: SensitiveString,
) -> Result<(), String> {
    validate_server_control_token(secret.expose())?;
    // Move the existing allocation to the platform writer, which clears its
    // buffer after the Windows call, instead of making another plaintext copy.
    platform::write_credential(target.into(), None, std::mem::take(&mut secret.0), origin)
}

pub(crate) fn delete_server_control_token(target: &str) -> Result<(), String> {
    platform::delete_credential(target)
}

pub(crate) fn validate_server_control_token(secret: &str) -> Result<(), String> {
    if secret.len() == 64 && secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("The server control token must contain exactly 64 ASCII hexadecimal characters.".into())
    }
}

fn origin_metadata(canonical_origin: &str) -> String {
    format!("{ORIGIN_METADATA_PREFIX}{canonical_origin}")
}

fn validate_origin_metadata(canonical_origin: &str) -> Result<String, String> {
    let origin_units = canonical_origin.encode_utf16().count();
    if canonical_origin.is_empty()
        || canonical_origin.contains('\0')
        || origin_units > MAX_CANONICAL_ORIGIN_UTF16_UNITS
    {
        return Err("The credential origin binding is invalid.".into());
    }

    let metadata = origin_metadata(canonical_origin);
    if metadata.encode_utf16().count() > MAX_CREDENTIAL_COMMENT_UTF16_UNITS {
        return Err(
            "The service origin is too long for Windows Credential Manager metadata.".into(),
        );
    }

    Ok(metadata)
}

fn origin_metadata_matches(comment: Option<&str>, expected_origin: &str) -> Result<bool, String> {
    let expected = validate_origin_metadata(expected_origin)?;
    Ok(comment.is_some_and(|comment| comment == expected))
}

#[cfg(windows)]
mod platform {
    use super::{
        origin_metadata_matches, validate_origin_metadata, CredentialBindingState,
        CredentialReadResult, OriginBoundCredential, SensitiveString, MAX_SECRET_BYTES,
        MAX_USERNAME_UTF16_UNITS,
    };
    use std::{ffi::c_void, ptr::null_mut};

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    const ERROR_NOT_FOUND: i32 = 1_168;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct FileTime {
        dwLowDateTime: u32,
        dwHighDateTime: u32,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct CredentialW {
        Flags: u32,
        Type: u32,
        TargetName: *mut u16,
        Comment: *mut u16,
        LastWritten: FileTime,
        CredentialBlobSize: u32,
        CredentialBlob: *mut u8,
        Persist: u32,
        AttributeCount: u32,
        Attributes: *mut c_void,
        TargetAlias: *mut u16,
        UserName: *mut u16,
    }

    #[link(name = "Advapi32")]
    extern "system" {
        fn CredDeleteW(target_name: *const u16, credential_type: u32, flags: u32) -> i32;
        fn CredFree(buffer: *const c_void);
        fn CredReadW(
            target_name: *const u16,
            credential_type: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
    }

    struct CredentialAllocation(*mut CredentialW);

    impl CredentialAllocation {
        fn credential(&self) -> Result<&CredentialW, String> {
            // SAFETY: the constructor rejects null and the allocation remains
            // owned by this guard until Drop invokes CredFree.
            unsafe { self.0.as_ref() }.ok_or_else(vault_unavailable)
        }
    }

    impl Drop for CredentialAllocation {
        fn drop(&mut self) {
            // Best-effort removal of plaintext from the CredReadW allocation
            // on every path, including existence checks and origin mismatch.
            // Corrupted external entries are never trusted beyond the same
            // bounds enforced when Personal Hub writes them.
            unsafe {
                if let Some(credential) = self.0.as_mut() {
                    let blob_size = credential.CredentialBlobSize as usize;
                    if !credential.CredentialBlob.is_null()
                        && blob_size > 0
                        && blob_size <= MAX_SECRET_BYTES
                    {
                        std::ptr::write_bytes(credential.CredentialBlob, 0, blob_size);
                    }

                    if !credential.UserName.is_null() {
                        for index in 0..=MAX_USERNAME_UTF16_UNITS {
                            let unit = credential.UserName.add(index);
                            if *unit == 0 {
                                break;
                            }
                            *unit = 0;
                        }
                    }
                }
            }
            // SAFETY: a successful CredReadW call transfers this allocation to
            // the caller and requires exactly one matching CredFree call.
            unsafe { CredFree(self.0.cast()) };
        }
    }

    pub(super) fn credential_exists(target: &str) -> Result<bool, String> {
        read_allocation(target).map(|credential| credential.is_some())
    }

    pub(super) fn inspect_origin_binding(
        target: &str,
        expected_origin: &str,
    ) -> Result<CredentialBindingState, String> {
        let Some(allocation) = read_allocation(target)? else {
            return Ok(CredentialBindingState::Missing);
        };
        let comment = credential_comment(allocation.credential()?)?;

        if origin_metadata_matches(comment.as_deref(), expected_origin)? {
            Ok(CredentialBindingState::Available)
        } else {
            Ok(CredentialBindingState::NeedsRebind)
        }
    }

    pub(super) fn read_origin_bound_credential(
        target: &str,
        expected_origin: &str,
    ) -> Result<CredentialReadResult, String> {
        let Some(allocation) = read_allocation(target)? else {
            return Ok(CredentialReadResult::Missing);
        };
        let credential = allocation.credential()?;
        let comment = credential_comment(credential)?;
        if !origin_metadata_matches(comment.as_deref(), expected_origin)? {
            return Ok(CredentialReadResult::NeedsRebind);
        }

        let username =
            read_wide_string(credential.UserName.cast_const(), MAX_USERNAME_UTF16_UNITS)?
                .map(SensitiveString::new);
        let blob_size = credential.CredentialBlobSize as usize;
        if blob_size == 0 || blob_size > MAX_SECRET_BYTES || credential.CredentialBlob.is_null() {
            return Err("The stored credential is invalid.".into());
        }

        // SAFETY: CredentialBlobSize describes the writable blob owned by the
        // CredReadW allocation. We copy it once, then overwrite that source
        // before the enclosing allocation is freed.
        let blob = unsafe { std::slice::from_raw_parts_mut(credential.CredentialBlob, blob_size) };
        let copied_secret = blob.to_vec();
        blob.fill(0);
        let secret = SensitiveString::from_bytes(copied_secret)?;

        Ok(CredentialReadResult::Available(
            OriginBoundCredential::from_sensitive(username, secret),
        ))
    }

    pub(super) fn write_credential(
        target: String,
        username: Option<String>,
        secret: String,
        canonical_origin: &str,
    ) -> Result<(), String> {
        let metadata = validate_origin_metadata(canonical_origin)?;
        let mut target = wide_null_terminated(&target);
        let mut comment = wide_null_terminated(&metadata);
        let mut username = username.map(|mut value| {
            let encoded = wide_null_terminated(&value);
            // SAFETY: replacing every byte with zero preserves UTF-8 validity.
            unsafe { value.as_bytes_mut() }.fill(0);
            encoded
        });
        let mut secret = secret.into_bytes();
        let username_pointer = username
            .as_mut()
            .map_or(null_mut(), |value| value.as_mut_ptr());

        let credential = CredentialW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            LastWritten: FileTime {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            },
            CredentialBlobSize: secret.len() as u32,
            CredentialBlob: secret.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: null_mut(),
            TargetAlias: null_mut(),
            UserName: username_pointer,
        };

        // SAFETY: all pointers in `credential` either point to mutable buffers
        // that outlive the call or are null, and all size fields match them.
        let succeeded = unsafe { CredWriteW(&credential, 0) } != 0;

        // Minimize the lifetime of plaintext copies retained by our temporary
        // buffers. The Windows credential store owns its encrypted copy now.
        secret.fill(0);
        if let Some(username) = &mut username {
            username.fill(0);
        }

        if succeeded {
            Ok(())
        } else {
            Err(vault_unavailable())
        }
    }

    pub(super) fn delete_credential(target: &str) -> Result<(), String> {
        let target = wide_null_terminated(target);

        // SAFETY: `target` is NUL-terminated and valid for the duration of the
        // synchronous CredDeleteW call.
        let succeeded = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0;

        if succeeded || std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NOT_FOUND) {
            Ok(())
        } else {
            Err(vault_unavailable())
        }
    }

    fn read_allocation(target: &str) -> Result<Option<CredentialAllocation>, String> {
        let target = wide_null_terminated(target);
        let mut credential = null_mut();

        // SAFETY: `target` is NUL-terminated and remains alive for the call. The
        // out-pointer is initialized to null and guarded after success.
        let succeeded =
            unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } != 0;

        if succeeded {
            if credential.is_null() {
                Err(vault_unavailable())
            } else {
                Ok(Some(CredentialAllocation(credential)))
            }
        } else if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NOT_FOUND) {
            Ok(None)
        } else {
            Err(vault_unavailable())
        }
    }

    fn credential_comment(credential: &CredentialW) -> Result<Option<String>, String> {
        read_wide_string(
            credential.Comment.cast_const(),
            super::MAX_CREDENTIAL_COMMENT_UTF16_UNITS,
        )
    }

    fn read_wide_string(pointer: *const u16, max_units: usize) -> Result<Option<String>, String> {
        if pointer.is_null() {
            return Ok(None);
        }

        let mut length = None;
        for index in 0..=max_units {
            // SAFETY: Windows guarantees CredentialW string fields are
            // NUL-terminated. The explicit API maximum bounds our scan if a
            // corrupted external entry violates that contract.
            if unsafe { *pointer.add(index) } == 0 {
                length = Some(index);
                break;
            }
        }
        let length = length.ok_or_else(|| "The stored credential is invalid.".to_string())?;
        // SAFETY: the bounded scan established that these units precede the
        // terminating NUL in the live CredReadW allocation.
        let units = unsafe { std::slice::from_raw_parts(pointer, length) };
        String::from_utf16(units)
            .map(Some)
            .map_err(|_| "The stored credential is invalid.".into())
    }

    fn wide_null_terminated(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn vault_unavailable() -> String {
        "The Windows credential vault is unavailable.".into()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{CredentialBindingState, CredentialReadResult};

    pub(super) fn credential_exists(_target: &str) -> Result<bool, String> {
        Err(unsupported())
    }

    pub(super) fn write_credential(
        _target: String,
        _username: Option<String>,
        mut secret: String,
        _canonical_origin: &str,
    ) -> Result<(), String> {
        // Keep the unsupported path from retaining a plaintext secret longer
        // than necessary as well.
        unsafe { secret.as_bytes_mut() }.fill(0);
        Err(unsupported())
    }

    pub(super) fn delete_credential(_target: &str) -> Result<(), String> {
        Err(unsupported())
    }

    pub(super) fn inspect_origin_binding(
        _target: &str,
        _expected_origin: &str,
    ) -> Result<CredentialBindingState, String> {
        Err(unsupported())
    }

    pub(super) fn read_origin_bound_credential(
        _target: &str,
        _expected_origin: &str,
    ) -> Result<CredentialReadResult, String> {
        Err(unsupported())
    }

    fn unsupported() -> String {
        "Secure credential storage is available only on Windows.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn credential_kinds_use_the_public_kebab_case_contract() {
        assert_eq!(
            serde_json::to_value(CredentialKind::ALL).unwrap(),
            json!(["api-key", "bearer-token", "http-basic", "username-password"])
        );
    }

    #[test]
    fn status_exposes_only_kind_and_existence() {
        let status = CredentialStatus {
            kind: CredentialKind::HttpBasic,
            exists: true,
        };

        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({ "kind": "http-basic", "exists": true })
        );
    }

    #[test]
    fn service_ids_match_the_application_contract() {
        assert!(validate_service_id("crafty-controller").is_ok());
        assert!(validate_service_id("service2").is_ok());
        assert!(validate_service_id("Crafty").is_err());
        assert!(validate_service_id("double--hyphen").is_err());
        assert!(validate_service_id("contains_space").is_err());
        assert!(validate_service_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn target_names_are_versioned_and_kind_specific() {
        let targets = CredentialKind::ALL.map(|kind| credential_target("homarr", kind));

        assert_eq!(targets[0], "PersonalHub/credentials/v1/homarr/api-key");
        assert_eq!(targets[1], "PersonalHub/credentials/v1/homarr/bearer-token");
        assert_eq!(targets[2], "PersonalHub/credentials/v1/homarr/http-basic");
        assert_eq!(
            targets[3],
            "PersonalHub/credentials/v1/homarr/username-password"
        );
    }

    #[test]
    fn bulk_delete_attempts_every_kind_after_an_error() {
        let mut attempted = Vec::new();
        let result = delete_all_service_credentials_with("homarr", |target| {
            attempted.push(target.to_string());
            if target.ends_with("/bearer-token") {
                Err("redacted test failure".into())
            } else {
                Ok(())
            }
        });

        assert!(result.is_err());
        assert_eq!(attempted.len(), CredentialKind::ALL.len());
        assert!(attempted[0].ends_with("/api-key"));
        assert!(attempted[1].ends_with("/bearer-token"));
        assert!(attempted[2].ends_with("/http-basic"));
        assert!(attempted[3].ends_with("/username-password"));
    }

    #[test]
    fn token_credentials_reject_usernames() {
        assert!(validate_credential(CredentialKind::ApiKey, None, "secret").is_ok());
        assert!(validate_credential(CredentialKind::BearerToken, None, "secret").is_ok());
        assert!(validate_credential(CredentialKind::ApiKey, Some("user"), "secret").is_err());
    }

    #[test]
    fn login_credentials_require_valid_usernames() {
        assert!(validate_credential(CredentialKind::HttpBasic, Some("admin"), "secret").is_ok());
        assert!(validate_credential(CredentialKind::HttpBasic, None, "secret").is_err());
        assert!(validate_credential(CredentialKind::UsernamePassword, Some(""), "secret").is_err());
        assert!(validate_credential(
            CredentialKind::UsernamePassword,
            Some("bad\0name"),
            "secret"
        )
        .is_err());
    }

    #[test]
    fn credential_payloads_are_bounded() {
        assert!(validate_credential(CredentialKind::ApiKey, None, "").is_err());
        assert!(
            validate_credential(CredentialKind::ApiKey, None, &"x".repeat(MAX_SECRET_BYTES))
                .is_ok()
        );
        assert!(validate_credential(
            CredentialKind::ApiKey,
            None,
            &"x".repeat(MAX_SECRET_BYTES + 1)
        )
        .is_err());
        assert!(validate_username(&"x".repeat(MAX_USERNAME_UTF16_UNITS)).is_ok());
        assert!(validate_username(&"x".repeat(MAX_USERNAME_UTF16_UNITS + 1)).is_err());
    }

    #[test]
    fn validation_errors_do_not_echo_sensitive_input() {
        let private_username = "username-that-must-not-escape";
        let private_secret = "secret-that-must-not-escape";
        let username_error = validate_credential(
            CredentialKind::ApiKey,
            Some(private_username),
            private_secret,
        )
        .unwrap_err();
        let oversized_secret = private_secret.repeat(300);
        let secret_error =
            validate_credential(CredentialKind::ApiKey, None, &oversized_secret).unwrap_err();

        assert!(!username_error.contains(private_username));
        assert!(!username_error.contains(private_secret));
        assert!(!secret_error.contains(private_secret));
    }

    #[test]
    fn set_request_uses_camel_case_and_rejects_extra_fields() {
        let valid = serde_json::from_value::<SetServiceCredentialRequest>(json!({
            "serviceId": "homarr",
            "kind": "api-key",
            "secret": "value"
        }));
        let unknown = serde_json::from_value::<SetServiceCredentialRequest>(json!({
            "serviceId": "homarr",
            "kind": "api-key",
            "secret": "value",
            "unexpected": true
        }));

        assert!(valid.is_ok());
        assert!(unknown.is_err());
    }

    #[test]
    fn origin_metadata_is_versioned_and_requires_an_exact_match() {
        let origin = "https://homarr.local:443";
        assert_eq!(
            validate_origin_metadata(origin).unwrap(),
            "PersonalHub-Origin-v1:https://homarr.local:443"
        );
        assert!(origin_metadata_matches(
            Some("PersonalHub-Origin-v1:https://homarr.local:443"),
            origin
        )
        .unwrap());
        assert!(!origin_metadata_matches(None, origin).unwrap());
        assert!(!origin_metadata_matches(
            Some("PersonalHub-Origin-v1:https://homarr.local:8443"),
            origin
        )
        .unwrap());
        assert!(!origin_metadata_matches(
            Some("PersonalHub-Origin-v0:https://homarr.local:443"),
            origin
        )
        .unwrap());
    }

    #[test]
    fn origin_metadata_honors_the_real_credential_comment_limit() {
        let prefix_units = ORIGIN_METADATA_PREFIX.encode_utf16().count();
        let fitting = "x".repeat(MAX_CREDENTIAL_COMMENT_UTF16_UNITS - prefix_units);
        let oversized = "x".repeat(MAX_CREDENTIAL_COMMENT_UTF16_UNITS - prefix_units + 1);

        assert_eq!(
            validate_origin_metadata(&fitting)
                .unwrap()
                .encode_utf16()
                .count(),
            MAX_CREDENTIAL_COMMENT_UTF16_UNITS
        );
        assert!(validate_origin_metadata(&oversized).is_err());
        assert!(validate_origin_metadata("").is_err());
        assert!(validate_origin_metadata("https://bad\0host:443").is_err());
    }

    #[test]
    fn sensitive_strings_are_redacted_and_can_be_zeroized_in_place() {
        let private = "private-value-that-must-not-escape";
        let mut value = SensitiveString::new(private.to_string());

        assert!(!format!("{value:?}").contains(private));
        value.zeroize();
        assert!(value.expose().as_bytes().iter().all(|byte| *byte == 0));
    }

    #[test]
    fn origin_bound_credential_debug_never_exposes_plaintext() {
        let credential =
            OriginBoundCredential::for_test(Some("private-username"), "private-secret");
        let debug = format!("{credential:?}");

        assert!(!debug.contains("private-username"));
        assert!(!debug.contains("private-secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn credential_revisions_are_service_scoped_and_monotonic() {
        let first_service = "vault-revision-test-one";
        let second_service = "vault-revision-test-two";
        let first_before = credential_revision(first_service);
        let second_before = credential_revision(second_service);
        let first_after = bump_credential_revision(first_service);
        let first_latest = bump_credential_revision(first_service);

        assert!(first_after > first_before);
        assert!(first_latest > first_after);
        assert_eq!(credential_revision(first_service), first_latest);
        assert_eq!(credential_revision(second_service), second_before);
    }

    #[test]
    fn server_control_token_validation_is_strict_and_redacted() {
        assert!(validate_server_control_token(&"0123456789abcdef".repeat(4)).is_ok());
        assert!(validate_server_control_token(&"ABCDEF0123456789".repeat(4)).is_ok());
        for invalid in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "é".repeat(32),
            format!("{}\n", "a".repeat(63)),
            format!("{}\0", "a".repeat(63)),
        ] {
            let error = validate_server_control_token(&invalid).unwrap_err();
            if !invalid.is_empty() {
                assert!(!error.contains(&invalid));
            }
        }
    }

    #[test]
    fn server_control_credential_is_not_addressable_by_service_cleanup() {
        let vault_key = server_control_credential_target("192.168.1.10", 9473);
        let bound_origin = server_control_origin("192.168.1.10", 9473);
        assert_eq!(vault_key, "PersonalHub/server-control/v1/192.168.1.10:9473");
        assert_eq!(bound_origin, "https://192.168.1.10:9473");
        // A different enrolled address must never resolve to the same vault key.
        assert_ne!(
            vault_key,
            server_control_credential_target("192.168.0.14", 9473)
        );
        assert!(!vault_key.starts_with(CREDENTIAL_TARGET_PREFIX));
        delete_all_service_credentials_with("server-control", |target| {
            assert_ne!(target, vault_key);
            Ok(())
        })
        .unwrap();
        assert!(validate_service_id("../server-control/v1/192.168.1.10:9473").is_err());
        assert!(
            origin_metadata_matches(Some(&origin_metadata(&bound_origin)), &bound_origin).unwrap()
        );
        for other in [
            "http://192.168.1.10:9473",
            "https://192.168.1.10:9474",
            "https://192.168.0.14:9473",
        ] {
            assert!(
                !origin_metadata_matches(Some(&origin_metadata(other)), &bound_origin).unwrap()
            );
        }
    }
}
