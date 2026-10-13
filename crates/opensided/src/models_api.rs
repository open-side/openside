use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{TimeZone, Utc};
use openside_config::{Config, ModelsConfig};
use openside_proto::{
    CreateProviderKeyRequest, DeviceKind, ModelCatalog, ModelSelection, ProviderKeyInfo,
    ProviderKeyList, SetProviderKeyRequest, UpdateModelsRequest, UpdateProviderKeyRequest,
};
use openside_vault::{AuditContext, Entry, Error, Secret};
use parking_lot::RwLock;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock as AsyncRwLock, RwLockReadGuard, RwLockWriteGuard};
use zeroize::Zeroize;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

pub(crate) struct Service {
    live: RwLock<(ModelsConfig, Value)>,
    catalog: Mutex<Option<(Instant, u64, ModelCatalog)>>,
    credentials_generation: AtomicU64,
    default_generation: AtomicU64,
    startups: AsyncRwLock<()>,
    pub(crate) policy: Mutex<crate::model_accounts::Policy>,
    catalog_refresh: Mutex<()>,
}

pub(crate) struct CredentialChange<'a> {
    generation: &'a AtomicU64,
    _permit: RwLockWriteGuard<'a, ()>,
}

impl Drop for CredentialChange<'_> {
    fn drop(&mut self) {
        // Advance before releasing the fence, including cancellation after a partial change.
        self.generation.fetch_add(1, Ordering::Release);
    }
}

impl Service {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            live: RwLock::new((config.models.clone(), config.credentials.clone())),
            catalog: Mutex::new(None),
            credentials_generation: AtomicU64::new(0),
            default_generation: AtomicU64::new(0),
            startups: AsyncRwLock::new(()),
            policy: Mutex::new(crate::model_accounts::Policy::default()),
            catalog_refresh: Mutex::new(()),
        }
    }
    pub(crate) fn default_generation(&self) -> u64 {
        self.default_generation.load(Ordering::Acquire)
    }
    pub(crate) fn models(&self) -> ModelsConfig {
        self.live.read().0.clone()
    }
    pub(crate) fn default_model(&self) -> Option<String> {
        self.live.read().0.default.clone()
    }
    pub(crate) fn fallback_models(&self) -> Vec<String> {
        self.live.read().0.fallback.clone()
    }
    pub(crate) fn front_model(&self) -> Option<String> {
        self.live.read().0.front.clone()
    }
    pub(crate) fn credentials_config(&self) -> Value {
        self.live.read().1.clone()
    }
    pub(crate) fn sidecar_options(&self) -> Value {
        let live = self.live.read();
        let version = live.1["anthropic"]["claude_code_version"]
            .as_str()
            .filter(|version| {
                version.len() <= 64
                    && version.split('.').count() == 3
                    && version.split('.').all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
            });
        version
            .map(|version| json!({"anthropic":{"claude_code_version":version}}))
            .unwrap_or_else(|| json!({}))
    }
    pub(crate) fn set_pools(&self, pools: &BTreeMap<String, openside_proto::ProviderPool>) {
        let mut live = self.live.write();
        if !live.1.is_object() {
            live.1 = json!({});
        }
        for (provider, pool) in pools {
            live.1["pools"][provider] = json!(pool.accounts);
        }
    }
    pub(crate) async fn admit_credentials(&self) -> RwLockReadGuard<'_, ()> {
        self.startups.read().await
    }
    pub(crate) fn credentials_generation(&self) -> u64 {
        self.credentials_generation.load(Ordering::Acquire)
    }
    pub(crate) async fn change_credentials(&self) -> CredentialChange<'_> {
        CredentialChange {
            generation: &self.credentials_generation,
            _permit: self.startups.write().await,
        }
    }
    pub(crate) async fn invalidate(&self, s: &Shared) {
        *self.catalog.lock().await = None;
        s.accounts.invalidate().await;
    }
}

fn context<'a>(who: &'a str) -> AuditContext<'a> {
    AuditContext {
        who,
        session_id: None,
        tool_call_id: None,
    }
}

fn unavailable() -> HttpError {
    HttpError(StatusCode::CONFLICT, "vault_unavailable")
}

pub(crate) fn person(caller: &Caller) -> Result<(), HttpError> {
    if caller.0.is_some() && caller.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_device_required"));
    }
    Ok(())
}

pub(crate) fn authorized(s: &Shared, caller: &Caller) -> Result<(), HttpError> {
    if caller
        .0
        .as_deref()
        .is_some_and(|id| s.remote.is_revoked(id))
    {
        return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(())
}

fn valid_provider(provider: &str) -> Result<(), HttpError> {
    if matches!(provider, "anthropic" | "openai" | "google") {
        Ok(())
    } else {
        Err(HttpError(StatusCode::BAD_REQUEST, "invalid_provider"))
    }
}

pub(crate) async fn fence_startup<'a>(
    s: &'a Shared,
    generation: u64,
    credentials_json: &mut String,
) -> RwLockReadGuard<'a, ()> {
    let permit = s.models.startups.read().await;
    if generation != s.models.credentials_generation() {
        credentials_json.zeroize();
        *credentials_json = s.models.sidecar_options().to_string();
    }
    // Keep this permit until Runtime.open returns: no asynchronous spawn can bypass it.
    permit
}

pub(crate) async fn catalog(s: &Shared) -> Result<ModelCatalog, HttpError> {
    let cache = s.models.catalog.lock().await;
    if let Some((at, generation, value)) = &*cache {
        if *generation == s.models.credentials_generation() && at.elapsed() < Duration::from_secs(5)
        {
            return Ok(value.clone());
        }
    }
    drop(cache);
    let _refresh = s.models.catalog_refresh.lock().await;
    let cache = s.models.catalog.lock().await;
    if let Some((at, generation, value)) = &*cache {
        if *generation == s.models.credentials_generation() && at.elapsed() < Duration::from_secs(5)
        {
            return Ok(value.clone());
        }
    }
    drop(cache);
    let generation = s.models.credentials_generation();
    let report = crate::model_accounts::catalog(s).await?;
    if generation == s.models.credentials_generation() {
        *s.models.catalog.lock().await = Some((Instant::now(), generation, report.clone()));
    }
    Ok(report)
}

pub(crate) async fn get(State(s): State<Shared>) -> Result<Json<ModelCatalog>, HttpError> {
    catalog(&s).await.map(Json)
}

pub(crate) async fn put(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<UpdateModelsRequest>,
) -> Result<Json<ModelSelection>, HttpError> {
    tokio::spawn(async move {
        authorized(&s, &caller)?;
        validate_selection(&request, &catalog(&s).await?)?;
        let _writer = crate::settings_api::WRITE.lock().await;
        authorized(&s, &caller)?;
        let mut current = crate::settings_api::stored(&s).await?;
        current.models.default = Some(request.default_model.clone());
        current.models.fallback = request.fallback_models.clone();
        crate::settings_api::persist(
            &s,
            serde_json::to_value(current).map_err(|_| HttpError::internal())?,
        )
        .await?;
        {
            let mut live = s.models.live.write();
            let changed = live.0.default.as_deref() != Some(request.default_model.as_str())
                || live.0.fallback != request.fallback_models;
            live.0.default = Some(request.default_model.clone());
            live.0.fallback = request.fallback_models.clone();
            if changed {
                s.models.default_generation.fetch_add(1, Ordering::Release);
            }
        }
        s.models.invalidate(&s).await;
        crate::pairing::audit(&s, "models", "written", json!({})).await;
        Ok(Json(ModelSelection {
            default_model: request.default_model,
            fallback_models: request.fallback_models,
            restart_required: false,
            effective: "new_sessions_and_home_next_turn".into(),
        }))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

fn validate_selection(
    request: &UpdateModelsRequest,
    catalog: &ModelCatalog,
) -> Result<(), HttpError> {
    let mut seen = HashSet::new();
    if request.fallback_models.len() > 16
        || std::iter::once(&request.default_model)
            .chain(&request.fallback_models)
            .any(|id| id.len() > 256 || !seen.insert(id))
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_models"));
    }
    for id in std::iter::once(&request.default_model).chain(&request.fallback_models) {
        if !catalog
            .providers
            .iter()
            .flat_map(|provider| &provider.models)
            .any(|model| &model.id == id && model.available)
        {
            return Err(HttpError(StatusCode::BAD_REQUEST, "model_unavailable"));
        }
    }
    Ok(())
}

fn info(info: openside_vault::ProviderKeyInfo) -> Result<ProviderKeyInfo, HttpError> {
    let created_at = i64::try_from(info.created_at_ms)
        .ok()
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
        .ok_or_else(HttpError::internal)?;
    Ok(ProviderKeyInfo {
        provider: info.provider,
        key_id: info.key_id,
        label: info.label,
        enabled: info.enabled,
        reserve_percent: info.reserve_percent,
        last_four: info.last_four,
        created_at,
    })
}

pub(crate) async fn keys(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<ProviderKeyList>, HttpError> {
    let vault = s.registry.vault.vault.lock().await;
    let vault = vault.as_ref().ok_or_else(unavailable)?;
    let keys = vault
        .provider_keys(&context(caller.id()))
        .map_err(|_| unavailable())?
        .into_iter()
        .map(info)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(ProviderKeyList { keys }))
}

pub(crate) fn opaque_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(HEX[usize::from(byte >> 4)] as char);
        id.push(HEX[usize::from(byte & 15)] as char);
    }
    id
}

fn key_error(error: Error) -> HttpError {
    match error {
        Error::Locked => unavailable(),
        Error::UnknownItem => HttpError(StatusCode::NOT_FOUND, "key_not_found"),
        Error::ItemExists => HttpError(StatusCode::CONFLICT, "key_id_conflict"),
        Error::InvalidEntry => HttpError::invalid_value(),
        _ => HttpError::internal(),
    }
}

pub(crate) async fn create_key(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(provider): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<ProviderKeyInfo>), HttpError> {
    person(&caller)?;
    valid_provider(&provider)?;
    let request: CreateProviderKeyRequest =
        serde_json::from_slice(&body).map_err(|_| HttpError::invalid_value())?;
    let key_id = opaque_id();
    let result = write_key(
        s,
        caller,
        provider,
        key_id,
        Some(request.label),
        Secret::new(request.key),
        true,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

pub(crate) async fn set_key(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((provider, key_id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ProviderKeyInfo>, HttpError> {
    person(&caller)?;
    valid_provider(&provider)?;
    let request: SetProviderKeyRequest =
        serde_json::from_slice(&body).map_err(|_| HttpError::invalid_value())?;
    write_key(
        s,
        caller,
        provider,
        key_id,
        request.label,
        Secret::new(request.key),
        false,
    )
    .await
    .map(Json)
}

async fn write_key(
    s: Shared,
    caller: Caller,
    provider: String,
    key_id: String,
    label: Option<String>,
    key: Secret,
    create: bool,
) -> Result<ProviderKeyInfo, HttpError> {
    tokio::spawn(async move {
        authorized(&s, &caller)?;
        crate::model_accounts::initialize(&s).await?;
        let change = s.models.change_credentials().await;
        let service = &s.registry.vault;
        let mut guard = service.vault.lock().await;
        authorized(&s, &caller)?;
        if guard.is_none() && create {
            let probe = crate::tools::vault::Service::probe(&s).await;
            if !probe.backend.is_ok_and(|backend| backend.is_keyring()) {
                return Err(unavailable());
            }
            service
                .setup(&s, &mut guard, caller.id(), None)
                .await
                .map_err(|_| unavailable())?;
        }
        let vault = guard.as_mut().ok_or_else(unavailable)?;
        let previous = vault
            .provider_keys(&context(caller.id()))
            .map_err(key_error)?
            .into_iter()
            .find(|info| info.provider == provider && info.key_id == key_id);
        if create && previous.is_some() {
            return Err(HttpError(StatusCode::CONFLICT, "key_id_conflict"));
        }
        if !create && previous.is_none() {
            return Err(HttpError(StatusCode::NOT_FOUND, "key_not_found"));
        }
        let previous_time = previous
            .as_ref()
            .map(|info| info.created_at_ms)
            .unwrap_or(0);
        let label = label
            .or_else(|| previous.map(|info| info.label))
            .ok_or_else(HttpError::invalid_value)?;
        let now = u64::try_from(Utc::now().timestamp_millis())
            .map_err(|_| HttpError::internal())?
            .max(previous_time.saturating_add(1));
        let entry = Entry::provider_key(&provider, &key_id, &label, key, now).map_err(key_error)?;
        s.registry.redactors.write().push(entry.redactor());
        let result = vault.set_provider_key(&context(caller.id()), entry);
        drop(guard);
        drop(change);
        s.models.invalidate(&s).await;
        info(result.map_err(key_error)?)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn update_key(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((provider, key_id)): Path<(String, String)>,
    Json(request): Json<UpdateProviderKeyRequest>,
) -> Result<Json<ProviderKeyInfo>, HttpError> {
    person(&caller)?;
    valid_provider(&provider)?;
    tokio::spawn(async move {
        authorized(&s, &caller)?;
        crate::model_accounts::initialize(&s).await?;
        let change = s.models.change_credentials().await;
        let mut guard = s.registry.vault.vault.lock().await;
        authorized(&s, &caller)?;
        let result = guard.as_mut().ok_or_else(unavailable)?.update_provider_key(
            &context(caller.id()),
            &provider,
            &key_id,
            request.label.as_deref(),
            request.enabled,
            request.reserve_percent,
        );
        drop(guard);
        drop(change);
        s.models.invalidate(&s).await;
        info(result.map_err(key_error)?).map(Json)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn delete_key(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path((provider, key_id)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    person(&caller)?;
    valid_provider(&provider)?;
    tokio::spawn(async move {
        authorized(&s, &caller)?;
        crate::model_accounts::initialize(&s).await?;
        let change = s.models.change_credentials().await;
        let mut guard = s.registry.vault.vault.lock().await;
        authorized(&s, &caller)?;
        let result = guard.as_mut().ok_or_else(unavailable)?.delete_provider_key(
            &context(caller.id()),
            &provider,
            &key_id,
        );
        drop(guard);
        drop(change);
        s.models.invalidate(&s).await;
        result.map_err(key_error)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| HttpError::internal())?
}
