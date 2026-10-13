//! `request_otp` end to end against a real Chromium: a login page that checks a 6-digit TOTP
//! server-side (register the authenticator once, then sign in again with no ticket) and one
//! that checks an emailed code typed into six one-character boxes. Neither the setup key nor
//! a code reaches events, the data directory, what the agent is told, or a snapshot.

use std::{collections::HashMap, time::Duration};

use axum::{extract::Form, response::Html, routing::get, Router};
use hmac::{Hmac, Mac};
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Totp, Vault};

use super::browser::{
    assert_private, assert_snapshot_hides_values, browser_config, model, open_tab, reference,
    resolve_value, string_values, tool, BrowserCleanup,
};
use super::*;

/// RFC 6238's SHA-256 test key, and the same bytes in base32.
const KEY: &[u8] = b"12345678901234567890123456789012";
const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";
const EMAIL_CODE: &str = "Q7K2XM";

fn otpauth() -> String {
    format!("otpauth://totp/Fixture:alice?secret={SEED}&algorithm=SHA256&digits=6&period=30&issuer=Fixture")
}

/// What the fixture server computes on its own, independently of the vault.
fn totp_at(unix: u64) -> String {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(KEY).unwrap();
    mac.update(&(unix / 30).to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let value = u32::from_be_bytes(digest[offset..offset + 4].try_into().unwrap()) & 0x7fff_ffff;
    format!("{:06}", value % 1_000_000)
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

const TOTP_PAGE: &str = r#"<!doctype html><title>Two-step verification</title>
<form method='post' action='/totp'><p>Enter the 6-digit code from your authenticator app.</p>
<label>Authentication code<input name='code' inputmode='numeric' autocomplete='one-time-code' maxlength='6'></label>
<button>Verify</button></form>"#;

const EMAIL_PAGE: &str = r#"<!doctype html><title>Check your email</title>
<form method='post' action='/email'><p>We emailed you a 6-character code.</p>
<label>Email code 1<input name='d1' maxlength='1'></label><label>Email code 2<input name='d2' maxlength='1'></label>
<label>Email code 3<input name='d3' maxlength='1'></label><label>Email code 4<input name='d4' maxlength='1'></label>
<label>Email code 5<input name='d5' maxlength='1'></label><label>Email code 6<input name='d6' maxlength='1'></label>
<button>Verify</button></form>"#;

/// The authenticator form with a second field holding plain text the page set itself.
const TOTP_NOTE_PAGE: &str = r#"<!doctype html><title>Two-step verification</title>
<form method='post' action='/totp'><label>Authentication code<input name='code' inputmode='numeric' autocomplete='one-time-code' maxlength='6'></label>
<label>Note<input name='note' value='visible-note-4471'></label><button>Verify</button></form>"#;
const NOTE: &str = "visible-note-4471";

fn outcome(ok: bool) -> Html<&'static str> {
    Html(if ok {
        "<!doctype html><title>Signed in</title><h1>Welcome back</h1>"
    } else {
        "<!doctype html><title>Wrong code</title><h1>Wrong code</h1>"
    })
}

/// A 2FA enrollment page showing the setup key only as a QR code generated now, with a
/// confirmation-code field the server checks.
fn setup_qr_page() -> String {
    let svg = qrcode::QrCode::new(otpauth().as_bytes())
        .unwrap()
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build();
    format!(
        "<!doctype html><title>Set up two-step verification</title>\
         <p>Scan this code with your authenticator app.</p><div>{svg}</div>\
         <form method='post' action='/setup'><label>Confirmation code<input name='code' \
         inputmode='numeric' maxlength='6'></label><button>Verify</button></form>"
    )
}

/// The same step for people who cannot scan: the key as grouped text only.
fn setup_text_page() -> String {
    let grouped: Vec<String> = SEED
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8(chunk.to_vec()).unwrap())
        .collect();
    format!(
        "<!doctype html><title>Set up two-step verification</title>\
         <p>Can't scan? Enter this key in your app: <code>{}</code></p>",
        grouped.join(" ")
    )
}

/// A second, different authenticator key.
const OTHER_SEED: &str = "JBSWY3DPEHPK3PXP";

/// A setup page offering two different authenticators as links, with this site's key also
/// printed as bare text outside any link.
fn setup_ambiguous_page() -> String {
    format!(
        "<!doctype html><title>Set up two-step verification</title>\
         <p><a href='{}'>Add to authenticator</a> \
         <a href='otpauth://totp/Other:bob?secret={OTHER_SEED}&issuer=Other'>Add the other one</a></p>\
         <p>Or enter this key: <code>{SEED}</code></p>",
        otpauth()
    )
}

async fn site() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let check_totp = |Form(form): Form<HashMap<String, String>>| async move {
        let code = form.get("code").cloned().unwrap_or_default();
        let now = now();
        // One step of clock drift either way, as real verifiers allow.
        outcome(
            [now - 30, now, now + 30]
                .iter()
                .any(|t| totp_at(*t) == code),
        )
    };
    let qr = setup_qr_page();
    let text = setup_text_page();
    let ambiguous = setup_ambiguous_page();
    let router = Router::new()
        .route("/totp", get(|| async { Html(TOTP_PAGE) }).post(check_totp))
        .route("/totp-note", get(|| async { Html(TOTP_NOTE_PAGE) }))
        .route("/setup-qr", get(move || async move { Html(qr) }))
        .route("/setup-text", get(move || async move { Html(text) }))
        .route(
            "/setup-ambiguous",
            get(move || async move { Html(ambiguous) }),
        )
        .route("/setup", axum::routing::post(check_totp))
        .route(
            "/email",
            get(|| async { Html(EMAIL_PAGE) }).post(
                |Form(form): Form<HashMap<String, String>>| async move {
                    let code: String = (1..=6)
                        .filter_map(|i| form.get(&format!("d{i}")).cloned())
                        .collect();
                    outcome(code == EMAIL_CODE)
                },
            ),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

/// Chromium and a passphrase vault holding `entries`.
pub(super) fn otp_config(root: &Path, entries: Vec<Entry>) -> Option<Resolved> {
    let mut config = browser_config(root)?;
    let path = root.join("vault.db");
    let mut vault = Vault::create(
        &path,
        root.join("setup-audit.jsonl"),
        &Secret::new("fixture-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    for entry in entries {
        vault.put(entry).unwrap();
    }
    drop(vault);
    let pass_file = root.join("passphrase.txt");
    std::fs::write(&pass_file, "fixture-passphrase\n").unwrap();
    config.loaded.config.vault.path = Some(path);
    config.loaded.config.vault.passphrase_cmd =
        Some(vec!["cat".into(), pass_file.to_string_lossy().into_owned()]);
    // The shared browser fixture shortens approvals to two seconds for tests that wait on
    // expiry. Authenticator flows span daemon restarts — far longer under load — so restore
    // the product lifetime; expiry is not what these tests exercise.
    config.loaded.config.tickets.confirm_ttl_secs = 600;
    Some(config)
}

/// Daemon with Chromium and an empty passphrase vault.
async fn otp_daemon(root: &Path) -> Option<(Daemon, TestRuntime)> {
    otp_daemon_with(root, Vec::new()).await
}

async fn otp_daemon_with(root: &Path, entries: Vec<Entry>) -> Option<(Daemon, TestRuntime)> {
    let config = otp_config(root, entries)?;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    Some((daemon, runtime))
}

/// A TOTP-only vault item for `origin`.
fn saved_authenticator(origin: &str, site: &str, seed: &str) -> Entry {
    Entry::new(
        site.into(),
        vec![AllowedOrigin::parse(origin).unwrap()],
        Secret::new(String::new()),
        Secret::new(String::new()),
        Some(Totp::parse(Secret::new(seed.into())).unwrap()),
        AccessPolicy::Always,
    )
    .unwrap()
}

pub(super) async fn live(daemon: &Daemon) -> Arc<openside_browser::Browser> {
    daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap()
}

/// What the page holds in its code field, read past every agent-facing filter.
async fn code_value(daemon: &Daemon, tab: &str) -> String {
    live(daemon)
        .await
        .evaluate(tab, "document.querySelector('[name=code]').value")
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

pub(super) async fn title(daemon: &Daemon, tab: &str) -> String {
    live(daemon)
        .await
        .evaluate(tab, "document.title")
        .await
        .unwrap()
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// Click Verify as the agent would and wait for the server's verdict.
async fn verify(daemon: &Daemon, session: &SessionId, tab: &str, label: &str) -> String {
    let button = reference(
        daemon,
        session,
        tab,
        "Verify",
        &format!("{label}-verify-ref"),
    )
    .await;
    model(
        tool(
            daemon,
            session,
            "browser_act",
            json!({"tab_id":tab,"target":button,"action":"click"}),
            &format!("{label}-verify"),
        )
        .await,
    );
    for _ in 0..100 {
        let title = title(daemon, tab).await;
        if title == "Signed in" || title == "Wrong code" {
            return title;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the fixture never answered {label}");
}

/// A deferring browser tool must never wait on the browser gate it already holds (opening
/// its ticket pins the field under that gate), so a hang fails here instead of stalling.
pub(super) async fn bounded(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> ToolOutcome {
    tokio::time::timeout(
        Duration::from_secs(60),
        tool(daemon, session, name, args, id),
    )
    .await
    .unwrap_or_else(|_| panic!("{name} deadlocked"))
}

async fn request_otp(daemon: &Daemon, session: &SessionId, args: Value, id: &str) -> ToolOutcome {
    bounded(daemon, session, "request_otp", args, id).await
}

pub(super) async fn ticket(daemon: &Daemon, id: &TicketId) -> Value {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "GET",
            &format!("/v1/tickets/{id}"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    json_body(response).await
}

/// Every string the agent was handed back, ticket resumptions included.
pub(super) fn agent_saw(runtime: &TestRuntime) -> String {
    format!("{:?}", runtime.0.lock().resolved)
}

#[tokio::test]
async fn request_otp_registers_an_authenticator_once_then_fills_without_asking() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-register", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/totp"), "otp-open").await;
    let field = reference(&daemon, &session, &tab, "Authentication code", "otp-ref").await;

    // No authenticator yet: one ticket offering registration, bound to the tab's origin.
    let ToolOutcome::Deferred {
        ticket: id,
        kind: DeferKind::External,
    } = request_otp(
        &daemon,
        &session,
        json!({"tab_id":tab,"field_ref":field,"channel":"authenticator"}),
        "otp-ask",
    )
    .await
    else {
        panic!("expected a ticket");
    };
    let opened = ticket(&daemon, &id).await;
    assert_eq!(opened["kind"], "secret_input");
    assert_eq!(
        opened["otp"],
        json!({"channel":"authenticator","can_register":true})
    );
    assert_eq!(opened["target"]["origin"], json!(origin));
    assert_eq!(opened["target"]["field_ref"], json!(field));

    // A malformed authenticator is refused before anything is touched; the ticket stays open.
    for bad in [
        json!({"totp":"otpauth://hotp/x?secret=GEZDGNBV"}),
        json!({"totp":"not base32!"}),
        json!({"seed":SEED}),
    ] {
        let response = resolve_value(&daemon, &id, bad).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    }

    // Keeping an authenticator takes both scopes: HumanStep for the code ticket and Approve
    // for the permanent factor. Either alone is refused before anything is touched.
    for scopes in [
        vec![CapabilityScope::HumanStep],
        vec![CapabilityScope::Approve],
    ] {
        let (_, token) = daemon.shared.db.pair(scopes.clone()).await.unwrap();
        let (status, _) = resolve_as(&daemon, &token, &id, json!({"totp":otpauth()})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{scopes:?}");
        assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    }
    assert!(!vault_audit(&daemon)
        .await
        .to_string()
        .contains("otp_register"));
    let (device, both) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep, CapabilityScope::Approve])
        .await
        .unwrap();
    let (status, resolved) = resolve_as(&daemon, &both, &id, json!({"totp":otpauth()})).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(
        resolved["resolution"],
        json!({"status":"filled","registered":true})
    );
    let registered = vault_audit(&daemon).await["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["what"] == "otp_register" && record["result"] == "registered")
        .cloned()
        .expect("registration audited");
    assert_eq!(registered["who"], json!(format!("device:{device}")));
    assert_snapshot_hides_values(&daemon, &session, &tab, &totp_at(now())).await;
    assert_eq!(verify(&daemon, &session, &tab, "first").await, "Signed in");

    // The next login needs nobody: the vault fills the current code and no ticket opens.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/totp")}),
            "otp-again",
        )
        .await,
    );
    let field = reference(&daemon, &session, &tab, "Authentication code", "otp-ref-2").await;
    let filled = model(
        request_otp(
            &daemon,
            &session,
            json!({"tab_id":tab,"field_ref":field}),
            "otp-saved",
        )
        .await,
    );
    assert_eq!(filled, json!({"status":"filled"}));
    assert_eq!(verify(&daemon, &session, &tab, "second").await, "Signed in");
    let tickets = daemon.shared.db.events(&session, 0).await.unwrap();
    let opened = tickets
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TicketOpened { .. }))
        .count();
    assert_eq!(opened, 1, "the saved authenticator opened no second ticket");

    // The setup key and the codes stay out of events, files, snapshots and the agent's view.
    let code = totp_at(now());
    assert_private(&daemon, &runtime, &session, SEED).await;
    assert!(!agent_saw(&runtime).contains(&code));
    let events = serde_json::to_value(&tickets).unwrap();
    let mut seen = Vec::new();
    string_values(&events, &mut seen);
    for t in [now() - 30, now(), now() + 30] {
        let code = totp_at(t);
        assert!(
            !seen.iter().any(|text| text.contains(&code)),
            "an event carries a code"
        );
    }
    let (status, audit) = {
        let response = api::router(daemon.shared.clone(), true)
            .oneshot(request("GET", "/v1/vault/audit", json!({}), None))
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    };
    assert_eq!(status, StatusCode::OK);
    assert!(!audit.to_string().contains(SEED));
    let actions: Vec<(String, String)> = audit["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                record["what"].as_str().unwrap().to_owned(),
                record["result"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        actions.contains(&("otp_register".into(), "registered".into())),
        "{actions:?}"
    );
    assert_eq!(
        actions
            .iter()
            .filter(|(what, decision)| what == "otp" && decision == "filled")
            .count(),
        2,
        "{actions:?}"
    );
    daemon.shutdown().await;
    fixture.abort();
}

#[tokio::test]
async fn request_otp_types_an_emailed_code_into_split_boxes_without_the_agent_seeing_it() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-email", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/email"), "email-open").await;
    let field = reference(&daemon, &session, &tab, "Email code 1", "email-ref").await;
    let ToolOutcome::Deferred {
        ticket: id,
        kind: DeferKind::External,
    } = request_otp(
        &daemon,
        &session,
        json!({"tab_id":tab,"field_ref":field,"channel":"email","summary":"Code from the sign-in email"}),
        "email-ask",
    )
    .await
    else {
        panic!("expected a ticket");
    };
    let opened = ticket(&daemon, &id).await;
    assert_eq!(opened["kind"], "secret_input");
    assert_eq!(opened["summary"], "Code from the sign-in email");
    assert_eq!(
        opened["otp"],
        json!({"channel":"email","can_register":false})
    );
    // An emailed code cannot be answered with an authenticator.
    let response = resolve_value(&daemon, &id, json!({"totp":otpauth()})).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = resolve_value(&daemon, &id, json!(EMAIL_CODE)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"status":"filled"})
    );
    assert_snapshot_hides_values(&daemon, &session, &tab, EMAIL_CODE).await;
    assert_eq!(verify(&daemon, &session, &tab, "email").await, "Signed in");
    let resumed = agent_saw(&runtime);
    assert!(resumed.contains("filled"), "{resumed}");
    assert_private(&daemon, &runtime, &session, EMAIL_CODE).await;
    daemon.shutdown().await;
    fixture.abort();
}

pub(super) async fn resolve_as(
    daemon: &Daemon,
    token: &str,
    id: &TicketId,
    value: Value,
) -> (StatusCode, Value) {
    let response = api::router(daemon.shared.clone(), false)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{id}/resolve"),
            json!({"value":value}),
            Some(token),
        ))
        .await
        .unwrap();
    let status = response.status();
    (status, json_body(response).await)
}

pub(super) async fn vault_audit(daemon: &Daemon) -> Value {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request("GET", "/v1/vault/audit", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

#[tokio::test]
async fn register_authenticator_reads_a_setup_qr_code_and_keeps_it_only_once_approved() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-setup-qr", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/setup-qr"),
        "setup-open",
    )
    .await;
    let field = reference(&daemon, &session, &tab, "Confirmation code", "setup-ref").await;

    let ToolOutcome::Deferred { ticket: id, .. } = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab,"field_ref":field}),
        "setup-qr",
    )
    .await
    else {
        panic!("expected one approval ticket");
    };
    let opened = ticket(&daemon, &id).await;
    assert_eq!(opened["kind"], "confirm");
    assert_eq!(
        opened["otp_register"],
        json!({"origin":origin,"issuer":"Fixture","account":"alice"})
    );
    assert_eq!(opened["target"]["field_ref"], json!(field));
    assert!(!opened.to_string().contains(SEED));
    // The page now shows a secret: pixels and scripts are refused on it.
    assert!(matches!(
        bounded(
            &daemon,
            &session,
            "browser_screenshot",
            json!({"tab_id":tab}),
            "setup-shot"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));

    // Saving a factor is an approval: a human-step-only device cannot decide it.
    let (_, human) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let (device, approver) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Approve])
        .await
        .unwrap();
    let (status, _) = resolve_as(&daemon, &human, &id, json!(true)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    let (status, resolved) = resolve_as(&daemon, &approver, &id, json!(true)).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(
        resolved["resolution"],
        json!({"status":"registered","filled":true})
    );
    assert_eq!(verify(&daemon, &session, &tab, "setup").await, "Signed in");

    // Later code prompts on this origin need nobody.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/totp")}),
            "setup-login",
        )
        .await,
    );
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "setup-login-ref",
    )
    .await;
    assert_eq!(
        model(
            request_otp(
                &daemon,
                &session,
                json!({"tab_id":tab,"field_ref":field}),
                "setup-otp"
            )
            .await
        ),
        json!({"status":"filled"})
    );
    assert_eq!(
        verify(&daemon, &session, &tab, "setup-login").await,
        "Signed in"
    );

    // A registered site never asks again, nor is its authenticator replaced.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/setup-qr")}),
            "setup-again",
        )
        .await,
    );
    let again = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab}),
        "setup-qr-2",
    )
    .await;
    assert!(
        matches!(&again, ToolOutcome::Denied { reason, .. } if reason == "otp_already_registered"),
        "{again:?}"
    );

    let audit = vault_audit(&daemon).await;
    assert!(!audit.to_string().contains(SEED));
    let registered = audit["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["what"] == "otp_register" && record["result"] == "registered")
        .expect("registration audited");
    assert_eq!(registered["who"], json!(format!("device:{device}")));
    assert_private(&daemon, &runtime, &session, SEED).await;
    assert!(!agent_saw(&runtime).contains("otpauth://"));
    daemon.shutdown().await;
    fixture.abort();
}

#[tokio::test]
async fn a_declined_text_setup_key_is_discarded_and_codes_are_still_asked_for() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-setup-text", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/setup-text"),
        "text-open",
    )
    .await;
    let ToolOutcome::Deferred { ticket: id, .. } = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab}),
        "setup-text",
    )
    .await
    else {
        panic!("expected one approval ticket");
    };
    let opened = ticket(&daemon, &id).await;
    assert_eq!(
        opened["otp_register"],
        json!({"origin":origin,"issuer":null,"account":null})
    );
    let response = resolve_value(&daemon, &id, json!(false)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"approved":false,"status":"declined"})
    );
    // Nothing was kept: the vault holds no authenticator, so a code prompt asks the person.
    assert!(!vault_audit(&daemon)
        .await
        .to_string()
        .contains("otp_register"));
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/totp")}),
            "text-login",
        )
        .await,
    );
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "text-login-ref",
    )
    .await;
    assert!(matches!(
        request_otp(
            &daemon,
            &session,
            json!({"tab_id":tab,"field_ref":field}),
            "text-otp"
        )
        .await,
        ToolOutcome::Deferred { .. }
    ));
    // Neither the key nor any grouped form of it reached any surface.
    assert_private(&daemon, &runtime, &session, SEED).await;
    assert_private(&daemon, &runtime, &session, &SEED[..16]).await;
    assert_private(&daemon, &runtime, &session, "GEZD GNBV").await;
    daemon.shutdown().await;
    fixture.abort();
}

async fn navigate(daemon: &Daemon, session: &SessionId, tab: &str, url: &str, id: &str) {
    model(
        tool(
            daemon,
            session,
            "browser_navigate",
            json!({"tab_id":tab,"url":url}),
            id,
        )
        .await,
    );
}

async fn reject(daemon: &Daemon, id: &TicketId) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{id}/reject"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// `(what, result)` of every vault audit record.
async fn audit_actions(daemon: &Daemon) -> Vec<(String, String)> {
    vault_audit(daemon).await["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                record["what"].as_str().unwrap().to_owned(),
                record["result"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn snapshot_denied(outcome: &ToolOutcome) -> bool {
    matches!(outcome, ToolOutcome::Denied { reason, .. } if reason == "snapshot_unavailable_after_secret_fill")
}

/// Secret-fill taint belongs to the document, not its URL: a page that moves itself to another
/// URL with history or a hash after a code was filled keeps every field masked, live and once
/// the daemon restarts without the live filters.
#[tokio::test]
async fn a_filled_code_stays_masked_across_same_document_url_changes_and_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let (origin, fixture) = site().await;
    let Some(config) = otp_config(
        root.path(),
        vec![saved_authenticator(&origin, "fixture", &otpauth())],
    ) else {
        return;
    };
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config.clone(), runtime.clone())
        .await
        .unwrap();
    let session = create_session(&daemon, "otp-taint", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/totp-note"),
        "taint-open",
    )
    .await;
    let field = reference(&daemon, &session, &tab, "Authentication code", "taint-ref").await;
    let before = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "taint-before",
        )
        .await,
    );
    assert!(
        before.to_string().contains(NOTE),
        "snapshots show field values: {before}"
    );
    assert_eq!(
        model(
            request_otp(
                &daemon,
                &session,
                json!({"tab_id":tab,"field_ref":field}),
                "taint-fill"
            )
            .await
        ),
        json!({"status":"filled"})
    );
    let code = code_value(&daemon, &tab).await;
    assert_eq!(code.len(), 6);

    let browser = live(&daemon).await;
    let document = browser.document_id(&tab).await.unwrap();
    let lifetime = browser.document_lifetime_id(&tab).await.unwrap();
    assert_eq!(
        browser
            .evaluate(&tab, "history.pushState({}, '', '/signed-in'); location.hash = 'done'; location.pathname + location.hash")
            .await
            .unwrap(),
        json!("/signed-in#done")
    );
    assert_ne!(
        browser.document_id(&tab).await.unwrap(),
        document,
        "the URL changed"
    );
    assert_eq!(
        browser.document_lifetime_id(&tab).await.unwrap(),
        lifetime,
        "the document did not"
    );
    let after = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "taint-after",
        )
        .await,
    )
    .to_string();
    assert!(!after.contains(NOTE) && !after.contains(&code), "{after}");

    // A pending ticket keeps the browser and tab alive across the restart.
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "taint-ref-2",
    )
    .await;
    assert!(matches!(
        request_otp(
            &daemon,
            &session,
            json!({"tab_id":tab,"field_ref":field,"channel":"email"}),
            "taint-pending"
        )
        .await,
        ToolOutcome::Deferred { .. }
    ));
    let _cleanup = BrowserCleanup(Some(browser.process_id()));
    drop(browser);
    daemon.shutdown().await;
    drop(daemon);

    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let outcome = tool(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab}),
        "taint-recovered",
    )
    .await;
    assert!(snapshot_denied(&outcome), "{outcome:?}");
    live(&daemon)
        .await
        .evaluate(
            &tab,
            "history.pushState({}, '', '/elsewhere'); location.hash = 'again'; true",
        )
        .await
        .unwrap();
    let outcome = tool(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab}),
        "taint-recovered-moved",
    )
    .await;
    assert!(snapshot_denied(&outcome), "{outcome:?}");
    assert!(!agent_saw(&runtime).contains(&code));
    let events = serde_json::to_value(daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    let mut seen = Vec::new();
    string_values(&events, &mut seen);
    assert!(!seen.iter().any(|text| text.contains(&code)));
    daemon.shutdown().await;
    fixture.abort();
}

/// The OTP tools share the takeover refusal of every browser tool: a takeover still being
/// published already holds the tab.
#[tokio::test]
async fn otp_tools_are_refused_while_a_takeover_of_their_tab_is_being_opened() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "otp-reserve", PolicyMode::Full, None).await;
    let shared = &daemon.shared;
    let open = json!({"kind":"takeover","summary":"CAPTCHA","tab_id":"tab-1"});
    let reservation = crate::takeover::reserve(shared, &session, "request_human_step", &open).await;
    assert!(reservation.is_some());
    let calls = |tab: &str| {
        [
            ("request_otp", json!({"tab_id":tab,"field_ref":"e1"})),
            ("register_authenticator", json!({"tab_id":tab})),
        ]
    };
    for (name, args) in calls("tab-1") {
        let refusal = crate::takeover::blocked(shared, &session, name, &args).await;
        assert_eq!(
            refusal.map(|(reason, _)| reason),
            Some("takeover_active"),
            "{name}"
        );
    }
    for (name, args) in calls("tab-2") {
        assert!(
            crate::takeover::blocked(shared, &session, name, &args)
                .await
                .is_none(),
            "{name}"
        );
    }
    drop(reservation);
    for (name, args) in calls("tab-1") {
        assert!(
            crate::takeover::blocked(shared, &session, name, &args)
                .await
                .is_none(),
            "{name}"
        );
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn request_otp_asks_when_an_origin_has_several_accounts_and_never_runs_under_takeover_or_incognito(
) {
    let root = tempfile::tempdir().unwrap();
    let (origin, fixture) = site().await;
    let Some((daemon, _runtime)) = otp_daemon_with(
        root.path(),
        vec![
            saved_authenticator(&origin, "alice", &otpauth()),
            saved_authenticator(&origin, "bob", OTHER_SEED),
        ],
    )
    .await
    else {
        return;
    };

    // An incognito session never reaches the vault, not even to look for an authenticator.
    let private = SessionId::new("otp-private");
    let stored = private.clone();
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session_with_privacy(
                stored,
                "Private",
                std::any::type_name::<TestRuntime>(),
                PolicyMode::Full,
                None,
                true,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    for (name, args) in [
        ("request_otp", json!({"tab_id":"tab-1","field_ref":"e1"})),
        ("register_authenticator", json!({"tab_id":"tab-1"})),
    ] {
        let outcome = tool(&daemon, &private, name, args, &format!("private-{name}")).await;
        assert!(
            matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "vault_incognito"),
            "{name}: {outcome:?}"
        );
    }
    assert!(!vault_audit(&daemon)
        .await
        .to_string()
        .contains("otp-private"));

    // Two accounts hold an authenticator for this origin: none is picked, the person types
    // the code of the one they mean, and registering yet another is not offered.
    let session = create_session(&daemon, "otp-accounts", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/totp"),
        "accounts-open",
    )
    .await;
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "accounts-ref",
    )
    .await;
    let ToolOutcome::Deferred { ticket: id, .. } = request_otp(
        &daemon,
        &session,
        json!({"tab_id":tab,"field_ref":field}),
        "accounts-ask",
    )
    .await
    else {
        panic!("expected a ticket");
    };
    assert_eq!(ticket(&daemon, &id).await["otp"]["can_register"], false);
    assert_eq!(code_value(&daemon, &tab).await, "");
    let response = resolve_value(&daemon, &id, json!({"totp":otpauth()})).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        !audit_actions(&daemon)
            .await
            .iter()
            .any(|(what, _)| what.starts_with("otp")),
        "no account was used or added"
    );
    let response = resolve_value(&daemon, &id, json!(totp_at(now()))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        verify(&daemon, &session, &tab, "accounts").await,
        "Signed in"
    );

    // While a person controls the tab, neither tool runs; once the takeover ends they do.
    navigate(
        &daemon,
        &session,
        &tab,
        &format!("{origin}/totp"),
        "accounts-again",
    )
    .await;
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "accounts-ref-2",
    )
    .await;
    let ToolOutcome::Deferred {
        ticket: takeover, ..
    } = tool(
        &daemon,
        &session,
        "request_human_step",
        json!({"kind":"takeover","summary":"Solve the CAPTCHA","tab_id":tab}),
        "accounts-takeover",
    )
    .await
    else {
        panic!("expected a takeover ticket");
    };
    for (name, args) in [
        ("request_otp", json!({"tab_id":tab,"field_ref":field})),
        ("register_authenticator", json!({"tab_id":tab})),
    ] {
        let outcome = bounded(&daemon, &session, name, args, &format!("takeover-{name}")).await;
        assert!(
            matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "takeover_active"),
            "{name}: {outcome:?}"
        );
    }
    reject(&daemon, &takeover).await;
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "accounts-ref-3",
    )
    .await;
    let ToolOutcome::Deferred { ticket: id, .. } = request_otp(
        &daemon,
        &session,
        json!({"tab_id":tab,"field_ref":field}),
        "accounts-after",
    )
    .await
    else {
        panic!("expected a ticket once the takeover ended");
    };
    reject(&daemon, &id).await;
    daemon.shutdown().await;
    fixture.abort();
}

/// Once the vault has saved an authenticator, a failed completion audit ends the ticket as
/// registered but unfilled rather than leaving it open over a saved factor.
#[tokio::test]
async fn a_saved_authenticator_whose_completion_audit_fails_ends_the_ticket_unfilled() {
    let root = tempfile::tempdir().unwrap();
    let (origin, fixture) = site().await;
    // Audit records are redacted against every vault secret and must stay valid JSON, so a
    // password that is the quoted word "registered" makes exactly the completion record of a
    // registration fail closed, after the factor is saved.
    let unrelated = Entry::new(
        "unrelated".into(),
        vec![AllowedOrigin::parse("https://example.com").unwrap()],
        Secret::new(String::new()),
        Secret::new("\"registered\"".into()),
        None,
        AccessPolicy::Always,
    )
    .unwrap();
    let Some((daemon, runtime)) = otp_daemon_with(root.path(), vec![unrelated]).await else {
        return;
    };
    let session = create_session(&daemon, "otp-audit", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/totp"), "audit-open").await;
    let field = reference(&daemon, &session, &tab, "Authentication code", "audit-ref").await;
    let ToolOutcome::Deferred { ticket: id, .. } = request_otp(
        &daemon,
        &session,
        json!({"tab_id":tab,"field_ref":field}),
        "audit-ask",
    )
    .await
    else {
        panic!("expected a ticket");
    };
    let response = resolve_value(&daemon, &id, json!({"totp":otpauth()})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let resolved = json_body(response).await;
    assert_eq!(
        resolved["resolution"],
        json!({"status":"registered","filled":false})
    );
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(code_value(&daemon, &tab).await, "");
    let actions = audit_actions(&daemon).await;
    assert!(
        actions.contains(&("otp_register".into(), "allowed".into())),
        "{actions:?}"
    );
    assert!(
        !actions.contains(&("otp_register".into(), "registered".into())),
        "{actions:?}"
    );

    // The factor was kept: the next prompt is filled from the vault without asking.
    navigate(
        &daemon,
        &session,
        &tab,
        &format!("{origin}/totp"),
        "audit-again",
    )
    .await;
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "audit-ref-2",
    )
    .await;
    assert_eq!(
        model(
            request_otp(
                &daemon,
                &session,
                json!({"tab_id":tab,"field_ref":field}),
                "audit-saved"
            )
            .await
        ),
        json!({"status":"filled"})
    );
    assert_eq!(verify(&daemon, &session, &tab, "audit").await, "Signed in");
    assert_private(&daemon, &runtime, &session, SEED).await;
    daemon.shutdown().await;
    fixture.abort();
}

/// A setup key is kept only from the enrollment document it was read from; a page that only
/// moved its URL keeps the key, but its pinned confirmation field is stale, so nothing is
/// filled and later prompts use the vault.
#[tokio::test]
async fn register_authenticator_keeps_nothing_from_a_replaced_page_and_fills_nothing_into_a_moved_one(
) {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-stale", PolicyMode::Full, None).await;

    // Another origin of the same fixture, so the second half can still register.
    let replaced = origin.replace("localhost", "127.0.0.1");
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{replaced}/setup-qr"),
        "stale-open",
    )
    .await;
    let ToolOutcome::Deferred { ticket: id, .. } = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab}),
        "stale-read",
    )
    .await
    else {
        panic!("expected one approval ticket");
    };
    navigate(
        &daemon,
        &session,
        &tab,
        &format!("{replaced}/setup-qr"),
        "stale-reload",
    )
    .await;
    let response = resolve_value(&daemon, &id, json!(true)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "secret_input_failed");
    assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    assert!(!vault_audit(&daemon)
        .await
        .to_string()
        .contains("otp_register"));
    let response = resolve_value(&daemon, &id, json!(false)).await;
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"approved":false,"status":"declined"})
    );

    navigate(
        &daemon,
        &session,
        &tab,
        &format!("{origin}/setup-qr"),
        "moved-open",
    )
    .await;
    let field = reference(&daemon, &session, &tab, "Confirmation code", "moved-ref").await;
    let ToolOutcome::Deferred { ticket: id, .. } = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab,"field_ref":field}),
        "moved-read",
    )
    .await
    else {
        panic!("expected one approval ticket");
    };
    // While a person controls the tab, nothing is kept; the key waits for the takeover to end.
    let ToolOutcome::Deferred {
        ticket: takeover, ..
    } = tool(
        &daemon,
        &session,
        "request_human_step",
        json!({"kind":"takeover","summary":"Solve the CAPTCHA","tab_id":tab}),
        "moved-takeover",
    )
    .await
    else {
        panic!("expected a takeover ticket");
    };
    let response = resolve_value(&daemon, &id, json!(true)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    assert!(!vault_audit(&daemon)
        .await
        .to_string()
        .contains("otp_register"));
    reject(&daemon, &takeover).await;
    live(&daemon)
        .await
        .evaluate(&tab, "history.pushState({}, '', '/setup-qr/confirm'); true")
        .await
        .unwrap();
    let response = resolve_value(&daemon, &id, json!(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"status":"registered","filled":false})
    );
    assert_eq!(code_value(&daemon, &tab).await, "");

    navigate(
        &daemon,
        &session,
        &tab,
        &format!("{origin}/totp"),
        "moved-login",
    )
    .await;
    let field = reference(
        &daemon,
        &session,
        &tab,
        "Authentication code",
        "moved-login-ref",
    )
    .await;
    assert_eq!(
        model(
            request_otp(
                &daemon,
                &session,
                json!({"tab_id":tab,"field_ref":field}),
                "moved-otp"
            )
            .await
        ),
        json!({"status":"filled"})
    );
    assert_eq!(verify(&daemon, &session, &tab, "moved").await, "Signed in");
    assert_private(&daemon, &runtime, &session, SEED).await;
    daemon.shutdown().await;
    fixture.abort();
}

/// Every factor a refused page shows is protected anyway: an ambiguous setup page masks all
/// of its keys in every representation and taints the tab, though nothing is kept.
#[tokio::test]
async fn an_ambiguous_setup_page_is_refused_with_every_key_masked_and_the_tab_tainted() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-ambiguous", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/setup-ambiguous"),
        "ambiguous-open",
    )
    .await;
    let outcome = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab}),
        "ambiguous-read",
    )
    .await;
    assert!(
        matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "setup_key_ambiguous"),
        "{outcome:?}"
    );
    assert!(matches!(
        bounded(
            &daemon,
            &session,
            "browser_screenshot",
            json!({"tab_id":tab}),
            "ambiguous-shot"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));
    let text = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "ambiguous-snap",
        )
        .await,
    )
    .to_string();
    assert!(!text.contains(SEED) && !text.contains(OTHER_SEED), "{text}");
    assert!(!vault_audit(&daemon)
        .await
        .to_string()
        .contains("otp_register"));
    assert_private(&daemon, &runtime, &session, SEED).await;
    assert_private(&daemon, &runtime, &session, OTHER_SEED).await;
    daemon.shutdown().await;
    fixture.abort();
}

/// Save a factor at `origin` under `operation`, as an approval does before its ticket commit.
async fn plant_factor(daemon: &Daemon, origin: &str, operation: &str) {
    let mut guard = daemon.shared.registry.vault.vault.lock().await;
    let audit = openside_vault::AuditContext {
        who: "local",
        session_id: None,
        tool_call_id: None,
    };
    assert!(guard
        .as_mut()
        .unwrap()
        .register_totp(
            &audit,
            origin,
            Totp::parse(Secret::new(otpauth())).unwrap(),
            operation,
        )
        .unwrap()
        .is_ok());
}

async fn reject_ticket(daemon: &Daemon, id: &TicketId) -> Value {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{id}/reject"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// An approval that saved the factor and then failed to commit its ticket leaves its intent
/// behind, naming the operation it saved under. After a restart, an explicit reject reports
/// the registration the vault holds instead of `rejected`.
#[tokio::test]
async fn an_approval_whose_commit_failed_is_reported_as_registered_after_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let (origin, fixture) = site().await;
    let Some(config) = otp_config(root.path(), Vec::new()) else {
        return;
    };
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config.clone(), runtime.clone())
        .await
        .unwrap();
    let session = create_session(&daemon, "otp-commit", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/setup-text"),
        "commit-open",
    )
    .await;
    let ToolOutcome::Deferred { ticket: id, .. } = bounded(
        &daemon,
        &session,
        "register_authenticator",
        json!({"tab_id":tab}),
        "commit-first",
    )
    .await
    else {
        panic!("expected one approval ticket");
    };
    daemon.shared.db.fail_resolution(id.as_str());
    let response = resolve_value(&daemon, &id, json!(true)).await;
    assert_ne!(response.status(), StatusCode::OK);
    let controller = &daemon.shared.registry.browser;
    let intent = controller
        .fill_intent(&daemon.shared, &id)
        .await
        .unwrap()
        .expect("the failed commit keeps its intent");
    let operation = intent
        .registration
        .expect("an approval records its operation");
    {
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        let vault = guard.as_mut().unwrap();
        assert!(vault.registered_by(&operation).unwrap());
        // Another device changes only the item's policy and sync replaces the item here; the
        // local evidence of this operation must survive the import.
        let audit = openside_vault::AuditContext {
            who: "local",
            session_id: None,
            tool_call_id: None,
        };
        let site = vault
            .item_ids()
            .unwrap()
            .into_iter()
            .find(|site| site.starts_with("otp-"))
            .unwrap();
        let exported = vault.export_item(&audit, &site).unwrap();
        let mut synced: Value = serde_json::from_slice(&exported).unwrap();
        synced["policy"] = json!("Always");
        let synced = serde_json::to_vec(&synced).unwrap();
        let prepared = openside_vault::PreparedImport::parse(&site, &synced).unwrap();
        vault.commit_import(&audit, prepared).unwrap();
        assert!(vault.registered_by(&operation).unwrap());
    }
    assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    daemon.shutdown().await;
    drop(daemon);

    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let rejected = reject_ticket(&daemon, &id).await;
    assert_eq!(rejected["status"], "resolved");
    assert_eq!(
        rejected["resolution"],
        json!({"status":"registered","filled":false})
    );
    assert!(!daemon
        .shared
        .registry
        .browser
        .fill_intent_pending(&daemon.shared, &id)
        .await
        .unwrap());
    daemon.shutdown().await;
    fixture.abort();
}

/// Recovery trusts only the operation an intent names: an unrelated factor at the same origin
/// leaves a decline as `declined`; expiry and a session stop report what the vault holds for
/// their own operation, or `registration_indeterminate` when the vault cannot say.
#[tokio::test]
async fn recovery_follows_the_intent_operation_through_every_terminal_path() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, _runtime)) = otp_daemon(root.path()).await else {
        return;
    };
    let (origin, fixture) = site().await;
    let session = create_session(&daemon, "otp-recover", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/setup-text"),
        "recover-open",
    )
    .await;
    let mut ids = Vec::new();
    for label in ["recover-declined", "recover-expired", "recover-stopped"] {
        let ToolOutcome::Deferred { ticket, .. } = bounded(
            &daemon,
            &session,
            "register_authenticator",
            json!({"tab_id":tab}),
            label,
        )
        .await
        else {
            panic!("expected one approval ticket");
        };
        ids.push(ticket);
    }
    let [declined, expired, stopped] = <[TicketId; 3]>::try_from(ids).unwrap();
    let controller = &daemon.shared.registry.browser;
    for (id, operation) in [
        (&declined, "op-crashed"),
        (&expired, "op-expired"),
        (&stopped, "op-stopped"),
    ] {
        controller
            .record_fill_intent(&daemon.shared, id, Some(operation))
            .await
            .unwrap();
    }
    // Someone else registered at the same origin; the declined ticket never saved anything.
    plant_factor(&daemon, &origin, "op-unrelated").await;
    // The expired ticket's own save landed (at another origin, which recovery never consults).
    plant_factor(&daemon, "https://planted.example", "op-expired").await;

    let response = resolve_value(&daemon, &declined, json!(false)).await;
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"approved":false,"status":"declined"})
    );
    assert!(!controller
        .fill_intent_pending(&daemon.shared, &declined)
        .await
        .unwrap());

    {
        let _gate = daemon.shared.ticket_gate.lock().await;
        let ticket = crate::tickets::get(&daemon.shared, &expired)
            .await
            .unwrap()
            .unwrap();
        crate::tickets::close_unanswered(&daemon.shared, &ticket, None)
            .await
            .unwrap();
    }
    let closed = ticket(&daemon, &expired).await;
    assert_eq!(closed["status"], "resolved");
    assert_eq!(
        closed["resolution"],
        json!({"status":"registered","filled":false})
    );

    daemon
        .shared
        .registry
        .vault
        .lock(&daemon.shared, "local")
        .await
        .unwrap();
    crate::tickets::reject_open(&daemon.shared, &session, "stopped")
        .await
        .unwrap();
    let closed = ticket(&daemon, &stopped).await;
    assert_eq!(closed["status"], "resolved");
    assert_eq!(
        closed["resolution"],
        json!({"status":"registration_indeterminate"})
    );
    // The outcome is terminal, so the intent goes with the ticket.
    assert!(!controller
        .fill_intent_pending(&daemon.shared, &stopped)
        .await
        .unwrap());
    daemon.shutdown().await;
    fixture.abort();
}
