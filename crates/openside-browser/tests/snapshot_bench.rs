//! B-3 snapshot bench over the committed page set in `bench/snapshot` (see its README.md).
//!
//! Acceptance metric: bytes the agent receives over a fixed workflow per page (snapshot after
//! load, then after each of three scripted steps). openside sends the full snapshot once and
//! diffs afterwards; the committed agent-browser baseline is its default `snapshot` output
//! after every step. Single-snapshot and raw-AX ratios are printed and bounded too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

// Shared with the other browser test binaries; this one uses only the launch helpers.
#[allow(dead_code)]
mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use axum::{extract::Path, http::StatusCode, response::Html, routing::get, Router};
use openside_browser::{Browser, SnapshotPatch};
use serde_json::{json, Value};

use common::{browser_config, gone, ready, serial};

const OWNER: &str = "bench";
const WORKFLOW_RATIO_LIMIT: f64 = 0.35;
const RAW_RATIO_LIMIT: f64 = 0.10;
/// DOM elements a user can operate; each must be reachable through a snapshot ref.
const ACTIONABLE: &str =
    "a[href], button, input:not([type=hidden]), select, textarea, summary, [role=tab]";

fn bench_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bench/snapshot")
}

fn read_json(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(bench_dir().join(name)).unwrap()).unwrap()
}

async fn serve() -> (String, tokio::task::JoinHandle<()>) {
    let pages = bench_dir().join("pages");
    let app = Router::new().route(
        "/{page}",
        get(move |Path(page): Path<String>| {
            let pages = pages.clone();
            async move {
                match std::fs::read_to_string(pages.join(&page)) {
                    Ok(html) if !page.contains('/') => Ok(Html(html)),
                    _ => Err(StatusCode::NOT_FOUND),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, task)
}

/// Backend node ids of every actionable DOM element, read without page script.
async fn actionable_nodes(browser: &Browser, tab: &str) -> HashSet<u64> {
    let document = browser
        .page_command(OWNER, tab, "DOM.getDocument", json!({"depth":0}))
        .await
        .unwrap();
    let found = browser
        .page_command(
            OWNER,
            tab,
            "DOM.querySelectorAll",
            json!({"nodeId":document["root"]["nodeId"],"selector":ACTIONABLE}),
        )
        .await
        .unwrap();
    let mut nodes = HashSet::new();
    for id in found["nodeIds"].as_array().unwrap() {
        let described = browser
            .page_command(OWNER, tab, "DOM.describeNode", json!({"nodeId":id}))
            .await
            .unwrap();
        nodes.insert(described["node"]["backendNodeId"].as_u64().unwrap());
    }
    nodes
}

/// Every actionable element is printed with a ref that resolves back to that element.
async fn assert_actionable_addressable(browser: &Browser, tab: &str, text: &str, page: &str) {
    let snapshot = browser.snapshot(tab).await.unwrap();
    assert_eq!(
        snapshot.text, text,
        "{page}: snapshot is stable without page changes"
    );
    let mut addressed = HashSet::new();
    for reference in snapshot
        .nodes
        .iter()
        .filter_map(|node| node.reference.as_deref())
    {
        if text.contains(&format!("[{reference}] ")) {
            if let Ok(node) = browser.reference_node(OWNER, tab, reference).await {
                addressed.insert(node);
            }
        }
    }
    let actionable = actionable_nodes(browser, tab).await;
    assert!(
        !actionable.is_empty(),
        "{page}: fixture has actionable elements"
    );
    let missing = actionable.difference(&addressed).count();
    assert_eq!(
        missing,
        0,
        "{page}: {missing} of {} actionable elements have no printed ref",
        actionable.len()
    );
}

#[tokio::test]
async fn snapshot_bench_workflow_ratio_raw_ratio_and_accuracy() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let baseline = read_json("agent-browser-baseline.json");
    let workflow = read_json("workflow.json");
    let steps: Vec<&str> = workflow["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["script"].as_str().unwrap())
        .collect();
    let mut pages: Vec<String> = std::fs::read_dir(bench_dir().join("pages"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    pages.sort();
    let baseline_pages: BTreeMap<String, Vec<u64>> =
        serde_json::from_value(baseline["pages"].clone()).unwrap();
    assert!(pages.len() >= 10, "fixed page set has at least ten pages");
    assert_eq!(
        pages,
        baseline_pages.keys().cloned().collect::<Vec<_>>(),
        "baseline covers exactly the committed pages"
    );
    let (origin, server) = serve().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    println!(
        "agent-browser baseline: {}",
        baseline["version"].as_str().unwrap()
    );
    println!("| page | raw AX | openside full | full/raw | agent-browser full | full/ab | workflow openside | workflow ab | workflow ratio |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    let (mut sum_raw, mut sum_full, mut sum_ab_full, mut sum_flow, mut sum_ab_flow) =
        (0, 0, 0, 0, 0);
    for page in &pages {
        let tab = browser
            .create_agent_tab(OWNER, &format!("{origin}/{page}"))
            .await
            .unwrap();
        ready(&browser, &tab).await;
        let first = browser.snapshot(&tab.id).await.unwrap();
        assert_actionable_addressable(&browser, &tab.id, &first.text, page).await;
        let mut flow = first.text.len();
        let mut previous = first.clone();
        for script in &steps {
            assert_eq!(
                browser.evaluate(&tab.id, script).await.unwrap(),
                true,
                "{page}"
            );
            let next = browser.snapshot(&tab.id).await.unwrap();
            assert_eq!(
                next.document_root, previous.document_root,
                "{page}: steps keep the document"
            );
            let patch = SnapshotPatch::between(&previous.text, &next.text);
            assert!(!patch.is_empty(), "{page}: every step changes the snapshot");
            let sent = patch.for_agent();
            let received = SnapshotPatch::parse(sent.split_once('\n').unwrap().1).unwrap();
            assert_eq!(
                received.apply(&previous.text).as_deref(),
                Some(next.text.as_str()),
                "{page}: diff applied to the previous snapshot equals the full snapshot"
            );
            for node in &previous.nodes {
                if let Some(reference) = &node.reference {
                    if let Some(same) = next
                        .nodes
                        .iter()
                        .find(|later| later.reference.as_ref() == Some(reference))
                    {
                        assert_eq!(
                            same.role, node.role,
                            "{page}: refs keep identity across diffs"
                        );
                    }
                }
            }
            flow += sent.len();
            previous = next;
        }
        assert_actionable_addressable(&browser, &tab.id, &previous.text, page).await;
        let ab = &baseline_pages[page];
        let ab_flow: u64 = ab.iter().sum();
        let full = first.text.len();
        println!(
            "| {page} | {} | {full} | {:.3} | {} | {:.3} | {flow} | {ab_flow} | {:.3} |",
            first.raw_bytes,
            full as f64 / first.raw_bytes as f64,
            ab[0],
            full as f64 / ab[0] as f64,
            flow as f64 / ab_flow as f64,
        );
        assert!(
            (full as f64) / (first.raw_bytes as f64) <= RAW_RATIO_LIMIT,
            "{page}: compact snapshot must stay under {RAW_RATIO_LIMIT} of the raw AX tree"
        );
        assert!(
            flow as f64 / ab_flow as f64 <= WORKFLOW_RATIO_LIMIT,
            "{page}: workflow bytes exceed {WORKFLOW_RATIO_LIMIT} of agent-browser"
        );
        sum_raw += first.raw_bytes;
        sum_full += full;
        sum_ab_full += ab[0];
        sum_flow += flow;
        sum_ab_flow += ab_flow;
        browser.close_agent_tab(OWNER, &tab.id).await.unwrap();
    }
    let workflow_ratio = sum_flow as f64 / sum_ab_flow as f64;
    println!(
        "| total | {sum_raw} | {sum_full} | {:.3} | {sum_ab_full} | {:.3} | {sum_flow} | {sum_ab_flow} | {workflow_ratio:.3} |",
        sum_full as f64 / sum_raw as f64,
        sum_full as f64 / sum_ab_full as f64,
    );
    println!(
        "approx tokens (bytes/4): openside workflow {} vs agent-browser {}",
        sum_flow / 4,
        sum_ab_flow / 4
    );
    assert!(workflow_ratio <= WORKFLOW_RATIO_LIMIT);
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
    server.abort();
}
