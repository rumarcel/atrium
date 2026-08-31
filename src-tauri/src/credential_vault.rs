use crate::{service_settings::ServiceSettings, service_webviews::ServiceCatalog};
use serde::{Deserialize, Serialize};
use tauri::Webview;

const MAIN_WEBVIEW_LABEL: &str = "main";
const CREDENTIAL_TARGET_PREFIX: &str = "PersonalHub/credentials/v1";
const MAX_SERVICE_ID_LENGTH: usize = 64;
const MAX_SECRET_BYTES: usize = 2_560;
const MAX_USERNAME_UTF16_UNITS: usize = 513;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    ApiKey,
    BearerToken,
    HttpBasic,
    UsernamePassword,
}

impl CredentialKind {
    const ALL: [Self; 4] = [
        Self::ApiKey,
        Self::BearerToken,
        Self::HttpBasic,
        Self::UsernamePassword,
    ];

    const fn target_suffix(self) -> &'static str {
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
    catalog: tauri::State<'_, ServiceCatalog>,
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

    platform::write_credential(credential_target(&service_id, kind), username, secret)?;

    Ok(CredentialStatus { kind, exists: true })
}

#[tauri::command]
pub fn delete_service_credential(
    caller: Webview,
    request: DeleteServiceCredentialRequest,
    catalog: tauri::State<'_, ServiceCatalog>,
    settings: tauri::State<'_, ServiceSettings>,
) -> Result<CredentialStatus, String> {
    ensure_trusted_caller(&caller)?;
    validate_service_id(&request.service_id)?;
    let _operation = settings.lock_operation()?;
    ensure_known_service(&catalog, &request.service_id)?;

    platform::delete_credential(&credential_target(&request.service_id, request.kind))?;

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
    delete_all_service_credentials_with(service_id, platform::delete_credential)
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

#[cfg(windows)]
mod platform {
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

    pub(super) fn credential_exists(target: &str) -> Result<bool, String> {
        let target = wide_null_terminated(target);
        let mut credential = null_mut();

        // SAFETY: `target` is NUL-terminated and remains alive for the call. The
        // out-pointer is initialized to null and is released with CredFree only
        // when Windows reports success.
        let succeeded =
            unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } != 0;

        if succeeded {
            if credential.is_null() {
                return Err(vault_unavailable());
            }

            // SAFETY: a successful CredReadW call owns this allocation and the
            // API requires it to be released with CredFree.
            unsafe { CredFree(credential.cast()) };
            return Ok(true);
        }

        if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_NOT_FOUND) {
            Ok(false)
        } else {
            Err(vault_unavailable())
        }
    }

    pub(super) fn write_credential(
        target: String,
        username: Option<String>,
        secret: String,
    ) -> Result<(), String> {
        let mut target = wide_null_terminated(&target);
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
            Comment: null_mut(),
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

    fn wide_null_terminated(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn vault_unavailable() -> String {
        "The Windows credential vault is unavailable.".into()
    }
}

#[cfg(not(windows))]
mod platform {
    pub(super) fn credential_exists(_target: &str) -> Result<bool, String> {
        Err(unsupported())
    }

    pub(super) fn write_credential(
        _target: String,
        _username: Option<String>,
        mut secret: String,
    ) -> Result<(), String> {
        // Keep the unsupported path from retaining a plaintext secret longer
        // than necessary as well.
        unsafe { secret.as_bytes_mut() }.fill(0);
        Err(unsupported())
    }

    pub(super) fn delete_credential(_target: &str) -> Result<(), String> {
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
}
