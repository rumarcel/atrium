//! The system domain (M1F): what `GET /api/v1/system`, `/system/metrics`,
//! `/system/capabilities`, `/system/diagnostics`, `/network/interfaces` and
//! `/storage/filesystems` answer in normal mode.
//!
//! [`System`] assembles typed responses from the native providers
//! ([`crate::providers`]), the Agent monitor's published status and facts
//! fixed at startup. Each response is partial rather than failed: a value
//! that cannot be read is `null` and listed in `unavailable` with a closed
//! reason, and the request still succeeds. Nothing here parses a request or
//! a token — every route that reaches it has passed the `Auth::Device`
//! check in dispatch — and no request value reaches a path.
//!
//! Recovery mode has no [`System`]; its diagnostics are
//! [`crate::recovery::DiagnosticsReport`], a strictly smaller allowlist.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use time::OffsetDateTime;

use crate::capability::{self, AgentStatus};
use crate::identity::{format_time, ServerId};
use crate::layout::Layout;
use crate::providers::{
    fs, load, memory, net, os, thermal, Absences, HardwareProvider, NetworkProvider, Probe, Reason,
    StorageProvider, Unavailable,
};

// ---------------------------------------------------------------------------
// Responses

/// `GET /api/v1/system`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    /// This installation.
    pub server_id: String,
    /// Core's version.
    pub core_version: &'static str,
    /// The CPU architecture Core was built for.
    pub arch: &'static str,
    /// The OS.
    pub os: os::OsRelease,
    /// The kernel.
    pub kernel: os::Kernel,
    /// The kernel's hostname.
    pub hostname: Option<String>,
    /// Whole seconds since boot.
    pub uptime_seconds: Option<u64>,
    /// This boot.
    pub boot_id: Option<String>,
    /// What could not be read, and why.
    pub unavailable: Vec<Unavailable>,
}

/// `cpu` in the metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cpu {
    /// Busy share of all CPUs over the last sampling interval, 0–100.
    pub usage_percent: Option<f64>,
    /// Logical CPUs present.
    pub cores: Option<u32>,
}

/// `GET /api/v1/system/metrics`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    /// When this response was assembled.
    pub at: String,
    /// CPU.
    pub cpu: Cpu,
    /// Load averages.
    pub load: load::Load,
    /// Memory.
    pub memory: memory::Memory,
    /// Swap.
    pub swap: memory::Swap,
    /// Every readable temperature sensor.
    pub temperatures: Vec<thermal::Sensor>,
    /// What could not be read, and why.
    pub unavailable: Vec<Unavailable>,
}

/// A feature a capability lacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Missing {
    /// The feature; `*` is all of it.
    pub feature: &'static str,
    /// The reason code.
    pub reason: &'static str,
}

fn missing(feature: &'static str, reason: &'static str) -> Missing {
    Missing { feature, reason }
}

/// A native capability: hardware, network, storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Native {
    /// At least one feature works.
    pub available: bool,
    /// Which provider serves it.
    pub provider: &'static str,
    /// What works.
    pub features: Vec<&'static str>,
    /// What does not, and why.
    pub missing: Vec<Missing>,
}

/// A capability M1 does not provide at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Absent {
    /// Always `false`.
    pub available: bool,
    /// Always empty.
    pub features: Vec<&'static str>,
    /// Why.
    pub missing: Vec<Missing>,
}

/// `container`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerCapability {
    /// A known runtime socket is present (not a claim that it runs).
    pub available: bool,
    /// Always `agent`.
    pub via: &'static str,
    /// `docker` or `podman`, when one was found.
    pub provider: Option<&'static str>,
    /// Never probed in M1.
    pub version: Option<String>,
    /// Always empty in M1.
    pub features: Vec<&'static str>,
    /// What is missing, and why.
    pub missing: Vec<Missing>,
    /// When Agent was last asked.
    pub checked_at: Option<String>,
}

/// `privileged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivilegedCapability {
    /// Agent answered.
    pub available: bool,
    /// Always `agent`.
    pub via: &'static str,
    /// Agent's version.
    pub agent_version: Option<String>,
    /// The Core–Agent protocol version.
    pub protocol: Option<u32>,
    /// Why not.
    pub missing: Vec<Missing>,
    /// When Agent was last asked.
    pub checked_at: Option<String>,
}

/// `GET /api/v1/system/capabilities` (plan §9.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// CPU, memory, load, uptime, temperature.
    pub hardware: Native,
    /// Interfaces and addresses.
    pub network: Native,
    /// Filesystems.
    pub storage: Native,
    /// A container runtime, through Agent.
    pub container: ContainerCapability,
    /// Service control: not in M1.
    pub services: Absent,
    /// Packages: not in Alpha.
    pub packages: Absent,
    /// The Agent boundary.
    pub privileged: PrivilegedCapability,
    /// mDNS discovery: M1G.
    pub discovery: Absent,
}

/// One recurring problem, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// The reason code.
    pub code: &'static str,
    /// What it was about: a response field or `agent`.
    pub subject: String,
    /// Times seen since Core started.
    pub count: u64,
    /// Last seen.
    pub last_seen: String,
}

/// Versions in normal diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Versions {
    /// Core.
    pub core: &'static str,
    /// The public API.
    pub api: u32,
    /// Agent, when it answered.
    pub agent: Option<String>,
    /// The Core–Agent protocol, when Agent answered.
    pub agent_protocol: Option<u32>,
}

/// A capability's state in one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Available.
    pub available: bool,
    /// Reason codes of what is missing.
    pub reasons: Vec<&'static str>,
}

/// Backup availability, without names or paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Backups {
    /// At least one exists.
    pub available: bool,
    /// How many.
    pub count: usize,
}

/// `GET /api/v1/system/diagnostics` in normal mode: an allowlist. More than
/// recovery's (which anyone on the LAN can read) and far less than a dump:
/// no hostname, no addresses, no paths, no device list, no audit content,
/// no log text, nothing secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    /// Always `normal`.
    pub state: &'static str,
    /// When Core started.
    pub since: String,
    /// Component versions.
    pub versions: Versions,
    /// The state database's schema version.
    pub schema_version: u32,
    /// Pre-migration backups.
    pub backups: Backups,
    /// Which provider serves each native domain.
    pub providers: ProviderNames,
    /// Every capability, in one line each.
    pub capabilities: std::collections::BTreeMap<&'static str, Summary>,
    /// Recent problems, most recent first, bounded.
    pub recent_issues: Vec<Issue>,
}

/// Provider names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderNames {
    /// Hardware.
    pub hardware: &'static str,
    /// Network.
    pub network: &'static str,
    /// Storage.
    pub storage: &'static str,
}

// ---------------------------------------------------------------------------
// Recent issues

/// Issues kept at most.
pub const MAX_ISSUES: usize = 32;

/// A bounded record of recurring problems: codes and subjects only, never
/// text from a source or an error chain.
#[derive(Debug, Default)]
pub struct Issues(Mutex<VecDeque<Issue>>);

impl Issues {
    /// Records `code` about `subject`.
    pub fn record(&self, code: &'static str, subject: &str, at: OffsetDateTime) {
        let Ok(mut issues) = self.0.lock() else {
            return;
        };
        let last_seen = format_time(at);
        if let Some(index) = issues
            .iter()
            .position(|issue| issue.code == code && issue.subject == subject)
        {
            if let Some(mut issue) = issues.remove(index) {
                issue.count = issue.count.saturating_add(1);
                issue.last_seen = last_seen;
                issues.push_front(issue);
            }
            return;
        }
        issues.push_front(Issue {
            code,
            subject: subject.to_owned(),
            count: 1,
            last_seen,
        });
        issues.truncate(MAX_ISSUES);
    }

    /// Most recent first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Issue> {
        self.0
            .lock()
            .map(|issues| issues.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Reasons that describe the machine as it is (no swap, no sensor, a
/// virtual NIC without a speed, a first sample) rather than a problem.
fn is_problem(reason: Reason) -> bool {
    matches!(
        reason,
        Reason::ProcfsUnreadable
            | Reason::SysfsUnreadable
            | Reason::OsReleaseUnreadable
            | Reason::SourceMalformed
            | Reason::MetadataUnavailable
            | Reason::SampleStale
    )
}

// ---------------------------------------------------------------------------
// The service

/// What diagnostics reports about the state, fixed at startup.
#[derive(Debug, Clone)]
pub struct StateFacts {
    /// Where the backups are.
    pub layout: Layout,
    /// The schema version startup verified.
    pub schema_version: u32,
}

/// The Core-side providers the domain reads through (ADR-002).
#[derive(Debug, Clone)]
pub struct Providers {
    /// Hardware.
    pub hardware: Arc<dyn HardwareProvider>,
    /// Network.
    pub network: Arc<dyn NetworkProvider>,
    /// Storage (read-only).
    pub storage: Arc<dyn StorageProvider>,
}

/// The system domain.
#[derive(Debug)]
pub struct System {
    providers: Providers,
    agent: Arc<AgentStatus>,
    issues: Arc<Issues>,
    server_id: ServerId,
    started: OffsetDateTime,
    state: StateFacts,
}

impl System {
    /// The system domain over `providers`.
    #[must_use]
    pub fn new(
        providers: Providers,
        agent: Arc<AgentStatus>,
        issues: Arc<Issues>,
        server_id: ServerId,
        state: StateFacts,
    ) -> Self {
        Self {
            providers,
            agent,
            issues,
            server_id,
            started: OffsetDateTime::now_utc(),
            state,
        }
    }

    fn note(&self, unavailable: &[Unavailable]) {
        let now = OffsetDateTime::now_utc();
        for absent in unavailable.iter().filter(|a| is_problem(a.reason)) {
            self.issues.record(absent.reason.code(), &absent.field, now);
        }
    }

    /// `GET /api/v1/system`.
    #[must_use]
    pub fn info(&self) -> SystemInfo {
        let host: os::HostIdentity = self.providers.hardware.identity();
        self.note(&host.unavailable);
        SystemInfo {
            server_id: self.server_id.to_string(),
            core_version: crate::VERSION,
            arch: std::env::consts::ARCH,
            os: host.os,
            kernel: host.kernel,
            hostname: host.hostname,
            uptime_seconds: host.uptime_seconds,
            boot_id: host.boot_id,
            unavailable: host.unavailable,
        }
    }

    /// `GET /api/v1/system/metrics`.
    #[must_use]
    pub fn metrics(&self, now: Instant) -> Metrics {
        let mut absences = Absences::default();
        let hardware = &self.providers.hardware;
        let cpu = Cpu {
            usage_percent: absences.take("cpu.usagePercent", hardware.cpu_usage(now)),
            cores: absences.take("cpu.cores", hardware.cpu_cores()),
        };
        let load: load::Load = hardware.load(&mut absences);
        let (memory, swap): (memory::Memory, memory::Swap) = hardware.memory(&mut absences);
        let temperatures: Vec<thermal::Sensor> = hardware.temperatures(&mut absences);
        let unavailable = absences.into_vec();
        self.note(&unavailable);
        Metrics {
            at: format_time(OffsetDateTime::now_utc()),
            cpu,
            load,
            memory,
            swap,
            temperatures,
            unavailable,
        }
    }

    /// `GET /api/v1/network/interfaces`.
    #[must_use]
    pub fn interfaces(&self) -> net::Interfaces {
        let view: net::Interfaces = self.providers.network.interfaces();
        self.note(&view.unavailable);
        view
    }

    /// `GET /api/v1/storage/filesystems`.
    pub async fn filesystems(&self) -> fs::Filesystems {
        let view: fs::Filesystems = self.providers.storage.filesystems().await;
        self.note(&view.unavailable);
        for filesystem in &view.filesystems {
            self.note(&filesystem.unavailable);
        }
        view
    }

    fn native(name: &'static str, probe: Probe, core: &[&str]) -> Native {
        Native {
            available: probe.features.iter().any(|feature| core.contains(feature)),
            provider: name,
            features: probe.features,
            missing: probe
                .missing
                .into_iter()
                .map(|(feature, reason)| missing(feature, reason.code()))
                .collect(),
        }
    }

    /// `GET /api/v1/system/capabilities`.
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        let pending = capability::Reason::AgentCheckPending.code();
        let latest = self.agent.latest();
        let checked_at = latest.as_ref().map(|(_, at)| format_time(*at));
        let (container, privileged) = match &latest {
            Some((agent, _)) => (
                ContainerCapability {
                    available: agent.container.available,
                    via: agent.container.via(),
                    provider: agent.container.provider.map(|p| p.as_str()),
                    version: None,
                    features: Vec::new(),
                    missing: agent
                        .container
                        .missing
                        .iter()
                        .map(|m| missing(m.feature, m.reason.code()))
                        .collect(),
                    checked_at: checked_at.clone(),
                },
                PrivilegedCapability {
                    available: agent.privileged.available,
                    via: "agent",
                    agent_version: agent
                        .privileged
                        .agent_version
                        .as_ref()
                        .map(ToString::to_string),
                    protocol: agent.privileged.protocol,
                    missing: agent
                        .privileged
                        .missing
                        .iter()
                        .map(|m| missing(m.feature, m.reason.code()))
                        .collect(),
                    checked_at,
                },
            ),
            None => (
                ContainerCapability {
                    available: false,
                    via: "agent",
                    provider: None,
                    version: None,
                    features: Vec::new(),
                    missing: vec![missing("*", pending)],
                    checked_at: None,
                },
                PrivilegedCapability {
                    available: false,
                    via: "agent",
                    agent_version: None,
                    protocol: None,
                    missing: vec![missing("*", pending)],
                    checked_at: None,
                },
            ),
        };
        let absent = |feature: &'static str, reason: capability::Reason| Absent {
            available: false,
            features: Vec::new(),
            missing: vec![missing(feature, reason.code())],
        };
        let alpha = capability::Reason::NotEnabledInAlpha.code();
        let mut storage = Self::native(
            self.providers.storage.name(),
            self.providers.storage.probe(),
            &["filesystems"],
        );
        storage.missing.push(missing("smart", alpha));
        storage.missing.push(missing("block_devices", alpha));
        Capabilities {
            // Temperature alone does not make the hardware provider
            // available.
            hardware: Self::native(
                self.providers.hardware.name(),
                self.providers.hardware.probe(),
                &["cpu", "memory", "load", "uptime"],
            ),
            network: Self::native(
                self.providers.network.name(),
                self.providers.network.probe(),
                &["interfaces", "addresses"],
            ),
            storage,
            container,
            services: absent("service_control", capability::Reason::NotEnabledInM1),
            packages: absent("install", capability::Reason::NotEnabledInAlpha),
            privileged,
            discovery: absent("mdns", capability::Reason::NotYetImplemented),
        }
    }

    /// `GET /api/v1/system/diagnostics` in normal mode.
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics {
        let capabilities = self.capabilities();
        let summary = |available: bool, missing: &[Missing]| Summary {
            available,
            reasons: missing.iter().map(|m| m.reason).collect(),
        };
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "hardware",
            summary(
                capabilities.hardware.available,
                &capabilities.hardware.missing,
            ),
        );
        map.insert(
            "network",
            summary(
                capabilities.network.available,
                &capabilities.network.missing,
            ),
        );
        map.insert(
            "storage",
            summary(
                capabilities.storage.available,
                &capabilities.storage.missing,
            ),
        );
        map.insert(
            "container",
            summary(
                capabilities.container.available,
                &capabilities.container.missing,
            ),
        );
        map.insert(
            "privileged",
            summary(
                capabilities.privileged.available,
                &capabilities.privileged.missing,
            ),
        );
        map.insert("discovery", summary(false, &capabilities.discovery.missing));
        let backups = crate::db::backups::list(&self.state.layout)
            .map(|list| list.len())
            .unwrap_or(0);
        Diagnostics {
            state: "normal",
            since: format_time(self.started),
            versions: Versions {
                core: crate::VERSION,
                api: atrium_api_types::API_VERSION,
                agent: capabilities.privileged.agent_version.clone(),
                agent_protocol: capabilities.privileged.protocol,
            },
            schema_version: self.state.schema_version,
            backups: Backups {
                available: backups > 0,
                count: backups,
            },
            providers: ProviderNames {
                hardware: self.providers.hardware.name(),
                network: self.providers.network.name(),
                storage: self.providers.storage.name(),
            },
            capabilities: map,
            recent_issues: self.issues.snapshot(),
        }
    }
}

#[cfg(test)]
mod tests;
