//! Core semantics: token scopes over every public route (F-3, PP-27) and the daemon
//! hook that gates and redacts every registered tool (A-7, C-8, D-4).

use super::*;
use axum::{response::Html, routing::get, Router};
use openside_policy::Capability;
use openside_vault::{
    AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Redactor, Secret, Totp, Vault,
};
use std::collections::BTreeSet;

const ALL_SCOPES: [CapabilityScope; 8] = [
    CapabilityScope::Read,
    CapabilityScope::Steer,
    CapabilityScope::Approve,
    CapabilityScope::HumanStep,
    CapabilityScope::Admin,
    CapabilityScope::Browse,
    CapabilityScope::VaultUse,
    CapabilityScope::OtpIngest,
];

async fn send(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, token))
        .await
        .unwrap();
    let status = response.status();
    let json = response
        .headers()
        .get("content-type")
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    let body = if json {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    (status, body)
}

fn concrete(path: &str) -> String {
    let out = path
        .replace("{mid}", "message")
        .replace("{sid}", "child")
        .replace("{name}", "skill")
        .replace("{path}", "transcript.jsonl")
        .replace("{thumbnail_id}", "thumbnail")
        .replace("{id}", "session");
    assert!(!out.contains('{'), "unsubstituted path parameter in {out}");
    out
}

async fn token(daemon: &Daemon, scope: CapabilityScope) -> String {
    daemon.shared.db.pair(vec![scope]).await.unwrap().1
}

/// The reviewed scope of every public route, written out by hand rather than read back from
/// `ROUTES`, so a route that is mis-tagged (a mutation marked `Read`) or newly added without
/// review fails here instead of passing against itself. A route added to `ROUTES` must be
/// added to this table, in the same change, with the scope its review settled on.
const EXPECTED_SCOPES: &[(&str, &str, CapabilityScope)] = {
    use CapabilityScope::{Admin, Approve, Browse, HumanStep, OtpIngest, Read, Steer};
    &[
        ("GET", "/v1/health", Read),
        ("GET", "/v1/accounts", Read),
        ("GET", "/v1/browser/tabs", Read),
        ("GET", "/v1/sessions", Read),
        ("POST", "/v1/sessions", Steer),
        ("GET", "/v1/sessions/{id}", Read),
        ("PATCH", "/v1/sessions/{id}", Steer),
        ("DELETE", "/v1/sessions/{id}", Admin),
        ("POST", "/v1/sessions/{id}/messages", Steer),
        ("PATCH", "/v1/sessions/{id}/queue/{mid}", Steer),
        ("DELETE", "/v1/sessions/{id}/queue/{mid}", Steer),
        ("POST", "/v1/sessions/{id}/queue/{mid}/promote", Steer),
        ("POST", "/v1/sessions/{id}/queue/{mid}/move", Steer),
        ("POST", "/v1/sessions/{id}/stop", Steer),
        ("POST", "/v1/sessions/{id}/prewarm", Steer),
        ("POST", "/v1/browser/prewarm", Steer),
        ("GET", "/v1/sessions/{id}/events", Read),
        ("GET", "/v1/sessions/{id}/subagents", Read),
        ("POST", "/v1/sessions/{id}/subagents/{sid}/cancel", Steer),
        ("POST", "/v1/sessions/{id}/subagents/{sid}/steer", Steer),
        ("POST", "/v1/sessions/{id}/repl", Browse),
        ("POST", "/v1/sessions/{id}/tools/{name}", Browse),
        ("GET", "/v1/tools", Read),
        ("GET", "/v1/sessions/{id}/artifacts", Read),
        ("GET", "/v1/sessions/{id}/artifacts/{path}", Read),
        ("GET", "/v1/sessions/{id}/artifacts/{path}/preview", Read),
        ("GET", "/v1/sessions/{id}/visuals", Read),
        ("GET", "/v1/settings", Read),
        ("GET", "/v1/settings/config", Admin),
        ("PUT", "/v1/settings/config", Admin),
        ("GET", "/v1/tickets", Read),
        ("GET", "/v1/tickets/{id}", Read),
        ("POST", "/v1/tickets/{id}/resolve", Approve),
        ("POST", "/v1/tickets/{id}/reject", Approve),
        ("POST", "/v1/tickets/{id}/done", HumanStep),
        ("GET", "/v1/tickets/{id}/takeover", HumanStep),
        ("GET", "/v1/sessions/{id}/browser/preview", Browse),
        (
            "GET",
            "/v1/sessions/{id}/browser/thumbnails/{thumbnail_id}",
            Browse,
        ),
        ("GET", "/v1/routines", Read),
        ("POST", "/v1/routines", Admin),
        ("GET", "/v1/routines/suggestions", Read),
        ("GET", "/v1/routines/{id}", Read),
        ("PATCH", "/v1/routines/{id}", Admin),
        ("DELETE", "/v1/routines/{id}", Admin),
        ("POST", "/v1/routines/{id}/run", Admin),
        ("GET", "/v1/routines/{id}/runs", Read),
        ("GET", "/v1/memory", Read),
        ("POST", "/v1/memory", Admin),
        ("GET", "/v1/memory/health", Read),
        ("GET", "/v1/memory/{id}", Read),
        ("PATCH", "/v1/memory/{id}", Admin),
        ("DELETE", "/v1/memory/{id}", Admin),
        ("GET", "/v1/memory/{id}/history", Read),
        ("GET", "/v1/skills", Read),
        ("POST", "/v1/skills", Admin),
        ("GET", "/v1/skills/{name}", Read),
        ("PATCH", "/v1/skills/{name}", Admin),
        ("DELETE", "/v1/skills/{name}", Admin),
        ("GET", "/v1/policy", Read),
        ("PUT", "/v1/policy", Admin),
        ("POST", "/v1/policy/test", Read),
        ("GET", "/v1/vault/items", Admin),
        ("PATCH", "/v1/vault/items/{id}", Admin),
        ("GET", "/v1/vault/audit", Admin),
        ("GET", "/v1/vault", Admin),
        ("POST", "/v1/vault/setup", Admin),
        ("POST", "/v1/vault/unlock", Admin),
        ("POST", "/v1/vault/lock", Admin),
        ("POST", "/v1/vault/items", Admin),
        ("DELETE", "/v1/vault/items/{id}", Admin),
        ("GET", "/v1/pinned-tabs", Read),
        ("POST", "/v1/pinned-tabs", Admin),
        ("DELETE", "/v1/pinned-tabs/{id}", Admin),
        ("GET", "/v1/sync", Read),
        ("POST", "/v1/sync/setup", Admin),
        ("POST", "/v1/sync/pair", Admin),
        ("POST", "/v1/sync/run", Admin),
        ("POST", "/v1/devices/pair", Admin),
        ("DELETE", "/v1/devices/{id}", Admin),
        ("GET", "/v1/devices", Admin),
        ("GET", "/v1/onboarding", Read),
        ("PUT", "/v1/onboarding", Admin),
        ("GET", "/v1/external-agents", Admin),
        ("POST", "/v1/external-agents/{id}/browser-skill", Admin),
        ("GET", "/v1/import/profiles", Admin),
        ("POST", "/v1/import", Admin),
        ("GET", "/v1/remote/status", Admin),
        ("POST", "/v1/pairing", Admin),
        ("POST", "/v1/pairing/exchange", Admin),
        ("GET", "/v1/push/vapid", Read),
        ("GET", "/v1/push/subscriptions", Read),
        ("POST", "/v1/push/subscriptions", Read),
        ("DELETE", "/v1/push/subscriptions/{id}", Read),
        ("POST", "/v1/push/presence", Read),
        ("POST", "/v1/otp", OtpIngest),
    ]
};

fn expected_scope(method: &str, path: &str) -> CapabilityScope {
    EXPECTED_SCOPES
        .iter()
        .find(|(m, p, _)| *m == method && *p == path)
        .map(|(_, _, scope)| *scope)
        .unwrap_or_else(|| {
            panic!("unreviewed route {method} {path}: add it to EXPECTED_SCOPES in core_gaps.rs")
        })
}

/// Hand-written statement of the documented scope rules, deliberately not
/// `CapabilityScope::grants` and not `Route::scope`: admin reaches everything, steer also
/// reaches browse routes, ticket resolve/reject accept either human scope at the route layer
/// (the ticket kind decides later), and nothing else implies anything.
fn admits(token: CapabilityScope, route: &Route) -> bool {
    use CapabilityScope::*;
    if matches!(
        route.path,
        "/v1/tickets/{id}/resolve" | "/v1/tickets/{id}/reject"
    ) {
        return matches!(token, Approve | HumanStep | Admin);
    }
    let needed = expected_scope(route.method.as_str(), route.path);
    match token {
        Admin => true,
        Steer => matches!(needed, Steer | Browse),
        other => other == needed,
    }
}

/// The table and the public route list agree exactly, route by route: nothing is mis-tagged,
/// nothing is unreviewed, and no reviewed entry is stale.
#[test]
fn route_table_matches_the_reviewed_scopes() {
    for route in ROUTES {
        assert_eq!(
            route.scope,
            expected_scope(route.method.as_str(), route.path),
            "{} {} is tagged with a scope the review did not settle on",
            route.method.as_str(),
            route.path
        );
    }
    for (method, path, _) in EXPECTED_SCOPES {
        assert!(
            ROUTES
                .iter()
                .any(|r| r.method.as_str() == *method && r.path == *path),
            "reviewed route {method} {path} is no longer in ROUTES"
        );
    }
    assert_eq!(EXPECTED_SCOPES.len(), ROUTES.len());
}

/// Every path the API router registers is in the public route list, which `authorize` needs
/// to find the scope; otherwise TCP callers would be refused (fail closed) with no way in.
#[test]
fn every_router_path_is_in_the_public_route_table() {
    let source = include_str!("../api.rs");
    let mut found = 0;
    for part in source.split(".route(").skip(1) {
        let part = part.trim_start();
        let Some(rest) = part.strip_prefix('"') else {
            continue;
        };
        let path = &rest[..rest.find('"').unwrap()];
        found += 1;
        assert!(
            ROUTES.iter().any(|route| route.path == path),
            "{path} is routed by the daemon but missing from ROUTES"
        );
    }
    assert!(
        found >= 50,
        "only {found} routes found in api.rs; the scan is stale"
    );
}

/// A route the router serves but `ROUTES` does not list is refused over TCP, whatever token
/// the caller holds, instead of running unauthenticated.
#[tokio::test]
async fn an_unlisted_route_is_refused_over_tcp() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let admin = token(&daemon, CapabilityScope::Admin).await;
    let router = Router::new()
        .route("/v1/unlisted", get(|| async { "ran" }))
        .route_layer(axum::middleware::from_fn_with_state(
            (daemon.shared.clone(), false),
            api::authorize,
        ));
    for token in [Some(admin.as_str()), None] {
        let (status, _) = send(&router, "GET", "/v1/unlisted", json!({}), token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "token {token:?}");
    }
    let unix = Router::new()
        .route("/v1/unlisted", get(|| async { "ran" }))
        .route_layer(axum::middleware::from_fn_with_state(
            (daemon.shared.clone(), true),
            api::authorize,
        ));
    assert_eq!(
        send(&unix, "GET", "/v1/unlisted", json!({}), None).await.0,
        StatusCode::OK
    );
    daemon.shutdown().await;
}

/// F-3: every (route, token scope) pair of the public table behaves as documented. A token
/// that holds exactly one scope gets 403 on every route that scope does not admit and gets
/// through the capability layer on every route it admits; no token gets through to the
/// socket-only pairing route.
#[tokio::test]
async fn scope_matrix_covers_every_route_and_every_scope() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let mut tokens = Vec::new();
    for scope in ALL_SCOPES {
        tokens.push((scope, token(&daemon, scope).await));
    }
    let mut reachable: HashMap<CapabilityScope, usize> = HashMap::new();
    for route in ROUTES {
        let path = concrete(route.path);
        let method = route.method.as_str();
        let public = openside_proto::PUBLIC_ROUTES.contains(&(method, route.path));
        let (status, _) = send(&router, method, &path, json!({}), None).await;
        if public {
            // The one-time code in the body is the credential; no bearer token is asked for.
            assert!(
                status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                "{method} {path}: {status}"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path} without a token"
            );
        }
        for (scope, token) in &tokens {
            let (status, body) = send(&router, method, &path, json!({}), Some(token)).await;
            let socket_only = api::SOCKET_ONLY.contains(&(method, route.path));
            if public {
                assert!(
                    status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                    "{scope:?} was refused public {method} {path}: {status}"
                );
            } else if !admits(*scope, route) {
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{scope:?} reached {method} {path}"
                );
            } else if socket_only {
                assert_eq!(
                    (status, body["code"].as_str()),
                    (StatusCode::FORBIDDEN, Some("local_socket_required")),
                    "{scope:?} {method} {path}: TCP may never mint tokens"
                );
            } else {
                assert!(
                    status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                    "{scope:?} was refused {method} {path}: {status}"
                );
                *reachable.entry(*scope).or_default() += 1;
            }
        }
    }
    for scope in [
        CapabilityScope::Read,
        CapabilityScope::Steer,
        CapabilityScope::Approve,
        CapabilityScope::HumanStep,
        CapabilityScope::Admin,
        CapabilityScope::Browse,
    ] {
        assert!(
            reachable.get(&scope).copied().unwrap_or(0) > 0,
            "{scope:?} reaches nothing"
        );
    }
    // Only the OTP ingestion route takes the otp_ingest scope, and nothing takes vault_use: a
    // token holding only the latter is inert over the API.
    assert_eq!(reachable.get(&CapabilityScope::OtpIngest), Some(&1));
    assert_eq!(reachable.get(&CapabilityScope::VaultUse), None);
    daemon.shutdown().await;
}

/// F-3: the ticket kind picks the scope that may resolve or reject a ticket, for every scope
/// and both routes. Refusal is a 403 and leaves the ticket open; the scope that is allowed
/// gets past the capability layer (a 200 where the body is valid for the kind).
#[tokio::test]
async fn ticket_kind_decides_which_scope_may_resolve_or_reject_it() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let admin = token(&daemon, CapabilityScope::Admin).await;
    let deadline = Utc::now() + chrono::Duration::hours(1);
    let mut serial = 0;
    for (kind, answer) in [
        (TicketKind::Confirm, json!({"value": true})),
        (TicketKind::Info, json!({"value": "answer"})),
        (TicketKind::SecretInput, json!({"value": "typed"})),
        (TicketKind::DeviceApproval, json!({"value": true})),
        (TicketKind::Takeover, json!({"value": true})),
    ] {
        let needed = match kind {
            TicketKind::Confirm | TicketKind::Info => CapabilityScope::Approve,
            _ => CapabilityScope::HumanStep,
        };
        for action in ["resolve", "reject"] {
            for scope in ALL_SCOPES {
                serial += 1;
                let id = format!("ticket-{serial}");
                opened(&daemon, &session, kind, &id, deadline).await;
                let token = token(&daemon, scope).await;
                let body = if action == "resolve" {
                    answer.clone()
                } else {
                    json!({})
                };
                let (status, _) = send(
                    &router,
                    "POST",
                    &format!("/v1/tickets/{id}/{action}"),
                    body,
                    Some(&token),
                )
                .await;
                let allowed = scope == needed || scope == CapabilityScope::Admin;
                let what = format!("{scope:?} {action} of a {kind:?} ticket: {status}");
                if allowed {
                    assert!(
                        status != StatusCode::FORBIDDEN && status != StatusCode::UNAUTHORIZED,
                        "{what}"
                    );
                    if action == "reject" || matches!(kind, TicketKind::Confirm | TicketKind::Info)
                    {
                        assert_eq!(status, StatusCode::OK, "{what}");
                    }
                } else {
                    assert_eq!(status, StatusCode::FORBIDDEN, "{what}");
                    // The refused call changed nothing: an admin can still read it as pending.
                    let (_, ticket) = send(
                        &router,
                        "GET",
                        &format!("/v1/tickets/{id}"),
                        json!({}),
                        Some(&admin),
                    )
                    .await;
                    assert_eq!(ticket["status"], "pending", "{what}");
                }
            }
        }
    }
    daemon.shutdown().await;
}

/// F-3 positive side: each scope really performs its own work (2xx with the expected effect),
/// not merely "is not 403".
#[tokio::test]
async fn every_scope_succeeds_at_its_own_work() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let read = token(&daemon, CapabilityScope::Read).await;
    let steer = token(&daemon, CapabilityScope::Steer).await;
    let approve = token(&daemon, CapabilityScope::Approve).await;
    let human = token(&daemon, CapabilityScope::HumanStep).await;
    let admin = token(&daemon, CapabilityScope::Admin).await;
    let browse = token(&daemon, CapabilityScope::Browse).await;

    for path in [
        "/v1/health",
        "/v1/sessions",
        "/v1/sessions/session",
        "/v1/tickets",
        "/v1/policy",
    ] {
        assert_eq!(
            send(&router, "GET", path, json!({}), Some(&read)).await.0,
            StatusCode::OK,
            "{path}"
        );
    }

    // The first message starts a run that never settles in the test runtime, so the later
    // ones stay pending and the queue routes have something to act on.
    for (id, mode) in [
        ("running", "now"),
        ("q1", "queue"),
        ("q2", "queue"),
        ("q3", "queue"),
    ] {
        let (status, _) = send(
            &router,
            "POST",
            "/v1/sessions/session/messages",
            json!({"message_id": id, "text": id, "mode": mode}),
            Some(&steer),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{id}");
    }
    let queue = |token: String| {
        let router = router.clone();
        async move {
            let (_, body) = send(
                &router,
                "GET",
                "/v1/sessions/session",
                json!({}),
                Some(&token),
            )
            .await;
            body["queue"]
                .as_array()
                .unwrap()
                .iter()
                .map(|message| message["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(queue(read.clone()).await, ["q1", "q2", "q3"]);
    let (status, moved) = send(
        &router,
        "POST",
        "/v1/sessions/session/queue/q3/move",
        json!({"position": 0}),
        Some(&steer),
    )
    .await;
    assert_eq!((status, moved["id"].as_str()), (StatusCode::OK, Some("q3")));
    assert_eq!(queue(read.clone()).await, ["q3", "q1", "q2"]);
    let (status, edited) = send(
        &router,
        "PATCH",
        "/v1/sessions/session/queue/q1",
        json!({"text": "q1 edited"}),
        Some(&steer),
    )
    .await;
    assert_eq!(
        (status, edited["text"].as_str()),
        (StatusCode::OK, Some("q1 edited"))
    );
    assert_eq!(
        send(
            &router,
            "DELETE",
            "/v1/sessions/session/queue/q2",
            json!({}),
            Some(&steer)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(queue(read.clone()).await, ["q3", "q1"]);
    assert_eq!(
        send(
            &router,
            "POST",
            "/v1/sessions/session/queue/q1/promote",
            json!({}),
            Some(&steer)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            "POST",
            "/v1/sessions/session/stop",
            json!({}),
            Some(&steer)
        )
        .await
        .0,
        StatusCode::OK
    );
    // Moving or editing a message that already left the queue is a conflict, not a no-op.
    for (method, path, body) in [
        (
            "POST",
            "/v1/sessions/session/queue/q1/move",
            json!({"position": 0}),
        ),
        (
            "PATCH",
            "/v1/sessions/session/queue/q1",
            json!({"text": "late"}),
        ),
        (
            "POST",
            "/v1/sessions/session/queue/q2/move",
            json!({"position": 0}),
        ),
    ] {
        assert_eq!(
            send(&router, method, path, body, Some(&steer)).await.0,
            StatusCode::CONFLICT,
            "{path}"
        );
    }

    let deadline = Utc::now() + chrono::Duration::hours(1);
    opened(&daemon, &session, TicketKind::Confirm, "confirm", deadline).await;
    let (status, resolved) = send(
        &router,
        "POST",
        "/v1/tickets/confirm/resolve",
        json!({"value": true}),
        Some(&approve),
    )
    .await;
    assert_eq!(
        (status, resolved["status"].as_str()),
        (StatusCode::OK, Some("resolved"))
    );
    opened(
        &daemon,
        &session,
        TicketKind::DeviceApproval,
        "device",
        deadline,
    )
    .await;
    assert_eq!(
        send(
            &router,
            "POST",
            "/v1/tickets/device/done",
            json!({}),
            Some(&human)
        )
        .await
        .0,
        StatusCode::OK
    );

    // The live policy is local-socket-only even for admin (a_remote_admin_cannot_widen_the_agent_*).
    assert_eq!(
        send(
            &router,
            "PUT",
            "/v1/policy",
            json!({"default_mode": "guard"}),
            Some(&admin)
        )
        .await
        .1["code"],
        "local_socket_required"
    );
    assert_eq!(
        send(&router, "GET", "/v1/vault/audit", json!({}), Some(&admin))
            .await
            .0,
        StatusCode::OK
    );
    // Admin implies the other scopes; browse reaches the REPL route (which then fails for lack
    // of a browser, but past the capability layer).
    assert_eq!(
        send(
            &router,
            "POST",
            "/v1/sessions/session/messages",
            json!({"message_id": "by-admin", "text": "x", "mode": "queue"}),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/session/repl",
        json!({"code": "1 + 1"}),
        Some(&browse),
    )
    .await;
    assert!(
        status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
        "{status}"
    );
    daemon.shutdown().await;
}

/// PP-27: a read-scoped token cannot steer. Every steer-scoped route answers 403 and the
/// session's durable log and queue are untouched; a steer token on the same session works.
#[tokio::test]
async fn read_token_cannot_steer_through_any_steer_route() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let read = token(&daemon, CapabilityScope::Read).await;
    let steer = token(&daemon, CapabilityScope::Steer).await;
    for (id, mode) in [("running", "now"), ("pending", "queue")] {
        assert_eq!(
            send(
                &router,
                "POST",
                "/v1/sessions/session/messages",
                json!({"message_id": id, "text": id, "mode": mode}),
                Some(&steer)
            )
            .await
            .0,
            StatusCode::ACCEPTED
        );
    }
    let before = daemon.shared.db.events(&session, 0).await.unwrap().len();
    let steer_routes: Vec<_> = ROUTES
        .iter()
        .filter(|route| route.scope == CapabilityScope::Steer)
        .collect();
    assert!(
        steer_routes.len() >= 8,
        "the steer-scoped route set shrank: {}",
        steer_routes.len()
    );
    for route in &steer_routes {
        let path = concrete(route.path).replace("/queue/message", "/queue/pending");
        let body = match route.path {
            "/v1/sessions/{id}/messages" => json!({"text": "hijack", "mode": "now"}),
            "/v1/sessions/{id}/queue/{mid}" => json!({"text": "hijack"}),
            "/v1/sessions/{id}/queue/{mid}/move" => json!({"position": 0}),
            _ => json!({}),
        };
        let (status, _) = send(&router, route.method.as_str(), &path, body, Some(&read)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{} {path} accepted a read token",
            route.method.as_str()
        );
    }
    assert_eq!(
        daemon.shared.db.events(&session, 0).await.unwrap().len(),
        before
    );
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(state.queue.pending().len(), 1);
    assert_eq!(
        state.queue.text(&MessageId::new("pending")),
        Some("pending")
    );
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id": "by-steer", "text": "ok", "mode": "queue"}),
        Some(&steer),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(daemon.shared.db.events(&session, 0).await.unwrap().len() > before);
    daemon.shutdown().await;
}

/// A-4 ③: reordering also works in the steer lane, and a move never crosses lanes. While a
/// ticket is pending nothing is delivered, so steer messages stay queued and can be moved.
#[tokio::test]
async fn steer_lane_messages_reorder_without_crossing_into_the_queue_lane() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let steer = token(&daemon, CapabilityScope::Steer).await;
    let read = token(&daemon, CapabilityScope::Read).await;
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "blocker",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    for (id, mode) in [
        ("q1", "queue"),
        ("s1", "steer"),
        ("s2", "steer"),
        ("s3", "now"),
    ] {
        let (status, _) = send(
            &router,
            "POST",
            "/v1/sessions/session/messages",
            json!({"message_id": id, "text": id, "mode": mode}),
            Some(&steer),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{id}");
    }
    let order = || async {
        let (_, body) = send(
            &router,
            "GET",
            "/v1/sessions/session",
            json!({}),
            Some(&read),
        )
        .await;
        body["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        order().await,
        ["s1", "s2", "s3", "q1"],
        "steer lane first, then follow-ups"
    );
    for (id, position, expected) in [
        ("s3", 0, ["s3", "s1", "s2", "q1"]),
        ("s3", 1, ["s1", "s3", "s2", "q1"]),
        ("s1", 99, ["s3", "s2", "s1", "q1"]),
        // The follow-up is alone in its lane: no position lifts it above the steer messages.
        ("q1", 0, ["s3", "s2", "s1", "q1"]),
    ] {
        let (status, moved) = send(
            &router,
            "POST",
            &format!("/v1/sessions/session/queue/{id}/move"),
            json!({"position": position}),
            Some(&steer),
        )
        .await;
        assert_eq!(
            (status, moved["id"].as_str()),
            (StatusCode::OK, Some(id)),
            "move {id} to {position}"
        );
        assert_eq!(order().await, expected, "after moving {id} to {position}");
    }
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    let moves = events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::MessageMoved { .. }))
        .count();
    assert_eq!(moves, 4, "each accepted move is a durable event");
    daemon.shutdown().await;
}

// ---------------------------------------------------------------------------------------
// A-7 / C-8 / D-4: the harness hook, not the prompt, gates and redacts every tool.
// ---------------------------------------------------------------------------------------

const CANARY: &str = "core-gaps-canary-7d41";
const SEED: &str = "JBSWY3DPEHPK3PXP";

/// Tools whose `actions()` is empty by design: they touch no file, process, origin or
/// credential. A new tool with no policy action fails the gate test until it is listed here
/// with a reason: `ask_user` and `request_human_step` only open tickets, `browser_tabs`
/// lists the session's own tabs, `memory_*`/`skill` use the daemon's own stores (secrets are
/// refused on write), `delegate` and `task_start` admit children whose calls re-enter
/// this hook, Task management touches only their lifecycle, and `browser_repl`
/// evaluates policy per operation inside the snippet.
const NO_POLICY_CAPABILITY: [&str; 11] = [
    "ask_user",
    "browser_repl",
    "browser_tabs",
    "delegate",
    "memory_search",
    "memory_write",
    "skill",
    "task_cancel",
    "task_start",
    "task_status",
    "task_steer",
];

struct Killer(u32);
impl Drop for Killer {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let pid = nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap());
            let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = nix::sys::wait::waitpid(pid, None);
        }
    }
}

struct Rig {
    daemon: Daemon,
    session: SessionId,
    workspace: std::path::PathBuf,
    good: String,
    server: tokio::task::JoinHandle<()>,
    killer: parking_lot::Mutex<Option<Killer>>,
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
        drop(self.killer.get_mut().take());
    }
}

/// Full-access session on a daemon with real Chromium, an unlocked vault and a loopback page
/// that carries the canary in its title and body.
async fn rig(root: &Path, orchestration: openside_config::Orchestration) -> Option<Rig> {
    let mut config = resolved_config(root);
    config.loaded.config.agent.orchestration = orchestration;
    match test_browser() {
        Ok(executable) => config.loaded.config.browser.executable = Some(executable),
        Err(error) => {
            eprintln!("SKIP core gaps tool-gate tests need Chromium: {error}");
            return None;
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let good = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let page = format!(
        "<!doctype html><title>{CANARY}</title><body style='margin:0'><h1>{CANARY} page</h1>\
         <label>Username<input id='username'></label><label>Password<input id='password' type='password'></label>\
         <label>Code<input id='code'></label><button>Use passkey</button>\
         <button id='pay' onclick=\"document.getElementById('paid').textContent = 'paid'\">Pay now</button>\
         <span id='paid'>unpaid</span>"
    );
    let router = Router::new()
        .route(
            "/",
            get({
                let page = page.clone();
                move || async move { Html(page) }
            }),
        )
        .route(
            "/other",
            get({
                let page = page.clone();
                move || async move { Html(page) }
            }),
        )
        .route(
            &format!("/{CANARY}"),
            get(move || async move { Html(page) }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let path = root.join("vault.db");
    let mut vault = Vault::create(
        &path,
        root.join("setup-audit.jsonl"),
        &Secret::new("fixture-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(&good).unwrap()],
                Secret::new("gaps-user".into()),
                Secret::new("gaps-password".into()),
                Some(Totp::parse(Secret::new(SEED.into())).unwrap()),
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let pass_file = root.join("passphrase.txt");
    std::fs::write(&pass_file, "fixture-passphrase\n").unwrap();
    config.loaded.config.vault.path = Some(path);
    config.loaded.config.vault.passphrase_cmd =
        Some(vec!["cat".into(), pass_file.to_string_lossy().into_owned()]);
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let library = daemon.shared.config.data_dir.join("skills/gapskill");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("SKILL.md"),
        format!("---\nname: gapskill\ndescription: probe\n---\nSkill body mentions {CANARY}\n"),
    )
    .unwrap();
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session = create_session(&daemon, "gate", PolicyMode::Full, Some(&workspace)).await;
    Some(Rig {
        daemon,
        session,
        workspace,
        good,
        server,
        killer: parking_lot::Mutex::new(None),
    })
}

fn call(name: &str, args: Value, id: &str) -> openside_core::ToolCall {
    let mut call = super::call(name, args, id);
    // Probe policy hooks even when front orchestration would otherwise refuse acting tools.
    call.tool_call_id
        .insert_str(0, crate::tools::API_CALL_PREFIX);
    call
}

async fn run(rig: &Rig, name: &str, args: Value, id: &str) -> ToolOutcome {
    let outcome = host(&rig.daemon, &rig.session)
        .await
        .call(call(name, args, id))
        .await;
    if let Ok(browser) = rig.daemon.shared.registry.browser.existing().await {
        rig.killer
            .lock()
            .get_or_insert_with(|| Killer(browser.process_id()));
    }
    outcome
}

/// The fixture page as the tools see it: tab, accessibility refs by name, and viewport
/// coordinates (CSS pixels) of the heading and the payment button.
struct Tab {
    id: String,
    refs: HashMap<String, String>,
    heading: (f64, f64),
    pay: (f64, f64),
}

/// Opens the fixture page in a fresh session tab.
async fn open_tab(rig: &Rig, label: &str) -> Tab {
    let ToolOutcome::Done { for_model_json } = run(
        rig,
        "browser_open_tab",
        json!({"url": rig.good}),
        &format!("open-{label}"),
    )
    .await
    else {
        panic!("opening the fixture tab failed");
    };
    let id = serde_json::from_str::<Value>(&for_model_json).unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    run(
        rig,
        "browser_snapshot",
        json!({"tab_id": id}),
        &format!("refs-{label}"),
    )
    .await;
    let browser = rig
        .daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let refs = browser
        .snapshot(&id)
        .await
        .unwrap()
        .nodes
        .into_iter()
        .filter_map(|node| Some((node.name, node.reference?)))
        .collect();
    let center = |selector: &'static str| {
        let browser = browser.clone();
        let id = id.clone();
        async move {
            let point = browser
                .evaluate(
                    &id,
                    &format!("JSON.stringify((() => {{ const r = document.querySelector('{selector}').getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})())"),
                )
                .await
                .unwrap();
            let point: Vec<f64> = serde_json::from_str(point.as_str().unwrap()).unwrap();
            (point[0], point[1])
        }
    };
    let (heading, pay) = (center("h1").await, center("#pay").await);
    Tab {
        id,
        refs,
        heading,
        pay,
    }
}

/// One representative, valid call per registered tool. The state-ending calls come last.
fn tool_calls(
    rig: &Rig,
    tab: &Tab,
    file: &Path,
    env: &crate::tools::ToolEnv,
) -> Vec<(&'static str, Value)> {
    let (id, refs) = (&tab.id, &tab.refs);
    let marker = file.with_extension("marker");
    let mut calls = vec![
        ("read", json!({"path": file})),
        ("write", json!({"path": file, "content": "written"})),
        (
            "edit",
            json!({"path": file, "old_text": "written", "new_text": "edited"}),
        ),
        (
            "bash",
            json!({"command": format!("echo {CANARY} && touch {}", marker.display())}),
        ),
        (
            "browser_open_tab",
            json!({"url": format!("{}/{CANARY}?q={CANARY}", rig.good)}),
        ),
        ("browser_tabs", json!({})),
        ("browser_snapshot", json!({"tab_id": id})),
        ("browser_screenshot", json!({"tab_id": id})),
        (
            "browser_act",
            json!({"tab_id": id, "target": refs["Code"], "action": "focus"}),
        ),
        (
            "browser_pointer",
            json!({"tab_id": id, "action": "scroll", "x": tab.heading.0, "y": tab.heading.1, "delta_y": 1.0}),
        ),
        (
            "browser_repl",
            json!({"tab_id": id, "code": "await page.content()"}),
        ),
        (
            "vault_login",
            json!({"tab_id": id, "site": "fixture", "username_ref": refs["Username"], "password_ref": refs["Password"]}),
        ),
        (
            "vault_otp",
            json!({"tab_id": id, "site": "fixture", "field_ref": refs["Code"]}),
        ),
        (
            "request_otp",
            json!({"tab_id": id, "field_ref": refs["Code"]}),
        ),
        (
            "register_authenticator",
            json!({"tab_id": id, "field_ref": refs["Code"]}),
        ),
        ("request_login", json!({"tab_id": id})),
        ("request_choice", json!({"tab_id": id})),
        ("wait_for_approval", json!({"tab_id": id})),
        (
            "vault_passkey",
            json!({"tab_id": id, "site": "fixture", "action": "login", "submit_ref": refs["Use passkey"]}),
        ),
        (
            "memory_write",
            json!({"content": format!("note about {CANARY}")}),
        ),
        ("memory_search", json!({"query": CANARY})),
        ("skill", json!({"name": "gapskill"})),
        (
            "ask_user",
            json!({"question": format!("what is {CANARY}?")}),
        ),
        (
            "request_human_step",
            json!({"kind": "secret_input", "summary": "Password", "tab_id": id,
            "origin": rig.good, "field_ref": refs["Password"]}),
        ),
        (
            "delegate",
            json!({"task": format!("look at {CANARY}"), "tools": ["read"]}),
        ),
        (
            "task_start",
            json!({"goal":format!("look at {CANARY}"),"tools":["read"]}),
        ),
        ("task_status", json!({})),
        (
            "task_steer",
            json!({"task_id":"missing-task","message":format!("find {CANARY}")}),
        ),
        ("task_cancel", json!({"task_id":"missing-task"})),
        (
            "browser_navigate",
            json!({"tab_id": id, "url": format!("{}/{CANARY}", rig.good)}),
        ),
        ("browser_close_tab", json!({"tab_id": id})),
    ];
    let mut covered: Vec<_> = calls.iter().map(|(name, _)| *name).collect();
    covered.sort_unstable();
    let mut registered = rig.daemon.shared.registry.names();
    registered.sort_unstable();
    assert_eq!(
        covered, registered,
        "every registered tool needs a probe call (and every probe a registered tool)"
    );
    calls.retain(|(name, _)| rig.daemon.shared.registry.get(name).unwrap().visible(env));
    calls
}

/// Calls that would click the payment button, one per tool that can click a page control
/// after policy (browser_repl is probed separately: it decides inside the snippet).
fn sensitive_calls(tab: &Tab) -> Vec<(&'static str, Value)> {
    let (id, pay) = (&tab.id, &tab.refs["Pay now"]);
    vec![
        (
            "browser_act",
            json!({"tab_id": id, "target": pay, "action": "click"}),
        ),
        (
            "browser_pointer",
            json!({"tab_id": id, "action": "click", "x": tab.pay.0, "y": tab.pay.1}),
        ),
        (
            "vault_login",
            json!({"tab_id": id, "site": "fixture", "username_ref": tab.refs["Username"], "password_ref": tab.refs["Password"], "submit_ref": pay}),
        ),
        (
            "vault_passkey",
            json!({"tab_id": id, "site": "fixture", "action": "login", "submit_ref": pay}),
        ),
    ]
}

fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::FsRead => "fs_read",
        Capability::FsWrite => "fs_write",
        Capability::Exec => "exec",
        Capability::Browse => "browse",
        Capability::Fetch => "fetch",
        Capability::VaultUse => "vault_use",
        Capability::Sensitive => "sensitive",
    }
}

async fn put_policy(daemon: &Daemon, rules: Value) {
    let unix = api::router(daemon.shared.clone(), true);
    let (status, body) = send(
        &unix,
        "PUT",
        "/v1/policy",
        json!({"default_mode": "full", "rules": rules}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Page effects and durable side effects a tool call could have had, read back from outside
/// the tool: the unfilled form, the unclicked button, the tabs, the vault audit and the
/// artifacts directory.
async fn side_effects(rig: &Rig, tab: &Tab, label: &str) -> Value {
    let browser = rig
        .daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let page = browser
        .evaluate(
            &tab.id,
            "JSON.stringify([document.getElementById('username').value, document.getElementById('password').value, document.getElementById('code').value, document.getElementById('paid').textContent, location.href])",
        )
        .await
        .unwrap();
    let page: Value = serde_json::from_str(page.as_str().unwrap()).unwrap();
    let tabs = run(rig, "browser_tabs", json!({}), &format!("tabs-{label}")).await;
    let ToolOutcome::Done { for_model_json } = tabs else {
        panic!("browser_tabs: {tabs:?}")
    };
    let tabs: Value = serde_json::from_str(&for_model_json).unwrap();
    let unix = api::router(rig.daemon.shared.clone(), true);
    let (_, audit) = send(
        &unix,
        "GET",
        &format!("/v1/vault/audit?session={}", rig.session),
        json!({}),
        None,
    )
    .await;
    json!({
        "page": page,
        "tab_urls": tabs.as_array().unwrap().iter().map(|t| t["url"].clone()).collect::<Vec<_>>(),
        "vault_uses": audit["records"].as_array().unwrap().len(),
        "artifacts": rig.daemon.shared.config.data_dir.join("artifacts").join(rig.session.as_str()).exists(),
        "bash_marker": rig.workspace.join(format!("{CANARY}.marker")).exists(),
        "file": std::fs::read_to_string(rig.workspace.join(format!("{CANARY}.txt"))).unwrap(),
    })
}

/// A-7 / C-8 / D-4: the before-call gate is the daemon's, whatever the model is told. Every
/// registered tool (the loop iterates the registry) is decided by the live policy before it
/// runs: a deny rule for each capability the call needs refuses it, an ask rule parks it
/// behind an approval ticket, and a forged approval reference never releases it, even for
/// tools that need no capability. None of that leaves a side effect, which is read back from
/// the page, the file system, the tab list and the vault audit, with a positive control that
/// the same calls do act once allowed. A tool without any policy capability must be on the
/// reviewed list, and every tool refuses arguments outside its schema before doing anything.
#[tokio::test]
async fn every_registered_tool_is_decided_by_the_live_policy_before_it_runs() {
    check_policy_hooks(openside_config::Orchestration::Single).await;
    check_policy_hooks(openside_config::Orchestration::Front).await;
}

async fn check_policy_hooks(orchestration: openside_config::Orchestration) {
    let root = tempfile::tempdir().unwrap();
    let Some(rig) = rig(root.path(), orchestration).await else {
        return;
    };
    let file = rig.workspace.join(format!("{CANARY}.txt"));
    std::fs::write(&file, CANARY).unwrap();
    let tab = open_tab(&rig, "gate").await;
    let host = host(&rig.daemon, &rig.session).await;
    let calls = tool_calls(&rig, &tab, &file, &host.env);
    let before = side_effects(&rig, &tab, "before").await;
    assert_eq!(
        before["page"],
        json!(["", "", "", "unpaid", format!("{}/", rig.good)])
    );
    assert_eq!(before["tab_urls"], json!([format!("{}/", rig.good)]));
    assert_eq!(before["vault_uses"], 0);
    assert_eq!(before["artifacts"], false);
    assert_eq!(before["bash_marker"], false);

    let mut without_capability = Vec::new();
    let mut serial = 0;
    for (name, args) in &calls {
        serial += 1;
        let tool = rig.daemon.shared.registry.get(name).unwrap();
        let actions = tool
            .actions(args, &host.env)
            .await
            .unwrap_or_else(|error| panic!("{name}: probe call is not valid: {}", error.0));
        let capabilities: BTreeSet<_> = actions
            .iter()
            .map(|a| capability_name(a.capability))
            .collect();
        if capabilities.is_empty() {
            without_capability.push(*name);
        }
        for capability in &capabilities {
            put_policy(
                &rig.daemon,
                json!([{"effect": "deny", "capability": capability, "pattern": "**"}]),
            )
            .await;
            let outcome = host
                .call(call(
                    name,
                    args.clone(),
                    &format!("deny-{serial}-{capability}"),
                ))
                .await;
            let ToolOutcome::Denied { reason, .. } = &outcome else {
                panic!("{name} ran under a deny rule for {capability}: {outcome:?}");
            };
            assert!(
                reason.contains("Policy denied"),
                "{name}/{capability}: {reason}"
            );

            put_policy(
                &rig.daemon,
                json!([{"effect": "ask", "capability": capability, "pattern": "**"}]),
            )
            .await;
            let outcome = host
                .call(call(
                    name,
                    args.clone(),
                    &format!("ask-{serial}-{capability}"),
                ))
                .await;
            assert!(
                matches!(
                    outcome,
                    ToolOutcome::Deferred {
                        kind: DeferKind::Approval,
                        ..
                    }
                ),
                "{name} ran under an ask rule for {capability} without approval: {outcome:?}"
            );
        }
        // A forged approval reference is refused whether or not the call needed one.
        put_policy(&rig.daemon, json!([])).await;
        let mut forged = call(name, args.clone(), &format!("forged-{serial}"));
        forged.approval_ticket = Some(TicketId::new("forged"));
        let outcome = host.call(forged).await;
        let ToolOutcome::Denied { reason, .. } = &outcome else {
            panic!("{name}: a forged approval reference released the call: {outcome:?}");
        };
        assert!(
            reason.contains("Approval ticket does not authorize"),
            "{name}: {reason}"
        );
        let outcome = host
            .call(call(
                name,
                json!({"unexpected_field": CANARY}),
                &format!("schema-{serial}"),
            ))
            .await;
        assert!(
            matches!(
                outcome,
                ToolOutcome::Denied { .. } | ToolOutcome::Failed { .. }
            ),
            "{name} accepted arguments outside its schema: {outcome:?}"
        );
    }
    without_capability.sort_unstable();
    assert_eq!(
        without_capability,
        NO_POLICY_CAPABILITY
            .into_iter()
            .filter(|name| rig
                .daemon
                .shared
                .registry
                .get(name)
                .unwrap()
                .visible(&host.env))
            .collect::<Vec<_>>(),
        "the set of tools without a policy capability changed; review the new tool"
    );

    // Sensitive actions (payment, purchase, send, delete) are never auto-allowed, not even in
    // a Full-access session with no rules at all: each tool that can click a page control
    // parks the call behind an approval, and the control is not clicked.
    put_policy(&rig.daemon, json!([])).await;
    for (name, args) in sensitive_calls(&tab) {
        let outcome = host
            .call(call(name, args.clone(), &format!("sensitive-{name}")))
            .await;
        assert!(
            matches!(
                outcome,
                ToolOutcome::Deferred {
                    kind: DeferKind::Approval,
                    ..
                }
            ),
            "{name} clicked a payment control in Full mode without approval: {outcome:?}"
        );
        let mut forged = call(name, args, &format!("sensitive-forged-{name}"));
        forged.approval_ticket = Some(TicketId::new("forged"));
        assert!(
            matches!(host.call(forged).await, ToolOutcome::Denied { .. }),
            "{name}"
        );
    }
    // browser_repl decides per operation inside the snippet: the click stops at the step, the
    // caller gets the approval descriptors, and sending them back opens a ticket instead of
    // running the snippet.
    let snippet = json!({"tab_id": tab.id, "code": format!("await page.locator('aria-ref={}').click()", tab.refs["Pay now"])});
    let outcome = host
        .call(call("browser_repl", snippet.clone(), "sensitive-repl"))
        .await;
    let ToolOutcome::Denied { reason, hint } = &outcome else {
        panic!("browser_repl clicked a payment control without approval: {outcome:?}");
    };
    assert!(reason.contains("approval_required"), "{reason}");
    let hint = hint.as_deref().unwrap();
    let approvals: Value = serde_json::from_str(
        &hint[hint.rfind("\"approvals\": ").unwrap() + "\"approvals\": ".len()..],
    )
    .unwrap();
    let mut again = snippet.clone();
    again["approvals"] = approvals;
    let outcome = host
        .call(call("browser_repl", again, "sensitive-repl-approval"))
        .await;
    assert!(
        matches!(
            outcome,
            ToolOutcome::Deferred {
                kind: DeferKind::Approval,
                ..
            }
        ),
        "{outcome:?}"
    );

    // browser_repl also obeys a Browse deny inside the snippet: it cannot read its own page.
    put_policy(
        &rig.daemon,
        json!([{"effect": "deny", "capability": "browse", "pattern": "**"}]),
    )
    .await;
    let outcome = run(
        &rig,
        "browser_repl",
        json!({"tab_id": tab.id, "code": "await page.content()"}),
        "repl-deny",
    )
    .await;
    let ToolOutcome::Denied { reason, .. } = &outcome else {
        panic!("browser_repl ignored a Browse deny: {outcome:?}");
    };
    assert!(reason.contains("policy"), "{reason}");
    put_policy(&rig.daemon, json!([])).await;

    // None of the refused, parked or forged calls did anything.
    assert_eq!(
        side_effects(&rig, &tab, "refused").await,
        before,
        "a refused call had a side effect"
    );

    // Positive control: the same bash, write and vault_login probes do act when allowed, so
    // the unchanged state above is the gate's doing, not an inert probe.
    for name in ["bash", "write", "vault_login"] {
        let args = calls.iter().find(|(n, _)| *n == name).unwrap().1.clone();
        let outcome = run(&rig, name, args, &format!("control-{name}")).await;
        assert!(
            matches!(outcome, ToolOutcome::Done { .. }),
            "{name}: {outcome:?}"
        );
    }
    let after = side_effects(&rig, &tab, "after").await;
    assert_eq!(after["bash_marker"], true);
    assert_eq!(after["file"], "written");
    assert_eq!(
        after["page"][0], "gaps-user",
        "vault_login did not fill the form"
    );
    assert!(
        after["vault_uses"].as_u64().unwrap() > 0,
        "vault audit stayed empty"
    );

    // Every call, allowed or refused, left a tool.started/tool.ended pair in the durable log.
    let events = rig.daemon.shared.db.events(&rig.session, 0).await.unwrap();
    let ended: BTreeSet<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolEnded { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    for (name, _) in &calls {
        let started: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ToolStarted {
                    tool_call_id,
                    name: n,
                    ..
                } if n == name => Some(tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(!started.is_empty(), "{name} left no tool.started event");
        for id in started {
            assert!(
                ended.contains(id),
                "{name} call {id} has no tool.ended event"
            );
        }
    }
    rig.daemon.shutdown().await;
}

/// Everything a tool returned that the model would see, plus the durable tool.ended result.
async fn run_every_tool(rig: &Rig, label: &str) -> Vec<(&'static str, String)> {
    let file = rig.workspace.join(format!("{CANARY}-{label}.txt"));
    std::fs::write(&file, CANARY).unwrap();
    let tab = open_tab(rig, label).await;
    let host = host(&rig.daemon, &rig.session).await;
    let mut outputs = Vec::new();
    for (index, (name, args)) in tool_calls(rig, &tab, &file, &host.env)
        .into_iter()
        .enumerate()
    {
        let id = format!("{label}-{index}");
        let outcome = tokio::time::timeout(Duration::from_secs(60), run(rig, name, args, &id))
            .await
            .unwrap_or_else(|_| panic!("{name} did not finish"));
        let events = rig.daemon.shared.db.events(&rig.session, 0).await.unwrap();
        let recorded: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::ToolEnded {
                    tool_call_id,
                    result,
                    ..
                } if *tool_call_id == format!("{}{id}", crate::tools::API_CALL_PREFIX) => {
                    Some(result.to_string())
                }
                _ => None,
            })
            .collect();
        outputs.push((name, format!("{outcome:?}\n{}", recorded.join("\n"))));
    }
    outputs
}

/// Exactly the tools whose result carries text that a page, file, note, URL or the model's own
/// arguments control. The control run must leak the canary through these and no others, so a
/// tool that starts echoing page text (or stops) changes this list under review.
const TEXT_RETURNING: [&str; 11] = [
    "bash",
    "browser_navigate",
    "browser_open_tab",
    "browser_pointer",
    "browser_repl",
    "browser_snapshot",
    "browser_tabs",
    "memory_search",
    "read",
    "skill",
    "task_status",
];

/// A-7 / D-4: the after-call hook redacts secrets from every tool result before the model or
/// the durable log sees it. The same calls run twice, first without and then with the
/// redactor loaded: the canary must appear unredacted through exactly the `TEXT_RETURNING`
/// tools (positive control: the test fails if redaction were removed) and through no tool
/// afterwards, nor in any event the second run appended to the durable log.
#[tokio::test]
async fn every_tool_result_is_redacted_by_the_daemon_after_the_call() {
    check_redaction_hooks(openside_config::Orchestration::Single).await;
    check_redaction_hooks(openside_config::Orchestration::Front).await;
}

async fn check_redaction_hooks(orchestration: openside_config::Orchestration) {
    let root = tempfile::tempdir().unwrap();
    let Some(rig) = rig(root.path(), orchestration).await else {
        return;
    };
    let control = run_every_tool(&rig, "control").await;
    let control_last_seq = rig
        .daemon
        .shared
        .db
        .events(&rig.session, 0)
        .await
        .unwrap()
        .last()
        .unwrap()
        .seq;
    rig.daemon
        .shared
        .registry
        .redactors
        .write()
        .push(Redactor::new([&Secret::new(CANARY.into())]));
    let redacted = run_every_tool(&rig, "redacted").await;

    let mut leaking: Vec<_> = control
        .iter()
        .filter(|(_, text)| text.contains(CANARY))
        .map(|(name, _)| *name)
        .collect();
    leaking.sort_unstable();
    let host = host(&rig.daemon, &rig.session).await;
    assert_eq!(
        leaking,
        TEXT_RETURNING
            .into_iter()
            .filter(|name| rig
                .daemon
                .shared
                .registry
                .get(name)
                .unwrap()
                .visible(&host.env))
            .collect::<Vec<_>>(),
        "control run leaked through a different tool set:\n{control:#?}"
    );
    assert_eq!(control.len(), redacted.len());
    for (name, text) in &redacted {
        assert!(
            !text.contains(CANARY),
            "{name} leaked the canary through the hook:\n{text}"
        );
    }
    // Not only tool.ended: every event the redacted run appended (tool.started arguments,
    // tickets, tab and subagent events, replies) is clean.
    let later = rig
        .daemon
        .shared
        .db
        .events(&rig.session, control_last_seq)
        .await
        .unwrap();
    assert!(
        later.len() > redacted.len(),
        "the redacted run left too few events: {}",
        later.len()
    );
    for event in &later {
        let json = serde_json::to_string(event).unwrap();
        assert!(
            !json.contains(CANARY),
            "event {} carries the canary: {json}",
            event.seq
        );
    }
    rig.daemon.shutdown().await;
}
