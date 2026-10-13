use std::{collections::VecDeque, pin::Pin, time::Duration};

use futures_util::{Stream, StreamExt};
use openside_proto::{
    ApiError, CreateSessionRequest, Event, Message, MessageMode, RejectTicketRequest,
    ResolveTicketRequest, Session, SessionList, Settings, SubmitMessageRequest, Ticket, TicketList,
    ROUTES,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::{Host, Url};

use crate::{
    error::{ChannelError, Result},
    secret::Secret,
    sse::Decoder,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Client of the public daemon API v1. Every request is checked against
/// [`openside_proto::ROUTES`] before it is sent, so adapters cannot grow a private path.
#[derive(Clone)]
pub struct DaemonClient {
    http: reqwest::Client,
    stream: reqwest::Client,
    base: Url,
    token: Secret,
    agent_id: Option<String>,
}

impl DaemonClient {
    /// `base` must be `http://` on a loopback host or `https://`; the bearer token is
    /// never sent over cleartext to a remote host.
    pub fn new(base: &str, token: Secret) -> Result<Self> {
        let mut url = Url::parse(base)
            .map_err(|_| ChannelError::Config("daemon URL is not a valid URL".into()))?;
        if !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ChannelError::Config(
                "daemon URL must be a bare origin without credentials, path, query or fragment"
                    .into(),
            ));
        }
        let loopback = match url.host() {
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            Some(Host::Domain(name)) => name == "localhost",
            None => false,
        };
        match url.scheme() {
            "https" => {}
            "http" if loopback => {}
            _ => {
                return Err(ChannelError::Config(
                    "daemon URL must be https, or http on a loopback host".into(),
                ))
            }
        }
        url.set_path("/");
        let build = |timeout: bool| {
            let builder = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5));
            let builder = if timeout {
                builder.timeout(REQUEST_TIMEOUT)
            } else {
                builder
            };
            builder.build().map_err(ChannelError::http)
        };
        Ok(Self {
            http: build(true)?,
            stream: build(false)?,
            base: url,
            token,
            agent_id: None,
        })
    }

    /// Bind channel traffic to one effective owner; old sessions keep their immutable tags.
    pub fn for_agent(mut self, agent_id: Option<String>) -> Self {
        self.agent_id = Some(agent_id.unwrap_or_else(|| "chief".into()));
        self
    }

    pub(crate) fn owns_session(&self, session: &Session) -> bool {
        self.agent_id
            .as_deref()
            .map_or(true, |owner| session.agent_id.as_deref() == Some(owner))
    }

    pub(crate) fn has_owner(&self) -> bool {
        self.agent_id.is_some()
    }

    pub(crate) async fn owner_name(&self) -> Result<String> {
        let owner = self
            .agent_id
            .as_deref()
            .ok_or_else(|| ChannelError::Config("channel owner is not configured".into()))?;
        let agent: openside_proto::Agent = self
            .call(Method::GET, &format!("/v1/agents/{owner}"), None)
            .await?;
        if agent.status != openside_proto::AgentStatus::Active {
            return Err(ChannelError::Refused("agent_retired".into()));
        }
        Ok(agent.name)
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        check_public(method.as_str(), path)?;
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .header("x-openside-client", "channel")
            .bearer_auth(self.token.expose());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(ChannelError::http)?;
        let status = response.status();
        if !status.is_success() {
            let code = response
                .json::<ApiError>()
                .await
                .map(|error| error.code)
                .unwrap_or_default();
            return Err(ChannelError::Daemon {
                status: status.as_u16(),
                code,
            });
        }
        response.json().await.map_err(ChannelError::http)
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path.trim_start_matches('/'))
            .map_err(|_| ChannelError::Config("invalid daemon path".into()))
    }

    /// `POST /v1/sessions`.
    pub async fn create_session(&self, request: &CreateSessionRequest) -> Result<Session> {
        let mut body = to_json(request);
        if let (Some(agent_id), Some(body)) = (&self.agent_id, body.as_mut()) {
            body["agent_id"] = Value::String(agent_id.clone());
        }
        self.call(Method::POST, "/v1/sessions", body).await
    }

    /// `GET /v1/sessions/{id}`.
    pub async fn session(&self, id: &str) -> Result<Session> {
        self.call(Method::GET, &session_path(id, ""), None).await
    }

    /// `GET /v1/sessions`.
    pub async fn sessions(&self) -> Result<SessionList> {
        self.call(Method::GET, "/v1/sessions", None).await
    }

    /// `GET /v1/settings`.
    pub async fn settings(&self) -> Result<Settings> {
        self.call(Method::GET, "/v1/settings", None).await
    }

    /// `POST /v1/sessions/{id}/messages` in follow-up (`queue`) mode.
    pub async fn submit(&self, id: &str, message_id: &str, text: &str) -> Result<Message> {
        let request = SubmitMessageRequest {
            message_id: Some(openside_proto::MessageId::new(message_id)),
            text: text.to_owned(),
            mode: MessageMode::Queue,
            attachments: Vec::new(),
        };
        self.call(
            Method::POST,
            &session_path(id, "/messages"),
            to_json(&request),
        )
        .await
    }

    /// `POST /v1/sessions/{id}/stop`.
    pub async fn stop(&self, id: &str) -> Result<Session> {
        self.call(Method::POST, &session_path(id, "/stop"), None)
            .await
    }

    /// `GET /v1/tickets?status=pending`.
    pub async fn pending_tickets(&self) -> Result<TicketList> {
        self.call(Method::GET, "/v1/tickets?status=pending", None)
            .await
    }

    /// `GET /v1/tickets/{id}`.
    pub async fn ticket(&self, id: &str) -> Result<Ticket> {
        self.call(Method::GET, &ticket_path(id, ""), None).await
    }

    /// `POST /v1/tickets/{id}/resolve`.
    pub async fn resolve(&self, id: &str, value: Value) -> Result<Ticket> {
        let request = ResolveTicketRequest { hire: None, value };
        self.call(
            Method::POST,
            &ticket_path(id, "/resolve"),
            to_json(&request),
        )
        .await
    }

    /// `POST /v1/tickets/{id}/reject`.
    pub async fn reject(&self, id: &str, reason: &str) -> Result<Ticket> {
        let request = RejectTicketRequest {
            reason: Some(reason.to_owned()),
        };
        self.call(Method::POST, &ticket_path(id, "/reject"), to_json(&request))
            .await
    }

    /// `GET /v1/sessions/{id}/events` after the exclusive sequence `after`.
    pub async fn events(&self, id: &str, after: u64) -> Result<EventStream> {
        let path = format!("{}?subagents=false", session_path(id, "/events"));
        check_public("GET", &path)?;
        let response = self
            .stream
            .get(self.url(&path)?)
            .bearer_auth(self.token.expose())
            .header("accept", "text/event-stream")
            .header("last-event-id", after.to_string())
            .send()
            .await
            .map_err(ChannelError::http)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ChannelError::Daemon {
                status: status.as_u16(),
                code: String::new(),
            });
        }
        let stream: Pin<Box<dyn Stream<Item = reqwest::Result<Vec<u8>>> + Send>> = Box::pin(
            response
                .bytes_stream()
                .map(|chunk| chunk.map(|bytes| bytes.to_vec())),
        );
        Ok(EventStream {
            stream,
            decoder: Decoder::default(),
            queue: VecDeque::new(),
        })
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> Option<Value> {
    serde_json::to_value(value).ok()
}

/// Server-sent events decoded into [`Event`]s; unknown event kinds are skipped.
pub struct EventStream {
    stream: Pin<Box<dyn Stream<Item = reqwest::Result<Vec<u8>>> + Send>>,
    decoder: Decoder,
    queue: VecDeque<Event>,
}

impl EventStream {
    /// Next event, `None` when the connection closed.
    pub async fn next(&mut self) -> Option<Result<Event>> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Some(Ok(event));
            }
            match self.stream.next().await? {
                Ok(chunk) => {
                    for record in self.decoder.push(&chunk) {
                        if let Ok(event) = serde_json::from_slice::<Event>(&record.data) {
                            self.queue.push_back(event);
                        }
                    }
                }
                Err(error) => return Some(Err(ChannelError::http(error))),
            }
        }
    }
}

pub(crate) fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn session_path(id: &str, suffix: &str) -> String {
    format!("/v1/sessions/{}{suffix}", encode_segment(id))
}

fn ticket_path(id: &str, suffix: &str) -> String {
    format!("/v1/tickets/{}{suffix}", encode_segment(id))
}

fn template_matches(template: &str, path: &str) -> bool {
    let template: Vec<_> = template.split('/').collect();
    let path: Vec<_> = path.split('/').collect();
    template.len() == path.len()
        && template.iter().zip(&path).all(|(t, p)| {
            if t.starts_with('{') && t.ends_with('}') {
                !p.is_empty()
            } else {
                t == p
            }
        })
}

/// Reject any `(method, path)` that is not a documented public API v1 route.
pub fn check_public(method: &str, path_and_query: &str) -> Result<()> {
    let path = path_and_query.split('?').next().unwrap_or_default();
    if ROUTES
        .iter()
        .any(|route| route.method.as_str() == method && template_matches(route.path, path))
    {
        Ok(())
    } else {
        Err(ChannelError::Refused(format!(
            "{method} {path} is not part of the public API v1"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_documented_routes_pass_the_public_api_check() {
        assert!(check_public("POST", "/v1/sessions/abc/messages").is_ok());
        assert!(check_public("GET", "/v1/tickets?status=pending").is_ok());
        assert!(check_public("GET", "/v1/sessions/abc/events?subagents=false").is_ok());
        assert!(check_public("GET", "/v1/internal/secret").is_err());
        assert!(check_public("DELETE", "/v1/sessions/abc/messages").is_err());
        assert!(check_public("POST", "/v1/sessions//messages").is_err());
    }

    #[test]
    fn ids_are_percent_encoded_into_single_segments() {
        assert_eq!(session_path("a/b", "/stop"), "/v1/sessions/a%2Fb/stop");
    }

    #[test]
    fn base_url_rules_protect_the_bearer_token() {
        let token = || Secret::new("t");
        assert!(DaemonClient::new("http://127.0.0.1:8080", token()).is_ok());
        assert!(DaemonClient::new("https://daemon.example", token()).is_ok());
        assert!(DaemonClient::new("http://daemon.example", token()).is_err());
        assert!(DaemonClient::new("http://127.0.0.1:1/v1", token()).is_err());
    }
}
