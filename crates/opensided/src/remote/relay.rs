//! Relay transport (remote.mode = "relay").
//!
//! The daemon registers with `remote.relay.url` over an outbound WebSocket and serves the
//! loopback API router on every end-to-end encrypted stream a device opens, so remote callers
//! get exactly the bearer-token surface of `daemon.listen`. The static key lives in
//! `<data_dir>/relay-key` (0600); its public half and routing id go into relay pairing links,
//! and deleting the file makes every relay-paired device pair again.

use std::path::Path;

use openside_proto::RelayStatus;
use openside_relay::{
    host::{self, Host, HostConfig, HostState},
    mux::MuxStream,
    noise::{self, StaticKeypair},
};
use tokio::sync::mpsc;

use crate::engine::Shared;

const KEY_FILE: &str = "relay-key";

/// A running relay registration and the API server behind it.
pub(crate) struct Link {
    host: Host,
    relay_url: String,
    daemon_id: String,
    daemon_key: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.host.shutdown();
        self.server.abort();
    }
}

impl Link {
    pub(crate) fn relay_url(&self) -> &str {
        &self.relay_url
    }
    pub(crate) fn daemon_id(&self) -> &str {
        &self.daemon_id
    }
    pub(crate) fn daemon_key(&self) -> &str {
        &self.daemon_key
    }
    pub(crate) fn status(&self) -> RelayStatus {
        let status = self.host.status();
        RelayStatus {
            state: match status.state {
                HostState::Connecting => "connecting",
                HostState::Connected => "connected",
                HostState::Backoff => "backoff",
            }
            .into(),
            url: self.relay_url.clone(),
            daemon_id: self.daemon_id.clone(),
            daemon_key: self.daemon_key.clone(),
            clients: status.clients,
            last_error: status.last_error,
        }
    }
}

/// Load the daemon's relay key, creating it on first use.
pub(crate) fn load_or_create_key(data_dir: &Path) -> Result<StaticKeypair, String> {
    let path = data_dir.join(KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => noise::decode_key(text.trim())
            .map(StaticKeypair::from_secret)
            .ok_or_else(|| {
                format!(
                    "{} is not a relay key; delete it to make a new one",
                    path.display()
                )
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let keypair = StaticKeypair::generate();
            crate::persistence::write_private(&path, noise::encode_key(&keypair.secret).as_bytes())
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            Ok(keypair)
        }
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Streams from relayed devices, served like accepted TCP connections.
struct Streams(mpsc::UnboundedReceiver<MuxStream>);

impl axum::serve::Listener for Streams {
    type Io = MuxStream;
    type Addr = ();

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.0.recv().await {
            Some(stream) => (stream, ()),
            // Every sender is gone: the link was dropped and this server is being aborted.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(())
    }
}

pub(crate) fn start(shared: &Shared) -> Result<Link, String> {
    let remote = &shared.config.loaded.config.remote;
    let relay_url = remote
        .relay
        .url
        .clone()
        .ok_or("remote.relay.url is not set")?
        .trim_end_matches('/')
        .to_owned();
    let token =
        openside_config::read_secret(remote.relay.token_file.as_deref(), "OPENSIDE_RELAY_TOKEN")
            .ok_or(
                "no relay registration token (remote.relay.token_file or OPENSIDE_RELAY_TOKEN)",
            )?;
    let keypair = load_or_create_key(&shared.config.data_dir)?;
    let daemon_id = noise::daemon_id(&keypair.public);
    let daemon_key = noise::encode_key(&keypair.public);
    let (streams, incoming) = mpsc::unbounded_channel();
    let router = crate::api::router(shared.clone(), false);
    let server = tokio::spawn(async move {
        let _ = axum::serve(Streams(incoming), router).await;
    });
    let host = host::spawn(
        HostConfig {
            relay_url: relay_url.clone(),
            token,
            keypair,
        },
        std::sync::Arc::new(move |stream| {
            let _ = streams.send(stream);
        }),
    );
    Ok(Link {
        host,
        relay_url,
        daemon_id,
        daemon_key,
        server,
    })
}
