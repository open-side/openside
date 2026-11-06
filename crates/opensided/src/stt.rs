//! `GET /v1/stt` and `POST /v1/stt`: dictation for the person's composer.
//!
//! The web records a clip with MediaRecorder and posts it here because the browser's own
//! speech recognition needs vendor keys our Chromium build does not carry. A configured local
//! `channels.stt_cmd` wins, so audio stays on the computer whenever the person set one up;
//! otherwise the clip goes to OpenAI with the person's own API key. Clips and transcripts are
//! never stored, logged or turned into events: the text only goes back to the composer.

use std::{fmt::Write as _, path::Path, sync::Arc, time::Duration};

use axum::{body::Body, extract::State, http::StatusCode, Extension, Json};
use base64::Engine as _;
use openside_channels::{ChannelError, Stt};
use openside_core::MessageAuthorKind;
use openside_proto::{SttStatus, SttUnavailable, TranscribeRequest, Transcript};
use tokio::{
    io::AsyncWriteExt,
    sync::{Semaphore, SemaphorePermit},
};
use zeroize::Zeroizing;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SECONDS: u32 = 120;
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;
/// Room in a request body for everything but the clip: `mime`, `language`, JSON syntax.
const BODY_OVERHEAD: usize = 64 * 1024;
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
/// Lets endpoint tests point the OpenAI path at a local fake; compiled out of the daemon.
#[cfg(test)]
pub(crate) static TEST_OPENAI_BASE: parking_lot::Mutex<Option<String>> =
    parking_lot::Mutex::new(None);
const OPENAI_MODEL: &str = "gpt-4o-mini-transcribe";
const OPENAI_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// `openside_channels::Stt` reports an empty transcript with this reason.
const NO_SPEECH_REASON: &str = "no speech recognized";

/// Two clips are transcribed at a time: one person dictating never needs more.
pub(crate) static RUNNING: Semaphore = Semaphore::const_new(2);
/// Requests admitted at all: the two running plus four waiting. Every admitted request holds
/// its clip in memory, so beyond this one is refused (429 `stt_busy`) before its body is read
/// instead of queueing without bound; waiting is bounded by the running transcribers' timeouts.
pub(crate) static ADMITTED: Semaphore = Semaphore::const_new(6);

/// Why a transcriber produced no text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// It ran and heard nothing.
    NoSpeech,
    /// It failed or timed out; the cause is deliberately not kept.
    Failed,
}

impl From<Failure> for HttpError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::NoSpeech => HttpError(StatusCode::UNPROCESSABLE_ENTITY, "no_speech"),
            Failure::Failed => HttpError(StatusCode::BAD_GATEWAY, "stt_failed"),
        }
    }
}

fn invalid() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_audio")
}

fn too_large() -> HttpError {
    HttpError(StatusCode::PAYLOAD_TOO_LARGE, "audio_too_large")
}

fn max_bytes(s: &Shared) -> u64 {
    MAX_BYTES.min(s.channels.config().max_audio_bytes)
}

fn command(s: &Shared) -> Option<Stt> {
    let config = s.channels.config();
    Stt::new(
        config.stt_cmd?,
        Duration::from_secs(u64::from(config.stt_timeout_secs)),
        max_bytes(s),
    )
    .ok()
}

/// Answers every caller so a client can grey out its microphone instead of failing on POST;
/// an agent client learns only that it may not dictate, never whether a transcriber exists.
pub(crate) async fn status(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
) -> Json<SttStatus> {
    let reason = if crate::takeover::person_only(&caller, author).is_err() {
        Some(SttUnavailable::PersonDeviceRequired)
    } else if command(&s).is_some() || crate::model_accounts::openai_transcription_ready(&s).await {
        None
    } else {
        Some(SttUnavailable::NoTranscriber)
    };
    Json(SttStatus {
        available: reason.is_none(),
        reason,
        max_bytes: max_bytes(&s),
        max_seconds: MAX_SECONDS,
    })
}

/// Request body cap for this route: base64 of the effective `max_bytes` plus `BODY_OVERHEAD`,
/// so a smaller `channels.max_audio_bytes` also stops oversized bodies before they are read.
pub(crate) fn body_limit(s: &Shared) -> usize {
    usize::try_from(max_bytes(s).div_ceil(3) * 4)
        .unwrap_or(usize::MAX)
        .saturating_add(BODY_OVERHEAD)
}

/// A held `ADMITTED` permit. The TCP auth middleware takes it before reading the body and
/// passes it on as a request extension (which must be `Clone`, hence the `Arc`).
#[derive(Clone)]
pub(crate) struct Admission {
    _permit: Arc<SemaphorePermit<'static>>,
}

pub(crate) fn admit() -> Result<Admission, HttpError> {
    ADMITTED
        .try_acquire()
        .map(|permit| Admission {
            _permit: Arc::new(permit),
        })
        .map_err(|_| HttpError(StatusCode::TOO_MANY_REQUESTS, "stt_busy"))
}

/// How long an admitted request may take to deliver its body. 30 s fits a full ~11 MiB body
/// at about 3 Mbit/s; tests shorten it so stalled bodies time out quickly.
pub(crate) const BODY_READ_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(30)
};

pub(crate) fn body_timeout() -> HttpError {
    HttpError(StatusCode::REQUEST_TIMEOUT, "audio_timeout")
}

pub(crate) async fn transcribe(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    admission: Option<Extension<Admission>>,
    body: Body,
) -> Result<Json<Transcript>, HttpError> {
    crate::takeover::person_only(&caller, author)?;
    // The local socket skips the auth middleware, so its requests are admitted here.
    let _admitted = match admission {
        Some(Extension(admission)) => admission,
        None => admit()?,
    };
    // The body is read here rather than by `Json`, whose cap is fixed when the router is
    // built; on TCP the auth middleware has already applied the same limit and deadline.
    let body = tokio::time::timeout(
        BODY_READ_TIMEOUT,
        axum::body::to_bytes(body, body_limit(&s)),
    )
    .await
    .map_err(|_| body_timeout())?
    .map_err(|_| too_large())?;
    let request: TranscribeRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
    drop(body);
    let limit = max_bytes(&s);
    let encoded = request.audio_base64.trim();
    // The encoded length already bounds the decoded size; refuse before allocating for it.
    if encoded.len() as u64 > limit.div_ceil(3) * 4 {
        return Err(too_large());
    }
    let mime = essence(&request.mime).ok_or_else(invalid)?;
    let language = language(request.language.as_deref())?;
    // Decode only once running, so at most `RUNNING` decoded clips exist at a time.
    let _running = RUNNING.acquire().await.map_err(|_| HttpError::internal())?;
    let audio = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| invalid())?,
    );
    if audio.is_empty() {
        return Err(invalid());
    }
    if audio.len() as u64 > limit {
        return Err(too_large());
    }
    let text = if let Some(stt) = command(&s) {
        run_command(&s, &stt, &audio, &mime).await?
    } else if let Some(key) = crate::model_accounts::openai_transcription_key(&s).await {
        #[cfg(test)]
        let base = TEST_OPENAI_BASE
            .lock()
            .clone()
            .unwrap_or_else(|| OPENAI_API_BASE.into());
        #[cfg(not(test))]
        let base = OPENAI_API_BASE.to_owned();
        transcribe_openai(&base, &key, &audio, &mime, language.as_deref()).await?
    } else {
        return Err(HttpError(StatusCode::CONFLICT, "stt_unavailable"));
    };
    Ok(Json(Transcript {
        text: bounded(&text).ok_or(Failure::NoSpeech)?,
    }))
}

/// Lower-case `type/subtype` of an `audio/*` MIME type, parameters dropped.
fn essence(mime: &str) -> Option<String> {
    let mime = mime.trim();
    if mime.len() > 128 || !mime.bytes().all(|b| b.is_ascii_graphic() || b == b' ') {
        return None;
    }
    let essence = mime.split(';').next()?.trim().to_ascii_lowercase();
    let subtype = essence.strip_prefix("audio/")?;
    let token = |b: u8| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b);
    (!subtype.is_empty() && subtype.bytes().all(token)).then_some(essence)
}

/// A short BCP-47 tag (`ko`, `en-US`); empty means "detect".
fn language(tag: Option<&str>) -> Result<Option<String>, HttpError> {
    let Some(tag) = tag.map(str::trim).filter(|tag| !tag.is_empty()) else {
        return Ok(None);
    };
    let mut subtags = tag.split('-');
    let primary = subtags.next().unwrap_or_default();
    let valid = tag.len() <= 16
        && !primary.is_empty()
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && subtags.all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric()));
    if valid {
        Ok(Some(tag.to_owned()))
    } else {
        Err(invalid())
    }
}

/// File extension transcribers use to detect the container.
fn extension(essence: &str) -> &'static str {
    match essence {
        "audio/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mp4" => "mp4",
        "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "wav",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => "audio",
    }
}

/// Trimmed, non-empty text of at most `MAX_TRANSCRIPT_BYTES`, cut on a character boundary.
fn bounded(text: &str) -> Option<String> {
    let mut text = text.trim();
    if text.len() > MAX_TRANSCRIPT_BYTES {
        let mut end = MAX_TRANSCRIPT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text = text[..end].trim_end();
    }
    (!text.is_empty()).then(|| text.to_owned())
}

fn clips_root(s: &Shared) -> std::path::PathBuf {
    s.config.loaded.paths.runtime_dir.join("stt")
}

/// Remove clips a crash or kill left under the private directory; run at daemon start, before
/// any request can be using it. Entries are removed without following symlinks.
pub(crate) async fn sweep(s: &Shared) {
    let root = clips_root(s);
    match tokio::fs::symlink_metadata(&root).await {
        Ok(metadata) if metadata.is_dir() => {}
        _ => return,
    }
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let removed = match entry.file_type().await {
            Ok(kind) if kind.is_dir() => tokio::fs::remove_dir_all(&path).await,
            _ => tokio::fs::remove_file(&path).await,
        };
        if let Err(error) = removed {
            crate::diag!("opensided: removing a leftover dictation clip failed: {error}");
        }
    }
}

/// Run `channels.stt_cmd` on the clip in a private directory under the runtime directory.
/// One deadline (`stt_timeout_secs`) covers all of it, so a stalled filesystem cannot hold a
/// slot longer than the command could. The directory is removed when this returns, whether
/// the command succeeded, failed or timed out (dropping the work kills the command's process
/// group and then drops the directory); a crash leaves it for [`sweep`] at the next start.
async fn run_command(s: &Shared, stt: &Stt, audio: &[u8], mime: &str) -> Result<String, HttpError> {
    let root = clips_root(s);
    let work = async {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(&root)
            .await
            .map_err(|_| HttpError::internal())?;
        // `tempfile` creates the directory 0700, so no other user can open the clip.
        let dir = tempfile::Builder::new()
            .prefix("clip-")
            .tempdir_in(&root)
            .map_err(|_| HttpError::internal())?;
        let path = dir.path().join(format!("clip.{}", extension(mime)));
        write_private(&path, audio)
            .await
            .map_err(|_| HttpError::internal())?;
        let result = stt.transcribe(&path).await;
        drop(dir);
        match result {
            Ok(text) => Ok(text),
            Err(ChannelError::Refused(reason)) if reason == NO_SPEECH_REASON => {
                Err(Failure::NoSpeech.into())
            }
            Err(_) => Err(Failure::Failed.into()),
        }
    };
    tokio::time::timeout(stt.timeout(), work)
        .await
        .unwrap_or_else(|_| Err(Failure::Failed.into()))
}

async fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(bytes).await?;
    file.flush().await
}

/// Transcribe through OpenAI's `/audio/transcriptions` at `base` (production passes the
/// public API; tests a local server). reqwest is built without its multipart feature, so the
/// `multipart/form-data` body is assembled here. Only the primary language subtag is sent,
/// because the API takes ISO-639-1 codes.
pub(crate) async fn transcribe_openai(
    base: &str,
    key: &str,
    audio: &[u8],
    mime: &str,
    language: Option<&str>,
) -> Result<String, Failure> {
    let boundary = format!("openside-{:032x}", rand::random::<u128>());
    let mut head = String::new();
    let mut field = |name: &str, value: &str| {
        let _ = write!(
            head,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        );
    };
    field("model", OPENAI_MODEL);
    field("response_format", "json");
    if let Some(primary) = language.and_then(|tag| tag.split('-').next()) {
        field("language", &primary.to_ascii_lowercase());
    }
    let _ = write!(
        head,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.{}\"\r\nContent-Type: {mime}\r\n\r\n",
        extension(mime)
    );
    let tail = format!("\r\n--{boundary}--\r\n");
    let mut body = Vec::with_capacity(head.len() + audio.len() + tail.len());
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(audio);
    body.extend_from_slice(tail.as_bytes());

    let client = reqwest::Client::builder()
        .timeout(OPENAI_TIMEOUT)
        // A redirect would resend the bearer key and the clip somewhere else; 3xx is a failure.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("opensided/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| Failure::Failed)?;
    let mut response = client
        .post(format!(
            "{}/audio/transcriptions",
            base.trim_end_matches('/')
        ))
        .bearer_auth(key)
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|_| Failure::Failed)?;
    // Upstream error bodies can echo request details; they are never read or returned.
    if !response.status().is_success() {
        return Err(Failure::Failed);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Failed)? {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Failure::Failed);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Failed)?;
    let text = value
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or(Failure::Failed)?
        .trim();
    if text.is_empty() {
        return Err(Failure::NoSpeech);
    }
    Ok(text.to_owned())
}
