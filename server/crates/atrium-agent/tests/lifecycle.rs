//! Process lifecycle for Atrium Agent.
//!
//! Every assertion reports the child's own stderr on failure, for the reason
//! given in the Core equivalent: the process usually explains itself, and
//! throwing that away costs a diagnosis.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);
const SETTLE: Duration = Duration::from_millis(200);

type Capture = Arc<Mutex<Vec<String>>>;

/// A private directory for one test's socket, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("atrium-agent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory must be creatable");
        Self(path)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("agent.sock")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running `atrium-agent`, with everything it has written to stderr.
struct Proc {
    child: Child,
    capture: Capture,
}

impl Proc {
    fn start(socket: Option<&Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_atrium-agent"));
        command
            .env("ATRIUM_AGENT_LOG", "info")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("LISTEN_FDS")
            .env_remove("LISTEN_PID")
            .env_remove("ATRIUM_AGENT_SOCKET")
            .env(
                "ATRIUM_AGENT_STATE_DIR",
                std::env::temp_dir().join("atrium-agent-no-state"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(path) = socket {
            command.env("ATRIUM_AGENT_SOCKET", path);
        }

        let mut child = command.spawn().expect("atrium-agent must be spawnable");
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

fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => format!("ended in an unknown way: {status:?}"),
    }
}

#[test]
fn listens_and_shuts_down_cleanly() {
    let scratch = Scratch::new("accept");
    let socket = scratch.socket();
    let mut agent = Proc::start(Some(&socket));
    agent.wait_for("\"event\":\"ready\"");

    let mut stream = UnixStream::connect(&socket).expect("the socket must accept a connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout must be settable");

    // A peer that says nothing and closes its side gets nothing back: no
    // banner, no greeting, no information. The protocol itself is exercised
    // in tests/protocol.rs.
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown must succeed");
    let mut buffer = [0_u8; 1];
    let read = stream.read(&mut buffer).expect("reading must not error");
    assert_eq!(read, 0, "agent must close the connection without speaking");
    drop(stream);

    agent.shutdown_cleanly();
}

#[test]
fn refuses_to_start_without_a_socket() {
    // Fail closed: no guessed path, no silent fallback.
    let mut agent = Proc::start(None);
    let status = agent.wait_exit();
    assert!(
        !status.success(),
        "starting with no socket configured must fail, {}",
        describe(status)
    );
    let lines = agent.lines();
    assert!(
        lines.iter().any(|line| line.contains("startup_failed")),
        "the refusal must be logged: {lines:#?}"
    );
}

#[test]
fn refuses_a_relative_socket_path() {
    let mut agent = Proc::start(Some(Path::new("relative.sock")));
    let status = agent.wait_exit();
    assert!(
        !status.success(),
        "a relative socket path must be refused, {}",
        describe(status)
    );
}

#[test]
fn refuses_to_bind_over_an_existing_path() {
    let scratch = Scratch::new("occupied");
    let socket = scratch.socket();
    std::fs::write(&socket, b"not ours").expect("the decoy file must be writable");

    let mut agent = Proc::start(Some(&socket));
    let status = agent.wait_exit();
    assert!(
        !status.success(),
        "agent must not unlink a path it did not create, {}",
        describe(status)
    );
    assert!(socket.exists(), "the existing file must be left alone");
}

#[test]
fn holds_no_tcp_socket() {
    // Agent speaks one Unix socket. The unit enforces that with PrivateNetwork
    // and RestrictAddressFamilies=AF_UNIX; this checks the process itself does
    // not try, by intersecting its socket inodes with the kernel's TCP tables.
    let scratch = Scratch::new("tcp");
    let mut agent = Proc::start(Some(&scratch.socket()));
    agent.wait_for("\"event\":\"ready\"");

    let held = socket_inodes(agent.child.id());
    let tcp = tcp_inodes();
    let tcp_held: Vec<u64> = held.iter().copied().filter(|i| tcp.contains(i)).collect();

    agent.shutdown_cleanly();

    assert!(
        tcp_held.is_empty(),
        "atrium-agent must hold no TCP socket, found inodes {tcp_held:?}"
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
