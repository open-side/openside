use std::time::Duration;

use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig, BrowserEvent, TabInfo};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub const FIXTURES: &[&str] = &[
    "forms",
    "login",
    "select",
    "popup",
    "download",
    "dialog",
    "long-list",
    "nested-divs",
];
static BROWSER_TEST: Mutex<()> = Mutex::const_new(());

pub async fn serial() -> MutexGuard<'static, ()> {
    BROWSER_TEST.lock().await
}

pub fn browser_config() -> Option<BrowserConfig> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(executable) => Some(BrowserConfig::new(executable)),
        Err(error) => {
            eprintln!(
                "skip: no chromium-compatible browser found (tried {})",
                error.tried.join(", ")
            );
            None
        }
    }
}

pub struct Server {
    pub url: String,
    task: JoinHandle<()>,
}

impl Server {
    pub async fn start() -> Self {
        let app = Router::new().route(
            "/{fixture}",
            get(
                |axum::extract::Path(fixture): axum::extract::Path<String>| async move {
                    let html = match fixture.as_str() {
                        "forms" => include_str!("../fixtures/forms.html"),
                        "login" => include_str!("../fixtures/login.html"),
                        "select" => include_str!("../fixtures/select.html"),
                        "welcome" => "<!doctype html><html lang=\"en\"><head><title>Welcome</title></head><body><h1>Welcome</h1></body></html>",
                        "popup" => include_str!("../fixtures/popup.html"),
                        "download" => include_str!("../fixtures/download.html"),
                        "dialog" => include_str!("../fixtures/dialog.html"),
                        "long-list" => include_str!("../fixtures/long-list.html"),
                        "nested-divs" => include_str!("../fixtures/nested-divs.html"),
                        "secret-race" => include_str!("../fixtures/secret-race.html"),
                        "secret-hooks" => include_str!("../fixtures/secret-hooks.html"),
                        "download-file" => {
                            return (
                                [
                                    (header::CONTENT_TYPE, "application/octet-stream"),
                                    (
                                        header::CONTENT_DISPOSITION,
                                        "attachment; filename=fixture.txt",
                                    ),
                                ],
                                "openside local download fixture\n",
                            )
                                .into_response()
                        }
                        _ => return StatusCode::NOT_FOUND.into_response(),
                    };
                    Html(html).into_response()
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, task }
    }

    pub fn page(&self, fixture: &str) -> String {
        format!("{}/{fixture}", self.url)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn ready(browser: &Browser, tab: &TabInfo) {
    let condition = format!(
        "document.readyState === 'complete' && location.href === {}",
        json!(tab.url)
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut poll = tokio::time::interval(Duration::from_millis(20));
        loop {
            poll.tick().await;
            if browser.evaluate(&tab.id, &condition).await.unwrap() == true {
                break;
            }
        }
    })
    .await
    .expect("local fixture did not become ready");
}

pub async fn event(
    receiver: &mut mpsc::UnboundedReceiver<BrowserEvent>,
    predicate: impl Fn(&BrowserEvent) -> bool,
) -> BrowserEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = receiver.recv().await.expect("browser event channel closed");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("expected browser event did not arrive")
}

pub async fn gone(profile: &str, pid: u32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut poll = tokio::time::interval(Duration::from_millis(20));
        loop {
            poll.tick().await;
            let output = std::process::Command::new("pgrep")
                .args(["-af", &format!("user-data-dir={profile}")])
                .output()
                .unwrap();
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() && !output.status.success() {
                break;
            }
        }
    })
    .await
    .expect("Chromium processes remained after cleanup");
}

pub async fn window_id(profile: &std::path::Path, target: &str) -> u64 {
    tokio::time::timeout(Duration::from_secs(5), async {
        let endpoint = tokio::fs::read_to_string(profile.join("DevToolsActivePort"))
            .await
            .unwrap();
        let mut lines = endpoint.lines();
        let url = format!(
            "ws://127.0.0.1:{}{}",
            lines.next().unwrap(),
            lines.next().unwrap()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        socket
            .send(Message::Text(
                json!({"id":1,"method":"Browser.getWindowForTarget","params":{"targetId":target}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        loop {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                let reply: Value = serde_json::from_str(&text).unwrap();
                if reply["id"] == 1 {
                    return reply["result"]["windowId"].as_u64().unwrap();
                }
            }
        }
    })
    .await
    .expect("window metadata did not arrive")
}
