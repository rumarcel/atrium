//! Process lifecycle for Atrium Core.
//!
//! These spawn the real binary, because the property under test is what the
//! process does when the operating system signals it — something a unit test
//! cannot stand in for. Every assertion reports the child's own stderr on
//! failure: a lifecycle bug is almost always explained by the last thing the
//! process said, and discarding that turns a five-minute diagnosis into a
//! guessing game.
//!
//! From M1B, Core needs an installation tree. These tests give it one with a
//! configuration and no identity, so it comes up in recovery mode — which is
//! still a started, ready, signal-handling process, and the lifecycle
//! properties are the same in either mode. Normal mode is exercised by the
//! privileged suite, where the identity can be owned by root as it must be.

#![cfg(unix)]

mod common;

use common::{Proc, Tree};

#[test]
fn starts_reports_its_identity_and_signals_readiness() {
    let tree = Tree::new("identity");
    let mut core = Proc::start(&tree.root);
    core.ready();

    let lines = core.lines();
    let startup = lines
        .iter()
        .find(|line| line.contains("\"event\":\"starting\""))
        .expect("startup must be logged before readiness");

    // Criterion 5 asks that Core runs as atrium rather than root. The unit
    // enforces it; the process logs what it actually is, so the journal shows
    // the truth rather than the intent.
    for field in [
        "\"component\":\"atrium-core\"",
        "\"pid\":",
        "\"euid\":",
        "\"version\":",
    ] {
        assert!(
            startup.contains(field),
            "startup line is missing {field}: {startup}"
        );
    }

    core.shutdown_cleanly();
}

#[test]
fn sigterm_shuts_down_cleanly() {
    let tree = Tree::new("sigterm");
    let mut core = Proc::start(&tree.root);
    core.ready();
    core.shutdown_cleanly();

    let lines = core.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"event\":\"stopped\"")),
        "a clean shutdown must be logged: {lines:#?}"
    );
}

#[test]
fn logs_are_json_on_stderr() {
    let tree = Tree::new("json");
    let mut core = Proc::start(&tree.root);
    core.ready();
    core.shutdown_cleanly();

    for line in &core.lines() {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "every log line must be a JSON object: {line}"
        );
    }
}

#[test]
fn opens_no_tcp_socket() {
    // M1B has no HTTP API. Rather than trust that nobody added one, this
    // intersects the process's own socket inodes with the kernel's TCP tables.
    // An accidental listener would be an unauthenticated management surface.
    //
    // Unix socket pairs are expected - tokio's signal driver uses one - so the
    // test asks about TCP specifically rather than counting sockets.
    let tree = Tree::new("tcp");
    let mut core = Proc::start(&tree.root);
    core.ready();

    let held = socket_inodes(core.child.id());
    let tcp = tcp_inodes();
    let tcp_held: Vec<u64> = held.iter().copied().filter(|i| tcp.contains(i)).collect();

    core.shutdown_cleanly();

    assert!(
        tcp_held.is_empty(),
        "atrium-core must hold no TCP socket in M1B, found inodes {tcp_held:?}"
    );
}

/// Inodes of every socket this process holds.
fn socket_inodes(pid: u32) -> Vec<u64> {
    let mut inodes = Vec::new();
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return inodes;
    };
    for entry in entries.flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let text = target.to_string_lossy();
        if let Some(rest) = text.strip_prefix("socket:[") {
            if let Some(number) = rest.strip_suffix(']') {
                if let Ok(inode) = number.parse::<u64>() {
                    inodes.push(inode);
                }
            }
        }
    }
    inodes
}

/// Inodes of every TCP socket on the host, v4 and v6.
fn tcp_inodes() -> Vec<u64> {
    let mut inodes = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            if let Some(field) = line.split_whitespace().nth(9) {
                if let Ok(inode) = field.parse::<u64>() {
                    inodes.push(inode);
                }
            }
        }
    }
    inodes
}
