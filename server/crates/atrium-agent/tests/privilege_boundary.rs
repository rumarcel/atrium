//! The Agent boundary against the real kernel: root Agent, real uids.
//!
//! `docs/M1-TEST-PLAN.md` section 5. Every test here needs root and the
//! `atrium` system user, so each is `#[ignore]` and CI runs this binary under
//! `sudo … --ignored --test-threads=1`. Agent runs as root, exactly as in
//! production, and receives its listening socket by real socket activation
//! (`systemd-socket-activate`). The socket is made `0660 root:atrium`, as
//! `atrium-agent.socket` makes it.
//!
//! Clients run as other users by re-executing this binary as that user with
//! [`CLIENT_ENV`] set. The user-side behaviour is [`client_helper`]; it prints
//! one `RESULT <key> <value>` line per observation, and the tests assert on
//! those.
//!
//! Criteria: 8 (other uids rejected, Core's accepted), 41 (the state
//! directory is root-only), 33's Agent half (the journal records every
//! connection, and Core cannot alter it).

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const CLIENT_ENV: &str = "ATRIUM_PRIV_CLIENT";
const SOCKET_ENV: &str = "ATRIUM_PRIV_SOCKET";
const STATE_ENV: &str = "ATRIUM_PRIV_STATE";
const SOCKET_ACTIVATE: &str = "/usr/bin/systemd-socket-activate";
const NOBODY: u32 = 65_534;
const TIMEOUT: Duration = Duration::from_secs(20);

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct User {
    uid: u32,
    gid: u32,
}

fn atrium() -> User {
    assert!(
        nix::unistd::geteuid().is_root(),
        "the privileged suite must run as root (sudo <test binary> --ignored)"
    );
    let user = nix::unistd::User::from_name("atrium")
        .expect("user lookup")
        .expect("an `atrium` system user must exist for the privileged suite");
    User {
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
    }
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn chown(path: &Path, uid: u32, gid: u32) {
    std::os::unix::fs::chown(path, Some(uid), Some(gid)).expect("chown");
}

/// A root Agent on a socket-activated `0660 root:atrium` socket, with a
/// `0700 root:root` state directory, under a private root.
struct RootAgent {
    root: PathBuf,
    atrium: User,
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
}

impl RootAgent {
    fn start(tag: &str) -> Self {
        let atrium = atrium();
        assert!(
            Path::new(SOCKET_ACTIVATE).exists(),
            "{SOCKET_ACTIVATE} is required to hand Agent a socket the way systemd does"
        );
        let root = PathBuf::from(format!(
            "/tmp/atrium-agentpriv-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["", "run", "run/atrium", "var", "var/lib", "bin"] {
            std::fs::create_dir_all(root.join(dir)).expect("dir");
            chmod(&root.join(dir), 0o755);
        }
        let state = root.join("var/lib/atrium-agent");
        std::fs::create_dir(&state).expect("state");
        chmod(&state, 0o700);

        // The binaries, somewhere every user can execute them from.
        let agent = root.join("bin/atrium-agent");
        std::fs::copy(env!("CARGO_BIN_EXE_atrium-agent"), &agent).expect("copy agent");
        chmod(&agent, 0o755);
        let client = root.join("bin/client");
        std::fs::copy(std::env::current_exe().expect("exe"), &client).expect("copy client");
        chmod(&client, 0o755);

        let socket = root.join("run/atrium/agent.sock");
        let mut child = Command::new(SOCKET_ACTIVATE)
            .arg("--listen")
            .arg(&socket)
            .arg(format!(
                "--setenv=ATRIUM_AGENT_STATE_DIR={}",
                state.display()
            ))
            .arg("--setenv=ATRIUM_AGENT_LOG=info")
            .arg(&agent)
            .env_clear()
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn systemd-socket-activate");
        let stderr = child.stderr.take().expect("stderr");
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().expect("lock").push(line);
            }
        });

        // The socket unit's declaration, applied by hand.
        let deadline = Instant::now() + TIMEOUT;
        while !socket.exists() {
            assert!(Instant::now() < deadline, "the socket never appeared");
            std::thread::sleep(Duration::from_millis(25));
        }
        chown(&socket, 0, atrium.gid);
        chmod(&socket, 0o660);

        Self {
            root,
            atrium,
            child,
            log,
        }
    }

    fn socket(&self) -> PathBuf {
        self.root.join("run/atrium/agent.sock")
    }

    fn state(&self) -> PathBuf {
        self.root.join("var/lib/atrium-agent")
    }

    fn journal(&self) -> Vec<Value> {
        journal_text(&self.state())
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
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
                "journal never reached {count} lines: {lines:#?}\nagent: {:#?}",
                self.log.lock().expect("lock")
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Runs [`client_helper`] as `uid:gid` and returns its observations.
    fn client(&self, uid: u32, gid: u32, scenario: &str) -> Vec<(String, String)> {
        let output = Command::new(self.root.join("bin/client"))
            .args([
                "client_helper",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env(CLIENT_ENV, scenario)
            .env(SOCKET_ENV, self.socket())
            .env(STATE_ENV, self.state())
            .current_dir(&self.root)
            .uid(uid)
            .gid(gid)
            .output()
            .expect("run client");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "client {scenario} as {uid}:{gid} failed: {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // libtest prints "test client_helper ... " without a line break
        // under --nocapture; find the marker anywhere in the line.
        stdout
            .lines()
            .filter_map(|line| {
                let (_, rest) = line.split_once("RESULT ")?;
                let (key, value) = rest.split_once(' ')?;
                Some((key.to_owned(), value.trim().to_owned()))
            })
            .collect()
    }
}

impl Drop for RootAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn journal_text(state: &Path) -> String {
    let dir = state.join("journal");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    files.sort();
    files
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("journal"))
        .collect()
}

fn get<'a>(results: &'a [(String, String)], key: &str) -> &'a str {
    results
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("no result {key}: {results:#?}"))
}

// ---------------------------------------------------------------------------
// The client side, executed as another user.

fn framed(value: &Value) -> Vec<u8> {
    let body = value.to_string();
    let mut bytes = u32::try_from(body.len())
        .expect("fits")
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

fn errno(error: &std::io::Error) -> String {
    error
        .raw_os_error()
        .map(|code| format!("errno={code}"))
        .unwrap_or_else(|| format!("error={}", error.kind()))
}

/// Reads one frame and names it: its top-level key, `eof`, or an errno.
fn read_kind(stream: &mut UnixStream) -> String {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return "closed".into(),
        // Agent closed without reading what was sent; the kernel discards the
        // unread bytes and resets the connection. Also "closed, no answer".
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {
            return "closed".into()
        }
        Err(error) => return errno(&error),
    }
    let mut body = vec![0_u8; u32::from_be_bytes(header).min(65_536) as usize];
    if stream.read_exact(&mut body).is_err() {
        return "truncated".into();
    }
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match value.as_object().and_then(|o| o.keys().next().cloned()) {
        Some(key) if key == "error" => {
            format!("error:{}", value["error"]["code"].as_str().unwrap_or("?"))
        }
        Some(key) => key,
        None => "unparseable".into(),
    }
}

fn exchange(socket: &Path, frames: &[Value]) {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) => {
            println!("RESULT connect {}", errno(&error));
            return;
        }
    };
    println!("RESULT connect ok");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    for (index, frame) in frames.iter().enumerate() {
        if stream.write_all(&framed(frame)).is_err() {
            println!("RESULT reply{index} write_failed");
            return;
        }
        let kind = read_kind(&mut stream);
        println!("RESULT reply{index} {kind}");
        if kind == "closed" || kind.starts_with("error") {
            return;
        }
    }
}

fn hello() -> Value {
    json!({"hello": {"protocol": 1, "core_version": "0.1.0", "request_id": "c0ffee"}})
}

/// Attempts, as the calling user, everything a compromised Core might try
/// against Agent's state.
fn attack_state(state: &Path) {
    let journal = state.join("journal");
    let report = |what: &str, result: std::io::Result<()>| {
        println!(
            "RESULT {what} {}",
            match result {
                Ok(()) => "ok".to_owned(),
                Err(error) => errno(&error),
            }
        );
    };
    report("list_state", std::fs::read_dir(state).map(|_| ()));
    report("list_journal", std::fs::read_dir(&journal).map(|_| ()));
    // The file names are not listable by this user; the test passes the one
    // it knows through the scenario, so every per-file attempt is real.
    let name = std::env::var(CLIENT_ENV)
        .ok()
        .and_then(|scenario| scenario.strip_prefix("attack:").map(str::to_owned))
        .expect("attack names a file");
    let file = journal.join(&name);
    report("read_file", std::fs::read(&file).map(|_| ()));
    report(
        "append_file",
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .map(|_| ()),
    );
    report(
        "truncate_file",
        std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&file)
            .map(|_| ()),
    );
    report("unlink_file", std::fs::remove_file(&file));
    report("rename_file", std::fs::rename(&file, journal.join("moved")));
    report(
        "chmod_file",
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)),
    );
    report(
        "create_in_journal",
        std::fs::write(journal.join("forged.jsonl"), b"{}"),
    );
    report(
        "create_in_state",
        std::fs::write(state.join("forged"), b"x"),
    );
    report(
        "chmod_state",
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o777)),
    );
}

/// Not a test of its own: executed as another user by the tests below.
#[test]
#[ignore = "helper executed as another user by the privileged tests"]
fn client_helper() {
    let Ok(scenario) = std::env::var(CLIENT_ENV) else {
        return;
    };
    let socket = PathBuf::from(std::env::var(SOCKET_ENV).expect("socket"));
    let state = PathBuf::from(std::env::var(STATE_ENV).expect("state"));
    match scenario.as_str() {
        "agent_info" => exchange(&socket, &[hello(), json!({"call": {"op": "agent_info"}})]),
        "runtime_probe" => exchange(
            &socket,
            &[hello(), json!({"call": {"op": "runtime_probe"}})],
        ),
        "claimed_uid" => exchange(
            &socket,
            &[json!({"hello": {"protocol": 1, "core_version": "0.1.0",
                               "request_id": "c0ffee", "uid": 996}})],
        ),
        "oversized" => {
            let mut stream = UnixStream::connect(&socket).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("timeout");
            stream.write_all(&u32::MAX.to_be_bytes()).expect("header");
            println!("RESULT reply0 {}", read_kind(&mut stream));
        }
        "unknown_op" => exchange(&socket, &[hello(), json!({"call": {"op": "execute"}})]),
        other if other.starts_with("attack:") => attack_state(&state),
        other => panic!("unknown scenario {other}"),
    }
}

// ---------------------------------------------------------------------------
// Criterion 8.

#[test]
#[ignore = "requires root and the atrium user"]
fn agent_accepts_core_uid() {
    let agent = RootAgent::start("accept");
    let atrium = (agent.atrium.uid, agent.atrium.gid);
    let results = agent.client(atrium.0, atrium.1, "agent_info");
    assert_eq!(get(&results, "connect"), "ok");
    assert_eq!(get(&results, "reply0"), "hello_ok");
    assert_eq!(get(&results, "reply1"), "agent_info");

    let results = agent.client(atrium.0, atrium.1, "runtime_probe");
    assert_eq!(get(&results, "reply1"), "runtime_probe");

    let lines = agent.wait_journal(2);
    for line in &lines {
        assert_eq!(line["accepted"], true);
        assert_eq!(line["peer"]["uid"], atrium.0);
        assert_eq!(line["request_id"], "c0ffee");
    }
    assert_eq!(lines[0]["op"], "agent_info");
    assert_eq!(lines[1]["op"], "runtime_probe");
}

#[test]
#[ignore = "requires root and the atrium user"]
fn agent_rejects_non_core_uid() {
    let agent = RootAgent::start("reject");
    let atrium_gid = agent.atrium.gid;

    // A member of the atrium group that is not the atrium user: the socket's
    // mode lets it connect, and SO_PEERCRED stops it. Nothing is read from
    // it and nothing is sent to it.
    let results = agent.client(NOBODY, atrium_gid, "agent_info");
    assert_eq!(get(&results, "connect"), "ok");
    assert_eq!(get(&results, "reply0"), "closed", "{results:?}");
    let lines = agent.wait_journal(1);
    let line = lines.last().expect("a line");
    assert_eq!(line["accepted"], false);
    assert_eq!(line["reason"], "peer_uid_rejected");
    assert_eq!(line["peer"]["uid"], NOBODY);
    assert_eq!(line["peer"]["gid"], atrium_gid);
    assert!(line.get("request_id").is_none(), "nothing was read from it");

    // Root is not Core either.
    let results = agent.client(0, 0, "agent_info");
    assert_eq!(get(&results, "reply0"), "closed");
    let lines = agent.wait_journal(2);
    assert_eq!(lines[1]["peer"]["uid"], 0);
    assert_eq!(lines[1]["reason"], "peer_uid_rejected");

    // Outside the group, the socket's mode refuses the connection itself.
    let results = agent.client(NOBODY, NOBODY, "agent_info");
    assert_eq!(get(&results, "connect"), "errno=13");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        agent.journal().len(),
        2,
        "a refused connect never reaches Agent"
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn a_claimed_uid_in_the_request_changes_nothing() {
    let agent = RootAgent::start("claim");
    let results = agent.client(NOBODY, agent.atrium.gid, "claimed_uid");
    assert_eq!(get(&results, "reply0"), "closed");
    let line = agent.wait_journal(1)[0].clone();
    // Refused on the kernel's word before the frame was read, so the reason
    // is the uid, not the unknown field.
    assert_eq!(line["reason"], "peer_uid_rejected");
    assert_eq!(line["peer"]["uid"], NOBODY);

    // From Core's own uid the same frame is simply malformed: a field
    // cannot claim an identity.
    let results = agent.client(agent.atrium.uid, agent.atrium.gid, "claimed_uid");
    assert_eq!(get(&results, "reply0"), "error:malformed_frame");
}

#[test]
#[ignore = "requires root and the atrium user"]
fn hostile_frames_from_the_core_uid_are_refused() {
    let agent = RootAgent::start("hostile");
    let (uid, gid) = (agent.atrium.uid, agent.atrium.gid);
    assert_eq!(
        get(&agent.client(uid, gid, "oversized"), "reply0"),
        "error:frame_too_large"
    );
    assert_eq!(
        get(&agent.client(uid, gid, "unknown_op"), "reply1"),
        "error:unknown_operation"
    );
    let reasons: Vec<String> = agent
        .wait_journal(2)
        .iter()
        .map(|line| line["reason"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(reasons, ["frame_too_large", "unknown_operation"]);
}

// ---------------------------------------------------------------------------
// Criterion 41, and Core's inability to touch Agent's evidence.

#[test]
#[ignore = "requires root and the atrium user"]
fn agent_state_dir_is_root_only() {
    let agent = RootAgent::start("state");
    agent.client(agent.atrium.uid, agent.atrium.gid, "agent_info");
    agent.wait_journal(1);

    let state = std::fs::symlink_metadata(agent.state()).expect("stat");
    assert_eq!(
        (state.uid(), state.gid(), state.mode() & 0o7777),
        (0, 0, 0o700)
    );
    let journal_dir = agent.state().join("journal");
    let meta = std::fs::symlink_metadata(&journal_dir).expect("stat");
    assert_eq!(
        (meta.uid(), meta.gid(), meta.mode() & 0o7777),
        (0, 0, 0o700)
    );
    let files: Vec<PathBuf> = std::fs::read_dir(&journal_dir)
        .expect("read")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(files.len(), 1);
    let file_meta = std::fs::symlink_metadata(&files[0]).expect("stat");
    assert_eq!(
        (file_meta.uid(), file_meta.gid(), file_meta.mode() & 0o7777),
        (0, 0, 0o600)
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_cannot_read_forge_or_erase_the_journal() {
    let agent = RootAgent::start("evidence");
    agent.client(agent.atrium.uid, agent.atrium.gid, "agent_info");
    agent.wait_journal(1);
    let journal_dir = agent.state().join("journal");
    let name = std::fs::read_dir(&journal_dir)
        .expect("read")
        .flatten()
        .next()
        .expect("a file")
        .file_name()
        .into_string()
        .expect("utf-8");
    let before = journal_text(&agent.state());

    let results = agent.client(
        agent.atrium.uid,
        agent.atrium.gid,
        &format!("attack:{name}"),
    );
    // The state directory is 0700 root:root, so every attempt stops at the
    // first directory with EACCES — except chmod, which needs ownership and
    // fails with EPERM.
    for key in [
        "list_state",
        "list_journal",
        "read_file",
        "append_file",
        "truncate_file",
        "unlink_file",
        "rename_file",
        "create_in_journal",
        "create_in_state",
    ] {
        assert_eq!(get(&results, key), "errno=13", "{key}: {results:?}");
    }
    assert_eq!(get(&results, "chmod_file"), "errno=13");
    assert_eq!(get(&results, "chmod_state"), "errno=1");

    assert_eq!(
        journal_text(&agent.state()),
        before,
        "the journal is unchanged"
    );
    let names: Vec<_> = std::fs::read_dir(&journal_dir)
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(names.len(), 1, "nothing was planted: {names:?}");
    assert!(!agent.state().join("forged").exists());
}
