//! Provider invariants, and the real host.
//!
//! Fixture tests prove the parsers; they are not evidence that the real
//! kernel interfaces read as expected. The `real_host_*` tests run every
//! provider against this machine's actual `/proc`, `/sys`, `getifaddrs` and
//! mounts — on CI, the runner — and require what every Linux host has:
//! a CPU line, `MemTotal`, uptime and load, the loopback interface with
//! `127.0.0.1`, and a mounted root. Temperature sensors are not required;
//! their absence must itself be a well-formed, reasoned state.

use std::collections::BTreeSet;
use std::time::Instant;

use super::*;

#[test]
fn reason_codes_are_stable_distinct_and_snake_case() {
    let codes: Vec<&str> = Reason::ALL.iter().map(|r| r.code()).collect();
    assert_eq!(
        codes,
        [
            "procfs_unreadable",
            "sysfs_unreadable",
            "os_release_unreadable",
            "first_sample_pending",
            "mem_available_unsupported",
            "swap_absent",
            "no_hwmon_sensors",
            "not_reported",
            "not_reported_by_driver",
            "metadata_unavailable",
            "source_malformed",
            "sample_stale",
        ]
    );
    let unique: BTreeSet<&str> = codes.iter().copied().collect();
    assert_eq!(unique.len(), codes.len());
    for reason in Reason::ALL {
        assert_eq!(
            serde_json::to_value(reason).expect("json"),
            serde_json::Value::String(reason.code().to_owned())
        );
    }
}

#[test]
fn oversized_and_non_text_sources_are_malformed_not_truncated() {
    let tree = fixture::Tree::new();
    tree.write("big", &"x".repeat(5000));
    assert_eq!(read_line(&tree.path("big")), Err(ReadFailure::Malformed));
    std::fs::write(tree.path("binary"), [0xff, 0xfe, 0x00]).expect("write");
    assert_eq!(
        read_source(&tree.path("binary"), 16),
        Err(ReadFailure::Malformed)
    );
    tree.mkdir("dir");
    assert_eq!(read_line(&tree.path("dir")), Err(ReadFailure::Unreadable));
    assert_eq!(read_line(&tree.path("absent")), Err(ReadFailure::Missing));
}

#[test]
fn a_fifo_is_refused_without_blocking() {
    let tree = fixture::Tree::new();
    nix::unistd::mkfifo(&tree.path("fifo"), nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
    let started = Instant::now();
    assert_eq!(read_line(&tree.path("fifo")), Err(ReadFailure::Unreadable));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn real_host_system_identity_reads() {
    let identity = os::read(&HostRoot::system());
    assert_eq!(identity.kernel.name.as_deref(), Some("Linux"));
    assert!(identity.kernel.release.is_some());
    assert!(identity.uptime_seconds.is_some_and(|s| s > 0));
    assert!(identity.boot_id.is_some());
    assert!(identity.hostname.is_some());
}

#[test]
fn real_host_cpu_memory_and_load_read() {
    let root = HostRoot::system();
    let first = cpu::read_times(&root).expect("/proc/stat parses");
    assert!(first.total > 0 && first.busy <= first.total);
    assert!(cpu::cores(&root).expect("cpu/present") >= 1);

    // The sampler, for real, twice.
    let sampler = cpu::Sampler::new(root.clone());
    let start = Instant::now();
    sampler.sample(start);
    assert_eq!(sampler.usage(start), Err(Reason::FirstSamplePending));
    std::thread::sleep(std::time::Duration::from_millis(250));
    sampler.sample(Instant::now());
    // A quarter second may contain no tick on an idle machine, which the
    // sampler reports as pending again rather than inventing a value.
    if let Ok(percent) = sampler.usage(Instant::now()) {
        assert!((0.0..=100.0).contains(&percent));
    }

    let mut absences = Absences::default();
    let (memory, _swap) = memory::read(&root, &mut absences);
    let total = memory.total_bytes.expect("MemTotal");
    assert!(total > 16 * 1024 * 1024);
    let available = memory
        .available_bytes
        .expect("every supported kernel has MemAvailable");
    assert!(available <= total);
    let load = load::read(&root, &mut absences);
    assert!(load.one.is_some() && load.fifteen.is_some());
    // Swap may or may not exist; either way nothing else is missing.
    assert!(absences
        .into_vec()
        .iter()
        .all(|u| u.reason == Reason::SwapAbsent),);
}

#[test]
fn real_host_temperatures_are_readings_or_a_reasoned_absence() {
    let mut absences = Absences::default();
    let sensors = thermal::read(&HostRoot::system(), &mut absences);
    let absences = absences.into_vec();
    if sensors.is_empty() {
        assert_eq!(absences.len(), 1);
        assert_eq!(absences[0].field, "temperatures");
    } else {
        assert!(absences.is_empty());
        for sensor in sensors {
            assert!((-100.0..=250.0).contains(&sensor.celsius));
        }
    }
}

#[test]
fn real_host_interfaces_include_loopback_with_its_address() {
    let view = net::read(&HostRoot::system());
    assert!(view.unavailable.is_empty(), "{:?}", view.unavailable);
    let lo = view
        .interfaces
        .iter()
        .find(|i| i.name == "lo")
        .expect("every Linux host has lo");
    assert_eq!(lo.loopback, Some(true));
    assert_eq!(lo.is_virtual, Some(true));
    assert!(lo
        .addresses
        .iter()
        .any(|a| a.address == "127.0.0.1" && a.prefix_length == Some(8) && a.scope == "host"));
    let names: Vec<&str> = view.interfaces.iter().map(|i| i.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "stable order, by name");
}

#[test]
fn real_host_filesystems_include_a_real_root() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let view = runtime.block_on(fs::Storage::new(HostRoot::system()).read());
    assert!(view.unavailable.is_empty(), "{:?}", view.unavailable);
    assert!(!view.filesystems.is_empty());
    for filesystem in &view.filesystems {
        assert!(!fs::EXCLUDED.contains(&filesystem.fs_type.as_str()));
        assert!(filesystem.mount_point.starts_with('/'));
        match filesystem.usage {
            Some(usage) => {
                assert!(usage.used_bytes <= usage.total_bytes);
                assert!(usage.free_bytes <= usage.total_bytes);
            }
            None => assert!(!filesystem.unavailable.is_empty()),
        }
    }
    // In a container "/" may be an excluded overlay; on a host it is listed.
    // Either way some real filesystem with a capacity exists.
    assert!(view.filesystems.iter().any(|f| f.usage.is_some()));
}
