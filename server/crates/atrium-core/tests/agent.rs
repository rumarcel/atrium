//! Core's Agent client and monitor, against the real Agent binary and against
//! hostile fake agents.
//!
//! These run unprivileged. Normal-mode Core needs a root-owned identity, so
//! the Core *binary* talking to a root Agent is in `atriumctl`'s privileged
//! suite. Here the client and the monitor are driven as a library, with a
//! real state database, so every path — success, Agent stopped, mismatch,
//! garbage, refusal — is exercised with the audit rows it writes.

#![cfg(unix)]

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atrium_core::agentmonitor::Monitor;
use atrium_core::capability::Reason;
use atrium_core::db::{self, Database};
use atrium_core::layout::Layout;
use common::Tree;
use serde_json::Value;

/// The Agent binary, built alongside this test's crate by `cargo test` for
/// the workspace.
fn agent_binary() -> PathBuf {
    let deps = std::env::current_exe().expect("exe");
    let path = deps
        .parent()
        .and_then(Path::parent)
        .expect("target dir")
        .join("atrium-agent");
    assert!(
        path.exists(),
        "{} must be built first (cargo build --workspace)",
        path.display()
    );
    path
}

struct Fixture {
    tree: Tree,
    layout: Layout,
    database: Database,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let tree = Tree::new(tag);
        let layout = Layout::under(&tree.root).expect("layout");
        let database = db::create(&layout).expect("database");
        std::fs::create_dir_all(layout.agent_socket().parent().expect("run dir")).expect("run");
        Self {
            tree,
            layout,
            database,
        }
    }

    fn agent_state(&self) -> PathBuf {
        self.tree.root.join("var/lib/atrium-agent")
    }

    /// `(request_id, target, outcome, error_code)` of every `agent.call` row.
    fn audit(&self) -> Vec<(String, String, String, Option<String>)> {
        let mut statement = self
            .database
            .connection()
            .prepare(
                "SELECT request_id, target, outcome, error_code FROM audit \
                 WHERE action = 'agent.call' ORDER BY id",
            )
            .expect("prepare");
        statement
            .query_map((), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }
}

/// A real, unprivileged Agent on the fixture's socket. It admits this test
/// process's uid, which is also the uid the client connects as.
struct RealAgent {
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
}

impl RealAgent {
    fn start(fixture: &Fixture) -> Self {
        let state = fixture.agent_state();
        std::fs::create_dir_all(&state).expect("agent state");
        common::chmod(&state, 0o700);
        let mut child = Command::new(agent_binary())
            .env("ATRIUM_AGENT_LOG", "info")
            .env("ATRIUM_AGENT_SOCKET", fixture.layout.agent_socket())
            .env("ATRIUM_AGENT_STATE_DIR", &state)
            .env_remove("NOTIFY_SOCKET")
            .env_remove("LISTEN_FDS")
            .env_remove("LISTEN_PID")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn agent");
        let stderr = child.stderr.take().expect("stderr");
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().expect("lock").push(line);
            }
        });
        let agent = Self { child, log };
        let deadline = Instant::now() + Duration::from_secs(20);
        while !agent
            .log
            .lock()
            .expect("lock")
            .iter()
            .any(|line| line.contains("\"event\":\"ready\""))
        {
            assert!(Instant::now() < deadline, "agent never became ready");
            std::thread::sleep(Duration::from_millis(25));
        }
        agent
    }

    fn stop(mut self, socket: &Path) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).expect("pid"));
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("term");
        assert!(self.child.wait().expect("wait").success());
        let _ = std::fs::remove_file(socket);
    }
}

impl Drop for RealAgent {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn journal(state: &Path) -> Vec<Value> {
    let dir = state.join("journal");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    files.sort();
    files
        .iter()
        .flat_map(|path| {
            std::fs::read_to_string(path)
                .expect("journal")
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).expect("json"))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// A fake agent: accepts connections on `socket`, counts them, and answers
/// each with `respond`, which gets the raw stream after the peer's first
/// frame has been read.
struct FakeAgent {
    connections: Arc<AtomicUsize>,
}

impl FakeAgent {
    fn start(socket: &Path, respond: fn(&mut std::os::unix::net::UnixStream)) -> Self {
        let listener = UnixListener::bind(socket).expect("bind fake agent");
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("timeout");
                let mut header = [0_u8; 4];
                if stream.read_exact(&mut header).is_err() {
                    continue;
                }
                let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
                if stream.read_exact(&mut body).is_err() {
                    continue;
                }
                respond(&mut stream);
            }
        });
        Self { connections }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

fn send_json(stream: &mut std::os::unix::net::UnixStream, text: &str) {
    let mut bytes = u32::try_from(text.len())
        .expect("fits")
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(text.as_bytes());
    let _ = stream.write_all(&bytes);
}

// ---------------------------------------------------------------------------

#[test]
fn every_call_is_audited_by_core_and_journaled_by_agent_with_one_request_id() {
    let fixture = Fixture::new("correlate");
    let agent = RealAgent::start(&fixture);
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");

    let rt = runtime();
    for _ in 0..3 {
        let capabilities = rt.block_on(monitor.refresh(&fixture.database)).clone();
        assert!(capabilities.privileged.available, "{capabilities:?}");
        assert_eq!(capabilities.container.via(), "agent");
    }

    let audit = fixture.audit();
    assert_eq!(audit.len(), 6, "AgentInfo and RuntimeProbe, three times");
    let lines = journal(&fixture.agent_state());
    assert_eq!(
        lines.len(),
        audit.len(),
        "one journal line per audited call"
    );
    for (request_id, target, outcome, code) in &audit {
        assert_eq!(outcome, "ok");
        assert_eq!(code, &None);
        let line = lines
            .iter()
            .find(|line| line["request_id"] == request_id.as_str())
            .unwrap_or_else(|| panic!("{request_id} is not in Agent's journal: {lines:#?}"));
        assert_eq!(line["op"], target.as_str());
        assert_eq!(line["accepted"], true);
        assert_eq!(line["peer"]["uid"], nix::unistd::geteuid().as_raw());
        assert_eq!(request_id.len(), 16);
    }
    let ids: std::collections::HashSet<&String> = audit.iter().map(|row| &row.0).collect();
    assert_eq!(ids.len(), audit.len(), "a fresh id per call");
    agent.stop(fixture.layout.agent_socket());
}

#[test]
fn agent_stopped_makes_both_capabilities_unavailable_and_nothing_else_is_tried() {
    let fixture = Fixture::new("stopped");
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");
    let capabilities = runtime()
        .block_on(monitor.refresh(&fixture.database))
        .clone();
    assert!(!capabilities.privileged.available);
    assert!(!capabilities.container.available);
    assert_eq!(
        capabilities.summary(),
        (
            Some(Reason::AgentUnreachable),
            Some(Reason::AgentUnreachable)
        )
    );
    assert_eq!(
        capabilities.privileged.missing[0].reason.code(),
        "agent_unreachable"
    );
    assert_eq!(
        capabilities.container.missing[0].reason.code(),
        "agent_unreachable"
    );
    assert_eq!(capabilities.container.via(), "agent");

    // One attempt, AgentInfo, audited as failed; RuntimeProbe is not tried
    // once Agent is known to be down, and nothing but the socket is touched.
    let audit = fixture.audit();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].1, "agent_info");
    assert_eq!(audit[0].2, "failed");
    assert_eq!(audit[0].3.as_deref(), Some("socket_missing"));
}

#[test]
fn a_stale_socket_with_nobody_listening_is_unreachable() {
    let fixture = Fixture::new("stale");
    drop(UnixListener::bind(fixture.layout.agent_socket()).expect("bind"));
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");
    let capabilities = runtime()
        .block_on(monitor.refresh(&fixture.database))
        .clone();
    assert_eq!(capabilities.summary().0, Some(Reason::AgentUnreachable));
    assert_eq!(fixture.audit()[0].3.as_deref(), Some("connection_refused"));
}

#[test]
fn a_protocol_mismatch_is_terminal_and_never_negotiated() {
    let fixture = Fixture::new("mismatch");
    let fake = FakeAgent::start(fixture.layout.agent_socket(), |stream| {
        send_json(
            stream,
            r#"{"error":{"code":"protocol_version_mismatch","agent_protocol":2}}"#,
        );
    });
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");
    let capabilities = runtime()
        .block_on(monitor.refresh(&fixture.database))
        .clone();
    assert_eq!(
        capabilities.summary(),
        (
            Some(Reason::AgentProtocolMismatch),
            Some(Reason::AgentProtocolMismatch)
        )
    );
    let audit = fixture.audit();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].2, "refused");
    assert_eq!(audit[0].3.as_deref(), Some("protocol_version_mismatch"));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fake.connections(),
        1,
        "no retry, no second protocol, no fallback"
    );
}

#[test]
fn a_hello_ok_for_another_version_is_also_a_mismatch() {
    let fixture = Fixture::new("hello-v2");
    FakeAgent::start(fixture.layout.agent_socket(), |stream| {
        send_json(
            stream,
            r#"{"hello_ok":{"protocol":2,"agent_version":"9.9.9"}}"#,
        );
    });
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");
    let capabilities = runtime()
        .block_on(monitor.refresh(&fixture.database))
        .clone();
    assert_eq!(
        capabilities.summary().0,
        Some(Reason::AgentProtocolMismatch)
    );
}

#[test]
fn a_journal_that_cannot_be_written_is_named() {
    let fixture = Fixture::new("journal");
    FakeAgent::start(fixture.layout.agent_socket(), |stream| {
        send_json(
            stream,
            r#"{"error":{"code":"journal_unavailable","agent_protocol":1}}"#,
        );
    });
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");
    let capabilities = runtime()
        .block_on(monitor.refresh(&fixture.database))
        .clone();
    assert_eq!(
        capabilities.summary().0,
        Some(Reason::AgentJournalUnavailable)
    );
}

#[test]
fn garbage_from_agent_is_a_protocol_error_not_a_crash() {
    for (tag, respond) in [
        (
            "garbage",
            (|stream: &mut std::os::unix::net::UnixStream| {
                let _ = stream.write_all(b"\x00\x00\x00\x05hello");
            }) as fn(&mut std::os::unix::net::UnixStream),
        ),
        ("oversized", |stream| {
            // Four gigabytes declared; Core must refuse on the header.
            let _ = stream.write_all(&u32::MAX.to_be_bytes());
        }),
        ("unknown-field", |stream| {
            send_json(
                stream,
                r#"{"hello_ok":{"protocol":1,"agent_version":"0.1.0","shell":"/bin/sh"}}"#,
            );
        }),
        ("path-for-a-socket", |stream| {
            send_json(
                stream,
                r#"{"runtime_probe":{"runtime":"docker","socket":"/var/run/x","reachable":true,"version":null,"version_reason":"runtime_version_probe_not_in_m1","also_present":[]}}"#,
            );
        }),
    ] {
        let fixture = Fixture::new(tag);
        FakeAgent::start(fixture.layout.agent_socket(), respond);
        let mut monitor = Monitor::new(&fixture.layout).expect("monitor");
        let capabilities = runtime()
            .block_on(monitor.refresh(&fixture.database))
            .clone();
        assert_eq!(
            capabilities.summary().0,
            Some(Reason::AgentProtocolError),
            "{tag}"
        );
        assert_eq!(
            fixture.audit()[0].3.as_deref(),
            Some("protocol_error"),
            "{tag}"
        );
    }
}

#[test]
fn agent_restart_is_picked_up_and_its_sequence_continues() {
    let fixture = Fixture::new("restart");
    let socket = fixture.layout.agent_socket().to_path_buf();
    let rt = runtime();
    let mut monitor = Monitor::new(&fixture.layout).expect("monitor");

    let agent = RealAgent::start(&fixture);
    assert!(
        rt.block_on(monitor.refresh(&fixture.database))
            .privileged
            .available
    );
    agent.stop(&socket);

    let down = rt.block_on(monitor.refresh(&fixture.database)).clone();
    assert_eq!(down.summary().0, Some(Reason::AgentUnreachable));

    let agent = RealAgent::start(&fixture);
    assert!(
        rt.block_on(monitor.refresh(&fixture.database))
            .privileged
            .available
    );
    agent.stop(&socket);

    let seqs: Vec<u64> = journal(&fixture.agent_state())
        .iter()
        .map(|line| line["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4], "continuous across the restart");
    let outcomes: Vec<String> = fixture.audit().into_iter().map(|row| row.2).collect();
    assert_eq!(outcomes, ["ok", "ok", "failed", "ok", "ok"]);
}
