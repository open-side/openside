//! Read-only share links (`docs/specs/sharing.md`).
//!
//! A person publishes a frozen snapshot of one chat under an unguessable id; anyone holding
//! the link reads it through the public `GET /v1/shares/{share_id}` routes, without a token,
//! until it is revoked, expires or the chat is deleted.
//!
//! Privacy model: the snapshot is built from the durable event log, never from tool data.
//! It keeps delivered messages, final replies and (optionally) one tool name per step; tool
//! arguments and results, thinking, tickets and answers, queued or dropped messages and
//! cards never enter it. Every string goes through the live redaction filters (vault items,
//! session secrets) and then the memory secret detector, line by line, before it is written,
//! and a set-up vault must be unlocked so its filters are installed. Chosen text artifacts
//! get the same filters and are refused if they still look like a credential; binary
//! artifacts are copied as the person chose them.
//!
//! Layout: `<data_dir>/shares/<id>/share.json` plus `<id>/<n>` artifact copies (dirs 0700,
//! files 0600), staged in `.<id>.tmp` and renamed into place, so a reader never sees half a
//! share. Revocation deletes the directory.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use chrono::DateTime;
use tokio::{
    io::AsyncReadExt,
    sync::{OwnedSemaphorePermit, Semaphore},
};

use axum::{
    extract::{Path as UrlPath, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use base64::Engine as _;
use chrono::Utc;
use openside_proto::{
    CreateShareRequest, EventKind, MessageAuthorKind, MessageId, SessionId, ShareLink,
    ShareLinkList, SharedArtifact, SharedChat, SharedItem, SharedStep, VaultState,
    MAX_SHARES_PER_SESSION, MAX_SHARE_ARTIFACTS, MAX_SHARE_LIFETIME_SECS, MIN_SHARE_LIFETIME_SECS,
    SHARE_ID_LEN,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

use crate::{api::HttpError, engine::Shared};

const RECORD: &str = "share.json";
const MAX_CONVERSATION_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_ARTIFACTS_BYTES: u64 = 25 * 1024 * 1024;
/// Upper bound for reading a stored record back; anything larger was not written by us.
const MAX_RECORD_BYTES: u64 = 2 * MAX_CONVERSATION_BYTES as u64;
/// Requests one caller may make to one link per rolling [`WINDOW`] (a page load plus its
/// files, many times), and all callers together; per caller first, so one holder of a link
/// cannot spend it for everyone else.
pub(crate) const LINK_RATE: usize = 120;
const LINK_TOTAL_RATE: usize = 600;
/// Failed lookups (unknown, revoked, expired, malformed) one caller may make per
/// [`WINDOW`], and all callers together. Misses never spend a live link's budget.
pub(crate) const MISS_RATE: usize = 60;
const MISS_TOTAL_RATE: usize = 600;
const WINDOW: Duration = Duration::from_secs(60);
const REDACTED: &str = "[REDACTED]";
/// Public requests served at once per daemon, per link, and per caller on one link; the
/// rest are told to retry.
pub(crate) const GLOBAL_IN_FLIGHT: usize = 32;
const LINK_IN_FLIGHT: usize = 16;
pub(crate) const SOURCE_IN_FLIGHT: usize = 4;
/// File bytes one link may serve per [`WINDOW`].
const LINK_BYTES_PER_WINDOW: u64 = 256 * 1024 * 1024;
/// Files are streamed in chunks of this size, so memory per download stays bounded.
const CHUNK: usize = 64 * 1024;
/// Longest side of a shared image; larger ones are scaled down when re-encoded.
const IMAGE_MAX_SIDE: u32 = 4096;
/// Refuse to decode images beyond this size or allocation (decompression bombs). 192 MiB
/// holds a 48-megapixel RGBA image, larger than any phone photo.
const IMAGE_DECODE_MAX_SIDE: u32 = 16_384;
const IMAGE_DECODE_MAX_ALLOC: u64 = 192 * 1024 * 1024;
/// Delete attempts before a revoked share's files are left to the next sweep.
const PURGE_ATTEMPTS: u32 = 3;

/// Serializes link creation, revocation and cleanup so the per-chat cap cannot be raced.
static GATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);
/// One image decode at a time per process, so concurrent share requests cannot stack
/// [`IMAGE_DECODE_MAX_ALLOC`]-sized buffers.
static DECODES: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));
/// Rolling request times (or byte counts) per limiter key. Keys carry the data directory,
/// so daemons sharing a process (tests) never share a budget.
type Hits = parking_lot::Mutex<HashMap<String, VecDeque<(Instant, u64)>>>;
static HITS: LazyLock<Hits> = LazyLock::new(Default::default);
/// In-flight permits per limiter key.
static SLOTS: LazyLock<parking_lot::Mutex<HashMap<String, Arc<Semaphore>>>> =
    LazyLock::new(Default::default);
/// Links revoked by this process, keyed by [`revocation_key`]. A public read checks it (and
/// the on-disk tombstone) right before it answers, and a download checks it before and after
/// reading each chunk, so revoking refuses new reads at once and stops downloads in progress
/// at the next chunk. A body or chunk already handed to the connection may still arrive; no
/// lock is held through delivery, so a slow reader cannot stall revocation.
static REVOKED: LazyLock<parking_lot::RwLock<HashSet<String>>> = LazyLock::new(Default::default);

#[derive(Serialize, Deserialize)]
struct Record {
    session_id: SessionId,
    steps: bool,
    chat: SharedChat,
}

fn root(shared: &Shared) -> PathBuf {
    openside_config::data_layout::DataPath::Shares.path(&shared.config.data_dir)
}

fn valid_id(id: &str) -> bool {
    id.len() == SHARE_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn new_id() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn not_found() -> HttpError {
    HttpError(StatusCode::NOT_FOUND, "share_not_found")
}

/// Admits `weight` more units under `key` if the rolling [`WINDOW`] stays within `limit`.
fn admit_weighted(key: String, weight: u64, limit: u64) -> bool {
    let now = Instant::now();
    let mut hits = HITS.lock();
    if hits.len() > 4096 {
        hits.retain(|_, times| {
            times
                .back()
                .is_some_and(|(at, _)| now.duration_since(*at) < WINDOW)
        });
    }
    let times = hits.entry(key).or_default();
    while times
        .front()
        .is_some_and(|(at, _)| now.duration_since(*at) >= WINDOW)
    {
        times.pop_front();
    }
    let used: u64 = times.iter().map(|(_, weight)| weight).sum();
    if used.saturating_add(weight) > limit {
        return false;
    }
    times.push_back((now, weight));
    true
}

fn admit(key: String, limit: usize) -> bool {
    admit_weighted(key, 1, limit as u64)
}

/// One in-flight slot under `key`, or `None` when all `size` are taken.
fn slot(key: String, size: usize) -> Option<OwnedSemaphorePermit> {
    let semaphore = {
        let mut slots = SLOTS.lock();
        if slots.len() > 4096 {
            slots.retain(|_, semaphore| Arc::strong_count(semaphore) > 1);
        }
        slots
            .entry(key)
            .or_insert_with(|| Arc::new(Semaphore::new(size)))
            .clone()
    };
    semaphore.try_acquire_owned().ok()
}

/// The daemon-wide and the per-link slot a public request holds until its body is sent.
struct Permits {
    _global: OwnedSemaphorePermit,
    _link: OwnedSemaphorePermit,
    _source: OwnedSemaphorePermit,
}

fn revocation_key(root: &Path, id: &str) -> String {
    format!("{}/{id}", root.display())
}

fn tombstone(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".{id}.revoked"))
}

/// Whether `id` was revoked: by this process, or by any earlier one (its tombstone).
fn revoked(root: &Path, id: &str) -> bool {
    is_revoked(&REVOKED.read(), root, id)
}

fn is_revoked(revocations: &HashSet<String>, root: &Path, id: &str) -> bool {
    revocations.contains(&revocation_key(root, id)) || tombstone(root, id).exists()
}

fn rate_limited() -> Response {
    let mut response =
        HttpError(StatusCode::TOO_MANY_REQUESTS, "share_rate_limited").into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    response
}

/// Headers of every public response: never cached, never indexed, no Referer onward.
fn public(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(unix)]
fn private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

fn private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn read_record(root: &Path, id: &str) -> Option<Record> {
    use std::io::Read;
    let file = std::fs::File::open(root.join(id).join(RECORD)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Every stored, unrevoked share. A revoked share whose files survived an earlier delete is
/// purged again here instead of being listed.
fn records(root: &Path) -> Vec<(String, Record)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            if !valid_id(&name) {
                return None;
            }
            if revoked(root, &name) {
                purge(root, &name);
                return None;
            }
            let record = read_record(root, &name)?;
            Some((name, record))
        })
        .collect()
}

/// Revokes `id` fail-closed: the tombstone is durable before any file is touched, so a
/// delete that fails halfway still leaves the link dead. Errors only when the tombstone
/// itself cannot be written, in which case the link is still live and the caller says so.
fn retire(root: &Path, id: &str) -> std::io::Result<()> {
    let marker = tombstone(root, id);
    if !marker.exists() {
        private_file(&marker, b"")?;
    }
    REVOKED.write().insert(revocation_key(root, id));
    purge(root, id);
    Ok(())
}

/// Deletes a revoked share's files, retrying briefly; the tombstone goes only with them.
fn purge(root: &Path, id: &str) {
    for attempt in 1..=PURGE_ATTEMPTS {
        match std::fs::remove_dir_all(root.join(id)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                if attempt == PURGE_ATTEMPTS {
                    crate::diag!("opensided: deleting a revoked share failed: {error}");
                    return;
                }
                std::thread::sleep(Duration::from_millis(50 * u64::from(attempt)));
                continue;
            }
        }
        let _ = std::fs::remove_file(tombstone(root, id));
        return;
    }
}

fn expired(record: &Record, now: DateTime<Utc>) -> bool {
    record.chat.expires_at.is_some_and(|at| at <= now)
}

fn link(shared: &Shared, id: &str, record: &Record) -> ShareLink {
    let path = format!("/share#{id}");
    ShareLink {
        id: id.to_owned(),
        session_id: record.session_id.clone(),
        title: record.chat.title.clone(),
        created_at: record.chat.shared_at,
        expires_at: record.chat.expires_at,
        url: crate::remote::public_base(shared).map(|base| format!("{base}{path}")),
        path,
        messages: record
            .chat
            .items
            .iter()
            .filter(|item| !matches!(item, SharedItem::Steps { .. }))
            .count()
            .try_into()
            .unwrap_or(u32::MAX),
        steps: record.steps,
        artifacts: record.chat.artifacts.clone(),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, HttpError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| HttpError::internal())
}

/// The live redaction filters, then the secret detector line by line: a line that still
/// looks like a credential is replaced whole rather than trusted to a partial mask.
fn scrub(shared: &Shared, text: &str) -> Result<String, HttpError> {
    let masked = shared
        .registry
        .redact(serde_json::Value::String(text.to_owned()))
        .map_err(|_| HttpError::internal())?;
    let masked = masked.as_str().unwrap_or_default();
    Ok(masked
        .split_inclusive('\n')
        .map(|line| {
            if openside_memory::looks_secret(line) {
                if line.ends_with('\n') {
                    format!("{REDACTED}\n")
                } else {
                    REDACTED.to_owned()
                }
            } else {
                line.to_owned()
            }
        })
        .collect())
}

/// Delivered conversation from the event log, oldest first.
async fn conversation(
    shared: &Shared,
    session: &SessionId,
    steps: bool,
) -> Result<Vec<SharedItem>, HttpError> {
    let mut items = Vec::new();
    // Charged as the snapshot grows, so an oversized chat is refused before it is all built.
    let mut used = 0_usize;
    let mut charge = |bytes: usize| {
        used = used.saturating_add(bytes);
        if used > MAX_CONVERSATION_BYTES {
            Err(HttpError(StatusCode::PAYLOAD_TOO_LARGE, "share_too_large"))
        } else {
            Ok(())
        }
    };
    let mut pending: HashMap<MessageId, String> = HashMap::new();
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();
    let mut after = 0;
    loop {
        let page = shared
            .db
            .events(session, after)
            .await
            .map_err(|_| HttpError::internal())?;
        let Some(last) = page.last() else { break };
        after = last.seq;
        for event in page {
            match event.kind {
                EventKind::MessageQueued {
                    message_id, text, ..
                }
                | EventKind::MessageEdited { message_id, text } => {
                    pending.insert(message_id, text);
                }
                EventKind::MessageDropped { message_id, .. } => {
                    pending.remove(&message_id);
                }
                EventKind::MessageInjected { message_id } => {
                    if let Some(text) = pending.remove(&message_id) {
                        charge(text.len() + 64)?;
                        items.push(SharedItem::User {
                            text: scrub(shared, &text)?,
                            at: event.ts,
                        });
                    }
                }
                EventKind::AssistantFinal { text, .. } if !text.trim().is_empty() => {
                    charge(text.len() + 64)?;
                    items.push(SharedItem::Assistant {
                        text: scrub(shared, &text)?,
                        at: event.ts,
                    });
                }
                EventKind::ToolStarted {
                    tool_call_id, name, ..
                } if steps => {
                    charge(name.len() + 32)?;
                    let step = SharedStep {
                        tool: scrub(shared, &name)?,
                        failed: false,
                    };
                    match items.last_mut() {
                        Some(SharedItem::Steps { steps }) => steps.push(step),
                        _ => items.push(SharedItem::Steps { steps: vec![step] }),
                    }
                    let item = items.len() - 1;
                    if let Some(SharedItem::Steps { steps }) = items.last() {
                        calls.insert(tool_call_id, (item, steps.len() - 1));
                    }
                }
                EventKind::ToolEnded {
                    tool_call_id,
                    is_error: true,
                    ..
                } => {
                    if let Some((item, step)) = calls.get(&tool_call_id) {
                        if let Some(SharedItem::Steps { steps }) = items.get_mut(*item) {
                            steps[*step].failed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(items)
}

fn unsupported() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "unsupported_share_artifact")
}

/// What a chosen file becomes in the snapshot: `(mime, bytes)`.
///
/// Only text and raster images can be shared. Text gets the conversation's filters and is
/// refused if a credential survives them; it is always served as plain text, so shared HTML
/// or SVG never renders. Images are decoded and re-encoded, which drops every metadata
/// chunk (EXIF, XMP, ICC, text) and caps their size; the person picked them after seeing a
/// preview, so their pixels are shared as shown. Everything else (PDFs, archives, office
/// documents) is refused: its hidden content cannot be screened.
fn prepare_artifact(
    shared: &Shared,
    mime: &str,
    bytes: Vec<u8>,
) -> Result<(String, Vec<u8>), HttpError> {
    if matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return reencode_image(&bytes, mime == "image/jpeg");
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Err(unsupported());
    };
    if text.contains('\0') {
        return Err(unsupported());
    }
    let masked = shared
        .registry
        .redact(serde_json::Value::String(text))
        .map_err(|_| HttpError::internal())?;
    let masked = masked.as_str().unwrap_or_default();
    if openside_memory::looks_secret(masked) {
        return Err(HttpError(StatusCode::BAD_REQUEST, "sensitive_artifact"));
    }
    Ok((
        "text/plain; charset=utf-8".into(),
        masked.as_bytes().to_vec(),
    ))
}

fn reencode_image(bytes: &[u8], jpeg: bool) -> Result<(String, Vec<u8>), HttpError> {
    let mut reader = image::io::Reader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| unsupported())?;
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(IMAGE_DECODE_MAX_SIDE);
    limits.max_image_height = Some(IMAGE_DECODE_MAX_SIDE);
    limits.max_alloc = Some(IMAGE_DECODE_MAX_ALLOC);
    reader.limits(limits);
    let mut image = reader.decode().map_err(|_| unsupported())?;
    if image.width() > IMAGE_MAX_SIDE || image.height() > IMAGE_MAX_SIDE {
        image = image.resize(
            IMAGE_MAX_SIDE,
            IMAGE_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        );
    }
    let mut out = std::io::Cursor::new(Vec::new());
    let (mime, written) = if jpeg {
        let rgb = image::DynamicImage::ImageRgb8(image.to_rgb8());
        (
            "image/jpeg",
            rgb.write_to(&mut out, image::ImageOutputFormat::Jpeg(88)),
        )
    } else {
        (
            "image/png",
            image.write_to(&mut out, image::ImageOutputFormat::Png),
        )
    };
    written.map_err(|_| HttpError::internal())?;
    Ok((mime.into(), out.into_inner()))
}

pub(crate) async fn list(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ShareLinkList>, HttpError> {
    let session = SessionId::new(id);
    s.db.state(&session)
        .await
        .map_err(|_| HttpError::not_found())?;
    let _gate = GATE.lock().await;
    let root = root(&s);
    let mut shares = blocking(move || {
        let now = Utc::now();
        let mut live = Vec::new();
        for (id, record) in records(&root) {
            if record.session_id != session {
                continue;
            }
            if expired(&record, now) {
                if let Err(error) = retire(&root, &id) {
                    crate::diag!("opensided: retiring an expired share failed: {error}");
                }
            } else {
                live.push((id, record));
            }
        }
        live
    })
    .await?;
    shares.sort_by_key(|share| std::cmp::Reverse(share.1.chat.shared_at));
    Ok(Json(ShareLinkList {
        shares: shares
            .iter()
            .map(|(id, record)| link(&s, id, record))
            .collect(),
    }))
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(author): Extension<MessageAuthorKind>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<CreateShareRequest>,
) -> Result<(StatusCode, Json<ShareLink>), HttpError> {
    // Publishing outside the daemon is the person's call, never an agent's or a channel's.
    if author != MessageAuthorKind::Person {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_required"));
    }
    if request
        .expires_in_secs
        .is_some_and(|secs| !(MIN_SHARE_LIFETIME_SECS..=MAX_SHARE_LIFETIME_SECS).contains(&secs))
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_expiry"));
    }
    let mut seen = HashSet::new();
    if request.artifacts.len() > MAX_SHARE_ARTIFACTS
        || !request.artifacts.iter().all(|path| seen.insert(path))
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_share_artifact"));
    }
    let session = SessionId::new(id);
    let state =
        s.db.state(&session)
            .await
            .map_err(|_| HttpError::not_found())?;
    if state.incognito {
        return Err(HttpError(StatusCode::CONFLICT, "private_session"));
    }
    // Vault filters are installed on unlock; a locked vault's values could not be masked.
    if s.registry.vault.status(&s).await.state == VaultState::Locked {
        return Err(HttpError(StatusCode::CONFLICT, "vault_locked"));
    }
    let items = conversation(&s, &session, request.steps).await?;
    if serde_json::to_vec(&items)
        .map_err(|_| HttpError::internal())?
        .len()
        > MAX_CONVERSATION_BYTES
    {
        return Err(HttpError(StatusCode::PAYLOAD_TOO_LARGE, "share_too_large"));
    }
    let chosen = crate::artifacts::shareable(
        &s,
        &session,
        &request.artifacts,
        MAX_ARTIFACT_BYTES,
        MAX_ARTIFACTS_BYTES,
    )
    .await?;
    let mut artifacts = Vec::with_capacity(chosen.len());
    let mut files = Vec::with_capacity(chosen.len());
    let mut total = 0_u64;
    for (index, (name, mime, bytes)) in chosen.into_iter().enumerate() {
        // Image decoding is CPU-bound and memory-hungry: off the async workers, one at a time.
        // The permit moves into the task, so a dropped request still holds it until done.
        let decode = DECODES
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HttpError::internal())?;
        let shared = s.clone();
        let (mime, bytes) = blocking(move || {
            let _decode = decode;
            prepare_artifact(&shared, &mime, bytes)
        })
        .await??;
        total += bytes.len() as u64;
        if total > MAX_ARTIFACTS_BYTES {
            return Err(HttpError(StatusCode::PAYLOAD_TOO_LARGE, "share_too_large"));
        }
        artifacts.push(SharedArtifact {
            index: index as u32,
            name: scrub(&s, &name)?,
            mime,
            size: bytes.len() as u64,
        });
        files.push(bytes);
    }
    let now = Utc::now();
    let record = Record {
        session_id: session.clone(),
        steps: request.steps,
        chat: SharedChat {
            title: scrub(&s, &state.title)?,
            shared_at: now,
            expires_at: request
                .expires_in_secs
                .map(|secs| now + chrono::Duration::seconds(secs as i64)),
            items,
            artifacts,
        },
    };
    let body = serde_json::to_vec(&record).map_err(|_| HttpError::internal())?;
    let _gate = GATE.lock().await;
    // A chat deleted while the snapshot was built must not leave a link behind.
    if !crate::artifacts::alive(&s, &session).await {
        return Err(HttpError::not_found());
    }
    let root = root(&s);
    let id = new_id();
    let staged_id = id.clone();
    let owner = session.clone();
    blocking(move || -> Result<(), HttpError> {
        let live = records(&root)
            .into_iter()
            .filter(|(_, record)| record.session_id == owner && !expired(record, Utc::now()))
            .count();
        if live >= MAX_SHARES_PER_SESSION {
            return Err(HttpError(StatusCode::CONFLICT, "too_many_shares"));
        }
        let staging = root.join(format!(".{staged_id}.tmp"));
        let written = private_dir(&root)
            .and_then(|()| private_dir(&staging))
            .and_then(|()| {
                for (index, bytes) in files.iter().enumerate() {
                    private_file(&staging.join(index.to_string()), bytes)?;
                }
                private_file(&staging.join(RECORD), &body)
            })
            .and_then(|()| std::fs::rename(&staging, root.join(&staged_id)));
        if let Err(error) = written {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(HttpError::write_failed(&error));
        }
        Ok(())
    })
    .await??;
    Ok((StatusCode::CREATED, Json(link(&s, &id, &record))))
}

pub(crate) async fn revoke(
    State(s): State<Shared>,
    UrlPath((session, id)): UrlPath<(String, String)>,
) -> Result<StatusCode, HttpError> {
    if !valid_id(&id) {
        return Err(not_found());
    }
    let session = SessionId::new(session);
    let _gate = GATE.lock().await;
    let root = root(&s);
    blocking(move || {
        if revoked(&root, &id) {
            return Err(not_found());
        }
        match read_record(&root, &id) {
            Some(record) if record.session_id == session => retire(&root, &id)
                .map(|()| StatusCode::NO_CONTENT)
                .map_err(|error| HttpError::write_failed(&error)),
            _ => Err(not_found()),
        }
    })
    .await?
}

/// Revokes every link of a chat that is gone ([`crate::session_delete::on_session_deleted`]).
/// Public reads also check that the chat is alive, so a tombstone that cannot be written
/// here still never lets a deleted chat's link answer.
pub(crate) async fn remove_session(shared: &Shared, session: &SessionId) {
    let _gate = GATE.lock().await;
    let root = root(shared);
    let session = session.clone();
    let _ = blocking(move || {
        for (id, record) in records(&root) {
            if record.session_id == session {
                if let Err(error) = retire(&root, &id) {
                    crate::diag!("opensided: revoking a deleted chat's share failed: {error}");
                }
            }
        }
    })
    .await;
}

/// How often expired links and links of deleted chats are removed in the background.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// Retires every link that expired by `now` or whose chat is gone, and deletes staging
/// directories a crash left behind; returns how many links it retired.
pub(crate) async fn sweep(shared: &Shared, now: DateTime<Utc>) -> usize {
    let _gate = GATE.lock().await;
    let stored = {
        let root = root(shared);
        blocking(move || records(&root)).await.unwrap_or_default()
    };
    let mut dead = Vec::new();
    for (id, record) in stored {
        if expired(&record, now) || shared.db.state(&record.session_id).await.is_err() {
            dead.push(id);
        }
    }
    let count = dead.len();
    let root = root(shared);
    let _ = blocking(move || {
        for id in dead {
            if let Err(error) = retire(&root, &id) {
                crate::diag!("opensided: retiring an expired share failed: {error}");
            }
        }
        // Creation stages under the same gate, so any staging directory seen here is stale.
        for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') && name.ends_with(".tmp") {
                if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                    crate::diag!(
                        "opensided: removing a stale share staging folder failed: {error}"
                    );
                }
            }
        }
    })
    .await;
    count
}

/// Sweeps at startup and then every [`SWEEP_EVERY`], so an expired snapshot does not wait
/// for someone to open or list it before its files are deleted.
pub(crate) fn start(shared: &Shared) {
    let shared = shared.clone();
    tokio::spawn(async move {
        let mut shutdown = shared.shutdown.subscribe();
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = tick.tick() => { sweep(&shared, Utc::now()).await; }
            }
        }
    });
}

/// The live record behind a public request and the slots it holds, or the response that
/// ends it. Only failed lookups spend the daemon-wide miss budget, so probing cannot lock
/// out real links.
// A refusal is already the finished response; it is built once on a cold path, so its size
// costs nothing worth a Box allocation.
#[allow(clippy::result_large_err)]
async fn open(s: &Shared, id: &str, source: &str) -> Result<(Record, Permits), Response> {
    let found = if valid_id(id) {
        let root = root(s);
        let owned = id.to_owned();
        blocking(move || {
            (!revoked(&root, &owned))
                .then(|| read_record(&root, &owned))
                .flatten()
        })
        .await
        .map_err(|error| public(error.into_response()))?
    } else {
        None
    };
    let live = match found {
        Some(record)
            if !expired(&record, Utc::now()) && s.db.state(&record.session_id).await.is_ok() =>
        {
            Some(record)
        }
        Some(_) => {
            let _gate = GATE.lock().await;
            let root = root(s);
            let owned = id.to_owned();
            let _ = blocking(move || retire(&root, &owned)).await;
            None
        }
        None => None,
    };
    let data_dir = s.config.data_dir.display();
    let Some(record) = live else {
        if !admit(format!("miss:{data_dir}:{source}"), MISS_RATE)
            || !admit(format!("miss:{data_dir}"), MISS_TOTAL_RATE)
        {
            return Err(public(rate_limited()));
        }
        return Err(public(not_found().into_response()));
    };
    if !admit(format!("link:{data_dir}:{id}:{source}"), LINK_RATE)
        || !admit(format!("link:{data_dir}:{id}"), LINK_TOTAL_RATE)
    {
        return Err(public(rate_limited()));
    }
    let (Some(global), Some(link), Some(own)) = (
        slot(format!("global:{data_dir}"), GLOBAL_IN_FLIGHT),
        slot(format!("slot:{data_dir}:{id}"), LINK_IN_FLIGHT),
        slot(format!("slot:{data_dir}:{id}:{source}"), SOURCE_IN_FLIGHT),
    ) else {
        return Err(public(rate_limited()));
    };
    Ok((
        record,
        Permits {
            _global: global,
            _link: link,
            _source: own,
        },
    ))
}

pub(crate) async fn get(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
    parts: axum::http::request::Parts,
) -> Response {
    let source = crate::remote::public_client(&s, &parts);
    let (record, _permits) = match open(&s, &id, &source).await {
        Ok(open) => open,
        Err(response) => return response,
    };
    let root = root(&s);
    let response = Json(record.chat).into_response();
    // A revoke that finished while this was built wins: see [`REVOKED`].
    let revocations = REVOKED.read();
    if is_revoked(&revocations, &root, &id) {
        return public(not_found().into_response());
    }
    public(response)
}

/// File bytes in [`CHUNK`]s; each chunk first checks that the link was not revoked meanwhile.
struct Download {
    file: tokio::fs::File,
    key: String,
    _permits: Permits,
    done: bool,
}

pub(crate) async fn artifact(
    State(s): State<Shared>,
    UrlPath((id, artifact)): UrlPath<(String, String)>,
    parts: axum::http::request::Parts,
) -> Response {
    let source = crate::remote::public_client(&s, &parts);
    let (record, permits) = match open(&s, &id, &source).await {
        Ok(open) => open,
        Err(response) => return response,
    };
    let Some(entry) = artifact
        .parse::<u32>()
        .ok()
        .and_then(|index| record.chat.artifacts.iter().find(|a| a.index == index))
        .cloned()
    else {
        return public(not_found().into_response());
    };
    let data_dir = s.config.data_dir.display();
    if !admit_weighted(
        format!("bytes:{data_dir}:{id}"),
        entry.size,
        LINK_BYTES_PER_WINDOW,
    ) {
        return public(rate_limited());
    }
    let root = root(&s);
    let Ok(file) = tokio::fs::File::open(root.join(&id).join(entry.index.to_string())).await else {
        return public(not_found().into_response());
    };
    let download = Download {
        file,
        key: revocation_key(&root, &id),
        _permits: permits,
        done: false,
    };
    let stream = futures_util::stream::unfold(download, |mut download| async move {
        if download.done {
            return None;
        }
        if REVOKED.read().contains(&download.key) {
            download.done = true;
            return Some((Err(std::io::Error::other("share revoked")), download));
        }
        let mut buffer = vec![0_u8; CHUNK];
        match download.file.read(&mut buffer).await {
            Ok(0) => None,
            // Checked again after the read: a revoke during it must not release this chunk.
            Ok(_) if REVOKED.read().contains(&download.key) => {
                download.done = true;
                Some((Err(std::io::Error::other("share revoked")), download))
            }
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok(axum::body::Bytes::from(buffer)), download))
            }
            Err(error) => {
                download.done = true;
                Some((Err(error), download))
            }
        }
    });
    let inline = entry.mime.starts_with("image/");
    let mut response = axum::body::Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&entry.mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(entry.size));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "{}; filename=\"{}\"",
            if inline { "inline" } else { "attachment" },
            entry.name.replace(['"', '\\'], "")
        ))
        .unwrap_or(HeaderValue::from_static("attachment")),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    // A revoke that finished while this was built wins: see [`REVOKED`].
    let revocations = REVOKED.read();
    if is_revoked(&revocations, &root, &id) {
        return public(not_found().into_response());
    }
    public(response)
}
