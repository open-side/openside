//! First-run guide and external-agent skills folders. Folder edits require local authority;
//! every destination, preset or custom, is checked again immediately before installing
//! bundled skills (see [`checked_destination`]).

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use openside_config::data_layout::DataPath;
use openside_core::MessageAuthorKind;
use openside_proto::{
    ExternalAgent, ExternalAgentList, InstallAgentSkillRequest, InstalledAgentSkill,
    OnboardingAlreadySet, OnboardingState, OnboardingStatus, SetExternalAgentRequest,
    UpdateOnboardingRequest,
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

/// Serializes the read-check-write of the flag so two devices finishing the guide at once can
/// never both record an outcome.
static WRITE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn path(s: &Shared) -> PathBuf {
    DataPath::Onboarding.path(&s.config.data_dir)
}

/// A missing or unreadable file means the guide was never finished: showing it once more is
/// harmless, hiding it forever on a corrupt file is not.
fn load(path: &Path) -> Option<Stored> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
}

fn read(path: &Path) -> OnboardingStatus {
    match load(path) {
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

/// Compare-and-set: writes only while the stored state is `pending`; otherwise 409
/// `onboarding_already_set` with the outcome already recorded, which stays untouched.
pub(crate) async fn put(
    State(s): State<Shared>,
    Json(request): Json<UpdateOnboardingRequest>,
) -> Response {
    if request.state == OnboardingState::Pending {
        return HttpError::invalid_value().into_response();
    }
    let path = path(&s);
    let written = tokio::task::spawn_blocking(move || {
        let _write = WRITE.lock();
        if let Some(current) = load(&path).filter(|s| s.state != OnboardingState::Pending) {
            return Ok(Err(current));
        }
        let stored = Stored {
            state: request.state,
            updated_at: Utc::now(),
        };
        let bytes = serde_json::to_vec(&stored).map_err(|_| HttpError::internal())?;
        crate::persistence::write_private(&path, &bytes).map_err(|_| HttpError::internal())?;
        Ok::<_, HttpError>(Ok(stored))
    })
    .await;
    match written {
        Ok(Ok(Ok(stored))) => Json(OnboardingStatus {
            state: stored.state,
            updated_at: Some(stored.updated_at),
        })
        .into_response(),
        Ok(Ok(Err(current))) => (
            StatusCode::CONFLICT,
            Json(OnboardingAlreadySet {
                code: "onboarding_already_set".into(),
                message: "onboarding already set".into(),
                state: current.state,
                updated_at: current.updated_at,
            }),
        )
            .into_response(),
        Ok(Err(error)) => error.into_response(),
        Err(_) => HttpError::internal().into_response(),
    }
}

/// The environment the skills-folder rules read (HOME, the agents' own variables). Each test
/// daemon gets its own instead of the process environment, which parallel tests share.
#[cfg(test)]
static TEST_ENV: parking_lot::Mutex<std::collections::BTreeMap<PathBuf, Vec<(String, OsString)>>> =
    parking_lot::Mutex::new(std::collections::BTreeMap::new());

/// Make the daemon whose data folder is `data_dir` see only `vars` as its environment.
#[cfg(test)]
pub(crate) fn set_test_env(data_dir: &Path, vars: &[(&str, &Path)]) {
    TEST_ENV.lock().insert(
        data_dir.to_path_buf(),
        vars.iter()
            .map(|(key, value)| ((*key).to_owned(), value.as_os_str().to_owned()))
            .collect(),
    );
}

#[cfg(test)]
fn env_var(s: &Shared, key: &str) -> Option<OsString> {
    match TEST_ENV.lock().get(&s.config.data_dir) {
        Some(vars) => vars
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone()),
        None => std::env::var_os(key),
    }
}

#[cfg(not(test))]
fn env_var(_: &Shared, key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

fn home(s: &Shared) -> Option<PathBuf> {
    env_var(s, "HOME")
        .or_else(|| env_var(s, "USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

fn skills_dir(s: &Shared, agent: &str) -> Option<PathBuf> {
    agent_skills_dir(agent, &s.ops.agents.read(), &|key| env_var(s, key))
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

/// `POST /v1/external-agents/{id}/browser-skill`. Local access comes from
/// `api::LOCAL_REQUIRED`; the agent then trusts what lands in that folder, so the request
/// must also come from the person, never from an MCP client or agent token.
pub(crate) async fn install_skill(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<InstallAgentSkillRequest>,
) -> Result<(StatusCode, Json<InstalledAgentSkill>), HttpError> {
    if crate::takeover::person_only(&caller, author).is_err() {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_access_required"));
    }
    if !known_agents(&s).iter().any(|(known, _)| *known == id) {
        return Err(HttpError(StatusCode::NOT_FOUND, "unknown_agent"));
    }
    let Some(dir) = skills_dir(&s, &id) else {
        return Err(HttpError(StatusCode::BAD_REQUEST, "skills_dir_unknown"));
    };
    let preset = PRESETS.iter().any(|(preset, _)| *preset == id);
    let home = home(&s);
    let roots = roots(&s);
    let data_dirs = [
        s.config.data_dir.clone(),
        s.config.loaded.paths.data_dir.clone(),
    ];
    let files = bundled(BROWSER_SKILL).ok_or_else(HttpError::internal)?;
    let force = request.force;
    // Checked on the same blocking thread right before writing, to keep the window in which
    // a folder could be swapped for a link as short as possible.
    let agent = id.clone();
    let installed = tokio::task::spawn_blocking(move || {
        let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_skills_dir");
        let home = home.ok_or_else(invalid)?;
        let dir = if preset {
            dir
        } else {
            validate_path(&dir.to_string_lossy(), &roots, Some(&home))?
        };
        let dir = checked_destination(&dir, &home, &data_dirs)?;
        write_skill(&dir, BROWSER_SKILL, files, force).map_err(|error| match error {
            Error::Exists(_) => HttpError(StatusCode::CONFLICT, "exists"),
            other => {
                crate::diag!("opensided: installing the browser skill for {agent} failed: {other}");
                HttpError::internal()
            }
        })
    })
    .await
    .map_err(|_| HttpError::internal())??;
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

/// A folder a bundled skill may be written to: inside the person's home, reached without
/// following a symbolic link below it, and outside openside's data folder. Presets need this
/// as much as custom folders: their location comes from variables such as
/// `CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `OMP_CODING_AGENT_DIR` that nothing else checks.
/// Returns the canonical folder.
fn checked_destination(
    dir: &Path,
    home: &Path,
    data_dirs: &[PathBuf],
) -> Result<PathBuf, HttpError> {
    use std::path::Component;
    let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_skills_dir");
    if !dir.is_absolute()
        || dir
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid());
    }
    let canonical_home = home.canonicalize().map_err(|_| invalid())?;
    // A custom folder arrives canonical already, so it may be spelled through the canonical
    // home even when HOME itself goes through a link (`/home` -> `/var/home`).
    let (base, below) = match dir.strip_prefix(home) {
        Ok(below) => (home, below),
        Err(_) => (
            canonical_home.as_path(),
            dir.strip_prefix(&canonical_home).map_err(|_| invalid())?,
        ),
    };
    if below.as_os_str().is_empty() {
        return Err(invalid());
    }
    let mut current = base.to_path_buf();
    for part in below.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err(invalid()),
            Ok(_) => {}
            // Everything from here down is created by the install.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(invalid()),
        }
    }
    let canonical = canonical_destination(dir).ok_or_else(invalid)?;
    if !canonical.starts_with(&canonical_home) {
        return Err(invalid());
    }
    let inside_data = data_dirs.iter().any(|data| {
        dir.starts_with(data)
            || canonical.starts_with(data)
            || data
                .canonicalize()
                .is_ok_and(|data| canonical.starts_with(data))
    });
    if inside_data {
        return Err(invalid());
    }
    Ok(canonical)
}

fn roots(s: &Shared) -> Vec<PathBuf> {
    ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME"]
        .into_iter()
        .filter_map(|key| env_var(s, key))
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

fn validate_path(raw: &str, roots: &[PathBuf], home: Option<&Path>) -> Result<PathBuf, HttpError> {
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
        home.ok_or_else(invalid)?.join(rest)
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
    let dir = validate_path(&request.skills_dir, &roots(&s), home(&s).as_deref())?;
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
            validate_path(
                &root.path().join("new/skills").to_string_lossy(),
                &allowed,
                None
            )
            .unwrap_or_else(|_| panic!("valid folder rejected")),
            root.path().join("new/skills")
        );
        for path in [
            "relative".to_owned(),
            root.path().join("../escape").display().to_string(),
            outside.path().display().to_string(),
        ] {
            assert!(validate_path(&path, &allowed, None).is_err(), "{path}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
            assert!(validate_path(
                &root.path().join("escape/skills").to_string_lossy(),
                &allowed,
                None
            )
            .is_err());
            std::os::unix::fs::symlink(
                outside.path().join("missing"),
                root.path().join("dangling"),
            )
            .unwrap();
            assert!(validate_path(
                &root.path().join("dangling/skills").to_string_lossy(),
                &allowed,
                None
            )
            .is_err());
        }
        for id in ["", "../agent", "a/b", "Agent", "claude", "codex", "omp"] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_id("my-agent2").is_ok());
    }

    #[test]
    fn skill_destinations_stay_in_home_off_links_and_out_of_openside_data() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let data = home.join(".local/share/openside");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let data_dirs = [data.clone()];
        let check = |dir: &Path| checked_destination(dir, &home, &data_dirs);
        assert_eq!(
            check(&home.join(".claude/skills")).unwrap(),
            home.join(".claude/skills")
        );
        // Missing folders below an existing one are fine: the install creates them.
        assert!(check(&home.join("new/deeper/skills")).is_ok());
        for refused in [
            outside.path().join("skills"),
            home.clone(),
            home.join("../escape/skills"),
            PathBuf::from("relative/skills"),
            data.join("skills"),
            data.clone(),
        ] {
            assert!(check(&refused).is_err(), "{}", refused.display());
        }
        #[cfg(unix)]
        {
            // A link anywhere below home, even to a folder that is itself inside home.
            std::os::unix::fs::symlink(outside.path(), home.join("away")).unwrap();
            std::os::unix::fs::symlink(home.join(".claude"), home.join("alias")).unwrap();
            std::os::unix::fs::symlink(&data, home.join("data-link")).unwrap();
            for refused in ["away/skills", "alias/skills", "data-link/skills", "alias"] {
                assert!(check(&home.join(refused)).is_err(), "{refused}");
            }
            // A file in the way is not a folder.
            std::fs::write(home.join("file"), "x").unwrap();
            assert!(check(&home.join("file/skills")).is_err());
        }
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
