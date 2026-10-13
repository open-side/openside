//! Chat-channel round trips against the real daemon and the scripted Python sidecar.
//!
//! The bot talks to the daemon only through a recording proxy, so every scenario also
//! proves (G-5) that adapters use nothing but public `/v1/*` routes.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../openside-channels/tests/support/mod.rs"]
mod support;

use std::{collections::HashMap, net::SocketAddr, path::PathBuf};

use ed25519_dalek::SigningKey;
use openside_channels::{start, Running};
use openside_e2e::Harness;
use openside_proto::{EventKind, ROUTES};
use serde_json::{json, Value};
use support::*;
use tokio_util::sync::CancellationToken;

const BIN: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CHAT: i64 = 100;
const USER: i64 = 7;

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}

struct Rig {
    harness: Harness,
    proxy: RecordingProxy,
    token_file: PathBuf,
    agent_id: String,
    home_session_id: String,
}

impl Rig {
    async fn start(script: Value) -> Option<Self> {
        Self::start_with(script, |_| None).await
    }

    /// Like `start`, but `rewrite` may replace the script once the temp root (and so the
    /// path of the file the agent will write) is known, before the daemon launches.
    async fn start_with(
        script: Value,
        rewrite: impl FnOnce(&std::path::Path) -> Option<Value>,
    ) -> Option<Self> {
        let mut harness = Harness::prepare(BIN, &script)?;
        // The bot never picks a permission mode; sessions get the daemon default, which
        // the harness sets to `full`. Use the product default (`guard`) so writes outside
        // the workspace open approval tickets.
        let config = harness.root().join("config/openside/config.toml");
        let text = std::fs::read_to_string(&config)
            .unwrap()
            .replace("mode = \"full\"", "mode = \"guard\"");
        std::fs::write(&config, text).unwrap();
        if let Some(script) = rewrite(harness.root()) {
            harness.replace_script(&script);
        }
        harness.launch().await;
        // The chief delegates acting tools; channel round trips exercise an assigned teammate.
        let agent = harness
            .request(
                "POST",
                "/v1/agents",
                Some(json!({
                    "name": "Channel worker",
                    "persona": {
                        "role": "Chat inbox assistant",
                        "responsibilities": ["Handle messages received by this channel"]
                    }
                })),
            )
            .await;
        assert_eq!(agent.status, 201, "teammate fixture: {}", agent.body);
        let agent_id = agent.body["id"].as_str().unwrap().to_owned();
        let home_session_id = agent.body["home_session_id"].as_str().unwrap().to_owned();
        let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
        let upstream: SocketAddr = format!("127.0.0.1:{}", port.trim()).parse().unwrap();
        let proxy = RecordingProxy::start(upstream).await;
        // The real `pair` path: minted over the Unix socket, stored 0600, never printed.
        let token_file = harness.root().join("channels.token");
        {
            // A pre-existing world-readable file must end up owner-only once the token is in it.
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&token_file, "stale").unwrap();
            std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let device = openside_channels::pair(&harness.socket, "channel bot", &token_file).await;
        match device {
            Ok(_) => {}
            Err(error) => {
                harness.kill().await;
                panic!("pairing failed: {error}");
            }
        }
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "pair must tighten an existing token file");
        }
        Some(Self {
            harness,
            proxy,
            token_file,
            agent_id,
            home_session_id,
        })
    }

    fn outside(&self) -> PathBuf {
        self.harness.root().join("outside.txt")
    }

    fn load(&self, top: &str, tables: &str) -> openside_config::Loaded {
        let dir = self.harness.root();
        let config = format!(
            "[channels]\napi_url = \"{}\"\ntoken_file = \"{}\"\nstate_dir = \"{}\"\nedit_interval_ms = 200\n{top}\n{tables}",
            self.proxy.url,
            self.token_file.display(),
            dir.join("channels-state").display(),
        );
        let path = dir.join("bot.toml");
        std::fs::write(&path, config).unwrap();
        let mut loaded = openside_config::load(Some(&path)).unwrap_or_else(|e| panic!("{e}"));
        for account in &mut loaded.config.channels.accounts {
            account.agent_id = Some(self.agent_id.clone());
        }
        loaded
    }

    async fn bot(&self, top: &str, tables: &str, env: &[(&str, &str)]) -> Bot {
        let loaded = self.load(top, tables);
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let lookup = move |name: &str| env.get(name).cloned();
        let shutdown = CancellationToken::new();
        let running = start(&loaded, &lookup, shutdown.clone())
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        Bot {
            shutdown,
            running: Some(running),
        }
    }

    async fn sessions(&self) -> Vec<Value> {
        let response = self.harness.request("GET", "/v1/sessions", None).await;
        response.body["sessions"]
            .as_array()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter(|session| {
                        session["agent_id"].as_str() == Some(self.agent_id.as_str())
                            && session["id"].as_str() != Some(self.home_session_id.as_str())
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn pending(&self) -> Vec<Value> {
        let response = self
            .harness
            .request("GET", "/v1/tickets?status=pending", None)
            .await;
        response.body["tickets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    async fn pending_ticket(&self) -> Value {
        for _ in 0..200 {
            if let Some(ticket) = self.pending().await.into_iter().next() {
                return ticket;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no pending ticket");
    }

    /// Texts the daemon received as user messages in a session.
    async fn user_messages(&self, session: &str) -> Vec<String> {
        let last = self
            .harness
            .request("GET", &format!("/v1/sessions/{session}"), None)
            .await
            .body["last_seq"]
            .as_u64()
            .unwrap();
        let mut stream = self.harness.events(session, 0).await;
        let mut texts = Vec::new();
        loop {
            let event = stream.next().await;
            if let EventKind::MessageQueued {
                text,
                author_kind,
                from_agent,
                ..
            } = event.kind
            {
                if author_kind == openside_proto::MessageAuthorKind::Person {
                    assert_eq!(from_agent, None);
                    texts.push(text);
                }
            }
            if event.seq >= last {
                return texts;
            }
        }
    }

    /// G-5: every request the bot made is a documented public route, and the listed
    /// routes were actually exercised.
    fn assert_public_api_only(&self, expected: &[(&str, &str)]) {
        let requests = self.proxy.requests();
        assert!(!requests.is_empty(), "the bot made no daemon request");
        for (method, path) in &requests {
            assert!(
                is_public_route(method, path),
                "non-public daemon route used: {method} {path}"
            );
            assert!(path.starts_with("/v1/"), "{path}");
        }
        for (method, template) in expected {
            let hit = requests.iter().any(|(m, path)| {
                m == method && {
                    let route = ROUTES
                        .iter()
                        .find(|r| r.path == *template)
                        .unwrap_or_else(|| panic!("{template} is not a route"));
                    let (t, a): (Vec<_>, Vec<_>) =
                        (route.path.split('/').collect(), path.split('/').collect());
                    t.len() == a.len()
                        && t.iter().zip(&a).all(|(t, a)| t.starts_with('{') || t == a)
                }
            });
            assert!(
                hit,
                "expected the bot to call {method} {template}; saw {requests:?}"
            );
        }
    }

    async fn finish(mut self, bot: Bot) {
        bot.stop().await;
        self.harness.kill().await;
    }
}

struct Bot {
    shutdown: CancellationToken,
    running: Option<Running>,
}

impl Bot {
    async fn stop(mut self) {
        self.shutdown.cancel();
        if let Some(running) = self.running.take() {
            running.join().await;
        }
    }
}

fn telegram(base: &str) -> String {
    format!(
        "[channels.telegram]\nenabled = true\napi_base = \"{base}\"\ntoken_env = \"TG_TOKEN\"\nallowed_chats = [{CHAT}]\nallowed_users = [{USER}]\npoll_timeout_secs = 1\n"
    )
}

const TG_ENV: &[(&str, &str)] = &[("TG_TOKEN", "tg-secret")];

fn count(fake: &FakeTelegram, needle: &str) -> usize {
    fake.messages()
        .iter()
        .filter(|m| m.text.contains(needle))
        .count()
}

fn short(session: &Value) -> String {
    session["id"].as_str().unwrap()[..8].to_owned()
}

#[tokio::test]
async fn telegram_thread_is_a_session_commands_work_and_mapping_survives_bot_restart() {
    let Some(rig) =
        Rig::start(json!({"responses":[text("first reply"), text("second reply")]})).await
    else {
        return;
    };
    let fake = FakeTelegram::start("tg-secret").await;
    let tables = telegram(&fake.base);
    let bot = rig.bot("", &tables, TG_ENV).await;

    // A plain message starts a session; the reply streams into one chat message.
    fake.push_text(CHAT, USER, "hello");
    let reply = fake.wait_text("first reply").await;
    assert_eq!(reply.chat, CHAT.to_string());
    let sessions = rig.sessions().await;
    assert_eq!(sessions.len(), 1);
    let first = sessions[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(rig.user_messages(&first).await, ["hello"]);

    // The same thread keeps talking to the same session (follow-up, not a new session).
    fake.push_text(CHAT, USER, "again");
    fake.wait_text("second reply").await;
    assert_eq!(rig.sessions().await.len(), 1);
    assert_eq!(rig.user_messages(&first).await, ["hello", "again"]);

    fake.push_text(CHAT, USER, "/sessions");
    let listing = fake.wait_text("this thread").await;
    assert!(
        listing.text.contains(&short(&sessions[0])),
        "{}",
        listing.text
    );

    fake.push_text(CHAT, USER, "/usage");
    let usage = fake
        .wait_message("usage reply", |m| m.text.to_lowercase().contains("usage"))
        .await;
    assert!(
        usage.text.starts_with("Usage for this session")
            || usage.text.starts_with("No model usage recorded"),
        "{}",
        usage.text
    );

    // /model applies to the next session of this thread; /new starts it right away.
    fake.push_text(CHAT, USER, "/model script");
    fake.wait_text("Model set to script").await;
    fake.push_text(CHAT, USER, "/new");
    fake.wait_text("Started a new session").await;
    let sessions = rig.sessions().await;
    assert_eq!(sessions.len(), 2);
    let second = sessions
        .iter()
        .find(|s| s["id"] != first.as_str())
        .unwrap()
        .clone();
    assert_eq!(second["model"], "script");
    fake.push_text(CHAT, USER, "hello B");
    // A brand-new session replays the script from its start: a second "first reply".
    eventually("second thread reply", WAIT, || {
        (count(&fake, "first reply") == 2).then_some(())
    })
    .await;
    let second_id = second["id"].as_str().unwrap().to_owned();
    assert_eq!(rig.user_messages(&second_id).await, ["hello B"]);

    // Restart the bot: the thread keeps its session and old replies are not re-posted.
    bot.stop().await;
    let bot = rig.bot("", &tables, TG_ENV).await;
    fake.push_text(CHAT, USER, "again B");
    eventually("reply after restart", WAIT, || {
        (count(&fake, "second reply") == 2).then_some(())
    })
    .await;
    assert_eq!(
        rig.sessions().await.len(),
        2,
        "restart must not create a session"
    );
    assert_eq!(rig.user_messages(&second_id).await, ["hello B", "again B"]);
    assert_eq!(
        count(&fake, "first reply"),
        2,
        "restart replayed an old reply"
    );

    fake.push_text(CHAT, USER, "/stop");
    fake.wait_text("Stopped.").await;
    let status = rig
        .harness
        .request("GET", &format!("/v1/sessions/{second_id}"), None)
        .await;
    assert_eq!(status.body["status"], "stopped");

    rig.assert_public_api_only(&[
        ("POST", "/v1/sessions"),
        ("GET", "/v1/sessions"),
        ("GET", "/v1/sessions/{id}"),
        ("POST", "/v1/sessions/{id}/messages"),
        ("GET", "/v1/sessions/{id}/events"),
        ("POST", "/v1/sessions/{id}/stop"),
    ]);
    rig.finish(bot).await;
}

#[tokio::test]
async fn approval_card_buttons_reject_then_approve_resume_the_session() {
    let Some(rig) = Rig::start_with(json!({"responses":[]}), |root| {
        Some(json!({"responses":[
            tool("write", json!({"path": root.join("outside.txt"), "content": "approved by chat"}), "w1"),
            text("write finished")
        ]}))
    })
    .await
    else {
        return;
    };
    let fake = FakeTelegram::start("tg-secret").await;
    let bot = rig.bot("", &telegram(&fake.base), TG_ENV).await;

    // --- first session: Reject
    fake.push_text(CHAT, USER, "please write the report");
    let card = fake
        .wait_message("approval card", |m| m.text.contains("Approval needed"))
        .await;
    let ticket = rig.pending_ticket().await;
    let id = ticket["id"].as_str().unwrap().to_owned();
    assert!(card.text.contains("write"), "{}", card.text);
    assert_eq!(
        card.buttons,
        vec![
            ("Approve".to_owned(), format!("a:{id}")),
            ("Reject".to_owned(), format!("r:{id}"))
        ]
    );
    assert!(!rig.outside().exists());

    // A press from a user outside the allow-list is ignored entirely.
    let before = fake.calls("answerCallbackQuery").len();
    let disallowed = fake.push_callback(CHAT, 8, card.id, &format!("a:{id}"));
    let allowed = fake.push_callback(CHAT, USER, card.id, &format!("r:{id}"));
    let settled = fake
        .wait_message("rejected card", |m| {
            m.id == card.id && m.text.contains("Rejected by user 7")
        })
        .await;
    assert!(
        settled.buttons.is_empty(),
        "buttons must be removed once decided"
    );
    // The bot edits the card before answering the press, so wait for the allowed press's answer.
    eventually("allowed press answer", WAIT, || {
        fake.calls("answerCallbackQuery")
            .iter()
            .any(|c| c["callback_query_id"] == allowed)
            .then_some(())
    })
    .await;
    let answered = fake.calls("answerCallbackQuery");
    assert_eq!(answered.len(), before + 1);
    assert!(
        !answered
            .iter()
            .any(|c| c["callback_query_id"] == disallowed),
        "the disallowed press must not be answered"
    );
    let rejected = rig
        .harness
        .request("GET", &format!("/v1/tickets/{id}"), None)
        .await;
    assert_eq!(rejected.body["status"], "rejected");
    fake.wait_text("write finished").await;
    assert!(!rig.outside().exists(), "a rejected write must not run");

    // Pressing the stale card again is refused politely.
    fake.push_callback(CHAT, USER, card.id, &format!("a:{id}"));
    eventually("stale press answer", WAIT, || {
        fake.calls("answerCallbackQuery")
            .iter()
            .any(|c| c["text"] == "Already handled.")
            .then_some(())
    })
    .await;

    // --- /new, then Approve
    fake.push_text(CHAT, USER, "/new");
    fake.wait_text("Started a new session").await;
    fake.push_text(CHAT, USER, "try again");
    let card = fake
        .wait_message("second approval card", |m| {
            m.text.contains("Approval needed") && m.id != settled.id
        })
        .await;
    let ticket = rig.pending_ticket().await;
    let id = ticket["id"].as_str().unwrap().to_owned();
    fake.push_callback(CHAT, USER, card.id, &format!("a:{id}"));
    fake.wait_message("approved card", |m| {
        m.id == card.id && m.text.contains("Approved by user 7")
    })
    .await;
    eventually("file written after approval", WAIT, || {
        rig.outside().exists().then_some(())
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(rig.outside()).unwrap(),
        "approved by chat"
    );
    let resolved = rig
        .harness
        .request("GET", &format!("/v1/tickets/{id}"), None)
        .await;
    assert_eq!(resolved.body["status"], "resolved");
    eventually("session resumed", WAIT, || {
        (count(&fake, "write finished") == 2).then_some(())
    })
    .await;

    rig.assert_public_api_only(&[
        ("GET", "/v1/tickets/{id}"),
        ("POST", "/v1/tickets/{id}/reject"),
        ("POST", "/v1/tickets/{id}/resolve"),
    ]);
    rig.finish(bot).await;
}

#[tokio::test]
async fn info_ticket_options_become_buttons_and_free_form_questions_use_answer() {
    let Some(rig) = Rig::start(json!({"responses":[
        tool("ask_user", json!({"question":"Choose a color","options":["blue","green"]}), "ask1"),
        text("blue selected"),
        tool("ask_user", json!({"question":"Name a fruit"}), "ask2"),
        text("fruit noted")
    ]}))
    .await
    else {
        return;
    };
    let fake = FakeTelegram::start("tg-secret").await;
    let bot = rig.bot("", &telegram(&fake.base), TG_ENV).await;

    fake.push_text(CHAT, USER, "ask me something");
    let card = fake
        .wait_message("option card", |m| {
            m.text.contains("Question from the agent")
        })
        .await;
    assert!(card.text.contains("Choose a color"));
    let id = rig.pending_ticket().await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let labels: Vec<_> = card
        .buttons
        .iter()
        .map(|(label, _)| label.as_str())
        .collect();
    assert_eq!(labels, ["blue", "green", "Dismiss"]);
    assert_eq!(card.buttons[0].1, format!("o:{id}:0"));
    fake.push_callback(CHAT, USER, card.id, &card.buttons[0].1);
    fake.wait_text("blue selected").await;
    let ticket = rig
        .harness
        .request("GET", &format!("/v1/tickets/{id}"), None)
        .await;
    assert_eq!(ticket.body["resolution"]["answer"], "blue");
    fake.wait_message("answered card", |m| {
        m.id == card.id && m.text.contains("Answered: blue") && m.buttons.is_empty()
    })
    .await;

    // Nothing pending: /answer says so instead of guessing.
    fake.push_text(CHAT, USER, "/answer anything");
    fake.wait_text("No question is waiting").await;

    fake.push_text(CHAT, USER, "one more");
    let card = fake
        .wait_message("free-form card", |m| m.text.contains("Name a fruit"))
        .await;
    assert!(card.text.contains("/answer <your answer>"), "{}", card.text);
    let id = rig.pending_ticket().await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    fake.push_text(CHAT, USER, "/answer purple fig");
    fake.wait_text("Answer sent.").await;
    fake.wait_text("fruit noted").await;
    let ticket = rig
        .harness
        .request("GET", &format!("/v1/tickets/{id}"), None)
        .await;
    assert_eq!(ticket.body["resolution"]["answer"], "purple fig");
    rig.assert_public_api_only(&[("GET", "/v1/tickets"), ("POST", "/v1/tickets/{id}/resolve")]);
    rig.finish(bot).await;
}

#[tokio::test]
async fn secret_input_tickets_are_never_answerable_from_chat() {
    let Some(rig) = Rig::start(json!({"responses":[
        tool("request_human_step", json!({
            "kind": "secret_input", "summary": "Enter the verification code",
            "tab_id": "tab-1", "origin": "https://example.test", "field_ref": "e1"}), "secret1"),
        text("never reached")
    ]}))
    .await
    else {
        return;
    };
    let rig = rig;
    let fake = FakeTelegram::start("tg-secret").await;
    let bot = rig.bot("", &telegram(&fake.base), TG_ENV).await;

    fake.push_text(CHAT, USER, "log me in");
    let card = fake
        .wait_message("secret notice", |m| {
            m.text.contains("Secret input requested")
        })
        .await;
    let id = rig.pending_ticket().await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        card.buttons.is_empty(),
        "a secret ticket must not offer chat buttons"
    );
    assert!(card.text.contains("web UI"), "{}", card.text);
    assert!(
        card.text
            .contains(&format!("openside ticket resolve {id} -")),
        "{}",
        card.text
    );

    // A forged button press and /answer both fail to touch the ticket.
    fake.push_callback(CHAT, USER, card.id, &format!("a:{id}"));
    eventually("forged press refused", WAIT, || {
        fake.calls("answerCallbackQuery")
            .iter()
            .any(|c| c["text"] == "That request cannot be answered from chat.")
            .then_some(())
    })
    .await;
    fake.push_text(CHAT, USER, "/answer canary-4242");
    fake.wait_text("No question is waiting").await;
    let ticket = rig
        .harness
        .request("GET", &format!("/v1/tickets/{id}"), None)
        .await;
    assert_eq!(ticket.body["status"], "pending");
    assert!(fake
        .messages()
        .iter()
        .all(|m| !m.text.contains("never reached")));
    rig.harness.assert_no_canaries(&["canary-4242"]);
    rig.finish(bot).await;
}

#[tokio::test]
async fn voice_message_is_transcribed_locally_and_the_transcript_reaches_the_session() {
    let Some(rig) = Rig::start(json!({"responses":[text("first reply")]})).await else {
        return;
    };
    use std::os::unix::fs::PermissionsExt;
    let stt = rig.harness.root().join("stt.sh");
    std::fs::write(&stt, "#!/bin/sh\ncat \"$1\"\n").unwrap();
    std::fs::set_permissions(&stt, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fake = FakeTelegram::start("tg-secret").await;
    let top = format!("stt_cmd = [\"{}\"]", stt.display());
    let bot = rig.bot(&top, &telegram(&fake.base), TG_ENV).await;

    fake.push_voice(CHAT, USER, "v1", b"what is on my calendar");
    fake.wait_text("Heard: what is on my calendar").await;
    fake.wait_text("first reply").await;
    let sessions = rig.sessions().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        rig.user_messages(sessions[0]["id"].as_str().unwrap()).await,
        ["what is on my calendar"]
    );
    rig.assert_public_api_only(&[("POST", "/v1/sessions/{id}/messages")]);
    rig.finish(bot).await;
}

fn slack_tables(base: &str) -> String {
    format!(
        "[channels.slack]\nenabled = true\napi_base = \"{base}\"\nlisten = \"127.0.0.1:0\"\nbot_token_env = \"SLACK_BOT\"\nsigning_secret_env = \"SLACK_SIGNING\"\nallowed_workspaces = [\"T1\"]\nallowed_channels = [\"D1\"]\nallowed_users = [\"U1\"]\n"
    )
}

#[tokio::test]
async fn slack_message_reply_and_block_kit_approval_round_trip() {
    let Some(rig) = Rig::start_with(json!({"responses":[]}), |root| {
        Some(json!({"responses":[
            tool("write", json!({"path": root.join("outside.txt"), "content": "approved from slack"}), "w1"),
            text("write finished")
        ]}))
    })
    .await
    else {
        return;
    };
    let fake = FakeSlack::start("xoxb-test").await;
    let bot = rig
        .bot(
            "",
            &slack_tables(&fake.api_base()),
            &[
                ("SLACK_BOT", "xoxb-test"),
                ("SLACK_SIGNING", "signing-secret"),
            ],
        )
        .await;
    let addr = bot.running.as_ref().unwrap().slack_addr.unwrap();

    let event = slack_message_event(
        "T1",
        "D1",
        "U1",
        "1700000100.000100",
        None,
        "write it",
        "im",
    );
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now_secs(),
            "application/json",
            event.clone()
        )
        .await,
        200
    );
    // Slack redelivers events when an ack is slow; the duplicate must not create a second message.
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now_secs(),
            "application/json",
            event
        )
        .await,
        200
    );
    let card = eventually("slack approval card", WAIT, || {
        fake.messages()
            .into_iter()
            .find(|m| m.text.contains("Approval needed"))
    })
    .await;
    let id = rig.pending_ticket().await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(card.buttons[0], ("Approve".to_owned(), format!("a:{id}")));
    let sessions = rig.sessions().await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        rig.user_messages(sessions[0]["id"].as_str().unwrap()).await,
        ["write it"]
    );

    let payload = slack_press_payload("T1", "D1", "U1", &card.ts, None, &format!("a:{id}"));
    let status = slack_post(
        addr,
        "/slack/interactivity",
        "signing-secret",
        now_secs(),
        "application/x-www-form-urlencoded",
        payload,
    )
    .await;
    assert_eq!(status, 200);
    eventually("slack card decided", WAIT, || {
        fake.messages().into_iter().find(|m| {
            m.ts == card.ts && m.text.contains("Approved by user U1") && m.buttons.is_empty()
        })
    })
    .await;
    eventually("file written", WAIT, || {
        rig.outside().exists().then_some(())
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(rig.outside()).unwrap(),
        "approved from slack"
    );
    fake.wait_text("write finished").await;
    rig.assert_public_api_only(&[("POST", "/v1/tickets/{id}/resolve")]);
    rig.finish(bot).await;
}

#[tokio::test]
async fn discord_slash_command_reply_and_component_approval_round_trip() {
    let Some(rig) = Rig::start_with(json!({"responses":[]}), |root| {
        Some(json!({"responses":[
            tool("write", json!({"path": root.join("outside.txt"), "content": "approved from discord"}), "w1"),
            text("write finished")
        ]}))
    })
    .await
    else {
        return;
    };
    let fake = FakeDiscord::start("discord-token").await;
    let key = SigningKey::from_bytes(&[3; 32]);
    let tables = format!(
        "[channels.discord]\nenabled = true\napi_base = \"{}\"\nlisten = \"127.0.0.1:0\"\npublic_key = \"{}\"\nbot_token_env = \"DISCORD_BOT\"\nallowed_guilds = [\"G1\"]\nallowed_channels = [\"555\"]\nallowed_users = [\"42\"]\n",
        fake.api_base(),
        hex(key.verifying_key().as_bytes())
    );
    let bot = rig
        .bot("", &tables, &[("DISCORD_BOT", "discord-token")])
        .await;
    let addr = bot.running.as_ref().unwrap().discord_addr.unwrap();

    let ask = discord_command(
        "i1",
        "G1",
        "555",
        "42",
        "ask",
        json!([{"name": "text", "type": 3, "value": "write it"}]),
        json!({}),
    );
    let (status, ack) = discord_post(addr, &key, &ask).await;
    assert_eq!((status, ack["type"].as_i64()), (200, Some(4)));
    let card = eventually("discord approval card", WAIT, || {
        fake.messages()
            .into_iter()
            .find(|m| m.text.contains("Approval needed"))
    })
    .await;
    let id = rig.pending_ticket().await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(card.channel, "555");
    assert_eq!(card.buttons[0], ("Approve".to_owned(), format!("a:{id}")));
    assert_eq!(
        rig.user_messages(rig.sessions().await[0]["id"].as_str().unwrap())
            .await,
        ["write it"]
    );

    let (status, response) = discord_post(
        addr,
        &key,
        &discord_press("G1", "555", "42", &card.id, &format!("a:{id}")),
    )
    .await;
    assert_eq!((status, response["type"].as_i64()), (200, Some(6)));
    eventually("discord card decided", WAIT, || {
        fake.messages().into_iter().find(|m| {
            m.id == card.id && m.text.contains("Approved by user 42") && m.buttons.is_empty()
        })
    })
    .await;
    eventually("file written", WAIT, || {
        rig.outside().exists().then_some(())
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(rig.outside()).unwrap(),
        "approved from discord"
    );
    fake.wait_text("write finished").await;
    rig.assert_public_api_only(&[("POST", "/v1/tickets/{id}/resolve")]);
    rig.finish(bot).await;
}

#[tokio::test]
async fn concurrent_presses_on_one_card_decide_once_and_the_card_names_the_winner() {
    let Some(rig) = Rig::start_with(json!({"responses":[]}), |root| {
        Some(json!({"responses":[
            tool("write", json!({"path": root.join("outside.txt"), "content": "approved by chat"}), "w1"),
            text("write finished")
        ]}))
    })
    .await
    else {
        return;
    };
    let fake = FakeTelegram::start("tg-secret").await;
    let bot = rig.bot("", &telegram(&fake.base), TG_ENV).await;
    fake.push_text(CHAT, USER, "write it");
    let card = fake
        .wait_message("approval card", |m| m.text.contains("Approval needed"))
        .await;
    let id = rig.pending_ticket().await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Pushed together so the poller dispatches them as one concurrent batch.
    for round in 0..4 {
        let action = if round % 2 == 0 { "a" } else { "r" };
        fake.push_callback(CHAT, USER, card.id, &format!("{action}:{id}"));
    }
    let settled = fake
        .wait_message("decided card", |m| {
            m.id == card.id && m.buttons.is_empty() && m.text.contains("by user 7")
        })
        .await;
    let ticket = rig
        .harness
        .request("GET", &format!("/v1/tickets/{id}"), None)
        .await;
    let status = ticket.body["status"].as_str().unwrap().to_owned();
    // The attribution on the card must match the decision the daemon actually recorded,
    // even though the losing presses failed after the winner.
    match status.as_str() {
        "resolved" => assert!(
            settled.text.contains("Approved by user 7"),
            "{}",
            settled.text
        ),
        "rejected" => assert!(
            settled.text.contains("Rejected by user 7"),
            "{}",
            settled.text
        ),
        other => panic!("unexpected ticket status {other}"),
    }
    assert_eq!(
        settled.text.matches("by user 7").count(),
        1,
        "{}",
        settled.text
    );
    eventually("every press answered", WAIT, || {
        (fake.calls("answerCallbackQuery").len() == 4).then_some(())
    })
    .await;
    let notes: Vec<String> = fake
        .calls("answerCallbackQuery")
        .iter()
        .map(|c| c["text"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        notes
            .iter()
            .filter(|n| *n == "Approved" || *n == "Rejected")
            .count(),
        1,
        "{notes:?}"
    );
    rig.finish(bot).await;
}

#[tokio::test]
async fn check_validates_without_serving_and_works_next_to_a_running_bot() {
    let Some(rig) = Rig::start(json!({"responses":[text("first reply")]})).await else {
        return;
    };
    let fake = FakeTelegram::start("tg-secret").await;
    // Occupy the Slack port; reserve (then release) the Discord one.
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let slack_listen = occupied.local_addr().unwrap();
    let discord_listen = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    let key = SigningKey::from_bytes(&[5; 32]);
    let tables = format!(
        "{}\n[channels.slack]\nenabled = true\nlisten = \"{slack_listen}\"\nbot_token_env = \"SLACK_BOT\"\nsigning_secret_env = \"SLACK_SIGNING\"\nallowed_channels = [\"D1\"]\n\n[channels.discord]\nenabled = true\nlisten = \"{discord_listen}\"\npublic_key = \"{}\"\nbot_token_env = \"DISCORD_BOT\"\nallowed_users = [\"42\"]\n",
        telegram(&fake.base),
        hex(key.verifying_key().as_bytes())
    );
    let loaded = rig.load("", &tables);
    let env: HashMap<String, String> = [
        ("TG_TOKEN", "t"),
        ("SLACK_BOT", "b"),
        ("SLACK_SIGNING", "s"),
        ("DISCORD_BOT", "d"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let lookup = move |name: &str| env.get(name).cloned();

    // A thread already mapped to a session would be resumed (SSE opened) by `run`, not `check`.
    openside_channels::check(&loaded, &lookup)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        rig.proxy.requests(),
        vec![("GET".to_owned(), "/v1/sessions".to_owned())],
        "check may only read"
    );
    assert!(
        fake.calls("getUpdates").is_empty(),
        "check must not poll Telegram"
    );
    assert_eq!(fake.messages().len(), 0);
    assert!(
        std::net::TcpListener::bind(discord_listen).is_ok(),
        "check must not bind the Discord listener"
    );
    drop(occupied);

    // A token the daemon rejects fails the check with a clear reason.
    std::fs::write(&rig.token_file, "not-a-valid-token").unwrap();
    let error = openside_channels::check(&loaded, &lookup)
        .await
        .expect_err("bad token must fail");
    assert!(
        matches!(error, openside_channels::ChannelError::Daemon { status: 401, ref code } if code == "unauthorized"),
        "{error}"
    );
    // A missing secret fails too.
    let missing = openside_channels::check(&loaded, &|_: &str| None)
        .await
        .expect_err("missing secret");
    assert!(
        missing.to_string().contains("TG_TOKEN") || missing.to_string().contains("token"),
        "{missing}"
    );
    let mut rig = rig;
    rig.harness.kill().await;
}
