//! `request_otp` through the real daemon, a scripted sidecar and Chromium against loopback
//! login pages that verify codes themselves: a 6-digit TOTP page (the person registers the
//! authenticator once through `openside ticket resolve - --totp`, the next login needs no
//! ticket) and an emailed code typed into six one-character boxes. The setup key and every
//! submitted code stay out of the model transcript, the event log and every file.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{extract::Form, response::Html, routing::get, Router};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, OtpChannel, Ticket, TicketKind};
use openside_vault::{PassphraseLock, Secret, Vault};
use parking_lot::Mutex;
use serde_json::{json, Value};

/// RFC 6238's SHA-256 test key, and the same bytes in base32.
const KEY: &[u8] = b"12345678901234567890123456789012";
const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZA";
const EMAIL_CODE: &str = "Q7K2XM";

fn otpauth() -> String {
    format!("otpauth://totp/Fixture:alice?secret={SEED}&algorithm=SHA256&digits=6&period=30")
}

/// The fixture's own TOTP, computed independently of the vault.
fn totp_at(unix: u64) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, KEY);
    let tag = ring::hmac::sign(&key, &(unix / 30).to_be_bytes());
    let digest = tag.as_ref();
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

/// Every code the site received, and whether it accepted it.
type Submissions = Arc<Mutex<Vec<(String, bool)>>>;

struct Site {
    origin: String,
    submissions: Submissions,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Site {
    async fn start() -> Self {
        let submissions = Submissions::default();
        let (totp, email) = (submissions.clone(), submissions.clone());
        let verdict = |ok: bool| {
            Html(if ok {
                "<!doctype html><title>Signed in</title><h1>Welcome back</h1>"
            } else {
                "<!doctype html><title>Wrong code</title><h1>Wrong code</h1>"
            })
        };
        let app = Router::new()
            .route(
                "/totp",
                get(|| async { Html(TOTP_PAGE) }).post(
                    move |Form(form): Form<HashMap<String, String>>| async move {
                        let code = form.get("code").cloned().unwrap_or_default();
                        let now = now();
                        // One step of clock drift either way, as real verifiers allow.
                        let ok = [now - 30, now, now + 30]
                            .iter()
                            .any(|t| totp_at(*t) == code);
                        totp.lock().push((code, ok));
                        verdict(ok)
                    },
                ),
            )
            .route(
                "/email",
                get(|| async { Html(EMAIL_PAGE) }).post(
                    move |Form(form): Form<HashMap<String, String>>| async move {
                        let code: String = (1..=6)
                            .filter_map(|i| form.get(&format!("d{i}")).cloned())
                            .collect();
                        let ok = code == EMAIL_CODE;
                        email.lock().push((code, ok));
                        verdict(ok)
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Vault allowlists accept DNS origins, not IP literals.
        Self {
            origin: format!("http://localhost:{port}"),
            submissions,
            task,
        }
    }
    async fn accepted(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if self.submissions.lock().len() >= count {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the site never received the code");
        let submissions = self.submissions.lock();
        assert!(
            submissions.iter().all(|(_, ok)| *ok),
            "the site rejected a filled code"
        );
    }
    fn codes(&self) -> Vec<String> {
        self.submissions
            .lock()
            .iter()
            .map(|(code, _)| code.clone())
            .collect()
    }
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}
fn result<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                card: _,
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
/// Appends scripted turns; a fresh scripted sidecar resumes at the cursor, so the script can
/// use tab ids and refs returned by earlier real tool calls.
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
fn final_text(events: &[Event], expected: &str) -> bool {
    events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == expected))
}

/// Daemon with Chromium and an empty passphrase vault; `None` skips without Chromium.
async fn launch(responses: &[Value]) -> Option<Harness> {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: request_otp acceptance requires Chromium: {reason}");
            return None;
        }
    };
    let mut harness = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":responses}),
    )?;
    let vault_path = harness.root().join("otp.vault");
    drop(
        Vault::create(
            &vault_path,
            harness.root().join("setup.audit.jsonl"),
            &Secret::new("disposable-otp-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap(),
    );
    let unlock = harness.root().join("unlock.sh");
    std::fs::write(
        &unlock,
        "#!/bin/sh\nprintf '%s\\n' 'disposable-otp-passphrase'\n",
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
    Some(harness)
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

/// No code appears as a value anywhere the model or the event log could see it.
fn assert_codes_absent(surface: &str, codes: &[String]) {
    for code in codes {
        assert!(
            !surface.contains(&format!("\"{code}\"")),
            "a submitted code reached the model transcript or the event log"
        );
    }
}

#[tokio::test]
async fn totp_registered_once_through_the_cli_then_filled_from_the_vault_without_a_ticket() {
    let site = Site::start().await;
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{}/totp",site.origin)}),
            "open",
        ),
        text("opened"),
    ];
    let Some(mut harness) = launch(&responses).await else {
        return;
    };
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let mut all = turn(&harness, &mut stream, &session, "open", "opened").await;
    let tab = result(&all, "open")["tab_id"].as_str().unwrap().to_owned();

    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab}), "totp-fields"),
            text("fields"),
        ],
    )
    .await;
    let events = turn(&harness, &mut stream, &session, "fields", "fields").await;
    let code_ref = field(result(&events, "totp-fields"), "Authentication code");
    let verify_ref = field(result(&events, "totp-fields"), "Verify");
    all.extend(events);

    // First login: nothing saved, so the person is asked and registers the authenticator.
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool(
                "request_otp",
                json!({"tab_id":tab,"field_ref":code_ref,"channel":"authenticator"}),
                "otp-ask",
            ),
            tool("browser_snapshot", json!({"tab_id":tab}), "after-fill"),
            tool(
                "browser_act",
                json!({"tab_id":tab,"target":verify_ref,"action":"click"}),
                "verify-1",
            ),
            text("signed in"),
        ],
    )
    .await;
    submit(&harness, &session, "login-1").await;
    let ticket = opened(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::SecretInput);
    assert_eq!(ticket.tool_call_id, "otp-ask");
    let otp = ticket.otp.clone().expect("a request_otp ticket");
    assert_eq!(otp.channel, OtpChannel::Authenticator);
    assert!(otp.can_register);
    assert_eq!(
        ticket.target.as_ref().unwrap().origin.as_deref(),
        Some(site.origin.as_str())
    );
    all.extend(stream.settled().await);
    let output = harness
        .human_cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["ticket", "resolve", ticket.id.as_str(), "-", "--totp"],
            Some(format!("{}\n", otpauth()).as_bytes()),
        )
        .await;
    assert!(
        output.status.success(),
        "register through the CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        assert!(
            !String::from_utf8_lossy(bytes).contains(SEED),
            "CLI echoed the setup key"
        );
    }
    let events = stream.settled().await;
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, resolution, .. } if ticket_id == &ticket.id && *resolution == json!({"status":"filled","registered":true}))));
    assert!(final_text(&events, "signed in"), "{events:?}");
    all.extend(events);
    site.accepted(1).await;

    // Second login: the vault fills the current code, no ticket opens.
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool(
                "browser_navigate",
                json!({"tab_id":tab,"url":format!("{}/totp",site.origin)}),
                "again",
            ),
            tool("browser_snapshot", json!({"tab_id":tab}), "totp-fields-2"),
            text("fields again"),
        ],
    )
    .await;
    let events = turn(&harness, &mut stream, &session, "again", "fields again").await;
    let code_ref = field(result(&events, "totp-fields-2"), "Authentication code");
    let verify_ref = field(result(&events, "totp-fields-2"), "Verify");
    all.extend(events);
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool(
                "request_otp",
                json!({"tab_id":tab,"field_ref":code_ref}),
                "otp-saved",
            ),
            tool(
                "browser_act",
                json!({"tab_id":tab,"target":verify_ref,"action":"click"}),
                "verify-2",
            ),
            text("signed in again"),
        ],
    )
    .await;
    let events = turn(
        &harness,
        &mut stream,
        &session,
        "login-2",
        "signed in again",
    )
    .await;
    assert_eq!(*result(&events, "otp-saved"), json!({"status":"filled"}));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::TicketOpened { .. })),
        "the saved authenticator still opened a ticket"
    );
    all.extend(events);
    site.accepted(2).await;

    let audit = harness
        .request("GET", &format!("/v1/vault/audit?session={session}"), None)
        .await;
    let records: Vec<(String, String)> = audit.body["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["what"].as_str().unwrap().into(),
                r["result"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert!(
        records.contains(&("otp_register".into(), "registered".into())),
        "{records:?}"
    );
    assert_eq!(
        records
            .iter()
            .filter(|r| *r == &("otp".to_owned(), "filled".to_owned()))
            .count(),
        2,
        "{records:?}"
    );

    let transcript = harness.snapshot(&session).to_string();
    let events_text = serde_json::to_string(&all).unwrap();
    assert!(
        transcript.contains("otp-saved"),
        "transcript lacks the calls"
    );
    let codes = site.codes();
    for surface in [&transcript, &events_text] {
        // Tool descriptions name "otpauth link"; the URI itself must never appear.
        assert!(!surface.contains(SEED) && !surface.contains("otpauth://"));
        assert_codes_absent(surface, &codes);
    }
    harness.kill().await;
    harness.assert_no_canaries(&[SEED]);
}

#[tokio::test]
async fn emailed_code_entered_through_the_cli_fills_split_boxes_and_reaches_no_surface() {
    let site = Site::start().await;
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{}/email",site.origin)}),
            "open",
        ),
        text("opened"),
    ];
    let Some(mut harness) = launch(&responses).await else {
        return;
    };
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let mut all = turn(&harness, &mut stream, &session, "open", "opened").await;
    let tab = result(&all, "open")["tab_id"].as_str().unwrap().to_owned();
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab}), "email-fields"),
            text("fields"),
        ],
    )
    .await;
    let events = turn(&harness, &mut stream, &session, "fields", "fields").await;
    let first_box = field(result(&events, "email-fields"), "Email code 1");
    let verify_ref = field(result(&events, "email-fields"), "Verify");
    all.extend(events);

    extend_script(&mut harness, &mut stream, &mut responses, vec![
        tool("request_otp", json!({"tab_id":tab,"field_ref":first_box,"channel":"email","summary":"Code from the sign-in email"}), "otp-email"),
        tool("browser_snapshot", json!({"tab_id":tab}), "after-fill"),
        tool("browser_act", json!({"tab_id":tab,"target":verify_ref,"action":"click"}), "verify"),
        text("signed in"),
    ]).await;
    submit(&harness, &session, "login").await;
    let ticket = opened(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::SecretInput);
    assert_eq!(ticket.summary, "Code from the sign-in email");
    let otp = ticket.otp.clone().expect("a request_otp ticket");
    assert_eq!(otp.channel, OtpChannel::Email);
    assert!(!otp.can_register, "an emailed code offers no registration");
    all.extend(stream.settled().await);

    // An authenticator is no answer to an emailed code; the ticket stays open.
    let refused = harness
        .human_cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["ticket", "resolve", ticket.id.as_str(), "-", "--totp"],
            Some(format!("{}\n", otpauth()).as_bytes()),
        )
        .await;
    assert!(!refused.status.success());
    let pending = harness
        .request("GET", &format!("/v1/tickets/{}", ticket.id), None)
        .await;
    assert_eq!(pending.body["status"], "pending");

    let output = harness
        .cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["ticket", "resolve", ticket.id.as_str(), "-"],
            Some(format!("{EMAIL_CODE}\n").as_bytes()),
        )
        .await;
    assert!(
        output.status.success(),
        "code through the CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [
        &output.stdout,
        &output.stderr,
        &refused.stdout,
        &refused.stderr,
    ] {
        let text = String::from_utf8_lossy(bytes);
        assert!(
            !text.contains(EMAIL_CODE) && !text.contains(SEED),
            "CLI echoed a secret"
        );
    }
    let events = stream.settled().await;
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, resolution, .. } if ticket_id == &ticket.id && *resolution == json!({"status":"filled"}))));
    assert!(final_text(&events, "signed in"), "{events:?}");
    assert!(!result(&events, "after-fill")
        .to_string()
        .contains(EMAIL_CODE));
    all.extend(events);
    site.accepted(1).await;
    assert_eq!(site.codes(), [EMAIL_CODE]);

    let snapshot = harness.snapshot(&session);
    let returns: Vec<_> = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .filter(|part| part["part_kind"] == "tool-return" && part["tool_call_id"] == "otp-email")
        .collect();
    assert_eq!(returns.len(), 1);
    assert_eq!(returns[0]["content"], json!({"status":"filled"}));
    let transcript = snapshot.to_string();
    let events_text = serde_json::to_string(&all).unwrap();
    for surface in [&transcript, &events_text] {
        assert!(!surface.contains(EMAIL_CODE));
    }
    harness.kill().await;
    harness.assert_no_canaries(&[EMAIL_CODE]);
}
