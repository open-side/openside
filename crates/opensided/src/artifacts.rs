//! Per-session task artifacts (A-12), message attachments (PP-14) and visual blocks (A-15).
//!
//! Layout under `<data_dir>/artifacts/<session>/` (directories 0700, files 0600):
//! `attachments/<message>/<n>-<name>` user uploads, `files/<workspace path>` copies of files
//! tools created or modified, `screenshots/<tool_call_id>.png` explicit screenshot outputs,
//! `visuals/<seq>-<n>.<ext>` visual blocks and `transcript.jsonl` (rewritten on every read).
//! The event log is the index: `message.queued` carries attachments, `artifact.saved` tool
//! files, `assistant.final` visual blocks. Bytes never enter the log.
//!
//! Preview threat model: artifacts are attacker-influenced (web pages, model output). The
//! preview route is unauthenticated so an `<iframe src>` can load it, and is instead bound to
//! one session, path and expiry by an HMAC under a per-process random key. Every preview and
//! raw response carries `Content-Security-Policy: sandbox` without `allow-same-origin`, so the
//! document gets an opaque origin even when opened top-level: it cannot read the daemon
//! origin's storage or cookies, script its parent, or send same-origin requests. Bearer tokens
//! live in the web UI's storage, never in cookies, so no request can carry them implicitly.
//! `default-src 'none'` blocks all network loads (no exfiltration, no API calls). No preview
//! runs script: CSP cannot stop a document navigating its own frame to a URL that carries
//! data, and without `allow-scripts` Chromium also ignores meta refresh in the sandbox.
//! `frame-ancestors 'self'` keeps other sites from framing it; `nosniff` pins the type.
//! All file access goes through `fsx`, descriptor-relative with `O_NOFOLLOW`, and every
//! producer and reader holds the session's lifecycle gate, so a deleted task's directory
//! is never recreated.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Component, Path, PathBuf},
    sync::LazyLock,
    time::SystemTime,
};

use axum::{
    extract::{Path as UrlPath, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use openside_core::{ContentKind, ContentPart, MessageId, SessionId};
use openside_policy::Capability;
use openside_proto::{
    Artifact, ArtifactList, ArtifactPurpose, ArtifactSource, Attachment, AttachmentUpload,
    EventKind, Visual, VisualKind, VisualList, MAX_ATTACHMENTS, MAX_ATTACHMENT_BYTES,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{api::HttpError, engine::Shared, tools::ToolEnv};

/// JSON body limit includes base64 padding and envelope/attachment metadata.
pub(crate) const SUBMIT_BODY_LIMIT: usize =
    MAX_ATTACHMENT_BYTES.div_ceil(3) * 4 + openside_agent_sidecar::MAX_USER_TEXT_BYTES + 64 * 1024;
/// Largest PNG handed to the model with a screenshot; larger captures are downscaled.
pub(crate) const MODEL_IMAGE_MAX_BYTES: usize = 1024 * 1024;
/// Bash write tracking gives up (records nothing) on trees with more regular files.
const MAX_TRACKED_ENTRIES: usize = 20_000;
/// Changed files above this size are not copied into the artifact directory.
const MAX_TRACKED_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// At most this many changed files are copied per tool call.
const MAX_TRACKED_FILES: usize = 200;
const PREVIEW_TTL_SECS: i64 = 600;
const TRANSCRIPT: &str = "transcript.jsonl";
/// Largest artifact served by the raw and preview routes.
const READ_MAX_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn session_dir(shared: &Shared, session: &SessionId) -> PathBuf {
    openside_config::data_layout::DataPath::Artifacts
        .path(&shared.config.data_dir)
        .join(session.as_str())
}

fn component(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', '\0'])
}

/// Validates an artifact path: relative, `/`-separated, no `.`/`..`/empty components.
fn relative(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.len() > 1024 || path.starts_with('/') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if !component(part) {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

#[cfg(unix)]
/// Descriptor-relative artifact I/O. Every component below `<data_dir>/artifacts` is opened
/// with `O_NOFOLLOW | O_DIRECTORY` and files with `O_NOFOLLOW`, so neither a symlinked
/// session root nor an intermediate directory swapped for a link can redirect a read or a
/// write. `data_dir` itself is daemon configuration and may legitimately contain links.
mod fsx {
    use std::{
        ffi::{OsStr, OsString},
        io::{self, Read, Write},
        os::fd::OwnedFd,
        path::{Component, Path},
    };

    use rustix::{
        fs::{self, AtFlags, FileType, Mode, OFlags, CWD},
        io::Errno,
    };

    const DIR: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    fn invalid(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }

    fn child_dir(parent: &OwnedFd, name: &OsStr, create: bool) -> io::Result<OwnedFd> {
        match fs::openat(parent, name, DIR, Mode::empty()) {
            Err(Errno::NOENT) if create => {
                match fs::mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(fs::openat(parent, name, DIR, Mode::empty())?)
            }
            other => Ok(other?),
        }
    }

    fn descend(mut dir: OwnedFd, rel: &Path, create: bool) -> io::Result<OwnedFd> {
        for component in rel.components() {
            match component {
                Component::Normal(name) => dir = child_dir(&dir, name, create)?,
                Component::RootDir => {}
                _ => return Err(invalid("path must be normalized")),
            }
        }
        Ok(dir)
    }

    /// `<data_dir>/artifacts/<rel>` as a directory descriptor.
    pub(super) fn open_dir(data_dir: &Path, rel: &Path, create: bool) -> io::Result<OwnedFd> {
        let root = fs::openat(
            CWD,
            data_dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let artifacts = child_dir(&root, OsStr::new("artifacts"), create)?;
        descend(artifacts, rel, create)
    }

    fn split(rel: &Path) -> io::Result<(&Path, &OsStr)> {
        match (rel.parent(), rel.file_name()) {
            (Some(parent), Some(name)) => Ok((parent, name)),
            _ => Err(invalid("artifact path needs a file name")),
        }
    }

    /// Reads a regular file; anything larger than `max` is an error, checked on the open
    /// descriptor so a file growing after a size check cannot slip past it.
    fn read_fd(fd: OwnedFd, max: u64) -> io::Result<Vec<u8>> {
        let stat = fs::fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode as _) != FileType::RegularFile {
            return Err(invalid("not a regular file"));
        }
        let mut bytes = Vec::new();
        std::fs::File::from(fd)
            .take(max.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
        }
        Ok(bytes)
    }

    fn open_file(dir: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
        // NONBLOCK keeps a FIFO from stalling the open; it has no effect on regular files.
        Ok(fs::openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?)
    }

    pub(super) fn read(data_dir: &Path, rel: &Path, max: u64) -> io::Result<Vec<u8>> {
        let (parent, name) = split(rel)?;
        let dir = open_dir(data_dir, parent, false)?;
        read_fd(open_file(&dir, name)?, max)
    }

    /// Reads an absolute canonical path without following any link on the way.
    pub(super) fn read_absolute(
        path: &Path,
        max: u64,
        protected: &crate::credential_paths::ProtectedPaths,
    ) -> io::Result<Vec<u8>> {
        if !path.is_absolute() {
            return Err(invalid("path must be absolute"));
        }
        let (parent, name) = split(path)?;
        let root = fs::openat(
            CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let dir = descend(root, parent, false)?;
        let fd = open_file(&dir, name)?;
        protected.check_fd(&fd)?;
        read_fd(fd, max)
    }

    pub(super) fn exists(data_dir: &Path, rel: &Path) -> bool {
        let Ok((parent, name)) = split(rel) else {
            return false;
        };
        open_dir(data_dir, parent, false)
            .and_then(|dir| Ok(fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW)?))
            .is_ok()
    }

    /// Publishes `bytes` at `rel` (0600). `exclusive` files must not exist yet and are never
    /// rewritten; others are written to a fresh temporary name and renamed over the old one.
    pub(super) fn write(
        data_dir: &Path,
        rel: &Path,
        bytes: &[u8],
        exclusive: bool,
    ) -> io::Result<()> {
        let (parent, name) = split(rel)?;
        let dir = open_dir(data_dir, parent, true)?;
        let target = if exclusive {
            name.to_os_string()
        } else {
            let mut temp = OsString::from(format!(".tmp-{:016x}-", rand::random::<u64>()));
            temp.push(name);
            temp
        };
        let fd = fs::openat(
            &dir,
            &target,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        let mut file = std::fs::File::from(fd);
        let mut result = file.write_all(bytes).and_then(|()| file.sync_all());
        if result.is_ok() && !exclusive {
            result = fs::renameat(&dir, &target, &dir, name).map_err(io::Error::from);
        }
        if result.is_err() {
            let _ = fs::unlinkat(&dir, &target, AtFlags::empty());
        }
        result
    }
}

#[cfg(not(unix))]
mod fsx {
    use std::{io, path::Path};
    pub(super) fn read(_: &Path, _: &Path, _: u64) -> io::Result<Vec<u8>> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub(super) fn read_absolute(
        _: &Path,
        _: u64,
        _: &crate::credential_paths::ProtectedPaths,
    ) -> io::Result<Vec<u8>> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub(super) fn exists(_: &Path, _: &Path) -> bool {
        false
    }
    pub(super) fn write(_: &Path, _: &Path, _: &[u8], _: bool) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

type Gate = std::sync::Arc<tokio::sync::Mutex<()>>;
static GATES: LazyLock<parking_lot::Mutex<HashMap<PathBuf, Gate>>> =
    LazyLock::new(Default::default);

/// Per-session lifecycle gate. Everything that publishes into a session's artifact directory
/// holds it across its liveness check, file writes and event append, and deletion holds it
/// while removing the directory: nothing can recreate files for a deleted task, and two
/// submissions of one message id cannot both publish attachments.
pub(crate) async fn lifecycle(
    shared: &Shared,
    session: &SessionId,
) -> tokio::sync::OwnedMutexGuard<()> {
    let gate = GATES
        .lock()
        .entry(session_dir(shared, session))
        .or_default()
        .clone();
    gate.lock_owned().await
}

async fn alive(shared: &Shared, session: &SessionId) -> bool {
    shared.db.state(session).await.is_ok()
}

/// Writes `bytes` to the session artifact path `rel`; the caller holds [`lifecycle`].
async fn store(
    shared: &Shared,
    session: &SessionId,
    rel: &str,
    bytes: Vec<u8>,
    exclusive: bool,
) -> std::io::Result<u64> {
    let relative = relative(rel).ok_or_else(|| std::io::Error::other("invalid artifact path"))?;
    if !component(session.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    let path = Path::new(session.as_str()).join(relative);
    let data_dir = shared.config.data_dir.clone();
    let size = bytes.len() as u64;
    tokio::task::spawn_blocking(move || fsx::write(&data_dir, &path, &bytes, exclusive))
        .await
        .map_err(std::io::Error::other)??;
    Ok(size)
}

async fn load(
    shared: &Shared,
    session: &SessionId,
    rel: &str,
    max: u64,
) -> std::io::Result<Vec<u8>> {
    let relative = relative(rel).ok_or_else(|| std::io::Error::other("invalid artifact path"))?;
    if !component(session.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    let path = Path::new(session.as_str()).join(relative);
    let data_dir = shared.config.data_dir.clone();
    tokio::task::spawn_blocking(move || fsx::read(&data_dir, &path, max))
        .await
        .map_err(std::io::Error::other)?
}

/// Stores a tool output in the artifact directory and records it as `artifact.saved`.
pub(crate) async fn save_tool_output(
    env: &ToolEnv,
    tool: &str,
    rel: &str,
    bytes: Vec<u8>,
) -> std::io::Result<()> {
    let _gate = lifecycle(&env.shared, &env.session).await;
    if !alive(&env.shared, &env.session).await {
        return Err(std::io::Error::other("session was deleted"));
    }
    let mime = mime_for(rel, &bytes);
    let size = store(&env.shared, &env.session, rel, bytes, false).await?;
    env.shared
        .emit(
            &env.session,
            None,
            EventKind::ArtifactSaved {
                path: rel.into(),
                size,
                mime,
                tool_call_id: env.tool_call_id.clone(),
                tool: tool.into(),
            },
        )
        .await
        .map(|_| ())
        .map_err(|_| std::io::Error::other("artifact event could not be recorded"))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Allowlisted magic-byte sniffing; declared types are never trusted.
pub(crate) fn sniff(bytes: &[u8]) -> Option<(ContentKind, &'static str, &'static str)> {
    let riff = |tag: &[u8]| bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == tag;
    Some(if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        (ContentKind::Image, "image/png", "png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        (ContentKind::Image, "image/jpeg", "jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        (ContentKind::Image, "image/gif", "gif")
    } else if riff(b"WEBP") {
        (ContentKind::Image, "image/webp", "webp")
    } else if bytes.starts_with(b"%PDF-") {
        (ContentKind::File, "application/pdf", "pdf")
    } else if riff(b"WAVE") {
        (ContentKind::Audio, "audio/wav", "wav")
    } else if bytes.starts_with(b"ID3")
        || (bytes.len() > 1 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        (ContentKind::Audio, "audio/mpeg", "mp3")
    } else if bytes.starts_with(b"OggS") {
        (ContentKind::Audio, "audio/ogg", "ogg")
    } else if bytes.starts_with(b"fLaC") {
        (ContentKind::Audio, "audio/flac", "flac")
    } else if !bytes.is_empty() && !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
        (ContentKind::Text, "text/plain", "txt")
    } else {
        return None;
    })
}

/// Content type for a stored artifact: extension first for text formats, then magic bytes.
pub(crate) fn mime_for(path: &str, bytes: &[u8]) -> String {
    let ext = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let by_ext = match ext.as_str() {
        "html" | "htm" => Some("text/html"),
        "svg" => Some("image/svg+xml"),
        "md" | "markdown" => Some("text/markdown"),
        "csv" => Some("text/csv"),
        "json" | "jsonl" => Some("application/json"),
        _ => None,
    };
    if let Some(mime) = by_ext {
        return mime.into();
    }
    match sniff(bytes) {
        Some((_, mime, _)) => mime.into(),
        None => "application/octet-stream".into(),
    }
}

fn file_name(name: Option<&str>, ext: &str) -> String {
    let base = name
        .and_then(|name| name.rsplit(['/', '\\']).next())
        .unwrap_or_default();
    let clean: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(64)
        .collect();
    let clean = clean.trim_start_matches('.');
    if clean.is_empty() {
        format!("attachment.{ext}")
    } else {
        clean.to_owned()
    }
}

fn message_dir(id: &MessageId) -> String {
    let raw = id.as_str();
    if raw.len() <= 64
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        raw.to_owned()
    } else {
        sha256_hex(raw.as_bytes())[..32].to_owned()
    }
}

/// Validates, sniffs and stores uploads before `message.queued` is written, so the event
/// only ever references bytes that exist.
pub(crate) async fn store_attachments(
    shared: &Shared,
    session: &SessionId,
    message: &MessageId,
    uploads: Vec<AttachmentUpload>,
) -> Result<Vec<Attachment>, HttpError> {
    let too_large = HttpError(StatusCode::PAYLOAD_TOO_LARGE, "attachments_too_large");
    if uploads.len() > MAX_ATTACHMENTS {
        return Err(too_large);
    }
    let mut decoded = Vec::with_capacity(uploads.len());
    let mut total = 0;
    for upload in uploads {
        // Reject before decoding: base64 never decodes to fewer than 3/4 of its length minus padding.
        if (upload.data_base64.len() / 4 * 3).saturating_sub(2) > MAX_ATTACHMENT_BYTES - total {
            return Err(too_large);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(upload.data_base64.as_bytes())
            .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_attachment"))?;
        total += bytes.len();
        if total > MAX_ATTACHMENT_BYTES {
            return Err(too_large);
        }
        let (kind, mime, ext) = sniff(&bytes).ok_or(HttpError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_attachment",
        ))?;
        decoded.push((upload.name, bytes, kind, mime, ext));
    }
    let mut stored = Vec::with_capacity(decoded.len());
    for (index, (name, bytes, kind, mime, ext)) in decoded.into_iter().enumerate() {
        let file = file_name(name.as_deref(), ext);
        let path = format!("attachments/{}/{}-{file}", message_dir(message), index + 1);
        let sha256 = sha256_hex(&bytes);
        // Published files are immutable. A leftover from an attempt whose event append failed
        // is reused only when it holds exactly these bytes.
        let size = match store(shared, session, &path, bytes.clone(), true).await {
            Ok(size) => size,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match load(shared, session, &path, MAX_ATTACHMENT_BYTES as u64).await {
                    Ok(existing) if existing == bytes => bytes.len() as u64,
                    _ => return Err(HttpError(StatusCode::CONFLICT, "attachment_conflict")),
                }
            }
            Err(error) => return Err(HttpError::write_failed(&error)),
        };
        stored.push(Attachment {
            path,
            kind,
            mime: mime.into(),
            size,
            sha256,
            name: name.map(|_| file),
        });
    }
    Ok(stored)
}

/// Loads a message's attachments for the runtime. The bytes must match the size and SHA-256
/// recorded in the event; a missing or altered file becomes a reference-only part so the
/// model is told about it instead of receiving substituted content.
pub(crate) async fn message_parts(
    shared: &Shared,
    session: &SessionId,
    attachments: Option<&Vec<Attachment>>,
) -> Vec<ContentPart> {
    let mut parts = Vec::new();
    for attachment in attachments.into_iter().flatten() {
        let data = load(shared, session, &attachment.path, attachment.size)
            .await
            .ok()
            .filter(|bytes| {
                bytes.len() as u64 == attachment.size && sha256_hex(bytes) == attachment.sha256
            });
        parts.push(ContentPart {
            kind: attachment.kind,
            mime: attachment.mime.clone(),
            data,
            artifact_ref: Some(attachment.path.clone()),
            name: attachment.name.clone(),
        });
    }
    parts
}

/// PNG for vision models: unchanged when small enough, otherwise halved up to three times.
pub(crate) fn model_image(png_bytes: &[u8]) -> Option<Vec<u8>> {
    if png_bytes.len() <= MODEL_IMAGE_MAX_BYTES {
        return Some(png_bytes.to_vec());
    }
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return None;
    }
    let channels = color.samples();
    let (mut width, mut height) = (frame.width as usize, frame.height as usize);
    let mut pixels = buffer[..frame.buffer_size()].to_vec();
    for _ in 0..3 {
        let (half_w, half_h) = ((width / 2).max(1), (height / 2).max(1));
        let mut next = vec![0u8; half_w * half_h * channels];
        for y in 0..half_h {
            for x in 0..half_w {
                for c in 0..channels {
                    let mut sum = 0u32;
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        let sy = (2 * y + dy).min(height - 1);
                        let sx = (2 * x + dx).min(width - 1);
                        sum += u32::from(pixels[(sy * width + sx) * channels + c]);
                    }
                    next[(y * half_w + x) * channels + c] = (sum / 4) as u8;
                }
            }
        }
        (width, height, pixels) = (half_w, half_h, next);
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(&pixels).ok()?;
        }
        if out.len() <= MODEL_IMAGE_MAX_BYTES {
            return Some(out);
        }
    }
    None
}

/// What a write-capable tool may change, captured before it runs.
pub(crate) enum Tracker {
    Off,
    File(PathBuf),
    Tree {
        root: PathBuf,
        before: HashMap<PathBuf, (SystemTime, u64)>,
    },
}

/// `skip` is the daemon data directory: a workspace containing it must not report the
/// daemon's own database and artifact writes as tool output.
fn snapshot(root: &Path, skip: &Path) -> Option<HashMap<PathBuf, (SystemTime, u64)>> {
    let mut files = HashMap::new();
    let mut walked = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            walked += 1;
            if walked > MAX_TRACKED_ENTRIES {
                return None;
            }
            // DirEntry metadata does not follow symlinks, so links are neither walked nor copied.
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if entry.file_name() != ".git" && entry.path() != skip {
                    stack.push(entry.path());
                }
            } else if meta.is_file() {
                files.insert(
                    entry.path(),
                    (
                        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        meta.len(),
                    ),
                );
            }
        }
    }
    Some(files)
}

/// Credential-bearing names that are never copied, whatever the policy says.
fn sensitive_path(path: &Path) -> bool {
    const DIRS: [&str; 6] = [".ssh", ".aws", ".gnupg", ".kube", ".docker", ".config"];
    const NAMES: [&str; 9] = [
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        "credentials",
        "id_rsa",
        "id_ecdsa",
        "id_ed25519",
        "id_dsa",
    ];
    const SUFFIXES: [&str; 6] = [".pem", ".key", ".p12", ".pfx", ".kdbx", ".keystore"];
    let in_dir = path
        .components()
        .any(|c| matches!(c, Component::Normal(name) if DIRS.iter().any(|dir| name == *dir)));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    in_dir
        || name.starts_with(".env")
        || NAMES.contains(&name.as_str())
        || SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Whether a changed file may be opened for collection: never daemon-private storage or
/// the configured Vault (whatever their names), never credential names, and only when the
/// session may read it on its own (the tool was approved to write or execute, not read).
fn collectable(env: &ToolEnv, path: &Path) -> bool {
    let Some(target) = path.to_str() else {
        return false;
    };
    // Compare resolved locations too: a workspace path can reach protected storage
    // through a symlinked parent.
    let resolved = path
        .parent()
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .zip(path.file_name())
        .map(|(parent, name)| parent.join(name));
    let data_dir = &env.shared.config.data_dir;
    let data_dirs = [Some(data_dir.clone()), std::fs::canonicalize(data_dir).ok()];
    let vault = Some(env.shared.config.loaded.config.vault.file(data_dir));
    let vaults = [
        vault.clone(),
        vault.as_ref().and_then(|v| std::fs::canonicalize(v).ok()),
    ];
    let protected = |candidate: &Path| {
        env.shared.credential_paths.is_protected(candidate)
            || data_dirs
                .iter()
                .flatten()
                .any(|dir| candidate.starts_with(dir))
            || vaults.iter().flatten().any(|vault| {
                // Lock and backup files sit beside the Vault and share its name as a prefix.
                candidate.parent() == vault.parent()
                    && candidate
                        .file_name()
                        .zip(vault.file_name())
                        .is_some_and(|(name, vault)| {
                            name.as_encoded_bytes()
                                .starts_with(vault.as_encoded_bytes())
                        })
            })
    };
    !(protected(path)
        || resolved.as_deref().is_some_and(protected)
        || sensitive_path(path)
        || env
            .policy
            .decide(&openside_policy::Action::new(Capability::FsRead, target))
            .effect
            != openside_policy::Effect::Allow)
}

/// Screens contents for secrets the vault redactor knows. Binary files are decoded lossily:
/// invalid bytes become U+FFFD while every valid UTF-8 run, and so any embedded secret,
/// survives for matching.
fn clean(env: &ToolEnv, bytes: &[u8]) -> bool {
    let original = serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned());
    env.shared
        .registry
        .redact(original.clone())
        .is_ok_and(|redacted| redacted == original)
}

impl Tracker {
    /// `write`/`edit` name their approved target; `bash` may write anywhere under the
    /// workspace (the only writable bind inside bubblewrap) or, unsandboxed without a
    /// workspace, under its working directory, so that tree is compared before and after.
    pub(crate) async fn before(env: &ToolEnv, tool: &str) -> Self {
        match tool {
            "write" | "edit" => env
                .target(Capability::FsWrite)
                .map(|path| Self::File(path.to_owned()))
                .unwrap_or(Self::Off),
            "bash" => {
                let root = match &env.workspace {
                    Some(workspace) => workspace.clone(),
                    None => match env.target(Capability::Exec) {
                        Ok(cwd) => cwd.to_owned(),
                        Err(_) => return Self::Off,
                    },
                };
                let walk = root.clone();
                let skip = env.shared.config.data_dir.clone();
                match tokio::task::spawn_blocking(move || snapshot(&walk, &skip)).await {
                    Ok(Some(before)) => Self::Tree { root, before },
                    _ => Self::Off,
                }
            }
            _ => Self::Off,
        }
    }

    /// Copies files the call created or modified into `files/` and records them.
    pub(crate) async fn record(self, env: &ToolEnv, tool: &str) {
        let changed = match self {
            Self::Off => return,
            Self::File(path) => vec![path],
            Self::Tree { root, before } => {
                let walk = root.clone();
                let skip = env.shared.config.data_dir.clone();
                let Ok(Some(after)) =
                    tokio::task::spawn_blocking(move || snapshot(&walk, &skip)).await
                else {
                    return;
                };
                let mut changed: Vec<PathBuf> = after
                    .into_iter()
                    .filter(|(path, stamp)| before.get(path) != Some(stamp))
                    .map(|(path, _)| path)
                    .collect();
                changed.sort();
                changed
            }
        };
        for path in changed.into_iter().take(MAX_TRACKED_FILES) {
            if !collectable(env, &path) {
                continue;
            }
            let source = path.clone();
            let protected = env.shared.credential_paths.clone();
            let Ok(Ok(bytes)) = tokio::task::spawn_blocking(move || {
                fsx::read_absolute(&source, MAX_TRACKED_FILE_BYTES, &protected)
            })
            .await
            else {
                continue;
            };
            if !clean(env, &bytes) {
                continue;
            }
            let inside = env
                .workspace
                .as_ref()
                .and_then(|workspace| path.strip_prefix(workspace).ok());
            let rel = match inside {
                Some(rel) => rel.to_path_buf(),
                None => path
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(name) => Some(name),
                        _ => None,
                    })
                    .collect(),
            };
            let Some(rel) = rel.to_str().map(|rel| format!("files/{rel}")) else {
                continue;
            };
            if let Err(error) = save_tool_output(env, tool, &rel, bytes).await {
                crate::diag!("opensided: artifact copy failed: {error}");
            }
        }
    }
}

/// Per-process preview signing key; restarting the daemon invalidates outstanding URLs.
static PREVIEW_KEY: LazyLock<[u8; 32]> = LazyLock::new(|| {
    use rand::RngCore;
    let mut key = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    key
});

fn signature(session: &SessionId, path: &str, exp: i64) -> Hmac<Sha256> {
    #[allow(clippy::expect_used)] // HMAC accepts keys of any length.
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&*PREVIEW_KEY).expect("any key size");
    mac.update(format!("{}\n{path}\n{exp}", session.as_str()).as_bytes());
    mac
}

fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn preview_url(session: &SessionId, path: &str) -> String {
    signed_preview_url(session, path, Utc::now().timestamp() + PREVIEW_TTL_SECS)
}

pub(crate) fn signed_preview_url(session: &SessionId, path: &str, exp: i64) -> String {
    let sig: String = signature(session, path, exp)
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(
        "/v1/sessions/{}/artifacts/{}/preview?exp={exp}&sig={sig}",
        encode_segment(session.as_str()),
        encode_segment(path)
    )
}

/// Content type and CSP for sandboxed rendering, or `None` when not previewable.
fn preview_kind(mime: &str) -> Option<(&'static str, &'static str)> {
    // No preview runs script: CSP has no navigation directive, so a scripted document could
    // send data out by navigating its own frame. Without `allow-scripts` Chromium also
    // refuses `<meta http-equiv=refresh>` in the sandbox; links need a user click.
    const HTML: &str = "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";
    const STATIC: &str = "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; frame-ancestors 'self'";
    Some(match mime {
        "text/html" => ("text/html; charset=utf-8", HTML),
        "image/svg+xml" => ("image/svg+xml", STATIC),
        "image/png" => ("image/png", STATIC),
        "image/jpeg" => ("image/jpeg", STATIC),
        "image/gif" => ("image/gif", STATIC),
        "image/webp" => ("image/webp", STATIC),
        "application/pdf" => ("application/pdf", STATIC),
        "application/json" | "text/plain" | "text/markdown" | "text/csv" => {
            ("text/plain; charset=utf-8", STATIC)
        }
        _ => return None,
    })
}

#[derive(Default)]
struct Entry {
    size: u64,
    mime: String,
    source: Option<ArtifactSource>,
    created: Option<DateTime<Utc>>,
    modified: Option<DateTime<Utc>>,
    tool_call_id: Option<String>,
    tool: Option<String>,
}

/// Visual blocks: fenced code whose info string is `html` or `svg` followed by `visual`.
fn visual_blocks(text: &str) -> Vec<(VisualKind, String)> {
    let mut blocks = Vec::new();
    let mut open: Option<(String, VisualKind, Vec<&str>)> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some((fence, kind, body)) = open.as_mut() {
            if trimmed != fence.as_str() {
                body.push(line);
                continue;
            }
            blocks.push((*kind, body.join("\n")));
            open = None;
            continue;
        }
        let fence_len = trimmed.chars().take_while(|c| *c == '`').count();
        if fence_len < 3 {
            continue;
        }
        let mut words = trimmed[fence_len..].split_whitespace();
        let kind = match words.next() {
            Some("html") => VisualKind::Html,
            Some("svg") => VisualKind::Svg,
            _ => continue,
        };
        if words.any(|word| word == "visual") {
            open = Some(("`".repeat(fence_len), kind, Vec::new()));
        }
    }
    blocks
}

struct Index {
    entries: BTreeMap<String, Entry>,
    order: Vec<String>,
    visuals: Vec<(u64, usize, VisualKind, String)>,
    first: Option<DateTime<Utc>>,
    last: Option<DateTime<Utc>>,
}

async fn index(shared: &Shared, session: &SessionId) -> Result<Index, HttpError> {
    let mut index = Index {
        entries: BTreeMap::new(),
        order: Vec::new(),
        visuals: Vec::new(),
        first: None,
        last: None,
    };
    let upsert = |index: &mut Index, path: &str, ts: DateTime<Utc>, update: Entry| {
        if !index.entries.contains_key(path) {
            index.order.push(path.to_owned());
        }
        let entry = index.entries.entry(path.to_owned()).or_default();
        entry.created.get_or_insert(ts);
        entry.modified = Some(ts);
        entry.size = update.size;
        entry.mime = update.mime;
        entry.source = update.source;
        entry.tool_call_id = update.tool_call_id;
        entry.tool = update.tool;
    };
    let mut transcript = Vec::new();
    let mut after = 0;
    loop {
        let events = shared
            .db
            .events(session, after)
            .await
            .map_err(|_| HttpError::internal())?;
        let Some(last) = events.last() else { break };
        // events() uses an exclusive cursor; retaining the last sequence skips no event.
        after = last.seq;
        index.last = Some(last.ts);
        index.first = index.first.or_else(|| events.first().map(|event| event.ts));
        for event in &events {
            match &event.kind {
                EventKind::MessageQueued { attachments, .. } => {
                    for attachment in attachments {
                        upsert(
                            &mut index,
                            &attachment.path,
                            event.ts,
                            Entry {
                                size: attachment.size,
                                mime: attachment.mime.clone(),
                                source: Some(ArtifactSource::Attachment),
                                ..Entry::default()
                            },
                        );
                    }
                }
                EventKind::ArtifactSaved {
                    path,
                    size,
                    mime,
                    tool_call_id,
                    tool,
                } => upsert(
                    &mut index,
                    path,
                    event.ts,
                    Entry {
                        size: *size,
                        mime: mime.clone(),
                        source: Some(ArtifactSource::Tool),
                        tool_call_id: Some(tool_call_id.clone()),
                        tool: Some(tool.clone()),
                        ..Entry::default()
                    },
                ),
                EventKind::AssistantFinal { text } => {
                    for (n, (kind, source)) in visual_blocks(text).into_iter().enumerate() {
                        let path = visual_path(event.seq, n, kind);
                        upsert(
                            &mut index,
                            &path,
                            event.ts,
                            Entry {
                                size: source.len() as u64,
                                mime: match kind {
                                    VisualKind::Html => "text/html",
                                    VisualKind::Svg => "image/svg+xml",
                                }
                                .into(),
                                source: Some(ArtifactSource::Visual),
                                ..Entry::default()
                            },
                        );
                        index.visuals.push((event.seq, n, kind, source));
                    }
                }
                _ => {}
            }
            // The transcript is the complete redacted public log, one event per line.
            serde_json::to_writer(&mut transcript, event).map_err(|_| HttpError::internal())?;
            transcript.push(b'\n');
        }
    }
    let size = store(shared, session, TRANSCRIPT, transcript, false)
        .await
        .map_err(|_| HttpError::internal())?;
    if let (Some(first), Some(last)) = (index.first, index.last) {
        index.order.push(TRANSCRIPT.into());
        index.entries.insert(
            TRANSCRIPT.into(),
            Entry {
                size,
                mime: "application/json".into(),
                source: Some(ArtifactSource::Transcript),
                created: Some(first),
                modified: Some(last),
                ..Entry::default()
            },
        );
    }
    for (seq, n, kind, source) in &index.visuals {
        let path = visual_path(*seq, *n, *kind);
        let rel = Path::new(session.as_str()).join(&path);
        let data_dir = shared.config.data_dir.clone();
        let present = tokio::task::spawn_blocking(move || fsx::exists(&data_dir, &rel))
            .await
            .unwrap_or(false);
        if !present {
            match store(shared, session, &path, source.clone().into_bytes(), true).await {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(HttpError::write_failed(&error)),
            }
        }
    }
    Ok(index)
}

fn visual_path(seq: u64, n: usize, kind: VisualKind) -> String {
    let ext = match kind {
        VisualKind::Html => "html",
        VisualKind::Svg => "svg",
    };
    format!("visuals/{seq}-{n}.{ext}")
}

/// Holds the session's lifecycle gate after confirming, under it, that the session is live:
/// a deletion that already ran cannot be followed by this request recreating files.
async fn gated(
    shared: &Shared,
    id: String,
) -> Result<(SessionId, tokio::sync::OwnedMutexGuard<()>), HttpError> {
    let id = SessionId::new(id);
    let gate = lifecycle(shared, &id).await;
    if !alive(shared, &id).await {
        return Err(HttpError::not_found());
    }
    Ok((id, gate))
}

/// Type served for an artifact: uploads keep their sniffed type, so a text upload named
/// `.html` is never served as HTML; tool and visual files are typed by extension.
fn served_mime(path: &str, bytes: &[u8]) -> String {
    if path.starts_with("attachments/") {
        sniff(bytes)
            .map_or("application/octet-stream", |(_, mime, _)| mime)
            .into()
    } else {
        mime_for(path, bytes)
    }
}

pub(crate) async fn list(
    State(shared): State<Shared>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ArtifactList>, HttpError> {
    let (id, _gate) = gated(&shared, id).await?;
    let mut index = index(&shared, &id).await?;
    let artifacts = index
        .order
        .iter()
        .filter_map(|path| {
            let entry = index.entries.remove(path)?;
            let purpose = ArtifactPurpose::classify(path, entry.tool.as_deref());
            if purpose == ArtifactPurpose::Working {
                return None;
            }
            let created = entry.created?;
            Some(Artifact {
                preview_url: preview_kind(&entry.mime).map(|_| preview_url(&id, path)),
                path: path.clone(),
                size: entry.size,
                mime: entry.mime,
                source: entry.source?,
                purpose,
                created_at: created,
                modified_at: entry.modified.unwrap_or(created),
                tool_call_id: entry.tool_call_id,
                tool: entry.tool,
            })
        })
        .collect();
    Ok(Json(ArtifactList { artifacts }))
}

/// Cards may reuse delivered images, never a legacy working screenshot or an unindexed file.
pub(crate) async fn card_image(
    shared: &Shared,
    session: &SessionId,
    path: &str,
    max: u64,
) -> Result<(Vec<u8>, String), HttpError> {
    let (_, _gate) = gated(shared, session.as_str().to_owned()).await?;
    relative(path).ok_or_else(HttpError::not_found)?;
    let mut index = index(shared, session).await?;
    let entry = index
        .entries
        .remove(path)
        .ok_or_else(HttpError::not_found)?;
    if ArtifactPurpose::classify(path, entry.tool.as_deref()) == ArtifactPurpose::Working {
        return Err(HttpError::not_found());
    }
    let bytes = load(shared, session, path, max)
        .await
        .map_err(|_| HttpError::not_found())?;
    Ok((bytes, entry.mime))
}

pub(crate) async fn visuals(
    State(shared): State<Shared>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<VisualList>, HttpError> {
    let (id, _gate) = gated(&shared, id).await?;
    let index = index(&shared, &id).await?;
    let visuals = index
        .visuals
        .into_iter()
        .map(|(seq, n, kind, source)| {
            let path = visual_path(seq, n, kind);
            Visual {
                id: format!("{seq}-{n}"),
                seq,
                kind,
                source,
                preview_url: preview_url(&id, &path),
                path,
            }
        })
        .collect();
    Ok(Json(VisualList { visuals }))
}

async fn read_artifact(
    shared: &Shared,
    session: &SessionId,
    path: &str,
) -> Result<Vec<u8>, HttpError> {
    relative(path).ok_or(HttpError::not_found())?;
    if path == TRANSCRIPT {
        index(shared, session).await?;
    }
    load(shared, session, path, READ_MAX_BYTES)
        .await
        .map_err(|_| HttpError::not_found())
}

fn header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or(HeaderValue::from_static("application/octet-stream"))
}

pub(crate) async fn raw(
    State(shared): State<Shared>,
    UrlPath((id, path)): UrlPath<(String, String)>,
) -> Result<Response, HttpError> {
    let (id, _gate) = gated(&shared, id).await?;
    let bytes = read_artifact(&shared, &id, &path).await?;
    let mime = served_mime(&path, &bytes);
    let name = file_name(path.rsplit('/').next(), "bin");
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, header_value(&mime));
    headers.insert(
        header::CONTENT_DISPOSITION,
        header_value(&format!("attachment; filename=\"{name}\"")),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Deserialize)]
pub(crate) struct PreviewQuery {
    exp: Option<i64>,
    sig: Option<String>,
}

pub(crate) async fn preview(
    State(shared): State<Shared>,
    UrlPath((id, path)): UrlPath<(String, String)>,
    Query(query): Query<PreviewQuery>,
) -> Result<Response, HttpError> {
    let invalid = HttpError(StatusCode::BAD_REQUEST, "invalid_preview_signature");
    let (Some(exp), Some(sig)) = (query.exp, query.sig) else {
        return Err(invalid);
    };
    let session = SessionId::new(id.clone());
    let sig = (0..sig.len())
        .step_by(2)
        .map(|i| {
            sig.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>()
        .ok_or(HttpError(
            StatusCode::BAD_REQUEST,
            "invalid_preview_signature",
        ))?;
    if exp < Utc::now().timestamp() || signature(&session, &path, exp).verify_slice(&sig).is_err() {
        return Err(invalid);
    }
    let (id, _gate) = gated(&shared, id).await?;
    let bytes = read_artifact(&shared, &id, &path).await?;
    let mime = served_mime(&path, &bytes);
    let (content_type, csp) = preview_kind(&mime).ok_or(HttpError(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "preview_unsupported",
    ))?;
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Session deletion removes the whole artifact directory.
pub(crate) async fn remove_session(shared: &Shared, session: &SessionId) {
    if !component(session.as_str()) {
        return;
    }
    // Under the gate: producers that started before the tombstone finish first, and any
    // that come after see the session as deleted and create nothing.
    let _gate = lifecycle(shared, session).await;
    let dir = session_dir(shared, session);
    let removed = tokio::fs::remove_dir_all(&dir).await;
    GATES.lock().remove(&dir);
    match removed {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        // The tombstone is already durable; `sweep_deleted` retries on the next start.
        Err(error) => crate::diag!("opensided: artifact cleanup after session deletion failed (retried at startup): {error}"),
    }
}

/// Retries cleanup that failed at deletion time: any artifact directory whose session is
/// unknown or deleted is removed at startup, so a successful DELETE never leaves files
/// behind for good.
pub(crate) async fn sweep_deleted(shared: &Shared) {
    let Ok(mut entries) = tokio::fs::read_dir(
        openside_config::data_layout::DataPath::Artifacts.path(&shared.config.data_dir),
    )
    .await
    else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let session = SessionId::new(name);
        if component(session.as_str()) && !alive(shared, &session).await {
            remove_session(shared, &session).await;
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod descriptor_security_tests {
    use super::*;

    #[test]
    fn collection_checks_opened_identity_and_pseudo_filesystems_before_reading() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.txt");
        let alias = root.path().join("ordinary.txt");
        std::fs::write(&source, "test-artifact-identity-canary").unwrap();
        let protected = crate::credential_paths::ProtectedPaths::build(
            &serde_json::json!({"test_source":{"token_file":source}}),
            None,
            None,
        );
        std::fs::hard_link(&source, &alias).unwrap();
        assert!(fsx::read_absolute(&alias, 1024, &protected).is_err());
        std::fs::rename(&source, root.path().join("renamed.txt")).unwrap();
        assert!(fsx::read_absolute(&alias, 1024, &protected).is_err());
        let mut child = std::process::Command::new(openside_config::find_on_path("sleep").unwrap())
            .arg("10")
            .env_clear()
            .env("TEST_API_KEY", "test-artifact-process-canary")
            .spawn()
            .unwrap();
        let result = fsx::read_absolute(
            Path::new(&format!("/proc/{}/environ", child.id())),
            1024,
            &protected,
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(result.is_err());
        assert!(!format!("{result:?}").contains("test-artifact-process-canary"));
    }
}
