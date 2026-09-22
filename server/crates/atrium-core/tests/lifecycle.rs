//! Process lifecycle for Atrium Core.
//!
//! These spawn the real binary, because the property under test is what the
//! process does when the operating system signals it — something a unit test
//! cannot stand in for.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Spawns the binary with its stderr piped and a predictable log filter.
fn spawn() -> Child {
    Command::new(env!("CARGO_BIN_EXE_atrium-core"))
        .env("ATRIUM_CORE_LOG", "info")
        .env_remove("NOTIFY_SOCKET")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("atrium-core must be spawnable")
}

/// Reads stderr until a line containing `needle` arrives, returning everything
/// read. Fails rather than hanging if the process never says it.
fn wait_for_line(child: &mut Child, needle: &str) -> Vec<String> {
    let stderr = child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                return;
            }
        }
    });

    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {needle:?}; saw: {seen:#?}"
        );
        match receiver.recv_timeout(remaining) {
            Ok(line) => {
                let matched = line.contains(needle);
                seen.push(line);
                if matched {
                    return seen;
                }
            }
            Err(_) => panic!("stderr closed before {needle:?}; saw: {seen:#?}"),
        }
    }
}

fn terminate(child: &mut Child) {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).expect("pid fits in i32"));
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM)
        .expect("SIGTERM must be sendable");
}

fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("wait must succeed") {
            return status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the process did not exit within {EXIT_TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn starts_reports_its_identity_and_signals_readiness() {
    let mut child = spawn();
    let lines = wait_for_line(&mut child, "\"event\":\"ready\"");

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

    terminate(&mut child);
    let status = wait_for_exit(&mut child);
    assert!(
        status.success(),
        "clean shutdown expected, {}",
        describe(status)
    );
}

#[test]
fn sigterm_shuts_down_cleanly() {
    let mut child = spawn();
    wait_for_line(&mut child, "\"event\":\"ready\"");
    terminate(&mut child);

    let status = wait_for_exit(&mut child);
    assert!(
        status.success(),
        "SIGTERM must produce a clean exit, {}",
        describe(status)
    );
}

#[test]
fn logs_are_json_on_stderr() {
    let mut child = spawn();
    let lines = wait_for_line(&mut child, "\"event\":\"ready\"");
    terminate(&mut child);
    wait_for_exit(&mut child);

    for line in &lines {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "every log line must be a JSON object: {line}"
        );
    }
}

#[test]
fn opens_no_tcp_socket() {
    // M1A has no HTTP API. Rather than trust that nobody added one, this
    // intersects the process's own socket inodes with the kernel's TCP tables.
    // An accidental listener would be an unauthenticated management surface,
    // and this is the direct question to ask about it.
    //
    // Unix socket pairs are expected here - tokio's signal driver uses one - so
    // the test asks about TCP specifically rather than counting sockets.
    let mut child = spawn();
    wait_for_line(&mut child, "\"event\":\"ready\"");

    let held = socket_inodes(child.id());
    let tcp = tcp_inodes();
    let tcp_held: Vec<u64> = held.iter().copied().filter(|i| tcp.contains(i)).collect();

    terminate(&mut child);
    wait_for_exit(&mut child);

    assert!(
        tcp_held.is_empty(),
        "atrium-core must hold no TCP socket in M1A, found inodes {tcp_held:?}"
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

/// Exact description of how a process ended: an exit code and a signal are very
/// different failures, and a bare `.success()` hides which one happened.
fn describe(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => format!("ended in an unknown way: {status:?}"),
    }
}
