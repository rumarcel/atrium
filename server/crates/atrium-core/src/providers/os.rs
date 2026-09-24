//! System identity (plan §9.1): OS release, kernel, hostname, uptime and
//! boot id. `arch`, `serverId` and `coreVersion` are always known and are
//! added by the caller.

use serde::Serialize;

use super::{
    read_line, read_source, reason_for, safe_text, Absences, HostRoot, ReadFailure, Reason,
    Sourced, Unavailable, SMALL_SOURCE,
};

/// The OS, from `os-release(5)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OsRelease {
    /// `ID`.
    pub id: Option<String>,
    /// `VERSION_ID`.
    pub version_id: Option<String>,
    /// `PRETTY_NAME`.
    pub pretty_name: Option<String>,
}

/// The kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kernel {
    /// `ostype` (`Linux`).
    pub name: Option<String>,
    /// `osrelease`.
    pub release: Option<String>,
    /// `version`.
    pub version: Option<String>,
}

/// Everything read from the host for `GET /api/v1/system`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    /// The OS.
    pub os: OsRelease,
    /// The kernel.
    pub kernel: Kernel,
    /// The kernel's hostname.
    pub hostname: Option<String>,
    /// Whole seconds since boot.
    pub uptime_seconds: Option<u64>,
    /// This boot's id.
    pub boot_id: Option<String>,
    /// What could not be read, and why.
    pub unavailable: Vec<Unavailable>,
}

/// Largest value kept from `os-release` or the kernel's text files.
const MAX_TEXT: usize = 256;

/// Parses `os-release(5)`: `KEY=value`, the value optionally quoted with
/// `"` or `'`, backslash escapes inside double quotes. Returns the value of
/// `key`, or `None` when absent; `Err` when present but malformed.
fn os_release_value(text: &str, key: &str) -> Result<Option<String>, ()> {
    let mut found = None;
    for line in text.lines().take(512) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, raw)) = line.split_once('=') else {
            continue;
        };
        if name != key {
            continue;
        }
        let value = unquote(raw).ok_or(())?;
        found = Some(safe_text(&value, MAX_TEXT).ok_or(())?);
    }
    Ok(found)
}

fn unquote(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    match bytes.first() {
        Some(b'"') => {
            let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => out.push(chars.next()?),
                    '"' | '`' | '$' => return None,
                    c => out.push(c),
                }
            }
            Some(out)
        }
        Some(b'\'') => {
            let inner = raw.strip_prefix('\'')?.strip_suffix('\'')?;
            (!inner.contains('\'')).then(|| inner.to_owned())
        }
        _ => (!raw.contains([' ', '"', '\'', '\\', '`', '$'])).then(|| raw.to_owned()),
    }
}

fn read_os_release(root: &HostRoot, absences: &mut Absences) -> OsRelease {
    let text = read_source(&root.path("etc/os-release"), SMALL_SOURCE).or_else(|first| {
        if first == ReadFailure::Missing {
            read_source(&root.path("usr/lib/os-release"), SMALL_SOURCE)
        } else {
            Err(first)
        }
    });
    let Ok(text) = text else {
        let reason = reason_for(
            text.err().unwrap_or(ReadFailure::Missing),
            Reason::OsReleaseUnreadable,
        );
        return OsRelease {
            id: absences.none("os.id", reason),
            version_id: absences.none("os.versionId", reason),
            pretty_name: absences.none("os.prettyName", reason),
        };
    };
    let mut field = |key: &str, name: &str| match os_release_value(&text, key) {
        Ok(Some(value)) => Some(value),
        Ok(None) => absences.none(name, Reason::NotReported),
        Err(()) => absences.none(name, Reason::SourceMalformed),
    };
    OsRelease {
        id: field("ID", "os.id"),
        version_id: field("VERSION_ID", "os.versionId"),
        pretty_name: field("PRETTY_NAME", "os.prettyName"),
    }
}

/// A one-line kernel text value.
fn kernel_text(root: &HostRoot, relative: &str) -> Sourced<String> {
    let line = read_line(&root.path(relative))
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable))?;
    safe_text(&line, MAX_TEXT).ok_or(Reason::SourceMalformed)
}

/// `/proc/uptime`'s first field, in whole seconds.
pub(crate) fn uptime_seconds(root: &HostRoot) -> Sourced<u64> {
    let line = read_line(&root.path("proc/uptime"))
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable))?;
    let first = line
        .split_whitespace()
        .next()
        .ok_or(Reason::SourceMalformed)?;
    let seconds: f64 = first.parse().map_err(|_| Reason::SourceMalformed)?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > 1.0e12 {
        return Err(Reason::SourceMalformed);
    }
    // Truncation to whole seconds is the documented unit.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(seconds.floor() as u64)
}

fn boot_id(root: &HostRoot) -> Sourced<String> {
    let line = read_line(&root.path("proc/sys/kernel/random/boot_id"))
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable))?;
    let valid = line.len() == 36
        && line.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit() && !c.is_ascii_uppercase()
            }
        });
    valid.then_some(line).ok_or(Reason::SourceMalformed)
}

/// Reads the host's identity. Never fails: every field degrades alone.
#[must_use]
pub fn read(root: &HostRoot) -> HostIdentity {
    let mut absences = Absences::default();
    let os = read_os_release(root, &mut absences);
    let kernel = Kernel {
        name: absences.take("kernel.name", kernel_text(root, "proc/sys/kernel/ostype")),
        release: absences.take(
            "kernel.release",
            kernel_text(root, "proc/sys/kernel/osrelease"),
        ),
        version: absences.take(
            "kernel.version",
            kernel_text(root, "proc/sys/kernel/version"),
        ),
    };
    let hostname = absences.take("hostname", kernel_text(root, "proc/sys/kernel/hostname"));
    let uptime_seconds = absences.take("uptimeSeconds", uptime_seconds(root));
    let boot_id = absences.take("bootId", boot_id(root));
    HostIdentity {
        os,
        kernel,
        hostname,
        uptime_seconds,
        boot_id,
        unavailable: absences.into_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fixture::Tree;

    #[test]
    fn os_release_is_parsed_strictly() {
        let text = "NAME=\"Debian GNU/Linux\"\nID=debian\nVERSION_ID=\"12\"\n\
                    PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\n# ID=ignored\n";
        assert_eq!(os_release_value(text, "ID"), Ok(Some("debian".into())));
        assert_eq!(os_release_value(text, "VERSION_ID"), Ok(Some("12".into())));
        assert_eq!(
            os_release_value(text, "PRETTY_NAME"),
            Ok(Some("Debian GNU/Linux 12 (bookworm)".into()))
        );
        assert_eq!(os_release_value(text, "BUILD_ID"), Ok(None));
        assert_eq!(os_release_value("ID='x y'", "ID"), Ok(Some("x y".into())));
        assert_eq!(
            os_release_value("ID=\"a\\\"b\"", "ID"),
            Ok(Some("a\"b".into()))
        );
        for bad in [
            "ID=\"open",
            "ID=a b",
            "ID=\"$(x)\"",
            "ID=\"\"",
            "ID=\"a\u{7}\"",
        ] {
            assert_eq!(os_release_value(bad, "ID"), Err(()), "{bad:?}");
        }
    }

    #[test]
    fn a_full_tree_reads_every_field() {
        let tree = Tree::new();
        tree.write(
            "etc/os-release",
            "ID=ubuntu\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\n",
        );
        tree.write("proc/sys/kernel/ostype", "Linux\n");
        tree.write("proc/sys/kernel/osrelease", "6.8.0-45-generic\n");
        tree.write(
            "proc/sys/kernel/version",
            "#45-Ubuntu SMP PREEMPT_DYNAMIC\n",
        );
        tree.write("proc/sys/kernel/hostname", "atrium-box\n");
        tree.write("proc/uptime", "12345.67 40000.00\n");
        tree.write(
            "proc/sys/kernel/random/boot_id",
            "3c9d2f4e-8a1b-4c2d-9e0f-1a2b3c4d5e6f\n",
        );
        let identity = read(&tree.root());
        assert!(
            identity.unavailable.is_empty(),
            "{:?}",
            identity.unavailable
        );
        assert_eq!(identity.os.id.as_deref(), Some("ubuntu"));
        assert_eq!(identity.kernel.release.as_deref(), Some("6.8.0-45-generic"));
        assert_eq!(identity.hostname.as_deref(), Some("atrium-box"));
        assert_eq!(identity.uptime_seconds, Some(12345));
    }

    #[test]
    fn os_release_falls_back_to_usr_lib() {
        let tree = Tree::new();
        tree.write("usr/lib/os-release", "ID=fedora\n");
        let identity = read(&tree.root());
        assert_eq!(identity.os.id.as_deref(), Some("fedora"));
        assert!(identity.unavailable.contains(&Unavailable {
            field: "os.versionId".into(),
            reason: Reason::NotReported
        }));
    }

    #[test]
    fn os_release_is_followed_through_its_conventional_symlink() {
        let tree = Tree::new();
        tree.write("usr/lib/os-release", "ID=debian\nVERSION_ID=\"13\"\n");
        tree.symlink(&tree.path("usr/lib/os-release"), "etc/os-release");
        let identity = read(&tree.root());
        assert_eq!(identity.os.id.as_deref(), Some("debian"));
        assert_eq!(identity.os.version_id.as_deref(), Some("13"));
    }

    #[test]
    fn malformed_values_degrade_their_field_only() {
        let tree = Tree::new();
        tree.write("proc/uptime", "soon 1\n");
        tree.write("proc/sys/kernel/random/boot_id", "not-a-uuid\n");
        tree.write("proc/sys/kernel/hostname", "\u{1b}[31mred\n");
        tree.write("proc/sys/kernel/ostype", "Linux\n");
        let identity = read(&tree.root());
        assert_eq!(identity.kernel.name.as_deref(), Some("Linux"));
        assert_eq!(identity.uptime_seconds, None);
        assert_eq!(identity.boot_id, None);
        assert_eq!(identity.hostname, None);
        for field in ["uptimeSeconds", "bootId", "hostname"] {
            assert!(identity.unavailable.contains(&Unavailable {
                field: field.into(),
                reason: Reason::SourceMalformed
            }));
        }
        assert!(identity.unavailable.contains(&Unavailable {
            field: "os.id".into(),
            reason: Reason::OsReleaseUnreadable
        }));
    }
}
