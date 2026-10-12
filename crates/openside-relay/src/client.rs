//! Device side: one Noise session to a daemon through the relay, carrying many streams.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{
    mux::{self, MuxStream, Role},
    net::{self, Origin},
    noise, Error,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
// Below the idle timeouts of common fronts (Cloudflare closes idle WebSockets at 100 s).
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(20);
const QUEUE: usize = 64;

/// An encrypted session to one daemon. Cheap to clone; every clone shares the session.
#[derive(Clone)]
pub struct Tunnel {
    session: mux::Session,
    origin: Origin,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel")
            .field("relay", &self.origin.http_url())
            .finish()
    }
}

/// Daemon ids are 22 base64url characters (see [`noise::daemon_id`]).
pub fn valid_daemon_id(id: &str) -> bool {
    id.len() == 22
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Tunnel {
    /// Connect to `relay_url` (an `https://` origin, or `http://` on loopback) and complete
    /// the Noise handshake with the daemon whose static key is `daemon_key`.
    pub async fn connect(
        relay_url: &str,
        daemon_id: &str,
        daemon_key: &[u8; 32],
    ) -> Result<Tunnel, Error> {
        if !valid_daemon_id(daemon_id) || noise::daemon_id(daemon_key) != daemon_id {
            return Err(Error::Protocol("daemon id does not match the daemon key"));
        }
        let origin = Origin::parse(relay_url)?;
        let mut socket =
            net::websocket(&origin, &format!("/v1/relay/connect/{daemon_id}"), None).await?;
        let (state, first) = noise::initiate(daemon_id, daemon_key)?;
        tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            socket.send(Message::Binary(first.into())),
        )
        .await
        .map_err(|_| Error::Handshake)?
        .map_err(|_| Error::Closed)?;
        let reply = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            while let Some(message) = socket.next().await {
                match message {
                    Ok(Message::Binary(bytes)) => return Ok(bytes),
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            Err(Error::Closed)
        })
        .await
        .map_err(|_| Error::Handshake)??;
        let transport = noise::finish(state, &reply)?;

        let (mut sink, mut stream) = socket.split();
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        tokio::spawn(async move {
            let writer = async {
                let mut ping = tokio::time::interval(PING_INTERVAL);
                ping.tick().await;
                loop {
                    let message = tokio::select! {
                        message = out_rx.recv() => match message {
                            Some(message) => Message::Binary(message.into()),
                            None => break,
                        },
                        _ = ping.tick() => Message::Ping(Vec::new().into()),
                    };
                    if !matches!(
                        tokio::time::timeout(Duration::from_secs(5), sink.send(message)).await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
            };
            let reader = async {
                while let Some(Ok(message)) = stream.next().await {
                    match message {
                        Message::Binary(bytes) => {
                            if bytes.len() > noise::MAX_MESSAGE
                                || !matches!(
                                    tokio::time::timeout(
                                        Duration::from_secs(5),
                                        in_tx.send(bytes.to_vec())
                                    )
                                    .await,
                                    Ok(Ok(()))
                                )
                            {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            };
            tokio::select! { _ = writer => {}, _ = reader => {}, _ = in_tx.closed() => {} }
            let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
        });
        Ok(Tunnel {
            session: mux::spawn(transport, Role::Client, out_tx, in_rx, None),
            origin,
        })
    }

    /// Open a new stream to the daemon's HTTP API.
    pub async fn open(&self) -> Result<MuxStream, Error> {
        self.session.open().await
    }

    /// Whether the session has ended (relay or daemon went away); reconnect with
    /// [`Tunnel::connect`].
    pub fn is_closed(&self) -> bool {
        self.session.is_closed()
    }

    /// The relay origin, e.g. for the `Host` header of tunnelled requests.
    pub fn relay(&self) -> &Origin {
        &self.origin
    }
}
