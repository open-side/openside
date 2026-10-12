//! Loopback-only browser acceptance with encrypted fixture credentials.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, SessionId, Ticket, TicketDeferral, TicketKind};
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Vault};
use serde_json::{json, Value};

const PASSWORD: &str = "E2E-VAULT-PASSWORD-CANARY-7a4f9d";
const OTP: &str = "863927451086";

#[derive(Default)]
struct SiteState {
    password_accepted: AtomicBool,
    completed: AtomicBool,
    payments: AtomicUsize,
}
struct Site {
    origin: String,
    state: Arc<SiteState>,
    task: tokio::task::JoinHandle<()>,
}
impl Site {
    async fn start() -> Self {
        let state = Arc::new(SiteState::default());
        let app = Router::new()
            .route("/", get(|| async { Html(LOGIN) }))
            .route("/login", post(login))
            .route("/otp", get(|| async { Html(OTP_PAGE) }))
            .route("/verify", post(verify))
            .route(
                "/complete",
                get(|| async { Html("<title>Complete</title><h1>Login completed</h1>") }),
            )
            .route("/payment", get(|| async { Html(PAYMENT_PAGE) }))
            .route("/pay", post(pay))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Vault allowlists deliberately accept DNS origins, not IP literals.
        Self {
            origin: format!("http://localhost:{port}"),
            state,
            task,
        }
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const LOGIN: &str = r#"<!doctype html><title>Fixture login</title>
<form id="login"><label>Username<input aria-label="Username" id="username" autocomplete="off"></label>
<label>Password<input aria-label="Password" id="password" type="password" autocomplete="off"></label>
<button aria-label="Sign in">Sign in</button></form>
<script>document.querySelector('#login').onsubmit=async e=>{e.preventDefault();const r=await fetch('/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:document.querySelector('#username').value,password:document.querySelector('#password').value})});if(r.ok)location.assign('/otp')};</script>"#;
const OTP_PAGE: &str = r#"<!doctype html><title>Verification</title>
<label>Verification code<input type="password" aria-label="Verification code" id="code" autocomplete="off"></label>
<script>document.querySelector('#code').oninput=async e=>{if(e.target.value.length<12)return;const r=await fetch('/verify',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({code:e.target.value})});if(r.ok)location.assign('/complete')};</script>"#;
const PAYMENT_PAGE: &str = r#"<!doctype html><title>Payment fixture</title><button aria-label="Pay now" onclick="fetch('/pay',{method:'POST'})">Pay now</button>"#;

async fn login(State(state): State<Arc<SiteState>>, Json(body): Json<Value>) -> StatusCode {
    if body["username"] == "fixture-user" && body["password"] == PASSWORD {
        state.password_accepted.store(true, Ordering::SeqCst);
        StatusCode::NO_CONTENT
    } else {
        StatusCode::FORBIDDEN
    }
}
async fn verify(State(state): State<Arc<SiteState>>, Json(body): Json<Value>) -> StatusCode {
    if state.password_accepted.load(Ordering::SeqCst) && body["code"] == OTP {
        state.completed.store(true, Ordering::SeqCst);
        StatusCode::NO_CONTENT
    } else {
        StatusCode::FORBIDDEN
    }
}
async fn pay(State(state): State<Arc<SiteState>>) -> StatusCode {
    state.payments.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}
fn browser() -> Option<std::path::PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: browser acceptance requires Chromium: {reason}");
            None
        }
    }
}
async fn create(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
async fn submit(harness: &Harness, session: &str, id: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":id,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}
fn result<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
            } if tool_call_id == call => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {call}: {events:?}"))
}
fn field(snapshot: &Value, name: &str) -> String {
    let text = snapshot.as_str().expect("compact accessibility snapshot");
    let label = format!("\"{name}\"");
    let line = text
        .lines()
        .find(|line| line.contains(&label) && (line.contains("textbox") || line.contains("button")))
        .unwrap_or_else(|| panic!("snapshot lacks {name}: {text}"));
    let start = line.find('[').unwrap() + 1;
    let end = line[start..].find(']').unwrap() + start;
    line[start..end].to_owned()
}
async fn opened(stream: &mut SseReader) -> Ticket {
    loop {
        let event = stream.next().await;
        if let EventKind::TicketOpened { ticket } = event.kind {
            return *ticket;
        }
        assert!(
            !matches!(event.kind, EventKind::SessionSettled {}),
            "settled without ticket"
        );
    }
}

// Dynamic tab and accessibility IDs come from real tool results, not a fake
// browser. At committed boundaries a new scripted sidecar reloads the extended
// script while preserving its cursor and conversation via resume_state.
async fn extend_script(
    harness: &mut Harness,
    stream: &mut SseReader,
    responses: &mut Vec<Value>,
    additions: Vec<Value>,
) {
    responses.extend(additions);
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    let boundary = stream.settled().await;
    assert!(boundary.iter().any(
        |event| matches!(&event.kind, EventKind::RuntimeError { code, .. } if code == "interrupted")
    ));
}
async fn turn(
    harness: &Harness,
    stream: &mut SseReader,
    session: &str,
    id: &str,
    final_text: &str,
) -> Vec<Event> {
    submit(harness, session, id).await;
    let events = stream.settled().await;
    assert!(
        events.iter().any(
            |event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == final_text)
        ),
        "turn missing final: {events:?}"
    );
    events
}

#[tokio::test]
async fn hs1_vault_login_secret_ticket_survives_daemon_restart_without_canary_leaks() {
    let Some(executable) = browser() else {
        return;
    };
    let site = Site::start().await;
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{}/",site.origin)}),
            "open",
        ),
        text("tab opened"),
    ];
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":responses}),
    ) else {
        return;
    };
    let vault_path = harness.root().join("fixture.vault");
    let passphrase = Secret::new("disposable-fixture-passphrase".into());
    let mut vault = Vault::create(
        &vault_path,
        harness.data_dir.join("fixture-vault.audit.jsonl"),
        &passphrase,
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(&site.origin).unwrap()],
                Secret::new("fixture-user".into()),
                Secret::new(PASSWORD.into()),
                None,
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    vault.lock();
    let unlock = harness.root().join("unlock.sh");
    std::fs::write(
        &unlock,
        "#!/bin/sh\nprintf '%s\\n' 'disposable-fixture-passphrase'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unlock, std::fs::Permissions::from_mode(0o700)).unwrap();
    harness.append_config(&format!(
        "[browser]\nexecutable = {}\n[vault]\npath = {}\npassphrase_cmd = [{}]\n",
        json!(executable),
        json!(vault_path),
        json!(unlock)
    ));
    harness.launch().await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "open-message",
        "tab opened",
    )
    .await;
    let tab = result(&events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab}), "login-snapshot"),
            text("login fields"),
        ],
    )
    .await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "snapshot-message",
        "login fields",
    )
    .await;
    let snapshot = result(&events, "login-snapshot");
    let username_ref = field(snapshot, "Username");
    let password_ref = field(snapshot, "Password");
    let submit_ref = field(snapshot, "Sign in");
    extend_script(&mut harness, &mut stream, &mut responses, vec![
        tool("vault_login", json!({"tab_id":tab,"site":"fixture","username_ref":username_ref,"password_ref":password_ref}), "vault-login"),
        tool("browser_snapshot", json!({"tab_id":tab}), "password-snapshot"),
        tool("browser_act", json!({"tab_id":tab,"target":submit_ref,"action":"click"}), "login-submit"),
        text("password filled")
    ]).await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "vault-message",
        "password filled",
    )
    .await;
    result(&events, "vault-login");
    assert!(
        !result(&events, "password-snapshot")
            .to_string()
            .contains(PASSWORD),
        "post-fill snapshot leaked vault password"
    );
    result(&events, "login-submit");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !site.state.password_accepted.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture did not receive vault login");
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab}), "otp-snapshot"),
            text("verification fields"),
        ],
    )
    .await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "otp-snapshot-message",
        "verification fields",
    )
    .await;
    let otp_ref = field(result(&events, "otp-snapshot"), "Verification code");
    extend_script(&mut harness, &mut stream, &mut responses, vec![
        tool("request_human_step", json!({"kind":"secret_input","summary":"Enter the verification code directly in the browser","tab_id":tab,"origin":site.origin,"field_ref":otp_ref}), "hs1-code"),
        tool("browser_snapshot", json!({"tab_id":tab}), "post-otp-snapshot"),
        text("login completed")
    ]).await;
    submit(&harness, &session, "code-request").await;
    let ticket = opened(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::SecretInput);
    assert_eq!(ticket.tool_call_id, "hs1-code");
    let deferred = stream.settled().await;
    let cursor = deferred.last().unwrap().seq;
    assert_eq!(
        harness.snapshot(&session)["pending_deferred"][0]["tool_call_id"],
        "hs1-code"
    );
    harness.assert_no_canaries(&[PASSWORD, OTP]);
    drop(stream);
    harness.restart().await;
    let pending = harness
        .request("GET", &format!("/v1/tickets/{}", ticket.id), None)
        .await;
    assert_eq!(pending.status, 200);
    assert_eq!(pending.body["status"], "pending");
    assert_eq!(pending.body["tool_call_id"], "hs1-code");
    let mut stream = harness.events(&session, cursor).await;
    let output = harness
        .cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["ticket", "resolve", ticket.id.as_str(), "-"],
            Some(format!("{OTP}\n").as_bytes()),
        )
        .await;
    assert!(
        output.status.success(),
        "secret CLI resolve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        assert!(
            !bytes.windows(OTP.len()).any(|part| part == OTP.as_bytes()),
            "CLI echoed secret input"
        );
    }
    let resumed = stream.settled().await;
    assert!(resumed.iter().any(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, resolution } if ticket_id == &ticket.id && resolution["status"] == "filled")));
    assert!(resumed.iter().any(|event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == "login completed")));
    assert!(
        !result(&resumed, "post-otp-snapshot")
            .to_string()
            .contains(OTP),
        "post-fill snapshot leaked verification code"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !site.state.completed.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture did not complete verification");
    let snapshot = harness.snapshot(&session);
    let returns: Vec<_> = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .filter(|part| part["part_kind"] == "tool-return" && part["tool_call_id"] == "hs1-code")
        .collect();
    assert_eq!(returns.len(), 1);
    assert_eq!(returns[0]["content"]["status"], "filled");
    assert!(snapshot["pending_deferred"].as_array().unwrap().is_empty());
    assert!(!snapshot.to_string().contains(PASSWORD) && !snapshot.to_string().contains(OTP));
    let store = openside_sessions::Store::open(harness.data_dir.join("events.db")).unwrap();
    let durable = store
        .events_after(&SessionId::new(&session), 0, 4096)
        .unwrap();
    assert_eq!(durable.iter().filter(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id == &ticket.id)).count(), 1);
    assert_eq!(
        harness
            .request(
                "POST",
                &format!("/v1/tickets/{}/resolve", ticket.id),
                Some(json!({"value":"already-completed"}))
            )
            .await
            .status,
        409
    );
    harness.assert_no_canaries(&[PASSWORD, OTP]);
    harness.kill().await;
    harness.assert_no_canaries(&[PASSWORD, OTP]);
}

#[tokio::test]
async fn sensitive_payment_ask_expires_and_denies_original_call_without_clicking() {
    let Some(executable) = browser() else {
        return;
    };
    let site = Site::start().await;
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{}/payment",site.origin)}),
            "payment-open",
        ),
        text("payment page"),
    ];
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":responses}),
    ) else {
        return;
    };
    harness.append_config(&format!(
        "[browser]\nexecutable = {}\n[tickets]\nconfirm_ttl_secs = 2\n",
        json!(executable)
    ));
    harness.launch().await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "payment-open-message",
        "payment page",
    )
    .await;
    let tab = result(&events, "payment-open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool(
                "browser_snapshot",
                json!({"tab_id":tab}),
                "payment-snapshot",
            ),
            text("payment target"),
        ],
    )
    .await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "payment-snapshot-message",
        "payment target",
    )
    .await;
    let target = field(result(&events, "payment-snapshot"), "Pay now");
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool(
                "browser_act",
                json!({"tab_id":tab,"target":target,"action":"click"}),
                "payment-click",
            ),
            text("payment denied"),
        ],
    )
    .await;
    submit(&harness, &session, "payment-request").await;
    let ticket = opened(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::Confirm);
    assert_eq!(ticket.deferral, TicketDeferral::Approval);
    assert_eq!(ticket.tool_call_id, "payment-click");
    assert_eq!(ticket.tool.as_ref().unwrap().name, "browser_act");
    assert!(
        ticket
            .expires_at
            .signed_duration_since(ticket.created_at)
            .num_milliseconds()
            <= 2500,
        "configured short expiry was not applied"
    );
    assert_eq!(site.state.payments.load(Ordering::SeqCst), 0);
    let deferred = stream.settled().await;
    assert!(deferred.iter().any(|event| matches!(&event.kind, EventKind::ToolEnded { tool_call_id, result, .. } if tool_call_id == "payment-click" && result["status"] == "deferred")));
    let mut resumed = Vec::new();
    loop {
        let event = stream.next().await;
        let done = matches!(event.kind, EventKind::SessionSettled {})
            && resumed
                .iter()
                .any(|event: &Event| matches!(event.kind, EventKind::TicketExpired { .. }));
        resumed.push(event);
        if done {
            break;
        }
    }
    assert!(resumed.iter().any(|event| matches!(&event.kind, EventKind::TicketExpired { ticket_id } if ticket_id == &ticket.id)));
    assert!(resumed.iter().any(|event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == "payment denied")));
    assert_eq!(
        site.state.payments.load(Ordering::SeqCst),
        0,
        "expired sensitive operation executed"
    );
    let pending = harness
        .request("GET", &format!("/v1/tickets/{}", ticket.id), None)
        .await;
    assert_eq!(pending.body["status"], "expired");
    assert_eq!(
        harness
            .human_request(
                "POST",
                &format!("/v1/tickets/{}/resolve", ticket.id),
                Some(json!({"value":true}))
            )
            .await
            .status,
        409
    );
    let snapshot = harness.snapshot(&session);
    assert!(snapshot["pending_deferred"].as_array().unwrap().is_empty());
    let denied = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .find(|part| part["tool_call_id"] == "payment-click" && part["part_kind"] == "tool-return")
        .expect("expired approval must deliver denial for original tool call");
    assert_eq!(
        denied["outcome"], "denied",
        "expired approval must use Pydantic AI denial semantics"
    );
    let explanation = denied["content"].to_string().to_lowercase();
    assert!(
        explanation.contains("denied") || explanation.contains("expired"),
        "expiry did not deny original call: {denied}"
    );
    harness.kill().await;
}
