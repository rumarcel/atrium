//! The passive runtime probe against real socket activation.
//!
//! `systemd-socket-activate` holds a listening socket and starts its program
//! only when something connects — exactly what Docker's socket unit does for
//! `dockerd`. If the probe connected, it would wake the runtime. It must not.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use atrium_agent::probe::probe_paths;
use atrium_protocol::messages::RuntimeSocket;

const SOCKET_ACTIVATE: &str = "/usr/bin/systemd-socket-activate";

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn probing_does_not_activate_a_socket_activated_service() {
    assert!(
        Path::new(SOCKET_ACTIVATE).exists(),
        "{SOCKET_ACTIVATE} is required: this property is proven against real socket \
         activation, not a mock"
    );
    let dir =
        Dir(std::env::temp_dir().join(format!("atrium-agent-activation-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    std::fs::create_dir_all(&dir.0).expect("dir");
    let socket = dir.0.join("runtime.sock");
    let marker = dir.0.join("activated");
    let mut activator = Command::new(SOCKET_ACTIVATE)
        .arg("--listen")
        .arg(&socket)
        .arg("/usr/bin/touch")
        .arg(&marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn systemd-socket-activate");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline,
            "the activation socket never appeared"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    for _ in 0..5 {
        let result = probe_paths(&[(RuntimeSocket::DockerVarRun, &socket)]);
        assert_eq!(result.socket, Some(RuntimeSocket::DockerVarRun), "present");
    }
    std::thread::sleep(Duration::from_millis(500));
    let activated = marker.exists();

    // Control: a real connection does activate it, so the check above can
    // fail.
    drop(std::os::unix::net::UnixStream::connect(&socket).expect("connect"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let control = marker.exists();
    let _ = activator.kill();
    let _ = activator.wait();

    assert!(!activated, "the probe activated a socket-activated service");
    assert!(control, "the control connection should have activated it");
}
