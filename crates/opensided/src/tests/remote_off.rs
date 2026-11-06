//! Remote Control off (`remote.enabled = false`): only this computer reaches the daemon,
//! transports stop with their settings kept, and nothing advertises a way in.

use std::{net::SocketAddr, path::Path};

use axum::{extract::ConnectInfo, Router};
use openside_config::RemoteMode;
use openside_proto::{CapabilityScope, DeviceKind};

use super::*;
use crate::remote::{RelayOrigin, LOCAL_SECRET_HEADER};

const LISTEN: &str = "127.0.0.1:47821";
const SERVE: &str = "https://desk.example.ts.net";
/// A Tailscale setup saved before Remote Control had its own switch.
const TAILSCALE: &str = "[daemon]\nlisten = \"127.0.0.1:47821\"\n\n[remote]\nmode = \"tailscale\"\npublic_url = \"https://desk.example.ts.net\"\n";
/// Remote Control switched on with no transport, as after the person turned it on once.
const ON: &str = "[remote]\nenabled = true\n";

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `request` as it reaches the TCP listener from `peer`, naming `host`.
fn tcp(peer: &str, host: &str, mut request: Request<Body>) -> Request<Body> {
    request.headers_mut().insert("host", host.parse().unwrap());
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    request
}

fn health(token: &str) -> Request<Body> {
    request("GET", "/v1/health", Value::Null, Some(token))
}

/// A daemon whose config file holds `text`, loaded as it would be at startup: the settings API
/// rewrites that file, so it has to agree with what the daemon runs.
async fn open(root: &Path, text: &str, edit: impl FnOnce(&mut openside_config::Config)) -> Daemon {
    let mut config = resolved_config(root);
    let file = config.loaded.paths.config_file.clone();
    let parsed = openside_config::parse_document(&file, text).unwrap();
    config.loaded.config.daemon.listen = parsed.daemon.listen;
    config.loaded.config.remote = parsed.remote;
    edit(&mut config.loaded.config);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap()
}

/// A paired phone's admin token: what a remote device holds.
async fn phone(daemon: &Daemon) -> String {
    daemon
        .shared
        .db
        .pair_local(
            "phone".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap()
        .1
}

/// A `tailscale` whose Serve status is `status` and whose every other command exits with `code`.
fn fake_tailscale(root: &Path, status: &str, code: u8) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("tailscale");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}/calls'\nif [ \"$1 $2\" = \"serve status\" ]; then printf '%s\\n' '{status}'; exit 0; fi\nexit {code}\n",
            root.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    bin
}

/// Serve status with openside's root handler on port 443 proxying to `LISTEN`.
fn serving() -> String {
    format!(
        r#"{{"TCP":{{"443":{{"HTTPS":true}}}},"Web":{{"desk.example.ts.net:443":{{"Handlers":{{"/":{{"Proxy":"http://{LISTEN}"}}}}}}}}}}"#
    )
}

async fn disable(unix: &Router) -> Value {
    let (status, body) = send(
        unix,
        request("POST", "/v1/remote/disable", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"]["enabled"], false);
    body
}

fn refused(answer: (StatusCode, Value)) -> bool {
    answer.0 == StatusCode::FORBIDDEN && answer.1["code"] == "remote_disabled"
}

#[tokio::test]
async fn remote_off_refuses_remote_tcp_requests() {
    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), ON, |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let token = phone(&daemon).await;
    let remote = || tcp("203.0.113.4:50001", "desk.example.ts.net", health(&token));
    assert_eq!(send(&tcp_router, remote()).await.0, StatusCode::OK);

    let body = disable(&unix).await;
    assert_eq!(body["still_on"], json!([]));
    assert!(refused(send(&tcp_router, remote()).await));
    // A LAN peer naming the listener's own address is still another computer.
    assert!(refused(
        send(&tcp_router, tcp("192.0.2.7:50001", LISTEN, health(&token))).await
    ));
    // Public routes need no token, so they are refused before one would be looked at.
    let exchange = request(
        "POST",
        "/v1/pairing/exchange",
        json!({"code":"anything"}),
        None,
    );
    assert!(refused(
        send(
            &tcp_router,
            tcp("203.0.113.4:50001", "desk.example.ts.net", exchange)
        )
        .await
    ));
    let mut relayed = tcp("127.0.0.1:50001", LISTEN, health(&token));
    relayed.extensions_mut().insert(RelayOrigin);
    assert!(refused(send(&tcp_router, relayed).await));
    let audit = std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl")).unwrap();
    assert!(audit.contains("\"disabled\""), "{audit}");
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_off_refuses_proxied_loopback_requests() {
    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), ON, |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let token = phone(&daemon).await;
    disable(&unix).await;
    // Tailscale Serve and cloudflared connect from loopback; their headers give them away.
    for (header, value) in [
        ("tailscale-user-login", "person@example.test"),
        ("cf-connecting-ip", "203.0.113.4"),
        ("x-forwarded-for", "203.0.113.4"),
        ("x-forwarded-host", "desk.example.ts.net"),
        ("forwarded", "for=203.0.113.4"),
        ("via", "1.1 proxy"),
    ] {
        let mut proxied = tcp("127.0.0.1:50001", LISTEN, health(&token));
        proxied.headers_mut().insert(
            axum::http::HeaderName::from_static(header),
            value.parse().unwrap(),
        );
        assert!(refused(send(&tcp_router, proxied).await), "{header}");
    }
    // A proxy that keeps the visitor's host name.
    assert!(refused(
        send(
            &tcp_router,
            tcp("127.0.0.1:50001", "desk.example.ts.net", health(&token))
        )
        .await
    ));
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_off_keeps_the_local_web_ui_and_the_socket() {
    let root = tempfile::tempdir().unwrap();
    let ui = root.path().join("ui");
    std::fs::create_dir_all(&ui).unwrap();
    std::fs::write(
        ui.join("index.html"),
        "<!doctype html><title>openside</title>",
    )
    .unwrap();
    let daemon = open(root.path(), ON, |config| config.ui.dir = Some(ui.clone())).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let token = phone(&daemon).await;
    disable(&unix).await;

    for host in [LISTEN, "localhost:47821", "[::1]:47821"] {
        assert_eq!(
            send(&tcp_router, tcp("127.0.0.1:50001", host, health(&token)))
                .await
                .0,
            StatusCode::OK,
            "{host}"
        );
    }
    let page = |peer: &str| {
        tcp(
            peer,
            LISTEN,
            Request::builder().uri("/").body(Body::empty()).unwrap(),
        )
    };
    assert_eq!(
        tcp_router
            .clone()
            .oneshot(page("127.0.0.1:50001"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // The web UI itself is behind the switch too.
    assert!(refused(send(&tcp_router, page("203.0.113.4:50001")).await));
    assert_eq!(
        send(&unix, request("GET", "/v1/health", Value::Null, None))
            .await
            .0,
        StatusCode::OK
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_on_again_restores_remote_access() {
    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), ON, |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let token = phone(&daemon).await;
    let remote = || tcp("203.0.113.4:50001", "desk.example.ts.net", health(&token));
    disable(&unix).await;
    assert!(refused(send(&tcp_router, remote()).await));
    let stored = crate::settings_api::stored(&daemon.shared).await.unwrap();
    assert_eq!(stored.remote.enabled, Some(false));

    let (status, body) = send(
        &unix,
        request("POST", "/v1/remote/enable", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["restart_required"], false);
    assert_eq!(send(&tcp_router, remote()).await.0, StatusCode::OK);
    assert_eq!(
        crate::settings_api::stored(&daemon.shared)
            .await
            .unwrap()
            .remote
            .enabled,
        Some(true)
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_off_lists_a_transport_it_could_not_stop() {
    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), TAILSCALE, |_| {}).await;
    crate::remote::start(&daemon.shared, Some(LISTEN.parse().unwrap()));
    // Serve reports openside's handler, but turning it off fails.
    *daemon.shared.transports.tailscale_bin.lock() =
        Some(fake_tailscale(root.path(), &serving(), 1));
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let token = phone(&daemon).await;

    let body = disable(&unix).await;
    assert_eq!(body["still_on"], json!(["tailscale"]));
    let calls = std::fs::read_to_string(root.path().join("calls")).unwrap();
    assert!(
        calls.contains("serve --https=443 --set-path=/ off"),
        "{calls}"
    );
    // Whatever Serve still forwards is refused, and the setting is saved anyway.
    let mut proxied = tcp("127.0.0.1:50001", "desk.example.ts.net", health(&token));
    proxied
        .headers_mut()
        .insert("x-forwarded-for", "203.0.113.4".parse().unwrap());
    assert!(refused(send(&tcp_router, proxied).await));
    let stored = crate::settings_api::stored(&daemon.shared).await.unwrap();
    assert_eq!(stored.remote.enabled, Some(false));
    assert_eq!(stored.remote.mode, vec![RemoteMode::Tailscale]);
    assert_eq!(stored.remote.public_url.as_deref(), Some(SERVE));
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_enabled_defaults_on_for_a_config_written_before_the_switch() {
    let parsed = openside_config::parse_document(Path::new("config.toml"), TAILSCALE).unwrap();
    assert_eq!(parsed.remote.enabled, None);
    assert!(parsed.remote.is_enabled());
    assert_eq!(parsed.remote.mode, vec![RemoteMode::Tailscale]);
    let url_only = "[remote]\npublic_url = \"https://desk.example.ts.net\"\n";
    let parsed = openside_config::parse_document(Path::new("config.toml"), url_only).unwrap();
    assert!(parsed.remote.is_enabled());

    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), TAILSCALE, |_| {}).await;
    assert!(crate::remote::enabled(&daemon.shared));
    let unix = api::router(daemon.shared.clone(), true);
    let (status, body) = send(
        &unix,
        request("GET", "/v1/remote/status", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["modes"], json!(["tailscale"]));
    assert_eq!(body["public_url"], SERVE);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_fresh_install_starts_with_remote_control_off_until_the_person_turns_it_on() {
    let parsed = openside_config::parse_document(Path::new("config.toml"), "").unwrap();
    assert_eq!(parsed.remote.enabled, None);
    assert!(!parsed.remote.is_enabled());

    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), "", |_| {}).await;
    assert!(!crate::remote::enabled(&daemon.shared));
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let token = phone(&daemon).await;
    let remote = || tcp("203.0.113.4:50001", "desk.example.ts.net", health(&token));
    let (status, body) = send(
        &unix,
        request("GET", "/v1/remote/status", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    assert!(refused(send(&tcp_router, remote()).await));

    let (status, body) = send(
        &unix,
        request("POST", "/v1/remote/enable", Value::Null, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(send(&tcp_router, remote()).await.0, StatusCode::OK);
    // The switch is written, so it stays on with no transport or public_url configured.
    let stored = crate::settings_api::stored(&daemon.shared).await.unwrap();
    assert_eq!(stored.remote.enabled, Some(true));
    assert!(stored.remote.mode.is_empty() && stored.remote.public_url.is_none());
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_switch_needs_the_person_at_this_computer() {
    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), ON, |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp_router = api::router(daemon.shared.clone(), false);
    let (_, access) = send(
        &unix,
        request("GET", "/v1/remote/local-access", Value::Null, None),
    )
    .await;
    let launcher = access["token"].as_str().unwrap().to_owned();
    let secret = access["secret"].as_str().unwrap().to_owned();
    let remote_admin = phone(&daemon).await;
    let (_, agent) = daemon
        .shared
        .db
        .pair_local(
            "agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let local = |path: &str, token: &str| {
        let mut request = tcp(
            "127.0.0.1:50001",
            LISTEN,
            request("POST", path, Value::Null, Some(token)),
        );
        request
            .headers_mut()
            .insert(LOCAL_SECRET_HEADER, secret.parse().unwrap());
        request
    };

    for path in ["/v1/remote/disable", "/v1/remote/enable"] {
        // An agent-client token is never this computer's person, even with the secret.
        let (status, body) = send(&tcp_router, local(path, &agent)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("local_access_required")),
            "{path}"
        );
        // A phone's admin token, from another computer.
        let (status, body) = send(
            &tcp_router,
            tcp(
                "203.0.113.4:50001",
                "desk.example.ts.net",
                request("POST", path, Value::Null, Some(&remote_admin)),
            ),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("local_access_required")),
            "{path}"
        );
        // An MCP client has local access but is not the person, on the socket or the launcher.
        let mut marked = request("POST", path, Value::Null, None);
        marked
            .headers_mut()
            .insert("x-openside-client", "mcp".parse().unwrap());
        let (status, body) = send(&unix, marked).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("person_access_required")),
            "{path}"
        );
        let mut marked = local(path, &launcher);
        marked
            .headers_mut()
            .insert("x-openside-client", "mcp".parse().unwrap());
        let (status, body) = send(&tcp_router, marked).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("person_access_required")),
            "{path}"
        );
    }
    assert!(crate::remote::enabled(&daemon.shared));

    // Positive control: the launcher on this computer turns it off and on.
    let (status, body) = send(&tcp_router, local("/v1/remote/disable", &launcher)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!crate::remote::enabled(&daemon.shared));
    let (status, body) = send(&tcp_router, local("/v1/remote/enable", &launcher)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(crate::remote::enabled(&daemon.shared));
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_off_pairing_hides_the_saved_public_url() {
    let root = tempfile::tempdir().unwrap();
    let daemon = open(root.path(), TAILSCALE, |_| {}).await;
    // Serve has nothing on the port, so turning it off is a no-op that succeeds.
    *daemon.shared.transports.tailscale_bin.lock() =
        Some(fake_tailscale(root.path(), r#"{"TCP":{},"Web":{}}"#, 0));
    let unix = api::router(daemon.shared.clone(), true);
    let pair = |via: Value| {
        request(
            "POST",
            "/v1/pairing",
            json!({"name":"phone","scopes":["read"],"via":via}),
            None,
        )
    };
    let (status, offer) = send(&unix, pair(Value::Null)).await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    assert!(offer["url"].as_str().unwrap().starts_with(SERVE), "{offer}");
    assert_eq!(
        crate::remote::public_base(&daemon.shared).as_deref(),
        Some(SERVE)
    );

    let body = disable(&unix).await;
    assert_eq!(body["still_on"], json!([]));
    assert_eq!(body["status"]["modes"], json!([]));
    assert_eq!(body["status"]["public_url"], Value::Null);
    // The saved settings stay for turning Remote Control back on.
    assert_eq!(
        body["status"]["configuration"]["tailscale"]["public_url"],
        SERVE
    );
    assert_eq!(
        body["status"]["configuration"]["tailscale"]["enabled"],
        true
    );
    let (status, offer) = send(&unix, pair(Value::Null)).await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    assert_eq!(offer["url"], Value::Null, "{offer}");
    assert_eq!(offer["transport"], Value::Null, "{offer}");
    let (status, answer) = send(&unix, pair(json!("tailscale"))).await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (StatusCode::CONFLICT, Some("remote_disabled"))
    );
    assert_eq!(crate::remote::public_base(&daemon.shared), None);
    // Turning a transport on behind the switch is refused too.
    let (status, answer) = send(
        &unix,
        request("PUT", "/v1/remote/tailscale", json!({"enabled":true}), None),
    )
    .await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (StatusCode::CONFLICT, Some("remote_disabled"))
    );
    daemon.shutdown().await;
}
