//! Compressed PP-23 soak: real processes, scripted replies and loopback pages only.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn browser_pid(h: &Harness) -> i32 {
    let state: Value =
        serde_json::from_slice(&std::fs::read(h.data_dir.join("browser-state.json")).unwrap())
            .unwrap();
    state["endpoint"]["pid"].as_i64().unwrap() as i32
}

fn resources(pid: i32) -> (u64, usize) {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let rss = status
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse().ok())
        })
        .unwrap();
    (
        rss,
        std::fs::read_dir(format!("/proc/{pid}/fd"))
            .unwrap()
            .count(),
    )
}

async fn send(h: &Harness, session: &str, id: &str) {
    let response = h
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":"soak loopback task","mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}

#[tokio::test]
#[ignore = "runs real browser/sidecar fault injection; use tools/soak.sh"]
async fn soak() {
    let seconds: u64 = std::env::var("OPENSIDE_SOAK_SECONDS")
        .unwrap_or_else(|_| "900".into())
        .parse()
        .unwrap();
    let minimum: usize = std::env::var("OPENSIDE_SOAK_SESSIONS")
        .unwrap_or_else(|_| "10".into())
        .parse()
        .unwrap();
    assert!(seconds > 0 && minimum > 0);
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("soak requires a Chromium-compatible browser; configure OPENSIDE_BROWSER");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/",
                get(|| async { Html("<title>Soak</title><h1>Loopback soak</h1>") }),
            ),
        )
        .await
        .unwrap();
    });
    let open = json!({"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":url},"id":"open"}]});
    let done = json!({"parts":[{"type":"text","text":"soak completed"}]});
    let script = json!({"responses":[open, done, open, done]});
    let mut h = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script)
        .expect("soak requires uv sync --project agent");
    h.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    h.launch().await;
    let started = Instant::now();
    let mut cycles = 0;
    let mut restarts = 0;
    let mut routines = 0;
    let mut samples = Vec::new();
    let mut baseline: Option<(u64, usize)> = None;
    let mut max_rss_growth = 0;
    let mut max_fd_growth = 0;
    let mut expected_browser = None;
    while started.elapsed() < Duration::from_secs(seconds) || cycles < minimum {
        let response = h
            .request(
                "POST",
                "/v1/sessions",
                Some(json!({"mode":"full","workspace":h.workspace()})),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.body);
        let session = response.body["id"].as_str().unwrap().to_owned();
        let mut stream = h.events(&session, 0).await;
        send(&h, &session, "first").await;
        let first = stream.settled().await;
        assert!(first.iter().any(
            |e| matches!(&e.kind, EventKind::AssistantFinal { text } if text == "soak completed")
        ));
        assert!(
            !first.iter().any(|e| matches!(
                &e.kind,
                EventKind::RuntimeError { .. } | EventKind::ToolEnded { is_error: true, .. }
            )),
            "{first:?}"
        );
        let old_browser = browser_pid(&h);
        if let Some(expected) = expected_browser {
            assert_eq!(
                old_browser, expected,
                "browser restarted outside an injected fault"
            );
        }
        h.kill_sidecar();
        let killed = stream.settled().await;
        assert!(killed.iter().any(
            |e| matches!(&e.kind, EventKind::RuntimeError { code, .. } if code == "interrupted")
        ));
        assert_eq!(
            browser_pid(&h),
            old_browser,
            "browser restarted during sidecar-only fault"
        );
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(old_browser),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if browser_pid(&h) != old_browser {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("browser did not recover");
        let recovered_browser = browser_pid(&h);
        expected_browser = Some(recovered_browser);
        send(&h, &session, "second").await;
        let second = stream.settled().await;
        assert!(second.iter().any(|e| matches!(&e.kind, EventKind::AssistantFinal { text } if text == "soak completed")), "{second:?}");
        assert!(
            !second.iter().any(|e| matches!(
                &e.kind,
                EventKind::RuntimeError { .. } | EventKind::ToolEnded { is_error: true, .. }
            )),
            "{second:?}"
        );
        let stopped = h
            .request("POST", &format!("/v1/sessions/{session}/stop"), None)
            .await;
        assert!(stopped.status < 300, "{}", stopped.body);
        assert_eq!(
            h.request("DELETE", &format!("/v1/sessions/{session}"), None)
                .await
                .status,
            204
        );
        drop(stream);
        if cycles % 5 == 0 {
            let added = h
                .cli(
                    env!("CARGO_BIN_EXE_e2e-openside"),
                    &[
                        "--json",
                        "routine",
                        "add",
                        "--name",
                        "Soak",
                        "--cron",
                        "0 9 * * *",
                        "--prompt",
                        "loopback task",
                    ],
                    None,
                )
                .await;
            assert!(
                added.status.success(),
                "{}",
                String::from_utf8_lossy(&added.stderr)
            );
            let routine: Value =
                serde_json::from_slice(added.stdout.split(|byte| *byte == b'\n').next().unwrap())
                    .unwrap();
            let id = routine["id"].as_str().unwrap();
            let run = h
                .cli(
                    env!("CARGO_BIN_EXE_e2e-openside"),
                    &["--json", "routine", "run", id],
                    None,
                )
                .await;
            assert!(run.status.success());
            let run: Value =
                serde_json::from_slice(run.stdout.split(|byte| *byte == b'\n').next().unwrap())
                    .unwrap();
            let task = run["session_id"].as_str().unwrap();
            let events = h.events(task, 0).await.settled().await;
            assert!(events
                .iter()
                .any(|e| matches!(&e.kind, EventKind::AssistantFinal { .. })));
            assert!(!events.iter().any(|e| matches!(
                &e.kind,
                EventKind::RuntimeError { .. } | EventKind::ToolEnded { is_error: true, .. }
            )));
            let stopped = h
                .request("POST", &format!("/v1/sessions/{task}/stop"), None)
                .await;
            assert!(stopped.status < 300);
            assert_eq!(
                h.request("DELETE", &format!("/v1/sessions/{task}"), None)
                    .await
                    .status,
                204
            );
            let removed = h
                .cli(
                    env!("CARGO_BIN_EXE_e2e-openside"),
                    &["routine", "rm", id],
                    None,
                )
                .await;
            assert!(
                removed.status.success(),
                "{}",
                String::from_utf8_lossy(&removed.stderr)
            );
            routines += 1;
        }
        assert_eq!(
            browser_pid(&h),
            recovered_browser,
            "browser restarted after injected-fault recovery"
        );
        let (rss, fds) = resources(h.daemon_pid() as i32);
        let (initial_rss, initial_fds) = *baseline.get_or_insert((rss, fds));
        max_rss_growth = max_rss_growth.max(rss.saturating_sub(initial_rss));
        max_fd_growth = max_fd_growth.max(fds.saturating_sub(initial_fds));
        assert!(
            max_rss_growth <= 128 * 1024,
            "daemon RSS grew by {max_rss_growth} KiB"
        );
        assert!(
            max_fd_growth <= 32,
            "daemon descriptors grew by {max_fd_growth}"
        );
        samples.push(json!({"session":cycles,"elapsed_seconds":started.elapsed().as_secs_f64(),"rss_kib":rss,"fd_count":fds,"generation":restarts}));
        cycles += 1;
        if cycles % 3 == 0 {
            h.restart().await;
            restarts += 1;
            baseline = None;
            expected_browser = None;
        }
    }
    let stderr = std::fs::read_to_string(h.root().join("daemon.stderr")).unwrap();
    assert!(!stderr.contains("panicked at"), "daemon panic: {stderr}");
    let crash_dir = h.data_dir.join("crash-reports");
    let crashes = if crash_dir.exists() {
        std::fs::read_dir(crash_dir).unwrap().count()
    } else {
        0
    };
    assert_eq!(crashes, 0, "unexpected crash reports");
    let summary = json!({"elapsed_seconds":started.elapsed().as_secs_f64(),"sessions":cycles,"sidecar_sigkills":cycles,"browser_sigkills":cycles,"daemon_restarts":restarts,"routine_runs":routines,"unexpected_crashes":crashes,"max_rss_growth_kib":max_rss_growth,"max_fd_growth":max_fd_growth,"rss_bound_kib":128*1024,"fd_bound":32,"samples":samples});
    h.kill().await;
    server.abort();
    println!("SOAK_SUMMARY {summary}");
    if let Ok(output) = std::env::var("OPENSIDE_SOAK_OUTPUT") {
        let path = Path::new(&output);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&summary).unwrap()).unwrap();
    }
}
