//! Discord adapter over the HTTP Interactions endpoint (Ed25519-signed requests)
//! and REST `POST /channels/{id}/messages`.
//!
//! The Gateway websocket is deliberately not used: plain-message input would need a
//! long-lived Gateway session and the privileged message-content intent. Slash
//! commands (`/ask`, `/new`, `/stop`, ...) arrive as interactions instead, and
//! approval buttons are message components.

use std::{path::Path, sync::Arc};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    allow::AllowList,
    core::Core,
    error::{log, ChannelError, Result},
    http,
    messenger::Messenger,
    model::{AudioRef, Button, Inbound, MsgRef, Press, ThreadKey},
    secret::Secret,
    text::unhex,
};

const PLATFORM: &str = "discord";

/// Outbound Discord REST calls.
pub struct DiscordMessenger {
    http: http::Client,
    base: String,
    token: Secret,
}

impl DiscordMessenger {
    /// `api_base` is `https://discord.com/api/v10` or a test server; attachments come from
    /// Discord's CDN hosts.
    pub fn new(api_base: &str, token: Secret, network_allow: &[String]) -> Result<Self> {
        Ok(Self {
            http: http::client(
                api_base,
                &["https://cdn.discordapp.com", "https://media.discordapp.net"],
                network_allow,
            )?,
            base: api_base.trim_end_matches('/').to_owned(),
            token,
        })
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .request(method, format!("{}{path}", self.base))?
            .header("authorization", format!("Bot {}", self.token.expose()))
            .json(&body)
            .send()
            .await
            .map_err(ChannelError::http)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ChannelError::Platform(format!(
                "discord REST failed (HTTP {})",
                status.as_u16()
            )));
        }
        response
            .json()
            .await
            .map_err(|_| ChannelError::Platform("discord REST: unreadable response".into()))
    }
}

fn components(buttons: &[Button]) -> Value {
    let rows: Vec<Value> = buttons
        .iter()
        .take(25)
        .collect::<Vec<_>>()
        .chunks(5)
        .map(|row| {
            let items: Vec<Value> = row
                .iter()
                .map(|button| {
                    json!({
                        "type": 2,
                        "style": if button.danger { 4 } else { 1 },
                        "label": crate::text::truncate(&button.label, 80),
                        "custom_id": button.data,
                    })
                })
                .collect();
            json!({"type": 1, "components": items})
        })
        .collect();
    Value::Array(rows)
}

impl Messenger for DiscordMessenger {
    const PLATFORM: &'static str = PLATFORM;

    fn max_chars(&self) -> usize {
        1900
    }

    async fn send(&self, thread: &ThreadKey, text: &str, buttons: &[Button]) -> Result<MsgRef> {
        let mut body = json!({"content": text, "allowed_mentions": {"parse": []}});
        if !buttons.is_empty() {
            body["components"] = components(buttons);
        }
        let value = self
            .call(
                reqwest::Method::POST,
                &format!("/channels/{}/messages", thread.chat),
                body,
            )
            .await?;
        let id = value["id"]
            .as_str()
            .ok_or_else(|| ChannelError::Platform("discord REST: no message id".into()))?;
        Ok(MsgRef(id.to_owned()))
    }

    async fn edit(
        &self,
        thread: &ThreadKey,
        message: &MsgRef,
        text: &str,
        buttons: &[Button],
    ) -> Result<()> {
        let body = json!({"content": text, "allowed_mentions": {"parse": []}, "components": components(buttons)});
        self.call(
            reqwest::Method::PATCH,
            &format!("/channels/{}/messages/{}", thread.chat, message.0),
            body,
        )
        .await
        .map(|_| ())
    }

    async fn download_audio(&self, audio: &AudioRef, dest: &Path, max_bytes: u64) -> Result<()> {
        // Attachment URLs are public CDN links; the bot token is not sent.
        let response = self
            .http
            .get(&audio.reference)?
            .send()
            .await
            .map_err(ChannelError::http)?;
        http::save_body(response, dest, max_bytes).await
    }
}

/// Parse the application public key (64 hex characters).
pub fn parse_public_key(hex_key: &str) -> Result<VerifyingKey> {
    let bytes = unhex(hex_key.trim())
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| {
            ChannelError::Config("discord public_key must be 64 hex characters".into())
        })?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|_| ChannelError::Config("discord public_key is not a valid Ed25519 key".into()))
}

/// Verify `X-Signature-Ed25519` over `X-Signature-Timestamp ++ body`.
pub fn verify_signature(
    key: &VerifyingKey,
    timestamp: &str,
    signature_hex: &str,
    body: &[u8],
) -> bool {
    let Some(signature) = unhex(signature_hex).and_then(|bytes| Signature::from_slice(&bytes).ok())
    else {
        return false;
    };
    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);
    key.verify(&message, &signature).is_ok()
}

/// HTTP receiver state.
pub struct DiscordReceiver {
    /// Platform core.
    pub core: Arc<Core<DiscordMessenger>>,
    /// Allow-lists (guild, channel, user).
    pub allow: AllowList,
    /// Application public key.
    pub public_key: VerifyingKey,
}

impl DiscordReceiver {
    /// Build the receiver state.
    pub fn new(
        core: Arc<Core<DiscordMessenger>>,
        allow: AllowList,
        public_key: VerifyingKey,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            allow,
            public_key,
        })
    }
}

/// Route: `POST /discord/interactions`.
pub fn router(receiver: Arc<DiscordReceiver>) -> Router {
    Router::new()
        .route("/discord/interactions", post(interactions))
        .with_state(receiver)
}

#[derive(Deserialize)]
struct Interaction {
    #[serde(rename = "type")]
    kind: u8,
    id: Option<String>,
    guild_id: Option<String>,
    channel_id: Option<String>,
    member: Option<Member>,
    user: Option<User>,
    data: Option<Data>,
    message: Option<MessageRef>,
}

#[derive(Deserialize)]
struct Member {
    user: User,
}

#[derive(Deserialize)]
struct User {
    id: String,
}

#[derive(Deserialize)]
struct MessageRef {
    id: String,
}

#[derive(Deserialize, Default)]
struct Data {
    name: Option<String>,
    custom_id: Option<String>,
    #[serde(default)]
    options: Vec<CommandOption>,
    resolved: Option<Resolved>,
}

#[derive(Deserialize)]
struct CommandOption {
    name: String,
    value: Option<Value>,
}

#[derive(Deserialize)]
struct Resolved {
    #[serde(default)]
    attachments: std::collections::HashMap<String, Attachment>,
}

#[derive(Deserialize)]
struct Attachment {
    url: String,
    content_type: Option<String>,
    size: Option<u64>,
}

impl Data {
    fn option(&self, name: &str) -> Option<&Value> {
        self.options
            .iter()
            .find(|option| option.name == name)
            .and_then(|option| option.value.as_ref())
    }

    fn text_option(&self, name: &str) -> String {
        self.option(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    }
}

async fn interactions(
    State(receiver): State<Arc<DiscordReceiver>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };
    if !verify_signature(
        &receiver.public_key,
        header("x-signature-timestamp"),
        header("x-signature-ed25519"),
        &body,
    ) {
        log("discord interaction rejected: bad signature");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(interaction) = serde_json::from_slice::<Interaction>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match interaction.kind {
        1 => Json(json!({"type": 1})).into_response(),
        2 => command(receiver, interaction),
        3 => component(receiver, interaction),
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}

fn ephemeral(content: &str) -> Response {
    Json(json!({"type": 4, "data": {"content": content, "flags": 64}})).into_response()
}

fn command(receiver: Arc<DiscordReceiver>, interaction: Interaction) -> Response {
    let (Some(channel), Some(data)) = (interaction.channel_id.clone(), interaction.data.as_ref())
    else {
        return ephemeral("Unsupported request.");
    };
    let Some(user) = interaction
        .member
        .as_ref()
        .map(|member| &member.user)
        .or(interaction.user.as_ref())
    else {
        return ephemeral("Unsupported request.");
    };
    if !receiver
        .allow
        .permits(&channel, &user.id, interaction.guild_id.as_deref())
    {
        return ephemeral("This bot is not enabled for you here.");
    }
    let name = data.name.clone().unwrap_or_default();
    let (text, audio, acknowledgement) = match name.as_str() {
        "ask" => {
            let audio = data
                .option("audio")
                .and_then(Value::as_str)
                .and_then(|id| data.resolved.as_ref()?.attachments.get(id))
                .filter(|attachment| {
                    attachment
                        .content_type
                        .as_deref()
                        .is_some_and(|mime| mime.starts_with("audio/"))
                })
                .map(|attachment| AudioRef {
                    reference: attachment.url.clone(),
                    size: attachment.size,
                });
            (
                data.text_option("text"),
                audio,
                "Sent to the agent.".to_owned(),
            )
        }
        "model" => (
            format!("/model {}", data.text_option("name")),
            None,
            "Updating the model setting.".to_owned(),
        ),
        "answer" => (
            format!("/answer {}", data.text_option("text")),
            None,
            "Sending your answer.".to_owned(),
        ),
        "new" | "stop" | "sessions" | "usage" | "help" => {
            (format!("/{name}"), None, format!("Running /{name}."))
        }
        _ => return ephemeral("Unknown command."),
    };
    let inbound = Inbound {
        thread: ThreadKey::new(PLATFORM, channel, ""),
        user: user.id.clone(),
        message_id: interaction.id.unwrap_or_default(),
        text,
        audio,
    };
    let core = receiver.core.clone();
    tokio::spawn(async move { core.handle_inbound(inbound).await });
    ephemeral(&acknowledgement)
}

fn component(receiver: Arc<DiscordReceiver>, interaction: Interaction) -> Response {
    let (Some(channel), Some(message), Some(data)) = (
        interaction.channel_id,
        interaction.message,
        interaction.data,
    ) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(user) = interaction
        .member
        .map(|member| member.user)
        .or(interaction.user)
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if receiver
        .allow
        .permits(&channel, &user.id, interaction.guild_id.as_deref())
    {
        let press = Press {
            thread: ThreadKey::new(PLATFORM, channel, ""),
            user: user.id,
            data: data.custom_id.unwrap_or_default(),
            message: MsgRef(message.id),
            token: String::new(),
        };
        let core = receiver.core.clone();
        tokio::spawn(async move { core.handle_press(press).await });
    }
    // DEFERRED_UPDATE_MESSAGE: the card is edited through REST once the press is handled.
    Json(json!({"type": 6})).into_response()
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    #[test]
    fn ed25519_signature_verifies_and_rejects_tampering() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let key = signing.verifying_key();
        let body = br#"{"type":1}"#;
        let signature = crate::text::hex(
            &signing
                .sign(&[b"1700000000".as_slice(), body].concat())
                .to_bytes(),
        );
        assert!(verify_signature(&key, "1700000000", &signature, body));
        assert!(!verify_signature(&key, "1700000001", &signature, body));
        assert!(!verify_signature(&key, "1700000000", &signature, b"{}"));
        assert!(!verify_signature(&key, "1700000000", "00", body));
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key();
        assert!(!verify_signature(&other, "1700000000", &signature, body));
    }

    #[test]
    fn public_key_parsing_rejects_garbage() {
        let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
        assert!(parse_public_key(&crate::text::hex(key.as_bytes())).is_ok());
        assert!(parse_public_key("abcd").is_err());
        assert!(parse_public_key("zz").is_err());
    }
}
