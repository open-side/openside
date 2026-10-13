use std::{
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{extract::ConnectInfo, http::StatusCode, Extension, Router};
use http_body_util::BodyExt;
use openside_proto::{CapabilityScope, DeviceKind};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

use super::*;
use crate::remote::{LocalAuthority, RelayOrigin, LOCAL_SECRET_HEADER};

async fn send(router: &Router, request: axum::extract::Request) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn local_request(
    method: &str,
    path: &str,
    body: Value,
    token: &str,
    secret: &str,
) -> axum::extract::Request {
    let mut request = request(method, path, body, Some(token));
    request
        .headers_mut()
        .insert("host", "127.0.0.1:47821".parse().unwrap());
    request
        .headers_mut()
        .insert(LOCAL_SECRET_HEADER, secret.parse().unwrap());
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50001".parse::<SocketAddr>().unwrap(),
    ));
    request
}

async fn bootstrap(unix: &Router) -> Value {
    let (status, access) = send(
        unix,
        request("GET", "/v1/remote/local-access", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(access["kind"], "person_device");
    assert_eq!(
        access["scopes"],
        json!(["read", "steer", "approve", "human_step", "admin"])
    );
    access
}

#[tokio::test]
async fn remote_setup_local_web_pairing_refuses_each_proxy_origin_and_remote_admin_provenance() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let access = bootstrap(&unix).await;
    let token = access["token"].as_str().unwrap();
    let secret = access["secret"].as_str().unwrap();
    let body = json!({"name":"phone","scopes":["read","approve","read"]});

    let (status, offer) = send(
        &tcp,
        local_request("POST", "/v1/pairing", body.clone(), token, secret),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(offer["kind"], "person_device");
    assert_eq!(offer["scopes"], json!(["read", "approve"]));
    assert_eq!(
        send(
            &tcp,
            local_request(
                "POST",
                "/v1/pairing",
                json!({"name":"agent","scopes":["admin"],"kind":"agent_client"}),
                token,
                secret
            )
        )
        .await
        .0,
        StatusCode::CREATED
    );
    for (header, value) in [
        ("tailscale-user-login", "person@example.test"),
        ("cf-connecting-ip", "203.0.113.4"),
        ("x-forwarded-for", "203.0.113.4"),
        ("forwarded", "for=203.0.113.4"),
        ("via", "1.1 proxy"),
    ] {
        let mut request = local_request("POST", "/v1/pairing", body.clone(), token, secret);
        request.headers_mut().insert(
            axum::http::HeaderName::from_static(header),
            value.parse().unwrap(),
        );
        let (status, refused) = send(&tcp, request).await;
        assert_eq!(
            (status, refused["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("local_access_required")),
            "{header}"
        );
    }
    let mut relay = local_request("POST", "/v1/pairing", body.clone(), token, secret);
    relay.extensions_mut().insert(RelayOrigin);
    assert_eq!(send(&tcp, relay).await.1["code"], "local_access_required");
    let mut remote_host = local_request("POST", "/v1/pairing", body.clone(), token, secret);
    remote_host
        .headers_mut()
        .insert("host", "desk.example.ts.net".parse().unwrap());
    assert_eq!(
        send(&tcp, remote_host).await.1["code"],
        "local_access_required"
    );
    let mut nonlocal_peer = local_request("POST", "/v1/pairing", body.clone(), token, secret);
    nonlocal_peer.extensions_mut().insert(ConnectInfo(
        "203.0.113.4:50001".parse::<SocketAddr>().unwrap(),
    ));
    assert_eq!(
        send(&tcp, nonlocal_peer).await.1["code"],
        "local_access_required"
    );
    assert_eq!(
        send(
            &tcp,
            local_request("POST", "/v1/pairing", body.clone(), token, &"0".repeat(64))
        )
        .await
        .1["code"],
        "local_access_required"
    );
    assert_eq!(
        send(
            &tcp,
            request("POST", "/v1/pairing", body.clone(), Some(token))
        )
        .await
        .1["code"],
        "local_access_required"
    );

    let (_, nonadmin) = daemon
        .shared
        .db
        .pair_local(
            "local read".into(),
            vec![CapabilityScope::Read],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    assert_eq!(
        send(
            &tcp,
            local_request("POST", "/v1/pairing", body.clone(), &nonadmin, secret)
        )
        .await
        .1["code"],
        "forbidden"
    );
    let (_, agent) = daemon
        .shared
        .db
        .pair_local(
            "local agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    assert_eq!(
        send(
            &tcp,
            local_request("POST", "/v1/pairing", body.clone(), &agent, secret)
        )
        .await
        .1["code"],
        "local_access_required"
    );

    let (_, admin_offer) = send(
        &unix,
        request(
            "POST",
            "/v1/pairing",
            json!({"name":"remote admin","scopes":["admin"]}),
            None,
        ),
    )
    .await;
    let (status, paired) = send(
        &tcp,
        request(
            "POST",
            "/v1/pairing/exchange",
            json!({"code":admin_offer["code"]}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let remote_admin = paired["token"].as_str().unwrap();
    assert_eq!(
        send(
            &tcp,
            local_request("POST", "/v1/pairing", body, remote_admin, secret)
        )
        .await
        .1["code"],
        "local_access_required"
    );
    for path in [
        "/v1/remote/tailscale",
        "/v1/remote/cloudflare",
        "/v1/remote/relay",
    ] {
        assert_eq!(
            send(
                &tcp,
                local_request("PUT", path, json!({}), remote_admin, secret)
            )
            .await
            .1["code"],
            "local_access_required"
        );
    }
    assert_eq!(
        send(
            &tcp,
            local_request("GET", "/v1/remote/local-access", Value::Null, token, secret)
        )
        .await
        .1["code"],
        "local_socket_required"
    );
    let audit = std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl")).unwrap();
    assert!(!audit.contains(secret));
    assert!(!audit.contains(token));
    assert!(!audit.contains(offer["code"].as_str().unwrap()));
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_setup_pairing_scope_ceiling_expiry_single_use_and_fixed_kind() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let access = bootstrap(&unix).await;
    let token = access["token"].as_str().unwrap();
    let secret = access["secret"].as_str().unwrap();
    // Isolate the delegation guard: route-level admin admission cannot bypass this check.
    let scoped = Router::new()
        .route("/pair", axum::routing::post(crate::pairing::create))
        .layer(Extension(api::Access(vec![CapabilityScope::Read])))
        .layer(Extension(LocalAuthority(true)))
        .with_state(daemon.shared.clone());
    assert_eq!(
        send(
            &scoped,
            request(
                "POST",
                "/pair",
                json!({"name":"phone","scopes":["read","admin"]}),
                None
            )
        )
        .await
        .1["code"],
        "scope_ceiling_exceeded"
    );
    let base = Instant::now();
    let offset = Arc::new(parking_lot::Mutex::new(Duration::ZERO));
    let clock = Arc::clone(&offset);
    daemon
        .shared
        .remote
        .set_clocks(move || base + *clock.lock(), chrono::Utc::now);
    let (_, offer) = send(
        &tcp,
        local_request(
            "POST",
            "/v1/pairing",
            json!({"name":"phone","scopes":["read"]}),
            token,
            secret,
        ),
    )
    .await;
    assert_eq!(
        send(
            &tcp,
            request(
                "POST",
                "/v1/pairing/exchange",
                json!({"code":offer["code"],"kind":"agent_client"}),
                None
            )
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    *offset.lock() = Duration::from_secs(179);
    let (status, paired) = send(
        &tcp,
        request(
            "POST",
            "/v1/pairing/exchange",
            json!({"code":offer["code"]}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(paired["scopes"], json!(["read"]));
    assert_eq!(paired["kind"], "person_device");
    assert_eq!(
        send(
            &tcp,
            request(
                "POST",
                "/v1/pairing/exchange",
                json!({"code":offer["code"]}),
                None
            )
        )
        .await
        .1["code"],
        "pairing_not_found"
    );
    let (_, expired) = send(
        &tcp,
        local_request(
            "POST",
            "/v1/pairing",
            json!({"name":"phone2","scopes":["read"]}),
            token,
            secret,
        ),
    )
    .await;
    *offset.lock() = Duration::from_secs(359);
    assert_eq!(
        send(
            &tcp,
            request(
                "POST",
                "/v1/pairing/exchange",
                json!({"code":expired["code"]}),
                None
            )
        )
        .await
        .1["code"],
        "pairing_expired"
    );
    assert_eq!(
        send(
            &tcp,
            request(
                "POST",
                "/v1/pairing/exchange",
                json!({"code":expired["code"]}),
                None
            )
        )
        .await
        .1["code"],
        "pairing_not_found"
    );
    daemon.shutdown().await;
}

fn fake_cloudflared(root: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("fake-cloudflared");
    std::fs::write(&bin, format!("#!/bin/sh\necho $$ > '{}/cloudflared.pid'\necho 'https://remote-test.trycloudflare.com' >&2\nexec sleep 60\n", root.display())).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    bin
}

async fn persist_fixture(daemon: &Daemon) {
    assert!(crate::settings_api::persist(
        &daemon.shared,
        serde_json::to_value(&daemon.shared.config.loaded.config).unwrap()
    )
    .await
    .is_ok());
}

async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("remote transport did not reach expected state");
}

#[tokio::test]
async fn remote_setup_cloudflare_and_relay_start_stop_live_idempotently_and_report_failures() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let relay_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url = format!("http://{}", relay_listener.local_addr().unwrap());
    let relay_server = tokio::spawn(async move {
        axum::serve(
            relay_listener,
            openside_relay::server::router(openside_relay::server::ServerConfig {
                daemon_tokens: vec!["remote-registration-test-token-12345".into()],
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    });
    let token_file = root.path().join("registration-token");
    std::fs::write(&token_file, "remote-registration-test-token-12345").unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.remote.cloudflare.bin =
        fake_cloudflared(root.path()).display().to_string();
    config.loaded.config.daemon.listen = Some("127.0.0.1:47821".parse().unwrap());
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    persist_fixture(&daemon).await;
    crate::remote::start(&daemon.shared, Some("127.0.0.1:47821".parse().unwrap()));
    let unix = api::router(daemon.shared.clone(), true);
    let cloudflare = json!({"enabled":true,"tunnel":"quick","hostname":null,"token_file":null});
    assert_eq!(
        send(
            &unix,
            request("PUT", "/v1/remote/cloudflare", cloudflare.clone(), None)
        )
        .await
        .0,
        StatusCode::OK
    );
    eventually(|| {
        crate::remote::snapshot(&daemon.shared)
            .cloudflare
            .is_some_and(|status| status.state == "running")
    })
    .await;
    let pid = std::fs::read_to_string(root.path().join("cloudflared.pid")).unwrap();
    let (_, status) = send(
        &unix,
        request("PUT", "/v1/remote/cloudflare", cloudflare, None),
    )
    .await;
    assert_eq!(
        status["cloudflare"]["url"],
        "https://remote-test.trycloudflare.com"
    );
    assert_eq!(status["restart_required"], false);
    assert_eq!(
        std::fs::read_to_string(root.path().join("cloudflared.pid")).unwrap(),
        pid
    );
    let relay =
        json!({"enabled":true,"url":relay_url,"token_file":token_file.display().to_string()});
    assert_eq!(
        send(
            &unix,
            request("PUT", "/v1/remote/relay", relay.clone(), None)
        )
        .await
        .0,
        StatusCode::OK
    );
    eventually(|| {
        crate::remote::snapshot(&daemon.shared)
            .relay
            .is_some_and(|status| status.state == "connected")
    })
    .await;
    let (_, status) = send(
        &unix,
        request("PUT", "/v1/remote/relay", relay.clone(), None),
    )
    .await;
    assert_eq!(status["relay"]["state"], "connected");
    assert_eq!(status["relay"]["e2ee"], true);
    assert_eq!(status["modes"], json!(["cloudflare", "relay"]));
    let (_, offer) = send(
        &unix,
        request(
            "POST",
            "/v1/pairing",
            json!({"name":"phone","scopes":["read"],"via":"relay"}),
            None,
        ),
    )
    .await;
    assert!(offer["url"].as_str().unwrap().contains("&t=relay&d="));
    assert_eq!(
        send(
            &unix,
            request(
                "PUT",
                "/v1/remote/cloudflare",
                json!({"enabled":false,"tunnel":"quick","hostname":null,"token_file":null}),
                None
            )
        )
        .await
        .1["cloudflare"],
        Value::Null
    );
    assert!(nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid.trim().parse().unwrap()),
        None
    )
    .is_err());
    let mut off = relay;
    off["enabled"] = json!(false);
    assert_eq!(
        send(&unix, request("PUT", "/v1/remote/relay", off, None))
            .await
            .1["relay"],
        Value::Null
    );
    assert_eq!(send(&unix, request("PUT", "/v1/remote/relay", json!({"enabled":true,"url":relay_url,"token_file":root.path().join("missing-token").display().to_string()}), None)).await.1["relay"]["state"], "failed");
    assert_eq!(
        send(
            &unix,
            request(
                "PUT",
                "/v1/remote/cloudflare",
                json!({"enabled":true,"tunnel":"named","hostname":null,"token_file":null}),
                None
            )
        )
        .await
        .1["code"],
        "invalid_config"
    );
    daemon.shutdown().await;
    relay_server.abort();
    let _ = relay_server.await;
}

#[tokio::test]
async fn remote_setup_missing_listener_reports_restart_required() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let (status, remote) = send(
        &unix,
        request(
            "PUT",
            "/v1/remote/cloudflare",
            json!({"enabled":true,"tunnel":"quick","hostname":null,"token_file":null}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(remote["restart_required"], true);
    assert_eq!(remote["configuration"]["cloudflare"]["enabled"], true);
    assert_eq!(remote["cloudflare"], Value::Null);
    daemon.shutdown().await;
}

async fn wire(
    mut io: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    method: &str,
    path: &str,
    body: Value,
    headers: &str,
) -> (u16, Value) {
    let body = body.to_string();
    io.write_all(format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), io.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap(),
    )
}

#[tokio::test]
async fn remote_setup_temp_daemon_socket_bootstrap_to_loopback_pairing_smoke() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix_path = root.path().join("daemon.sock");
    let unix_listener = tokio::net::UnixListener::bind(&unix_path).unwrap();
    std::fs::set_permissions(&unix_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp_listener.local_addr().unwrap();
    crate::remote::start(&daemon.shared, Some(addr));
    let unix_router = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let unix_task = tokio::spawn(async move {
        axum::serve(unix_listener, unix_router).await.unwrap();
    });
    let tcp_task = tokio::spawn(async move {
        axum::serve(
            tcp_listener,
            tcp_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let (status, access) = wire(
        tokio::net::UnixStream::connect(&unix_path).await.unwrap(),
        "GET",
        "/v1/remote/local-access",
        Value::Null,
        "",
    )
    .await;
    assert_eq!(status, 200);
    assert!(access["url"]
        .as_str()
        .unwrap()
        .starts_with(&format!("http://{addr}/#local_secret=")));
    let headers = format!(
        "Authorization: Bearer {}\r\nX-Openside-Local-Secret: {}\r\n",
        access["token"].as_str().unwrap(),
        access["secret"].as_str().unwrap()
    );
    let (status, offer) = wire(
        tokio::net::TcpStream::connect(addr).await.unwrap(),
        "POST",
        "/v1/pairing",
        json!({"name":"phone smoke","scopes":["read"]}),
        &headers,
    )
    .await;
    assert_eq!(status, 201);
    let (status, paired) = wire(
        tokio::net::TcpStream::connect(addr).await.unwrap(),
        "POST",
        "/v1/pairing/exchange",
        json!({"code":offer["code"]}),
        "",
    )
    .await;
    assert_eq!(status, 201);
    let remote_headers = format!(
        "Authorization: Bearer {}\r\n",
        paired["token"].as_str().unwrap()
    );
    assert_eq!(
        wire(
            tokio::net::TcpStream::connect(addr).await.unwrap(),
            "GET",
            "/v1/sessions",
            Value::Null,
            &remote_headers
        )
        .await
        .0,
        200
    );
    assert_eq!(
        wire(
            tokio::net::TcpStream::connect(addr).await.unwrap(),
            "POST",
            "/v1/pairing",
            json!({"name":"not local","scopes":["read"]}),
            &remote_headers
        )
        .await
        .0,
        403
    );
    unix_task.abort();
    tcp_task.abort();
    let _ = unix_task.await;
    let _ = tcp_task.await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_setup_poll_replays_gapless_pages_filters_wakes_and_delivers_tombstone() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "poll", PolicyMode::Guard, None).await;
    for n in 0..520 {
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::AssistantDelta {
                    text: n.to_string(),
                },
            )
            .await
            .unwrap();
    }
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (device, token) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let path = format!("/v1/sessions/{session}/events/poll");
    let (status, first) = send(
        &tcp,
        request(
            "GET",
            &format!("{path}?after=0&wait=0"),
            Value::Null,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["events"].as_array().unwrap().len(), 512);
    assert_eq!(first["last_seq"], 512);
    let (_, second) = send(
        &tcp,
        request(
            "GET",
            &format!("{path}?after=512&wait=0"),
            Value::Null,
            Some(&token),
        ),
    )
    .await;
    let joined = first["events"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["events"].as_array().unwrap());
    for (index, event) in joined.enumerate() {
        assert_eq!(event["seq"], json!(index as u64 + 1));
    }
    let cursor = second["last_seq"].as_u64().unwrap();
    let child = daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::SubagentProgress {
                subagent_id: "child".into(),
                progress: json!({"text":"filtered"}),
            },
        )
        .await
        .unwrap();
    let (_, filtered) = send(
        &tcp,
        request(
            "GET",
            &format!("{path}?after={cursor}&wait=0&subagents=false"),
            Value::Null,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(filtered["events"], json!([]));
    assert_eq!(filtered["last_seq"], child.seq);
    let cursor = child.seq;
    assert_eq!(
        send(
            &tcp,
            request(
                "GET",
                &format!("{path}?after={}&wait=0", cursor + 1),
                Value::Null,
                Some(&token)
            )
        )
        .await
        .1["code"],
        "invalid_event_id"
    );
    assert_eq!(
        send(
            &tcp,
            request("GET", &format!("{path}?wait=26"), Value::Null, Some(&token))
        )
        .await
        .1["code"],
        "invalid_value"
    );
    let (_, empty) = send(
        &tcp,
        request(
            "GET",
            &format!("{path}?after={cursor}&wait=0"),
            Value::Null,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(empty["events"], json!([]));
    assert_eq!(empty["last_seq"], cursor);

    let mut response = tcp
        .clone()
        .oneshot(request(
            "GET",
            &format!("/v1/sessions/{session}/events"),
            Value::Null,
            Some(&token),
        ))
        .await
        .unwrap()
        .into_body();
    let first_frame = tokio::time::timeout(Duration::from_secs(1), response.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert!(std::str::from_utf8(&first_frame)
        .unwrap()
        .starts_with(": ok\n\n"));
    drop(response);

    let poll = tcp.clone();
    let waiting = request(
        "GET",
        &format!("{path}?after={cursor}&wait=25"),
        Value::Null,
        Some(&token),
    );
    let waiter = tokio::spawn(async move { send(&poll, waiting).await });
    let live = daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::AssistantDelta {
                text: "live".into(),
            },
        )
        .await
        .unwrap();
    let (status, page) = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["last_seq"], live.seq);
    let poll = tcp.clone();
    let waiting = request(
        "GET",
        &format!("{path}?after={}&wait=25", live.seq),
        Value::Null,
        Some(&token),
    );
    let waiter = tokio::spawn(async move { send(&poll, waiting).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(crate::pairing::revoke(&daemon.shared, &device)
        .await
        .is_ok());
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap()
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &unix,
            request(
                "DELETE",
                &format!("/v1/sessions/{session}"),
                Value::Null,
                None
            )
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, deleted) = send(
        &unix,
        request(
            "GET",
            &format!("{path}?after={}&wait=0", live.seq),
            Value::Null,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        deleted["events"].as_array().unwrap().last().unwrap()["kind"],
        "session.deleted"
    );
    let after = deleted["last_seq"].as_u64().unwrap();
    assert_eq!(
        send(
            &unix,
            request(
                "GET",
                &format!("{path}?after={after}&wait=0"),
                Value::Null,
                None
            )
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_setup_local_boot_secret_rotates_and_revoked_bootstrap_device_is_replaced() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let (intentional_id, intentional_token) = daemon
        .shared
        .db
        .pair_local(
            "openside-browser".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let first = bootstrap(&unix).await;
    let again = bootstrap(&unix).await;
    assert_ne!(first["device_id"], again["device_id"]);
    assert_ne!(first["token"], again["token"]);
    assert_eq!(first["secret"], again["secret"]);
    assert!(daemon
        .shared
        .db
        .device(first["token"].as_str().unwrap().to_owned())
        .await
        .unwrap()
        .is_none());
    assert!(daemon
        .shared
        .remote
        .is_revoked(first["device_id"].as_str().unwrap()));
    assert!(daemon
        .shared
        .db
        .device(intentional_token.clone())
        .await
        .unwrap()
        .is_some());
    assert_eq!(daemon.shared.db.devices().await.unwrap().len(), 2);
    let device = again["device_id"].as_str().unwrap();
    assert!(crate::pairing::revoke(&daemon.shared, device).await.is_ok());
    let replacement = bootstrap(&unix).await;
    assert_ne!(again["device_id"], replacement["device_id"]);
    assert_eq!(again["secret"], replacement["secret"]);
    let old_token = replacement["token"].as_str().unwrap().to_owned();
    let old_secret = replacement["secret"].as_str().unwrap().to_owned();
    let old_id = replacement["device_id"].as_str().unwrap().to_owned();
    daemon.shutdown().await;
    drop(unix);
    drop(daemon);

    let restarted = super::daemon(root.path(), &runtime).await;
    let unix = api::router(restarted.shared.clone(), true);
    let current = bootstrap(&unix).await;
    assert_ne!(current["secret"], old_secret);
    assert_ne!(current["device_id"], old_id);
    assert!(restarted
        .shared
        .db
        .device(old_token.clone())
        .await
        .unwrap()
        .is_none());
    assert!(restarted.shared.remote.is_revoked(&old_id));
    let devices = restarted.shared.db.devices().await.unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices
        .iter()
        .any(|device| device.device_id == intentional_id));
    assert!(restarted
        .shared
        .db
        .device(intentional_token)
        .await
        .unwrap()
        .is_some());
    let request = local_request("POST", "/v1/pairing", json!({}), &old_token, &old_secret);
    assert!(!crate::remote::local_tcp(&restarted.shared, &request));
    let audit =
        std::fs::read_to_string(restarted.shared.config.data_dir.join("audit.jsonl")).unwrap();
    assert!(audit.contains("bootstrap_rotation"));
    assert!(!audit.contains(&old_token));
    restarted.shutdown().await;
}

#[tokio::test]
async fn remote_setup_revocation_cancels_a_paused_event_page_read_without_releasing_data() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "read-race", PolicyMode::Guard, None).await;
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::AssistantDelta {
                text: "revoked-page-canary".into(),
            },
        )
        .await
        .unwrap();
    let (device, token) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let pause = daemon.shared.db.pause_after_events(session.as_str());
    let router = api::router(daemon.shared.clone(), false);
    let request = request(
        "GET",
        &format!("/v1/sessions/{session}/events/poll?after=0&wait=25"),
        Value::Null,
        Some(&token),
    );
    let poll = tokio::spawn(async move { send(&router, request).await });
    tokio::time::timeout(Duration::from_secs(2), pause.reached())
        .await
        .unwrap();
    let shared = daemon.shared.clone();
    let revoke = tokio::spawn(async move { crate::pairing::revoke(&shared, &device).await });
    let (status, body) = tokio::time::timeout(Duration::from_secs(2), poll)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
    assert!(!body.to_string().contains("revoked-page-canary"));
    assert!(tokio::time::timeout(Duration::from_secs(2), revoke)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    // The blocked read stays paused until cancellation and revoke have both completed.
    pause.release();
    daemon.shutdown().await;
}
