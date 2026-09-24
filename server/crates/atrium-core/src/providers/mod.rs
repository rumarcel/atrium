//! Native system providers (M1F): what the machine is and how it is doing,
//! read from the kernel's own interfaces and nothing else (plan §9).
//!
//! No Glances, no Homarr, no Cockpit, no container runtime, no shell, no
//! third-party monitoring and no HTTP client. Every value comes from a file
//! under `/proc`, `/sys` or `/etc`, from `getifaddrs(3)` or from
//! `statvfs(3)`.
//!
//! **The invariant: a value that cannot be read is `null` with a reason.**
//! Never `0`, never an empty string, never a plausible default. Every
//! response carries `unavailable: [{field, reason}]`, with the reason from
//! the closed [`Reason`] set, and a missing source never becomes a number.
//! `empty_fixture_tree_invents_nothing` walks every response built from an
//! empty tree and fails on any number it finds.
//!
//! **Where sources are.** Every path is a compile-time relative path joined
//! to a [`HostRoot`], which is `/` in the service and a fixture directory in
//! tests. Nothing a request carries, and nothing in the environment, can
//! change a path a provider reads. Reads are bounded ([`read_source`]);
//! lists are bounded (interfaces, mounts, sensors).
//!
//! **The provider layer (ADR-002).** Upper layers see three traits —
//! [`HardwareProvider`], [`NetworkProvider`] and [`StorageProvider`], the
//! Core-side providers of ADR-002 — and typed domain values. They never see
//! a path, a file format or a distribution name. M1 has one adapter per
//! trait, [`linux`]: procfs and hwmon, sysfs and `getifaddrs`, mountinfo and
//! `statvfs`. Each trait reports what it can do on this host by trying it
//! ([`Probe`]), never by reading a distribution's name. The Agent-side
//! providers (container, service, package, privileged storage) are reached
//! through the typed protocol, not through a trait in Core.

pub mod cpu;
pub mod fs;
pub mod load;
pub mod memory;
pub mod net;
pub mod os;
pub mod thermal;

use std::fs::File;
use std::future::Future;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;

/// Why a value is absent. A closed set; the codes are stable and are plan
/// §9.5's, extended as recorded in the M1F as-built notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A `/proc` source could not be read.
    ProcfsUnreadable,
    /// A `/sys` source could not be read.
    SysfsUnreadable,
    /// Neither `/etc/os-release` nor `/usr/lib/os-release` could be read.
    OsReleaseUnreadable,
    /// A rate or percentage needs two samples and there has been one.
    FirstSamplePending,
    /// The kernel does not report `MemAvailable`.
    MemAvailableUnsupported,
    /// No swap is configured (`SwapTotal` is 0).
    SwapAbsent,
    /// No readable temperature sensor exists.
    NoHwmonSensors,
    /// The source exists but does not report this value.
    NotReported,
    /// The network driver does not report a link speed.
    NotReportedByDriver,
    /// `statvfs` failed or did not answer in time for this mount.
    MetadataUnavailable,
    /// The source was read but the value is not in the expected form.
    SourceMalformed,
    /// The background sample is older than it should be.
    SampleStale,
}

impl Reason {
    /// The stable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ProcfsUnreadable => "procfs_unreadable",
            Self::SysfsUnreadable => "sysfs_unreadable",
            Self::OsReleaseUnreadable => "os_release_unreadable",
            Self::FirstSamplePending => "first_sample_pending",
            Self::MemAvailableUnsupported => "mem_available_unsupported",
            Self::SwapAbsent => "swap_absent",
            Self::NoHwmonSensors => "no_hwmon_sensors",
            Self::NotReported => "not_reported",
            Self::NotReportedByDriver => "not_reported_by_driver",
            Self::MetadataUnavailable => "metadata_unavailable",
            Self::SourceMalformed => "source_malformed",
            Self::SampleStale => "sample_stale",
        }
    }

    /// Every reason, for enumeration tests.
    pub const ALL: [Self; 12] = [
        Self::ProcfsUnreadable,
        Self::SysfsUnreadable,
        Self::OsReleaseUnreadable,
        Self::FirstSamplePending,
        Self::MemAvailableUnsupported,
        Self::SwapAbsent,
        Self::NoHwmonSensors,
        Self::NotReported,
        Self::NotReportedByDriver,
        Self::MetadataUnavailable,
        Self::SourceMalformed,
        Self::SampleStale,
    ];
}

/// One absent value: which field, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unavailable {
    /// The field, as a dotted path in the response (`memory.availableBytes`).
    pub field: String,
    /// Why.
    pub reason: Reason,
}

/// Collects absences while a response is built.
#[derive(Debug, Default)]
pub struct Absences(Vec<Unavailable>);

impl Absences {
    /// Records that `field` is absent for `reason`, and returns `None` so a
    /// call site can write `absences.none("x", reason)` as the value.
    pub fn none<T>(&mut self, field: &str, reason: Reason) -> Option<T> {
        self.0.push(Unavailable {
            field: field.to_owned(),
            reason,
        });
        None
    }

    /// Unwraps a sourced value, recording the absence when there is none.
    pub fn take<T>(&mut self, field: &str, value: Sourced<T>) -> Option<T> {
        match value {
            Ok(value) => Some(value),
            Err(reason) => self.none(field, reason),
        }
    }

    /// The list, in the order recorded.
    #[must_use]
    pub fn into_vec(self) -> Vec<Unavailable> {
        self.0
    }
}

/// A value, or why there is none.
pub type Sourced<T> = Result<T, Reason>;

/// Where the host's `/proc`, `/sys` and `/etc` are. `/` in the service; a
/// fixture tree in tests. Built only in code, never from a request or the
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRoot(PathBuf);

impl HostRoot {
    /// The real machine.
    #[must_use]
    pub fn system() -> Self {
        Self(PathBuf::from("/"))
    }

    /// A fixture tree, for tests.
    #[must_use]
    pub fn fixture(root: &Path) -> Self {
        Self(root.to_path_buf())
    }

    /// `relative` under the root. `relative` is always a literal in this
    /// module or a name read from the kernel's own directory listing.
    #[must_use]
    pub fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative.trim_start_matches('/'))
    }
}

/// How a source read failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailure {
    /// It does not exist.
    Missing,
    /// It exists and could not be read (permission, I/O, not a regular
    /// file, a kernel refusal such as `EINVAL` on a down link's `speed`).
    Unreadable,
    /// It is larger than the source can legitimately be, or not UTF-8.
    Malformed,
}

/// Largest ordinary source (a line or a small table).
pub const SMALL_SOURCE: u64 = 64 * 1024;
/// Largest `/proc/self/mountinfo` or `/proc/stat` accepted.
pub const LARGE_SOURCE: u64 = 1024 * 1024;

/// Reads a text source of at most `limit` bytes. Symbolic links are followed
/// (`/etc/os-release` is conventionally one); FIFOs and devices are refused
/// without blocking.
///
/// # Errors
///
/// [`ReadFailure`].
pub fn read_source(path: &Path, limit: u64) -> Result<String, ReadFailure> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path);
    let mut file: File = match file {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReadFailure::Missing)
        }
        Err(_) => return Err(ReadFailure::Unreadable),
    };
    match file.metadata() {
        Ok(metadata) if metadata.file_type().is_file() => {}
        _ => return Err(ReadFailure::Unreadable),
    }
    let mut buffer = Vec::new();
    // /proc and /sys files report a size of 0 or a page; the bound is on
    // what is actually read.
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| ReadFailure::Unreadable)?;
    if buffer.len() as u64 > limit {
        return Err(ReadFailure::Malformed);
    }
    String::from_utf8(buffer).map_err(|_| ReadFailure::Malformed)
}

/// Reads a single-line value and trims its trailing newline.
///
/// # Errors
///
/// [`ReadFailure`].
pub fn read_line(path: &Path) -> Result<String, ReadFailure> {
    let text = read_source(path, 4096)?;
    Ok(text.trim_end_matches(['\n', '\r']).to_owned())
}

/// Maps a read failure to the reason for its family of source.
#[must_use]
pub fn reason_for(failure: ReadFailure, unreadable: Reason) -> Reason {
    match failure {
        ReadFailure::Malformed => Reason::SourceMalformed,
        ReadFailure::Missing | ReadFailure::Unreadable => unreadable,
    }
}

/// Text that is safe to put in a response: printable, bounded, no control
/// characters. Anything else is malformed rather than repaired.
#[must_use]
pub fn safe_text(text: &str, max_chars: usize) -> Option<String> {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    (count > 0 && count <= max_chars && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.to_owned())
}

/// What a provider can do on this host: each feature either works or is
/// missing with a reason. Found by trying, never by a distribution's name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// Features that work.
    pub features: Vec<&'static str>,
    /// Features that do not, and why.
    pub missing: Vec<(&'static str, Reason)>,
}

impl Probe {
    fn check<T>(&mut self, feature: &'static str, result: Sourced<T>) {
        match result {
            Ok(_) => self.features.push(feature),
            Err(reason) => self.missing.push((feature, reason)),
        }
    }
}

/// CPU, memory, load, uptime, temperatures and the host's identity.
pub trait HardwareProvider: std::fmt::Debug + Send + Sync {
    /// The adapter's name, for capabilities and diagnostics.
    fn name(&self) -> &'static str;
    /// OS release, kernel, hostname, uptime, boot id.
    fn identity(&self) -> os::HostIdentity;
    /// CPU usage over the last sampling interval, in percent.
    ///
    /// # Errors
    ///
    /// Why there is no value.
    fn cpu_usage(&self, now: Instant) -> Sourced<f64>;
    /// Logical CPUs present.
    ///
    /// # Errors
    ///
    /// Why there is no value.
    fn cpu_cores(&self) -> Sourced<u32>;
    /// Load averages; absences recorded under `load.*`.
    fn load(&self, absences: &mut Absences) -> load::Load;
    /// Memory and swap; absences recorded under `memory.*` and `swap.*`.
    fn memory(&self, absences: &mut Absences) -> (memory::Memory, memory::Swap);
    /// Temperature sensors; an absence recorded under `temperatures`.
    fn temperatures(&self, absences: &mut Absences) -> Vec<thermal::Sensor>;
    /// Which of cpu, memory, load, uptime and temperature work here.
    fn probe(&self) -> Probe;
}

/// Network interfaces and their addresses.
pub trait NetworkProvider: std::fmt::Debug + Send + Sync {
    /// The adapter's name.
    fn name(&self) -> &'static str;
    /// Every interface.
    fn interfaces(&self) -> net::Interfaces;
    /// Whether interfaces and addresses can be listed here.
    fn probe(&self) -> Probe;
}

/// A future a provider returns; boxed so the trait stays object-safe.
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Mounted filesystems and their capacity (the read-only half of ADR-002's
/// `StorageProvider`; the privileged half is Agent's, and not in M1).
pub trait StorageProvider: std::fmt::Debug + Send + Sync {
    /// The adapter's name.
    fn name(&self) -> &'static str;
    /// Every real filesystem.
    fn filesystems(&self) -> Pending<'_, fs::Filesystems>;
    /// Whether filesystems can be listed here.
    fn probe(&self) -> Probe;
}

/// The Linux adapters.
pub mod linux {
    use super::*;

    /// procfs, sysfs `cpu/present` and hwmon.
    #[derive(Debug)]
    pub struct Hardware {
        root: HostRoot,
        sampler: Arc<cpu::Sampler>,
    }

    impl HardwareProvider for Hardware {
        fn name(&self) -> &'static str {
            "linux-procfs"
        }

        fn identity(&self) -> os::HostIdentity {
            os::read(&self.root)
        }

        fn cpu_usage(&self, now: Instant) -> Sourced<f64> {
            self.sampler.usage(now)
        }

        fn cpu_cores(&self) -> Sourced<u32> {
            cpu::cores(&self.root)
        }

        fn load(&self, absences: &mut Absences) -> load::Load {
            load::read(&self.root, absences)
        }

        fn memory(&self, absences: &mut Absences) -> (memory::Memory, memory::Swap) {
            memory::read(&self.root, absences)
        }

        fn temperatures(&self, absences: &mut Absences) -> Vec<thermal::Sensor> {
            thermal::read(&self.root, absences)
        }

        fn probe(&self) -> Probe {
            let mut probe = Probe::default();
            probe.check("cpu", cpu::read_times(&self.root));
            let mut scratch = Absences::default();
            let (memory, _) = memory::read(&self.root, &mut scratch);
            probe.check(
                "memory",
                first(memory.total_bytes, scratch, "memory.totalBytes"),
            );
            let mut scratch = Absences::default();
            let load = load::read(&self.root, &mut scratch);
            probe.check("load", first(load.one, scratch, "load.one"));
            probe.check("uptime", os::uptime_seconds(&self.root));
            let mut scratch = Absences::default();
            let sensors = thermal::read(&self.root, &mut scratch);
            let sensor = sensors.first().map(|_| ());
            probe.check("temperature", first(sensor, scratch, "temperatures"));
            probe
        }
    }

    /// `/sys/class/net` and `getifaddrs`.
    #[derive(Debug)]
    pub struct Network {
        root: HostRoot,
    }

    impl NetworkProvider for Network {
        fn name(&self) -> &'static str {
            "linux-sysfs+getifaddrs"
        }

        fn interfaces(&self) -> net::Interfaces {
            net::read(&self.root)
        }

        fn probe(&self) -> Probe {
            let mut probe = Probe::default();
            probe.check(
                "interfaces",
                std::fs::read_dir(self.root.path("sys/class/net"))
                    .map_err(|_| Reason::SysfsUnreadable),
            );
            probe.check("addresses", net::system_addresses());
            probe
        }
    }

    /// `/proc/self/mountinfo` and guarded `statvfs`.
    #[derive(Debug)]
    pub struct Storage(fs::Storage);

    impl StorageProvider for Storage {
        fn name(&self) -> &'static str {
            "linux-mountinfo+statvfs"
        }

        fn filesystems(&self) -> Pending<'_, fs::Filesystems> {
            Box::pin(self.0.read())
        }

        fn probe(&self) -> Probe {
            let mut probe = Probe::default();
            probe.check("filesystems", self.0.mounts());
            probe
        }
    }

    fn first<T>(value: Option<T>, scratch: Absences, field: &str) -> Sourced<T> {
        value.ok_or_else(|| {
            scratch
                .into_vec()
                .into_iter()
                .find(|u| u.field == field)
                .map_or(Reason::NotReported, |u| u.reason)
        })
    }

    /// The three Linux adapters over `root`, reading CPU usage from
    /// `sampler`. The only selection M1 makes: Core is built for Linux.
    #[must_use]
    pub fn adapters(
        root: &HostRoot,
        sampler: Arc<cpu::Sampler>,
    ) -> (
        Arc<dyn HardwareProvider>,
        Arc<dyn NetworkProvider>,
        Arc<dyn StorageProvider>,
    ) {
        (
            Arc::new(Hardware {
                root: root.clone(),
                sampler,
            }),
            Arc::new(Network { root: root.clone() }),
            Arc::new(Storage(fs::Storage::new(root.clone()))),
        )
    }
}

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests;
