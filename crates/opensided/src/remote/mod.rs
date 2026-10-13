//! Live remote transport setup and local-person authority for credential issuance.
//! Proxies terminate on loopback, so address checks alone never grant local authority.

pub(crate) mod cloudflare;
pub(crate) mod relay;
mod tailscale;

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use hmac::{Hmac, Mac};
use openside_config::{CloudflareTunnel, RemoteConfig, RemoteMode};
use openside_proto::{
    CapabilityScope, ConfigureTailscaleRequest, DeviceKind, LocalAccess, PairingTransport,
    RemoteConfiguration, RemoteStatus, SetCloudflareRequest, SetRelayRequest,
    TailscaleConfiguration, TailscaleStatus,
};
use parking_lot::Mutex;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{api::HttpError, engine::Shared};

pub(crate) const LOCAL_SECRET_HEADER: &str = "x-openside-local-secret";

#[derive(Clone, Copy)]
pub(crate) struct LocalAuthority(pub(crate) bool);

impl LocalAuthority {
    pub(crate) fn require(self) -> Result<(), HttpError> {
        if self.0 {
            Ok(())
        } else {
            Err(HttpError(StatusCode::FORBIDDEN, "local_access_required"))
        }
    }
}

/// Set by the relay listener, never inferred from client-provided headers.
#[derive(Clone, Copy)]
pub(crate) struct RelayOrigin;

pub(crate) struct Transports {
    cloudflare: Mutex<Option<cloudflare::Supervisor>>,
    relay: Mutex<Option<relay::Link>>,
    relay_error: Mutex<Option<String>>,
    effective: Mutex<Option<RemoteConfig>>,
    listen: Mutex<Option<SocketAddr>>,
    boot_secret: Zeroizing<String>,
    boot_tag: [u8; 32],
    local_device: tokio::sync::Mutex<()>,
}

impl Default for Transports {
    fn default() -> Self {
        let secret = Zeroizing::new(format!(
            "{}{}",
            crate::persistence::random_id(),
            crate::persistence::random_id()
        ));
        Self::with_secret(secret)
    }
}

impl Transports {
    pub(crate) fn with_secret(secret: Zeroizing<String>) -> Self {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap_or_else(|_| unreachable!());
        mac.update(b"openside-local-person");
        Self {
            cloudflare: Mutex::default(),
            relay: Mutex::default(),
            relay_error: Mutex::default(),
            effective: Mutex::default(),
            listen: Mutex::default(),
            boot_secret: secret,
            boot_tag: mac.finalize().into_bytes().into(),
            local_device: tokio::sync::Mutex::default(),
        }
    }

    pub(crate) fn boot_secret(&self) -> &str {
        &self.boot_secret
    }

    pub(crate) async fn stop(&self) {
        let supervisor = self.cloudflare.lock().take();
        if let Some(supervisor) = supervisor {
            supervisor.stop().await;
        }
        drop(self.relay.lock().take());
    }

    fn valid_secret(&self, headers: &HeaderMap) -> bool {
        let Some(secret) = headers
            .get(LOCAL_SECRET_HEADER)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        if secret.len() != 64 {
            return false;
        }
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(b"openside-local-person");
        mac.verify_slice(&self.boot_tag).is_ok()
    }
}

pub(crate) fn local_tcp(shared: &Shared, request: &axum::extract::Request) -> bool {
    if request.extensions().get::<RelayOrigin>().is_some() {
        return false;
    }
    if !request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|peer| peer.0.ip().is_loopback())
    {
        return false;
    }
    // These headers can only reduce authority. They are not trusted origin assertions.
    if request.headers().keys().any(|name| {
        let name = name.as_str();
        name == "forwarded"
            || name == "via"
            || name.starts_with("x-forwarded-")
            || name.starts_with("cf-")
            || name.starts_with("tailscale-")
    }) {
        return false;
    }
    let Some(host) = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if !host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        return false;
    }
    shared.transports.valid_secret(request.headers())
}

pub(crate) fn transport(mode: RemoteMode) -> PairingTransport {
    match mode {
        RemoteMode::Tailscale => PairingTransport::Tailscale,
        RemoteMode::Cloudflare => PairingTransport::Cloudflare,
        RemoteMode::Relay => PairingTransport::Relay,
    }
}

pub(crate) fn effective(shared: &Shared) -> RemoteConfig {
    shared
        .transports
        .effective
        .lock()
        .clone()
        .unwrap_or_else(|| shared.config.loaded.config.remote.clone())
}

fn spawn_cloudflare(shared: &Shared, remote: &RemoteConfig, listen: SocketAddr) {
    let token = (remote.cloudflare.tunnel == CloudflareTunnel::Named)
        .then(|| {
            openside_config::read_secret(
                remote.cloudflare.token_file.as_deref(),
                "OPENSIDE_CLOUDFLARE_TOKEN",
            )
        })
        .flatten();
    *shared.transports.cloudflare.lock() = Some(cloudflare::spawn(
        cloudflare::Options::from_config(&remote.cloudflare, listen, token),
        shared.shutdown.subscribe(),
    ));
}

fn spawn_relay(shared: &Shared, remote: &RemoteConfig) {
    *shared.transports.relay_error.lock() = None;
    match relay::start_config(shared, remote) {
        Ok(link) => *shared.transports.relay.lock() = Some(link),
        Err(error) => {
            crate::diag!("opensided: relay not started: {error}");
            *shared.transports.relay_error.lock() = Some(error);
        }
    }
}

pub(crate) fn start(shared: &Shared, listen: Option<SocketAddr>) {
    let remote = &shared.config.loaded.config.remote;
    *shared.transports.listen.lock() = listen;
    *shared.transports.effective.lock() = Some(remote.clone());
    if remote.has(RemoteMode::Cloudflare) {
        if let Some(listen) = listen {
            spawn_cloudflare(shared, remote, listen);
        }
    }
    if remote.has(RemoteMode::Relay) {
        spawn_relay(shared, remote);
    }
}

fn direct_base(shared: &Shared, via: PairingTransport) -> Option<String> {
    match via {
        PairingTransport::Tailscale => effective(shared)
            .public_url
            .map(|url| url.trim_end_matches('/').to_owned()),
        PairingTransport::Cloudflare => shared
            .transports
            .cloudflare
            .lock()
            .as_ref()
            .and_then(|supervisor| supervisor.status().url),
        PairingTransport::Relay => None,
    }
}

pub(crate) fn pairing_link(
    shared: &Shared,
    via: Option<PairingTransport>,
    code: &str,
) -> Result<(Option<String>, Option<PairingTransport>), HttpError> {
    let remote = effective(shared);
    let enabled: Vec<PairingTransport> = remote.mode.iter().copied().map(transport).collect();
    let via = match via.or_else(|| enabled.first().copied()) {
        None => {
            return Ok((
                remote
                    .public_url
                    .as_deref()
                    .map(|base| format!("{}/pair#{code}", base.trim_end_matches('/'))),
                None,
            ))
        }
        Some(via) if !enabled.contains(&via) => {
            return Err(HttpError(StatusCode::CONFLICT, "transport_not_enabled"))
        }
        Some(via) => via,
    };
    let unavailable = HttpError(StatusCode::SERVICE_UNAVAILABLE, "transport_unavailable");
    let url = match via {
        PairingTransport::Relay => {
            let guard = shared.transports.relay.lock();
            let link = guard.as_ref().ok_or(unavailable)?;
            format!(
                "{}/pair#c={code}&t=relay&d={}&k={}",
                link.relay_url(),
                link.daemon_id(),
                link.daemon_key()
            )
        }
        direct => format!(
            "{}/pair#{code}",
            direct_base(shared, direct).ok_or(unavailable)?
        ),
    };
    Ok((Some(url), Some(via)))
}

fn configuration(remote: &RemoteConfig) -> RemoteConfiguration {
    RemoteConfiguration {
        tailscale: TailscaleConfiguration {
            enabled: remote.has(RemoteMode::Tailscale),
            public_url: remote.public_url.clone(),
        },
        cloudflare: SetCloudflareRequest {
            enabled: remote.has(RemoteMode::Cloudflare),
            tunnel: match remote.cloudflare.tunnel {
                CloudflareTunnel::Quick => "quick",
                CloudflareTunnel::Named => "named",
            }
            .into(),
            hostname: remote.cloudflare.hostname.clone(),
            token_file: remote
                .cloudflare
                .token_file
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        },
        relay: SetRelayRequest {
            enabled: remote.has(RemoteMode::Relay),
            url: remote.relay.url.clone(),
            token_file: remote
                .relay
                .token_file
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        },
    }
}

pub(crate) fn snapshot(shared: &Shared) -> RemoteStatus {
    let remote = effective(shared);
    let relay =
        shared
            .transports
            .relay
            .lock()
            .as_ref()
            .map(relay::Link::status)
            .or_else(|| {
                shared.transports.relay_error.lock().clone().map(|error| {
                    openside_proto::RelayStatus {
                        state: "failed".into(),
                        url: remote.relay.url.clone().unwrap_or_default(),
                        daemon_id: String::new(),
                        daemon_key: String::new(),
                        clients: 0,
                        last_error: Some(error),
                        e2ee: true,
                    }
                })
            });
    RemoteStatus {
        modes: remote.mode.iter().copied().map(transport).collect(),
        public_url: remote.public_url.clone(),
        cloudflare: shared
            .transports
            .cloudflare
            .lock()
            .as_ref()
            .map(cloudflare::Supervisor::status),
        relay,
        tailscale: TailscaleStatus {
            installed: openside_config::find_on_path("tailscale").is_some(),
            state: "stopped".into(),
            url: None,
            last_error: None,
        },
        configuration: configuration(&remote),
        pairing_allowed: false,
        configure_allowed: false,
        restart_required: false,
    }
}

async fn document(shared: &Shared, local: bool) -> Result<RemoteStatus, HttpError> {
    let configured = crate::settings_api::stored(shared).await?.remote;
    let mut status = snapshot(shared);
    status.restart_required = configured != effective(shared);
    status.configuration = configuration(&configured);
    status.pairing_allowed = local;
    status.configure_allowed = local;
    let listen = *shared.transports.listen.lock();
    status.tailscale = tailscale::status(listen).await;
    Ok(status)
}

pub(crate) async fn status(
    State(shared): State<Shared>,
    Extension(local): Extension<LocalAuthority>,
) -> Result<Json<RemoteStatus>, HttpError> {
    Ok(Json(document(&shared, local.0).await?))
}

pub(crate) async fn local_access(
    State(shared): State<Shared>,
) -> Result<Json<LocalAccess>, HttpError> {
    tokio::spawn(async move {
        let _issue = shared.transports.local_device.lock().await;
        let previous = shared.db.bootstrap_device().await.map_err(|_| HttpError::internal())?;
        let _previous_gate = if let Some(device) = previous.as_deref() {
            shared.remote.mark_revoked(device);
            Some(shared.remote.device_gate(device).write_owned().await)
        } else { None };
        let scopes = vec![CapabilityScope::Read, CapabilityScope::Steer, CapabilityScope::Approve, CapabilityScope::HumanStep, CapabilityScope::Admin];
        let (device_id, token) = shared.db.pair_bootstrap(scopes.clone()).await.map_err(|_| HttpError::internal())?;
        if let Some(previous) = previous.as_deref() {
            crate::push::forget_device(&shared, previous).await?;
            crate::pairing::audit(&shared, "device", "revoked", serde_json::json!({"device_id":previous,"via":"bootstrap_rotation"})).await;
        }
        crate::pairing::audit(&shared, "device", "paired", serde_json::json!({"device_id":device_id,"name":"openside-browser","kind":"person_device","via":"local_bootstrap","replaces":previous})).await;
        let secret = shared.transports.boot_secret.to_string();
        let url = shared.transports.listen.lock().map(|listen| format!("http://{listen}/#local_secret={secret}&local_token={token}&local_device={device_id}"));
        Ok(Json(LocalAccess { secret, url, device_id, token, scopes, kind: DeviceKind::PersonDevice }))
    }).await.map_err(|_| HttpError::internal())?
}

enum Change {
    Tailscale(ConfigureTailscaleRequest),
    Cloudflare(SetCloudflareRequest),
    Relay(SetRelayRequest),
}

pub(crate) async fn configure_tailscale(
    State(shared): State<Shared>,
    Json(request): Json<ConfigureTailscaleRequest>,
) -> Result<Json<RemoteStatus>, HttpError> {
    update(shared, Change::Tailscale(request)).await
}
pub(crate) async fn configure_cloudflare(
    State(shared): State<Shared>,
    Json(request): Json<SetCloudflareRequest>,
) -> Result<Json<RemoteStatus>, HttpError> {
    update(shared, Change::Cloudflare(request)).await
}
pub(crate) async fn configure_relay(
    State(shared): State<Shared>,
    Json(request): Json<SetRelayRequest>,
) -> Result<Json<RemoteStatus>, HttpError> {
    update(shared, Change::Relay(request)).await
}

async fn update(shared: Shared, change: Change) -> Result<Json<RemoteStatus>, HttpError> {
    tokio::spawn(update_owned(shared, change))
        .await
        .map_err(|_| HttpError::internal())?
}

async fn update_owned(shared: Shared, change: Change) -> Result<Json<RemoteStatus>, HttpError> {
    let _write = crate::settings_api::WRITE.lock().await;
    let mut config = crate::settings_api::stored(&shared).await?;
    let (mode, enabled) = match change {
        Change::Tailscale(request) => (RemoteMode::Tailscale, request.enabled),
        Change::Cloudflare(request) => {
            config.remote.cloudflare.tunnel = match request.tunnel.as_str() {
                "quick" => CloudflareTunnel::Quick,
                "named" => CloudflareTunnel::Named,
                _ => return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_config")),
            };
            config.remote.cloudflare.hostname = request.hostname;
            config.remote.cloudflare.token_file = request.token_file.map(Into::into);
            (RemoteMode::Cloudflare, request.enabled)
        }
        Change::Relay(request) => {
            config.remote.relay.url = request.url.map(|url| url.trim_end_matches('/').to_owned());
            config.remote.relay.token_file = request.token_file.map(Into::into);
            (RemoteMode::Relay, request.enabled)
        }
    };
    if enabled {
        if !config.remote.mode.contains(&mode) {
            config.remote.mode.push(mode);
        }
    } else {
        config.remote.mode.retain(|item| *item != mode);
    }
    let listen = *shared.transports.listen.lock();
    if enabled && mode != RemoteMode::Relay && config.daemon.listen.is_none() {
        config.daemon.listen = Some(
            "127.0.0.1:47820"
                .parse()
                .map_err(|_| HttpError::internal())?,
        );
    }
    let value = serde_json::to_value(&config).map_err(|_| HttpError::internal())?;
    crate::settings_api::persist(&shared, value).await?;
    let mut running = effective(&shared);
    match mode {
        RemoteMode::Cloudflare => {
            let unchanged = running.has(mode) == enabled
                && running.cloudflare == config.remote.cloudflare
                && (!enabled
                    || shared
                        .transports
                        .cloudflare
                        .lock()
                        .as_ref()
                        .is_some_and(|supervisor| supervisor.status().state != "failed"));
            if !unchanged {
                let old = shared.transports.cloudflare.lock().take();
                if let Some(old) = old {
                    old.stop().await;
                }
                if enabled {
                    if let Some(listen) = listen {
                        spawn_cloudflare(&shared, &config.remote, listen);
                    }
                }
            }
            running.cloudflare = config.remote.cloudflare.clone();
        }
        RemoteMode::Relay => {
            let unchanged = running.has(mode) == enabled
                && running.relay == config.remote.relay
                && (!enabled || shared.transports.relay.lock().is_some());
            if !unchanged {
                drop(shared.transports.relay.lock().take());
                *shared.transports.relay_error.lock() = None;
                if enabled {
                    spawn_relay(&shared, &config.remote);
                }
            }
            running.relay = config.remote.relay.clone();
        }
        RemoteMode::Tailscale => {
            if let Some(listen) = listen {
                if let Some(url) = tailscale::configure(enabled, listen).await? {
                    config.remote.public_url = Some(url);
                    crate::settings_api::persist(
                        &shared,
                        serde_json::to_value(&config).map_err(|_| HttpError::internal())?,
                    )
                    .await?;
                }
            }
            running.public_url = config.remote.public_url.clone();
        }
    }
    if enabled && (mode == RemoteMode::Relay || listen.is_some()) {
        if !running.mode.contains(&mode) {
            running.mode.push(mode);
        }
    } else {
        running.mode.retain(|item| *item != mode);
    }
    *shared.transports.effective.lock() = Some(running);
    crate::pairing::audit(
        &shared,
        "remote",
        "configured",
        serde_json::json!({"transport":transport(mode),"enabled":enabled}),
    )
    .await;
    Ok(Json(document(&shared, true).await?))
}
