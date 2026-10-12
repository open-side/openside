//! Page events and files against real Chromium with a loopback page: dialog claims, downloads
//! saved into the session's folder, popups of an isolated space, file chooser interception and
//! screenshot areas. Temporary profiles only.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use std::time::Duration;

use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use openside_browser::{
    Browser, BrowserError, BrowserEvent, DownloadStatus, PageEvent, ScreenshotArea, ScreenshotClip,
};
use serde_json::json;
use tokio::{sync::broadcast, task::JoinHandle};

use common::{browser_config, event, ready, serial};

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>events</title>
<style>body{margin:0}#box{position:absolute;left:30px;top:2000px;width:200px;height:100px;background:#c33}
#tall{height:3000px;width:600px}</style></head><body>
<div id="tall"></div><div id="box"></div>
<input id="file" type="file" multiple style="position:fixed;left:0;top:0;width:100px;height:40px">
<a id="get" href="/file.txt" download="ignored.txt">get</a>
<a id="popup" href="/blank" onclick="window.open(this.href,'popup-window');return false">pop</a>
</body></html>"#;

struct Fixture {
    base: String,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let app = Router::new()
            .route("/page", get(|| async { Html(PAGE) }))
            .route(
                "/blank",
                get(|| async { Html("<!doctype html><title>popup</title>popup body") }),
            )
            .route(
                "/file.txt",
                get(|| async {
                    (
                        [
                            (header::CONTENT_TYPE, "application/octet-stream"),
                            (
                                header::CONTENT_DISPOSITION,
                                "attachment; filename=\"server.txt\"",
                            ),
                        ],
                        "downloaded bytes\n",
                    )
                        .into_response()
                }),
            )
            .fallback(|| async { StatusCode::NOT_FOUND });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { base, task }
    }

    fn page(&self) -> String {
        format!("{}/page", self.base)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn next_page_event(
    receiver: &mut broadcast::Receiver<PageEvent>,
    predicate: impl Fn(&PageEvent) -> bool,
) -> PageEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = receiver.recv().await.expect("page event channel closed");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("expected page event did not arrive")
}

fn png_size(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    (
        u32::from_be_bytes(png[16..20].try_into().unwrap()),
        u32::from_be_bytes(png[20..24].try_into().unwrap()),
    )
}

/// A dialog of a claimed tab reaches the claimant only; the controller's automatic answer stays
/// out of its way. Dropping the claim hands an unanswered dialog back to the controller.
#[tokio::test]
async fn claimed_dialogs_skip_the_controller_until_the_claim_is_dropped() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let fixture = Fixture::start().await;
    let (browser, mut controller) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &fixture.page())
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let mut events = browser.page_events();

    let claim = browser.claim_dialogs("owner", &tab.id).await.unwrap();
    assert!(
        browser.claim_dialogs("owner", &tab.id).await.is_err(),
        "one claim per tab"
    );
    assert!(matches!(
        browser.claim_dialogs("someone-else", &tab.id).await,
        Err(BrowserError::OwnershipDenied(_) | BrowserError::InvalidAction(_))
    ));

    browser
        .evaluate(&tab.id, "setTimeout(() => confirm('keep going?'), 0); true")
        .await
        .unwrap();
    let opened = next_page_event(&mut events, |event| {
        matches!(event, PageEvent::DialogOpened(_))
    })
    .await;
    let PageEvent::DialogOpened(dialog) = opened else {
        unreachable!()
    };
    assert_eq!(
        (dialog.kind.as_str(), dialog.message.as_str()),
        ("confirm", "keep going?")
    );
    assert_eq!(browser.open_dialog(&tab.id), Some(dialog));
    browser.handle_dialog(&tab.id, true, None).await.unwrap();
    next_page_event(&mut events, |event| {
        matches!(event, PageEvent::DialogClosed { .. })
    })
    .await;
    assert_eq!(browser.open_dialog(&tab.id), None);
    while let Ok(event) = controller.try_recv() {
        assert!(
            !matches!(event, BrowserEvent::DialogOpened { .. }),
            "a claimed dialog must not reach the controller"
        );
    }

    browser
        .evaluate(&tab.id, "setTimeout(() => alert('left open'), 0); true")
        .await
        .unwrap();
    next_page_event(&mut events, |event| {
        matches!(event, PageEvent::DialogOpened(_))
    })
    .await;
    drop(claim);
    event(&mut controller, |event| {
        matches!(event, BrowserEvent::DialogOpened { message, .. } if message == "left open")
    })
    .await;
    browser.handle_dialog(&tab.id, true, None).await.unwrap();

    // Without a claim every dialog goes to the controller again.
    browser
        .evaluate(&tab.id, "setTimeout(() => alert('unclaimed'), 0); true")
        .await
        .unwrap();
    event(&mut controller, |event| {
        matches!(event, BrowserEvent::DialogOpened { message, .. } if message == "unclaimed")
    })
    .await;
    browser.handle_dialog(&tab.id, true, None).await.unwrap();
    browser.shutdown().await.ok();
}

/// A download is attributed to its tab's session, kept under its guid until saved, and
/// `save_download` confines the copy to the session's folder, refuses other owners and names
/// that are paths.
#[tokio::test]
async fn downloads_are_saved_into_the_session_folder_only() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let fixture = Fixture::start().await;
    let (browser, _controller) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("alice", &fixture.page())
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let mut events = browser.page_events();

    browser
        .evaluate(&tab.id, "document.getElementById('get').click(); true")
        .await
        .unwrap();
    let started = next_page_event(&mut events, |event| {
        matches!(event, PageEvent::DownloadStarted(_))
    })
    .await;
    let PageEvent::DownloadStarted(download) = started else {
        unreachable!()
    };
    assert_eq!(download.owner.as_deref(), Some("alice"));
    assert_eq!(download.tab_id.as_deref(), Some(tab.id.as_str()));
    assert_eq!(download.url, format!("{}/file.txt", fixture.base));
    // The server's Content-Disposition outranks the link's download attribute.
    assert_eq!(download.suggested_filename, "server.txt");
    let status = browser
        .wait_download(
            &download.guid,
            std::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Completed);
    assert_eq!(
        tokio::fs::read(
            browser
                .profile_path()
                .join("downloads")
                .join(&download.guid)
        )
        .await
        .unwrap(),
        b"downloaded bytes\n"
    );

    let folder = browser.session_downloads("alice").await.unwrap();
    assert!(folder.starts_with(browser.profile_path().join("downloads")));
    let saved = browser
        .save_download("alice", &download.guid, None)
        .await
        .unwrap();
    assert_eq!(saved, folder.join("server.txt"));
    assert_eq!(
        tokio::fs::read(&saved).await.unwrap(),
        b"downloaded bytes\n"
    );
    let renamed = browser
        .save_download("alice", &download.guid, Some("copy.txt"))
        .await
        .unwrap();
    assert_eq!(renamed, folder.join("copy.txt"));

    for name in ["../escape.txt", "a/b", "..", ""] {
        assert!(
            browser
                .save_download("alice", &download.guid, Some(name))
                .await
                .is_err(),
            "{name:?} must be refused"
        );
    }
    assert!(!folder.parent().unwrap().join("escape.txt").exists());
    assert!(matches!(
        browser.save_download("mallory", &download.guid, None).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser.save_download("alice", "no-such-guid", None).await,
        Err(BrowserError::InvalidAction(_))
    ));
    browser.shutdown().await.ok();
}

/// An isolated owner's downloads stay in its space's folder, and a closed space has none.
#[tokio::test]
async fn space_downloads_stay_in_the_space_folder() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let fixture = Fixture::start().await;
    let (browser, _controller) = Browser::launch(config).await.unwrap();
    let context = browser.open_space("carol").await.unwrap();
    let tab = browser
        .create_agent_tab("carol", &fixture.page())
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let mut events = browser.page_events();
    browser
        .evaluate(&tab.id, "document.getElementById('get').click(); true")
        .await
        .unwrap();
    let started = next_page_event(&mut events, |event| {
        matches!(event, PageEvent::DownloadStarted(_))
    })
    .await;
    let PageEvent::DownloadStarted(download) = started else {
        unreachable!()
    };
    assert_eq!(download.space.as_deref(), Some(context.as_str()));
    browser
        .wait_download(
            &download.guid,
            std::time::Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
    let folder = browser.session_downloads("carol").await.unwrap();
    assert_eq!(folder, browser.space_downloads(&context));
    let saved = browser
        .save_download("carol", &download.guid, None)
        .await
        .unwrap();
    assert_eq!(saved.parent().unwrap(), folder);
    assert_eq!(tokio::fs::read(saved).await.unwrap(), b"downloaded bytes\n");

    browser.close_space("carol").await.unwrap();
    assert!(matches!(
        browser.session_downloads("carol").await,
        Err(BrowserError::SpaceClosed(_))
    ));
    browser.shutdown().await.ok();
}

/// A popup is reported with its opener and keeps the opener's session and space.
#[tokio::test]
async fn popups_are_reported_with_opener_owner_and_space() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let fixture = Fixture::start().await;
    let (browser, _controller) = Browser::launch(config).await.unwrap();
    let context = browser.open_space("dave").await.unwrap();
    let tab = browser
        .create_agent_tab("dave", &fixture.page())
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let mut events = browser.page_events();
    browser
        .evaluate(&tab.id, "document.getElementById('popup').click(); true")
        .await
        .unwrap();
    let opened = next_page_event(&mut events, |event| {
        matches!(event, PageEvent::PopupOpened { .. })
    })
    .await;
    let PageEvent::PopupOpened {
        tab: popup,
        opener_id,
    } = opened
    else {
        unreachable!()
    };
    assert_eq!(opener_id, tab.id);
    assert_eq!(popup.owner_session_id.as_deref(), Some("dave"));
    assert_eq!(popup.browser_context_id.as_deref(), Some(context.as_str()));
    browser.shutdown().await.ok();
}

/// With interception on, activating a file input reports the chooser instead of opening it, and
/// the reported input accepts files.
#[tokio::test]
async fn file_chooser_is_intercepted_and_files_can_be_set() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let fixture = Fixture::start().await;
    let (browser, _controller) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("erin", &fixture.page())
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let mut events = browser.page_events();
    browser
        .page_command(
            "erin",
            &tab.id,
            "Page.setInterceptFileChooserDialog",
            json!({"enabled": true}),
        )
        .await
        .unwrap();
    browser
        .evaluate(&tab.id, "document.getElementById('file').click(); true")
        .await
        .unwrap();
    let opened = next_page_event(&mut events, |event| {
        matches!(event, PageEvent::FileChooserOpened(_))
    })
    .await;
    let PageEvent::FileChooserOpened(chooser) = opened else {
        unreachable!()
    };
    assert_eq!(chooser.tab_id, tab.id);
    assert!(chooser.multiple);
    let backend = chooser.backend_node_id.expect("the input is reported");

    let directory = tempfile_dir();
    let file = directory.join("upload.txt");
    tokio::fs::write(&file, "hello").await.unwrap();
    browser
        .page_command(
            "erin",
            &tab.id,
            "DOM.setFileInputFiles",
            json!({"files": [file.to_str().unwrap()], "backendNodeId": backend}),
        )
        .await
        .unwrap();
    let names = browser
        .evaluate(
            &tab.id,
            "Array.from(document.getElementById('file').files).map((f) => f.name + ':' + f.size)",
        )
        .await
        .unwrap();
    assert_eq!(names, json!(["upload.txt:5"]));
    tokio::fs::remove_dir_all(&directory).await.ok();
    browser.shutdown().await.ok();
}

fn tempfile_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("openside-events-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// Full-page, clip and element captures have the sizes asked for, and a capture leaves the
/// viewport as it was.
#[tokio::test]
async fn screenshot_areas_have_the_requested_size() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let fixture = Fixture::start().await;
    let (browser, _controller) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("fay", &fixture.page())
        .await
        .unwrap();
    ready(&browser, &tab).await;

    let viewport = png_size(
        &browser
            .screenshot_area(&tab.id, ScreenshotArea::Viewport)
            .await
            .unwrap(),
    );
    let full = png_size(
        &browser
            .screenshot_area(&tab.id, ScreenshotArea::FullPage)
            .await
            .unwrap(),
    );
    // The page scrolls, so the document is narrower than the viewport by the scrollbar.
    assert!(
        (viewport.0 - 40..=viewport.0).contains(&full.0),
        "{full:?} vs {viewport:?}"
    );
    assert!(
        full.1 >= 3000,
        "the document is 3000px tall plus an absolutely placed box: {full:?}"
    );

    let clip = ScreenshotClip {
        x: 10.0,
        y: 20.0,
        width: 120.0,
        height: 50.0,
    };
    assert_eq!(
        png_size(
            &browser
                .screenshot_area(&tab.id, ScreenshotArea::Clip(clip))
                .await
                .unwrap()
        ),
        (120, 50)
    );
    let page_clip = ScreenshotClip {
        x: 0.0,
        y: 2500.0,
        width: 300.0,
        height: 80.0,
    };
    assert_eq!(
        png_size(
            &browser
                .screenshot_area(&tab.id, ScreenshotArea::PageClip(page_clip))
                .await
                .unwrap()
        ),
        (300, 80)
    );

    let object = browser
        .page_command(
            "fay",
            &tab.id,
            "Runtime.evaluate",
            json!({"expression": "document.getElementById('box')"}),
        )
        .await
        .unwrap();
    let described = browser
        .page_command(
            "fay",
            &tab.id,
            "DOM.describeNode",
            json!({"objectId": object["result"]["objectId"]}),
        )
        .await
        .unwrap();
    let backend = described["node"]["backendNodeId"].as_u64().unwrap();
    assert_eq!(
        png_size(
            &browser
                .screenshot_area(&tab.id, ScreenshotArea::Element(backend))
                .await
                .unwrap()
        ),
        (200, 100)
    );
    // A later plain capture is unaffected by the area captures before it.
    assert_eq!(
        png_size(
            &browser
                .screenshot_area(&tab.id, ScreenshotArea::Viewport)
                .await
                .unwrap()
        ),
        viewport
    );
    browser.shutdown().await.ok();
}
