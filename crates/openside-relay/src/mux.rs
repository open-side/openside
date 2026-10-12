//! Stream multiplexing inside one Noise session.
//!
//! Every Noise transport message carries one frame `[kind u8][stream u32 BE][payload]`.
//! Devices open streams (odd ids); the daemon accepts them. Each direction of each stream
//! has a credit window, so one slow reader (an idle SSE consumer, a paused screencast)
//! cannot make the other streams' frames pile up in memory: a sender only sends what the
//! receiver has room for, and the receiver grants more as the application consumes.

use std::{
    collections::{HashMap, VecDeque},
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use bytes::{Buf, Bytes};
use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc, oneshot},
};

use crate::{noise::TAG_LEN, Error};

/// Device → daemon: a new stream.
pub const OPEN: u8 = 1;
/// Payload bytes.
pub const DATA: u8 = 2;
/// The sender finished writing; the other direction stays open.
pub const FIN: u8 = 3;
/// Abort both directions.
pub const RST: u8 = 4;
/// Credit increment (u32 BE) for the receiver of this frame's sends.
pub const WINDOW: u8 = 5;
/// Credit each direction of a new stream starts with.
pub const INITIAL_WINDOW: u32 = 262_144;
/// Largest `DATA` payload.
pub const MAX_DATA: usize = 16_384;
/// Concurrent streams per session; further `OPEN`s are reset.
pub const MAX_STREAMS: usize = 128;
/// Maximum queued DATA frames per direction, independently of their byte lengths.
pub const MAX_QUEUED_FRAMES: usize = 256;
const HEADER: usize = 5;
// Grant credit back in quarter-window steps rather than per read.
const GRANT_STEP: u64 = INITIAL_WINDOW as u64 / 4;

/// Which end of the session this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Opens streams.
    Client,
    /// Accepts streams.
    Daemon,
}

/// Callback receiving every stream a device opens.
pub type Accept = Arc<dyn Fn(MuxStream) + Send + Sync>;

#[derive(Default)]
struct StreamState {
    recv: VecDeque<Bytes>,
    recv_fin: bool,
    reset: bool,
    recv_waker: Option<Waker>,
    send_credit: u64,
    send_waker: Option<Waker>,
    send_queued: usize,
    send_outstanding: u64,
    // Bytes the application consumed that have not been granted back yet.
    consumed: u64,
    // Bytes the peer may still send before it violates the window.
    recv_allowed: u64,
    fin_sent: bool,
}

impl StreamState {
    fn new() -> Self {
        Self {
            send_credit: INITIAL_WINDOW.into(),
            recv_allowed: INITIAL_WINDOW.into(),
            ..Self::default()
        }
    }

    fn receive_data(&mut self, payload: &[u8]) -> bool {
        if payload.is_empty()
            || payload.len() > MAX_DATA
            || payload.len() as u64 > self.recv_allowed
            || self.recv_fin
            || self.recv.len() >= MAX_QUEUED_FRAMES
        {
            return false;
        }
        self.recv_allowed -= payload.len() as u64;
        self.recv.push_back(Bytes::copy_from_slice(payload));
        if let Some(waker) = self.recv_waker.take() {
            waker.wake();
        }
        true
    }

    fn grant(&mut self, grant: u32) -> bool {
        if grant == 0 || u64::from(grant) > self.send_outstanding {
            return false;
        }
        self.send_outstanding -= u64::from(grant);
        self.send_credit += u64::from(grant);
        if let Some(waker) = self.send_waker.take() {
            waker.wake();
        }
        true
    }

    fn abort(&mut self) {
        self.reset = true;
        // Data before a FIN is complete (a response followed by the peer dropping its stream),
        // so it stays readable; anything else is cut off.
        if !self.recv_fin {
            self.recv.clear();
        }
        if let Some(waker) = self.recv_waker.take() {
            waker.wake();
        }
        if let Some(waker) = self.send_waker.take() {
            waker.wake();
        }
    }
}

pub(crate) enum Cmd {
    Frame(u8, u32, Bytes),
    Open(oneshot::Sender<Result<MuxStream, Error>>),
    Close,
}

/// One bidirectional byte stream (an HTTP exchange, possibly upgraded to a WebSocket).
pub struct MuxStream {
    id: u32,
    state: Arc<Mutex<StreamState>>,
    tx: mpsc::UnboundedSender<Cmd>,
}

impl std::fmt::Debug for MuxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuxStream").field("id", &self.id).finish()
    }
}

impl MuxStream {
    /// Stream id (odd, chosen by the device).
    pub fn id(&self) -> u32 {
        self.id
    }
}

impl AsyncRead for MuxStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = self.state.lock();
        if state.reset && !state.recv_fin {
            return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
        }
        if state.recv.is_empty() {
            if state.recv_fin {
                return Poll::Ready(Ok(()));
            }
            state.recv_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let mut copied = 0;
        while buf.remaining() > 0 {
            let Some(front) = state.recv.front_mut() else {
                break;
            };
            let n = front.len().min(buf.remaining());
            buf.put_slice(&front[..n]);
            front.advance(n);
            if front.is_empty() {
                state.recv.pop_front();
            }
            copied += n;
        }
        state.consumed += copied as u64;
        if state.consumed >= GRANT_STEP && !state.recv_fin {
            let grant = u32::try_from(state.consumed).unwrap_or(u32::MAX);
            state.consumed -= u64::from(grant);
            state.recv_allowed += u64::from(grant);
            let _ = self.tx.send(Cmd::Frame(
                WINDOW,
                self.id,
                Bytes::copy_from_slice(&grant.to_be_bytes()),
            ));
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MuxStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = self.state.lock();
        if state.reset || state.fin_sent {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if state.send_credit == 0 || state.send_queued >= MAX_QUEUED_FRAMES {
            state.send_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = buf
            .len()
            .min(MAX_DATA)
            .min(usize::try_from(state.send_credit).unwrap_or(MAX_DATA));
        state.send_credit -= n as u64;
        state.send_queued += 1;
        drop(state);
        self.tx
            .send(Cmd::Frame(DATA, self.id, Bytes::copy_from_slice(&buf[..n])))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.state.lock();
        if !state.fin_sent && !state.reset {
            state.fin_sent = true;
            drop(state);
            let _ = self.tx.send(Cmd::Frame(FIN, self.id, Bytes::new()));
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for MuxStream {
    fn drop(&mut self) {
        let state = self.state.lock();
        if !state.reset && !(state.fin_sent && state.recv_fin) {
            let _ = self.tx.send(Cmd::Frame(RST, self.id, Bytes::new()));
        }
    }
}

/// Handle used to open streams on a client session; the session ends when every handle
/// and stream is dropped or the transport fails.
#[derive(Clone)]
pub(crate) struct Session {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Session {
    pub(crate) async fn open(&self) -> Result<MuxStream, Error> {
        let (reply, response) = oneshot::channel();
        self.tx.send(Cmd::Open(reply)).map_err(|_| Error::Closed)?;
        response.await.map_err(|_| Error::Closed)?
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    pub(crate) fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

/// Run a session over an established Noise transport. `outbound` receives ciphertext
/// messages to send; `inbound` yields received ciphertext. The session ends when either
/// side closes, a message fails to decrypt, or (client) every handle is dropped.
pub(crate) fn spawn(
    transport: snow::TransportState,
    role: Role,
    outbound: mpsc::Sender<Vec<u8>>,
    inbound: mpsc::Receiver<Vec<u8>>,
    accept: Option<Accept>,
) -> Session {
    let (tx, rx) = mpsc::unbounded_channel();
    let weak = tx.downgrade();
    // A daemon session lives as long as its transport, not as long as some handle does.
    let keep = (role == Role::Daemon).then(|| tx.clone());
    tokio::spawn(async move {
        let _keep = keep;
        drive(transport, role, outbound, inbound, rx, weak, accept).await;
    });
    Session { tx }
}

fn encode(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&id.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

async fn drive(
    mut transport: snow::TransportState,
    role: Role,
    outbound: mpsc::Sender<Vec<u8>>,
    mut inbound: mpsc::Receiver<Vec<u8>>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    weak: mpsc::WeakUnboundedSender<Cmd>,
    accept: Option<Accept>,
) {
    let mut streams: HashMap<u32, Arc<Mutex<StreamState>>> = HashMap::new();
    let mut next_id: u32 = 1;
    let mut highest_peer_id = 0;
    let mut plain = vec![0_u8; crate::noise::MAX_MESSAGE];
    'session: loop {
        // Frames this iteration must send, in order.
        let mut send: Vec<Vec<u8>> = Vec::new();
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break 'session };
                match cmd {
                    Cmd::Close => break 'session,
                    Cmd::Frame(kind, id, payload) => {
                        match kind {
                            DATA => {
                                if let Some(state) = streams.get(&id) {
                                    let mut state = state.lock();
                                    state.send_queued -= 1;
                                    state.send_outstanding += payload.len() as u64;
                                    if let Some(waker) = state.send_waker.take() { waker.wake(); }
                                } else { continue; }
                            }
                            RST => { streams.remove(&id); }
                            FIN => {
                                if streams.get(&id).is_some_and(|s| s.lock().recv_fin) {
                                    streams.remove(&id);
                                }
                            }
                            // Frames of a stream that is already gone are dropped, except the
                            // RST above, which tells the peer.
                            _ if !streams.contains_key(&id) => continue,
                            _ => {}
                        }
                        send.push(encode(kind, id, &payload));
                    }
                    Cmd::Open(reply) => {
                        if role != Role::Client || streams.len() >= MAX_STREAMS {
                            let _ = reply.send(Err(Error::Protocol("too many streams")));
                            continue;
                        }
                        let Some(tx) = weak.upgrade() else { break 'session };
                        let id = next_id;
                        let Some(next) = next_id.checked_add(2) else {
                            let _ = reply.send(Err(Error::Protocol("stream ids exhausted")));
                            break 'session;
                        };
                        next_id = next;
                        let state = Arc::new(Mutex::new(StreamState::new()));
                        streams.insert(id, state.clone());
                        send.push(encode(OPEN, id, &[]));
                        if reply.send(Ok(MuxStream { id, state, tx })).is_err() {
                            // The opener gave up; its stream was dropped without a RST
                            // because it never existed for it.
                            streams.remove(&id);
                            send.push(encode(RST, id, &[]));
                        }
                    }
                }
            }
            message = inbound.recv() => {
                let Some(message) = message else { break 'session };
                let Ok(len) = transport.read_message(&message, &mut plain) else { break 'session };
                if len < HEADER {
                    break 'session;
                }
                let kind = plain[0];
                let id = u32::from_be_bytes([plain[1], plain[2], plain[3], plain[4]]);
                let payload = &plain[HEADER..len];
                match kind {
                    OPEN => {
                        // Commands can outlive a retired stream, so its id must never select a new state.
                        let fresh = role == Role::Daemon && id % 2 == 1 && id > highest_peer_id;
                        if fresh { highest_peer_id = id; }
                        match (&accept, weak.upgrade()) {
                            (Some(accept), Some(tx)) if fresh && streams.len() < MAX_STREAMS => {
                                let state = Arc::new(Mutex::new(StreamState::new()));
                                streams.insert(id, state.clone());
                                accept(MuxStream { id, state, tx });
                            }
                            _ => send.push(encode(RST, id, &[])),
                        }
                    }
                    DATA => {
                        if let Some(state) = streams.get(&id) {
                            let mut state = state.lock();
                            if !state.receive_data(payload) {
                                state.abort();
                                drop(state);
                                streams.remove(&id);
                                send.push(encode(RST, id, &[]));
                            }
                        }
                    }
                    FIN => {
                        if let Some(state) = streams.get(&id) {
                            let mut guard = state.lock();
                            guard.recv_fin = true;
                            if let Some(waker) = guard.recv_waker.take() { waker.wake(); }
                            let done = guard.fin_sent;
                            drop(guard);
                            if done { streams.remove(&id); }
                        }
                    }
                    RST => {
                        if let Some(state) = streams.remove(&id) {
                            state.lock().abort();
                        }
                    }
                    WINDOW if payload.len() == 4 => {
                        if let Some(state) = streams.get(&id) {
                            let grant = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                            let mut state = state.lock();
                            if !state.grant(grant) {
                                state.abort();
                                drop(state);
                                streams.remove(&id);
                                send.push(encode(RST, id, &[]));
                            }
                        }
                    }
                    _ => break 'session,
                }
            }
        }
        for frame in send {
            let mut message = vec![0_u8; frame.len() + TAG_LEN];
            let Ok(len) = transport.write_message(&frame, &mut message) else {
                break 'session;
            };
            message.truncate(len);
            if !matches!(
                tokio::time::timeout(std::time::Duration::from_secs(5), outbound.send(message))
                    .await,
                Ok(Ok(()))
            ) {
                break 'session;
            }
        }
    }
    for state in streams.values() {
        state.lock().abort();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn receive_queue_bounds_entries_and_bytes_including_empty_data() {
        let mut state = StreamState::new();
        assert!(!state.receive_data(&[]));
        assert!(state.recv.is_empty());
        for _ in 0..MAX_QUEUED_FRAMES {
            assert!(state.receive_data(&[1]));
        }
        assert!(!state.receive_data(&[1]));
        assert_eq!(state.recv.len(), MAX_QUEUED_FRAMES);
        let mut state = StreamState::new();
        for _ in 0..INITIAL_WINDOW as usize / MAX_DATA {
            assert!(state.receive_data(&[0; MAX_DATA]));
        }
        assert_eq!(state.recv_allowed, 0);
        assert!(!state.receive_data(&[1]));
    }

    #[test]
    fn window_cannot_grant_unsent_or_already_credited_bytes() {
        let mut state = StreamState::new();
        assert!(!state.grant(1));
        assert!(!state.grant(0));
        state.send_credit -= 12;
        state.send_outstanding = 12;
        assert!(!state.grant(13));
        assert!(state.grant(12));
        assert_eq!(state.send_credit, u64::from(INITIAL_WINDOW));
        assert!(!state.grant(1));
    }

    async fn inject(
        transport: &mut snow::TransportState,
        inbound: &mpsc::Sender<Vec<u8>>,
        kind: u8,
        id: u32,
        payload: &[u8],
    ) {
        let plain = encode(kind, id, payload);
        let mut cipher = vec![0; plain.len() + TAG_LEN];
        let len = transport.write_message(&plain, &mut cipher).unwrap();
        cipher.truncate(len);
        inbound.send(cipher).await.unwrap();
    }

    #[tokio::test]
    async fn retired_peer_ids_are_rejected_and_stale_commands_cannot_touch_a_new_stream() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let key = crate::noise::StaticKeypair::generate();
        let id = crate::noise::daemon_id(&key.public);
        let (state, first) = crate::noise::initiate(&id, &key.public).unwrap();
        let (daemon, reply) = crate::noise::respond(&key, &id, &first).unwrap();
        let mut client = crate::noise::finish(state, &reply).unwrap();
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        let (accepted_tx, mut accepted) = mpsc::unbounded_channel();
        let accept: Accept = Arc::new(move |stream| {
            accepted_tx.send(stream).unwrap();
        });
        let session = spawn(daemon, Role::Daemon, out_tx, in_rx, Some(accept));
        inject(&mut client, &in_tx, OPEN, 1, &[]).await;
        let mut old = accepted.recv().await.unwrap();
        old.write_all(b"queued").await.unwrap();
        inject(&mut client, &in_tx, RST, 1, &[]).await;
        assert!(old.read_exact(&mut [0; 1]).await.is_err());
        inject(&mut client, &in_tx, OPEN, 1, &[]).await;
        let mut plain = [0; 64];
        loop {
            let cipher = tokio::time::timeout(std::time::Duration::from_secs(1), out_rx.recv())
                .await
                .unwrap()
                .unwrap();
            let len = client.read_message(&cipher, &mut plain).unwrap();
            if plain[..len] == encode(RST, 1, &[]) {
                break;
            }
        }
        assert!(accepted.try_recv().is_err());
        inject(&mut client, &in_tx, OPEN, 3, &[]).await;
        let mut next = accepted.recv().await.unwrap();
        session
            .tx
            .send(Cmd::Frame(DATA, 1, Bytes::from_static(b"stale")))
            .unwrap();
        session.tx.send(Cmd::Frame(FIN, 1, Bytes::new())).unwrap();
        session.tx.send(Cmd::Frame(RST, 1, Bytes::new())).unwrap();
        inject(&mut client, &in_tx, DATA, 3, b"live").await;
        let mut received = [0; 4];
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            next.read_exact(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&received, b"live");
        session.close();
    }
}
