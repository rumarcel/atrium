//! The console commands, and what they share.

pub mod diagnostics;
pub mod pair;
pub mod restore;
pub mod rotate_identity;

use std::io::BufRead;
use std::os::unix::fs::MetadataExt;
use std::process::ExitCode;

use atrium_core::layout::Layout;

use crate::privilege::{self, ServiceAccount};

/// Where the command runs and as whom.
pub struct Context {
    /// The tree.
    pub layout: Layout,
    /// The service account when running as root; `None` when already an
    /// unprivileged user, which is how the unprivileged test suite drives
    /// these commands against a tree it owns.
    pub account: Option<ServiceAccount>,
}

impl Context {
    /// Resolves the tree and the account.
    ///
    /// # Errors
    ///
    /// When the layout override is invalid or unsafe, or when running as root
    /// on a machine with no service account.
    pub fn resolve() -> Result<Self, String> {
        let layout = Layout::from_env().map_err(|error| error.to_string())?;
        if !nix::unistd::geteuid().is_root() {
            return Ok(Self {
                layout,
                account: None,
            });
        }
        // Root honouring a relocated tree must not be steered into a tree
        // someone else controls.
        if let Some(root) = layout.root() {
            let metadata = std::fs::symlink_metadata(root)
                .map_err(|error| format!("{}: {error}", root.display()))?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                return Err(format!(
                    "{} must be a root-owned directory writable only by root",
                    root.display()
                ));
            }
        }
        Ok(Self {
            layout,
            account: Some(privilege::service_account()?),
        })
    }

    /// Becomes the service user for good, when running as root.
    ///
    /// # Errors
    ///
    /// See [`privilege::drop_permanently`].
    pub fn drop_to_service(&self) -> Result<(), String> {
        match self.account {
            Some(account) => privilege::drop_permanently(account),
            None => Ok(()),
        }
    }
}

/// Asks the operator to type `expected`. Anything else, including end of
/// input, is a refusal.
///
/// # Errors
///
/// When the answer does not match.
pub fn confirm(expected: &str) -> Result<(), String> {
    eprint!("Type the server name ({expected}) to confirm: ");
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|error| format!("could not read the confirmation: {error}"))?;
    if answer.trim_end_matches(['\n', '\r']) == expected {
        Ok(())
    } else {
        Err("the name did not match; nothing was changed".to_owned())
    }
}

/// Prints a refusal and returns the failure code.
pub fn fail(command: &str, message: impl std::fmt::Display) -> ExitCode {
    eprintln!("atriumctl {command}: {message}");
    ExitCode::FAILURE
}
