//! Daemon side: keep an outbound WebSocket to the relay (no inbound port at home) and
//! answer each device connection with a Noise responder and a stream multiplexer.

use std::{collections::HashMap, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    client::PING_INTERVAL,
    mux::{self, Accept, Role},
    net::{self, Origin},
    noise::{self, StaticKeypair},
    server::{frame, CLOSE, DATA, OPEN},
    Error,
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const QUEUE: usize = 64;
const MAX_CLIENTS: usize = 32;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

struct ClientInbox {
    tx: mpsc::Sender<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ClientInbox {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct CloseSession(mux::Session);

impl Drop for CloseSession {
    fn drop(&mut self) {
        self.0.close();
    }
}

struct AbortTask(tokio::task::JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct CloseConnection {
    conn: u32,
    control: mpsc::Sender<Vec<u8>>,
    stop: watch::Sender<bool>,
}

impl Drop for CloseConnection {
    fn drop(&mut self) {
        if self.control.try_send(frame(CLOSE, self.conn, &[])).is_err() {
            self.stop.send_replace(true);
        }
    }
}

async fn register_identity(
    socket: &mut tokio_tungstenite::WebSocketStream<net::BoxIo>,
    config: &HostConfig,
    id: &str,
) -> Result<(), Error> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        socket
            .send(Message::Binary(config.keypair.public.to_vec().into()))
            .await
            .map_err(|_| Error::Closed)?;
        let Some(Ok(Message::Binary(first))) = socket.next().await else {
            return Err(Error::Handshake);
        };
        let (_, reply) = noise::respond_registration(&config.keypair, id, &first)?;
        socket
            .send(Message::Binary(reply.into()))
            .await
            .map_err(|_| Error::Closed)?;
        match socket.next().await {
            Some(Ok(Message::Binary(ready))) if ready.as_ref() == [0] => Ok(()),
            _ => Err(Error::Unauthorized),
        }
    })
    .await
    .map_err(|_| Error::Handshake)?
}

fn dispatch_data(conns: &mut HashMap<u32, ClientInbox>, conn: u32, bytes: Vec<u8>) -> bool {
    if conns
        .get(&conn)
        .is_some_and(|inbox| inbox.tx.try_send(bytes).is_err())
    {
        conns.remove(&conn);
        return false;
    }
    true
}

/// Where and how the daemon registers.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Relay origin, `https://…` (or `http://` on loopback).
    pub relay_url: String,
    /// Registration token the relay operator issued.
    pub token: String,
    /// The daemon's static key; its public half is in every relay pairing QR.
    pub keypair: StaticKeypair,
}

/// Link state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostState {
    /// Dialling the relay.
    Connecting,
    /// Registered; devices can connect.
    Connected,
    /// Waiting before the next attempt after a failure.
    Backoff,
}

/// Snapshot for `GET /v1/remote/status` and `openside doctor`.
#[derive(Clone, Debug)]
pub struct HostStatus {
    /// Current state.
    pub state: HostState,
    /// Devices connected through the relay right now.
    pub clients: u32,
    /// Most recent failure, if any (never contains the token).
    pub last_error: Option<String>,
}

/// Running registration; dropping it disconnects.
pub struct Host {
    status: Arc<Mutex<HostStatus>>,
    task: tokio::task::JoinHandle<()>,
}

impl Host {
    /// Current link state.
    pub fn status(&self) -> HostStatus {
        self.status.lock().clone()
    }

    /// Disconnect and stop reconnecting.
    pub fn shutdown(&self) {
        self.task.abort();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start registering with the relay; every stream a device opens goes to `accept`.
pub fn spawn(config: HostConfig, accept: Accept) -> Host {
    let status = Arc::new(Mutex::new(HostStatus {
        state: HostState::Connecting,
        clients: 0,
        last_error: None,
    }));
    let task = tokio::spawn(run(config, accept, status.clone()));
    Host { status, task }
}

fn jitter(base: Duration) -> Duration {
    let mut byte = [0_u8; 1];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut byte);
    base + base.mul_f64(f64::from(byte[0]) / 512.0)
}

async fn run(config: HostConfig, accept: Accept, status: Arc<Mutex<HostStatus>>) {
    let id = noise::daemon_id(&config.keypair.public);
    let mut backoff = Duration::from_secs(1);
    let origin = match Origin::parse(&config.relay_url) {
        Ok(origin) => origin,
        Err(_) => {
            let mut status = status.lock();
            status.state = HostState::Backoff;
            status.last_error = Some("remote.relay.url is not an https:// origin".into());
            return;
        }
    };
    loop {
        status.lock().state = HostState::Connecting;
        let error = match net::websocket(
            &origin,
            &format!("/v1/relay/daemon/{id}"),
            Some(&config.token),
        )
        .await
        {
            Ok(mut socket) => {
                if let Err(error) = register_identity(&mut socket, &config, &id).await {
                    status.lock().last_error = Some(error.to_string());
                    status.lock().state = HostState::Backoff;
                    tokio::time::sleep(jitter(backoff)).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
                backoff = Duration::from_secs(1);
                {
                    let mut status = status.lock();
                    status.state = HostState::Connected;
                    status.last_error = None;
                }
                link(socket, &config, &id, &accept, &status).await;
                "relay connection closed".to_owned()
            }
            Err(Error::Unauthorized) => "the relay rejected the registration token".to_owned(),
            Err(error) => error.to_string(),
        };
        {
            let mut status = status.lock();
            status.state = HostState::Backoff;
            status.clients = 0;
            status.last_error = Some(error);
        }
        tokio::time::sleep(jitter(backoff)).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn link(
    socket: tokio_tungstenite::WebSocketStream<net::BoxIo>,
    config: &HostConfig,
    id: &str,
    accept: &Accept,
    status: &Arc<Mutex<HostStatus>>,
) {
    let (mut sink, mut stream) = socket.split();
    let (ws_tx, mut ws_rx) = mpsc::channel::<Vec<u8>>(256);
    let (control, mut control_rx) = mpsc::channel::<Vec<u8>>(QUEUE);
    let (stop, mut stopped) = watch::channel(false);
    let _writer = AbortTask(tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        loop {
            let message = tokio::select! {
                biased;
                message = control_rx.recv() => match message {
                    Some(message) => Message::Binary(message.into()),
                    None => break,
                },
                message = ws_rx.recv() => match message {
                    Some(message) => Message::Binary(message.into()),
                    None => break,
                },
                _ = ping.tick() => Message::Ping(Vec::new().into()),
            };
            if !matches!(
                tokio::time::timeout(WRITE_TIMEOUT, sink.send(message)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    }));
    let mut conns: HashMap<u32, ClientInbox> = HashMap::new();
    loop {
        let message = tokio::select! {
            message = stream.next() => message,
            _ = ws_tx.closed() => break,
            _ = stopped.changed() => break,
        };
        let Some(Ok(message)) = message else { break };
        let bytes = match message {
            Message::Binary(bytes) => bytes,
            Message::Close(_) => break,
            _ => continue,
        };
        if bytes.len() < 5 || bytes.len() > noise::MAX_MESSAGE + 5 {
            break;
        }
        let conn = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        match bytes[0] {
            OPEN => {
                if conns.contains_key(&conn) || conns.len() >= MAX_CLIENTS {
                    if control.try_send(frame(CLOSE, conn, &[])).is_err() {
                        break;
                    }
                    continue;
                }
                let (in_tx, in_rx) = mpsc::channel(QUEUE);
                let task = tokio::spawn(serve_conn(
                    in_rx,
                    ws_tx.clone(),
                    CloseConnection {
                        conn,
                        control: control.clone(),
                        stop: stop.clone(),
                    },
                    config.keypair.clone(),
                    id.to_owned(),
                    accept.clone(),
                ));
                conns.insert(conn, ClientInbox { tx: in_tx, task });
            }
            DATA => {
                if !dispatch_data(&mut conns, conn, bytes[5..].to_vec())
                    && control.try_send(frame(CLOSE, conn, &[])).is_err()
                {
                    break;
                }
            }
            CLOSE => {
                conns.remove(&conn);
            }
            _ => break,
        }
        conns.retain(|_, inbox| !inbox.tx.is_closed());
        status.lock().clients = u32::try_from(conns.len()).unwrap_or(u32::MAX);
    }
}

async fn serve_conn(
    mut inbound: mpsc::Receiver<Vec<u8>>,
    ws_tx: mpsc::Sender<Vec<u8>>,
    _close: CloseConnection,
    keypair: StaticKeypair,
    id: String,
    accept: Accept,
) {
    let conn = _close.conn;
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, inbound.recv()).await;
    let Ok(Some(first)) = first else {
        return;
    };
    let Ok((transport, reply)) = noise::respond(&keypair, &id, &first) else {
        return;
    };
    if !matches!(
        tokio::time::timeout(WRITE_TIMEOUT, ws_tx.send(frame(DATA, conn, &reply))).await,
        Ok(Ok(()))
    ) {
        return;
    }
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(QUEUE);
    let _session = CloseSession(mux::spawn(
        transport,
        Role::Daemon,
        out_tx,
        inbound,
        Some(accept),
    ));
    while let Some(message) = out_rx.recv().await {
        if !matches!(
            tokio::time::timeout(WRITE_TIMEOUT, ws_tx.send(frame(DATA, conn, &message))).await,
            Ok(Ok(()))
        ) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[tokio::test]
    async fn a_full_client_inbox_does_not_block_other_data_or_close() {
        let mut conns = HashMap::new();
        let (slow, _slow_rx) = mpsc::channel(1);
        let (fast, mut fast_rx) = mpsc::channel(1);
        slow.try_send(vec![1]).unwrap();
        conns.insert(
            1,
            ClientInbox {
                tx: slow,
                task: tokio::spawn(std::future::pending()),
            },
        );
        conns.insert(
            2,
            ClientInbox {
                tx: fast,
                task: tokio::spawn(std::future::pending()),
            },
        );
        assert!(!dispatch_data(&mut conns, 1, vec![2]));
        assert!(!conns.contains_key(&1));
        assert!(dispatch_data(&mut conns, 2, vec![3]));
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), fast_rx.recv())
                .await
                .unwrap(),
            Some(vec![3])
        );
        conns.remove(&2);
        assert_eq!(fast_rx.recv().await, None);
    }
}
