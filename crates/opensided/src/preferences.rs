//! `GET`/`PATCH /v1/settings/preferences`: the person's own Settings choices
//! (`[preferences]` in config.toml).
//!
//! Unlike most configuration these apply the moment they are saved, so the running copy lives
//! here and is replaced after every write, including a whole-document `PUT /v1/settings/config`.

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_config::{
    ConfirmationSetting, FollowUpSetting, NotificationKindSetting, PreferencesConfig,
};
use openside_core::MessageAuthorKind;
use openside_proto::{
    ChatConfirmation, DeviceKind, FollowUpMode, NotificationKind, Preferences,
    UpdatePreferencesRequest,
};
use parking_lot::RwLock;

use crate::{api::HttpError, engine::Shared};

pub(crate) struct Live(RwLock<PreferencesConfig>);

impl Live {
    pub(crate) fn new(config: &PreferencesConfig) -> Self {
        Self(RwLock::new(config.clone()))
    }

    pub(crate) fn get(&self) -> PreferencesConfig {
        self.0.read().clone()
    }

    pub(crate) fn replace(&self, config: PreferencesConfig) {
        *self.0.write() = config;
    }

    pub(crate) fn notifies(&self, kind: NotificationKindSetting) -> bool {
        self.0.read().notifications.contains(&kind)
    }

    /// Confirmation for a new chat that does not choose one.
    pub(crate) fn confirmation(&self) -> ChatConfirmation {
        confirmation(self.0.read().confirmation)
    }
}

fn confirmation(setting: ConfirmationSetting) -> ChatConfirmation {
    match setting {
        ConfirmationSetting::AskFirst => ChatConfirmation::AskFirst,
        ConfirmationSetting::Skip => ChatConfirmation::Skip,
    }
}

fn notification(setting: NotificationKindSetting) -> NotificationKind {
    match setting {
        NotificationKindSetting::NeedsAttention => NotificationKind::NeedsAttention,
        NotificationKindSetting::TaskCompleted => NotificationKind::TaskCompleted,
        NotificationKindSetting::RoutineCompleted => NotificationKind::RoutineCompleted,
    }
}

fn notification_setting(kind: NotificationKind) -> NotificationKindSetting {
    match kind {
        NotificationKind::NeedsAttention => NotificationKindSetting::NeedsAttention,
        NotificationKind::TaskCompleted => NotificationKindSetting::TaskCompleted,
        NotificationKind::RoutineCompleted => NotificationKindSetting::RoutineCompleted,
    }
}

pub(crate) fn public(config: &PreferencesConfig) -> Preferences {
    Preferences {
        name: config.name.clone(),
        notifications: config
            .notifications
            .iter()
            .copied()
            .map(notification)
            .collect(),
        completion_sound: config.completion_sound,
        follow_up: match config.follow_up {
            FollowUpSetting::Queue => FollowUpMode::Queue,
            FollowUpSetting::Steer => FollowUpMode::Steer,
        },
        confirmation: confirmation(config.confirmation),
        agent_tab_cleanup_minutes: config.agent_tab_cleanup_minutes,
        save_sessions: config.save_sessions,
        mcp_server: config.mcp_server,
        computer_use: config.computer_use,
    }
}

/// Apply a request to `config`, refusing a value the file parser would also refuse.
pub(crate) fn apply(
    config: &mut PreferencesConfig,
    request: UpdatePreferencesRequest,
) -> Result<(), HttpError> {
    if let Some(name) = request.name {
        let name = name.trim();
        config.name = (!name.is_empty()).then(|| name.to_owned());
    }
    if let Some(kinds) = request.notifications {
        config.notifications = kinds.into_iter().map(notification_setting).collect();
    }
    if let Some(sound) = request.completion_sound {
        config.completion_sound = sound;
    }
    if let Some(follow_up) = request.follow_up {
        config.follow_up = match follow_up {
            FollowUpMode::Queue => FollowUpSetting::Queue,
            FollowUpMode::Steer => FollowUpSetting::Steer,
        };
    }
    if let Some(confirmation) = request.confirmation {
        config.confirmation = match confirmation {
            ChatConfirmation::AskFirst => ConfirmationSetting::AskFirst,
            ChatConfirmation::Skip => ConfirmationSetting::Skip,
        };
    }
    if let Some(minutes) = request.agent_tab_cleanup_minutes {
        config.agent_tab_cleanup_minutes = minutes;
    }
    if let Some(save) = request.save_sessions {
        config.save_sessions = save;
    }
    if let Some(enabled) = request.mcp_server {
        config.mcp_server = enabled;
    }
    if let Some(enabled) = request.computer_use {
        config.computer_use = enabled;
    }
    config
        .validate()
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_preferences"))
}

pub(crate) async fn get(State(s): State<Shared>) -> Json<Preferences> {
    Json(public(&s.preferences.get()))
}

pub(crate) async fn patch(
    State(s): State<Shared>,
    Extension(caller): Extension<crate::api::Caller>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Extension(author): Extension<MessageAuthorKind>,
    Json(request): Json<UpdatePreferencesRequest>,
) -> Result<Json<Preferences>, HttpError> {
    // The file rewrite and the live swap must not be split by a dropped connection.
    tokio::spawn(patch_owned(s, caller, local, author, request))
        .await
        .map_err(|_| HttpError::internal())?
}

async fn patch_owned(
    s: Shared,
    caller: crate::api::Caller,
    local: crate::remote::LocalAuthority,
    author: MessageAuthorKind,
    request: UpdatePreferencesRequest,
) -> Result<Json<Preferences>, HttpError> {
    if request.is_empty() {
        return Err(HttpError::invalid_value());
    }
    let _write = crate::settings_api::WRITE.lock().await;
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
    // Turning the MCP server on lets any local MCP client drive the browser, and computer use
    // lets agents ask for control of desktop apps; both stay decisions made on this computer,
    // like every other listener.
    let local_only: Vec<&str> = [
        (request.mcp_server.is_some(), "preferences.mcp_server"),
        (request.computer_use.is_some(), "preferences.computer_use"),
    ]
    .into_iter()
    .filter_map(|(present, key)| present.then_some(key))
    .collect();
    if !local_only.is_empty() && !local.0 {
        crate::pairing::audit(
            &s,
            "preferences",
            "refused_remote",
            serde_json::json!({"device_id": caller.0, "keys": local_only}),
        )
        .await;
        return Err(HttpError(StatusCode::FORBIDDEN, "local_access_required"));
    }
    // Local authority is not enough: an MCP client on the socket, or an agent client holding
    // the local secret, could otherwise grant itself computer use.
    if !local_only.is_empty() && crate::takeover::person_only(&caller, author).is_err() {
        crate::pairing::audit(
            &s,
            "preferences",
            "refused_agent",
            serde_json::json!({"device_id": caller.0, "keys": local_only}),
        )
        .await;
        return Err(HttpError(StatusCode::FORBIDDEN, "person_access_required"));
    }
    let mut config = crate::settings_api::stored(&s).await?;
    apply(&mut config.preferences, request)?;
    let document = serde_json::to_value(&config).map_err(|_| HttpError::internal())?;
    crate::settings_api::persist(&s, document).await?;
    {
        // See `computer_use::Service::gate`: a connect never lands between these two steps.
        let _gate = s.computer.gate.lock().await;
        s.preferences.replace(config.preferences.clone());
        if !config.preferences.computer_use {
            crate::computer_use::stop_all(&s, "disabled").await;
        }
    }
    crate::pairing::audit(
        &s,
        "preferences",
        "written",
        serde_json::json!({"device_id": caller.0, "local": local.0}),
    )
    .await;
    Ok(Json(public(&config.preferences)))
}
