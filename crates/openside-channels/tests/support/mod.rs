//! Hermetic fake chat platforms (axum on 127.0.0.1:0) and a recording daemon proxy.
//!
//! Shared by this crate's tests and `crates/openside-e2e/tests/channels.rs` through a
//! `#[path]` module, so both exercise the same fakes. Not every test uses every item.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::{json, Value};

pub const WAIT: Duration = Duration::from_secs(30);

/// Poll `check` until it yields a value or `timeout` passes.
pub async fn eventually<T>(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    address
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- Telegram

#[derive(Clone, Debug)]
pub struct TgMsg {
    pub id: i64,
    pub chat: String,
    pub text: String,
    /// (label, callback_data)
    pub buttons: Vec<(String, String)>,
    pub edits: u32,
}

#[derive(Default)]
struct TgInner {
    updates: Vec<Value>,
    next_update: i64,
    next_message: i64,
    messages: Vec<TgMsg>,
    calls: Vec<(String, Value)>,
    files: HashMap<String, Vec<u8>>,
}

struct TgState {
    token: String,
    inner: Mutex<TgInner>,
}

pub struct FakeTelegram {
    pub base: String,
    state: Arc<TgState>,
}

impl FakeTelegram {
    pub async fn start(token: &str) -> Self {
        let state = Arc::new(TgState {
            token: token.to_owned(),
            inner: Mutex::new(TgInner {
                next_update: 1,
                next_message: 1000,
                ..TgInner::default()
            }),
        });
        let router = Router::new()
            .route("/{bot}/{method}", post(tg_method))
            .route("/file/{bot}/{*path}", get(tg_file))
            .with_state(state.clone());
        let address = serve(router).await;
        Self {
            base: format!("http://{address}"),
            state,
        }
    }

    fn push(&self, build: impl FnOnce(i64) -> Value) {
        let mut inner = lock(&self.state.inner);
        let id = inner.next_update;
        inner.next_update += 1;
        let update = build(id);
        inner.updates.push(update);
    }

    fn next_message_id(&self) -> i64 {
        let mut inner = lock(&self.state.inner);
        inner.next_message += 1;
        inner.next_message
    }

    /// A user message; returns the Telegram message id.
    pub fn push_text(&self, chat: i64, user: i64, text: &str) -> i64 {
        let message_id = self.next_message_id();
        self.push(|update_id| {
            json!({"update_id": update_id, "message": {
                "message_id": message_id, "from": {"id": user}, "chat": {"id": chat, "type": "private"}, "text": text}})
        });
        message_id
    }

    pub fn push_voice(&self, chat: i64, user: i64, file_id: &str, audio: &[u8]) -> i64 {
        lock(&self.state.inner)
            .files
            .insert(file_id.to_owned(), audio.to_vec());
        let message_id = self.next_message_id();
        self.push(|update_id| {
            json!({"update_id": update_id, "message": {
                "message_id": message_id, "from": {"id": user}, "chat": {"id": chat, "type": "private"},
                "voice": {"file_id": file_id, "file_size": audio.len(), "duration": 2}}})
        });
        message_id
    }

    /// A button press; returns the callback query id the bot must answer with.
    pub fn push_callback(&self, chat: i64, user: i64, message_id: i64, data: &str) -> String {
        let mut callback = String::new();
        self.push(|update_id| {
            callback = format!("cb-{update_id}");
            json!({"update_id": update_id, "callback_query": {
                "id": callback, "from": {"id": user}, "data": data,
                "message": {"message_id": message_id, "chat": {"id": chat, "type": "private"}}}})
        });
        callback
    }

    pub fn messages(&self) -> Vec<TgMsg> {
        lock(&self.state.inner).messages.clone()
    }

    pub fn calls(&self, method: &str) -> Vec<Value> {
        lock(&self.state.inner)
            .calls
            .iter()
            .filter(|(name, _)| name == method)
            .map(|(_, body)| body.clone())
            .collect()
    }

    pub async fn wait_message(
        &self,
        what: &str,
        mut predicate: impl FnMut(&TgMsg) -> bool,
    ) -> TgMsg {
        eventually(what, WAIT, || {
            self.messages()
                .into_iter()
                .find(|message| predicate(message))
        })
        .await
    }

    pub async fn wait_text(&self, needle: &str) -> TgMsg {
        let needle = needle.to_owned();
        self.wait_message(&format!("message containing {needle:?}"), move |m| {
            m.text.contains(&needle)
        })
        .await
    }
}

async fn tg_method(
    State(state): State<Arc<TgState>>,
    Path((bot, method)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if bot != format!("bot{}", state.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "description": "Unauthorized"})),
        )
            .into_response();
    }
    lock(&state.inner)
        .calls
        .push((method.clone(), body.clone()));
    let ok = |result: Value| Json(json!({"ok": true, "result": result})).into_response();
    match method.as_str() {
        "getUpdates" => {
            let offset = body["offset"].as_i64().unwrap_or(0);
            let wait = body["timeout"].as_u64().unwrap_or(0).min(1);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(wait);
            loop {
                let due: Vec<Value> = {
                    let mut inner = lock(&state.inner);
                    inner
                        .updates
                        .retain(|u| u["update_id"].as_i64().unwrap_or(0) >= offset);
                    inner.updates.clone()
                };
                if !due.is_empty() || tokio::time::Instant::now() >= deadline {
                    return ok(Value::Array(due));
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        }
        "sendMessage" => {
            let mut inner = lock(&state.inner);
            inner.next_message += 1;
            let id = inner.next_message;
            let message = TgMsg {
                id,
                chat: body["chat_id"].to_string().trim_matches('"').to_owned(),
                text: body["text"].as_str().unwrap_or_default().to_owned(),
                buttons: tg_buttons(&body),
                edits: 0,
            };
            inner.messages.push(message);
            ok(json!({"message_id": id}))
        }
        "editMessageText" => {
            let mut inner = lock(&state.inner);
            let id = body["message_id"].as_i64().unwrap_or(0);
            let Some(message) = inner.messages.iter_mut().find(|m| m.id == id) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"ok": false, "description": "message to edit not found"})),
                )
                    .into_response();
            };
            message.text = body["text"].as_str().unwrap_or_default().to_owned();
            message.buttons = tg_buttons(&body);
            message.edits += 1;
            ok(json!({"message_id": id}))
        }
        "answerCallbackQuery" => ok(json!(true)),
        "getFile" => {
            let id = body["file_id"].as_str().unwrap_or_default();
            if lock(&state.inner).files.contains_key(id) {
                ok(json!({"file_path": format!("voice/{id}.oga")}))
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"ok": false, "description": "file not found"})),
                )
                    .into_response()
            }
        }
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({"ok": false, "description": "unknown method"})),
        )
            .into_response(),
    }
}

fn tg_buttons(body: &Value) -> Vec<(String, String)> {
    body["reply_markup"]["inline_keyboard"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row.as_array().into_iter().flatten())
        .map(|button| {
            (
                button["text"].as_str().unwrap_or_default().to_owned(),
                button["callback_data"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

async fn tg_file(
    State(state): State<Arc<TgState>>,
    Path((bot, path)): Path<(String, String)>,
) -> Response {
    if bot != format!("bot{}", state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = path.trim_start_matches("voice/").trim_end_matches(".oga");
    match lock(&state.inner).files.get(id) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ------------------------------------------------------------------- Slack

#[derive(Clone, Debug)]
pub struct SlackMsg {
    pub ts: String,
    pub channel: String,
    pub thread_ts: Option<String>,
    pub text: String,
    /// (label, action_id)
    pub buttons: Vec<(String, String)>,
    pub edits: u32,
}

#[derive(Default)]
struct SlackInner {
    next_ts: u64,
    messages: Vec<SlackMsg>,
    calls: Vec<(String, Value)>,
    files: HashMap<String, Vec<u8>>,
    file_auth: Vec<Option<String>>,
}

struct SlackState {
    token: String,
    inner: Mutex<SlackInner>,
}

pub struct FakeSlack {
    pub base: String,
    state: Arc<SlackState>,
}

impl FakeSlack {
    pub async fn start(token: &str) -> Self {
        let state = Arc::new(SlackState {
            token: token.to_owned(),
            inner: Mutex::new(SlackInner {
                next_ts: 1_700_000_000,
                ..SlackInner::default()
            }),
        });
        let router = Router::new()
            .route("/api/{method}", post(slack_method))
            .route("/files/{name}", get(slack_file))
            .with_state(state.clone());
        let address = serve(router).await;
        Self {
            base: format!("http://{address}"),
            state,
        }
    }

    pub fn api_base(&self) -> String {
        format!("{}/api", self.base)
    }

    pub fn put_file(&self, name: &str, bytes: &[u8]) -> String {
        lock(&self.state.inner)
            .files
            .insert(name.to_owned(), bytes.to_vec());
        format!("{}/files/{name}", self.base)
    }

    pub fn messages(&self) -> Vec<SlackMsg> {
        lock(&self.state.inner).messages.clone()
    }

    pub fn calls(&self) -> usize {
        lock(&self.state.inner).calls.len()
    }

    pub fn file_auth_headers(&self) -> Vec<Option<String>> {
        lock(&self.state.inner).file_auth.clone()
    }

    pub async fn wait_text(&self, needle: &str) -> SlackMsg {
        let needle = needle.to_owned();
        eventually(
            &format!("slack message containing {needle:?}"),
            WAIT,
            || {
                self.messages()
                    .into_iter()
                    .find(|m| m.text.contains(&needle))
            },
        )
        .await
    }
}

async fn slack_method(
    State(state): State<Arc<SlackState>>,
    headers: axum::http::HeaderMap,
    Path(method): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let authorized = headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {}", state.token));
    if !authorized {
        return Json(json!({"ok": false, "error": "invalid_auth"})).into_response();
    }
    let mut inner = lock(&state.inner);
    inner.calls.push((method.clone(), body.clone()));
    let buttons = |body: &Value| -> Vec<(String, String)> {
        body["blocks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "actions")
            .flat_map(|block| block["elements"].as_array().into_iter().flatten())
            .map(|e| {
                (
                    e["text"]["text"].as_str().unwrap_or_default().to_owned(),
                    e["action_id"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    };
    match method.as_str() {
        "chat.postMessage" => {
            inner.next_ts += 1;
            let ts = format!("{}.000100", inner.next_ts);
            let message = SlackMsg {
                ts: ts.clone(),
                channel: body["channel"].as_str().unwrap_or_default().to_owned(),
                thread_ts: body["thread_ts"].as_str().map(str::to_owned),
                text: body["text"].as_str().unwrap_or_default().to_owned(),
                buttons: buttons(&body),
                edits: 0,
            };
            inner.messages.push(message);
            Json(json!({"ok": true, "ts": ts})).into_response()
        }
        "chat.update" => {
            let ts = body["ts"].as_str().unwrap_or_default();
            let Some(message) = inner.messages.iter_mut().find(|m| m.ts == ts) else {
                return Json(json!({"ok": false, "error": "message_not_found"})).into_response();
            };
            message.text = body["text"].as_str().unwrap_or_default().to_owned();
            message.buttons = buttons(&body);
            message.edits += 1;
            Json(json!({"ok": true, "ts": ts})).into_response()
        }
        _ => Json(json!({"ok": false, "error": "unknown_method"})).into_response(),
    }
}

async fn slack_file(
    State(state): State<Arc<SlackState>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut inner = lock(&state.inner);
    inner.file_auth.push(auth);
    match inner.files.get(&name) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// POST a signed Slack request; returns the HTTP status.
pub async fn slack_post(
    addr: SocketAddr,
    path: &str,
    secret: &str,
    timestamp: i64,
    content_type: &str,
    body: String,
) -> u16 {
    let signature = openside_channels::slack::sign(secret, &timestamp.to_string(), body.as_bytes());
    slack_post_raw(
        addr,
        path,
        &timestamp.to_string(),
        &signature,
        content_type,
        body,
    )
    .await
}

pub async fn slack_post_raw(
    addr: SocketAddr,
    path: &str,
    timestamp: &str,
    signature: &str,
    content_type: &str,
    body: String,
) -> u16 {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header("content-type", content_type)
        .header("x-slack-request-timestamp", timestamp)
        .header("x-slack-signature", signature)
        .body(body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub fn slack_message_event(
    team: &str,
    channel: &str,
    user: &str,
    ts: &str,
    thread_ts: Option<&str>,
    text: &str,
    channel_type: &str,
) -> String {
    let mut event = json!({"type": "message", "user": user, "text": text, "ts": ts, "channel": channel, "channel_type": channel_type});
    if let Some(thread_ts) = thread_ts {
        event["thread_ts"] = json!(thread_ts);
    }
    json!({"type": "event_callback", "team_id": team, "event_id": format!("Ev{ts}"), "event": event}).to_string()
}

pub fn slack_press_payload(
    team: &str,
    channel: &str,
    user: &str,
    message_ts: &str,
    thread_ts: Option<&str>,
    action_id: &str,
) -> String {
    let mut message = json!({"ts": message_ts});
    if let Some(thread_ts) = thread_ts {
        message["thread_ts"] = json!(thread_ts);
    }
    let payload = json!({"type": "block_actions", "user": {"id": user}, "team": {"id": team}, "channel": {"id": channel},
        "message": message, "actions": [{"action_id": action_id, "value": action_id}]});
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("payload", &payload.to_string())
        .finish()
}

// ----------------------------------------------------------------- Discord

#[derive(Clone, Debug)]
pub struct DiscordMsg {
    pub id: String,
    pub channel: String,
    pub text: String,
    /// (label, custom_id)
    pub buttons: Vec<(String, String)>,
    pub edits: u32,
}

#[derive(Default)]
struct DiscordInner {
    next_id: u64,
    messages: Vec<DiscordMsg>,
    files: HashMap<String, Vec<u8>>,
    file_auth: Vec<Option<String>>,
}

struct DiscordState {
    token: String,
    inner: Mutex<DiscordInner>,
}

pub struct FakeDiscord {
    pub base: String,
    state: Arc<DiscordState>,
}

impl FakeDiscord {
    pub async fn start(token: &str) -> Self {
        let state = Arc::new(DiscordState {
            token: token.to_owned(),
            inner: Mutex::new(DiscordInner {
                next_id: 5000,
                ..DiscordInner::default()
            }),
        });
        let router = Router::new()
            .route("/api/channels/{channel}/messages", post(discord_send))
            .route("/api/channels/{channel}/messages/{id}", patch(discord_edit))
            .route("/cdn/{name}", get(discord_file))
            .with_state(state.clone());
        let address = serve(router).await;
        Self {
            base: format!("http://{address}"),
            state,
        }
    }

    pub fn api_base(&self) -> String {
        format!("{}/api", self.base)
    }

    pub fn put_file(&self, name: &str, bytes: &[u8]) -> String {
        lock(&self.state.inner)
            .files
            .insert(name.to_owned(), bytes.to_vec());
        format!("{}/cdn/{name}", self.base)
    }

    pub fn messages(&self) -> Vec<DiscordMsg> {
        lock(&self.state.inner).messages.clone()
    }

    pub fn file_auth_headers(&self) -> Vec<Option<String>> {
        lock(&self.state.inner).file_auth.clone()
    }

    pub async fn wait_text(&self, needle: &str) -> DiscordMsg {
        let needle = needle.to_owned();
        eventually(
            &format!("discord message containing {needle:?}"),
            WAIT,
            || {
                self.messages()
                    .into_iter()
                    .find(|m| m.text.contains(&needle))
            },
        )
        .await
    }
}

fn discord_buttons(body: &Value) -> Vec<(String, String)> {
    body["components"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row["components"].as_array().into_iter().flatten())
        .map(|b| {
            (
                b["label"].as_str().unwrap_or_default().to_owned(),
                b["custom_id"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

fn discord_authorized(state: &DiscordState, headers: &axum::http::HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(&format!("Bot {}", state.token))
}

async fn discord_send(
    State(state): State<Arc<DiscordState>>,
    headers: axum::http::HeaderMap,
    Path(channel): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if !discord_authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut inner = lock(&state.inner);
    inner.next_id += 1;
    let id = inner.next_id.to_string();
    let message = DiscordMsg {
        id: id.clone(),
        channel,
        text: body["content"].as_str().unwrap_or_default().to_owned(),
        buttons: discord_buttons(&body),
        edits: 0,
    };
    inner.messages.push(message);
    Json(json!({"id": id})).into_response()
}

async fn discord_edit(
    State(state): State<Arc<DiscordState>>,
    headers: axum::http::HeaderMap,
    Path((_channel, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if !discord_authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut inner = lock(&state.inner);
    let Some(message) = inner.messages.iter_mut().find(|m| m.id == id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    message.text = body["content"].as_str().unwrap_or_default().to_owned();
    message.buttons = discord_buttons(&body);
    message.edits += 1;
    Json(json!({"id": id})).into_response()
}

async fn discord_file(
    State(state): State<Arc<DiscordState>>,
    headers: axum::http::HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut inner = lock(&state.inner);
    inner.file_auth.push(auth);
    match inner.files.get(&name) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// POST a signed Discord interaction; returns status and JSON body.
pub async fn discord_post(
    addr: SocketAddr,
    signing: &ed25519_dalek::SigningKey,
    body: &Value,
) -> (u16, Value) {
    use ed25519_dalek::Signer;
    let timestamp = now_secs().to_string();
    let body = body.to_string();
    let signature = signing.sign(&[timestamp.as_bytes(), body.as_bytes()].concat());
    discord_post_raw(addr, &timestamp, &hex(&signature.to_bytes()), body).await
}

pub async fn discord_post_raw(
    addr: SocketAddr,
    timestamp: &str,
    signature_hex: &str,
    body: String,
) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/discord/interactions"))
        .header("content-type", "application/json")
        .header("x-signature-timestamp", timestamp)
        .header("x-signature-ed25519", signature_hex)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn discord_command(
    id: &str,
    guild: &str,
    channel: &str,
    user: &str,
    name: &str,
    options: Value,
    resolved: Value,
) -> Value {
    json!({"type": 2, "id": id, "guild_id": guild, "channel_id": channel, "member": {"user": {"id": user}},
        "data": {"name": name, "options": options, "resolved": resolved}})
}

pub fn discord_press(
    guild: &str,
    channel: &str,
    user: &str,
    message_id: &str,
    custom_id: &str,
) -> Value {
    json!({"type": 3, "id": "press", "guild_id": guild, "channel_id": channel, "member": {"user": {"id": user}},
        "message": {"id": message_id}, "data": {"custom_id": custom_id}})
}

// ------------------------------------------------------------ daemon proxy

/// Reverse proxy that records every `(method, path-without-query)` it forwards to the
/// real daemon, so a test can prove the bot only used public API routes.
pub struct RecordingProxy {
    pub url: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

struct ProxyState {
    upstream: String,
    client: reqwest::Client,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl RecordingProxy {
    pub async fn start(upstream: SocketAddr) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(ProxyState {
            upstream: format!("http://{upstream}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            seen: seen.clone(),
        });
        let router = Router::new().fallback(forward).with_state(state);
        let address = serve(router).await;
        Self {
            url: format!("http://{address}"),
            seen,
        }
    }

    pub fn requests(&self) -> Vec<(String, String)> {
        lock(&self.seen).clone()
    }
}

async fn forward(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().to_owned();
    lock(&state.seen).push((parts.method.to_string(), path));
    let target = format!(
        "{}{}",
        state.upstream,
        parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
    );
    let bytes = axum::body::to_bytes(body, 8 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let mut upstream = state.client.request(parts.method, target).body(bytes);
    for (name, value) in &parts.headers {
        if name != "host" && name != "content-length" {
            upstream = upstream.header(name, value);
        }
    }
    let Ok(response) = upstream.send().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut builder = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if name != "transfer-encoding" && name != "content-length" {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Body::from_stream(response.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// Whether `(method, path)` matches a documented public API route.
pub fn is_public_route(method: &str, path: &str) -> bool {
    openside_proto::ROUTES.iter().any(|route| {
        let template: Vec<_> = route.path.split('/').collect();
        let actual: Vec<_> = path.split('/').collect();
        route.method.as_str() == method
            && template.len() == actual.len()
            && template
                .iter()
                .zip(&actual)
                .all(|(t, a)| (t.starts_with('{') && !a.is_empty()) || t == a)
    })
}

/// Await `future` with a deadline, for readable timeouts in tests.
pub async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future)
        .await
        .unwrap_or_else(|_| panic!("timed out: {what}"))
}
