//! Browser activity is untrusted internal data: popup, download, and closure notices queue
//! behind the owner's running turn. Real daemon, Python sidecar, Chromium, and loopback pages.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{
    http::header,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::{json, Value};

const PAGE: &str = r#"<!doctype html><title>Reports</title><h1>Reports</h1>
<button onclick="window.popup = window.open('/popup', 'report-popup', 'popup')">Open popup</button>
<button onclick="window.popup.close()">Close popup</button>
<a href="/report.csv" download>Get report</a>"#;

const FINAL: &str = "Saw every browser event.";

fn browser() -> Option<std::path::PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: browser event steer acceptance requires Chromium: {reason}");
            None
        }
    }
}

fn repl(code: &str, id: &str) -> Value {
    json!({"parts":[
        {"type":"wait_steer","timeout_ms":5000},
        {"type":"tool_call","name":"browser_repl","args":{"code":code},"id":id}
    ]})
}

#[tokio::test]
async fn popup_download_and_closed_tab_reach_the_running_agent_as_steer() {
    let Some(executable) = browser() else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route(
            "/popup",
            get(|| async { Html("<!doctype html><title>Popup</title><p>Popup page</p>") }),
        )
        .route(
            "/report.csv",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/csv"),
                        (
                            header::CONTENT_DISPOSITION,
                            "attachment; filename=\"q3-secret-name.csv\"",
                        ),
                    ],
                    "month,total\njuly,3\n",
                )
                    .into_response()
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":format!("{origin}/")},"id":"open"}]},
        repl("await page.getByRole('button', { name: 'Open popup' }).click()", "popup"),
        repl("await page.getByRole('button', { name: 'Close popup' }).click(); await page.getByRole('link', { name: 'Get report' }).click()", "close-and-download"),
        {"parts":[{"type":"text","text":FINAL}]},
        {"parts":[{"type":"text","text":"Notices received"}]},
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        server.abort();
        return;
    };
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
    let submitted = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"go","text":"Open the report popup and download it","mode":"now"})),
        )
        .await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    let events = stream.settled().await;

    let failed: Vec<_> = events
        .iter()
        .filter(|event| matches!(&event.kind, EventKind::ToolEnded { is_error: true, .. }))
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
    let steered: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::MessageQueued {
                text,
                author_kind,
                mode,
                from_agent,
                ..
            } if text.starts_with("[browser event]") => {
                assert_eq!(*author_kind, openside_proto::MessageAuthorKind::Internal);
                assert_eq!(*mode, openside_proto::QueueMode::Queue);
                assert_eq!(*from_agent, None);
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    let expected = [
        "opened a popup tab",
        "was closed",
        &format!("started from {origin}"),
        "finished",
    ];
    for needle in expected {
        assert!(
            steered.iter().any(|text| text.contains(needle)),
            "no steer containing {needle:?}: {steered:?}"
        );
    }
    // Page-chosen file names never reach the model.
    assert!(
        !steered.iter().any(|text| text.contains("q3-secret-name")),
        "{steered:?}"
    );

    // Internal notices cannot steer the owner's in-flight model request.
    let conversation = harness.snapshot(&session)["messages"].to_string();
    let answered = conversation
        .find(FINAL)
        .expect("final answer in conversation");
    for needle in expected {
        let at = conversation
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} never reached the model: {conversation}"));
        assert!(at > answered, "{needle:?} interrupted the owner's turn");
    }
    assert!(conversation.contains("data_only"));

    harness.kill().await;
    server.abort();
}
