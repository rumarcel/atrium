//! Static policy tests for what Atrium will install on a machine.
//!
//! These live in `atriumctl` because it is the console tool that owns
//! deployment concerns, and because it is dependency-free and therefore builds
//! everywhere — including on the maintainer's Windows workstation, where the
//! Linux server binaries do not compile. Keeping the policy checks portable
//! means they run on every `cargo test`, not only in CI.
//!
//! **What these prove and what they do not.** They prove the *shape* of the
//! service assets and the documented expectations: directive present, value
//! correct, forbidden string absent. They prove nothing about how systemd or
//! the kernel behaves at runtime. The operating-system properties — that Core
//! really runs as `atrium`, that the identity key really cannot be written,
//! that the socket really has mode 0660 — belong to the privileged and VM test
//! layers in `docs/M1-TEST-PLAN.md`, and are not claimed here.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the server workspace root must resolve")
}

fn repository_root() -> PathBuf {
    workspace_root()
        .join("..")
        .canonicalize()
        .expect("the repository root must resolve")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn unit(name: &str) -> String {
    read(
        &workspace_root()
            .join("packaging")
            .join("systemd")
            .join(name),
    )
}

/// Directive lines, with comments and blanks removed, so a value that only
/// appears inside a comment cannot satisfy an assertion.
fn directives(unit_text: &str) -> Vec<&str> {
    unit_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('['))
        .collect()
}

fn assert_directive(unit_text: &str, expected: &str, unit_name: &str) {
    assert!(
        directives(unit_text).contains(&expected),
        "{unit_name} must contain the directive `{expected}`"
    );
}

#[test]
fn core_unit_runs_unprivileged_with_no_supplementary_groups() {
    let text = unit("atrium-core.service");
    assert_directive(&text, "User=atrium", "atrium-core.service");
    assert_directive(&text, "Group=atrium", "atrium-core.service");
    // Criterion 34: the empty assignment is what clears the group list. An
    // absent directive would inherit the user's groups instead.
    assert_directive(&text, "SupplementaryGroups=", "atrium-core.service");
    assert_directive(&text, "NoNewPrivileges=true", "atrium-core.service");
    assert_directive(&text, "CapabilityBoundingSet=", "atrium-core.service");
    assert_directive(&text, "ProtectSystem=strict", "atrium-core.service");
    assert_directive(&text, "MemoryDenyWriteExecute=true", "atrium-core.service");
    assert_directive(&text, "RestrictSUIDSGID=true", "atrium-core.service");

    assert!(
        !directives(&text)
            .iter()
            .any(|line| line.starts_with("User=root")),
        "atrium-core.service must never run as root"
    );
}

#[test]
fn core_unit_treats_the_identity_directory_as_read_only() {
    let text = unit("atrium-core.service");
    let read_only = directives(&text)
        .into_iter()
        .find(|line| line.starts_with("ReadOnlyPaths="))
        .expect("atrium-core.service must declare ReadOnlyPaths");
    assert!(
        read_only.contains("/etc/atrium"),
        "the identity directory must be read-only to Core as defence in depth: {read_only}"
    );
    assert!(
        !directives(&text)
            .iter()
            .any(|line| line.starts_with("ReadWritePaths=") && line.contains("/etc/atrium")),
        "nothing may make /etc/atrium writable to Core"
    );
}

#[test]
fn core_unit_does_not_hard_depend_on_the_agent() {
    // Criterion 29: reads must keep working when Agent is stopped. A Requires=
    // here would stop Core along with it.
    let text = unit("atrium-core.service");
    assert!(
        !directives(&text)
            .iter()
            .any(|line| line.starts_with("Requires=") && line.contains("atrium-agent")),
        "atrium-core.service must Want, not Require, atrium-agent.socket"
    );
    assert!(
        directives(&text)
            .iter()
            .any(|line| line.starts_with("Wants=") && line.contains("atrium-agent.socket")),
        "atrium-core.service should Want atrium-agent.socket"
    );
}

#[test]
fn agent_unit_is_root_with_no_capabilities_and_no_network() {
    let text = unit("atrium-agent.service");
    assert_directive(&text, "User=root", "atrium-agent.service");
    assert_directive(&text, "NoNewPrivileges=true", "atrium-agent.service");
    // M1A's Agent has no privileged action, so it needs no capability at all.
    assert_directive(&text, "CapabilityBoundingSet=", "atrium-agent.service");
    // Together these make a listening TCP socket impossible, not merely absent.
    assert_directive(&text, "PrivateNetwork=true", "atrium-agent.service");
    assert_directive(
        &text,
        "RestrictAddressFamilies=AF_UNIX",
        "atrium-agent.service",
    );
}

#[test]
fn agent_socket_declares_its_own_permissions() {
    let text = unit("atrium-agent.socket");
    assert_directive(
        &text,
        "ListenStream=/run/atrium/agent.sock",
        "atrium-agent.socket",
    );
    assert_directive(&text, "SocketUser=root", "atrium-agent.socket");
    assert_directive(&text, "SocketGroup=atrium", "atrium-agent.socket");
    assert_directive(&text, "SocketMode=0660", "atrium-agent.socket");
    assert_directive(&text, "Accept=no", "atrium-agent.socket");
}

#[test]
fn units_stay_inside_the_atrium_namespace() {
    // ADR-015: the canary must not reuse a single prototype identifier.
    const RESERVED: [&str; 6] = [
        "personal-hub",
        "personalhub",
        "9473",
        "/opt/personal-hub-agent",
        "/etc/personal-hub-agent",
        "/var/lib/personal-hub-agent",
    ];
    for name in [
        "atrium-core.service",
        "atrium-agent.service",
        "atrium-agent.socket",
    ] {
        let text = unit(name);
        for reserved in RESERVED {
            assert!(
                !text.contains(reserved),
                "{name} references the reserved prototype identifier {reserved}"
            );
        }
    }
}

#[test]
fn units_reference_no_container_runtime() {
    for name in [
        "atrium-core.service",
        "atrium-agent.service",
        "atrium-agent.socket",
    ] {
        // Directives only. A comment may name docker — the Core unit's
        // `SupplementaryGroups=` is empty precisely so that Core is not in the
        // docker group, and saying so is the point of the comment. What must
        // not happen is a *directive* granting runtime access.
        let text = unit(name);
        for directive in directives(&text) {
            let lowered = directive.to_ascii_lowercase();
            for forbidden in ["docker", "podman", "containerd"] {
                assert!(
                    !lowered.contains(forbidden),
                    "{name} has a directive referencing {forbidden}: {directive}"
                );
            }
        }
    }
}

#[test]
fn packaging_installs_no_sudoers_file() {
    let packaging = workspace_root().join("packaging");
    let mut found = Vec::new();
    visit(&packaging, &mut |path| {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().contains("sudoers"))
        {
            found.push(path.to_path_buf());
        }
    });
    assert!(
        found.is_empty(),
        "ADR-004 removed sudo entirely; found {found:?}"
    );
}

#[test]
fn documented_identity_permissions_are_traversable_by_the_service_user() {
    // The invariant this guards: /etc/atrium must be group-owned by atrium, or
    // the service user cannot traverse it and cannot read the key it is
    // supposed to read. A root:root directory would deny the read the design
    // depends on, which is the correction made before M1A.
    let plan = read(
        &repository_root()
            .join("docs")
            .join("M1-IMPLEMENTATION-PLAN.md"),
    );
    assert!(
        plan.contains("| `/etc/atrium/` | `root:atrium` | `0750` |"),
        "the plan must specify /etc/atrium as root:atrium 0750"
    );
    assert!(
        plan.contains("| `/etc/atrium/tls.key` | `root:atrium` | `0640` |"),
        "the plan must specify tls.key as root:atrium 0640"
    );
    for sensitive in ["identity.json", "secrets.key"] {
        assert!(
            plan.contains(&format!(
                "| `/etc/atrium/{sensitive}` | `root:atrium` | `0640` |"
            )),
            "the plan must specify {sensitive} as root:atrium 0640"
        );
    }
    assert!(
        !plan.contains("| `/etc/atrium/` | `root:root` |"),
        "a root:root identity directory would make the key unreadable to Core"
    );
}

#[test]
fn the_desktop_workspace_is_not_coupled_to_the_server_workspace() {
    // The two workspaces are independent by decision. If src-tauri ever gains a
    // path dependency on a server crate it must be a deliberate, reviewed step
    // in M1G — not something that arrives by accident in an earlier pass.
    let manifest = read(&repository_root().join("src-tauri").join("Cargo.toml"));
    assert!(
        !manifest.contains("atrium-client"),
        "src-tauri must not depend on atrium-client until M1G"
    );
    assert!(
        !manifest.contains("../server"),
        "src-tauri must not path-depend on the server workspace in M1A"
    );
    assert!(
        !manifest.contains("[workspace]"),
        "src-tauri must stay a standalone package, not become a workspace member"
    );
}

#[test]
fn the_server_workspace_does_not_adopt_the_repository_root() {
    // Creating a root Cargo.toml would move the Tauri target directory and
    // break the Windows CI cache key.
    assert!(
        !repository_root().join("Cargo.toml").exists(),
        "the repository root must not become a Cargo workspace"
    );
    assert!(
        workspace_root().join("Cargo.lock").exists(),
        "the server workspace must carry its own lockfile"
    );
    assert!(
        repository_root()
            .join("src-tauri")
            .join("Cargo.lock")
            .exists(),
        "the desktop application must keep its own lockfile"
    );
}

fn visit(directory: &Path, visitor: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, visitor);
        } else {
            visitor(&path);
        }
    }
}
