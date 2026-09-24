//! Mounted filesystems and their capacity (plan §9.4).
//!
//! Mounts come from `/proc/self/mountinfo` (not `/etc/mtab`). Pseudo
//! filesystems are excluded by the plan's fstype denylist ([`EXCLUDED`]).
//! Bind mounts and duplicates are collapsed by device (`major:minor`): the
//! first mount point in mountinfo order is the filesystem's, and the others
//! are listed as `alsoMountedAt` — nothing is hidden.
//!
//! Capacity comes from `statvfs(3)`. A mount whose `statvfs` fails — no
//! permission, gone, or not answering within [`STATVFS_TIMEOUT`], as a dead
//! network mount does — is **still listed**, with `usage: null` and
//! `metadata_unavailable`. A mount whose previous `statvfs` is still stuck is
//! not asked again until that call returns, so a hung mount costs at most
//! one blocked thread however often the route is called.
//!
//! Sizes are bytes: `total = f_blocks × f_frsize`, `free = f_bfree ×
//! f_frsize` (including blocks reserved for root), `available = f_bavail ×
//! f_frsize` (what an unprivileged writer can use), `used = total − free`.
//! Every product is checked; an overflow or `f_bfree > f_blocks` is
//! `source_malformed`, never a wrapped number.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use super::{
    read_source, reason_for, safe_text, Absences, HostRoot, Reason, Sourced, Unavailable,
    LARGE_SOURCE,
};

/// Plan §9.4's denylist, exactly.
pub const EXCLUDED: [&str; 23] = [
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "tmpfs",
    "cgroup",
    "cgroup2",
    "pstore",
    "bpf",
    "tracefs",
    "debugfs",
    "securityfs",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "efivarfs",
    "ramfs",
    "squashfs",
    "overlay",
    "nsfs",
];
/// Longest a single `statvfs` is waited for.
pub const STATVFS_TIMEOUT: Duration = Duration::from_secs(2);
/// Mountinfo lines examined at most.
const MAX_MOUNTS: usize = 4096;
/// `statvfs` calls in flight at once for one request.
const PARALLEL: usize = 16;

/// Capacity, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Size.
    pub total_bytes: u64,
    /// `total − free`.
    pub used_bytes: u64,
    /// Free, including blocks reserved for root.
    pub free_bytes: u64,
    /// Free to an unprivileged writer.
    pub available_bytes: u64,
}

/// One mounted filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Filesystem {
    /// Where it is mounted (the first mount point, in mountinfo order).
    pub mount_point: String,
    /// Its other mount points (bind mounts).
    pub also_mounted_at: Vec<String>,
    /// The filesystem type.
    pub fs_type: String,
    /// The mount source (a device path, `server:/export`, …).
    pub source: Option<String>,
    /// Mounted read-only.
    pub read_only: bool,
    /// Capacity, or `null` with a reason.
    pub usage: Option<Usage>,
    /// What could not be read, and why.
    pub unavailable: Vec<Unavailable>,
}

/// `GET /api/v1/storage/filesystems`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Filesystems {
    /// Every real filesystem.
    pub filesystems: Vec<Filesystem>,
    /// What could not be read for the whole list, and why.
    pub unavailable: Vec<Unavailable>,
}

/// One parsed mountinfo line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// `major:minor`.
    pub device: String,
    /// The mount point's bytes, unescaped.
    pub mount_point: Vec<u8>,
    /// The filesystem type.
    pub fs_type: String,
    /// The source, unescaped.
    pub source: Vec<u8>,
    /// `ro` among the mount options.
    pub read_only: bool,
}

/// Undoes mountinfo's octal escapes (`\040` for a space, and so on).
fn unescape(field: &str) -> Option<Vec<u8>> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let digits = bytes.get(i + 1..i + 4)?;
            let text = std::str::from_utf8(digits).ok()?;
            out.push(u8::from_str_radix(text, 8).ok()?);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Parses one mountinfo line (proc(5)).
#[must_use]
pub fn parse_line(line: &str) -> Option<Mount> {
    let fields: Vec<&str> = line.split(' ').collect();
    let separator = fields.iter().position(|f| *f == "-")?;
    if separator < 6 || fields.len() < separator + 3 {
        return None;
    }
    let device = fields[2];
    let (major, minor) = device.split_once(':')?;
    major.parse::<u32>().ok()?;
    minor.parse::<u32>().ok()?;
    let mount_point = unescape(fields[4])?;
    if mount_point.first() != Some(&b'/') {
        return None;
    }
    Some(Mount {
        device: device.to_owned(),
        mount_point,
        fs_type: fields[separator + 1].to_owned(),
        source: unescape(fields[separator + 2])?,
        read_only: fields[5].split(',').any(|option| option == "ro"),
    })
}

fn excluded(fs_type: &str) -> bool {
    EXCLUDED.contains(&fs_type)
}

/// Real mounts, collapsed by device, in mountinfo order.
///
/// # Errors
///
/// `procfs_unreadable` or `source_malformed` for the file as a whole.
pub fn mounts(root: &HostRoot) -> Sourced<Vec<(Mount, Vec<Vec<u8>>)>> {
    let text = read_source(&root.path("proc/self/mountinfo"), LARGE_SOURCE)
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable))?;
    let mut order: Vec<(Mount, Vec<Vec<u8>>)> = Vec::new();
    let mut by_device: HashMap<String, usize> = HashMap::new();
    for line in text.lines().take(MAX_MOUNTS) {
        let Some(mount) = parse_line(line) else {
            continue;
        };
        if excluded(&mount.fs_type) {
            continue;
        }
        match by_device.get(&mount.device) {
            Some(&index) => {
                let (first, also) = &mut order[index];
                if first.mount_point != mount.mount_point && !also.contains(&mount.mount_point) {
                    also.push(mount.mount_point);
                }
            }
            None => {
                by_device.insert(mount.device.clone(), order.len());
                order.push((mount, Vec::new()));
            }
        }
    }
    Ok(order)
}

fn display(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// The statvfs field types are `c_ulong` and `fsblkcnt_t`, which are `u64` on
// this target and narrower elsewhere; the conversions are for portability.
#[allow(clippy::useless_conversion)]
fn usage_of(stat: &nix::sys::statvfs::Statvfs) -> Sourced<Usage> {
    let unit = u64::from(stat.fragment_size());
    if unit == 0 {
        return Err(Reason::SourceMalformed);
    }
    let (blocks, free, available) = (
        u64::from(stat.blocks()),
        u64::from(stat.blocks_free()),
        u64::from(stat.blocks_available()),
    );
    let bytes = |count: u64| count.checked_mul(unit).ok_or(Reason::SourceMalformed);
    let total_bytes = bytes(blocks)?;
    let free_bytes = bytes(free)?;
    Ok(Usage {
        total_bytes,
        used_bytes: total_bytes
            .checked_sub(free_bytes)
            .ok_or(Reason::SourceMalformed)?,
        free_bytes,
        available_bytes: bytes(available)?,
    })
}

/// The storage provider: mountinfo plus guarded `statvfs`.
#[derive(Debug)]
pub struct Storage {
    root: HostRoot,
    in_flight: Arc<Mutex<HashSet<PathBuf>>>,
}

impl Storage {
    /// Storage under `root`.
    #[must_use]
    pub fn new(root: HostRoot) -> Self {
        Self {
            root,
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Whether mountinfo can be read and parsed.
    ///
    /// # Errors
    ///
    /// Its reason.
    pub fn mounts(&self) -> Sourced<()> {
        mounts(&self.root).map(|_| ())
    }

    /// `statvfs` for one mount point, with the timeout and the in-flight
    /// guard.
    async fn capacity(&self, path: PathBuf) -> Sourced<Usage> {
        {
            let Ok(mut in_flight) = self.in_flight.lock() else {
                return Err(Reason::MetadataUnavailable);
            };
            if !in_flight.insert(path.clone()) {
                return Err(Reason::MetadataUnavailable);
            }
        }
        let guard = Arc::clone(&self.in_flight);
        let call = tokio::task::spawn_blocking(move || {
            let result = nix::sys::statvfs::statvfs(path.as_path());
            if let Ok(mut in_flight) = guard.lock() {
                in_flight.remove(&path);
            }
            result
        });
        match tokio::time::timeout(STATVFS_TIMEOUT, call).await {
            Ok(Ok(Ok(stat))) => usage_of(&stat),
            _ => Err(Reason::MetadataUnavailable),
        }
    }

    /// Every real filesystem, each with its capacity or the reason it has
    /// none.
    pub async fn read(&self) -> Filesystems {
        let mut absences = Absences::default();
        let mounts = match mounts(&self.root) {
            Ok(mounts) => mounts,
            Err(reason) => {
                absences.none::<()>("filesystems", reason);
                return Filesystems {
                    filesystems: Vec::new(),
                    unavailable: absences.into_vec(),
                };
            }
        };
        let mut filesystems = Vec::with_capacity(mounts.len());
        for chunk in mounts.chunks(PARALLEL) {
            let mut calls = tokio::task::JoinSet::new();
            for (index, (mount, _)) in chunk.iter().enumerate() {
                let point = OsString::from_vec(mount.mount_point.clone());
                let path = self.root.path(&point.to_string_lossy());
                let path = if self.root == HostRoot::system() {
                    PathBuf::from(point)
                } else {
                    path
                };
                let this = Self {
                    root: self.root.clone(),
                    in_flight: Arc::clone(&self.in_flight),
                };
                calls.spawn(async move { (index, this.capacity(path).await) });
            }
            let mut results: Vec<Option<Sourced<Usage>>> = vec![None; chunk.len()];
            while let Some(joined) = calls.join_next().await {
                if let Ok((index, result)) = joined {
                    results[index] = Some(result);
                }
            }
            for ((mount, also), result) in chunk.iter().zip(results) {
                let mut own = Absences::default();
                let usage = own.take("usage", result.unwrap_or(Err(Reason::MetadataUnavailable)));
                let source = safe_text(&display(&mount.source), 256);
                let source = source.or_else(|| own.none("source", Reason::SourceMalformed));
                filesystems.push(Filesystem {
                    mount_point: display(&mount.mount_point),
                    also_mounted_at: also.iter().map(|p| display(p)).collect(),
                    fs_type: mount.fs_type.clone(),
                    source,
                    read_only: mount.read_only,
                    usage,
                    unavailable: own.into_vec(),
                });
            }
        }
        Filesystems {
            filesystems,
            unavailable: absences.into_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fixture::Tree;

    const MOUNTINFO: &str = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw,errors=remount-ro
23 22 0:21 / /proc rw,nosuid shared:5 - proc proc rw
24 22 0:22 / /sys rw,nosuid shared:6 - sysfs sysfs rw
25 22 0:5 / /dev rw,nosuid shared:2 - devtmpfs udev rw
26 22 0:24 / /run rw,nosuid shared:7 - tmpfs tmpfs rw
27 22 8:1 / /boot/efi rw,relatime shared:8 - vfat /dev/sda1 rw
28 22 8:3 / /srv/media\\040library ro,relatime shared:9 - ext4 /dev/sda3 rw
29 22 8:3 /photos /home/owner/photos ro,relatime shared:9 - ext4 /dev/sda3 rw
30 22 0:50 / /snap/core/1 ro shared:10 - squashfs /dev/loop0 ro
31 22 0:60 / /var/lib/docker/overlay2/x/merged rw - overlay overlay rw
32 22 0:70 / /mnt/nas rw shared:11 - nfs4 nas:/export rw
33 22 bad / /x rw - ext4 /dev/sdz rw
";

    #[test]
    fn mountinfo_is_filtered_and_bind_mounts_collapse_without_hiding() {
        let tree = Tree::new();
        tree.write("proc/self/mountinfo", MOUNTINFO);
        let mounts = mounts(&tree.root()).expect("mounts");
        let summary: Vec<(String, Vec<String>, &str, bool)> = mounts
            .iter()
            .map(|(m, also)| {
                (
                    display(&m.mount_point),
                    also.iter().map(|p| display(p)).collect(),
                    m.fs_type.as_str(),
                    m.read_only,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("/".into(), vec![], "ext4", false),
                ("/boot/efi".into(), vec![], "vfat", false),
                (
                    "/srv/media library".into(),
                    vec!["/home/owner/photos".to_owned()],
                    "ext4",
                    true
                ),
                ("/mnt/nas".into(), vec![], "nfs4", false),
            ]
        );
    }

    #[test]
    fn every_listed_mount_has_usage_or_a_reason_and_none_is_dropped() {
        let tree = Tree::new();
        tree.write("proc/self/mountinfo", MOUNTINFO);
        // Only "/" and "/boot/efi" exist in the fixture tree; the rest fail
        // statvfs and must still be listed.
        tree.mkdir("boot/efi");
        let storage = Storage::new(tree.root());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let view = runtime.block_on(storage.read());
        assert_eq!(view.filesystems.len(), 4);
        for fs in &view.filesystems {
            match fs.usage {
                Some(usage) => {
                    assert!(usage.total_bytes >= usage.used_bytes);
                    assert!(fs.unavailable.is_empty());
                }
                None => assert!(fs.unavailable.contains(&Unavailable {
                    field: "usage".into(),
                    reason: Reason::MetadataUnavailable
                })),
            }
        }
        assert!(
            view.filesystems[0].usage.is_some(),
            "the fixture root exists"
        );
        assert!(view.filesystems[3].usage.is_none(), "/mnt/nas does not");
    }

    #[test]
    fn malformed_lines_are_skipped_and_no_mountinfo_is_a_reason() {
        for bad in [
            "",
            "1 2 8:1 / / rw - ext4",
            "1 2 8:1 / relative rw - ext4 /dev/x rw",
            "1 2 8:1 / /a\\09 rw - ext4 /dev/x rw",
            "1 2 x:y / / rw - ext4 /dev/x rw",
        ] {
            assert_eq!(parse_line(bad), None, "{bad:?}");
        }
        let tree = Tree::new();
        assert_eq!(mounts(&tree.root()), Err(Reason::ProcfsUnreadable));
    }
}
