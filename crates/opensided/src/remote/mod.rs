//! Remote transports (F-8): how devices outside the home network reach the daemon.
//!
//! - `tailscale`: Tailscale Serve proxies `remote.public_url` to the loopback listener; the
//!   daemon only reports it.
//! - `cloudflare`: the daemon supervises `cloudflared` ([`cloudflare`]); its public URL goes
//!   into pairing links.
//! - `relay`: the daemon keeps an outbound WebSocket to an `openside-relay` and serves the API
//!   over end-to-end encrypted streams ([`relay`]); the relay sees only ciphertext.
//!
//! Every transport ends at the same bearer-token API as the loopback listener
//! (`api::router(_, false)`), so remote callers never get Unix-socket powers.

pub(crate) mod cloudflare;
pub(crate) mod relay;

use std::net::SocketAddr;

use axum::{extract::State, http::StatusCode, Json};
use openside_config::RemoteMode;
use openside_proto::{PairingTransport, RemoteStatus};
use parking_lot::Mutex;

use crate::{api::HttpError, engine::Shared};

/// Running transports; empty until [`start`].
#[derive(Default)]
pub(crate) struct Transports {
    cloudflare: Mutex<Option<cloudflare::Supervisor>>,
    relay: Mutex<Option<relay::Link>>,
}

impl Transports {
    /// Stop every transport (daemon shutdown). Dropping the handles kills `cloudflared` and
    /// closes the relay link.
    pub(crate) async fn stop(&self) {
        let supervisor = self.cloudflare.lock().take();
        if let Some(supervisor) = supervisor {
            supervisor.stop().await;
        }
        let link = self.relay.lock().take();
        drop(link);
    }
}

pub(crate) fn transport(mode: RemoteMode) -> PairingTransport {
    match mode {
        RemoteMode::Tailscale => PairingTransport::Tailscale,
        RemoteMode::Cloudflare => PairingTransport::Cloudflare,
        RemoteMode::Relay => PairingTransport::Relay,
    }
}

/// Start the configured transports. `listen` is the bound loopback listener that Cloudflare
/// Tunnel forwards to; the relay serves the API without it.
pub(crate) fn start(shared: &Shared, listen: Option<SocketAddr>) {
    let remote = &shared.config.loaded.config.remote;
    if remote.has(RemoteMode::Cloudflare) {
        match listen {
            Some(listen) => {
                let token = matches!(
                    remote.cloudflare.tunnel,
                    openside_config::CloudflareTunnel::Named
                )
                .then(|| {
                    openside_config::read_secret(
                        remote.cloudflare.token_file.as_deref(),
                        "OPENSIDE_CLOUDFLARE_TOKEN",
                    )
                })
                .flatten();
                let supervisor = cloudflare::spawn(
                    cloudflare::Options::from_config(&remote.cloudflare, listen, token),
                    shared.shutdown.subscribe(),
                );
                *shared.transports.cloudflare.lock() = Some(supervisor);
            }
            None => crate::diag!(
                "opensided: remote.mode cloudflare needs daemon.listen; tunnel not started"
            ),
        }
    }
    if remote.has(RemoteMode::Relay) {
        match relay::start(shared) {
            Ok(link) => *shared.transports.relay.lock() = Some(link),
            Err(error) => crate::diag!("opensided: relay not started: {error}"),
        }
    }
}

/// Public base URL (no trailing slash) for a direct transport, if it is up.
fn direct_base(shared: &Shared, via: PairingTransport) -> Option<String> {
    let remote = &shared.config.loaded.config.remote;
    match via {
        PairingTransport::Tailscale => remote
            .public_url
            .as_deref()
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

/// Pairing link for `code`: `(url, transport)`. Without `via`, the first enabled mode, or
/// the plain `remote.public_url` (no transport) when none is enabled.
pub(crate) fn pairing_link(
    shared: &Shared,
    via: Option<PairingTransport>,
    code: &str,
) -> Result<(Option<String>, Option<PairingTransport>), HttpError> {
    let remote = &shared.config.loaded.config.remote;
    let enabled: Vec<PairingTransport> = remote.mode.iter().copied().map(transport).collect();
    let via = match via.or_else(|| enabled.first().copied()) {
        None => {
            let url = remote
                .public_url
                .as_deref()
                .map(|base| format!("{}/pair#{code}", base.trim_end_matches('/')));
            return Ok((url, None));
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

/// `GET /v1/remote/status` (admin).
pub(crate) async fn status(State(shared): State<Shared>) -> Json<RemoteStatus> {
    Json(snapshot(&shared))
}

pub(crate) fn snapshot(shared: &Shared) -> RemoteStatus {
    let remote = &shared.config.loaded.config.remote;
    RemoteStatus {
        modes: remote.mode.iter().copied().map(transport).collect(),
        public_url: remote.public_url.clone(),
        cloudflare: shared
            .transports
            .cloudflare
            .lock()
            .as_ref()
            .map(cloudflare::Supervisor::status),
        relay: shared
            .transports
            .relay
            .lock()
            .as_ref()
            .map(relay::Link::status),
    }
}
