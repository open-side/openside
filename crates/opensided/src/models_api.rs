use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    CreateProviderKeyRequest, DeviceKind, ModelCatalog, ModelKind, ModelSelection,
    ProviderConnection, ProviderConnectionKind, ProviderConnectionList, ProviderConnectionStatus,
    ProviderKeyInfo, ProviderKeyList, SessionId, SetProviderKeyRequest, UpdateModelsRequest,
    UpdateProviderKeyRequest,
};
use openside_vault::{AuditContext, Entry, Error, Secret};
use parking_lot::{Mutex as SyncMutex, RwLock};
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
    pub(crate) plugins: crate::model_plugins::Cache,
    image_grants: SyncMutex<HashMap<(SessionId, String), ImageGrant>>,
    /// Best available image model from the last fresh catalog; serves an unset `models.image`.
    auto_image: RwLock<Option<String>>,
}

/// Wire value of `image_model` that turns image generation off (stored as `models.image_off`).
pub(crate) const IMAGE_OFF: &str = "off";

/// One admitted `generate_image` call: the sidecar may lease `model` for it and store up to
/// `count` images under its tool call id until `expires`.
struct ImageGrant {
    model: String,
    subagent: Option<String>,
    count: usize,
    stored: HashSet<usize>,
    expires: Instant,
}

/// Long enough for a slow hosted image request plus its credential retries.
const IMAGE_GRANT_TTL: Duration = Duration::from_secs(10 * 60);

/// Which role a subagent or Task serves; resolved against `[models]` with session fallback.
#[derive(Clone, Copy, Debug, Default, serde::Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskRole {
    /// Ordinary subagent work.
    #[default]
    Standard,
    /// Hard reasoning, planning, judging or synthesis.
    Deep,
    /// Interpreting images, screenshots, CAPTCHAs or page visuals.
    Visual,
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
            plugins: crate::model_plugins::Cache::default(),
            image_grants: SyncMutex::new(HashMap::new()),
            auto_image: RwLock::new(None),
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
    pub(crate) fn visual_model(&self) -> Option<String> {
        self.live.read().0.visual.clone()
    }
    /// The model `generate_image` uses: none when turned off, else the chosen one, else the best
    /// available image model seen by the last catalog refresh.
    pub(crate) fn image_model(&self) -> Option<String> {
        let live = self.live.read();
        if live.0.image_off {
            return None;
        }
        live.0
            .image
            .clone()
            .or_else(|| self.auto_image.read().clone())
    }
    /// The configured model for `role`, or `session_model` when that role is unset.
    pub(crate) fn task_model(&self, role: TaskRole, session_model: &str) -> String {
        let live = self.live.read();
        match role {
            TaskRole::Standard => live.0.standard.as_ref(),
            TaskRole::Deep => live.0.deep.as_ref(),
            TaskRole::Visual => live.0.visual.as_ref(),
        }
        .cloned()
        .unwrap_or_else(|| session_model.to_owned())
    }
    pub(crate) fn grant_image(
        &self,
        session: &SessionId,
        tool_call_id: &str,
        subagent: Option<&str>,
        model: &str,
        count: usize,
    ) {
        let now = Instant::now();
        let mut grants = self.image_grants.lock();
        grants.retain(|_, grant| grant.expires > now);
        grants.insert(
            (session.clone(), tool_call_id.to_owned()),
            ImageGrant {
                model: model.to_owned(),
                subagent: subagent.map(str::to_owned),
                count,
                stored: HashSet::new(),
                expires: now + IMAGE_GRANT_TTL,
            },
        );
    }
    /// A credential request for an image model is valid only inside an admitted call.
    pub(crate) fn image_lease_allowed(
        &self,
        session: &SessionId,
        subagent: Option<&str>,
        model: &str,
    ) -> bool {
        let now = Instant::now();
        self.image_grants.lock().iter().any(|((owner, _), grant)| {
            owner == session
                && grant.expires > now
                && grant.subagent.as_deref() == subagent
                && grant.model == model
        })
    }
    /// Claims image slot `index` of an admitted call; `last` ends the grant.
    pub(crate) fn claim_image(
        &self,
        session: &SessionId,
        tool_call_id: &str,
        subagent: Option<&str>,
        index: usize,
        last: bool,
    ) -> bool {
        let now = Instant::now();
        let mut grants = self.image_grants.lock();
        let key = (session.clone(), tool_call_id.to_owned());
        let Some(grant) = grants.get_mut(&key) else {
            return false;
        };
        if grant.expires <= now
            || grant.subagent.as_deref() != subagent
            || index >= grant.count
            || !grant.stored.insert(index)
        {
            return false;
        }
        if last || grant.stored.len() == grant.count {
            grants.remove(&key);
        }
        true
    }
    #[cfg(test)]
    pub(crate) fn expire_image_grants(&self) {
        let past = Instant::now() - Duration::from_secs(1);
        for grant in self.image_grants.lock().values_mut() {
            grant.expires = past;
        }
    }
    /// Ends a grant whose generation failed, so its model stops being leasable.
    pub(crate) fn end_image(
        &self,
        session: &SessionId,
        tool_call_id: &str,
        subagent: Option<&str>,
    ) {
        let key = (session.clone(), tool_call_id.to_owned());
        let mut grants = self.image_grants.lock();
        if grants
            .get(&key)
            .is_some_and(|grant| grant.subagent.as_deref() == subagent)
        {
            grants.remove(&key);
        }
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
    *s.models.auto_image.write() = best_image_model(&report.providers);
    if generation == s.models.credentials_generation() {
        *s.models.catalog.lock().await = Some((Instant::now(), generation, report.clone()));
    }
    Ok(report)
}

pub(crate) async fn auto_select(s: &Shared) -> Result<(), HttpError> {
    let models = s.models.models();
    if models.default.is_some() || models.front.is_some() {
        return Ok(());
    }
    let catalog = catalog(s).await?;
    let Some((provider, reasoning)) = catalog.providers.iter().find_map(|provider| {
        provider
            .models
            .iter()
            .find(|model| {
                model.available
                    && model.recommended_for == Some(openside_proto::ModelRole::Reasoning)
            })
            .map(|model| (provider, model))
    }) else {
        return Ok(());
    };
    let fast = provider
        .models
        .iter()
        .chain(
            catalog
                .providers
                .iter()
                .filter(|other| other.provider != provider.provider)
                .flat_map(|other| &other.models),
        )
        .find(|model| {
            model.available && model.recommended_for == Some(openside_proto::ModelRole::Fast)
        })
        .map(|model| model.id.clone());
    let _writer = crate::settings_api::WRITE.lock().await;
    let mut current = crate::settings_api::stored(s).await?;
    if current.models.default.is_some() || current.models.front.is_some() {
        return Ok(());
    }
    current.models.default = Some(reasoning.id.clone());
    current.models.front = fast.clone();
    crate::settings_api::persist(
        s,
        serde_json::to_value(current).map_err(|_| HttpError::internal())?,
    )
    .await?;
    {
        let mut live = s.models.live.write();
        live.0.default = Some(reasoning.id.clone());
        live.0.front = fast;
        s.models.default_generation.fetch_add(1, Ordering::Release);
    }
    s.models.invalidate(s).await;
    Ok(())
}

pub(crate) async fn get(State(s): State<Shared>) -> Result<Json<ModelCatalog>, HttpError> {
    catalog(&s).await.map(Json)
}

/// The first usable image model in catalog order: what an unset `models.image` resolves to.
pub(crate) fn best_image_model(providers: &[openside_proto::ModelProvider]) -> Option<String> {
    providers
        .iter()
        .flat_map(|provider| &provider.models)
        .find(|model| model.available && model.kind == ModelKind::Image)
        .map(|model| model.id.clone())
}

/// The wire form of the stored image choice: null (automatic), "off", or the chosen id.
pub(crate) fn image_choice(models: &ModelsConfig) -> Option<String> {
    if models.image_off {
        Some(IMAGE_OFF.into())
    } else {
        models.image.clone()
    }
}

/// `request.x` when present (an explicit null clears), else the current value.
fn patched(request: &Option<Option<String>>, current: &Option<String>) -> Option<String> {
    match request {
        Some(value) => value.clone(),
        None => current.clone(),
    }
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
        let mut next = current.models.clone();
        next.front = request
            .fast_model
            .clone()
            .filter(|model| model != &request.default_model);
        next.default = Some(request.default_model.clone());
        next.fallback = request.fallback_models.clone();
        next.standard = patched(&request.standard_model, &current.models.standard);
        next.deep = patched(&request.deep_model, &current.models.deep);
        next.visual = patched(&request.visual_model, &current.models.visual);
        match &request.image_model {
            None => {}
            Some(Some(value)) if value == IMAGE_OFF => {
                next.image = None;
                next.image_off = true;
            }
            Some(value) => {
                next.image.clone_from(value);
                next.image_off = false;
            }
        }
        current.models = next.clone();
        crate::settings_api::persist(
            &s,
            serde_json::to_value(current).map_err(|_| HttpError::internal())?,
        )
        .await?;
        {
            let mut live = s.models.live.write();
            let selected = |models: &ModelsConfig| models.selected().cloned().collect::<Vec<_>>();
            let changed = selected(&live.0) != selected(&next) || live.0.front != next.front;
            live.0 = next.clone();
            if changed {
                s.models.default_generation.fetch_add(1, Ordering::Release);
            }
        }
        s.models.invalidate(&s).await;
        crate::pairing::audit(&s, "models", "written", json!({})).await;
        let image_model = image_choice(&next);
        Ok(Json(ModelSelection {
            default_model: request.default_model,
            fast_model: next.front,
            standard_model: next.standard,
            deep_model: next.deep,
            visual_model: next.visual,
            image_model,
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
    let roles = |kind: ModelKind| {
        let ids: Vec<&String> = match kind {
            ModelKind::Chat => std::iter::once(&request.default_model)
                .chain(&request.fallback_models)
                .chain(request.fast_model.iter())
                .chain(
                    [
                        &request.standard_model,
                        &request.deep_model,
                        &request.visual_model,
                    ]
                    .into_iter()
                    .filter_map(|role| role.as_ref().and_then(Option::as_ref)),
                )
                .collect(),
            ModelKind::Image => request
                .image_model
                .as_ref()
                .and_then(Option::as_ref)
                .filter(|id| *id != IMAGE_OFF)
                .into_iter()
                .collect(),
        };
        ids.into_iter().map(move |id| (id, kind))
    };
    for (id, kind) in roles(ModelKind::Chat).chain(roles(ModelKind::Image)) {
        if id.len() > 256 {
            return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_models"));
        }
        let Some(model) = catalog
            .providers
            .iter()
            .flat_map(|provider| &provider.models)
            .find(|model| &model.id == id && model.available)
        else {
            return Err(HttpError(StatusCode::BAD_REQUEST, "model_unavailable"));
        };
        if model.kind != kind {
            return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_model_kind"));
        }
    }
    Ok(())
}

/// The composer's model choice becomes the default for new chats ("last used model").
/// Best effort: an unavailable or image model, or a failed write, leaves the default alone.
pub(crate) async fn remember_default(s: &Shared, model: &str) {
    if s.models.default_model().as_deref() == Some(model) {
        return;
    }
    let Ok(catalog) = catalog(s).await else {
        return;
    };
    if !catalog
        .providers
        .iter()
        .flat_map(|provider| &provider.models)
        .any(|candidate| {
            candidate.id == model && candidate.available && candidate.kind == ModelKind::Chat
        })
    {
        return;
    }
    let _writer = crate::settings_api::WRITE.lock().await;
    let Ok(mut current) = crate::settings_api::stored(s).await else {
        return;
    };
    current.models.default = Some(model.to_owned());
    // A fallback equal to the default would be tried twice.
    current.models.fallback.retain(|id| id != model);
    if current.models.front.as_deref() == Some(model) {
        current.models.front = None;
    }
    let next = current.models.clone();
    let Ok(value) = serde_json::to_value(current) else {
        return;
    };
    if crate::settings_api::persist(s, value).await.is_err() {
        return;
    }
    {
        let mut live = s.models.live.write();
        live.0.default = next.default;
        live.0.fallback = next.fallback;
        live.0.front = next.front;
        s.models.default_generation.fetch_add(1, Ordering::Release);
    }
    s.models.invalidate(s).await;
}

/// Product name of an SDK provider namespace for the UI.
pub(crate) fn provider_display_name(provider: &str) -> &str {
    match provider {
        "anthropic" => "Claude",
        "openai-codex" => "ChatGPT",
        "openai" | "openai-responses" | "openai-chat" => "OpenAI",
        "google" => "Gemini",
        "google-gemini-cli" => "Gemini CLI",
        "google-antigravity" => "Antigravity",
        "github-copilot" => "GitHub Copilot",
        "openrouter" => "OpenRouter",
        other => other,
    }
}

/// Signed-in subscriptions versus person-supplied keys; quiet plugin routes are neither.
fn connection_kind(source: &str) -> Option<ProviderConnectionKind> {
    match source {
        "vault" | "env" | "command" | "builtin" => Some(ProviderConnectionKind::ApiKey),
        crate::model_plugins::SOURCE => None,
        _ => Some(ProviderConnectionKind::Subscription),
    }
}

/// One row per provider and connection kind, collapsing every account behind it.
pub(crate) fn connections(accounts: &[crate::model_accounts::Account]) -> ProviderConnectionList {
    let mut rows: BTreeMap<(String, &'static str), ProviderConnection> = BTreeMap::new();
    for account in accounts {
        let Some(kind) = connection_kind(&account.reference.source) else {
            continue;
        };
        // Environment probes and the default CLI-home accounts exist whether or not anything
        // was ever connected; only a ready one counts as a connection.
        if !account.ready
            && (matches!(account.reference.source.as_str(), "env" | "builtin")
                || account.reference.account_id == "default")
        {
            continue;
        }
        let tag = match kind {
            ProviderConnectionKind::Subscription => "subscription",
            ProviderConnectionKind::ApiKey => "api_key",
        };
        let row = rows
            .entry((account.provider.clone(), tag))
            .or_insert_with(|| {
                let name = provider_display_name(&account.provider);
                ProviderConnection {
                    id: format!("{}/{tag}", account.provider),
                    provider: account.provider.clone(),
                    kind,
                    display_name: match kind {
                        ProviderConnectionKind::Subscription => name.to_owned(),
                        ProviderConnectionKind::ApiKey => match account.provider.as_str() {
                            "anthropic" => "Anthropic API key".to_owned(),
                            _ => format!("{name} API key"),
                        },
                    },
                    status: ProviderConnectionStatus::NeedsAttention,
                    accounts: Vec::new(),
                }
            });
        if account.enabled && account.ready {
            row.status = ProviderConnectionStatus::Connected;
        }
        row.accounts.push(account.reference.clone());
    }
    ProviderConnectionList {
        providers: rows.into_values().collect(),
    }
}

pub(crate) async fn providers(
    State(s): State<Shared>,
) -> Result<Json<ProviderConnectionList>, HttpError> {
    let inventory = crate::model_accounts::inventory(&s).await;
    Ok(Json(connections(&inventory.accounts)))
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
                .setup(&s, &mut guard, caller.id(), None, false)
                .await
                .map_err(|error| match error {
                    (403, "biometric_required") => crate::vault_api::lifecycle(error),
                    _ => unavailable(),
                })?;
        }
        let vault = guard.as_mut().ok_or_else(unavailable)?;
        s.registry.vault.touch(&s);
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
        let _ = auto_select(&s).await;
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
        s.registry.vault.touch(&s);
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
        let _ = auto_select(&s).await;
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
        s.registry.vault.touch(&s);
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
