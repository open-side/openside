use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::Incoming,
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, HOST},
    Request, Response,
};
use hyper_util::rt::TokioIo;
use openside_proto::{ApiError, ROUTES};
use openside_relay::{client::Tunnel, net::Origin};
use serde::{de::DeserializeOwned, Serialize};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use url::{Host, Url};

use crate::remotes::{self, Transport};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
// Relays and tunnels add TLS, a WebSocket upgrade and a Noise handshake before the request.
const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
// Response budget for routine local calls. A Unix-socket daemon that is merely busy (events
// replay, queued database work, a loaded CI machine) can take far longer than the connect
// budget, and a slow answer is not a dead daemon.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
// A REPL snippet may run for 30 s after waiting for the browser gate and a browser launch;
// a direct browser tool call (navigate, login) can wait as long.
const REPL_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_BYTES: usize = 32 << 20;

enum Endpoint {
    Unix(PathBuf),
    Tcp(SocketAddr),
    /// A remote profile reached over HTTPS (Tailscale Serve, Cloudflare Tunnel, a proxy).
    Direct {
        name: String,
        origin: Origin,
    },
    /// A remote profile reached through a relay, end-to-end encrypted to the daemon key.
    Relay {
        name: String,
        relay: String,
        daemon_id: String,
        daemon_key: [u8; 32],
        tunnel: tokio::sync::Mutex<Option<Tunnel>>,
    },
}

pub(crate) struct Client {
    endpoint: Endpoint,
    token: Option<HeaderValue>,
    key: String,
    host: String,
    agent_client: bool,
}

fn bearer(token: &str) -> Result<HeaderValue, String> {
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "token is not a valid HTTP header value".to_owned())?;
    value.set_sensitive(true);
    Ok(value)
}

impl Client {
    /// The connection every command uses: `--remote`/`OPENSIDE_REMOTE` (a profile saved by
    /// `openside connect`), else `--url` (loopback HTTP with a token), else the local socket.
    pub(crate) async fn from_flags(
        socket: Option<&Path>,
        url: Option<&str>,
        token: Option<&str>,
        remote: Option<&str>,
    ) -> Result<Self, String> {
        match remote {
            Some(name) => Self::remote(name),
            None => Self::new(socket, url, token),
        }
    }

    /// A saved remote profile with its stored device token.
    pub(crate) fn remote(name: &str) -> Result<Self, String> {
        let (profile, token) = remotes::load(name)?;
        let origin = Origin::parse(&profile.url).map_err(|error| error.to_string())?;
        let endpoint = match profile.transport {
            Transport::Direct => Endpoint::Direct {
                name: name.to_owned(),
                origin: origin.clone(),
            },
            Transport::Relay => {
                let (Some(daemon_id), Some(daemon_key)) = (
                    profile.daemon_id.clone(),
                    profile
                        .daemon_key
                        .as_deref()
                        .and_then(openside_relay::noise::decode_key),
                ) else {
                    return Err(format!(
                        "remote {name} is a relay profile without a daemon key; run `openside connect` again"
                    ));
                };
                Endpoint::Relay {
                    name: name.to_owned(),
                    relay: profile.url.clone(),
                    daemon_id,
                    daemon_key,
                    tunnel: tokio::sync::Mutex::new(None),
                }
            }
        };
        Ok(Self {
            host: origin.authority(),
            endpoint,
            token: Some(bearer(&token)?),
            key: format!("remote:{name}"),
            agent_client: false,
        })
    }

    /// An unauthenticated client for `POST /v1/pairing/exchange` during `openside connect`.
    pub(crate) fn pairing(link: &remotes::PairingLink) -> Result<Self, String> {
        let origin = Origin::parse(&link.origin).map_err(|error| error.to_string())?;
        let endpoint = match &link.relay {
            None => Endpoint::Direct {
                name: String::new(),
                origin: origin.clone(),
            },
            Some((daemon_id, daemon_key)) => Endpoint::Relay {
                name: String::new(),
                relay: link.origin.clone(),
                daemon_id: daemon_id.clone(),
                daemon_key: *daemon_key,
                tunnel: tokio::sync::Mutex::new(None),
            },
        };
        Ok(Self {
            host: origin.authority(),
            endpoint,
            token: None,
            agent_client: false,
            key: format!("pairing:{}", origin.http_url()),
        })
    }

    pub(crate) fn new(
        socket: Option<&Path>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> Result<Self, String> {
        if let Some(url) = url {
            let url =
                Url::parse(url).map_err(|_| "--url must be an HTTP loopback URL".to_owned())?;
            if url.scheme() != "http"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("--url must be an HTTP loopback URL without credentials, path, query, or fragment".into());
            }
            let ip = match url.host() {
                Some(Host::Ipv4(ip)) if ip.is_loopback() => IpAddr::V4(ip),
                Some(Host::Ipv6(ip)) if ip.is_loopback() => IpAddr::V6(ip),
                Some(Host::Domain("localhost")) => IpAddr::V4(Ipv4Addr::LOCALHOST),
                _ => return Err(
                    "refusing non-loopback --url (pair with `openside connect` and use --remote)"
                        .into(),
                ),
            };
            let address = SocketAddr::new(
                ip,
                url.port_or_known_default()
                    .ok_or("--url needs a valid port")?,
            );
            let token = token
                .filter(|token| !token.is_empty())
                .ok_or("--url requires --token or OPENSIDE_TOKEN")?;
            Ok(Self {
                endpoint: Endpoint::Tcp(address),
                token: Some(bearer(token)?),
                key: format!("http://{address}"),
                host: "localhost".into(),
                agent_client: false,
            })
        } else {
            let socket = match socket {
                Some(socket) => socket.to_owned(),
                None => openside_config::load(None)
                    .map_err(|err| format!("{err}; supply --socket"))?
                    .socket_path(),
            };
            let socket = if socket.is_absolute() {
                socket
            } else {
                std::env::current_dir()
                    .map_err(|_| "could not resolve relative socket path")?
                    .join(socket)
            };
            Ok(Self {
                key: format!("unix:{}", socket.display()),
                endpoint: Endpoint::Unix(socket),
                token: None,
                host: "localhost".into(),
                agent_client: false,
            })
        }
    }

    pub(crate) fn with_agent_client(mut self) -> Self {
        self.agent_client = true;
        self
    }

    pub(crate) fn endpoint_key(&self) -> &str {
        &self.key
    }

    /// Whether requests leave this machine (a `--remote` profile).
    pub(crate) fn is_remote(&self) -> bool {
        matches!(
            self.endpoint,
            Endpoint::Direct { .. } | Endpoint::Relay { .. }
        )
    }

    // A socket or relay does not establish an HTTP address for the daemon's web app.
    pub(crate) fn web_origin(&self) -> Option<String> {
        match &self.endpoint {
            Endpoint::Tcp(address) => Some(format!("http://{address}")),
            Endpoint::Direct { origin, .. } => Some(origin.http_url()),
            Endpoint::Unix(_) | Endpoint::Relay { .. } => None,
        }
    }

    fn command_prefix(&self) -> String {
        let connection = match &self.endpoint {
            Endpoint::Unix(path) => format!("--socket {}", shell_quote(&path.to_string_lossy())),
            Endpoint::Tcp(address) => format!("--url http://{address}"),
            Endpoint::Direct { name, .. } | Endpoint::Relay { name, .. } => {
                format!("--remote {}", shell_quote(name))
            }
        };
        format!("openside {connection}")
    }

    pub(crate) fn team_command(&self, action: &str, id: Option<&str>) -> String {
        let mut command = format!("{} team {action}", self.command_prefix());
        if let Some(id) = id {
            command.push(' ');
            command.push_str(&shell_quote(id));
        }
        command
    }

    pub(crate) fn ticket_command(&self, id: &str, action: &str, value: Option<&str>) -> String {
        let mut command = format!(
            "{} ticket {action} {}",
            self.command_prefix(),
            shell_quote(id)
        );
        if let Some(value) = value {
            command.push(' ');
            command.push_str(&shell_quote(value));
        }
        command
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.json::<_, serde_json::Value>("GET", path, None).await
    }

    /// `GET` a non-JSON route (artifact bytes): `(Content-Type, body)`, at most 32 MiB.
    pub(crate) async fn bytes(&self, path: &str) -> Result<(String, Vec<u8>), String> {
        let response = self.request("GET", path, Vec::new(), None).await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let body = Limited::new(response.into_body(), MAX_BYTES)
            .collect()
            .await
            .map_err(|_| "could not read API response (or it exceeds 32 MiB)")?
            .to_bytes();
        Ok((content_type, body.to_vec()))
    }

    /// A fresh relay stream, reconnecting the shared tunnel when it has ended.
    async fn relay_stream(
        relay: &str,
        daemon_id: &str,
        daemon_key: &[u8; 32],
        tunnel: &tokio::sync::Mutex<Option<Tunnel>>,
    ) -> Result<openside_relay::mux::MuxStream, String> {
        let mut slot = tunnel.lock().await;
        if let Some(current) = slot.as_ref().filter(|tunnel| !tunnel.is_closed()) {
            if let Ok(stream) = current.open().await {
                return Ok(stream);
            }
        }
        let fresh = tokio::time::timeout(
            REMOTE_CONNECT_TIMEOUT,
            Tunnel::connect(relay, daemon_id, daemon_key),
        )
        .await
        .map_err(|_| format!("relay {relay} did not answer"))?
        .map_err(|error| format!("relay {relay}: {error}"))?;
        let stream = fresh.open().await.map_err(|error| error.to_string())?;
        *slot = Some(fresh);
        Ok(stream)
    }

    pub(crate) async fn json<T: DeserializeOwned, B: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, String> {
        let body = body
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| "could not encode API request")?
            .unwrap_or_default();
        let response = self.request(method, path, body, None).await?;
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|_| "could not read API response")?
            .to_bytes();
        serde_json::from_slice(&bytes).map_err(|_| "invalid JSON in API response".into())
    }

    /// A call whose success response has no body (204).
    pub(crate) async fn send(&self, method: &str, path: &str) -> Result<(), String> {
        self.request(method, path, Vec::new(), None).await.map(drop)
    }

    pub(crate) async fn send_json<B: Serialize>(
        &self,
        method: &str,
        path: &str,
        body: &B,
    ) -> Result<(), String> {
        let body = serde_json::to_vec(body).map_err(|_| "could not encode API request")?;
        self.request(method, path, body, None).await.map(drop)
    }

    pub(crate) async fn events(&self, id: &str, seq: u64) -> Result<Response<Incoming>, String> {
        let response = self
            .request("GET", &session_path(id, "events"), Vec::new(), Some(seq))
            .await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next());
        if content_type != Some("text/event-stream") {
            return Err("events endpoint did not return text/event-stream".into());
        }
        Ok(response)
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        seq: Option<u64>,
    ) -> Result<Response<Incoming>, String> {
        let route = ROUTES
            .iter()
            .find(|route| route.method.as_str() == method && route_matches(route.path, path))
            .ok_or("request is not part of the public API v1")?;
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, &self.host);
        if !body.is_empty() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        if let Some(token) = &self.token {
            builder = builder.header(AUTHORIZATION, token);
        }
        if self.agent_client {
            builder = builder.header("x-openside-client", "mcp");
        }
        if let Some(seq) = seq {
            builder = builder
                .header("accept", "text/event-stream")
                .header("last-event-id", seq.to_string());
        }
        let request = builder
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| "invalid API request")?;
        let wait = if matches!(
            route.path,
            "/v1/sessions/{id}/repl" | "/v1/sessions/{id}/tools/{name}"
        ) {
            REPL_TIMEOUT
        } else if route.path == "/v1/accounts" {
            Duration::from_secs(15)
        } else if self.is_remote() {
            REMOTE_CONNECT_TIMEOUT
        } else {
            REQUEST_TIMEOUT
        };
        let response = match &self.endpoint {
            #[cfg(not(unix))]
            Endpoint::Unix(_) => {
                return Err("Unix daemon transport is unsupported on this platform".into());
            }
            #[cfg(unix)]
            Endpoint::Unix(path) => {
                let stream = tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(path))
                    .await
                    .map_err(|_| "daemon connection timed out")?
                    .map_err(|_| {
                        format!(
                            "could not connect to daemon socket {} (is opensided running? `openside service install` sets it up)",
                            path.display()
                        )
                    })?;
                exchange(stream, request, wait).await?
            }
            Endpoint::Tcp(address) => {
                let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
                    .await
                    .map_err(|_| "daemon connection timed out")?
                    .map_err(|_| "could not connect to loopback daemon")?;
                exchange(stream, request, wait).await?
            }
            Endpoint::Direct { origin, .. } => {
                let stream = openside_relay::net::connect(origin)
                    .await
                    .map_err(|error| format!("could not reach {}: {error}", origin.http_url()))?;
                exchange(stream, request, wait).await?
            }
            Endpoint::Relay {
                relay,
                daemon_id,
                daemon_key,
                tunnel,
                ..
            } => {
                let stream = Self::relay_stream(relay, daemon_id, daemon_key, tunnel).await?;
                exchange(stream, request, wait).await?
            }
        };
        if response.status().as_u16() == route.success_status {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|_| format!("API error (HTTP {status})"))?
            .to_bytes();
        match serde_json::from_slice::<ApiError>(&bytes) {
            Ok(error) => {
                let message = format!(
                    "API error {} (HTTP {status}): {}",
                    error.code, error.message
                );
                if error.code == "person_approval_required" {
                    return Err(format!("{message}. This decision needs a person-device token, not an agent client or the local socket. Ask a person on a paired web/phone/extension/channel device, then use ticket_wait. The operator can issue a person token with openside token create --person."));
                }
                if error.code == "self_approval_forbidden" {
                    Err(format!("{message}. This client cannot approve its own work. Ask a person to approve on another paired device, then wait with MCP ticket_wait (CLI browser/run commands wait for the ticket)."))
                } else {
                    Err(message)
                }
            }
            Err(_) => Err(format!("API error (HTTP {status})")),
        }
    }
}

async fn exchange<S>(
    stream: S,
    request: Request<Full<Bytes>>,
    wait: Duration,
) -> Result<Response<Incoming>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| "daemon HTTP handshake failed")?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    tokio::time::timeout(wait, sender.send_request(request))
        .await
        .map_err(|_| "daemon response timed out".to_owned())?
        .map_err(|_| "daemon HTTP request failed".into())
}

fn route_matches(template: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let mut actual = path.split('/');
    for expected in template.split('/') {
        let Some(segment) = actual.next() else {
            return false;
        };
        if expected.starts_with('{') && expected.ends_with('}') {
            if segment.is_empty() {
                return false;
            }
        } else if segment != expected {
            return false;
        }
    }
    actual.next().is_none()
}

pub(crate) fn session_path(id: &str, suffix: &str) -> String {
    let encoded = encode_segment(id);
    if suffix.is_empty() {
        format!("/v1/sessions/{encoded}")
    } else {
        format!("/v1/sessions/{encoded}/{suffix}")
    }
}

pub(crate) fn ticket_path(id: &str, suffix: &str) -> String {
    let encoded = encode_segment(id);
    if suffix.is_empty() {
        format!("/v1/tickets/{encoded}")
    } else {
        format!("/v1/tickets/{encoded}/{suffix}")
    }
}

pub(crate) fn encode_segment(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            result.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::route_matches;

    #[test]
    fn encoded_artifact_paths_preserve_exact_route_segments() {
        let template = "/v1/sessions/{id}/artifacts/{path}";
        assert!(route_matches(
            template,
            "/v1/sessions/s/artifacts/screenshot.png"
        ));
        assert!(route_matches(
            template,
            "/v1/sessions/s/artifacts/screenshots%2Ftab.png?download=1"
        ));
        assert!(!route_matches(
            template,
            "/v1/sessions/s/artifacts/screenshots/tab.png"
        ));
        assert!(!route_matches(template, "/v1/sessions/s/artifacts/"));
        assert!(!route_matches(
            template,
            "/v1/sessions/s/artifacts//tab.png"
        ));
        assert!(!route_matches(template, "/v1/sessions//artifacts/tab.png"));
        assert!(route_matches("/v1/tickets/{id}", "/v1/tickets/t"));
        assert!(!route_matches("/v1/tickets/{id}", "/v1/tickets/t/extra"));
        assert!(!route_matches("/v1/tools", "/v1/tools/extra"));
        assert!(!route_matches("/v1/{path}/status", "/v1/a/b/status"));
    }
}
