//! Process lifecycle for Atrium Agent.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const READY_TIMEOUT: Duration = Duration::from_secs(20);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);

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

fn spawn(socket: Option<&Path>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_atrium-agent"));
    command
        .env("ATRIUM_AGENT_LOG", "info")
        .env_remove("NOTIFY_SOCKET")
        .env_remove("LISTEN_FDS")
        .env_remove("LISTEN_PID")
        .env_remove("ATRIUM_AGENT_SOCKET")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(path) = socket {
        command.env("ATRIUM_AGENT_SOCKET", path);
    }
    command.spawn().expect("atrium-agent must be spawnable")
}

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
fn listens_accepts_and_closes_then_shuts_down_cleanly() {
    let scratch = Scratch::new("accept");
    let socket = scratch.socket();
    let mut child = spawn(Some(&socket));
    wait_for_line(&mut child, "\"event\":\"ready\"");

    let mut stream = UnixStream::connect(&socket).expect("the socket must accept a connection");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout must be settable");

    // M1A defines no protocol, so Agent closes immediately and the read sees
    // end-of-file. Anything else would mean a parser exists that nobody
    // reviewed.
    let mut buffer = [0_u8; 1];
    let read = stream.read(&mut buffer).expect("reading must not error");
    assert_eq!(read, 0, "agent must close the connection without speaking");

    terminate(&mut child);
    assert!(wait_for_exit(&mut child).success());
}

#[test]
fn refuses_to_start_without_a_socket() {
    // Fail closed: no guessed path, no silent fallback.
    let mut child = spawn(None);
    let status = wait_for_exit(&mut child);
    assert!(
        !status.success(),
        "starting with no socket configured must fail, got {status:?}"
    );

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr was piped")
        .read_to_string(&mut stderr)
        .expect("stderr must be readable");
    assert!(
        stderr.contains("startup_failed"),
        "the refusal must be logged: {stderr}"
    );
}

#[test]
fn refuses_a_relative_socket_path() {
    let mut child = spawn(Some(Path::new("relative.sock")));
    let status = wait_for_exit(&mut child);
    assert!(
        !status.success(),
        "a relative socket path must be refused, got {status:?}"
    );
}

#[test]
fn refuses_to_bind_over_an_existing_path() {
    let scratch = Scratch::new("occupied");
    let socket = scratch.socket();
    std::fs::write(&socket, b"not ours").expect("the decoy file must be writable");

    let mut child = spawn(Some(&socket));
    let status = wait_for_exit(&mut child);
    assert!(
        !status.success(),
        "agent must not unlink a path it did not create, got {status:?}"
    );
    assert!(socket.exists(), "the existing file must be left alone");
}

#[test]
fn holds_no_tcp_socket() {
    // Agent speaks one Unix socket. The unit enforces that with PrivateNetwork
    // and RestrictAddressFamilies=AF_UNIX; this checks the process itself does
    // not try, by intersecting its socket inodes with the kernel's TCP tables.
    let scratch = Scratch::new("tcp");
    let mut child = spawn(Some(&scratch.socket()));
    wait_for_line(&mut child, "\"event\":\"ready\"");

    let held = socket_inodes(child.id());
    let tcp = tcp_inodes();
    let tcp_held: Vec<u64> = held.iter().copied().filter(|i| tcp.contains(i)).collect();

    terminate(&mut child);
    wait_for_exit(&mut child);

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
