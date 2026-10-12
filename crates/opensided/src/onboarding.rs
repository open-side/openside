//! First-run guide (F-5): the persisted "seen it" flag and the external-agent endpoints the
//! guide's skill step calls. Everything here works on fixed content and known locations:
//! the skill text is bundled, and the directory comes from a preset or `[skills.agents]`,
//! never from the request. See `docs/specs/onboarding.md`.

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
    OnboardingState, OnboardingStatus, UpdateOnboardingRequest,
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
    agent_skills_dir(agent, &s.config.loaded.config.skills.agents, &|key| {
        std::env::var_os(key)
    })
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
    for id in s.config.loaded.config.skills.agents.keys() {
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
    ExternalAgent {
        id,
        label,
        detected,
        skill_installed,
        skills_dir: dir.as_deref().map(home_relative),
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

#[cfg(test)]
mod tests {
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
