use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    Extension, Json,
};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use openside_import::{DataKind, DiscoveredProfile, ProfileRead};
use openside_proto::{
    ImportDeliveryAck, ImportDeliveryBatch, ImportDeliveryBookmark, ImportDeliveryCookie,
    ImportDeliveryList, ImportDeliveryPending, ImportDeliveryQuery, ImportDeliverySigned,
    ImportDeliverySource, ImportDeliveryTarget, ImportDeliveryTicket, ImportDeliveryTicketRequest,
    ImportDeliveryVisit,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    api::HttpError,
    engine::Shared,
    persistence::{random_id, write_private},
    remote::LocalAuthority,
};

static STORE_GATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
const MAX_DOCUMENT: u64 = 256 * 1024 * 1024;
const BATCH_ITEMS: usize = 250;

#[derive(Default)]
pub(crate) struct DeliveryState {
    tickets: parking_lot::Mutex<BTreeMap<String, DeliveryTicket>>,
}

struct DeliveryTicket {
    device: String,
    batch: String,
    deadline: Instant,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

fn pairing_path(root: &Path, device: &str) -> Result<PathBuf, HttpError> {
    batch_path(root, device)?;
    Ok(openside_config::data_layout::DataPath::ExtensionImportKeys
        .path(root)
        .join(device))
}

pub(crate) async fn create_pairing_secret(
    shared: &Shared,
    device: &str,
) -> Result<String, HttpError> {
    let path = pairing_path(&shared.config.data_dir, device)?;
    tokio::task::spawn_blocking(move || {
        let parent = path.parent().ok_or_else(HttpError::internal)?;
        fs::create_dir_all(parent).map_err(|_| HttpError::internal())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|_| HttpError::internal())?;
        }
        let mut key = Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, key.as_mut());
        write_private(&path, key.as_ref()).map_err(|_| HttpError::internal())?;
        Ok(hex(key.as_ref()))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

fn pairing_secret(root: &Path, device: &str) -> Result<Zeroizing<[u8; 32]>, HttpError> {
    let path = pairing_path(root, device)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|_| HttpError(StatusCode::FORBIDDEN, "import_pairing_required"))?;
    let metadata = file.metadata().map_err(|_| HttpError::internal())?;
    if !metadata.is_file() || metadata.len() != 32 {
        return Err(HttpError::internal());
    }
    let mut key = Zeroizing::new([0u8; 32]);
    file.read_exact(key.as_mut())
        .map_err(|_| HttpError::internal())?;
    Ok(key)
}

pub(crate) async fn pairing_continuity_proof(
    shared: &Shared,
    headers: &HeaderMap,
    payload: &[u8],
) -> Result<Option<String>, HttpError> {
    let Some(device) = headers
        .get("x-openside-import-device")
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    let Some(nonce) = headers
        .get("x-openside-import-nonce")
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    if device.len() != 32
        || nonce.len() != 64
        || !device
            .bytes()
            .chain(nonce.bytes())
            .all(|byte| byte.is_ascii_hexdigit())
        || shared.remote.is_revoked(device)
        || !shared
            .db
            .devices()
            .await
            .map_err(|_| HttpError::internal())?
            .iter()
            .any(|paired| paired.device_id == device)
    {
        return Ok(None);
    }
    let root = shared.config.data_dir.clone();
    let previous = device.to_owned();
    let key = tokio::task::spawn_blocking(move || pairing_secret(&root, &previous).ok())
        .await
        .map_err(|_| HttpError::internal())?;
    let Some(key) = key else {
        return Ok(None);
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_ref()).map_err(|_| HttpError::internal())?;
    // Bind the entire replacement response and domain-separate this proof from native import signatures.
    mac.update(b"openside-import-pairing-v1\n");
    mac.update(device.as_bytes());
    mac.update(nonce.as_bytes());
    mac.update(&Sha256::digest(payload));
    Ok(Some(hex(&mac.finalize().into_bytes())))
}

pub(crate) async fn forget_device(shared: &Shared, device: &str) -> Result<(), HttpError> {
    shared
        .import_delivery
        .tickets
        .lock()
        .retain(|_, ticket| ticket.device != device);
    let path = pairing_path(&shared.config.data_dir, device)?;
    tokio::task::spawn_blocking(move || match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(HttpError::internal()),
    })
    .await
    .map_err(|_| HttpError::internal())?
}

fn wipe_json(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(wipe_json),
        Value::Object(values) => values.values_mut().for_each(wipe_json),
        _ => {}
    }
}

struct SecretJson(Value);
impl std::ops::Deref for SecretJson {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl std::ops::DerefMut for SecretJson {
    fn deref_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}
impl Drop for SecretJson {
    fn drop(&mut self) {
        wipe_json(&mut self.0);
    }
}

#[derive(Serialize, Deserialize)]
struct CookieKey {
    host: String,
    name: String,
    path: String,
}

#[derive(Serialize, Deserialize)]
struct StoredBatch {
    id: String,
    source: ImportDeliverySource,
    bookmarks: Vec<ImportDeliveryBookmark>,
    history: Vec<ImportDeliveryVisit>,
    cookie_keys: Vec<CookieKey>,
    user_ack: bool,
    agent_ack: bool,
}

fn directory(root: &Path) -> PathBuf {
    openside_config::data_layout::DataPath::BrowserImports
        .path(root)
        .join("default/delivery")
}

fn batch_path(root: &Path, id: &str) -> Result<PathBuf, HttpError> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HttpError(StatusCode::NOT_FOUND, "unknown_import_batch"));
    }
    Ok(directory(root).join(format!("{id}.json")))
}

fn read_document<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, HttpError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| HttpError::internal())?;
    let metadata = file.metadata().map_err(|_| HttpError::internal())?;
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT {
        return Err(HttpError::internal());
    }
    serde_json::from_reader(file.take(MAX_DOCUMENT)).map_err(|_| HttpError::internal())
}

fn stored(root: &Path) -> Result<Vec<StoredBatch>, HttpError> {
    let entries = match fs::read_dir(directory(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(HttpError::internal()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| HttpError::internal())?;
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    paths.into_iter().map(|path| read_document(&path)).collect()
}

fn save(root: &Path, batch: &StoredBatch) -> Result<(), HttpError> {
    let directory = directory(root);
    fs::create_dir_all(&directory).map_err(|_| HttpError::internal())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| HttpError::internal())?;
    }
    write_private(
        &batch_path(root, &batch.id)?,
        &serde_json::to_vec(batch).map_err(|_| HttpError::internal())?,
    )
    .map_err(|_| HttpError::internal())
}

pub(crate) async fn stage(
    shared: &Shared,
    profile: &DiscoveredProfile,
    read: &mut ProfileRead,
) -> Result<(), HttpError> {
    let cookie_keys = apply_agent_cookies(shared, read).await;
    crate::connections::imported(
        shared,
        cookie_keys
            .iter()
            .map(|key| (key.host.as_str(), key.name.as_str(), key.path.as_str())),
    )
    .await
    .map_err(|_| HttpError::internal())?;
    let bookmarks: Vec<_> = if read
        .reports
        .get(&DataKind::Bookmarks)
        .is_some_and(|report| report.note.is_none())
    {
        read.data
            .bookmarks
            .drain(..)
            .map(|item| ImportDeliveryBookmark {
                url: item.url,
                title: item.title,
                folder: item.folder,
                added_at_ms: item.added_at_ms,
            })
            .collect()
    } else {
        Vec::new()
    };
    let history: Vec<_> = if read
        .reports
        .get(&DataKind::History)
        .is_some_and(|report| report.note.is_none())
    {
        read.data
            .history
            .drain(..)
            .map(|item| ImportDeliveryVisit {
                url: item.url,
                title: item.title,
                visited_at_ms: item.visited_at_ms,
            })
            .collect()
    } else {
        Vec::new()
    };
    let source = ImportDeliverySource {
        id: profile.id.clone(),
        brand: profile.brand.clone(),
        name: profile.name.clone(),
    };
    let root = shared.config.data_dir.clone();
    let _gate = STORE_GATE.lock().await;
    tokio::task::spawn_blocking(move || {
        let mut bookmarks = bookmarks.into_iter();
        let mut history = history.into_iter();
        let mut cookie_keys = cookie_keys.into_iter();
        loop {
            let batch = StoredBatch {
                id: random_id(),
                source: source.clone(),
                bookmarks: bookmarks.by_ref().take(BATCH_ITEMS).collect(),
                history: history.by_ref().take(BATCH_ITEMS).collect(),
                cookie_keys: cookie_keys.by_ref().take(BATCH_ITEMS).collect(),
                user_ack: false,
                agent_ack: true,
            };
            if batch.bookmarks.is_empty()
                && batch.history.is_empty()
                && batch.cookie_keys.is_empty()
            {
                break;
            }
            save(&root, &batch)?;
        }
        Ok(())
    })
    .await
    .map_err(|_| HttpError::internal())?
}

struct NativeCdp {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: u64,
    session: Option<String>,
    _write: openside_browser::ProfileWriteGuard,
}

impl NativeCdp {
    async fn connect(shared: &Shared) -> Result<Self, HttpError> {
        let browser = shared
            .registry
            .browser
            .get(shared)
            .await
            .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "browser_unavailable"))?;
        let write = browser
            .profile_write()
            .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "browser_unavailable"))?;
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(15),
            tokio_tungstenite::connect_async(browser.endpoint().ws_url),
        )
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|_| HttpError::internal())?;
        Ok(Self {
            socket,
            next: 0,
            session: None,
            _write: write,
        })
    }
    async fn command(&mut self, method: &str, params: Value) -> Result<SecretJson, HttpError> {
        self.next += 1;
        let id = self.next;
        let request = async {
            let mut request = SecretJson(json!({"id":id,"method":method,"params":params}));
            if let Some(session) = &self.session {
                request["sessionId"] = Value::String(session.clone());
            }
            // WebSocket-owned frames cannot be wiped here; owned parsed values are wiped on every exit.
            self.socket
                .send(Message::text(request.0.to_string()))
                .await
                .map_err(|_| HttpError::internal())?;
            while let Some(frame) = self.socket.next().await {
                let frame = frame.map_err(|_| HttpError::internal())?;
                let Message::Text(text) = frame else { continue };
                let mut reply =
                    SecretJson(serde_json::from_str(&text).map_err(|_| HttpError::internal())?);
                if reply["id"] != id {
                    continue;
                }
                if reply.get("error").is_some() {
                    return Err(HttpError(StatusCode::BAD_REQUEST, "native_cookie_rejected"));
                }
                return Ok(SecretJson(reply["result"].take()));
            }
            Err(HttpError::internal())
        };
        tokio::time::timeout(Duration::from_secs(15), request)
            .await
            .map_err(|_| HttpError::internal())?
    }
}

async fn apply_agent_cookies(shared: &Shared, read: &mut ProfileRead) -> Vec<CookieKey> {
    let Some(report) = read.reports.get_mut(&DataKind::Cookies) else {
        return Vec::new();
    };
    if report.note.is_some() || read.data.cookies.is_empty() {
        return Vec::new();
    }
    report.imported = 0;
    let mut connection = match NativeCdp::connect(shared).await {
        Ok(connection) => connection,
        Err(_) => {
            report.skipped += read.data.cookies.len() as u64;
            report.note = Some("The agent browser is unavailable. Cookies were not imported; other categories are still saved.".into());
            return Vec::new();
        }
    };
    let now = chrono::Utc::now().timestamp();
    let mut keys = Vec::new();
    for (index, cookie) in read.data.cookies.iter().enumerate() {
        let Some(params) = openside_import::cdp_cookie(cookie, now) else {
            report.skipped += 1;
            continue;
        };
        match connection
            .command("Storage.setCookies", json!({"cookies":[params]}))
            .await
        {
            Ok(_) => {
                report.imported += 1;
                keys.push(CookieKey {
                    host: cookie.host.clone(),
                    name: cookie.name.clone(),
                    path: cookie.path.clone(),
                });
            }
            Err(HttpError(StatusCode::BAD_REQUEST, _)) => {
                report.skipped += 1;
                report.rejected.push(openside_import::RejectedCookie {
                    host: cookie.host.clone(),
                    name: cookie.name.clone(),
                });
            }
            Err(_) => {
                report.skipped += (read.data.cookies.len() - index) as u64;
                report.note = Some("The agent browser disconnected before all cookies were imported. Retry the import.".into());
                break;
            }
        }
    }
    let _ = connection.socket.close(None).await;
    keys
}

fn cookie_url(key: &CookieKey, secure: bool) -> Option<String> {
    // HTTPS setting access does not change the source Secure flag.
    let scheme = if secure
        || !matches!(
            key.host.as_str(),
            "localhost" | "127.0.0.1" | "[::1]" | "::1"
        ) {
        "https"
    } else {
        "http"
    };
    url::Url::parse(&format!(
        "{scheme}://{}{}",
        key.host.trim_start_matches('.'),
        key.path
    ))
    .ok()
    .map(Into::into)
}

async fn native_cookies(
    shared: &Shared,
    keys: &[CookieKey],
) -> Result<Vec<ImportDeliveryCookie>, HttpError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut connection = NativeCdp::connect(shared).await?;
    let targets = connection.command("Target.getTargets", json!({})).await?;
    let existing = targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|target| target["type"] == "page" && target["browserContextId"].is_null())
        .and_then(|target| target["targetId"].as_str())
        .map(str::to_owned);
    let temporary = existing.is_none();
    let target = match existing {
        Some(target) => target,
        None => connection
            .command(
                "Target.createTarget",
                json!({"url":"about:blank","background":true}),
            )
            .await?["targetId"]
            .as_str()
            .ok_or_else(HttpError::internal)?
            .to_owned(),
    };
    connection.session = Some(
        connection
            .command(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
            )
            .await?["sessionId"]
            .as_str()
            .ok_or_else(HttpError::internal)?
            .to_owned(),
    );
    let urls: BTreeSet<_> = keys
        .iter()
        .flat_map(|key| [cookie_url(key, true), cookie_url(key, false)])
        .flatten()
        .collect();
    let result = connection
        .command("Network.getCookies", json!({"urls":urls}))
        .await;
    connection.session = None;
    if temporary {
        let _ = connection
            .command("Target.closeTarget", json!({"targetId":target}))
            .await;
    }
    let _ = connection.socket.close(None).await;
    let mut reply = result?;
    let Value::Array(cookies) = reply["cookies"].take() else {
        return Err(HttpError::internal());
    };
    let wanted: BTreeMap<_, _> = keys
        .iter()
        .map(|key| {
            (
                (key.host.as_str(), key.name.as_str(), key.path.as_str()),
                key,
            )
        })
        .collect();
    let mut mapped = Vec::new();
    for cookie in cookies {
        let mut cookie = SecretJson(cookie);
        if cookie.get("partitionKey").is_some()
            || cookie["partitionKeyOpaque"].as_bool().unwrap_or(false)
        {
            continue;
        }
        let scope = (
            cookie["domain"].as_str().unwrap_or_default(),
            cookie["name"].as_str().unwrap_or_default(),
            cookie["path"].as_str().unwrap_or_default(),
        );
        if let Some(key) = wanted.get(&scope) {
            if let Some(cookie) = live_cookie(key, &mut cookie) {
                mapped.push(cookie);
            }
        }
    }
    Ok(mapped)
}

fn live_cookie(key: &CookieKey, cookie: &mut Value) -> Option<ImportDeliveryCookie> {
    let secure = cookie["secure"].as_bool()?;
    let url = cookie_url(key, secure)?;
    let expiration_date = cookie["expires"].as_f64().filter(|expires| *expires > 0.0);
    if expiration_date.is_some_and(|expires| expires <= chrono::Utc::now().timestamp() as f64) {
        return None;
    }
    let same_site = match cookie["sameSite"].as_str() {
        Some("None") => Some("no_restriction".to_owned()),
        Some("Lax") => Some("lax".to_owned()),
        Some("Strict") => Some("strict".to_owned()),
        _ => None,
    };
    let http_only = cookie["httpOnly"].as_bool()?;
    let Value::String(value) = cookie["value"].take() else {
        return None;
    };
    Some(ImportDeliveryCookie {
        url,
        name: key.name.clone(),
        value,
        domain: key.host.starts_with('.').then(|| key.host.clone()),
        path: key.path.clone(),
        secure,
        http_only,
        same_site,
        expiration_date,
        partition_key: None,
    })
}

pub(crate) async fn pending(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Query(query): Query<ImportDeliveryQuery>,
) -> Result<(HeaderMap, Json<ImportDeliveryList>), HttpError> {
    local.require()?;
    let root = shared.config.data_dir.clone();
    let batches = {
        let _gate = STORE_GATE.lock().await;
        tokio::task::spawn_blocking(move || stored(&root))
            .await
            .map_err(|_| HttpError::internal())??
    };
    let batches: Vec<_> = batches
        .into_iter()
        .filter(|batch| match query.target {
            ImportDeliveryTarget::User => !batch.user_ack,
            ImportDeliveryTarget::Agent => !batch.agent_ack,
        })
        .collect();
    let batches = batches
        .into_iter()
        .map(|batch| ImportDeliveryPending {
            id: batch.id,
            cookie_origins: batch
                .cookie_keys
                .iter()
                .filter_map(|key| cookie_url(key, false))
                .filter_map(|url| {
                    let url = url::Url::parse(&url).ok()?;
                    Some(format!("{}://{}/*", url.scheme(), url.host_str()?))
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        })
        .collect();
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok((headers, Json(ImportDeliveryList { batches })))
}

pub(crate) async fn ack(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ImportDeliveryAck>,
) -> Result<StatusCode, HttpError> {
    local.require()?;
    let root = shared.config.data_dir.clone();
    let _gate = STORE_GATE.lock().await;
    tokio::task::spawn_blocking(move || {
        let path = batch_path(&root, &id)?;
        if !path.exists() {
            return Err(HttpError(StatusCode::NOT_FOUND, "unknown_import_batch"));
        }
        let mut batch: StoredBatch = read_document(&path)?;
        match request.target {
            ImportDeliveryTarget::User => batch.user_ack = true,
            ImportDeliveryTarget::Agent => batch.agent_ack = true,
        }
        save(&root, &batch)
    })
    .await
    .map_err(|_| HttpError::internal())??;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn mint(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<ImportDeliveryTicketRequest>,
) -> Result<Json<ImportDeliveryTicket>, HttpError> {
    local.require()?;
    if shared.remote.is_revoked(&request.device_id)
        || !shared
            .db
            .devices()
            .await
            .map_err(|_| HttpError::internal())?
            .iter()
            .any(|device| device.device_id == request.device_id)
    {
        return Err(HttpError(StatusCode::FORBIDDEN, "import_pairing_required"));
    }
    let root = shared.config.data_dir.clone();
    let device = request.device_id.clone();
    let batch = id.clone();
    tokio::task::spawn_blocking(move || {
        pairing_secret(&root, &device)?;
        let batch: StoredBatch = read_document(&batch_path(&root, &batch)?)?;
        if batch.user_ack {
            return Err(HttpError(StatusCode::CONFLICT, "import_batch_acknowledged"));
        }
        Ok::<_, HttpError>(())
    })
    .await
    .map_err(|_| HttpError::internal())??;
    let mut random = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut random);
    let ticket = hex(&random);
    let now = Instant::now();
    let mut tickets = shared.import_delivery.tickets.lock();
    tickets.retain(|_, ticket| ticket.deadline > now);
    if tickets.len() >= 1024 {
        return Err(HttpError(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_import_tickets",
        ));
    }
    tickets.insert(
        ticket.clone(),
        DeliveryTicket {
            device: request.device_id,
            batch: id,
            deadline: now + Duration::from_secs(120),
        },
    );
    Ok(Json(ImportDeliveryTicket { ticket }))
}

pub(crate) async fn fetch(
    State(shared): State<Shared>,
    Extension(caller): Extension<crate::api::Caller>,
    AxumPath(ticket): AxumPath<String>,
) -> Result<(HeaderMap, Json<ImportDeliverySigned>), HttpError> {
    let Some(device) = caller.0 else {
        return Err(HttpError(StatusCode::FORBIDDEN, "import_pairing_required"));
    };
    let entry = {
        let mut tickets = shared.import_delivery.tickets.lock();
        let authorized = tickets
            .get(&ticket)
            .is_some_and(|entry| entry.device == device && entry.deadline > Instant::now());
        if !authorized {
            return Err(HttpError(StatusCode::FORBIDDEN, "invalid_import_ticket"));
        }
        tickets.remove(&ticket).ok_or_else(HttpError::internal)?
    };
    let root = shared.config.data_dir.clone();
    let (key, batch) = tokio::task::spawn_blocking(move || {
        let key = pairing_secret(&root, &device)?;
        let mut batch: StoredBatch = read_document(&batch_path(&root, &entry.batch)?)?;
        if batch.user_ack {
            return Err(HttpError(StatusCode::CONFLICT, "import_batch_acknowledged"));
        }
        sanitize_batch(&mut batch);
        Ok::<_, HttpError>((key, batch))
    })
    .await
    .map_err(|_| HttpError::internal())??;
    let cookies = native_cookies(&shared, &batch.cookie_keys).await?;
    let batch = ImportDeliveryBatch {
        id: batch.id,
        source: batch.source,
        bookmarks: batch.bookmarks,
        history: batch.history,
        cookies,
    };
    let payload = serde_json::to_string(&batch).map_err(|_| HttpError::internal())?;
    let hash = hex(&Sha256::digest(payload.as_bytes()));
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_ref()).map_err(|_| HttpError::internal())?;
    mac.update(ticket.as_bytes());
    mac.update(batch.id.as_bytes());
    mac.update(hash.as_bytes());
    let signed = ImportDeliverySigned {
        ticket,
        batch_id: batch.id,
        payload,
        signature: hex(&mac.finalize().into_bytes()),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok((headers, Json(signed)))
}

fn sanitize_batch(batch: &mut StoredBatch) {
    batch.bookmarks.retain_mut(|bookmark| {
        let Some(safe) = openside_import::sanitize_import_url(&bookmark.url) else {
            return false;
        };
        if bookmark.title == bookmark.url {
            bookmark.title = safe.clone();
        }
        bookmark.url = safe;
        true
    });
    batch.history.retain_mut(|visit| {
        let Some(safe) = openside_import::sanitize_import_url(&visit.url)
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        else {
            return false;
        };
        if visit.title.as_deref() == Some(&visit.url) {
            visit.title = Some(safe.clone());
        }
        visit.url = safe;
        true
    });
}

fn source_paths(root: &Path) -> Result<Vec<PathBuf>, HttpError> {
    let sources = openside_config::data_layout::DataPath::BrowserImports
        .path(root)
        .join("default/sources");
    let entries = match fs::read_dir(sources) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(HttpError::internal()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| HttpError::internal())?;
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn source_documents(root: &Path) -> Result<Vec<Value>, HttpError> {
    source_paths(root)?
        .into_iter()
        .map(|path| {
            let mut document = read_document(&path)?;
            openside_import::sanitize_import_document_urls(&mut document);
            Ok(document)
        })
        .collect()
}

pub(crate) fn source_addresses(
    root: &Path,
) -> Result<Vec<openside_import::AutofillAddress>, HttpError> {
    #[derive(Deserialize)]
    struct Addresses {
        #[serde(default)]
        autofill: Vec<openside_import::AutofillAddress>,
    }
    let mut addresses = Vec::new();
    for path in source_paths(root)? {
        // Login suggestions must not allocate a copy of the source's potentially large history.
        let document: Addresses = read_document(&path)?;
        addresses.extend(document.autofill);
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_delivery_metadata_has_only_cookie_keys_and_live_mapping_preserves_native_scope() {
        let root = tempfile::tempdir().unwrap();
        let key = CookieKey {
            host: ".example.test".into(),
            name: "sid".into(),
            path: "/account".into(),
        };
        let batch = StoredBatch {
            id: random_id(),
            source: ImportDeliverySource {
                id: "chrome:Default".into(),
                brand: "chrome".into(),
                name: "Personal".into(),
            },
            bookmarks: Vec::new(),
            history: Vec::new(),
            cookie_keys: vec![key],
            user_ack: false,
            agent_ack: true,
        };
        save(root.path(), &batch).map_err(|error| error.1).unwrap();
        let persisted = fs::read_to_string(
            batch_path(root.path(), &batch.id)
                .map_err(|error| error.1)
                .unwrap(),
        )
        .unwrap();
        assert!(!persisted.contains("value"));
        assert!(!persisted.contains("synthetic-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(
                    batch_path(root.path(), &batch.id)
                        .map_err(|error| error.1)
                        .unwrap()
                )
                .unwrap()
                .permissions()
                .mode()
                    & 0o777,
                0o600
            );
        }
        let mut native = json!({"domain":".example.test","name":"sid","path":"/account","value":"synthetic-secret","secure":true,"httpOnly":true,"sameSite":"Strict","expires":2_000_000_000.0});
        let cookie = live_cookie(&batch.cookie_keys[0], &mut native).unwrap();
        assert_eq!(cookie.url, "https://example.test/account");
        assert_eq!(cookie.domain.as_deref(), Some(".example.test"));
        assert_eq!(cookie.value, "synthetic-secret");
        assert_eq!(cookie.same_site.as_deref(), Some("strict"));
        assert_eq!(cookie.expiration_date, Some(2_000_000_000.0));
        assert!(cookie.secure && cookie.http_only);
        let host_only = CookieKey {
            host: "example.test".into(),
            name: "sid".into(),
            path: "/".into(),
        };
        let cookie = live_cookie(&host_only, &mut json!({"domain":"example.test","name":"sid","path":"/","value":"synthetic-secret","secure":false,"httpOnly":false,"expires":-1.0})).unwrap();
        assert!(
            cookie.domain.is_none()
                && cookie.expiration_date.is_none()
                && cookie.same_site.is_none()
        );
    }
}
