//! `GET`/`PATCH /v1/settings/cli`: the browser profile new CLI and MCP sessions on this
//! computer use, kept as the `profile` key of `<config dir>/cli.toml`.
//!
//! The CLI owns that file and reads it on every run, so the daemon edits only `profile` in
//! place and keeps every other key and comment. A file neither side can parse is reported and
//! never overwritten. PATCH is in `api::LOCAL_REQUIRED`: only this computer changes it.

use std::path::{Path, PathBuf};

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_proto::{CliDefaults, CliProfile, UpdateCliDefaultsRequest};
use toml_edit::DocumentMut;

use crate::{api::HttpError, engine::Shared};

/// Serializes the daemon's read-modify-write of the file.
static WRITE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

const KEY: &str = "profile";

fn invalid() -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "cli_settings_invalid")
}

fn path(s: &Shared) -> Result<PathBuf, HttpError> {
    s.config
        .loaded
        .paths
        .cli_file()
        .ok_or_else(HttpError::internal)
}

/// The file as an editable document; an empty one when it does not exist.
fn load(path: &Path) -> Result<DocumentMut, HttpError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err(HttpError::internal()),
    };
    let document: DocumentMut = text.parse().map_err(|_| invalid())?;
    profile(&document)?;
    Ok(document)
}

fn profile(document: &DocumentMut) -> Result<Option<CliProfile>, HttpError> {
    document
        .get(KEY)
        .map(|item| {
            item.as_str()
                .and_then(CliProfile::from_id)
                .ok_or_else(invalid)
        })
        .transpose()
}

fn public(document: &DocumentMut) -> Result<CliDefaults, HttpError> {
    let saved = profile(document)?;
    Ok(CliDefaults {
        profile: saved.unwrap_or(CliProfile::Shared),
        profile_is_default: saved.is_none(),
    })
}

fn store(path: &Path, document: &DocumentMut) -> Result<(), HttpError> {
    use std::io::Write;
    let dir = path.parent().ok_or_else(HttpError::internal)?;
    std::fs::create_dir_all(dir).map_err(|_| HttpError::internal())?;
    // Rename over the old file, as the CLI does, so a reader never sees half of it.
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|_| HttpError::internal())?;
    temp.write_all(document.to_string().as_bytes())
        .map_err(|_| HttpError::internal())?;
    temp.persist(path).map_err(|_| HttpError::internal())?;
    Ok(())
}

fn update(path: &Path, profile: Option<CliProfile>) -> Result<CliDefaults, HttpError> {
    let _write = WRITE.lock();
    let mut document = load(path)?;
    match profile {
        Some(profile) => {
            let mut value = toml_edit::Value::from(profile.id());
            // Replace the value in place so the key, the comment line above it and the person's
            // inline comment (`profile = "a"  # mine`) all stay.
            match document
                .get_mut(KEY)
                .and_then(toml_edit::Item::as_value_mut)
            {
                Some(old) => {
                    *value.decor_mut() = old.decor().clone();
                    *old = value;
                }
                None => document[KEY] = toml_edit::Item::Value(value),
            }
        }
        None => {
            document.remove(KEY);
        }
    }
    store(path, &document)?;
    public(&document)
}

pub(crate) async fn get(State(s): State<Shared>) -> Result<Json<CliDefaults>, HttpError> {
    let path = path(&s)?;
    tokio::task::spawn_blocking(move || public(&load(&path)?))
        .await
        .map_err(|_| HttpError::internal())?
        .map(Json)
}

pub(crate) async fn patch(
    State(s): State<Shared>,
    Extension(caller): Extension<crate::api::Caller>,
    Json(request): Json<UpdateCliDefaultsRequest>,
) -> Result<Json<CliDefaults>, HttpError> {
    let path = path(&s)?;
    // A blocking task finishes even when the connection drops, so the file is never left
    // between read and rename.
    let defaults = tokio::task::spawn_blocking(move || update(&path, request.profile))
        .await
        .map_err(|_| HttpError::internal())??;
    crate::pairing::audit(
        &s,
        "cli_defaults",
        "written",
        serde_json::json!({"device_id": caller.0, "profile": request.profile.map(CliProfile::id)}),
    )
    .await;
    Ok(Json(defaults))
}
