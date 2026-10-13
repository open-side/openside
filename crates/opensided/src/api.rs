use std::convert::Infallible;

use axum::{
    extract::{MatchedPath, Path, Query, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, patch, post},
    Extension, Json, Router,
};
use chrono::Utc;
use futures_util::Stream;
use openside_core::{MessageId, TicketId};
use openside_proto::*;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::{
    engine::{Action, Shared},
    persistence::{random_id, Error},
};

#[derive(Debug)]
pub(crate) struct HttpError(pub(crate) StatusCode, pub(crate) &'static str);
impl HttpError {
    pub(crate) fn internal() -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
    }
    pub(crate) fn not_found() -> Self {
        Self(StatusCode::NOT_FOUND, "not_found")
    }
    /// A failed file write: a full volume is the caller's cue to free space, anything else is ours.
    pub(crate) fn write_failed(error: &std::io::Error) -> Self {
        if storage_full(error) {
            Self(StatusCode::INSUFFICIENT_STORAGE, "storage_full")
        } else {
            Self::internal()
        }
    }
    fn conflict() -> Self {
        Self(StatusCode::CONFLICT, "invalid_transition")
    }
    pub(crate) fn invalid_value() -> Self {
        Self(StatusCode::BAD_REQUEST, "invalid_value")
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ApiError {
                code: self.1.into(),
                message: self.1.replace('_', " "),
            }),
        )
            .into_response()
    }
}
/// SQLite reports a full volume as `SQLITE_FULL`; plain file writes (artifacts, sidecar state
/// files) surface `ENOSPC` or a quota error. Either way retrying is pointless until space is
/// freed, so it must not read as a state conflict.
fn storage_full(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(rusqlite::Error::SqliteFailure(failure, _)) =
            error.downcast_ref::<rusqlite::Error>()
        {
            if failure.code == rusqlite::ErrorCode::DiskFull {
                return true;
            }
        }
        // `ErrorKind::StorageFull`/`QuotaExceeded` need a newer Rust than the workspace MSRV.
        #[cfg(unix)]
        if let Some(errno) = error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
        {
            if errno == rustix::io::Errno::NOSPC.raw_os_error()
                || errno == rustix::io::Errno::DQUOT.raw_os_error()
            {
                return true;
            }
        }
        current = error.source();
    }
    false
}
impl From<Error> for HttpError {
    fn from(error: Error) -> Self {
        if storage_full(error.as_ref()) {
            return Self(StatusCode::INSUFFICIENT_STORAGE, "storage_full");
        }
        Self::conflict()
    }
}
type Result<T, E = HttpError> = std::result::Result<T, E>;

#[derive(Clone)]
pub(crate) struct Access(pub(crate) Vec<CapabilityScope>);
impl Access {
    pub(crate) fn has(&self, scope: CapabilityScope) -> bool {
        self.0.iter().any(|held| held.grants(scope))
    }
    fn ticket(&self, ticket: &Ticket) -> Result<()> {
        let scope = match ticket.kind {
            TicketKind::Confirm | TicketKind::Info | TicketKind::Choice | TicketKind::Hire => {
                CapabilityScope::Approve
            }
            TicketKind::SecretInput
            | TicketKind::DeviceApproval
            | TicketKind::Takeover
            | TicketKind::Login => CapabilityScope::HumanStep,
        };
        if self.has(scope) {
            Ok(())
        } else {
            Err(HttpError(StatusCode::FORBIDDEN, "forbidden"))
        }
    }
}

/// Device id behind the request's token; `None` for the local Unix socket.
#[derive(Clone)]
pub(crate) struct Caller(pub(crate) Option<String>, pub(crate) DeviceKind);

impl Caller {
    pub(crate) fn id(&self) -> &str {
        self.0.as_deref().unwrap_or("local")
    }

    pub(crate) async fn record_driver(&self, shared: &Shared, session: &SessionId) -> Result<()> {
        if self.1 != DeviceKind::AgentClient {
            return Ok(());
        }
        // Serialize driver registration with ticket creation and approval commits.
        let _gate = shared.ticket_gate.lock().await;
        let session = session.clone();
        let device_id = self.id().to_owned();
        let event = shared
            .db
            .store(move |store| {
                let state = store.session(&session).ok_or("unknown session")?;
                if state.driver_ids.contains(&device_id) {
                    return Ok(None);
                }
                Ok(Some(store.append(
                    &session,
                    None,
                    EventKind::SessionDriverRecorded { device_id },
                )?))
            })
            .await?;
        if let Some(event) = event {
            let _ = shared.events.send(event);
        }
        Ok(())
    }
    async fn person_approval(&self, shared: &Shared, ticket: &Ticket) -> Result<()> {
        if self.1 != DeviceKind::PersonDevice {
            return Err(HttpError(StatusCode::FORBIDDEN, "person_approval_required"));
        }
        if ticket.origin.device_id.as_deref() == Some(self.id())
            || ticket.origin.driver_ids.iter().any(|id| id == self.id())
        {
            return Err(HttpError(StatusCode::FORBIDDEN, "self_approval_forbidden"));
        }
        let session = ticket.session_id.clone();
        let device = self.id().to_owned();
        if shared
            .db
            .store(move |store| {
                Ok(store
                    .session(&session)
                    .ok_or("unknown session")?
                    .driver_ids
                    .contains(&device))
            })
            .await?
        {
            return Err(HttpError(StatusCode::FORBIDDEN, "self_approval_forbidden"));
        }
        Ok(())
    }

    /// A paired device must not widen what the agent may do: choosing a mode looser than the
    /// daemon default or a workspace outside the default sandbox is for the local socket only.
    pub(crate) fn widens(
        &self,
        shared: &Shared,
        mode: Option<PolicyMode>,
        workspace: Option<&str>,
    ) -> bool {
        let rank = |mode: PolicyMode| match mode {
            PolicyMode::ReadOnly => 0,
            PolicyMode::Guard => 1,
            PolicyMode::Full => 2,
        };
        self.0.is_some()
            && (workspace.is_some()
                || mode.is_some_and(|mode| rank(mode) > rank(shared.policy.default_mode())))
    }
}

pub(crate) fn router(shared: Shared, unix: bool) -> Router {
    let ui = shared.config.loaded.config.ui.dir.clone();
    let api = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/accounts", get(crate::accounts::list))
        .route(
            "/v1/agents",
            get(crate::agents::api::list).post(crate::agents::api::create),
        )
        .route(
            "/v1/agents/{id}",
            get(crate::agents::api::get).patch(crate::agents::api::update),
        )
        .route("/v1/agents/{id}/retire", post(crate::agents::api::retire))
        .route("/v1/agents/{id}/restore", post(crate::agents::api::restore))
        .route(
            "/v1/agents/{id}/transfer",
            post(crate::agents::api::transfer),
        )
        .route(
            "/v1/agents/{id}/handoffs",
            get(crate::agents::api::handoffs).post(crate::agents::api::handoff),
        )
        .route(
            "/v1/team/proposals/suppressed",
            get(crate::agents::api::suppressed_proposals),
        )
        .route(
            "/v1/team/proposals/{key}",
            axum::routing::delete(crate::agents::api::clear_proposal),
        )
        .route("/v1/browser/tabs", get(crate::browser_tabs_api::list))
        .route("/v1/settings", get(crate::session_meta::settings))
        .route(
            "/v1/settings/models",
            get(crate::models_api::get).put(crate::models_api::put),
        )
        .route("/v1/settings/keys", get(crate::models_api::keys))
        .route(
            "/v1/settings/keys/{provider}",
            post(crate::models_api::create_key),
        )
        .route(
            "/v1/settings/keys/{provider}/{key_id}",
            axum::routing::put(crate::models_api::set_key)
                .patch(crate::models_api::update_key)
                .delete(crate::models_api::delete_key),
        )
        .route(
            "/v1/settings/subscriptions",
            get(crate::model_subscriptions_api::list),
        )
        .route(
            "/v1/settings/subscriptions/{source}",
            post(crate::model_subscriptions_api::create),
        )
        .route(
            "/v1/settings/subscriptions/{source}/{account_id}",
            get(crate::model_subscriptions_api::get)
                .patch(crate::model_subscriptions_api::update)
                .delete(crate::model_subscriptions_api::remove),
        )
        .route("/v1/settings/pools", get(crate::model_accounts::list_pools))
        .route(
            "/v1/settings/pools/{provider}",
            axum::routing::put(crate::model_accounts::update_pool),
        )
        .route(
            "/v1/sessions/{id}/account",
            axum::routing::put(crate::model_accounts::set_preference),
        )
        .route("/v1/settings/channels", get(crate::channels_api::get))
        .route(
            "/v1/settings/channels/{kind}/accounts",
            post(crate::channels_api::add),
        )
        .route(
            "/v1/settings/channels/{kind}/accounts/{account_id}",
            axum::routing::patch(crate::channels_api::patch).delete(crate::channels_api::delete),
        )
        .route(
            "/v1/settings/channels/{kind}/accounts/{account_id}/test",
            post(crate::channels_api::test),
        )
        .route(
            "/v1/hooks/{kind}/{account_id}",
            post(crate::channels_api::hook),
        )
        .route("/v1/sessions", get(crate::session_meta::list).post(create))
        .route(
            "/v1/sessions/{id}",
            get(session)
                .patch(crate::session_meta::update)
                .delete(crate::session_delete::delete),
        )
        .route(
            "/v1/sessions/{id}/messages",
            post(submit).layer(axum::extract::DefaultBodyLimit::max(
                crate::artifacts::SUBMIT_BODY_LIMIT,
            )),
        )
        .route("/v1/sessions/{id}/artifacts", get(crate::artifacts::list))
        .route("/v1/favicons/{origin}", get(crate::favicons::get))
        .route(
            "/v1/sessions/{id}/artifacts/{path}",
            get(crate::artifacts::raw),
        )
        .route(
            "/v1/sessions/{id}/artifacts/{path}/preview",
            get(crate::artifacts::preview),
        )
        .route("/v1/sessions/{id}/visuals", get(crate::artifacts::visuals))
        .route("/v1/sessions/{id}/queue/{mid}", patch(edit).delete(remove))
        .route("/v1/sessions/{id}/queue/{mid}/promote", post(promote))
        .route("/v1/sessions/{id}/queue/{mid}/move", post(reorder))
        .route("/v1/sessions/{id}/stop", post(stop))
        .route("/v1/sessions/{id}/prewarm", post(prewarm_session))
        .route("/v1/browser/prewarm", post(prewarm_browser))
        .route("/v1/sessions/{id}/events", get(events))
        .route("/v1/sessions/{id}/events/poll", get(poll_events))
        .route("/v1/sessions/{id}/repl", post(crate::tools::repl::route))
        .route("/v1/tools", get(crate::tools::api_call::catalog))
        .route(
            "/v1/sessions/{id}/tools/{name}",
            post(crate::tools::api_call::route),
        )
        .route(
            "/v1/sessions/{id}/subagents",
            get(crate::subagents::list_route),
        )
        .route(
            "/v1/sessions/{id}/subagents/{sid}/cancel",
            post(crate::subagents::cancel_route),
        )
        .route(
            "/v1/sessions/{id}/subagents/{sid}/steer",
            post(crate::subagents::steer_route),
        )
        .route("/v1/tickets", get(tickets))
        .route("/v1/tickets/{id}", get(ticket))
        .route("/v1/tickets/{id}/resolve", post(resolve))
        .route("/v1/tickets/{id}/done", post(done))
        .route("/v1/tickets/{id}/reject", post(reject))
        .route("/v1/tickets/{id}/takeover", get(crate::takeover::stream))
        .route(
            "/v1/sessions/{id}/browser/preview",
            get(crate::preview::stream),
        )
        .route(
            "/v1/sessions/{id}/browser/thumbnails/{thumbnail_id}",
            get(crate::preview::thumbnail),
        )
        .route("/v1/devices/pair", post(pair))
        .route("/v1/devices/{id}", delete(revoke))
        .route(
            "/v1/routines",
            get(crate::routines::list).post(crate::routines::create),
        )
        .route(
            "/v1/routines/suggestions",
            get(crate::routines::suggestions),
        )
        .route(
            "/v1/routines/{id}",
            get(crate::routines::get)
                .patch(crate::routines::patch)
                .delete(crate::routines::delete),
        )
        .route("/v1/routines/{id}/run", post(crate::routines::run))
        .route("/v1/routines/{id}/runs", get(crate::routines::runs))
        .route(
            "/v1/memory",
            get(crate::memory::search).post(crate::memory::create),
        )
        .route("/v1/memory/health", get(crate::memory::health))
        .route("/v1/memory/model", get(crate::memory_model::status))
        .route(
            "/v1/memory/model/download",
            post(crate::memory_model::download),
        )
        .route("/v1/memory/model/cancel", post(crate::memory_model::cancel))
        .route("/v1/daemon", get(crate::lifecycle::status))
        .route("/v1/daemon/restart", post(crate::lifecycle::restart))
        .route("/v1/doctor", get(crate::doctor_api::get))
        .route(
            "/v1/memory/{id}",
            get(crate::memory::get)
                .patch(crate::memory::update)
                .delete(crate::memory::delete),
        )
        .route("/v1/memory/{id}/history", get(crate::memory::history))
        .route(
            "/v1/skills",
            get(crate::skills::list).post(crate::skills::create),
        )
        .route(
            "/v1/skills/{name}",
            get(crate::skills::get)
                .patch(crate::skills::update)
                .delete(crate::skills::delete),
        )
        .route(
            "/v1/onboarding",
            get(crate::onboarding::get).put(crate::onboarding::put),
        )
        .route("/v1/external-agents", get(crate::onboarding::agents))
        .route(
            "/v1/external-agents/{id}",
            axum::routing::put(crate::onboarding::set_agent)
                .delete(crate::onboarding::delete_agent),
        )
        .route(
            "/v1/external-agents/{id}/browser-skill",
            post(crate::onboarding::install_skill),
        )
        .route("/v1/import/profiles", get(crate::import_api::profiles))
        .route("/v1/import", post(crate::import_api::run))
        .route("/v1/import/preview", post(crate::import_api::preview))
        .route("/v1/import/passwords", post(crate::import_api::passwords))
        .route("/v1/import/jobs/{id}", get(crate::import_api::job))
        .route(
            "/v1/import/jobs/{id}/events",
            get(crate::import_api::events),
        )
        .route("/v1/import/delivery", get(crate::import_delivery::pending))
        .route(
            "/v1/import/delivery/{id}/ack",
            post(crate::import_delivery::ack),
        )
        .route(
            "/v1/import/delivery/{id}/ticket",
            post(crate::import_delivery::mint),
        )
        .route(
            "/v1/extension/import/{ticket}",
            get(crate::import_delivery::fetch),
        )
        .route(
            "/v1/policy",
            get(crate::policy_api::get).put(crate::policy_api::put),
        )
        .route("/v1/policy/test", post(crate::policy_api::test))
        .route(
            "/v1/policy/validate",
            post(crate::policy_api::validate_document),
        )
        .route(
            "/v1/policy/network",
            get(crate::policy_api::network).put(crate::policy_api::put_network),
        )
        .route("/v1/connections", get(crate::connections::api::list))
        .route("/v1/connections/audit", get(crate::connections::api::audit))
        .route(
            "/v1/connections/spaces",
            post(crate::connections::spaces::create),
        )
        .route(
            "/v1/connections/spaces/{space_id}",
            delete(crate::connections::spaces::delete),
        )
        .route(
            "/v1/connections/{id}",
            get(crate::connections::api::get).patch(crate::connections::api::update),
        )
        .route(
            "/v1/connections/{id}/check",
            post(crate::connections::api::check),
        )
        .route(
            "/v1/connections/{id}/relogin",
            post(crate::connections::api::relogin),
        )
        .route(
            "/v1/connections/{id}/revoke",
            post(crate::connections::api::revoke),
        )
        .route(
            "/v1/connections/candidates/{id}/dismiss",
            post(crate::connections::api::dismiss),
        )
        .route("/v1/vault", get(crate::vault_api::status))
        .route("/v1/vault/setup", post(crate::vault_api::setup))
        .route("/v1/vault/unlock", post(crate::vault_api::unlock))
        .route("/v1/vault/lock", post(crate::vault_api::lock))
        .route(
            "/v1/vault/items",
            get(crate::vault_api::items).post(crate::vault_api::create),
        )
        .route(
            "/v1/vault/items/{id}",
            patch(crate::vault_api::update).delete(crate::vault_api::delete),
        )
        .route("/v1/vault/audit", get(crate::vault_api::audit))
        .route(
            "/v1/pinned-tabs",
            get(crate::sync::list_pinned).post(crate::sync::pin),
        )
        .route(
            "/v1/pinned-tabs/{id}",
            axum::routing::delete(crate::sync::unpin),
        )
        .route("/v1/sync", get(crate::sync::get_status))
        .route(
            "/v1/sync/config",
            axum::routing::put(crate::sync::configure),
        )
        .route("/v1/sync/unlock", post(crate::sync::unlock))
        .route("/v1/sync/setup", post(crate::sync::setup))
        .route("/v1/sync/pair", post(crate::sync::pair))
        .route("/v1/sync/run", post(crate::sync::run))
        .route("/v1/pairing", post(crate::pairing::create))
        .route("/v1/pairing/exchange", post(crate::pairing::exchange))
        .route("/v1/devices", get(crate::pairing::list))
        .route("/v1/devices/me", get(crate::access_requests::mine))
        .route(
            "/v1/devices/me/access-requests",
            post(crate::access_requests::request),
        )
        .route("/v1/access-requests", get(crate::access_requests::list))
        .route(
            "/v1/access-requests/{id}/approve",
            post(crate::access_requests::approve),
        )
        .route(
            "/v1/access-requests/{id}/deny",
            post(crate::access_requests::deny),
        )
        .route("/v1/remote/status", get(crate::remote::status))
        .route("/v1/remote/local-access", get(crate::remote::local_access))
        .route(
            "/v1/remote/tailscale",
            axum::routing::put(crate::remote::configure_tailscale),
        )
        .route(
            "/v1/remote/cloudflare",
            axum::routing::put(crate::remote::configure_cloudflare),
        )
        .route(
            "/v1/remote/relay",
            axum::routing::put(crate::remote::configure_relay),
        )
        .route("/v1/push/vapid", get(crate::push::vapid))
        .route(
            "/v1/push/subscriptions",
            get(crate::push::list).post(crate::push::create),
        )
        .route("/v1/push/subscriptions/{id}", delete(crate::push::delete))
        .route("/v1/push/presence", post(crate::push::presence))
        .route("/v1/otp", post(crate::otp_ingest::ingest))
        .route(
            "/v1/settings/config",
            get(crate::settings_api::get).put(crate::settings_api::put),
        )
        .route_layer(middleware::from_fn_with_state(
            (shared.clone(), unix),
            authorize,
        ))
        .layer(middleware::from_fn(json_rejections))
        .with_state(shared);
    crate::ui::attach(api, ui.as_deref())
}

/// Axum rejects malformed bodies and queries with a bare text response; clients of this API parse
/// `{code, message}` for every failure, so rewrite those into the same shape.
async fn json_rejections(request: axum::extract::Request, next: Next) -> Response {
    let upload =
        request.method() == axum::http::Method::POST && request.uri().path().ends_with("/messages");
    let response = next.run(request).await;
    let status = response.status();
    let plain = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/plain"));
    if !(status.is_client_error() && plain) {
        return response;
    }
    let code = match status {
        StatusCode::PAYLOAD_TOO_LARGE if upload => "attachments_too_large",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        _ => "invalid_request",
    };
    let detail = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .ok()
        .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok());
    let message = match (code, detail) {
        ("invalid_request", Some(detail)) => detail,
        _ => code.replace('_', " "),
    };
    (
        status,
        Json(ApiError {
            code: code.into(),
            message,
        }),
    )
        .into_response()
}

/// Token of an `Authorization: Bearer <token>` value; the scheme is case-insensitive (RFC 7235).
fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token)
}

/// Routes only Unix-socket callers may use, whatever scopes a TCP token holds.
pub(crate) const SOCKET_ONLY: &[(&str, &str)] = &[
    ("POST", "/v1/devices/pair"),
    ("GET", "/v1/remote/local-access"),
];

pub(crate) const LOCAL_REQUIRED: &[(&str, &str)] = &[
    ("PUT", "/v1/sync/config"),
    ("POST", "/v1/daemon/restart"),
    ("PUT", "/v1/policy"),
    ("PUT", "/v1/policy/network"),
    ("PUT", "/v1/external-agents/{id}"),
    ("DELETE", "/v1/external-agents/{id}"),
];

pub(crate) const LOCAL_AWARE: &[(&str, &str)] = &[
    ("GET", "/v1/settings/config"),
    ("PUT", "/v1/settings/config"),
    ("POST", "/v1/memory/model/download"),
    ("POST", "/v1/sync/setup"),
    ("POST", "/v1/sync/pair"),
    ("POST", "/v1/sync/unlock"),
];

/// WebSocket routes (GET templates) whose browser clients may authenticate by subprotocol.
/// Browsers cannot set `Authorization` on a WebSocket handshake, so they offer
/// `Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>` instead.
pub(crate) const WEBSOCKET_ROUTES: &[&str] = &[
    "/v1/tickets/{id}/takeover",
    "/v1/sessions/{id}/browser/preview",
];

/// The subprotocol bearer of a genuine RFC 6455 handshake (GET, `Connection: upgrade`,
/// `Upgrade: websocket`, version 13, a 16-byte key) to a [`WEBSOCKET_ROUTES`] route. The
/// caller consults it only when no `Authorization` header is present; the bearer entry is
/// never echoed.
fn websocket_bearer<'a>(request: &'a axum::extract::Request, route: &str) -> Option<&'a str> {
    use axum::http::header;
    use base64::Engine;
    let headers = request.headers();
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let handshake = request.method() == axum::http::Method::GET
        && WEBSOCKET_ROUTES.contains(&route)
        && value(header::CONNECTION).is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
        && value(header::UPGRADE).is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        && value(header::SEC_WEBSOCKET_VERSION) == Some("13")
        && value(header::SEC_WEBSOCKET_KEY).is_some_and(|key| {
            base64::engine::general_purpose::STANDARD
                .decode(key)
                .is_ok_and(|bytes| bytes.len() == 16)
        });
    if !handshake {
        return None;
    }
    headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|p| p.trim().strip_prefix("openside.bearer."))
        .filter(|t| {
            !t.is_empty()
                && t.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
        })
}

pub(crate) async fn authorize(
    State((shared, unix)): State<(Shared, bool)>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    // This marker can only remove person-message authority, never grant it to a token.
    let agent_client = request
        .headers()
        .get("x-openside-client")
        .is_some_and(|value| value == "mcp");
    let channel_client = request
        .headers()
        .get("x-openside-client")
        .is_some_and(|value| value == "channel");
    if unix {
        request
            .extensions_mut()
            .insert(Access(vec![CapabilityScope::Admin]));
        request
            .extensions_mut()
            .insert(Caller(None, DeviceKind::AgentClient));
        request
            .extensions_mut()
            .insert(crate::remote::LocalAuthority(true));
        request.extensions_mut().insert(if agent_client {
            MessageAuthorKind::AgentClient
        } else if channel_client {
            MessageAuthorKind::Channel
        } else {
            MessageAuthorKind::Person
        });
        return next.run(request).await;
    }
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str());
    let route = ROUTES
        .iter()
        .find(|r| Some(r.path) == template && r.method.as_str() == request.method().as_str());
    // A served route with no ROUTES entry has no reviewed scope, so TCP callers get none.
    let Some(route) = route else {
        return HttpError(StatusCode::FORBIDDEN, "forbidden").into_response();
    };
    if PUBLIC_ROUTES.contains(&(route.method.as_str(), route.path)) {
        return next.run(request).await;
    }
    // Subscribe before the token check so a revocation racing this request still ends
    // the stream it opens.
    let mut revoked = shared.remote.revoked.subscribe();
    // A present Authorization header is the only credential considered, even if malformed.
    let token = match request.headers().get(axum::http::header::AUTHORIZATION) {
        Some(value) => value.to_str().ok().and_then(bearer_token),
        None => websocket_bearer(&request, route.path),
    };
    let Some(token) = token else {
        return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    let (device, access, kind) = match shared.db.device(token.to_owned()).await {
        Ok(Some((device, scopes, kind))) => (device, Access(scopes), kind),
        Ok(None) => return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        Err(_) => return HttpError::internal().into_response(),
    };
    let kind_specific = matches!(
        route.path,
        "/v1/tickets/{id}/resolve" | "/v1/tickets/{id}/reject"
    );
    let allowed = if kind_specific {
        access.has(CapabilityScope::Approve) || access.has(CapabilityScope::HumanStep)
    } else {
        access.has(route.scope)
    };
    if !allowed {
        return HttpError(StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    // Minting credentials needs the local user (the 0600 socket). A leaked TCP token, even
    // an admin one, must not be able to mint further tokens.
    if SOCKET_ONLY.contains(&(route.method.as_str(), route.path)) {
        return HttpError(StatusCode::FORBIDDEN, "local_socket_required").into_response();
    }
    let local_required = route.path == "/v1/pairing"
        || route.path == "/v1/access-requests/{id}/approve"
        || route.path.starts_with("/v1/import")
        || (route.method.as_str() == "PUT" && route.path.starts_with("/v1/remote/"))
        || LOCAL_REQUIRED.contains(&(route.method.as_str(), route.path));
    let local_route = local_required
        || route.path.starts_with("/v1/remote/")
        || LOCAL_AWARE.contains(&(route.method.as_str(), route.path));
    let local = if local_route
        && kind == DeviceKind::PersonDevice
        && access.has(CapabilityScope::Admin)
        && crate::remote::local_tcp(&shared, &request)
    {
        match shared.db.device_is_local(device.clone()).await {
            Ok(local) => local,
            Err(_) => return HttpError::internal().into_response(),
        }
    } else {
        false
    };
    if !local && local_required {
        return HttpError(StatusCode::FORBIDDEN, "local_access_required").into_response();
    }
    request
        .extensions_mut()
        .insert(crate::remote::LocalAuthority(local));
    if shared.remote.is_revoked(&device) {
        return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    request.extensions_mut().insert(access);
    request
        .extensions_mut()
        .insert(Caller(Some(device.clone()), kind));
    request
        .extensions_mut()
        .insert(if agent_client || kind == DeviceKind::AgentClient {
            MessageAuthorKind::AgentClient
        } else if channel_client {
            MessageAuthorKind::Channel
        } else {
            MessageAuthorKind::Person
        });
    // Never admit a device-controlled, unfinished body into a commit gate.
    let body_limit = if route.path == "/v1/sessions/{id}/messages" {
        crate::artifacts::SUBMIT_BODY_LIMIT
    } else {
        2 * 1024 * 1024
    };
    let (parts, body) = request.into_parts();
    let bytes = tokio::select! {
        bytes = axum::body::to_bytes(body, body_limit) => match bytes {
            Ok(bytes) => bytes,
            Err(_) => return HttpError(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large").into_response(),
        },
        () = crate::pairing::until_revoked(&shared, &mut revoked, &device) => {
            return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    };
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    // Exempt every revoke, not only self-revoke, to avoid reciprocal gate deadlocks.
    let revoking = route.method.as_str() == "DELETE" && route.path == "/v1/devices/{id}";
    let guard = if revoking {
        None
    } else {
        Some(shared.remote.device_gate(&device).read_owned().await)
    };
    if shared.remote.is_revoked(&device) {
        return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    // The handler, not its HTTP waiter, owns the commit and its audit/continuation.
    // Streaming handlers return at stream start and release the gate there.
    let response = match tokio::spawn(async move {
        let response = next.run(request).await;
        drop(guard);
        response
    })
    .await
    {
        Ok(response) => response,
        Err(_) => return HttpError::internal().into_response(),
    };
    if !route.sse {
        return crate::person_only::response(
            response,
            route.response_schema,
            &Caller(Some(device), kind),
        )
        .await;
    }
    // A revoked device must not keep reading: end its streams the moment it is revoked.
    let (parts, body) = response.into_parts();
    let mut data = body.into_data_stream();
    let stream = async_stream::stream! {
        loop {
            tokio::select! {
                chunk = futures_util::StreamExt::next(&mut data) => match chunk {
                    Some(chunk) => yield chunk,
                    None => break,
                },
                () = crate::pairing::until_revoked(&shared, &mut revoked, &device) => break,
            }
        }
    };
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        api_version: "v1".into(),
    })
}
async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<Session>)> {
    let _agent_gate = s.agents.gate.lock().await;
    let agent = request
        .agent_id
        .as_deref()
        .map(|id| {
            let agent = s
                .agents
                .get(id)?
                .ok_or(HttpError(StatusCode::NOT_FOUND, "agent_not_found"))?;
            if agent.status == AgentStatus::Retired {
                return Err(HttpError(StatusCode::CONFLICT, "agent_retired"));
            }
            Ok::<_, HttpError>(agent)
        })
        .transpose()?;
    if agent.is_some() && (request.incognito || request.browser_space != BrowserSpace::Shared) {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_browser_space"));
    }
    if caller.widens(&s, request.mode, request.workspace.as_deref()) {
        return Err(HttpError(StatusCode::FORBIDDEN, "local_socket_required"));
    }
    let id = SessionId::new(random_id());
    let stored_id = id.clone();
    let title = match request.title.as_deref().map(str::trim) {
        None | Some("") => DEFAULT_SESSION_TITLE.to_owned(),
        Some(title) => crate::session_meta::valid_title(title)?,
    };
    let lock_default_title = title == DEFAULT_SESSION_TITLE
        && request
            .title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty());
    let model = request
        .model
        .or_else(|| agent.as_ref().and_then(|agent| agent.model.clone()))
        .or_else(|| s.models.default_model())
        .filter(|model| !model.trim().is_empty())
        .ok_or(HttpError(StatusCode::BAD_REQUEST, "model_not_configured"))?;
    let mode = request.mode.unwrap_or_else(|| s.policy.default_mode());
    let mode = match &agent {
        Some(agent) => crate::agents::ceiling(&s, agent, mode)?,
        None => mode,
    };
    let workspace = match request.workspace {
        Some(path) => {
            if !std::path::Path::new(&path).is_absolute() {
                return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_workspace"));
            }
            let canonical = tokio::fs::canonicalize(path)
                .await
                .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_workspace"))?;
            if !canonical.is_dir() {
                return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_workspace"));
            }
            Some(canonical.to_string_lossy().into_owned())
        }
        None => None,
    };
    let incognito = request.incognito;
    let browser_space = request.browser_space;
    // An incognito session already browses in its own Chromium; "isolated" would be a second,
    // contradictory answer to where it browses.
    if incognito && browser_space == openside_proto::BrowserSpace::Isolated {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_browser_space"));
    }
    let event =
        s.db.store(move |db| {
            Ok(db.append(
                &stored_id,
                None,
                EventKind::SessionCreated {
                    agent_id: request.agent_id,
                    title,
                    model,
                    mode,
                    workspace,
                    incognito,
                    browser_space,
                },
            )?)
        })
        .await?;
    let _ = s.events.send(event);
    if lock_default_title {
        s.emit(
            &id,
            None,
            EventKind::SessionUpdated {
                mode: None,
                title: Some(DEFAULT_SESSION_TITLE.into()),
                pinned: None,
                archived: None,
            },
        )
        .await?;
    }
    caller.record_driver(&s, &id).await?;
    if crate::tasks::front(&s, s.db.state(&id).await?.agent_id.as_deref())
        && !(std::env::var_os("OPENSIDE_AGENT_SCRIPT").is_some()
            && std::env::var_os("OPENSIDE_TEST_DISABLE_PREWARM").is_some())
    {
        let shared = s.clone();
        let session = id.clone();
        tokio::spawn(async move { crate::browser_state::prewarm_session(&shared, &session).await });
    }
    Ok((StatusCode::CREATED, Json(s.db.state(&id).await?.session())))
}
async fn session(State(s): State<Shared>, Path(id): Path<String>) -> Result<Json<Session>> {
    Ok(Json(
        s.db.state(&SessionId::new(id))
            .await
            .map_err(|_| HttpError::not_found())?
            .session(),
    ))
}
async fn submit(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author_kind): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
    Json(request): Json<SubmitMessageRequest>,
) -> Result<(StatusCode, Json<Message>)> {
    if request.text.len() > openside_agent_sidecar::MAX_USER_TEXT_BYTES {
        return Err(HttpError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "message_too_large",
        ));
    }
    if request.text.trim().is_empty() && request.attachments.is_empty() {
        return Err(HttpError(StatusCode::BAD_REQUEST, "empty_message"));
    }
    let id = SessionId::new(id);
    let agent_gate = s.agents.gate.lock().await;
    // Existence check, attachment publication and the append are one step per session:
    // concurrent retries of one message id cannot both publish, and a deleted session
    // cannot gain uploads.
    let gate = crate::artifacts::lifecycle(&s, &id).await;
    let state = s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
    if let Some(agent_id) = &state.agent_id {
        let agent = s
            .agents
            .get(agent_id)?
            .ok_or(HttpError(StatusCode::NOT_FOUND, "agent_not_found"))?;
        if agent.status == AgentStatus::Retired {
            return Err(HttpError(StatusCode::CONFLICT, "agent_retired"));
        }
    }
    caller.record_driver(&s, &id).await?;
    if s.auth_failed(&id).await? {
        return Err(HttpError::conflict());
    }
    let mid = request
        .message_id
        .unwrap_or_else(|| MessageId::new(random_id()));
    // A retry of an accepted message keeps the original files and event.
    let attachments = if state.queue.status(&mid).is_some() {
        Vec::new()
    } else {
        crate::artifacts::store_attachments(&s, &id, &mid, request.attachments).await?
    };
    s.emit(
        &id,
        None,
        EventKind::MessageQueued {
            message_id: mid.clone(),
            text: request.text,
            mode: if author_kind == MessageAuthorKind::Person {
                openside_core::QueueMode::Steer
            } else {
                openside_core::QueueMode::Queue
            },
            attachments,
            author_kind,
            from_agent: None,
        },
    )
    .await?;
    drop(gate);
    drop(agent_gate);
    s.command(&id, Action::Deliver).await?;
    Ok((StatusCode::ACCEPTED, Json(s.message(&id, &mid).await?)))
}
async fn edit(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((id, mid)): Path<(String, String)>,
    Json(request): Json<EditMessageRequest>,
) -> Result<Json<Message>> {
    if request.text.len() > openside_agent_sidecar::MAX_USER_TEXT_BYTES {
        return Err(HttpError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "message_too_large",
        ));
    }
    let id = SessionId::new(id);
    let mid = MessageId::new(mid);
    if request.text.trim().is_empty() {
        let state = s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
        // Only a message that carries files may be left without words.
        if state.attachments.get(&mid).map_or(true, Vec::is_empty) {
            return Err(HttpError(StatusCode::BAD_REQUEST, "empty_message"));
        }
    }
    caller.record_driver(&s, &id).await?;
    s.command(&id, Action::Edit(mid.clone(), request.text))
        .await?;
    Ok(Json(s.message(&id, &mid).await?))
}
async fn remove(
    State(s): State<Shared>,
    Path((id, mid)): Path<(String, String)>,
) -> Result<Json<Message>> {
    let id = SessionId::new(id);
    let mid = MessageId::new(mid);
    s.command(&id, Action::Delete(mid.clone())).await?;
    Ok(Json(s.message(&id, &mid).await?))
}
async fn promote(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((id, mid)): Path<(String, String)>,
) -> Result<Json<Message>> {
    let id = SessionId::new(id);
    let mid = MessageId::new(mid);
    caller.record_driver(&s, &id).await?;
    s.command(&id, Action::Promote(mid.clone())).await?;
    Ok(Json(s.message(&id, &mid).await?))
}
async fn reorder(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((id, mid)): Path<(String, String)>,
    Json(request): Json<MoveMessageRequest>,
) -> Result<Json<Message>> {
    let id = SessionId::new(id);
    let mid = MessageId::new(mid);
    caller.record_driver(&s, &id).await?;
    s.command(&id, Action::Move(mid.clone(), request.position))
        .await?;
    Ok(Json(s.message(&id, &mid).await?))
}
async fn stop(State(s): State<Shared>, Path(id): Path<String>) -> Result<Json<Session>> {
    let id = SessionId::new(id);
    s.command(&id, Action::Stop).await?;
    Ok(Json(s.db.state(&id).await?.session()))
}
/// Returns at once; the warm-up runs in the background and its failures stay silent, since
/// the next message reports any real problem.
async fn prewarm_session(State(s): State<Shared>, Path(id): Path<String>) -> Result<StatusCode> {
    let id = SessionId::new(id);
    let state = s.db.state(&id).await?;
    if !state.deleted {
        tokio::spawn(async move { crate::browser_state::prewarm_session(&s, &id).await });
    }
    Ok(StatusCode::ACCEPTED)
}
async fn prewarm_browser(State(s): State<Shared>) -> StatusCode {
    tokio::spawn(async move { crate::browser_state::prewarm_browser(&s).await });
    StatusCode::ACCEPTED
}
#[derive(Deserialize)]
struct EventsQuery {
    subagents: Option<bool>,
}
async fn events(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = std::result::Result<SseEvent, Infallible>>>> {
    let subagents = query.subagents.unwrap_or(true);
    let id = SessionId::new(id);
    let last_seq =
        s.db.state(&id)
            .await
            .map_err(|_| HttpError::not_found())?
            .last_seq;
    let mut after = match headers.get("last-event-id") {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or(HttpError(StatusCode::BAD_REQUEST, "invalid_event_id"))?,
        None => 0,
    };
    // A cursor beyond the durable log can never be satisfied, so the stream would wait
    // forever even after the session is deleted.
    if after > last_seq {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_event_id"));
    }
    // Subscribe before replay; notifications wake a durable cursor, so broadcast
    // lag never creates event gaps or a replay/live race.
    let mut live = s.events.subscribe();
    let stream = async_stream::stream! {
        yield Ok(SseEvent::default().comment("ok"));
        loop {
            match s.db.events(&id, after).await {
                Ok(page) if !page.is_empty() => {
                    for event in page {
                        after = event.seq;
                        let child = matches!(
                            event.kind,
                            EventKind::SubagentStarted { .. }
                                | EventKind::SubagentLifecycle { .. }
                                | EventKind::SubagentProgress { .. }
                                | EventKind::SubagentEnded { .. }
                        );
                        if subagents || !child {
                            if let Ok(mut value) = serde_json::to_value(&event) {
                                crate::person_only::strip_person_only(&mut value, "Event", &caller);
                                if let Ok(data) = serde_json::to_string(&value) {
                                    yield Ok(SseEvent::default().id(event.seq.to_string()).data(data));
                                }
                            }
                        }
                        if matches!(event.kind, EventKind::SessionDeleted {}) { return; }
                    }
                    continue;
                }
                Err(_) => break,
                Ok(_) => {
                    // Replay is exhausted. A deleted session never produces more events, so
                    // end the stream. Look once more first: the tombstone may have been
                    // committed after the read above.
                    if s.db.state(&id).await.is_err() {
                        match s.db.events(&id, after).await {
                            Ok(page) if !page.is_empty() => continue,
                            _ => break,
                        }
                    }
                }
            }
            match live.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Deserialize)]
struct EventPollQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "poll_wait")]
    wait: u64,
    subagents: Option<bool>,
}

fn poll_wait() -> u64 {
    20
}

async fn poll_events(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Query(query): Query<EventPollQuery>,
) -> Result<Json<EventPoll>> {
    if query.wait > 25 {
        return Err(HttpError::invalid_value());
    }
    let id = SessionId::new(id);
    let mut live = s.events.subscribe();
    let mut revocations = s.remote.revoked.subscribe();
    let revocation = async {
        match caller.0.as_deref() {
            Some(device) => crate::pairing::until_revoked(&s, &mut revocations, device).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(revocation);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(query.wait);
    loop {
        if caller
            .0
            .as_deref()
            .is_some_and(|id| s.remote.is_revoked(id))
        {
            return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
        }
        let read = async {
            let state_id = id.clone();
            let last_seq =
                s.db.store(move |store| Ok(store.session(&state_id).map(|state| state.last_seq)))
                    .await?;
            if last_seq.is_some_and(|last_seq| query.after > last_seq) {
                return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_event_id"));
            }
            let page = s.db.events(&id, query.after).await?;
            Ok::<_, HttpError>((last_seq, page))
        };
        let (last_seq, mut page) = tokio::select! {
            biased;
            () = &mut revocation => return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized")),
            read = read => read?,
        };
        if caller
            .0
            .as_deref()
            .is_some_and(|id| s.remote.is_revoked(id))
        {
            return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
        }
        if !page.is_empty() {
            let last_seq = page.last().map_or(query.after, |event| event.seq);
            if query.subagents == Some(false) {
                page.retain(|event| {
                    !matches!(
                        event.kind,
                        EventKind::SubagentStarted { .. }
                            | EventKind::SubagentLifecycle { .. }
                            | EventKind::SubagentProgress { .. }
                            | EventKind::SubagentEnded { .. }
                    )
                });
            }
            return Ok(Json(EventPoll {
                events: page,
                last_seq,
            }));
        }
        if last_seq.is_none() {
            return Err(HttpError::not_found());
        }
        tokio::select! {
            biased;
            () = &mut revocation => return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized")),
            _ = tokio::time::sleep_until(deadline) => return Ok(Json(EventPoll { events: Vec::new(), last_seq: query.after })),
            result = live.recv() => {
                if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    return Ok(Json(EventPoll { events: Vec::new(), last_seq: query.after }));
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct TicketQuery {
    status: Option<TicketStatus>,
}
async fn tickets(
    State(s): State<Shared>,
    Query(query): Query<TicketQuery>,
) -> Result<Json<TicketList>> {
    Ok(Json(TicketList {
        tickets: s
            .tickets()
            .await?
            .into_iter()
            .filter(|ticket| query.status.map_or(true, |status| status == ticket.status))
            .collect(),
    }))
}
async fn find(s: &Shared, id: String) -> Result<Ticket> {
    crate::tickets::get(s, &TicketId::new(id))
        .await?
        .ok_or_else(HttpError::not_found)
}
async fn ticket(State(s): State<Shared>, Path(id): Path<String>) -> Result<Json<Ticket>> {
    Ok(Json(find(&s, id).await?))
}
fn pending(ticket: &Ticket) -> Result<()> {
    if ticket.status != TicketStatus::Pending || ticket.expires_at <= Utc::now() {
        Err(HttpError::conflict())
    } else {
        Ok(())
    }
}

enum SecretAnswer {
    Code(Zeroizing<String>),
    Authenticator(openside_vault::Totp),
}

/// The live policy decides at release time, not only when the step was opened: a Deny added
/// since then (origin list, rule, read-only session) refuses releasing a code or key into
/// `origin`, and the human's answer cannot override it.
async fn release_denied(s: &Shared, ticket: &Ticket, origin: &str) -> Result<()> {
    denied_on(
        s,
        ticket,
        origin,
        &[
            openside_policy::Capability::Browse,
            openside_policy::Capability::VaultUse,
        ],
    )
    .await
}

/// [`release_denied`] for any capabilities: a `choice` answer only clicks the page (Browse).
async fn denied_on(
    s: &Shared,
    ticket: &Ticket,
    origin: &str,
    capabilities: &[openside_policy::Capability],
) -> Result<()> {
    let state = s.db.state(&ticket.session_id).await?;
    let policy =
        crate::policy_api::effective(s, &crate::tools::ToolEnv::new(s.clone(), &state).policy);
    if capabilities.iter().any(|capability| {
        policy
            .decide(&openside_policy::Action::new(*capability, origin))
            .effect
            == openside_policy::Effect::Deny
    }) {
        return Err(HttpError(StatusCode::FORBIDDEN, "policy_denied"));
    }
    Ok(())
}

pub(crate) fn refused(code: &str) -> HttpError {
    match code {
        "origin_mismatch" => HttpError(StatusCode::CONFLICT, "origin_mismatch"),
        "otp_already_registered" => HttpError(StatusCode::CONFLICT, "otp_already_registered"),
        "unavailable" => HttpError(StatusCode::SERVICE_UNAVAILABLE, "secret_input_unavailable"),
        "vault_unavailable" => HttpError(StatusCode::SERVICE_UNAVAILABLE, "vault_unavailable"),
        "vault_denied" => HttpError(StatusCode::FORBIDDEN, "vault_denied"),
        _ => HttpError(StatusCode::CONFLICT, "secret_input_failed"),
    }
}

async fn resolve(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<ResolveTicketRequest>,
) -> Result<Json<Ticket>> {
    let ticket;
    let who = crate::tools::otp::approver(caller.0.as_deref());
    {
        let _gate = s.ticket_gate.lock().await;
        ticket = find(&s, id).await?;
        access.ticket(&ticket)?;
        pending(&ticket)?;
        if request.hire.is_some() && ticket.kind != TicketKind::Hire {
            return Err(HttpError::invalid_value());
        }
        let value = match ticket.kind {
            TicketKind::SecretInput | TicketKind::Info if ticket.form.is_some() => {
                let form = ticket.form.as_ref().ok_or_else(HttpError::invalid_value)?;
                release_denied(&s, &ticket, &form.origin).await?;
                for field in &form.fields {
                    let origin = field
                        .target_origin
                        .as_deref()
                        .ok_or_else(HttpError::invalid_value)?;
                    release_denied(&s, &ticket, origin).await?;
                }
                serde_json::to_value(
                    crate::tools::forms::resolve_form(&s, &ticket, request.value).await?,
                )
                .map_err(|_| HttpError::invalid_value())?
            }
            TicketKind::Confirm if crate::tools::connections::is_grant_ticket(&ticket) => {
                caller.person_approval(&s, &ticket).await?;
                crate::tools::connections::resolve_grant(&s, &ticket, request.value).await?
            }
            TicketKind::Confirm if crate::routines::proposals::is_proposal(&ticket) => {
                caller.person_approval(&s, &ticket).await?;
                crate::routines::proposals::resolve(&s, &ticket, &caller, request.value).await?
            }
            TicketKind::Confirm => {
                if request.value != true
                    && request.value != false
                    && request.value != "approve"
                    && request.value != "reject"
                {
                    return Err(HttpError::invalid_value());
                }
                let approved = crate::tickets::approved(&request.value);
                if approved {
                    caller.person_approval(&s, &ticket).await?;
                }
                match &ticket.otp_register {
                    None => json!({"approved":approved}),
                    Some(register) => {
                        match crate::tools::otp::interrupted_registration(&s, &ticket).await {
                            // An earlier approval may have saved the factor and never
                            // committed: the answer cannot undo it, so the truthful outcome
                            // (registered, or indeterminate) is reported.
                            Some(outcome) => outcome,
                            None if !approved => {
                                crate::tools::otp::discard(&ticket);
                                json!({"approved":false,"status":"declined"})
                            }
                            None => {
                                release_denied(&s, &ticket, &register.origin).await?;
                                let sink = s.registry.secret_sink.read().clone();
                                crate::tools::otp::approve_registration(&s, &ticket, &who, sink)
                                    .await
                                    .map_err(|error| refused(error.code))?
                            }
                        }
                    }
                }
            }
            TicketKind::Info => {
                let Value::String(answer) = request.value else {
                    return Err(HttpError::invalid_value());
                };
                if !ticket.options.is_empty()
                    && !ticket.options.iter().any(|option| option.value == answer)
                {
                    return Err(HttpError::invalid_value());
                }
                if ticket
                    .tool
                    .as_ref()
                    .is_some_and(|tool| tool.name == "browser_import_lookup")
                {
                    caller.person_approval(&s, &ticket).await?;
                    denied_on(
                        &s,
                        &ticket,
                        "browser-imports/default",
                        &[openside_policy::Capability::PrivacyRead],
                    )
                    .await?;
                }
                json!({"answer":answer})
            }
            TicketKind::SecretInput => {
                // A `request_otp` ticket that offers registration may carry the site's
                // authenticator instead of a code; it is parsed before anything is touched.
                let value = match request.value {
                    Value::String(value) => SecretAnswer::Code(Zeroizing::new(value)),
                    Value::Object(mut object)
                        if object.len() == 1
                            && ticket.otp.as_ref().is_some_and(|otp| otp.can_register) =>
                    {
                        // Keeping a permanent vault factor is an approval, like a
                        // register_authenticator ticket; a code alone needs only HumanStep.
                        if !access.has(CapabilityScope::Approve) {
                            return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
                        }
                        caller.person_approval(&s, &ticket).await?;
                        let Some(Value::String(text)) = object.remove("totp") else {
                            return Err(HttpError::invalid_value());
                        };
                        let totp =
                            openside_vault::Totp::parse(openside_vault::Secret::new(text))
                                .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_totp"))?;
                        if !s.registry.vault.available() {
                            return Err(HttpError(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "vault_unavailable",
                            ));
                        }
                        SecretAnswer::Authenticator(totp)
                    }
                    _ => return Err(HttpError::invalid_value()),
                };
                if let Some(outcome) =
                    crate::tools::otp::interrupted_registration(&s, &ticket).await
                {
                    // An earlier authenticator answer may have saved the factor and never
                    // committed; this answer is not applied over it.
                    outcome
                } else {
                    let sink = s.registry.secret_sink.read().clone().ok_or(HttpError(
                        StatusCode::NOT_IMPLEMENTED,
                        "secret_input_unavailable",
                    ))?;
                    let target = ticket
                        .target
                        .as_ref()
                        .ok_or_else(HttpError::invalid_value)?;
                    let browser = &s.registry.browser;
                    // A browser mutation and the TicketResolved commit cannot be made atomic,
                    // even under ticket_gate. An intent left by a crash or a failed commit means
                    // the page may already hold the secret, so a blind retry could insert it a
                    // second time; the ticket stays pending until the user rejects it or the
                    // agent opens a new step.
                    if browser.fill_intent_pending(&s, &ticket.id).await? {
                        return Err(HttpError(
                            StatusCode::CONFLICT,
                            "secret_input_indeterminate",
                        ));
                    }
                    let origin = target
                        .origin
                        .as_deref()
                        .ok_or_else(HttpError::invalid_value)?;
                    release_denied(&s, &ticket, origin).await?;
                    browser
                        .mark_ticket_secret(&s, &ticket.id, &ticket.session_id, target)
                        .await
                        .map_err(|error| refused(error.code))?;
                    // The vault item an authenticator answer creates carries this id, so a
                    // later recovery recognizes exactly that save.
                    let operation = crate::persistence::random_id();
                    let registers = matches!(value, SecretAnswer::Authenticator(_));
                    browser
                        .record_fill_intent(&s, &ticket.id, registers.then_some(operation.as_str()))
                        .await?;
                    let filled = match value {
                        SecretAnswer::Code(value) => sink
                            .fill(&ticket.id, &ticket.session_id, target, value)
                            .await
                            .map(|()| json!({"status":"filled"})),
                        SecretAnswer::Authenticator(totp) => crate::tools::otp::register_and_fill(
                            &s,
                            &ticket,
                            target,
                            origin,
                            totp,
                            &operation,
                            sink.clone(),
                            &who,
                        )
                        .await
                        .map(|registered| match registered {
                            crate::tools::otp::Registered::Filled => {
                                json!({"status":"filled","registered":true})
                            }
                            // Saved, so the ticket ends here; a later request_otp fills from
                            // the vault.
                            crate::tools::otp::Registered::Unfilled => {
                                json!({"status":"registered","filled":false})
                            }
                        }),
                    };
                    let value = match filled {
                        Ok(value) => value,
                        Err(error) => {
                            // Only `fill_failed` may have reached the page; every other code is
                            // decided before any write.
                            if error.code != "fill_failed" {
                                browser.clear_fill_intent(&s, &ticket.id).await?;
                            }
                            return Err(refused(error.code));
                        }
                    };
                    if value.get("status").and_then(Value::as_str) == Some("filled") {
                        crate::tools::otp::filled(&s, &ticket).await;
                    }
                    browser.persist(&s).await?;
                    value
                }
            }
            TicketKind::Login => {
                if request.value.get("save").and_then(Value::as_bool) == Some(true) {
                    if !access.has(CapabilityScope::Approve) {
                        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
                    }
                    caller.person_approval(&s, &ticket).await?;
                }
                // Authorized values get redactors before any page work or value-bearing error.
                let answer = crate::tools::login::parse_login_answer(&s, &ticket, request.value)?;
                if let Some(outcome) = crate::tools::login::interrupted(&s, &ticket).await {
                    // An earlier answer submitted the page and kept the login, and its
                    // commit never happened; this answer is not applied over it.
                    outcome
                } else {
                    if s.registry
                        .browser
                        .fill_intent_pending(&s, &ticket.id)
                        .await?
                    {
                        return Err(HttpError(
                            StatusCode::CONFLICT,
                            "secret_input_indeterminate",
                        ));
                    }
                    let origin = ticket
                        .login
                        .as_ref()
                        .map(|login| login.origin.clone())
                        .ok_or_else(HttpError::invalid_value)?;
                    release_denied(&s, &ticket, &origin).await?;
                    crate::tools::login::resolve_login(&s, &ticket, answer, &who).await?
                }
            }
            TicketKind::Choice => {
                caller.person_approval(&s, &ticket).await?;
                let origin = ticket
                    .choice
                    .as_ref()
                    .map(|choice| choice.origin.clone())
                    .ok_or_else(HttpError::invalid_value)?;
                denied_on(&s, &ticket, &origin, &[openside_policy::Capability::Browse]).await?;
                crate::tools::login::resolve_choice(&s, &ticket, request.value).await?
            }
            TicketKind::Hire => {
                caller.person_approval(&s, &ticket).await?;
                crate::tickets::resolve_hire(&s, &ticket, request.value, request.hire).await?
            }
            TicketKind::DeviceApproval | TicketKind::Takeover => {
                return Err(HttpError(StatusCode::BAD_REQUEST, "use_done"))
            }
        };
        // Statuses, next steps and ids are daemon constants; free text (an answer, a page's
        // error message) is scrubbed.
        let mut keep: Vec<&[&str]> = vec![
            &["status"],
            &["next"],
            &["option"],
            &["item_id"],
            &["agent_id"],
            &["routine_id"],
        ];
        if crate::tools::connections::is_grant_ticket(&ticket) {
            keep.push(&["access"]);
        }
        let value = s.registry.redact_except(value, &keep)?;
        crate::tickets::audit(
            &s,
            &ticket.session_id,
            &ticket.tool_call_id,
            "ticket",
            "resolved",
            0,
        )
        .await?;
        s.emit(
            &ticket.session_id,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.id.clone(),
                resolution: value,
            },
        )
        .await?;
        if matches!(ticket.kind, TicketKind::SecretInput | TicketKind::Login)
            || ticket.otp_register.is_some()
            || ticket.form.is_some()
        {
            // The ticket is no longer pending, so a stale intent can never block anything.
            let _ = s.registry.browser.clear_fill_intent(&s, &ticket.id).await;
        }
    }
    resume(&s, &ticket).await;
    Ok(Json(find(&s, ticket.id.as_str().into()).await?))
}
async fn done(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Path(id): Path<String>,
) -> Result<Json<Ticket>> {
    let ticket;
    {
        let _gate = s.ticket_gate.lock().await;
        ticket = find(&s, id).await?;
        access.ticket(&ticket)?;
        pending(&ticket)?;
        if !matches!(
            ticket.kind,
            TicketKind::DeviceApproval | TicketKind::Takeover
        ) {
            return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_ticket_kind"));
        }
        crate::takeover::finish(&s, &ticket, openside_proto::TakeoverEndReason::Done).await;
        let resolution = if ticket.kind == TicketKind::Takeover {
            crate::takeover::done_result(&s, &ticket).await
        } else {
            json!({"status":"done"})
        };
        crate::tickets::audit(
            &s,
            &ticket.session_id,
            &ticket.tool_call_id,
            "ticket",
            "resolved",
            0,
        )
        .await?;
        s.emit(
            &ticket.session_id,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.id.clone(),
                resolution,
            },
        )
        .await?;
    }
    resume(&s, &ticket).await;
    Ok(Json(find(&s, ticket.id.as_str().into()).await?))
}
async fn reject(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<RejectTicketRequest>,
) -> Result<Json<Ticket>> {
    let ticket;
    {
        let _gate = s.ticket_gate.lock().await;
        ticket = find(&s, id).await?;
        access.ticket(&ticket)?;
        pending(&ticket)?;
        if ticket.kind == TicketKind::Hire {
            caller.person_approval(&s, &ticket).await?;
            if let Some(draft) = &ticket.hire {
                s.agents.put_proposal(
                    &crate::tickets::proposal_key(draft),
                    "declined",
                    chrono::Utc::now(),
                )?;
            }
        }
        let reason = s
            .registry
            .redact(Value::String(
                request.reason.unwrap_or_else(|| "rejected".into()),
            ))?
            .as_str()
            .unwrap_or("rejected")
            .to_owned();
        crate::tickets::close_unanswered(&s, &ticket, Some(&reason)).await?;
    }
    resume(&s, &ticket).await;
    Ok(Json(find(&s, ticket.id.as_str().into()).await?))
}
async fn resume(s: &Shared, ticket: &Ticket) {
    // Persistence is the commit point; transient runtime failure cannot turn a
    // successful human response into a retry that inserts a secret twice.
    let _ = s
        .command(&ticket.session_id, Action::Resume(ticket.id.clone()))
        .await;
}
async fn pair(
    State(s): State<Shared>,
    Json(request): Json<PairDeviceRequest>,
) -> Result<(StatusCode, Json<PairDeviceResponse>)> {
    let (device_id, token) =
        s.db.pair_local(request.name.clone(), request.scopes.clone(), request.kind)
            .await?;
    crate::pairing::audit(
        &s,
        "device",
        "paired",
        json!({"device_id": device_id, "name": request.name, "scopes": request.scopes, "via": "socket"}),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(PairDeviceResponse {
            device_id,
            token,
            scopes: request.scopes,
            kind: request.kind,
            import_secret: None,
        }),
    ))
}
async fn revoke(State(s): State<Shared>, Path(id): Path<String>) -> Result<StatusCode> {
    crate::pairing::revoke(&s, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resolve a ticket with a string value exactly as a `human_step` client would, for
/// daemon-side helpers (OTP ingest). The value is zeroized by the secret-input path.
pub(crate) async fn resolve_as_human_step(
    s: &Shared,
    id: &str,
    value: Zeroizing<String>,
) -> Result<Json<Ticket>> {
    resolve(
        State(s.clone()),
        Extension(Access(vec![CapabilityScope::HumanStep])),
        // A plain code only: nothing is registered, so no approver is recorded.
        Extension(Caller(None, DeviceKind::AgentClient)),
        Path(id.to_owned()),
        Json(ResolveTicketRequest {
            hire: None,
            value: Value::String(value.as_str().to_owned()),
        }),
    )
    .await
}

#[cfg(test)]
mod storage_full_tests {
    use super::*;

    #[test]
    fn a_full_volume_is_507_storage_full_and_other_failures_stay_conflicts() {
        let full = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            None,
        );
        let sqlite: Error = Box::new(openside_sessions::Error::Sqlite(full));
        let error = HttpError::from(sqlite);
        assert_eq!(
            (error.0, error.1),
            (StatusCode::INSUFFICIENT_STORAGE, "storage_full")
        );

        #[cfg(unix)]
        {
            let io: Error = Box::new(std::io::Error::from_raw_os_error(28));
            let error = HttpError::from(io);
            assert_eq!(
                (error.0, error.1),
                (StatusCode::INSUFFICIENT_STORAGE, "storage_full")
            );
        }

        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            None,
        );
        let other: Error = Box::new(openside_sessions::Error::Sqlite(busy));
        assert_eq!(HttpError::from(other).0, StatusCode::CONFLICT);
        assert_eq!(
            HttpError::from(Error::from("session actor closed")).0,
            StatusCode::CONFLICT
        );
    }
}
