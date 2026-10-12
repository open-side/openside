//! Relay, daemon host and device client against each other over real sockets.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::result_large_err,
    missing_docs
)]

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use openside_relay::{
    client::Tunnel,
    host::{self, HostConfig, HostState},
    mux::MuxStream,
    noise::{self, StaticKeypair},
    server::{self, ServerConfig},
    Error,
};
use parking_lot::Mutex;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinSet,
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const TOKEN: &str = "test-registration-token-0123456789";

async fn start_relay(config: ServerConfig) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server::router(config)).await.unwrap() });
    addr
}

fn relay_config() -> ServerConfig {
    ServerConfig {
        daemon_tokens: vec![TOKEN.into()],
        ..Default::default()
    }
}

/// WebSocket proxy between the daemon host and the relay that records every binary
/// message in both directions, i.e. everything the relay operator could see.
struct Tap {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    tasks: Arc<Mutex<JoinSet<()>>>,
    accept: tokio::task::JoinHandle<()>,
}

impl Tap {
    async fn start(bind: SocketAddr, upstream: SocketAddr, seen: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
        let listener = TcpListener::bind(bind).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tasks = Arc::new(Mutex::new(JoinSet::new()));
        let accept = {
            let tasks = tasks.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((tcp, _)) = listener.accept().await else {
                        return;
                    };
                    let seen = seen.clone();
                    tasks.lock().spawn(async move {
                        let request = Arc::new(Mutex::new(None));
                        let captured = request.clone();
                        let downstream = tokio_tungstenite::accept_hdr_async(
                            tcp,
                            move |req: &http::Request<()>, resp| {
                                *captured.lock() = Some((
                                    req.uri().path().to_owned(),
                                    req.headers().get("authorization").cloned(),
                                ));
                                Ok(resp)
                            },
                        )
                        .await
                        .unwrap();
                        let (path, auth) = request.lock().take().unwrap();
                        let mut upstream_request = format!("ws://{upstream}{path}")
                            .into_client_request()
                            .unwrap();
                        if let Some(auth) = auth {
                            upstream_request.headers_mut().insert("authorization", auth);
                        }
                        let (up, _) = tokio_tungstenite::connect_async(upstream_request)
                            .await
                            .unwrap();
                        let (mut down_tx, mut down_rx) = downstream.split();
                        let (mut up_tx, mut up_rx) = up.split();
                        let seen_up = seen.clone();
                        let a = async move {
                            while let Some(Ok(message)) = down_rx.next().await {
                                if let Message::Binary(bytes) = &message {
                                    seen_up.lock().push(bytes.to_vec());
                                }
                                if up_tx.send(message).await.is_err() {
                                    break;
                                }
                            }
                        };
                        let b = async move {
                            while let Some(Ok(message)) = up_rx.next().await {
                                if let Message::Binary(bytes) = &message {
                                    seen.lock().push(bytes.to_vec());
                                }
                                if down_tx.send(message).await.is_err() {
                                    break;
                                }
                            }
                        };
                        tokio::select! { _ = a => {}, _ = b => {} }
                    });
                }
            })
        };
        Tap {
            addr,
            seen,
            tasks,
            accept,
        }
    }

    fn kill(self) -> Arc<Mutex<Vec<Vec<u8>>>> {
        self.accept.abort();
        self.tasks.lock().abort_all();
        self.seen
    }
}

/// Daemon behaviour per stream, chosen by the first byte the device sends: `E` echoes
/// until EOF, `F` floods 2 MiB without reading.
fn daemon_accept() -> openside_relay::mux::Accept {
    Arc::new(|mut stream: MuxStream| {
        tokio::spawn(async move {
            let mut mode = [0_u8; 1];
            if stream.read_exact(&mut mode).await.is_err() {
                return;
            }
            match mode[0] {
                b'E' => {
                    let (mut r, mut w) = tokio::io::split(stream);
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                    let _ = w.shutdown().await;
                }
                b'F' => {
                    let chunk = vec![b'f'; 16_384];
                    for _ in 0..128 {
                        if stream.write_all(&chunk).await.is_err() {
                            return;
                        }
                    }
                }
                _ => {}
            }
        });
    })
}

async fn wait_for(mut check: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn echo(tunnel: &Tunnel, payload: Vec<u8>) -> Vec<u8> {
    let stream = tunnel.open().await.unwrap();
    let (mut r, mut w) = tokio::io::split(stream);
    let writer = tokio::spawn(async move {
        w.write_all(b"E").await.unwrap();
        w.write_all(&payload).await.unwrap();
        w.shutdown().await.unwrap();
        w
    });
    let mut back = Vec::new();
    r.read_to_end(&mut back).await.unwrap();
    drop(writer.await.unwrap());
    back
}

fn marker_payload(tag: usize) -> Vec<u8> {
    // Larger than one stream window, so the transfer only completes if credit flows.
    format!("PLAINTEXT-MARKER-{tag:04}|")
        .into_bytes()
        .repeat(30_000)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_devices_share_one_daemon_and_the_relay_sees_only_ciphertext() {
    let relay = start_relay(relay_config()).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tap = Tap::start("127.0.0.1:0".parse().unwrap(), relay, seen).await;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{}", tap.addr),
            token: TOKEN.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || host.status().state == HostState::Connected,
        "registration",
    )
    .await;

    let relay_url = format!("http://{relay}");
    let a = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    let b = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    let mut jobs = JoinSet::new();
    for (index, tunnel) in [a.clone(), a.clone(), b.clone(), b.clone()]
        .into_iter()
        .enumerate()
    {
        jobs.spawn(async move {
            let payload = marker_payload(index);
            assert_eq!(echo(&tunnel, payload.clone()).await, payload);
        });
    }
    while let Some(result) = jobs.join_next().await {
        result.unwrap();
    }
    wait_for(|| host.status().clients == 2, "two clients counted").await;

    let seen = tap.kill();
    let seen = seen.lock();
    let total: usize = seen.iter().map(Vec::len).sum();
    assert!(
        total > 4 * 30_000 * 20,
        "the tap saw the transfer ({total} bytes)"
    );
    for message in seen.iter() {
        assert!(
            !message.windows(16).any(|w| w == b"PLAINTEXT-MARKER"),
            "plaintext crossed the relay"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_wrong_token_unknown_daemon_and_mismatched_key() {
    let relay = start_relay(relay_config()).await;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: "not-the-registration-token".into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || {
            host.status()
                .last_error
                .is_some_and(|error| error.contains("rejected the registration token"))
        },
        "token rejection",
    )
    .await;
    assert_ne!(host.status().state, HostState::Connected);
    let relay_url = format!("http://{relay}");
    assert!(matches!(
        Tunnel::connect(&relay_url, &id, &keypair.public).await,
        Err(Error::DaemonOffline)
    ));
    let other = StaticKeypair::generate();
    assert!(matches!(
        Tunnel::connect(&relay_url, &id, &other.public).await,
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        Tunnel::connect("http://relay.example.test", &id, &keypair.public).await,
        Err(Error::InvalidUrl)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemon_reconnects_after_the_relay_link_drops() {
    let relay = start_relay(relay_config()).await;
    let tap = Tap::start("127.0.0.1:0".parse().unwrap(), relay, Arc::default()).await;
    let tap_addr = tap.addr;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{tap_addr}"),
            token: TOKEN.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || host.status().state == HostState::Connected,
        "registration",
    )
    .await;
    let relay_url = format!("http://{relay}");
    let tunnel = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    assert_eq!(echo(&tunnel, b"before".to_vec()).await, b"before");

    tap.kill();
    wait_for(|| host.status().state != HostState::Connected, "link loss").await;
    wait_for(|| tunnel.is_closed(), "device session to end").await;
    assert!(tunnel.open().await.is_err());

    let _tap = Tap::start(tap_addr, relay, Arc::default()).await;
    wait_for(
        || host.status().state == HostState::Connected,
        "re-registration",
    )
    .await;
    let tunnel = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    assert_eq!(echo(&tunnel, b"after".to_vec()).await, b"after");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_that_stops_reading_is_dropped_without_stalling_the_others() {
    let relay = start_relay(ServerConfig {
        client_buffer: 1 << 20,
        ..relay_config()
    })
    .await;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: TOKEN.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || host.status().state == HostState::Connected,
        "registration",
    )
    .await;
    let relay_url = format!("http://{relay}");

    // The slow device's network stops delivering (a phone that lost signal, a stalled
    // process): a proxy stops reading the relay's side after the floods start. Per-stream
    // windows let the daemon send 64 x 256 KiB, far more than the relay buffers for one
    // device plus the loopback socket buffers.
    let paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let proxy = pausable_proxy(relay, paused.clone()).await;
    let slow = Tunnel::connect(&format!("http://{proxy}"), &id, &keypair.public)
        .await
        .unwrap();
    let fast = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    wait_for(|| host.status().clients == 2, "both devices").await;
    paused.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut floods = Vec::new();
    for _ in 0..64 {
        let mut stream = slow.open().await.unwrap();
        stream.write_all(b"F").await.unwrap();
        floods.push(stream);
    }
    wait_for(|| host.status().clients == 1, "slow device to be dropped").await;
    for round in 0..5 {
        let payload = marker_payload(round);
        assert_eq!(echo(&fast, payload.clone()).await, payload);
    }
    drop(floods);
}

/// TCP proxy to `upstream` for one connection; while `paused`, it stops reading from the
/// upstream side, so the upstream's writes back up.
async fn pausable_proxy(
    upstream: SocketAddr,
    paused: Arc<std::sync::atomic::AtomicBool>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (client, _) = listener.accept().await.unwrap();
        let server = tokio::net::TcpStream::connect(upstream).await.unwrap();
        let (mut client_r, mut client_w) = client.into_split();
        let (mut server_r, mut server_w) = server.into_split();
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut client_r, &mut server_w).await;
        });
        let mut buf = vec![0_u8; 16_384];
        loop {
            while paused.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            match server_r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client_w.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_and_text_messages_close_the_device_and_the_client_cap_holds() {
    let relay = start_relay(ServerConfig {
        max_clients_per_daemon: 2,
        ..relay_config()
    })
    .await;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: TOKEN.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || host.status().state == HostState::Connected,
        "registration",
    )
    .await;

    async fn close_code(relay: SocketAddr, id: &str, message: Message) -> u16 {
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{relay}/v1/relay/connect/{id}"))
                .await
                .unwrap();
        socket.send(message).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Close(Some(frame)) = message {
                return frame.code.into();
            }
        }
        0
    }
    assert_eq!(
        close_code(relay, &id, Message::Binary(vec![0_u8; 70_000].into())).await,
        1009
    );
    assert_eq!(
        close_code(relay, &id, Message::Text("hi".into())).await,
        1003
    );

    let relay_url = format!("http://{relay}");
    let _one = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    let _two = Tunnel::connect(&relay_url, &id, &keypair.public)
        .await
        .unwrap();
    assert!(matches!(
        Tunnel::connect(&relay_url, &id, &keypair.public).await,
        Err(Error::TooManyClients)
    ));
}

#[tokio::test]
async fn the_relay_serves_the_web_app_with_the_daemon_security_headers() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<!doctype html>shell").unwrap();
    std::fs::create_dir(web.path().join("assets")).unwrap();
    std::fs::write(web.path().join("assets/app-1.js"), "console.log(1)").unwrap();
    let relay = start_relay(ServerConfig {
        web_dir: Some(web.path().to_owned()),
        ..relay_config()
    })
    .await;
    let get = |path: &'static str| async move {
        let mut tcp = tokio::net::TcpStream::connect(relay).await.unwrap();
        tcp.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let mut response = String::new();
        tcp.read_to_string(&mut response).await.unwrap();
        response.to_ascii_lowercase()
    };
    let pair = get("/pair").await;
    assert!(pair.starts_with("http/1.1 200"), "{pair}");
    assert!(pair.contains("content-security-policy: default-src 'self'; script-src 'self'"));
    assert!(pair.contains("x-frame-options: deny") && pair.ends_with("shell"));
    let asset = get("/assets/app-1.js").await;
    assert!(asset.contains("immutable") && asset.contains("text/javascript"));
    assert!(get("/assets/missing.js").await.starts_with("http/1.1 404"));
    assert!(get("/v1/sessions").await.starts_with("http/1.1 404"));
    assert!(get("/../etc/passwd").await.starts_with("http/1.1 4"));
    assert_eq!(get("/healthz").await.rsplit("\r\n\r\n").next(), Some("ok"));
    let unauth = get("/v1/relay/daemon/AAAAAAAAAAAAAAAAAAAAAA").await;
    assert!(unauth.starts_with("http/1.1 401"), "{unauth}");
}

#[tokio::test]
async fn registration_requires_key_proof_before_claiming_a_route_or_token_quota() {
    let relay = start_relay(relay_config()).await;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let mut request = format!("ws://{relay}/v1/relay/daemon/{id}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    let (mut unproven, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    unproven
        .send(Message::Binary(keypair.public.to_vec().into()))
        .await
        .unwrap();
    assert!(matches!(
        unproven.next().await,
        Some(Ok(Message::Binary(_)))
    ));
    unproven
        .send(Message::Binary(vec![0; 48].into()))
        .await
        .unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(2), unproven.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(Message::Binary(_)))));
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: TOKEN.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || host.status().state == HostState::Connected,
        "proved registration",
    )
    .await;
    let tunnel = Tunnel::connect(&format!("http://{relay}"), &id, &keypair.public)
        .await
        .unwrap();
    assert_eq!(
        echo(&tunnel, b"proof-policy".to_vec()).await,
        b"proof-policy"
    );
}

#[tokio::test]
async fn another_registration_token_cannot_displace_an_owned_route_and_each_token_has_one_id() {
    let second_token = "second-test-registration-token";
    let relay = start_relay(ServerConfig {
        daemon_tokens: vec![TOKEN.into(), second_token.into()],
        ..Default::default()
    })
    .await;
    let keypair = StaticKeypair::generate();
    let id = noise::daemon_id(&keypair.public);
    let host = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: TOKEN.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || host.status().state == HostState::Connected,
        "original registration",
    )
    .await;
    let displacement = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: second_token.into(),
            keypair: keypair.clone(),
        },
        daemon_accept(),
    );
    wait_for(
        || displacement.status().last_error.is_some(),
        "non-owner rejection",
    )
    .await;
    assert_ne!(displacement.status().state, HostState::Connected);
    let over_quota = host::spawn(
        HostConfig {
            relay_url: format!("http://{relay}"),
            token: TOKEN.into(),
            keypair: StaticKeypair::generate(),
        },
        daemon_accept(),
    );
    wait_for(
        || over_quota.status().last_error.is_some(),
        "registration quota rejection",
    )
    .await;
    assert_ne!(over_quota.status().state, HostState::Connected);
    assert_eq!(host.status().state, HostState::Connected);
    let tunnel = Tunnel::connect(&format!("http://{relay}"), &id, &keypair.public)
        .await
        .unwrap();
    assert_eq!(
        echo(&tunnel, b"original-route".to_vec()).await,
        b"original-route"
    );
}
