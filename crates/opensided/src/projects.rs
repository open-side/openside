//! Projects: a named folder with instructions that chats belong to (`docs/specs/projects.md`).
//! A chat in a project works in the project's folder (its workspace, so the Guard root and
//! the file tools' working folder) and its agent receives the project's instructions.
//!
//! Projects live in `projects.json`. Every change, and every check that a chat's project
//! still exists before it joins, runs under [`Service::gate`], so deleting a project cannot
//! miss a chat that was joining it at the same moment.

use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as UrlPath, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::Utc;
use openside_config::data_layout::DataPath;
use openside_proto::{
    normalize_title, CreateProjectRequest, EventKind, Project, ProjectList, Session, SessionId,
    SetSessionFolderRequest, UpdateProjectRequest, MAX_PROJECT_INSTRUCTIONS_BYTES,
    MAX_PROJECT_NAME_CHARS,
};

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    remote::LocalAuthority,
};

/// A project as `projects.json` keeps it: the API's [`Project`] plus what only the daemon uses.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Stored {
    #[serde(flatten)]
    project: Project,
    /// Identity of the folder `root` named when the project was made or its folder was last
    /// confirmed (`folder_picker::recorded_identity`). Absent in records written before it
    /// was kept; such a folder fails closed until a local person confirms it
    /// ([`Project::needs_confirmation`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_identity: Option<[u64; 2]>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredList {
    projects: Vec<Stored>,
}

pub(crate) struct Service {
    path: PathBuf,
    list: parking_lot::RwLock<Vec<Stored>>,
    /// Serialises project changes with chats joining a project.
    pub(crate) gate: tokio::sync::Mutex<()>,
}

impl Service {
    /// A missing file means no projects yet. An unreadable one also starts empty: refusing
    /// to start the daemon over it would cost the person every chat, not just the projects.
    /// A record the API could not have written is left out, not repaired, and the file is
    /// not rewritten here: a folder that is only unavailable for now returns on a later start,
    /// unless a project change rewrites the file first.
    pub(crate) fn open(data_dir: &Path) -> Self {
        let path = DataPath::Projects.path(data_dir);
        let list = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<StoredList>(&bytes) {
                Ok(list) => {
                    let homes = crate::folder_picker::home_dirs();
                    list.projects
                        .into_iter()
                        .filter(|stored| {
                            let ok = loadable(&stored.project, &homes, data_dir);
                            if !ok {
                                crate::diag!(
                                    "openside: projects.json: leaving out project {:?}: invalid name, instructions or folder",
                                    stored.project.id
                                );
                            }
                            ok
                        })
                        .map(|mut stored| {
                            stored.project.needs_confirmation =
                                unconfirmed(stored.root_identity);
                            stored
                        })
                        .collect()
                }
                Err(_) => {
                    crate::diag!(
                        "openside: projects.json is unreadable; starting without projects"
                    );
                    Vec::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => {
                crate::diag!(
                    "openside: projects.json could not be read; starting without projects"
                );
                Vec::new()
            }
        };
        Self {
            path,
            list: parking_lot::RwLock::new(sorted(list)),
            gate: Default::default(),
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<Project> {
        self.stored(id).map(|stored| stored.project)
    }

    fn stored(&self, id: &str) -> Option<Stored> {
        self.list
            .read()
            .iter()
            .find(|stored| stored.project.id == id)
            .cloned()
    }

    fn all(&self) -> Vec<Project> {
        self.list
            .read()
            .iter()
            .map(|stored| stored.project.clone())
            .collect()
    }

    /// Project `id`, if its folder is still the one it was made with. A folder renamed away
    /// with another made at its path, or swapped for a symlink, would otherwise hand the
    /// project's chats a folder nobody approved.
    pub(crate) async fn intact(&self, id: &str) -> Result<Project, HttpError> {
        let stored = self.stored(id).ok_or_else(not_found)?;
        let (root, recorded) = (stored.project.root.clone(), stored.root_identity);
        let same =
            tokio::task::spawn_blocking(move || crate::folder_picker::still_same(&root, recorded))
                .await
                .map_err(|_| HttpError::internal())?;
        if !same {
            return Err(root_changed());
        }
        Ok(stored.project)
    }

    /// Persist `projects` and make them current. The caller holds [`Self::gate`].
    async fn replace(&self, projects: Vec<Stored>) -> Result<(), HttpError> {
        let projects = sorted(projects);
        let bytes = serde_json::to_vec_pretty(&StoredList {
            projects: projects.clone(),
        })
        .map_err(|_| HttpError::internal())?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || crate::persistence::write_private(&path, &bytes))
            .await
            .map_err(|_| HttpError::internal())?
            .map_err(|error| HttpError::write_failed(&error))?;
        *self.list.write() = projects;
        Ok(())
    }
}

fn sorted(mut projects: Vec<Stored>) -> Vec<Stored> {
    projects.sort_by(|a, b| {
        a.project
            .name
            .to_lowercase()
            .cmp(&b.project.name.to_lowercase())
            .then_with(|| a.project.id.cmp(&b.project.id))
    });
    projects
}

/// The project's folder is no longer the folder the project was made with.
pub(crate) fn root_changed() -> HttpError {
    HttpError(StatusCode::CONFLICT, "project_root_changed")
}

pub(crate) fn not_found() -> HttpError {
    HttpError(StatusCode::NOT_FOUND, "project_not_found")
}

fn invalid_root() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_project_root")
}

/// Whether a folder without a recorded identity must be confirmed before use: on platforms
/// with folder identities `folder_picker::still_same` refuses it.
fn unconfirmed(root_identity: Option<[u64; 2]>) -> bool {
    root_identity.is_none() && cfg!(any(unix, windows))
}

/// The only folder a scratch project `id` may have: `<data_dir>/projects/<id>`. `None` for an
/// id that is not a single plain name, which could point elsewhere.
fn scratch_folder(id: &str, data_dir: &Path) -> Option<PathBuf> {
    let mut parts = Path::new(id).components();
    if !matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) {
        return None;
    }
    let folders = DataPath::ProjectFolders.path(data_dir);
    let folders = std::fs::canonicalize(&folders).unwrap_or(folders);
    Some(folders.join(id))
}

fn valid_name(raw: &str) -> Result<String, HttpError> {
    normalize_title(raw)
        .filter(|name| name.chars().count() <= MAX_PROJECT_NAME_CHARS)
        .ok_or(HttpError(StatusCode::BAD_REQUEST, "invalid_project_name"))
}

fn valid_instructions(raw: String) -> Result<String, HttpError> {
    if raw.len() > MAX_PROJECT_INSTRUCTIONS_BYTES {
        return Err(HttpError(StatusCode::BAD_REQUEST, "instructions_too_large"));
    }
    Ok(raw)
}

/// Whether a record read from `projects.json` is one the API could have written: the file is
/// the person's to edit, and its instructions become a system prompt part.
fn loadable(project: &Project, homes: &[PathBuf], data_dir: &Path) -> bool {
    if valid_name(&project.name).ok().as_deref() != Some(project.name.as_str())
        || project.instructions.len() > MAX_PROJECT_INSTRUCTIONS_BYTES
        || !Path::new(&project.root).is_absolute()
    {
        return false;
    }
    if project.scratch {
        // Only the daemon's own folder for this id skips the folder check.
        return scratch_folder(&project.id, data_dir)
            .is_some_and(|folder| Path::new(&project.root) == folder);
    }
    crate::folder_picker::validate_root(&project.root, homes, data_dir)
        .is_ok_and(|root| root == Path::new(&project.root))
}

/// The canonical folder a person named, as `folder_picker::validate_root` accepts it.
async fn checked_root(s: &Shared, raw: String) -> Result<String, HttpError> {
    let data_dir = s.config.data_dir.clone();
    let root = tokio::task::spawn_blocking(move || {
        crate::folder_picker::validate_root(&raw, &crate::folder_picker::home_dirs(), &data_dir)
    })
    .await
    .map_err(|_| HttpError::internal())??;
    root.into_os_string()
        .into_string()
        .map_err(|_| invalid_root())
}

/// The identity of `project`'s folder, once the folder passes the check it passed when the
/// project was made (`validate_root`, or the daemon's own path for a scratch project) and
/// still has that canonical path.
async fn confirmed_identity(s: &Shared, project: &Project) -> Result<[u64; 2], HttpError> {
    let data_dir = s.config.data_dir.clone();
    let (id, root, scratch) = (project.id.clone(), project.root.clone(), project.scratch);
    tokio::task::spawn_blocking(move || {
        let canonical = if scratch {
            let canonical = std::fs::canonicalize(&root).map_err(|_| invalid_root())?;
            if scratch_folder(&id, &data_dir).as_ref() != Some(&canonical) || !canonical.is_dir() {
                return Err(invalid_root());
            }
            canonical
        } else {
            crate::folder_picker::validate_root(
                &root,
                &crate::folder_picker::home_dirs(),
                &data_dir,
            )?
        };
        if canonical != Path::new(&root) {
            return Err(invalid_root());
        }
        crate::folder_picker::recorded_identity(&canonical).ok_or_else(invalid_root)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

/// An empty private folder for a project started from scratch. It is the daemon's own, so it
/// skips the check that keeps person-named folders out of openside's data.
fn make_scratch(folder: &Path) -> std::io::Result<PathBuf> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(folder)?;
    // The Guard compares canonical paths; a symlinked data folder must not split them.
    std::fs::canonicalize(folder)
}

/// The system prompt part a chat in `project` receives; empty instructions add none.
pub(crate) fn prompt_part(project: &Project) -> String {
    // Records are checked when loaded; the cap is applied here too because this text reaches
    // the model directly.
    let mut end = project
        .instructions
        .len()
        .min(MAX_PROJECT_INSTRUCTIONS_BYTES);
    while !project.instructions.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "Project \"{}\" (folder {}). The person's instructions for every chat in this project:\n{}",
        project.name,
        project.root,
        &project.instructions[..end]
    )
}

pub(crate) async fn list(State(s): State<Shared>) -> Json<ProjectList> {
    Json(ProjectList {
        projects: s.projects.all(),
    })
}

pub(crate) async fn get(
    State(s): State<Shared>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Project>, HttpError> {
    s.projects.get(&id).map(Json).ok_or_else(not_found)
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Json(request): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<Project>), HttpError> {
    local.require()?;
    let name = valid_name(&request.name)?;
    let instructions = valid_instructions(request.instructions.unwrap_or_default())?;
    let named = match request.root {
        Some(raw) => Some(checked_root(&s, raw).await?),
        None => None,
    };
    let _gate = s.projects.gate.lock().await;
    let mut projects = s.projects.list.read().clone();
    if let Some(root) = &named {
        let root = PathBuf::from(root);
        let existing: Vec<PathBuf> = projects
            .iter()
            .map(|stored| PathBuf::from(&stored.project.root))
            .collect();
        let taken = tokio::task::spawn_blocking(move || {
            existing
                .iter()
                .any(|other| crate::folder_picker::same_folder(other, &root))
        })
        .await
        .map_err(|_| HttpError::internal())?;
        if taken {
            return Err(HttpError(StatusCode::CONFLICT, "project_exists"));
        }
    }
    let id = crate::persistence::random_id();
    let scratch = named.is_none();
    let root = match named {
        Some(root) => root,
        None => {
            let folder = DataPath::ProjectFolders.path(&s.config.data_dir).join(&id);
            tokio::task::spawn_blocking(move || make_scratch(&folder))
                .await
                .map_err(|_| HttpError::internal())?
                .map_err(|error| HttpError::write_failed(&error))?
                .into_os_string()
                .into_string()
                .map_err(|_| HttpError::internal())?
        }
    };
    let identity_of = PathBuf::from(&root);
    let root_identity =
        tokio::task::spawn_blocking(move || crate::folder_picker::recorded_identity(&identity_of))
            .await
            .map_err(|_| HttpError::internal())?;
    // Without an identity a same-path replacement would go unnoticed, so on platforms that
    // have one the project is refused rather than stored unprotected.
    if unconfirmed(root_identity) {
        return Err(invalid_root());
    }
    let now = Utc::now();
    let project = Project {
        id,
        name,
        root,
        scratch,
        instructions,
        needs_confirmation: false,
        created_at: now,
        updated_at: now,
    };
    projects.push(Stored {
        project: project.clone(),
        root_identity,
    });
    s.projects.replace(projects).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

/// Person only: the instructions reach every chat in the project as a system prompt part.
/// Confirming the folder needs local access, like naming one ([`create`]).
pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<UpdateProjectRequest>,
) -> Result<Json<Project>, HttpError> {
    caller.require_person()?;
    if request == UpdateProjectRequest::default() {
        return Err(HttpError::invalid_value());
    }
    if request.confirm_root {
        local.require()?;
    }
    let name = request.name.as_deref().map(valid_name).transpose()?;
    let instructions = request.instructions.map(valid_instructions).transpose()?;
    let _gate = s.projects.gate.lock().await;
    let mut projects = s.projects.list.read().clone();
    let stored = projects
        .iter_mut()
        .find(|stored| stored.project.id == id)
        .ok_or_else(not_found)?;
    // The person at this computer editing a project whose folder was never recorded vouches
    // for it as much as by confirming; a folder that no longer passes stays unconfirmed.
    if request.confirm_root || (local.0 && unconfirmed(stored.root_identity)) {
        match confirmed_identity(&s, &stored.project).await {
            Ok(identity) => {
                stored.root_identity = Some(identity);
                stored.project.needs_confirmation = false;
            }
            Err(error) if request.confirm_root => return Err(error),
            Err(_) => {}
        }
    }
    let project = &mut stored.project;
    if name.is_some() || instructions.is_some() {
        project.updated_at = Utc::now();
    }
    if let Some(name) = name {
        project.name = name;
    }
    if let Some(instructions) = instructions {
        project.instructions = instructions;
    }
    let updated = project.clone();
    s.projects.replace(projects).await?;
    Ok(Json(updated))
}

/// The chats stay, outside any project; the folder and its files are never touched. Person
/// only, like [`update`].
pub(crate) async fn delete(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<ProjectList>, HttpError> {
    caller.require_person()?;
    let _gate = s.projects.gate.lock().await;
    let mut projects = s.projects.list.read().clone();
    let before = projects.len();
    projects.retain(|stored| stored.project.id != id);
    if projects.len() == before {
        return Err(not_found());
    }
    // Members leave before the project goes, all in one commit: a crash or a failed write of
    // projects.json afterwards leaves at worst an empty project, never chats that name a
    // project that is gone. Joining holds the gate, so no member is missed.
    let events =
        s.db.store(move |db| {
            let detach: Vec<(SessionId, EventKind)> = db
                .list_sessions()
                .into_iter()
                .filter(|session| session.project_id.as_deref() == Some(id.as_str()))
                .map(|session| {
                    (
                        session.id,
                        EventKind::SessionFolderChanged {
                            project_id: None,
                            workspace: None,
                        },
                    )
                })
                .collect();
            Ok(db.append_batch(detach)?)
        })
        .await?;
    for event in events {
        let _ = s.events.send(event);
    }
    s.projects.replace(projects).await?;
    Ok(Json(ProjectList {
        projects: s.projects.all(),
    }))
}

/// `PUT /v1/sessions/{id}/folder`: move a chat into a project, attach a folder of its own
/// (local access only: that folder becomes its Guard root), or drop both.
pub(crate) async fn set_folder(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<SetSessionFolderRequest>,
) -> Result<Json<Session>, HttpError> {
    let id = SessionId::new(id);
    let current = s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
    if request.project_id.is_some() && request.workspace.is_some() {
        return Err(HttpError::invalid_value());
    }
    // A project's folder was approved for people; an agent-client token cannot move a chat in.
    if request.project_id.is_some() {
        caller.require_person()?;
    }
    let attached = match request.workspace {
        Some(raw) => {
            local.require()?;
            Some(checked_root(&s, raw).await?)
        }
        None => None,
    };
    // An agent's home chat works where its agent works.
    if crate::agents::for_session(&s, &current)?
        .is_some_and(|agent| agent.home_session_id == current.id)
    {
        return Err(HttpError(StatusCode::CONFLICT, "agent_home"));
    }
    let _gate = s.projects.gate.lock().await;
    let (project_id, workspace) = match request.project_id {
        Some(project_id) => {
            let project = s.projects.intact(&project_id).await?;
            (Some(project.id), Some(project.root))
        }
        None => (None, attached),
    };
    // Decided against the chat's place at commit time, not the one read before the gate: a
    // project delete or another move may have changed it meanwhile.
    let changed = id.clone();
    let event =
        s.db.store(move |db| {
            let current = db.session(&changed).ok_or("unknown session")?;
            if current.project_id == project_id && current.workspace == workspace {
                return Ok(None);
            }
            Ok(Some(db.append(
                &changed,
                None,
                EventKind::SessionFolderChanged {
                    project_id,
                    workspace,
                },
            )?))
        })
        .await?;
    if let Some(event) = event {
        let _ = s.events.send(event);
    }
    Ok(Json(s.db.state(&id).await?.session()))
}
