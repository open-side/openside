//! Daemon side of local memory: store lifecycle, purge job, health and the
//! `/v1/memory` API. The store itself (`openside-memory`) never touches the network;
//! embedding weights are only read from `memory.embedding.model_dir`.

pub(crate) mod consolidation;
pub(crate) mod context;

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use openside_config::{MemoryRetentionSetting, EMBEDDING_MODEL_FILES};
use openside_memory::{Embedder, MemoryStore, NewMemory, Options, StaticEmbedder};
use openside_proto::{
    CreateMemoryRequest, Memory, MemoryHealth, MemoryHistory, MemoryKind, MemorySearchMode,
    MemorySearchResponse, Retention, UpdateMemoryRequest,
};
use openside_vault::Redactor;
use parking_lot::RwLock;
use serde::Deserialize;

use crate::{api::HttpError, config::Resolved, engine::Shared, persistence::Result};

const PURGE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const API_ACTOR: &str = "api";

pub(crate) struct MemoryService {
    store: Arc<MemoryStore>,
    problem: Option<String>,
    history: bool,
    /// Writers share it; a sync run holds it exclusively so no memory changes between the
    /// run's snapshot and its applies (docs/specs/sync.md).
    pub(crate) writers: Arc<tokio::sync::RwLock<()>>,
}

impl MemoryService {
    /// Open `<data_dir>/memory.db`; a missing or unusable model means lexical-only search.
    pub(crate) async fn open(
        config: &Resolved,
        redactors: Arc<RwLock<Vec<Redactor>>>,
    ) -> Result<Self> {
        let memory = config.loaded.config.memory.clone();
        let model_dir = memory.model_dir(&config.data_dir);
        let path = openside_config::data_layout::DataPath::Memory.path(&config.data_dir);
        let default_retention = match memory.default_retention {
            MemoryRetentionSetting::Permanent => Retention::Permanent,
            MemoryRetentionSetting::Days90 => Retention::Days90,
            MemoryRetentionSetting::Days30 => Retention::Days30,
        };
        tokio::task::spawn_blocking(move || -> Result<Self> {
            let (embedder, problem) = load_embedder(&model_dir);
            // Unlocked vault values are known secrets; a memory containing one is rejected.
            let known_secret: openside_memory::KnownSecret = Arc::new(move |text: &str| {
                redactors
                    .read()
                    .iter()
                    .any(|redactor| redactor.redact(text) != text)
            });
            let store = MemoryStore::open(
                &path,
                Options {
                    embedder,
                    clock: Arc::new(openside_memory::SystemClock),
                    default_retention,
                    known_secret: Some(known_secret),
                },
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(Self {
                store: Arc::new(store),
                problem,
                history: memory.history,
                writers: Arc::default(),
            })
        })
        .await?
    }

    /// Run a blocking store operation off the async runtime.
    pub(crate) async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&MemoryStore) -> openside_memory::Result<T> + Send + 'static,
    ) -> openside_memory::Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .unwrap_or_else(|_| Err(openside_memory::Error::Storage("memory task failed".into())))
    }

    /// [`Self::run`] for an operation that changes memories (including the expiry purge that
    /// `create`, `update`, `delete`, `history` and `record_visit` start with); waits while a
    /// sync run is in progress.
    pub(crate) async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&MemoryStore) -> openside_memory::Result<T> + Send + 'static,
    ) -> openside_memory::Result<T> {
        // The guard moves into the blocking task: if the caller is cancelled while the worker
        // still runs, a sync run must keep waiting until the worker has finished writing.
        let writer = Arc::clone(&self.writers).read_owned().await;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let result = f(&store);
            drop(writer);
            result
        })
        .await
        .unwrap_or_else(|_| Err(openside_memory::Error::Storage("memory task failed".into())))
    }

    pub(crate) async fn health(&self) -> MemoryHealth {
        let mode = self.store.mode();
        MemoryHealth {
            mode,
            model_id: self.store.model_id(),
            problem: self.problem.clone(),
            next_step: (mode == MemorySearchMode::LexicalOnly).then(|| {
                "Run `openside memory model fetch <hugging-face-repo>` or set memory.embedding.model_dir, then restart opensided.".into()
            }),
            count: self.run(|store| store.count()).await.unwrap_or_default(),
        }
    }

    /// Whether `memory.history` allows browsing-history indexing at all; per-session
    /// privacy comes from the durable `incognito` flag in session state.
    pub(crate) fn history_enabled(&self) -> bool {
        self.history
    }
}

fn load_embedder(dir: &std::path::Path) -> (Option<Arc<dyn Embedder>>, Option<String>) {
    let missing: Vec<&str> = EMBEDDING_MODEL_FILES
        .into_iter()
        .filter(|name| !dir.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        return (
            None,
            Some(format!(
                "no local embedding model: {} missing in {}",
                missing.join(", "),
                dir.display()
            )),
        );
    }
    match StaticEmbedder::load(dir) {
        Ok(embedder) => (Some(Arc::new(embedder)), None),
        Err(error) => {
            crate::diag!("opensided: memory search is lexical-only: {error}");
            (None, Some(error.to_string()))
        }
    }
}

/// Purge expired memories at startup and hourly until shutdown.
pub(crate) fn start(shared: &Shared) {
    let shared = shared.clone();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PURGE_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = tick.tick() => {
                    let _ = shared.memory.write(|store| store.purge_expired()).await;
                }
            }
        }
    });
}

fn error(err: openside_memory::Error) -> HttpError {
    let status = match err {
        openside_memory::Error::NotFound => StatusCode::NOT_FOUND,
        openside_memory::Error::SecretDetected | openside_memory::Error::Invalid(_) => {
            StatusCode::BAD_REQUEST
        }
        openside_memory::Error::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    HttpError(status, err.code())
}

#[derive(Deserialize)]
pub(crate) struct SearchQuery {
    q: Option<String>,
    kind: Option<MemoryKind>,
    limit: Option<usize>,
    owner: Option<String>,
}

pub(crate) async fn search(
    State(s): State<Shared>,
    Query(query): Query<SearchQuery>,
) -> std::result::Result<Json<MemorySearchResponse>, HttpError> {
    let text = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(20);
    let response = s
        .memory
        .run(move |store| {
            store.search_scoped(
                &text,
                query.kind,
                limit,
                query
                    .owner
                    .as_deref()
                    .map(|owner| (owner != "shared").then_some(owner)),
                false,
            )
        })
        .await
        .map_err(error)?;
    Ok(Json(redacted(&s, response)?))
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Json(request): Json<CreateMemoryRequest>,
) -> std::result::Result<(StatusCode, Json<Memory>), HttpError> {
    let memory = s
        .memory
        .write(move |store| {
            store.create(
                NewMemory {
                    tags: request.tags,
                    retention: request.retention,
                    ..NewMemory::note(request.content)
                },
                API_ACTOR,
            )
        })
        .await
        .map_err(error)?;
    Ok((StatusCode::CREATED, Json(memory)))
}

pub(crate) async fn get(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> std::result::Result<Json<Memory>, HttpError> {
    let memory = s
        .memory
        .run(move |store| store.get(&id))
        .await
        .map_err(error)?;
    Ok(Json(redacted(&s, memory)?))
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(request): Json<UpdateMemoryRequest>,
) -> std::result::Result<Json<Memory>, HttpError> {
    let memory = s
        .memory
        .write(move |store| store.update(&id, request, API_ACTOR))
        .await
        .map_err(error)?;
    Ok(Json(memory))
}

pub(crate) async fn delete(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> std::result::Result<Json<Memory>, HttpError> {
    let memory = s
        .memory
        .write(move |store| store.delete(&id, API_ACTOR))
        .await
        .map_err(error)?;
    Ok(Json(redacted(&s, memory)?))
}

pub(crate) async fn history(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> std::result::Result<Json<MemoryHistory>, HttpError> {
    let history = s
        .memory
        .write(move |store| store.history(&id))
        .await
        .map_err(error)?;
    Ok(Json(redacted(&s, history)?))
}

pub(crate) async fn health(State(s): State<Shared>) -> Json<MemoryHealth> {
    Json(s.memory.health().await)
}

/// Vault values unlocked after a memory was written are still masked on read.
fn redacted<T: serde::Serialize + serde::de::DeserializeOwned>(
    s: &Shared,
    value: T,
) -> std::result::Result<T, HttpError> {
    let value = serde_json::to_value(value).map_err(|_| HttpError::internal())?;
    let value = s
        .registry
        .redact(value)
        .map_err(|_| HttpError::internal())?;
    serde_json::from_value(value).map_err(|_| HttpError::internal())
}
