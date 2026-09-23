//! The console commands, driven as an unprivileged user against a tree that
//! user owns. As root the same commands drop to the service user first; that
//! path, and the ownership it leaves behind, is in `privileged.rs`.

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use atrium_core::certificate::AddressSet;
use atrium_core::db;
use atrium_core::identity::{self, Protection};
use atrium_core::init::{self, InitOutcome};
use atrium_core::layout::Layout;
use rusqlite::Connection;
use time::OffsetDateTime;

const SERVER_NAME: &str = "test-server";

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Tree {
    root: PathBuf,
    layout: Layout,
}

impl Tree {
    /// An initialized installation owned by the test user.
    fn initialized(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "atriumctl-it-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout::under(&root).expect("absolute");
        for dir in [layout.etc_dir(), layout.state_dir()] {
            std::fs::create_dir_all(dir).expect("dir");
            chmod(dir, 0o700);
        }
        std::fs::write(
            layout.config(),
            format!("server_name = \"{SERVER_NAME}\"\n"),
        )
        .expect("config");
        chmod(&layout.config(), 0o600);
        match init::run(&layout, &AddressSet::default(), OffsetDateTime::now_utc()).expect("init") {
            InitOutcome::Created { .. } => {}
            other => panic!("{other:?}"),
        }
        Self { root, layout }
    }

    fn ctl(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_atriumctl"))
            .args(args)
            .env("ATRIUM_ROOT", &self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("atriumctl must be spawnable");
        // A command that refuses before reading stdin may already have
        // exited; the broken pipe is not the test's concern, the exit is.
        let _ = child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin.as_bytes());
        child.wait_with_output().expect("wait")
    }

    /// Makes one backup exist: rewinds the schema to 0 behind a marker table,
    /// then lets a normal open migrate it.
    fn with_backup(&self) -> String {
        let path = self.layout.database();
        std::fs::remove_file(&path).expect("remove");
        {
            let connection = Connection::open(&path).expect("open");
            connection
                .execute_batch("CREATE TABLE marker (v TEXT); INSERT INTO marker VALUES ('old');")
                .expect("seed");
        }
        chmod(&path, 0o600);
        let opened = db::open(&self.layout, OffsetDateTime::now_utc()).expect("migrates");
        let name = opened
            .migrated
            .as_ref()
            .expect("migrated")
            .backup
            .file_name()
            .and_then(|n| n.to_str())
            .expect("name")
            .to_owned();
        drop(opened);
        name
    }

    fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
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
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, files),
            Ok(kind) if kind.is_file() => {
                files.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
            }
            _ => files.push((path, Vec::new())),
        }
    }
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn restore_list_shows_verified_backups() {
    let tree = Tree::initialized("list");
    let name = tree.with_backup();
    let output = tree.ctl(&["restore", "--list"], "");
    assert!(output.status.success(), "{}", text(&output));
    let listing = text(&output);
    assert!(listing.contains(&name), "{listing}");
    assert!(listing.contains(" ok"), "{listing}");
}

#[test]
fn restore_requires_the_server_name_and_changes_nothing_without_it() {
    let tree = Tree::initialized("confirm");
    let name = tree.with_backup();
    let before = tree.snapshot();
    for answer in ["", "wrong-name\n", "test-server-but-longer\n"] {
        let output = tree.ctl(&["restore", "--from", &name], answer);
        assert!(!output.status.success(), "{answer:?}: {}", text(&output));
        assert_eq!(tree.snapshot(), before, "{answer:?}: nothing may change");
    }
}

#[test]
fn local_restore_succeeds_and_moves_the_corrupt_database_aside() {
    let tree = Tree::initialized("restore");
    let name = tree.with_backup();
    std::fs::write(tree.layout.database(), vec![0x5a_u8; 8192]).expect("corrupt");

    let output = tree.ctl(&["restore", "--from", &name], &format!("{SERVER_NAME}\n"));
    assert!(output.status.success(), "{}", text(&output));

    let aside: Vec<PathBuf> = std::fs::read_dir(tree.layout.state_dir())
        .expect("list")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("atrium.db.corrupt-"))
        })
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read(&aside[0]).expect("kept"), vec![0x5a_u8; 8192]);

    // Normal service resumes: the database opens, at the current schema,
    // with the pre-corruption data, and the restore is audited.
    let opened = db::open(&tree.layout, OffsetDateTime::now_utc()).expect("opens");
    let connection = opened.database.connection();
    let marker: String = connection
        .query_row("SELECT v FROM marker", (), |row| row.get(0))
        .expect("marker");
    assert_eq!(marker, "old");
    let restored: i64 = connection
        .query_row(
            "SELECT count(*) FROM audit WHERE action = 'state.restored'",
            (),
            |row| row.get(0),
        )
        .expect("audit");
    assert_eq!(restored, 1);
}

#[test]
fn restore_refuses_while_core_holds_the_state_directory() {
    let tree = Tree::initialized("busy");
    let name = tree.with_backup();
    let _held = atrium_core::lock::acquire(tree.layout.state_dir()).expect("lock");
    let before = tree.snapshot();
    let output = tree.ctl(&["restore", "--from", &name], &format!("{SERVER_NAME}\n"));
    assert!(!output.status.success());
    assert!(text(&output).contains("Stop it first"), "{}", text(&output));
    assert_eq!(tree.snapshot(), before);
}

#[test]
fn restore_accepts_only_backup_names() {
    let tree = Tree::initialized("names");
    tree.with_backup();
    let before = tree.snapshot();
    for bad in ["../../etc/passwd", "/etc/shadow", "atrium.db", "tls.crt"] {
        let output = tree.ctl(&["restore", "--from", bad], &format!("{SERVER_NAME}\n"));
        assert!(!output.status.success(), "{bad}: {}", text(&output));
    }
    assert_eq!(tree.snapshot(), before);
}

#[test]
fn rotation_changes_spki_preserves_server_id_and_revokes_all_devices() {
    let tree = Tree::initialized("rotate");
    let before = identity::load(&tree.layout, Protection::NotChecked).expect("identity");
    {
        let connection = Connection::open(tree.layout.database()).expect("open");
        connection
            .execute(
                "INSERT INTO devices (device_id, name, platform, role, token_digest, created_at) \
                 VALUES ('00112233445566778899aabbccddeeff', 'laptop', 'linux', 'owner', x'00', \
                 '2026-09-24T00:00:00Z')",
                (),
            )
            .expect("device");
    }

    let output = tree.ctl(&["rotate-identity"], &format!("{SERVER_NAME}\n"));
    assert!(output.status.success(), "{}", text(&output));

    let after = identity::load(&tree.layout, Protection::NotChecked).expect("consistent");
    assert_eq!(
        after.server_id(),
        before.server_id(),
        "server_id survives rotation"
    );
    assert_ne!(after.pin(), before.pin(), "the pin must change");

    let opened = db::open(&tree.layout, OffsetDateTime::now_utc()).expect("opens");
    let connection = opened.database.connection();
    let devices: i64 = connection
        .query_row("SELECT count(*) FROM devices", (), |row| row.get(0))
        .expect("count");
    assert_eq!(devices, 0, "every device is revoked");
    let actions: Vec<String> = connection
        .prepare("SELECT action FROM audit ORDER BY id")
        .expect("prepare")
        .query_map((), |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert!(
        actions.contains(&"identity.devices_revoked".to_owned()),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"identity.key_rotated".to_owned()),
        "{actions:?}"
    );

    // The certificate on disk certifies the new key.
    let pem = std::fs::read(tree.layout.state_file("tls.crt")).expect("cert");
    let record = atrium_core::certificate::describe_pem(&pem).expect("valid");
    assert_eq!(record.spki, after.pin());
}

#[test]
fn rotation_needs_the_server_name() {
    let tree = Tree::initialized("rotateconfirm");
    let before = tree.snapshot();
    let output = tree.ctl(&["rotate-identity"], "nope\n");
    assert!(!output.status.success());
    assert_eq!(tree.snapshot(), before);
}

#[test]
fn diagnostics_never_prints_key_material() {
    let tree = Tree::initialized("diag");
    let output = tree.ctl(&["diagnostics"], "");
    assert!(output.status.success(), "{}", text(&output));
    let shown = text(&output);
    let identity = identity::load(&tree.layout, Protection::NotChecked).expect("identity");
    assert!(shown.contains(&identity.server_id().to_string()), "{shown}");

    let key = std::fs::read(tree.layout.etc_file("tls.key")).expect("key");
    let secrets = std::fs::read(tree.layout.etc_file("secrets.key")).expect("secrets");
    for material in [&key, &secrets] {
        let hex: String = material.iter().map(|b| format!("{b:02x}")).collect();
        for start in (0..=hex.len() - 32).step_by(2) {
            assert!(
                !shown.contains(&hex[start..start + 32]),
                "diagnostics leaked key material"
            );
        }
    }
    assert!(!shown.contains("PRIVATE KEY"), "{shown}");
}

#[test]
fn diagnostics_makes_no_durable_or_semantic_change() {
    let tree = Tree::initialized("diagquiet");
    let before = tree.snapshot();
    let output = tree.ctl(&["diagnostics"], "");
    assert!(output.status.success(), "{}", text(&output));
    // SQLite opens a WAL database, even read-only, by creating its companion
    // files: -shm, and an empty -wal when none existed. Neither holds state;
    // anything else, or a -wal with content, is a write.
    let after: Vec<_> = tree
        .snapshot()
        .into_iter()
        .filter(|(path, bytes)| {
            let path = path.to_string_lossy();
            !(path.ends_with("-shm") || (path.ends_with("-wal") && bytes.is_empty()))
        })
        .collect();
    assert_eq!(after, before);
}

#[test]
fn the_shipped_configuration_example_is_valid() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/core.toml.example");
    let text = std::fs::read_to_string(&path).expect("example");
    let config =
        atrium_core::config::parse(&text, &Layout::production()).expect("the example must parse");
    assert_eq!(config.server_name, "atrium");
    assert_eq!(config.listen.port(), 7443);
}
