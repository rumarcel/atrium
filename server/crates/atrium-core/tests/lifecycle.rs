//! Process lifecycle for Atrium Core.
//!
//! These spawn the real binary, because the property under test is what the
//! process does when the operating system signals it — something a unit test
//! cannot stand in for. Every assertion reports the child's own stderr on
//! failure: a lifecycle bug is almost always explained by the last thing the
//! process said, and discarding that turns a five-minute diagnosis into a
//! guessing game.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);
/// Time for the reader thread to drain whatever the child wrote on its way out.
const SETTLE: Duration = Duration::from_millis(200);

type Capture = Arc<Mutex<Vec<String>>>;

/// A running `atrium-core`, with everything it has written to stderr.
struct Proc {
    child: Child,
    capture: Capture,
}

impl Proc {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_atrium-core"))
            .env("ATRIUM_CORE_LOG", "info")
            .env_remove("NOTIFY_SOCKET")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("atrium-core must be spawnable");

        let stderr = child.stderr.take().expect("stderr was piped");
        let capture: Capture = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&capture);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().expect("capture lock").push(line);
            }
        });

        Self { child, capture }
    }

    fn lines(&self) -> Vec<String> {
        self.capture.lock().expect("capture lock").clone()
    }

    /// Waits for a line containing `needle`, or fails with everything seen.
    fn wait_for(&self, needle: &str) -> Vec<String> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let lines = self.lines();
            if lines.iter().any(|line| line.contains(needle)) {
                return lines;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; child said: {lines:#?}"
            );
            std::thread::sleep(POLL);
        }
    }

    fn terminate(&self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).expect("pid fits"));
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM)
            .expect("SIGTERM must be sendable");
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait must succeed") {
                std::thread::sleep(SETTLE);
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the process did not exit within {EXIT_TIMEOUT:?}; child said: {:#?}",
                self.lines()
            );
            std::thread::sleep(POLL);
        }
    }

    /// Terminates and requires a clean exit.
    fn shutdown_cleanly(&mut self) {
        self.terminate();
        let status = self.wait_exit();
        assert!(
            status.success(),
            "clean shutdown expected, {}; child said: {:#?}",
            describe(status),
            self.lines()
        );
    }
}

/// An exit code and a signal are very different failures, and a bare
/// `.success()` hides which one happened.
fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => format!("ended in an unknown way: {status:?}"),
    }
}

#[test]
fn starts_reports_its_identity_and_signals_readiness() {
    let mut core = Proc::start();
    let lines = core.wait_for("\"event\":\"ready\"");

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
    let mut core = Proc::start();
    core.wait_for("\"event\":\"ready\"");
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
    let mut core = Proc::start();
    let lines = core.wait_for("\"event\":\"ready\"");
    core.shutdown_cleanly();

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
    // An accidental listener would be an unauthenticated management surface.
    //
    // Unix socket pairs are expected - tokio's signal driver uses one - so the
    // test asks about TCP specifically rather than counting sockets.
    let mut core = Proc::start();
    core.wait_for("\"event\":\"ready\"");

    let held = socket_inodes(core.child.id());
    let tcp = tcp_inodes();
    let tcp_held: Vec<u64> = held.iter().copied().filter(|i| tcp.contains(i)).collect();

    core.shutdown_cleanly();

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
