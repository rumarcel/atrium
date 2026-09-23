//! `RuntimeProbe`: is there a container runtime socket, and does it accept a
//! connection?
//!
//! This is the **only** file in the server workspace that names a container
//! runtime socket path, and `server/ci/boundary-checks.sh` fails the build if
//! one appears anywhere else. It is not container management. For each path
//! in a compiled-in list, Agent:
//!
//! 1. `lstat`s the path and continues only if it is a socket — not a
//!    symbolic link, not a regular file;
//! 2. `connect`s with a short timeout;
//! 3. closes the connection **without sending a single byte**.
//!
//! Nothing is read from the runtime and nothing is written to it, so no
//! runtime API is spoken, parsed or proxied, and no client library is linked.
//! The result names candidates by enum, never by path, and no handle leaves
//! this module. Core cannot supply, choose or influence a path: the operation
//! has no parameter, and the list below is a `const`.
//!
//! The candidate directories (`/run`, `/var/run`, `/run/podman`) are writable
//! only by root, so the gap between `lstat` and `connect` is not reachable by
//! the `atrium` user.

use std::collections::HashSet;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;
use std::time::Duration;

use atrium_protocol::messages::{NotProbed, RuntimeProbe, RuntimeSocket, VersionReason};

/// The candidates, in probe order. A compiled-in constant: Core cannot add,
/// remove or reorder one.
pub const CANDIDATES: [(RuntimeSocket, &str); 3] = [
    (RuntimeSocket::DockerVarRun, "/var/run/docker.sock"),
    (RuntimeSocket::DockerRun, "/run/docker.sock"),
    (RuntimeSocket::PodmanRun, "/run/podman/podman.sock"),
];

/// How long one `connect` may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// Probes [`CANDIDATES`].
pub async fn probe() -> RuntimeProbe {
    let candidates = CANDIDATES.map(|(name, path)| (name, Path::new(path)));
    probe_paths(&candidates).await
}

/// One candidate's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Found {
    socket: RuntimeSocket,
    reachable: bool,
}

/// Probes an explicit list. Private to the crate: the operation always calls
/// [`probe`], and only this module's tests pass anything else.
pub(crate) async fn probe_paths(candidates: &[(RuntimeSocket, &Path)]) -> RuntimeProbe {
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut found: Vec<Found> = Vec::new();
    for (socket, path) in candidates {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if !metadata.file_type().is_socket() {
            continue;
        }
        // `/var/run` is usually a link to `/run`, so two candidates can name
        // one socket. Count it once, under the first name.
        if !seen.insert((metadata.dev(), metadata.ino())) {
            continue;
        }
        let reachable = matches!(
            tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::UnixStream::connect(path)).await,
            Ok(Ok(_))
        );
        // The stream, if any, was dropped above: closed, with nothing sent.
        found.push(Found {
            socket: *socket,
            reachable,
        });
    }

    let selected = found
        .iter()
        .find(|candidate| candidate.reachable)
        .or_else(|| found.first())
        .copied();
    let also_present = found
        .iter()
        .filter(|candidate| Some(candidate.socket) != selected.map(|s| s.socket))
        .map(|candidate| candidate.socket)
        .collect();

    RuntimeProbe {
        runtime: selected.map(|s| s.socket.runtime()),
        socket: selected.map(|s| s.socket),
        reachable: selected.is_some_and(|s| s.reachable),
        version: NotProbed,
        version_reason: VersionReason::RuntimeVersionProbeNotInM1,
        also_present,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_protocol::messages::Runtime;
    use std::io::Read;
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

    #[tokio::test]
    async fn no_candidate_present() {
        let dir = Dir::new("absent");
        let a = dir.at("a.sock");
        let result = probe_paths(&[(RuntimeSocket::DockerVarRun, &a)]).await;
        assert_eq!(result.runtime, None);
        assert_eq!(result.socket, None);
        assert!(!result.reachable);
        assert!(result.also_present.is_empty());
    }

    #[tokio::test]
    async fn a_reachable_socket_is_selected_and_receives_nothing() {
        let dir = Dir::new("reachable");
        let path = dir.at("docker.sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let result = probe_paths(&[(RuntimeSocket::DockerVarRun, &path)]).await;
        assert_eq!(result.runtime, Some(Runtime::Docker));
        assert_eq!(result.socket, Some(RuntimeSocket::DockerVarRun));
        assert!(result.reachable);

        // The probe connected and closed: the runtime side reads end-of-file
        // at once, never a byte.
        let (mut accepted, _) = listener.accept().expect("the probe connected");
        let mut buffer = Vec::new();
        accepted
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let read = accepted.read_to_end(&mut buffer).expect("read");
        assert_eq!(read, 0, "the probe must send nothing");
    }

    #[tokio::test]
    async fn a_socket_nobody_listens_on_is_present_but_unreachable() {
        let dir = Dir::new("stale");
        let path = dir.at("docker.sock");
        drop(UnixListener::bind(&path).expect("bind"));
        let result = probe_paths(&[(RuntimeSocket::DockerRun, &path)]).await;
        assert_eq!(result.socket, Some(RuntimeSocket::DockerRun));
        assert!(!result.reachable);
    }

    #[tokio::test]
    async fn a_regular_file_or_a_symlink_is_not_a_runtime() {
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
        ])
        .await;
        assert_eq!(result.socket, None);
        assert!(result.also_present.is_empty());
    }

    #[tokio::test]
    async fn several_candidates_select_the_first_reachable_and_list_the_rest() {
        let dir = Dir::new("several");
        let stale = dir.at("stale.sock");
        drop(UnixListener::bind(&stale).expect("bind"));
        let podman = dir.at("podman.sock");
        let _live = UnixListener::bind(&podman).expect("bind");
        let result = probe_paths(&[
            (RuntimeSocket::DockerVarRun, &stale),
            (RuntimeSocket::PodmanRun, &podman),
        ])
        .await;
        assert_eq!(result.socket, Some(RuntimeSocket::PodmanRun));
        assert_eq!(result.runtime, Some(Runtime::Podman));
        assert!(result.reachable);
        assert_eq!(result.also_present, vec![RuntimeSocket::DockerVarRun]);
    }

    #[tokio::test]
    async fn two_names_for_one_socket_count_once() {
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
        ])
        .await;
        assert_eq!(result.socket, Some(RuntimeSocket::DockerVarRun));
        assert!(result.also_present.is_empty());
    }
}
