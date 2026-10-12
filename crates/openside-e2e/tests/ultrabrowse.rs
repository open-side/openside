//! A-9 ultrabrowse acceptance: the real daemon, real Python sidecar and real Chromium browse
//! three loopback pages under the `ultrabrowse` profile; the citation harness scores the
//! answer against the pages the browser tool actually opened.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{io::Write, process::Stdio};

use axum::{response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

const PAGES: [(&str, &str); 3] = [
    (
        "rivers",
        "<title>Rivers</title><p>The Nile is about 6,650 km long.</p>",
    ),
    (
        "lakes",
        "<title>Lakes</title><p>Lake Baikal is the deepest lake on Earth.</p>",
    ),
    (
        "peaks",
        "<title>Peaks</title><p>Mount Everest rises 8,849 m above sea level.</p>",
    ),
];

fn browser() -> Option<std::path::PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: ultrabrowse acceptance requires Chromium: {reason}");
            None
        }
    }
}

fn repo() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn final_text(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::AssistantFinal { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no final answer: {events:?}"))
}

/// Run tools/citation-harness.py the way CI does; returns (exit code, JSON report).
fn harness_report(answer: &str, visited: &Value, threshold: bool) -> (i32, Value) {
    let dir = tempfile::tempdir().unwrap();
    let visited_file = dir.path().join("visited.json");
    std::fs::write(&visited_file, visited.to_string()).unwrap();
    let mut command = std::process::Command::new(repo().join("agent/.venv/bin/python"));
    command
        .arg(repo().join("tools/citation-harness.py"))
        .args(["--answer", "-", "--visited-file"])
        .arg(&visited_file);
    if threshold {
        command.args(["--max-uncited-ratio", "0"]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answer.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[tokio::test]
async fn ultrabrowse_answers_with_citations_to_visited_pages_and_flags_uncited_claims() {
    let Some(executable) = browser() else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let mut app = Router::new();
    for (path, html) in PAGES {
        app = app.route(&format!("/{path}"), get(move || async move { Html(html) }));
    }
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let url = |path: &str| format!("{origin}/{path}");

    let cited = format!(
        "- The Nile is about 6,650 km long [Rivers]({}).\n\
         - Lake Baikal is the deepest lake on Earth [Lakes]({}).\n\
         - Mount Everest rises 8,849 m above sea level [Peaks]({}).",
        url("rivers"),
        url("lakes"),
        url("peaks")
    );
    let uncited = format!(
        "The Nile is about 6,650 km long [Rivers]({}).\n\
         Lake Baikal also holds a fifth of the world's fresh surface water.",
        url("rivers")
    );
    let open = |path: &str| json!({"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":url(path)},"id":format!("open-{path}")}]});
    let mut planned = open("rivers");
    planned["parts"].as_array_mut().unwrap().insert(
        0,
        json!({"type":"text","text":"Plan:\n1. Rivers page\n2. Lakes page\n3. Peaks page\n"}),
    );
    let answer = |text: &str| json!({"parts":[{"type":"text","text":text}]});
    // Turn 1: browsing before a plan is refused inside the sidecar, and the first,
    // partly uncited answer is sent back. Turn 2: the uncited answer survives every
    // citation retry and is annotated.
    let script = json!({"responses":[
        open("rivers"), planned, open("lakes"), open("peaks"),
        answer(&uncited), answer(&cited),
        answer(&uncited), answer(&uncited), answer(&uncited),
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        server.abort();
        return;
    };
    let config = harness.root().join("config/openside/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replacen("[agent]\n", "[agent]\nprofile = \"ultrabrowse\"\n", 1),
    )
    .unwrap();
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;

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
    let messages = format!("/v1/sessions/{session}/messages");
    let submit = |id: &'static str| {
        harness.request(
            "POST",
            &messages,
            Some(json!({"message_id":id,"text":"Research rivers, lakes and peaks","mode":"now"})),
        )
    };

    assert_eq!(submit("research").await.status, 202);
    let events = stream.settled().await;
    let exposed = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::RuntimeStarted { tools, .. } => Some(tools.clone()),
            _ => None,
        })
        .expect("runtime started");
    let snapshot: Vec<String> = serde_json::from_str(
        &std::fs::read_to_string(repo().join("docs/specs/profiles/ultrabrowse.tools.json"))
            .unwrap(),
    )
    .unwrap();
    let mut exposed_sorted = exposed.clone();
    exposed_sorted.sort();
    assert_eq!(
        exposed_sorted, snapshot,
        "real sidecar exposes the profile snapshot"
    );
    let visited: Vec<Value> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolEnded {
                result,
                is_error: false,
                ..
            } => Some(result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(visited.len(), 3, "{events:?}");
    for path in ["rivers", "lakes", "peaks"] {
        assert!(
            visited
                .iter()
                .any(|result| result.to_string().contains(&url(path))),
            "browser did not open {path}: {visited:?}"
        );
    }
    let answer = final_text(&events);
    assert_eq!(
        answer, cited,
        "the retried, fully cited answer is delivered unaltered"
    );
    let streamed: String = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        streamed.contains("Plan:"),
        "narration streams: {streamed:?}"
    );
    assert!(
        !streamed.contains("fresh surface water") && !streamed.contains("Everest"),
        "rejected and final answers are never streamed as deltas: {streamed:?}"
    );
    let (code, report) = harness_report(&answer, &json!(visited), true);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["claims"], 3, "{report}");
    assert_eq!(report["uncited_ratio"], 0.0, "{report}");

    assert_eq!(submit("follow-up").await.status, 202);
    let answer = final_text(&stream.settled().await);
    assert!(
        answer.ends_with("fresh surface water. [citation needed]"),
        "uncited claim must be flagged: {answer}"
    );
    let (code, report) = harness_report(&uncited, &json!(visited), true);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["uncited_ratio"], 0.5, "{report}");

    harness.kill().await;
    server.abort();
}
