//! The system domain's responses, from fixture trees.

use std::sync::Arc;
use std::time::Instant;

use atrium_protocol::messages::RuntimeSocket;
use time::OffsetDateTime;

use super::*;
use crate::agentclient::Failure;
use crate::capability::tests::{info, probe};
use crate::capability::AgentCapabilities;
use crate::providers::fixture::Tree;
use crate::testutil::Fixture;

fn system(tree: &Tree, agent: Arc<AgentStatus>) -> (System, Fixture) {
    let state = Fixture::new("system");
    let (hardware, network, storage) = crate::providers::linux::adapters(
        &tree.root(),
        crate::providers::cpu::Sampler::new(tree.root()),
    );
    let system = System::new(
        Providers {
            hardware,
            network,
            storage,
        },
        agent,
        Arc::new(Issues::default()),
        ServerId::parse("00112233445566778899aabbccddeeff").expect("id"),
        StateFacts {
            layout: state.layout.clone(),
            schema_version: 3,
        },
    );
    (system, state)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// Every number anywhere in `value`, with its JSON path.
fn numbers(value: &serde_json::Value, path: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Number(n) => out.push(format!("{path} = {n}")),
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                numbers(item, &format!("{path}[{i}]"), out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                numbers(item, &format!("{path}.{key}"), out);
            }
        }
        _ => {}
    }
}

/// Test plan: the no-invented-values test. A tree with nothing in it must
/// produce `null` + reason everywhere a measurement would be — never a 0 —
/// in every M1F response. The only numbers allowed are the API version and
/// the schema version in diagnostics, which are not measurements.
#[test]
fn empty_fixture_tree_invents_nothing() {
    let tree = Tree::new();
    let (system, _state) = system(&tree, AgentStatus::new());
    let responses = [
        ("system", serde_json::to_value(system.info()).expect("json")),
        (
            "metrics",
            serde_json::to_value(system.metrics(Instant::now())).expect("json"),
        ),
        (
            "interfaces",
            serde_json::to_value(net::assemble(&tree.root(), Ok(Vec::new()))).expect("json"),
        ),
        (
            "filesystems",
            serde_json::to_value(runtime().block_on(system.filesystems())).expect("json"),
        ),
        (
            "capabilities",
            serde_json::to_value(system.capabilities()).expect("json"),
        ),
        (
            "diagnostics",
            serde_json::to_value(system.diagnostics()).expect("json"),
        ),
    ];
    let mut found = Vec::new();
    for (name, value) in &responses {
        numbers(value, name, &mut found);
    }
    // Diagnostics counts how often each source failed; those counts are
    // real tallies of real failures, not measurements of the machine.
    found.retain(|entry| {
        !(entry.starts_with("diagnostics.recentIssues[") && entry.contains("].count = "))
    });
    assert_eq!(
        found,
        [
            "diagnostics.backups.count = 0".to_owned(),
            "diagnostics.schemaVersion = 3".to_owned(),
            "diagnostics.versions.api = 1".to_owned(),
        ],
        "only versions, a real count of backups and failure tallies may be numbers"
    );

    // And every null measurement is explained.
    let metrics = system.metrics(Instant::now());
    let fields: Vec<&str> = metrics
        .unavailable
        .iter()
        .map(|u| u.field.as_str())
        .collect();
    for field in [
        "cpu.usagePercent",
        "cpu.cores",
        "load.one",
        "memory.totalBytes",
        "memory.availableBytes",
        "swap.totalBytes",
        "temperatures",
    ] {
        assert!(fields.contains(&field), "{field} not explained: {fields:?}");
    }
    assert!(metrics.temperatures.is_empty());
    let info = system.info();
    assert!(info.unavailable.len() >= 7);
    assert_eq!(info.arch, std::env::consts::ARCH, "always known");
}

fn full_tree() -> Tree {
    let tree = Tree::new();
    tree.write(
        "etc/os-release",
        "ID=debian\nVERSION_ID=\"12\"\nPRETTY_NAME=\"Debian 12\"\n",
    );
    tree.write("proc/sys/kernel/ostype", "Linux\n");
    tree.write("proc/sys/kernel/osrelease", "6.1.0-25-amd64\n");
    tree.write("proc/sys/kernel/version", "#1 SMP Debian\n");
    tree.write("proc/sys/kernel/hostname", "box\n");
    tree.write(
        "proc/sys/kernel/random/boot_id",
        "3c9d2f4e-8a1b-4c2d-9e0f-1a2b3c4d5e6f\n",
    );
    tree.write("proc/uptime", "100.5 50.0\n");
    tree.write("proc/stat", "cpu  10 0 10 80 0 0 0 0 0 0\n");
    tree.write("proc/loadavg", "0.10 0.20 0.30 1/100 42\n");
    tree.write(
        "proc/meminfo",
        "MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 500 kB\nBuffers: 10 kB\n\
         Cached: 200 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
    );
    tree.write("sys/devices/system/cpu/present", "0-3\n");
    tree.write("sys/class/hwmon/hwmon0/name", "k10temp\n");
    tree.write("sys/class/hwmon/hwmon0/temp1_input", "45500\n");
    tree.write(
        "proc/self/mountinfo",
        "22 1 8:2 / / rw - ext4 /dev/sda2 rw\n",
    );
    tree.mkdir("sys/class/net/eth0");
    tree
}

#[test]
fn a_full_tree_answers_with_real_values_and_only_true_absences() {
    let tree = full_tree();
    let (system, _state) = system(&tree, AgentStatus::new());
    let metrics = system.metrics(Instant::now());
    assert_eq!(metrics.cpu.cores, Some(4));
    assert_eq!(metrics.memory.total_bytes, Some(1000 * 1024));
    assert_eq!(metrics.load.five, Some(0.2));
    assert_eq!(metrics.temperatures[0].chip, "k10temp");
    assert!(metrics.temperatures[0].cpu);
    let reasons: Vec<(&str, Reason)> = metrics
        .unavailable
        .iter()
        .map(|u| (u.field.as_str(), u.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            ("cpu.usagePercent", Reason::FirstSamplePending),
            ("swap.totalBytes", Reason::SwapAbsent),
            ("swap.freeBytes", Reason::SwapAbsent),
        ]
    );
    let capabilities = system.capabilities();
    assert!(capabilities.hardware.available);
    assert_eq!(
        capabilities.hardware.features,
        ["cpu", "memory", "load", "uptime", "temperature"]
    );
    assert!(capabilities.storage.available);
    // Nothing states of the machine were recorded as problems.
    assert!(system.diagnostics().recent_issues.is_empty());
}

#[test]
fn capabilities_follow_the_agent_and_never_claim_a_running_runtime() {
    let tree = full_tree();
    let agent = AgentStatus::new();
    let (system, _state) = system(&tree, Arc::clone(&agent));

    // Before the first check.
    let pending = system.capabilities();
    assert!(!pending.privileged.available && !pending.container.available);
    assert_eq!(pending.container.missing[0].reason, "agent_check_pending");

    // Agent unreachable: both off with the reason, everything else unchanged.
    let unreachable = AgentCapabilities::derive(&Err(Failure::SocketMissing), None);
    agent.publish(&unreachable, OffsetDateTime::now_utc());
    let down = system.capabilities();
    assert!(!down.privileged.available && !down.container.available);
    assert_eq!(down.privileged.missing[0].reason, "agent_unreachable");
    assert_eq!(down.container.missing[0].reason, "agent_unreachable");
    assert_eq!(down.hardware, pending.hardware);

    // No runtime socket.
    let none = AgentCapabilities::derive(&Ok(info()), Some(&Ok(probe(None))));
    agent.publish(&none, OffsetDateTime::now_utc());
    let no_runtime = system.capabilities();
    assert!(no_runtime.privileged.available);
    assert!(!no_runtime.container.available);
    assert_eq!(
        no_runtime.container.missing[0].reason,
        "no_container_runtime"
    );

    // A socket present: available through Agent, liveness explicitly unknown.
    let docker = AgentCapabilities::derive(
        &Ok(info()),
        Some(&Ok(probe(Some(RuntimeSocket::DockerRun)))),
    );
    agent.publish(&docker, OffsetDateTime::now_utc());
    let present = system.capabilities();
    assert!(present.container.available);
    assert_eq!(present.container.via, "agent");
    assert_eq!(present.container.version, None);
    assert!(present
        .container
        .missing
        .contains(&missing("liveness", "runtime_liveness_not_probed_in_m1")));
    let json = serde_json::to_value(&present).expect("json");
    assert_eq!(json["container"]["version"], serde_json::Value::Null);
    assert_eq!(
        json["discovery"]["missing"][0]["reason"],
        "not_yet_implemented"
    );
    assert_eq!(json["storage"]["missing"][0]["feature"], "smart");
}

#[test]
fn normal_diagnostics_is_an_allowlist() {
    let tree = full_tree();
    let (system, _state) = system(&tree, AgentStatus::new());
    let value = serde_json::to_value(system.diagnostics()).expect("json");
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "backups",
            "capabilities",
            "providers",
            "recentIssues",
            "schemaVersion",
            "since",
            "state",
            "versions"
        ]
    );
    let text = value.to_string();
    for leak in [
        "box",
        "/dev/sda2",
        "eth0",
        "00112233445566778899aabbccddeeff",
        "/tmp",
        "debian",
    ] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
}

#[test]
fn issues_are_bounded_counted_and_most_recent_first() {
    let issues = Issues::default();
    let now = OffsetDateTime::now_utc();
    for n in 0..100 {
        issues.record("procfs_unreadable", &format!("field{n}"), now);
    }
    issues.record("procfs_unreadable", "field99", now);
    let snapshot = issues.snapshot();
    assert_eq!(snapshot.len(), MAX_ISSUES);
    assert_eq!(snapshot[0].subject, "field99");
    assert_eq!(snapshot[0].count, 2);

    // Problems are recorded; states of the machine are not.
    let tree = Tree::new();
    let (system, _state) = system(&tree, AgentStatus::new());
    let _ = system.metrics(Instant::now());
    let recorded: Vec<&str> = system
        .issues
        .snapshot()
        .iter()
        .map(|i| i.code)
        .collect::<Vec<_>>()
        .into_iter()
        .collect();
    assert!(recorded.contains(&"procfs_unreadable"));
    assert!(!recorded.contains(&"first_sample_pending"));
    assert!(!recorded.contains(&"no_hwmon_sensors"));
}
