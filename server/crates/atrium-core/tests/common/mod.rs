//! Shared fixtures for Core's process tests.
//!
//! Every test builds its own installation tree in a temporary directory and
//! points the binary at it with `ATRIUM_ROOT`, so tests run in parallel and
//! never touch `/etc/atrium` or `/var/lib/atrium`.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const READY_TIMEOUT: Duration = Duration::from_secs(20);
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
pub const POLL: Duration = Duration::from_millis(25);
/// Time for the reader thread to drain whatever the child wrote on its way out.
pub const SETTLE: Duration = Duration::from_millis(200);

pub const SERVER_NAME: &str = "test-server";

type Capture = Arc<Mutex<Vec<String>>>;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A private installation tree: `etc/atrium` and `var/lib/atrium`, both
/// `0700`, and a valid `core.toml` — the state the installer leaves before
/// `init-identity` runs.
pub struct Tree {
    pub root: PathBuf,
}

impl Tree {
    pub fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "atrium-it-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let tree = Self { root };
        for dir in [tree.etc(), tree.state()] {
            std::fs::create_dir_all(&dir).expect("fixture directory");
            chmod(&dir, 0o700);
        }
        tree.write_config(&format!("server_name = \"{SERVER_NAME}\"\n"));
        tree
    }

    pub fn etc(&self) -> PathBuf {
        self.root.join("etc/atrium")
    }

    pub fn state(&self) -> PathBuf {
        self.root.join("var/lib/atrium")
    }

    pub fn write_config(&self, text: &str) {
        let path = self.etc().join("core.toml");
        std::fs::write(&path, text).expect("core.toml");
        chmod(&path, 0o600);
    }

    /// Runs `atrium-core init-identity` against this tree.
    pub fn init(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_atrium-core"))
            .arg("init-identity")
            .env("ATRIUM_ROOT", &self.root)
            .output()
            .expect("init-identity must be spawnable")
    }

    /// Every regular file under the root, with its bytes, sorted.
    pub fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        walk(&self.root, &mut files);
        files.sort();
        files
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn walk(dir: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&path, files);
        } else if kind.is_file() {
            files.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
        } else {
            files.push((path, Vec::new()));
        }
    }
}

pub fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

/// A running `atrium-core`, with everything it has written to stderr.
pub struct Proc {
    pub child: Child,
    capture: Capture,
}

impl Proc {
    pub fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_atrium-core"))
            .env("ATRIUM_CORE_LOG", "info")
            .env("ATRIUM_ROOT", root)
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

    pub fn lines(&self) -> Vec<String> {
        self.capture.lock().expect("capture lock").clone()
    }

    /// Waits for a line containing `needle`, or fails with everything seen.
    pub fn wait_for(&self, needle: &str) -> String {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let lines = self.lines();
            if let Some(line) = lines.iter().find(|line| line.contains(needle)) {
                return line.clone();
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?}; child said: {lines:#?}"
            );
            std::thread::sleep(POLL);
        }
    }

    /// Waits for readiness and returns the ready line.
    pub fn ready(&self) -> String {
        self.wait_for("\"event\":\"ready\"")
    }

    pub fn terminate(&self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).expect("pid fits"));
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM)
            .expect("SIGTERM must be sendable");
    }

    pub fn wait_exit(&mut self) -> ExitStatus {
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

    /// Asserts the process is still running.
    pub fn assert_running(&mut self) {
        let status = self.child.try_wait().expect("wait must succeed");
        assert!(
            status.is_none(),
            "the process exited ({}); child said: {:#?}",
            status.map(describe).unwrap_or_default(),
            self.lines()
        );
    }

    /// Terminates and requires a clean exit.
    pub fn shutdown_cleanly(&mut self) {
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

impl Drop for Proc {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// An exit code and a signal are very different failures, and a bare
/// `.success()` hides which one happened.
pub fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => format!("ended in an unknown way: {status:?}"),
    }
}

/// The value of a string field in a JSON log line, without a JSON parser.
pub fn field(line: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let start = line.find(&key)? + key.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_owned())
}
