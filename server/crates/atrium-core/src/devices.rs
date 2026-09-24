//! Paired devices (M1E): authentication, `me`, listing and revocation.
//!
//! **Authentication** is one function, [`Devices::authenticate`], called at
//! exactly one place — the `Auth::Device` point in `http::dispatch` — before
//! any device handler runs. It accepts only
//! `Authorization: Bearer <43 characters of canonical base64url>`, hashes the
//! 32 decoded bytes with SHA-256, looks the digest up through the unique
//! index and compares the stored digest in constant time. There is no cache
//! of any kind between the request and that lookup (plan §7.3): a revoked
//! device's row is gone, so its next request finds nothing.
//!
//! A malformed header, an unknown token and a revoked token all produce the
//! same `None`, and therefore the same `401 auth.unauthorized`. A database
//! failure is not a `None`: it is an internal error, because telling a
//! client "unauthorized" when the server merely failed would make it
//! discard a valid token.
//!
//! Nothing here returns a token or a digest. [`DeviceView`] is the whole of
//! what leaves the server about a device.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

use atrium_pairing::token::DeviceToken;

use crate::db::pairing::DeviceRow;
use crate::db::Database;
use crate::identity::ServerId;
use crate::pairing::Clock;

/// The request is authenticated as this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authenticated {
    /// 32 lowercase hex.
    pub device_id: String,
    /// Display name.
    pub name: String,
    /// Platform.
    pub platform: String,
    /// Always `owner` in M1.
    pub role: String,
}

/// Core could not reach its state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

/// `GET /api/v1/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// This device.
    pub device_id: String,
    /// Its display name.
    pub name: String,
    /// Its platform.
    pub platform: String,
    /// Its role.
    pub role: String,
    /// The server.
    pub server_id: String,
}

/// One device in `GET /api/v1/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    /// Identifier.
    pub device_id: String,
    /// Display name.
    pub name: String,
    /// Platform.
    pub platform: String,
    /// Role.
    pub role: String,
    /// When it was paired.
    pub created_at: String,
    /// When it was last seen, to the minute.
    pub last_seen_at: Option<String>,
    /// Whether it is the device asking.
    pub current: bool,
}

/// `GET /api/v1/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceList {
    /// Every device, oldest first.
    pub items: Vec<DeviceView>,
    /// Always `null` in M1: the list is complete (see [`Devices::list`]).
    pub next_cursor: Option<String>,
}

/// The bearer token in an `Authorization` value, if it is exactly
/// `Bearer <token>`. The scheme is matched case-insensitively (RFC 9110
/// §11.1); everything else is exact.
#[must_use]
pub fn parse_bearer(value: &[u8]) -> Option<DeviceToken> {
    let text = std::str::from_utf8(value).ok()?;
    let (scheme, token) = text.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    DeviceToken::parse(token)
}

/// Device authentication and management.
pub struct Devices {
    database: Arc<Mutex<Database>>,
    server_id: ServerId,
    clock: Clock,
}

impl std::fmt::Debug for Devices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Devices").finish_non_exhaustive()
    }
}

fn log_unavailable(step: &'static str, error: &rusqlite::Error) -> Unavailable {
    tracing::error!(
        event = "devices_unavailable",
        component = "atrium-core",
        step = step,
        reason = %error,
        "the device table could not be used"
    );
    Unavailable
}

impl Devices {
    /// Devices in `database`.
    #[must_use]
    pub fn new(database: Arc<Mutex<Database>>, server_id: ServerId, clock: Clock) -> Self {
        Self {
            database,
            server_id,
            clock,
        }
    }

    fn database(&self) -> Result<MutexGuard<'_, Database>, Unavailable> {
        self.database.lock().map_err(|_| Unavailable)
    }

    /// The device an `Authorization` header value names, or `None`.
    ///
    /// # Errors
    ///
    /// [`Unavailable`] when the lookup itself failed.
    pub fn authenticate(
        &self,
        authorization: Option<&[u8]>,
    ) -> Result<Option<Authenticated>, Unavailable> {
        let Some(token) = authorization.and_then(parse_bearer) else {
            return Ok(None);
        };
        let digest = token.digest();
        drop(token);
        let database = self.database()?;
        let Some(row) = database
            .device_by_digest(&digest)
            .map_err(|error| log_unavailable("lookup", &error))?
        else {
            return Ok(None);
        };
        // Best effort, at most once a minute: a failed touch never fails
        // the request it describes.
        if let Err(error) = database.touch_device(&row.device_id, (self.clock)()) {
            tracing::warn!(
                event = "device_touch_failed",
                component = "atrium-core",
                reason = %error,
                "could not record when a device was last seen"
            );
        }
        Ok(Some(Authenticated {
            device_id: row.device_id,
            name: row.name,
            platform: row.platform,
            role: row.role,
        }))
    }

    /// `GET /api/v1/me`.
    #[must_use]
    pub fn me(&self, device: &Authenticated) -> Me {
        Me {
            device_id: device.device_id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            role: device.role.clone(),
            server_id: self.server_id.to_string(),
        }
    }

    /// `GET /api/v1/devices`.
    ///
    /// Complete rather than paginated: a device exists only by consuming a
    /// console-armed, single-use secret, so the table grows by at most one
    /// row per console action.
    ///
    /// # Errors
    ///
    /// [`Unavailable`].
    pub fn list(&self, current: &Authenticated) -> Result<DeviceList, Unavailable> {
        let rows = self
            .database()?
            .list_devices()
            .map_err(|error| log_unavailable("list", &error))?;
        Ok(DeviceList {
            items: rows
                .into_iter()
                .map(|row: DeviceRow| DeviceView {
                    current: row.device_id == current.device_id,
                    device_id: row.device_id,
                    name: row.name,
                    platform: row.platform,
                    role: row.role,
                    created_at: row.created_at,
                    last_seen_at: row.last_seen_at,
                })
                .collect(),
            next_cursor: None,
        })
    }

    /// `DELETE /api/v1/devices/{deviceId}`: deletes the row and its
    /// verifier, with the audit row, in one transaction. `Ok(false)` when
    /// there is no such device. A device may revoke itself.
    ///
    /// # Errors
    ///
    /// [`Unavailable`]; nothing was changed.
    pub fn revoke(&self, current: &Authenticated, target: &str) -> Result<bool, Unavailable> {
        let revoked = self
            .database()?
            .revoke_device(target, &current.device_id, (self.clock)())
            .map_err(|error| log_unavailable("revoke", &error))?;
        if revoked {
            tracing::info!(
                event = "device_revoked",
                component = "atrium-core",
                device_id = %target,
                by = %current.device_id,
                "a device was revoked"
            );
        }
        Ok(revoked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

    #[test]
    fn only_a_bearer_token_in_canonical_form_parses() {
        assert!(parse_bearer(format!("Bearer {TOKEN}").as_bytes()).is_some());
        assert!(parse_bearer(format!("bearer {TOKEN}").as_bytes()).is_some());
        for bad in [
            String::new(),
            TOKEN.to_owned(),
            format!("Bearer  {TOKEN}"),
            format!("Bearer {TOKEN} "),
            format!("Basic {TOKEN}"),
            format!("Bearer {TOKEN}="),
            format!("Bearer {}", &TOKEN[..42]),
            format!("Bearer\t{TOKEN}"),
            "Bearer ".to_owned(),
        ] {
            assert!(parse_bearer(bad.as_bytes()).is_none(), "{bad:?}");
        }
        assert!(parse_bearer(b"Bearer \xff\xfe").is_none());
    }
}
