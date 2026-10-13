//! Pinned tabs (`/v1/pinned-tabs`) and end-to-end encrypted device sync (`/v1/sync/*`,
//! `docs/specs/sync.md`) of the live policy, pinned tabs, vault items and tagged memories
//! through a WebDAV share or a shared directory. The sync key is derived from
//! `sync.passphrase_cmd` output and held only in memory; on disk this module keeps the
//! pinned-tab list and the per-device sync state (`<data_dir>/sync/state.json`: account,
//! device id, stamps, entity tags and keyed digests — never record bodies or key material).
//! Received memories keep their sync id in the memory database itself.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::{Path as UrlPath, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{TimeZone, Utc};
use openside_memory::NewMemory;
use openside_proto::{
    CreatePinnedTabRequest, MemoryKind, PairSyncRequest, PinnedTab, PinnedTabList, PolicySettings,
    Retention, SyncBackendKind, SyncReport, SyncSecretSource, SyncSecrets, SyncSetupResponse,
    SyncStatus, UpdateMemoryRequest, UpdateSyncConfigRequest,
};
use openside_sync::{
    AccountId, Applied, ClientState, DirBackend, Error, Kind, LocalStore, PairingCode, SyncKey,
    WebDavBackend,
};
use openside_vault::{AuditContext, PreparedImport, Redactor};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, sync::Mutex};
use zeroize::Zeroizing;

use crate::{api::HttpError, engine::Shared, tools::vault::DaemonVault};

const AUDIT: AuditContext<'static> = AuditContext {
    who: "sync",
    session_id: None,
    tool_call_id: None,
};
const MAX_TITLE: usize = 512;
/// Memory-store actor for writes made by sync.
const MEMORY_ACTOR: &str = "sync";
/// The only settings record: the live policy document.
const POLICY_ID: &str = "policy";

/// Serializes pinned-tab edits and sync runs, and holds the derived key.
#[derive(Default)]
pub(crate) struct Service {
    key: Mutex<Option<SyncKey>>,
    config: RwLock<Option<openside_config::SyncConfig>>,
    password: RwLock<Option<Zeroizing<String>>>,
    last_run: RwLock<Option<SyncReport>>,
}

pub(crate) fn effective(shared: &Shared) -> openside_config::SyncConfig {
    shared
        .sync
        .config
        .read()
        .clone()
        .unwrap_or_else(|| shared.config.loaded.config.sync.clone())
}

fn pinned_path(shared: &Shared) -> PathBuf {
    openside_config::data_layout::DataPath::Pins.path(&shared.config.data_dir)
}

fn state_path(shared: &Shared) -> PathBuf {
    openside_config::data_layout::DataPath::Sync
        .path(&shared.config.data_dir)
        .join("state.json")
}

fn load_pinned(path: &Path) -> Result<Vec<PinnedTab>, HttpError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| HttpError::internal()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(HttpError::internal()),
    }
}

fn save_pinned(path: &Path, tabs: &[PinnedTab]) -> Result<(), ()> {
    save_bytes(path, &serde_json::to_vec_pretty(tabs).map_err(|_| ())?)
}

/// Atomic replace via a temporary file in the same directory.
fn save_bytes(path: &Path, bytes: &[u8]) -> Result<(), ()> {
    use std::io::Write;
    let parent = path.parent().ok_or(())?;
    std::fs::create_dir_all(parent).map_err(|_| ())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| ())?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| ())?;
    file.persist(path).map_err(|_| ())?;
    Ok(())
}

/// Normalized http(s) URL and its device-independent id, so the same URL pinned on two
/// devices is one record rather than a duplicate.
fn normalize(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    // Pins sync to every device and show in the UI, so a URL's embedded credentials must not.
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let url = parsed.to_string();
    let digest = Sha256::digest(url.as_bytes());
    let id = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Some((url, id))
}

pub(crate) async fn list_pinned(State(s): State<Shared>) -> Result<Json<PinnedTabList>, HttpError> {
    let _guard = s.sync.key.lock().await;
    Ok(Json(PinnedTabList {
        tabs: load_pinned(&pinned_path(&s))?,
    }))
}

pub(crate) async fn pin(
    State(s): State<Shared>,
    Json(request): Json<CreatePinnedTabRequest>,
) -> Result<(StatusCode, Json<PinnedTab>), HttpError> {
    let (url, id) =
        normalize(&request.url).ok_or(HttpError(StatusCode::BAD_REQUEST, "invalid_url"))?;
    if request
        .title
        .as_ref()
        .is_some_and(|t| t.chars().count() > MAX_TITLE)
    {
        return Err(HttpError::invalid_value());
    }
    let _guard = s.sync.key.lock().await;
    let path = pinned_path(&s);
    let mut tabs = load_pinned(&path)?;
    let tab = match tabs.iter_mut().find(|tab| tab.id == id) {
        Some(existing) => {
            existing.title = request.title;
            existing.clone()
        }
        None => {
            let tab = PinnedTab {
                id,
                url,
                title: request.title,
                pinned_at: Utc::now(),
            };
            tabs.push(tab.clone());
            tab
        }
    };
    save_pinned(&path, &tabs).map_err(|()| HttpError::internal())?;
    Ok((StatusCode::CREATED, Json(tab)))
}

pub(crate) async fn unpin(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, HttpError> {
    let _guard = s.sync.key.lock().await;
    let path = pinned_path(&s);
    let mut tabs = load_pinned(&path)?;
    tabs.retain(|tab| tab.id != id);
    save_pinned(&path, &tabs).map_err(|()| HttpError::internal())?;
    // Idempotent: unpinning a tab that is not pinned (or was unpinned by sync) succeeds.
    Ok(StatusCode::NO_CONTENT)
}

fn backend_kind(shared: &Shared) -> Option<SyncBackendKind> {
    let config = effective(shared);
    if config.webdav_url.is_some() {
        Some(SyncBackendKind::Webdav)
    } else if config.dir.is_some() {
        Some(SyncBackendKind::Dir)
    } else {
        None
    }
}

fn status(shared: &Shared, state: Option<&ClientState>, unlocked: bool) -> SyncStatus {
    let config = effective(shared);
    SyncStatus {
        configured: backend_kind(shared).is_some(),
        backend: backend_kind(shared),
        set_up: state.is_some(),
        unlocked,
        account_id: state.map(|s| s.account.clone()),
        device_id: state.map(|s| s.device_id.clone()),
        last_sync: state
            .and_then(|s| s.last_sync)
            .and_then(|secs| Utc.timestamp_opt(i64::try_from(secs).ok()?, 0).single()),
        records: state.map_or(0, |s| s.records.len() as u64),
        pending: state.map_or(0, |s| s.pending() as u64),
        exhausted: state.is_some_and(ClientState::exhausted),
        location: config
            .webdav_url
            .clone()
            .or_else(|| config.dir.as_ref().map(|dir| dir.display().to_string())),
        webdav_username: config.webdav_username.clone(),
        settings: config.settings,
        memory_tags: config.memory_tags.clone(),
        passphrase_source: if config.passphrase_cmd.is_some() {
            SyncSecretSource::Command
        } else {
            SyncSecretSource::Entered
        },
        password_source: config.webdav_username.as_ref().map(|_| {
            if config.webdav_password_cmd.is_some() {
                SyncSecretSource::Command
            } else {
                SyncSecretSource::Entered
            }
        }),
        last_run: shared.sync.last_run.read().clone(),
    }
}

fn load_state(shared: &Shared) -> Result<Option<ClientState>, HttpError> {
    ClientState::load(&state_path(shared)).map_err(|_| HttpError::internal())
}

fn sync_error(error: &Error) -> HttpError {
    match error {
        Error::WrongPassphrase => HttpError(StatusCode::BAD_REQUEST, "sync_wrong_passphrase"),
        Error::Unauthorized => HttpError(StatusCode::BAD_GATEWAY, "sync_unauthorized"),
        Error::Backend(_) | Error::Io => {
            HttpError(StatusCode::BAD_GATEWAY, "sync_backend_unavailable")
        }
        Error::Contention => HttpError(StatusCode::CONFLICT, "sync_busy"),
        Error::Exhausted => HttpError(StatusCode::CONFLICT, "sync_counter_exhausted"),
        Error::Crypto | Error::Format | Error::Store(_) => HttpError::internal(),
    }
}

pub(crate) async fn get_status(State(s): State<Shared>) -> Result<Json<SyncStatus>, HttpError> {
    let key = s.sync.key.lock().await;
    let state = load_state(&s)?;
    Ok(Json(status(&s, state.as_ref(), key.is_some())))
}

/// stdout of a secret command run without a shell (trailing newlines removed, 64 KiB cap,
/// 20 s limit); stderr is discarded so the secret never reaches logs.
async fn command_output(argv: Option<&Vec<String>>) -> Option<Zeroizing<Vec<u8>>> {
    let argv = argv?;
    let program = argv.first().filter(|p| !p.is_empty())?;
    let read = async {
        let mut child = tokio::process::Command::new(program)
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut output = Zeroizing::new(Vec::new());
        stdout.take(65_537).read_to_end(&mut output).await.ok()?;
        if output.len() > 65_536 || !child.wait().await.ok()?.success() {
            return None;
        }
        let length = output.len()
            - output
                .iter()
                .rev()
                .take_while(|b| matches!(b, b'\r' | b'\n'))
                .count();
        output.truncate(length);
        Some(output)
    };
    tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .ok()
        .flatten()
}

/// Run `sync.passphrase_cmd` and derive the key off the async runtime.
async fn derive(
    shared: &Shared,
    account: AccountId,
    entered: Option<Zeroizing<String>>,
) -> Result<SyncKey, HttpError> {
    let config = effective(shared);
    let passphrase = match entered {
        Some(value) => Zeroizing::new(value.as_bytes().to_vec()),
        None => command_output(config.passphrase_cmd.as_ref())
            .await
            .ok_or(HttpError(
                StatusCode::CONFLICT,
                "sync_passphrase_unavailable",
            ))?,
    };
    tokio::task::spawn_blocking(move || SyncKey::derive(&passphrase, account))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|e| sync_error(&e))
}

/// The configured backend for `account`.
enum Remote {
    WebDav(WebDavBackend),
    Dir(DirBackend),
}

async fn remote(shared: &Shared, account: AccountId) -> Result<Remote, HttpError> {
    let config = effective(shared);
    if let Some(url) = &config.webdav_url {
        let credentials = match &config.webdav_username {
            Some(user) => {
                let entered = shared.sync.password.read().clone();
                let password = match entered {
                    Some(password) => password,
                    None => command_output(config.webdav_password_cmd.as_ref())
                        .await
                        .and_then(|bytes| {
                            std::str::from_utf8(&bytes)
                                .ok()
                                .map(|p| Zeroizing::new(p.to_owned()))
                        })
                        .ok_or(HttpError(StatusCode::CONFLICT, "sync_password_unavailable"))?,
                };
                Some((user.clone(), password))
            }
            None => None,
        };
        return WebDavBackend::new(url, account, credentials)
            .map(Remote::WebDav)
            .map_err(|e| sync_error(&e));
    }
    if let Some(dir) = &config.dir {
        return DirBackend::open(dir, account)
            .map(Remote::Dir)
            .map_err(|e| sync_error(&e));
    }
    Err(HttpError(StatusCode::CONFLICT, "sync_not_configured"))
}

async fn join(
    shared: &Shared,
    slot: &mut Option<SyncKey>,
    key: SyncKey,
) -> Result<ClientState, HttpError> {
    // The account collection must exist (and the credentials work) before the device
    // claims to be set up.
    if let Remote::WebDav(backend) = remote(shared, key.account()).await? {
        backend.ensure().await.map_err(|e| sync_error(&e))?;
    }
    let state = ClientState::new(&key).map_err(|_| HttpError::internal())?;
    state
        .save(&state_path(shared))
        .map_err(|_| HttpError::internal())?;
    *slot = Some(key);
    Ok(state)
}
fn require_unset(shared: &Shared) -> Result<(), HttpError> {
    if backend_kind(shared).is_none() {
        return Err(HttpError(StatusCode::CONFLICT, "sync_not_configured"));
    }
    if load_state(shared)?.is_some() {
        return Err(HttpError(StatusCode::CONFLICT, "sync_already_set_up"));
    }
    Ok(())
}

pub(crate) async fn setup(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    body: Bytes,
) -> Result<(StatusCode, Json<SyncSetupResponse>), HttpError> {
    let request: SyncSecrets = if body.is_empty() {
        SyncSecrets::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_request"))?
    };
    let (passphrase, password) = entered_secrets(local, request)?;
    let mut slot = s.sync.key.lock().await;
    require_unset(&s)?;
    if password.is_some() {
        *s.sync.password.write() = password;
    }
    let account = AccountId::generate().map_err(|_| HttpError::internal())?;
    let key = derive(&s, account, passphrase).await?;
    let pairing_code = key.pairing_code().encode();
    let state = join(&s, &mut slot, key).await?;
    Ok((
        StatusCode::CREATED,
        Json(SyncSetupResponse {
            pairing_code,
            status: status(&s, Some(&state), true),
        }),
    ))
}

pub(crate) async fn pair(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(request): Json<PairSyncRequest>,
) -> Result<Json<SyncStatus>, HttpError> {
    let (passphrase, password) = entered_secrets(
        local,
        SyncSecrets {
            passphrase: request.passphrase,
            webdav_password: request.webdav_password,
        },
    )?;
    let code = PairingCode::parse(&request.pairing_code)
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_pairing_code"))?;
    let mut slot = s.sync.key.lock().await;
    require_unset(&s)?;
    if password.is_some() {
        *s.sync.password.write() = password;
    }
    let key = derive(&s, code.account, passphrase).await?;
    if !key.matches(&code) {
        return Err(HttpError(StatusCode::BAD_REQUEST, "sync_wrong_passphrase"));
    }
    let state = join(&s, &mut slot, key).await?;
    Ok(Json(status(&s, Some(&state), true)))
}

type EnteredSecrets = (Option<Zeroizing<String>>, Option<Zeroizing<String>>);

fn entered_secrets(
    local: crate::remote::LocalAuthority,
    request: SyncSecrets,
) -> Result<EnteredSecrets, HttpError> {
    let passphrase = request.passphrase.map(Zeroizing::new);
    let password = request.webdav_password.map(Zeroizing::new);
    if passphrase.is_some() || password.is_some() {
        local.require()?;
    }
    if passphrase
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > 65_536)
        || password.as_ref().is_some_and(|value| value.len() > 65_536)
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_sync_secret"));
    }
    Ok((passphrase, password))
}

pub(crate) async fn unlock(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(request): Json<SyncSecrets>,
) -> Result<Json<SyncStatus>, HttpError> {
    let (passphrase, password) = entered_secrets(local, request)?;
    let mut slot = s.sync.key.lock().await;
    let state = load_state(&s)?.ok_or(HttpError(StatusCode::CONFLICT, "sync_not_set_up"))?;
    let account = state.account_id().map_err(|_| HttpError::internal())?;
    let key = derive(&s, account, passphrase).await?;
    if !state.accepts(&key) {
        return Err(HttpError(StatusCode::BAD_REQUEST, "sync_wrong_passphrase"));
    }
    if password.is_some() {
        *s.sync.password.write() = password;
    }
    *slot = Some(key);
    Ok(Json(status(&s, Some(&state), true)))
}

pub(crate) async fn configure(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(request): Json<UpdateSyncConfigRequest>,
) -> Result<Json<SyncStatus>, HttpError> {
    local.require()?;
    tokio::spawn(async move {
        let key = s.sync.key.lock().await;
        let _writer = crate::settings_api::WRITE.lock().await;
        let state = load_state(&s)?;
        let mut saved = crate::settings_api::stored(&s).await?;
        let current = effective(&s);
        let mut config = current.clone();
        config.dir = request.dir.map(PathBuf::from);
        config.webdav_url = request.webdav_url;
        config.webdav_username = request.webdav_username;
        config.settings = request.settings;
        config.memory_tags = request.memory_tags;
        let changed_backend = config.dir != current.dir
            || config.webdav_url != current.webdav_url
            || config.webdav_username != current.webdav_username;
        if state.is_some() && changed_backend {
            return Err(HttpError(StatusCode::CONFLICT, "sync_already_set_up"));
        }
        if config.webdav_username.is_none() {
            config.webdav_password_cmd = None;
        }
        config
            .validate()
            .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_sync_config"))?;
        // This screen owns only these fields; keep unrelated command edits pending in the file.
        saved.sync.dir.clone_from(&config.dir);
        saved.sync.webdav_url.clone_from(&config.webdav_url);
        saved
            .sync
            .webdav_username
            .clone_from(&config.webdav_username);
        saved.sync.settings = config.settings;
        saved.sync.memory_tags.clone_from(&config.memory_tags);
        if saved.sync.webdav_username.is_none() {
            saved.sync.webdav_password_cmd = None;
        }
        crate::settings_api::persist(
            &s,
            serde_json::to_value(saved).map_err(|_| HttpError::internal())?,
        )
        .await?;
        *s.sync.config.write() = Some(config);
        if changed_backend {
            *s.sync.password.write() = None;
        }
        Ok(Json(status(&s, state.as_ref(), key.is_some())))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

/// What a synced memory carries; ids, timestamps and versions stay per device.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryBody {
    kind: MemoryKind,
    content: String,
    tags: Vec<String>,
    retention: Retention,
    url: Option<String>,
    title: Option<String>,
}

fn memory_error(error: openside_memory::Error) -> Error {
    match error {
        openside_memory::Error::Invalid(_) | openside_memory::Error::SecretDetected => {
            Error::Format
        }
        _ => Error::Store("memory".into()),
    }
}

/// The device's synced data as seen by the sync engine: the live policy (when
/// `sync.settings`), pinned tabs, the vault while unlocked, and memories tagged with one of
/// `sync.memory_tags`. Built only while every local writer of that data is held off.
struct Store<'a> {
    shared: Shared,
    pinned_path: PathBuf,
    pinned: Vec<PinnedTab>,
    vault: Option<&'a mut DaemonVault>,
    redactors: &'a RwLock<Vec<Redactor>>,
}

fn selected(shared: &Shared, tags: &[String]) -> bool {
    let wanted = effective(shared).memory_tags;
    tags.iter().any(|tag| wanted.contains(tag))
}

// Free functions rather than `Store` methods: `&Store` is not `Send` (the vault lock is not
// `Sync`), so it must not be held across an await.
async fn memory_snapshot(shared: &Shared) -> Result<BTreeMap<String, Zeroizing<Vec<u8>>>, Error> {
    let tags = effective(shared).memory_tags;
    let memories = shared
        .memory
        .run(move |store| store.tagged(&tags))
        .await
        .map_err(memory_error)?;
    memories
        .into_iter()
        .map(|(sync_id, memory)| {
            let body = MemoryBody {
                kind: memory.kind,
                content: memory.content,
                tags: memory.tags,
                retention: memory.retention,
                url: memory.url,
                title: memory.title,
            };
            serde_json::to_vec(&body)
                .map(|bytes| (sync_id, Zeroizing::new(bytes)))
                .map_err(|_| Error::Format)
        })
        .collect()
}

async fn apply_memory(shared: &Shared, id: &str, body: Option<&[u8]>) -> Result<Applied, Error> {
    let sync_id = id.to_owned();
    let local = shared
        .memory
        .run(move |store| store.synced(&sync_id))
        .await
        .map_err(memory_error)?;
    // A local memory taken out of the selection here is the user's local data now: neither
    // a remote deletion nor a remote edit may touch it.
    if local.as_ref().is_some_and(|m| !selected(shared, &m.tags)) {
        return Ok(Applied::Declined);
    }
    let Some(body) = body else {
        if let Some(local) = local {
            match shared
                .memory
                .run(move |store| store.delete(&local.id, MEMORY_ACTOR))
                .await
            {
                Ok(_) | Err(openside_memory::Error::NotFound) => {}
                Err(error) => return Err(memory_error(error)),
            }
        }
        let sync_id = id.to_owned();
        shared
            .memory
            .run(move |store| store.forget_synced(&sync_id))
            .await
            .map_err(memory_error)?;
        return Ok(Applied::Applied);
    };
    let body: MemoryBody = serde_json::from_slice(body).map_err(|_| Error::Format)?;
    // A memory outside this device's selection would vanish from the next snapshot and be
    // synced back as a deletion; refuse it instead (devices must share memory_tags).
    if !selected(shared, &body.tags) {
        return Err(Error::Format);
    }
    match local {
        Some(local) => {
            let patch = UpdateMemoryRequest {
                content: Some(body.content),
                tags: Some(body.tags),
                retention: Some(body.retention),
            };
            shared
                .memory
                .run(move |store| store.update(&local.id, patch, MEMORY_ACTOR))
                .await
                .map_err(memory_error)?;
        }
        None => {
            let new = NewMemory {
                kind: body.kind,
                content: body.content,
                tags: body.tags,
                retention: Some(body.retention),
                url: body.url,
                title: body.title,
            };
            let sync_id = id.to_owned();
            shared
                .memory
                .run(move |store| store.create_synced(&sync_id, new, MEMORY_ACTOR))
                .await
                .map_err(memory_error)?;
        }
    }
    Ok(Applied::Applied)
}

async fn apply_setting(shared: &Shared, id: &str, body: Option<&[u8]>) -> Result<Applied, Error> {
    if id != POLICY_ID {
        return Err(Error::Format);
    }
    // The policy document always exists; a remote deletion has nothing to remove, and the
    // next local snapshot re-publishes it.
    let Some(body) = body else {
        return Ok(Applied::Applied);
    };
    let settings: PolicySettings = serde_json::from_slice(body).map_err(|_| Error::Format)?;
    // Same validation, persistence and live publication as `PUT /v1/policy`; the run
    // already holds the policy writer lock.
    crate::policy_api::replace_locked(&shared.policy, settings)
        .await
        .map_err(|error| {
            if error.0 == StatusCode::BAD_REQUEST {
                Error::Format
            } else {
                Error::Store("policy".into())
            }
        })?;
    crate::diag!("opensided: live policy replaced by sync from another device");
    Ok(Applied::Applied)
}

impl Store<'_> {
    fn apply_pinned(&mut self, id: &str, body: Option<&[u8]>) -> Result<Applied, Error> {
        let index = self.pinned.iter().position(|tab| tab.id == id);
        match body {
            Some(body) => {
                let tab: PinnedTab = serde_json::from_slice(body).map_err(|_| Error::Format)?;
                let canonical = normalize(&tab.url).ok_or(Error::Format)?;
                let too_long = tab
                    .title
                    .as_ref()
                    .is_some_and(|t| t.chars().count() > MAX_TITLE);
                if canonical != (tab.url.clone(), id.to_owned()) || tab.id != id || too_long {
                    return Err(Error::Format);
                }
                match index {
                    Some(index) => self.pinned[index] = tab,
                    None => self.pinned.push(tab),
                }
            }
            None => {
                if let Some(index) = index {
                    self.pinned.remove(index);
                }
            }
        }
        save_pinned(&self.pinned_path, &self.pinned)
            .map_err(|()| Error::Store("pinned tabs".into()))?;
        Ok(Applied::Applied)
    }

    fn apply_vault(&mut self, id: &str, body: Option<&[u8]>) -> Result<Applied, Error> {
        let vault = self
            .vault
            .as_mut()
            .ok_or_else(|| Error::Store("vault locked".into()))?;
        let Some(body) = body else {
            vault
                .remove_item(&AUDIT, id)
                .map_err(|_| Error::Store("vault".into()))?;
            return Ok(Applied::Applied);
        };
        // Validation, including the item id the authenticated record names, happens before
        // any side effect; a mismatched body is rejected without touching any item.
        let prepared = PreparedImport::parse(id, body).map_err(|error| match error {
            openside_vault::Error::Format
            | openside_vault::Error::InvalidEntry
            | openside_vault::Error::InvalidOrigin
            | openside_vault::Error::InvalidSeed => Error::Format,
            _ => Error::Store("vault".into()),
        })?;
        // Masked before the secret can be stored or used; a failed commit only leaves an
        // extra mask.
        self.redactors.write().push(prepared.redactor());
        vault
            .commit_import(&AUDIT, prepared)
            .map_err(|_| Error::Store("vault".into()))?;
        Ok(Applied::Applied)
    }
}

impl LocalStore for Store<'_> {
    fn available(&self, kind: Kind) -> bool {
        let config = effective(&self.shared);
        match kind {
            Kind::PinnedTab => true,
            Kind::VaultItem => self.vault.as_ref().is_some_and(|v| v.is_unlocked()),
            Kind::Setting => config.settings,
            Kind::Memory => !config.memory_tags.is_empty(),
        }
    }

    async fn snapshot(
        &mut self,
        kind: Kind,
    ) -> Result<BTreeMap<String, Zeroizing<Vec<u8>>>, Error> {
        match kind {
            Kind::PinnedTab => self
                .pinned
                .iter()
                .map(|tab| {
                    serde_json::to_vec(tab)
                        .map(|body| (tab.id.clone(), Zeroizing::new(body)))
                        .map_err(|_| Error::Store("pinned tab".into()))
                })
                .collect(),
            Kind::VaultItem => {
                let vault = self
                    .vault
                    .as_ref()
                    .ok_or_else(|| Error::Store("vault locked".into()))?;
                let ids = vault.item_ids().map_err(|_| Error::Store("vault".into()))?;
                ids.into_iter()
                    .map(|id| {
                        let body = vault
                            .export_item(&AUDIT, &id)
                            .map_err(|_| Error::Store("vault".into()))?;
                        Ok((id, body))
                    })
                    .collect()
            }
            Kind::Setting => {
                let settings = crate::policy_api::current(&self.shared.policy);
                let body = serde_json::to_vec(&settings).map_err(|_| Error::Format)?;
                Ok(BTreeMap::from([(
                    POLICY_ID.to_owned(),
                    Zeroizing::new(body),
                )]))
            }
            Kind::Memory => memory_snapshot(&self.shared).await,
        }
    }

    async fn apply(&mut self, kind: Kind, id: &str, body: Option<&[u8]>) -> Result<Applied, Error> {
        match kind {
            Kind::PinnedTab => self.apply_pinned(id, body),
            Kind::VaultItem => self.apply_vault(id, body),
            Kind::Setting => apply_setting(&self.shared, id, body).await,
            Kind::Memory => apply_memory(&self.shared, id, body).await,
        }
    }
}

pub(crate) async fn run(State(s): State<Shared>) -> Result<Json<SyncReport>, HttpError> {
    // In its own task: a client that disconnects cannot cancel a run between a remote write
    // and the state save.
    tokio::spawn(run_to_end(s))
        .await
        .map_err(|_| HttpError::internal())?
}

async fn run_to_end(s: Shared) -> Result<Json<SyncReport>, HttpError> {
    let mut slot = s.sync.key.lock().await;
    let mut state = load_state(&s)?.ok_or(HttpError(StatusCode::CONFLICT, "sync_not_set_up"))?;
    let account = state.account_id().map_err(|_| HttpError::internal())?;
    if slot.is_none() {
        *slot = Some(derive(&s, account, None).await?);
    }
    let Some(key) = slot.as_ref() else {
        return Err(HttpError::internal());
    };
    if !state.accepts(key) {
        *slot = None;
        return Err(HttpError(StatusCode::BAD_REQUEST, "sync_wrong_passphrase"));
    }
    let remote = remote(&s, account).await?;
    // Every local writer of synced data is held off for the whole run (pinned tabs by the
    // key lock above), so a snapshot and the applies that follow see the same data. Order:
    // sync key, policy writer, memory writers, vault.
    let _policy = s.policy.writer.lock().await;
    let _memory = s.memory.writers.write().await;
    let mut vault = s.registry.vault.vault.lock().await;
    let pinned_path = pinned_path(&s);
    let mut store = Store {
        shared: s.clone(),
        pinned: load_pinned(&pinned_path)?,
        pinned_path,
        vault: vault.as_mut(),
        redactors: &s.registry.redactors,
    };
    let vault_available = store.available(Kind::VaultItem);
    let path = state_path(&s);
    let mut checkpoint = |state: &ClientState| state.save(&path);
    let result = match &remote {
        Remote::WebDav(backend) => {
            openside_sync::sync(&mut state, key, backend, &mut store, &mut checkpoint).await
        }
        Remote::Dir(backend) => {
            openside_sync::sync(&mut state, key, backend, &mut store, &mut checkpoint).await
        }
    };
    // Stamps issued for local edits must persist even when the run failed midway.
    state
        .save(&state_path(&s))
        .map_err(|_| HttpError::internal())?;
    let report = result.map_err(|e| sync_error(&e))?;
    let report = SyncReport {
        pushed: report.pushed,
        pulled: report.pulled,
        applied: report.applied,
        conflicts: report.conflicts,
        rejected: report.rejected,
        deferred: report.deferred,
        vault_available,
    };
    *s.sync.last_run.write() = Some(report.clone());
    Ok(Json(report))
}
