//! QR pairing and device administration (F-3, WS-5).
//!
//! `POST /v1/pairing` (Unix socket only) issues a one-time code valid for
//! [`PAIRING_TTL_SECS`] of monotonic time; the UTC `expires_at` is for display and audit
//! only, so moving the wall clock never extends a code. Only the code's SHA-256 is kept, in
//! memory: a daemon restart voids every open offer. `POST /v1/pairing/exchange` (no bearer
//! token) consumes the code on its first attempt and mints a scoped device token whose hash
//! alone is stored. A known, unexpired code always works. Malformed and unknown codes are
//! refused and audited; past [`MAX_FAILURES`] of them per minute, further bad attempts get
//! 429 without an audit line each, so guessing cannot lock out a real code or flood the log.
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

use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use openside_proto::{
    CapabilityScope, CreatePairingRequest, DeviceList, ExchangePairingRequest, PairDeviceResponse,
    PairingOffer, PAIRING_TTL_SECS,
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
    deadline: Instant,
}

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
    Json(request): Json<CreatePairingRequest>,
) -> Result<(StatusCode, Json<PairingOffer>), HttpError> {
    let mut scopes = request.scopes;
    scopes.sort_by_key(|scope| *scope as u8);
    scopes.dedup();
    if !valid(&request.name, &scopes) {
        return Err(HttpError::invalid_value());
    }
    let now = s.remote.instant();
    let expires_at = s.remote.wall() + chrono::Duration::seconds(PAIRING_TTL_SECS);
    let code = format!("{}{}", random_id(), random_id());
    {
        let mut offers = s.remote.offers.lock();
        offers.retain(|_, offer| offer.deadline > now);
        offers.insert(
            digest(&code),
            Offer {
                name: request.name.clone(),
                scopes: scopes.clone(),
                deadline: now + Duration::from_secs(PAIRING_TTL_SECS.unsigned_abs()),
            },
        );
    }
    audit(
        &s,
        "device",
        "pairing_issued",
        json!({"name": request.name, "scopes": scopes, "expires_at": expires_at}),
    )
    .await;
    let path = format!("/pair#{code}");
    let url = s
        .config
        .loaded
        .config
        .remote
        .public_url
        .as_deref()
        .map(|base| format!("{}{path}", base.trim_end_matches('/')));
    Ok((
        StatusCode::CREATED,
        Json(PairingOffer {
            code,
            path,
            url,
            scopes,
            expires_at,
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
    Json(request): Json<ExchangePairingRequest>,
) -> Result<(StatusCode, Json<PairDeviceResponse>), HttpError> {
    let now = s.remote.instant();
    let limited = || HttpError(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts");
    if !well_formed(&request.code) {
        return Err(if bad_attempt(&s, now, "malformed").await {
            HttpError(StatusCode::BAD_REQUEST, "invalid_code")
        } else {
            limited()
        });
    }
    let offer = s.remote.offers.lock().remove(&digest(&request.code));
    let Some(offer) = offer else {
        return Err(if bad_attempt(&s, now, "unknown").await {
            HttpError(StatusCode::NOT_FOUND, "pairing_not_found")
        } else {
            limited()
        });
    };
    if now >= offer.deadline {
        audit(&s, "device", "pairing_expired", json!({"name": offer.name})).await;
        return Err(HttpError(StatusCode::GONE, "pairing_expired"));
    }
    let (device_id, token) =
        s.db.pair_named(offer.name.clone(), offer.scopes.clone())
            .await
            .map_err(|_| HttpError::internal())?;
    audit(
        &s,
        "device",
        "paired",
        json!({"device_id": device_id, "name": offer.name, "scopes": offer.scopes, "via": "pairing"}),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(PairDeviceResponse {
            device_id,
            token,
            scopes: offer.scopes,
        }),
    ))
}

pub(crate) async fn list(State(s): State<Shared>) -> Result<Json<DeviceList>, HttpError> {
    Ok(Json(DeviceList {
        devices: s.db.devices().await.map_err(|_| HttpError::internal())?,
    }))
}

/// Finish admitted commits before deleting the token and durably clearing subscriptions.
/// The detached owner makes self-revoke and a dropped DELETE waiter safe.
pub(crate) async fn revoke(s: &Shared, device_id: &str) -> Result<(), HttpError> {
    let (s, device_id) = (s.clone(), device_id.to_owned());
    tokio::spawn(async move {
        s.remote.revoked_ids.lock().insert(device_id.clone());
        let _ = s.remote.revoked.send(device_id.clone());
        let gate = s.remote.device_gate(&device_id);
        let _commit = gate.write().await;
        let existed =
            s.db.revoke(device_id.clone())
                .await
                .map_err(|_| HttpError::internal())?;
        crate::push::forget_device(&s, &device_id).await?;
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
