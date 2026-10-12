//! `/v1/vault/*`: lifecycle (status, setup, unlock, lock), adding and deleting items,
//! value-free item metadata, per-item policy overrides and audit query. Values (passphrase,
//! username, password, authenticator key) only ever arrive in request bodies; no handler can
//! return a username, password, TOTP seed/code, passphrase or passkey private key: responses
//! are built only from [`openside_vault::ItemInfo`], lifecycle state and audit records, and
//! malformed value-carrying bodies are refused without echoing them.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, TimeZone, Utc};
use openside_proto::{
    CreateVaultItemRequest, UpdateVaultItemRequest, VaultAuditList, VaultAuditRecord, VaultItem,
    VaultItemList, VaultPolicy, VaultPolicyOverride, VaultStatus, VaultUnlockRequest,
};
use openside_vault::{
    AccessPolicy, AllowedOrigin, AuditContext, AuditQuery, Entry, Error, ItemInfo, Secret, Totp,
};
use serde::Deserialize;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    tools::vault::{LifecycleError, Service, UnlockRefusal},
};

/// The vault audit label of the caller: `device:<id>` for a paired device, `local` for the
/// Unix socket.
fn who(caller: &Caller) -> String {
    crate::tools::otp::approver(caller.0.as_deref())
}

fn audit_context(who: &str) -> AuditContext<'_> {
    AuditContext {
        who,
        session_id: None,
        tool_call_id: None,
    }
}

const MIN_PASSPHRASE_CHARS: usize = 8;

/// Parse a body that may carry values; serde diagnostics can quote input, so they are
/// replaced by a fixed code.
fn value_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, HttpError> {
    serde_json::from_slice(body).map_err(|_| HttpError::invalid_value())
}

fn lifecycle((status, code): LifecycleError) -> HttpError {
    HttpError(
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        code,
    )
}

fn passphrase(request: VaultUnlockRequest) -> Option<Secret> {
    request.passphrase.map(Secret::new)
}

pub(crate) async fn status(State(s): State<Shared>) -> Json<VaultStatus> {
    Json(s.registry.vault.status(&s).await)
}

pub(crate) async fn setup(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> Result<Json<VaultStatus>, HttpError> {
    let request: VaultUnlockRequest = if body.is_empty() {
        VaultUnlockRequest::default()
    } else {
        value_body(&body)?
    };
    if request
        .passphrase
        .as_ref()
        .is_some_and(|value| value.chars().count() < MIN_PASSPHRASE_CHARS)
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "weak_passphrase"));
    }
    let service = &s.registry.vault;
    let mut guard = service.vault.lock().await;
    service
        .setup(&s, &mut guard, &who(&caller), passphrase(request))
        .await
        .map_err(lifecycle)?;
    drop(guard);
    Ok(Json(service.status(&s).await))
}

/// Wrong passphrases are throttled per caller (and across paired devices) before any key
/// derivation: `429 too_many_attempts` with `Retry-After` seconds.
pub(crate) async fn unlock(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> Response {
    let request: VaultUnlockRequest = if body.is_empty() {
        VaultUnlockRequest::default()
    } else {
        match value_body(&body) {
            Ok(request) => request,
            Err(error) => return error.into_response(),
        }
    };
    let service = &s.registry.vault;
    let remote = caller.0.is_some();
    match service
        .unlock_now(&s, &who(&caller), remote, passphrase(request))
        .await
    {
        Ok(()) => Json(service.status(&s).await).into_response(),
        Err(UnlockRefusal::Failed(error)) => lifecycle(error).into_response(),
        Err(UnlockRefusal::Throttled(seconds)) => {
            let mut response =
                HttpError(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts").into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, seconds.max(1).into());
            response
        }
    }
}

pub(crate) async fn lock(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<VaultStatus>, HttpError> {
    s.registry
        .vault
        .lock(&s, &who(&caller))
        .await
        .map_err(lifecycle)?;
    Ok(Json(s.registry.vault.status(&s).await))
}

/// Validate a new item before touching the vault, so a bad origin or key never creates the
/// vault file.
fn new_entry(request: CreateVaultItemRequest) -> Result<Entry, HttpError> {
    let bad = |code| HttpError(StatusCode::BAD_REQUEST, code);
    let origins = request
        .origins
        .iter()
        .map(|origin| AllowedOrigin::parse(origin.trim()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| bad("invalid_origin"))?;
    let password = request.password.filter(|value| !value.is_empty());
    let totp = request
        .totp
        .filter(|value| !value.trim().is_empty())
        .map(|value| Totp::parse(Secret::new(value)))
        .transpose()
        .map_err(|_| bad("invalid_totp"))?;
    if password.is_none() && totp.is_none() {
        return Err(bad("empty_item"));
    }
    Entry::with_policy_override(
        request.id,
        origins,
        Secret::new(request.username.unwrap_or_default()),
        Secret::new(password.unwrap_or_default()),
        totp,
        request.policy.map(|policy| match policy {
            VaultPolicy::Always => AccessPolicy::Always,
            VaultPolicy::WhileUnlocked => AccessPolicy::WhileUnlocked,
            VaultPolicy::Never => AccessPolicy::Never,
        }),
    )
    .map_err(|_| bad("invalid_item"))
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> Result<(StatusCode, Json<VaultItem>), HttpError> {
    let entry = new_entry(value_body(&body)?)?;
    let who = who(&caller);
    let service = &s.registry.vault;
    let mut guard = service.vault.lock().await;
    if guard.is_none() {
        // A keyring vault needs no secret to exist, so the first item creates it; a
        // passphrase vault must be set up (or unlocked) by a person first.
        let probe = Service::probe(&s).await;
        let keyring = probe.backend.is_ok_and(|backend| backend.is_keyring());
        if !keyring {
            return Err(unavailable());
        }
        service
            .setup(&s, &mut guard, &who, None)
            .await
            .map_err(|_| unavailable())?;
    }
    let vault = guard.as_mut().ok_or_else(unavailable)?;
    // Mask the new values in every later output before anything is saved, so no committed
    // outcome (including a save whose completion audit fails) leaves them unmasked.
    s.registry.redactors.write().push(entry.redactor());
    let info = match vault.add_item(&audit_context(&who), entry) {
        Ok(info) => info,
        Err(Error::ItemExists) => return Err(HttpError(StatusCode::CONFLICT, "item_exists")),
        Err(Error::Locked) => return Err(unavailable()),
        Err(Error::AuditIncomplete) => {
            return Err(HttpError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "item_added_audit_incomplete",
            ))
        }
        Err(_) => return Err(HttpError::internal()),
    };
    Ok((StatusCode::CREATED, Json(item(info))))
}

pub(crate) async fn delete(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    let who = who(&caller);
    let mut vault = s.registry.vault.vault.lock().await;
    let vault = vault.as_mut().ok_or_else(unavailable)?;
    match vault.delete_item(&audit_context(&who), &id) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(Error::UnknownItem) => Err(HttpError(StatusCode::NOT_FOUND, "unknown_item")),
        Err(Error::Locked) => Err(unavailable()),
        Err(Error::AuditIncomplete) => Err(HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "item_deleted_audit_incomplete",
        )),
        Err(_) => Err(HttpError::internal()),
    }
}

fn policy(policy: AccessPolicy) -> VaultPolicy {
    match policy {
        AccessPolicy::Always => VaultPolicy::Always,
        AccessPolicy::WhileUnlocked => VaultPolicy::WhileUnlocked,
        AccessPolicy::Never => VaultPolicy::Never,
    }
}

fn item(info: ItemInfo) -> VaultItem {
    VaultItem {
        id: info.site,
        origins: info.origins,
        policy: info.policy.map(policy),
        effective_policy: policy(info.effective_policy),
        has_totp: info.has_totp,
        passkey_rp_ids: info.passkey_rp_ids,
    }
}

fn unavailable() -> HttpError {
    HttpError(StatusCode::CONFLICT, "vault_unavailable")
}

pub(crate) async fn items(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
) -> std::result::Result<Json<VaultItemList>, HttpError> {
    let who = who(&caller);
    let vault = s.registry.vault.vault.lock().await;
    let Some(vault) = vault.as_ref() else {
        return Ok(Json(VaultItemList {
            available: false,
            default_policy: policy(AccessPolicy::WhileUnlocked),
            items: Vec::new(),
        }));
    };
    let items = vault
        .items(&audit_context(&who))
        .map_err(|_| HttpError::internal())?;
    Ok(Json(VaultItemList {
        available: true,
        default_policy: policy(vault.default_policy()),
        items: items.into_iter().map(item).collect(),
    }))
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<UpdateVaultItemRequest>,
) -> std::result::Result<Json<VaultItem>, HttpError> {
    let who = who(&caller);
    let mut vault = s.registry.vault.vault.lock().await;
    let vault = vault.as_mut().ok_or_else(unavailable)?;
    let override_policy = match request.policy {
        VaultPolicyOverride::Always => Some(AccessPolicy::Always),
        VaultPolicyOverride::WhileUnlocked => Some(AccessPolicy::WhileUnlocked),
        VaultPolicyOverride::Never => Some(AccessPolicy::Never),
        VaultPolicyOverride::Default => None,
    };
    match vault.set_policy(&audit_context(&who), &id, override_policy) {
        Ok(info) => Ok(Json(item(info))),
        Err(Error::UnknownItem) => Err(HttpError(StatusCode::NOT_FOUND, "unknown_item")),
        Err(Error::Locked) => Err(unavailable()),
        // The change is saved; only its completion audit record is missing.
        Err(Error::AuditIncomplete) => Err(HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "policy_applied_audit_incomplete",
        )),
        Err(_) => Err(HttpError::internal()),
    }
}

#[derive(Deserialize)]
pub(crate) struct AuditParams {
    since: Option<DateTime<Utc>>,
    item: Option<String>,
    session: Option<String>,
    limit: Option<usize>,
}

pub(crate) async fn audit(
    State(s): State<Shared>,
    Query(params): Query<AuditParams>,
) -> std::result::Result<Json<VaultAuditList>, HttpError> {
    let query = AuditQuery {
        since: params
            .since
            .map(|since| u64::try_from(since.timestamp()).unwrap_or(0)),
        site_id: params.item,
        session_id: params.session,
        limit: Some(params.limit.unwrap_or(200).clamp(1, 1000)),
    };
    let path = Service::audit_path(&s);
    let records = tokio::task::spawn_blocking(move || openside_vault::read_audit(path, &query))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|_| HttpError::internal())?;
    let records = records
        .into_iter()
        .map(|record| VaultAuditRecord {
            at: i64::try_from(record.when)
                .ok()
                .and_then(|when| Utc.timestamp_opt(when, 0).single())
                .unwrap_or_default(),
            who: record.who,
            what: record.what,
            item: record.site_id,
            origin: record.origin,
            result: record.decision,
            session_id: record.session_id,
            tool_call_id: record.tool_call_id,
            detail: record.detail,
        })
        .collect();
    // Audit lines are already redacted when written; this guards records written before a
    // secret (e.g. a newly registered passkey) was known to the redactor.
    let value =
        serde_json::to_value(VaultAuditList { records }).map_err(|_| HttpError::internal())?;
    let value = s
        .registry
        .redact(value)
        .map_err(|_| HttpError::internal())?;
    serde_json::from_value(value)
        .map(Json)
        .map_err(|_| HttpError::internal())
}
