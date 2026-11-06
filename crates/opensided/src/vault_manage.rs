//! Password settings and the person-only bulk steps on saved sign-ins: settings (auto-lock,
//! biometric unlock, autofill rules, default policy), biometric availability, CSV export,
//! clearing every sign-in, recovery keys and recovery. Export and recovery keys release
//! values, so they need the local socket, an explicit confirmation and fresh
//! authentication; their responses are never cached. Auto-lock runs here too.

use std::time::Duration;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{TimeZone, Utc};
use openside_config::{VaultAutoLock as AutoLock, VaultBackend, VaultPolicySetting};
use openside_proto::{
    DeviceKind, UpdateVaultSettingsRequest, VaultAutoLock, VaultAutofillSettings, VaultBiometrics,
    VaultClearResult, VaultConfirmRequest, VaultPolicy, VaultRecoverRequest, VaultRecoveryStatus,
    VaultSettings, VaultStatus,
};
use openside_vault::{AuditAction, BiometricError, Error, Secret};

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    remote::LocalAuthority,
    tools::vault::{lifecycle_audit, wire_policy, LiveSettings, Service, UnlockRefusal},
    vault_api::{audit_context, lifecycle, value_body, who, MIN_PASSPHRASE_CHARS},
};

/// How often auto-lock looks at the idle time; also how late "immediately" can be.
const AUTO_LOCK_TICK: Duration = Duration::from_secs(5);

/// Start the auto-lock loop; it ends with the daemon.
pub(crate) fn start(shared: &Shared) {
    let shared = shared.clone();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(AUTO_LOCK_TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = interval.tick() => { auto_lock(&shared).await; }
            }
        }
    });
}

/// Lock the vault if it has been idle for the auto-lock time, with the same credential
/// serialization as a person's lock. True when it locked.
pub(crate) async fn auto_lock(s: &Shared) -> bool {
    let service = &s.registry.vault;
    if !service.idle_due(s) {
        return false;
    }
    let channel_change = s.channels.change_credentials().await;
    let change = s.models.change_credentials().await;
    let locked = service.lock_if_idle(s).await;
    drop(change);
    drop(channel_change);
    if locked {
        s.models.invalidate(s).await;
        crate::diag!("opensided: vault locked after its auto-lock idle time");
    }
    locked
}

fn wire_auto_lock(value: AutoLock) -> VaultAutoLock {
    match value {
        AutoLock::Immediately => VaultAutoLock::Immediately,
        AutoLock::OneMinute => VaultAutoLock::OneMinute,
        AutoLock::FiveMinutes => VaultAutoLock::FiveMinutes,
        AutoLock::FifteenMinutes => VaultAutoLock::FifteenMinutes,
        AutoLock::OneHour => VaultAutoLock::OneHour,
        AutoLock::FourHours => VaultAutoLock::FourHours,
        AutoLock::OneDay => VaultAutoLock::OneDay,
        AutoLock::OneWeek => VaultAutoLock::OneWeek,
        AutoLock::Never => VaultAutoLock::Never,
    }
}

fn config_auto_lock(value: VaultAutoLock) -> AutoLock {
    match value {
        VaultAutoLock::Immediately => AutoLock::Immediately,
        VaultAutoLock::OneMinute => AutoLock::OneMinute,
        VaultAutoLock::FiveMinutes => AutoLock::FiveMinutes,
        VaultAutoLock::FifteenMinutes => AutoLock::FifteenMinutes,
        VaultAutoLock::OneHour => AutoLock::OneHour,
        VaultAutoLock::FourHours => AutoLock::FourHours,
        VaultAutoLock::OneDay => AutoLock::OneDay,
        VaultAutoLock::OneWeek => AutoLock::OneWeek,
        VaultAutoLock::Never => AutoLock::Never,
    }
}

fn config_policy(value: VaultPolicy) -> VaultPolicySetting {
    match value {
        VaultPolicy::Always => VaultPolicySetting::Always,
        VaultPolicy::WhileUnlocked => VaultPolicySetting::WhileUnlocked,
        VaultPolicy::Never => VaultPolicySetting::Never,
    }
}

/// Higher is stricter.
fn strictness(policy: VaultPolicySetting) -> u8 {
    match policy {
        VaultPolicySetting::Always => 0,
        VaultPolicySetting::WhileUnlocked => 1,
        VaultPolicySetting::Never => 2,
    }
}

/// The unlock method of the open vault, else the one the file would use.
async fn backend(s: &Shared) -> Option<VaultBackend> {
    match s.registry.vault.backend() {
        Some(backend) => Some(backend),
        None => Service::probe(s).await.backend.ok(),
    }
}

async fn biometric_view(s: &Shared) -> VaultBiometrics {
    let probe = s.registry.vault.biometric_probe().await;
    let passphrase = backend(s).await == Some(VaultBackend::Passphrase);
    match probe {
        Ok(mechanism) => VaultBiometrics {
            available: !passphrase,
            mechanism: Some(mechanism.wire().to_owned()),
            reason: passphrase.then(|| "passphrase_vault".to_owned()),
        },
        Err(error) => VaultBiometrics {
            available: false,
            mechanism: None,
            reason: Some(error.wire().to_owned()),
        },
    }
}

fn recovery_view(s: &Shared) -> VaultRecoveryStatus {
    match openside_vault::recovery_info(Service::file(s)) {
        Ok(Some(info)) => VaultRecoveryStatus {
            configured: true,
            current: info.current,
            created_at: i64::try_from(info.created_at)
                .ok()
                .and_then(|secs| Utc.timestamp_opt(secs, 0).single()),
        },
        _ => VaultRecoveryStatus {
            configured: false,
            current: false,
            created_at: None,
        },
    }
}

async fn view(s: &Shared, settings: &LiveSettings) -> VaultSettings {
    VaultSettings {
        auto_lock: wire_auto_lock(settings.auto_lock),
        biometric_unlock: settings.biometric_unlock,
        biometrics: biometric_view(s).await,
        autofill: VaultAutofillSettings {
            enabled: settings.autofill.enabled,
            auto_submit: settings.autofill.auto_submit,
            allow_insecure: settings.autofill.allow_insecure,
            excluded_domains: settings.autofill.excluded_domains.clone(),
        },
        default_policy: wire_policy(settings.default_policy),
        disable_in_incognito: true,
        recovery: recovery_view(s),
    }
}

pub(crate) async fn settings(State(s): State<Shared>) -> Json<VaultSettings> {
    let settings = s.registry.vault.settings(&s);
    Json(view(&s, &settings).await)
}

pub(crate) async fn biometrics(State(s): State<Shared>) -> Json<VaultBiometrics> {
    Json(biometric_view(&s).await)
}

pub(crate) async fn update_settings(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    Json(request): Json<UpdateVaultSettingsRequest>,
) -> Result<Json<VaultSettings>, HttpError> {
    // The file rewrite and the live swap must not be split by a dropped connection.
    tokio::spawn(update_owned(s, caller, local, request))
        .await
        .map_err(|_| HttpError::internal())?
}

/// The settings `request` asks for, and the names of the keys it changes.
fn apply(
    current: &LiveSettings,
    request: UpdateVaultSettingsRequest,
) -> Result<(LiveSettings, Vec<&'static str>), HttpError> {
    let mut next = current.clone();
    if request.disable_in_incognito == Some(false) {
        return Err(HttpError(
            StatusCode::BAD_REQUEST,
            "incognito_always_disabled",
        ));
    }
    if let Some(value) = request.auto_lock {
        next.auto_lock = config_auto_lock(value);
    }
    if let Some(value) = request.biometric_unlock {
        next.biometric_unlock = value;
    }
    if let Some(value) = request.default_policy {
        next.default_policy = config_policy(value);
    }
    if let Some(autofill) = request.autofill {
        if let Some(value) = autofill.enabled {
            next.autofill.enabled = value;
        }
        if let Some(value) = autofill.auto_submit {
            next.autofill.auto_submit = value;
        }
        if let Some(value) = autofill.allow_insecure {
            next.autofill.allow_insecure = value;
        }
        if let Some(domains) = autofill.excluded_domains {
            if domains.len() > openside_config::MAX_EXCLUDED_DOMAINS {
                return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_domain"));
            }
            let mut normalized = Vec::with_capacity(domains.len());
            for domain in &domains {
                let domain = openside_vault::normalize_domain(domain)
                    .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_domain"))?;
                if !normalized.contains(&domain) {
                    normalized.push(domain);
                }
            }
            next.autofill.excluded_domains = normalized;
        }
    }
    let mut changed = Vec::new();
    for (key, differs) in [
        ("auto_lock", current.auto_lock != next.auto_lock),
        (
            "biometric_unlock",
            current.biometric_unlock != next.biometric_unlock,
        ),
        (
            "default_policy",
            current.default_policy != next.default_policy,
        ),
        (
            "autofill.enabled",
            current.autofill.enabled != next.autofill.enabled,
        ),
        (
            "autofill.auto_submit",
            current.autofill.auto_submit != next.autofill.auto_submit,
        ),
        (
            "autofill.allow_insecure",
            current.autofill.allow_insecure != next.autofill.allow_insecure,
        ),
        (
            "autofill.excluded_domains",
            current.autofill.excluded_domains != next.autofill.excluded_domains,
        ),
    ] {
        if differs {
            changed.push(key);
        }
    }
    Ok((next, changed))
}

/// Whether `next` gives the vault less protection than `current` in any way.
fn loosens(current: &LiveSettings, next: &LiveSettings) -> bool {
    let idle = |value: AutoLock| value.idle().unwrap_or(Duration::MAX);
    idle(next.auto_lock) > idle(current.auto_lock)
        || (current.biometric_unlock && !next.biometric_unlock)
        || strictness(next.default_policy) < strictness(current.default_policy)
        || (!current.autofill.enabled && next.autofill.enabled)
        || (!current.autofill.auto_submit && next.autofill.auto_submit)
        || (!current.autofill.allow_insecure && next.autofill.allow_insecure)
        || current
            .autofill
            .excluded_domains
            .iter()
            .any(|domain| !next.autofill.excluded_domains.contains(domain))
}

async fn update_owned(
    s: Shared,
    caller: Caller,
    local: LocalAuthority,
    request: UpdateVaultSettingsRequest,
) -> Result<Json<VaultSettings>, HttpError> {
    if request == UpdateVaultSettingsRequest::default() {
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
    let service = &s.registry.vault;
    let current = service.settings(&s);
    let (next, changed) = apply(&current, request)?;
    if changed.is_empty() {
        return Ok(Json(view(&s, &current).await));
    }
    // An agent's device may only tighten; loosening is a person's decision.
    if loosens(&current, &next) && !local.0 && caller.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_access_required"));
    }
    if next.biometric_unlock && !current.biometric_unlock {
        // Only someone at this computer can touch its sensor.
        local.require()?;
        if backend(&s)
            .await
            .map_or(true, |backend| !backend.is_keyring())
        {
            return Err(HttpError(StatusCode::CONFLICT, "biometric_unavailable"));
        }
        if service.biometric_probe().await.is_err() {
            return Err(HttpError(StatusCode::CONFLICT, "biometric_unavailable"));
        }
        if service
            .biometric_verify("turn on biometric unlock for your saved passwords")
            .await
            .is_err()
        {
            return Err(HttpError(StatusCode::FORBIDDEN, "biometric_failed"));
        }
    }
    let mut config = crate::settings_api::stored(&s).await?;
    next.apply_to(&mut config.vault);
    let document = serde_json::to_value(&config).map_err(|_| HttpError::internal())?;
    crate::settings_api::persist(&s, document).await?;
    service.replace_settings(next.clone()).await;
    let detail = changed.join(",");
    lifecycle_audit(
        &s,
        &who(&caller),
        AuditAction::SettingsUpdate,
        "updated",
        Some(&detail),
    )
    .map_err(|()| {
        HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "settings_saved_audit_incomplete",
        )
    })?;
    Ok(Json(view(&s, &next).await))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn throttled(seconds: u64) -> Response {
    let mut response =
        HttpError(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts").into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, seconds.max(1).into());
    response
}

// Refusals are already the finished response, built once on a cold path, so their size costs
// nothing worth a Box allocation (same for `reauthenticate` and `authenticated`).
#[allow(clippy::result_large_err)]
fn confirm_body(body: &Bytes) -> Result<VaultConfirmRequest, Response> {
    let request: VaultConfirmRequest = if body.is_empty() {
        VaultConfirmRequest::default()
    } else {
        value_body(body).map_err(IntoResponse::into_response)?
    };
    if !request.confirm {
        return Err(HttpError(StatusCode::BAD_REQUEST, "confirmation_required").into_response());
    }
    Ok(request)
}

/// Fresh authentication for a step that releases values: the passphrase re-entered (a
/// passphrase vault), a biometric check, or a person's unlock on this computer within the
/// last few minutes. A keyring vault without biometric unlock has nothing more to ask than
/// the local socket and the confirmation, exactly as its unlock does. Refusals are audited
/// under `action`.
#[allow(clippy::result_large_err)]
async fn reauthenticate(
    s: &Shared,
    who: &str,
    action: AuditAction,
    request: &mut VaultConfirmRequest,
) -> Result<(), Response> {
    let service = &s.registry.vault;
    let denied = |detail: &'static str| {
        let _ = lifecycle_audit(s, who, action, "denied", Some(detail));
    };
    let backend = service.backend();
    if let Some(passphrase) = request.passphrase.take() {
        let passphrase = Secret::new(passphrase);
        if backend != Some(VaultBackend::Passphrase) {
            return Err(HttpError(StatusCode::BAD_REQUEST, "passphrase_not_used").into_response());
        }
        return match service.check_passphrase(s, who, &passphrase).await {
            Ok(true) => Ok(()),
            Ok(false) => {
                denied("wrong_passphrase");
                Err(HttpError(StatusCode::BAD_REQUEST, "wrong_passphrase").into_response())
            }
            Err(UnlockRefusal::Throttled(seconds)) => Err(throttled(seconds)),
            Err(UnlockRefusal::Failed(error)) => Err(lifecycle(error).into_response()),
        };
    }
    if request.biometric {
        // A passphrase vault is proven by its passphrase; biometrics only stand in for the
        // keyring's silent unlock.
        if !backend.is_some_and(VaultBackend::is_keyring)
            || service.biometric_probe().await.is_err()
        {
            return Err(HttpError(StatusCode::CONFLICT, "biometric_unavailable").into_response());
        }
        return match service.biometric_verify("confirm it is you").await {
            Ok(()) => Ok(()),
            Err(error) => {
                denied(match error {
                    BiometricError::Timeout => "biometric_timeout",
                    _ => "biometric_failed",
                });
                Err(HttpError(StatusCode::FORBIDDEN, "biometric_failed").into_response())
            }
        };
    }
    let keyring = backend.is_some_and(VaultBackend::is_keyring);
    if (keyring && !service.settings(s).biometric_unlock) || service.recently_opened(s) {
        return Ok(());
    }
    denied("reauth_required");
    Err(HttpError(StatusCode::CONFLICT, "reauth_required").into_response())
}

fn unavailable() -> Response {
    HttpError(StatusCode::CONFLICT, "vault_unavailable").into_response()
}

/// Whether the calling device was revoked after the middleware let the request in; checked
/// again right before a value-releasing or destructive step.
fn revoked(s: &Shared, caller: &Caller) -> bool {
    caller
        .0
        .as_deref()
        .is_some_and(|id| s.remote.is_revoked(id))
}

/// [`reauthenticate`], returning which opening of the vault it vouched for.
#[allow(clippy::result_large_err)]
async fn authenticated(
    s: &Shared,
    who: &str,
    action: AuditAction,
    request: &mut VaultConfirmRequest,
) -> Result<u64, Response> {
    let opening = s.registry.vault.opening();
    reauthenticate(s, who, action, request).await?;
    #[cfg(test)]
    let pause = s.registry.vault.after_reauth.lock().take();
    #[cfg(test)]
    if let Some((reached, resume)) = pause {
        reached.notify_one();
        resume.notified().await;
    }
    Ok(opening)
}

/// Checked with the vault held, right before a value-releasing or destructive step: the
/// calling device was revoked after the middleware let it in (401), or the vault was locked
/// and opened again after the authentication, which then no longer counts (409).
fn stale(
    s: &Shared,
    who: &str,
    action: AuditAction,
    caller: &Caller,
    opening: u64,
) -> Option<Response> {
    if revoked(s, caller) {
        return Some(HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response());
    }
    if s.registry.vault.opening() != opening {
        let _ = lifecycle_audit(s, who, action, "denied", Some("reauth_required"));
        return Some(HttpError(StatusCode::CONFLICT, "reauth_required").into_response());
    }
    None
}

pub(crate) async fn export(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    body: Bytes,
) -> Response {
    if let Err(error) = local.require() {
        return error.into_response();
    }
    let mut request = match confirm_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let who = who(&caller);
    if !s.registry.vault.available() {
        return unavailable();
    }
    let opening = match authenticated(&s, &who, AuditAction::Export, &mut request).await {
        Ok(opening) => opening,
        Err(response) => return response,
    };
    let guard = s.registry.vault.vault.lock().await;
    let Some(vault) = guard.as_ref() else {
        return unavailable();
    };
    if let Some(response) = stale(&s, &who, AuditAction::Export, &caller, opening) {
        return response;
    }
    s.registry.vault.touch(&s);
    let exported = match vault.export_csv(&audit_context(&who)) {
        Ok(exported) => exported,
        Err(Error::Locked) => return unavailable(),
        Err(Error::Audit | Error::AuditIncomplete) => {
            return HttpError(StatusCode::INTERNAL_SERVER_ERROR, "audit_unavailable")
                .into_response()
        }
        Err(_) => return HttpError::internal().into_response(),
    };
    drop(guard);
    secret_json(&ExportBody {
        format: "csv",
        filename: format!(
            "openside-passwords-{}.csv",
            s.remote.wall().format("%Y-%m-%d")
        ),
        content: exported.csv.as_str(),
        exported: u32::try_from(exported.exported).unwrap_or(u32::MAX),
        skipped: u32::try_from(exported.skipped).unwrap_or(u32::MAX),
    })
}

/// [`openside_proto::VaultExport`] borrowing the zeroizing CSV, so the serialized body is
/// the only copy that is not wiped (it leaves with the response).
#[derive(serde::Serialize)]
struct ExportBody<'a> {
    format: &'static str,
    filename: String,
    content: &'a str,
    exported: u32,
    skipped: u32,
}

/// [`openside_proto::VaultRecoveryKey`] borrowing the zeroizing key text.
#[derive(serde::Serialize)]
struct RecoveryKeyBody<'a> {
    recovery_key: &'a str,
    created_at: chrono::DateTime<Utc>,
}

/// A no-store JSON response for a body that carries values.
fn secret_json<T: serde::Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => no_store(
            (
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )],
                bytes,
            )
                .into_response(),
        ),
        Err(_) => HttpError::internal().into_response(),
    }
}

/// Irreversible, so it takes the same ceremony as export: local socket, `confirm` and fresh
/// authentication.
pub(crate) async fn clear(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    body: Bytes,
) -> Response {
    if let Err(error) = local.require() {
        return error.into_response();
    }
    let mut request = match confirm_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let who = who(&caller);
    if !s.registry.vault.available() {
        return unavailable();
    }
    let opening = match authenticated(&s, &who, AuditAction::Clear, &mut request).await {
        Ok(opening) => opening,
        Err(response) => return response,
    };
    let mut guard = s.registry.vault.vault.lock().await;
    let Some(vault) = guard.as_mut() else {
        return unavailable();
    };
    if let Some(response) = stale(&s, &who, AuditAction::Clear, &caller, opening) {
        return response;
    }
    s.registry.vault.touch(&s);
    let error = match vault.clear_logins(&audit_context(&who)) {
        Ok(removed) => {
            return Json(VaultClearResult {
                removed: u32::try_from(removed).unwrap_or(u32::MAX),
            })
            .into_response()
        }
        Err(Error::Locked) => return unavailable(),
        Err(Error::AuditIncomplete) => HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "vault_cleared_audit_incomplete",
        ),
        Err(Error::Audit) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "audit_unavailable"),
        Err(_) => HttpError::internal(),
    };
    error.into_response()
}

pub(crate) async fn recovery_key(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    body: Bytes,
) -> Response {
    if let Err(error) = local.require() {
        return error.into_response();
    }
    let mut request = match confirm_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let who = who(&caller);
    if !s.registry.vault.available() {
        return unavailable();
    }
    let opening = match authenticated(&s, &who, AuditAction::RecoveryKey, &mut request).await {
        Ok(opening) => opening,
        Err(response) => return response,
    };
    let guard = s.registry.vault.vault.lock().await;
    let Some(vault) = guard.as_ref() else {
        return unavailable();
    };
    if let Some(response) = stale(&s, &who, AuditAction::RecoveryKey, &caller, opening) {
        return response;
    }
    s.registry.vault.touch(&s);
    let key = match vault.generate_recovery_key(&audit_context(&who)) {
        Ok(key) => key,
        Err(Error::Locked) => return unavailable(),
        Err(Error::Audit) => {
            return HttpError(StatusCode::INTERNAL_SERVER_ERROR, "audit_unavailable")
                .into_response()
        }
        Err(_) => return HttpError::internal().into_response(),
    };
    drop(guard);
    s.registry.redactors.write().push(key.redactor());
    let text = key.text();
    secret_json(&RecoveryKeyBody {
        recovery_key: text.as_str(),
        created_at: s.remote.wall(),
    })
}

pub(crate) async fn recover(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    body: Bytes,
) -> Response {
    if let Err(error) = local.require() {
        return error.into_response();
    }
    let request: VaultRecoverRequest = match value_body(&body) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    if request
        .passphrase
        .as_ref()
        .is_some_and(|value| value.chars().count() < MIN_PASSPHRASE_CHARS)
    {
        return HttpError(StatusCode::BAD_REQUEST, "weak_passphrase").into_response();
    }
    let recovery_key = zeroize::Zeroizing::new(request.recovery_key);
    let passphrase = request.passphrase.map(Secret::new);
    let service = &s.registry.vault;
    if revoked(&s, &caller) {
        return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let result = service
        .recover(&s, &who(&caller), &recovery_key, passphrase)
        .await;
    s.models.invalidate(&s).await;
    match result {
        Ok(()) => {
            crate::channels_api::refresh(&s).await;
            let status: VaultStatus = service.status(&s).await;
            Json(status).into_response()
        }
        Err(UnlockRefusal::Failed(error)) => lifecycle(error).into_response(),
        Err(UnlockRefusal::Throttled(seconds)) => throttled(seconds),
    }
}
