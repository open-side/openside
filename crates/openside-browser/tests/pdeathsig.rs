//! Linux kernel supervision of launched Chromium: a spawner that dies without running
//! destructors (abort, SIGKILL, a harness timeout) must not leave the browser running.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
// This binary uses only `common::executable`; the shared helpers would warn as unused.
#[allow(dead_code)]
mod common;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use openside_browser::{Browser, BrowserConfig};

/// Set by the parent test when it re-runs this binary as the supervised child.
const HELPER: &str = "OPENSIDE_TEST_PDEATHSIG_HELPER";
/// `1` asks the helper for a detached launch; anything else launches normally.
const DETACHED: &str = "OPENSIDE_TEST_PDEATHSIG_DETACHED";
/// `1` asks the helper for `die_with_spawner` on top of a detached launch.
const DIE: &str = "OPENSIDE_TEST_PDEATHSIG_DIE_WITH_SPAWNER";

/// A process that exited is gone even while it waits to be reaped.
fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z')),
        Err(_) => false,
    }
}

/// A killed helper is still SIGKILLed on panic paths; std Command has no kill_on_drop.
struct HelperGuard(std::process::Child);

impl Drop for HelperGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn group_gone(profile: &Path) -> bool {
    let output = std::process::Command::new("pgrep")
        .args(["-f", &format!("user-data-dir={}", profile.display())])
        .output()
        .unwrap();
    !output.status.success()
}

fn wait_until(mut gone: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if gone() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    gone()
}

struct Launch {
    pid: u32,
    profile: PathBuf,
}

/// Re-run this test binary as the helper, read its launch report, then SIGKILL it.
fn spawn_and_kill_helper(detached: bool, die_with_spawner: bool) -> Launch {
    let binary = std::env::current_exe().unwrap();
    let child = std::process::Command::new(binary)
        .args([
            "pdeathsig_helper",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(HELPER, "1")
        .env(DETACHED, if detached { "1" } else { "0" })
        .env(DIE, if die_with_spawner { "1" } else { "0" })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = HelperGuard(child);
    let stderr = guard.0.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut pid = None;
    let mut profile = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    while pid.is_none() || profile.is_none() {
        let line = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("helper reported no Chromium launch");
        if let Some(rest) = line.strip_prefix("pid=") {
            pid = Some(rest.parse::<u32>().unwrap());
        }
        if let Some(rest) = line.strip_prefix("profile=") {
            profile = Some(PathBuf::from(rest));
        }
    }
    let launch = Launch {
        pid: pid.unwrap(),
        profile: profile.unwrap(),
    };
    assert!(alive(launch.pid), "Chromium should be running");
    kill(
        Pid::from_raw(i32::try_from(guard.0.id()).unwrap()),
        Signal::SIGKILL,
    )
    .unwrap();
    let status = guard.0.wait().unwrap();
    assert!(status.code().is_none(), "the helper should die by signal");
    launch
}

/// Helper entry: launches Chromium exactly once, reports pid and profile, then parks so the
/// parent can SIGKILL the process with no destructor ever running.
#[test]
fn pdeathsig_helper() {
    if std::env::var_os(HELPER).is_none() {
        return;
    }
    let mut config = BrowserConfig::new(common::executable().unwrap());
    config.detached = std::env::var_os(DETACHED).is_some_and(|v| v == "1");
    config.die_with_spawner = std::env::var_os(DIE).is_some_and(|v| v == "1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (browser, _events) = runtime.block_on(Browser::launch(config)).unwrap();
    eprintln!("pid={}", browser.process_id());
    eprintln!("profile={}", browser.endpoint().profile_dir.display());
    // The parent kills this process; destructors must never run, so the browser controller
    // and its runtime stay parked here forever.
    std::mem::forget(browser);
    loop {
        std::thread::park();
    }
}

/// PR_SET_PDEATHSIG coverage: a supervised launch dies with its spawner even when the
/// spawner is SIGKILLed, and an opted-in detached launch behaves the same. A plain detached
/// launch, by contract, outlives its spawner so a daemon can reconnect to it.
#[test]
fn killed_spawner_cannot_orphan_chromium() {
    if common::executable().is_none() {
        return;
    }

    // A normal launch is supervised: the kernel kills Chromium when its spawner dies.
    let launch = spawn_and_kill_helper(false, false);
    assert!(
        wait_until(|| !alive(launch.pid), Duration::from_secs(5)),
        "Chromium outlived its killed spawner"
    );
    // Renderers share the browser's tree; none may linger either.
    assert!(
        wait_until(|| group_gone(&launch.profile), Duration::from_secs(5)),
        "Chromium descendants outlived their killed spawner"
    );
    let _ = std::fs::remove_dir_all(&launch.profile);

    // A detached launch with die_with_spawner is supervised identically.
    let launch = spawn_and_kill_helper(true, true);
    assert!(
        wait_until(|| !alive(launch.pid), Duration::from_secs(5)),
        "a detached test browser outlived its killed spawner"
    );
    assert!(
        wait_until(|| group_gone(&launch.profile), Duration::from_secs(5)),
        "Chromium descendants outlived their killed spawner"
    );
    let _ = std::fs::remove_dir_all(&launch.profile);

    // A plain detached launch outlives its spawner by contract: the daemon reconnects to it.
    let launch = spawn_and_kill_helper(true, false);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        alive(launch.pid),
        "a detached browser must outlive its spawner"
    );
    let _ = killpg(
        Pid::from_raw(i32::try_from(launch.pid).unwrap()),
        Signal::SIGKILL,
    );
    assert!(wait_until(|| !alive(launch.pid), Duration::from_secs(5)));
    let _ = std::fs::remove_dir_all(&launch.profile);
}
