//! Telegram Bot API adapter: long polling `getUpdates`, `sendMessage`,
//! `editMessageText`, `answerCallbackQuery`, inline keyboards.

use std::{path::Path, sync::Arc, time::Duration};

use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::{
    allow::AllowList,
    core::Core,
    error::{log, ChannelError, Result},
    http,
    messenger::Messenger,
    model::{AudioRef, Button, Inbound, MsgRef, Press, ThreadKey},
    secret::Secret,
};

const PLATFORM: &str = "telegram";

/// Outbound Telegram calls.
pub struct TelegramMessenger {
    http: http::Client,
    base: String,
    token: Secret,
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
}

#[derive(Deserialize)]
struct Sent {
    message_id: i64,
}

#[derive(Deserialize)]
struct FileInfo {
    file_path: Option<String>,
}

impl TelegramMessenger {
    /// `api_base` is `https://api.telegram.org` or a test server; files come from it too.
    pub fn new(api_base: &str, token: Secret, network_allow: &[String]) -> Result<Self> {
        Ok(Self {
            http: http::client(api_base, &[], network_allow)?,
            base: api_base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    fn method_url(&self, method: &str) -> String {
        format!("{}/bot{}/{method}", self.base, self.token.expose())
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: Value,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let mut request = self.http.post(self.method_url(method))?.json(&body);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(ChannelError::http)?;
        let status = response.status();
        let envelope: Envelope<T> = response.json().await.map_err(|_| {
            ChannelError::Platform(format!(
                "{method}: unreadable response (HTTP {})",
                status.as_u16()
            ))
        })?;
        match (envelope.ok, envelope.result) {
            (true, Some(result)) => Ok(result),
            _ => Err(ChannelError::Platform(format!(
                "{method}: request refused (HTTP {})",
                status.as_u16()
            ))),
        }
    }
}

fn chat_id(thread: &ThreadKey) -> Value {
    thread
        .chat
        .parse::<i64>()
        .map_or_else(|_| json!(thread.chat), |id| json!(id))
}

fn keyboard(buttons: &[Button]) -> Value {
    let rows: Vec<Vec<Value>> = buttons
        .chunks(2)
        .map(|row| {
            row.iter()
                .map(|button| json!({"text": button.label, "callback_data": button.data}))
                .collect()
        })
        .collect();
    json!({ "inline_keyboard": rows })
}

impl Messenger for TelegramMessenger {
    const PLATFORM: &'static str = PLATFORM;

    fn max_chars(&self) -> usize {
        4000
    }

    async fn send(&self, thread: &ThreadKey, text: &str, buttons: &[Button]) -> Result<MsgRef> {
        let mut body = json!({"chat_id": chat_id(thread), "text": text});
        if let (Some(object), Ok(topic)) = (body.as_object_mut(), thread.thread.parse::<i64>()) {
            object.insert("message_thread_id".into(), json!(topic));
        }
        if !buttons.is_empty() {
            body["reply_markup"] = keyboard(buttons);
        }
        let sent: Sent = self.call("sendMessage", body, None).await?;
        Ok(MsgRef(sent.message_id.to_string()))
    }

    async fn edit(
        &self,
        thread: &ThreadKey,
        message: &MsgRef,
        text: &str,
        buttons: &[Button],
    ) -> Result<()> {
        let message_id = message
            .0
            .parse::<i64>()
            .map_err(|_| ChannelError::Platform("editMessageText: bad message id".into()))?;
        let body = json!({
            "chat_id": chat_id(thread),
            "message_id": message_id,
            "text": text,
            "reply_markup": keyboard(buttons),
        });
        self.call::<Value>("editMessageText", body, None)
            .await
            .map(|_| ())
    }

    async fn download_audio(&self, audio: &AudioRef, dest: &Path, max_bytes: u64) -> Result<()> {
        let info: FileInfo = self
            .call("getFile", json!({"file_id": audio.reference}), None)
            .await?;
        let path = info
            .file_path
            .ok_or_else(|| ChannelError::Platform("getFile: no file_path".into()))?;
        let url = format!(
            "{}/file/bot{}/{}",
            self.base,
            self.token.expose(),
            path.trim_start_matches('/')
        );
        let response = self
            .http
            .get(url)?
            .send()
            .await
            .map_err(ChannelError::http)?;
        http::save_body(response, dest, max_bytes).await
    }

    async fn answer_press(&self, press: &Press, note: &str) {
        if press.token.is_empty() {
            return;
        }
        let body = json!({"callback_query_id": press.token, "text": note});
        if let Err(error) = self.call::<Value>("answerCallbackQuery", body, None).await {
            log(format_args!("telegram answerCallbackQuery failed: {error}"));
        }
    }
}

#[derive(Deserialize)]
struct Update {
    update_id: i64,
    message: Option<TgMessage>,
    callback_query: Option<TgCallback>,
}

#[derive(Deserialize)]
struct TgMessage {
    message_id: i64,
    from: Option<TgUser>,
    chat: TgChat,
    text: Option<String>,
    caption: Option<String>,
    voice: Option<TgFile>,
    audio: Option<TgFile>,
    message_thread_id: Option<i64>,
    #[serde(default)]
    is_topic_message: bool,
}

#[derive(Deserialize)]
struct TgChat {
    id: i64,
}

#[derive(Deserialize)]
struct TgUser {
    id: i64,
}

#[derive(Deserialize)]
struct TgFile {
    file_id: String,
    file_size: Option<u64>,
}

#[derive(Deserialize)]
struct TgCallback {
    id: String,
    from: TgUser,
    message: Option<TgMessage>,
    data: Option<String>,
}

impl TgMessage {
    fn thread(&self) -> ThreadKey {
        let topic = match (self.is_topic_message, self.message_thread_id) {
            (true, Some(topic)) => topic.to_string(),
            _ => String::new(),
        };
        ThreadKey::new(PLATFORM, self.chat.id.to_string(), topic)
    }
}

/// Long-polling loop. Offsets are persisted so a restart neither replays handled
/// updates nor drops unhandled ones; delivery to the daemon is idempotent anyway.
pub struct Poller {
    /// Platform core (also reachable by tests).
    pub core: Arc<Core<TelegramMessenger>>,
    /// Allow-lists.
    pub allow: AllowList,
    /// `getUpdates` long-poll timeout.
    pub poll_timeout: Duration,
}

impl Poller {
    /// Run until `shutdown` is cancelled.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut offset = self.core.store.offset(PLATFORM).unwrap_or(0);
        let mut backoff = Duration::from_millis(500);
        loop {
            let body = json!({
                "offset": offset,
                "timeout": self.poll_timeout.as_secs(),
                "allowed_updates": ["message", "callback_query"],
            });
            let timeout = self.poll_timeout + Duration::from_secs(15);
            let updates = tokio::select! {
                _ = shutdown.cancelled() => return,
                result = self.core.messenger.call::<Vec<Update>>("getUpdates", body, Some(timeout)) => result,
            };
            match updates {
                Ok(updates) => {
                    backoff = Duration::from_millis(500);
                    let Some(last) = updates.iter().map(|update| update.update_id).max() else {
                        continue;
                    };
                    let mut tasks = Vec::new();
                    for update in updates {
                        if let Some(task) = self.dispatch(update) {
                            tasks.push(task);
                        }
                    }
                    for task in tasks {
                        let _ = task.await;
                    }
                    offset = last + 1;
                    if let Err(error) = self.core.store.set_offset(PLATFORM, offset) {
                        log(format_args!("telegram offset not saved: {error}"));
                    }
                }
                Err(error) => {
                    log(format_args!("telegram getUpdates: {error}"));
                    tokio::select! {
                        _ = shutdown.cancelled() => return,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    fn dispatch(&self, update: Update) -> Option<tokio::task::JoinHandle<()>> {
        let core = self.core.clone();
        if let Some(message) = update.message {
            let user = message.from.as_ref().map(|user| user.id.to_string())?;
            if !self
                .allow
                .permits(&message.chat.id.to_string(), &user, None)
            {
                return None;
            }
            let audio = message
                .voice
                .as_ref()
                .or(message.audio.as_ref())
                .map(|file| AudioRef {
                    reference: file.file_id.clone(),
                    size: file.file_size,
                });
            let inbound = Inbound {
                thread: message.thread(),
                user,
                message_id: message.message_id.to_string(),
                text: message
                    .text
                    .clone()
                    .or_else(|| message.caption.clone())
                    .unwrap_or_default(),
                audio,
            };
            return Some(tokio::spawn(
                async move { core.handle_inbound(inbound).await },
            ));
        }
        let callback = update.callback_query?;
        let message = callback.message?;
        let user = callback.from.id.to_string();
        if !self
            .allow
            .permits(&message.chat.id.to_string(), &user, None)
        {
            return None;
        }
        let press = Press {
            thread: message.thread(),
            user,
            data: callback.data.unwrap_or_default(),
            message: MsgRef(message.message_id.to_string()),
            token: callback.id,
        };
        Some(tokio::spawn(async move { core.handle_press(press).await }))
    }
}
