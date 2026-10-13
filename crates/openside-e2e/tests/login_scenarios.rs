//! Requirement C-10: the login scenarios of `openside_e2e::login_fixture` through the real
//! daemon, a scripted sidecar and headless Chromium. The person never touches the browser:
//! every answer goes through the ticket API the way the web card sends it. Each scenario checks
//! the site's own state (`/__fixture/state`), the tickets that opened, the vault, and that no
//! password, code or setup key reaches the model transcript, the event log or any file.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::{
    login_fixture::{
        totp_now, LoginFixture, ALICE, BOB, CARA, IDP_ACCOUNTS, PAT, SAM, SMS_CODE, TESS, TOM,
        TOTP_SEED,
    },
    Harness, SseReader,
};
use openside_proto::{Event, EventKind, Ticket, TicketKind};
use openside_vault::{PassphraseLock, Secret, Vault};
use serde_json::{json, Value};

const PASSWORDS: [&str; 7] = [ALICE.1, BOB.1, PAT.1, SAM.1, TOM.1, TESS.1, CARA.1];
const GROUPED_TOTP_SEED: &str = "GEZD GNBV GY3T QOJQ GEZD GNBV GY3T QOJQ";

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}
const TAB: &str = "{{result:open.tab_id}}";
fn on_tab(name: &str, id: &str) -> Value {
    tool(name, json!({"tab_id":TAB}), id)
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
/// Ref of the snapshot line naming `name` as a textbox or button.
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

/// Sidecar script, event stream and the events seen so far, for one session.
struct Run {
    harness: Harness,
    stream: SseReader,
    session: String,
    responses: Vec<Value>,
    events: Vec<Event>,
    live_tab: Option<String>,
}

impl Run {
    /// Daemon with Chromium and an empty vault; `None` skips without Chromium.
    async fn start(site: &LoginFixture, path: &str) -> Option<Self> {
        Self::start_with(site, path, "").await
    }

    /// [`Run::start`] with extra daemon configuration.
    async fn start_with(site: &LoginFixture, path: &str, config: &str) -> Option<Self> {
        Self::start_config(site, path, config, None).await
    }

    async fn start_config(
        site: &LoginFixture,
        path: &str,
        config: &str,
        live_count: Option<&std::path::Path>,
    ) -> Option<Self> {
        let executable = match openside_config::discover_browser(&openside_config::Config::default())
        {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("SKIP: login scenarios require Chromium: {reason}");
                return None;
            }
        };
        let responses = vec![
            tool(
                "browser_open_tab",
                json!({"url":format!("{}{path}", site.origin)}),
                "open",
            ),
            text("opened"),
        ];
        let mut harness = Harness::prepare(
            env!("CARGO_BIN_EXE_e2e-opensided"),
            &json!({"responses":responses}),
        )?;
        if let Some(count) = live_count {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let wrapper = harness.root().join("live-agent.py");
            std::fs::write(
                &wrapper,
                format!(
                    r#"# Opt-in live QA blocks report-driven broker mutations, but the daemon may
# refresh an expired token while acquiring a credential snapshot.
import fcntl, runpy
from pathlib import Path
from openside_agent.providers import AuthUnavailable
from openside_agent.providers.credential_rpc import DaemonCredentialRpc
credential_report = DaemonCredentialRpc.report
async def read_only_report(self, lease, outcome, request_id, **kwargs):
    if outcome.kind in ('auth', 'rate_limit', 'quota', 'policy'):
        raise AuthUnavailable('live', ['Live login QA forbids report-driven broker mutation']) from None
    return await credential_report(self, lease, outcome, request_id, **kwargs)
DaemonCredentialRpc.report = read_only_report
original = DaemonCredentialRpc.acquire
async def limited(self, provider, model, request_id, attempt, subagent):
    with Path({count:?}).open('a+') as ledger:
        fcntl.flock(ledger, fcntl.LOCK_EX)
        ledger.seek(0)
        used = len(ledger.readlines())
        if used >= 40:
            raise RuntimeError('Live login QA model request budget exhausted')
        ledger.write(str(used + 1) + '\n')
        ledger.flush()
    return await original(self, provider, model, request_id, attempt, subagent)
DaemonCredentialRpc.acquire = limited
runpy.run_module('openside_agent', run_name='__main__')
"#,
                    count = count.to_string_lossy(),
                ),
            )
            .unwrap();
            let command = json!([
                "env",
                "-u",
                "OPENSIDE_AGENT_SCRIPT",
                repo.join("agent/.venv/bin/python"),
                wrapper
            ]);
            let token = std::path::Path::new(&std::env::var("HOME").unwrap())
                .join(".omp/auth-broker.token");
            assert!(token.is_file(), "broker token file is unavailable");
            let model = std::env::var("OPENSIDE_LOGIN_LIVE_MODEL")
                .unwrap_or_else(|_| "anthropic:claude-haiku-5-5".into());
            std::fs::write(
                harness.root().join("config/openside/config.toml"),
                format!("[agent]\ncommand = {command}\norchestration = \"single\"\n[models]\ndefault = {}\n[policy]\nmode = \"full\"\n[credentials.omp_broker]\nurl = \"http://127.0.0.1:8765\"\ntoken_file = {}\n", json!(model), json!(token)),
            )
            .unwrap();
        }
        let vault_path = harness.root().join("login.vault");
        drop(
            Vault::create(
                &vault_path,
                harness.root().join("setup.audit.jsonl"),
                &Secret::new("disposable-login-passphrase".into()),
                PassphraseLock::default(),
            )
            .unwrap(),
        );
        let unlock = harness.root().join("unlock.sh");
        std::fs::write(
            &unlock,
            "#!/bin/sh\nprintf '%s\\n' 'disposable-login-passphrase'\n",
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
        if !config.is_empty() {
            harness.append_config(config);
        }
        harness.launch().await;
        let created = harness
            .request(
                "POST",
                "/v1/sessions",
                Some(json!({"mode":"full","workspace":harness.workspace()})),
            )
            .await;
        assert_eq!(created.status, 201, "{}", created.body);
        let session = created.body["id"].as_str().unwrap().to_owned();
        let stream = harness.events(&session, 0).await;
        let mut run = Self {
            harness,
            stream,
            session,
            responses,
            events: Vec::new(),
            live_tab: None,
        };
        if live_count.is_none() {
            run.send("open").await;
            run.settle("opened").await;
        } else {
            let opened = run
                .direct_tool(
                    "browser_open_tab",
                    json!({"url":format!("{}{path}", site.origin)}),
                )
                .await;
            run.live_tab = Some(opened["tab_id"].as_str().expect("opened tab").to_owned());
        }
        Some(run)
    }

    async fn direct_tool(&self, name: &str, args: Value) -> Value {
        let response = self
            .harness
            .request(
                "POST",
                &format!("/v1/sessions/{}/tools/{name}", self.session),
                Some(json!({"args":args})),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(response.body["status"], "done", "{}", response.body);
        response.body["result"].clone()
    }

    async fn send(&self, id: &str) {
        self.send_text(id, id).await;
    }

    async fn send_text(&self, id: &str, text: &str) {
        let response = self
            .harness
            .request(
                "POST",
                &format!("/v1/sessions/{}/messages", self.session),
                Some(json!({"message_id":id,"text":text,"mode":"now"})),
            )
            .await;
        assert_eq!(response.status, 202, "{}", response.body);
    }

    /// Appends scripted responses; the restarted sidecar resumes at its cursor.
    async fn script(&mut self, additions: Vec<Value>) {
        self.responses.extend(additions);
        self.harness
            .replace_script(&json!({"responses":self.responses}));
        self.harness.kill_sidecar();
        let boundary = self.stream.settled().await;
        assert!(boundary.iter().any(
            |event| matches!(&event.kind, EventKind::RuntimeError { code, .. } if code == "interrupted")
        ));
    }

    /// The next ticket that opens; panics if the session settles first.
    async fn ticket(&mut self) -> Ticket {
        loop {
            let event = self.stream.next().await;
            let opened = match &event.kind {
                EventKind::TicketOpened { ticket } => Some((**ticket).clone()),
                // A deferred turn settles while its ticket waits; the next one opens on resume.
                _ => None,
            };
            self.events.push(event);
            if let Some(ticket) = opened {
                return ticket;
            }
        }
    }

    /// Resolves `ticket` as the web card does and returns the resolution the agent sees.
    async fn answer(&mut self, ticket: &Ticket, value: Value) -> Value {
        let response = self
            .harness
            .human_request(
                "POST",
                &format!("/v1/tickets/{}/resolve", ticket.id),
                Some(json!({"value":value})),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.body);
        self.resolution(ticket).await
    }

    /// Waits for `ticket` to resolve by itself (a phone approval) and returns its resolution.
    async fn resolution(&mut self, ticket: &Ticket) -> Value {
        loop {
            let event = self.stream.next().await;
            let found = match &event.kind {
                EventKind::TicketResolved {
                    card: _,
                    ticket_id,
                    resolution,
                } if ticket_id == &ticket.id => Some(resolution.clone()),
                _ => None,
            };
            self.events.push(event);
            if let Some(resolution) = found {
                return resolution;
            }
        }
    }

    /// Runs to the settle and checks the final reply; returns this stretch's events.
    async fn settle(&mut self, final_text: &str) -> Vec<Event> {
        let events = self.stream.settled().await;
        assert!(
            events.iter().any(
                |event| matches!(&event.kind, EventKind::AssistantFinal { text, .. } if text == final_text)
            ),
            "missing final {final_text:?}: {events:?}"
        );
        self.events.extend(events.iter().cloned());
        events
    }

    /// Sends `id`, expects no ticket, and returns the turn's events.
    async fn quiet_turn(&mut self, id: &str, final_text: &str) -> Vec<Event> {
        self.send(id).await;
        let events = self.settle(final_text).await;
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, EventKind::TicketOpened { .. })),
            "{id} opened a ticket: {events:?}"
        );
        events
    }

    async fn vault_items(&self) -> Vec<Value> {
        let items = self.harness.request("GET", "/v1/vault/items", None).await;
        assert_eq!(items.status, 200, "{}", items.body);
        items.body["items"].as_array().unwrap().clone()
    }

    /// Nothing secret reached the transcript, the events or any file.
    async fn finish(mut self, codes: &[String]) {
        let transcript = self.harness.snapshot(&self.session).to_string();
        let events = serde_json::to_string(&self.events).unwrap();
        for surface in [&transcript, &events] {
            for password in PASSWORDS {
                assert!(!surface.contains(password), "a password reached a surface");
            }
            assert!(
                !surface.contains(TOTP_SEED)
                    && !surface.contains(GROUPED_TOTP_SEED)
                    && !surface.contains("otpauth://")
            );
            for code in codes {
                assert!(
                    !surface.contains(&format!("\"{code}\"")),
                    "a code reached a surface"
                );
            }
        }
        self.harness.kill().await;
        let mut canaries = PASSWORDS.to_vec();
        canaries.push(TOTP_SEED);
        canaries.push(GROUPED_TOTP_SEED);
        canaries.extend(codes.iter().map(String::as_str));
        self.harness.assert_no_canaries(&canaries);
    }
}

/// `{values, save}` for the login card: `typed` by field kind.
fn login_values(ticket: &Ticket, typed: &[(&str, &str)], save: bool) -> Value {
    let login = ticket.login.as_ref().expect("a login ticket");
    let mut values = serde_json::Map::new();
    for field in &login.fields {
        let kind = serde_json::to_value(field.kind).unwrap();
        if let Some((_, value)) = typed.iter().find(|(name, _)| kind == json!(name)) {
            values.insert(field.id.clone(), json!(value));
        }
    }
    json!({"values":values,"save":save})
}

fn option_id(ticket: &Ticket, label: &str) -> String {
    let choice = ticket.choice.as_ref().expect("a choice ticket");
    choice
        .options
        .iter()
        .find(|option| {
            option.label.contains(label)
                || option
                    .description
                    .as_ref()
                    .is_some_and(|text| text.contains(label))
        })
        .unwrap_or_else(|| panic!("no option {label}: {choice:?}"))
        .id
        .clone()
}

async fn logout(site: &LoginFixture) {
    let response = reqwest::Client::new()
        .post(format!("{}/__fixture/logout", site.origin))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

async fn push(site: &LoginFixture, verdict: &str) {
    let response = reqwest::Client::new()
        .post(format!("{}/__fixture/push/{verdict}", site.origin))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

fn navigate(path: &str, site: &LoginFixture, id: &str) -> Value {
    tool(
        "browser_navigate",
        json!({"tab_id":TAB,"url":format!("{}{path}", site.origin)}),
        id,
    )
}

#[tokio::test]
async fn authenticator_setup_is_protected_before_any_registration_or_secret_fill() {
    let site = LoginFixture::start().await;
    let Some(run) = Run::start(&site, "/security/authenticator").await else {
        return;
    };
    let tab = result(&run.events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let snapshot = run
        .direct_tool("browser_snapshot", json!({"tab_id":tab}))
        .await;
    let text = snapshot.as_str().unwrap();
    assert!(
        !text.contains(TOTP_SEED)
            && !text.contains(GROUPED_TOTP_SEED)
            && !text.contains("otpauth://")
    );
    assert!(text.contains("[REDACTED]"), "{snapshot}");
    assert!(!field(&snapshot, "Enter the 6-digit code from the app to confirm").is_empty());
    assert!(run.vault_items().await.is_empty());
    let image = run
        .harness
        .request(
            "POST",
            &format!("/v1/sessions/{}/tools/browser_screenshot", run.session),
            Some(json!({"args":{"tab_id":tab}})),
        )
        .await;
    assert_eq!(image.body["status"], "denied", "{}", image.body);
    assert_eq!(
        image.body["reason"],
        "screenshot_unavailable_after_secret_fill"
    );
    let script = run
        .harness
        .request(
            "POST",
            &format!("/v1/sessions/{}/repl", run.session),
            Some(json!({"tab_id":tab,"code":"await page.snapshot()"})),
        )
        .await;
    assert_eq!(script.body["status"], "denied", "{}", script.body);
    assert_eq!(script.body["reason"], "repl_unavailable_after_secret_fill");
    run.finish(&[]).await;
}

#[tokio::test]
async fn repeated_login_instructions_use_distinct_message_ids() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login").await else {
        return;
    };
    run.script(vec![text("first completed"), text("later completed")])
        .await;
    run.send_text("live-first", "Sign me in on this tab.").await;
    run.settle("first completed").await;
    run.send_text("live-later", "Sign me in on this tab.").await;
    run.settle("later completed").await;
    run.finish(&[]).await;
}

#[tokio::test]
async fn an_unproven_sign_in_with_a_delayed_error_is_not_saved() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login/slow-error").await else {
        return;
    };
    run.script(vec![on_tab("request_login", "login"), text("not saved")])
        .await;
    run.send("slow sign in").await;
    let login = run.ticket().await;
    let submitted = run
        .answer(
            &login,
            login_values(
                &login,
                &[("identifier", ALICE.0), ("password", ALICE.1)],
                true,
            ),
        )
        .await;
    assert_eq!(submitted["next"], "unknown", "{submitted}");
    assert_eq!(submitted["saved"], false, "{submitted}");
    run.settle("not saved").await;
    let tab = result(&run.events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut rejected = false;
    for _ in 0..30 {
        let snapshot = run
            .direct_tool("browser_snapshot", json!({"tab_id":tab}))
            .await;
        if snapshot
            .as_str()
            .unwrap()
            .contains("The sign-in was rejected.")
        {
            rejected = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(rejected, "the site's delayed rejection was not observed");
    assert!(run.vault_items().await.is_empty());
    assert!(site.state()["signed_in"].is_null());
    run.finish(&[]).await;
}

#[tokio::test]
async fn a_readonly_setup_key_is_masked_and_can_be_registered_without_a_qr_code() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/security/readonly-key").await else {
        return;
    };
    let tab = result(&run.events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let snapshot = run
        .direct_tool("browser_snapshot", json!({"tab_id":tab}))
        .await;
    assert!(
        !snapshot.as_str().unwrap().contains(TOTP_SEED),
        "{snapshot}"
    );
    assert!(snapshot.as_str().unwrap().contains("Setup key"));
    let image = run
        .harness
        .request(
            "POST",
            &format!("/v1/sessions/{}/tools/browser_screenshot", run.session),
            Some(json!({"args":{"tab_id":tab}})),
        )
        .await;
    assert_eq!(
        image.body["reason"],
        "screenshot_unavailable_after_secret_fill"
    );
    let ask = api_card(&mut run, "register_authenticator", json!({"tab_id":tab})).await;
    assert!(ask.otp_register.is_some());
    assert!(run.vault_items().await.is_empty());
    let declined = run.answer(&ask, json!(false)).await;
    assert_eq!(declined["status"], "declined", "{declined}");
    assert!(run.vault_items().await.is_empty());
    run.finish(&[]).await;
}

async fn live_attempt(
    run: &mut Run,
    site: &LoginFixture,
    account: (&str, &str),
    retry_password: bool,
    saved: bool,
    provider: bool,
    no_cards: bool,
) -> Value {
    use futures_util::FutureExt;
    let tab = run.live_tab.as_ref().unwrap().clone();
    let message_id = if no_cards { "live-later" } else { "live-first" };
    run.send_text(message_id, &format!("Sign me in on the already-open tab {tab}. Complete every step using the secure cards; I will answer them. Do not stop after filling a verification code. When a tool says next=done, report completion without an extra snapshot.")).await;
    let mut steps = Vec::new();
    let mut pending = std::collections::HashSet::new();
    let mut login_count = 0;
    let mut choice_count = 0;
    let mut cards = 0;
    let mut retry_error_seen = false;
    let mut error = None;
    loop {
        let read = std::panic::AssertUnwindSafe(
            run.stream
                .next_with_timeout(std::time::Duration::from_secs(120)),
        )
        .catch_unwind()
        .await;
        let event = match read {
            Ok(event) => event,
            Err(_) => {
                error = Some("SSE event read failed (120-second live deadline)".into());
                break;
            }
        };
        match &event.kind {
            EventKind::ToolStarted { name, .. } => steps.push(json!({"tool":name})),
            EventKind::ToolEnded {
                card: _,
                tool_call_id,
                result,
                is_error,
            } => {
                steps.push(json!({"tool_call_id":tool_call_id,"result":result,"is_error":is_error}))
            }
            EventKind::Usage {
                model,
                input_tokens,
                output_tokens,
            } => steps.push(
                json!({"model":model,"input_tokens":input_tokens,"output_tokens":output_tokens}),
            ),
            EventKind::SessionSettled {} if !pending.is_empty() => {
                steps.push(json!({"settled_with_pending_cards":pending.len()}))
            }
            EventKind::TicketOpened { ticket } => {
                cards += 1;
                pending.insert(ticket.id.clone());
                let mut seen = json!({"card":ticket.kind,"summary":ticket.summary});
                let value = if let Some(login) = &ticket.login {
                    login_count += 1;
                    retry_error_seen |= login_count > 1 && login.error_text.is_some();
                    seen["fields"] = json!(login
                        .fields
                        .iter()
                        .map(|field| &field.label)
                        .collect::<Vec<_>>());
                    seen["error_text"] = json!(login.error_text);
                    if provider {
                        let alternative = login
                            .alternatives
                            .iter()
                            .find(|alt| alt.label.contains("Google"))
                            .unwrap();
                        seen["selected"] = json!(alternative.label);
                        json!({"alternative":alternative.id})
                    } else {
                        let password = if retry_password && login_count == 1 {
                            "deliberately-wrong-password"
                        } else {
                            account.1
                        };
                        let save = saved
                            && login.fields.iter().any(|field| {
                                field.kind == openside_proto::LoginFieldKind::Password
                            });
                        seen["save_to_vault"] = json!(save);
                        login_values(
                            ticket,
                            &[("identifier", account.0), ("password", password)],
                            save,
                        )
                    }
                } else if let Some(choice) = &ticket.choice {
                    let labels: &[&str] = if provider {
                        &[IDP_ACCOUNTS[1], "Allow"]
                    } else {
                        &[
                            "Text message",
                            "Lane Studio LLC",
                            "Trust this device",
                            "Accept and continue",
                        ]
                    };
                    let Some(label) = labels.get(choice_count) else {
                        error = Some("unexpected additional choice".into());
                        break;
                    };
                    choice_count += 1;
                    seen["question"] = json!(choice.question);
                    seen["options"] = json!(choice
                        .options
                        .iter()
                        .map(
                            |option| json!({"label":option.label,"description":option.description})
                        )
                        .collect::<Vec<_>>());
                    seen["selected"] = json!(label);
                    json!({"option":option_id(ticket,label)})
                } else if let Some(approval) = &ticket.approval {
                    seen["message"] = json!(approval.message);
                    seen["number"] = json!(approval.number);
                    steps.push(seen);
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    push(site, "approve").await;
                    run.events.push(event);
                    continue;
                } else if let Some(otp) = &ticket.otp {
                    seen["channel"] = json!(otp.channel);
                    json!(if account == TOM {
                        totp_now()
                    } else {
                        SMS_CODE.to_owned()
                    })
                } else if ticket.otp_register.is_some() {
                    assert!(
                        !run.vault_items()
                            .await
                            .iter()
                            .any(|item| item["has_totp"] == true),
                        "authenticator saved before approval"
                    );
                    seen["selected"] = json!("Save to vault");
                    json!(true)
                } else {
                    error = Some(format!("unexpected card {:?}", ticket.kind));
                    break;
                };
                steps.push(seen);
                let response = run
                    .harness
                    .human_request(
                        "POST",
                        &format!("/v1/tickets/{}/resolve", ticket.id),
                        Some(json!({"value":value})),
                    )
                    .await;
                if response.status != 200 {
                    error = Some(format!("card resolution returned {}", response.status));
                    break;
                }
            }
            EventKind::TicketResolved {
                card: _,
                ticket_id,
                resolution,
            } => {
                pending.remove(ticket_id);
                steps.push(json!({"resolution":resolution}));
            }
            EventKind::AssistantFinal { text, .. } => steps.push(json!({"reply":text})),
            EventKind::MessageCompleted {
                outcome, reason, ..
            } => {
                steps.push(json!({"message_outcome":outcome,"reason":reason}));
                if matches!(
                    outcome,
                    openside_proto::MessageOutcome::Failed
                        | openside_proto::MessageOutcome::Aborted
                ) {
                    error = Some(
                        reason
                            .clone()
                            .unwrap_or_else(|| format!("message {outcome:?}")),
                    );
                }
            }
            EventKind::RuntimeError { code, message, .. } => {
                error = Some(format!("{code}: {message}"));
                run.events.push(event);
                break;
            }
            EventKind::SessionSettled {} if pending.is_empty() => {
                run.events.push(event);
                break;
            }
            _ => {}
        }
        run.events.push(event);
    }
    let state = site.state();
    let expected = if provider { IDP_ACCOUNTS[1] } else { account.0 };
    let expected_cards = if no_cards {
        0
    } else if provider {
        3
    } else if account == CARA {
        6
    } else {
        2
    };
    let expected_choices = if provider {
        2
    } else if account == CARA && !no_cards {
        4
    } else {
        0
    };
    let passed = error.is_none()
        && state["signed_in"] == expected
        && cards == expected_cards
        && choice_count == expected_choices
        && (!retry_password || retry_error_seen);
    json!({"pass":passed,"cards":cards,"steps":steps,"site":state,"error":error})
}

/// Opt-in real-model QA. A shared request ledger caps all runs and reruns at 40.
#[tokio::test]
#[ignore = "uses real broker credentials and a bounded model request budget"]
async fn login_scenarios_live_model() {
    let out = std::path::PathBuf::from(
        std::env::var("OPENSIDE_LOGIN_CARDS_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/polish/LoginScenarios".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let count = out.join("live-request-count");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&count)
        .unwrap();
    let report_path = out.join("live-results.json");
    let mut report: Vec<Value> = std::fs::read(&report_path)
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).expect("existing live report"))
        .unwrap_or_default();
    let selected = std::env::var("OPENSIDE_LOGIN_LIVE_SCENARIOS").ok();
    let mut passed = true;
    for (id, account, path, retry, save, provider, later) in [
        ("S4", PAT, "/login", false, false, false, false),
        ("S3", ALICE, "/login", false, false, true, false),
        ("S8/S9/S10", TESS, "/login", false, true, false, true),
        ("S5-SMS", SAM, "/login", false, false, false, false),
        ("S5-TOTP", TOM, "/login", false, false, false, false),
        ("S6", CARA, "/login", false, false, false, false),
        ("S2", BOB, "/signin", false, true, false, false),
        ("S1/S7", ALICE, "/login", true, false, false, false),
    ] {
        if selected
            .as_ref()
            .is_some_and(|selected| !selected.split(',').any(|item| item == id))
        {
            continue;
        }
        let before = std::fs::read_to_string(&count)
            .unwrap_or_default()
            .lines()
            .count();
        if before >= 40 {
            passed = false;
            report.push(json!({"scenario":id,"pass":null,"status":"deterministic only, live not run (budget)","model_requests":0}));
            std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
            continue;
        }
        let site = LoginFixture::start().await;
        let mut run = Run::start_config(&site, path, "", Some(&count))
            .await
            .unwrap();
        let first = live_attempt(&mut run, &site, account, retry, save, provider, false).await;
        let mut entry = json!({"scenario":id,"first":first});
        if later && entry["first"]["pass"] == true {
            logout(&site).await;
            run.direct_tool(
                "browser_navigate",
                json!({"tab_id":run.live_tab,"url":format!("{}/login",site.origin)}),
            )
            .await;
            entry["later"] =
                live_attempt(&mut run, &site, account, false, false, false, true).await;
        }
        let pass = entry["first"]["pass"] == true
            && (entry["later"].is_null() || entry["later"]["pass"] == true);
        entry["pass"] = json!(pass);
        passed &= pass;
        entry["model_requests"] = json!(
            std::fs::read_to_string(&count)
                .unwrap_or_default()
                .lines()
                .count()
                - before
        );
        entry["vault"] = json!(run.vault_items().await);
        run.finish(&[]).await;
        report.push(entry);
        std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
    let requests = std::fs::read_to_string(count).unwrap().lines().count();
    eprintln!("live model requests: {requests}/40");
    assert!(requests <= 40);
    assert!(passed, "live failures: {}", json!(report));
}

/// S1, S7, S10: a wrong password shows the page's error on the retried card; the right one
/// is saved from the card's "Save to vault", and the next sign-in fills from the vault.
#[tokio::test]
async fn password_sign_in_retries_on_the_page_error_saves_and_later_fills_from_the_vault() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login").await else {
        return;
    };
    run.script(vec![
        on_tab("request_login", "login-1"),
        on_tab("request_login", "login-2"),
        text("signed in"),
    ])
    .await;
    run.send("sign in").await;
    let first = run.ticket().await;
    assert_eq!(first.kind, TicketKind::Login);
    let login = first.login.as_ref().unwrap();
    assert_eq!(login.origin, site.origin);
    assert_eq!(login.site_name, "Northwind");
    let authority = site.origin.split_once("://").unwrap().1;
    assert!(first.summary.contains(authority), "{}", first.summary);
    assert!(first.instructions.as_deref().unwrap().contains(authority));
    assert!(login.can_save && login.error_text.is_none(), "{login:?}");
    assert!(
        login
            .alternatives
            .iter()
            .any(|alt| alt.label.contains("Continue with Google")),
        "{login:?}"
    );
    let wrong = run
        .answer(
            &first,
            login_values(
                &first,
                &[("identifier", ALICE.0), ("password", "Tr0ub4dor&3")],
                true,
            ),
        )
        .await;
    assert_eq!(wrong["next"], "error", "{wrong}");
    assert_eq!(wrong["saved"], false, "{wrong}");
    assert!(wrong["error_text"]
        .as_str()
        .unwrap()
        .contains("Incorrect email or password"));
    let retry = run.ticket().await;
    let page_error = retry
        .login
        .as_ref()
        .unwrap()
        .error_text
        .clone()
        .unwrap_or_default();
    assert!(
        page_error.contains("Incorrect email or password"),
        "{retry:?}"
    );
    let right = run
        .answer(
            &retry,
            login_values(
                &retry,
                &[("identifier", ALICE.0), ("password", ALICE.1)],
                true,
            ),
        )
        .await;
    assert_eq!(
        right,
        json!({"status":"submitted","saved":true,"next":"done"})
    );
    run.settle("signed in").await;
    let state = site.state();
    assert_eq!(state["signed_in"], ALICE.0, "{state}");
    let items = run.vault_items().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["origins"], json!([site.origin]));

    logout(&site).await;
    run.script(vec![
        navigate("/login", &site, "again"),
        on_tab("request_login", "login-3"),
        text("signed in again"),
    ])
    .await;
    let events = run.quiet_turn("again", "signed in again").await;
    let filled = result(&events, "login-3");
    assert_eq!(filled["status"], "filled_from_vault", "{filled}");
    assert_eq!(filled["next"], "done", "{filled}");
    assert_eq!(site.state()["signed_in"], ALICE.0);
    assert_eq!(site.posts().len(), 3);
    run.finish(&[]).await;
}

/// S2: the username page and the password page are two cards; the password card can still
/// save the login because the daemon remembers the name typed one page earlier.
#[tokio::test]
async fn two_step_sign_in_asks_twice_and_saves_the_pair() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/signin").await else {
        return;
    };
    run.script(vec![
        on_tab("request_login", "name"),
        on_tab("request_login", "password"),
        text("signed in"),
        navigate("/signin", &site, "again"),
        on_tab("request_login", "name-again"),
        text("name again"),
    ])
    .await;
    run.send("sign in").await;
    let name = run.ticket().await;
    let login = name.login.as_ref().unwrap();
    assert_eq!(login.fields.len(), 1, "{login:?}");
    let step = run
        .answer(&name, login_values(&name, &[("identifier", BOB.0)], false))
        .await;
    assert_eq!(step["next"], "login", "{step}");
    let password = run.ticket().await;
    let login = password.login.as_ref().unwrap();
    assert!(login.can_save, "{login:?}");
    let done = run
        .answer(
            &password,
            login_values(&password, &[("password", BOB.1)], true),
        )
        .await;
    assert_eq!(
        done,
        json!({"status":"submitted","saved":true,"next":"done"})
    );
    run.settle("signed in").await;
    assert_eq!(site.state()["signed_in"], BOB.0);
    assert_eq!(run.vault_items().await.len(), 1);
    run.finish(&[]).await;
}

/// S3: "Continue with Google" is the card's provider button; the provider's account chooser
/// and consent page are choice cards.
#[tokio::test]
async fn provider_sign_in_picks_the_provider_then_the_account_then_consents() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login").await else {
        return;
    };
    run.script(vec![
        on_tab("request_login", "login"),
        on_tab("request_choice", "account"),
        on_tab("request_choice", "consent"),
        text("signed in with Google"),
    ])
    .await;
    run.send("sign in").await;
    let login = run.ticket().await;
    let google = login
        .login
        .as_ref()
        .unwrap()
        .alternatives
        .iter()
        .find(|alt| alt.label.contains("Google"))
        .expect("the provider button")
        .clone();
    assert_eq!(serde_json::to_value(google.provider).unwrap(), "google");
    let chosen = run.answer(&login, json!({"alternative":google.id})).await;
    assert_eq!(chosen["next"], "choice", "{chosen}");
    let account = run.ticket().await;
    assert_eq!(account.kind, TicketKind::Choice);
    let choice = account.choice.as_ref().unwrap();
    assert_eq!(choice.origin, site.idp_origin);
    assert_eq!(choice.question, "Choose an account");
    assert_eq!(choice.options.len(), 2, "{choice:?}");
    let picked = run
        .answer(
            &account,
            json!({"option":option_id(&account, IDP_ACCOUNTS[1])}),
        )
        .await;
    assert_eq!(picked["next"], "choice", "{picked}");
    let consent = run.ticket().await;
    let choice = consent.choice.as_ref().unwrap();
    assert!(choice.question.contains("wants to access"), "{choice:?}");
    let allowed = run
        .answer(&consent, json!({"option":option_id(&consent, "Allow")}))
        .await;
    assert_eq!(allowed["next"], "done", "{allowed}");
    run.settle("signed in with Google").await;
    let state = site.state();
    assert_eq!(state["signed_in"], IDP_ACCOUNTS[1], "{state}");
    assert_eq!(state["oauth_consent"]["allowed"], true);
    run.finish(&[]).await;
}

/// S4: the phone-approval card waits while the site does; approval continues the sign-in,
/// a denial or an expired request comes back as the page's error.
#[tokio::test]
async fn phone_approval_waits_then_continues_or_reports_denial_and_expiry() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login").await else {
        return;
    };
    let attempt = |n: &str, last: &str| {
        vec![
            navigate("/login", &site, &format!("nav-{n}")),
            on_tab("request_login", &format!("login-{n}")),
            on_tab("wait_for_approval", &format!("wait-{n}")),
            text(last),
        ]
    };
    let mut script = attempt("1", "approved");
    script.extend(attempt("2", "denied"));
    script.extend(attempt("3", "expired"));
    run.script(script).await;
    for (n, verdict, next) in [
        ("1", "approve", "done"),
        ("2", "deny", "error"),
        ("3", "expire", "error"),
    ] {
        if n != "1" {
            logout(&site).await;
        }
        run.send(&format!("attempt {n}")).await;
        let login = run.ticket().await;
        let submitted = run
            .answer(
                &login,
                login_values(&login, &[("identifier", PAT.0), ("password", PAT.1)], false),
            )
            .await;
        assert_eq!(submitted["next"], "push_approval", "{submitted}");
        let wait = run.ticket().await;
        assert_eq!(wait.kind, TicketKind::DeviceApproval);
        let approval = wait.approval.clone().expect("a phone-approval ticket");
        assert_eq!(approval.origin, site.origin);
        let authority = site.origin.split_once("://").unwrap().1;
        assert!(wait.summary.contains(authority), "{}", wait.summary);
        assert!(wait.instructions.as_deref().unwrap().contains(authority));
        assert_eq!(approval.number.as_deref(), Some("42"));
        assert!(approval.message.contains("sign-in request"), "{approval:?}");
        // The card stays pending while the phone has not answered.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let pending = run
            .harness
            .request("GET", &format!("/v1/tickets/{}", wait.id), None)
            .await;
        assert_eq!(pending.body["status"], "pending", "{}", pending.body);
        push(&site, verdict).await;
        let outcome = run.resolution(&wait).await;
        assert_eq!(outcome["status"], "done", "{outcome}");
        assert_eq!(outcome["next"], next, "{outcome}");
        if next == "error" {
            assert!(outcome["error_text"].as_str().is_some(), "{outcome}");
        }
        let last = ["approved", "denied", "expired"][n.parse::<usize>().unwrap() - 1];
        run.settle(last).await;
        let state = site.state();
        if verdict == "approve" {
            assert_eq!(state["signed_in"], PAT.0, "{state}");
        } else {
            assert!(state["signed_in"].is_null(), "{state}");
        }
    }
    run.finish(&[]).await;
}

/// S5, S8, S9: an SMS code and a code from the person's own authenticator come through the
/// code card; an authenticator the site offers during sign-in is saved only after the person
/// says yes, and the next sign-in to that site needs no card at all.
#[tokio::test]
async fn codes_enrollment_and_the_saved_authenticator() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login").await else {
        return;
    };
    let mut codes = Vec::new();
    for (account, channel, code) in [
        (SAM, "sms", SMS_CODE.to_owned()),
        (TOM, "authenticator", String::new()),
    ] {
        logout(&site).await;
        run.script(vec![
            navigate("/login", &site, &format!("nav-{channel}")),
            on_tab("request_login", &format!("login-{channel}")),
            on_tab("browser_snapshot", &format!("fields-{channel}")),
            text("code page"),
        ])
        .await;
        run.send(&format!("sign in {channel}")).await;
        let login = run.ticket().await;
        let submitted = run
            .answer(
                &login,
                login_values(
                    &login,
                    &[("identifier", account.0), ("password", account.1)],
                    false,
                ),
            )
            .await;
        assert_eq!(submitted["next"], "otp", "{submitted}");
        let events = run.settle("code page").await;
        let snapshot = result(&events, &format!("fields-{channel}"));
        let code_ref = field(snapshot, "Verification code");
        let verify = field(snapshot, "Verify");
        run.script(vec![
            tool(
                "request_otp",
                json!({"tab_id":TAB,"field_ref":code_ref,"channel":channel}),
                &format!("otp-{channel}"),
            ),
            tool(
                "browser_act",
                json!({"tab_id":TAB,"target":verify,"action":"click"}),
                &format!("verify-{channel}"),
            ),
            text("signed in"),
        ])
        .await;
        run.send(&format!("code {channel}")).await;
        let ticket = run.ticket().await;
        assert_eq!(ticket.kind, TicketKind::SecretInput);
        let otp = ticket.otp.clone().expect("a code ticket");
        assert_eq!(serde_json::to_value(otp.channel).unwrap(), channel);
        let code = if code.is_empty() { totp_now() } else { code };
        codes.push(code.clone());
        let filled = run.answer(&ticket, json!(code)).await;
        assert_eq!(filled["status"], "filled", "{filled}");
        run.settle("signed in").await;
        let state = site.state();
        assert_eq!(state["signed_in"], account.0, "{state}");
    }

    // S8: first sign-in of TESS lands on the site's authenticator setup page.
    logout(&site).await;
    run.script(vec![
        navigate("/login", &site, "nav-enroll"),
        on_tab("request_login", "login-enroll"),
        on_tab("browser_snapshot", "fields-enroll"),
        text("setup page"),
    ])
    .await;
    run.send("sign in tess").await;
    let login = run.ticket().await;
    let submitted = run
        .answer(
            &login,
            login_values(
                &login,
                &[("identifier", TESS.0), ("password", TESS.1)],
                true,
            ),
        )
        .await;
    assert_eq!(submitted["next"], "enroll_totp", "{submitted}");
    assert_eq!(submitted["saved"], true, "{submitted}");
    let events = run.settle("setup page").await;
    let snapshot = result(&events, "fields-enroll");
    let code_ref = field(snapshot, "Enter the 6-digit code from the app to confirm");
    let confirm = field(snapshot, "Confirm");
    run.script(vec![
        tool(
            "register_authenticator",
            json!({"tab_id":TAB,"field_ref":code_ref}),
            "register",
        ),
        tool(
            "browser_act",
            json!({"tab_id":TAB,"target":confirm,"action":"click"}),
            "confirm",
        ),
        text("authenticator saved"),
    ])
    .await;
    run.send("save it").await;
    let ask = run.ticket().await;
    assert_eq!(ask.kind, TicketKind::Confirm);
    let raw = serde_json::to_value(&ask).unwrap();
    assert_eq!(raw["otp_register"]["origin"], site.origin, "{raw}");
    assert_eq!(raw["otp_register"]["issuer"], "Northwind", "{raw}");
    assert!(
        site.state()["enrolled"] == false,
        "saved before the person said yes"
    );
    let registered = run.answer(&ask, json!(true)).await;
    assert_eq!(registered["status"], "registered", "{registered}");
    run.settle("authenticator saved").await;
    let state = site.state();
    assert_eq!(state["enrolled"], true, "{state}");
    assert_eq!(state["signed_in"], TESS.0, "{state}");
    let items = run.vault_items().await;
    // The login and the authenticator are separate items bound to the same origin.
    let here: Vec<_> = items
        .iter()
        .filter(|item| item["origins"] == json!([site.origin]))
        .collect();
    assert!(
        here.iter().any(|item| item["has_totp"] == true),
        "{items:?}"
    );
    assert!(
        here.iter()
            .any(|item| item["id"].as_str().unwrap().starts_with("login-")),
        "{items:?}"
    );

    // S9: the next sign-in fills the password and the code from the vault, no card.
    logout(&site).await;
    run.script(vec![
        navigate("/login", &site, "nav-later"),
        on_tab("request_login", "login-later"),
        on_tab("browser_snapshot", "fields-later"),
        text("code page again"),
    ])
    .await;
    let events = run.quiet_turn("later", "code page again").await;
    let filled = result(&events, "login-later");
    assert_eq!(filled["status"], "filled_from_vault", "{filled}");
    assert_eq!(filled["next"], "otp", "{filled}");
    let snapshot = result(&events, "fields-later");
    let code_ref = field(snapshot, "Verification code");
    let verify = field(snapshot, "Verify");
    run.script(vec![
        tool(
            "request_otp",
            json!({"tab_id":TAB,"field_ref":code_ref}),
            "otp-later",
        ),
        tool(
            "browser_act",
            json!({"tab_id":TAB,"target":verify,"action":"click"}),
            "verify-later",
        ),
        text("signed in from the vault"),
    ])
    .await;
    let events = run
        .quiet_turn("code later", "signed in from the vault")
        .await;
    assert_eq!(*result(&events, "otp-later"), json!({"status":"filled"}));
    let state = site.state();
    assert_eq!(state["signed_in"], TESS.0, "{state}");
    assert_eq!(state["via"], "totp", "{state}");
    run.finish(&codes).await;
}

/// S6: verification method, profile, "trust this device?" and updated terms are each a
/// choice card, answered in order, with the SMS code card in between.
#[tokio::test]
async fn mid_login_choices_are_cards() {
    let site = LoginFixture::start().await;
    let Some(mut run) = Run::start(&site, "/login").await else {
        return;
    };
    run.script(vec![
        on_tab("request_login", "login"),
        on_tab("request_choice", "method"),
        on_tab("browser_snapshot", "fields"),
        text("code page"),
    ])
    .await;
    run.send("sign in").await;
    let login = run.ticket().await;
    let submitted = run
        .answer(
            &login,
            login_values(
                &login,
                &[("identifier", CARA.0), ("password", CARA.1)],
                false,
            ),
        )
        .await;
    assert_eq!(submitted["next"], "choice", "{submitted}");
    let method = run.ticket().await;
    let choice = method.choice.as_ref().unwrap();
    assert_eq!(choice.question, "Choose how to verify");
    assert_eq!(choice.options.len(), 2, "{choice:?}");
    let sms = run
        .answer(
            &method,
            json!({"option":option_id(&method, "Text message")}),
        )
        .await;
    assert_eq!(sms["next"], "otp", "{sms}");
    let events = run.settle("code page").await;
    let snapshot = result(&events, "fields");
    let code_ref = field(snapshot, "Verification code");
    let verify = field(snapshot, "Verify");
    run.script(vec![
        tool(
            "request_otp",
            json!({"tab_id":TAB,"field_ref":code_ref,"channel":"sms"}),
            "otp",
        ),
        tool(
            "browser_act",
            json!({"tab_id":TAB,"target":verify,"action":"click"}),
            "verify",
        ),
        on_tab("request_choice", "profile"),
        on_tab("request_choice", "trust"),
        on_tab("request_choice", "terms"),
        text("signed in"),
    ])
    .await;
    run.send("code").await;
    let code = run.ticket().await;
    run.answer(&code, json!(SMS_CODE)).await;
    for (question, option, next) in [
        (
            "Which account do you want to use?",
            "Lane Studio LLC",
            "choice",
        ),
        ("Trust this device?", "Trust this device", "choice"),
        (
            "Please accept the updated terms",
            "Accept and continue",
            "done",
        ),
    ] {
        let ticket = run.ticket().await;
        let choice = ticket.choice.as_ref().unwrap();
        assert_eq!(choice.question, question, "{choice:?}");
        assert!(choice.options.len() >= 2, "{choice:?}");
        let answered = run
            .answer(&ticket, json!({"option":option_id(&ticket, option)}))
            .await;
        assert_eq!(answered["next"], next, "{question}: {answered}");
    }
    run.settle("signed in").await;
    let state = site.state();
    assert_eq!(state["signed_in"], CARA.0, "{state}");
    assert_eq!(state["method"], "sms");
    assert_eq!(state["profile"], "business");
    assert_eq!(state["trusted"], true);
    assert_eq!(state["terms"], true);
    run.finish(&[SMS_CODE.to_owned()]).await;
}

/// The DevTools session of the UI tab, for viewport and colour-scheme emulation; the overrides
/// last as long as the socket, and captures go through it so they come out at the emulated size.
struct Emulation {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    session: String,
    next: u64,
}

impl Emulation {
    async fn attach(ws_url: &str, tab: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let mut emulation = Self {
            socket,
            session: String::new(),
            next: 0,
        };
        let attached = emulation
            .call(
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
            )
            .await;
        emulation.session = attached["sessionId"].as_str().unwrap().to_owned();
        emulation
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id":id,"method":method,"params":params});
        if !self.session.is_empty() {
            request["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        loop {
            let next = tokio::time::timeout(std::time::Duration::from_secs(10), self.socket.next())
                .await
                .expect("CDP reply")
                .unwrap()
                .unwrap();
            if let Message::Text(body) = next {
                let reply: Value = serde_json::from_str(body.as_str()).unwrap();
                if reply["id"] == id {
                    assert!(reply["error"].is_null(), "{method}: {reply}");
                    return reply["result"].clone();
                }
            }
        }
    }
}

/// The web app in its own headless Chromium, signed in with a paired token.
struct Ui {
    browser: openside_browser::Browser,
    tab: String,
    emulation: Emulation,
    out: std::path::PathBuf,
}

const UI_OWNER: &str = "login-cards";

impl Ui {
    async fn open(run: &Run, out: std::path::PathBuf) -> Self {
        let paired = run
            .harness
            .request(
                "POST",
                "/v1/devices/pair",
                Some(
                    json!({"name":"login cards","scopes":["read","steer","approve","human_step"]}),
                ),
            )
            .await;
        assert_eq!(paired.status, 201, "{}", paired.body);
        let token = paired.body["token"].as_str().unwrap().to_owned();
        let port = std::fs::read_to_string(run.harness.data_dir.join("opensided.port")).unwrap();
        let origin = format!("http://127.0.0.1:{}", port.trim());
        let executable =
            openside_config::discover_browser(&openside_config::Config::default()).unwrap();
        let (browser, _events) =
            openside_browser::Browser::launch(openside_browser::BrowserConfig::new(executable))
                .await
                .unwrap();
        let tab = browser
            .create_agent_tab(UI_OWNER, &format!("{origin}/"))
            .await
            .unwrap()
            .id;
        let emulation = Emulation::attach(&browser.endpoint().ws_url, &tab).await;
        let ui = Self {
            browser,
            tab,
            emulation,
            out,
        };
        ui.wait(
            "the token screen",
            "!!document.querySelector('.token-entry')",
        )
        .await;
        ui.eval(&format!(
            "localStorage.setItem('openside.token', {}); true",
            json!(token)
        ))
        .await;
        let _ = ui
            .browser
            .navigate(
                UI_OWNER,
                &ui.tab,
                &format!("{origin}/sessions/{}", run.session),
            )
            .await;
        ui.wait("the conversation", "!!document.querySelector('.shell')")
            .await;
        ui
    }

    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

    async fn wait(&self, what: &str, predicate: &str) {
        for _ in 0..600 {
            if self.eval(predicate).await == true {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let text = self
            .eval("document.body ? document.body.innerText : ''")
            .await;
        panic!("timed out waiting for {what}; page text: {text}");
    }

    /// The pending card of `ticket` in phone and desktop sizes, light and dark.
    async fn capture(&mut self, name: &str, ticket: &Ticket) {
        use base64::Engine;
        let card = format!(
            "document.querySelector('.oc-card[data-ticket=\"{}\"]')",
            ticket.id
        );
        self.wait(name, &format!("!!{card}")).await;
        self.eval(&format!("{card}.querySelector('.oc-pill')?.click(); true"))
            .await;
        for scheme in ["light", "dark"] {
            self.emulation
                .call(
                    "Emulation.setEmulatedMedia",
                    json!({"features":[{"name":"prefers-color-scheme","value":scheme}]}),
                )
                .await;
            for (size, width, height, scale, mobile) in [
                ("phone", 390, 844, 2.0, true),
                ("desktop", 1440, 900, 1.0, false),
            ] {
                self.emulation
                    .call(
                        "Emulation.setDeviceMetricsOverride",
                        json!({"width":width,"height":height,"deviceScaleFactor":scale,"mobile":mobile}),
                    )
                    .await;
                self.eval(&format!(
                    "{card}.scrollIntoView({{block:'center'}}); document.fonts.ready.then(() => true)"
                ))
                .await;
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                let bounds = self.eval(&format!("(() => {{ const r = {card}.getBoundingClientRect(); return {{x:r.x,y:r.y,width:r.width,height:r.height,viewport_width:innerWidth,viewport_height:innerHeight,scroll_width:document.documentElement.scrollWidth}}; }})()")).await;
                assert!(
                    bounds["scroll_width"].as_f64().unwrap() <= f64::from(width),
                    "{name} overflows horizontally: {bounds}"
                );
                assert!(
                    bounds["x"].as_f64().unwrap() >= 0.0
                        && bounds["x"].as_f64().unwrap() + bounds["width"].as_f64().unwrap()
                            <= f64::from(width),
                    "{name} card leaves the viewport: {bounds}"
                );
                let shot = self
                    .emulation
                    .call("Page.captureScreenshot", json!({"format":"png"}))
                    .await;
                let png = base64::engine::general_purpose::STANDARD
                    .decode(shot["data"].as_str().unwrap())
                    .unwrap();
                let path = self.out.join(format!("{name}-{size}-{scheme}.png"));
                std::fs::write(&path, png).unwrap();
                std::fs::write(
                    path.with_extension("json"),
                    serde_json::to_vec_pretty(&bounds).unwrap(),
                )
                .unwrap();
                eprintln!("captured {}", path.display());
            }
        }
    }
}

async fn api_card(run: &mut Run, name: &str, args: Value) -> Ticket {
    let response = run
        .harness
        .request(
            "POST",
            &format!("/v1/sessions/{}/tools/{name}", run.session),
            Some(json!({"args":args})),
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.body["status"], "deferred", "{}", response.body);
    let ticket = run.ticket().await;
    assert_eq!(json!(ticket.id), response.body["ticket_id"]);
    ticket
}

/// Visual QA through the public tool/ticket APIs, without sidecar restarts that
/// would add artificial crash warnings to the real cards.
#[tokio::test]
#[ignore = "visual QA harness; build web/dist first"]
async fn login_cards_capture() {
    use futures_util::FutureExt;
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    assert!(
        dist.join("index.html").is_file(),
        "web/dist is missing; run pnpm -C web build"
    );
    let out = std::path::PathBuf::from(
        std::env::var("OPENSIDE_LOGIN_CARDS_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/login-cards".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let site = LoginFixture::start().await;
    let config = format!("[ui]\ndir = {}\n", json!(dist.canonicalize().unwrap()));
    let mut run = Run::start_with(&site, "/login", &config)
        .await
        .expect("headless Chromium");
    let tab = result(&run.events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut ui = Ui::open(&run, out).await;
    let mut codes = vec![SMS_CODE.to_owned()];
    let captured = std::panic::AssertUnwindSafe(async {
        let first = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        ui.capture("login", &first).await;
        run.answer(
            &first,
            login_values(
                &first,
                &[("identifier", ALICE.0), ("password", "Tr0ub4dor&3")],
                false,
            ),
        )
        .await;
        let retry = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        ui.capture("login-error", &retry).await;
        run.answer(
            &retry,
            login_values(&retry, &[("identifier", PAT.0), ("password", PAT.1)], false),
        )
        .await;
        let wait = api_card(&mut run, "wait_for_approval", json!({"tab_id":tab})).await;
        ui.capture("approval", &wait).await;
        push(&site, "approve").await;
        run.resolution(&wait).await;

        logout(&site).await;
        run.direct_tool(
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{}/login",site.origin)}),
        )
        .await;
        let login = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        let google = login
            .login
            .as_ref()
            .unwrap()
            .alternatives
            .iter()
            .find(|alt| alt.label.contains("Google"))
            .unwrap()
            .id
            .clone();
        run.answer(&login, json!({"alternative":google})).await;
        let account = api_card(&mut run, "request_choice", json!({"tab_id":tab})).await;
        ui.capture("choice-account", &account).await;
        run.answer(
            &account,
            json!({"option":option_id(&account,IDP_ACCOUNTS[0])}),
        )
        .await;
        let consent = api_card(&mut run, "request_choice", json!({"tab_id":tab})).await;
        ui.capture("choice-consent", &consent).await;
        run.answer(&consent, json!({"option":option_id(&consent,"Allow")}))
            .await;

        logout(&site).await;
        run.direct_tool(
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{}/signin",site.origin)}),
        )
        .await;
        let name = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        ui.capture("login-identifier", &name).await;
        run.answer(&name, login_values(&name, &[("identifier", BOB.0)], false))
            .await;
        let password = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        ui.capture("login-password", &password).await;
        run.answer(
            &password,
            login_values(&password, &[("password", BOB.1)], false),
        )
        .await;

        for (account, name, channel) in [
            (SAM, "otp-sms", "sms"),
            (TOM, "otp-authenticator", "authenticator"),
        ] {
            logout(&site).await;
            run.direct_tool(
                "browser_navigate",
                json!({"tab_id":tab,"url":format!("{}/login",site.origin)}),
            )
            .await;
            let login = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
            run.answer(
                &login,
                login_values(
                    &login,
                    &[("identifier", account.0), ("password", account.1)],
                    false,
                ),
            )
            .await;
            let snapshot = run
                .direct_tool("browser_snapshot", json!({"tab_id":tab}))
                .await;
            let code_ref = field(&snapshot, "Verification code");
            let verify = field(&snapshot, "Verify");
            let code = api_card(
                &mut run,
                "request_otp",
                json!({"tab_id":tab,"field_ref":code_ref,"channel":channel}),
            )
            .await;
            ui.capture(name, &code).await;
            let value = if account == TOM {
                totp_now()
            } else {
                SMS_CODE.to_owned()
            };
            codes.push(value.clone());
            run.answer(&code, json!(value)).await;
            run.direct_tool(
                "browser_act",
                json!({"tab_id":tab,"target":verify,"action":"click"}),
            )
            .await;
        }

        logout(&site).await;
        run.direct_tool(
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{}/login",site.origin)}),
        )
        .await;
        let login = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        run.answer(
            &login,
            login_values(
                &login,
                &[("identifier", CARA.0), ("password", CARA.1)],
                false,
            ),
        )
        .await;
        let method = api_card(&mut run, "request_choice", json!({"tab_id":tab})).await;
        ui.capture("choice-method", &method).await;
        run.answer(&method, json!({"option":option_id(&method,"Text message")}))
            .await;
        let snapshot = run
            .direct_tool("browser_snapshot", json!({"tab_id":tab}))
            .await;
        let code_ref = field(&snapshot, "Verification code");
        let verify = field(&snapshot, "Verify");
        let code = api_card(
            &mut run,
            "request_otp",
            json!({"tab_id":tab,"field_ref":code_ref,"channel":"sms"}),
        )
        .await;
        run.answer(&code, json!(SMS_CODE)).await;
        run.direct_tool(
            "browser_act",
            json!({"tab_id":tab,"target":verify,"action":"click"}),
        )
        .await;
        for (name, label) in [
            ("choice-profile", "Lane Studio LLC"),
            ("choice-trust", "Trust this device"),
            ("choice-terms", "Accept and continue"),
        ] {
            let choice = api_card(&mut run, "request_choice", json!({"tab_id":tab})).await;
            ui.capture(name, &choice).await;
            run.answer(&choice, json!({"option":option_id(&choice,label)}))
                .await;
        }

        logout(&site).await;
        run.direct_tool(
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{}/login",site.origin)}),
        )
        .await;
        let login = api_card(&mut run, "request_login", json!({"tab_id":tab})).await;
        run.answer(
            &login,
            login_values(
                &login,
                &[("identifier", TESS.0), ("password", TESS.1)],
                false,
            ),
        )
        .await;
        let snapshot = run
            .direct_tool("browser_snapshot", json!({"tab_id":tab}))
            .await;
        assert!(!snapshot.to_string().contains(GROUPED_TOTP_SEED));
        let code_ref = field(&snapshot, "Enter the 6-digit code from the app to confirm");
        let ask = api_card(
            &mut run,
            "register_authenticator",
            json!({"tab_id":tab,"field_ref":code_ref}),
        )
        .await;
        ui.capture("enroll", &ask).await;
        run.answer(&ask, json!(false)).await;
    })
    .catch_unwind()
    .await;
    ui.browser.shutdown().await.expect("UI Chromium shutdown");
    if let Err(error) = captured {
        run.harness.kill().await;
        std::panic::resume_unwind(error);
    }
    run.finish(&codes).await;
}
