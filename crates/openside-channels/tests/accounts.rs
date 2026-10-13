//! Hermetic identity isolation: platform ids can overlap, but sessions and agents cannot.
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use openside_channels::{start_account, AccountSecrets, DaemonClient, Secret, Store, ThreadKey};
use openside_config::{ChannelAccountConfig, ChannelKind, ChannelOptions};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use support::{eventually, FakeTelegram, WAIT};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Capture {
    sessions: HashMap<String, String>,
    messages: Vec<(String, String)>,
}
async fn create(State(c): State<Arc<Mutex<Capture>>>, Json(body): Json<Value>) -> Json<Value> {
    let mut c = c.lock();
    let id = format!("session-{}", c.sessions.len());
    c.sessions
        .insert(id.clone(), body["agent_id"].as_str().unwrap().into());
    Json(
        json!({"id":id,"agent_id":body["agent_id"],"title":"New conversation","model":"script:test","mode":"guard","workspace":null,"status":"idle","last_seq":1,"queue":[]}),
    )
}
async fn session(State(c): State<Arc<Mutex<Capture>>>, Path(id): Path<String>) -> Json<Value> {
    let c = c.lock();
    Json(
        json!({"id":id,"agent_id":c.sessions[&id],"title":"New conversation","model":"script:test","mode":"guard","workspace":null,"status":"idle","last_seq":1,"queue":[]}),
    )
}
async fn submit(
    State(c): State<Arc<Mutex<Capture>>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    c.lock()
        .messages
        .push((id, body["text"].as_str().unwrap().into()));
    Json(json!({"id":body["message_id"],"text":body["text"],"status":"pending"}))
}
fn account(id: &str, base: &str, agent: Option<&str>) -> ChannelAccountConfig {
    ChannelAccountConfig {
        id: id.into(),
        kind: ChannelKind::Telegram,
        label: id.into(),
        enabled: true,
        agent_id: agent.map(str::to_owned),
        secret_id: Some(format!("channel:telegram:{id}")),
        legacy_secret: None,
        options: ChannelOptions {
            api_base: Some(base.into()),
            allowed_chats: vec!["100".into()],
            allowed_users: vec!["7".into()],
            poll_timeout_secs: 1,
            ..Default::default()
        },
    }
}
#[tokio::test]
async fn two_telegram_accounts_isolate_sessions_agents_offsets_and_removal() {
    let first = FakeTelegram::start("first-channel-canary-12345").await;
    let second = FakeTelegram::start("second-channel-canary-54321").await;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "[channels]\nstate_dir={}\n",
            serde_json::to_string(&root.path().join("channels")).unwrap()
        ),
    )
    .unwrap();
    let loaded = openside_config::load(Some(&path)).unwrap();
    let capture = Arc::new(Mutex::new(Capture::default()));
    let router = Router::new()
        .route("/v1/sessions", post(create))
        .route("/v1/sessions/{id}", get(session))
        .route("/v1/sessions/{id}/messages", post(submit))
        .route(
            "/v1/sessions/{id}/events",
            get(|| async {
                axum::response::Sse::new(futures_util::stream::pending::<
                    Result<axum::response::sse::Event, std::convert::Infallible>,
                >())
            }),
        )
        .with_state(capture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let daemon = DaemonClient::new(&base, Secret::new("daemon-test-token")).unwrap();
    let a = account("work", &first.base, None);
    let b = account("personal", &second.base, Some("mina"));
    let first_secrets = AccountSecrets {
        token: Secret::new("first-channel-canary-12345"),
        signing_secret: None,
    };
    assert!(!format!("{first_secrets:?}").contains("first-channel-canary"));
    let running_a = start_account(
        &loaded,
        &a,
        first_secrets,
        daemon.clone(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let running_b = start_account(
        &loaded,
        &b,
        AccountSecrets {
            token: Secret::new("second-channel-canary-54321"),
            signing_secret: None,
        },
        daemon,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    first.push_text(100, 7, "from work");
    second.push_text(100, 7, "from personal");
    eventually("both identities delivered", WAIT, || {
        let c = capture.lock();
        (c.messages.len() == 2).then_some(())
    })
    .await;
    let (work, personal) = {
        let c = capture.lock();
        let work = c
            .messages
            .iter()
            .find(|(_, text)| text == "from work")
            .unwrap()
            .0
            .clone();
        let personal = c
            .messages
            .iter()
            .find(|(_, text)| text == "from personal")
            .unwrap()
            .0
            .clone();
        assert_ne!(work, personal);
        assert_eq!(c.sessions[&work], "chief");
        assert_eq!(c.sessions[&personal], "mina");
        (work, personal)
    };
    running_a.stop();
    running_a.join().await;
    first.push_text(100, 7, "must not be delivered");
    second.push_text(100, 7, "still personal");
    eventually("remaining account continues", WAIT, || {
        capture
            .lock()
            .messages
            .iter()
            .any(|(_, text)| text == "still personal")
            .then_some(())
    })
    .await;
    {
        let c = capture.lock();
        assert_eq!(c.messages.len(), 3);
        assert_eq!(
            c.messages
                .iter()
                .find(|(_, text)| text == "still personal")
                .unwrap()
                .0,
            personal
        );
    }
    running_b.stop();
    running_b.join().await;
    for (id, session) in [("work", work), ("personal", personal)] {
        let store = Store::open(&root.path().join("channels/telegram").join(id)).unwrap();
        assert_eq!(
            store
                .get(&ThreadKey::new("telegram", "100", ""))
                .unwrap()
                .session_id
                .as_deref(),
            Some(session.as_str())
        );
        assert!(store.offset("telegram").is_some());
        let text = std::fs::read_to_string(
            root.path()
                .join("channels/telegram")
                .join(id)
                .join("threads.json"),
        )
        .unwrap();
        assert!(!text.contains("channel-canary"));
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn channel_upstream_canary_is_removed_from_loggable_errors_and_debug() {
    use openside_channels::{slack::SlackMessenger, telegram::TelegramMessenger, Messenger};
    const CANARY: &str = "reflected-channel-token-canary-102938";
    let router = Router::new()
        .fallback(|| async { Json(json!({"ok":false,"description":CANARY,"error":CANARY})) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let telegram = TelegramMessenger::new(&base, Secret::new(CANARY), &[]).unwrap();
    let slack = SlackMessenger::new(&base, Secret::new(CANARY), &[]).unwrap();
    let tg = telegram
        .send(&ThreadKey::new("telegram", "100", ""), "hello", &[])
        .await
        .unwrap_err();
    let sl = slack
        .send(&ThreadKey::new("slack", "D1", ""), "hello", &[])
        .await
        .unwrap_err();
    for error in [tg, sl] {
        assert!(!error.to_string().contains(CANARY));
        assert!(!format!("{error:?}").contains(CANARY));
    }
    server.abort();
    let _ = server.await;
}

#[test]
fn discord_authentication_rejects_low_order_verification_keys() {
    assert!(openside_channels::discord::parse_public_key(&"00".repeat(32)).is_err());
}
