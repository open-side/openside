//! Minimal MCP client: JSON-RPC 2.0 over a child's stdin/stdout or over the Streamable HTTP
//! transport. It implements only what a tool consumer needs (initialize, tools/list,
//! tools/call, ping, cancellation, list-changed notifications).

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    },
    time::Duration,
};

use futures_util::StreamExt;
use openside_config::{McpServerConfig, McpTransport};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::{oneshot, Mutex, Notify},
};

pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// A single message or response body larger than this is refused.
const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const STDERR_TAIL: usize = 1024;
/// Environment a stdio server inherits from the daemon; everything else is withheld so the
/// daemon's own credentials never reach a third-party process.
const INHERITED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TMPDIR",
    "SHELL",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "SYSTEMROOT",
    "APPDATA",
    "LOCALAPPDATA",
    "USERPROFILE",
];

/// A failure without secret values; `Display` is safe to show the person after redaction.
#[derive(Debug, Clone)]
pub(crate) struct ClientError(pub(crate) String);

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type Pending = Arc<parking_lot::Mutex<HashMap<i64, oneshot::Sender<Result<Value, ClientError>>>>>;

fn rpc_error(error: &Value) -> ClientError {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("request failed");
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    ClientError(format!("server error {code}: {}", truncate(message, 300)))
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Resolves the server once and accepts only this computer or a globally routable address
/// (plus, with `allow_private_network`, a LAN or tailnet address), so a configured URL or a
/// DNS answer for it cannot reach other networks or cloud metadata services with the
/// server's credentials.
async fn checked_peer(
    url: &str,
    allow_private_network: bool,
) -> Result<(String, std::net::SocketAddr), ClientError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| ClientError("invalid URL".into()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| ClientError("invalid URL".into()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ClientError("invalid URL".into()))?;
    let addresses: Vec<std::net::SocketAddr> = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host.as_str(), port)),
    )
    .await
    .map_err(|_| ClientError("server name lookup timed out".into()))?
    .map_err(|_| ClientError("server name not found".into()))?
    .collect();
    let allowed = |address: &std::net::SocketAddr| {
        let ip = address.ip().to_canonical();
        ip.is_loopback()
            || crate::net_guard::public_ip(ip)
            || (allow_private_network && crate::net_guard::private_network_ip(ip))
    };
    match addresses.first() {
        Some(first) if addresses.iter().all(allowed) => Ok((host, *first)),
        Some(_) if allow_private_network => Err(ClientError(
            "server address is link-local, a cloud metadata service or reserved; it cannot be reached"
                .into(),
        )),
        Some(_) => Err(ClientError(
            "server address is on a private network; only this computer or public hosts are allowed"
                .into(),
        )),
        None => Err(ClientError("server name not found".into())),
    }
}

/// Reply owed to a server-initiated request: only `ping` is supported.
fn reply_to(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str()?;
    Some(if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
    })
}

/// One connected server.
pub(crate) struct Client {
    transport: Transport,
    next_id: AtomicI64,
    /// Signalled when the server reports a changed tool list.
    pub(crate) tools_changed: Arc<Notify>,
    /// `serverInfo.name` from the handshake.
    pub(crate) server_name: Option<String>,
}

enum Transport {
    Stdio(Stdio),
    Http(Http),
}

impl Client {
    /// Start (stdio) or reach (http) the server and complete the handshake.
    pub(crate) async fn connect(
        config: &McpServerConfig,
        secrets: Vec<(String, String)>,
        data_dir: &std::path::Path,
    ) -> Result<Self, ClientError> {
        let tools_changed = Arc::new(Notify::new());
        let transport = match config.transport {
            McpTransport::Stdio => Transport::Stdio(Stdio::spawn(
                config,
                secrets,
                data_dir,
                tools_changed.clone(),
            )?),
            McpTransport::Http => Transport::Http(Http::new(config, secrets).await?),
        };
        let mut client = Self {
            transport,
            next_id: AtomicI64::new(1),
            tools_changed,
            server_name: None,
        };
        let result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "openside", "version": env!("CARGO_PKG_VERSION")}
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await
            .map_err(|error| client.with_stderr(error))?;
        if let Transport::Http(http) = &client.transport {
            if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
                *http.protocol.lock() = Some(version.to_owned());
            }
        }
        client.server_name = result
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .map(|name| truncate(name, 80));
        client
            .notify("notifications/initialized", json!({}))
            .await
            .map_err(|error| client.with_stderr(error))?;
        Ok(client)
    }

    fn with_stderr(&self, error: ClientError) -> ClientError {
        match &self.transport {
            Transport::Stdio(stdio) => {
                let tail = stdio.stderr.lock().trim().to_owned();
                if tail.is_empty() {
                    error
                } else {
                    ClientError(format!("{error}; stderr: {tail}"))
                }
            }
            Transport::Http(_) => error,
        }
    }

    /// Every tool, following pagination (bounded).
    pub(crate) async fn list_tools(&self) -> Result<Vec<Value>, ClientError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let page = self
                .request("tools/list", params, HANDSHAKE_TIMEOUT)
                .await
                .map_err(|error| self.with_stderr(error))?;
            if let Some(items) = page.get("tools").and_then(Value::as_array) {
                tools.extend(items.iter().cloned());
            }
            if tools.len() >= 512 {
                tools.truncate(512);
                break;
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Call a tool; cancelling the token tells the server to stop.
    pub(crate) async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = self.request_with_id(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            CALL_TIMEOUT,
        );
        tokio::select! {
            result = request => result,
            () = cancel.cancelled() => {
                let _ = self
                    .notify(
                        "notifications/cancelled",
                        json!({"requestId": id, "reason": "cancelled by openside"}),
                    )
                    .await;
                Err(ClientError("cancelled".into()))
            }
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        match &self.transport {
            Transport::Stdio(stdio) => stdio.closed.load(Ordering::SeqCst),
            Transport::Http(_) => false,
        }
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.request_with_id(id, method, params, timeout).await
    }

    async fn request_with_id(
        &self,
        id: i64,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if message.to_string().len() > MAX_MESSAGE {
            return Err(ClientError(format!("{method} request is too large")));
        }
        let work = async {
            match &self.transport {
                Transport::Stdio(stdio) => stdio.request(id, &message).await,
                Transport::Http(http) => http.request(id, &message, &self.tools_changed).await,
            }
        };
        let result = tokio::time::timeout(timeout, work).await;
        if let Transport::Stdio(stdio) = &self.transport {
            stdio.pending.lock().remove(&id);
        }
        result.map_err(|_| ClientError(format!("{method} timed out")))?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), ClientError> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if message.to_string().len() > MAX_MESSAGE {
            return Err(ClientError(format!("{method} notification is too large")));
        }
        let work = async {
            match &self.transport {
                Transport::Stdio(stdio) => stdio.send(&message).await,
                Transport::Http(http) => http.notify(&message).await,
            }
        };
        tokio::time::timeout(HANDSHAKE_TIMEOUT, work)
            .await
            .map_err(|_| ClientError(format!("{method} timed out")))?
    }
}

struct Stdio {
    child: Mutex<tokio::process::Child>,
    pid: Option<u32>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    stderr: Arc<parking_lot::Mutex<String>>,
}

impl Stdio {
    fn spawn(
        config: &McpServerConfig,
        secrets: Vec<(String, String)>,
        data_dir: &std::path::Path,
        tools_changed: Arc<Notify>,
    ) -> Result<Self, ClientError> {
        let argv = config
            .command
            .as_deref()
            .filter(|argv| !argv.is_empty())
            .ok_or_else(|| ClientError("no command configured".into()))?;
        let mut command = tokio::process::Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .current_dir(config.cwd.as_deref().unwrap_or(data_dir));
        for name in INHERITED_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (name, value) in secrets {
            command.env(name, value);
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| ClientError(format!("could not start {}: {error}", argv[0])))?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ClientError("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ClientError("no stdout".into()))?;
        let stderr_pipe = child.stderr.take();
        let stdin = Arc::new(Mutex::new(stdin));
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let stderr = Arc::new(parking_lot::Mutex::new(String::new()));
        if let Some(mut pipe) = stderr_pipe {
            let tail = stderr.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                while let Ok(read) = pipe.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    let mut tail = tail.lock();
                    tail.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    if tail.len() > STDERR_TAIL {
                        let mut cut = tail.len() - STDERR_TAIL;
                        while !tail.is_char_boundary(cut) {
                            cut += 1;
                        }
                        tail.drain(..cut);
                    }
                }
            });
        }
        {
            let pending = pending.clone();
            let closed = closed.clone();
            let stdin = stdin.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let mut limited = (&mut lines).take(MAX_MESSAGE as u64 + 1);
                    match limited.read_until(b'\n', &mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line.len() > MAX_MESSAGE => break,
                        Ok(_) => {}
                    }
                    let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                        continue;
                    };
                    if message.get("method").is_some() {
                        if let Some(reply) = reply_to(&message) {
                            let mut bytes = reply.to_string().into_bytes();
                            bytes.push(b'\n');
                            let mut stdin = stdin.lock().await;
                            let _ = stdin.write_all(&bytes).await;
                            let _ = stdin.flush().await;
                        } else if message["method"] == "notifications/tools/list_changed" {
                            tools_changed.notify_one();
                        }
                        continue;
                    }
                    let Some(id) = message.get("id").and_then(Value::as_i64) else {
                        continue;
                    };
                    if let Some(sender) = pending.lock().remove(&id) {
                        let result = match message.get("error") {
                            Some(error) => Err(rpc_error(error)),
                            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = sender.send(result);
                    }
                }
                closed.store(true, Ordering::SeqCst);
                for (_, sender) in pending.lock().drain() {
                    let _ = sender.send(Err(ClientError("server exited".into())));
                }
            });
        }
        Ok(Self {
            child: Mutex::new(child),
            pid,
            stdin,
            pending,
            closed,
            stderr,
        })
    }

    async fn send(&self, message: &Value) -> Result<(), ClientError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ClientError("server exited".into()));
        }
        let mut bytes = message.to_string().into_bytes();
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(&bytes)
            .await
            .map_err(|_| ClientError("server exited".into()))?;
        stdin
            .flush()
            .await
            .map_err(|_| ClientError("server exited".into()))
    }

    async fn request(&self, id: i64, message: &Value) -> Result<Value, ClientError> {
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().insert(id, sender);
        self.send(message).await?;
        receiver
            .await
            .map_err(|_| ClientError("server exited".into()))?
    }
}

impl Drop for Stdio {
    fn drop(&mut self) {
        // npx/uvx launchers leave the real server as a grandchild; kill the whole group.
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|pid| i32::try_from(pid).ok()) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

struct Http {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session: parking_lot::Mutex<Option<String>>,
    protocol: parking_lot::Mutex<Option<String>>,
}

impl Http {
    async fn new(
        config: &McpServerConfig,
        secrets: Vec<(String, String)>,
    ) -> Result<Self, ClientError> {
        let url = config
            .url
            .clone()
            .ok_or_else(|| ClientError("no URL configured".into()))?;
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in secrets {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ClientError("invalid header name".into()))?;
            let mut value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| ClientError(format!("invalid value for header {name}")))?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        let peer = checked_peer(&url, config.allow_private_network).await?;
        let client = reqwest::Client::builder()
            // Following a redirect could hand the configured credentials to another host.
            .redirect(reqwest::redirect::Policy::none())
            // Pinned to the address checked above so a later DNS answer cannot move the
            // connection onto this host's network.
            .resolve(&peer.0, peer.1)
            .connect_timeout(Duration::from_secs(10))
            // Also bounds notifications and replies to server requests, which have no
            // JSON-RPC deadline of their own.
            .timeout(CALL_TIMEOUT + Duration::from_secs(5))
            .build()
            .map_err(|_| ClientError("HTTP client unavailable".into()))?;
        Ok(Self {
            client,
            url,
            headers,
            session: parking_lot::Mutex::new(None),
            protocol: parking_lot::Mutex::new(None),
        })
    }

    fn post(&self, message: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(message.to_string());
        if let Some(session) = self.session.lock().clone() {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = self.protocol.lock().clone() {
            request = request.header("MCP-Protocol-Version", version);
        }
        request
    }

    async fn send(&self, message: &Value) -> Result<reqwest::Response, ClientError> {
        let response = self
            .post(message)
            .send()
            .await
            .map_err(|error| ClientError(format!("request failed: {}", error.without_url())))?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self.session.lock() = Some(session.to_owned());
        }
        let status = response.status();
        if status.is_redirection() {
            return Err(ClientError(format!("server redirected ({status})")));
        }
        if !status.is_success() {
            return Err(ClientError(match status.as_u16() {
                401 | 403 => format!("server refused the credentials ({status})"),
                _ => format!("server answered {status}"),
            }));
        }
        Ok(response)
    }

    async fn notify(&self, message: &Value) -> Result<(), ClientError> {
        self.send(message).await.map(drop)
    }

    async fn request(
        &self,
        id: i64,
        message: &Value,
        tools_changed: &Notify,
    ) -> Result<Value, ClientError> {
        let response = self.send(message).await?;
        let event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if !event_stream {
            let body = read_capped(response).await?;
            let parsed: Value = serde_json::from_slice(&body)
                .map_err(|_| ClientError("server sent invalid JSON".into()))?;
            let messages = match parsed {
                Value::Array(items) => items,
                other => vec![other],
            };
            return messages
                .into_iter()
                .find_map(|message| self.answer(id, &message, tools_changed))
                .unwrap_or_else(|| Err(ClientError("server sent no response".into())));
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut data = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ClientError("stream interrupted".into()))?;
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_MESSAGE {
                return Err(ClientError("response too large".into()));
            }
            while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    if data.is_empty() {
                        continue;
                    }
                    let event = std::mem::take(&mut data);
                    if let Ok(message) = serde_json::from_str::<Value>(&event) {
                        if let Some(result) = self.answer(id, &message, tools_changed) {
                            return result;
                        }
                    }
                } else if let Some(value) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(value.strip_prefix(' ').unwrap_or(value));
                    if data.len() > MAX_MESSAGE {
                        return Err(ClientError("response too large".into()));
                    }
                }
            }
        }
        Err(ClientError(
            "server closed the stream without a response".into(),
        ))
    }

    /// The result for `id`, or `None` after handling an unrelated message.
    fn answer(
        &self,
        id: i64,
        message: &Value,
        tools_changed: &Notify,
    ) -> Option<Result<Value, ClientError>> {
        if message.get("method").is_some() {
            if let Some(reply) = reply_to(message) {
                let request = self.post(&reply);
                tokio::spawn(async move {
                    let _ = request.send().await;
                });
            } else if message["method"] == "notifications/tools/list_changed" {
                tools_changed.notify_one();
            }
            return None;
        }
        if message.get("id").and_then(Value::as_i64) != Some(id) {
            return None;
        }
        Some(match message.get("error") {
            Some(error) => Err(rpc_error(error)),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        })
    }
}

async fn read_capped(response: reqwest::Response) -> Result<Vec<u8>, ClientError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ClientError("response interrupted".into()))?;
        body.extend_from_slice(&chunk);
        if body.len() > MAX_MESSAGE {
            return Err(ClientError("response too large".into()));
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::checked_peer;

    #[tokio::test]
    async fn only_this_computer_or_public_hosts_are_reachable() {
        for url in [
            "https://169.254.169.254/latest",
            "https://10.0.0.5/mcp",
            "https://192.168.1.2:8443/mcp",
            "https://100.64.0.1/mcp",
            "https://[fd00::1]/mcp",
            "https://[fe80::1]/mcp",
            "https://0.0.0.0/mcp",
        ] {
            assert!(checked_peer(url, false).await.is_err(), "{url}");
        }
        let (host, peer) = checked_peer("http://127.0.0.1:9/mcp", false).await.unwrap();
        assert_eq!((host.as_str(), peer.port()), ("127.0.0.1", 9));
        assert!(checked_peer("https://[::1]:9/mcp", false).await.is_ok());
        assert!(checked_peer("https://1.1.1.1/mcp", false).await.is_ok());
    }

    #[tokio::test]
    async fn private_network_opt_in_never_reaches_link_local_or_metadata() {
        for url in [
            "https://10.0.0.5/mcp",
            "https://172.20.1.1/mcp",
            "https://192.168.1.2:8443/mcp",
            "https://100.101.102.103/mcp",
            "https://[fd7a:115c:a1e0::1]/mcp",
            "https://1.1.1.1/mcp",
            "http://127.0.0.1:9/mcp",
        ] {
            assert!(checked_peer(url, true).await.is_ok(), "{url}");
        }
        for url in [
            "https://169.254.169.254/latest",
            "https://169.254.0.23/",
            "https://100.100.100.200/latest",
            "https://168.63.129.16/",
            "https://[fd00:ec2::254]/latest",
            "https://[fe80::1]/mcp",
            "https://[fec0::1]/mcp",
            "https://192.0.0.1/mcp",
            "https://198.18.0.1/mcp",
            "https://240.0.0.1/mcp",
            "https://0.0.0.0/mcp",
            "https://[::ffff:169.254.169.254]/mcp",
        ] {
            assert!(checked_peer(url, true).await.is_err(), "{url}");
        }
    }
}
