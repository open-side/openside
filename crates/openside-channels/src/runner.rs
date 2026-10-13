use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use openside_config::{ChannelAccountConfig, ChannelKind, Loaded};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    allow::AllowList,
    core::{Core, CoreOptions},
    daemon::DaemonClient,
    discord::{self, DiscordMessenger, DiscordReceiver},
    error::{ChannelError, Result},
    messenger::Messenger,
    secret::{self, Secret},
    slack::{self, SlackMessenger, SlackReceiver},
    store::Store,
    stt::Stt,
    telegram::{Poller, TelegramMessenger},
};

/// Injectable environment access keeps legacy secret tests isolated.
pub type EnvFn<'a> = &'a (dyn Fn(&str) -> Option<String> + Sync);
/// Ignore absent and empty environment credentials.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
/// Private state root, with separate subdirectories for managed accounts.
pub fn state_dir(loaded: &Loaded) -> PathBuf {
    loaded
        .config
        .channels
        .state_dir
        .clone()
        .unwrap_or_else(|| loaded.paths.data_dir.join("channels"))
}
/// Standalone legacy client using only explicitly provisioned public-API capabilities.
pub async fn daemon_client(loaded: &Loaded, env: EnvFn<'_>) -> Result<DaemonClient> {
    let c = &loaded.config.channels;
    let url = env("OPENSIDE_API_URL")
        .or_else(|| c.api_url.clone())
        .or_else(|| loaded.config.daemon.listen.map(|a| format!("http://{a}")))
        .ok_or_else(|| {
            ChannelError::Config(
                "no daemon URL: set OPENSIDE_API_URL, channels.api_url or daemon.listen".into(),
            )
        })?;
    let token = if let Some(path) = &c.token_file {
        let raw = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| ChannelError::Config("channels.token_file cannot be read".into()))?;
        Secret::new(raw.trim())
    } else {
        Secret::new(env(&c.token_env).ok_or_else(|| {
            ChannelError::Config(
                "no daemon token: set channels.token_file or channels.token_env".into(),
            )
        })?)
    };
    DaemonClient::new(&url, token)
}

/// Trusted adapter input; Debug is redacted and values are never configuration.
#[derive(Clone)]
pub struct AccountSecrets {
    /// Outbound platform credential, released only to that account's messenger.
    pub token: Secret,
    /// Slack inbound signature credential, never sent to the daemon.
    pub signing_secret: Option<Secret>,
}
impl std::fmt::Debug for AccountSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
/// Resolve existing file-only references without persisting their values.
pub async fn legacy_secrets(
    account: &ChannelAccountConfig,
    env: EnvFn<'_>,
) -> Result<AccountSecrets> {
    let s = account
        .legacy_secret
        .as_ref()
        .ok_or_else(|| ChannelError::Config("Vault-managed accounts run in opensided".into()))?;
    let token = secret::resolve(
        "channel token",
        s.token_env.as_deref(),
        s.token_cmd.as_deref(),
        env,
    )
    .await?;
    let signing_secret = if account.kind == ChannelKind::Slack {
        Some(
            secret::resolve(
                "slack signing secret",
                s.signing_secret_env.as_deref(),
                s.signing_secret_cmd.as_deref(),
                env,
            )
            .await?,
        )
    } else {
        None
    };
    Ok(AccountSecrets {
        token,
        signing_secret,
    })
}

enum Receiver {
    Slack(Arc<SlackReceiver>),
    Discord(Arc<DiscordReceiver>),
}

/// Owned account tasks; dropping or stopping them does not affect other accounts.
pub struct Running {
    /// Bound Slack webhook address, when present.
    pub slack_addr: Option<SocketAddr>,
    /// Bound Discord webhook address, when present.
    pub discord_addr: Option<SocketAddr>,
    tasks: Vec<JoinHandle<()>>,
    stops: Vec<Box<dyn Fn() + Send + Sync>>,
    shutdown: CancellationToken,
    receiver: Option<Receiver>,
    agent_id: Option<String>,
}
impl Running {
    fn new(shutdown: CancellationToken) -> Self {
        Self {
            slack_addr: None,
            discord_addr: None,
            tasks: Vec::new(),
            stops: Vec::new(),
            shutdown,
            receiver: None,
            agent_id: None,
        }
    }
    /// Actual owner of new sessions for a single supervised identity; aggregates have none.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }
    /// Cancel inbound polling and retained session event watchers.
    pub fn stop(&self) {
        self.shutdown.cancel();
        for stop in &self.stops {
            stop();
        }
    }
    /// Await cancelled listeners and pollers before account revocation completes.
    pub async fn join(mut self) {
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
        for stop in &self.stops {
            stop();
        }
    }
    /// Verify only the selected identity; no value is released to the caller.
    pub fn verify_webhook(&self, headers: &axum::http::HeaderMap, body: &[u8], now: i64) -> bool {
        if self.shutdown.is_cancelled() {
            return false;
        }
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
        };
        match &self.receiver {
            Some(Receiver::Slack(r)) => slack::verify_signature(
                r.signing_secret.expose(),
                header("x-slack-request-timestamp"),
                header("x-slack-signature"),
                body,
                now,
            ),
            Some(Receiver::Discord(r)) => {
                let timestamp = header("x-signature-timestamp");
                timestamp
                    .parse::<i64>()
                    .is_ok_and(|t| now.abs_diff(t) <= 300)
                    && discord::verify_signature(
                        &r.public_key,
                        timestamp,
                        header("x-signature-ed25519"),
                        body,
                    )
            }
            None => false,
        }
    }

    /// Dispatch verified daemon ingress through the existing signed platform handlers.
    pub async fn webhook(
        &self,
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        if self.shutdown.is_cancelled() {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        match &self.receiver {
            Some(Receiver::Slack(r)) => {
                if headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"))
                {
                    slack::interactivity(axum::extract::State(r.clone()), headers, body).await
                } else {
                    slack::events(axum::extract::State(r.clone()), headers, body).await
                }
            }
            Some(Receiver::Discord(r)) => {
                discord::interactions(axum::extract::State(r.clone()), headers, body).await
            }
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
        for task in &self.tasks {
            task.abort();
        }
    }
}
fn track<M: Messenger>(running: &mut Running, core: &Arc<Core<M>>) {
    let core = core.clone();
    running.stops.push(Box::new(move || core.stop_all()));
}
fn options(loaded: &Loaded) -> Result<CoreOptions> {
    let c = &loaded.config.channels;
    let stt = c
        .stt_cmd
        .as_ref()
        .map(|argv| {
            Stt::new(
                argv.clone(),
                Duration::from_secs(u64::from(c.stt_timeout_secs)),
                c.max_audio_bytes,
            )
        })
        .transpose()?;
    Ok(CoreOptions {
        edit_interval: Duration::from_millis(c.edit_interval_ms),
        stt,
        web_url: loaded.config.remote.public_url.clone(),
    })
}

/// Separate files isolate offsets, sessions and idempotency across platform identities.
pub async fn start_account(
    loaded: &Loaded,
    account: &ChannelAccountConfig,
    secrets: AccountSecrets,
    daemon: DaemonClient,
    shutdown: CancellationToken,
) -> Result<Running> {
    account
        .validate()
        .map_err(|reason| ChannelError::Config(reason.into()))?;
    let root = state_dir(loaded);
    let directory = if account.id == format!("legacy-{}", account.kind.as_str()) {
        root
    } else {
        root.join(account.kind.as_str()).join(&account.id)
    };
    let store = Arc::new(Store::open(&directory)?);
    let daemon = daemon.for_agent(account.agent_id.clone());
    let o = &account.options;
    let allow_network = &loaded.config.network.allow;
    let mut running = Running::new(shutdown.clone());
    running.agent_id = Some(account.agent_id.clone().unwrap_or_else(|| "chief".into()));
    match account.kind {
        ChannelKind::Telegram => {
            let allow = AllowList::new("telegram", &o.allowed_chats, &o.allowed_users, &[])?;
            let messenger = Arc::new(TelegramMessenger::new(
                account.api_base(),
                secrets.token,
                allow_network,
            )?);
            let core = Core::new(messenger, daemon, store, options(loaded)?, shutdown.clone());
            core.resume_all().await;
            track(&mut running, &core);
            let poller = Poller {
                core,
                allow,
                poll_timeout: Duration::from_secs(u64::from(o.poll_timeout_secs)),
            };
            running
                .tasks
                .push(tokio::spawn(async move { poller.run(shutdown).await }));
        }
        ChannelKind::Slack => {
            let allow = AllowList::new(
                "slack",
                &o.allowed_channels,
                &o.allowed_users,
                &o.allowed_workspaces,
            )?;
            let signing = secrets
                .signing_secret
                .ok_or_else(|| ChannelError::Config("slack signing secret is required".into()))?;
            let messenger = Arc::new(SlackMessenger::new(
                account.api_base(),
                secrets.token,
                allow_network,
            )?);
            let core = Core::new(messenger, daemon, store, options(loaded)?, shutdown.clone());
            core.resume_all().await;
            track(&mut running, &core);
            let receiver = SlackReceiver::new(core, allow, signing);
            let router = slack::router(receiver.clone());
            running.receiver = Some(Receiver::Slack(receiver));
            let (addr, task) = serve(
                o.listen.unwrap_or(SocketAddr::from(([127, 0, 0, 1], 0))),
                router,
                shutdown,
            )
            .await?;
            running.slack_addr = Some(addr);
            running.tasks.push(task);
        }
        ChannelKind::Discord => {
            let key =
                discord::parse_public_key(o.public_key.as_deref().ok_or_else(|| {
                    ChannelError::Config("discord public_key is required".into())
                })?)?;
            let allow = AllowList::new(
                "discord",
                &o.allowed_channels,
                &o.allowed_users,
                &o.allowed_guilds,
            )?;
            let messenger = Arc::new(DiscordMessenger::new(
                account.api_base(),
                secrets.token,
                allow_network,
            )?);
            let core = Core::new(messenger, daemon, store, options(loaded)?, shutdown.clone());
            core.resume_all().await;
            track(&mut running, &core);
            let receiver = DiscordReceiver::new(core, allow, key);
            let router = discord::router(receiver.clone());
            running.receiver = Some(Receiver::Discord(receiver));
            let (addr, task) = serve(
                o.listen.unwrap_or(SocketAddr::from(([127, 0, 0, 1], 0))),
                router,
                shutdown,
            )
            .await?;
            running.discord_addr = Some(addr);
            running.tasks.push(task);
        }
    }
    Ok(running)
}

/// Validate legacy references and daemon read access without starting adapters.
pub async fn check(loaded: &Loaded, env: EnvFn<'_>) -> Result<()> {
    let daemon = daemon_client(loaded, env).await?;
    let mut enabled = false;
    for a in loaded.config.channels.accounts.iter().filter(|a| a.enabled) {
        enabled = true;
        a.validate()
            .map_err(|reason| ChannelError::Config(reason.into()))?;
        let _ = legacy_secrets(a, env).await?;
    }
    if !enabled {
        return Err(ChannelError::Config(
            "no adapter is enabled: enable a channels.accounts entry".into(),
        ));
    }
    daemon.sessions().await?;
    Ok(())
}
/// Start each enabled legacy identity with an independent child cancellation token.
pub async fn start(
    loaded: &Loaded,
    env: EnvFn<'_>,
    shutdown: CancellationToken,
) -> Result<Running> {
    let daemon = daemon_client(loaded, env).await?;
    let mut running = Running::new(shutdown.clone());
    for a in loaded.config.channels.accounts.iter().filter(|a| a.enabled) {
        let secrets = legacy_secrets(a, env).await?;
        let mut account =
            start_account(loaded, a, secrets, daemon.clone(), shutdown.child_token()).await?;
        running.slack_addr = running.slack_addr.or(account.slack_addr);
        running.discord_addr = running.discord_addr.or(account.discord_addr);
        running.tasks.append(&mut account.tasks);
        running.stops.append(&mut account.stops);
        // The aggregate owns the child now; dropping it must not cancel the transferred tasks.
        account.shutdown = CancellationToken::new();
    }
    if running.tasks.is_empty() {
        return Err(ChannelError::Config(
            "no adapter is enabled: enable a channels.accounts entry".into(),
        ));
    }
    Ok(running)
}

/// Read-only identity checks; never sends a chat message or returns upstream text.
pub async fn test_account(
    account: &ChannelAccountConfig,
    secrets: &AccountSecrets,
    network_allow: &[String],
) -> Result<()> {
    let http = crate::http::client(account.api_base(), &[], network_allow)?;
    let base = account.api_base().trim_end_matches('/');
    let request = match account.kind {
        ChannelKind::Telegram => {
            http.post(format!("{base}/bot{}/getMe", secrets.token.expose()))?
        }
        ChannelKind::Slack => http
            .post(format!("{base}/auth.test"))?
            .bearer_auth(secrets.token.expose()),
        ChannelKind::Discord => http
            .get(format!("{base}/users/@me"))?
            .header("authorization", format!("Bot {}", secrets.token.expose())),
    };
    let response = request
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(ChannelError::http)?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ChannelError::Refused("expired".into()));
    }
    if !status.is_success() {
        return Err(ChannelError::Refused("unavailable".into()));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| ChannelError::Refused("unavailable".into()))?;
    if account.kind != ChannelKind::Discord && value["ok"] != true {
        return Err(ChannelError::Refused("expired".into()));
    }
    Ok(())
}
async fn serve(
    listen: SocketAddr,
    router: axum::Router,
    shutdown: CancellationToken,
) -> Result<(SocketAddr, JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|_| ChannelError::Config("channel listener unavailable".into()))?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await;
    });
    Ok((address, task))
}
/// Pairing requires the privileged local socket, unavailable on non-Unix platforms.
#[cfg(not(unix))]
pub async fn pair(_socket: &Path, _name: &str, _out: &Path) -> Result<String> {
    Err(ChannelError::Config(
        "Unix daemon transport is unsupported on this platform".into(),
    ))
}
/// Mint only read/steer/approve capabilities and persist the bearer token privately.
#[cfg(unix)]
pub async fn pair(socket: &Path, name: &str, out: &Path) -> Result<String> {
    use openside_proto::{CapabilityScope, PairDeviceRequest, PairDeviceResponse};
    crate::daemon::check_public("POST", "/v1/devices/pair")?;
    let client = reqwest::Client::builder()
        .unix_socket(socket)
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(ChannelError::http)?;
    let request = PairDeviceRequest {
        name: name.to_owned(),
        kind: openside_proto::DeviceKind::PersonDevice,
        scopes: vec![
            CapabilityScope::Read,
            CapabilityScope::Steer,
            CapabilityScope::Approve,
        ],
    };
    let response = client
        .post("http://localhost/v1/devices/pair")
        .json(&request)
        .send()
        .await
        .map_err(ChannelError::http)?;
    if !response.status().is_success() {
        return Err(ChannelError::Daemon {
            status: response.status().as_u16(),
            code: String::new(),
        });
    }
    let paired: PairDeviceResponse = response.json().await.map_err(ChannelError::http)?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    crate::store::write_secret_file(out, format!("{}\n", paired.token).as_bytes())?;
    Ok(paired.device_id)
}
