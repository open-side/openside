use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    time::{Duration, Instant},
};

use axum::{
    body::{to_bytes, Bytes},
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use openside_channels::{AccountSecrets, DaemonClient, Running};
use openside_config::{ChannelAccountConfig, ChannelKind, ChannelOptions, ChannelsConfig};
use openside_proto::{
    AddChannelAccountRequest, ChannelAccount, ChannelAccountList, ChannelAccountOptions,
    ChannelAccountStatus as Status, ChannelAccountTest, ChannelCredentials, DeviceKind,
    UpdateChannelAccountRequest,
};
use openside_vault::{AuditContext, Entry, Error, Secret};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

type Key = (ChannelKind, String);
const MAX_HOOK_BODY: usize = 256 * 1024;
const HOOK_RATE: usize = 120;
const SIGNATURE_WINDOW: Duration = Duration::from_secs(300);
#[derive(Default)]
struct HookLimits {
    requests: VecDeque<Instant>,
    seen: HashMap<[u8; 32], Instant>,
}
#[derive(Default)]
struct Supervisor {
    daemon: Option<DaemonClient>,
    device_id: Option<String>,
    listen: Option<SocketAddr>,
    running: HashMap<Key, Running>,
    status: HashMap<Key, Status>,
}
pub(crate) struct Service {
    live: RwLock<ChannelsConfig>,
    supervisor: Mutex<Supervisor>,
    hooks: Mutex<HashMap<Key, HookLimits>>,
}
pub(crate) struct CredentialChange<'a> {
    _permit: tokio::sync::MutexGuard<'a, Supervisor>,
}
impl Service {
    pub(crate) fn new(c: &ChannelsConfig) -> Self {
        Self {
            live: RwLock::new(c.clone()),
            supervisor: Mutex::new(Supervisor::default()),
            hooks: Mutex::new(HashMap::new()),
        }
    }
    pub(crate) fn config(&self) -> ChannelsConfig {
        self.live.read().clone()
    }
    pub(crate) async fn change_credentials(&self) -> CredentialChange<'_> {
        let mut supervisor = self.supervisor.lock().await;
        for (key, running) in std::mem::take(&mut supervisor.running) {
            running.stop();
            running.join().await;
            supervisor.status.insert(key, Status::VaultLocked);
        }
        CredentialChange {
            _permit: supervisor,
        }
    }
    pub(crate) async fn stop_all(&self) {
        let _change = self.change_credentials().await;
    }
    pub(crate) async fn shutdown(&self, s: &Shared) {
        let mut change = self.change_credentials().await;
        let id = change._permit.device_id.take();
        change._permit.daemon = None;
        change._permit.listen = None;
        drop(change);
        if let Some(id) = id {
            let _ = s.db.revoke(id).await;
        }
    }
}
fn context<'a>(who: &'a str) -> AuditContext<'a> {
    AuditContext {
        who,
        session_id: None,
        tool_call_id: None,
    }
}
fn invalid() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_channel_account")
}
fn unavailable() -> HttpError {
    HttpError(StatusCode::CONFLICT, "vault_unavailable")
}
fn kind(kind: &str) -> Result<ChannelKind, HttpError> {
    match kind {
        "telegram" => Ok(ChannelKind::Telegram),
        "slack" => Ok(ChannelKind::Slack),
        "discord" => Ok(ChannelKind::Discord),
        _ => Err(HttpError(StatusCode::BAD_REQUEST, "invalid_channel_kind")),
    }
}
pub(crate) fn binding_owner(
    s: &Shared,
    service: &str,
    account_id: &str,
) -> Result<Option<String>, HttpError> {
    let service = kind(service)?;
    let config = s.channels.live.read();
    let account = config
        .accounts
        .iter()
        .find(|a| a.kind == service && a.id == account_id)
        .ok_or(HttpError(
            StatusCode::NOT_FOUND,
            "channel_account_not_found",
        ))?;
    Ok(account.agent_id.clone())
}

fn resolve_binding(
    account: &ChannelAccountConfig,
    profile_owner: Option<&str>,
    active: impl FnOnce(&str) -> Result<Option<bool>, HttpError>,
) -> Result<String, HttpError> {
    let configured = account.agent_id.as_deref();
    if configured.zip(profile_owner).is_some_and(|(a, b)| a != b) {
        return Err(HttpError(StatusCode::CONFLICT, "channel_binding_conflict"));
    }
    let target = configured.or(profile_owner).unwrap_or("chief");
    match active(target)? {
        None => Err(HttpError(StatusCode::NOT_FOUND, "agent_not_found")),
        Some(false) => Err(HttpError(StatusCode::CONFLICT, "agent_retired")),
        Some(true) => Ok(target.to_owned()),
    }
}

fn bound_agent(s: &Shared, account: &ChannelAccountConfig) -> Result<String, HttpError> {
    let owner = s
        .agents
        .channel_owner(account.kind.as_str(), &account.id)
        .map_err(|_| HttpError::internal())?;
    resolve_binding(account, owner.as_deref(), |id| {
        s.agents
            .get(id)
            .map(|agent| agent.map(|agent| agent.status == openside_proto::AgentStatus::Active))
            .map_err(|_| HttpError::internal())
    })
}

/// Caller retains WRITE across its commit, but must release the agent registry gate first.
pub(crate) async fn binding_changed(
    s: &Shared,
    service: &str,
    account_id: &str,
) -> Result<(), HttpError> {
    let service = kind(service)?;
    let mut account = {
        let config = s.channels.live.read();
        config
            .accounts
            .iter()
            .find(|a| a.kind == service && a.id == account_id)
            .cloned()
            .ok_or(HttpError(
                StatusCode::NOT_FOUND,
                "channel_account_not_found",
            ))?
    };
    let key = (service, account_id.to_owned());
    let mut supervisor = s.channels.supervisor.lock().await;
    stop(&mut supervisor, &key).await;
    if !account.enabled {
        supervisor.status.insert(key, Status::Disabled);
        return Ok(());
    }
    supervisor.status.insert(key, Status::Unavailable);
    account.agent_id = Some(bound_agent(s, &account)?);
    apply(s, &mut supervisor, &account, true).await;
    Ok(())
}
fn person(c: &Caller) -> Result<(), HttpError> {
    if c.0.is_some() && c.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_device_required"));
    }
    Ok(())
}
fn authorized(s: &Shared, c: &Caller) -> Result<(), HttpError> {
    if c.0.as_deref().is_some_and(|id| s.remote.is_revoked(id)) {
        return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(())
}
fn options(o: ChannelAccountOptions) -> Result<ChannelOptions, HttpError> {
    Ok(ChannelOptions {
        api_base: o.api_base,
        listen: o
            .listen
            .map(|s| s.parse().map_err(|_| invalid()))
            .transpose()?,
        public_key: o.public_key,
        allowed_chats: o.allowed_chats,
        allowed_users: o.allowed_users,
        allowed_workspaces: o.allowed_workspaces,
        allowed_channels: o.allowed_channels,
        allowed_guilds: o.allowed_guilds,
        poll_timeout_secs: o.poll_timeout_secs.unwrap_or(30),
    })
}
fn public_options(o: &ChannelOptions) -> ChannelAccountOptions {
    ChannelAccountOptions {
        api_base: o.api_base.clone(),
        listen: o.listen.map(|a| a.to_string()),
        public_key: o.public_key.clone(),
        allowed_chats: o.allowed_chats.clone(),
        allowed_users: o.allowed_users.clone(),
        allowed_workspaces: o.allowed_workspaces.clone(),
        allowed_channels: o.allowed_channels.clone(),
        allowed_guilds: o.allowed_guilds.clone(),
        poll_timeout_secs: Some(o.poll_timeout_secs),
    }
}
fn validate(c: &Caller, a: &ChannelAccountConfig) -> Result<(), HttpError> {
    a.validate().map_err(|_| invalid())?;
    let base = url::Url::parse(a.api_base()).map_err(|_| invalid())?;
    if !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || !(base.scheme() == "https"
            || (base.scheme() == "http"
                && base.host_str().is_some_and(|h| {
                    h == "localhost"
                        || h.parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                })))
    {
        return Err(invalid());
    }
    if a.agent_id
        .as_ref()
        .is_some_and(|id| !openside_config::valid_channel_id(id))
    {
        return Err(invalid());
    }
    if a.kind == ChannelKind::Discord {
        openside_channels::discord::parse_public_key(
            a.options.public_key.as_deref().ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?;
    }
    // Network admins cannot turn an account into a credential-exfiltration destination.
    if c.0.is_some()
        && (a.api_base() != a.kind.api_base()
            || a.options.listen.is_some_and(|addr| {
                !addr.ip().is_loopback() || (addr.port() != 0 && addr.port() < 1024)
            }))
    {
        return Err(HttpError(StatusCode::FORBIDDEN, "local_socket_required"));
    }
    Ok(())
}
fn entry(
    a: &ChannelAccountConfig,
    mut credentials: ChannelCredentials,
) -> Result<Entry, HttpError> {
    let now =
        u64::try_from(chrono::Utc::now().timestamp_millis()).map_err(|_| HttpError::internal())?;
    Entry::channel_secret(
        a.kind.as_str(),
        &a.id,
        Secret::new(std::mem::take(&mut credentials.token)),
        credentials.signing_secret.take().map(Secret::new),
        now,
    )
    .map_err(|_| invalid())
}
async fn save_secret(s: &Shared, c: &Caller, entry: Entry) -> Result<(), HttpError> {
    let service = &s.registry.vault;
    let mut guard = service.vault.lock().await;
    authorized(s, c)?;
    if guard.is_none() {
        let probe = crate::tools::vault::Service::probe(s).await;
        if !probe.backend.is_ok_and(|b| b.is_keyring()) {
            return Err(unavailable());
        }
        service
            .setup(s, &mut guard, c.id(), None)
            .await
            .map_err(|_| unavailable())?;
    }
    let vault = guard.as_mut().ok_or_else(unavailable)?;
    s.registry.redactors.write().push(entry.redactor());
    vault
        .set_channel_secret(&context(c.id()), entry)
        .map_err(|e| match e {
            Error::Locked => unavailable(),
            Error::ItemExists => HttpError(StatusCode::CONFLICT, "channel_secret_conflict"),
            _ => HttpError::internal(),
        })?;
    Ok(())
}
async fn secrets(s: &Shared, a: &ChannelAccountConfig) -> Result<AccountSecrets, Status> {
    if a.legacy_secret.is_some() {
        return openside_channels::legacy_secrets(a, &openside_channels::process_env)
            .await
            .map_err(|_| Status::Unavailable);
    }
    let guard = s.registry.vault.vault.lock().await;
    let vault = guard.as_ref().ok_or(Status::VaultLocked)?;
    vault
        .with_channel_secret(
            &context("opensided"),
            a.kind.as_str(),
            &a.id,
            |token, signing| AccountSecrets {
                token: openside_channels::Secret::new(token),
                signing_secret: signing.map(openside_channels::Secret::new),
            },
        )
        .map_err(|e| {
            if e == Error::Locked {
                Status::VaultLocked
            } else {
                Status::Unavailable
            }
        })
}
async fn stop(supervisor: &mut Supervisor, key: &Key) {
    if let Some(running) = supervisor.running.remove(key) {
        running.stop();
        running.join().await;
    }
}
async fn client(s: &Shared, supervisor: &mut Supervisor) -> Result<DaemonClient, Status> {
    if let Some(client) = &supervisor.daemon {
        return Ok(client.clone());
    }
    let address = supervisor.listen.ok_or(Status::RestartRequired)?;
    use openside_proto::CapabilityScope;
    let (id, token) =
        s.db.pair_as(
            "Channel adapters".into(),
            vec![
                CapabilityScope::Read,
                CapabilityScope::Steer,
                CapabilityScope::Approve,
            ],
            DeviceKind::PersonDevice,
        )
        .await
        .map_err(|_| Status::Unavailable)?;
    match DaemonClient::new(
        &format!("http://{address}"),
        openside_channels::Secret::new(token),
    ) {
        Ok(client) => {
            supervisor.device_id = Some(id);
            supervisor.daemon = Some(client.clone());
            Ok(client)
        }
        Err(_) => {
            let _ = s.db.revoke(id).await;
            Err(Status::Unavailable)
        }
    }
}
async fn apply(
    s: &Shared,
    supervisor: &mut Supervisor,
    a: &ChannelAccountConfig,
    check: bool,
) -> Status {
    let key = (a.kind, a.id.clone());
    // Resolve the committed profile every time; a retired owner cannot become the chief.
    let mut account = a.clone();
    let owner = bound_agent(s, a);
    if let Ok(id) = &owner {
        account.agent_id = Some(id.clone());
    }
    let a = &account;
    stop(supervisor, &key).await;
    let status = if !a.enabled {
        Status::Disabled
    } else if a.legacy_secret.is_some() {
        Status::RestartRequired
    } else if owner.is_err() {
        Status::Unavailable
    } else {
        match client(s, supervisor).await {
            Err(status) => status,
            Ok(daemon) => match secrets(s, a).await {
                Err(status) => status,
                Ok(credentials) => {
                    let tested = if check {
                        openside_channels::test_account(
                            a,
                            &credentials,
                            &s.config.loaded.config.network.allow,
                        )
                        .await
                    } else {
                        Ok(())
                    };
                    match tested {
                        Err(openside_channels::ChannelError::Refused(reason))
                            if reason == "expired" =>
                        {
                            Status::Expired
                        }
                        Err(_) => Status::Unavailable,
                        Ok(()) => match openside_channels::start_account(
                            &s.config.loaded,
                            a,
                            credentials,
                            daemon,
                            CancellationToken::new(),
                        )
                        .await
                        {
                            Ok(running) => {
                                supervisor.running.insert(key.clone(), running);
                                Status::Connected
                            }
                            Err(_) => Status::Unavailable,
                        },
                    }
                }
            },
        }
    };
    supervisor.status.insert(key, status);
    status
}
async fn metadata(s: &Shared, supervisor: &Supervisor, a: &ChannelAccountConfig) -> ChannelAccount {
    let key = (a.kind, a.id.clone());
    let secret_present = if a.legacy_secret.is_some() {
        true
    } else {
        s.registry
            .vault
            .vault
            .lock()
            .await
            .as_ref()
            .is_some_and(|v| v.is_channel_secret(&format!("channel:{}:{}", a.kind.as_str(), a.id)))
    };
    let status = supervisor
        .status
        .get(&key)
        .copied()
        .unwrap_or(if a.enabled {
            Status::RestartRequired
        } else {
            Status::Disabled
        });
    let receiver_address = supervisor
        .running
        .get(&key)
        .and_then(|r| r.slack_addr.or(r.discord_addr))
        .map(|a| a.to_string());
    let effective_agent_id = supervisor
        .running
        .get(&key)
        .and_then(Running::agent_id)
        .map(str::to_owned);
    let webhook_url = if a.kind == ChannelKind::Telegram || a.legacy_secret.is_some() {
        None
    } else {
        crate::remote::effective(s)
            .public_url
            .or_else(|| supervisor.listen.map(|addr| format!("http://{addr}")))
            .map(|base| {
                format!(
                    "{}/v1/hooks/{}/{}",
                    base.trim_end_matches('/'),
                    a.kind.as_str(),
                    a.id
                )
            })
    };
    ChannelAccount {
        id: a.id.clone(),
        kind: a.kind.as_str().into(),
        label: a.label.clone(),
        enabled: a.enabled,
        agent_id: a.agent_id.clone(),
        options: public_options(&a.options),
        secret_present,
        credential_source: if a.legacy_secret.is_some() {
            "legacy"
        } else {
            "vault"
        }
        .into(),
        status,
        restart_required: status == Status::RestartRequired,
        receiver_address,
        webhook_url,
        effective_agent_id,
    }
}
async fn persist(s: &Shared, config: ChannelsConfig) -> Result<(), HttpError> {
    let mut stored = crate::settings_api::stored(s).await?;
    stored.channels = config.clone();
    crate::settings_api::persist(
        s,
        serde_json::to_value(stored).map_err(|_| HttpError::internal())?,
    )
    .await?;
    *s.channels.live.write() = config;
    Ok(())
}
pub(crate) async fn get(State(s): State<Shared>) -> Result<Json<ChannelAccountList>, HttpError> {
    let accounts = s.channels.config().accounts;
    let supervisor = s.channels.supervisor.lock().await;
    let mut rows = Vec::with_capacity(accounts.len());
    for a in &accounts {
        rows.push(metadata(&s, &supervisor, a).await);
    }
    Ok(Json(ChannelAccountList { accounts: rows }))
}
pub(crate) async fn add(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Path(k): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<ChannelAccount>), HttpError> {
    person(&c)?;
    let k = kind(&k)?;
    let r: AddChannelAccountRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
    let account = ChannelAccountConfig {
        id: r.id,
        label: r.label,
        kind: k,
        enabled: r.enabled,
        agent_id: r.agent_id,
        secret_id: None,
        options: options(r.options)?,
        legacy_secret: None,
    };
    validate(&c, &account)?;
    let entry = entry(&account, r.credentials)?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        authorized(&s, &c)?;
        let mut config = s.channels.config();
        if config
            .accounts
            .iter()
            .any(|a| a.kind == k && a.id == account.id)
        {
            return Err(HttpError(StatusCode::CONFLICT, "channel_account_exists"));
        }
        if config.accounts.len() >= 128 {
            return Err(HttpError(StatusCode::CONFLICT, "channel_account_limit"));
        }
        bound_agent(&s, &account)?;
        save_secret(&s, &c, entry).await?;
        let mut account = account;
        account.secret_id = Some(format!("channel:{}:{}", k.as_str(), account.id));
        config.accounts.push(account.clone());
        // A failed config write leaves an unreachable encrypted secret, never a live adapter.
        persist(&s, config).await?;
        let mut supervisor = s.channels.supervisor.lock().await;
        apply(&s, &mut supervisor, &account, true).await;
        Ok((
            StatusCode::CREATED,
            Json(metadata(&s, &supervisor, &account).await),
        ))
    })
    .await
    .map_err(|_| HttpError::internal())?
}
pub(crate) async fn patch(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Path((k, id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<ChannelAccount>, HttpError> {
    person(&c)?;
    let k = kind(&k)?;
    let mut r: UpdateChannelAccountRequest =
        serde_json::from_slice(&body).map_err(|_| invalid())?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        authorized(&s, &c)?;
        let mut config = s.channels.config();
        let a = config
            .accounts
            .iter_mut()
            .find(|a| a.kind == k && a.id == id)
            .ok_or(HttpError(
                StatusCode::NOT_FOUND,
                "channel_account_not_found",
            ))?;
        let routing_changed = r.agent_id.is_some() || r.enabled == Some(true);
        if let Some(label) = r.label {
            a.label = label;
        }
        if let Some(enabled) = r.enabled {
            a.enabled = enabled;
        }
        if let Some(agent) = r.agent_id {
            a.agent_id = (!agent.is_empty()).then_some(agent);
        }
        if let Some(o) = r.options {
            a.options = options(o)?;
        }
        if r.credentials.is_some() {
            a.secret_id = Some(format!("channel:{}:{}", k.as_str(), id));
            a.legacy_secret = None;
        }
        validate(&c, a)?;
        if routing_changed {
            bound_agent(&s, a)?;
        }
        let account = a.clone();
        let mut supervisor = s.channels.supervisor.lock().await;
        let key = (k, id.clone());
        stop(&mut supervisor, &key).await;
        // Stop the old identity before rotation. Failed writes must not revive old credentials.
        supervisor.status.insert(key, Status::Unavailable);
        if let Some(credentials) = r.credentials.take() {
            save_secret(&s, &c, entry(&account, credentials)?).await?;
        }
        persist(&s, config).await?;
        apply(&s, &mut supervisor, &account, true).await;
        Ok(Json(metadata(&s, &supervisor, &account).await))
    })
    .await
    .map_err(|_| HttpError::internal())?
}
pub(crate) async fn delete(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Path((k, id)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    person(&c)?;
    let k = kind(&k)?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        authorized(&s, &c)?;
        let mut config = s.channels.config();
        let index = config
            .accounts
            .iter()
            .position(|a| a.kind == k && a.id == id)
            .ok_or(HttpError(
                StatusCode::NOT_FOUND,
                "channel_account_not_found",
            ))?;
        let account = config.accounts.remove(index);
        let mut supervisor = s.channels.supervisor.lock().await;
        let key = (k, id.clone());
        stop(&mut supervisor, &key).await;
        supervisor.status.insert(key.clone(), Status::Disabled);
        if account.secret_id.is_some() {
            let mut guard = s.registry.vault.vault.lock().await;
            let vault = guard.as_mut().ok_or_else(unavailable)?;
            match vault.delete_channel_secret(&context(c.id()), k.as_str(), &id) {
                Ok(()) | Err(Error::UnknownItem) => {}
                Err(Error::Locked) => return Err(unavailable()),
                Err(_) => return Err(HttpError::internal()),
            }
        }
        persist(&s, config).await?;
        supervisor.status.remove(&key);
        drop(supervisor);
        let _agents = s.agents.gate.lock().await;
        s.agents.clear_channel_binding(k.as_str(), &id)?;
        s.channels.hooks.lock().await.remove(&key);
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| HttpError::internal())?
}
pub(crate) async fn test(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Path((k, id)): Path<(String, String)>,
) -> Result<Json<ChannelAccountTest>, HttpError> {
    person(&c)?;
    let k = kind(&k)?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        authorized(&s, &c)?;
        let a = s
            .channels
            .config()
            .accounts
            .into_iter()
            .find(|a| a.kind == k && a.id == id)
            .ok_or(HttpError(
                StatusCode::NOT_FOUND,
                "channel_account_not_found",
            ))?;
        let mut supervisor = s.channels.supervisor.lock().await;
        let status = match secrets(&s, &a).await {
            Err(status) => status,
            Ok(credentials) => match openside_channels::test_account(
                &a,
                &credentials,
                &s.config.loaded.config.network.allow,
            )
            .await
            {
                Ok(()) => Status::Connected,
                Err(openside_channels::ChannelError::Refused(reason)) if reason == "expired" => {
                    Status::Expired
                }
                Err(_) => Status::Unavailable,
            },
        };
        let key = (k, id.clone());
        if status != Status::Connected {
            stop(&mut supervisor, &key).await;
            supervisor.status.insert(key, status);
        } else if a.enabled {
            apply(&s, &mut supervisor, &a, false).await;
        }
        let effective = supervisor
            .status
            .get(&(k, id.clone()))
            .copied()
            .unwrap_or(status);
        Ok(Json(ChannelAccountTest {
            account_id: id,
            kind: k.as_str().into(),
            ok: status == Status::Connected,
            status,
            code: match status {
                Status::Connected => "ok",
                Status::Expired => "channel_credentials_expired",
                Status::VaultLocked => "vault_unavailable",
                _ => "channel_unavailable",
            }
            .into(),
            restart_required: effective == Status::RestartRequired,
        }))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

async fn hook_missing() -> Response {
    // No Vault lookup or platform request can make unknown ids a credential timing oracle.
    tokio::time::sleep(Duration::from_millis(5)).await;
    HttpError(StatusCode::NOT_FOUND, "channel_account_not_found").into_response()
}

pub(crate) async fn hook(
    State(s): State<Shared>,
    Path((service, id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Ok(service) = kind(&service) else {
        return hook_missing().await;
    };
    if service == ChannelKind::Telegram {
        return hook_missing().await;
    }
    let key = (service, id);
    {
        let supervisor = s.channels.supervisor.lock().await;
        if !supervisor.running.contains_key(&key) {
            drop(supervisor);
            return hook_missing().await;
        }
        let now = Instant::now();
        let mut hooks = s.channels.hooks.lock().await;
        let limits = hooks.entry(key.clone()).or_default();
        while limits
            .requests
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
        {
            limits.requests.pop_front();
        }
        if limits.requests.len() >= HOOK_RATE {
            let mut response =
                HttpError(StatusCode::TOO_MANY_REQUESTS, "channel_rate_limited").into_response();
            response
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("60"));
            return response;
        }
        limits.requests.push_back(now);
    }
    let (parts, body) = request.into_parts();
    let body = match tokio::time::timeout(Duration::from_secs(10), to_bytes(body, MAX_HOOK_BODY))
        .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => {
            return HttpError(StatusCode::PAYLOAD_TOO_LARGE, "channel_body_too_large")
                .into_response()
        }
        Err(_) => {
            return HttpError(StatusCode::REQUEST_TIMEOUT, "channel_body_timeout").into_response()
        }
    };
    // Hold the same fence used by rotation/removal through verification and dispatch.
    let supervisor = s.channels.supervisor.lock().await;
    let Some(running) = supervisor.running.get(&key) else {
        drop(supervisor);
        return hook_missing().await;
    };
    if !running.verify_webhook(&parts.headers, &body, chrono::Utc::now().timestamp()) {
        return HttpError(StatusCode::UNAUTHORIZED, "channel_bad_signature").into_response();
    }
    let timestamp = parts
        .headers
        .get(if service == ChannelKind::Slack {
            "x-slack-request-timestamp"
        } else {
            "x-signature-timestamp"
        })
        .map_or(&b""[..], |value| value.as_bytes());
    let mut digest = Sha256::new();
    digest.update(timestamp);
    digest.update([0]);
    digest.update(&body);
    let digest: [u8; 32] = digest.finalize().into();
    {
        let now = Instant::now();
        let mut hooks = s.channels.hooks.lock().await;
        let limits = hooks.entry(key).or_default();
        limits
            .seen
            .retain(|_, at| now.duration_since(*at) <= SIGNATURE_WINDOW);
        if limits.seen.contains_key(&digest) {
            return HttpError(StatusCode::CONFLICT, "channel_replay").into_response();
        }
        limits.seen.insert(digest, now);
    }
    running.webhook(parts.headers, body).await
}

pub(crate) async fn refresh(s: &Shared) {
    let accounts = s.channels.config().accounts;
    let mut supervisor = s.channels.supervisor.lock().await;
    for a in &accounts {
        let key = (a.kind, a.id.clone());
        if !supervisor.running.contains_key(&key)
            && matches!(
                supervisor.status.get(&key),
                None | Some(Status::VaultLocked) | Some(Status::RestartRequired)
            )
        {
            apply(s, &mut supervisor, a, true).await;
        }
    }
}
pub(crate) async fn initialize(s: &Shared, listen: Option<SocketAddr>) {
    s.channels.supervisor.lock().await.listen = listen;
}
pub(crate) fn start(s: &Shared, listen: Option<SocketAddr>) {
    let s = s.clone();
    tokio::spawn(async move {
        let mut shutdown = s.shutdown.subscribe();
        initialize(&s, listen).await;
        refresh(&s).await;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _=shutdown.recv()=>break,
                _=tick.tick()=> {
                    let locked={ let v=s.registry.vault.vault.lock().await; v.as_ref().map_or(true, |v| !v.is_unlocked()) };
                    if locked { s.channels.stop_all().await; }
                }
            }
        }
        s.channels.shutdown(&s).await;
    });
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRegistry {
        owner: Option<String>,
        active: HashMap<String, bool>,
        lookups: RefCell<Vec<String>>,
    }
    impl FakeRegistry {
        fn new(owner: Option<&str>) -> Self {
            Self {
                owner: owner.map(str::to_owned),
                active: HashMap::from([
                    ("chief".into(), true),
                    ("mina".into(), true),
                    ("ari".into(), true),
                    ("retired".into(), false),
                ]),
                lookups: RefCell::new(Vec::new()),
            }
        }
        fn resolve(&self, configured: Option<&str>) -> Result<String, HttpError> {
            let account = ChannelAccountConfig {
                id: "work".into(),
                kind: ChannelKind::Telegram,
                label: "Work".into(),
                enabled: true,
                agent_id: configured.map(str::to_owned),
                secret_id: None,
                legacy_secret: None,
                options: ChannelOptions::default(),
            };
            resolve_binding(&account, self.owner.as_deref(), |id| {
                self.lookups.borrow_mut().push(id.into());
                Ok(self.active.get(id).copied())
            })
        }
    }
    fn successful(result: Result<String, HttpError>) -> String {
        match result {
            Ok(id) => id,
            Err(HttpError(status, code)) => panic!("{status}: {code}"),
        }
    }

    #[test]
    fn channel_binding_defaults_to_active_chief_only_without_explicit_owners() {
        let registry = FakeRegistry::new(None);
        assert_eq!(successful(registry.resolve(None)), "chief");
        assert_eq!(*registry.lookups.borrow(), ["chief"]);
    }
    #[test]
    fn channel_binding_uses_single_explicit_owner_and_accepts_agreement() {
        assert_eq!(
            successful(FakeRegistry::new(None).resolve(Some("mina"))),
            "mina"
        );
        assert_eq!(
            successful(FakeRegistry::new(Some("mina")).resolve(None)),
            "mina"
        );
        assert_eq!(
            successful(FakeRegistry::new(Some("mina")).resolve(Some("mina"))),
            "mina"
        );
    }
    #[test]
    fn channel_binding_conflict_never_queries_an_alternate_identity() {
        let registry = FakeRegistry::new(Some("mina"));
        assert!(matches!(
            registry.resolve(Some("ari")),
            Err(HttpError(StatusCode::CONFLICT, "channel_binding_conflict"))
        ));
        assert!(registry.lookups.borrow().is_empty());
    }
    #[test]
    fn channel_binding_retirement_never_falls_through_to_chief() {
        let registry = FakeRegistry::new(Some("retired"));
        assert!(matches!(
            registry.resolve(None),
            Err(HttpError(StatusCode::CONFLICT, "agent_retired"))
        ));
        assert_eq!(*registry.lookups.borrow(), ["retired"]);
        assert!(matches!(
            FakeRegistry::new(None).resolve(Some("retired")),
            Err(HttpError(StatusCode::CONFLICT, "agent_retired"))
        ));
    }
    #[test]
    fn channel_binding_unknown_owner_is_not_a_default_owner() {
        let registry = FakeRegistry::new(Some("absent"));
        assert!(matches!(
            registry.resolve(None),
            Err(HttpError(StatusCode::NOT_FOUND, "agent_not_found"))
        ));
        assert_eq!(*registry.lookups.borrow(), ["absent"]);
    }
}
