//! Hermetic adapter tests: fake chat platforms, no daemon (it is unreachable on purpose,
//! so "the bot handled this update" is observable as its polite error reply).
//! Round trips with a real daemon live in `crates/openside-e2e/tests/channels.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::{collections::HashMap, path::Path};

use ed25519_dalek::SigningKey;
use openside_channels::{start, Running};
use serde_json::json;
use support::*;
use tokio_util::sync::CancellationToken;

const UNREACHABLE: &str = "http://127.0.0.1:1";

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

async fn launch(dir: &Path, extra: &str, env: &[(&str, &str)]) -> Result<Bot, String> {
    let config = format!(
        "[channels]\napi_url = \"{UNREACHABLE}\"\nstate_dir = \"{}\"\nedit_interval_ms = 200\n{extra}",
        dir.join("state").display()
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    let loaded = openside_config::load(Some(&path)).map_err(|e| e.to_string())?;
    let env: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let lookup = move |name: &str| env.get(name).cloned();
    let shutdown = CancellationToken::new();
    let running = start(&loaded, &lookup, shutdown.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(Bot {
        shutdown,
        running: Some(running),
    })
}

#[cfg(unix)]
fn script(dir: &Path, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("stt.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn telegram_config(base: &str, extra: &str) -> String {
    format!(
        "token_env = \"OPENSIDE_TOKEN\"\nstt_timeout_secs = 20\n{extra}\n[channels.telegram]\nenabled = true\napi_base = \"{base}\"\ntoken_env = \"TG_TOKEN\"\nallowed_chats = [100]\nallowed_users = [7]\npoll_timeout_secs = 1\n"
    )
}

#[tokio::test]
async fn telegram_serves_only_allow_listed_chat_and_user() {
    let fake = FakeTelegram::start("tg-secret").await;
    let dir = tempfile::tempdir().unwrap();
    let bot = launch(
        dir.path(),
        &telegram_config(&fake.base, ""),
        &[("TG_TOKEN", "tg-secret"), ("OPENSIDE_TOKEN", "x")],
    )
    .await
    .unwrap();
    // Denied updates come first: if they were served they would reply before the allowed one.
    fake.push_text(999, 7, "wrong chat");
    fake.push_text(100, 8, "wrong user");
    fake.push_text(100, 7, "hello");
    let reply = fake.wait_text("Could not reach the openside daemon").await;
    assert_eq!(reply.chat, "100");
    assert_eq!(
        fake.messages().len(),
        1,
        "denied chat/user must be ignored silently"
    );
    bot.stop().await;
}

#[tokio::test]
async fn telegram_voice_without_stt_is_refused_and_never_downloaded() {
    let fake = FakeTelegram::start("tg-secret").await;
    let dir = tempfile::tempdir().unwrap();
    let bot = launch(
        dir.path(),
        &telegram_config(&fake.base, ""),
        &[("TG_TOKEN", "tg-secret"), ("OPENSIDE_TOKEN", "x")],
    )
    .await
    .unwrap();
    fake.push_voice(100, 7, "voice-1", b"spoken words");
    let reply = fake.wait_text("local speech-to-text").await;
    assert!(reply.text.contains("never sent to a remote service"));
    assert!(
        fake.calls("getFile").is_empty(),
        "audio must not be fetched without a local STT command"
    );
    bot.stop().await;
}

#[tokio::test]
async fn telegram_help_and_model_choice_are_local_and_persisted_private() {
    let fake = FakeTelegram::start("tg-secret").await;
    let dir = tempfile::tempdir().unwrap();
    let bot = launch(
        dir.path(),
        &telegram_config(&fake.base, ""),
        &[("TG_TOKEN", "tg-secret"), ("OPENSIDE_TOKEN", "x")],
    )
    .await
    .unwrap();
    fake.push_text(100, 7, "/start@openside_bot");
    let help = fake.wait_text("/sessions").await;
    assert!(
        help.text.contains("/new") && help.text.contains("/stop") && help.text.contains("/usage")
    );
    fake.push_text(100, 7, "/model provider:some-model");
    fake.wait_text("Model set to provider:some-model").await;
    bot.stop().await;
    let map = std::fs::read_to_string(dir.path().join("state/threads.json")).unwrap();
    assert!(map.contains("provider:some-model"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("state/threads.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn telegram_transcribes_voice_with_local_stt_and_posts_what_it_heard() {
    let fake = FakeTelegram::start("tg-secret").await;
    let dir = tempfile::tempdir().unwrap();
    let stt = script(dir.path(), "cat \"$1\"");
    let config = telegram_config(&fake.base, &format!("stt_cmd = [\"{stt}\"]"));
    let bot = launch(
        dir.path(),
        &config,
        &[("TG_TOKEN", "tg-secret"), ("OPENSIDE_TOKEN", "x")],
    )
    .await
    .unwrap();
    fake.push_voice(100, 7, "voice-2", b"turn on the lights");
    fake.wait_text("Heard: turn on the lights").await;
    assert_eq!(fake.calls("getFile").len(), 1);
    // Oversized declared audio is refused before any download.
    let tiny = format!("max_audio_bytes = 4\nstt_cmd = [\"{stt}\"]");
    bot.stop().await;
    let dir2 = tempfile::tempdir().unwrap();
    let bot = launch(
        dir2.path(),
        &telegram_config(&fake.base, &tiny),
        &[("TG_TOKEN", "tg-secret"), ("OPENSIDE_TOKEN", "x")],
    )
    .await
    .unwrap();
    fake.push_voice(100, 7, "voice-3", b"way more than four bytes");
    fake.wait_text("too large to transcribe").await;
    assert_eq!(
        fake.calls("getFile").len(),
        1,
        "no second download for an oversized attachment"
    );
    bot.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn token_command_is_run_without_a_shell_and_missing_secrets_fail_before_listening() {
    let fake = FakeTelegram::start("from-command").await;
    let dir = tempfile::tempdir().unwrap();
    let config = format!(
        "[channels.telegram]\nenabled = true\napi_base = \"{}\"\ntoken_cmd = [\"echo\", \"from-command\"]\nallowed_users = [7]\npoll_timeout_secs = 1\n",
        fake.base
    );
    let bot = launch(
        dir.path(),
        &format!("token_env = \"T\"\n{config}"),
        &[("T", "x")],
    )
    .await
    .unwrap();
    fake.push_text(100, 7, "/help");
    fake.wait_text("/sessions").await;
    bot.stop().await;

    let dir = tempfile::tempdir().unwrap();
    let missing = launch(
        dir.path(),
        &telegram_config(&fake.base, ""),
        &[("OPENSIDE_TOKEN", "x")],
    )
    .await
    .err()
    .unwrap();
    assert!(missing.contains("TG_TOKEN"), "{missing}");
    let no_daemon_token = launch(
        dir.path(),
        &telegram_config(&fake.base, ""),
        &[("TG_TOKEN", "x")],
    )
    .await
    .err()
    .unwrap();
    assert!(
        no_daemon_token.contains("daemon token"),
        "{no_daemon_token}"
    );
    let none = launch(dir.path(), "", &[("OPENSIDE_TOKEN", "x")])
        .await
        .err()
        .unwrap();
    assert!(none.contains("no adapter is enabled"), "{none}");
}

fn slack_config(base: &str, extra: &str) -> String {
    format!(
        "token_env = \"OPENSIDE_TOKEN\"\n{extra}\n[channels.slack]\nenabled = true\napi_base = \"{base}\"\nlisten = \"127.0.0.1:0\"\nbot_token_env = \"SLACK_BOT\"\nsigning_secret_env = \"SLACK_SIGNING\"\nallowed_workspaces = [\"T1\"]\nallowed_channels = [\"D1\", \"C1\"]\nallowed_users = [\"U1\"]\n"
    )
}

const SLACK_ENV: &[(&str, &str)] = &[
    ("SLACK_BOT", "xoxb-test"),
    ("SLACK_SIGNING", "signing-secret"),
    ("OPENSIDE_TOKEN", "x"),
];

#[tokio::test]
async fn slack_rejects_unsigned_forged_and_stale_requests_then_serves_signed_ones() {
    let fake = FakeSlack::start("xoxb-test").await;
    let dir = tempfile::tempdir().unwrap();
    let bot = launch(dir.path(), &slack_config(&fake.api_base(), ""), SLACK_ENV)
        .await
        .unwrap();
    let addr = bot.running.as_ref().unwrap().slack_addr.unwrap();
    let dm = slack_message_event("T1", "D1", "U1", "1700000100.000100", None, "hello", "im");

    let now = now_secs();
    // No signature headers at all, a signature from the wrong secret, a tampered body
    // and a correctly signed but stale (replayed) request are all rejected.
    assert_eq!(
        slack_post_raw(
            addr,
            "/slack/events",
            &now.to_string(),
            "",
            "application/json",
            dm.clone()
        )
        .await,
        401
    );
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "wrong-secret",
            now,
            "application/json",
            dm.clone()
        )
        .await,
        401
    );
    let signature =
        openside_channels::slack::sign("signing-secret", &now.to_string(), dm.as_bytes());
    let tampered = dm.replace("hello", "rm -rf");
    assert_eq!(
        slack_post_raw(
            addr,
            "/slack/events",
            &now.to_string(),
            &signature,
            "application/json",
            tampered
        )
        .await,
        401
    );
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now - 3600,
            "application/json",
            dm.clone()
        )
        .await,
        401
    );
    assert_eq!(
        slack_post_raw(
            addr,
            "/slack/events",
            &i64::MIN.to_string(),
            &signature,
            "application/json",
            dm.clone()
        )
        .await,
        401,
        "extreme timestamp"
    );
    assert_eq!(
        slack_post(
            addr,
            "/slack/interactivity",
            "wrong-secret",
            now,
            "application/x-www-form-urlencoded",
            "payload=%7B%7D".into()
        )
        .await,
        401
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        fake.calls(),
        0,
        "rejected requests must not reach the handler"
    );

    // Slack's URL verification handshake echoes the challenge.
    let body = json!({"type": "url_verification", "challenge": "abc123"}).to_string();
    let signature =
        openside_channels::slack::sign("signing-secret", &now.to_string(), body.as_bytes());
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/slack/events"))
        .header("x-slack-request-timestamp", now.to_string())
        .header("x-slack-signature", signature)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["challenge"],
        "abc123"
    );

    // Ignored: other workspace, other channel, other user, bot echo, unmentioned channel chatter.
    for (team, channel, user, ts) in [
        ("T2", "D1", "U1", "1.1"),
        ("T1", "D9", "U1", "1.2"),
        ("T1", "D1", "U9", "1.3"),
    ] {
        let event = slack_message_event(team, channel, user, ts, None, "hi", "im");
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
    }
    let mut echo: serde_json::Value = serde_json::from_str(&slack_message_event(
        "T1", "D1", "U1", "1.4", None, "hi", "im",
    ))
    .unwrap();
    echo["event"]["bot_id"] = json!("B1");
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now_secs(),
            "application/json",
            echo.to_string()
        )
        .await,
        200
    );
    let chatter = slack_message_event(
        "T1",
        "C1",
        "U1",
        "1.5",
        None,
        "just people talking",
        "channel",
    );
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now_secs(),
            "application/json",
            chatter
        )
        .await,
        200
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(fake.calls(), 0, "ignored updates must produce no replies");

    // A signed direct message is served; the daemon is unreachable so the bot says so.
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now_secs(),
            "application/json",
            dm
        )
        .await,
        200
    );
    let reply = fake.wait_text("Could not reach the openside daemon").await;
    assert_eq!(reply.channel, "D1");
    assert_eq!(
        reply.thread_ts, None,
        "a direct-message channel is one thread"
    );

    // A mention in a channel starts a thread rooted at that message and replies inside it.
    let mut mention: serde_json::Value = serde_json::from_str(&slack_message_event(
        "T1",
        "C1",
        "U1",
        "1700000200.000200",
        None,
        "<@UBOT> /stop",
        "channel",
    ))
    .unwrap();
    mention["event"]["type"] = json!("app_mention");
    assert_eq!(
        slack_post(
            addr,
            "/slack/events",
            "signing-secret",
            now_secs(),
            "application/json",
            mention.to_string()
        )
        .await,
        200
    );
    let reply = fake.wait_text("No active session").await;
    assert_eq!(reply.thread_ts.as_deref(), Some("1700000200.000200"));
    bot.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn slack_audio_downloads_authenticate_only_to_slack_origins() {
    let fake = FakeSlack::start("xoxb-test").await;
    let foreign = FakeSlack::start("other").await;
    let dir = tempfile::tempdir().unwrap();
    let stt = script(dir.path(), "cat \"$1\"");
    let bot = launch(
        dir.path(),
        &slack_config(&fake.api_base(), &format!("stt_cmd = [\"{stt}\"]")),
        SLACK_ENV,
    )
    .await
    .unwrap();
    let addr = bot.running.as_ref().unwrap().slack_addr.unwrap();
    for (ts, url) in [
        ("1.1", fake.put_file("a.m4a", b"from slack")),
        ("1.2", foreign.put_file("b.m4a", b"from elsewhere")),
    ] {
        let mut event: serde_json::Value =
            serde_json::from_str(&slack_message_event("T1", "D1", "U1", ts, None, "", "im"))
                .unwrap();
        event["event"]["subtype"] = json!("file_share");
        event["event"]["files"] =
            json!([{"mimetype": "audio/mp4", "url_private_download": url, "size": 20}]);
        assert_eq!(
            slack_post(
                addr,
                "/slack/events",
                "signing-secret",
                now_secs(),
                "application/json",
                event.to_string()
            )
            .await,
            200
        );
    }
    fake.wait_text("Heard: from slack").await;
    fake.wait_text("Heard: from elsewhere").await;
    assert_eq!(
        fake.file_auth_headers(),
        vec![Some("Bearer xoxb-test".to_owned())]
    );
    assert_eq!(
        foreign.file_auth_headers(),
        vec![None],
        "the bot token must not leave the Slack API origin"
    );
    bot.stop().await;
}

fn discord_config(base: &str, key: &SigningKey, extra: &str) -> String {
    format!(
        "token_env = \"OPENSIDE_TOKEN\"\n{extra}\n[channels.discord]\nenabled = true\napi_base = \"{base}\"\nlisten = \"127.0.0.1:0\"\npublic_key = \"{}\"\nbot_token_env = \"DISCORD_BOT\"\nallowed_guilds = [\"G1\"]\nallowed_channels = [\"555\"]\nallowed_users = [\"42\"]\n",
        hex(key.verifying_key().as_bytes())
    )
}

const DISCORD_ENV: &[(&str, &str)] = &[("DISCORD_BOT", "discord-token"), ("OPENSIDE_TOKEN", "x")];

#[tokio::test]
async fn discord_verifies_ed25519_answers_ping_and_serves_only_allowed_users() {
    let fake = FakeDiscord::start("discord-token").await;
    let key = SigningKey::from_bytes(&[9; 32]);
    let dir = tempfile::tempdir().unwrap();
    let bot = launch(
        dir.path(),
        &discord_config(&fake.api_base(), &key, ""),
        DISCORD_ENV,
    )
    .await
    .unwrap();
    let addr = bot.running.as_ref().unwrap().discord_addr.unwrap();

    let ping = json!({"type": 1});
    let (status, body) = discord_post(addr, &key, &ping).await;
    assert_eq!((status, body["type"].as_i64()), (200, Some(1)));
    let forger = SigningKey::from_bytes(&[10; 32]);
    assert_eq!(
        discord_post(addr, &forger, &ping).await.0,
        401,
        "signature from another key"
    );
    let (status, _) = discord_post_raw(addr, &now_secs().to_string(), "00", ping.to_string()).await;
    assert_eq!(status, 401, "garbage signature");
    let (status, _) = discord_post_raw(addr, &now_secs().to_string(), "", ping.to_string()).await;
    assert_eq!(status, 401, "missing signature");
    // Signed for one body, replayed with another.
    use ed25519_dalek::Signer;
    let timestamp = now_secs().to_string();
    let signature = key.sign(&[timestamp.as_bytes(), ping.to_string().as_bytes()].concat());
    let (status, _) = discord_post_raw(
        addr,
        &timestamp,
        &hex(&signature.to_bytes()),
        json!({"type": 2}).to_string(),
    )
    .await;
    assert_eq!(status, 401, "tampered body");

    let denied = discord_command("1", "G1", "555", "41", "help", json!([]), json!({}));
    let (_, body) = discord_post(addr, &key, &denied).await;
    assert_eq!(body["data"]["flags"], 64);
    let wrong_guild = discord_command("2", "G9", "555", "42", "help", json!([]), json!({}));
    discord_post(addr, &key, &wrong_guild).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        fake.messages().is_empty(),
        "denied interactions must not post anything"
    );

    let allowed = discord_command("3", "G1", "555", "42", "help", json!([]), json!({}));
    let (status, body) = discord_post(addr, &key, &allowed).await;
    assert_eq!((status, body["type"].as_i64()), (200, Some(4)));
    let help = fake.wait_text("/sessions").await;
    assert_eq!(help.channel, "555");
    bot.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn discord_audio_attachment_goes_to_local_stt_without_the_bot_token() {
    let fake = FakeDiscord::start("discord-token").await;
    let key = SigningKey::from_bytes(&[9; 32]);
    let dir = tempfile::tempdir().unwrap();
    let stt = script(dir.path(), "cat \"$1\"");
    let config = discord_config(&fake.api_base(), &key, &format!("stt_cmd = [\"{stt}\"]"));
    let bot = launch(dir.path(), &config, DISCORD_ENV).await.unwrap();
    let addr = bot.running.as_ref().unwrap().discord_addr.unwrap();
    let url = fake.put_file("voice.ogg", b"hello discord");
    let ask = discord_command(
        "4",
        "G1",
        "555",
        "42",
        "ask",
        json!([{"name": "audio", "type": 11, "value": "att1"}]),
        json!({"attachments": {"att1": {"url": url, "content_type": "audio/ogg", "size": 13}}}),
    );
    assert_eq!(discord_post(addr, &key, &ask).await.0, 200);
    fake.wait_text("Heard: hello discord").await;
    assert_eq!(fake.file_auth_headers(), vec![None]);
    bot.stop().await;
}
