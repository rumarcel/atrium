//! Recovery mode and fail-closed startup, as an unprivileged process.
//!
//! Everything here runs as the ordinary test user, which cannot own files as
//! root. That is used deliberately: an identity created by the test user is an
//! identity the service user could rewrite, which is exactly the deployment
//! error Core must refuse. The normal-mode path, with a correctly root-owned
//! identity, is in `atriumctl`'s privileged suite.

#![cfg(unix)]

mod common;

use std::os::unix::net::UnixListener;
use std::time::Duration;

use common::{describe, field, Proc, Tree};

/// How long a recovered process must stay up to count as "not crash-looping".
const STAYS_UP: Duration = Duration::from_millis(1500);

fn ready_in_recovery(core: &Proc, reason: &str) -> String {
    let line = core.ready();
    assert_eq!(
        field(&line, "mode").as_deref(),
        Some("recovery"),
        "expected recovery mode: {line}"
    );
    assert_eq!(
        field(&line, "reason").as_deref(),
        Some(reason),
        "expected reason {reason}: {line}"
    );
    line
}

#[test]
fn missing_identity_enters_recovery_and_stays_up() {
    let tree = Tree::new("missing");
    let mut core = Proc::start(&tree.root);
    ready_in_recovery(&core, "identity.missing");

    // Criterion 30's "no crash loop": the process does not exit on its own.
    std::thread::sleep(STAYS_UP);
    core.assert_running();

    core.shutdown_cleanly();
    assert!(
        core.lines()
            .iter()
            .any(|line| line.contains("\"event\":\"recovery_entered\"")),
        "entering recovery must be logged: {:#?}",
        core.lines()
    );
}

#[test]
fn recovery_writes_nothing() {
    // No identity generated, no database created, no certificate, no lock file.
    let tree = Tree::new("nowrite");
    let before = tree.snapshot();
    let mut core = Proc::start(&tree.root);
    ready_in_recovery(&core, "identity.missing");
    core.shutdown_cleanly();
    assert_eq!(
        tree.snapshot(),
        before,
        "recovery mode must not write anything"
    );
}

#[test]
fn recovery_reports_the_failure_clearly() {
    let tree = Tree::new("report");
    let mut core = Proc::start(&tree.root);
    let line = ready_in_recovery(&core, "identity.missing");
    core.shutdown_cleanly();

    // The diagnostics payload is in the ready line, and says what is wrong.
    assert!(line.contains("identity.missing"), "{line}");
    assert!(line.contains("\\\"state\\\":\\\"recovery\\\""), "{line}");
    assert!(!line.contains("\"state\":\"ok\""), "{line}");
}

#[test]
fn partial_identity_enters_recovery_without_regenerating() {
    let tree = Tree::new("partial");
    let output = tree.init();
    assert!(output.status.success(), "init: {output:?}");
    std::fs::remove_file(tree.etc().join("identity.json")).expect("remove the record");
    let key = std::fs::read(tree.etc().join("tls.key")).expect("key");
    let before = tree.snapshot();

    let mut core = Proc::start(&tree.root);
    ready_in_recovery(&core, "identity.inconsistent");
    std::thread::sleep(STAYS_UP);
    core.assert_running();
    core.shutdown_cleanly();

    assert_eq!(
        std::fs::read(tree.etc().join("tls.key")).expect("key"),
        key,
        "the key must be byte-identical"
    );
    assert!(
        !tree.etc().join("identity.json").exists(),
        "no record may be regenerated"
    );
    assert_eq!(tree.snapshot(), before);
}

#[test]
fn key_without_its_record_or_record_without_its_key_is_never_regenerated() {
    for remove in ["tls.key", "secrets.key"] {
        let tree = Tree::new("halves");
        assert!(tree.init().status.success());
        std::fs::remove_file(tree.etc().join(remove)).expect("remove");
        let before = tree.snapshot();

        let mut core = Proc::start(&tree.root);
        ready_in_recovery(&core, "identity.inconsistent");
        core.shutdown_cleanly();
        assert_eq!(
            tree.snapshot(),
            before,
            "{remove}: nothing may be recreated"
        );
    }
}

#[test]
fn an_identity_the_service_user_could_rewrite_is_refused() {
    // Created by the test user, so owned by the user Core runs as — the
    // misconfiguration that would let a compromised Core replace its key.
    let tree = Tree::new("unprotected");
    assert!(tree.init().status.success());
    let before = tree.snapshot();

    let mut core = Proc::start(&tree.root);
    ready_in_recovery(&core, "identity.unprotected");
    core.shutdown_cleanly();
    assert_eq!(tree.snapshot(), before);
}

#[test]
fn recovery_never_contacts_the_agent() {
    // A listener where the agent socket lives in production, relative to the
    // tree. Recovery holds no agent client, so nothing may connect. (M1B Core
    // has no agent client at all; this test is here so it keeps holding when
    // M1C adds one.)
    let tree = Tree::new("agent");
    let run = tree.root.join("run/atrium");
    std::fs::create_dir_all(&run).expect("run dir");
    let listener = UnixListener::bind(run.join("agent.sock")).expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");

    let mut core = Proc::start(&tree.root);
    ready_in_recovery(&core, "identity.missing");
    std::thread::sleep(STAYS_UP);
    let accepted = listener.accept();
    core.shutdown_cleanly();

    match accepted {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("something connected to the agent socket in recovery: {other:?}"),
    }
}

#[test]
fn missing_configuration_is_a_startup_failure_not_a_default() {
    let tree = Tree::new("noconfig");
    std::fs::remove_file(tree.etc().join("core.toml")).expect("remove");
    let mut core = Proc::start(&tree.root);
    let status = core.wait_exit();
    assert!(!status.success(), "must refuse, {}", describe(status));
    assert!(
        core.lines()
            .iter()
            .any(|line| line.contains("startup_failed")),
        "{:#?}",
        core.lines()
    );
}

#[test]
fn invalid_configuration_is_a_startup_failure() {
    for text in [
        "server_name = \"x\"\nlisten = \"0.0.0.0:80\"\n",
        "server_name = \"x\"\ndata_dir = \"/tmp\"\n",
        "server_name = \"x\"\nunknown_key = 1\n",
    ] {
        let tree = Tree::new("badconfig");
        tree.write_config(text);
        let mut core = Proc::start(&tree.root);
        let status = core.wait_exit();
        assert!(
            !status.success(),
            "{text:?} must be refused, {}",
            describe(status)
        );
    }
}

#[test]
fn a_second_core_cannot_share_the_state_directory() {
    let tree = Tree::new("twocores");
    let mut first = Proc::start(&tree.root);
    first.ready();

    let mut second = Proc::start(&tree.root);
    let status = second.wait_exit();
    assert!(
        !status.success(),
        "the second Core must refuse, {}",
        describe(status)
    );
    assert!(
        second
            .lines()
            .iter()
            .any(|line| line.contains("another process holds the state directory")),
        "{:#?}",
        second.lines()
    );

    first.assert_running();
    first.shutdown_cleanly();
}

#[test]
fn init_identity_twice_keeps_the_key_and_exits_non_zero() {
    let tree = Tree::new("inittwice");
    let first = tree.init();
    assert!(first.status.success(), "{first:?}");
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(stdout.contains("server_id="), "{stdout}");
    let before = tree.snapshot();

    let second = tree.init();
    assert_eq!(second.status.code(), Some(3), "{second:?}");
    assert_eq!(
        tree.snapshot(),
        before,
        "a second run must not change a byte"
    );
    let first_id = stdout.lines().find(|l| l.starts_with("server_id="));
    let second_stdout = String::from_utf8_lossy(&second.stdout);
    let second_id = second_stdout.lines().find(|l| l.starts_with("server_id="));
    assert_eq!(first_id, second_id);
}

#[test]
fn init_identity_output_contains_no_key_material() {
    let tree = Tree::new("initquiet");
    let output = tree.init();
    assert!(output.status.success());
    let key = std::fs::read(tree.etc().join("tls.key")).expect("key");
    let secrets = std::fs::read(tree.etc().join("secrets.key")).expect("secrets");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let secrets_hex = hex(&secrets);
    assert!(!text.contains(&secrets_hex), "secrets.key leaked");
    // No 16-byte window of the key's encoding may appear in the output. The
    // printed values are a random server_id, a SHA-256 pin and a random
    // serial, none of which are derived byte-for-byte from the key.
    let key_hex = hex(&key);
    for start in (0..=key_hex.len() - 32).step_by(2) {
        let window = &key_hex[start..start + 32];
        assert!(!text.contains(window), "key material leaked: {window}");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
