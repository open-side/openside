//! First-run guide and external-agent skills folders. Folder edits require local authority;
//! custom destinations are validated again immediately before installing bundled skills.

use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use openside_config::data_layout::DataPath;
use openside_proto::{
    ExternalAgent, ExternalAgentList, InstallAgentSkillRequest, InstalledAgentSkill,
    OnboardingState, OnboardingStatus, SetExternalAgentRequest, UpdateOnboardingRequest,
};
use openside_skills::{agent_skills_dir, bundled, install::write_skill, Error, BROWSER_SKILL};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

/// The agents openside knows without configuration; labels are the products' own names.
const PRESETS: [(&str, &str); 3] = [
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("omp", "omp"),
];

#[derive(Serialize, Deserialize)]
struct Stored {
    state: OnboardingState,
    updated_at: DateTime<Utc>,
}

fn path(s: &Shared) -> PathBuf {
    DataPath::Onboarding.path(&s.config.data_dir)
}

/// A missing or unreadable file means the guide was never finished: showing it once more is
/// harmless, hiding it forever on a corrupt file is not.
fn read(path: &Path) -> OnboardingStatus {
    let stored = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok());
    match stored {
        Some(stored) => OnboardingStatus {
            state: stored.state,
            updated_at: Some(stored.updated_at),
        },
        None => OnboardingStatus {
            state: OnboardingState::Pending,
            updated_at: None,
        },
    }
}

pub(crate) async fn get(State(s): State<Shared>) -> Result<Json<OnboardingStatus>, HttpError> {
    let path = path(&s);
    tokio::task::spawn_blocking(move || read(&path))
        .await
        .map(Json)
        .map_err(|_| HttpError::internal())
}

pub(crate) async fn put(
    State(s): State<Shared>,
    Json(request): Json<UpdateOnboardingRequest>,
) -> Result<Json<OnboardingStatus>, HttpError> {
    if request.state == OnboardingState::Pending {
        return Err(HttpError::invalid_value());
    }
    let stored = Stored {
        state: request.state,
        updated_at: Utc::now(),
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| HttpError::internal())?;
    let path = path(&s);
    tokio::task::spawn_blocking(move || crate::persistence::write_private(&path, &bytes))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|_| HttpError::internal())?;
    Ok(Json(OnboardingStatus {
        state: stored.state,
        updated_at: Some(stored.updated_at),
    }))
}

fn skills_dir(s: &Shared, agent: &str) -> Option<PathBuf> {
    agent_skills_dir(agent, &s.ops.agents.read(), &|key| std::env::var_os(key))
}

/// Shown to people, so a path under the daemon user's home reads `~/.claude/skills`.
fn home_relative(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    relative_to(path, home.as_deref())
}

fn relative_to(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.to_string_lossy()),
        None => path.to_string_lossy().into_owned(),
    }
}

fn on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        let exe = dir.join(format!("{name}.exe"));
        [candidate, exe].iter().any(|path| is_executable(path))
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Presets first, then `[skills.agents]` entries by name.
fn known_agents(s: &Shared) -> Vec<(String, String)> {
    let mut agents: Vec<(String, String)> = PRESETS
        .iter()
        .map(|(id, label)| ((*id).to_owned(), (*label).to_owned()))
        .collect();
    for id in s.ops.agents.read().keys() {
        if !agents.iter().any(|(known, _)| known == id) {
            agents.push((id.clone(), id.clone()));
        }
    }
    agents
}

fn describe(s: &Shared, id: String, label: String) -> ExternalAgent {
    let dir = skills_dir(s, &id);
    let detected = on_path(&id)
        || dir
            .as_deref()
            .and_then(Path::parent)
            .is_some_and(Path::is_dir);
    let skill_installed = dir
        .as_deref()
        .is_some_and(|dir| dir.join(BROWSER_SKILL).join("SKILL.md").is_file());
    let custom = !PRESETS.iter().any(|(preset, _)| *preset == id);
    ExternalAgent {
        id,
        label,
        detected,
        skill_installed,
        skills_dir: dir.as_deref().map(home_relative),
        custom,
    }
}

pub(crate) async fn agents(State(s): State<Shared>) -> Result<Json<ExternalAgentList>, HttpError> {
    tokio::task::spawn_blocking(move || ExternalAgentList {
        agents: known_agents(&s)
            .into_iter()
            .map(|(id, label)| describe(&s, id, label))
            .collect(),
    })
    .await
    .map(Json)
    .map_err(|_| HttpError::internal())
}

pub(crate) async fn install_skill(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<InstallAgentSkillRequest>,
) -> Result<(StatusCode, Json<InstalledAgentSkill>), HttpError> {
    if !known_agents(&s).iter().any(|(known, _)| *known == id) {
        return Err(HttpError(StatusCode::NOT_FOUND, "unknown_agent"));
    }
    let Some(dir) = skills_dir(&s, &id) else {
        return Err(HttpError(StatusCode::BAD_REQUEST, "skills_dir_unknown"));
    };
    let dir = if PRESETS.iter().any(|(preset, _)| *preset == id) {
        dir
    } else {
        validate_path(&dir.to_string_lossy(), &roots())?
    };
    let files = bundled(BROWSER_SKILL).ok_or_else(HttpError::internal)?;
    let force = request.force;
    let installed =
        tokio::task::spawn_blocking(move || write_skill(&dir, BROWSER_SKILL, files, force))
            .await
            .map_err(|_| HttpError::internal())?
            .map_err(|error| match error {
                Error::Exists(_) => HttpError(StatusCode::CONFLICT, "exists"),
                other => {
                    crate::diag!(
                        "opensided: installing the browser skill for {id} failed: {other}"
                    );
                    HttpError::internal()
                }
            })?;
    crate::pairing::audit(
        &s,
        "external_agent",
        "skill_installed",
        json!({"device_id": caller.0, "agent": id, "forced": force}),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(InstalledAgentSkill {
            agent: id,
            skill: installed.name,
            path: installed.path.to_string_lossy().into_owned(),
            files: u32::try_from(installed.files).unwrap_or(u32::MAX),
        }),
    ))
}

fn roots() -> Vec<PathBuf> {
    ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect()
}

fn validate_id(id: &str) -> Result<(), HttpError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || PRESETS.iter().any(|(preset, _)| *preset == id)
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_agent"));
    }
    Ok(())
}

fn canonical_destination(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        // A dangling link is not a missing directory that may safely be created.
        if std::fs::symlink_metadata(existing).is_ok() {
            return None;
        }
        suffix.push(existing.file_name()?);
        existing = existing.parent()?;
    }
    let mut canonical = existing.canonicalize().ok()?;
    if !canonical.is_dir() {
        return None;
    }
    for part in suffix.into_iter().rev() {
        canonical.push(part);
    }
    Some(canonical)
}

fn validate_path(raw: &str, roots: &[PathBuf]) -> Result<PathBuf, HttpError> {
    let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_skills_dir");
    if raw.is_empty()
        || raw.chars().any(char::is_control)
        || Path::new(raw)
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(invalid());
    }
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").ok_or_else(invalid)?).join(rest)
    } else {
        PathBuf::from(raw)
    };
    if !path.is_absolute() {
        return Err(invalid());
    }
    let canonical = canonical_destination(&path).ok_or_else(invalid)?;
    if !roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical.starts_with(root))
    {
        return Err(invalid());
    }
    Ok(canonical)
}

pub(crate) async fn set_agent(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<SetExternalAgentRequest>,
) -> Result<Json<ExternalAgent>, HttpError> {
    local.require()?;
    validate_id(&id)?;
    let dir = validate_path(&request.skills_dir, &roots())?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        let mut config = crate::settings_api::stored(&s).await?;
        config.skills.agents.insert(id.clone(), dir.clone());
        crate::settings_api::persist(
            &s,
            serde_json::to_value(config).map_err(|_| HttpError::internal())?,
        )
        .await?;
        s.ops.agents.write().insert(id.clone(), dir);
        crate::pairing::audit(&s, "external_agent", "folder_saved", json!({"agent": id})).await;
        Ok(Json(describe(&s, id.clone(), id)))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn delete_agent(
    State(s): State<Shared>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ExternalAgentList>, HttpError> {
    local.require()?;
    validate_id(&id)?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        let mut config = crate::settings_api::stored(&s).await?;
        if config.skills.agents.remove(&id).is_none() {
            return Err(HttpError(StatusCode::NOT_FOUND, "unknown_agent"));
        }
        crate::settings_api::persist(
            &s,
            serde_json::to_value(config).map_err(|_| HttpError::internal())?,
        )
        .await?;
        s.ops.agents.write().remove(&id);
        crate::pairing::audit(&s, "external_agent", "folder_removed", json!({"agent": id})).await;
        agents(State(s)).await
    })
    .await
    .map_err(|_| HttpError::internal())?
}

#[cfg(test)]
mod tests {
    #[test]
    fn custom_folder_paths_reject_traversal_and_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let allowed = vec![root.path().to_path_buf()];
        assert_eq!(
            validate_path(&root.path().join("new/skills").to_string_lossy(), &allowed)
                .unwrap_or_else(|_| panic!("valid folder rejected")),
            root.path().join("new/skills")
        );
        for path in [
            "relative".to_owned(),
            root.path().join("../escape").display().to_string(),
            outside.path().display().to_string(),
        ] {
            assert!(validate_path(&path, &allowed).is_err(), "{path}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
            assert!(validate_path(
                &root.path().join("escape/skills").to_string_lossy(),
                &allowed
            )
            .is_err());
            std::os::unix::fs::symlink(
                outside.path().join("missing"),
                root.path().join("dangling"),
            )
            .unwrap();
            assert!(validate_path(
                &root.path().join("dangling/skills").to_string_lossy(),
                &allowed
            )
            .is_err());
        }
        for id in ["", "../agent", "a/b", "Agent", "claude", "codex", "omp"] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_id("my-agent2").is_ok());
    }

    use super::*;

    #[test]
    fn paths_under_home_are_shown_with_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(
            relative_to(Path::new("/home/me/.claude/skills"), Some(home)),
            "~/.claude/skills"
        );
        assert_eq!(relative_to(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(
            relative_to(Path::new("/home/mei/x"), Some(home)),
            "/home/mei/x"
        );
        assert_eq!(relative_to(Path::new("/opt/skills"), None), "/opt/skills");
    }
}
