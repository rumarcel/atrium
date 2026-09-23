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

        let installation = Self {
            root,
            atrium,
            core,
            ctl,
        };

        // install.sh step 4 and 7: directories, and core.toml as root:atrium 0640.
        let etc = installation.etc();
        let state = installation.state();
        std::fs::create_dir_all(&etc).expect("etc");
        std::fs::create_dir_all(&state).expect("state");
        chown(&state, atrium.uid, atrium.gid);
        chmod(&state, 0o700);
        let config = etc.join("core.toml");
        std::fs::write(&config, format!("server_name = \"{SERVER_NAME}\"\n")).expect("config");
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
        let mut command = self.command(&self.core, &[]);
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
                 VALUES ('00112233445566778899aabbccddeeff', 'laptop', 'linux', 'owner', x'00', \
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
