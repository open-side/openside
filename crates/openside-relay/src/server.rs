//! Pairs one registered daemon WebSocket with many device WebSockets and forwards device
//! ciphertext unchanged, without the device-to-daemon Noise transport keys. A separate,
//! domain-separated Noise handshake proves registration ownership. Routing bindings live in
//! memory; browser clients additionally trust the operator that distributes their code.

use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{
        ws::{
            rejection::WebSocketUpgradeRejection, CloseFrame, Message, WebSocket, WebSocketUpgrade,
        },
        Path as UrlPath, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};

use crate::{
    client::valid_daemon_id,
    noise::{self, MAX_MESSAGE},
};

/// Relay → daemon: a device connected as `conn`.
pub const OPEN: u8 = 1;
/// Either direction: one device WebSocket message.
pub const DATA: u8 = 2;
/// Either direction: the device connection ended (or must end).
pub const CLOSE: u8 = 3;

const CSP: &str =
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
connect-src 'self'; manifest-src 'self'; worker-src 'self'; font-src 'self'; object-src 'none'; \
base-uri 'none'; form-action 'self'; frame-src 'self' blob:; frame-ancestors 'none'";

/// Relay settings.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Registration credentials: each token owns one proven daemon id for this process lifetime.
    /// Empty refuses every daemon; use a separate token per daemon, even under one administrator.
    /// In multi-tenant deployments, never share credentials between tenants. A token has two
    /// socket permits (one live connection plus one reconnect proof), capped globally at 256.
    /// Restarting clears token bindings, but never bypasses the daemon's private-key proof.
    pub daemon_tokens: Vec<String>,
    /// Built web app (`web/dist`) to serve to phones, so the PWA and its WebSocket share
    /// one origin.
    pub web_dir: Option<PathBuf>,
    /// Device connections per daemon.
    pub max_clients_per_daemon: usize,
    /// Bytes buffered towards one device before it is dropped as too slow. Keep it above
    /// a few stream windows (256 KiB each), or a device that reads normally but has several
    /// busy streams gets dropped too.
    pub client_buffer: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            daemon_tokens: Vec::new(),
            web_dir: None,
            max_clients_per_daemon: 32,
            client_buffer: 8 << 20,
        }
    }
}

/// Wire frame between relay and daemon: `[kind][conn u32 BE][payload]`.
pub fn frame(kind: u8, conn: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&conn.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

const MAX_DAEMONS: usize = 256;
const MAX_CLIENTS: usize = 1024;
const QUEUE_ENTRIES: usize = 256;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

async fn client_lifetime(
    writer: impl Future<Output = ()>,
    reader: impl Future<Output = ()>,
    stopped: &mut watch::Receiver<u16>,
    daemon_stopped: &mut watch::Receiver<bool>,
) {
    if *stopped.borrow() != 0 || *daemon_stopped.borrow() {
        return;
    }
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
        _ = stopped.changed() => {}
        _ = daemon_stopped.changed() => {}
    }
}
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);

struct ClientLink {
    tx: mpsc::Sender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    stop: watch::Sender<u16>,
}

impl Drop for ClientLink {
    fn drop(&mut self) {
        if *self.stop.borrow() == 0 {
            self.stop.send_replace(1000);
        }
    }
}

struct DaemonLink {
    tx: mpsc::Sender<Vec<u8>>,
    control: mpsc::Sender<Vec<u8>>,
    clients: Mutex<HashMap<u32, ClientLink>>,
    admission: Arc<Semaphore>,
    next_conn: AtomicU32,
    stop: watch::Sender<bool>,
}

impl DaemonLink {
    fn drop_client(&self, conn: u32, code: u16) -> bool {
        match self.clients.lock().remove(&conn) {
            Some(client) => {
                client.stop.send_replace(code);
                true
            }
            None => false,
        }
    }

    fn control(&self, kind: u8, conn: u32) -> bool {
        if self.control.try_send(frame(kind, conn, &[])).is_err() {
            // Losing a CLOSE would leave host resources alive. End the bounded session instead.
            self.stop.send_replace(true);
            false
        } else {
            true
        }
    }
}

#[derive(Default)]
struct RegistrationPolicy {
    owners: HashMap<String, usize>,
    tokens: HashMap<usize, String>,
}

impl RegistrationPolicy {
    fn bind(&mut self, token: usize, id: &str) -> bool {
        if self.owners.get(id).is_some_and(|owner| *owner != token)
            || self.tokens.get(&token).is_some_and(|bound| bound != id)
            || (!self.owners.contains_key(id) && self.owners.len() >= MAX_DAEMONS)
        {
            return false;
        }
        self.owners.insert(id.to_owned(), token);
        self.tokens.insert(token, id.to_owned());
        true
    }
}

struct Relay {
    config: ServerConfig,
    daemons: Mutex<HashMap<String, Arc<DaemonLink>>>,
    policy: Mutex<RegistrationPolicy>,
    daemon_admission: Arc<Semaphore>,
    token_admission: Vec<Arc<Semaphore>>,
    client_admission: Arc<Semaphore>,
}

/// The relay's HTTP surface (see `docs/REMOTE.md`).
pub fn router(config: ServerConfig) -> Router {
    let token_admission = config
        .daemon_tokens
        .iter()
        .map(|_| Arc::new(Semaphore::new(2)))
        .collect();
    let relay = Arc::new(Relay {
        config,
        daemons: Mutex::new(HashMap::new()),
        policy: Mutex::new(RegistrationPolicy::default()),
        daemon_admission: Arc::new(Semaphore::new(MAX_DAEMONS)),
        token_admission,
        client_admission: Arc::new(Semaphore::new(MAX_CLIENTS)),
    });
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/relay/daemon/{id}", get(register))
        .route("/v1/relay/connect/{id}", get(connect))
        .fallback(get(static_file))
        .with_state(relay)
}

fn error(status: StatusCode, code: &'static str) -> Response {
    (status, Json(code_body(code))).into_response()
}

// `{"code": …}` like the daemon's errors, without a serde_json dependency.
fn code_body(code: &'static str) -> HashMap<&'static str, &'static str> {
    HashMap::from([("code", code)])
}

fn authorized(relay: &Relay, headers: &HeaderMap) -> Option<usize> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))?;
    let mut matched = None;
    // Check every configured token; duplicate credentials remain the same principal.
    for (index, candidate) in relay.config.daemon_tokens.iter().enumerate() {
        if bool::from(candidate.as_bytes().ct_eq(token.as_bytes())) && matched.is_none() {
            matched = Some(index);
        }
    }
    matched
}

fn upgrade(ws: WebSocketUpgrade) -> WebSocketUpgrade {
    // Larger than any Noise message, so an oversized message reaches the handler and is
    // refused with 1009 instead of a protocol error.
    ws.max_message_size(4 * MAX_MESSAGE)
        .max_frame_size(4 * MAX_MESSAGE)
}

async fn register(
    State(relay): State<Arc<Relay>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(token) = authorized(&relay, &headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !valid_daemon_id(&id) {
        return error(StatusCode::BAD_REQUEST, "invalid_daemon_id");
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    let Ok(permit) = relay.daemon_admission.clone().try_acquire_owned() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "too_many_daemons");
    };
    let Ok(token_permit) = relay.token_admission[token].clone().try_acquire_owned() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "too_many_daemons");
    };
    upgrade(ws)
        .on_upgrade(move |socket| daemon_session(relay, id, token, socket, permit, token_permit))
        .into_response()
}

async fn prove_identity(socket: &mut WebSocket, id: &str) -> bool {
    let proof = async {
        let Some(Ok(Message::Binary(key))) = socket.next().await else {
            return false;
        };
        let Ok(key) = <[u8; 32]>::try_from(key.as_ref()) else {
            return false;
        };
        if noise::daemon_id(&key) != id {
            return false;
        }
        let Ok((state, challenge)) = noise::initiate_registration(id, &key) else {
            return false;
        };
        if socket
            .send(Message::Binary(challenge.into()))
            .await
            .is_err()
        {
            return false;
        }
        let Some(Ok(Message::Binary(reply))) = socket.next().await else {
            return false;
        };
        noise::finish(state, &reply).is_ok()
    };
    tokio::time::timeout(REGISTRATION_TIMEOUT, proof)
        .await
        .unwrap_or(false)
}

async fn daemon_session(
    relay: Arc<Relay>,
    id: String,
    token: usize,
    mut socket: WebSocket,
    _permit: OwnedSemaphorePermit,
    _token_permit: OwnedSemaphorePermit,
) {
    if !prove_identity(&mut socket, &id).await || !relay.policy.lock().bind(token, &id) {
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, socket.close()).await;
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE_ENTRIES);
    let (control, mut control_rx) = mpsc::channel::<Vec<u8>>(QUEUE_ENTRIES);
    let (stop, mut stopped) = watch::channel(false);
    let link = Arc::new(DaemonLink {
        tx,
        control,
        clients: Mutex::new(HashMap::new()),
        admission: Arc::new(Semaphore::new(relay.config.max_clients_per_daemon)),
        next_conn: AtomicU32::new(1),
        stop,
    });
    if let Some(old) = relay.daemons.lock().insert(id.clone(), link.clone()) {
        old.stop.send_replace(true);
        old.clients.lock().clear();
    }
    let limit = relay.config.client_buffer.max(MAX_MESSAGE);
    let writer = async {
        if !matches!(
            tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Binary(vec![0].into()))).await,
            Ok(Ok(()))
        ) {
            return;
        }
        loop {
            let message = tokio::select! {
                biased;
                message = control_rx.recv() => message,
                message = rx.recv() => message,
            };
            let Some(message) = message else { break };
            if !matches!(
                tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Binary(message.into())))
                    .await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    };
    let reader = async {
        while let Some(Ok(message)) = stream.next().await {
            let bytes = match message {
                Message::Binary(bytes) => bytes,
                Message::Close(_) => break,
                _ => continue,
            };
            if bytes.len() < 5 || bytes.len() > MAX_MESSAGE + 5 {
                break;
            }
            let conn = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
            match bytes[0] {
                DATA => {
                    let sent = link.clients.lock().get(&conn).map(|client| {
                        let len = bytes.len() - 5;
                        let queued = client.queued.fetch_add(len, Ordering::AcqRel);
                        if len == 0
                            || queued.saturating_add(len) > limit
                            || client.tx.try_send(bytes[5..].to_vec()).is_err()
                        {
                            client.queued.fetch_sub(len, Ordering::AcqRel);
                            false
                        } else {
                            true
                        }
                    });
                    if sent == Some(false) && link.drop_client(conn, 1013) {
                        link.control(CLOSE, conn);
                    }
                }
                CLOSE => {
                    link.drop_client(conn, 1000);
                }
                _ => break,
            }
        }
    };
    tokio::select! {
        _ = writer => {}
        _ = reader => {}
        _ = stopped.changed() => {}
    }
    {
        let mut daemons = relay.daemons.lock();
        if daemons
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, &link))
        {
            daemons.remove(&id);
        }
    }
    link.stop.send_replace(true);
    link.clients.lock().clear();
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.close()).await;
}

struct ClientAdmission {
    _global: OwnedSemaphorePermit,
    _daemon: OwnedSemaphorePermit,
}

fn admit_client(relay: &Relay, link: &DaemonLink) -> Option<ClientAdmission> {
    let global = relay.client_admission.clone().try_acquire_owned().ok()?;
    let daemon = link.admission.clone().try_acquire_owned().ok()?;
    Some(ClientAdmission {
        _global: global,
        _daemon: daemon,
    })
}

async fn connect(
    State(relay): State<Arc<Relay>>,
    UrlPath(id): UrlPath<String>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(link) = relay.daemons.lock().get(&id).cloned() else {
        return error(StatusCode::NOT_FOUND, "daemon_offline");
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    let Some(admission) = admit_client(&relay, &link) else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "too_many_clients");
    };
    upgrade(ws)
        .on_upgrade(move |socket| client_session(link, socket, admission))
        .into_response()
}

async fn client_session(link: Arc<DaemonLink>, socket: WebSocket, _admission: ClientAdmission) {
    let mut conn = link.next_conn.load(Ordering::Relaxed);
    loop {
        let Some(next) = conn.checked_add(1) else {
            link.stop.send_replace(true);
            return;
        };
        match link
            .next_conn
            .compare_exchange_weak(conn, next, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(current) => conn = current,
        }
    }
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE_ENTRIES);
    let queued = Arc::new(AtomicUsize::new(0));
    let (stop, mut stopped) = watch::channel(0_u16);
    let mut daemon_stopped = link.stop.subscribe();
    if *daemon_stopped.borrow() {
        return;
    }
    link.clients.lock().insert(
        conn,
        ClientLink {
            tx,
            queued: queued.clone(),
            stop,
        },
    );
    if !link.control(OPEN, conn) {
        link.drop_client(conn, 1000);
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let writer = async {
        while let Some(message) = rx.recv().await {
            let len = message.len();
            let sent =
                tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::Binary(message.into())))
                    .await;
            queued.fetch_sub(len, Ordering::AcqRel);
            if !matches!(sent, Ok(Ok(()))) {
                break;
            }
        }
    };
    let reader = async {
        while let Some(Ok(message)) = stream.next().await {
            match message {
                Message::Binary(bytes) if bytes.len() > MAX_MESSAGE => {
                    link.drop_client(conn, 1009);
                    break;
                }
                Message::Binary(bytes) => {
                    if bytes.is_empty() || link.tx.try_send(frame(DATA, conn, &bytes)).is_err() {
                        break;
                    }
                }
                Message::Text(_) => {
                    link.drop_client(conn, 1003);
                    break;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    client_lifetime(writer, reader, &mut stopped, &mut daemon_stopped).await;
    if link.drop_client(conn, 1000) {
        link.control(CLOSE, conn);
    }
    let code = *stopped.borrow();
    let _ = tokio::time::timeout(
        CLOSE_TIMEOUT,
        sink.send(Message::Close(Some(CloseFrame {
            code: if code == 0 { 1000 } else { code },
            reason: "".into(),
        }))),
    )
    .await;
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

async fn read_inside(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let path = tokio::fs::canonicalize(root.join(relative)).await.ok()?;
    if !path.starts_with(&root) || !tokio::fs::metadata(&path).await.ok()?.is_file() {
        return None;
    }
    tokio::fs::read(path).await.ok()
}

async fn static_file(State(relay): State<Arc<Relay>>, uri: Uri) -> Response {
    let Some(root) = relay.config.web_dir.as_deref() else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("v1/") || path == "v1" {
        return error(StatusCode::NOT_FOUND, "not_found");
    }
    if path.split('/').any(|part| part == "..") || path.contains(['\\', '\0', '%']) {
        return error(StatusCode::BAD_REQUEST, "invalid_path");
    }
    let last = path.rsplit('/').next().unwrap_or_default();
    let (name, body) =
        match read_inside(root, if path.is_empty() { "index.html" } else { path }).await {
            Some(body) => (if path.is_empty() { "index.html" } else { path }, body),
            // Client-side routes (`/pair`, `/sessions/…`) fall back to the app shell.
            None if !last.contains('.') => match read_inside(root, "index.html").await {
                Some(body) => ("index.html", body),
                None => return error(StatusCode::NOT_FOUND, "not_found"),
            },
            None => return secured(error(StatusCode::NOT_FOUND, "not_found")),
        };
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    // The generic PWA must not offer direct token login at a relay-only origin.
    let body = if name == "index.html" {
        String::from_utf8_lossy(&body)
            .replacen(
                "<head>",
                "<head><meta name=\"openside-transport\" content=\"relay\">",
                1,
            )
            .into_bytes()
    } else {
        body
    };
    secured(
        Response::builder()
            .header(header::CONTENT_TYPE, content_type(name))
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(body))
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn fixture(global: usize, daemon: usize) -> (Relay, Arc<DaemonLink>) {
        let (tx, _) = mpsc::channel(1);
        let (control, _) = mpsc::channel(1);
        let (stop, _) = watch::channel(false);
        let link = Arc::new(DaemonLink {
            tx,
            control,
            stop,
            clients: Mutex::new(HashMap::new()),
            admission: Arc::new(Semaphore::new(daemon)),
            next_conn: AtomicU32::new(1),
        });
        let relay = Relay {
            config: ServerConfig::default(),
            daemons: Mutex::new(HashMap::new()),
            policy: Mutex::new(RegistrationPolicy::default()),
            daemon_admission: Arc::new(Semaphore::new(1)),
            client_admission: Arc::new(Semaphore::new(global)),
            token_admission: Vec::new(),
        };
        (relay, link)
    }

    #[test]
    fn registration_ownership_and_per_token_quota_are_enforced() {
        let mut policy = RegistrationPolicy::default();
        assert!(policy.bind(0, "first"));
        assert!(policy.bind(0, "first"));
        assert!(!policy.bind(1, "first"));
        assert!(!policy.bind(0, "second"));
        assert!(policy.bind(1, "second"));
        for token in 2..MAX_DAEMONS {
            assert!(policy.bind(token, &format!("daemon-{token}")));
        }
        assert!(!policy.bind(MAX_DAEMONS, "extra"));
    }

    #[tokio::test]
    async fn concurrent_admission_reserves_before_upgrade_and_failed_upgrade_releases() {
        let (relay, link) = fixture(3, 2);
        let relay = Arc::new(relay);
        let mut tasks = tokio::task::JoinSet::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        for _ in 0..8 {
            let (relay, link, barrier) = (relay.clone(), link.clone(), barrier.clone());
            tasks.spawn(async move {
                barrier.wait().await;
                admit_client(&relay, &link)
            });
        }
        let mut reservations = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Some(reservation) = result.unwrap() {
                reservations.push(reservation);
            }
        }
        assert_eq!(reservations.len(), 2);
        assert_eq!(relay.client_admission.available_permits(), 1);
        assert_eq!(link.admission.available_permits(), 0);
        let reservation = reservations.pop().unwrap();
        let failed_upgrade_callback = move || drop(reservation);
        drop(failed_upgrade_callback);
        assert_eq!(link.admission.available_permits(), 1);
        drop(reservations);
        assert_eq!(relay.client_admission.available_permits(), 3);
        let (relay, link) = fixture(1, 3);
        let admission = admit_client(&relay, &link).unwrap();
        assert!(admit_client(&relay, &link).is_none());
        assert_eq!(link.admission.available_permits(), 2);
        drop(admission);
        assert_eq!(relay.client_admission.available_permits(), 1);
    }

    #[tokio::test]
    async fn removal_or_daemon_shutdown_cancels_stalled_halves_and_returns_permits() {
        for daemon_close in [false, true] {
            let (relay, link) = fixture(1, 1);
            let admission = admit_client(&relay, &link).unwrap();
            let (tx, _rx) = mpsc::channel(1);
            let (stop, mut stopped) = watch::channel(0);
            let mut daemon_stopped = link.stop.subscribe();
            link.clients.lock().insert(
                1,
                ClientLink {
                    tx,
                    queued: Arc::new(AtomicUsize::new(0)),
                    stop,
                },
            );
            let task = tokio::spawn(async move {
                let _admission = admission;
                client_lifetime(
                    std::future::pending(),
                    std::future::pending(),
                    &mut stopped,
                    &mut daemon_stopped,
                )
                .await;
            });
            if daemon_close {
                link.stop.send_replace(true);
            } else {
                assert!(link.drop_client(1, 1013));
            }
            tokio::time::timeout(Duration::from_millis(100), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(relay.client_admission.available_permits(), 1);
            assert_eq!(link.admission.available_permits(), 1);
        }
    }

    #[test]
    fn close_control_does_not_wait_behind_a_full_data_queue() {
        let (_, mut link) = fixture(1, 1);
        let (tx, _rx) = mpsc::channel(1);
        let (control, mut control_rx) = mpsc::channel(1);
        let link = Arc::get_mut(&mut link).unwrap();
        link.tx = tx;
        link.control = control;
        link.tx.try_send(vec![1]).unwrap();
        assert!(link.control(CLOSE, 1));
        assert_eq!(control_rx.try_recv().unwrap(), frame(CLOSE, 1, &[]));
        assert!(!*link.stop.borrow());
    }
}
