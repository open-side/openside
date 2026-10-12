//! Remote devices: QR pairing, revocation, Web Push, OTP ingest, settings API and the
//! no-feature-flag guarantee (F-2, F-3, PP-13, PP-15, WS-5).

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{body::Body, http::StatusCode, Router};
use chrono::Utc;
use http_body_util::BodyExt;
use openside_push::fake_push::FakePushService;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;

async fn call(
    router: &Router,
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn audit_lines(daemon: &Daemon) -> Vec<Value> {
    std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn daemon_with(
    root: &std::path::Path,
    runtime: &TestRuntime,
    edit: impl FnOnce(&mut openside_config::Config),
) -> Daemon {
    let mut config = resolved_config(root);
    edit(&mut config.loaded.config);
    Daemon::open_runtime(config, runtime.clone()).await.unwrap()
}

/// Injected clocks: monotonic offset for deadlines, wall offset for display values.
struct TestClock {
    t0: chrono::DateTime<Utc>,
    mono: Arc<parking_lot::Mutex<Duration>>,
    wall: Arc<parking_lot::Mutex<chrono::Duration>>,
}

impl TestClock {
    fn install(daemon: &Daemon) -> Self {
        let (base, t0) = (std::time::Instant::now(), Utc::now());
        let mono = Arc::new(parking_lot::Mutex::new(Duration::ZERO));
        let wall = Arc::new(parking_lot::Mutex::new(chrono::Duration::zero()));
        let (m, w) = (mono.clone(), wall.clone());
        daemon
            .shared
            .remote
            .set_clocks(move || base + *m.lock(), move || t0 + *w.lock());
        Self { t0, mono, wall }
    }
    fn at(&self, secs: u64) {
        *self.mono.lock() = Duration::from_secs(secs);
    }
}

fn allow_push(push: &FakePushService) -> impl FnOnce(&mut openside_config::Config) {
    let entry = format!("http://{}", push.addr());
    move |config| config.network.allow = vec![entry]
}

#[tokio::test]
async fn pairing_code_expires_at_three_minutes_is_single_use_scoped_and_audited() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.remote.public_url = Some("https://desk.example.test".into());
    })
    .await;
    let clock = TestClock::install(&daemon);
    let t0 = clock.t0;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);

    let (status, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"phone","scopes":["read","approve","read"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{offer}");
    let code = offer["code"].as_str().unwrap().to_owned();
    assert_eq!(offer["scopes"], json!(["read", "approve"]));
    assert_eq!(offer["path"], format!("/pair#{code}"));
    assert_eq!(
        offer["url"],
        format!("https://desk.example.test/pair#{code}")
    );
    let expires: chrono::DateTime<Utc> =
        serde_json::from_value(offer["expires_at"].clone()).unwrap();
    assert_eq!(expires - t0, chrono::Duration::seconds(180));

    // Issuing needs the local socket, even for an admin token.
    let (_, admin) = daemon
        .shared
        .db
        .pair_named("admin".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/pairing",
        json!({"name":"x","scopes":["read"]}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_socket_required"))
    );
    let (status, _) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"x","scopes":[]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // One second before expiry the exchange works over TCP without any token.
    clock.at(179);
    let (status, paired) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":code}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{paired}");
    let token = paired["token"].as_str().unwrap().to_owned();
    assert_eq!(paired["scopes"], json!(["read", "approve"]));
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":code}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("pairing_not_found"))
    );

    // The token carries exactly the offered scopes.
    assert_eq!(
        call(&tcp, "GET", "/v1/sessions", Value::Null, Some(&token))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&tcp, "POST", "/v1/sessions", json!({}), Some(&token))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&tcp, "GET", "/v1/devices", Value::Null, Some(&token))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // At exactly three minutes the code is refused, and it stays consumed.
    clock.at(0);
    let (_, late) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"late","scopes":["read"]}),
        None,
    )
    .await;
    clock.at(180);
    let late = late["code"].as_str().unwrap();
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":late}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::GONE, Some("pairing_expired"))
    );
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/pairing/exchange",
            json!({"code":late}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let (status, devices) = call(&unix, "GET", "/v1/devices", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    let phone = devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "phone")
        .unwrap();
    assert_eq!(phone["device_id"], paired["device_id"]);
    assert_eq!(phone["scopes"], json!(["read", "approve"]));
    assert!(!devices.to_string().contains(&token));

    let decisions: Vec<(String, String)> = audit_lines(&daemon)
        .iter()
        .map(|line| {
            (
                line["tool"].as_str().unwrap().to_owned(),
                line["decision"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    for expected in ["pairing_issued", "paired", "pairing_expired"] {
        assert!(
            decisions.contains(&("device".into(), expected.into())),
            "{decisions:?}"
        );
    }
    let audit = std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl")).unwrap();
    assert!(!audit.contains(&code) && !audit.contains(late) && !audit.contains(&token));
    daemon.shutdown().await;
}

/// Read SSE frames until the stream ends; false if it is still open after `limit`.
async fn ends_within(body: &mut Body, limit: Duration) -> bool {
    tokio::time::timeout(limit, async {
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn revoked_device_is_refused_next_request_its_streams_close_and_push_subscriptions_go() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let daemon = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (gone_id, gone) = daemon
        .shared
        .db
        .pair_named("old phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, kept) = daemon
        .shared
        .db
        .pair_named("laptop".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let subscription = push.subscribe();
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/push/subscriptions",
            subscription.to_json(),
            Some(&gone)
        )
        .await
        .0,
        StatusCode::CREATED
    );

    let open = |token: String| {
        let tcp = tcp.clone();
        let path = format!("/v1/sessions/{}/events", session.as_str());
        async move {
            let response = tcp
                .oneshot(request("GET", &path, Value::Null, Some(&token)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response.into_body()
        }
    };
    let mut revoked_stream = open(gone.clone()).await;
    let mut kept_stream = open(kept.clone()).await;
    assert!(!ends_within(&mut revoked_stream, Duration::from_millis(200)).await);

    assert_eq!(
        call(
            &unix,
            "DELETE",
            &format!("/v1/devices/{gone_id}"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(
        ends_within(&mut revoked_stream, Duration::from_secs(2)).await,
        "revoked stream stayed open"
    );
    assert!(
        !ends_within(&mut kept_stream, Duration::from_millis(300)).await,
        "other device's stream closed"
    );
    let (status, body) = call(&tcp, "GET", "/v1/sessions", Value::Null, Some(&gone)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("unauthorized"))
    );
    assert_eq!(
        call(&tcp, "GET", "/v1/sessions", Value::Null, Some(&kept))
            .await
            .0,
        StatusCode::OK
    );
    let (_, list) = call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None).await;
    assert_eq!(list["subscriptions"], json!([]));
    assert!(audit_lines(&daemon)
        .iter()
        .any(|line| line["decision"] == "revoked"
            && line["device_id"] == gone_id
            && line["existed"] == true));
    daemon.shutdown().await;
}

#[tokio::test]
async fn push_reaches_unfocused_subscriptions_encrypted_with_deep_links_and_drops_gone_ones() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let daemon = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let clock = TestClock::install(&daemon);
    let session = create_session(&daemon, "s1", PolicyMode::Guard, None).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, phone) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, desk) = daemon
        .shared
        .db
        .pair_named("desk".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (phone_sub, desk_sub) = (push.subscribe(), push.subscribe());

    let (status, vapid) = call(&tcp, "GET", "/v1/push/vapid", Value::Null, Some(&phone)).await;
    assert_eq!(status, StatusCode::OK);
    let vapid = vapid["public_key"].as_str().unwrap().to_owned();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(daemon.shared.config.data_dir.join("push-vapid.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let bad = json!({"endpoint":"http://push.example.test/x","keys":phone_sub.to_json()["keys"]});
    assert_eq!(
        call(&tcp, "POST", "/v1/push/subscriptions", bad, Some(&phone))
            .await
            .1["code"],
        "invalid_subscription"
    );
    let (status, created) = call(
        &tcp,
        "POST",
        "/v1/push/subscriptions",
        phone_sub.to_json(),
        Some(&phone),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let phone_id = created["id"].as_str().unwrap().to_owned();
    let (_, created) = call(
        &tcp,
        "POST",
        "/v1/push/subscriptions",
        desk_sub.to_json(),
        Some(&desk),
    )
    .await;
    let desk_id = created["id"].as_str().unwrap().to_owned();
    // Each device sees only its own subscription.
    let (_, own) = call(
        &tcp,
        "GET",
        "/v1/push/subscriptions",
        Value::Null,
        Some(&phone),
    )
    .await;
    assert_eq!(own["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(own["subscriptions"][0]["id"], phone_id);
    assert!(!own.to_string().contains(&phone_sub.auth));
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/push/presence",
            json!({"subscription_id":phone_id,"focused":true}),
            Some(&desk)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // The desk is focused: only the phone is notified.
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/push/presence",
            json!({"subscription_id":desk_id,"focused":true}),
            Some(&desk)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "t1",
        Utc::now() + chrono::Duration::minutes(10),
    )
    .await;
    let deliveries = push.wait_for(1, Duration::from_secs(5)).await;
    assert_eq!(deliveries.len(), 1, "{:?}", push.rejections());
    assert_eq!(deliveries[0].subscriber, phone_sub.id);
    assert_eq!(deliveries[0].vapid_key, vapid);
    assert_eq!(deliveries[0].json()["url"], "/sessions/s1?ticket=t1");
    assert_eq!(deliveries[0].json()["kind"], "ticket");
    assert_eq!(deliveries[0].json()["title"], "Approval needed");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        push.deliveries().len(),
        1,
        "focused desk must not be pushed"
    );

    // Focus lapses after 60 seconds without renewal.
    clock.at(61);
    opened(
        &daemon,
        &session,
        TicketKind::Info,
        "t2",
        Utc::now() + chrono::Duration::minutes(10),
    )
    .await;
    let deliveries = push.wait_for(3, Duration::from_secs(5)).await;
    let mut reached: Vec<String> = deliveries[1..]
        .iter()
        .map(|d| d.subscriber.clone())
        .collect();
    reached.sort();
    let mut expected = vec![phone_sub.id.clone(), desk_sub.id.clone()];
    expected.sort();
    assert_eq!(reached, expected);

    // A subscription the push service reports gone is deleted.
    push.expire(&desk_sub.id);
    let quiet = create_session(&daemon, "s2", PolicyMode::Guard, None).await;
    daemon
        .shared
        .emit(&quiet, None, EventKind::SessionSettled {})
        .await
        .unwrap();
    let deliveries = push.wait_for(4, Duration::from_secs(5)).await;
    let settled = deliveries.last().unwrap().json();
    assert_eq!(
        (settled["kind"].as_str(), settled["url"].as_str()),
        (Some("settled"), Some("/sessions/s2"))
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, list) = call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None).await;
            if list["subscriptions"].as_array().unwrap().len() == 1 {
                assert_eq!(list["subscriptions"][0]["id"], phone_id);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // A settled session still waiting on a ticket does not push "finished".
    let before = push.deliveries().len();
    daemon
        .shared
        .emit(&session, None, EventKind::SessionSettled {})
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(push.deliveries().len(), before);
    assert!(
        push.rejections()
            .iter()
            .all(|reason| reason.starts_with("expired subscriber")),
        "{:?}",
        push.rejections()
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn otp_ingest_checks_scope_and_sender_fills_matching_secret_input_and_expires_codes() {
    const CODE: &str = "481516";
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.otp.senders = vec!["BANK".into()];
        config.otp.sites =
            BTreeMap::from([("https://example.test".into(), r"code[: ]+(\d{6})".into())]);
        config.otp.ttl_secs = 60;
    })
    .await;
    let clock = TestClock::install(&daemon);
    let filled = Arc::new(parking_lot::Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(CanarySink {
        filled: filled.clone(),
        expected: CODE,
        mismatch: false,
    }));
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, otp) = daemon
        .shared
        .db
        .pair_named("sms forwarder".into(), vec![CapabilityScope::OtpIngest])
        .await
        .unwrap();
    let (_, approve) = daemon
        .shared
        .db
        .pair_named(
            "phone".into(),
            vec![CapabilityScope::Approve, CapabilityScope::Steer],
        )
        .await
        .unwrap();
    let message =
        json!({"sender":"BANK","text":format!("Your login code: {CODE}. Do not share it.")});

    assert_eq!(
        call(&tcp, "POST", "/v1/otp", message.clone(), Some(&approve))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/otp",
        json!({"sender":"+15550100","text":message["text"]}),
        Some(&otp),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("sender_not_allowed"))
    );

    // A pending step for the site is filled at once.
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "now",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    let (status, body) = call(&tcp, "POST", "/v1/otp", message.clone(), Some(&otp)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"matched_sites":["https://example.test"],"resolved_tickets":["now"],"stored":0})
    );
    assert_eq!(*filled.lock(), 1);
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        state.tickets[&TicketId::new("now")].status,
        TicketStatus::Resolved
    );

    // A code that arrives first waits for the step, within its TTL.
    let (_, body) = call(&tcp, "POST", "/v1/otp", message.clone(), Some(&otp)).await;
    assert_eq!(body["stored"], 1);
    clock.at(59);
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "later",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    wait_event(&daemon, &session, |kind| matches!(kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id.as_str() == "later")).await;
    assert_eq!(*filled.lock(), 2);

    // An expired code is gone: the next step stays pending.
    clock.at(0);
    let (_, body) = call(&tcp, "POST", "/v1/otp", message.clone(), Some(&otp)).await;
    assert_eq!(body["stored"], 1);
    clock.at(61);
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "stale",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        state.tickets[&TicketId::new("stale")].status,
        TicketStatus::Pending
    );
    assert_eq!(*filled.lock(), 2);

    // Text without a code for any site matches nothing and stores nothing.
    let (_, body) = call(
        &tcp,
        "POST",
        "/v1/otp",
        json!({"sender":"BANK","text":"Your statement is ready"}),
        Some(&otp),
    )
    .await;
    assert_eq!(
        body,
        json!({"matched_sites":[],"resolved_tickets":[],"stored":0})
    );

    for entry in std::fs::read_dir(&daemon.shared.config.data_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes.windows(CODE.len()).any(|w| w == CODE.as_bytes()),
                "{path:?} holds the code"
            );
        }
    }
    assert!(audit_lines(&daemon)
        .iter()
        .any(|line| line["tool"] == "otp" && line["decision"] == "otp_rejected"));
    daemon.shutdown().await;
}

/// Every leaf key of the configuration JSON Schema, as a dotted path.
fn leaf_keys(schema: &Value) -> Vec<(String, Value)> {
    fn resolve<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
        if let Some(reference) = node["$ref"].as_str() {
            let name = reference.rsplit('/').next().unwrap();
            return &root["$defs"][name];
        }
        if let Some(any) = node["anyOf"].as_array() {
            if let Some(inner) = any.iter().find(|n| n["type"] != "null") {
                return resolve(root, inner);
            }
        }
        node
    }
    fn walk(root: &Value, node: &Value, prefix: &str, out: &mut Vec<(String, Value)>) {
        let node = resolve(root, node);
        match node["properties"].as_object() {
            Some(properties) => {
                for (key, child) in properties {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(root, child, &path, out);
                }
            }
            None => out.push((prefix.to_owned(), node.clone())),
        }
    }
    let mut out = Vec::new();
    walk(schema, schema, "", &mut out);
    out
}

fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}

/// A valid value different from `current` for one key; explicit samples for keys whose
/// strings have a format the schema cannot express.
fn changed_value(path: &str, schema: &Value, current: &Value) -> Value {
    let samples: BTreeMap<&str, Value> = BTreeMap::from([
        ("daemon.listen", json!("127.0.0.1:9")),
        ("remote.public_url", json!("https://desk.example.test")),
        ("remote.push_contact", json!("mailto:me@example.test")),
        ("remote.mode", json!(["cloudflare"])),
        ("remote.relay.url", json!("https://relay.example.test")),
        ("remote.cloudflare.hostname", json!("desk.example.test")),
        (
            "otp.sites",
            json!({"https://login.example.test": r"(\d{6})"}),
        ),
        ("otp.senders", json!(["BANK"])),
        (
            "sync.webdav_url",
            json!("https://dav.example.test/openside"),
        ),
        ("channels.api_url", json!("http://127.0.0.1:9")),
        ("channels.slack.listen", json!("127.0.0.1:9")),
        ("channels.discord.listen", json!("127.0.0.1:9")),
        (
            "channels.telegram.api_base",
            json!("https://telegram.example.test"),
        ),
        (
            "channels.slack.api_base",
            json!("https://slack.example.test"),
        ),
        (
            "channels.discord.api_base",
            json!("https://discord.example.test"),
        ),
        ("channels.discord.public_key", json!("00".repeat(32))),
        ("network.allow", json!(["example.test"])),
        ("policy.allow_origins", json!(["https://ok.example.test"])),
        ("policy.deny_origins", json!(["https://no.example.test"])),
    ]);
    if let Some(sample) = samples.get(path) {
        return sample.clone();
    }
    if let Some(values) = schema["enum"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        let variants: Vec<Value> = values
            .iter()
            .filter_map(|v| {
                v.get("const")
                    .cloned()
                    .or_else(|| v.is_string().then(|| v.clone()))
            })
            .collect();
        if let Some(other) = variants.iter().find(|v| *v != current) {
            return other.clone();
        }
    }
    let kind = schema["type"].as_str().map(str::to_owned).or_else(|| {
        schema["type"]
            .as_array()
            .and_then(|t| t.iter().find(|t| *t != "null"))
            .and_then(|t| t.as_str())
            .map(str::to_owned)
    });
    match kind.as_deref() {
        Some("boolean") => json!(!current.as_bool().unwrap_or(false)),
        Some("integer") | Some("number") => {
            let minimum = schema["minimum"].as_u64().unwrap_or(1);
            json!(current.as_u64().map_or(minimum.max(1), |n| n + 1))
        }
        Some("array") => json!(["x-test"]),
        Some("object") => json!({}),
        _ if path.ends_with("dir")
            || path.ends_with("path")
            || path.ends_with("file")
            || path.ends_with("executable")
            || path.ends_with("bwrap")
            || path.ends_with("socket")
            || path.ends_with("data_dir") =>
        {
            json!("/tmp/openside-settings-test")
        }
        _ => json!("x-test"),
    }
}

fn companions(path: &str) -> Vec<(&'static str, Value)> {
    match path {
        "channels.telegram.enabled" => vec![("channels.telegram.allowed_users", json!(["1"]))],
        "channels.slack.enabled" => vec![("channels.slack.allowed_users", json!(["U1"]))],
        "sync.webdav_username" => vec![("sync.webdav_password_cmd", json!(["pass"]))],
        "sync.webdav_password_cmd" => vec![("sync.webdav_username", json!("me"))],
        "channels.discord.enabled" => vec![("channels.discord.allowed_users", json!(["1"]))],
        "remote.mode" => vec![("daemon.listen", json!("127.0.0.1:9"))],
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn every_config_schema_key_is_readable_and_writable_through_the_settings_api() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, read) = daemon
        .shared
        .db
        .pair_named("r".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    assert_eq!(
        call(&tcp, "GET", "/v1/settings/config", Value::Null, Some(&read))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let (status, document) = call(&unix, "GET", "/v1/settings/config", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{document}");
    assert_eq!(document["file_only"], json!(["credentials"]));
    let schema = openside_config::config_schema();
    let keys = leaf_keys(&schema);
    assert!(
        keys.len() > 40,
        "schema walk found only {} keys",
        keys.len()
    );
    let mut file_only = 0;
    let extension = root.path().join("test-extension");
    std::fs::create_dir(&extension).unwrap();
    for (path, key_schema) in &keys {
        if crate::settings_api::FILE_ONLY
            .iter()
            .any(|section| path == section || path.starts_with(&format!("{section}.")))
        {
            file_only += 1;
            assert!(
                document["config"].pointer(&pointer(path)).is_none(),
                "{path} must stay file-only"
            );
            continue;
        }
        let current = document["config"]
            .pointer(&pointer(path))
            .unwrap_or_else(|| panic!("{path} is not readable through GET /v1/settings/config"))
            .clone();
        let value = if path == "browser.extensions" {
            json!([extension])
        } else {
            changed_value(path, key_schema, &current)
        };
        let mut updated = document["config"].clone();
        *updated.pointer_mut(&pointer(path)).unwrap() = value.clone();
        // Cross-key rules: a key that is only valid together with another gets its partner.
        for (partner, partner_value) in companions(path) {
            *updated.pointer_mut(&pointer(partner)).unwrap() = partner_value;
        }
        let (status, written) = call(
            &unix,
            "PUT",
            "/v1/settings/config",
            json!({"config": updated}),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path} = {value} was refused: {written}"
        );
        assert_eq!(
            written["config"].pointer(&pointer(path)),
            Some(&value),
            "{path} did not round-trip"
        );
        assert_eq!(written["restart_required"], true);
        let (_, reread) = call(&unix, "GET", "/v1/settings/config", Value::Null, None).await;
        assert_eq!(
            reread["config"].pointer(&pointer(path)),
            Some(&value),
            "{path} not persisted"
        );
        // Restore so each key is checked against a valid baseline.
        let (status, _) = call(
            &unix,
            "PUT",
            "/v1/settings/config",
            json!({"config": document["config"]}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(file_only, 1);

    // Schema-checked: unknown keys, wrong types and file-only sections are refused.
    let mut unknown = document["config"].clone();
    unknown["tickets"]["typo_ttl"] = json!(5);
    assert_eq!(
        call(
            &unix,
            "PUT",
            "/v1/settings/config",
            json!({"config": unknown}),
            None
        )
        .await
        .1["code"],
        "invalid_config"
    );
    let mut zero = document["config"].clone();
    zero["tickets"]["confirm_ttl_secs"] = json!(0);
    assert_eq!(
        call(
            &unix,
            "PUT",
            "/v1/settings/config",
            json!({"config": zero}),
            None
        )
        .await
        .1["code"],
        "invalid_config"
    );
    let mut secret = document["config"].clone();
    secret["credentials"] = json!({"x": 1});
    assert_eq!(
        call(
            &unix,
            "PUT",
            "/v1/settings/config",
            json!({"config": secret}),
            None
        )
        .await
        .1["code"],
        "file_only_section"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&daemon.shared.config.loaded.paths.config_file)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    daemon.shutdown().await;
}

/// PP-15: nothing a user can reach is behind a feature flag, paywall or a disabled-by-
/// default switch. Configuration keys that turn a feature on are flags; the only allowed
/// ones are chat adapters, which cannot work before the user supplies their own bot
/// credentials. Every route answers with its own behaviour under the default
/// configuration, never with a disabled/upgrade error, and every registered tool is
/// exposed in full-access mode unless it needs a local resource that is absent.
#[tokio::test]
async fn no_route_tool_or_setting_is_behind_a_feature_flag_or_paywall() {
    let schema = openside_config::config_schema();
    let defaults = serde_json::to_value(openside_config::Config::default()).unwrap();
    let exempt = [
        "channels.telegram.enabled",
        "channels.slack.enabled",
        "channels.discord.enabled",
    ];
    for (path, _) in leaf_keys(&schema) {
        let leaf = path.rsplit('.').next().unwrap();
        let flag = leaf == "enabled"
            || leaf.ends_with("_enabled")
            || leaf.starts_with("enable_")
            || leaf.contains("feature")
            || leaf.contains("premium")
            || leaf.contains("license");
        if flag && !exempt.contains(&path.as_str()) {
            assert_ne!(
                defaults.pointer(&pointer(&path)),
                Some(&json!(false)),
                "{path} gates a feature off by default"
            );
        }
    }

    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let banned = [
        "feature_disabled",
        "not_enabled",
        "disabled",
        "upgrade_required",
        "payment_required",
        "plan_required",
        "license_required",
    ];
    for route in ROUTES {
        // Destructive or long-running commands on real state are exercised elsewhere.
        if matches!(
            route.path,
            "/v1/sync/setup" | "/v1/sync/run" | "/v1/sync/pair"
        ) {
            continue;
        }
        let path = route
            .path
            .replace("{id}", "unknown")
            .replace("{mid}", "unknown")
            .replace("{sid}", "unknown")
            .replace("{thumbnail_id}", "unknown")
            .replace("{path}", "transcript.jsonl")
            .replace("{name}", "unknown");
        let body = if route.request_schema.is_some() {
            json!({})
        } else {
            Value::Null
        };
        let response = unix
            .clone()
            .oneshot(request(route.method.as_str(), &path, body, None))
            .await
            .unwrap();
        let status = response.status();
        assert_ne!(
            status,
            StatusCode::PAYMENT_REQUIRED,
            "{} {}",
            route.method.as_str(),
            route.path
        );
        if route.sse {
            continue;
        }
        let bytes = tokio::time::timeout(Duration::from_secs(5), response.into_body().collect())
            .await
            .unwrap()
            .unwrap()
            .to_bytes();
        let code = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v["code"].as_str().map(str::to_owned))
            .unwrap_or_default();
        assert!(
            !banned.contains(&code.as_str()),
            "{} {} answered {code}",
            route.method.as_str(),
            route.path
        );
    }

    daemon.shutdown().await;
    for orchestration in [
        openside_config::Orchestration::Single,
        openside_config::Orchestration::Front,
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut config = resolved_config(root.path());
        config.loaded.config.agent.orchestration = orchestration;
        let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
        let session = create_session(&daemon, "full", PolicyMode::Full, None).await;
        let visible: Vec<String> = host(&daemon, &session)
            .await
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        for name in daemon.shared.registry.names() {
            let shown = visible.iter().any(|v| v == name);
            if crate::tasks::is_task_tool(name) || name == "delegate" {
                let expected = if name == "delegate" {
                    orchestration == openside_config::Orchestration::Single
                } else {
                    orchestration == openside_config::Orchestration::Front
                };
                assert_eq!(shown, expected, "{name} visibility in {orchestration:?}");
                continue;
            }
            let needs_absent_resource = name.starts_with("browser_")
                || name.starts_with("vault_")
                || name.starts_with("tab")
                || name == "register_authenticator";
            assert!(
                shown || needs_absent_resource || name == "bash",
                "{name} is hidden in full access without a missing resource in {orchestration:?}"
            );
        }
        daemon.shutdown().await;
    }
}

fn alive(pid: i32) -> bool {
    // A reaped or never-started child has no /proc entry; a zombie counts as gone.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
        .unwrap_or(false)
}

async fn inhibitor_pid(file: &std::path::Path, want_alive: bool) -> Option<i32> {
    for _ in 0..200 {
        if let Some(pid) = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            if alive(pid) == want_alive {
                return Some(pid);
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

/// PP-4: with `service.inhibit_sleep`, an inhibitor process is held exactly while a
/// session runs and released as soon as none does; off by default.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn sleep_inhibitor_is_held_only_while_a_session_runs() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("inhibitor.pid");
    let runtime = TestRuntime::default();
    let argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        format!("echo $$ > '{}'; exec sleep 600", pid_file.display()),
    ];
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.service.inhibit_sleep = true;
        config.service.inhibit_cmd = argv;
    })
    .await;
    let session = create_session(&daemon, "running", PolicyMode::Guard, None).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !pid_file.exists(),
        "inhibitor started with no running session"
    );

    daemon
        .shared
        .emit(&session, None, EventKind::TurnStarted {})
        .await
        .unwrap();
    let held = inhibitor_pid(&pid_file, true)
        .await
        .expect("inhibitor not started for a running session");
    daemon
        .shared
        .emit(&session, None, EventKind::SessionSettled {})
        .await
        .unwrap();
    assert_eq!(
        inhibitor_pid(&pid_file, false).await,
        Some(held),
        "inhibitor outlived the last running session"
    );

    std::fs::remove_file(&pid_file).unwrap();
    daemon
        .shared
        .emit(&session, None, EventKind::TurnStarted {})
        .await
        .unwrap();
    let first = inhibitor_pid(&pid_file, true)
        .await
        .expect("inhibitor not re-acquired");
    // An inhibitor that dies while the session still runs is replaced.
    std::fs::remove_file(&pid_file).unwrap();
    assert!(std::process::Command::new("kill")
        .arg(first.to_string())
        .status()
        .unwrap()
        .success());
    let mut again = None;
    for _ in 0..80 {
        again = inhibitor_pid(&pid_file, true)
            .await
            .filter(|pid| *pid != first);
        if again.is_some() {
            break;
        }
    }
    let again = again.expect("a dead inhibitor was not respawned");
    daemon.shutdown().await;
    assert_eq!(
        inhibitor_pid(&pid_file, false).await,
        Some(again),
        "inhibitor outlived the daemon"
    );
    assert!(!openside_config::Config::default().service.inhibit_sleep);
}

#[tokio::test]
async fn push_endpoints_must_be_known_services_or_explicitly_allowed_and_redirects_are_not_followed(
) {
    for (ip, public) in [
        ("8.8.8.8", true),
        ("2606:4700::1111", true),
        ("127.0.0.1", false),
        ("10.0.0.1", false),
        ("172.16.5.5", false),
        ("192.168.1.1", false),
        ("169.254.169.254", false),
        ("100.64.0.1", false),
        ("0.0.0.0", false),
        ("::1", false),
        ("fd00::1", false),
        ("fe80::1", false),
        ("::ffff:127.0.0.1", false),
    ] {
        assert_eq!(crate::push::public_ip(ip.parse().unwrap()), public, "{ip}");
    }

    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let target = push.subscribe();
    // A push service that answers every request with a redirect to the real fake service.
    let redirector = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_addr = redirector.local_addr().unwrap();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let location = target.endpoint.clone();
    let counter = hits.clone();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut stream, _) = redirector.accept().await.unwrap();
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buffer = vec![0_u8; 65536];
            let _ = stream.read(&mut buffer).await;
            let reply = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(reply.as_bytes()).await;
        }
    });
    let redirect_entry = format!("http://{redirect_addr}");
    let push_entry = format!("http://{}", push.addr());
    let daemon = daemon_with(root.path(), &runtime, |config| {
        // Both are allowed, so only the no-redirect rule keeps the push from being forwarded.
        config.network.allow = vec![redirect_entry, push_entry];
    })
    .await;
    let session = create_session(&daemon, "s", PolicyMode::Guard, None).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, phone) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let with_endpoint = |endpoint: &str| {
        let mut body = target.to_json();
        body["endpoint"] = json!(endpoint);
        body
    };
    for refused in [
        "https://10.0.0.1/push/x",
        "https://169.254.169.254/latest",
        "https://push.example.test/x",
        "http://fcm.googleapis.com/fcm/send/x",
        "https://user:pw@fcm.googleapis.com/fcm/send/x",
        "http://127.0.0.1:9/x",
    ] {
        let (status, body) = call(
            &tcp,
            "POST",
            "/v1/push/subscriptions",
            with_endpoint(refused),
            Some(&phone),
        )
        .await;
        assert!(
            status.is_client_error() && status != StatusCode::UNAUTHORIZED,
            "{refused}: {status} {body}"
        );
    }
    let (status, _) = call(
        &tcp,
        "POST",
        "/v1/push/subscriptions",
        with_endpoint("https://fcm.googleapis.com/fcm/send/abc"),
        Some(&phone),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a known push service is accepted"
    );
    let (_, listed) = call(
        &tcp,
        "GET",
        "/v1/push/subscriptions",
        Value::Null,
        Some(&phone),
    )
    .await;
    assert_eq!(
        listed["subscriptions"].as_array().unwrap().len(),
        1,
        "{listed}"
    );
    let (status, _) = call(
        &tcp,
        "DELETE",
        &format!(
            "/v1/push/subscriptions/{}",
            listed["subscriptions"][0]["id"].as_str().unwrap()
        ),
        Value::Null,
        Some(&phone),
    )
    .await;
    assert!(status.is_success());

    let redirecting = format!("http://{redirect_addr}/push/sub");
    let (status, _) = call(
        &tcp,
        "POST",
        "/v1/push/subscriptions",
        with_endpoint(&redirecting),
        Some(&phone),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    opened(
        &daemon,
        &session,
        TicketKind::Info,
        "r1",
        Utc::now() + chrono::Duration::minutes(10),
    )
    .await;
    for _ in 0..100 {
        if hits.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the redirecting service was tried once"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(push.deliveries().is_empty(), "the 307 was followed");
    server.abort();
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_admin_can_change_only_behaviour_settings() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, admin) = daemon
        .shared
        .db
        .pair_named("admin".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, before) = call(&unix, "GET", "/v1/settings/config", Value::Null, None).await;
    // The settings screen renders from these fields instead of a copy of the schema and rule.
    assert_eq!(before["schema"], openside_config::config_schema());
    assert_eq!(
        before["remote_editable"],
        json!(crate::settings_api::REMOTE_EDITABLE)
    );
    assert_eq!(before["remote"], false);
    let (status, remote_view) = call(
        &tcp,
        "GET",
        "/v1/settings/config",
        Value::Null,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{remote_view}");
    assert_eq!(remote_view["remote"], true);
    assert_eq!(remote_view["config"], before["config"]);

    let mut widened = before["config"].clone();
    widened["network"]["allow"] = json!(["https://exfil.example.test"]);
    let (status, body) = call(
        &tcp,
        "PUT",
        "/v1/settings/config",
        json!({"config": widened}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_socket_required"))
    );
    let (_, after) = call(&unix, "GET", "/v1/settings/config", Value::Null, None).await;
    assert_eq!(
        after["config"], before["config"],
        "a refused remote write left the file unchanged"
    );
    assert!(audit_lines(&daemon).iter().any(
        |line| line["decision"] == "refused_remote" && line["keys"] == json!(["network.allow"])
    ));

    let mut model = before["config"].clone();
    model["models"]["default"] = json!("second-model");
    let (status, body) = call(
        &tcp,
        "PUT",
        "/v1/settings/config",
        json!({"config": model}),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["config"]["models"]["default"], "second-model");
    // The local socket keeps full access.
    assert_eq!(
        call(
            &unix,
            "PUT",
            "/v1/settings/config",
            json!({"config": widened}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn revoke_never_waits_for_device_bodies_streams_or_push_and_allows_reciprocal_revoke() {
    let bound = Duration::from_secs(3);
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let daemon = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let session = create_session(&daemon, "s", PolicyMode::Guard, None).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (phone_id, phone) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let events = tcp
        .clone()
        .oneshot(request(
            "GET",
            &format!("/v1/sessions/{}/events", session.as_str()),
            Value::Null,
            Some(&phone),
        ))
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    let mut events = events.into_body();

    // A subscription request whose body is withheld forever.
    let body = serde_json::to_vec(&push.subscribe().to_json()).unwrap();
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(2);
    sender
        .send(Ok(axum::body::Bytes::copy_from_slice(
            &body[..body.len() / 2],
        )))
        .await
        .unwrap();
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    let stalled = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/push/subscriptions")
        .header("authorization", format!("Bearer {phone}"))
        .header("content-type", "application/json")
        .body(Body::from_stream(stream))
        .unwrap();
    let in_flight = tokio::spawn(tcp.clone().oneshot(stalled));
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(bound, crate::pairing::revoke(&daemon.shared, &phone_id))
        .await
        .expect("revoke waited for a stalled device request")
        .map_err(|_| "revoke failed")
        .unwrap();
    let answered = tokio::time::timeout(bound, in_flight)
        .await
        .expect("stalled request not cancelled");
    assert_eq!(
        answered.unwrap().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        ends_within(&mut events, bound).await,
        "the revoked device's stream stayed open"
    );
    let (_, listed) = call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None).await;
    assert_eq!(listed["subscriptions"], json!([]));
    assert_eq!(
        call(&tcp, "GET", "/v1/push/vapid", Value::Null, Some(&phone))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    drop(sender);

    // A device revoking itself, and two devices revoking each other, finish promptly.
    let (me_id, me) = daemon
        .shared
        .db
        .pair_named("me".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let own = tokio::time::timeout(
        bound,
        call(
            &tcp,
            "DELETE",
            &format!("/v1/devices/{me_id}"),
            Value::Null,
            Some(&me),
        ),
    )
    .await
    .expect("self-revoke deadlocked");
    assert_eq!(own.0, StatusCode::NO_CONTENT);
    assert_eq!(
        call(&tcp, "GET", "/v1/push/vapid", Value::Null, Some(&me))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (a_id, a) = daemon
        .shared
        .db
        .pair_named("a".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (b_id, b) = daemon
        .shared
        .db
        .pair_named("b".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (first, second) = tokio::time::timeout(
        bound,
        futures_util::future::join(
            call(
                &tcp,
                "DELETE",
                &format!("/v1/devices/{b_id}"),
                Value::Null,
                Some(&a),
            ),
            call(
                &tcp,
                "DELETE",
                &format!("/v1/devices/{a_id}"),
                Value::Null,
                Some(&b),
            ),
        ),
    )
    .await
    .expect("reciprocal revokes deadlocked");
    for (status, _) in [&first, &second] {
        assert!([StatusCode::NO_CONTENT, StatusCode::UNAUTHORIZED].contains(status));
    }
    let a_left = call(&tcp, "GET", "/v1/push/vapid", Value::Null, Some(&a))
        .await
        .0;
    let b_left = call(&tcp, "GET", "/v1/push/vapid", Value::Null, Some(&b))
        .await
        .0;
    assert!(a_left == StatusCode::UNAUTHORIZED || b_left == StatusCode::UNAUTHORIZED);

    // After a revoke, pushes reach only the devices that remain.
    let (laptop_id, laptop) = daemon
        .shared
        .db
        .pair_named("laptop".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, desk) = daemon
        .shared
        .db
        .pair_named("desk".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (laptop_sub, desk_sub) = (push.subscribe(), push.subscribe());
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/push/subscriptions",
            laptop_sub.to_json(),
            Some(&laptop)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/push/subscriptions",
            desk_sub.to_json(),
            Some(&desk)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert!(crate::pairing::revoke(&daemon.shared, &laptop_id)
        .await
        .is_ok());
    opened(
        &daemon,
        &session,
        TicketKind::Info,
        "after",
        Utc::now() + chrono::Duration::minutes(10),
    )
    .await;
    let deliveries = push.wait_for(1, Duration::from_secs(5)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let reached: Vec<String> = push
        .deliveries()
        .iter()
        .map(|d| d.subscriber.clone())
        .collect();
    assert_eq!(reached, vec![desk_sub.id.clone()], "{deliveries:?}");
    daemon.shutdown().await;
}

#[tokio::test]
async fn pairing_deadlines_ignore_wall_clock_and_bad_codes_are_refused_then_rate_limited() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let clock = TestClock::install(&daemon);
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);

    // The wall clock steps back an hour; three minutes of monotonic time still expire it.
    let (_, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"phone","scopes":["read"]}),
        None,
    )
    .await;
    *clock.wall.lock() = -chrono::Duration::hours(1);
    clock.at(180);
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":offer["code"]}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::GONE, Some("pairing_expired"))
    );

    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":"../../etc"}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_code"))
    );
    for _ in 1..crate::pairing::MAX_FAILURES {
        let (status, _) = call(
            &tcp,
            "POST",
            "/v1/pairing/exchange",
            json!({"code":"0".repeat(64)}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let failed = audit_lines(&daemon)
        .iter()
        .filter(|line| line["decision"] == "pairing_failed")
        .count();
    assert_eq!(failed, crate::pairing::MAX_FAILURES);
    // Guessing cannot lock out a real code: a known, unexpired code still pairs.
    let (_, offer) = call(
        &unix,
        "POST",
        "/v1/pairing",
        json!({"name":"phone","scopes":["read"]}),
        None,
    )
    .await;
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":offer["code"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Further guesses are throttled without an audit line each.
    for _ in 0..5 {
        let (status, body) = call(
            &tcp,
            "POST",
            "/v1/pairing/exchange",
            json!({"code":"1".repeat(64)}),
            None,
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::TOO_MANY_REQUESTS, Some("too_many_attempts"))
        );
    }
    let failed = audit_lines(&daemon)
        .iter()
        .filter(|line| line["decision"] == "pairing_failed")
        .count();
    assert_eq!(failed, crate::pairing::MAX_FAILURES);
    // The window is a minute.
    clock.at(241);
    let (status, _) = call(
        &tcp,
        "POST",
        "/v1/pairing/exchange",
        json!({"code":"1".repeat(64)}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    daemon.shutdown().await;
}

#[tokio::test]
async fn otp_is_refused_when_ambiguous_and_a_code_racing_a_new_step_fills_it_once() {
    const CODE: &str = "481516";
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with(root.path(), &runtime, |config| {
        config.otp.senders = vec!["BANK".into()];
        config.otp.sites = BTreeMap::from([
            ("https://example.test".into(), r"code[: ]+(\d{6})".into()),
            ("https://other.test".into(), r"pin (\d{4})".into()),
        ]);
        config.otp.ttl_secs = 60;
    })
    .await;
    let filled = Arc::new(parking_lot::Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(CanarySink {
        filled: filled.clone(),
        expected: CODE,
        mismatch: false,
    }));
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, otp) = daemon
        .shared
        .db
        .pair_named("sms forwarder".into(), vec![CapabilityScope::OtpIngest])
        .await
        .unwrap();

    let ambiguous = json!({"sender":"BANK","text":format!("code: {CODE} pin 1234")});
    let (status, body) = call(&tcp, "POST", "/v1/otp", ambiguous, Some(&otp)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("ambiguous_otp"))
    );
    assert!(audit_lines(&daemon)
        .iter()
        .any(|line| line["decision"] == "otp_rejected" && line["reason"] == "ambiguous"));

    // The step opens while an ingest is between its ticket query and its store (the
    // reconciliation lock is held): the code still fills it, exactly once.
    let held = daemon.shared.remote.otp.hold().await;
    let router = tcp.clone();
    let token = otp.clone();
    let ingest = tokio::spawn(async move {
        call(
            &router,
            "POST",
            "/v1/otp",
            json!({"sender":"BANK","text":format!("code: {CODE}")}),
            Some(&token),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "raced",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(held);
    assert_eq!(ingest.await.unwrap().0, StatusCode::OK);
    wait_event(&daemon, &session, |kind| matches!(kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id.as_str() == "raced")).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*filled.lock(), 1);
    daemon.shutdown().await;
}

#[tokio::test]
async fn revoke_waits_for_subscription_rename_and_cleanup_survives_reload_and_a_dropped_waiter() {
    let bound = Duration::from_secs(3);
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let daemon = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (device_id, token) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let path = daemon
        .shared
        .config
        .data_dir
        .join("push-subscriptions.json");
    let pause = crate::persistence::pause_before_rename(&path);
    let subscription = push.subscribe().to_json();
    let create = tokio::spawn(async move {
        call(
            &tcp,
            "POST",
            "/v1/push/subscriptions",
            subscription,
            Some(&token),
        )
        .await
    });
    tokio::time::timeout(bound, pause.reached())
        .await
        .expect("subscription never reached rename");
    let mut revoked = daemon.shared.remote.revoked.subscribe();
    let s = daemon.shared.clone();
    let id = device_id.clone();
    let mut revoke = tokio::spawn(async move { crate::pairing::revoke(&s, &id).await });
    tokio::time::timeout(
        bound,
        crate::pairing::until_revoked(&daemon.shared, &mut revoked, &device_id),
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revoke)
            .await
            .is_err(),
        "revoke overtook the admitted subscription commit"
    );
    assert!(daemon
        .shared
        .db
        .device_exists(device_id.clone())
        .await
        .unwrap());
    create.abort();
    assert!(create.await.unwrap_err().is_cancelled());
    pause.release();
    assert!(tokio::time::timeout(bound, revoke)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    assert!(!daemon.shared.db.device_exists(device_id).await.unwrap());
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        on_disk,
        json!([]),
        "old snapshot resurrected a revoked subscription"
    );
    daemon.shutdown().await;
    drop(daemon);

    // A fresh Remote/PushState must agree with the durable file, not only the old cache.
    let reloaded = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let unix = api::router(reloaded.shared.clone(), true);
    assert_eq!(
        call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None)
            .await
            .1["subscriptions"],
        json!([])
    );
    reloaded.shutdown().await;
}

#[tokio::test]
async fn revoked_settings_commit_cannot_overwrite_a_later_local_write_and_every_commit_is_audited()
{
    let bound = Duration::from_secs(3);
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (device_id, token) = daemon
        .shared
        .db
        .pair_named("admin".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, document) = call(&unix, "GET", "/v1/settings/config", Value::Null, None).await;
    let mut remote_config = document["config"].clone();
    remote_config["tickets"]["confirm_ttl_secs"] = json!(321);
    let mut local_config = remote_config.clone();
    local_config["tickets"]["confirm_ttl_secs"] = json!(654);
    local_config["network"]["allow"] = json!(["https://local-choice.example.test"]);
    let pause =
        crate::persistence::pause_before_rename(&daemon.shared.config.loaded.paths.config_file);
    let remote = tokio::spawn(async move {
        call(
            &tcp,
            "PUT",
            "/v1/settings/config",
            json!({"config": remote_config}),
            Some(&token),
        )
        .await
    });
    tokio::time::timeout(bound, pause.reached())
        .await
        .expect("settings never reached rename");
    let mut revoked = daemon.shared.remote.revoked.subscribe();
    let s = daemon.shared.clone();
    let id = device_id.clone();
    let mut revoke = tokio::spawn(async move { crate::pairing::revoke(&s, &id).await });
    tokio::time::timeout(
        bound,
        crate::pairing::until_revoked(&daemon.shared, &mut revoked, &device_id),
    )
    .await
    .unwrap();
    let local_router = unix.clone();
    let mut local = tokio::spawn(async move {
        call(
            &local_router,
            "PUT",
            "/v1/settings/config",
            json!({"config": local_config}),
            None,
        )
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revoke)
            .await
            .is_err(),
        "revoke dropped an admitted settings commit"
    );
    remote.abort();
    assert!(remote.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut local)
            .await
            .is_err(),
        "local update overtook the paused writer"
    );
    pause.release();
    assert!(tokio::time::timeout(bound, revoke)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    let (status, written) = tokio::time::timeout(bound, local).await.unwrap().unwrap();
    assert_eq!(status, StatusCode::OK, "{written}");
    let (_, final_document) = call(&unix, "GET", "/v1/settings/config", Value::Null, None).await;
    assert_eq!(final_document["config"]["tickets"]["confirm_ttl_secs"], 654);
    assert_eq!(
        final_document["config"]["network"]["allow"],
        json!(["https://local-choice.example.test"])
    );
    assert_eq!(
        audit_lines(&daemon)
            .iter()
            .filter(|line| line["tool"] == "settings" && line["decision"] == "written")
            .count(),
        2
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn resolve_committed_during_revoke_publishes_once_and_resumes_once_despite_a_dropped_waiter()
{
    let bound = Duration::from_secs(3);
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "resolve-race", PolicyMode::Guard, None).await;
    let ticket = opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "admitted",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    runtime.0.lock().ready_deferred.push(DeferredCall {
        tool_call_id: ticket.tool_call_id.clone(),
        ticket: ticket.id.clone(),
        kind: DeferKind::External,
    });
    let tcp = api::router(daemon.shared.clone(), false);
    let unix = api::router(daemon.shared.clone(), true);
    let (device_id, token) = daemon
        .shared
        .db
        .pair_named("approver".into(), vec![CapabilityScope::Approve])
        .await
        .unwrap();
    let pause = daemon.shared.db.pause_after_resolution(ticket.id.as_str());
    let mut events = daemon.shared.events.subscribe();
    let resolve = tokio::spawn(async move {
        call(
            &tcp,
            "POST",
            "/v1/tickets/admitted/resolve",
            json!({"value": true}),
            Some(&token),
        )
        .await
    });
    tokio::time::timeout(bound, pause.reached())
        .await
        .expect("resolve never reached its durable commit");
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&ticket.id].status,
        TicketStatus::Resolved
    );
    assert!(
        runtime.0.lock().resolved.is_empty(),
        "the test must pause before continuation"
    );
    assert!(
        matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "the test must pause before event publication"
    );
    let mut revoked = daemon.shared.remote.revoked.subscribe();
    let s = daemon.shared.clone();
    let id = device_id.clone();
    let mut revoke = tokio::spawn(async move { crate::pairing::revoke(&s, &id).await });
    tokio::time::timeout(
        bound,
        crate::pairing::until_revoked(&daemon.shared, &mut revoked, &device_id),
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revoke)
            .await
            .is_err(),
        "revoke interrupted a committed resolution"
    );
    resolve.abort();
    assert!(resolve.await.unwrap_err().is_cancelled());
    pause.release();
    assert!(tokio::time::timeout(bound, revoke)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    wait_event(&daemon, &session, |kind| {
        matches!(kind, EventKind::SessionSettled {})
    })
    .await;
    let mut resolved_events = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event.kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id == ticket.id)
        {
            resolved_events += 1;
        }
    }
    assert_eq!(
        resolved_events, 1,
        "durable resolve must also publish exactly once"
    );
    assert_eq!(runtime.0.lock().resolved.len(), 1);
    assert_eq!(runtime.0.lock().resolved[0].0, ticket.id);
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(state.tickets[&ticket.id].status, TicketStatus::Resolved);
    assert_eq!(state.session().status, SessionStatus::Idle);
    assert_eq!(daemon.shared.db.events(&session, 0).await.unwrap().iter().filter(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id == &ticket.id)).count(), 1);
    assert_eq!(
        call(
            &unix,
            "POST",
            "/v1/tickets/admitted/resolve",
            json!({"value":true}),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        runtime.0.lock().resolved.len(),
        1,
        "retry resumed the ticket twice"
    );
    daemon.shutdown().await;
}

struct PausedSecretSink {
    inner: CanarySink,
    filled: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl SecretSink for PausedSecretSink {
    fn fill<'a>(
        &'a self,
        ticket: &'a TicketId,
        session: &'a SessionId,
        target: &'a TicketTarget,
        value: Zeroizing<String>,
    ) -> BoxFuture<'a, std::result::Result<(), SecretSinkError>> {
        Box::pin(async move {
            self.inner.fill(ticket, session, target, value).await?;
            self.filled.notify_one();
            self.release.notified().await;
            Ok(())
        })
    }
}

#[tokio::test]
async fn revoke_during_irreversible_secret_fill_completes_resolution_and_clears_intent_once() {
    const CANARY: &str = "remote-fill-canary";
    let bound = Duration::from_secs(3);
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let filled = Arc::new(parking_lot::Mutex::new(0));
    let sink = Arc::new(PausedSecretSink {
        inner: CanarySink {
            filled: filled.clone(),
            expected: CANARY,
            mismatch: false,
        },
        filled: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    *daemon.shared.registry.secret_sink.write() = Some(sink.clone());
    let session = create_session(&daemon, "fill-race", PolicyMode::Guard, None).await;
    let ticket = opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "fill-once",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    runtime.0.lock().ready_deferred.push(DeferredCall {
        tool_call_id: ticket.tool_call_id.clone(),
        ticket: ticket.id.clone(),
        kind: DeferKind::External,
    });
    let tcp = api::router(daemon.shared.clone(), false);
    let unix = api::router(daemon.shared.clone(), true);
    let (device_id, token) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let mut events = daemon.shared.events.subscribe();
    let resolve = tokio::spawn(async move {
        call(
            &tcp,
            "POST",
            "/v1/tickets/fill-once/resolve",
            json!({"value": CANARY}),
            Some(&token),
        )
        .await
    });
    tokio::time::timeout(bound, sink.filled.notified())
        .await
        .expect("secret never reached the browser");
    assert_eq!(*filled.lock(), 1);
    assert!(daemon
        .shared
        .registry
        .browser
        .fill_intent_pending(&daemon.shared, &ticket.id)
        .await
        .unwrap());
    let mut revoked = daemon.shared.remote.revoked.subscribe();
    let s = daemon.shared.clone();
    let id = device_id.clone();
    let mut revoke = tokio::spawn(async move { crate::pairing::revoke(&s, &id).await });
    tokio::time::timeout(
        bound,
        crate::pairing::until_revoked(&daemon.shared, &mut revoked, &device_id),
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut revoke)
            .await
            .is_err(),
        "revoke abandoned an irreversible fill"
    );
    resolve.abort();
    assert!(resolve.await.unwrap_err().is_cancelled());
    sink.release.notify_one();
    assert!(tokio::time::timeout(bound, revoke)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
    wait_event(&daemon, &session, |kind| {
        matches!(kind, EventKind::SessionSettled {})
    })
    .await;
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&ticket.id].status,
        TicketStatus::Resolved
    );
    assert!(!daemon
        .shared
        .registry
        .browser
        .fill_intent_pending(&daemon.shared, &ticket.id)
        .await
        .unwrap());
    assert_eq!(*filled.lock(), 1);
    assert_eq!(runtime.0.lock().resolved.len(), 1);
    let mut published = 0;
    while let Ok(event) = events.try_recv() {
        assert!(!serde_json::to_string(&event).unwrap().contains(CANARY));
        if matches!(event.kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id == ticket.id)
        {
            published += 1;
        }
    }
    assert_eq!(published, 1);
    assert_eq!(
        call(
            &unix,
            "POST",
            "/v1/tickets/fill-once/resolve",
            json!({"value": CANARY}),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(*filled.lock(), 1, "retry repeated an irreversible fill");
    assert!(
        !std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl"))
            .unwrap()
            .contains(CANARY)
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn failed_subscription_cleanup_reloads_the_durable_candidate_and_retry_revoke_survives_restart(
) {
    let bound = Duration::from_secs(3);
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let daemon = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let unix = api::router(daemon.shared.clone(), true);
    let (device_id, token) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let subscription = push.subscribe().to_json();
    let (status, registered) = call(
        &tcp,
        "POST",
        "/v1/push/subscriptions",
        subscription,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{registered}");
    let path = daemon
        .shared
        .config
        .data_dir
        .join("push-subscriptions.json");
    let committed = std::fs::read(&path).unwrap();
    let pause = crate::persistence::pause_before_rename(&path);
    let s = daemon.shared.clone();
    let id = device_id.clone();
    let revoke = tokio::spawn(async move { crate::pairing::revoke(&s, &id).await });
    tokio::time::timeout(bound, pause.reached())
        .await
        .expect("cleanup never reached rename");
    pause.fail();
    assert!(
        tokio::time::timeout(bound, revoke)
            .await
            .unwrap()
            .unwrap()
            .is_err(),
        "failed cleanup returned success"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        committed,
        "failed rename changed the durable snapshot"
    );
    let (_, failed_state) = call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None).await;
    assert_eq!(
        failed_state["subscriptions"],
        json!([registered]),
        "cache published an uncommitted removal"
    );
    assert_eq!(
        call(&tcp, "GET", "/v1/sessions", Value::Null, Some(&token))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    assert!(
        tokio::time::timeout(bound, crate::pairing::revoke(&daemon.shared, &device_id))
            .await
            .unwrap()
            .is_ok()
    );
    let (_, retried_state) = call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None).await;
    assert_eq!(retried_state["subscriptions"], json!([]));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        json!([])
    );
    assert_eq!(
        audit_lines(&daemon)
            .iter()
            .filter(|line| line["decision"] == "revoked" && line["device_id"] == device_id)
            .count(),
        1
    );
    daemon.shutdown().await;
    drop(tcp);
    drop(unix);
    drop(daemon);

    let reloaded = daemon_with(root.path(), &runtime, allow_push(&push)).await;
    let unix = api::router(reloaded.shared.clone(), true);
    let (status, fresh_state) =
        call(&unix, "GET", "/v1/push/subscriptions", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        fresh_state["subscriptions"],
        json!([]),
        "fresh PushState resurrected the revoked subscription"
    );
    reloaded.shutdown().await;
}
