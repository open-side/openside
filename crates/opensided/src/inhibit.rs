//! Optional sleep inhibition while sessions run (`service.inhibit_sleep`, PP-4).
//!
//! The inhibitor is a child process held for exactly as long as at least one session is
//! `running`: `systemd-inhibit … sleep infinity` on Linux, `caffeinate -i` on macOS, or
//! `service.inhibit_cmd`. Killing the child releases the lock. On a clean exit or panic
//! the kill-on-drop handle ends it; after SIGKILL of the daemon only the service manager
//! does (systemd stops the unit's cgroup, launchd the job's process group), so an
//! inhibitor can outlive a hard-killed daemon started by hand. A child that exits on its own is
//! respawned within five seconds while a session still runs. Off by default.

use openside_proto::{EventKind, SessionStatus};
use tokio::process::{Child, Command};

use crate::engine::Shared;

fn default_argv() -> Option<Vec<String>> {
    let argv: &[&str] = if cfg!(target_os = "linux") {
        &[
            "systemd-inhibit",
            "--what=sleep:idle",
            "--who=openside",
            "--why=an openside session is running",
            "--mode=block",
            "sleep",
            "infinity",
        ]
    } else if cfg!(target_os = "macos") {
        &["caffeinate", "-i"]
    } else {
        return None;
    };
    Some(argv.iter().map(|arg| (*arg).to_owned()).collect())
}

async fn any_running(s: &Shared) -> bool {
    s.db.store(|store| Ok(store.list_sessions()))
        .await
        .map(|sessions| {
            sessions
                .iter()
                .any(|session| session.status == SessionStatus::Running)
        })
        .unwrap_or(false)
}

fn spawn(argv: &[String]) -> Option<Child> {
    let (program, args) = argv.split_first()?;
    match Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => Some(child),
        Err(error) => {
            crate::diag!("opensided: sleep inhibitor {program} failed to start: {error}");
            None
        }
    }
}

pub(crate) fn start(shared: &Shared) {
    let service = &shared.config.loaded.config.service;
    if !service.inhibit_sleep {
        return;
    }
    let argv = if service.inhibit_cmd.is_empty() {
        default_argv()
    } else {
        Some(service.inhibit_cmd.clone())
    };
    let Some(argv) = argv else {
        crate::diag!("opensided: service.inhibit_sleep has no inhibitor on this platform; set service.inhibit_cmd");
        return;
    };
    let s = shared.clone();
    let mut events = shared.events.subscribe();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut child: Option<Child> = None;
        let mut check = true;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            if child
                .as_mut()
                .is_some_and(|held| !matches!(held.try_wait(), Ok(None)))
            {
                child = None;
                check = true;
            }
            if check {
                let running = any_running(&s).await;
                match (running, child.as_mut()) {
                    (true, None) => child = spawn(&argv),
                    (false, Some(held)) => {
                        let _ = held.kill().await;
                        child = None;
                    }
                    _ => {}
                }
            }
            let event = tokio::select! {
                _ = shutdown.recv() => break,
                _ = tick.tick() => {
                    check = false;
                    continue;
                }
                event = events.recv() => event,
            };
            check = match event {
                Ok(event) => matches!(
                    event.kind,
                    EventKind::MessageInjected { .. }
                        | EventKind::MessageCompleted { .. }
                        | EventKind::TurnStarted { .. }
                        | EventKind::SessionSettled {}
                        | EventKind::SessionStopped { .. }
                        | EventKind::SessionDeleted {}
                ),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
        }
        if let Some(mut held) = child {
            let _ = held.kill().await;
        }
    });
}
