use super::*;
use crate::tests::{spec, Echo, Peer};
use nix::errno::Errno;
use nix::sys::signal::kill;
use std::path::{Path, PathBuf};
use tokio::net::UnixStream;

const TEST_TIMEOUT: Duration = Duration::from_secs(4);

fn config(directory: &Path, script: &str) -> SidecarConfig {
    SidecarConfig {
        argv: vec!["sh".into(), "-c".into(), script.into()],
        runtime_dir: directory.join("runtime"),
        startup_timeout: Duration::from_secs(2),
        env: Vec::new(),
    }
}

async fn wait_file(path: &Path) -> String {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                if text.ends_with('\n') {
                    return text;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

async fn gone(pid: Pid) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if matches!(kill(pid, None), Err(Errno::ESRCH)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("pid {pid} remains after process-group termination"));
}

async fn open_shell(script_tail: &str) -> (SidecarSession, Peer, TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let handshake = temp.path().join("handshake");
    let exit = temp.path().join("exit");
    // Write then rename: a shell printf may emit one line per write, so a reader polling the
    // final path could otherwise see only the first line.
    let script = format!("printf '%s\\n%s\\n%s\\n' \"$OPENSIDE_SIDECAR_SOCKET\" \"$OPENSIDE_SIDECAR_NONCE\" \"$$\" > \"$HANDSHAKE.tmp\" && mv \"$HANDSHAKE.tmp\" \"$HANDSHAKE\"; {script_tail}");
    let mut settings = config(temp.path(), &script);
    settings.env = vec![
        ("HANDSHAKE".into(), handshake.to_string_lossy().into_owned()),
        ("EXITFILE".into(), exit.to_string_lossy().into_owned()),
        ("OPENSIDE_SIDECAR_NONCE".into(), "must-be-overridden".into()),
    ];
    let runtime = SidecarRuntime::new(settings);
    let handshake_peer = tokio::spawn(async move {
        let info = wait_file(&handshake).await;
        let mut fields = info.lines();
        let socket = PathBuf::from(fields.next().unwrap());
        let nonce = fields.next().unwrap();
        assert_eq!(nonce.len(), 32);
        assert!(nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_ne!(nonce, "must-be-overridden");
        assert_eq!(
            fs::metadata(socket.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let stream = UnixStream::connect(&socket).await.unwrap();
        let mut peer = Peer::new(stream);
        peer.hello(nonce).await;
        assert_eq!(peer.recv().await["type"], "open");
        assert!(!socket.exists());
        tokio::time::sleep(Duration::from_millis(30)).await;
        peer.send(serde_json::json!({"v":1,"type":"ready","tools":["echo"],"pending_deferred":[],"startup_ms":999999})).await;
        peer
    });
    let session = runtime.open_session(spec(), Arc::new(Echo)).await.unwrap();
    let peer = handshake_peer.await.unwrap();
    (session, peer, temp, exit)
}

#[tokio::test]
async fn startup_timeout_kills_and_reaps_a_sidecar_that_never_connects() {
    let temp = tempfile::tempdir().unwrap();
    let pidfile = temp.path().join("pid");
    let mut settings = config(
        temp.path(),
        "printf '%s\\n' \"$$\" > \"$PIDFILE\"; exec sleep 30",
    );
    settings.startup_timeout = Duration::from_millis(100);
    settings
        .env
        .push(("PIDFILE".into(), pidfile.to_string_lossy().into_owned()));
    let runtime_dir = settings.runtime_dir.clone();
    let result = SidecarRuntime::new(settings)
        .open_session(spec(), Arc::new(Echo))
        .await;
    assert!(
        matches!(result, Err(RuntimeError::Unavailable(message)) if message == "sidecar startup timed out")
    );
    let pid = Pid::from_raw(wait_file(&pidfile).await.trim().parse().unwrap());
    gone(pid).await;
    assert_eq!(fs::read_dir(runtime_dir).unwrap().count(), 0);
}

#[tokio::test]
async fn startup_detects_exit_without_waiting_for_the_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = config(temp.path(), "printf 'startup diagnostic\\n' >&2; exit 7");
    settings.startup_timeout = Duration::from_secs(10);
    let started = Instant::now();
    let result = SidecarRuntime::new(settings)
        .open_session(spec(), Arc::new(Echo))
        .await;
    assert!(
        matches!(result, Err(RuntimeError::Unavailable(message)) if message == "sidecar exited during startup")
    );
    assert!(started.elapsed() < TEST_TIMEOUT);
}

#[tokio::test]
async fn stderr_retains_only_64_lines_of_at_most_2_kib() {
    let temp = tempfile::tempdir().unwrap();
    let settings = config(temp.path(), "i=0; while [ \"$i\" -lt 80 ]; do printf 'line-%s\\n' \"$i\" >&2; i=$((i+1)); done; i=0; while [ \"$i\" -lt 3000 ]; do printf x >&2; i=$((i+1)); done; printf '\\nlast-line' >&2");
    let (_listener, mut process, _) = spawn(&settings).unwrap();
    process.child.wait().await.unwrap();
    process.finish().await;
    let lines = process.stderr.lock().iter().cloned().collect::<Vec<_>>();
    assert_eq!(lines.len(), 64);
    assert_eq!(lines[0], "line-18");
    assert_eq!(lines[62].len(), 2048);
    assert_eq!(lines[63], "last-line");
    assert!(lines.iter().all(|line| line.len() <= 2048));
}

#[tokio::test]
async fn stderr_invalid_utf8_never_expands_past_the_byte_limit() {
    let temp = tempfile::tempdir().unwrap();
    let settings = config(
        temp.path(),
        "i=0; while [ \"$i\" -lt 3000 ]; do printf '\\377' >&2; i=$((i+1)); done; printf '\\n' >&2",
    );
    let (_listener, mut process, _) = spawn(&settings).unwrap();
    process.child.wait().await.unwrap();
    process.finish().await;
    let lines = process.stderr.lock();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].len() <= 2048);
    assert!(lines[0].contains('\u{fffd}'));
}

#[tokio::test]
async fn private_socket_nonce_and_host_startup_measurement_work_with_a_shell_child() {
    let (mut session, peer, _temp, _exit) = open_shell("exec sleep 30").await;
    assert!(session.info().startup_ms >= 30);
    assert!(session.info().startup_ms < 999999);
    let pid = session.process.group.unwrap();
    session.process.finish().await;
    gone(pid).await;
    drop(peer);
    assert!(tokio::time::timeout(TEST_TIMEOUT, session.next_event())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn orderly_shutdown_observes_bye_and_reaps_the_process() {
    let (mut session, mut peer, _temp, exit) =
        open_shell("while [ ! -e \"$EXITFILE\" ]; do sleep 0.01; done; exit 0").await;
    let pid = session.process.group.unwrap();
    let farewell = tokio::spawn(async move {
        assert_eq!(peer.recv().await["type"], "shutdown");
        peer.send(serde_json::json!({"v":1,"type":"state_snapshot","state":{"format":"openside-agent-state/1","data":"final-state"}})).await;
        peer.send(serde_json::json!({"v":1,"type":"bye"})).await;
        fs::write(exit, "exit").unwrap();
    });
    tokio::time::timeout(TEST_TIMEOUT, session.shutdown())
        .await
        .unwrap()
        .unwrap();
    farewell.await.unwrap();
    gone(pid).await;
    assert!(
        matches!(session.next_event().await, Some(RuntimeEvent::StateSnapshot { state }) if state.data == "final-state")
    );
    session.shutdown().await.unwrap();
    assert!(session.next_event().await.is_none());
}

#[tokio::test]
async fn shutdown_kills_an_uncooperative_process_group_after_five_seconds() {
    let (mut session, mut peer, temp, _exit) =
        open_shell("trap '' CHLD; sleep 30 & printf '%s\\n' \"$!\" > \"$HANDSHAKE.child\"; wait")
            .await;
    let pid = session.process.group.unwrap();
    let child = Pid::from_raw(
        wait_file(&temp.path().join("handshake.child"))
            .await
            .trim()
            .parse()
            .unwrap(),
    );
    let command = tokio::spawn(async move {
        assert_eq!(peer.recv().await["type"], "shutdown");
        peer
    });
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(7), session.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(started.elapsed() >= SHUTDOWN_TIMEOUT);
    let _peer = command.await.unwrap();
    gone(pid).await;
    gone(child).await;
}

#[tokio::test]
async fn dropping_a_session_kills_the_process_group() {
    let (session, _peer, _temp, _exit) = open_shell("exec sleep 30").await;
    let pid = session.process.group.unwrap();
    drop(session);
    gone(pid).await;
}

#[tokio::test]
async fn dropping_the_process_guard_kills_descendants_as_well_as_the_parent() {
    let temp = tempfile::tempdir().unwrap();
    let pidfile = temp.path().join("pids");
    let mut settings = config(
        temp.path(),
        "trap '' CHLD; sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PIDFILE\"; wait",
    );
    settings
        .env
        .push(("PIDFILE".into(), pidfile.to_string_lossy().into_owned()));
    let (_listener, process, _) = spawn(&settings).unwrap();
    let pids = wait_file(&pidfile)
        .await
        .split_whitespace()
        .map(|value| Pid::from_raw(value.parse().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(pids.len(), 2);
    drop(process);
    for pid in pids {
        gone(pid).await;
    }
}

#[tokio::test]
async fn empty_and_missing_commands_fail_without_leaving_socket_directories() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = config(temp.path(), "exit 0");
    settings.argv.clear();
    assert!(matches!(
        spawn(&settings),
        Err(RuntimeError::Unavailable(_))
    ));
    settings.argv = vec![temp
        .path()
        .join("nonexistent")
        .to_string_lossy()
        .into_owned()];
    assert!(matches!(
        spawn(&settings),
        Err(RuntimeError::Unavailable(_))
    ));
    assert_eq!(fs::read_dir(settings.runtime_dir).unwrap().count(), 0);
}

#[tokio::test]
async fn exit_preserves_the_preceding_authentication_error_and_snapshot() {
    let (mut session, mut peer, _temp, _exit) = open_shell("exec sleep 30").await;
    peer.send(serde_json::json!({"v":1,"type":"state_snapshot","state":{"format":"openside-agent-state/1","data":"before-exit"}})).await;
    peer.send(serde_json::json!({"v":1,"type":"error","code":"auth_unavailable","message":"configured sources unavailable","fatal":true})).await;
    session.process.finish().await;
    drop(peer);
    assert!(
        matches!(session.next_event().await, Some(RuntimeEvent::StateSnapshot { state }) if state.data == "before-exit")
    );
    assert_eq!(
        session.next_event().await,
        Some(RuntimeEvent::Error {
            error: RuntimeError::AuthUnavailable("configured sources unavailable".into()),
        })
    );
    assert!(session.next_event().await.is_none());
}

#[tokio::test]
async fn exit_drain_is_bounded_when_another_process_keeps_the_socket_open() {
    let (mut session, _peer, _temp, _exit) = open_shell("exec sleep 30").await;
    session.process.finish().await;
    let started = Instant::now();
    assert!(
        tokio::time::timeout(Duration::from_secs(7), session.next_event())
            .await
            .unwrap()
            .is_none()
    );
    assert!(started.elapsed() >= SHUTDOWN_TIMEOUT);
}

#[tokio::test]
async fn startup_timeout_also_covers_a_connected_peer_that_never_sends_hello() {
    let temp = tempfile::tempdir().unwrap();
    let handshake = temp.path().join("handshake");
    let mut settings = config(
        temp.path(),
        "printf '%s\\n' \"$OPENSIDE_SIDECAR_SOCKET\" > \"$HANDSHAKE\"; exec sleep 30",
    );
    settings.startup_timeout = Duration::from_millis(150);
    settings
        .env
        .push(("HANDSHAKE".into(), handshake.to_string_lossy().into_owned()));
    let peer = tokio::spawn(async move {
        let socket = wait_file(&handshake).await;
        UnixStream::connect(socket.trim()).await.unwrap()
    });
    let result = SidecarRuntime::new(settings)
        .open_session(spec(), Arc::new(Echo))
        .await;
    let _peer = peer.await.unwrap();
    assert!(
        matches!(result, Err(RuntimeError::Unavailable(message)) if message == "sidecar startup timed out")
    );
}

#[tokio::test]
async fn stderr_tail_is_exposed_without_logging_or_implicit_formatting() {
    let (mut session, _peer, _temp, _exit) =
        open_shell("printf 'sanitized diagnostic\\n' >&2; exec sleep 30").await;
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if session.stderr_tail() == ["sanitized diagnostic"] {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    session.process.finish().await;
}

#[tokio::test]
async fn authentication_failure_at_open_wins_over_a_simultaneous_child_exit() {
    let temp = tempfile::tempdir().unwrap();
    let handshake = temp.path().join("handshake");
    let mut settings = config(temp.path(), "printf '%s\\n%s\\n%s\\n' \"$OPENSIDE_SIDECAR_SOCKET\" \"$OPENSIDE_SIDECAR_NONCE\" \"$$\" > \"$HANDSHAKE.tmp\" && mv \"$HANDSHAKE.tmp\" \"$HANDSHAKE\"; exec sleep 30");
    settings
        .env
        .push(("HANDSHAKE".into(), handshake.to_string_lossy().into_owned()));
    let peer = tokio::spawn(async move {
        let info = wait_file(&handshake).await;
        let mut fields = info.lines();
        let socket = fields.next().unwrap();
        let nonce = fields.next().unwrap();
        let pid = Pid::from_raw(fields.next().unwrap().parse().unwrap());
        let mut peer = Peer::new(UnixStream::connect(socket).await.unwrap());
        peer.hello(nonce).await;
        assert_eq!(peer.recv().await["type"], "open");
        peer.send(serde_json::json!({"v":1,"type":"error","code":"auth_unavailable","message":"configure credentials","fatal":true})).await;
        killpg(pid, Signal::SIGKILL).unwrap();
        peer
    });
    let result = SidecarRuntime::new(settings)
        .open_session(spec(), Arc::new(Echo))
        .await;
    let _peer = peer.await.unwrap();
    assert!(
        matches!(result, Err(RuntimeError::AuthUnavailable(message)) if message == "configure credentials")
    );
}

#[tokio::test]
async fn sidecar_children_disable_core_dumps_without_changing_parent_limits() {
    use nix::sys::resource::{getrlimit, Resource};
    let parent = getrlimit(Resource::RLIMIT_CORE).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let limits = temp.path().join("core-limits");
    let mut settings = config(temp.path(), "printf '%s\\n%s\\n' \"$(ulimit -S -c)\" \"$(ulimit -H -c)\" > \"$CORE_LIMITS.tmp\" && mv \"$CORE_LIMITS.tmp\" \"$CORE_LIMITS\"; exec sleep 30");
    settings
        .env
        .push(("CORE_LIMITS".into(), limits.to_string_lossy().into_owned()));
    let (_listener, mut process, _nonce) = spawn(&settings).unwrap();
    let observed = wait_file(&limits).await;
    process.finish().await;
    assert_eq!(observed, "0\n0\n");
    assert_eq!(getrlimit(Resource::RLIMIT_CORE).unwrap(), parent);
}

#[tokio::test]
async fn sidecar_child_never_inherits_a_daemon_credential_environment_variable() {
    const NAME: &str = "OPENSIDE_TEST_DAEMON_PROVIDER_KEY";
    std::env::set_var(NAME, "daemon-credential-env-canary-8461");
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("child-environment");
    let mut settings = config(temp.path(), "printf '%s\\n' \"${OPENSIDE_TEST_DAEMON_PROVIDER_KEY-unset}\" > \"$ENV_RESULT\"; exec sleep 30");
    settings
        .env
        .push(("ENV_RESULT".into(), output.to_string_lossy().into_owned()));
    let result = spawn(&settings);
    std::env::remove_var(NAME);
    let (_listener, mut process, _nonce) = result.unwrap();
    let actual = wait_file(&output).await;
    process.finish().await;
    assert_eq!(actual, "unset\n");
}
