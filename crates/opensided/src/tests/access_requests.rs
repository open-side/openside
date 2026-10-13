//! A paired device asking for more scopes and a person on the computer allowing it.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{extract::ConnectInfo, http::StatusCode, Router};
use http_body_util::BodyExt;
use openside_proto::{CapabilityScope, DeviceKind};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::remote::LOCAL_SECRET_HEADER;

async fn send(router: &Router, request: axum::extract::Request) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn local(method: &str, path: &str, token: &str, secret: &str) -> axum::extract::Request {
    let mut request = request(method, path, Value::Null, Some(token));
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

async fn paired(
    daemon: &Daemon,
    scopes: Vec<CapabilityScope>,
    kind: DeviceKind,
) -> (String, String) {
    daemon
        .shared
        .db
        .pair_as("phone".into(), scopes, kind)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_remote_device_asks_and_only_a_local_admin_can_allow() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, access) = send(
        &unix,
        request("GET", "/v1/remote/local-access", Value::Null, None),
    )
    .await;
    let (admin, secret) = (
        access["token"].as_str().unwrap(),
        access["secret"].as_str().unwrap(),
    );
    let (phone_id, phone) = paired(
        &daemon,
        vec![CapabilityScope::Read, CapabilityScope::Approve],
        DeviceKind::PersonDevice,
    )
    .await;
    let (_, remote_admin) = paired(
        &daemon,
        vec![CapabilityScope::Admin],
        DeviceKind::PersonDevice,
    )
    .await;

    let (status, me) = send(
        &tcp,
        request("GET", "/v1/devices/me", Value::Null, Some(&phone)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["scopes"], json!(["read", "approve"]));
    assert!(me["request"].is_null());

    let ask = |scopes: Value| {
        request(
            "POST",
            "/v1/devices/me/access-requests",
            json!({ "scopes": scopes }),
            Some(&phone),
        )
    };
    assert_eq!(send(&tcp, ask(json!([]))).await.1["code"], "invalid_value");
    assert_eq!(
        send(&tcp, ask(json!(["read"]))).await.1["code"],
        "invalid_value"
    );
    let (status, first) = send(&tcp, ask(json!(["admin"]))).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, second) = send(&tcp, ask(json!(["admin"]))).await;
    assert_ne!(first["id"], second["id"]);
    let (_, me) = send(
        &tcp,
        request("GET", "/v1/devices/me", Value::Null, Some(&phone)),
    )
    .await;
    assert_eq!(me["request"]["id"], second["id"]);

    // A phone cannot read the list, and neither it nor a remotely paired admin can allow.
    assert_eq!(
        send(
            &tcp,
            request("GET", "/v1/access-requests", Value::Null, Some(&phone))
        )
        .await
        .1["code"],
        "forbidden"
    );
    let (_, listed) = send(&tcp, local("GET", "/v1/access-requests", admin, secret)).await;
    assert_eq!(listed["requests"].as_array().unwrap().len(), 1);
    let approve = format!(
        "/v1/access-requests/{}/approve",
        second["id"].as_str().unwrap()
    );
    assert_eq!(
        send(&tcp, request("POST", &approve, Value::Null, Some(&phone)))
            .await
            .1["code"],
        "forbidden"
    );
    assert_eq!(
        send(
            &tcp,
            request("POST", &approve, Value::Null, Some(&remote_admin))
        )
        .await
        .1["code"],
        "local_access_required"
    );
    assert_eq!(
        send(&tcp, local("POST", &approve, remote_admin.as_str(), secret))
            .await
            .1["code"],
        "local_access_required"
    );

    let (status, granted) = send(&tcp, local("POST", &approve, admin, secret)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(granted["device_id"], phone_id);
    assert_eq!(granted["scopes"], json!(["read", "approve", "admin"]));
    assert_eq!(
        send(&tcp, local("POST", &approve, admin, secret)).await.1["code"],
        "not_found"
    );
    let (_, me) = send(
        &tcp,
        request("GET", "/v1/devices/me", Value::Null, Some(&phone)),
    )
    .await;
    assert_eq!(me["scopes"], json!(["read", "approve", "admin"]));
    assert!(me["request"].is_null());
    daemon.shutdown().await;
}

#[tokio::test]
async fn declined_and_expired_requests_grant_nothing_and_agents_cannot_ask() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let base = Instant::now();
    let offset = Arc::new(parking_lot::Mutex::new(Duration::ZERO));
    let clock = Arc::clone(&offset);
    daemon
        .shared
        .remote
        .set_clocks(move || base + *clock.lock(), chrono::Utc::now);
    let (_, phone) = paired(
        &daemon,
        vec![CapabilityScope::Read],
        DeviceKind::PersonDevice,
    )
    .await;
    let (_, agent) = paired(
        &daemon,
        vec![CapabilityScope::Read],
        DeviceKind::AgentClient,
    )
    .await;
    let ask = |token: &str| {
        request(
            "POST",
            "/v1/devices/me/access-requests",
            json!({"scopes": ["admin"]}),
            Some(token),
        )
    };

    assert_eq!(
        send(&tcp, ask(&agent)).await.1["code"],
        "person_approval_required"
    );
    let (_, declined) = send(&tcp, ask(&phone)).await;
    let deny = format!(
        "/v1/access-requests/{}/deny",
        declined["id"].as_str().unwrap()
    );
    assert_eq!(
        send(&unix, request("POST", &deny, Value::Null, None))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&unix, request("POST", &deny, Value::Null, None))
            .await
            .1["code"],
        "not_found"
    );
    let (_, me) = send(
        &tcp,
        request("GET", "/v1/devices/me", Value::Null, Some(&phone)),
    )
    .await;
    assert_eq!(me["scopes"], json!(["read"]));

    let (_, stale) = send(&tcp, ask(&phone)).await;
    *offset.lock() = Duration::from_secs(601);
    assert_eq!(
        send(
            &unix,
            request("GET", "/v1/access-requests", Value::Null, None)
        )
        .await
        .1["requests"],
        json!([])
    );
    let approve = format!(
        "/v1/access-requests/{}/approve",
        stale["id"].as_str().unwrap()
    );
    assert_eq!(
        send(&unix, request("POST", &approve, Value::Null, None))
            .await
            .1["code"],
        "not_found"
    );
    daemon.shutdown().await;
}
