//! Remote transports (F-8): supervised `cloudflared`, relay registration, transport-aware
//! pairing links, and the relayed API keeping the loopback listener's authorization.

use std::{net::SocketAddr, path::Path, time::Duration};

use axum::{http::StatusCode, Router};
use http_body_util::BodyExt;
use openside_relay::{
    client::Tunnel,
    server::{self, ServerConfig},
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

use super::*;
use crate::remote::cloudflare;

const RELAY_TOKEN: &str = "relay-registration-token-0123456789";

async fn call(router: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn daemon_with(
    root: &Path,
    runtime: &TestRuntime,
    edit: impl FnOnce(&mut openside_config::Config),
) -> Daemon {
    let mut config = resolved_config(root);
    edit(&mut config.loaded.config);
    Daemon::open_runtime(config, runtime.clone()).await.unwrap()
}

fn script(root: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

fn read_pid(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[tokio::test]
async fn quick_tunnel_url_goes_into_pairing_and_a_crashed_cloudflared_is_restarted() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().display().to_string();
    // The first run crashes after announcing its URL; the second stays up like the real one.
    let bin = script(
        root.path(),
        "cloudflared",
        &format!(
            "echo \"$@\" >> '{dir}/args'\nn=$(wc -l < '{dir}/args' | tr -d ' ')\n\
             echo \"$$\" > '{dir}/pid'\n\
             echo \"INF |  https://fake-$n.trycloudflare.com  |\" >&2\n\
             if [ \"$n\" -lt 2 ]; then exit 3; fi\nexec sleep 30\n"
        ),
    );
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.remote.mode = vec![openside_config::RemoteMode::Cloudflare];
        config.remote.cloudflare.bin = bin.display().to_string();
    })
    .await;
    let listen: SocketAddr = "127.0.0.1:47999".parse().unwrap();
    crate::remote::start(&daemon.shared, Some(listen));
    let unix = api::router(daemon.shared.clone(), true);

    eventually("the restarted tunnel URL", || {
        let status = serde_json::to_value(crate::remote::snapshot(&daemon.shared)).unwrap();
        status["cloudflare"]["url"] == json!("https://fake-2.trycloudflare.com")
    })
    .await;
    let (status, body) = call(&unix, "GET", "/v1/remote/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["modes"], json!(["cloudflare"]));
    assert_eq!(body["cloudflare"]["state"], "running");
    assert_eq!(body["cloudflare"]["restarts"], 1);
    let args = std::fs::read_to_string(root.path().join("args")).unwrap();
    assert_eq!(
        args.lines().next().unwrap(),
        "tunnel --no-autoupdate --url http://127.0.0.1:47999"
    );

    let (status, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"phone","scopes":["read"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let code = offer["code"].as_str().unwrap();
    assert_eq!(offer["transport"], "cloudflare");
    assert_eq!(
        offer["url"],
        format!("https://fake-2.trycloudflare.com/pair#{code}")
    );
    // A transport that is not enabled is refused without issuing a code.
    let (status, body) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"phone","scopes":["read"],"via":"relay"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("transport_not_enabled"))
    );

    let pid = read_pid(&root.path().join("pid")).unwrap();
    assert!(alive(pid));
    daemon.shutdown().await;
    eventually("cloudflared to exit on shutdown", || !alive(pid)).await;
}

#[tokio::test]
async fn named_tunnel_gets_its_token_from_the_environment_and_reports_its_hostname() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().display().to_string();
    let bin = script(
        root.path(),
        "cloudflared",
        &format!(
            "echo \"$@\" > '{dir}/args'\nprintf '%s' \"$TUNNEL_TOKEN\" > '{dir}/env'\n\
             echo \"$$\" > '{dir}/pid'\n\
             echo 'INF Registered tunnel connection connIndex=0' >&2\nexec sleep 30\n"
        ),
    );
    let (_shutdown, receiver) = tokio::sync::broadcast::channel(1);
    let supervisor = cloudflare::spawn(
        cloudflare::Options {
            bin: bin.display().to_string(),
            listen: "127.0.0.1:47999".parse().unwrap(),
            hostname: Some("desk.example.test".into()),
            token: Some(zeroize::Zeroizing::new("named-tunnel-token-secret".into())),
            initial_backoff: Duration::from_millis(20),
            max_backoff: Duration::from_millis(100),
        },
        receiver,
    );
    eventually("the named tunnel to register", || {
        supervisor.status().state == "running"
    })
    .await;
    assert_eq!(
        supervisor.status().url.as_deref(),
        Some("https://desk.example.test")
    );
    let args = std::fs::read_to_string(root.path().join("args")).unwrap();
    assert_eq!(args.trim(), "tunnel --no-autoupdate run");
    assert!(!args.contains("named-tunnel-token-secret"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("env")).unwrap(),
        "named-tunnel-token-secret"
    );
    let pid = read_pid(&root.path().join("pid")).unwrap();
    supervisor.stop().await;
    eventually("cloudflared to be killed", || !alive(pid)).await;
}

#[test]
fn quick_url_ignores_other_cloudflare_addresses() {
    assert_eq!(
        cloudflare::quick_url("2024 INF |  https://a-b-c.trycloudflare.com  |").as_deref(),
        Some("https://a-b-c.trycloudflare.com")
    );
    assert_eq!(
        cloudflare::quick_url("see https://api.trycloudflare.com/tunnel"),
        None
    );
    assert_eq!(
        cloudflare::quick_url("https://developers.cloudflare.com/x"),
        None
    );
    assert_eq!(cloudflare::quick_url("no url"), None);
}

/// One HTTP/1.1 request over a relayed stream; `(status, body)`.
async fn relayed(
    tunnel: &Tunnel,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (u16, Value) {
    let mut stream = tunnel.open().await.unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: relay.test\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(token) = token {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status line in {text:?}"));
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
    (
        status,
        serde_json::from_str(body.trim()).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn relay_pairing_link_carries_the_daemon_key_and_relayed_calls_keep_tcp_authorization() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let relay_server = tokio::spawn(async move {
        let config = ServerConfig {
            daemon_tokens: vec![RELAY_TOKEN.into()],
            ..Default::default()
        };
        let _ = axum::serve(listener, server::router(config)).await;
    });
    let relay_url = format!("http://{relay_addr}");
    let token_file = root.path().join("relay-token");
    std::fs::write(&token_file, RELAY_TOKEN).unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.remote.mode = vec![openside_config::RemoteMode::Relay];
        config.remote.relay.url = Some(relay_url.clone());
        config.remote.relay.token_file = Some(token_file.clone());
    })
    .await;
    crate::remote::start(&daemon.shared, None);
    let unix = api::router(daemon.shared.clone(), true);
    eventually("the relay registration", || {
        serde_json::to_value(crate::remote::snapshot(&daemon.shared)).unwrap()["relay"]["state"]
            == "connected"
    })
    .await;

    // The key survives restarts: it is created once, private, next to the data.
    let key_file = daemon.shared.config.data_dir.join("relay-key");
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let (status, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"laptop","scopes":["read"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    assert_eq!(offer["transport"], "relay");
    let code = offer["code"].as_str().unwrap().to_owned();
    let url = offer["url"].as_str().unwrap();
    let fragment = url
        .strip_prefix(&format!("{relay_url}/pair#"))
        .unwrap_or_else(|| panic!("{url}"));
    let fields: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(fragment.as_bytes())
            .into_owned()
            .collect();
    assert_eq!(fields["c"], code);
    assert_eq!(fields["t"], "relay");
    let key = openside_relay::noise::decode_key(&fields["k"]).unwrap();
    assert_eq!(fields["d"], openside_relay::noise::daemon_id(&key));
    let (status, body) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"x","scopes":["read"],"via":"tailscale"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("transport_not_enabled"))
    );

    let tunnel = Tunnel::connect(&relay_url, &fields["d"], &key)
        .await
        .unwrap();
    // No token: the relayed API is the bearer-token API.
    let (status, _) = relayed(&tunnel, "GET", "/v1/sessions", None, "").await;
    assert_eq!(status, 401);
    let (status, paired) = relayed(
        &tunnel,
        "POST",
        "/v1/pairing/exchange",
        None,
        &json!({ "code": code }).to_string(),
    )
    .await;
    assert_eq!(status, 201, "{paired}");
    let token = paired["token"].as_str().unwrap().to_owned();
    let (status, sessions) = relayed(&tunnel, "GET", "/v1/sessions", Some(&token), "").await;
    assert_eq!(status, 200, "{sessions}");
    // Even an admin token cannot reach Unix-socket-only routes through the relay.
    let (_, admin) = daemon
        .shared
        .db
        .pair_named("admin".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (status, body) = relayed(
        &tunnel,
        "POST",
        "/v1/pairing",
        Some(&admin),
        &json!({"name":"x","scopes":["admin"]}).to_string(),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("local_access_required"))
    );
    let (status, remote) = relayed(&tunnel, "GET", "/v1/remote/status", Some(&admin), "").await;
    assert_eq!(status, 200);
    assert_eq!(remote["relay"]["clients"], 1);
    assert_eq!(remote["relay"]["daemon_id"], fields["d"]);

    // A wrong registration token is reported without revealing the configured one.
    std::fs::write(&token_file, "a-wrong-but-long-enough-token").unwrap();
    let link = crate::remote::relay::start_config(
        &daemon.shared,
        &daemon.shared.config.loaded.config.remote,
    )
    .unwrap();
    eventually("the rejected registration", || {
        link.status().last_error.is_some()
    })
    .await;
    let error = link.status().last_error.unwrap();
    assert!(error.contains("rejected the registration token"), "{error}");
    assert!(!error.contains("a-wrong-but-long-enough-token"));
    assert_eq!(link.daemon_id(), fields["d"], "the key file is reused");
    drop(link);

    daemon.shutdown().await;
    relay_server.abort();
}

#[tokio::test]
async fn relay_mode_without_a_running_link_refuses_relay_pairing_with_503() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.remote.mode = vec![openside_config::RemoteMode::Relay];
        config.remote.relay.url = Some("https://relay.example.test".into());
    })
    .await;
    let unix = api::router(daemon.shared.clone(), true);
    let (status, body) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"x","scopes":["read"]}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("transport_unavailable")
        )
    );
    // With no transport configured, the plain public URL keeps working as before.
    let plain = daemon_with(root.path().join("plain").as_path(), &runtime, |config| {
        config.remote.public_url = Some("https://desk.example.test/".into());
    })
    .await;
    let unix = api::router(plain.shared.clone(), true);
    let (status, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"x","scopes":["read"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(offer["transport"], Value::Null);
    assert_eq!(
        offer["url"],
        format!(
            "https://desk.example.test/pair#{}",
            offer["code"].as_str().unwrap()
        )
    );
}

#[tokio::test]
async fn tailscale_mode_keeps_the_existing_public_url_pairing_and_tcp_authorization() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.remote.mode = vec![openside_config::RemoteMode::Tailscale];
        config.remote.public_url = Some("https://desk.tail1234.ts.net/".into());
    })
    .await;
    crate::remote::start(&daemon.shared, None);
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (status, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"tailnet phone","scopes":["read"],"via":"tailscale"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    assert_eq!(offer["transport"], "tailscale");
    assert_eq!(
        offer["url"],
        format!(
            "https://desk.tail1234.ts.net/pair#{}",
            offer["code"].as_str().unwrap()
        )
    );
    let (status, paired) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":offer["code"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{paired}");
    let token = paired["token"].as_str().unwrap();
    let response = tcp
        .clone()
        .oneshot(request("GET", "/v1/sessions", Value::Null, Some(token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = tcp
        .oneshot(request("POST", "/v1/sessions", json!({}), Some(token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    daemon.shutdown().await;
}
