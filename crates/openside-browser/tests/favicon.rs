//! Bounded site icons from a real headless browser, without page-script hooks or cookies.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use axum::{
    http::{header, HeaderMap},
    response::Html,
    routing::get,
    Router,
};
use base64::Engine;
use openside_browser::{Browser, BrowserError};
use parking_lot::Mutex;
use std::sync::Arc;

const ICON: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

#[tokio::test]
async fn favicon_is_a_small_png_and_collection_omits_credentials_and_page_hooks() {
    let _serial = common::serial().await;
    let config = common::browser_config().expect("Chromium is required for the favicon smoke");
    let cookies = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&cookies);
    let app = Router::new()
        .route("/", get(|| async { Html("<!doctype html><title>Pricing</title><h1>Pricing</h1><script>window.fetch = () => { throw new Error('page fetch must not run'); };</script>") }))
        .route("/icon.png", get(move |headers: HeaderMap| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().push(headers.get(header::COOKIE).is_some());
                ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], base64::engine::general_purpose::STANDARD.decode(ICON).unwrap())
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("favicon", &format!("{origin}/"))
        .await
        .unwrap();
    common::ready(&browser, &tab).await;
    browser.evaluate(&tab.id, "document.cookie='test_cookie=private; path=/'; const link = document.createElement('link'); link.rel='stylesheet'; link.href='/icon.png'; document.head.append(link); link.rel='icon';").await.unwrap();
    // An uncached icon makes the collector's credential-free request observable even if
    // Chromium also starts its ordinary favicon request when the link changes.
    cookies.lock().clear();
    let data = browser
        .favicon("favicon", &tab.id)
        .await
        .unwrap()
        .expect("usable PNG");
    let encoded = data.strip_prefix("data:image/png;base64,").unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert!(bytes.len() <= 8192);
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 1);
    assert!(cookies.lock().iter().any(|has_cookie| !has_cookie));
    assert!(matches!(
        browser.favicon("another-session", &tab.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    browser.shutdown().await.unwrap();
    server.abort();
}
