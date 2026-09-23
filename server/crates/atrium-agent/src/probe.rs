//! `RuntimeProbe`: which known container runtime sockets exist?
//!
//! This is the **only** file in the server workspace that names a container
//! runtime socket path, and `server/ci/boundary-checks.sh` fails the build if
//! one appears anywhere else. It is not container management, and it is
//! **passive**. For each path in a compiled-in list, Agent `lstat`s the path
//! and counts it only if it is a socket — not a symbolic link, not a regular
//! file. That is all.
//!
//! The probe never `connect`s. A connection, even one that sends nothing,
//! makes systemd start a socket-activated Docker or Podman daemon, and Atrium
//! must not wake a stopped runtime because someone looked at a status page.
//! Metadata does not activate anything. The consequence is stated on the
//! wire rather than hidden: the answer says which sockets are **present**,
//! and `liveness` is `null` with a reason. M1 cannot tell whether a runtime
//! is running, healthy or able to answer, and does not claim to.
//!
//! Nothing is read from or written to a runtime, no runtime API is spoken,
//! and no client library is linked. The result names candidates by enum,
//! never by path, and no handle of any kind exists. Core cannot supply,
//! choose or influence a path: the operation has no parameter, and the list
//! below is a `const`.

use std::collections::HashSet;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

use atrium_protocol::messages::{
    LivenessReason, NotProbed, RuntimeProbe, RuntimeSocket, VersionReason,
};

/// The candidates, in probe order. A compiled-in constant: Core cannot add,
/// remove or reorder one.
pub const CANDIDATES: [(RuntimeSocket, &str); 3] = [
    (RuntimeSocket::DockerVarRun, "/var/run/docker.sock"),
    (RuntimeSocket::DockerRun, "/run/docker.sock"),
    (RuntimeSocket::PodmanRun, "/run/podman/podman.sock"),
];

/// Probes [`CANDIDATES`].
#[must_use]
pub fn probe() -> RuntimeProbe {
    let candidates = CANDIDATES.map(|(name, path)| (name, Path::new(path)));
    probe_paths(&candidates)
}

/// Probes an explicit list. The operation always calls [`probe`], with the
/// constant list; this exists so tests can point the same logic at sockets
/// they create. It is reachable only by Rust code linking this crate — never
/// through the protocol, whose operations take no parameter, and never from
/// Core, which a boundary gate forbids from depending on Agent.
#[doc(hidden)]
#[must_use]
pub fn probe_paths(candidates: &[(RuntimeSocket, &Path)]) -> RuntimeProbe {
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut present: Vec<RuntimeSocket> = Vec::new();
    for (socket, path) in candidates {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if !metadata.file_type().is_socket() {
            continue;
        }
        // `/var/run` is usually a link to `/run`, so two candidates can name
        // one socket. Count it once, under the first name.
        if seen.insert((metadata.dev(), metadata.ino())) {
            present.push(*socket);
        }
    }

    let selected = present.first().copied();
    RuntimeProbe {
        runtime: selected.map(RuntimeSocket::runtime),
        socket: selected,
        also_present: present.into_iter().skip(1).collect(),
        liveness: NotProbed,
        liveness_reason: LivenessReason::PassiveProbeInM1,
        version: NotProbed,
        version_reason: VersionReason::RuntimeVersionProbeNotInM1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_protocol::messages::Runtime;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("atrium-agent-probe-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("dir");
            Self(path)
        }

        fn at(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn candidates_are_the_planned_list() {
        let names: Vec<RuntimeSocket> = CANDIDATES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, RuntimeSocket::ALL);
        for (_, path) in CANDIDATES {
            assert!(path.starts_with("/run/") || path.starts_with("/var/run/"));
        }
    }

    #[test]
    fn no_candidate_present() {
        let dir = Dir::new("absent");
        let a = dir.at("a.sock");
        let result = probe_paths(&[(RuntimeSocket::DockerVarRun, &a)]);
        assert_eq!(result.runtime, None);
        assert_eq!(result.socket, None);
        assert!(result.also_present.is_empty());
        assert_eq!(result.liveness_reason, LivenessReason::PassiveProbeInM1);
    }

    #[test]
    fn a_listening_socket_is_present_and_is_never_connected_to() {
        let dir = Dir::new("listening");
        let path = dir.at("docker.sock");
        let listener = UnixListener::bind(&path).expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");

        let result = probe_paths(&[(RuntimeSocket::DockerVarRun, &path)]);
        assert_eq!(result.runtime, Some(Runtime::Docker));
        assert_eq!(result.socket, Some(RuntimeSocket::DockerVarRun));

        // Nothing is waiting in the backlog: the probe did not connect.
        match listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("the probe connected to the runtime socket: {other:?}"),
        }
    }

    #[test]
    fn a_stale_socket_counts_as_present_without_any_claim_of_liveness() {
        let dir = Dir::new("stale");
        let path = dir.at("docker.sock");
        drop(UnixListener::bind(&path).expect("bind"));
        let result = probe_paths(&[(RuntimeSocket::DockerRun, &path)]);
        assert_eq!(result.socket, Some(RuntimeSocket::DockerRun));
        let text = serde_json::to_string(&result).expect("encode");
        assert!(text.contains("\"liveness\":null"), "{text}");
        assert!(!text.contains("reachable"), "{text}");
    }

    #[test]
    fn a_regular_file_or_a_symlink_is_not_a_runtime() {
        let dir = Dir::new("impostor");
        let file = dir.at("file.sock");
        std::fs::write(&file, b"").expect("file");
        let real = dir.at("real.sock");
        let _listener = UnixListener::bind(&real).expect("bind");
        let link = dir.at("link.sock");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        let result = probe_paths(&[
            (RuntimeSocket::DockerVarRun, &file),
            (RuntimeSocket::DockerRun, &link),
        ]);
        assert_eq!(result.socket, None);
        assert!(result.also_present.is_empty());
    }

    #[test]
    fn several_candidates_select_the_first_present_and_list_the_rest() {
        let dir = Dir::new("several");
        let docker = dir.at("docker.sock");
        drop(UnixListener::bind(&docker).expect("bind"));
        let podman = dir.at("podman.sock");
        let _live = UnixListener::bind(&podman).expect("bind");
        let result = probe_paths(&[
            (RuntimeSocket::DockerVarRun, &docker),
            (RuntimeSocket::PodmanRun, &podman),
        ]);
        assert_eq!(result.socket, Some(RuntimeSocket::DockerVarRun));
        assert_eq!(result.runtime, Some(Runtime::Docker));
        assert_eq!(result.also_present, vec![RuntimeSocket::PodmanRun]);
    }

    #[test]
    fn two_names_for_one_socket_count_once() {
        let dir = Dir::new("alias");
        let real_dir = dir.at("run");
        std::fs::create_dir(&real_dir).expect("run");
        let socket = real_dir.join("docker.sock");
        let _live = UnixListener::bind(&socket).expect("bind");
        let alias_dir = dir.at("var-run");
        std::os::unix::fs::symlink(&real_dir, &alias_dir).expect("alias");
        let via_alias = alias_dir.join("docker.sock");
        let result = probe_paths(&[
            (RuntimeSocket::DockerVarRun, &via_alias),
            (RuntimeSocket::DockerRun, &socket),
        ]);
        assert_eq!(result.socket, Some(RuntimeSocket::DockerVarRun));
        assert!(result.also_present.is_empty());
    }
}
