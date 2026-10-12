use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
    body::Incoming,
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, HOST},
    Request, Response,
};
use hyper_util::rt::TokioIo;
use openside_proto::{ApiError, ROUTES};
use serde::{de::DeserializeOwned, Serialize};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use url::{Host, Url};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
// A REPL snippet may run for 30 s after waiting for the browser gate and a browser launch.
const REPL_TIMEOUT: Duration = Duration::from_secs(90);

enum Endpoint {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

pub(crate) struct Client {
    endpoint: Endpoint,
    token: Option<HeaderValue>,
    key: String,
}

impl Client {
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
                _ => return Err("refusing non-loopback --url".into()),
            };
            let address = SocketAddr::new(
                ip,
                url.port_or_known_default()
                    .ok_or("--url needs a valid port")?,
            );
            let token = token
                .filter(|token| !token.is_empty())
                .ok_or("--url requires --token or OPENSIDE_TOKEN")?;
            let mut token = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "token is not a valid HTTP header value".to_owned())?;
            token.set_sensitive(true);
            Ok(Self {
                endpoint: Endpoint::Tcp(address),
                token: Some(token),
                key: format!("http://{address}"),
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
            })
        }
    }

    pub(crate) fn endpoint_key(&self) -> &str {
        &self.key
    }

    pub(crate) fn ticket_command(&self, id: &str, action: &str, value: Option<&str>) -> String {
        let connection = match &self.endpoint {
            Endpoint::Unix(path) => format!("--socket {}", shell_quote(&path.to_string_lossy())),
            Endpoint::Tcp(address) => format!("--url http://{address}"),
        };
        let mut command = format!("openside {connection} ticket {action} {}", shell_quote(id));
        if let Some(value) = value {
            command.push(' ');
            command.push_str(&shell_quote(value));
        }
        command
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.json::<_, serde_json::Value>("GET", path, None).await
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
            .header(HOST, "localhost");
        if !body.is_empty() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        if let Some(token) = &self.token {
            builder = builder.header(AUTHORIZATION, token);
        }
        if let Some(seq) = seq {
            builder = builder
                .header("accept", "text/event-stream")
                .header("last-event-id", seq.to_string());
        }
        let request = builder
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| "invalid API request")?;
        let wait = if route.path == "/v1/sessions/{id}/repl" {
            REPL_TIMEOUT
        } else if route.path == "/v1/accounts" {
            Duration::from_secs(15)
        } else {
            CONNECT_TIMEOUT
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
                    .map_err(|_| "could not connect to daemon socket")?;
                exchange(stream, request, wait).await?
            }
            Endpoint::Tcp(address) => {
                let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
                    .await
                    .map_err(|_| "daemon connection timed out")?
                    .map_err(|_| "could not connect to loopback daemon")?;
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
            Ok(error) => Err(format!(
                "API error {} (HTTP {status}): {}",
                error.code, error.message
            )),
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
        let Some(actual) = actual.next() else {
            return false;
        };
        if expected.starts_with('{') && expected.ends_with('}') {
            if actual.is_empty() {
                return false;
            }
        } else if actual != expected {
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
