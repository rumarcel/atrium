//! Privileged integration tests (`docs/M1-TEST-PLAN.md` section 5).
//!
//! These need root and an `atrium` system user, so they are `#[ignore]`d and
//! run explicitly — in CI by the server job's privileged step:
//!
//! ```text
//! sudo <this test binary> --ignored --test-threads=1
//! ```
//!
//! Each test builds a real installation in a root-owned temporary directory
//! the way `install.sh` will (plan section 12.2, step 8): the identity is
//! created by `atrium-core init-identity` running *as* `atrium` while the
//! identity directory is still `atrium`'s, then the directory and the three
//! files are handed to root. Everything after that is asserted against the
//! kernel, not against a description of it: `stat` for ownership and modes,
//! and real attempts — as the `atrium` user, in a separate process — to write,
//! truncate, unlink, rename over, chmod and create beside the identity files.
//!
//! The attempts run in a copy of this test binary executed as `atrium`
//! (`probe_helper` below), because changing the uid of the test harness itself
//! would change it for every thread in it.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};

const SERVER_NAME: &str = "priv-server";
const READY_TIMEOUT: Duration = Duration::from_secs(20);
const IDENTITY_FILES: [&str; 3] = ["tls.key", "identity.json", "secrets.key"];

static COUNTER: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Fixture

#[derive(Clone, Copy)]
struct Atrium {
    uid: u32,
    gid: u32,
}

fn atrium() -> Atrium {
    assert!(
        nix::unistd::geteuid().is_root(),
        "the privileged suite must run as root (sudo <test binary> --ignored)"
    );
    let user = nix::unistd::User::from_name("atrium")
        .expect("user lookup")
        .expect("an `atrium` system user must exist for the privileged suite");
    Atrium {
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

/// A locked-down installation in a root-owned temporary directory.
struct Installation {
    root: PathBuf,
    atrium: Atrium,
    core: PathBuf,
    ctl: PathBuf,
    /// Core's listening port, on 127.0.0.1.
    port: u16,
}

impl Installation {
    fn new(tag: &str) -> Self {
        let atrium = atrium();
        let root = PathBuf::from(format!(
            "/tmp/atrium-priv-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        chmod(&root, 0o755);
        for parent in ["etc", "var", "var/lib", "bin"] {
            std::fs::create_dir_all(root.join(parent)).expect("parent");
            chmod(&root.join(parent), 0o755);
        }

        // The binaries, somewhere the atrium user can execute them from: the
        // build tree may sit under a home directory it cannot traverse.
        let ctl_source = PathBuf::from(env!("CARGO_BIN_EXE_atriumctl"));
        let core_source = ctl_source.with_file_name("atrium-core");
        assert!(
            core_source.exists(),
            "{} must be built first (cargo build --workspace)",
            core_source.display()
        );
        let core = root.join("bin/atrium-core");
        let ctl = root.join("bin/atriumctl");
        std::fs::copy(&core_source, &core).expect("copy atrium-core");
        std::fs::copy(&ctl_source, &ctl).expect("copy atriumctl");
        chmod(&core, 0o755);
        chmod(&ctl, 0o755);

        // A free loopback port per installation, so no test ever binds the
        // production port or another test's.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("a free port")
            .port();
        let installation = Self {
            root,
            atrium,
            core,
            ctl,
            port,
        };

        // install.sh step 4 and 7: directories, and core.toml as root:atrium 0640.
        let etc = installation.etc();
        let state = installation.state();
        std::fs::create_dir_all(&etc).expect("etc");
        std::fs::create_dir_all(&state).expect("state");
        chown(&state, atrium.uid, atrium.gid);
        chmod(&state, 0o700);
        let config = etc.join("core.toml");
        let port = installation.port;
        std::fs::write(
            &config,
            format!("server_name = \"{SERVER_NAME}\"\nlisten = \"127.0.0.1:{port}\"\n"),
        )
        .expect("config");
        chown(&config, 0, atrium.gid);
        chmod(&config, 0o640);

        // Step 8: the identity directory is atrium's while init-identity runs.
        chown(&etc, atrium.uid, atrium.gid);
        chmod(&etc, 0o700);
        let output = installation.as_atrium(&installation.core, &["init-identity"], "");
        assert!(
            output.status.success(),
            "init-identity as atrium: {}",
            text(&output)
        );
        installation.lock_down();
        installation
    }

    /// Step 8, second half: hand the directory and the files to root.
    fn lock_down(&self) {
        let etc = self.etc();
        for name in IDENTITY_FILES {
            chown(&etc.join(name), 0, self.atrium.gid);
            chmod(&etc.join(name), 0o640);
        }
        chown(&etc, 0, self.atrium.gid);
        chmod(&etc, 0o750);
    }

    fn etc(&self) -> PathBuf {
        self.root.join("etc/atrium")
    }

    fn state(&self) -> PathBuf {
        self.root.join("var/lib/atrium")
    }

    fn command(&self, program: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .env("ATRIUM_ROOT", &self.root)
            .env("ATRIUM_CORE_LOG", "info")
            .current_dir(&self.root);
        command
    }

    fn run(mut command: Command, stdin: &str) -> Output {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        // A command that refuses before reading stdin may already have
        // exited; the broken pipe is not the test's concern, the exit is.
        let _ = child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin.as_bytes());
        child.wait_with_output().expect("wait")
    }

    /// Runs a program as the `atrium` user, with no supplementary groups.
    fn as_atrium(&self, program: &Path, args: &[&str], stdin: &str) -> Output {
        let mut command = self.command(program, args);
        command.uid(self.atrium.uid).gid(self.atrium.gid);
        Self::run(command, stdin)
    }

    /// Runs a program as root.
    fn as_root(&self, program: &Path, args: &[&str], stdin: &str) -> Output {
        Self::run(self.command(program, args), stdin)
    }

    /// Starts Core as `atrium`.
    fn start_core(&self) -> Core {
        self.start_core_with_log("info")
    }

    /// Starts Core as `atrium` with a given log level.
    fn start_core_with_log(&self, level: &str) -> Core {
        let mut command = self.command(&self.core, &[]);
        command.env("ATRIUM_CORE_LOG", level);
        command
            .uid(self.atrium.uid)
            .gid(self.atrium.gid)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn atrium-core");
        let stderr = child.stderr.take().expect("stderr");
        let capture = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&capture);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink.lock().expect("lock").push(line);
            }
        });
        Core { child, capture }
    }

    fn read(&self, path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Opens the state database as root without leaving root-owned `-wal` or
    /// `-shm` files behind, by giving back ownership of anything it creates.
    fn with_database<T>(&self, work: impl FnOnce(&Connection) -> T) -> T {
        let path = self.state().join("atrium.db");
        let result = {
            let connection = Connection::open(&path).expect("open");
            work(&connection)
        };
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let companion = PathBuf::from(format!("{}{suffix}", path.display()));
            if companion.exists() {
                chown(&companion, self.atrium.uid, self.atrium.gid);
            }
        }
        result
    }

    /// Reads the state database without writing anything next to it.
    fn query_count(&self, sql: &str) -> i64 {
        let uri = format!(
            "file:{}?immutable=1",
            self.state().join("atrium.db").display()
        );
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("open read-only");
        connection
            .query_row(sql, (), |row| row.get(0))
            .expect("query")
    }
}

/// A root Agent for an installation, started the way systemd starts it: by
/// socket activation on `<root>/run/atrium/agent.sock`, `0660 root:atrium`,
/// with a `0700 root:root` state directory.
struct RootAgent {
    child: Child,
    state: PathBuf,
}

impl RootAgent {
    fn journal(&self) -> Vec<serde_json::Value> {
        let dir = self.state.join("journal");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        files.sort();
        files
            .iter()
            .flat_map(|path| {
                std::fs::read_to_string(path)
                    .expect("journal")
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("json"))
                    .collect::<Vec<serde_json::Value>>()
            })
            .collect()
    }
}

impl Drop for RootAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Installation {
    fn start_agent(&self) -> RootAgent {
        const SOCKET_ACTIVATE: &str = "/usr/bin/systemd-socket-activate";
        assert!(
            Path::new(SOCKET_ACTIVATE).exists(),
            "{SOCKET_ACTIVATE} is required to hand Agent a socket the way systemd does"
        );
        let source = PathBuf::from(env!("CARGO_BIN_EXE_atriumctl")).with_file_name("atrium-agent");
        assert!(
            source.exists(),
            "{} must be built first (cargo build --workspace)",
            source.display()
        );
        let agent = self.root.join("bin/atrium-agent");
        std::fs::copy(&source, &agent).expect("copy atrium-agent");
        chmod(&agent, 0o755);
        for dir in ["run", "run/atrium"] {
            std::fs::create_dir_all(self.root.join(dir)).expect("run");
            chmod(&self.root.join(dir), 0o755);
        }
        let state = self.root.join("var/lib/atrium-agent");
        std::fs::create_dir_all(&state).expect("agent state");
        chmod(&state, 0o700);
        let socket = self.root.join("run/atrium/agent.sock");
        let child = Command::new(SOCKET_ACTIVATE)
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
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn systemd-socket-activate");
        let deadline = Instant::now() + READY_TIMEOUT;
        while !socket.exists() {
            assert!(Instant::now() < deadline, "the agent socket never appeared");
            std::thread::sleep(Duration::from_millis(25));
        }
        chown(&socket, 0, self.atrium.gid);
        chmod(&socket, 0o660);
        RootAgent { child, state }
    }

    /// `(request_id, target, outcome, error_code)` of every `agent.call` row,
    /// read without writing anything next to the database.
    fn agent_calls(&self) -> Vec<(String, String, String, Option<String>)> {
        let uri = format!(
            "file:{}?immutable=1",
            self.state().join("atrium.db").display()
        );
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("open read-only");
        let mut statement = connection
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

impl Drop for Installation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Core {
    child: Child,
    capture: Arc<Mutex<Vec<String>>>,
}

impl Core {
    fn lines(&self) -> Vec<String> {
        self.capture.lock().expect("lock").clone()
    }

    fn wait_for(&self, needle: &str) -> String {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let lines = self.lines();
            if let Some(line) = lines.iter().find(|l| l.contains(needle)) {
                return line.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no line with {needle}; child said: {lines:#?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn ready(&self) -> String {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let lines = self.lines();
            if let Some(line) = lines.iter().find(|l| l.contains("\"event\":\"ready\"")) {
                return line.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no ready line; child said: {lines:#?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn assert_running(&mut self) {
        assert!(
            self.child.try_wait().expect("wait").is_none(),
            "Core exited; it said: {:#?}",
            self.lines()
        );
    }

    fn stop(&mut self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).expect("pid"));
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("SIGTERM");
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait") {
                std::thread::sleep(Duration::from_millis(200));
                assert!(
                    status.success(),
                    "unclean exit {status:?}: {:#?}",
                    self.lines()
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Core did not stop: {:#?}",
                self.lines()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn text(output: &Output) -> String {
    format!(
        "status {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn field(line: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let start = line.find(&key)? + key.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_owned())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// The probe: runs as `atrium`, attempts each operation, reports the errno.

const PROBE_ENV: &str = "ATRIUM_PROBE_ROOT";

/// Not a test of its own: the privileged tests execute a copy of this binary
/// as `atrium` with `ATRIUM_PROBE_ROOT` set, and this function performs the
/// attempts and prints one `PROBE <operation> <target> <result>` line each.
#[test]
#[ignore = "helper executed as the atrium user by the privileged tests"]
fn probe_helper() {
    let Some(root) = std::env::var_os(PROBE_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let etc = root.join("etc/atrium");
    let state = root.join("var/lib/atrium");

    let report = |operation: &str, target: &str, result: std::io::Result<()>| {
        let outcome = match result {
            Ok(()) => "ok".to_owned(),
            Err(error) => match error.raw_os_error() {
                Some(code) => format!("errno={code}"),
                None => format!("error={error}"),
            },
        };
        println!("PROBE {operation} {target} {outcome}");
    };

    for name in ["tls.key", "identity.json", "secrets.key", "core.toml"] {
        let path = etc.join(name);
        report("read", name, std::fs::read(&path).map(|_| ()));
        report(
            "open_write",
            name,
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .map(|_| ()),
        );
        report(
            "open_truncate",
            name,
            std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .map(|_| ()),
        );
        report("unlink", name, std::fs::remove_file(&path));
        let replacement = state.join(format!("replacement-{name}"));
        let _ = std::fs::write(&replacement, b"attacker");
        report("rename_over", name, std::fs::rename(&replacement, &path));
        let _ = std::fs::remove_file(&replacement);
        report(
            "rename_away",
            name,
            std::fs::rename(&path, etc.join(format!("{name}.moved"))),
        );
        report(
            "chmod",
            name,
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)),
        );
        report(
            "chown",
            name,
            std::os::unix::fs::chown(&path, Some(nix::unistd::geteuid().as_raw()), None),
        );
    }

    report(
        "create",
        "tls.key.new",
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(etc.join("tls.key.new"))
            .map(|_| ()),
    );
    report(
        "create",
        ".atrium-tmp-x",
        std::fs::write(etc.join(".atrium-tmp-x"), b"x"),
    );
    report("mkdir", "sub", std::fs::create_dir(etc.join("sub")));
    report(
        "symlink",
        "tls.key.link",
        std::os::unix::fs::symlink(state.join("x"), etc.join("tls.key.link")),
    );
    let source = state.join("hardlink-source");
    let _ = std::fs::write(&source, b"x");
    report(
        "hardlink",
        "tls.key.hard",
        std::fs::hard_link(&source, etc.join("tls.key.hard")),
    );
    let _ = std::fs::remove_file(&source);
    report(
        "chmod_dir",
        "etc",
        std::fs::set_permissions(&etc, std::fs::Permissions::from_mode(0o770)),
    );
}

/// Runs the probe as `atrium` and returns `(operation, target) -> outcome`.
fn probe(installation: &Installation) -> Vec<(String, String, String)> {
    let source = std::env::current_exe().expect("test binary");
    let copy = installation.root.join("bin/probe");
    std::fs::copy(&source, &copy).expect("copy probe");
    chmod(&copy, 0o755);
    let mut command = Command::new(&copy);
    command
        .args([
            "--exact",
            "probe_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(PROBE_ENV, &installation.root)
        .current_dir(&installation.root)
        .uid(installation.atrium.uid)
        .gid(installation.atrium.gid);
    let output = command.output().expect("run probe");
    assert!(output.status.success(), "probe failed: {}", text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    // With --nocapture libtest writes `test probe_helper ... ` without a line
    // break, so the first report shares its line; find the marker anywhere.
    let results: Vec<(String, String, String)> = stdout
        .lines()
        .filter_map(|line| {
            let (_, report) = line.split_once("PROBE ")?;
            let mut parts = report.splitn(3, ' ');
            Some((
                parts.next()?.to_owned(),
                parts.next()?.to_owned(),
                parts.next()?.to_owned(),
            ))
        })
        .collect();
    assert!(
        !results.is_empty(),
        "the probe reported nothing: {}",
        text(&output)
    );
    results
}

fn outcome(results: &[(String, String, String)], operation: &str, target: &str) -> String {
    results
        .iter()
        .find(|(o, t, _)| o == operation && t == target)
        .map(|(_, _, r)| r.clone())
        .unwrap_or_else(|| panic!("no probe result for {operation} {target}: {results:#?}"))
}

const EACCES: &str = "errno=13";
const EPERM: &str = "errno=1";

// ---------------------------------------------------------------------------
// Criterion 6: stat, and attempted writes as atrium.

#[test]
#[ignore = "requires root and the atrium user"]
fn file_modes_and_owners() {
    let installation = Installation::new("modes");
    let gid = installation.atrium.gid;
    let uid = installation.atrium.uid;
    let expect = |path: PathBuf, owner: u32, group: u32, mode: u32| {
        let metadata = std::fs::symlink_metadata(&path).expect("stat");
        assert!(!metadata.file_type().is_symlink(), "{}", path.display());
        assert_eq!(
            (metadata.uid(), metadata.gid(), metadata.mode() & 0o7777),
            (owner, group, mode),
            "{}: (uid, gid, mode)",
            path.display()
        );
    };
    let etc = installation.etc();
    let state = installation.state();
    expect(etc.clone(), 0, gid, 0o750);
    for name in IDENTITY_FILES {
        expect(etc.join(name), 0, gid, 0o640);
    }
    expect(etc.join("core.toml"), 0, gid, 0o640);
    expect(state.clone(), uid, gid, 0o700);
    expect(state.join("atrium.db"), uid, gid, 0o600);
    expect(state.join("tls.crt"), uid, gid, 0o644);
    expect(state.join("identity-state.json"), uid, gid, 0o600);

    let secrets = installation.read(&etc.join("secrets.key"));
    assert_eq!(secrets.len(), 32);
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_can_read_the_identity_key() {
    let installation = Installation::new("read");
    let results = probe(&installation);
    for name in ["tls.key", "identity.json", "secrets.key", "core.toml"] {
        assert_eq!(
            outcome(&results, "read", name),
            "ok",
            "atrium must be able to read {name}"
        );
    }
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_cannot_modify_the_identity_key() {
    let installation = Installation::new("modify");
    let before: Vec<Vec<u8>> = IDENTITY_FILES
        .iter()
        .map(|name| installation.read(&installation.etc().join(name)))
        .collect();
    let results = probe(&installation);

    // Every attempt, each asserted separately with its errno.
    for name in IDENTITY_FILES {
        assert_eq!(
            outcome(&results, "open_write", name),
            EACCES,
            "write {name}"
        );
        assert_eq!(
            outcome(&results, "open_truncate", name),
            EACCES,
            "truncate {name}"
        );
        assert_eq!(outcome(&results, "unlink", name), EACCES, "unlink {name}");
        assert_eq!(
            outcome(&results, "rename_over", name),
            EACCES,
            "rename over {name}"
        );
        assert_eq!(
            outcome(&results, "rename_away", name),
            EACCES,
            "rename {name} away"
        );
        assert_eq!(outcome(&results, "chmod", name), EPERM, "chmod {name}");
        assert_eq!(outcome(&results, "chown", name), EPERM, "chown {name}");
    }

    let after: Vec<Vec<u8>> = IDENTITY_FILES
        .iter()
        .map(|name| installation.read(&installation.etc().join(name)))
        .collect();
    assert_eq!(
        before, after,
        "the identity must be byte-identical afterwards"
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_cannot_create_in_the_identity_directory() {
    let installation = Installation::new("create");
    let results = probe(&installation);
    assert_eq!(outcome(&results, "create", "tls.key.new"), EACCES);
    assert_eq!(outcome(&results, "create", ".atrium-tmp-x"), EACCES);
    assert_eq!(outcome(&results, "mkdir", "sub"), EACCES);
    assert_eq!(outcome(&results, "symlink", "tls.key.link"), EACCES);
    assert_eq!(outcome(&results, "hardlink", "tls.key.hard"), EACCES);
    assert_eq!(outcome(&results, "chmod_dir", "etc"), EPERM);

    let mut names: Vec<String> = std::fs::read_dir(installation.etc())
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["core.toml", "identity.json", "secrets.key", "tls.key"]
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_cannot_replace_core_toml() {
    let installation = Installation::new("config");
    let before = installation.read(&installation.etc().join("core.toml"));
    let results = probe(&installation);
    for operation in [
        "open_write",
        "open_truncate",
        "unlink",
        "rename_over",
        "rename_away",
    ] {
        assert_eq!(
            outcome(&results, operation, "core.toml"),
            EACCES,
            "{operation}"
        );
    }
    assert_eq!(outcome(&results, "chmod", "core.toml"), EPERM);
    assert_eq!(
        installation.read(&installation.etc().join("core.toml")),
        before
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn init_identity_is_idempotent_and_refuses_to_overwrite() {
    let installation = Installation::new("reinit");
    let key = installation.read(&installation.etc().join("tls.key"));
    let record = installation.read(&installation.etc().join("identity.json"));

    let output = installation.as_atrium(&installation.core, &["init-identity"], "");
    assert_eq!(output.status.code(), Some(3), "{}", text(&output));

    // And as root, which it refuses outright.
    let output = installation.as_root(&installation.core, &["init-identity"], "");
    assert!(!output.status.success(), "{}", text(&output));

    assert_eq!(installation.read(&installation.etc().join("tls.key")), key);
    assert_eq!(
        installation.read(&installation.etc().join("identity.json")),
        record
    );
}

// ---------------------------------------------------------------------------
// Normal mode, criterion 7, and secrecy of the logs.

#[test]
#[ignore = "requires root and the atrium user"]
fn core_runs_normally_as_atrium_and_the_pin_survives_restart() {
    let installation = Installation::new("normal");

    let mut first = installation.start_core();
    let ready = first.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    let pin = field(&ready, "spki_sha256").expect("pin");
    let server_id = field(&ready, "server_id").expect("server_id");
    let serial = field(&ready, "certificate_serial").expect("serial");
    first.stop();

    let mut second = installation.start_core();
    let ready = second.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    assert_eq!(
        field(&ready, "spki_sha256"),
        Some(pin),
        "the pin must survive a restart"
    );
    assert_eq!(field(&ready, "server_id"), Some(server_id));
    assert_eq!(
        field(&ready, "certificate_serial"),
        Some(serial),
        "nothing changed, so nothing was reissued"
    );
    second.stop();

    // Everything Core wrote belongs to atrium.
    for entry in walk(&installation.state()) {
        let metadata = std::fs::symlink_metadata(&entry).expect("stat");
        assert_eq!(
            metadata.uid(),
            installation.atrium.uid,
            "{}",
            entry.display()
        );
    }
}

#[test]
#[ignore = "requires root and the atrium user"]
fn logs_never_contain_private_key_material() {
    let installation = Installation::new("secrecy");
    let mut core = installation.start_core();
    core.ready();
    core.stop();
    let logs = core.lines().join("\n");

    for name in ["tls.key", "secrets.key"] {
        let material = hex(&installation.read(&installation.etc().join(name)));
        for start in (0..=material.len() - 32).step_by(2) {
            assert!(
                !logs.contains(&material[start..start + 32]),
                "{name} material appears in the log"
            );
        }
    }
    assert!(!logs.contains("PRIVATE KEY"));
}

/// A named way to misconfigure an installation.
type Breakage = (&'static str, fn(&Installation));

#[test]
#[ignore = "requires root and the atrium user"]
fn core_refuses_an_identity_that_is_not_protected() {
    // Each misconfiguration on its own, each into recovery, none rewritten.
    let cases: [Breakage; 4] = [
        ("world-readable key", |i| {
            chmod(&i.etc().join("tls.key"), 0o644)
        }),
        ("group-writable key", |i| {
            chmod(&i.etc().join("tls.key"), 0o660)
        }),
        ("group-writable directory", |i| chmod(&i.etc(), 0o770)),
        ("key owned by atrium", |i| {
            chown(&i.etc().join("tls.key"), i.atrium.uid, i.atrium.gid);
        }),
    ];
    for (what, break_it) in cases {
        let installation = Installation::new("unprotected");
        break_it(&installation);
        let key = installation.read(&installation.etc().join("tls.key"));
        let mut core = installation.start_core();
        let ready = core.ready();
        assert_eq!(
            field(&ready, "mode").as_deref(),
            Some("recovery"),
            "{what}: {ready}"
        );
        assert_eq!(
            field(&ready, "reason").as_deref(),
            Some("identity.unprotected"),
            "{what}: {ready}"
        );
        core.stop();
        assert_eq!(
            installation.read(&installation.etc().join("tls.key")),
            key,
            "{what}"
        );
    }
}

#[test]
#[ignore = "requires root and the atrium user"]
fn an_unreadable_key_is_reported_not_regenerated() {
    let installation = Installation::new("unreadable");
    chown(&installation.etc().join("tls.key"), 0, 0);
    chmod(&installation.etc().join("tls.key"), 0o600);
    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(
        field(&ready, "reason").as_deref(),
        Some("identity.unreadable"),
        "{ready}"
    );
    core.stop();
}

// ---------------------------------------------------------------------------
// Criterion 30: corruption, recovery, console restore.

/// Replaces the database with a version-0 one holding a marker, and lets Core
/// migrate it — which is what leaves a backup behind.
fn make_backup(installation: &Installation) {
    let path = installation.state().join("atrium.db");
    std::fs::remove_file(&path).expect("remove");
    {
        let connection = Connection::open(&path).expect("create");
        connection
            .execute_batch("CREATE TABLE marker (v TEXT); INSERT INTO marker VALUES ('before');")
            .expect("seed");
    }
    chown(&path, installation.atrium.uid, installation.atrium.gid);
    chmod(&path, 0o600);

    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    core.stop();
    assert!(
        core.lines()
            .iter()
            .any(|l| l.contains("\"event\":\"state_migrated\"")),
        "{:#?}",
        core.lines()
    );
}

fn backup_name(installation: &Installation) -> String {
    std::fs::read_dir(installation.state().join("backups"))
        .expect("backups")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with("atrium.db.pre-"))
        .expect("a backup")
}

#[test]
#[ignore = "requires root and the atrium user"]
fn corrupt_db_enters_recovery_not_crash_loop() {
    let installation = Installation::new("corrupt");
    let path = installation.state().join("atrium.db");
    let mut bytes = installation.read(&path);
    bytes.truncate(4096 + 100);
    std::fs::write(&path, &bytes).expect("damage mid-page");
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(
        field(&ready, "mode").as_deref(),
        Some("recovery"),
        "{ready}"
    );
    assert_eq!(
        field(&ready, "reason").as_deref(),
        Some("state.database_unreadable"),
        "{ready}"
    );
    std::thread::sleep(Duration::from_secs(2));
    core.assert_running();
    core.stop();

    assert_eq!(
        installation.read(&path),
        bytes,
        "not deleted, truncated or repaired"
    );
    assert!(
        core.lines()
            .iter()
            .any(|l| l.contains("\"event\":\"state_database_unreadable\"")),
        "the SQLite message must be logged"
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn restore_as_root_leaves_nothing_root_owned_and_service_resumes() {
    let installation = Installation::new("restore");
    make_backup(&installation);
    let name = backup_name(&installation);
    std::fs::write(installation.state().join("atrium.db"), vec![0x5a_u8; 8192]).expect("corrupt");

    let output = installation.as_root(
        &installation.ctl,
        &["restore", "--from", &name],
        &format!("{SERVER_NAME}\n"),
    );
    assert!(output.status.success(), "{}", text(&output));

    for entry in walk(&installation.state()) {
        let metadata = std::fs::symlink_metadata(&entry).expect("stat");
        assert_eq!(
            metadata.uid(),
            installation.atrium.uid,
            "{} is not owned by atrium",
            entry.display()
        );
    }

    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    core.stop();
    assert_eq!(
        installation.query_count("SELECT count(*) FROM marker WHERE v = 'before'"),
        1
    );
}

#[test]
#[ignore = "requires root and the atrium user"]
fn restore_refuses_while_core_is_running() {
    let installation = Installation::new("restorebusy");
    make_backup(&installation);
    let name = backup_name(&installation);
    let mut core = installation.start_core();
    core.ready();
    let output = installation.as_root(
        &installation.ctl,
        &["restore", "--from", &name],
        &format!("{SERVER_NAME}\n"),
    );
    assert!(!output.status.success(), "{}", text(&output));
    core.assert_running();
    core.stop();
}

// ---------------------------------------------------------------------------
// Criterion 19 (partial): deliberate rotation.

#[test]
#[ignore = "requires root and the atrium user"]
fn rotation_as_root_changes_spki_keeps_server_id_and_revokes_devices() {
    let installation = Installation::new("rotate");
    let record_before = installation.read(&installation.etc().join("identity.json"));
    installation.with_database(|connection| {
        connection
            .execute(
                "INSERT INTO devices (device_id, name, platform, role, token_digest, created_at) \
                 VALUES ('00112233445566778899aabbccddeeff', 'laptop', 'linux', 'owner', \
                 randomblob(32), \
                 '2026-09-24T00:00:00Z')",
                (),
            )
            .expect("device");
    });

    let mut core = installation.start_core();
    let before = core.ready();
    core.stop();

    let output = installation.as_root(
        &installation.ctl,
        &["rotate-identity"],
        &format!("{SERVER_NAME}\n"),
    );
    assert!(output.status.success(), "{}", text(&output));
    // Plan §5.5 step 3: a fresh pairing code, armed and shown.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Pairing code:"), "{}", text(&output));
    assert_eq!(
        installation.query_count("SELECT armed FROM pairing_state"),
        1
    );

    // The new key is installed exactly as the installer would have left it.
    let key = std::fs::metadata(installation.etc().join("tls.key")).expect("stat");
    assert_eq!(
        (key.uid(), key.gid(), key.mode() & 0o7777),
        (0, installation.atrium.gid, 0o640)
    );
    let record = std::fs::metadata(installation.etc().join("identity.json")).expect("stat");
    assert_eq!(
        (record.uid(), record.gid(), record.mode() & 0o7777),
        (0, installation.atrium.gid, 0o640)
    );
    assert_ne!(
        installation.read(&installation.etc().join("identity.json")),
        record_before
    );

    let mut core = installation.start_core();
    let after = core.ready();
    core.stop();
    assert_eq!(field(&after, "mode").as_deref(), Some("normal"), "{after}");
    assert_eq!(field(&after, "server_id"), field(&before, "server_id"));
    assert_ne!(field(&after, "spki_sha256"), field(&before, "spki_sha256"));
    assert_eq!(installation.query_count("SELECT count(*) FROM devices"), 0);
    assert_eq!(
        installation
            .query_count("SELECT count(*) FROM audit WHERE action = 'identity.key_rotated'"),
        1
    );

    for entry in walk(&installation.state()) {
        let metadata = std::fs::symlink_metadata(&entry).expect("stat");
        assert_eq!(
            metadata.uid(),
            installation.atrium.uid,
            "{}",
            entry.display()
        );
    }
}

/// Every path under `dir`, directories included.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        found.push(path.clone());
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            found.extend(walk(&path));
        }
    }
    found
}

// ---------------------------------------------------------------------------
// M1C: Core and Agent, both real, both with their production identities.

#[test]
#[ignore = "requires root and the atrium user"]
fn agent_journal_and_core_audit_correlate() {
    // Criterion 33: every call appears in Core's audit log and, independently,
    // in Agent's journal, and the two agree on the request id.
    let installation = Installation::new("correlate");
    let agent = installation.start_agent();
    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    let status = core.wait_for("\"event\":\"agent_status\"");
    assert!(status.contains("\"privileged_available\":true"), "{status}");
    assert!(status.contains("\"container_via\":\"agent\""), "{status}");
    core.stop();

    let calls = installation.agent_calls();
    assert_eq!(calls.len(), 2, "AgentInfo and RuntimeProbe: {calls:?}");
    let journal = agent.journal();
    assert_eq!(journal.len(), calls.len(), "{journal:#?}");
    for (request_id, target, outcome, code) in &calls {
        assert_eq!(outcome, "ok");
        assert_eq!(code, &None);
        let line = journal
            .iter()
            .find(|line| line["request_id"] == request_id.as_str())
            .unwrap_or_else(|| panic!("{request_id} missing from the journal: {journal:#?}"));
        assert_eq!(line["op"], target.as_str());
        assert_eq!(line["accepted"], true);
        assert_eq!(
            line["peer"]["uid"], installation.atrium.uid,
            "the kernel's word"
        );
    }
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_without_agent_reports_unreachable_and_keeps_running() {
    // Criteria 29 and 40, Core's half: no Agent, so both capabilities are
    // unavailable with agent_unreachable; Core stays up in normal mode and
    // tries nothing else.
    let installation = Installation::new("noagent");
    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    let status = core.wait_for("\"event\":\"agent_status\"");
    assert!(
        status.contains("\"privileged_available\":false"),
        "{status}"
    );
    assert_eq!(
        field(&status, "privileged_reason").as_deref(),
        Some("agent_unreachable")
    );
    assert_eq!(
        field(&status, "container_reason").as_deref(),
        Some("agent_unreachable")
    );
    std::thread::sleep(Duration::from_millis(1000));
    core.assert_running();
    core.stop();

    let calls = installation.agent_calls();
    assert_eq!(calls.len(), 1, "one attempt, no retry loop: {calls:?}");
    assert_eq!(calls[0].1, "agent_info");
    assert_eq!(calls[0].2, "failed");
    assert_eq!(calls[0].3.as_deref(), Some("socket_missing"));
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_stops_promptly_while_agent_stalls() {
    // An Agent socket that accepts and never answers holds Core's call for
    // its full timeout. A stop request must not wait behind it.
    let installation = Installation::new("stall");
    for dir in ["run", "run/atrium"] {
        std::fs::create_dir_all(installation.root.join(dir)).expect("run");
        chmod(&installation.root.join(dir), 0o755);
    }
    let socket = installation.root.join("run/atrium/agent.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    chown(&socket, 0, installation.atrium.gid);
    chmod(&socket, 0o660);
    let held = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&held);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            keep.lock().expect("lock").push(stream);
        }
    });

    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    let deadline = Instant::now() + READY_TIMEOUT;
    while held.lock().expect("lock").is_empty() {
        assert!(
            Instant::now() < deadline,
            "Core never called the stalled Agent"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    let started = Instant::now();
    core.stop();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "Core took {:?} to stop behind a stalled Agent call",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// M1D: the network boundary, with the real Core binary as `atrium`.

mod https {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::time::Duration;

    use atrium_core::identity::SpkiPin;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

    /// Accepts exactly one SPKI, as an Atrium client with a pin does.
    #[derive(Debug)]
    struct Pin(SpkiPin);

    impl ServerCertVerifier for Pin {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            let (_, certificate) = x509_parser::parse_x509_certificate(end_entity)
                .map_err(|_| rustls::Error::General("unparseable".into()))?;
            if SpkiPin::of(certificate.public_key().raw) == self.0 {
                Ok(ServerCertVerified::assertion())
            } else {
                Err(rustls::Error::General("wrong key".into()))
            }
        }

        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                m,
                c,
                d,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &CertificateDer<'_>,
            d: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                m,
                c,
                d,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    pub type Stream = StreamOwned<ClientConnection, TcpStream>;

    /// A TLS 1.3 connection that accepts only `pin`.
    pub fn connect(port: u16, pin: &str) -> Stream {
        let pin = SpkiPin::parse(pin).expect("the ready line's pin parses");
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin(pin)))
        .with_no_client_auth();
        let connection = ClientConnection::new(
            Arc::new(config),
            ServerName::try_from("localhost").expect("name"),
        )
        .expect("client");
        let tcp = TcpStream::connect(("127.0.0.1", port)).expect("tcp");
        tcp.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        let mut stream = StreamOwned::new(connection, tcp);
        stream.flush().expect("handshake");
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .expect("TLS handshake with the pinned key");
        }
        stream
    }

    /// One GET on `stream`; returns the status and the body.
    pub fn get(stream: &mut Stream, port: u16, path: &str) -> (u16, String) {
        request(stream, port, "GET", path, "", "")
    }

    /// One request with a JSON body.
    pub fn post(stream: &mut Stream, port: u16, path: &str, body: &str) -> (u16, String) {
        let headers = format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        request(stream, port, "POST", path, &headers, body)
    }

    /// One request with a device token.
    pub fn authorized(
        stream: &mut Stream,
        port: u16,
        method: &str,
        path: &str,
        token: &str,
    ) -> (u16, String) {
        let headers = format!("Authorization: Bearer {token}\r\n");
        request(stream, port, method, path, &headers, "")
    }

    /// One request; returns the status and the body.
    pub fn request(
        stream: &mut Stream,
        port: u16,
        method: &str,
        path: &str,
        headers: &str,
        body: &str,
    ) -> (u16, String) {
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{headers}\r\n{body}"
        )
        .expect("write");
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0);
                while buffer.len() < end + 4 + length {
                    let read = stream.read(&mut chunk).expect("body");
                    assert!(read > 0, "connection closed mid-body");
                    buffer.extend_from_slice(&chunk[..read]);
                }
                let status = head
                    .split(' ')
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .expect("status");
                let body = String::from_utf8_lossy(&buffer[end + 4..end + 4 + length]).into_owned();
                return (status, body);
            }
            let read = stream.read(&mut chunk).expect("head");
            assert!(read > 0, "connection closed before a response");
            buffer.extend_from_slice(&chunk[..read]);
        }
    }

    /// `SHA-256(SPKI)` of the certificate this connection's handshake
    /// presented.
    pub fn spki(stream: &Stream) -> [u8; 32] {
        let certificate = &stream.conn.peer_certificates().expect("certificate")[0];
        let (_, parsed) = x509_parser::parse_x509_certificate(certificate).expect("x509");
        *SpkiPin::of(parsed.public_key().raw).as_bytes()
    }

    /// This connection's exporter, as the client computes it.
    pub fn exporter(stream: &Stream) -> [u8; 32] {
        stream
            .conn
            .export_keying_material([0_u8; 32], b"EXPORTER-Atrium-Pairing-v1", Some(b""))
            .expect("exporter")
    }
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_serves_tls_with_the_identity_key_and_a_restart_keeps_the_pin() {
    let installation = Installation::new("tls");
    let port = installation.port;

    let mut first = installation.start_core();
    let ready = first.ready();
    assert_eq!(field(&ready, "mode").as_deref(), Some("normal"), "{ready}");
    assert_eq!(
        field(&ready, "listen").as_deref(),
        Some(format!("127.0.0.1:{port}").as_str())
    );
    let pin = field(&ready, "spki_sha256").expect("pin");
    let mut stream = https::connect(port, &pin);
    let (status, body) = https::get(&mut stream, port, "/healthz");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"state\":\"normal\""), "{body}");
    drop(stream);
    first.stop();

    let mut second = installation.start_core();
    let ready = second.ready();
    assert_eq!(field(&ready, "spki_sha256"), Some(pin.clone()));
    // The pinned client connects again: same key, same pin.
    let mut stream = https::connect(port, &pin);
    assert_eq!(https::get(&mut stream, port, "/healthz").0, 200);
    drop(stream);
    second.stop();
}

#[test]
#[ignore = "requires root and the atrium user"]
fn the_exporter_never_reaches_the_log() {
    let installation = Installation::new("exporter");
    let port = installation.port;
    let mut core = installation.start_core_with_log("debug");
    let pin = field(&core.ready(), "spki_sha256").expect("pin");
    let mut exporters = Vec::new();
    for _ in 0..3 {
        let mut stream = https::connect(port, &pin);
        exporters.push(https::exporter(&stream));
        for path in ["/healthz", "/api/v1/system/diagnostics", "/nothing"] {
            https::get(&mut stream, port, path);
        }
    }
    core.stop();
    let log = core.lines().join("\n").to_lowercase();
    assert!(log.contains("http_request"), "debug request logging ran");
    for exporter in exporters {
        let hex = hex(&exporter);
        for start in (0..=hex.len() - 16).step_by(2) {
            assert!(
                !log.contains(&hex[start..start + 16]),
                "exporter bytes in the log"
            );
        }
        let decimal = format!("{}, {}, {}", exporter[0], exporter[1], exporter[2]);
        assert!(!log.contains(&decimal), "exporter bytes in the log");
    }
}

#[test]
#[ignore = "requires root and the atrium user"]
fn recovery_serves_health_over_tls_and_never_contacts_the_agent() {
    let installation = Installation::new("recnet");
    let port = installation.port;
    // A listener where Agent's socket belongs: recovery must never connect.
    for dir in ["run", "run/atrium"] {
        std::fs::create_dir_all(installation.root.join(dir)).expect("run");
        chmod(&installation.root.join(dir), 0o755);
    }
    let socket = installation.root.join("run/atrium/agent.sock");
    let agent = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    agent.set_nonblocking(true).expect("nonblocking");
    chown(&socket, 0, installation.atrium.gid);
    chmod(&socket, 0o660);

    let path = installation.state().join("atrium.db");
    let mut bytes = installation.read(&path);
    bytes.truncate(4096 + 100);
    std::fs::write(&path, &bytes).expect("damage");

    let mut core = installation.start_core();
    let ready = core.ready();
    assert_eq!(
        field(&ready, "mode").as_deref(),
        Some("recovery"),
        "{ready}"
    );
    // The pin is the identity's, read without the database.
    let pin = atrium_core::identity::load(
        &atrium_core::layout::Layout::under(&installation.root).expect("layout"),
        atrium_core::identity::Protection::NotChecked,
    )
    .expect("identity")
    .pin()
    .to_string();

    let mut stream = https::connect(port, &pin);
    let (status, health) = https::get(&mut stream, port, "/healthz");
    assert_eq!(status, 200);
    assert!(health.contains("\"state\":\"recovery\""), "{health}");
    let (status, diagnostics) = https::get(&mut stream, port, "/api/v1/system/diagnostics");
    assert_eq!(status, 200);
    for leak in ["127.0.0.1", "localhost", "atrium-", "/var", "/etc"] {
        assert!(!diagnostics.contains(leak), "{leak} in {diagnostics}");
    }
    for path in ["/", "/api/v1/pair/info", "/api/v1/devices"] {
        let (status, body) = https::get(&mut stream, port, path);
        assert_eq!(status, 503, "{path}: {body}");
        assert!(body.contains("internal.recovery_mode"), "{body}");
    }
    drop(stream);
    std::thread::sleep(Duration::from_millis(500));
    core.stop();

    match agent.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("recovery contacted the Agent socket: {other:?}"),
    }
    assert_eq!(installation.read(&path), bytes, "recovery wrote nothing");
}

#[test]
#[ignore = "requires root and the atrium user"]
fn core_stops_promptly_with_stalled_network_clients() {
    let installation = Installation::new("netstall");
    let port = installation.port;
    let mut core = installation.start_core();
    let pin = field(&core.ready(), "spki_sha256").expect("pin");
    // One silent TCP connection, one TLS connection mid-request.
    let _silent = std::net::TcpStream::connect(("127.0.0.1", port)).expect("tcp");
    let mut half = https::connect(port, &pin);
    half.write_all(b"GET /healthz HTTP/1.1\r\nHost: ")
        .expect("partial");
    half.flush().expect("flush");
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    core.stop();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "Core took {:?} to stop behind stalled clients",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Pairing (M1E)

/// What one pairing exchange put on the wire, for the secrecy scan.
struct Exchange {
    proof_c: [u8; 32],
    proof_s: Option<[u8; 32]>,
    exporter: [u8; 32],
    bodies: Vec<String>,
}

/// The harness client, synchronously: info, begin, prove, complete, and
/// `proofS` checked before anything is kept.
fn pair(
    stream: &mut https::Stream,
    port: u16,
    secret: &atrium_pairing::secret::PairingSecret,
    name: &str,
) -> (Result<atrium_pairing::client::Paired, u16>, Exchange) {
    use atrium_pairing::client::ClientPairing;
    use atrium_pairing::device::{DeviceMetadata, DeviceName, Platform};
    use atrium_pairing::wire::{BeginResponse, CompleteResponse, InfoResponse};

    let mut bodies = Vec::new();
    let (status, body) = https::get(stream, port, "/api/v1/pair/info");
    assert_eq!(status, 200, "{body}");
    let info: InfoResponse = serde_json::from_str(&body).expect("info");
    bodies.push(body);
    let exporter = https::exporter(stream);
    let mut nonce = [0_u8; 32];
    getrandom_fill(&mut nonce);
    let pairing = ClientPairing::new(
        atrium_pairing::secret::PairingSecret::from_bytes(*secret.expose()),
        DeviceMetadata::new(DeviceName::parse(name).expect("name"), Platform::Linux),
        info.server_id.0,
        https::spki(stream),
        exporter,
        nonce,
    );
    let (status, body) = https::post(
        stream,
        port,
        "/api/v1/pair/begin",
        &serde_json::to_string(&pairing.begin_request()).expect("json"),
    );
    assert_eq!(status, 200, "{body}");
    let begun: BeginResponse = serde_json::from_str(&body).expect("begin");
    bodies.push(body);
    let (request, awaiting) = pairing.prove(&begun);
    let (status, body) = https::post(
        stream,
        port,
        "/api/v1/pair/complete",
        &serde_json::to_string(&request).expect("json"),
    );
    let mut exchange = Exchange {
        proof_c: request.proof_c.0,
        proof_s: None,
        exporter,
        bodies: Vec::new(),
    };
    if status != 200 {
        bodies.push(body);
        exchange.bodies = bodies;
        return (Err(status), exchange);
    }
    let response: CompleteResponse = serde_json::from_str(&body).expect("complete");
    exchange.proof_s = Some(response.proof_s.0);
    exchange.bodies = bodies;
    (
        Ok(awaiting.finish(response).expect("the server proved itself")),
        exchange,
    )
}

fn getrandom_fill(bytes: &mut [u8]) {
    let mut file = std::fs::File::open("/dev/urandom").expect("urandom");
    std::io::Read::read_exact(&mut file, bytes).expect("random");
}

/// Every form a value could be written in.
fn forms(bytes: &[u8]) -> Vec<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    use base64::Engine as _;
    vec![
        bytes.to_vec(),
        hex(bytes).into_bytes(),
        hex(bytes).to_uppercase().into_bytes(),
        STANDARD.encode(bytes).into_bytes(),
        STANDARD_NO_PAD.encode(bytes).into_bytes(),
        URL_SAFE_NO_PAD.encode(bytes).into_bytes(),
    ]
}

fn find(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Test plan §9 "log scan" and "bundle scan", and criterion 32, on a real
/// installation: Core as `atrium`, `atriumctl pair` as root, a real client.
#[test]
#[ignore = "requires root and the atrium user"]
fn pairing_end_to_end_leaves_no_secret_in_logs_audit_diagnostics_or_responses() {
    let installation = Installation::new("pair");
    let port = installation.port;
    // Debug: the most Core will ever say.
    let mut core = installation.start_core_with_log("debug");
    let ready = core.ready();
    let pin = field(&ready, "spki_sha256").expect("pin");

    let armed = installation.as_root(&installation.ctl, &["pair"], "");
    assert!(armed.status.success(), "{}", text(&armed));
    let shown = String::from_utf8_lossy(&armed.stdout).into_owned();
    let code = shown
        .lines()
        .find_map(|line| line.trim().strip_prefix("Pairing code:"))
        .expect("the code is shown")
        .trim()
        .to_owned();
    assert_eq!(code.len(), 32, "XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XX: {code}");
    let secret = atrium_pairing::secret::PairingSecret::decode(&code).expect("the code decodes");
    // `atriumctl_leaves_no_root_owned_wal`: arming as root left nothing of
    // root's in the state directory.
    for entry in walk(&installation.state()) {
        let metadata = std::fs::symlink_metadata(&entry).expect("stat");
        assert_eq!(
            metadata.uid(),
            installation.atrium.uid,
            "{}",
            entry.display()
        );
    }

    // One failed attempt, then the real one, on one pinned connection.
    let mut stream = https::connect(port, &pin);
    let wrong = atrium_pairing::secret::PairingSecret::from_bytes([0x5a; 16]);
    let (refused, failed) = pair(&mut stream, port, &wrong, "Guess");
    assert_eq!(refused.err(), Some(403));
    let (paired, succeeded) = pair(&mut stream, port, &secret, "Owner laptop");
    let paired = paired.expect("paired");
    assert_eq!(hex(&paired.spki), pin);
    let token = paired.token.encode().to_string();
    let (status, me) = https::authorized(&mut stream, port, "GET", "/api/v1/me", &token);
    assert_eq!(status, 200, "{me}");
    let (status, list) = https::authorized(&mut stream, port, "GET", "/api/v1/devices", &token);
    assert_eq!(status, 200, "{list}");
    let (status, unauthorized) = https::get(&mut stream, port, "/api/v1/devices");
    assert_eq!(status, 401);
    // Criterion 32: the device-only diagnostics carries no secret either.
    let (status, normal_diagnostics) = https::authorized(
        &mut stream,
        port,
        "GET",
        "/api/v1/system/diagnostics",
        &token,
    );
    assert_eq!(status, 200, "{normal_diagnostics}");
    drop(stream);
    std::thread::sleep(Duration::from_millis(300));
    core.stop();

    let diagnostics = installation.as_root(&installation.ctl, &["diagnostics"], "");
    let audit: String = {
        let uri = format!(
            "file:{}?immutable=1",
            installation.state().join("atrium.db").display()
        );
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("open read-only");
        let mut statement = connection
            .prepare("SELECT action || ' ' || coalesce(target,'') || ' ' || coalesce(detail,'') FROM audit")
            .expect("audit");
        let rows: Vec<String> = statement
            .query_map((), |r| r.get(0))
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("rows");
        rows.join("\n")
    };
    for action in ["pairing.armed", "pairing.consumed", "device.created"] {
        assert!(audit.contains(action), "{action}: {audit}");
    }
    // ADR-020: the failed attempt is counted in the row that ends the arming.
    assert!(audit.contains(r#""failed_attempts":1"#), "{audit}");

    let token_bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .expect("token")
    };
    let mut forbidden = forms(secret.expose());
    forbidden.push(secret.canonical().as_bytes().to_vec());
    forbidden.push(code.clone().into_bytes());
    forbidden.extend(forms(&token_bytes));
    forbidden.extend(forms(paired.token.digest().as_bytes()));
    for exchange in [&failed, &succeeded] {
        forbidden.extend(forms(&exchange.proof_c));
        forbidden.extend(forms(&exchange.exporter));
        if let Some(proof_s) = exchange.proof_s {
            forbidden.extend(forms(&proof_s));
        }
    }

    let log = core.lines().join("\n");
    assert!(log.contains("device_paired"), "the run did log: {log}");
    let mut places: Vec<(&str, Vec<u8>)> = vec![
        ("core log", log.into_bytes()),
        ("atriumctl pair stderr", armed.stderr.clone()),
        (
            "diagnostics",
            [diagnostics.stdout.clone(), diagnostics.stderr.clone()].concat(),
        ),
        ("audit", audit.into_bytes()),
        ("device list", list.into_bytes()),
        ("me", me.into_bytes()),
        ("401 body", unauthorized.into_bytes()),
        ("normal diagnostics", normal_diagnostics.into_bytes()),
    ];
    for body in failed.bodies.iter().chain(&succeeded.bodies) {
        places.push(("pairing response", body.clone().into_bytes()));
    }
    for (place, bytes) in &places {
        for form in &forbidden {
            assert!(
                !find(bytes, form),
                "secret material in the {place}: {:?}",
                String::from_utf8_lossy(form)
            );
        }
    }
    // The database holds the digest (the verifier) and nothing else of
    // either secret.
    let mut on_disk = forms(secret.expose());
    on_disk.push(secret.canonical().as_bytes().to_vec());
    on_disk.extend(forms(&token_bytes));
    on_disk.push(token.clone().into_bytes());
    for entry in walk(&installation.state()) {
        if entry.is_file() {
            let bytes = installation.read(&entry);
            for form in &on_disk {
                assert!(!find(&bytes, form), "{}", entry.display());
            }
        }
    }
}

/// Test plan 4.5 `local_restore_succeeds_and_normal_service_resumes`: a device
/// paired before the corruption still authenticates after a console restore.
/// The backup is the one Core takes before migrating a schema-1 database
/// that already holds the device.
#[test]
#[ignore = "requires root and the atrium user"]
fn a_device_paired_before_corruption_authenticates_after_restore() {
    let installation = Installation::new("restoredevice");
    let port = installation.port;
    let pin = atrium_core::identity::load(
        &atrium_core::layout::Layout::under(&installation.root).expect("layout"),
        atrium_core::identity::Protection::NotChecked,
    )
    .expect("identity")
    .pin()
    .to_string();
    let mut raw = [0_u8; 32];
    getrandom_fill(&mut raw);
    let token = atrium_pairing::token::DeviceToken::from_bytes(raw);

    // A schema-1 database with a paired device, as M1D left them.
    let path = installation.state().join("atrium.db");
    std::fs::remove_file(&path).expect("remove");
    {
        let connection = Connection::open(&path).expect("create");
        connection
            .execute_batch(include_str!(
                "../../atrium-core/src/db/migrations/0001_initial.sql"
            ))
            .expect("schema 1");
        connection
            .execute(
                "INSERT INTO devices (device_id, name, platform, role, token_digest, created_at) \
                 VALUES ('00112233445566778899aabbccddeeff', 'Laptop', 'linux', 'owner', ?1, \
                 '2026-09-24T00:00:00Z')",
                [token.digest().as_bytes().as_slice()],
            )
            .expect("device");
        connection
            .execute("INSERT INTO pairing_state (id, claimed) VALUES (1, 1)", ())
            .expect("claimed");
        connection
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                (atrium_core::db::IDENTITY_PIN_SETTING, &pin),
            )
            .expect("pin");
        connection
            .pragma_update(None, "user_version", 1)
            .expect("version");
    }
    chown(&path, installation.atrium.uid, installation.atrium.gid);
    chmod(&path, 0o600);

    // Core migrates behind a backup; the device authenticates.
    let mut core = installation.start_core();
    assert_eq!(field(&core.ready(), "mode").as_deref(), Some("normal"));
    let mut stream = https::connect(port, &pin);
    let bearer = token.encode().to_string();
    assert_eq!(
        https::authorized(&mut stream, port, "GET", "/api/v1/me", &bearer).0,
        200
    );
    drop(stream);
    core.stop();

    // Corruption, then the console restore.
    let name = backup_name(&installation);
    std::fs::write(&path, vec![0x5a_u8; 8192]).expect("corrupt");
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let mut core = installation.start_core();
    assert_eq!(field(&core.ready(), "mode").as_deref(), Some("recovery"));
    core.stop();
    let output = installation.as_root(
        &installation.ctl,
        &["restore", "--from", &name],
        &format!("{SERVER_NAME}\n"),
    );
    assert!(output.status.success(), "{}", text(&output));

    let mut core = installation.start_core();
    assert_eq!(field(&core.ready(), "mode").as_deref(), Some("normal"));
    let mut stream = https::connect(port, &pin);
    let (status, me) = https::authorized(&mut stream, port, "GET", "/api/v1/me", &bearer);
    assert_eq!(status, 200, "{me}");
    assert!(me.contains("00112233445566778899aabbccddeeff"), "{me}");
    drop(stream);
    core.stop();
}

// ---------------------------------------------------------------------------
// System domain (M1F), against the real kernel

impl Installation {
    /// A paired device, inserted before Core starts: its bearer token.
    fn device_token(&self) -> String {
        let mut raw = [0_u8; 32];
        getrandom_fill(&mut raw);
        let token = atrium_pairing::token::DeviceToken::from_bytes(raw);
        let mut id = [0_u8; 16];
        getrandom_fill(&mut id);
        self.with_database(|connection| {
            connection
                .execute(
                    "INSERT INTO devices (device_id, name, platform, role, token_digest, \
                     created_at) VALUES (?1, 'Probe', 'linux', 'owner', ?2, \
                     '2026-09-24T00:00:00Z')",
                    (hex(&id), token.digest().as_bytes().as_slice()),
                )
                .expect("device");
        });
        token.encode().to_string()
    }
}

fn api_json(stream: &mut https::Stream, port: u16, path: &str, token: &str) -> serde_json::Value {
    let (status, body) = https::authorized(stream, port, "GET", path, token);
    assert_eq!(status, 200, "{path}: {body}");
    serde_json::from_str(&body).expect("json")
}

fn ip(args: &[&str]) {
    let output = Command::new("ip")
        .args(args)
        .output()
        .expect("iproute2's `ip` is required for the multi-NIC test (it is on every CI runner)");
    assert!(output.status.success(), "ip {args:?}: {}", text(&output));
}

/// Removes the test's veth pair whatever happens.
struct Veth(String);

impl Drop for Veth {
    fn drop(&mut self) {
        let _ = Command::new("ip").args(["link", "del", &self.0]).output();
    }
}

/// Criterion 23 on a real kernel: two NICs — a veth pair, both ends up —
/// each with its own IPv4 and IPv6 addresses, listed with every address and
/// no notion of a primary one.
#[test]
#[ignore = "requires root and the atrium user"]
fn two_nics_every_interface_and_every_address_reach_the_api() {
    let installation = Installation::new("nics");
    let port = installation.port;
    let tag = std::process::id() % 100_000;
    let (a, b) = (format!("atp{tag}a"), format!("atp{tag}b"));
    ip(&["link", "add", &a, "type", "veth", "peer", "name", &b]);
    let _cleanup = Veth(a.clone());
    ip(&["addr", "add", "10.211.1.1/24", "dev", &a]);
    ip(&["addr", "add", "10.211.1.2/24", "dev", &a]);
    // IPv6 is part of the criterion wherever the kernel has it. The CI
    // runner does, so there its absence fails; a sandbox kernel built
    // without IPv6 checks the IPv4 half and says so.
    let ipv6 = Path::new("/proc/net/if_inet6").exists();
    assert!(
        ipv6 || std::env::var_os("CI").is_none(),
        "the CI runner must have IPv6"
    );
    if ipv6 {
        ip(&["addr", "add", "fd00:211::1/64", "dev", &a, "nodad"]);
    } else {
        eprintln!("\n*** IPv6 is not available in this kernel: checking IPv4 only ***\n");
    }
    ip(&["addr", "add", "10.212.0.1/16", "dev", &b]);
    ip(&["link", "set", &a, "up"]);
    ip(&["link", "set", &b, "up"]);

    let token = installation.device_token();
    let mut core = installation.start_core();
    let pin = field(&core.ready(), "spki_sha256").expect("pin");
    let mut stream = https::connect(port, &pin);
    let view = api_json(&mut stream, port, "/api/v1/network/interfaces", &token);
    drop(stream);
    core.stop();

    let interfaces = view["interfaces"].as_array().expect("a list");
    let find = |name: &str| {
        interfaces
            .iter()
            .find(|i| i["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {view}"))
    };
    let addresses = |interface: &serde_json::Value| -> Vec<(String, String, u64)> {
        interface["addresses"]
            .as_array()
            .expect("addresses")
            .iter()
            .map(|a| {
                (
                    a["family"].as_str().expect("family").to_owned(),
                    a["address"].as_str().expect("address").to_owned(),
                    a["prefixLength"].as_u64().expect("prefix"),
                )
            })
            .collect()
    };
    let first = addresses(find(&a));
    let mut expected_addresses = vec![("ipv4", "10.211.1.1", 24), ("ipv4", "10.211.1.2", 24)];
    if ipv6 {
        expected_addresses.push(("ipv6", "fd00:211::1", 64));
    }
    for expected in expected_addresses {
        assert!(
            first.contains(&(expected.0.into(), expected.1.into(), expected.2)),
            "{expected:?} in {first:?}"
        );
    }
    let second = addresses(find(&b));
    assert!(second.contains(&("ipv4".into(), "10.212.0.1".into(), 16)));
    assert_eq!(find(&a)["virtual"], true);
    assert_eq!(find("lo")["loopback"], true);
    for interface in interfaces {
        assert!(interface.get("primary").is_none());
    }
    assert!(view.get("primaryInterface").is_none() && view.get("ip").is_none());
}

/// Removes the test's mounts whatever happens.
struct Mounted(Vec<PathBuf>);

impl Drop for Mounted {
    fn drop(&mut self) {
        for path in self.0.iter().rev() {
            let _ = Command::new("umount").arg(path).output();
        }
    }
}

/// Criterion 24 on a real kernel: real capacity for a real mount, a tmpfs
/// excluded, and a bind mount collapsed into its filesystem rather than
/// listed twice or hidden.
#[test]
#[ignore = "requires root and the atrium user"]
fn filesystems_report_real_capacity_and_collapse_bind_mounts() {
    let installation = Installation::new("mounts");
    let port = installation.port;
    let base = installation.root.join("mnt");
    let (scratch, source, target) = (base.join("scratch"), base.join("src"), base.join("dst"));
    for dir in [&scratch, &source, &target] {
        std::fs::create_dir_all(dir).expect("dir");
    }
    let run = |args: &[&str]| {
        let output = Command::new("mount").args(args).output().expect("mount");
        assert!(output.status.success(), "mount {args:?}: {}", text(&output));
    };
    let mut guard = Mounted(Vec::new());
    run(&["-t", "tmpfs", "tmpfs", &scratch.display().to_string()]);
    guard.0.push(scratch.clone());
    run(&[
        "--bind",
        &source.display().to_string(),
        &target.display().to_string(),
    ]);
    guard.0.push(target.clone());

    let token = installation.device_token();
    let mut core = installation.start_core();
    let pin = field(&core.ready(), "spki_sha256").expect("pin");
    let mut stream = https::connect(port, &pin);
    let view = api_json(&mut stream, port, "/api/v1/storage/filesystems", &token);
    drop(stream);
    core.stop();

    let list = view["filesystems"].as_array().expect("a list");
    let points: Vec<&str> = list
        .iter()
        .map(|f| f["mountPoint"].as_str().expect("mount point"))
        .collect();
    let scratch_text = scratch.display().to_string();
    let target_text = target.display().to_string();
    assert!(
        !points.contains(&scratch_text.as_str()),
        "tmpfs is excluded"
    );
    assert!(
        !points.contains(&target_text.as_str()),
        "a bind mount is not a filesystem"
    );
    let holder = list
        .iter()
        .find(|f| {
            f["alsoMountedAt"]
                .as_array()
                .is_some_and(|also| also.iter().any(|p| p == &target_text))
        })
        .unwrap_or_else(|| panic!("the bind mount is listed under its filesystem: {view}"));

    // The capacity is the kernel's own figure for that filesystem.
    let stat = nix::sys::statvfs::statvfs(Path::new(
        holder["mountPoint"].as_str().expect("mount point"),
    ))
    .expect("statvfs");
    #[allow(clippy::useless_conversion)]
    let total = u64::from(stat.blocks()) * u64::from(stat.fragment_size());
    assert_eq!(holder["usage"]["totalBytes"].as_u64(), Some(total));
    let usage = &holder["usage"];
    assert_eq!(
        usage["usedBytes"].as_u64().expect("used") + usage["freeBytes"].as_u64().expect("free"),
        total
    );
}

fn proc_stat_busy_total() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/stat").expect("/proc/stat");
    let fields: Vec<u64> = text
        .lines()
        .next()
        .expect("cpu line")
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|f| f.parse().expect("number"))
        .collect();
    let total: u64 = fields.iter().sum();
    (total - fields[3] - fields[4], total)
}

/// Criteria 21, 22, 25 and 29 on a real kernel. With every CPU kept busy,
/// Core's usage agrees with a reading of `/proc/stat` taken around the same
/// window; memory is exactly `MemTotal`; and with Agent stopped, hardware
/// reads keep working while privileged and container say why they cannot.
#[test]
#[ignore = "requires root and the atrium user"]
fn metrics_match_proc_under_known_load_and_reads_survive_a_stopped_agent() {
    let installation = Installation::new("metrics");
    let port = installation.port;
    let token = installation.device_token();
    let mut core = installation.start_core();
    let pin = field(&core.ready(), "spki_sha256").expect("pin");
    let mut stream = https::connect(port, &pin);

    // Right after start the first sample may not exist: null, never 0.
    let early = api_json(&mut stream, port, "/api/v1/system/metrics", &token);
    let early_cpu = &early["cpu"]["usagePercent"];
    assert!(early_cpu.is_null() || early_cpu.as_f64().is_some());
    if early_cpu.is_null() {
        assert!(early["unavailable"]
            .to_string()
            .contains("first_sample_pending"));
    }

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpus = std::thread::available_parallelism().map_or(2, usize::from);
    let workers: Vec<_> = (0..cpus)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut x = 0_u64;
                while !stop.load(Ordering::Relaxed) {
                    x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(5));
    let before = proc_stat_busy_total();
    std::thread::sleep(Duration::from_millis(2500));
    let after = proc_stat_busy_total();
    let loaded = api_json(&mut stream, port, "/api/v1/system/metrics", &token);
    stop.store(true, Ordering::Relaxed);
    for worker in workers {
        worker.join().expect("worker");
    }
    #[allow(clippy::cast_precision_loss)]
    let ours = (after.0 - before.0) as f64 * 100.0 / (after.1 - before.1).max(1) as f64;
    let core_value = loaded["cpu"]["usagePercent"]
        .as_f64()
        .expect("a sampled value");
    assert!(
        (core_value - ours).abs() <= 25.0,
        "Core says {core_value}%, /proc/stat says {ours}%"
    );
    assert!(core_value >= 50.0, "every CPU was busy: {core_value}%");

    let meminfo = std::fs::read_to_string("/proc/meminfo").expect("meminfo");
    let mem_total_kib: u64 = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .expect("MemTotal");
    assert_eq!(
        loaded["memory"]["totalBytes"].as_u64(),
        Some(mem_total_kib * 1024)
    );
    assert!(loaded["load"]["one"].as_f64().is_some());

    // No Agent is running in this installation: reads keep working.
    let capabilities = api_json(&mut stream, port, "/api/v1/system/capabilities", &token);
    assert_eq!(capabilities["hardware"]["available"], true);
    assert_eq!(capabilities["network"]["available"], true);
    assert_eq!(capabilities["storage"]["available"], true);
    for name in ["privileged", "container"] {
        assert_eq!(capabilities[name]["available"], false, "{capabilities}");
        assert_eq!(
            capabilities[name]["missing"][0]["reason"],
            "agent_unreachable"
        );
    }
    let system = api_json(&mut stream, port, "/api/v1/system", &token);
    assert_eq!(system["kernel"]["name"], "Linux");
    let diagnostics = api_json(&mut stream, port, "/api/v1/system/diagnostics", &token);
    assert_eq!(
        diagnostics["capabilities"]["privileged"]["reasons"][0],
        "agent_unreachable"
    );
    assert!(diagnostics["recentIssues"]
        .as_array()
        .expect("issues")
        .iter()
        .any(|i| i["code"] == "agent_unreachable" && i["subject"] == "agent"));
    drop(stream);
    core.stop();

    // With Agent running, both come through it; a runtime counts as present
    // only when its socket exists, and never as running.
    let _agent = installation.start_agent();
    let mut core = installation.start_core();
    let pin = field(&core.ready(), "spki_sha256").expect("pin");
    core.wait_for("\"event\":\"agent_status\"");
    let mut stream = https::connect(port, &pin);
    let capabilities = api_json(&mut stream, port, "/api/v1/system/capabilities", &token);
    drop(stream);
    core.stop();
    assert_eq!(
        capabilities["privileged"]["available"], true,
        "{capabilities}"
    );
    assert_eq!(capabilities["privileged"]["protocol"], 1);
    assert_eq!(capabilities["container"]["via"], "agent");
    assert!(capabilities["container"]["version"].is_null());
    // Present means a socket Agent's passive probe found (its own tests
    // cover which paths); either way nothing claims the runtime is running.
    let available = capabilities["container"]["available"]
        .as_bool()
        .expect("bool");
    let reasons = capabilities["container"]["missing"].to_string();
    if available {
        assert!(
            reasons.contains("runtime_liveness_not_probed_in_m1"),
            "{reasons}"
        );
        assert!(capabilities["container"]["provider"].is_string());
    } else {
        assert!(reasons.contains("no_container_runtime"), "{reasons}");
        assert!(capabilities["container"]["provider"].is_null());
    }
}
