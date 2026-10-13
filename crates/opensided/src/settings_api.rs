//! `GET`/`PUT /v1/settings/config` (PP-13): every configuration key, read and written as
//! the JSON form of `config.toml`.
//!
//! PUT validates the document with the same parser as the file (unknown keys, types,
//! ranges, cross-key rules) before an atomic 0600 rewrite. The opaque `credentials`
//! section is file-only: never returned, and kept from the file on write. A written
//! document applies on the next daemon start; only the live policy (`/v1/policy`) changes
//! a running daemon.

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_proto::{DeviceKind, SettingsDocument, UpdateSettingsRequest};
use serde_json::Value;

use crate::{api::HttpError, engine::Shared};

/// Sections the API neither returns nor writes.
pub(crate) const FILE_ONLY: &[&str] = &["credentials"];

pub(crate) static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn path(s: &Shared) -> std::path::PathBuf {
    s.config.loaded.paths.config_file.clone()
}

/// The stored document: the file when present, else defaults.
pub(crate) async fn stored(s: &Shared) -> Result<openside_config::Config, HttpError> {
    let path = path(s);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => openside_config::parse_document(&path, &text)
            .map_err(|_| HttpError(StatusCode::CONFLICT, "config_file_invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(openside_config::Config::default())
        }
        Err(_) => Err(HttpError::internal()),
    }
}

fn public(config: &openside_config::Config) -> Result<Value, HttpError> {
    let mut value = serde_json::to_value(config).map_err(|_| HttpError::internal())?;
    if let Some(object) = value.as_object_mut() {
        for section in FILE_ONLY {
            object.remove(*section);
        }
    }
    Ok(value)
}

/// The configuration the running daemon uses, including the parts that change live.
fn running(s: &Shared) -> Result<Value, HttpError> {
    let mut running = s.config.loaded.config.clone();
    running.remote = crate::remote::effective(s);
    running.models = s.models.models();
    running.channels = s.channels.config();
    running.skills.agents = s.ops.agents.read().clone();
    running.sync = crate::sync::effective(s);
    public(&running)
}

pub(crate) async fn restart_required(s: &Shared) -> Result<bool, HttpError> {
    Ok(public(&stored(s).await?)? != running(s)?)
}

async fn document(
    s: &Shared,
    caller: &crate::api::Caller,
    local: crate::remote::LocalAuthority,
) -> Result<SettingsDocument, HttpError> {
    let config = public(&stored(s).await?)?;
    Ok(SettingsDocument {
        restart_required: config != running(s)?,
        config,
        file_only: FILE_ONLY
            .iter()
            .map(|section| (*section).to_owned())
            .collect(),
        schema: openside_config::config_schema(),
        // An agent client without local authority cannot write at all, so it is told nothing
        // is editable.
        remote_editable: if local.0 || caller.1 == DeviceKind::PersonDevice {
            REMOTE_EDITABLE
                .iter()
                .map(|key| (*key).to_owned())
                .collect()
        } else {
            Vec::new()
        },
        remote: !local.0,
    })
}

pub(crate) async fn get(
    State(s): State<Shared>,
    Extension(caller): Extension<crate::api::Caller>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
) -> Result<Json<SettingsDocument>, HttpError> {
    Ok(Json(document(&s, &caller, local).await?))
}

/// TOML has no null: an unset optional key is simply absent.
pub(crate) fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|_, child| !child.is_null());
            object.values_mut().for_each(strip_nulls);
        }
        Value::Array(array) => array.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Caller holds WRITE through file replacement and any live application of the change.
pub(crate) async fn persist(s: &Shared, mut config: Value) -> Result<(), HttpError> {
    strip_nulls(&mut config);
    let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_config");
    let text = toml::Value::try_from(&config)
        .ok()
        .and_then(|value| toml::to_string(&value).ok())
        .ok_or_else(invalid)?;
    let path = path(s);
    openside_config::parse_document(&path, &text).map_err(|_| invalid())?;
    tokio::task::spawn_blocking(move || crate::persistence::write_private(&path, text.as_bytes()))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|error| HttpError::write_failed(&error))
}

pub(crate) async fn put(
    State(s): State<Shared>,
    Extension(caller): Extension<crate::api::Caller>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(request): Json<UpdateSettingsRequest>,
) -> Result<Json<SettingsDocument>, HttpError> {
    // Local callers also need cancellation-safe serialization through rename and audit.
    tokio::spawn(put_owned(s, caller, local, request))
        .await
        .map_err(|_| HttpError::internal())?
}

async fn put_owned(
    s: Shared,
    caller: crate::api::Caller,
    local: crate::remote::LocalAuthority,
    request: UpdateSettingsRequest,
) -> Result<Json<SettingsDocument>, HttpError> {
    let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_config");
    let mut config = request.config;
    let Some(object) = config.as_object_mut() else {
        return Err(invalid());
    };
    if FILE_ONLY
        .iter()
        .any(|section| object.contains_key(*section))
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "file_only_section"));
    }
    let _write = WRITE.lock().await;
    if caller
        .0
        .as_deref()
        .is_some_and(|id| s.remote.is_revoked(id))
    {
        return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    if !local.0 && caller.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
    let current = stored(&s).await?;
    // Without local authority (the Unix socket, or a browser opened from the launcher on this
    // computer) only reviewed, behaviour-only keys may change; anything that names an
    // executable, path, listener, trust list or policy needs it, so a leaked remote admin
    // token cannot turn into code execution after a restart.
    if !local.0 {
        let mut before = Vec::new();
        leaves(&public(&current)?, "", &mut before);
        let mut after = Vec::new();
        leaves(&Value::Object(object.clone()), "", &mut after);
        let mut restricted: Vec<String> = before
            .iter()
            .filter(|leaf| !after.contains(leaf))
            .chain(after.iter().filter(|leaf| !before.contains(leaf)))
            .map(|(path, _)| path.clone())
            .filter(|path| !remote_editable(path))
            .collect();
        restricted.sort();
        restricted.dedup();
        if !restricted.is_empty() {
            crate::pairing::audit(
                &s,
                "settings",
                "refused_remote",
                serde_json::json!({"device_id": caller.0, "keys": restricted}),
            )
            .await;
            return Err(HttpError(StatusCode::FORBIDDEN, "local_access_required"));
        }
    }
    for section in FILE_ONLY {
        let kept = serde_json::to_value(&current)
            .map_err(|_| HttpError::internal())?
            .get(*section)
            .cloned();
        if let Some(kept) = kept {
            object.insert((*section).to_owned(), kept);
        }
    }
    persist(&s, config).await?;
    crate::pairing::audit(
        &s,
        "settings",
        "written",
        serde_json::json!({"device_id": caller.0, "local": local.0}),
    )
    .await;
    Ok(Json(document(&s, &caller, local).await?))
}

/// Keys a paired person device without local authority may change: ticket lifetimes, model
/// choice, agent tuning, memory retention and sleep inhibition. Each is plain behaviour and
/// names no executable, path, listener, credential or trust list; everything else needs local
/// authority.
pub(crate) const REMOTE_EDITABLE: &[&str] = &[
    "tickets.",
    "models.default",
    "models.fallback",
    "models.front",
    "memory.default_retention",
    "memory.history",
    "agent.startup_timeout_ms",
    "agent.tool_retries",
    "agent.compaction_max_messages",
    "agent.compaction_keep_messages",
    "agent.orchestration",
    "service.inhibit_sleep",
];

pub(crate) fn remote_editable(path: &str) -> bool {
    REMOTE_EDITABLE.iter().any(|key| {
        if key.ends_with('.') {
            path.starts_with(key)
        } else {
            path == *key
        }
    })
}

fn leaves(value: &Value, prefix: &str, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, child) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                leaves(child, &path, out);
            }
        }
        // An absent optional key and a null are the same setting.
        Value::Null => {}
        other => out.push((prefix.to_owned(), other.clone())),
    }
}
