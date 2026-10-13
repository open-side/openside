//! Frame-bound FillOnly behavior and sensitive-recorder safety in headless Chromium.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(dead_code)]
mod common;

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserError};
use serde_json::json;

const LOCATE: &str = "({data:Array.from(document.querySelectorAll('input')).map(e=>({type:e.type})),nodes:Array.from(document.querySelectorAll('input'))})";
const FORM: &str = "<!doctype html><form onsubmit=\"document.documentElement.dataset.submitted='yes';return false\"><input autocomplete='cc-number'><input type='checkbox'><button>Pay</button></form><script>document.addEventListener('click',()=>document.documentElement.dataset.clicked='yes')</script>";

#[tokio::test]
async fn frame_bound_fill_only_and_navigation_refusal() {
    let _serial = common::serial().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app=Router::new().route("/",get(move ||async move {Html(format!("<!doctype html><input id='top'><iframe src='/form'></iframe><iframe src='http://localhost:{port}/form'></iframe>"))})).route("/form",get(||async{Html(FORM)}));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = common::browser_config().expect("Chromium is required for frame form tests");
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("forms", &format!("http://127.0.0.1:{port}/"))
        .await
        .unwrap();
    common::ready(&browser, &tab).await;
    let frames = browser.frames("forms", &tab.id).await.unwrap();
    assert_eq!(frames.len(), 3);
    let child = frames
        .iter()
        .find(|f| f.origin == format!("http://localhost:{port}"))
        .unwrap();
    assert!(child.host.is_some(), "cross-site fixture must be an OOPIF");
    let (_, cross) = browser
        .locate_form_fields("forms", &tab.id, &child.id, LOCATE)
        .await
        .unwrap();
    let same = frames
        .iter()
        .find(|f| f.parent.is_some() && f.host.is_none())
        .unwrap();
    let (_, local) = browser
        .locate_form_fields("forms", &tab.id, &same.id, LOCATE)
        .await
        .unwrap();
    let canary = "FORM_VALUE_CANARY_8db43";
    browser.record_protocol(true);
    let (_, top) = browser
        .locate(
            "forms",
            &tab.id,
            "({data:null,nodes:[document.querySelector('#top')]})",
        )
        .await
        .unwrap();
    browser
        .insert_text_node("forms", &tab.id, &top[0], canary)
        .await
        .unwrap();
    for nodes in [&local, &cross] {
        assert!(!serde_json::to_string(nodes).unwrap().contains(canary));
        browser
            .fill_form_fields("forms", &tab.id, nodes, &[json!(canary), json!(true)])
            .await
            .unwrap();
        let pin = nodes[0].frame.as_ref().unwrap();
        let world = browser
            .frame_command(
                "forms",
                &tab.id,
                &pin.frame_id,
                "Page.createIsolatedWorld",
                json!({"frameId":pin.frame_id,"worldName":"fixture-check"}),
            )
            .await
            .unwrap();
        let checked=browser.frame_command("forms",&tab.id,&pin.frame_id,"Runtime.evaluate",json!({"expression":"[document.querySelector('input').value,document.querySelector('[type=checkbox]').checked]","contextId":world["executionContextId"],"returnByValue":true})).await.unwrap();
        assert_eq!(checked["result"]["value"], json!([canary, true]));
        let result=browser.frame_command("forms",&tab.id,&pin.frame_id,"Runtime.evaluate",json!({"expression":"[document.documentElement.dataset.clicked||null,document.documentElement.dataset.submitted||null]","contextId":world["executionContextId"],"returnByValue":true})).await.unwrap();
        assert_eq!(result["result"]["value"], json!([null, null]));
        assert!(!serde_json::to_string(&browser.take_protocol_log())
            .unwrap()
            .contains(canary));
    }
    browser
        .evaluate(
            &tab.id,
            &format!(
                "document.querySelectorAll('iframe')[1].src='http://127.0.0.1:{port}/form?changed'"
            ),
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if browser
            .validate_form_fields("forms", &tab.id, &cross)
            .await
            .is_err()
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let error = browser
        .fill_form_fields("forms", &tab.id, &cross, &[json!(canary), json!(true)])
        .await
        .unwrap_err();
    assert!(!error.to_string().contains(canary));
    assert!(matches!(
        error,
        BrowserError::StaleReference | BrowserError::OriginMismatch
    ));
    browser
        .page_command(
            "forms",
            &tab.id,
            "Page.navigate",
            json!({"url":format!("http://127.0.0.1:{port}/form?top_changed")}),
        )
        .await
        .unwrap();
    let mut tab = tab;
    tab.url = format!("http://127.0.0.1:{port}/form?top_changed");
    common::ready(&browser, &tab).await;
    assert!(browser
        .fill_form_fields("forms", &tab.id, &local, &[json!(canary), json!(true)])
        .await
        .is_err());
    browser.shutdown().await.unwrap();
    server.abort();
}
