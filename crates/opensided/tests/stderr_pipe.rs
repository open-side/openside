//! A daemon whose stderr reader has gone away must keep running and exit normally.
//!
//! `eprintln!` panics on EPIPE; a panic while the default panic hook (or ours) reports on the
//! same dead stderr aborts the process with SIGABRT.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn closed_stderr_pipe_does_not_abort_the_daemon() {
    let root = tempfile::tempdir().unwrap();
    let empty_path = root.path().join("empty-path");
    std::fs::create_dir(&empty_path).unwrap();
    // tempfile directories follow the umask; the daemon requires a private socket parent.
    let run = root.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = run.join("opensided.sock");

    // The write end is the child's stderr; the read end is closed before the child starts, so
    // every diagnostic write fails with EPIPE.
    let (reader, writer) = nix::unistd::pipe().unwrap();
    drop(reader);

    let mut child = Command::new(env!("CARGO_BIN_EXE_opensided"))
        .arg("--data-dir")
        .arg(root.path().join("data"))
        .arg("--socket")
        .arg(&socket)
        .env_clear()
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("xdg-data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        // No `uv` on PATH makes startup print an agent-discovery notice to stderr.
        .env("PATH", &empty_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while !socket.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("daemon exited before serving: {status:?}");
        }
        assert!(Instant::now() < deadline, "daemon never created its socket");
        std::thread::sleep(Duration::from_millis(20));
    }

    let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("daemon did not shut down after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        status.signal(),
        None,
        "daemon was killed by a signal: {status:?}"
    );
    assert!(status.success(), "daemon exit status: {status:?}");
}
