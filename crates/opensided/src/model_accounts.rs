use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    model_broker::Broker,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_core::{
    CredentialAction, CredentialAuth, CredentialAuthKind, CredentialError, CredentialErrorCode,
    CredentialOutcome, CredentialReport, CredentialReportAck, CredentialRequest, CredentialScope,
    CredentialSnapshot, ModelAccountRef, SessionId, MAX_CREDENTIAL_ATTEMPTS,
};
use openside_proto::{
    AccountSource, AccountStatus, AccountUsageWindow, AccountsResponse, CatalogModel, ModelAccount,
    ModelCatalog, ModelProvider, ProviderPool, ProviderPoolList, SessionAccountSelection,
    UpdateProviderPoolRequest, UpdateSessionAccountRequest,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;

const SOURCES: &[&str] = &[
    "omp-broker",
    "vault",
    "env",
    "command",
    "claude-code",
    "codex-cli",
    "builtin",
];

#[derive(Clone)]
pub(crate) struct Account {
    pub provider: String,
    pub reference: ModelAccountRef,
    pub label: String,
    pub enabled: bool,
    pub ready: bool,
    pub reason: String,
    pub models: Vec<String>,
    pub reserve: u8,
    pub windows: Vec<AccountUsageWindow>,
    pub blocked_until: Option<u64>,
    pub model_blocks: Vec<(String, u64)>,
    pub revision: u64,
}

pub(crate) struct Inventory {
    pub accounts: Vec<Account>,
    pub broker_present: bool,
    pub broker_failed: bool,
}
const LEASE_TTL: Duration = Duration::from_secs(10 * 60);
const SESSION_LEASE_LIMIT: usize = 8;
const REPORTED_BACKOFF_MS: u64 = 60_000;
const MAX_REPORTED_BACKOFF_MS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Clone)]
struct Block {
    until: Option<u64>,
    reason: &'static str,
}
#[derive(Default)]
struct Health {
    revision: u64,
    windows: Vec<AccountUsageWindow>,
    windows_until_ms: Option<u64>,
    observation: u64,
    blocks: HashMap<Option<String>, Block>,
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}
#[derive(Clone)]
struct Lease {
    session: SessionId,
    request: String,
    provider: String,
    model: String,
    account: ModelAccountRef,
    revision: u64,
    subagent: Option<String>,
    expires: tokio::time::Instant,
    issued_at_ms: u64,
    observation: u64,
}
#[derive(Default)]
struct Attempt {
    next: u32,
    tried: HashSet<(ModelAccountRef, u64)>,
    refreshed: HashSet<(ModelAccountRef, u64)>,
    refresh: Option<ModelAccountRef>,
    in_flight: bool,
    provider: String,
    model: String,
    touched: Option<tokio::time::Instant>,
}
#[derive(Default)]
pub(crate) struct Policy {
    health: HashMap<(String, ModelAccountRef), Health>,
    sticky: HashMap<(SessionId, String), ModelAccountRef>,
    preferred: HashMap<(SessionId, String), ModelAccountRef>,
    attempts: HashMap<(SessionId, String), Attempt>,
    leases: HashMap<String, Lease>,
    acknowledgements: HashMap<(SessionId, String), CredentialReportAck>,
    registered: HashSet<(ModelAccountRef, u64)>,
    next_observation: u64,
    report_rate: HashMap<SessionId, (tokio::time::Instant, u32)>,
    models: HashMap<SessionId, Vec<String>>,
    fallbacks: HashMap<SessionId, Vec<String>>,
}

fn now() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}
fn context() -> openside_vault::AuditContext<'static> {
    openside_vault::AuditContext {
        who: "model-credentials",
        session_id: None,
        tool_call_id: None,
    }
}
fn reference(source: &str, id: &str) -> ModelAccountRef {
    ModelAccountRef {
        source: source.into(),
        account_id: id.into(),
    }
}
fn revision(value: &str) -> u64 {
    u64::from_be_bytes(
        Sha256::digest(value.as_bytes())[..8]
            .try_into()
            .unwrap_or_default(),
    )
}
fn names(provider: &str, credentials: &Value) -> Vec<String> {
    let configured = &credentials["env"][provider];
    if let Some(name) = configured.as_str() {
        return vec![name.into()];
    }
    if let Some(names) = configured.as_array() {
        return names
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .take(64)
            .collect();
    }
    #[cfg(test)]
    if configured.is_null() {
        return vec![];
    }
    match provider {
        "anthropic" => vec!["ANTHROPIC_API_KEY".into()],
        "openai" | "openai-chat" | "openai-responses" => vec!["OPENAI_API_KEY".into()],
        "google" | "google-gla" => vec!["GOOGLE_API_KEY".into(), "GEMINI_API_KEY".into()],
        "groq" => vec!["GROQ_API_KEY".into()],
        "mistral" => vec!["MISTRAL_API_KEY".into()],
        "deepseek" => vec!["DEEPSEEK_API_KEY".into()],
        "openrouter" => vec!["OPENROUTER_API_KEY".into()],
        "github-copilot" => [
            "GITHUB_COPILOT_API_KEY",
            "GITHUB_COPILOT_API_TOKEN",
            "COPILOT_GITHUB_TOKEN",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "openai-codex" => vec![],
        _ => vec![format!(
            "{}_API_KEY",
            provider.to_ascii_uppercase().replace('-', "_")
        )],
    }
}
fn env_value(name: &str) -> Option<Zeroizing<String>> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty() && v.len() <= 16_384)
        .map(Zeroizing::new)
}
fn env_id(provider: &str, name: &str) -> String {
    format!("{provider}-{:016x}", revision(name))
}
fn auth(value: String, provider: &str) -> CredentialAuth {
    CredentialAuth {
        kind: if provider == "github-copilot" {
            CredentialAuthKind::Bearer
        } else {
            CredentialAuthKind::ApiKey
        },
        value,
        upstream_account_id: None,
        expires_at_ms: None,
    }
}
fn error(
    s: &Shared,
    code: CredentialErrorCode,
    reason: &'static str,
    retry: Option<u64>,
) -> CredentialError {
    CredentialError {
        code,
        reason: reason.into(),
        retry_at_ms: retry,
        generation: s.models.credentials_generation(),
    }
}

pub(crate) async fn inventory(s: &Shared) -> Inventory {
    let credentials = s.models.credentials_config();
    let mut accounts = Vec::new();
    let configured = Broker::configured(&credentials, s.broker_environment);
    let broker_present = !matches!(configured, Ok(None));
    let mut broker_failed = configured.is_err();
    if let Ok(Some(broker)) = configured {
        match broker.accounts().await {
            Ok(rows) => {
                for (index, row) in rows.into_iter().enumerate() {
                    accounts.push(Account {
                        provider: row.provider,
                        reference: reference("omp-broker", &row.id),
                        label: format!("Broker account {}", index + 1),
                        enabled: row.enabled,
                        ready: row.enabled,
                        reason: if row.enabled {
                            "Broker credential available"
                        } else {
                            "Broker account disabled"
                        }
                        .into(),
                        models: row.models,
                        reserve: 10,
                        windows: row.windows,
                        blocked_until: row.blocked_until_ms.map(|v| v.min(u64::MAX as f64) as u64),
                        model_blocks: row.model_blocks,
                        revision: revision(&row.auth.value),
                    });
                }
            }
            Err(_) => broker_failed = true,
        }
    }
    let subscriptions = s.managed_subscriptions.list(s).await.unwrap_or_default();
    let vault = s.registry.vault.vault.lock().await;
    if let Some(vault) = vault.as_ref() {
        if let Ok(keys) = vault.provider_keys(&context()) {
            for key in keys {
                accounts.push(Account {
                    provider: key.provider,
                    reference: reference("vault", &key.key_id),
                    label: key.label,
                    enabled: key.enabled,
                    ready: key.enabled,
                    reason: if key.enabled {
                        "Vault API key available"
                    } else {
                        "Account disabled"
                    }
                    .into(),
                    models: vec![],
                    reserve: key.reserve_percent,
                    windows: vec![],
                    blocked_until: None,
                    model_blocks: vec![],
                    revision: key.created_at_ms,
                });
            }
        }
        let revisions = vault.subscriptions(&context()).unwrap_or_default();
        for account in subscriptions {
            let version = revisions
                .iter()
                .find(|info| info.source == account.source && info.account_id == account.account_id)
                .map(|info| info.revision)
                .unwrap_or(0);
            accounts.push(Account {
                provider: account.provider,
                reference: reference(&account.source, &account.account_id),
                label: account.label,
                enabled: account.enabled,
                ready: account.available,
                reason: account.reason,
                models: vec![],
                reserve: account.reserve_percent,
                windows: vec![],
                blocked_until: None,
                model_blocks: vec![],
                revision: version,
            });
        }
    }
    drop(vault);
    let mut providers: HashSet<String> = [
        "anthropic",
        "openai",
        "google",
        "openai-codex",
        "github-copilot",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    providers.extend(accounts.iter().map(|a| a.provider.clone()));
    for section in ["env", "command", "order", "pools"] {
        if let Some(map) = credentials[section].as_object() {
            providers.extend(
                map.keys()
                    .filter(|name| *name != "timeout_seconds")
                    .cloned(),
            );
        }
    }
    let models = s.models.models();
    for model in models
        .default
        .iter()
        .chain(models.front.iter())
        .chain(&models.fallback)
    {
        if let Some((provider, _)) = model.split_once(':') {
            providers.insert(provider.into());
        }
    }
    for provider in providers {
        for (index, name) in names(&provider, &credentials).into_iter().enumerate() {
            let value = env_value(&name);
            accounts.push(Account {
                provider: provider.clone(),
                reference: reference("env", &env_id(&provider, &name)),
                label: format!("Environment account {}", index + 1),
                enabled: true,
                ready: value.is_some(),
                reason: if value.is_some() {
                    "Environment credential available"
                } else {
                    "Credential environment variable missing"
                }
                .into(),
                models: vec![],
                reserve: 10,
                windows: vec![],
                blocked_until: None,
                model_blocks: vec![],
                revision: value.as_deref().map(|v| revision(v)).unwrap_or(0),
            });
        }
        if let Some(argv) = credentials["command"][&provider]
            .as_array()
            .filter(|argv| !argv.is_empty())
        {
            let ready = argv[0]
                .as_str()
                .is_some_and(|program| openside_config::find_on_path(program).is_some());
            accounts.push(Account {
                provider: provider.clone(),
                reference: reference("command", &provider),
                label: "Credential command".into(),
                enabled: true,
                ready,
                reason: if ready {
                    "Credential command configured; evaluated per request"
                } else {
                    "Credential command executable unavailable"
                }
                .into(),
                models: vec![],
                reserve: 10,
                windows: vec![],
                blocked_until: None,
                model_blocks: vec![],
                revision: 0,
            });
        }
        if credentials["order"][&provider]
            .as_array()
            .is_some_and(|order| {
                order
                    .iter()
                    .any(|source| source.as_str() == Some("builtin"))
            })
        {
            let value = names(&provider, &Value::Null)
                .iter()
                .find_map(|name| env_value(name));
            accounts.push(Account {
                provider: provider.clone(),
                reference: reference("builtin", &provider),
                label: "Provider environment account".into(),
                enabled: true,
                ready: value.is_some(),
                reason: if value.is_some() {
                    "Provider environment credential available"
                } else {
                    "No daemon-resolvable builtin credential"
                }
                .into(),
                models: vec![],
                reserve: 10,
                windows: vec![],
                blocked_until: None,
                model_blocks: vec![],
                revision: value.as_deref().map(|v| revision(v)).unwrap_or(0),
            });
        }
    }
    Inventory {
        accounts,
        broker_present,
        broker_failed,
    }
}

fn source_order(provider: &str, credentials: &Value) -> Vec<String> {
    if let Some(order) = credentials["order"][provider].as_array() {
        return order
            .iter()
            .filter_map(Value::as_str)
            .filter(|source| SOURCES.contains(source))
            .map(str::to_owned)
            .collect();
    }
    match provider {
        "anthropic" => ["omp-broker", "claude-code", "vault", "env", "command"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "openai-codex" => ["omp-broker", "codex-cli"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        _ => ["vault", "env", "command"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    }
}

pub(crate) async fn initialize(s: &Shared) -> Result<(), HttpError> {
    let inventory = inventory(s).await;
    pools(s, &inventory).await.map(|_| ())
}

pub(crate) async fn pools(
    s: &Shared,
    inventory: &Inventory,
) -> Result<BTreeMap<String, ProviderPool>, HttpError> {
    let _writer = crate::settings_api::WRITE.lock().await;
    let credentials = s.models.credentials_config();
    let mut result: BTreeMap<String, ProviderPool> = BTreeMap::new();
    if let Some(configured) = credentials["pools"].as_object() {
        for (provider, accounts) in configured {
            let accounts = serde_json::from_value::<Vec<ModelAccountRef>>(accounts.clone())
                .map_err(|_| HttpError::invalid_value())?;
            result.insert(
                provider.clone(),
                ProviderPool {
                    provider: provider.clone(),
                    accounts,
                },
            );
        }
    }
    let mut providers: HashSet<_> = inventory
        .accounts
        .iter()
        .map(|account| account.provider.clone())
        .collect();
    let models = s.models.models();
    for model in models
        .default
        .iter()
        .chain(models.front.iter())
        .chain(&models.fallback)
    {
        if let Some((provider, _)) = model.split_once(':') {
            providers.insert(provider.into());
        }
    }
    let mut changed = false;
    for provider in providers {
        if result.contains_key(&provider) {
            continue;
        }
        let selected = source_order(&provider, &credentials)
            .into_iter()
            .find(|source| {
                (*source == "omp-broker" && inventory.broker_present && inventory.broker_failed)
                    || inventory.accounts.iter().any(|account| {
                        account.provider == provider
                            && account.reference.source == *source
                            && account.ready
                    })
            });
        if selected.is_none() {
            continue;
        }
        let accounts = inventory
            .accounts
            .iter()
            .filter(|account| {
                account.provider == provider && Some(&account.reference.source) == selected.as_ref()
            })
            .map(|account| account.reference.clone())
            .collect();
        result.insert(provider.clone(), ProviderPool { provider, accounts });
        changed = true;
    }
    if changed {
        let change = s.models.change_credentials().await;
        let mut current = crate::settings_api::stored(s).await?;
        if !current.credentials.is_object() {
            current.credentials = json!({});
        }
        for (provider, pool) in &result {
            current.credentials["pools"][provider] = json!(pool.accounts);
        }
        crate::settings_api::persist(
            s,
            serde_json::to_value(current).map_err(|_| HttpError::internal())?,
        )
        .await?;
        s.models.set_pools(&result);
        drop(change);
    }
    Ok(result)
}

impl Policy {
    fn reap(&mut self) {
        let current = tokio::time::Instant::now();
        let expired: Vec<_> = self
            .leases
            .iter()
            .filter(|(_, lease)| lease.expires <= current)
            .map(|(id, lease)| (id.clone(), (lease.session.clone(), lease.request.clone())))
            .collect();
        for (id, request) in expired {
            self.leases.remove(&id);
            self.attempts.remove(&request);
        }
        self.attempts.retain(|_, attempt| {
            attempt
                .touched
                .map_or(true, |at| current.duration_since(at) < LEASE_TTL)
        });
        self.report_rate
            .retain(|_, (at, _)| current.duration_since(*at) < LEASE_TTL);
    }
    fn release(&mut self, session: &SessionId) {
        self.leases.retain(|_, lease| &lease.session != session);
        self.attempts.retain(|(owner, _), _| owner != session);
        self.acknowledgements
            .retain(|(owner, _), _| owner != session);
        self.report_rate.remove(session);
        self.models.remove(session);
        self.fallbacks.remove(session);
    }
    fn shared_report_allowed(&mut self, session: &SessionId) -> bool {
        let current = tokio::time::Instant::now();
        let (at, count) = self
            .report_rate
            .entry(session.clone())
            .or_insert((current, 0));
        if current.duration_since(*at) >= Duration::from_secs(60) {
            *at = current;
            *count = 0;
        }
        if *count >= 8 {
            return false;
        }
        *count += 1;
        true
    }
    fn can_lease(&self, session: &SessionId) -> bool {
        self.leases.len() < 256
            && self
                .leases
                .values()
                .filter(|lease| &lease.session == session)
                .count()
                < SESSION_LEASE_LIMIT
            && self
                .attempts
                .keys()
                .filter(|(owner, _)| owner == session)
                .count()
                < 64
    }
    fn health(&mut self, account: &Account) -> &mut Health {
        let health = self
            .health
            .entry((account.provider.clone(), account.reference.clone()))
            .or_default();
        if account.revision != 0 && health.revision != account.revision {
            *health = Health {
                revision: account.revision,
                ..Default::default()
            };
        }
        if health.windows_until_ms.is_some_and(|until| until <= now()) {
            health.windows.clear();
            health.windows_until_ms = None;
        }
        if !account.windows.is_empty() {
            health.windows = account.windows.clone();
            health.windows_until_ms = None;
        }
        if account
            .windows
            .iter()
            .any(|window| window.used_percent.is_some())
            && account
                .windows
                .iter()
                .filter_map(|window| window.used_percent)
                .all(|used| used < f64::from(100 - account.reserve))
            && health
                .blocks
                .get(&None)
                .is_some_and(|block| block.reason == "Account quota exhausted")
        {
            health.blocks.remove(&None);
        }
        health
            .blocks
            .retain(|_, block| block.until.map_or(true, |until| until > now()));
        health
    }
    fn status(&mut self, account: &Account, model: &str, in_pool: bool) -> ModelAccount {
        let mut available = account.enabled && account.ready && in_pool;
        let mut reason = if !in_pool {
            "Account is not in this provider pool".into()
        } else {
            account.reason.clone()
        };
        let mut until = account.blocked_until.filter(|until| *until > now());
        if !model.is_empty()
            && !account.models.is_empty()
            && !account.models.iter().any(|candidate| {
                candidate == model
                    || model
                        .split_once(':')
                        .is_some_and(|(_, name)| name == candidate)
            })
        {
            available = false;
            reason = "Model is not in this account's reported catalog".into();
        }
        let health = self.health(account);
        for window in &health.windows {
            if window
                .reset_at_ms
                .is_some_and(|reset| reset <= now() as f64)
            {
                continue;
            }
            if window
                .used_percent
                .is_some_and(|used| used >= f64::from(100 - account.reserve))
            {
                available = false;
                reason = "Account usage reserve reached".into();
                until = until.max(
                    window
                        .reset_at_ms
                        .map(|reset| reset.min(u64::MAX as f64) as u64),
                );
            }
        }
        if account.blocked_until.is_some_and(|blocked| blocked > now()) {
            available = false;
            reason = "Account is rate-limited".into();
        }
        for (scope, blocked) in &account.model_blocks {
            if *blocked > now()
                && (scope == model || model.split_once(':').is_some_and(|(_, name)| name == scope))
            {
                available = false;
                reason = "Account is blocked for this model".into();
                until = until.max(Some(*blocked));
            }
        }
        for scope in [None, Some(model.to_owned())] {
            if let Some(block) = health.blocks.get(&scope) {
                available = false;
                reason = block.reason.into();
                until = until.max(block.until);
            }
        }
        ModelAccount {
            source: account.reference.source.clone(),
            account_id: account.reference.account_id.clone(),
            label: account.label.clone(),
            enabled: account.enabled,
            ready: account.ready,
            in_pool,
            available,
            reason,
            usage_windows: health.windows.clone(),
            blocked_until_ms: until.map(|v| v as f64),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    providers: Vec<IndexProvider>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexProvider {
    provider: String,
    models: Vec<IndexModel>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexModel {
    id: String,
    label: String,
    #[serde(default)]
    recommended_for: Option<openside_proto::ModelRole>,
    #[serde(default)]
    in_catalog: bool,
    #[serde(default)]
    tier: openside_proto::ModelTier,
    #[serde(default)]
    strengths: String,
}

pub(crate) async fn catalog(s: &Shared) -> Result<ModelCatalog, HttpError> {
    let inventory = inventory(s).await;
    let pools = pools(s, &inventory).await?;
    let config =
        json!({"models":s.models.models(),"provider_names":pools.keys().collect::<Vec<_>>()});
    let environment = openside_config::runtime_environment(&s.models.credentials_config());
    let deadline = Duration::from_millis(s.config.loaded.config.agent.startup_timeout_ms)
        .max(Duration::from_secs(10));
    let value =
        crate::accounts::diagnostic(&s.config.argv, &config, &environment, deadline, "models")
            .await
            .map_err(|_| {
                HttpError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "account_sources_unavailable",
                )
            })?;
    let mut index: Index = serde_json::from_value(value).map_err(|_| {
        HttpError(
            StatusCode::SERVICE_UNAVAILABLE,
            "account_sources_unavailable",
        )
    })?;
    if index.providers.len() > 64
        || index.providers.iter().any(|p| {
            p.provider.len() > 64
                || p.models.len() > 256
                || p.models
                    .iter()
                    .any(|m| m.id.len() > 256 || m.label.len() > 256 || m.strengths.len() > 256)
        })
    {
        return Err(HttpError::invalid_value());
    }
    let models = s.models.models();
    let mut policy = s.models.policy.lock().await;
    let mut providers = Vec::new();
    for provider in &mut index.providers {
        let pool = pools
            .get(&provider.provider)
            .cloned()
            .unwrap_or(ProviderPool {
                provider: provider.provider.clone(),
                accounts: vec![],
            });
        let mut candidates: BTreeMap<_, _> = provider
            .models
            .iter()
            .map(|m| (m.id.clone(), m.clone()))
            .collect();
        for id in models
            .default
            .iter()
            .chain(models.front.iter())
            .chain(&models.fallback)
        {
            let qualified = if id.contains(':') {
                id.clone()
            } else {
                format!("{}:{id}", provider.provider)
            };
            if qualified
                .split_once(':')
                .is_some_and(|(name, model)| name == provider.provider && !model.is_empty())
            {
                candidates
                    .entry(qualified.clone())
                    .or_insert_with(|| IndexModel {
                        label: qualified
                            .split_once(':')
                            .map(|(_, model)| model.to_owned())
                            .unwrap_or_default(),
                        id: qualified,
                        recommended_for: None,
                        in_catalog: false,
                        tier: openside_proto::ModelTier::Reasoning,
                        strengths: "Explicitly configured model; capabilities not curated".into(),
                    });
            }
        }
        let models = candidates
            .into_iter()
            .map(|(id, metadata)| {
                let mut rows: Vec<_> = inventory
                    .accounts
                    .iter()
                    .filter(|account| account.provider == provider.provider)
                    .map(|account| {
                        let mut row =
                            policy.status(account, &id, pool.accounts.contains(&account.reference));
                        if !account.models.is_empty()
                            && !account.models.iter().any(|candidate| {
                                candidate == &id
                                    || id
                                        .split_once(':')
                                        .is_some_and(|(_, model)| model == candidate)
                            })
                        {
                            row.available = false;
                            row.reason = "Model is not in this account's reported catalog".into();
                        }
                        row
                    })
                    .collect();
                rows.sort_by_key(|row| {
                    pool.accounts
                        .iter()
                        .position(|reference| {
                            reference.source == row.source && reference.account_id == row.account_id
                        })
                        .unwrap_or(usize::MAX)
                });
                let selected = rows.iter().find(|row| row.available);
                CatalogModel {
                    recommended_for: metadata.recommended_for,
                    in_catalog: metadata.in_catalog,
                    tier: metadata.tier,
                    strengths: metadata.strengths,
                    id,
                    label: metadata.label,
                    available: selected.is_some(),
                    reason: selected.map(|row| row.reason.clone()).unwrap_or_else(|| {
                        if inventory.broker_failed
                            && pool.accounts.iter().any(|a| a.source == "omp-broker")
                        {
                            "Configured broker unavailable; no unlisted-source fallback".into()
                        } else {
                            "No eligible account in the configured pool".into()
                        }
                    }),
                    source: selected.map(|row| row.source.clone()),
                    account_id: selected.map(|row| row.account_id.clone()),
                    accounts: rows,
                }
            })
            .collect();
        providers.push(ModelProvider {
            provider: provider.provider.clone(),
            models,
            pool,
        });
    }
    providers.sort_by(|a, b| a.provider.cmp(&b.provider));
    Ok(ModelCatalog {
        providers,
        default_model: models.default,
        fast_model: models.front,
        fallback_models: models.fallback,
        cached_for_ms: 5000,
        truncated: false,
    })
}

pub(crate) async fn diagnostics(s: &Shared) -> Result<AccountsResponse, HttpError> {
    let inventory = inventory(s).await;
    let pools = pools(s, &inventory).await?;
    let mut policy = s.models.policy.lock().await;
    let mut sources = Vec::with_capacity(SOURCES.len());
    for source in SOURCES {
        let accounts: Vec<_> = inventory
            .accounts
            .iter()
            .filter(|a| a.reference.source == *source)
            .map(|a| {
                let row = policy.status(
                    a,
                    "",
                    pools
                        .get(&a.provider)
                        .is_some_and(|pool| pool.accounts.contains(&a.reference)),
                );
                AccountStatus {
                    id: a.reference.account_id.clone(),
                    label: a.label.clone(),
                    provider: a.provider.clone(),
                    models: a.models.clone(),
                    usage_windows: row.usage_windows,
                    blocked_until_ms: row.blocked_until_ms,
                    available: Some(row.available),
                    reason: Some(row.reason),
                }
            })
            .collect();
        let available = accounts.iter().any(|a| a.available == Some(true));
        sources.push(AccountSource {
            name: (*source).into(),
            kind: (*source).into(),
            available,
            reason: if *source == "omp-broker" && inventory.broker_failed {
                "Configured broker unavailable; fail closed"
            } else if available {
                "Accounts available in explicit provider pools"
            } else {
                "No eligible accounts in configured pools"
            }
            .into(),
            accounts,
        });
    }
    Ok(AccountsResponse {
        sources,
        cached_for_ms: 5000,
        truncated: false,
    })
}

async fn resolve(
    s: &Shared,
    account: &Account,
    force: bool,
    model: &str,
) -> Result<(CredentialAuth, u64), ()> {
    let credentials = s.models.credentials_config();
    match account.reference.source.as_str() {
        "vault" => {
            let vault = s.registry.vault.vault.lock().await;
            let vault = vault.as_ref().ok_or(())?;
            let version = vault
                .provider_keys(&context())
                .map_err(|_| ())?
                .into_iter()
                .find(|info| {
                    info.provider == account.provider && info.key_id == account.reference.account_id
                })
                .ok_or(())?
                .created_at_ms;
            let value = vault
                .with_provider_key(
                    &context(),
                    &account.provider,
                    &account.reference.account_id,
                    |value| value.to_owned(),
                )
                .map_err(|_| ())?;
            Ok((auth(value, &account.provider), version))
        }
        "claude-code" | "codex-cli" => {
            let result = s
                .managed_subscriptions
                .resolve(
                    s,
                    &account.reference.source,
                    &account.reference.account_id,
                    force,
                )
                .await
                .map_err(|_| ())?;
            Ok((result.auth, result.revision))
        }
        "omp-broker" => {
            let broker = Broker::configured(&credentials, s.broker_environment)
                .map_err(|_| ())?
                .ok_or(())?;
            if force {
                broker
                    .refresh(&account.reference.account_id)
                    .await
                    .map_err(|_| ())?;
            }
            let mut row = broker
                .accounts()
                .await
                .map_err(|_| ())?
                .into_iter()
                .find(|row| {
                    row.id == account.reference.account_id
                        && row.provider == account.provider
                        && row.enabled
                })
                .ok_or(())?;
            if row.auth.expires_at_ms.is_some_and(|expiry| expiry <= now()) {
                broker
                    .refresh(&account.reference.account_id)
                    .await
                    .map_err(|_| ())?;
                row = broker
                    .accounts()
                    .await
                    .map_err(|_| ())?
                    .into_iter()
                    .find(|row| {
                        row.id == account.reference.account_id
                            && row.provider == account.provider
                            && row.enabled
                    })
                    .ok_or(())?;
                if row.auth.expires_at_ms.is_some_and(|expiry| expiry <= now()) {
                    return Err(());
                }
            }
            if row
                .blocked_until_ms
                .is_some_and(|until| until > now() as f64)
                || row.model_blocks.iter().any(|(scope, until)| {
                    *until > now()
                        && (scope == model
                            || model.split_once(':').is_some_and(|(_, name)| name == scope))
                })
            {
                return Err(());
            }
            let revision = revision(&row.auth.value);
            Ok((row.auth, revision))
        }
        "env" | "builtin" => {
            let variables = if account.reference.source == "builtin" {
                names(&account.provider, &Value::Null)
            } else {
                names(&account.provider, &credentials)
            };
            let value = variables
                .iter()
                .filter(|name| {
                    account.reference.source == "builtin"
                        || env_id(&account.provider, name) == account.reference.account_id
                })
                .find_map(|name| env_value(name))
                .ok_or(())?;
            let revision = revision(&value);
            Ok((auth(value.to_string(), &account.provider), revision))
        }
        "command" => {
            let argv = credentials["command"][&account.provider]
                .as_array()
                .ok_or(())?;
            let argv = argv
                .iter()
                .map(|v| v.as_str().ok_or(()))
                .collect::<Result<Vec<_>, _>>()?;
            let (program, args) = argv.split_first().ok_or(())?;
            let mut child = tokio::process::Command::new(program)
                .args(args)
                .env_clear()
                .envs(openside_config::runtime_environment(&credentials))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| ())?;
            let timeout = credentials["command"]["timeout_seconds"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0)
                .unwrap_or(10.0)
                .clamp(0.1, 60.0);
            let mut bytes = Zeroizing::new(Vec::new());
            let result = tokio::time::timeout(Duration::from_secs_f64(timeout), async {
                child
                    .stdout
                    .take()
                    .ok_or(())?
                    .take(8193)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| ())?;
                if bytes.len() > 8192 || !child.wait().await.map_err(|_| ())?.success() {
                    return Err(());
                }
                Ok(())
            })
            .await
            .map_err(|_| ())?;
            result?;
            let value = std::str::from_utf8(&bytes).map_err(|_| ())?.trim();
            if value.is_empty() {
                return Err(());
            }
            Ok((auth(value.into(), &account.provider), revision(value)))
        }
        _ => Err(()),
    }
}

async fn permitted(
    s: &Shared,
    session: &SessionId,
    request: &CredentialRequest,
    pool: &ProviderPool,
) -> Result<Vec<ModelAccountRef>, CredentialError> {
    let invalid = || {
        error(
            s,
            CredentialErrorCode::InvalidCredentialRequest,
            "Invalid session/model credential request",
            None,
        )
    };
    if request.attempt >= MAX_CREDENTIAL_ATTEMPTS
        || request.request_id.is_empty()
        || request.request_id.len() > 128
        || !request
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
        || !request
            .model
            .split_once(':')
            .is_some_and(|(provider, model)| provider == request.provider && !model.is_empty())
    {
        return Err(invalid());
    }
    let state = s.db.state(session).await.map_err(|_| invalid())?;
    let models = s.models.models();
    let agent = crate::agents::for_session(s, &state).map_err(|_| invalid())?;
    if agent
        .as_ref()
        .is_some_and(|agent| agent.status != openside_proto::AgentStatus::Active)
    {
        return Err(invalid());
    }
    let effective_model =
        crate::engine::session_model(s, &state, agent.as_ref()).ok_or_else(invalid)?;
    let valid_model = if let Some(child) = &request.subagent {
        let child = crate::subagents::get(s, session, child)
            .await
            .map_err(|_| invalid())?;
        let policy = s.models.policy.lock().await;
        child.is_some_and(|child| {
            !child.status.is_terminal()
                && (child.model == request.model
                    || policy
                        .fallbacks
                        .get(session)
                        .is_some_and(|chain| chain.contains(&request.model)))
        })
    } else {
        let policy = s.models.policy.lock().await;
        if let Some(binding) = policy.models.get(session) {
            binding.contains(&request.model)
        } else {
            request.model == effective_model
                || models.fallback.contains(&request.model)
                || models.front.as_ref() == Some(&request.model)
        }
    };
    if !valid_model {
        return Err(invalid());
    }
    let accounts = if let Some(restriction) = agent.and_then(|agent| agent.account_pool) {
        restriction
            .into_iter()
            .filter(|account| pool.accounts.contains(account))
            .collect()
    } else {
        pool.accounts.clone()
    };
    Ok(accounts)
}

pub(crate) async fn credentials(
    s: &Shared,
    session: &SessionId,
    request: CredentialRequest,
) -> Result<CredentialSnapshot, CredentialError> {
    for _ in 0..4 {
        let generation = s.models.credentials_generation();
        let inventory = inventory(s).await;
        let pools = pools(s, &inventory).await.map_err(|_| {
            error(
                s,
                CredentialErrorCode::ProviderUnavailable,
                "Credential pool configuration unavailable",
                None,
            )
        })?;
        let pool = pools.get(&request.provider).ok_or_else(|| {
            error(
                s,
                CredentialErrorCode::AuthUnavailable,
                "No configured provider pool",
                None,
            )
        })?;
        let allowed = permitted(s, session, &request, pool).await?;
        if generation != s.models.credentials_generation() {
            continue;
        }
        let key = (session.clone(), request.request_id.clone());
        let affinity = (session.clone(), request.provider.clone());
        let (candidates, refresh) = {
            let mut policy = s.models.policy.lock().await;
            policy.reap();
            if !policy.can_lease(session)
                || (policy.attempts.len() >= 1024 && !policy.attempts.contains_key(&key))
            {
                return Err(error(
                    s,
                    CredentialErrorCode::InvalidCredentialRequest,
                    "Too many active credential requests",
                    None,
                ));
            }
            let attempt = policy.attempts.entry(key.clone()).or_default();
            attempt.touched = Some(tokio::time::Instant::now());
            if attempt.next != request.attempt
                || attempt.in_flight
                || (request.attempt > 0
                    && (attempt.provider != request.provider || attempt.model != request.model))
            {
                return Err(error(
                    s,
                    CredentialErrorCode::InvalidCredentialRequest,
                    "Attempt must follow its committed report acknowledgement",
                    None,
                ));
            }
            let tried = attempt.tried.clone();
            let refresh = attempt.refresh.clone();
            let preferred = policy
                .preferred
                .get(&affinity)
                .or_else(|| policy.sticky.get(&affinity))
                .cloned();
            let mut candidates: Vec<_> = inventory
                .accounts
                .iter()
                .filter(|a| {
                    a.provider == request.provider
                        && allowed.contains(&a.reference)
                        && (!tried.contains(&(a.reference.clone(), a.revision))
                            || refresh.as_ref() == Some(&a.reference))
                })
                .filter(|a| {
                    policy.status(a, &request.model, true).available
                        || (refresh.as_ref() == Some(&a.reference) && a.ready && a.enabled)
                })
                .cloned()
                .collect();
            candidates.sort_by(|a, b| {
                let rank = |a: &Account| {
                    let warm = refresh.as_ref() == Some(&a.reference)
                        || preferred.as_ref() == Some(&a.reference);
                    let measured = policy
                        .health
                        .get(&(a.provider.clone(), a.reference.clone()))
                        .into_iter()
                        .flat_map(|health| &health.windows)
                        .filter_map(|w| w.used_percent)
                        .max_by(f64::total_cmp);
                    (
                        !warm,
                        measured
                            .map(|used| (used * 1000.0) as u64)
                            .unwrap_or(100_000),
                        allowed
                            .iter()
                            .position(|r| r == &a.reference)
                            .unwrap_or(usize::MAX),
                    )
                };
                rank(a).cmp(&rank(b))
            });
            (candidates, refresh)
        };
        let retry = {
            let mut policy = s.models.policy.lock().await;
            inventory
                .accounts
                .iter()
                .filter(|a| a.provider == request.provider && allowed.contains(&a.reference))
                .filter_map(|account| {
                    policy
                        .status(account, &request.model, true)
                        .blocked_until_ms
                })
                .map(|time| time.min(u64::MAX as f64) as u64)
                .filter(|time| *time > now())
                .min()
        };
        for account in candidates {
            let resolved = resolve(
                s,
                &account,
                refresh.as_ref() == Some(&account.reference),
                &request.model,
            )
            .await;
            let Ok((auth, revision)) = resolved else {
                continue;
            };
            let permit = s.models.admit_credentials().await;
            if generation != s.models.credentials_generation() {
                drop(auth);
                drop(permit);
                break;
            }
            let mut policy = s.models.policy.lock().await;
            policy.reap();
            if !policy.can_lease(session) {
                return Err(error(
                    s,
                    CredentialErrorCode::InvalidCredentialRequest,
                    "Session credential lease capacity reached",
                    None,
                ));
            }
            let attempt = policy.attempts.entry(key.clone()).or_default();
            if attempt
                .tried
                .contains(&(account.reference.clone(), revision))
            {
                continue;
            }
            if attempt.next != request.attempt || attempt.in_flight {
                return Err(error(
                    s,
                    CredentialErrorCode::InvalidCredentialRequest,
                    "Concurrent attempt is already leased",
                    None,
                ));
            }
            attempt.provider = request.provider.clone();
            attempt.model = request.model.clone();
            attempt.in_flight = true;
            attempt.refresh = None;
            attempt.touched = Some(tokio::time::Instant::now());
            attempt.tried.insert((account.reference.clone(), revision));
            let lease_id = crate::models_api::opaque_id();
            policy.next_observation = policy.next_observation.saturating_add(1);
            let observation = policy.next_observation;
            policy.leases.insert(
                lease_id.clone(),
                Lease {
                    session: session.clone(),
                    request: request.request_id.clone(),
                    provider: request.provider.clone(),
                    model: request.model.clone(),
                    account: account.reference.clone(),
                    revision,
                    subagent: request.subagent.clone(),
                    expires: tokio::time::Instant::now() + LEASE_TTL,
                    issued_at_ms: now(),
                    observation,
                },
            );
            let health = policy
                .health
                .entry((account.provider.clone(), account.reference.clone()))
                .or_default();
            if health.revision != revision {
                *health = Health {
                    revision,
                    ..Default::default()
                };
            }
            if policy
                .registered
                .insert((account.reference.clone(), revision))
            {
                let value = openside_vault::Secret::new(auth.value.clone());
                let identity = openside_vault::Secret::new(
                    auth.upstream_account_id.clone().unwrap_or_default(),
                );
                s.registry
                    .redactors
                    .write()
                    .push(openside_vault::Redactor::new([&value, &identity]));
            }
            let snapshot = CredentialSnapshot {
                request_id: request.request_id.clone(),
                lease_id,
                generation,
                account: account.reference,
                auth,
            };
            drop(policy);
            drop(permit);
            return Ok(snapshot);
        }
        if generation != s.models.credentials_generation() {
            continue;
        }
        s.models.policy.lock().await.attempts.remove(&key);
        return Err(error(
            s,
            if retry.is_some() {
                CredentialErrorCode::AccountBlocked
            } else if inventory.broker_failed {
                CredentialErrorCode::ProviderUnavailable
            } else {
                CredentialErrorCode::AuthUnavailable
            },
            "No eligible account in the configured pool; no unlisted-source fallback",
            retry,
        ));
    }
    Err(error(
        s,
        CredentialErrorCode::AuthUnavailable,
        "Credentials changed during request admission",
        None,
    ))
}

pub(crate) async fn report(
    s: &Shared,
    session: &SessionId,
    report: CredentialReport,
) -> Result<CredentialReportAck, CredentialError> {
    let invalid = || {
        error(
            s,
            CredentialErrorCode::InvalidCredentialRequest,
            "Invalid credential report lease",
            None,
        )
    };
    if report.usage_windows.len() > 16
        || report.usage_windows.iter().any(|window| {
            window.name.len() > 64
                || window
                    .used_percent
                    .is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v))
                || window
                    .reset_at_ms
                    .is_some_and(|v| !v.is_finite() || v < 0.0)
        })
    {
        return Err(invalid());
    }
    let preview = {
        let mut policy = s.models.policy.lock().await;
        policy.reap();
        if let Some(ack) = policy
            .acknowledgements
            .get(&(session.clone(), report.lease_id.clone()))
        {
            if ack.request_id != report.request_id {
                return Err(invalid());
            }
            return Ok(copy_ack(ack));
        }
        let lease = policy
            .leases
            .get(&report.lease_id)
            .filter(|lease| &lease.session == session && lease.request == report.request_id)
            .ok_or_else(invalid)?;
        lease.clone()
    };
    let inventory = inventory(s).await;
    let configured = s.models.credentials_config();
    let pool = ProviderPool {
        provider: preview.provider.clone(),
        accounts: serde_json::from_value(configured["pools"][&preview.provider].clone())
            .unwrap_or_default(),
    };
    let allowed = permitted(
        s,
        session,
        &CredentialRequest {
            request_id: preview.request.clone(),
            provider: preview.provider.clone(),
            model: preview.model.clone(),
            attempt: 0,
            subagent: preview.subagent.clone(),
        },
        &pool,
    )
    .await
    .unwrap_or_default();
    let mut policy = s.models.policy.lock().await;
    if let Some(ack) = policy
        .acknowledgements
        .get(&(session.clone(), report.lease_id.clone()))
    {
        return Ok(copy_ack(ack));
    }
    let lease = policy.leases.remove(&report.lease_id).ok_or_else(invalid)?;
    let key = (session.clone(), report.request_id.clone());
    let changes_shared = !report.usage_windows.is_empty()
        || matches!(
            report.outcome,
            CredentialOutcome::Auth
                | CredentialOutcome::RateLimit
                | CredentialOutcome::Quota
                | CredentialOutcome::Policy
        );
    let shared_allowed = !changes_shared || policy.shared_report_allowed(session);
    let current = inventory
        .accounts
        .iter()
        .find(|a| a.provider == lease.provider && a.reference == lease.account)
        .is_some_and(|a| a.enabled && (a.revision == 0 || a.revision == lease.revision))
        && policy
            .health
            .get(&(lease.provider.clone(), lease.account.clone()))
            .is_some_and(|health| health.revision == lease.revision);
    let fresh = policy
        .health
        .get(&(lease.provider.clone(), lease.account.clone()))
        .is_some_and(|health| health.observation <= lease.observation);
    let mut retry = None;
    if current {
        let health = policy
            .health
            .entry((lease.provider.clone(), lease.account.clone()))
            .or_default();
        health.requests = health.requests.saturating_add(1);
        if let Some(usage) = &report.usage {
            health.input_tokens = health.input_tokens.saturating_add(usage.input_tokens);
            health.output_tokens = health.output_tokens.saturating_add(usage.output_tokens);
            health.cache_read_tokens = health
                .cache_read_tokens
                .saturating_add(usage.cache_read_tokens);
            health.cache_write_tokens = health
                .cache_write_tokens
                .saturating_add(usage.cache_write_tokens);
        }
    }
    if current && fresh && shared_allowed {
        let health = policy
            .health
            .entry((lease.provider.clone(), lease.account.clone()))
            .or_default();
        health.observation = lease.observation;
        if !report.usage_windows.is_empty() {
            let maximum = lease.issued_at_ms.saturating_add(MAX_REPORTED_BACKOFF_MS);
            health.windows = report
                .usage_windows
                .iter()
                .enumerate()
                .map(|(index, w)| AccountUsageWindow {
                    name: format!("Window {}", index + 1),
                    used_percent: w.used_percent,
                    reset_at_ms: w.reset_at_ms.map(|time| time.min(maximum as f64)),
                })
                .collect();
            health.windows_until_ms = Some(
                health
                    .windows
                    .iter()
                    .filter_map(|window| window.reset_at_ms)
                    .map(|time| time.min(u64::MAX as f64) as u64)
                    .max()
                    .unwrap_or_else(|| lease.issued_at_ms.saturating_add(REPORTED_BACKOFF_MS)),
            );
        }
        if report.outcome == CredentialOutcome::Success {
            health
                .blocks
                .retain(|_, block| block.reason != "Account needs credential check");
        }
        if matches!(
            report.outcome,
            CredentialOutcome::RateLimit
                | CredentialOutcome::Quota
                | CredentialOutcome::Policy
                | CredentialOutcome::Auth
        ) {
            let backoff = if report.outcome == CredentialOutcome::Auth {
                REPORTED_BACKOFF_MS
            } else {
                report
                    .retry_after_ms
                    .unwrap_or(REPORTED_BACKOFF_MS)
                    .min(MAX_REPORTED_BACKOFF_MS)
            };
            retry = Some(lease.issued_at_ms.saturating_add(backoff));
            if report.outcome == CredentialOutcome::Quota {
                retry = retry.max(
                    health
                        .windows
                        .iter()
                        .filter_map(|w| w.reset_at_ms)
                        .filter(|time| *time > now() as f64)
                        .map(|time| time.min(u64::MAX as f64) as u64)
                        .max(),
                );
            }
            let scope = if report.scope == CredentialScope::Model
                && report.outcome != CredentialOutcome::Auth
            {
                Some(lease.model.clone())
            } else {
                None
            };
            let reason = match report.outcome {
                CredentialOutcome::Auth => "Account needs credential check",
                CredentialOutcome::RateLimit => "Account rate-limited",
                CredentialOutcome::Quota => "Account quota exhausted",
                _ => "Account cannot access this model",
            };
            if retry.is_some_and(|time| time > now()) {
                let block = health.blocks.entry(scope).or_insert(Block {
                    until: retry,
                    reason,
                });
                block.until = block.until.max(retry);
                block.reason = reason;
            }
        }
    }
    let attempt = policy.attempts.get_mut(&key).ok_or_else(invalid)?;
    attempt.in_flight = false;
    attempt.next = attempt.next.saturating_add(1);
    attempt.touched = Some(tokio::time::Instant::now());
    let refreshable = matches!(
        lease.account.source.as_str(),
        "claude-code" | "codex-cli" | "omp-broker"
    );
    let refresh = current
        && report.outcome == CredentialOutcome::Auth
        && refreshable
        && attempt
            .refreshed
            .insert((lease.account.clone(), lease.revision));
    let mut action = if matches!(
        report.outcome,
        CredentialOutcome::Success | CredentialOutcome::Cancelled
    ) {
        CredentialAction::Complete
    } else if report.stream_started
        || report.outcome == CredentialOutcome::Transient
        || attempt.next >= MAX_CREDENTIAL_ATTEMPTS
    {
        CredentialAction::Stop
    } else if refresh {
        attempt.refresh = Some(lease.account.clone());
        CredentialAction::Refresh
    } else {
        CredentialAction::Rotate
    };
    let tried = attempt.tried.clone();
    if action == CredentialAction::Rotate
        && !inventory.accounts.iter().any(|account| {
            account.provider == lease.provider
                && account.reference != lease.account
                && allowed.contains(&account.reference)
                && !tried.contains(&(account.reference.clone(), account.revision))
                && policy.status(account, &lease.model, true).available
        })
    {
        action = CredentialAction::Stop;
    }
    if report.outcome == CredentialOutcome::Success && current {
        policy.sticky.insert(
            (session.clone(), lease.provider.clone()),
            lease.account.clone(),
        );
    }
    if matches!(action, CredentialAction::Complete | CredentialAction::Stop) {
        policy.attempts.remove(&key);
    }
    let reason = if !current || !fresh {
        Some("Stale lease effects ignored".into())
    } else if !shared_allowed {
        Some("Shared account health report rate limited".into())
    } else {
        None
    };
    let ack = CredentialReportAck {
        request_id: report.request_id,
        lease_id: report.lease_id,
        generation: s.models.credentials_generation(),
        action,
        retry_at_ms: retry,
        reason,
    };
    if policy.acknowledgements.len() >= 1024 {
        policy.acknowledgements.clear();
    }
    policy
        .acknowledgements
        .insert((session.clone(), ack.lease_id.clone()), copy_ack(&ack));
    drop(policy);
    s.models.invalidate(s).await;
    if current
        && fresh
        && shared_allowed
        && report.outcome != CredentialOutcome::Auth
        && lease.account.source == "omp-broker"
    {
        if let Some(until) = retry.filter(|until| *until > now()) {
            if let Ok(Some(broker)) =
                Broker::configured(&s.models.credentials_config(), s.broker_environment)
            {
                let _ = broker
                    .block(
                        &lease.account.account_id,
                        &lease.provider,
                        (report.scope == CredentialScope::Model).then_some(lease.model.as_str()),
                        until,
                    )
                    .await;
            }
        }
    }
    Ok(ack)
}

fn copy_ack(ack: &CredentialReportAck) -> CredentialReportAck {
    CredentialReportAck {
        request_id: ack.request_id.clone(),
        lease_id: ack.lease_id.clone(),
        generation: ack.generation,
        action: ack.action,
        retry_at_ms: ack.retry_at_ms,
        reason: ack.reason.clone(),
    }
}

pub(crate) async fn list_pools(
    State(s): State<Shared>,
) -> Result<Json<ProviderPoolList>, HttpError> {
    let inventory = inventory(&s).await;
    Ok(Json(ProviderPoolList {
        pools: pools(&s, &inventory).await?.into_values().collect(),
    }))
}
pub(crate) async fn update_pool(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(provider): Path<String>,
    Json(request): Json<UpdateProviderPoolRequest>,
) -> Result<Json<ProviderPool>, HttpError> {
    crate::models_api::person(&caller)?;
    tokio::spawn(async move {
        crate::models_api::authorized(&s, &caller)?;
        let inventory = inventory(&s).await;
        let mut seen = HashSet::new();
        if provider.is_empty()
            || provider.len() > 64
            || !provider
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || request.accounts.len() > 64
            || request.accounts.iter().any(|reference| {
                !seen.insert(reference)
                    || !inventory.accounts.iter().any(|account| {
                        account.provider == provider && &account.reference == reference
                    })
            })
        {
            return Err(HttpError::invalid_value());
        }
        let _writer = crate::settings_api::WRITE.lock().await;
        let change = s.models.change_credentials().await;
        crate::models_api::authorized(&s, &caller)?;
        let pool = ProviderPool {
            provider: provider.clone(),
            accounts: request.accounts,
        };
        let mut current = crate::settings_api::stored(&s).await?;
        if !current.credentials.is_object() {
            current.credentials = json!({});
        }
        current.credentials["pools"][&provider] = json!(pool.accounts);
        crate::settings_api::persist(
            &s,
            serde_json::to_value(current).map_err(|_| HttpError::internal())?,
        )
        .await?;
        s.models
            .set_pools(&BTreeMap::from([(provider, pool.clone())]));
        drop(change);
        s.models.invalidate(&s).await;
        drop(_writer);
        crate::models_api::auto_select(&s).await?;
        Ok(Json(pool))
    })
    .await
    .map_err(|_| HttpError::internal())?
}
pub(crate) async fn set_preference(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<UpdateSessionAccountRequest>,
) -> Result<Json<SessionAccountSelection>, HttpError> {
    crate::models_api::authorized(&s, &caller)?;
    let session = SessionId::new(id);
    let state =
        s.db.state(&session)
            .await
            .map_err(|_| HttpError::not_found())?;
    let agent = crate::agents::for_session(&s, &state).map_err(|_| HttpError::internal())?;
    let model = crate::engine::session_model(&s, &state, agent.as_ref())
        .ok_or_else(HttpError::invalid_value)?;
    let provider = model
        .split_once(':')
        .map(|(provider, _)| provider.to_owned())
        .ok_or_else(HttpError::invalid_value)?;
    let inventory = inventory(&s).await;
    let pools = pools(&s, &inventory).await?;
    let pool = pools.get(&provider).ok_or_else(HttpError::invalid_value)?;
    let allowed = permitted(
        &s,
        &session,
        &CredentialRequest {
            request_id: "preference".into(),
            provider: provider.clone(),
            model,
            attempt: 0,
            subagent: None,
        },
        pool,
    )
    .await
    .map_err(|_| HttpError::invalid_value())?;
    if request
        .account
        .as_ref()
        .is_some_and(|reference| !allowed.contains(reference))
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "account_not_in_pool"));
    }
    let mut policy = s.models.policy.lock().await;
    let key = (session.clone(), provider.clone());
    if let Some(account) = &request.account {
        policy.preferred.insert(key, account.clone());
    } else {
        policy.preferred.remove(&key);
        policy.sticky.remove(&key);
    }
    Ok(Json(SessionAccountSelection {
        session_id: session,
        provider,
        explicit: request.account.is_some(),
        account: request.account,
        effective: "next_request".into(),
    }))
}

pub(crate) async fn release_session(s: &Shared, session: &SessionId) {
    s.models.policy.lock().await.release(session);
}

pub(crate) async fn reap(s: &Shared) {
    s.models.policy.lock().await.reap();
}

pub(crate) async fn bind_session(
    s: &Shared,
    session: &SessionId,
    model: &str,
    fallback: &[String],
) {
    let mut allowed = Vec::with_capacity(fallback.len() + 2);
    allowed.push(model.to_owned());
    allowed.extend_from_slice(fallback);
    if let Some(front) = s.models.front_model() {
        allowed.push(front);
    }
    let mut policy = s.models.policy.lock().await;
    policy.models.insert(session.clone(), allowed);
    policy.fallbacks.insert(session.clone(), fallback.to_vec());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(session: &SessionId, id: &str) -> Lease {
        Lease {
            session: session.clone(),
            request: id.into(),
            provider: "anthropic".into(),
            model: "anthropic:test-ready".into(),
            account: reference("vault", "key1"),
            revision: 1,
            subagent: None,
            expires: tokio::time::Instant::now() + LEASE_TTL,
            issued_at_ms: now(),
            observation: 1,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn lease_capacity_is_per_session_and_expiry_and_disconnect_release_it() {
        let session = SessionId::new("one");
        let other = SessionId::new("two");
        let mut policy = Policy::default();
        for index in 0..SESSION_LEASE_LIMIT {
            assert!(policy.can_lease(&session));
            policy
                .leases
                .insert(index.to_string(), lease(&session, &index.to_string()));
        }
        assert!(!policy.can_lease(&session));
        assert!(policy.can_lease(&other));
        policy.leases.insert("other".into(), lease(&other, "other"));
        policy.release(&session);
        assert!(policy.can_lease(&session));
        assert_eq!(policy.leases.len(), 1);
        tokio::time::advance(LEASE_TTL + Duration::from_secs(1)).await;
        policy.reap();
        assert!(policy.leases.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn shared_health_reports_have_a_per_session_rate_limit_and_reset() {
        let session = SessionId::new("one");
        let other = SessionId::new("two");
        let mut policy = Policy::default();
        for _ in 0..8 {
            assert!(policy.shared_report_allowed(&session));
        }
        assert!(!policy.shared_report_allowed(&session));
        assert!(policy.shared_report_allowed(&other));
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(policy.shared_report_allowed(&session));
    }

    #[test]
    fn broker_model_blocks_are_scoped_and_advisory_unknown_windows_expire() {
        let account = Account {
            provider: "anthropic".into(),
            reference: reference("omp-broker", "1"),
            label: "Broker account1".into(),
            enabled: true,
            ready: true,
            reason: "Ready".into(),
            models: vec![],
            reserve: 10,
            windows: vec![],
            blocked_until: None,
            model_blocks: vec![("test-ready".into(), now() + 60_000)],
            revision: 1,
        };
        let mut policy = Policy::default();
        assert!(
            !policy
                .status(&account, "anthropic:test-ready", true)
                .available
        );
        assert!(
            policy
                .status(&account, "anthropic:test-second", true)
                .available
        );
        let health = policy.health(&account);
        health.windows = vec![AccountUsageWindow {
            name: "Window1".into(),
            used_percent: Some(100.0),
            reset_at_ms: None,
        }];
        health.windows_until_ms = Some(now().saturating_sub(1));
        assert!(
            policy
                .status(&account, "anthropic:test-second", true)
                .available
        );
        assert!(policy
            .status(&account, "anthropic:test-second", true)
            .usage_windows
            .is_empty());
    }
}
