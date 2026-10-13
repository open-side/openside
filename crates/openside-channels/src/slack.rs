//! Slack adapter: Events API + interactivity over an HTTP receiver (requests are
//! authenticated by the signing secret), replies through `chat.postMessage` /
//! `chat.update` with Block Kit buttons.

use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::{
    allow::AllowList,
    core::Core,
    error::{log, ChannelError, Result},
    http,
    messenger::Messenger,
    model::{AudioRef, Button, Inbound, MsgRef, Press, ThreadKey},
    secret::Secret,
    text::{truncate, unhex},
};

const PLATFORM: &str = "slack";
/// Slack rejects signatures whose timestamp is further than this from now.
const MAX_SKEW_SECS: i64 = 300;
const SEEN_CAP: usize = 512;

/// Outbound Slack Web API calls.
pub struct SlackMessenger {
    http: http::Client,
    base: String,
    token: Secret,
}

impl SlackMessenger {
    /// `api_base` is `https://slack.com/api` or a test server; private file downloads come
    /// from Slack's own hosts.
    pub fn new(api_base: &str, token: Secret, network_allow: &[String]) -> Result<Self> {
        Ok(Self {
            http: http::client(
                api_base,
                &["https://slack.com", "https://*.slack.com"],
                network_allow,
            )?,
            base: api_base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base))?
            .bearer_auth(self.token.expose())
            .json(&body)
            .send()
            .await
            .map_err(ChannelError::http)?;
        let status = response.status();
        let value: Value = response.json().await.map_err(|_| {
            ChannelError::Platform(format!(
                "{method}: unreadable response (HTTP {})",
                status.as_u16()
            ))
        })?;
        if value["ok"] == true {
            Ok(value)
        } else {
            Err(ChannelError::Platform(format!(
                "{method}: request refused (HTTP {})",
                status.as_u16()
            )))
        }
    }

    /// The bot token only goes to Slack (or the configured API origin).
    fn may_authenticate(&self, url: &str) -> bool {
        let Ok(target) = url::Url::parse(url) else {
            return false;
        };
        let same_origin =
            url::Url::parse(&self.base).is_ok_and(|base| base.origin() == target.origin());
        let slack_host = target.scheme() == "https"
            && target
                .host_str()
                .is_some_and(|host| host == "slack.com" || host.ends_with(".slack.com"));
        same_origin || slack_host
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn blocks(text: &str, buttons: &[Button]) -> Value {
    let mut blocks =
        vec![json!({"type": "section", "text": {"type": "mrkdwn", "text": escape(text)}})];
    if !buttons.is_empty() {
        let elements: Vec<Value> = buttons
            .iter()
            .take(25)
            .map(|button| {
                let mut element = json!({
                    "type": "button",
                    "text": {"type": "plain_text", "text": truncate(&button.label, 75)},
                    "action_id": button.data,
                    "value": button.data,
                });
                if button.danger {
                    element["style"] = json!("danger");
                }
                element
            })
            .collect();
        blocks.push(json!({"type": "actions", "elements": elements}));
    }
    Value::Array(blocks)
}

impl Messenger for SlackMessenger {
    const PLATFORM: &'static str = PLATFORM;

    fn max_chars(&self) -> usize {
        2800
    }

    async fn send(&self, thread: &ThreadKey, text: &str, buttons: &[Button]) -> Result<MsgRef> {
        let mut body =
            json!({"channel": thread.chat, "text": text, "blocks": blocks(text, buttons)});
        if !thread.thread.is_empty() {
            body["thread_ts"] = json!(thread.thread);
        }
        let value = self.call("chat.postMessage", body).await?;
        let ts = value["ts"]
            .as_str()
            .ok_or_else(|| ChannelError::Platform("chat.postMessage: no ts".into()))?;
        Ok(MsgRef(ts.to_owned()))
    }

    async fn edit(
        &self,
        thread: &ThreadKey,
        message: &MsgRef,
        text: &str,
        buttons: &[Button],
    ) -> Result<()> {
        let body = json!({"channel": thread.chat, "ts": message.0, "text": text, "blocks": blocks(text, buttons)});
        self.call("chat.update", body).await.map(|_| ())
    }

    async fn download_audio(&self, audio: &AudioRef, dest: &Path, max_bytes: u64) -> Result<()> {
        let mut request = self.http.get(&audio.reference)?;
        if self.may_authenticate(&audio.reference) {
            request = request.bearer_auth(self.token.expose());
        }
        let response = request.send().await.map_err(ChannelError::http)?;
        http::save_body(response, dest, max_bytes).await
    }
}

/// `true` when `signature` (`v0=<hex>`) matches HMAC-SHA256 of `v0:{timestamp}:{body}`
/// and the timestamp is within five minutes of `now`. Comparison is constant time.
pub fn verify_signature(
    secret: &str,
    timestamp: &str,
    signature: &str,
    body: &[u8],
    now: i64,
) -> bool {
    let Ok(timestamp_secs) = timestamp.parse::<i64>() else {
        return false;
    };
    if now.abs_diff(timestamp_secs) > MAX_SKEW_SECS.unsigned_abs() {
        return false;
    }
    let Some(expected) = signature.strip_prefix("v0=").and_then(unhex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("v0:{timestamp}:").as_bytes());
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// Test helper and documentation of the signing scheme: `v0=<hex hmac>`.
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC accepts any key length"));
    mac.update(format!("v0:{timestamp}:").as_bytes());
    mac.update(body);
    format!("v0={}", crate::text::hex(&mac.finalize().into_bytes()))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// HTTP receiver state.
pub struct SlackReceiver {
    /// Platform core.
    pub core: Arc<Core<SlackMessenger>>,
    /// Allow-lists (workspace, channel, user).
    pub allow: AllowList,
    /// Signing secret verifying every request.
    pub signing_secret: Secret,
    seen: Mutex<(VecDeque<String>, HashSet<String>)>,
}

impl SlackReceiver {
    /// Build the receiver state.
    pub fn new(
        core: Arc<Core<SlackMessenger>>,
        allow: AllowList,
        signing_secret: Secret,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            allow,
            signing_secret,
            seen: Mutex::new((VecDeque::new(), HashSet::new())),
        })
    }

    /// Slack retries and mention+message pairs deliver one user message several times.
    fn first_sight(&self, key: String) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if !seen.1.insert(key.clone()) {
            return false;
        }
        seen.0.push_back(key);
        if seen.0.len() > SEEN_CAP {
            if let Some(old) = seen.0.pop_front() {
                seen.1.remove(&old);
            }
        }
        true
    }

    fn authentic(&self, headers: &HeaderMap, body: &[u8]) -> bool {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        };
        verify_signature(
            self.signing_secret.expose(),
            header("x-slack-request-timestamp"),
            header("x-slack-signature"),
            body,
            now(),
        )
    }
}

/// Routes: `POST /slack/events`, `POST /slack/interactivity`.
pub fn router(receiver: Arc<SlackReceiver>) -> Router {
    Router::new()
        .route("/slack/events", post(events))
        .route("/slack/interactivity", post(interactivity))
        .with_state(receiver)
}

#[derive(Deserialize)]
struct Outer {
    #[serde(rename = "type")]
    kind: String,
    challenge: Option<String>,
    team_id: Option<String>,
    event: Option<SlackEvent>,
}

#[derive(Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    kind: String,
    user: Option<String>,
    text: Option<String>,
    ts: Option<String>,
    thread_ts: Option<String>,
    channel: Option<String>,
    channel_type: Option<String>,
    bot_id: Option<String>,
    subtype: Option<String>,
    #[serde(default)]
    files: Vec<SlackFile>,
}

#[derive(Deserialize)]
struct SlackFile {
    mimetype: Option<String>,
    url_private_download: Option<String>,
    size: Option<u64>,
}

fn unauthorized() -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}

pub(crate) async fn events(
    State(receiver): State<Arc<SlackReceiver>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !receiver.authentic(&headers, &body) {
        log("slack request rejected: bad signature");
        return unauthorized();
    }
    let Ok(outer) = serde_json::from_slice::<Outer>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if outer.kind == "url_verification" {
        return Json(json!({"challenge": outer.challenge.unwrap_or_default()})).into_response();
    }
    if outer.kind == "event_callback" {
        if let Some(event) = outer.event {
            // Slack expects an acknowledgement within three seconds.
            tokio::spawn(handle_event(receiver, outer.team_id, event));
        }
    }
    StatusCode::OK.into_response()
}

async fn handle_event(receiver: Arc<SlackReceiver>, team: Option<String>, event: SlackEvent) {
    if event.bot_id.is_some() || !matches!(event.kind.as_str(), "message" | "app_mention") {
        return;
    }
    if event
        .subtype
        .as_deref()
        .is_some_and(|subtype| subtype != "file_share")
    {
        return;
    }
    let (Some(channel), Some(user), Some(ts)) =
        (event.channel.clone(), event.user.clone(), event.ts.clone())
    else {
        return;
    };
    if !receiver.allow.permits(&channel, &user, team.as_deref()) {
        return;
    }
    let direct = event.channel_type.as_deref() == Some("im");
    let thread = match (&event.thread_ts, direct) {
        (Some(thread_ts), _) => thread_ts.clone(),
        (None, true) => String::new(),
        (None, false) => ts.clone(),
    };
    let key = ThreadKey::new(PLATFORM, channel.clone(), thread);
    // In shared channels only mentions and follow-ups inside a thread the bot already
    // serves are for us; everything else is human conversation.
    let follow_up = event.kind == "message"
        && !direct
        && event.thread_ts.is_some()
        && receiver
            .core
            .store
            .get(&key)
            .is_some_and(|state| state.session_id.is_some());
    if event.kind == "message" && !direct && !follow_up {
        return;
    }
    if !receiver.first_sight(format!("{channel}:{ts}")) {
        return;
    }
    let audio = event.files.iter().find_map(|file| {
        file.mimetype
            .as_deref()
            .filter(|mime| mime.starts_with("audio/"))
            .and(file.url_private_download.clone())
            .map(|reference| AudioRef {
                reference,
                size: file.size,
            })
    });
    let inbound = Inbound {
        thread: key,
        user,
        message_id: ts,
        text: strip_mentions(&event.text.unwrap_or_default()),
        audio,
    };
    receiver.core.handle_inbound(inbound).await;
}

fn strip_mentions(text: &str) -> String {
    let mut rest = text.trim_start();
    while rest.starts_with("<@") {
        match rest.find('>') {
            Some(end) => rest = rest[end + 1..].trim_start(),
            None => break,
        }
    }
    unescape(rest)
}

#[derive(Deserialize)]
struct Interaction {
    user: Option<IdOnly>,
    team: Option<IdOnly>,
    channel: Option<IdOnly>,
    message: Option<InteractionMessage>,
    #[serde(default)]
    actions: Vec<Action>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
struct InteractionMessage {
    ts: String,
    thread_ts: Option<String>,
}

#[derive(Deserialize)]
struct Action {
    action_id: String,
}

pub(crate) async fn interactivity(
    State(receiver): State<Arc<SlackReceiver>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !receiver.authentic(&headers, &body) {
        log("slack interactivity rejected: bad signature");
        return unauthorized();
    }
    let payload = url::form_urlencoded::parse(&body)
        .find(|(key, _)| key == "payload")
        .and_then(|(_, value)| serde_json::from_str::<Interaction>(&value).ok());
    let Some(interaction) = payload else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let (Some(user), Some(channel), Some(message), Some(action)) = (
        interaction.user,
        interaction.channel,
        interaction.message,
        interaction.actions.into_iter().next(),
    ) else {
        return StatusCode::OK.into_response();
    };
    let team = interaction.team.map(|team| team.id);
    if receiver
        .allow
        .permits(&channel.id, &user.id, team.as_deref())
    {
        let press = Press {
            thread: ThreadKey::new(PLATFORM, channel.id, message.thread_ts.unwrap_or_default()),
            user: user.id,
            data: action.action_id,
            message: MsgRef(message.ts),
            token: String::new(),
        };
        let core = receiver.core.clone();
        tokio::spawn(async move { core.handle_press(press).await });
    }
    StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_round_trips_and_rejects_tampering_and_stale_timestamps() {
        let body = br#"{"type":"event_callback"}"#;
        let signature = sign("secret", "1000", body);
        assert!(verify_signature("secret", "1000", &signature, body, 1000));
        assert!(!verify_signature("other", "1000", &signature, body, 1000));
        assert!(!verify_signature("secret", "1000", &signature, b"{}", 1000));
        assert!(!verify_signature("secret", "1000", "v0=00", body, 1000));
        assert!(!verify_signature(
            "secret",
            "1000",
            &signature,
            body,
            1000 + MAX_SKEW_SECS + 1
        ));
        assert!(!verify_signature("secret", "nope", &signature, body, 1000));
        // Extreme values must be rejected, not overflow the subtraction.
        for extreme in [i64::MIN, i64::MAX] {
            assert!(!verify_signature(
                "secret",
                &extreme.to_string(),
                &signature,
                body,
                1000
            ));
        }
        assert!(!verify_signature(
            "secret",
            &i64::MIN.to_string(),
            &signature,
            body,
            i64::MAX
        ));
    }

    #[test]
    fn mentions_are_stripped_and_entities_decoded() {
        assert_eq!(strip_mentions("<@U123> /stop"), "/stop");
        assert_eq!(strip_mentions("a &lt;b&gt; &amp; c"), "a <b> & c");
    }
}
