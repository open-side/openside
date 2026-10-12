//! The harness is a child subreaper, so every process the daemon's browsers leave behind
//! (renderers outliving a SIGKILLed browser, double-forked crashpad handlers, a browser
//! adopted across a daemon restart) is re-parented to this test process. Force-kill,
//! relaunch and close cycles must leave no zombie and no Chromium of the profile behind.
//! Its own binary: the zombie check covers the whole test process.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use axum::{response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::{json, Value};

fn browser_pid(harness: &Harness) -> i32 {
    let state: Value = serde_json::from_slice(
        &std::fs::read(harness.data_dir.join("browser-state.json")).unwrap(),
    )
    .unwrap();
    i32::try_from(state["endpoint"]["pid"].as_u64().unwrap()).unwrap()
}

/// `(pid, comm)` of every zombie whose parent is this test process.
fn zombie_children() -> Vec<(i32, String)> {
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| {
            let pid: i32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let (head, rest) = stat.rsplit_once(") ")?;
            let comm = head.split_once(" (")?.1.to_owned();
            let mut fields = rest.split(' ');
            let state = fields.next()?;
            let parent = fields.next()?;
            (state == "Z" && parent == me).then_some((pid, comm))
        })
        .collect()
}

/// Pids whose command line names a path inside the harness root (the browser profile,
/// crashpad's database under it).
fn processes_under(root: &std::path::Path) -> Vec<i32> {
    let root = root.as_os_str().as_encoded_bytes();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| {
            let pid: i32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            cmdline
                .windows(root.len())
                .any(|part| part == root)
                .then_some(pid)
        })
        .collect()
}

async fn open_tab(harness: &Harness) {
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let mut stream = harness.events(&session, 0).await;
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"open","text":"open the page","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let events = stream.settled().await;
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolEnded {
                is_error: false,
                ..
            }
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn browser_kill_relaunch_and_close_cycles_leave_no_zombies() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: browser zombie regression requires Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let app = Router::new().route("/", get(|| async { Html("<title>Page</title>") }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":url},"id":"open"}]},
        {"parts":[{"type":"text","text":"opened"}]},
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        server.abort();
        return;
    };
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;
    for cycle in 0..5 {
        // Each session's scripted turn opens one agent tab, so a restarted daemon adopts the
        // running browser (now this process's child) instead of closing it.
        open_tab(&harness).await;
        if cycle % 2 == 1 {
            harness.restart().await;
        }
        let killed = browser_pid(&harness);
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(killed),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();
        // Quick deaths back off 1, 2, 4... seconds before the daemon relaunches.
        tokio::time::timeout(Duration::from_secs(45), async {
            while browser_pid(&harness) == killed {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("cycle {cycle}: the daemon did not relaunch the browser"));
    }
    // The relaunched browser owns no tab, so the restarted daemon closes the adopted browser
    // and waits for it to exit.
    harness.restart().await;
    assert!(!harness.data_dir.join("browser-state.json").exists());

    let root = harness.root().to_owned();
    harness.kill().await;
    server.abort();
    assert_eq!(
        zombie_children(),
        Vec::<(i32, String)>::new(),
        "zombies re-parented to the test process"
    );
    assert_eq!(
        processes_under(&root),
        Vec::<i32>::new(),
        "processes of the harness profile survived"
    );
}
