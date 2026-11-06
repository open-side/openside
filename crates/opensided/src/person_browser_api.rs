//! HTTP routes over the person's own browser (see [`crate::person_browser`]). Only the person
//! uses them: agent clients get `person_device_required`, and agents reach the person's tabs
//! only through the policy-gated `browser_user_tab*` tools.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use openside_core::{MessageAuthorKind, SessionId};
use openside_proto::{
    AttachTabRequest, AttachedBy, AttachedTab, AttachedTabList, ClearBrowsingDataRequest,
    CreatePersonBookmarkRequest, PersonBookmark, PersonBookmarkTree, PersonDownload,
    PersonDownloadList, PersonExtension, PersonExtensionList, PersonHistoryItem, PersonHistoryList,
    PersonRecentlyClosed, PersonRecentlyClosedList, PersonTabCommand, PersonTabCommandResult,
    PersonTabList,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::{Caller, HttpError};
use crate::engine::Shared;
use crate::person_browser::{bounded, BridgeError};

const MAX_IDS: usize = 500;
const MAX_TITLE: usize = 1000;
const MAX_GROUP_TITLE: usize = 200;
const MAX_QUERY: usize = 200;
const MAX_ID: usize = 128;
/// Browser pages the person may open from openside's menus; web pages cannot open these.
const BROWSER_PAGES: &[&str] = &[
    "chrome://bookmarks",
    "chrome://downloads",
    "chrome://extensions",
    "chrome://history",
    "chrome://settings",
    "chrome://inspect",
    "chrome://settings/clearBrowserData",
    "chrome://settings/appearance",
    "chrome://settings/adPrivacy",
    "chrome://settings/content",
    "chrome://settings/security",
    "chrome://settings/languages",
    "chrome://extensions/shortcuts",
    "chrome://settings/performance",
    "chrome://profile-picker",
];

type Result<T, E = HttpError> = std::result::Result<T, E>;

fn person(caller: &Caller, author: MessageAuthorKind) -> Result<()> {
    crate::takeover::person_only(caller, author)
}

/// A read when no extension is connected answers with the empty "not connected" shape.
fn offline<T>(outcome: std::result::Result<T, BridgeError>, empty: T) -> Result<T> {
    match outcome {
        Err(BridgeError::NotConnected) => Ok(empty),
        other => other.map_err(HttpError::from),
    }
}

/// An http(s) URL without embedded credentials.
fn web_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
    }) && value.len() <= 8 * 1024
}

fn validate(command: &PersonTabCommand) -> Result<()> {
    let ids = |ids: &[i64]| (1..=MAX_IDS).contains(&ids.len());
    let valid = match command {
        PersonTabCommand::Create { url: Some(url), .. } => {
            web_url(url) || BROWSER_PAGES.contains(&url.trim_end_matches('/'))
        }
        PersonTabCommand::Close { tab_ids } | PersonTabCommand::Ungroup { tab_ids } => ids(tab_ids),
        PersonTabCommand::Group { tab_ids, title, .. } => {
            ids(tab_ids)
                && title
                    .as_ref()
                    .map_or(true, |title| title.chars().count() <= MAX_GROUP_TITLE)
        }
        _ => true,
    };
    valid.then_some(()).ok_or_else(HttpError::invalid_value)
}

fn command_name(command: &PersonTabCommand) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|value| value["command"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Re-check and bound a tab the extension reported back, exactly like the tab list: a tab from
/// an incognito window never leaves the daemon.
fn command_result(wire: WireCommandResult) -> PersonTabCommandResult {
    PersonTabCommandResult {
        tab: wire
            .tab
            .filter(|tab| !tab.incognito)
            .map(|tab| crate::person_browser::sanitize_tab(tab.tab)),
        group_id: wire.group_id,
    }
}

#[derive(Deserialize)]
struct WireCommandResult {
    #[serde(default)]
    tab: Option<WireTab>,
    #[serde(default)]
    group_id: Option<i64>,
}

#[derive(Deserialize)]
struct WireTab {
    #[serde(flatten)]
    tab: openside_proto::PersonTab,
    #[serde(default)]
    incognito: bool,
}

pub(crate) async fn tabs(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
) -> Result<Json<PersonTabList>> {
    person(&caller, author)?;
    let empty = PersonTabList {
        connected: false,
        focused_window_id: None,
        tabs: Vec::new(),
        groups: Vec::new(),
    };
    offline(s.registry.person_browser.tabs().await, empty).map(Json)
}

pub(crate) async fn command(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Json(command): Json<PersonTabCommand>,
) -> Result<Json<PersonTabCommandResult>> {
    person(&caller, author)?;
    validate(&command)?;
    let name = command_name(&command);
    let params = serde_json::to_value(&command).map_err(|_| HttpError::internal())?;
    let wire: WireCommandResult = s
        .registry
        .person_browser
        .request("tabs.command", params)
        .await?;
    crate::pairing::audit(
        &s,
        "person_browser.tabs",
        "allow",
        json!({"device": caller.0, "command": name}),
    )
    .await;
    Ok(Json(command_result(wire)))
}

#[derive(Deserialize)]
pub(crate) struct LimitQuery {
    limit: Option<u32>,
}

pub(crate) async fn recently_closed(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<PersonRecentlyClosedList>> {
    person(&caller, author)?;
    let limit = query.limit.unwrap_or(10).clamp(1, 25);
    #[derive(Deserialize)]
    struct Wire {
        entries: Vec<PersonRecentlyClosed>,
    }
    let wire = s
        .registry
        .person_browser
        .request::<Wire>("sessions.recently_closed", json!({"limit": limit}))
        .await
        .map(|wire| PersonRecentlyClosedList {
            connected: true,
            entries: wire
                .entries
                .into_iter()
                .take(limit as usize)
                .map(|mut entry| {
                    entry.title = bounded(entry.title, MAX_TITLE);
                    entry
                })
                .collect(),
        });
    offline(
        wire,
        PersonRecentlyClosedList {
            connected: false,
            entries: Vec::new(),
        },
    )
    .map(Json)
}

pub(crate) async fn restore(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(session_id): Path<String>,
) -> Result<Json<PersonTabCommandResult>> {
    person(&caller, author)?;
    if session_id.is_empty() || session_id.len() > MAX_ID {
        return Err(HttpError::invalid_value());
    }
    let wire: WireCommandResult = s
        .registry
        .person_browser
        .request("sessions.restore", json!({"session_id": session_id}))
        .await?;
    crate::pairing::audit(
        &s,
        "person_browser.tabs",
        "allow",
        json!({"device": caller.0, "command": "restore"}),
    )
    .await;
    Ok(Json(command_result(wire)))
}

#[derive(Deserialize)]
pub(crate) struct HistoryQuery {
    q: Option<String>,
    limit: Option<u32>,
}

pub(crate) async fn history(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<PersonHistoryList>> {
    person(&caller, author)?;
    let text = query.q.unwrap_or_default();
    if text.chars().count() > MAX_QUERY {
        return Err(HttpError::invalid_value());
    }
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    #[derive(Deserialize)]
    struct Wire {
        items: Vec<PersonHistoryItem>,
    }
    let wire = s
        .registry
        .person_browser
        .request::<Wire>("history.search", json!({"query": text, "limit": limit}))
        .await
        .map(|wire| PersonHistoryList {
            connected: true,
            items: wire
                .items
                .into_iter()
                .take(limit as usize)
                .map(|mut item| {
                    item.title = bounded(item.title, MAX_TITLE);
                    item
                })
                .collect(),
        });
    offline(
        wire,
        PersonHistoryList {
            connected: false,
            items: Vec::new(),
        },
    )
    .map(Json)
}

pub(crate) async fn bookmarks(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
) -> Result<Json<PersonBookmarkTree>> {
    person(&caller, author)?;
    #[derive(Deserialize)]
    struct Wire {
        roots: Vec<PersonBookmark>,
    }
    let wire = s
        .registry
        .person_browser
        .request::<Wire>("bookmarks.tree", json!({}))
        .await
        .map(|wire| PersonBookmarkTree {
            connected: true,
            roots: wire.roots,
        });
    offline(
        wire,
        PersonBookmarkTree {
            connected: false,
            roots: Vec::new(),
        },
    )
    .map(Json)
}

pub(crate) async fn create_bookmark(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Json(request): Json<CreatePersonBookmarkRequest>,
) -> Result<(StatusCode, Json<PersonBookmark>)> {
    person(&caller, author)?;
    let valid = web_url(&request.url)
        && request.title.chars().count() <= MAX_TITLE
        && request
            .parent_id
            .as_ref()
            .map_or(true, |parent| !parent.is_empty() && parent.len() <= MAX_ID);
    if !valid {
        return Err(HttpError::invalid_value());
    }
    let bookmark: PersonBookmark = s
        .registry
        .person_browser
        .request("bookmarks.create", json!(request))
        .await?;
    crate::pairing::audit(
        &s,
        "person_browser.bookmarks",
        "allow",
        json!({"device": caller.0, "command": "create"}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(bookmark)))
}

pub(crate) async fn delete_bookmark(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    person(&caller, author)?;
    if id.is_empty() || id.len() > MAX_ID {
        return Err(HttpError::invalid_value());
    }
    s.registry
        .person_browser
        .call("bookmarks.remove", json!({"id": id}))
        .await?;
    crate::pairing::audit(
        &s,
        "person_browser.bookmarks",
        "allow",
        json!({"device": caller.0, "command": "remove"}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Clear data from the person's own browser. Only the person reaches this route; the audit
/// records which kinds were cleared, never the data.
pub(crate) async fn clear_data(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Json(request): Json<ClearBrowsingDataRequest>,
) -> Result<StatusCode> {
    person(&caller, author)?;
    let distinct: std::collections::HashSet<_> = request.types.iter().collect();
    if request.types.is_empty() || distinct.len() != request.types.len() || request.since_ms < 0 {
        return Err(HttpError::invalid_value());
    }
    s.registry
        .person_browser
        .call("browsing_data.clear", json!(request))
        .await?;
    crate::pairing::audit(
        &s,
        "person_browser.clear_data",
        "allow",
        json!({"device": caller.0, "types": request.types, "since_ms": request.since_ms}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn downloads(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<PersonDownloadList>> {
    person(&caller, author)?;
    let limit = query.limit.unwrap_or(10).clamp(1, 50);
    #[derive(Deserialize)]
    struct WireDownload {
        #[serde(flatten)]
        download: PersonDownload,
        #[serde(default)]
        incognito: bool,
    }
    #[derive(Deserialize)]
    struct Wire {
        downloads: Vec<WireDownload>,
    }
    let wire = s
        .registry
        .person_browser
        .request::<Wire>("downloads.list", json!({"limit": limit}))
        .await
        .map(|wire| PersonDownloadList {
            connected: true,
            downloads: wire
                .downloads
                .into_iter()
                .filter(|download| !download.incognito)
                .take(limit as usize)
                .map(|download| {
                    let mut download = download.download;
                    // Folders reveal the person's file layout; only the base name is shown.
                    let name = download
                        .filename
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    download.filename = bounded(name, MAX_TITLE);
                    download
                })
                .collect(),
        });
    offline(
        wire,
        PersonDownloadList {
            connected: false,
            downloads: Vec::new(),
        },
    )
    .map(Json)
}

pub(crate) async fn extensions(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
) -> Result<Json<PersonExtensionList>> {
    person(&caller, author)?;
    #[derive(Deserialize)]
    struct Wire {
        extensions: Vec<PersonExtension>,
    }
    let wire = s
        .registry
        .person_browser
        .request::<Wire>("extensions.list", json!({}))
        .await
        .map(|wire| PersonExtensionList {
            connected: true,
            extensions: wire.extensions,
        });
    offline(
        wire,
        PersonExtensionList {
            connected: false,
            extensions: Vec::new(),
        },
    )
    .map(Json)
}

async fn existing_session(s: &Shared, id: String) -> Result<SessionId> {
    let id = SessionId::new(id);
    let state = s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
    if state.deleted {
        return Err(HttpError::not_found());
    }
    Ok(id)
}

pub(crate) async fn attached(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
) -> Result<Json<AttachedTabList>> {
    person(&caller, author)?;
    let session = existing_session(&s, id).await?;
    Ok(Json(AttachedTabList {
        tabs: s.registry.person_browser.attached(&session),
    }))
}

pub(crate) async fn attach(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
    Json(request): Json<AttachTabRequest>,
) -> Result<(StatusCode, Json<AttachedTab>)> {
    person(&caller, author)?;
    let session = existing_session(&s, id).await?;
    let hub = &s.registry.person_browser;
    let tab = hub.find_tab(request.tab_id).await?;
    if !crate::person_browser::web_page(&s, &tab.url) {
        return Err(HttpError::invalid_value());
    }
    let attached = hub.attach(&session, &tab, AttachedBy::Person);
    crate::pairing::audit(
        &s,
        "person_browser.attach",
        "allow",
        json!({"session": session, "device": caller.0, "tab_id": tab.tab_id, "by": "person"}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(attached)))
}

pub(crate) async fn detach(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path((id, tab_id)): Path<(String, i64)>,
) -> Result<StatusCode> {
    person(&caller, author)?;
    let session = existing_session(&s, id).await?;
    if !s.registry.person_browser.detach(&session, tab_id) {
        return Err(HttpError(StatusCode::NOT_FOUND, "tab_not_found"));
    }
    crate::pairing::audit(
        &s,
        "person_browser.detach",
        "allow",
        json!({"session": session, "device": caller.0, "tab_id": tab_id}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
