//! Moving between root and the `atrium` service user.
//!
//! `atriumctl` starts as root (the commands are console actions) but does its
//! state-directory work as the service user, so every file it creates there
//! belongs to the service. There is exactly one move, [`drop_permanently`]:
//! real, effective and saved ids all become the service user's, and the drop
//! is verified by trying — and failing — to become root again.
//!
//! There is deliberately no temporary drop. The state directory is writable
//! by the service user, so its contents are data a compromised Core controls;
//! parsing them in a process that kept a saved uid of 0 would give that data a
//! way back to root. Every command therefore finishes whatever it needs root
//! for, then drops for good, then touches state.

use nix::unistd::{geteuid, setgroups, setresgid, setresuid, Gid, Uid, User};

/// The service account's name, as the installer creates it.
pub const SERVICE_USER: &str = "atrium";

/// The service account's ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceAccount {
    /// User id.
    pub uid: Uid,
    /// Primary group id.
    pub gid: Gid,
}

/// Looks up the service account.
///
/// # Errors
///
/// When it does not exist — Atrium is not installed on this machine.
pub fn service_account() -> Result<ServiceAccount, String> {
    match User::from_name(SERVICE_USER) {
        Ok(Some(user)) => Ok(ServiceAccount {
            uid: user.uid,
            gid: user.gid,
        }),
        Ok(None) => Err(format!(
            "there is no `{SERVICE_USER}` user on this machine; is Atrium installed?"
        )),
        Err(error) => Err(format!(
            "could not look up the `{SERVICE_USER}` user: {error}"
        )),
    }
}

/// Becomes the service user for good.
///
/// # Errors
///
/// When any step fails, or when root can still be regained afterwards.
pub fn drop_permanently(account: ServiceAccount) -> Result<(), String> {
    setgroups(&[account.gid]).map_err(|e| format!("setgroups: {e}"))?;
    setresgid(account.gid, account.gid, account.gid).map_err(|e| format!("setresgid: {e}"))?;
    setresuid(account.uid, account.uid, account.uid).map_err(|e| format!("setresuid: {e}"))?;
    if setresuid(Uid::from_raw(0), Uid::from_raw(0), Uid::from_raw(0)).is_ok() {
        return Err("privileges were not dropped: root could be regained".to_owned());
    }
    if geteuid() != account.uid {
        return Err("privileges were not dropped: unexpected effective uid".to_owned());
    }
    Ok(())
}
