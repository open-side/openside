//! A paired person's device asking for more scopes, answered on the computer running openside.
//!
//! The device posts `POST /v1/devices/me/access-requests` and polls `GET /v1/devices/me`; the
//! person sees the request in the desktop UI (`GET /v1/access-requests`) and allows or declines
//! it. Allowing needs the same local-person authority as issuing a pairing code, so a phone
//! that was paired remotely can ask but never grant. Requests live in memory for
//! [`ACCESS_REQUEST_TTL_SECS`] of monotonic time: a daemon restart drops them and the device
//! simply asks again.

use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_proto::{
    AccessRequest, AccessRequestList, Device, DeviceKind, MyDevice, RequestAccessRequest,
    ACCESS_REQUEST_TTL_SECS,
};
use parking_lot::Mutex;
use serde_json::json;

use crate::{
    api::{Access, Caller, HttpError},
    engine::Shared,
    pairing::audit,
    persistence::random_id,
    remote::LocalAuthority,
};

struct Entry {
    shown: AccessRequest,
    deadline: Instant,
}

/// Open requests, at most one per device.
#[derive(Default)]
pub(crate) struct Pending(Mutex<Vec<Entry>>);

impl Pending {
    fn open(&self, now: Instant) -> Vec<AccessRequest> {
        let mut entries = self.0.lock();
        entries.retain(|entry| entry.deadline > now);
        entries.iter().map(|entry| entry.shown.clone()).collect()
    }
}

fn caller_device(caller: &Caller) -> Result<&str, HttpError> {
    caller.0.as_deref().ok_or_else(HttpError::not_found)
}

async fn device_record(s: &Shared, id: &str) -> Result<Device, HttpError> {
    s.db.devices()
        .await
        .map_err(|_| HttpError::internal())?
        .into_iter()
        .find(|device| device.device_id == id)
        .ok_or_else(HttpError::not_found)
}

pub(crate) async fn mine(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<MyDevice>, HttpError> {
    let id = caller_device(&caller)?;
    let device = device_record(&s, id).await?;
    let request = s
        .remote
        .access
        .open(s.remote.instant())
        .into_iter()
        .find(|request| request.device_id == id);
    Ok(Json(MyDevice {
        device_id: device.device_id,
        name: device.name,
        kind: device.kind,
        scopes: device.scopes,
        request,
    }))
}

pub(crate) async fn request(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(access): Extension<Access>,
    Json(body): Json<RequestAccessRequest>,
) -> Result<(StatusCode, Json<AccessRequest>), HttpError> {
    let id = caller_device(&caller)?.to_owned();
    if caller.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_approval_required"));
    }
    let mut scopes = body.scopes;
    scopes.sort_by_key(|scope| *scope as u8);
    scopes.dedup();
    if scopes.is_empty() || scopes.iter().all(|scope| access.has(*scope)) {
        return Err(HttpError::invalid_value());
    }
    let device = device_record(&s, &id).await?;
    let now = s.remote.instant();
    let shown = AccessRequest {
        id: random_id(),
        device_id: id.clone(),
        name: device.name,
        scopes: scopes.clone(),
        created_at: s.remote.wall(),
        expires_at: s.remote.wall() + chrono::Duration::seconds(ACCESS_REQUEST_TTL_SECS),
    };
    {
        let mut entries = s.remote.access.0.lock();
        entries.retain(|entry| entry.deadline > now && entry.shown.device_id != id);
        entries.push(Entry {
            shown: shown.clone(),
            deadline: now + Duration::from_secs(ACCESS_REQUEST_TTL_SECS.unsigned_abs()),
        });
    }
    audit(
        &s,
        "device",
        "access_requested",
        json!({"device_id": id, "scopes": scopes}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(shown)))
}

pub(crate) async fn list(State(s): State<Shared>) -> Json<AccessRequestList> {
    Json(AccessRequestList {
        requests: s.remote.access.open(s.remote.instant()),
    })
}

/// Removes the open request `id`, or 404 when it expired or was already decided.
fn take(s: &Shared, id: &str) -> Result<AccessRequest, HttpError> {
    let now = s.remote.instant();
    let mut entries = s.remote.access.0.lock();
    entries.retain(|entry| entry.deadline > now);
    let position = entries
        .iter()
        .position(|entry| entry.shown.id == id)
        .ok_or_else(HttpError::not_found)?;
    Ok(entries.remove(position).shown)
}

pub(crate) async fn approve(
    State(s): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<Device>, HttpError> {
    local.require()?;
    let request = take(&s, &id)?;
    let device = device_record(&s, &request.device_id).await?;
    let mut scopes = device.scopes;
    scopes.extend(request.scopes.iter().copied());
    scopes.sort_by_key(|scope| *scope as u8);
    scopes.dedup();
    let existed: bool =
        s.db.set_scopes(request.device_id.clone(), scopes)
            .await
            .map_err(|_| HttpError::internal())?;
    if !existed {
        return Err(HttpError::not_found());
    }
    audit(
        &s,
        "device",
        "access_granted",
        json!({"device_id": request.device_id, "scopes": request.scopes, "by": caller.id()}),
    )
    .await;
    Ok(Json(device_record(&s, &request.device_id).await?))
}

pub(crate) async fn deny(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    let request = take(&s, &id)?;
    audit(
        &s,
        "device",
        "access_declined",
        json!({"device_id": request.device_id, "scopes": request.scopes, "by": caller.id()}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
