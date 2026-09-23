//! Agent against hostile and malformed clients, over a real Unix socket.
//!
//! Core is treated as potentially compromised: none of these clients is the
//! real Core client. Each test writes raw bytes, reads raw frames back, and
//! checks Agent's journal on disk. Agent runs unprivileged here, so it
//! admits only this test process's own uid (see `atrium_agent::peer`); the
//! tests that need a second uid and a root Agent are in
//! `privilege_boundary.rs`.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atrium_protocol::frame;
use atrium_protocol::messages::{AgentFrame, ErrorCode};
use serde_json::{json, Value};

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(25);
static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Agent {
    dir: PathBuf,
    child: Option<Child>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl Agent {
    fn start(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atrium-agent-proto-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("state")).expect("dirs");
        chmod(&dir.join("state"), 0o700);
        let mut agent = Self {
            dir,
            child: None,
            lines: Arc::new(Mutex::new(Vec::new())),
        };
        agent.spawn();
        agent
    }

    fn spawn(&mut self) {
        // A fresh capture per process, so "ready" means this one is ready.
        self.lines = Arc::new(Mutex::new(Vec::new()));
        let mut child = Command::new(env!("CARGO_BIN_EXE_atrium-agent"))
            .env("ATRIUM_AGENT_LOG", "info")
            .env("ATRIUM_AGENT_SOCKET", self.socket())
            .env("ATRIUM_AGENT_STATE_DIR", self.state())
            .env_remove("NOTIFY_SOCKET")
            .env_remove("LISTEN_FDS")
            .env_remove("LISTEN_PID")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn atrium-agent");
        let stderr = child.stderr.take().expect("stderr");
        let sink = Arc::clone(&self.lines);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().expect("lock").push(line);
            }
        });
        self.child = Some(child);
        self.wait_for("\"event\":\"ready\"");
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).expect("pid"));
            nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("term");
            let status = child.wait().expect("wait");
            assert!(status.success(), "clean shutdown: {:#?}", self.log());
        }
        let _ = std::fs::remove_file(self.socket());
    }

    fn restart(&mut self) {
        self.stop();
        self.spawn();
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().expect("running").id()
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("agent.sock")
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn log(&self) -> Vec<String> {
        self.lines.lock().expect("lock").clone()
    }

    fn wait_for(&self, needle: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.log().iter().any(|line| line.contains(needle)) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle}: {:#?}",
                self.log()
            );
            std::thread::sleep(POLL);
        }
    }

    fn alive(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| child.try_wait().expect("try_wait").is_none())
    }

    fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(self.socket()).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .expect("timeout");
        stream
    }

    /// Every journal line on disk, parsed.
    fn journal(&self) -> Vec<Value> {
        journal_text(&self.state())
            .lines()
            .map(|line| serde_json::from_str(line).expect("every journal line is JSON"))
            .collect()
    }

    fn wait_journal(&self, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let lines = self.journal();
            if lines.len() >= count {
                return lines;
            }
            assert!(
                Instant::now() < deadline,
                "journal never reached {count} lines: {lines:#?}"
            );
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn journal_text(state: &Path) -> String {
    let dir = state.join("journal");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    names.sort();
    names
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("journal file"))
        .collect()
}

fn framed(value: &Value) -> Vec<u8> {
    raw(value.to_string().as_bytes())
}

fn raw(body: &[u8]) -> Vec<u8> {
    let mut bytes = u32::try_from(body.len())
        .expect("fits")
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(body);
    bytes
}

fn hello(id: &str) -> Value {
    json!({"hello": {"protocol": 1, "core_version": "0.1.0", "request_id": id}})
}

fn call(op: &str) -> Value {
    json!({"call": {"op": op}})
}

/// Reads one frame; `None` on end-of-file.
fn read_frame(stream: &mut UnixStream) -> Option<(AgentFrame, String)> {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => return None,
        Err(error) => panic!("read: {error}"),
    }
    let length = frame::body_length(header).expect("agent frames are well-formed");
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).expect("body");
    let text = String::from_utf8(body.clone()).expect("utf-8");
    let decoded = frame::decode_agent_frame(&body).expect("agent frames decode strictly");
    Some((decoded, text))
}

fn expect_error(stream: &mut UnixStream, code: ErrorCode) {
    match read_frame(stream) {
        Some((AgentFrame::Error(error), _)) => {
            assert_eq!(error.code, code);
            assert_eq!(error.agent_protocol, 1);
        }
        other => panic!("expected {code:?}, got {other:?}"),
    }
    assert!(
        read_frame(stream).is_none(),
        "the connection closes after a refusal"
    );
}

fn expect_hello_ok(stream: &mut UnixStream) {
    match read_frame(stream) {
        Some((AgentFrame::HelloOk(ok), _)) => assert_eq!(ok.protocol, 1),
        other => panic!("expected hello_ok, got {other:?}"),
    }
}

/// A full, valid exchange; returns the result frame and its raw text.
fn perform(agent: &Agent, id: &str, op: &str) -> (AgentFrame, String) {
    let mut stream = agent.connect();
    stream.write_all(&framed(&hello(id))).expect("hello");
    expect_hello_ok(&mut stream);
    stream.write_all(&framed(&call(op))).expect("call");
    let result = read_frame(&mut stream).expect("a result");
    assert!(read_frame(&mut stream).is_none(), "one call per connection");
    result
}

fn last(lines: &[Value]) -> &Value {
    lines.last().expect("a journal line")
}

fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

// ---------------------------------------------------------------------------

#[test]
fn agent_info_round_trip_is_journaled_with_the_kernel_peer() {
    let agent = Agent::start("info");
    let (result, _) = perform(&agent, "3f1c", "agent_info");
    let AgentFrame::AgentInfo(info) = result else {
        panic!("expected agent_info, got {result:?}");
    };
    assert_eq!(info.uid, euid());
    assert_eq!(info.pid, agent.pid());
    assert_eq!(info.protocol, 1);
    assert!(info.journal.writable);
    assert_eq!(
        info.state_dir.path.as_str(),
        agent.state().to_str().expect("utf-8")
    );
    assert_eq!(info.state_dir.mode.map(|m| m.bits()), Some(0o700));

    let lines = agent.journal();
    let line = last(&lines);
    assert_eq!(line["accepted"], true);
    assert_eq!(line["outcome"], "ok");
    assert_eq!(line["op"], "agent_info");
    assert_eq!(line["request_id"], "3f1c");
    assert_eq!(line["core_version"], "0.1.0");
    assert_eq!(line["peer"]["uid"], euid());
    assert_eq!(line["peer"]["pid"], std::process::id());
    assert!(line.get("reason").is_none());
    assert_eq!(line["seq"], 1);
}

#[test]
fn runtime_probe_names_no_path_no_version_and_no_liveness() {
    let agent = Agent::start("probe");
    let (result, text) = perform(&agent, "ab", "runtime_probe");
    let AgentFrame::RuntimeProbe(probe) = result else {
        panic!("expected runtime_probe, got {result:?}");
    };
    assert!(!text.contains('/'), "no path crosses the boundary: {text}");
    assert!(text.contains("\"version\":null"), "{text}");
    assert!(text.contains("\"liveness\":null"), "{text}");
    assert!(
        text.contains("\"liveness_reason\":\"passive_probe_in_m1\""),
        "{text}"
    );
    assert!(!text.contains("reachable"), "presence only: {text}");
    assert_eq!(probe.runtime, probe.socket.map(|s| s.runtime()));
    assert_eq!(last(&agent.journal())["op"], "runtime_probe");
}

#[test]
fn version_mismatch_fails_closed_and_changes_nothing() {
    let agent = Agent::start("mismatch");
    let mut stream = agent.connect();
    stream
        .write_all(&framed(
            &json!({"hello": {"protocol": 2, "core_version": "0.2.0", "request_id": "aa"}}),
        ))
        .expect("hello");
    expect_error(&mut stream, ErrorCode::ProtocolVersionMismatch);
    let line = last(&agent.journal()).clone();
    assert_eq!(line["accepted"], false);
    assert_eq!(line["reason"], "protocol_version_mismatch");
    assert_eq!(line["found_protocol"], 2);
    assert_eq!(line["request_id"], "aa");
    assert!(line.get("op").is_none());

    // A future hello of a different shape is still a named mismatch.
    let mut stream = agent.connect();
    stream
        .write_all(&framed(&json!({"hello": {"protocol": 0, "features": []}})))
        .expect("hello");
    expect_error(&mut stream, ErrorCode::ProtocolVersionMismatch);

    // No downgrade, no memory of the attempt: the next correct call works.
    let (result, _) = perform(&agent, "bb", "agent_info");
    assert!(matches!(result, AgentFrame::AgentInfo(_)));
}

#[test]
fn a_zero_length_frame_is_refused() {
    let agent = Agent::start("empty");
    let mut stream = agent.connect();
    stream.write_all(&[0, 0, 0, 0]).expect("header");
    expect_error(&mut stream, ErrorCode::EmptyFrame);
    assert_eq!(last(&agent.journal())["reason"], "empty_frame");
}

#[test]
fn an_oversized_frame_is_refused_without_reading_its_body() {
    let agent = Agent::start("oversized");
    for declared in [65_537_u32, 1 << 30, u32::MAX] {
        let mut stream = agent.connect();
        let started = Instant::now();
        // The header alone. Agent must answer without waiting for, or
        // allocating, the body it was promised.
        stream.write_all(&declared.to_be_bytes()).expect("header");
        expect_error(&mut stream, ErrorCode::FrameTooLarge);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "refused at once, not after waiting for the body"
        );
    }
    assert_eq!(last(&agent.journal())["reason"], "frame_too_large");
    assert!(perform_ok(&agent));
}

fn perform_ok(agent: &Agent) -> bool {
    matches!(
        perform(agent, "cc", "agent_info").0,
        AgentFrame::AgentInfo(_)
    )
}

#[test]
fn a_truncated_payload_is_journaled_and_answered_with_nothing() {
    let agent = Agent::start("truncated");
    let mut stream = agent.connect();
    let full = framed(&hello("ab"));
    stream.write_all(&full[..full.len() - 5]).expect("partial");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    assert!(read_frame(&mut stream).is_none());
    let lines = agent.wait_journal(1);
    assert_eq!(last(&lines)["reason"], "truncated_frame");
    assert!(last(&lines).get("request_id").is_none());
}

#[test]
fn unknown_fields_and_a_claimed_uid_are_malformed() {
    let agent = Agent::start("unknown");
    for body in [
        json!({"hello": {"protocol": 1, "core_version": "0.1.0", "request_id": "ab", "uid": 0}}),
        json!({"hello": {"protocol": 1, "core_version": "0.1.0", "request_id": "ab", "peer_uid": 999}}),
        json!({"hello": {"protocol": 1, "core_version": "0.1.0", "request_id": "ab"}, "token": "x"}),
    ] {
        let mut stream = agent.connect();
        stream.write_all(&framed(&body)).expect("frame");
        expect_error(&mut stream, ErrorCode::MalformedFrame);
    }
    let lines = agent.journal();
    assert!(lines.iter().all(|line| line["reason"] == "malformed_frame"));
    assert!(lines.iter().all(|line| line["peer"]["uid"] == euid()));
    // Unknown fields after the handshake too.
    let mut stream = agent.connect();
    stream.write_all(&framed(&hello("ab"))).expect("hello");
    expect_hello_ok(&mut stream);
    stream
        .write_all(&framed(
            &json!({"call": {"op": "runtime_probe", "socket": "/tmp/evil.sock"}}),
        ))
        .expect("call");
    expect_error(&mut stream, ErrorCode::MalformedFrame);
}

#[test]
fn malformed_json_and_non_utf8_are_refused() {
    let agent = Agent::start("malformed");
    for body in [
        b"{".as_slice(),
        b"not json",
        b"[]",
        b"null",
        b"\xff\xfe\xfd",
        b"{\"hello\":{\"protocol\":1,\"core_version\":\"0.1.0\",\"request_id\":\"ab\"}} trailing",
    ] {
        let mut stream = agent.connect();
        stream.write_all(&raw(body)).expect("frame");
        expect_error(&mut stream, ErrorCode::MalformedFrame);
    }
    assert!(perform_ok(&agent));
}

#[test]
fn an_unsupported_operation_is_refused_by_name() {
    let agent = Agent::start("unknown-op");
    for op in ["execute", "command", "shell", "runtime_probe_v2"] {
        let mut stream = agent.connect();
        stream.write_all(&framed(&hello("de"))).expect("hello");
        expect_hello_ok(&mut stream);
        stream.write_all(&framed(&call(op))).expect("call");
        expect_error(&mut stream, ErrorCode::UnknownOperation);
        let line = last(&agent.journal()).clone();
        assert_eq!(line["reason"], "unknown_operation");
        assert!(line.get("op").is_none(), "an unknown op is never journaled");
        assert_eq!(line["request_id"], "de");
    }
    assert!(!journal_text(&agent.state()).contains("execute"));
}

#[test]
fn an_invalid_request_id_never_reaches_the_journal() {
    let agent = Agent::start("request-id");
    let marker = "DEADBEEFMARKER";
    for id in [
        marker.to_owned(),
        format!("ab\n{marker}"),
        format!("{}{marker}", "a".repeat(64)),
        String::new(),
    ] {
        let mut stream = agent.connect();
        stream
            .write_all(&framed(
                &json!({"hello": {"protocol": 1, "core_version": "0.1.0", "request_id": id}}),
            ))
            .expect("hello");
        expect_error(&mut stream, ErrorCode::InvalidRequestId);
        let line = last(&agent.journal()).clone();
        assert_eq!(line["reason"], "invalid_request_id");
        assert!(line.get("request_id").is_none());
        assert!(
            line["seq"].as_u64().is_some(),
            "seq is Agent's own correlation"
        );
    }
    assert!(!journal_text(&agent.state()).contains("MARKER"));
}

#[test]
fn attacker_bytes_never_reach_the_journal() {
    let agent = Agent::start("bytes");
    let payloads: Vec<Vec<u8>> = vec![
        b"{\"hello\":\"ATTACKER-MARKER\x1b[31m\"}".to_vec(),
        json!({"ATTACKER-MARKER": 1}).to_string().into_bytes(),
        json!({"hello": {"protocol": 1, "core_version": "ATTACKER-MARKER", "request_id": "ab"}})
            .to_string()
            .into_bytes(),
        b"ATTACKER-MARKER\n{\"seq\":1}".to_vec(),
    ];
    for payload in payloads {
        let mut stream = agent.connect();
        stream.write_all(&raw(&payload)).expect("frame");
        let _ = read_frame(&mut stream);
    }
    let text = journal_text(&agent.state());
    assert!(!text.contains("ATTACKER"), "{text}");
    assert!(!text.contains('\u{1b}'));
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).expect("json");
        assert!(value["seq"].is_u64());
    }
}

#[test]
fn frames_out_of_order_are_refused() {
    let agent = Agent::start("order");
    let mut stream = agent.connect();
    stream
        .write_all(&framed(&call("agent_info")))
        .expect("call");
    expect_error(&mut stream, ErrorCode::UnexpectedFrame);

    let mut stream = agent.connect();
    stream.write_all(&framed(&hello("ab"))).expect("hello");
    expect_hello_ok(&mut stream);
    stream
        .write_all(&framed(&hello("ab")))
        .expect("hello again");
    expect_error(&mut stream, ErrorCode::UnexpectedFrame);
    assert_eq!(last(&agent.journal())["reason"], "unexpected_frame");
}

#[test]
fn a_connection_that_sends_nothing_is_closed_without_an_answer() {
    let agent = Agent::start("silent");
    let mut stream = agent.connect();
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    assert!(read_frame(&mut stream).is_none());
    assert_eq!(last(&agent.wait_journal(1))["reason"], "no_request");
}

#[test]
fn a_stalled_client_is_cut_off_at_the_deadline() {
    let agent = Agent::start("stall");
    let mut stream = agent.connect();
    stream.write_all(&framed(&hello("5a"))).expect("hello");
    expect_hello_ok(&mut stream);
    let started = Instant::now();
    // Say nothing more. Agent's deadline covers the whole exchange.
    assert!(read_frame(&mut stream).is_none());
    assert!(started.elapsed() < Duration::from_secs(10));
    let line = last(&agent.wait_journal(1)).clone();
    assert_eq!(line["reason"], "timed_out");
    assert_eq!(line["request_id"], "5a");
}

#[test]
fn rapid_invalid_connections_are_bounded_and_accounted_for() {
    let mut agent = Agent::start("flood");
    const ATTEMPTS: u64 = 200;
    let flood = Instant::now();
    for _ in 0..ATTEMPTS {
        if let Ok(mut stream) = UnixStream::connect(agent.socket()) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.write_all(&raw(b"garbage"));
            let mut sink = Vec::new();
            let _ = stream.read_to_end(&mut sink);
        }
    }
    let elapsed = flood.elapsed().as_secs_f64();
    assert!(agent.alive(), "Agent survives a flood");
    // Let the bucket refill, then make one good call; it also flushes the
    // summary of what the limiter dropped.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(perform_ok(&agent));

    let lines = agent.journal();
    let refused = lines
        .iter()
        .filter(|line| line["reason"] == "malformed_frame")
        .count() as u64;
    let suppressed: u64 = lines
        .iter()
        .filter(|line| line["reason"] == "rate_limited")
        .map(|line| line["suppressed"].as_u64().expect("count"))
        .sum();
    // The bucket admits its burst plus the refill over the flood's own
    // duration, whatever the speed of the machine running this.
    let bound = u64::from(atrium_agent::serve::RATE_BURST)
        + (elapsed * f64::from(atrium_agent::serve::RATE_PER_SECOND)).ceil() as u64
        + 1;
    assert!(
        refused <= bound,
        "journaling was bounded: {refused} > {bound} after {elapsed:.2}s"
    );
    if bound < ATTEMPTS {
        assert!(suppressed > 0, "the limiter engaged: {lines:#?}");
    }
    assert_eq!(
        refused + suppressed,
        ATTEMPTS,
        "every attempt is accounted for"
    );
    let seqs: Vec<u64> = lines
        .iter()
        .map(|l| l["seq"].as_u64().expect("seq"))
        .collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
}

#[test]
fn an_unwritable_journal_refuses_every_operation_until_repaired() {
    let agent = Agent::start("journal");
    let before = journal_text(&agent.state());
    chmod(&agent.state(), 0o755);
    let mut stream = agent.connect();
    stream.write_all(&framed(&hello("ab"))).expect("hello");
    expect_hello_ok(&mut stream);
    stream
        .write_all(&framed(&call("agent_info")))
        .expect("call");
    expect_error(&mut stream, ErrorCode::JournalUnavailable);
    assert_eq!(
        journal_text(&agent.state()),
        before,
        "nothing is written while the directory is unprotected"
    );

    chmod(&agent.state(), 0o700);
    let (result, _) = perform(&agent, "ab", "agent_info");
    let AgentFrame::AgentInfo(info) = result else {
        panic!("expected agent_info");
    };
    assert!(!info.journal.writable, "reports the state before this call");
    assert_eq!(last(&agent.journal())["outcome"], "ok");
}

#[test]
fn the_sequence_continues_across_an_agent_restart() {
    let mut agent = Agent::start("restart");
    perform(&agent, "01", "agent_info");
    perform(&agent, "02", "runtime_probe");
    agent.restart();
    let (result, _) = perform(&agent, "03", "agent_info");
    let AgentFrame::AgentInfo(info) = result else {
        panic!("expected agent_info");
    };
    assert_eq!(info.journal.last_seq, 2, "recovered before answering");
    let seqs: Vec<u64> = agent
        .journal()
        .iter()
        .map(|line| line["seq"].as_u64().expect("seq"))
        .collect();
    assert_eq!(seqs, [1, 2, 3]);
}
