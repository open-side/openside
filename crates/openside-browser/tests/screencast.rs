//! Real Chromium screencast of an owned background tab (human takeover frames).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use std::time::Duration;

use base64::Engine;
use openside_browser::{Browser, BrowserError};
use serde_json::json;

use common::{browser_config, serial, Server};

#[tokio::test]
async fn owned_tab_streams_jpeg_frames_one_screencast_at_a_time() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    assert!(matches!(
        browser.screencast("intruder", &tab.id, 60).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    let cast = browser.screencast("owner", &tab.id, 60).await.unwrap();
    assert!(matches!(
        browser.screencast("owner", &tab.id, 60).await,
        Err(BrowserError::InvalidAction(_))
    ));
    let frame = tokio::time::timeout(Duration::from_secs(10), cast.next())
        .await
        .expect("a background agent tab must produce a screencast frame");
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(&frame.data)
        .unwrap();
    assert_eq!(&jpeg[..2], b"\xff\xd8", "frames are JPEG");
    assert_eq!(frame.viewport, (1440.0, 900.0));
    cast.ack(&frame);
    // A visible change after the acknowledgement produces the next frame.
    browser
        .page_command(
            "owner",
            &tab.id,
            "Page.navigate",
            json!({"url": server.page("welcome")}),
        )
        .await
        .unwrap();
    let next = tokio::time::timeout(Duration::from_secs(10), cast.next())
        .await
        .expect("acknowledged screencast keeps streaming");
    cast.ack(&next);
    drop(cast);
    // Dropping released the tab for a new controller.
    let again = browser.screencast("owner", &tab.id, 60).await.unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(10), again.next())
        .await
        .expect("restarted screencast streams");
    again.ack(&frame);
    drop(again);
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn thumbnails_leave_page_dimensions_and_resize_observers_unchanged() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    let initial = browser
        .evaluate(
            &tab.id,
            "(async () => { await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))); \
             const size = () => [innerWidth, innerHeight, document.documentElement.clientWidth, \
             visualViewport.width, visualViewport.height, visualViewport.scale, devicePixelRatio, \
             matchMedia('(width: 1440px)').matches]; window.__initialSize = size(); \
             window.__resizeSamples = []; window.addEventListener('resize', () => window.__resizeSamples.push(size())); \
             visualViewport.addEventListener('resize', () => window.__resizeSamples.push(size())); \
             window.__sizeSamples = []; const sample = () => { window.__sizeSamples.push(size()); \
             window.__sizeFrame = requestAnimationFrame(sample); }; window.__sizeFrame = requestAnimationFrame(sample); \
             return window.__initialSize; })()",
        )
        .await
        .unwrap();
    assert_eq!(initial, json!([1440, 900, 1440, 1440, 900, 1, 1, true]));
    for _ in 0..5 {
        browser.thumbnail("owner", &tab.id).await.unwrap();
    }
    browser
        .evaluate(&tab.id, "new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))")
        .await
        .unwrap();
    let observed = browser
        .evaluate(
            &tab.id,
            "cancelAnimationFrame(window.__sizeFrame); ({size:window.__initialSize, events:window.__resizeSamples, \
             samples:window.__sizeSamples.length, \
             changed:window.__sizeSamples.filter((size) => JSON.stringify(size) !== JSON.stringify(window.__initialSize))})",
        )
        .await
        .unwrap();
    assert!(observed["samples"].as_u64().unwrap() > 0);
    assert_eq!(observed["size"], initial);
    assert_eq!(observed["events"], json!([]));
    assert_eq!(observed["changed"], json!([]));
    browser.shutdown().await.unwrap();
}
