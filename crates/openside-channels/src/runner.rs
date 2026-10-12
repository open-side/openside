use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use openside_config::Loaded;
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

/// Environment lookup, injectable so tests never touch the process environment.
pub type EnvFn<'a> = &'a (dyn Fn(&str) -> Option<String> + Sync);

/// The process environment.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Directory of the thread→session map.
pub fn state_dir(loaded: &Loaded) -> PathBuf {
    loaded
        .config
        .channels
        .state_dir
        .clone()
        .unwrap_or_else(|| loaded.paths.data_dir.join("channels"))
}

/// Public-API client from `OPENSIDE_API_URL` / `channels.api_url` / `daemon.listen`
/// and the bearer token from `channels.token_file` or the `channels.token_env` variable.
pub async fn daemon_client(loaded: &Loaded, env: EnvFn<'_>) -> Result<DaemonClient> {
    let channels = &loaded.config.channels;
    let url = env("OPENSIDE_API_URL")
        .or_else(|| channels.api_url.clone())
        .or_else(|| {
            loaded
                .config
                .daemon
                .listen
                .map(|address| format!("http://{address}"))
        })
        .ok_or_else(|| {
            ChannelError::Config(
                "no daemon URL: set OPENSIDE_API_URL, channels.api_url or daemon.listen".into(),
            )
        })?;
    let token = if let Some(path) = &channels.token_file {
        let raw = tokio::fs::read_to_string(path)
            .await
            .map_err(|_| ChannelError::Config("channels.token_file cannot be read".into()))?;
        Secret::new(raw.trim())
    } else {
        let name = channels.token_env.as_str();
        Secret::new(env(name).ok_or_else(|| {
            ChannelError::Config(format!(
                "no daemon token: set channels.token_file or the {name} environment variable"
            ))
        })?)
    };
    DaemonClient::new(&url, token)
}

/// Adapters started by [`start`].
pub struct Running {
    /// Bound Slack receiver address, when Slack is enabled.
    pub slack_addr: Option<SocketAddr>,
    /// Bound Discord receiver address, when Discord is enabled.
    pub discord_addr: Option<SocketAddr>,
    tasks: Vec<JoinHandle<()>>,
    stops: Vec<Box<dyn Fn() + Send + Sync>>,
}

impl Running {
    /// Wait for every adapter task to end (after the shutdown token fired) and
    /// stop their event watchers.
    pub async fn join(self) {
        for task in self.tasks {
            let _ = task.await;
        }
        for stop in &self.stops {
            stop();
        }
    }
}

fn track<M: Messenger>(running: &mut Running, core: &Arc<Core<M>>) {
    let core = core.clone();
    running.stops.push(Box::new(move || core.stop_all()));
}

/// Everything `run` and `check` both need, resolved without serving anything.
struct Prepared {
    daemon: DaemonClient,
    store: Arc<Store>,
    stt: Option<Stt>,
    edit_interval: Duration,
    telegram: Option<(Arc<TelegramMessenger>, AllowList, Duration)>,
    slack: Option<(Arc<SlackMessenger>, AllowList, Secret, SocketAddr)>,
    discord: Option<(
        Arc<DiscordMessenger>,
        AllowList,
        ed25519_dalek::VerifyingKey,
        SocketAddr,
    )>,
}

/// Validate configuration, resolve secrets and open the state directory. Binds nothing
/// and contacts no chat platform, so a missing token fails before anything listens.
async fn prepare(loaded: &Loaded, env: EnvFn<'_>) -> Result<Prepared> {
    let channels = &loaded.config.channels;
    if !(channels.telegram.enabled || channels.slack.enabled || channels.discord.enabled) {
        return Err(ChannelError::Config(
            "no adapter is enabled: set channels.<platform>.enabled = true".into(),
        ));
    }
    let daemon = daemon_client(loaded, env).await?;
    let store = Arc::new(Store::open(&state_dir(loaded))?);
    let stt = match &channels.stt_cmd {
        Some(argv) => Some(Stt::new(
            argv.clone(),
            Duration::from_secs(u64::from(channels.stt_timeout_secs)),
            channels.max_audio_bytes,
        )?),
        None => None,
    };
    let mut prepared = Prepared {
        daemon,
        store,
        stt,
        edit_interval: Duration::from_millis(channels.edit_interval_ms),
        telegram: None,
        slack: None,
        discord: None,
    };

    if channels.telegram.enabled {
        let config = &channels.telegram;
        let token = secret::resolve(
            "telegram bot token",
            config.token_env.as_deref(),
            config.token_cmd.as_deref(),
            env,
        )
        .await?;
        let allow = AllowList::new(
            "telegram",
            &config.allowed_chats,
            &config.allowed_users,
            &[],
        )?;
        let messenger = Arc::new(TelegramMessenger::new(
            &config.api_base,
            token,
            &loaded.config.network.allow,
        )?);
        let poll_timeout = Duration::from_secs(u64::from(config.poll_timeout_secs));
        prepared.telegram = Some((messenger, allow, poll_timeout));
    }

    if channels.slack.enabled {
        let config = &channels.slack;
        let bot = secret::resolve(
            "slack bot token",
            config.bot_token_env.as_deref(),
            config.bot_token_cmd.as_deref(),
            env,
        )
        .await?;
        let signing = secret::resolve(
            "slack signing secret",
            config.signing_secret_env.as_deref(),
            config.signing_secret_cmd.as_deref(),
            env,
        )
        .await?;
        let allow = AllowList::new(
            "slack",
            &config.allowed_channels,
            &config.allowed_users,
            &config.allowed_workspaces,
        )?;
        let messenger = Arc::new(SlackMessenger::new(
            &config.api_base,
            bot,
            &loaded.config.network.allow,
        )?);
        prepared.slack = Some((messenger, allow, signing, config.listen));
    }

    if channels.discord.enabled {
        let config = &channels.discord;
        let key = config.public_key.as_deref().ok_or_else(|| {
            ChannelError::Config("channels.discord.public_key is required".into())
        })?;
        let public_key = discord::parse_public_key(key)?;
        let bot = secret::resolve(
            "discord bot token",
            config.bot_token_env.as_deref(),
            config.bot_token_cmd.as_deref(),
            env,
        )
        .await?;
        let allow = AllowList::new(
            "discord",
            &config.allowed_channels,
            &config.allowed_users,
            &config.allowed_guilds,
        )?;
        let messenger = Arc::new(DiscordMessenger::new(
            &config.api_base,
            bot,
            &loaded.config.network.allow,
        )?);
        prepared.discord = Some((messenger, allow, public_key, config.listen));
    }
    Ok(prepared)
}

/// Verify configuration, secrets, the state directory and the daemon connection (the
/// token needs `read`) without polling, watching sessions or binding any listener, so it
/// is safe to run next to a live `openside-channels run`.
pub async fn check(loaded: &Loaded, env: EnvFn<'_>) -> Result<()> {
    let prepared = prepare(loaded, env).await?;
    prepared.daemon.sessions().await.map_err(|error| {
        ChannelError::Config(format!(
            "daemon check failed (token needs read scope): {error}"
        ))
    })?;
    Ok(())
}

/// Start every enabled adapter.
pub async fn start(
    loaded: &Loaded,
    env: EnvFn<'_>,
    shutdown: CancellationToken,
) -> Result<Running> {
    let prepared = prepare(loaded, env).await?;
    let Prepared {
        daemon,
        store,
        stt,
        edit_interval,
        telegram,
        slack,
        discord,
    } = prepared;
    // Voice needs a size cap even when no STT command is set; without one it is refused.
    let options = || CoreOptions {
        edit_interval,
        stt: stt.clone(),
        web_url: loaded.config.remote.public_url.clone(),
    };
    let mut running = Running {
        slack_addr: None,
        discord_addr: None,
        tasks: Vec::new(),
        stops: Vec::new(),
    };

    if let Some((messenger, allow, poll_timeout)) = telegram {
        let core = Core::new(
            messenger,
            daemon.clone(),
            store.clone(),
            options(),
            shutdown.clone(),
        );
        core.resume_all().await;
        track(&mut running, &core);
        let poller = Poller {
            core,
            allow,
            poll_timeout,
        };
        let token = shutdown.clone();
        running
            .tasks
            .push(tokio::spawn(async move { poller.run(token).await }));
    }

    if let Some((messenger, allow, signing, listen)) = slack {
        let core = Core::new(
            messenger,
            daemon.clone(),
            store.clone(),
            options(),
            shutdown.clone(),
        );
        core.resume_all().await;
        track(&mut running, &core);
        let router = slack::router(SlackReceiver::new(core, allow, signing));
        let (address, task) = serve(listen, router, shutdown.clone()).await?;
        running.slack_addr = Some(address);
        running.tasks.push(task);
    }

    if let Some((messenger, allow, public_key, listen)) = discord {
        let core = Core::new(
            messenger,
            daemon.clone(),
            store.clone(),
            options(),
            shutdown.clone(),
        );
        core.resume_all().await;
        track(&mut running, &core);
        let router = discord::router(DiscordReceiver::new(core, allow, public_key));
        let (address, task) = serve(listen, router, shutdown.clone()).await?;
        running.discord_addr = Some(address);
        running.tasks.push(task);
    }
    Ok(running)
}

async fn serve(
    listen: SocketAddr,
    router: axum::Router,
    shutdown: CancellationToken,
) -> Result<(SocketAddr, JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| ChannelError::Config(format!("cannot listen on {listen}: {error}")))?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await;
    });
    Ok((address, task))
}

#[cfg(not(unix))]
/// Reject pairing without the local, privileged daemon transport.
pub async fn pair(_socket: &Path, _name: &str, _out: &Path) -> Result<String> {
    Err(ChannelError::Config(
        "Unix daemon transport is unsupported on this platform".into(),
    ))
}

#[cfg(unix)]
/// Mint a device token with the scopes the bot needs through the daemon's Unix
/// socket (`POST /v1/devices/pair`) and store it in a 0600 file. The token is never
/// printed. Returns the device id (for `DELETE /v1/devices/{id}` revocation).
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
    let status = response.status();
    if !status.is_success() {
        return Err(ChannelError::Daemon {
            status: status.as_u16(),
            code: String::new(),
        });
    }
    let paired: PairDeviceResponse = response.json().await.map_err(ChannelError::http)?;
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    crate::store::write_secret_file(out, format!("{}\n", paired.token).as_bytes())?;
    Ok(paired.device_id)
}
