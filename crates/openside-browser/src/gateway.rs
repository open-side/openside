//! Client for the patched browser's AgentGateway (`browser/specs/gateway-protocol.md`).
//!
//! The gateway is an authenticated, same-UID unix socket published by a patched Chromium
//! started with `--openside-gateway-instance-id`. It reports browser-side tab ownership,
//! focus decisions, hidden-tab screenshots, and popup/dialog/download events. A stock
//! Chromium ignores the switches and never publishes discovery, so callers fall back to
//! CDP only. The gateway exists only on Linux.
//!
//! Resource bounds: requests are written by one writer task in whole frames, so a cancelled
//! request never leaves a torn frame on the socket, and one deadline covers queueing, sending,
//! and the response. Pushed events go to a bounded buffer of [`EVENT_BUFFER`] events; when the
//! consumer falls behind, the oldest unread events are discarded and [`Gateway::next_event`]
//! reports [`GatewayEventError::Lagged`] before delivering newer ones. Requests and results
//! never depend on the event consumer; a lagged consumer resynchronizes with
//! [`Gateway::tabs`] (`through_event_seq`).

use std::collections::HashMap;
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use base64::Engine;
#[cfg(target_os = "linux")]
use nix::fcntl::OFlag;
use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(target_os = "linux")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(target_os = "linux")]
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
#[cfg(target_os = "linux")]
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::BrowserError;

#[cfg(target_os = "linux")]
const PROTOCOL: &str = "openside.browser";
#[cfg(target_os = "linux")]
const DISCOVERY_FILE: &str = "gateway.json";
#[cfg(target_os = "linux")]
const SOCKET_FILE: &str = "gateway.sock";
#[cfg(target_os = "linux")]
const TOKEN_FILE: &str = "gateway.token";
#[cfg(target_os = "linux")]
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// The browser refuses larger request frames; checked here so callers get a local error.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
#[cfg(target_os = "linux")]
const MAX_DISCOVERY_BYTES: u64 = 4096;
#[cfg(target_os = "linux")]
/// Largest sequence number a JSON peer can represent exactly (2^53 - 1).
const MAX_SEQUENCE: u64 = (1 << 53) - 1;
#[cfg(target_os = "linux")]
/// Whole request frames waiting for the writer task.
const WRITE_QUEUE: usize = 16;
/// Pushed events retained for [`Gateway::next_event`] before the oldest are discarded.
pub const EVENT_BUFFER: usize = 256;

/// Opt-in gateway mode for a launched patched browser; one browser serves one agent scope.
#[derive(Clone, Debug)]
pub struct GatewayConfig {
    /// Agent session that owns every tab the browser creates for automation.
    pub session_id: String,
    /// Subagent within the session; None is the session's root agent.
    pub subagent_id: Option<String>,
    /// Private runtime directory for discovery, passed to the browser as
    /// `--openside-gateway-runtime-dir`; None uses the daemon's `XDG_RUNTIME_DIR`.
    pub runtime_dir: Option<PathBuf>,
    /// How long to wait for discovery after the debugging endpoint is ready. A patched
    /// browser publishes discovery before its debugging endpoint, so a short deadline suffices.
    pub discovery_timeout: Duration,
    /// True fails the launch when the gateway cannot be used; false falls back to CDP only
    /// when no private runtime directory exists or the browser publishes no gateway.
    pub required: bool,
}

impl GatewayConfig {
    /// Required gateway mode for a root agent session with the default discovery deadline.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            subagent_id: None,
            runtime_dir: None,
            discovery_timeout: Duration::from_secs(5),
            required: true,
        }
    }
}

/// Launch-time parameters derived from [`GatewayConfig`].
pub(crate) struct GatewayLaunch {
    pub(crate) runtime_dir: PathBuf,
    pub(crate) instance_id: String,
    pub(crate) session_id: String,
    pub(crate) subagent_id: Option<String>,
}

impl GatewayLaunch {
    #[cfg(not(target_os = "linux"))]
    pub(crate) fn prepare(config: &GatewayConfig) -> Result<Option<Self>, BrowserError> {
        if config.required {
            Err(invalid("the browser gateway is available only on Linux"))
        } else {
            Ok(None)
        }
    }

    #[cfg(target_os = "linux")]
    /// None leaves the browser CDP-only: the platform has no gateway, or an optional gateway
    /// has no private runtime directory.
    pub(crate) fn prepare(config: &GatewayConfig) -> Result<Option<Self>, BrowserError> {
        let runtime_dir = match &config.runtime_dir {
            Some(path) => path.clone(),
            None => match std::env::var_os("XDG_RUNTIME_DIR") {
                Some(path) if !path.is_empty() => PathBuf::from(path),
                _ if config.required => return Err(invalid("XDG_RUNTIME_DIR is not set")),
                _ => return Ok(None),
            },
        };
        // Chromium reads the directory from a switch and refuses a relative one; the
        // browser's own environment stays the user's (native-messaging hosts inherit it).
        if !runtime_dir.is_absolute() {
            return if config.required {
                Err(invalid("gateway runtime directory is not absolute"))
            } else {
                Ok(None)
            };
        }
        // The browser refuses a shared runtime directory; checking here avoids waiting for a
        // discovery file that will never appear.
        let private = std::fs::symlink_metadata(&runtime_dir)
            .map_err(BrowserError::from)
            .and_then(|metadata| {
                if !metadata.is_dir() {
                    return Err(invalid("gateway runtime directory is not a directory"));
                }
                check_private(&metadata, 0o700)
            });
        match private {
            Ok(()) => {}
            Err(error) if config.required => return Err(error),
            Err(_) => return Ok(None),
        }
        let mut random = [0_u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let instance_id = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Some(Self {
            runtime_dir,
            instance_id,
            session_id: config.session_id.clone(),
            subagent_id: config.subagent_id.clone(),
        }))
    }

    /// Switches understood only by the patched browser; stock Chromium ignores them.
    pub(crate) fn args(&self) -> Vec<OsString> {
        let mut runtime_dir = OsString::from("--openside-gateway-runtime-dir=");
        runtime_dir.push(&self.runtime_dir);
        let mut args = vec![
            OsString::from(format!(
                "--openside-gateway-instance-id={}",
                self.instance_id
            )),
            runtime_dir,
            OsString::from(format!("--openside-agent-session-id={}", self.session_id)),
        ];
        if let Some(subagent) = &self.subagent_id {
            args.push(OsString::from(format!(
                "--openside-agent-subagent-id={subagent}"
            )));
        }
        args
    }

    /// Directory the browser creates for this instance.
    pub(crate) fn directory(&self) -> PathBuf {
        self.runtime_dir
            .join("openside")
            .join("browser")
            .join(&self.instance_id)
    }
}

/// Tab control state reported by the browser, not inferred from CDP metadata.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GatewayOwner {
    /// Owned by the browser's agent scope.
    Agent,
    /// Taken over by the user.
    User,
    /// Neither created by the agent nor claimed by the user.
    External,
}

/// Browser-side tab record from `tabs.list` and tab events.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GatewayTab {
    /// Stable gateway tab identity for the life of the tab.
    pub tab_id: String,
    /// Matching CDP target, when one exists.
    pub cdp_target_id: Option<String>,
    /// Current controller.
    pub owner: GatewayOwner,
    /// Creating agent session.
    pub session_id: Option<String>,
    /// Creating subagent; None is the session's root agent.
    pub subagent_id: Option<String>,
    /// Display label; not an authorization input.
    pub label: String,
    /// Whether the tab was created and kept in the background.
    pub background: bool,
    /// Immutable creation origin.
    pub opened_by: GatewayOwner,
    /// Untrusted page URL.
    pub url: String,
    /// Untrusted page title.
    pub title: String,
    /// `live`, `asleep`, or `closed`.
    pub state: String,
    /// Title of the tab group holding the tab, when grouped.
    #[serde(default)]
    pub group_title: Option<String>,
}

/// Atomic tab sample and the last event it reflects.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct GatewayTabs {
    /// Every tab known to the browser.
    pub tabs: Vec<GatewayTab>,
    /// Events up to this sequence number are reflected in `tabs`.
    pub through_event_seq: u64,
}

/// A focus decision; denial is a normal result, not an error.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct FocusDecision {
    /// Whether the tab may take focus.
    pub allowed: bool,
    /// Fixed reason code.
    pub reason: String,
    /// Who holds focus afterwards.
    pub owner: String,
}

/// A hidden-tab capture taken without activating the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayScreenshot {
    /// Captured width in pixels.
    pub width: u32,
    /// Captured height in pixels.
    pub height: u32,
    /// PNG bytes.
    pub png: Vec<u8>,
}

/// One pushed browser event in connection order.
#[derive(Clone, Debug, PartialEq)]
pub struct GatewayEvent {
    /// Per-connection sequence starting at 1.
    pub seq: u64,
    /// Event type such as `tab.opened` or `dialog.opened`.
    pub kind: String,
    /// Gateway tab the event concerns.
    pub tab_id: Option<String>,
    /// Agent session the event is attributed to.
    pub session_id: Option<String>,
    /// Subagent the event is attributed to.
    pub subagent_id: Option<String>,
    /// Type-specific payload; page text is redacted by the browser.
    pub payload: Value,
}

/// Why [`Gateway::next_event`] returned no event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GatewayEventError {
    /// The consumer fell behind by more than [`EVENT_BUFFER`] events; this many of the oldest
    /// unread events were discarded. Later calls continue with newer events.
    #[error("gateway event consumer lagged; {missed} events were discarded")]
    Lagged {
        /// Number of discarded events.
        missed: u64,
    },
    /// The gateway connection closed.
    #[error("gateway connection closed")]
    Closed,
}

type Responder = oneshot::Sender<Result<Value, BrowserError>>;

/// Connection state shared by the request path and the reader and writer tasks.
#[derive(Default)]
struct Shared {
    pending: std::sync::Mutex<HashMap<String, Responder>>,
    closed: AtomicBool,
    #[cfg(target_os = "linux")]
    /// Wakes the writer so it drops the write half and the peer sees the close.
    shutdown: tokio::sync::Notify,
}

impl Shared {
    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Responder>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(&self, id: &str, responder: Responder) -> Result<(), BrowserError> {
        let mut pending = self.pending();
        if self.closed.load(Ordering::Acquire) {
            return Err(BrowserError::Disconnected);
        }
        pending.insert(id.to_owned(), responder);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    /// Fails every pending request; dropped responders surface as `Disconnected`.
    fn close(&self) {
        let mut pending = self.pending();
        self.closed.store(true, Ordering::Release);
        pending.clear();
        self.shutdown.notify_one();
    }
}

/// Removes a request's pending entry on every exit path, including cancellation.
struct PendingEntry<'a> {
    shared: &'a Shared,
    id: String,
}

impl Drop for PendingEntry<'_> {
    fn drop(&mut self) {
        self.shared.pending().remove(&self.id);
    }
}

/// An authenticated gateway connection.
pub struct Gateway {
    instance_id: String,
    session_id: String,
    subagent_id: Option<String>,
    capabilities: Vec<String>,
    shared: Arc<Shared>,
    frames: mpsc::Sender<Vec<u8>>,
    next_id: AtomicU64,
    events: Mutex<broadcast::Receiver<GatewayEvent>>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    command_timeout: Duration,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Gateway")
            .field("instance_id", &self.instance_id)
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

#[cfg(target_os = "linux")]
#[derive(Deserialize)]
struct Discovery {
    protocol: String,
    versions: Vec<u64>,
    instance_id: String,
    socket: String,
    token_file: String,
}

fn invalid(reason: &'static str) -> BrowserError {
    BrowserError::InvalidEndpoint(reason)
}

#[cfg(target_os = "linux")]
/// The directory and files are trusted only if the current user alone can reach them. Special
/// bits count too, matching the browser's own check of the runtime directory.
fn check_private(metadata: &std::fs::Metadata, mode: u32) -> Result<(), BrowserError> {
    if metadata.uid() != nix::unistd::geteuid().as_raw() || metadata.mode() & 0o7777 != mode {
        return Err(invalid("gateway files are not private to this user"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_symlink_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(nix::errno::Errno::ELOOP as i32)
}

#[cfg(target_os = "linux")]
/// The verified instance directory, held open so every later lookup resolves inside the
/// directory that passed the checks even if its path is replaced afterwards.
struct PinnedDirectory {
    directory: std::fs::File,
}

#[cfg(target_os = "linux")]
impl PinnedDirectory {
    /// NotFound is passed through so discovery can wait for the browser to create it.
    fn open(path: &Path) -> Result<Self, BrowserError> {
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
            .open(path)
            .map_err(|error| {
                if is_symlink_error(&error) {
                    invalid("gateway directory is a symlink")
                } else {
                    error.into()
                }
            })?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() {
            return Err(invalid("gateway directory is not a directory"));
        }
        check_private(&metadata, 0o700)?;
        Ok(Self { directory })
    }

    fn join(&self, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd())).join(name)
    }

    /// Reads a private regular file without following a symlink or blocking on a FIFO.
    fn read_private(&self, name: &str, limit: u64) -> Result<String, BrowserError> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(self.join(name))
            .map_err(|error| {
                if is_symlink_error(&error) {
                    invalid("gateway file is a symlink")
                } else {
                    error.into()
                }
            })?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("gateway file is not a regular file"));
        }
        check_private(&metadata, 0o600)?;
        let mut contents = String::new();
        file.by_ref()
            .take(limit + 1)
            .read_to_string(&mut contents)
            .map_err(|_| invalid("gateway file is not UTF-8"))?;
        if contents.len() as u64 > limit {
            return Err(invalid("gateway file is too large"));
        }
        Ok(contents)
    }

    fn discovery(&self, instance_id: &str) -> Result<Discovery, BrowserError> {
        let contents = self.read_private(DISCOVERY_FILE, MAX_DISCOVERY_BYTES)?;
        let discovery: Discovery =
            serde_json::from_str(&contents).map_err(|_| invalid("malformed gateway discovery"))?;
        // Only the fixed relative names are accepted so discovery cannot redirect the client.
        if discovery.protocol != PROTOCOL
            || !discovery.versions.contains(&1)
            || discovery.instance_id != instance_id
            || discovery.socket != SOCKET_FILE
            || discovery.token_file != TOKEN_FILE
        {
            return Err(invalid("gateway discovery does not match this browser"));
        }
        Ok(discovery)
    }

    fn token(&self) -> Result<String, BrowserError> {
        let contents = self.read_private(TOKEN_FILE, 65)?;
        let token = contents
            .strip_suffix('\n')
            .ok_or(invalid("malformed gateway token"))?;
        if token.len() != 64
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("malformed gateway token"));
        }
        Ok(token.to_owned())
    }

    /// Path of a verified private socket inside the pinned directory.
    fn socket(&self) -> Result<PathBuf, BrowserError> {
        let path = self.join(SOCKET_FILE);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket() {
            return Err(invalid("gateway socket is not a socket"));
        }
        check_private(&metadata, 0o600)?;
        Ok(path)
    }
}

fn encode_frame(message: &Value, limit: usize) -> Result<Vec<u8>, BrowserError> {
    let body =
        serde_json::to_vec(message).map_err(|error| BrowserError::Transport(error.to_string()))?;
    let length = u32::try_from(body.len())
        .ok()
        .filter(|length| *length as usize <= limit)
        .ok_or(BrowserError::InvalidAction("gateway frame too large"))?;
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

#[cfg(target_os = "linux")]
async fn read_frame(reader: &mut OwnedReadHalf) -> Result<Value, BrowserError> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            BrowserError::Disconnected
        } else {
            error.into()
        }
    })?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(BrowserError::InvalidResponse("gateway frame length"));
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(|_| BrowserError::InvalidResponse("gateway frame JSON"))
}

#[cfg(target_os = "linux")]
fn gateway_error(message: &Value) -> BrowserError {
    BrowserError::Gateway {
        code: message["code"].as_str().unwrap_or("unknown").to_owned(),
        message: message["message"].as_str().unwrap_or_default().to_owned(),
    }
}

#[cfg(target_os = "linux")]
fn optional_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

#[cfg(target_os = "linux")]
/// A field that must be absent, null, or a string; Err for any other type.
fn nullable_string(message: &Value, field: &str) -> Result<Option<String>, ()> {
    match message.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

#[cfg(target_os = "linux")]
/// Validates an event envelope; sequences must advance by exactly one per connection.
fn parse_event(message: &Value, instance_id: &str, last_seq: u64) -> Option<GatewayEvent> {
    let seq = message["event_seq"].as_u64()?;
    if seq != last_seq + 1 || seq > MAX_SEQUENCE || message["instance_id"] != instance_id {
        return None;
    }
    let payload = message
        .get("payload")
        .filter(|payload| payload.is_object())?;
    Some(GatewayEvent {
        seq,
        kind: message["type"].as_str()?.to_owned(),
        tab_id: nullable_string(message, "tab_id").ok()?,
        session_id: nullable_string(message, "session_id").ok()?,
        subagent_id: nullable_string(message, "subagent_id").ok()?,
        payload: payload.clone(),
    })
}

impl Gateway {
    #[cfg(not(target_os = "linux"))]
    pub(crate) async fn discover(
        _launch: &GatewayLaunch,
        _timeout: Duration,
        _command_timeout: Duration,
    ) -> Result<Self, BrowserError> {
        Err(invalid("the browser gateway is available only on Linux"))
    }

    #[cfg(target_os = "linux")]
    /// Wait for the instance's discovery file, verify its private files, and authenticate.
    pub(crate) async fn discover(
        launch: &GatewayLaunch,
        timeout: Duration,
        command_timeout: Duration,
    ) -> Result<Self, BrowserError> {
        let directory = launch.directory();
        let (pinned, discovery) = tokio::time::timeout(timeout, async {
            loop {
                let found = PinnedDirectory::open(&directory).and_then(|pinned| {
                    let discovery = pinned.discovery(&launch.instance_id)?;
                    Ok((pinned, discovery))
                });
                match found {
                    Ok(found) => return Ok(found),
                    Err(BrowserError::Io(error))
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        })
        .await
        .map_err(|_| BrowserError::Timeout("gateway discovery"))??;
        let token = pinned.token()?;
        let socket = pinned.socket()?;
        Self::connect(
            &socket,
            &discovery.instance_id,
            &token,
            launch.session_id.clone(),
            launch.subagent_id.clone(),
            command_timeout,
        )
        .await
    }

    #[cfg(target_os = "linux")]
    /// Authenticate over an already discovered socket.
    async fn connect(
        socket: &Path,
        instance_id: &str,
        token: &str,
        session_id: String,
        subagent_id: Option<String>,
        command_timeout: Duration,
    ) -> Result<Self, BrowserError> {
        let hello = encode_frame(
            &json!({"kind":"hello","protocol":PROTOCOL,"versions":[1],
                "instance_id":instance_id,"token":token}),
            MAX_REQUEST_BYTES,
        )?;
        let (reader, writer, welcome) = tokio::time::timeout(command_timeout, async {
            let stream = UnixStream::connect(socket).await?;
            if stream.peer_cred()?.uid() != nix::unistd::geteuid().as_raw() {
                return Err(invalid("gateway peer belongs to another user"));
            }
            let (mut reader, mut writer) = stream.into_split();
            writer.write_all(&hello).await?;
            let welcome = read_frame(&mut reader).await?;
            Ok((reader, writer, welcome))
        })
        .await
        .map_err(|_| BrowserError::Timeout("gateway handshake"))??;
        if welcome["kind"] == "error" {
            return Err(gateway_error(&welcome));
        }
        if welcome["kind"] != "welcome"
            || welcome["v"] != 1
            || welcome["instance_id"] != instance_id
        {
            return Err(BrowserError::InvalidResponse("gateway welcome"));
        }
        let capabilities = welcome["capabilities"]
            .as_array()
            .map(|values| values.iter().filter_map(optional_string).collect())
            .unwrap_or_default();
        let shared = Arc::<Shared>::default();
        let (events, receiver) = broadcast::channel(EVENT_BUFFER);
        let (frames, queue) = mpsc::channel(WRITE_QUEUE);
        let reader = tokio::spawn(read_loop(
            reader,
            Arc::clone(&shared),
            events,
            instance_id.to_owned(),
        ));
        let writer = tokio::spawn(write_loop(
            writer,
            queue,
            Arc::clone(&shared),
            command_timeout,
        ));
        Ok(Self {
            instance_id: instance_id.to_owned(),
            session_id,
            subagent_id,
            capabilities,
            shared,
            frames,
            next_id: AtomicU64::new(1),
            events: Mutex::new(receiver),
            reader,
            writer,
            command_timeout,
        })
    }

    /// Browser instance identity from discovery and `welcome`.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Capabilities the browser actually implements.
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Send one request in this browser's agent scope and wait for its result. One deadline
    /// covers queueing, sending, and the response; dropping the future at any point leaves
    /// no pending entry and never a partial frame.
    pub async fn request(&self, kind: &str, payload: Value) -> Result<Value, BrowserError> {
        let deadline = Instant::now() + self.command_timeout;
        let id = format!("req-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let frame = encode_frame(
            &json!({"v":1,"kind":"request","type":kind,"id":id,
                "session_id":self.session_id,"subagent_id":self.subagent_id,"payload":payload}),
            MAX_REQUEST_BYTES,
        )?;
        let (responder, response) = oneshot::channel();
        self.shared.register(&id, responder)?;
        let _pending = PendingEntry {
            shared: &self.shared,
            id,
        };
        tokio::time::timeout_at(deadline, async {
            self.frames
                .send(frame)
                .await
                .map_err(|_| BrowserError::Disconnected)?;
            response.await.map_err(|_| BrowserError::Disconnected)?
        })
        .await
        .map_err(|_| BrowserError::Timeout("gateway request"))?
    }

    /// Atomic sample of browser-side tab records.
    pub async fn tabs(&self) -> Result<GatewayTabs, BrowserError> {
        let result = self.request("tabs.list", json!({})).await?;
        serde_json::from_value(result).map_err(|_| BrowserError::InvalidResponse("gateway tabs"))
    }

    /// Gateway record for a CDP target, if the browser knows it.
    pub async fn tab_for_target(
        &self,
        target_id: &str,
    ) -> Result<Option<GatewayTab>, BrowserError> {
        Ok(self
            .tabs()
            .await?
            .tabs
            .into_iter()
            .find(|tab| tab.cdp_target_id.as_deref() == Some(target_id)))
    }

    /// Fill through the browser's password-autofill IPC; the secret never enters CDP.
    pub async fn fill_secret_node(
        &self,
        target_id: &str,
        backend_node_id: u64,
        origin: &str,
        value: &str,
    ) -> Result<(), BrowserError> {
        let tab = self
            .tab_for_target(target_id)
            .await?
            .ok_or(BrowserError::InvalidResponse("gateway tab is missing"))?;
        let fields = self
            .request(
                "autofill.fields",
                json!({"tab_id":tab.tab_id,"backend_node_id":backend_node_id}),
            )
            .await?;
        let field = fields["fields"]
            .as_array()
            .and_then(|fields| {
                fields.iter().find(|field| {
                    field["backend_node_id"].as_u64() == Some(backend_node_id)
                        && field["origin"].as_str() == Some(origin)
                })
            })
            .ok_or(BrowserError::StaleReference)?;
        let handle = field["handle"]
            .as_str()
            .ok_or(BrowserError::InvalidResponse("gateway autofill handle"))?;
        let result = self
            .request(
                "autofill.fill",
                json!({"tab_id":tab.tab_id,"handle":handle,"origin":origin,"value":value}),
            )
            .await?;
        if result["filled"] == true {
            Ok(())
        } else {
            Err(BrowserError::TextInsertionFailed)
        }
    }

    /// Ask the browser's focus arbiter whether an agent tab may take focus.
    pub async fn request_focus(&self, tab_id: &str) -> Result<FocusDecision, BrowserError> {
        let result = self
            .request("focus.request", json!({"tab_id":tab_id,"grant_id":null}))
            .await?;
        serde_json::from_value(result).map_err(|_| BrowserError::InvalidResponse("focus decision"))
    }

    /// Capture an owned tab without activating or showing it.
    pub async fn screenshot(&self, tab_id: &str) -> Result<GatewayScreenshot, BrowserError> {
        let result = self
            .request(
                "screenshot.capture",
                json!({"tab_id":tab_id,"format":"png"}),
            )
            .await?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(result["data_base64"].as_str().unwrap_or_default())
            .map_err(|_| BrowserError::InvalidResponse("screenshot data"))?;
        let dimension = |field: &'static str| {
            result[field]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(BrowserError::InvalidResponse(field))
        };
        Ok(GatewayScreenshot {
            width: dimension("width")?,
            height: dimension("height")?,
            png,
        })
    }

    /// Change the display label of an owned tab.
    pub async fn set_label(&self, tab_id: &str, label: &str) -> Result<GatewayTab, BrowserError> {
        let result = self
            .request("tab.set_label", json!({"tab_id":tab_id,"label":label}))
            .await?;
        serde_json::from_value(result["tab"].clone())
            .map_err(|_| BrowserError::InvalidResponse("gateway tab"))
    }

    /// Next pushed event in connection order; see the module notes for overflow behavior.
    pub async fn next_event(&self) -> Result<GatewayEvent, GatewayEventError> {
        match self.events.lock().await.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                Err(GatewayEventError::Lagged { missed })
            }
            Err(broadcast::error::RecvError::Closed) => Err(GatewayEventError::Closed),
        }
    }
}

#[cfg(target_os = "linux")]
/// Owns the write half. A frame is either written completely or the connection is
/// abandoned, so the browser never sees a torn frame followed by another one.
async fn write_loop(
    mut writer: OwnedWriteHalf,
    mut frames: mpsc::Receiver<Vec<u8>>,
    shared: Arc<Shared>,
    write_timeout: Duration,
) {
    loop {
        let frame = tokio::select! {
            frame = frames.recv() => frame,
            () = shared.shutdown.notified() => None,
        };
        let Some(frame) = frame else { break };
        match tokio::time::timeout(write_timeout, writer.write_all(&frame)).await {
            Ok(Ok(())) => {}
            _ => break,
        }
    }
    shared.close();
    // Dropping the write half shuts the socket down; the browser then closes its side.
}

#[cfg(target_os = "linux")]
async fn read_loop(
    mut reader: OwnedReadHalf,
    shared: Arc<Shared>,
    events: broadcast::Sender<GatewayEvent>,
    instance_id: String,
) {
    let mut last_seq = 0;
    while let Ok(message) = read_frame(&mut reader).await {
        if message["v"] != 1 {
            break;
        }
        match message["kind"].as_str() {
            Some("event") => {
                let Some(event) = parse_event(&message, &instance_id, last_seq) else {
                    break;
                };
                last_seq = event.seq;
                // Fails only without receivers, which cannot happen while the Gateway lives.
                let _ = events.send(event);
            }
            Some(kind @ ("result" | "error")) => {
                // A connection-level error has no id and ends the connection.
                let (Some(id), Some(_)) = (message["id"].as_str(), message["type"].as_str()) else {
                    break;
                };
                let response = if kind == "result" {
                    match message.get("payload").filter(|payload| payload.is_object()) {
                        Some(payload) => Ok(payload.clone()),
                        None => break,
                    }
                } else {
                    Err(gateway_error(&message))
                };
                let responder = shared.pending().remove(id);
                if let Some(responder) = responder {
                    let _ = responder.send(response);
                }
            }
            _ => break,
        }
    }
    shared.close();
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tokio::net::UnixListener;

    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Fixture {
        root: tempfile::TempDir,
        launch: GatewayLaunch,
    }

    fn private(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn fixture(subagent_id: Option<&str>) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let launch = GatewayLaunch {
            runtime_dir: root.path().to_owned(),
            instance_id: "0123456789abcdef".to_owned(),
            session_id: "session-a".to_owned(),
            subagent_id: subagent_id.map(str::to_owned),
        };
        let directory = launch.directory();
        std::fs::create_dir_all(&directory).unwrap();
        private(&directory, 0o700);
        let token = directory.join(TOKEN_FILE);
        std::fs::write(&token, format!("{TOKEN}\n")).unwrap();
        private(&token, 0o600);
        Fixture { root, launch }
    }

    fn publish(launch: &GatewayLaunch) {
        let path = launch.directory().join(DISCOVERY_FILE);
        std::fs::write(
            &path,
            json!({"protocol":PROTOCOL,"versions":[1],"instance_id":launch.instance_id,
                "pid":1,"socket":SOCKET_FILE,"token_file":TOKEN_FILE})
            .to_string(),
        )
        .unwrap();
        private(&path, 0o600);
    }

    fn listen(launch: &GatewayLaunch) -> UnixListener {
        let path = launch.directory().join(SOCKET_FILE);
        let listener = UnixListener::bind(&path).unwrap();
        private(&path, 0o600);
        publish(launch);
        listener
    }

    async fn server_frame(stream: &mut UnixStream) -> Value {
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn server_send(stream: &mut UnixStream, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        stream
            .write_all(&u32::try_from(body.len()).unwrap().to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&body).await.unwrap();
    }

    /// Accepts the client and answers its hello.
    async fn accept(listener: &UnixListener, instance: &str) -> UnixStream {
        let (mut stream, _) = listener.accept().await.unwrap();
        let hello = server_frame(&mut stream).await;
        assert_eq!(hello["token"], TOKEN);
        assert_eq!(hello["instance_id"], instance);
        server_send(
            &mut stream,
            json!({"kind":"welcome","v":1,"instance_id":instance,"capabilities":["focus"]}),
        )
        .await;
        stream
    }

    fn event(instance: &str, seq: u64) -> Value {
        json!({"v":1,"kind":"event","type":"tab.updated","event_seq":seq,
            "instance_id":instance,"tab_id":"tab-1","session_id":"session-a",
            "subagent_id":null,"payload":{"n":seq}})
    }

    async fn connect(fixture: &Fixture, command_timeout: Duration) -> Gateway {
        Gateway::discover(&fixture.launch, Duration::from_secs(2), command_timeout)
            .await
            .unwrap()
    }

    #[test]
    fn prepare_requires_a_private_runtime_dir_and_builds_switches() {
        let runtime = tempfile::tempdir().unwrap();
        private(runtime.path(), 0o755);
        let mut config = GatewayConfig::new("session-a");
        config.runtime_dir = Some(runtime.path().to_owned());
        assert!(matches!(
            GatewayLaunch::prepare(&config),
            Err(BrowserError::InvalidEndpoint(_))
        ));
        // An optional gateway leaves the browser CDP-only instead of failing the launch.
        config.required = false;
        assert!(GatewayLaunch::prepare(&config).unwrap().is_none());
        private(runtime.path(), 0o700);
        config.subagent_id = Some("child".to_owned());
        let launch = GatewayLaunch::prepare(&config).unwrap().unwrap();
        assert_eq!(launch.instance_id.len(), 32);
        assert!(launch
            .instance_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        assert_eq!(
            launch.args(),
            [
                format!("--openside-gateway-instance-id={}", launch.instance_id),
                format!(
                    "--openside-gateway-runtime-dir={}",
                    runtime.path().display()
                ),
                "--openside-agent-session-id=session-a".to_owned(),
                "--openside-agent-subagent-id=child".to_owned(),
            ]
            .map(OsString::from)
        );
        // A relative directory would resolve against the browser's working directory.
        config.runtime_dir = Some(PathBuf::from("relative/runtime"));
        assert!(GatewayLaunch::prepare(&config).unwrap().is_none());
        config.required = true;
        assert!(matches!(
            GatewayLaunch::prepare(&config),
            Err(BrowserError::InvalidEndpoint(_))
        ));
        assert!(launch
            .directory()
            .ends_with(format!("openside/browser/{}", launch.instance_id)));
    }

    #[tokio::test]
    async fn discovery_rejects_shared_special_and_symlinked_files() {
        let discover = |launch: &GatewayLaunch| {
            let launch = GatewayLaunch {
                runtime_dir: launch.runtime_dir.clone(),
                instance_id: launch.instance_id.clone(),
                session_id: launch.session_id.clone(),
                subagent_id: None,
            };
            async move {
                Gateway::discover(&launch, Duration::from_secs(1), Duration::from_secs(1)).await
            }
        };
        let fixture = fixture(None);
        let _listener = listen(&fixture.launch);
        let directory = fixture.launch.directory();
        let token = directory.join(TOKEN_FILE);

        private(&token, 0o644);
        let refused = discover(&fixture.launch).await;
        assert!(
            matches!(refused, Err(BrowserError::InvalidEndpoint(_))),
            "{refused:?}"
        );
        private(&token, 0o600);

        let real = fixture.root.path().join("elsewhere");
        std::fs::rename(&token, &real).unwrap();
        std::os::unix::fs::symlink(&real, &token).unwrap();
        let refused = discover(&fixture.launch).await;
        assert!(
            matches!(refused, Err(BrowserError::InvalidEndpoint(_))),
            "{refused:?}"
        );
        std::fs::remove_file(&token).unwrap();

        // A FIFO in place of the token must be refused without blocking the open.
        nix::unistd::mkfifo(&token, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        let refused = discover(&fixture.launch).await;
        assert!(
            matches!(refused, Err(BrowserError::InvalidEndpoint(_))),
            "{refused:?}"
        );
        std::fs::remove_file(&token).unwrap();
        std::fs::rename(&real, &token).unwrap();

        let discovery = directory.join(DISCOVERY_FILE);
        private(&discovery, 0o644);
        let refused = discover(&fixture.launch).await;
        assert!(
            matches!(refused, Err(BrowserError::InvalidEndpoint(_))),
            "{refused:?}"
        );
        private(&discovery, 0o600);

        let socket = directory.join(SOCKET_FILE);
        private(&socket, 0o666);
        let refused = discover(&fixture.launch).await;
        assert!(
            matches!(refused, Err(BrowserError::InvalidEndpoint(_))),
            "{refused:?}"
        );
        private(&socket, 0o600);

        private(&directory, 0o755);
        let refused = discover(&fixture.launch).await;
        assert!(
            matches!(refused, Err(BrowserError::InvalidEndpoint(_))),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn missing_discovery_times_out_so_callers_fall_back_to_cdp() {
        let fixture = fixture(None);
        assert!(matches!(
            Gateway::discover(
                &fixture.launch,
                Duration::from_millis(100),
                Duration::from_secs(1)
            )
            .await,
            Err(BrowserError::Timeout("gateway discovery"))
        ));
    }

    #[tokio::test]
    async fn authenticates_correlates_results_and_streams_events() {
        let fixture = fixture(None);
        let listener = listen(&fixture.launch);
        let instance = fixture.launch.instance_id.clone();
        let server = tokio::spawn(async move {
            let mut stream = accept(&listener, &instance).await;
            let request = server_frame(&mut stream).await;
            assert_eq!(request["type"], "focus.request");
            assert_eq!(request["session_id"], "session-a");
            assert_eq!(request["subagent_id"], Value::Null);
            assert_eq!(
                request["payload"],
                json!({"tab_id":"tab-1","grant_id":null})
            );
            server_send(
                &mut stream,
                json!({"v":1,"kind":"event","type":"focus.decided","event_seq":1,
                    "instance_id":instance,"tab_id":"tab-1","session_id":"session-a",
                    "subagent_id":null,"payload":{"allowed":false}}),
            )
            .await;
            server_send(
                &mut stream,
                json!({"v":1,"kind":"result","type":"focus.request","id":request["id"],
                    "payload":{"allowed":false,"reason":"no_user_grant","owner":"user"}}),
            )
            .await;
            let request = server_frame(&mut stream).await;
            server_send(
                &mut stream,
                json!({"v":1,"kind":"error","type":"screenshot.capture","id":request["id"],
                    "code":"tab_asleep","message":"tab is asleep","details":{}}),
            )
            .await;
        });
        let gateway = connect(&fixture, Duration::from_secs(2)).await;
        assert_eq!(gateway.capabilities(), ["focus"]);
        let decision = gateway.request_focus("tab-1").await.unwrap();
        assert_eq!(
            decision,
            FocusDecision {
                allowed: false,
                reason: "no_user_grant".to_owned(),
                owner: "user".to_owned()
            }
        );
        let event = gateway.next_event().await.unwrap();
        assert_eq!((event.seq, event.kind.as_str()), (1, "focus.decided"));
        assert_eq!(event.tab_id.as_deref(), Some("tab-1"));
        assert!(matches!(
            gateway.screenshot("tab-1").await,
            Err(BrowserError::Gateway { code, .. }) if code == "tab_asleep"
        ));
        server.await.unwrap();
        assert_eq!(gateway.next_event().await, Err(GatewayEventError::Closed));
    }

    #[tokio::test]
    async fn subagent_scope_is_sent_on_every_request() {
        let fixture = fixture(Some("child-1"));
        let listener = listen(&fixture.launch);
        let instance = fixture.launch.instance_id.clone();
        let server = tokio::spawn(async move {
            let mut stream = accept(&listener, &instance).await;
            for _ in 0..2 {
                let request = server_frame(&mut stream).await;
                assert_eq!(
                    (&request["session_id"], &request["subagent_id"]),
                    (&json!("session-a"), &json!("child-1"))
                );
                server_send(
                    &mut stream,
                    json!({"v":1,"kind":"result","type":request["type"],"id":request["id"],
                        "payload":{"tabs":[],"through_event_seq":0}}),
                )
                .await;
            }
        });
        let gateway = connect(&fixture, Duration::from_secs(2)).await;
        assert!(gateway.tabs().await.unwrap().tabs.is_empty());
        assert!(gateway.tabs().await.unwrap().tabs.is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_errors_surface_without_echoing_the_token() {
        let fixture = fixture(None);
        let listener = listen(&fixture.launch);
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            server_frame(&mut stream).await;
            server_send(
                &mut stream,
                json!({"v":1,"kind":"error","type":null,"id":null,"code":"auth_failed",
                    "message":"authentication failed","details":{}}),
            )
            .await;
        });
        let error = Gateway::discover(
            &fixture.launch,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(matches!(&error, BrowserError::Gateway { code, .. } if code == "auth_failed"));
        assert!(!error.to_string().contains(TOKEN));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn undrained_events_stay_bounded_and_report_the_lag() {
        let fixture = fixture(None);
        let listener = listen(&fixture.launch);
        let instance = fixture.launch.instance_id.clone();
        let total = EVENT_BUFFER as u64 * 4;
        let server = tokio::spawn(async move {
            let mut stream = accept(&listener, &instance).await;
            for seq in 1..=total {
                server_send(&mut stream, event(&instance, seq)).await;
            }
            // Results are answered after the burst, so they queue behind every event.
            let request = server_frame(&mut stream).await;
            server_send(
                &mut stream,
                json!({"v":1,"kind":"result","type":"tabs.list","id":request["id"],
                    "payload":{"tabs":[],"through_event_seq":total}}),
            )
            .await;
            stream
        });
        let gateway = connect(&fixture, Duration::from_secs(5)).await;
        // Nobody consumes events while the browser pushes four buffers' worth.
        let tabs = gateway.tabs().await.unwrap();
        assert_eq!(tabs.through_event_seq, total);
        let missed = total - EVENT_BUFFER as u64;
        assert_eq!(
            gateway.next_event().await,
            Err(GatewayEventError::Lagged { missed })
        );
        // Only the newest EVENT_BUFFER events were retained, in order.
        for seq in missed + 1..=total {
            assert_eq!(gateway.next_event().await.unwrap().seq, seq);
        }
        drop(server.await.unwrap());
        assert_eq!(gateway.next_event().await, Err(GatewayEventError::Closed));
    }

    #[tokio::test]
    async fn malformed_event_sequences_close_the_connection() {
        let fixture = fixture(None);
        let listener = listen(&fixture.launch);
        let instance = fixture.launch.instance_id.clone();
        let server = tokio::spawn(async move {
            let mut stream = accept(&listener, &instance).await;
            server_send(&mut stream, event(&instance, 1)).await;
            // A gap means an event was lost; the ledger can no longer be trusted.
            server_send(&mut stream, event(&instance, 3)).await;
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest).await;
        });
        let gateway = connect(&fixture, Duration::from_secs(2)).await;
        assert_eq!(gateway.next_event().await.unwrap().seq, 1);
        assert_eq!(gateway.next_event().await, Err(GatewayEventError::Closed));
        assert!(matches!(
            gateway.tabs().await,
            Err(BrowserError::Disconnected)
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_stalled_peer_times_out_requests_within_one_deadline() {
        let fixture = fixture(None);
        let listener = listen(&fixture.launch);
        let instance = fixture.launch.instance_id.clone();
        let (release, released) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let stream = accept(&listener, &instance).await;
            // Never reads again until released, so the socket buffer fills up.
            let _ = released.await;
            drop(stream);
        });
        let timeout = Duration::from_millis(500);
        let gateway = connect(&fixture, timeout).await;
        // Larger than the socket buffer, so the write itself stalls.
        let label = "x".repeat(900 * 1024);
        let started = std::time::Instant::now();
        let stalled = gateway.set_label("tab-1", &label).await;
        assert!(
            matches!(
                stalled,
                Err(BrowserError::Timeout("gateway request") | BrowserError::Disconnected)
            ),
            "{stalled:?}"
        );
        assert!(started.elapsed() < timeout * 3, "{:?}", started.elapsed());
        // The writer abandoned the wedged connection; later requests fail promptly.
        let started = std::time::Instant::now();
        assert!(matches!(
            gateway.tabs().await,
            Err(BrowserError::Disconnected)
        ));
        assert!(started.elapsed() < timeout * 2, "{:?}", started.elapsed());
        assert!(gateway.shared.pending().is_empty());
        let _ = release.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_requests_leave_no_pending_entry_or_torn_frame() {
        let fixture = fixture(None);
        let listener = listen(&fixture.launch);
        let instance = fixture.launch.instance_id.clone();
        let (seen, mut first) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let mut stream = accept(&listener, &instance).await;
            // The cancelled request still arrives whole; it is never answered.
            let cancelled = server_frame(&mut stream).await;
            seen.send(cancelled["id"].clone()).unwrap();
            let request = server_frame(&mut stream).await;
            assert_eq!(request["type"], "tabs.list");
            assert_ne!(request["id"], cancelled["id"]);
            server_send(
                &mut stream,
                json!({"v":1,"kind":"result","type":"tabs.list","id":request["id"],
                    "payload":{"tabs":[],"through_event_seq":0}}),
            )
            .await;
        });
        let gateway = connect(&fixture, Duration::from_secs(5)).await;
        let label = "y".repeat(256 * 1024);
        tokio::select! {
            _ = gateway.set_label("tab-1", &label) => panic!("the server never answers"),
            _ = first.recv() => {}
        }
        assert!(gateway.shared.pending().is_empty());
        assert!(gateway.tabs().await.unwrap().tabs.is_empty());
        server.await.unwrap();
    }
}
