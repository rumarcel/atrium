//! `AgentInfo`: Agent's own identity and state.
//!
//! Every field is read from this process or from the kernel at the time of
//! the call, apart from `started_at`, which is recorded once at startup. A
//! value that cannot be read is `null`, never a default that looks real. No
//! environment variable, secret, token, key or root-only file content is
//! read here, and the one path reported is Agent's own state directory.

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use atrium_protocol::messages::{AgentInfo, JournalInfo, StateDirInfo};
use atrium_protocol::values::{BootId, FileMode, ReportedPath, Timestamp, Version};
use atrium_protocol::AGENT_PROTOCOL_VERSION;

use crate::journal::Journal;

/// Largest `/proc` file read here.
const PROC_READ_LIMIT: u64 = 16 * 1024;

/// Facts fixed when the process started.
#[derive(Debug, Clone)]
pub struct Startup {
    /// When Agent started.
    pub started_at: Timestamp,
    /// Agent's build version.
    pub version: Version,
}

/// Collects `AgentInfo` now.
#[must_use]
pub fn collect(startup: &Startup, journal: &Journal) -> AgentInfo {
    let status = read_small(Path::new("/proc/self/status"));
    let status = status.as_deref().unwrap_or("");
    AgentInfo {
        agent_version: startup.version.clone(),
        protocol: AGENT_PROTOCOL_VERSION,
        uid: nix::unistd::geteuid().as_raw(),
        pid: std::process::id(),
        started_at: startup.started_at.clone(),
        boot_id: read_small(Path::new("/proc/sys/kernel/random/boot_id"))
            .and_then(|text| BootId::parse(text.trim_end()).ok()),
        state_dir: state_dir(journal.state_dir()),
        journal: JournalInfo {
            writable: journal.writable(),
            last_seq: journal.last_seq(),
        },
        capability_bounding_set_empty: status_field(status, "CapBnd")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .map(|bits| bits == 0),
        no_new_privileges: status_field(status, "NoNewPrivs").and_then(|value| match value {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        }),
    }
}

fn state_dir(path: &Path) -> StateDirInfo {
    let metadata = std::fs::symlink_metadata(path).ok();
    StateDirInfo {
        // The directory is chosen by Agent, never by Core; it is reported as
        // a validated display value, or as the production constant when the
        // configured one cannot be expressed as one (it always can: it was
        // validated when Agent started).
        path: path
            .to_str()
            .and_then(|text| ReportedPath::parse(text).ok())
            .unwrap_or_else(|| {
                ReportedPath::parse(atrium_protocol::AGENT_STATE_DIR).expect("constant")
            }),
        mode: metadata.as_ref().map(|m| FileMode::from_bits(m.mode())),
        uid: metadata.as_ref().map(MetadataExt::uid),
        gid: metadata.as_ref().map(MetadataExt::gid),
    }
}

fn read_small(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(PROC_READ_LIMIT)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The value of `Name:\tvalue` in a `/proc/<pid>/status` text.
fn status_field<'a>(status: &'a str, name: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key == name).then(|| value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tatrium-agent\nCapBnd:\t0000000000000000\nNoNewPrivs:\t1\n";

    #[test]
    fn status_fields_are_parsed_exactly() {
        assert_eq!(status_field(STATUS, "CapBnd"), Some("0000000000000000"));
        assert_eq!(status_field(STATUS, "NoNewPrivs"), Some("1"));
        assert_eq!(status_field(STATUS, "CapBn"), None);
        assert_eq!(status_field(STATUS, "Missing"), None);
    }

    #[test]
    fn this_process_is_described_truthfully() {
        let startup = Startup {
            started_at: Timestamp::parse("2026-09-24T10:00:00.000Z").expect("ts"),
            version: Version::parse("0.1.0").expect("version"),
        };
        let journal = Journal::new(std::env::temp_dir(), nix::unistd::geteuid().as_raw());
        let info = collect(&startup, &journal);
        assert_eq!(info.uid, nix::unistd::geteuid().as_raw());
        assert_eq!(info.pid, std::process::id());
        assert_eq!(info.protocol, AGENT_PROTOCOL_VERSION);
        // A test process runs with the full bounding set and without
        // no_new_privs unless its parent set them; whatever the truth is,
        // it must be reported, not assumed.
        let status = std::fs::read_to_string("/proc/self/status").expect("status");
        let cap = status_field(&status, "CapBnd").expect("CapBnd");
        assert_eq!(
            info.capability_bounding_set_empty,
            Some(u64::from_str_radix(cap, 16).expect("hex") == 0)
        );
        assert!(info.no_new_privileges.is_some());
        assert!(!info.journal.writable, "not opened yet, so not claimed");
    }
}
