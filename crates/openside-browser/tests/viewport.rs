//! `Browser::snapshot_viewport` against real headless Chromium: only what overlaps the viewport
//! (and its ancestors), with the tab's usual stable refs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

// Shared with the other browser test binaries; this one uses only the launch helpers.
#[allow(dead_code)]
mod common;

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, Snapshot};

use common::{browser_config, gone, ready, serial};

const OWNER: &str = "viewport";

/// A fixed header over sixty 100px-pitch buttons, a scroll container with its own overflow, and
/// a footer: every case the viewport filter has to get right.
fn long_page() -> String {
    let mut page = String::from(
        "<!doctype html><html lang=en><title>Long</title>\
         <body style=\"margin:0\"><header style=\"position:fixed;top:0;left:0;right:0;height:40px\">\
         <button>Site menu</button></header><main style=\"padding-top:50px\">",
    );
    for item in 1..=60 {
        page.push_str(&format!(
            "<button style=\"display:block;height:60px;margin:0 0 40px\">Item {item}</button>"
        ));
    }
    page.push_str("<div role=region aria-label=Feed style=\"height:200px;overflow:auto\">");
    for entry in 1..=40 {
        page.push_str(&format!(
            "<p style=\"height:50px\"><a href=\"#e{entry}\">Entry {entry}</a></p>"
        ));
    }
    page.push_str("</div></main><footer><a href=\"#top\">Back to top</a></footer></body></html>");
    page
}

async fn serve() -> (String, tokio::task::JoinHandle<()>) {
    let long = long_page();
    let app = Router::new().route("/long", get(move || async move { Html(long) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (
        origin,
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    )
}

fn reference(snapshot: &Snapshot, name: &str) -> Option<String> {
    snapshot
        .nodes
        .iter()
        .find(|node| node.name == name)
        .and_then(|node| node.reference.clone())
}

fn shown(snapshot: &Snapshot, name: &str) -> bool {
    snapshot.nodes.iter().any(|node| node.name == name)
}

#[tokio::test]
async fn viewport_snapshot_keeps_what_is_on_screen_with_ancestors_and_stable_refs() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let (origin, server) = serve().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab(OWNER, &format!("{origin}/long"))
        .await
        .unwrap();
    ready(&browser, &tab).await;

    let full = browser.snapshot(&tab.id).await.unwrap();
    let top = browser.snapshot_viewport(&tab.id).await.unwrap();
    println!(
        "long page at top: full {} bytes / {} nodes, viewport {} bytes / {} nodes",
        full.text.len(),
        full.nodes.len(),
        top.text.len(),
        top.nodes.len()
    );
    assert!(shown(&top, "Site menu") && shown(&top, "Item 1") && shown(&top, "Item 8"));
    assert!(!shown(&top, "Item 20") && !shown(&top, "Item 60") && !shown(&top, "Back to top"));
    // Ancestors stay so the tree keeps its shape: the region of an off-screen feed does not.
    assert!(top.nodes.iter().any(|node| node.role == "main"));
    assert!(!top.nodes.iter().any(|node| node.role == "region"));
    assert_eq!(top.text.lines().count(), top.nodes.len());
    assert!(top.text.len() * 4 < full.text.len());
    assert_eq!(top.document_root, full.document_root);

    // The ref of a node that is shown is the ref the full snapshot gave it.
    assert_eq!(reference(&top, "Item 3"), reference(&full, "Item 3"));

    // Scrolled: a fixed header is still on screen, the first items are not, items 31-39 are.
    browser
        .evaluate(&tab.id, "window.scrollTo(0, 3000); true")
        .await
        .unwrap();
    let middle = browser.snapshot_viewport(&tab.id).await.unwrap();
    println!(
        "long page scrolled to 3000: viewport {} bytes / {} nodes",
        middle.text.len(),
        middle.nodes.len()
    );
    assert!(shown(&middle, "Site menu"), "{}", middle.text);
    assert!(shown(&middle, "Item 35"), "{}", middle.text);
    assert!(!shown(&middle, "Item 3") && !shown(&middle, "Item 60"));
    assert_eq!(reference(&middle, "Item 35"), reference(&full, "Item 35"));

    // A ref handed out earlier for a node that is not shown still resolves to it.
    let far = reference(&full, "Item 60").unwrap();
    assert!(!middle.text.contains(&format!("[{far}]")));
    browser
        .reference_node(OWNER, &tab.id, &far)
        .await
        .expect("an earlier ref stays valid after a viewport capture");

    // The bottom shows the footer and what follows the buttons.
    browser
        .evaluate(
            &tab.id,
            "window.scrollTo(0, document.body.scrollHeight); true",
        )
        .await
        .unwrap();
    let bottom = browser.snapshot_viewport(&tab.id).await.unwrap();
    assert!(shown(&bottom, "Back to top") && shown(&bottom, "Item 60"));
    assert!(!shown(&bottom, "Item 1"));

    // A full capture afterwards is whole again, numbered like before.
    let again = browser.snapshot(&tab.id).await.unwrap();
    assert_eq!(again.text, full.text);

    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
    server.abort();
}
