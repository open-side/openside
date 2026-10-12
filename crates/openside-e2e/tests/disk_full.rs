//! A full data volume must refuse writes with a clear error, never corrupt the event log, and
//! leave a run that was in flight settled once space returns. The daemon runs inside a private
//! mount namespace whose data directory is a tiny tmpfs; the test fills and frees it from outside
//! (`unshare`/`nsenter`, no root needed). Skips where unprivileged namespaces are unavailable.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

use openside_e2e::Harness;
use openside_proto::{Event, EventKind, MessageOutcome};
use serde_json::{json, Value};

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

/// Wrapper that mounts a 6 MB tmpfs over the data directory, then execs the real daemon with the
/// harness's arguments (`--data-dir <dir>` comes first).
fn tmpfs_daemon(dir: &Path) -> Option<String> {
    let probe = dir.join("probe");
    std::fs::create_dir_all(&probe).unwrap();
    let mounted = Command::new("unshare")
        .args([
            "-rm",
            "sh",
            "-c",
            "mount -t tmpfs -o size=1m tmpfs \"$0\"",
            probe.to_str().unwrap(),
        ])
        .status()
        .is_ok_and(|status| status.success());
    if !mounted || Command::new("nsenter").arg("--version").output().is_err() {
        eprintln!("SKIP: unprivileged mount namespaces (unshare -rm, nsenter) are unavailable");
        return None;
    }
    let wrapper = dir.join("daemon-on-tmpfs");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec unshare -rm sh -c 'mount -t tmpfs -o size=6m tmpfs \"$1\" && shift && exec {} \"$@\"' sh \"$2\" \"$@\"\n",
            env!("CARGO_BIN_EXE_e2e-opensided")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    Some(wrapper.to_str().unwrap().to_owned())
}

/// Run `script` inside the daemon's mount namespace.
fn in_daemon_namespace(pid: u32, script: &str) {
    let output = Command::new("nsenter")
        .args([
            "-t",
            &pid.to_string(),
            "-U",
            "-m",
            "--preserve-credentials",
            "--",
            "sh",
            "-c",
            script,
        ])
        .output()
        .expect("nsenter");
    // `dd` ends with ENOSPC by design when it fills the volume.
    eprintln!(
        "{script}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

#[tokio::test]
async fn a_full_data_volume_is_refused_cleanly_and_an_interrupted_run_settles_once_space_returns() {
    let scratch = tempfile::tempdir().unwrap();
    let Some(daemon) = tmpfs_daemon(scratch.path()) else {
        return;
    };
    let Some(mut harness) = Harness::prepare(
        &daemon,
        &json!({"responses":[text("first"), text("second"), text("third")],"delay_ms":1500}),
    ) else {
        return;
    };
    harness.launch().await;
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"disk","mode":"guard","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["id"].as_str().unwrap().to_owned();
    let mut stream = harness.events(&id, 0).await;
    let submit = |message: &'static str, mode: &'static str| {
        let (harness, id) = (&harness, id.clone());
        async move {
            harness
                .request(
                    "POST",
                    &format!("/v1/sessions/{id}/messages"),
                    Some(json!({"message_id":message,"text":message,"mode":mode})),
                )
                .await
        }
    };

    assert_eq!(submit("m1", "now").await.status, 202);
    let mut events: Vec<Event> = Vec::new();
    loop {
        let event = stream.next().await;
        let injected = matches!(&event.kind, EventKind::MessageInjected { message_id } if message_id.as_str() == "m1");
        events.push(event);
        if injected {
            break;
        }
    }

    // The disk fills while the model is still thinking: the run's next write cannot persist.
    let pid = harness.daemon_pid();
    let data = harness.data_dir.display().to_string();
    in_daemon_namespace(
        pid,
        &format!("dd if=/dev/zero of='{data}/.filler' bs=1k 2>&1 | tail -1"),
    );
    tokio::time::sleep(Duration::from_secs(4)).await;

    let refused = submit("while-full", "now").await;
    assert_eq!(refused.status, 507, "{}", refused.body);
    assert_eq!(refused.body["code"], "storage_full");
    let health = harness.request("GET", "/v1/health", None).await;
    assert_eq!(health.status, 200, "reads must keep working on a full disk");

    in_daemon_namespace(pid, &format!("rm -f '{data}/.filler'"));
    // The actor retries the bookkeeping the failed writes left undone.
    let settled = tokio::time::timeout(Duration::from_secs(30), stream.settled())
        .await
        .expect("the interrupted run must settle after space returns");
    events.extend(settled);
    assert!(
        events.iter().any(|event| matches!(&event.kind, EventKind::MessageCompleted { message_id, outcome: MessageOutcome::Failed, .. } if message_id.as_str() == "m1")),
        "the in-flight message must be recorded as failed: {events:?}"
    );
    let session = harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await;
    assert_eq!(session.body["status"], "idle", "{}", session.body);

    // Normal service resumes without a daemon restart, and the log stays gapless.
    assert_eq!(submit("m2", "now").await.status, 202);
    events.extend(
        tokio::time::timeout(Duration::from_secs(30), stream.settled())
            .await
            .unwrap(),
    );
    for pair in events.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq + 1, "event log has a gap");
    }
    for message in ["m1", "m2"] {
        let completions = events
            .iter()
            .filter(|event| matches!(&event.kind, EventKind::MessageCompleted { message_id, .. } if message_id.as_str() == message))
            .count();
        assert_eq!(completions, 1, "{message} must complete exactly once");
    }
    assert!(
        !events.iter().any(|event| matches!(&event.kind, EventKind::MessageQueued { message_id, .. } if message_id.as_str() == "while-full")),
        "a refused message must not appear in the log"
    );
    harness.kill().await;
}
