//! The embedding model download behind `/v1/memory/model`. The memory store itself stays
//! offline; this runs only when a person asks for it, through the shared fetcher that
//! `openside memory model fetch` also uses: checksums come from the model hub's file listing,
//! every request and redirect must pass `network.allow`, and the verified model is swapped in
//! atomically.

use axum::{body::Bytes, extract::State, http::StatusCode, Extension, Json};
use chrono::{DateTime, Utc};
use openside_model_fetch::{CancellationToken, Failure, Phase, Progress, Source};
use openside_proto::{
    ApiError, MemoryModelSource, MemoryModelState, MemoryModelStatus, StartMemoryModelRequest,
};
use parking_lot::Mutex;
use serde_json::json;

use crate::{api::HttpError, engine::Shared, remote::LocalAuthority};

#[derive(Default)]
pub(crate) struct Job {
    inner: Mutex<Inner>,
}

struct Inner {
    state: MemoryModelState,
    /// The source of the current or last download.
    source: Option<Source>,
    progress: Option<Progress>,
    error: Option<ApiError>,
    /// Model installed by this process after it loaded its memory store.
    installed: Option<String>,
    finished_at: Option<DateTime<Utc>>,
    cancel: Option<CancellationToken>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            state: MemoryModelState::Idle,
            source: None,
            progress: None,
            error: None,
            installed: None,
            finished_at: None,
            cancel: None,
        }
    }
}

impl Inner {
    fn running(&self) -> bool {
        matches!(
            self.state,
            MemoryModelState::Resolving
                | MemoryModelState::Downloading
                | MemoryModelState::Verifying
                | MemoryModelState::Installing
        )
    }
}

fn state_of(phase: Phase) -> MemoryModelState {
    match phase {
        Phase::Resolving => MemoryModelState::Resolving,
        Phase::Downloading => MemoryModelState::Downloading,
        Phase::Verifying => MemoryModelState::Verifying,
        Phase::Installing => MemoryModelState::Installing,
    }
}

impl Job {
    /// One line for the diagnostics report while a download runs or has just failed.
    pub(crate) fn headline(&self) -> Option<(MemoryModelState, String)> {
        let inner = self.inner.lock();
        match inner.state {
            MemoryModelState::Idle | MemoryModelState::Installed | MemoryModelState::Cancelled => {
                None
            }
            MemoryModelState::Failed => Some((
                inner.state,
                format!(
                    "the last model download failed: {}",
                    inner
                        .error
                        .as_ref()
                        .map_or("unknown error", |error| error.message.as_str())
                ),
            )),
            running => {
                let percent = inner.progress.as_ref().and_then(|progress| {
                    (progress.bytes_total > 0)
                        .then(|| progress.bytes_done * 100 / progress.bytes_total)
                });
                Some((
                    running,
                    match percent {
                        Some(percent) => format!("downloading the embedding model ({percent}%)"),
                        None => "downloading the embedding model".to_owned(),
                    },
                ))
            }
        }
    }
}

fn target(s: &Shared) -> std::path::PathBuf {
    s.config.loaded.config.memory.model_dir(&s.config.data_dir)
}

async fn snapshot(s: &Shared) -> MemoryModelStatus {
    let target = target(s);
    let installed = openside_config::EMBEDDING_MODEL_FILES
        .into_iter()
        .all(|name| target.join(name).is_file());
    let loaded = s.memory.health().await.model_id;
    let inner = s.ops.model.inner.lock();
    let progress = inner.progress.clone();
    let source = inner.source.clone().unwrap_or_default();
    MemoryModelStatus {
        state: inner.state,
        installed,
        model_dir: target.display().to_string(),
        source: MemoryModelSource {
            endpoint: source.endpoint,
            repo: source.repo,
            revision: source.revision,
        },
        file: progress.as_ref().and_then(|progress| progress.file.clone()),
        files_done: progress.as_ref().map_or(0, |progress| progress.files_done),
        files_total: progress.as_ref().map_or(
            openside_config::EMBEDDING_MODEL_FILES.len() as u32,
            |progress| progress.files_total,
        ),
        bytes_done: progress.as_ref().map_or(0, |progress| progress.bytes_done),
        bytes_total: progress.as_ref().map_or(0, |progress| progress.bytes_total),
        error: inner.error.clone(),
        model_id: inner.installed.clone(),
        restart_required: inner
            .installed
            .as_ref()
            .is_some_and(|installed| loaded.as_ref() != Some(installed)),
        finished_at: inner.finished_at,
    }
}

pub(crate) async fn status(State(s): State<Shared>) -> Json<MemoryModelStatus> {
    Json(snapshot(&s).await)
}

/// A model was installed after this process loaded its memory store.
pub(crate) async fn restart_required(s: &Shared) -> bool {
    snapshot(s).await.restart_required
}

pub(crate) async fn download(
    State(s): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    body: Bytes,
) -> Result<(StatusCode, Json<MemoryModelStatus>), HttpError> {
    let request: StartMemoryModelRequest = if body.is_empty() {
        StartMemoryModelRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_request"))?
    };
    // The default model from the default source is harmless from any admin device; choosing
    // where to fetch from decides which destinations this computer contacts.
    if request.repo.is_some() || request.revision.is_some() || request.endpoint.is_some() {
        local.require()?;
    }
    let mut source = Source::default();
    if let Some(repo) = request.repo {
        source.repo = repo;
    }
    if let Some(revision) = request.revision {
        source.revision = revision;
    }
    if let Some(endpoint) = request.endpoint {
        source.endpoint = endpoint;
    }
    source
        .validate()
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_model_source"))?;
    let cancel = CancellationToken::new();
    {
        let mut inner = s.ops.model.inner.lock();
        if inner.running() {
            return Err(HttpError(StatusCode::CONFLICT, "download_in_progress"));
        }
        inner.state = MemoryModelState::Resolving;
        inner.source = Some(source.clone());
        inner.progress = None;
        inner.error = None;
        inner.finished_at = None;
        inner.cancel = Some(cancel.clone());
    }
    let shared = s.clone();
    tokio::spawn(run(shared, source, cancel));
    Ok((StatusCode::ACCEPTED, Json(snapshot(&s).await)))
}

async fn run(s: Shared, source: Source, cancel: CancellationToken) {
    let target = target(&s);
    let allow = s.config.loaded.config.network.allow.clone();
    let tracker = s.clone();
    let on_progress = move |progress: Progress| {
        let mut inner = tracker.ops.model.inner.lock();
        inner.state = state_of(progress.phase);
        inner.progress = Some(progress);
    };
    let mut shutdown = s.shutdown.subscribe();
    let result = tokio::select! {
        result = openside_model_fetch::fetch(&source, &allow, &target, &on_progress, &cancel) => result,
        _ = shutdown.recv() => Err(Failure::Cancelled),
    };
    let audit = {
        let mut inner = s.ops.model.inner.lock();
        inner.cancel = None;
        inner.finished_at = Some(Utc::now());
        match &result {
            Ok(installed) => {
                inner.state = MemoryModelState::Installed;
                inner.error = None;
                inner.installed = Some(installed.model_id.clone());
            }
            Err(Failure::Cancelled) => {
                inner.state = MemoryModelState::Cancelled;
                inner.error = None;
            }
            Err(failure) => {
                inner.state = MemoryModelState::Failed;
                inner.error = Some(ApiError {
                    code: failure.code().to_owned(),
                    message: failure.to_string(),
                });
            }
        }
        match &result {
            Ok(installed) => (
                "installed",
                json!({"model_id": installed.model_id, "revision": installed.revision}),
            ),
            Err(failure) => ("not_installed", json!({"code": failure.code()})),
        }
    };
    let mut fields = audit.1;
    if let Some(fields) = fields.as_object_mut() {
        fields.insert("repo".into(), json!(source.repo));
        fields.insert("endpoint".into(), json!(source.endpoint));
    }
    crate::pairing::audit(&s, "memory_model", audit.0, fields).await;
}

pub(crate) async fn cancel(State(s): State<Shared>) -> Json<MemoryModelStatus> {
    if let Some(token) = s.ops.model.inner.lock().cancel.clone() {
        token.cancel();
    }
    Json(snapshot(&s).await)
}
