//! QR pairing and device administration (F-3, WS-5).
//!
//! `POST /v1/pairing` (local socket or verified local-person browser) issues a one-time code valid for
//! [`PAIRING_TTL_SECS`] of monotonic time; the UTC `expires_at` is for display and audit
//! only, so moving the wall clock never extends a code. Only the code's SHA-256 is kept, in
//! memory: a daemon restart voids every open offer. `POST /v1/pairing/exchange` (no bearer
//! token) consumes the code on its first attempt and mints a scoped device token whose hash
//! alone is stored. A known, unexpired code always works. Codes bound to the openside
//! extension (import delivery, launcher pairing) are redeemable only with its Origin; a request
//! from another origin is refused as a bad attempt without consuming the code. Malformed and
//! unknown codes are refused and audited; past [`MAX_FAILURES`] bad attempts per minute,
//! further ones get 429 without an audit line each, so guessing cannot lock out a real code
//! or flood the log.
//!
//! Revocation marks the device and closes its streams before waiting for already-admitted
//! handlers to finish their commits. Body reception and push DNS/HTTP never hold the
//! device gate. Once admitted, a handler owns its gate through commit, audit and required
//! continuation even if its HTTP waiter disappears. Cleanup writes the subscription
//! file last, so no revoked-device snapshot can reappear after revoke returns.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use openside_proto::{
    CapabilityScope, CreatePairingRequest, DeviceList, ExchangePairingRequest, PairDeviceResponse,
    PairingOffer, RenameDeviceRequest, PAIRING_TTL_SECS,
};
use parking_lot::{Mutex, RwLock};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::{api::HttpError, engine::Shared, persistence::random_id};

type Mono = Arc<dyn Fn() -> Instant + Send + Sync>;
type Wall = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Malformed or unknown codes tolerated per rolling minute before further ones get 429.
pub(crate) const MAX_FAILURES: usize = 20;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

struct Offer {
    name: String,
    scopes: Vec<CapabilityScope>,
    kind: openside_proto::DeviceKind,
    deadline: Instant,
    import_delivery: bool,
    /// Issued to the desktop launcher for its browser's extension: redemption is bound to the
    /// extension's origin and replaces the previously launcher-paired extension device.
    launcher_extension: bool,
}

/// The pinned id of the openside extension (web/src/desktopNotifyProtocol.ts `EXTENSION_ID`).
pub(crate) const EXTENSION_ORIGIN: &str = "chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj";
const LAUNCHER_EXTENSION_NAME: &str = "openside extension";

/// In-memory remote-access state shared by pairing, push and OTP ingest.
pub(crate) struct Remote {
    offers: Mutex<HashMap<[u8; 32], Offer>>,
    failures: Mutex<VecDeque<Instant>>,
    revoked_ids: Mutex<HashSet<String>>,
    gates: Mutex<HashMap<String, Arc<tokio::sync::RwLock<()>>>>,
    /// Broadcast before waiting for admitted commits, so streams and pushes end promptly.
    pub(crate) revoked: broadcast::Sender<String>,
    mono: RwLock<Mono>,
    wall: RwLock<Wall>,
    pub(crate) push: crate::push::PushState,
    pub(crate) otp: crate::otp_ingest::Codes,
    pub(crate) access: crate::access_requests::Pending,
}

impl Default for Remote {
    fn default() -> Self {
        Self {
            offers: Mutex::default(),
            failures: Mutex::default(),
            revoked_ids: Mutex::default(),
            gates: Mutex::default(),
            revoked: broadcast::channel(64).0,
            mono: RwLock::new(Arc::new(Instant::now)),
            wall: RwLock::new(Arc::new(Utc::now)),
            push: Default::default(),
            otp: Default::default(),
            access: Default::default(),
        }
    }
}

impl Remote {
    /// Monotonic time for every in-memory deadline (pairing, OTP, push focus).
    pub(crate) fn instant(&self) -> Instant {
        let clock = self.mono.read().clone();
        clock()
    }
    /// Wall time, for values shown to people and written to the audit log.
    pub(crate) fn wall(&self) -> DateTime<Utc> {
        let clock = self.wall.read().clone();
        clock()
    }
    #[cfg(test)]
    pub(crate) fn set_clocks(
        &self,
        mono: impl Fn() -> Instant + Send + Sync + 'static,
        wall: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) {
        *self.mono.write() = Arc::new(mono);
        *self.wall.write() = Arc::new(wall);
    }
    /// Set (before the token is deleted) by revocation; device ids are never reused.
    pub(crate) fn is_revoked(&self, device_id: &str) -> bool {
        self.revoked_ids.lock().contains(device_id)
    }
    pub(crate) fn mark_revoked(&self, device_id: &str) {
        self.revoked_ids.lock().insert(device_id.to_owned());
        let _ = self.revoked.send(device_id.to_owned());
    }
    pub(crate) fn device_gate(&self, device_id: &str) -> Arc<tokio::sync::RwLock<()>> {
        // Keep gate identity stable even after revoke: an admitted waiter may still own it.
        let mut gates = self.gates.lock();
        if let Some(gate) = gates.get(device_id) {
            return gate.clone();
        }
        let gate = Arc::default();
        gates.insert(device_id.to_owned(), Arc::clone(&gate));
        gate
    }
}

/// Resolves once `device` is revoked. Subscribe `revocations` before checking
/// [`Remote::is_revoked`] so no revocation falls between the two.
pub(crate) async fn until_revoked(
    s: &Shared,
    revocations: &mut broadcast::Receiver<String>,
    device: &str,
) {
    loop {
        match revocations.recv().await {
            Ok(id) if id == device => return,
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if s.remote.is_revoked(device) {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

fn digest(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

pub(crate) async fn audit(s: &Shared, tool: &str, decision: &str, fields: serde_json::Value) {
    let mut record = json!({"ts": Utc::now(), "tool": tool, "decision": decision});
    if let (Some(record), Some(fields)) = (record.as_object_mut(), fields.as_object()) {
        record.extend(fields.clone());
    }
    if let Err(error) = crate::tickets::audit_record(s, record).await {
        crate::diag!("opensided: {tool} audit failed: {error}");
    }
}

fn valid(name: &str, scopes: &[CapabilityScope]) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 100 && !scopes.is_empty()
}

/// Codes are two `random_id`s: 64 lowercase hex digits.
fn well_formed(code: &str) -> bool {
    code.len() == 64
        && code
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(access): Extension<crate::api::Access>,
    Extension(local): Extension<crate::remote::LocalAuthority>,
    Json(request): Json<CreatePairingRequest>,
) -> Result<(StatusCode, Json<PairingOffer>), HttpError> {
    local.require()?;
    let mut scopes = request.scopes;
    scopes.sort_by_key(|scope| *scope as u8);
    scopes.dedup();
    if scopes.iter().any(|scope| !access.has(*scope)) {
        return Err(HttpError(StatusCode::FORBIDDEN, "scope_ceiling_exceeded"));
    }
    if !valid(&request.name, &scopes) {
        return Err(HttpError::invalid_value());
    }
    if request.import_delivery && request.kind != openside_proto::DeviceKind::PersonDevice {
        return Err(HttpError::invalid_value());
    }
    let code = format!("{}{}", random_id(), random_id());
    // Resolve the link first: an unavailable transport must not leave a live code behind.
    let (url, transport) = crate::remote::pairing_link(&s, request.via, &code)?;
    let now = s.remote.instant();
    let expires_at = s.remote.wall() + chrono::Duration::seconds(PAIRING_TTL_SECS);
    {
        let mut offers = s.remote.offers.lock();
        offers.retain(|_, offer| offer.deadline > now);
        offers.insert(
            digest(&code),
            Offer {
                name: request.name.clone(),
                scopes: scopes.clone(),
                kind: request.kind,
                import_delivery: request.import_delivery,
                launcher_extension: false,
                deadline: now + Duration::from_secs(PAIRING_TTL_SECS.unsigned_abs()),
            },
        );
    }
    audit(
        &s,
        "device",
        "pairing_issued",
        json!({"name": request.name, "scopes": scopes, "kind": request.kind, "expires_at": expires_at, "transport": transport}),
    )
    .await;
    let path = format!("/pair#{code}");
    Ok((
        StatusCode::CREATED,
        Json(PairingOffer {
            code,
            path,
            url,
            scopes,
            expires_at,
            transport,
            kind: request.kind,
        }),
    ))
}

/// Count a bad attempt; `false` once the minute's budget is spent (then: 429, no audit).
async fn bad_attempt(s: &Shared, now: Instant, reason: &'static str) -> bool {
    let within = {
        let mut failures = s.remote.failures.lock();
        while failures
            .front()
            .is_some_and(|at| now.duration_since(*at) >= FAILURE_WINDOW)
        {
            failures.pop_front();
        }
        let within = failures.len() < MAX_FAILURES;
        if within {
            failures.push_back(now);
        }
        within
    };
    if within {
        audit(s, "device", "pairing_failed", json!({"reason": reason})).await;
    }
    within
}

pub(crate) async fn exchange(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ExchangePairingRequest>,
) -> Result<Response, HttpError> {
    let now = s.remote.instant();
    let limited = || HttpError(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts");
    if !well_formed(&request.code) {
        return Err(if bad_attempt(&s, now, "malformed").await {
            HttpError(StatusCode::BAD_REQUEST, "invalid_code")
        } else {
            limited()
        });
    }
    let extension_origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(EXTENSION_ORIGIN);
    // Inspect and consume under one lock, so the code stays one-time. An expired code is spent;
    // one presented from the wrong origin is not, so a page cannot burn the extension's code.
    let key = digest(&request.code);
    enum Taken {
        Missing,
        WrongOrigin,
        Expired(Offer),
        Live(Offer),
    }
    let taken = {
        let mut offers = s.remote.offers.lock();
        let check = offers.get(&key).map(|offer| {
            (
                now >= offer.deadline,
                (offer.import_delivery || offer.launcher_extension) && !extension_origin,
            )
        });
        match check {
            None => Taken::Missing,
            Some((false, true)) => Taken::WrongOrigin,
            Some((expired, _)) => match offers.remove(&key) {
                Some(offer) if expired => Taken::Expired(offer),
                Some(offer) => Taken::Live(offer),
                None => Taken::Missing,
            },
        }
    };
    let offer = match taken {
        Taken::Live(offer) => offer,
        Taken::Expired(offer) => {
            audit(&s, "device", "pairing_expired", json!({"name": offer.name})).await;
            return Err(HttpError(StatusCode::GONE, "pairing_expired"));
        }
        Taken::Missing => {
            return Err(if bad_attempt(&s, now, "unknown").await {
                HttpError(StatusCode::NOT_FOUND, "pairing_not_found")
            } else {
                limited()
            })
        }
        Taken::WrongOrigin => {
            return Err(if bad_attempt(&s, now, "origin").await {
                HttpError(StatusCode::FORBIDDEN, "extension_origin_required")
            } else {
                limited()
            })
        }
    };
    let (device_id, token) = if offer.launcher_extension {
        pair_launcher_extension(&s, offer.name.clone(), offer.scopes.clone()).await?
    } else {
        s.db.pair_as(offer.name.clone(), offer.scopes.clone(), offer.kind)
            .await
            .map_err(|_| HttpError::internal())?
    };
    let import_secret = if offer.import_delivery {
        Some(crate::import_delivery::create_pairing_secret(&s, &device_id).await?)
    } else {
        None
    };
    audit(
        &s,
        "device",
        "paired",
        json!({"device_id": device_id, "name": offer.name, "scopes": offer.scopes, "kind": offer.kind, "via": if offer.launcher_extension { "launcher" } else { "pairing" }}),
    )
    .await;
    let response = PairDeviceResponse {
        device_id,
        token,
        scopes: offer.scopes,
        kind: offer.kind,
        import_secret,
    };
    let payload = serde_json::to_vec(&response).map_err(|_| HttpError::internal())?;
    let mut response_headers = axum::http::HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    response_headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    if offer.import_delivery {
        if let Some(proof) =
            crate::import_delivery::pairing_continuity_proof(&s, &headers, &payload).await?
        {
            response_headers.insert(
                "x-openside-import-proof",
                proof.parse().map_err(|_| HttpError::internal())?,
            );
        }
    }
    Ok((StatusCode::CREATED, response_headers, payload).into_response())
}

/// A one-time code for the extension in the desktop launcher's browser profile, handed over by
/// the launcher through a file inside the unpacked extension (never a page or a QR). Only
/// [`crate::remote::local_access`] issues it, so it carries the same local-socket proof.
pub(crate) async fn issue_launcher_extension_code(s: &Shared) -> String {
    let code = format!("{}{}", random_id(), random_id());
    let scopes = vec![
        CapabilityScope::Read,
        CapabilityScope::Steer,
        CapabilityScope::Approve,
        CapabilityScope::HumanStep,
    ];
    let now = s.remote.instant();
    let expires_at = s.remote.wall() + chrono::Duration::seconds(PAIRING_TTL_SECS);
    {
        let mut offers = s.remote.offers.lock();
        offers.retain(|_, offer| offer.deadline > now);
        offers.insert(
            digest(&code),
            Offer {
                name: LAUNCHER_EXTENSION_NAME.into(),
                scopes: scopes.clone(),
                kind: openside_proto::DeviceKind::PersonDevice,
                import_delivery: false,
                launcher_extension: true,
                deadline: now + Duration::from_secs(PAIRING_TTL_SECS.unsigned_abs()),
            },
        );
    }
    audit(
        s,
        "device",
        "pairing_issued",
        json!({"name": LAUNCHER_EXTENSION_NAME, "scopes": scopes, "kind": openside_proto::DeviceKind::PersonDevice, "expires_at": expires_at, "transport": "launcher"}),
    )
    .await;
    code
}

/// Replaces the previous launcher-paired extension device like `local_access` replaces the
/// launcher's own sign-in, so browser restarts never pile up stale tokens. The detached owner
/// keeps the swap whole even if the HTTP waiter disappears.
async fn pair_launcher_extension(
    s: &Shared,
    name: String,
    scopes: Vec<CapabilityScope>,
) -> Result<(String, String), HttpError> {
    let s = s.clone();
    tokio::spawn(async move {
        let _issue = s.transports.local_device.lock().await;
        let previous =
            s.db.launcher_extension_device()
                .await
                .map_err(|_| HttpError::internal())?;
        let _previous_gate = if let Some(device) = previous.as_deref() {
            s.remote.mark_revoked(device);
            Some(s.remote.device_gate(device).write_owned().await)
        } else {
            None
        };
        let paired =
            s.db.pair_launcher_extension(name, scopes)
                .await
                .map_err(|_| HttpError::internal())?;
        if let Some(previous) = previous.as_deref() {
            crate::push::forget_device(&s, previous).await?;
            crate::import_delivery::forget_device(&s, previous).await?;
            audit(
                &s,
                "device",
                "revoked",
                json!({"device_id": previous, "via": "launcher_rotation"}),
            )
            .await;
        }
        Ok(paired)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn list(State(s): State<Shared>) -> Result<Json<DeviceList>, HttpError> {
    Ok(Json(DeviceList {
        devices: s.db.devices().await.map_err(|_| HttpError::internal())?,
    }))
}

/// Admin scope, like revoke; the local socket renames any device. An agent-client token may
/// relabel only itself: it must not be able to disguise another device in the list a person
/// reviews before revoking.
pub(crate) async fn rename(
    State(s): State<Shared>,
    Extension(caller): Extension<crate::api::Caller>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(request): Json<RenameDeviceRequest>,
) -> Result<Json<openside_proto::Device>, HttpError> {
    let agent_token = caller.0.is_some() && caller.1 == openside_proto::DeviceKind::AgentClient;
    if agent_token && caller.0.as_deref() != Some(id.as_str()) {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_approval_required"));
    }
    let name = request.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(HttpError::invalid_value());
    }
    if !s
        .db
        .rename_device(id.clone(), name)
        .await
        .map_err(|_| HttpError::internal())?
    {
        return Err(HttpError(StatusCode::NOT_FOUND, "not_found"));
    }
    audit(&s, "device", "renamed", json!({"device_id": id})).await;
    let device =
        s.db.devices()
            .await
            .map_err(|_| HttpError::internal())?
            .into_iter()
            .find(|device| device.device_id == id)
            .ok_or(HttpError(StatusCode::NOT_FOUND, "not_found"))?;
    Ok(Json(device))
}

/// Finish admitted commits before deleting the token and durably clearing subscriptions.
/// The detached owner makes self-revoke and a dropped DELETE waiter safe.
pub(crate) async fn revoke(s: &Shared, device_id: &str) -> Result<(), HttpError> {
    let (s, device_id) = (s.clone(), device_id.to_owned());
    tokio::spawn(async move {
        s.remote.mark_revoked(&device_id);
        let gate = s.remote.device_gate(&device_id);
        let _commit = gate.write().await;
        let existed =
            s.db.revoke(device_id.clone())
                .await
                .map_err(|_| HttpError::internal())?;
        crate::push::forget_device(&s, &device_id).await?;
        crate::import_delivery::forget_device(&s, &device_id).await?;
        audit(
            &s,
            "device",
            "revoked",
            json!({"device_id": device_id, "existed": existed}),
        )
        .await;
        Ok(())
    })
    .await
    .map_err(|_| HttpError::internal())?
}
