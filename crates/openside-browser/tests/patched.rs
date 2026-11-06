//! Patched-Chromium (browser/ overlay) coverage: gateway handshake and input bounds, focus
//! arbitration (including popup descendants, `Page.bringToFront`, and fullscreen), ownership
//! ledger and subagent scope, native 1440×900 viewport for the tab lifetime, hidden-tab
//! rendering, automation artifacts, and popup/dialog/download events. Runs only when `OPENSIDE_PATCHED_BROWSER` names the
//! patched `chrome` binary; every test uses loopback fixtures, temporary profiles, and a
//! temporary private runtime directory.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::response::Html;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use openside_browser::{
    Browser, BrowserConfig, BrowserError, BrowserEvent, BrowserPrefUpdate, CookiePolicy, Gateway,
    GatewayConfig, GatewayEvent, GatewayOwner, GatewayScreenshot, OpenedBy,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use common::{event, ready, serial, Server};

const PROBE: &str = r#"<!doctype html><html lang="en"><head><title>Probe</title></head><body>
<h1>Probe</h1><script>
window.frames_drawn = 0;
(function frame() { window.frames_drawn++; requestAnimationFrame(frame); })();
// Classic Runtime.enable detector: an inspector that previews console arguments reads `stack`.
window.inspector_seen = false;
const bait = new Error('bait');
Object.defineProperty(bait, 'stack', { get() { window.inspector_seen = true; return ''; } });
setInterval(() => console.debug(bait), 50);
</script></body></html>"#;

const CHILD: &str = r#"<!doctype html><html lang="en"><head><title>Child</title></head>
<body><h1>Child</h1></body></html>"#;

/// The agent role; the person role advertises only `browser_prefs` and `native_tools`.
const CAPABILITIES: [&str; 8] = [
    "tab_lifecycle",
    "focus",
    "hidden_screenshot",
    "popup",
    "dialog",
    "download",
    "browser_status",
    "native_autofill",
];

fn patched_executable() -> Option<PathBuf> {
    let Some(path) = std::env::var_os("OPENSIDE_PATCHED_BROWSER") else {
        eprintln!(
            "SKIP patched: OPENSIDE_PATCHED_BROWSER is not set; build the browser/ overlay and \
             point it at the patched chrome binary to run this suite"
        );
        return None;
    };
    let path = PathBuf::from(path);
    assert!(
        path.is_file(),
        "OPENSIDE_PATCHED_BROWSER does not name a file: {}",
        path.display()
    );
    Some(path)
}

struct Patched {
    browser: Browser,
    events: mpsc::UnboundedReceiver<BrowserEvent>,
    runtime: TempDir,
}

fn walk(root: &Path) -> Vec<(PathBuf, u32)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            pending.push(entry.path());
            found.push((entry.path(), mode(&entry.path())));
        }
    }
    found
}

/// The browser refuses a runtime directory other users could reach.
fn private_runtime_dir() -> TempDir {
    let runtime = tempfile::Builder::new()
        .prefix("openside-runtime-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    runtime
}

impl Patched {
    async fn launch(executable: &Path, session: &str) -> Self {
        Self::launch_scope(executable, session, None).await
    }

    async fn launch_scope(executable: &Path, session: &str, subagent: Option<&str>) -> Self {
        Self::launch_args(executable, session, subagent, &[]).await
    }

    async fn launch_args(
        executable: &Path,
        session: &str,
        subagent: Option<&str>,
        args: &[String],
    ) -> Self {
        Self::launch_profile(executable, session, subagent, args, None).await
    }

    async fn launch_profile(
        executable: &Path,
        session: &str,
        subagent: Option<&str>,
        args: &[String],
        profile: Option<&Path>,
    ) -> Self {
        Self::launch_with(executable, session, subagent, args, profile, Vec::new()).await
    }

    async fn launch_with(
        executable: &Path,
        session: &str,
        subagent: Option<&str>,
        args: &[String],
        profile: Option<&Path>,
        extensions: Vec<PathBuf>,
    ) -> Self {
        let runtime = private_runtime_dir();
        let mut gateway = GatewayConfig::new(session);
        gateway.subagent_id = subagent.map(str::to_owned);
        gateway.runtime_dir = Some(runtime.path().to_owned());
        gateway.discovery_timeout = Duration::from_secs(10);
        let launcher = runtime.path().join("chrome-launcher");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let mut script = format!("#!/bin/sh\nexec {}", quote(&executable.to_string_lossy()));
        for arg in args {
            script.push(' ');
            script.push_str(&quote(arg));
        }
        script.push_str(" \"$@\"\n");
        std::fs::write(&launcher, script).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = BrowserConfig {
            gateway: Some(gateway),
            profile_dir: profile.map(Path::to_owned),
            extensions,
            ..BrowserConfig::new(launcher)
        };
        let (browser, events) = Browser::launch(config).await.unwrap();
        assert!(
            browser.gateway().is_some(),
            "patched browser did not publish a gateway; runtime dir {:o} holds {:?}; command line {:?}",
            mode(runtime.path()),
            walk(runtime.path()),
            std::fs::read(format!("/proc/{}/cmdline", browser.process_id()))
                .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        );
        Self {
            browser,
            events,
            runtime,
        }
    }

    fn gateway(&self) -> &Gateway {
        self.browser.gateway().unwrap()
    }

    fn instance_dir(&self) -> PathBuf {
        self.runtime
            .path()
            .join("openside/browser")
            .join(self.gateway().instance_id())
    }

    async fn gateway_tab(&self, target: &str) -> openside_browser::GatewayTab {
        self.gateway()
            .tab_for_target(target)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("gateway does not know target {target}"))
    }
}

/// The person's own browser as `tools/openside-browser` starts it: a person-role gateway for
/// the Default profile, and no agent gateway for `Browser::launch` to attach.
struct Person {
    browser: Browser,
    gateway: Gateway,
    _runtime: TempDir,
}

impl Person {
    async fn launch(executable: &Path, args: &[String], extensions: Vec<PathBuf>) -> Self {
        let runtime = private_runtime_dir();
        let launcher = runtime.path().join("chrome-launcher");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let mut script = format!("#!/bin/sh\nexec {}", quote(&executable.to_string_lossy()));
        for arg in [
            "--openside-gateway-instance-id=person-0123456789abcdef".to_owned(),
            format!(
                "--openside-gateway-runtime-dir={}",
                runtime.path().display()
            ),
            "--openside-gateway-role=person".to_owned(),
            "--profile-directory=Default".to_owned(),
        ]
        .iter()
        .chain(args)
        {
            script.push(' ');
            script.push_str(&quote(arg));
        }
        script.push_str(" \"$@\"\n");
        std::fs::write(&launcher, script).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = BrowserConfig {
            gateway: None,
            extensions,
            ..BrowserConfig::new(launcher)
        };
        let (browser, _events) = Browser::launch(config).await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let gateway = loop {
            if let Some(gateway) = Gateway::connect_person(runtime.path(), Duration::from_secs(10))
                .await
                .unwrap()
            {
                break gateway;
            }
            assert!(
                Instant::now() < deadline,
                "person browser did not publish a gateway; runtime dir holds {:?}",
                walk(runtime.path())
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        Self {
            browser,
            gateway,
            _runtime: runtime,
        }
    }
}

/// A preference as chrome://settings in `browser` reads it.
async fn settings_pref(browser: &Browser, key: &str) -> Value {
    let mut raw = RawCdp::connect(browser).await;
    let (_, session) = raw.open("chrome://settings/languages", false).await;
    let expression = format!(
        r#"(async () => {{
        for (let i = 0; i < 100 && !(window.chrome && chrome.settingsPrivate); i++) {{
            await new Promise(r => setTimeout(r, 100));
        }}
        return await new Promise(r => chrome.settingsPrivate.getPref({key:?}, p => r(p.value)));
    }})()"#
    );
    // The evaluation can start in the blank document the navigation then replaces.
    for _ in 0..20 {
        let reply = raw
            .reply(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
            )
            .await;
        if reply.get("error").is_none() {
            return reply["result"]["result"]["value"].clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("chrome://settings never answered for {key}");
}

async fn gateway_event(
    gateway: &Gateway,
    predicate: impl Fn(&GatewayEvent) -> bool,
) -> GatewayEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = gateway.next_event().await.expect("gateway event");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("expected gateway event did not arrive")
}

/// Loopback fixture server; dropping it stops the server task.
struct Probe {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::ops::Deref for Probe {
    type Target = str;
    fn deref(&self) -> &str {
        &self.url
    }
}

async fn probe_server() -> Probe {
    let app = Router::new()
        .route("/probe", get(|| async { Html(PROBE) }))
        .route("/child", get(|| async { Html(CHILD) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/probe", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Probe { url, task }
}

impl Probe {
    fn child(&self) -> String {
        self.url.replace("/probe", "/child")
    }
}

/// A second, independent CDP client: it never applies the controller's viewport emulation.
struct RawCdp {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: u64,
}

impl RawCdp {
    async fn connect(browser: &Browser) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(browser.endpoint().ws_url)
            .await
            .unwrap();
        Self { socket, next: 0 }
    }

    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let reply = self.reply(session, method, params).await;
        assert!(reply.get("error").is_none(), "{method} failed: {reply}");
        reply["result"].clone()
    }

    /// The protocol error message of a command that must be refused.
    async fn refused(&mut self, session: Option<&str>, method: &str, params: Value) -> String {
        let reply = self.reply(session, method, params).await;
        reply["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("{method} was not refused: {reply}"))
            .to_owned()
    }

    async fn reply(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.next += 1;
        let mut message = json!({"id":self.next,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Message::Text(text) = self.socket.next().await.unwrap().unwrap() {
                    let reply: Value = serde_json::from_str(&text).unwrap();
                    if reply["id"] == self.next {
                        return reply;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{method} did not answer"))
    }

    /// Sends a command without waiting for its reply, for commands that block the page.
    async fn send(&mut self, session: Option<&str>, method: &str, params: Value) {
        self.next += 1;
        let mut message = json!({"id":self.next,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .unwrap();
    }

    async fn attach(&mut self, target: &str) -> String {
        self.call(
            None,
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
        )
        .await["sessionId"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn open(&mut self, url: &str, new_window: bool) -> (String, String) {
        let target = self
            .call(
                None,
                "Target.createTarget",
                json!({"url":url,"newWindow":new_window,"background":true}),
            )
            .await["targetId"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = self
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        (target, session)
    }

    async fn eval(&mut self, session: &str, expression: &str) -> Value {
        self.call(
            Some(session),
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
        )
        .await["result"]["value"]
            .clone()
    }

    async fn wait_until(&mut self, session: &str, condition: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.eval(session, condition).await != true {
            assert!(
                Instant::now() < deadline,
                "condition never held: {condition}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// An unpacked MV3 extension with the `tabs` permission and an empty service worker; tests
/// drive the extension APIs by evaluating in that worker.
fn tabs_extension() -> TempDir {
    let extension = tempfile::tempdir().unwrap();
    std::fs::write(
        extension.path().join("manifest.json"),
        json!({"manifest_version":3,"name":"tabs probe","version":"1",
            "permissions":["tabs"],"background":{"service_worker":"sw.js"}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(extension.path().join("sw.js"), "// tabs probe\n").unwrap();
    extension
}

/// Session on the probe extension's activated service worker.
async fn extension_worker(raw: &mut RawCdp) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let worker = loop {
        let targets = raw.call(None, "Target.getTargets", json!({})).await;
        let worker = targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| {
                target["type"] == "service_worker"
                    && target["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("/sw.js"))
            })
            .map(|target| target["targetId"].as_str().unwrap().to_owned());
        if let Some(worker) = worker {
            break worker;
        }
        assert!(Instant::now() < deadline, "probe extension never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let worker = raw.attach(&worker).await;
    raw.wait_until(&worker, "self.serviceWorker?.state === 'activated'")
        .await;
    worker
}

/// Value of an async expression evaluated in the extension worker. An extension API
/// rejection surfaces as `{"error": message}`.
async fn in_extension(raw: &mut RawCdp, worker: &str, body: &str) -> Value {
    let script = format!(
        "(async () => {{ try {{ {body} }} catch (error) {{ return {{error: String(error.message)}}; }} }})()"
    );
    let reply = raw
        .call(
            Some(worker),
            "Runtime.evaluate",
            json!({"expression":script,"awaitPromise":true,"returnByValue":true}),
        )
        .await;
    assert!(reply.get("exceptionDetails").is_none(), "{body}: {reply}");
    reply["result"]["value"].clone()
}

async fn frame(stream: &mut UnixStream) -> Option<Value> {
    let mut header = [0_u8; 4];
    if stream.read_exact(&mut header).await.is_err() {
        return None;
    }
    let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut body).await.ok()?;
    Some(serde_json::from_slice(&body).unwrap())
}

async fn send(stream: &mut UnixStream, message: &Value) {
    let body = serde_json::to_vec(message).unwrap();
    stream
        .write_all(&u32::try_from(body.len()).unwrap().to_be_bytes())
        .await
        .unwrap();
    stream.write_all(&body).await.unwrap();
}

fn png_size(png: &[u8]) -> (u32, u32) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    (width, height)
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn gateway_hello_publishes_private_discovery_and_rejects_bad_clients() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let patched = Patched::launch(&executable, "hello-session").await;
    assert_eq!(patched.gateway().capabilities(), CAPABILITIES);
    let directory = patched.instance_dir();
    assert_eq!(mode(&directory), 0o700);
    assert_eq!(mode(&directory.join("gateway.token")), 0o600);
    let socket = directory.join("gateway.sock");
    assert!(std::fs::symlink_metadata(&socket)
        .unwrap()
        .file_type()
        .is_socket());
    assert_eq!(mode(&socket), 0o600);
    let discovery: Value =
        serde_json::from_slice(&std::fs::read(directory.join("gateway.json")).unwrap()).unwrap();
    assert_eq!(
        discovery,
        json!({"protocol":"openside.browser","versions":[1],
            "instance_id":patched.gateway().instance_id(),
            "pid":patched.browser.process_id(),
            "socket":"gateway.sock","token_file":"gateway.token","role":"agent"})
    );
    let token = std::fs::read_to_string(directory.join("gateway.token")).unwrap();
    assert!(!discovery.to_string().contains(token.trim()));

    // The authenticated controller holds the only control connection.
    let mut second = UnixStream::connect(&socket).await.unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(5), second.read(&mut [0_u8; 1]))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed, 0, "second gateway client was not refused");

    patched.browser.shutdown().await.unwrap();
    assert!(!directory.exists(), "instance directory survived shutdown");

    // Handshake failures, against a browser with no controller connected.
    let runtime = private_runtime_dir();
    let profile = tempfile::tempdir().unwrap();
    let instance = "handshake-probe-0001";
    let mut command = tokio::process::Command::new(&executable);
    command
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--password-store=basic",
            "--disable-extensions",
            "--disable-sync",
            &format!("--user-data-dir={}", profile.path().display()),
            &format!("--openside-gateway-instance-id={instance}"),
            "--openside-agent-session-id=handshake-session",
            "about:blank",
        ])
        .env("XDG_RUNTIME_DIR", runtime.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    common::supervise(&mut command);
    let mut child = command.spawn().unwrap();
    let group = nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap());
    let directory = runtime.path().join("openside/browser").join(instance);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !directory.join("gateway.json").exists() {
        assert!(
            Instant::now() < deadline,
            "raw patched browser published no gateway"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let token = std::fs::read_to_string(directory.join("gateway.token")).unwrap();
    let socket = directory.join("gateway.sock");
    let hello = |token: &str| {
        json!({"kind":"hello","protocol":"openside.browser","versions":[1],
            "instance_id":instance,"token":token})
    };

    let mut stream = UnixStream::connect(&socket).await.unwrap();
    let wrong = "0".repeat(64);
    send(&mut stream, &hello(&wrong)).await;
    let refused = frame(&mut stream).await.unwrap();
    assert_eq!(refused["kind"], "error");
    assert_eq!(refused["code"], "auth_failed");
    assert!(!refused.to_string().contains(&wrong));
    assert!(
        frame(&mut stream).await.is_none(),
        "connection stayed open after auth_failed"
    );

    // Declared lengths are refused before any body byte is accepted: above the protocol
    // limit, and above the much smaller limit for an unauthenticated hello.
    for declared in [17_u32 * 1024 * 1024, 4097] {
        let mut stream = UnixStream::connect(&socket).await.unwrap();
        stream.write_all(&declared.to_be_bytes()).await.unwrap();
        let oversized = frame(&mut stream).await.unwrap();
        assert_eq!(oversized["code"], "frame_too_large", "{declared}");
        assert!(frame(&mut stream).await.is_none());
    }

    // EOF above must also release the controller slot, even if the browser
    // defers destroying the previous connection. Do not sleep before reconnecting.
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    let mut versions = hello(token.trim());
    versions["versions"] = json!([2]);
    send(&mut stream, &versions).await;
    assert_eq!(
        frame(&mut stream).await.unwrap()["code"],
        "version_mismatch"
    );
    assert!(
        frame(&mut stream).await.is_none(),
        "version failure must close its connection"
    );

    let mut stream = UnixStream::connect(&socket).await.unwrap();
    send(&mut stream, &hello(token.trim())).await;
    let welcome = frame(&mut stream).await.unwrap();
    assert_eq!(
        welcome,
        json!({"kind":"welcome","v":1,"instance_id":instance,"capabilities":CAPABILITIES})
    );
    send(
        &mut stream,
        &json!({"v":1,"kind":"request","type":"tab.open","id":"open-1",
            "session_id":"handshake-session","subagent_id":null,"payload":{}}),
    )
    .await;
    let unsupported = frame(&mut stream).await.unwrap();
    assert_eq!(
        (unsupported["id"].clone(), unsupported["code"].clone()),
        (json!("open-1"), json!("unsupported"))
    );
    send(
        &mut stream,
        &json!({"v":1,"kind":"request","type":"tabs.list","id":"list-1",
            "session_id":"other-session","subagent_id":null,"payload":{}}),
    )
    .await;
    assert_eq!(frame(&mut stream).await.unwrap()["code"], "scope_not_found");
    // A subagent scope this browser was not launched for is refused as well.
    send(
        &mut stream,
        &json!({"v":1,"kind":"request","type":"tabs.list","id":"list-2",
            "session_id":"handshake-session","subagent_id":"child","payload":{}}),
    )
    .await;
    assert_eq!(frame(&mut stream).await.unwrap()["code"], "scope_not_found");
    // Authenticated request frames are capped at 1 MiB, also before the body arrives.
    stream
        .write_all(&(2_u32 * 1024 * 1024).to_be_bytes())
        .await
        .unwrap();
    assert_eq!(frame(&mut stream).await.unwrap()["code"], "frame_too_large");
    assert!(frame(&mut stream).await.is_none());

    // A hello that never completes is dropped by the 5 s handshake timer.
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    stream.write_all(&[0, 0]).await.unwrap();
    let stalled_hello = Instant::now();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut rest))
        .await
        .expect("partial hello was never closed")
        .unwrap();
    assert!(stalled_hello.elapsed() >= Duration::from_secs(4));
    drop(stream);

    // Many pipelined requests in one write are all answered, in order, while the client
    // keeps reading; the per-task frame budget only defers work.
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    send(&mut stream, &hello(token.trim())).await;
    assert_eq!(frame(&mut stream).await.unwrap()["kind"], "welcome");
    let (mut reader, mut writer) = stream.into_split();
    let mut burst = Vec::new();
    for index in 0..500 {
        let body = serde_json::to_vec(&json!({"v":1,"kind":"request","type":"tabs.list",
            "id":format!("pipe-{index}"),"session_id":"handshake-session",
            "subagent_id":null,"payload":{}}))
        .unwrap();
        burst.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
        burst.extend_from_slice(&body);
    }
    let pipelined = Instant::now();
    let writing = tokio::spawn(async move {
        writer.write_all(&burst).await.unwrap();
        writer
    });
    for index in 0..500 {
        let mut header = [0_u8; 4];
        reader.read_exact(&mut header).await.unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(header) as usize];
        reader.read_exact(&mut body).await.unwrap();
        let reply: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(reply["kind"], "result", "{reply}");
        assert_eq!(reply["id"], format!("pipe-{index}"));
    }
    eprintln!(
        "patched pipelined: 500 requests in {:?}",
        pipelined.elapsed()
    );
    drop(writing.await.unwrap());
    drop(reader);
    // Let the browser observe the close before the next client authenticates.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A client that streams requests without reading replies is disconnected once the
    // browser's bounded outbound queue fills; the browser neither buffers without limit
    // nor stops serving the next client.
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    send(&mut stream, &hello(token.trim())).await;
    assert_eq!(frame(&mut stream).await.unwrap()["kind"], "welcome");
    let flood_started = Instant::now();
    let mut sent = 0_u64;
    let disconnected = loop {
        let body = serde_json::to_vec(&json!({"v":1,"kind":"request","type":"tabs.list",
            "id":format!("flood-{sent}"),"session_id":"handshake-session",
            "subagent_id":null,"payload":{}}))
        .unwrap();
        let mut bytes = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        bytes.extend_from_slice(&body);
        match tokio::time::timeout(Duration::from_secs(10), stream.write_all(&bytes)).await {
            Ok(Ok(())) => sent += 1,
            Ok(Err(_)) => break true,
            Err(_) => break false,
        }
        assert!(
            flood_started.elapsed() < Duration::from_secs(60),
            "flood never disconnected after {sent} requests"
        );
    };
    assert!(disconnected, "browser stopped reading without closing");
    drop(stream);
    let mut stream = UnixStream::connect(&socket).await.unwrap();
    send(&mut stream, &hello(token.trim())).await;
    let mut welcome = frame(&mut stream).await;
    // The flooding connection is dropped asynchronously; one retry covers that window.
    if welcome.is_none() {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stream = UnixStream::connect(&socket).await.unwrap();
        send(&mut stream, &hello(token.trim())).await;
        welcome = frame(&mut stream).await;
    }
    assert_eq!(welcome.unwrap()["kind"], "welcome");
    send(
        &mut stream,
        &json!({"v":1,"kind":"request","type":"tabs.list","id":"after-flood",
            "session_id":"handshake-session","subagent_id":null,"payload":{}}),
    )
    .await;
    let after = frame(&mut stream).await.unwrap();
    assert_eq!(after["kind"], "result", "{after}");
    eprintln!(
        "patched flood: disconnected after {sent} unread requests in {:?}",
        flood_started.elapsed()
    );
    drop(stream);
    let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
    child.wait().await.unwrap();
    eprintln!("patched timing: gateway hello {:?}", started.elapsed());
}

#[tokio::test]
async fn agent_tabs_never_take_focus() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch(&executable, "focus-session").await;
    let agent = patched
        .browser
        .create_agent_tab("focus-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &agent).await;
    let agent_tab = patched.gateway_tab(&agent.id).await;

    // Stock Chromium focuses the page for both of these; the patched arbiter refuses.
    let mut raw = RawCdp::connect(&patched.browser).await;
    raw.call(None, "Target.activateTarget", json!({"targetId":agent.id}))
        .await;
    let denied = gateway_event(patched.gateway(), |event| {
        event.kind == "focus.decided" && event.tab_id.as_deref() == Some(&agent_tab.tab_id)
    })
    .await;
    assert_eq!(
        denied.payload,
        json!({"request_id":null,"allowed":false,"reason":"no_user_grant","owner":"user"})
    );
    assert_eq!(denied.session_id.as_deref(), Some("focus-session"));
    assert_eq!(
        patched
            .browser
            .evaluate(&agent.id, "window.focus(); document.hasFocus()")
            .await
            .unwrap(),
        false
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        patched
            .browser
            .evaluate(&agent.id, "document.hasFocus()")
            .await
            .unwrap(),
        false
    );

    // Page.bringToFront activates and then focuses the view directly; both are refused.
    let session = raw.attach(&agent.id).await;
    raw.call(Some(&session), "Page.bringToFront", json!({}))
        .await;
    let denied = gateway_event(patched.gateway(), |event| {
        event.kind == "focus.decided" && event.tab_id.as_deref() == Some(&agent_tab.tab_id)
    })
    .await;
    assert_eq!(denied.payload["allowed"], false);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(raw.eval(&session, "document.hasFocus()").await, false);
    // Fullscreen would take over the window; agent pages never get it.
    assert_eq!(
        patched
            .browser
            .evaluate(
                &agent.id,
                "document.documentElement.requestFullscreen().then(() => 'entered', () => 'denied')"
            )
            .await
            .unwrap(),
        "denied"
    );

    let decision = patched
        .gateway()
        .request_focus(&agent_tab.tab_id)
        .await
        .unwrap();
    assert!(!decision.allowed);
    assert_eq!(
        (decision.reason.as_str(), decision.owner.as_str()),
        ("no_user_grant", "user")
    );
    patched.browser.shutdown().await.unwrap();
    eprintln!("patched timing: focus invariant {:?}", started.elapsed());
}

#[tokio::test]
async fn agent_popup_descendants_never_take_focus_or_authority() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch(&executable, "descendant-session").await;
    let agent = patched
        .browser
        .create_agent_tab("descendant-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &agent).await;
    let agent_tab = patched.gateway_tab(&agent.id).await;
    let mut raw = RawCdp::connect(&patched.browser).await;

    // Agent tab -> popup -> grandchild popup, each opened with a user gesture.
    let open = format!("window.open({:?}, '_blank', 'popup'); true", probe.child());
    patched.browser.evaluate(&agent.id, &open).await.unwrap();
    let child = gateway_event(patched.gateway(), |event| {
        event.kind == "popup.opened" && event.payload["opener_tab_id"] == agent_tab.tab_id.as_str()
    })
    .await;
    let child_tab = child.payload["child_tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let child_target = child.payload["child_tab"]["cdp_target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let child_session = raw.attach(&child_target).await;
    raw.wait_until(&child_session, "document.readyState === 'complete'")
        .await;
    raw.call(
        Some(&child_session),
        "Runtime.evaluate",
        json!({"expression":open,"userGesture":true}),
    )
    .await;
    let grandchild = gateway_event(patched.gateway(), |event| {
        event.kind == "popup.opened" && event.payload["opener_tab_id"] == child_tab.as_str()
    })
    .await;
    let grandchild_tab = grandchild.payload["child_tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let grandchild_target = grandchild.payload["child_tab"]["cdp_target_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Descendants keep external ownership: no agent authority flows through ancestry.
    for (popup, tab_id) in [(&child, &child_tab), (&grandchild, &grandchild_tab)] {
        assert_eq!(popup.payload["child_tab"]["owner"], "external");
        assert_eq!(popup.payload["child_tab"]["opened_by"], "external");
        assert!(matches!(
            patched.gateway().set_label(tab_id, "mine").await,
            Err(openside_browser::BrowserError::Gateway { code, .. }) if code == "ownership_denied"
        ));
    }

    // ...but they inherit the agent's focus restrictions on every activation path.
    let grandchild_session = raw.attach(&grandchild_target).await;
    for (target, session, tab_id) in [
        (&child_target, &child_session, &child_tab),
        (&grandchild_target, &grandchild_session, &grandchild_tab),
    ] {
        raw.call(None, "Target.activateTarget", json!({"targetId":target}))
            .await;
        gateway_event(patched.gateway(), |event| {
            event.kind == "focus.decided" && event.tab_id.as_deref() == Some(tab_id.as_str())
        })
        .await;
        raw.call(Some(session), "Page.bringToFront", json!({}))
            .await;
        gateway_event(patched.gateway(), |event| {
            event.kind == "focus.decided" && event.tab_id.as_deref() == Some(tab_id.as_str())
        })
        .await;
        raw.call(
            Some(session),
            "Runtime.evaluate",
            json!({"expression":"window.focus()","userGesture":true}),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(raw.eval(session, "document.hasFocus()").await, false);
    }
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: popup descendant focus {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn subagent_scope_round_trips_through_the_gateway() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch_scope(&executable, "parent-session", Some("child-1")).await;
    let agent = patched
        .browser
        .create_agent_tab("parent-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &agent).await;
    let tab = patched.gateway_tab(&agent.id).await;
    assert_eq!(tab.owner, GatewayOwner::Agent);
    assert_eq!(tab.session_id.as_deref(), Some("parent-session"));
    assert_eq!(tab.subagent_id.as_deref(), Some("child-1"));
    // Ownership-checked requests succeed only because the client sends the child scope.
    let labelled = patched
        .gateway()
        .set_label(&tab.tab_id, "child work")
        .await
        .unwrap();
    assert_eq!(labelled.label, "child work");
    let shot = patched.gateway().screenshot(&tab.tab_id).await.unwrap();
    assert_eq!((shot.width, shot.height), (1440, 900));
    let event = gateway_event(patched.gateway(), |event| {
        event.kind == "tab.updated" && event.payload["reason"] == "label"
    })
    .await;
    assert_eq!(event.subagent_id.as_deref(), Some("child-1"));
    patched.browser.shutdown().await.unwrap();
    eprintln!("patched timing: subagent scope {:?}", started.elapsed());
}

#[tokio::test]
async fn agent_viewport_stays_1440x900_after_resize_attempts() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let extension = tabs_extension();
    let patched = Patched::launch_with(
        &executable,
        "resize-session",
        None,
        &[],
        None,
        vec![extension.path().to_owned()],
    )
    .await;
    let mut raw = RawCdp::connect(&patched.browser).await;
    let user_target = patched
        .browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.opened_by == OpenedBy::External)
        .expect("initial browser tab")
        .id;
    let (windowed, windowed_session) = raw.open(&probe, true).await;
    let (hidden, hidden_session) = raw.open(&probe, false).await;
    for session in [&windowed_session, &hidden_session] {
        raw.wait_until(session, "innerWidth === 1440 && innerHeight === 900")
            .await;
    }

    // DevTools clients are the agent here: every size or state change is refused.
    let window = window_of(&mut raw, &windowed).await;
    for (method, params) in [
        (
            "Browser.setWindowBounds",
            json!({"windowId":window,"bounds":{"width":800,"height":600}}),
        ),
        (
            "Browser.setWindowBounds",
            json!({"windowId":window,"bounds":{"windowState":"maximized"}}),
        ),
        (
            "Browser.setContentsSize",
            json!({"windowId":window,"width":800,"height":600}),
        ),
    ] {
        let message = raw.refused(None, method, params).await;
        assert!(message.contains("never resizes"), "{method}: {message}");
    }

    // The user resizes their own window, which also hosts the hidden agent tab (here
    // through an extension, which acts for the user); the hidden agent tab keeps its size.
    let user_window = window_of(&mut raw, &user_target).await;
    let worker = extension_worker(&mut raw).await;
    let resized = in_extension(
        &mut raw,
        &worker,
        &format!(
            "await chrome.windows.update({user_window}, {{width: 900, height: 700}}); \
             return 'resized';"
        ),
    )
    .await;
    assert_eq!(resized, "resized");
    tokio::time::sleep(Duration::from_millis(500)).await;
    for session in [&windowed_session, &hidden_session] {
        raw.wait_until(session, "innerWidth === 1440 && innerHeight === 900")
            .await;
    }
    for target in [&windowed, &hidden] {
        let tab = patched.gateway_tab(target).await;
        let shot = patched.gateway().screenshot(&tab.tab_id).await.unwrap();
        assert_eq!((shot.width, shot.height), (1440, 900));
        assert_eq!(png_size(&shot.png), (1440, 900));
    }
    // At most two captures run per connection; extra concurrent requests are refused
    // instead of queueing unbounded bitmaps.
    let tab = patched.gateway_tab(&hidden).await;
    let results =
        futures_util::future::join_all((0..6).map(|_| patched.gateway().screenshot(&tab.tab_id)))
            .await;
    let mut captured = 0;
    let mut refused = 0;
    for result in results {
        match result {
            Ok(shot) => {
                assert_eq!((shot.width, shot.height), (1440, 900));
                captured += 1;
            }
            Err(openside_browser::BrowserError::Gateway { code, .. })
                if code == "resource_exhausted" =>
            {
                refused += 1
            }
            Err(error) => panic!("unexpected capture error {error:?}"),
        }
    }
    assert!(
        captured >= 2 && refused >= 1,
        "{captured} captured, {refused} refused"
    );
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: viewport after resize attempts {:?}",
        started.elapsed()
    );
}

/// The browser is the single owner of an agent page's layout: 1440x900 CSS pixels at device
/// scale factor 1. Another DevTools client that overrides the scale factor, shrinks the page,
/// clears, or detaches with an override still set is undone; the controller adds no override
/// of its own. Showing the tab in the user's window or moving it between windows keeps the
/// layout, and the user's tabs are never emulated.
#[tokio::test]
async fn agent_viewport_has_one_owner_across_clients_and_tab_moves() {
    const LAYOUT: &str = "innerWidth === 1440 && innerHeight === 900 && devicePixelRatio === 1";
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let extension = tabs_extension();
    let patched = Patched::launch_with(
        &executable,
        "viewport-owner-session",
        None,
        &[],
        None,
        vec![extension.path().to_owned()],
    )
    .await;
    let mut raw = RawCdp::connect(&patched.browser).await;
    let user_target = patched
        .browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.opened_by == OpenedBy::External)
        .expect("initial browser tab")
        .id;
    let controlled = patched
        .browser
        .create_agent_tab("viewport-owner-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &controlled).await;
    let controlled_tab = patched.gateway_tab(&controlled.id).await;
    let controlled_session = raw.attach(&controlled.id).await;
    raw.wait_until(&controlled_session, LAYOUT).await;

    // Another client's override at the same size changes only the scale factor; the next
    // capture re-applies the browser's own override first.
    let mut other = RawCdp::connect(&patched.browser).await;
    let other_session = other.attach(&controlled.id).await;
    let override_at = |width: u32, height: u32, scale: u32| json!({"width":width,"height":height,"deviceScaleFactor":scale,"mobile":false});
    other
        .call(
            Some(&other_session),
            "Emulation.setDeviceMetricsOverride",
            override_at(1440, 900, 2),
        )
        .await;
    assert_eq!(
        raw.eval(&controlled_session, "devicePixelRatio").await,
        2,
        "precondition: the other client's override took effect"
    );
    let shot = patched
        .gateway()
        .screenshot(&controlled_tab.tab_id)
        .await
        .unwrap();
    assert_eq!(png_size(&shot.png), (1440, 900));
    raw.wait_until(&controlled_session, LAYOUT).await;
    // A smaller override, a clear, and a detach with an override still set.
    other
        .call(
            Some(&other_session),
            "Emulation.setDeviceMetricsOverride",
            override_at(800, 600, 1),
        )
        .await;
    raw.wait_until(&controlled_session, LAYOUT).await;
    other
        .call(
            Some(&other_session),
            "Emulation.clearDeviceMetricsOverride",
            json!({}),
        )
        .await;
    raw.wait_until(&controlled_session, LAYOUT).await;
    other
        .call(
            Some(&other_session),
            "Emulation.setDeviceMetricsOverride",
            override_at(1440, 900, 3),
        )
        .await;
    assert_eq!(
        raw.eval(&controlled_session, "devicePixelRatio").await,
        3,
        "precondition: the other client's override took effect"
    );
    other
        .call(
            None,
            "Target.detachFromTarget",
            json!({"sessionId":other_session}),
        )
        .await;
    raw.wait_until(&controlled_session, LAYOUT).await;
    drop(other);

    // A background agent tab in the user's window; the user resizes that window and shows
    // the agent tab in it (through an extension, which acts for the user).
    let (hidden, hidden_session) = raw.open(&probe, false).await;
    raw.wait_until(&hidden_session, LAYOUT).await;
    let hidden_tab = patched.gateway_tab(&hidden).await;
    let user_window = window_of(&mut raw, &user_target).await;
    let controlled_window = window_of(&mut raw, &controlled.id).await;
    let worker = extension_worker(&mut raw).await;
    let hidden_id = in_extension(
        &mut raw,
        &worker,
        &format!(
            "await chrome.windows.update({user_window}, {{width: 900, height: 700}}); \
             const [tab] = await chrome.tabs.query({{windowId: {user_window}, url: {:?}}}); \
             await chrome.tabs.update(tab.id, {{active: true}}); return tab.id;",
            &*probe
        ),
    )
    .await;
    assert!(hidden_id.is_i64(), "{hidden_id}");
    raw.wait_until(&hidden_session, LAYOUT).await;
    let shot = patched
        .gateway()
        .screenshot(&hidden_tab.tab_id)
        .await
        .unwrap();
    assert_eq!(png_size(&shot.png), (1440, 900));
    let user_session = raw.attach(&user_target).await;
    let user_width = raw.eval(&user_session, "innerWidth").await;
    assert!(
        user_width.as_i64().is_some_and(|width| width < 1440),
        "the user's tab was emulated: innerWidth {user_width}"
    );

    // Into the controlled tab's window (now shared by two agent tabs), then out into a new
    // window of its own.
    for (step, script) in [
        (
            "move into the agent window",
            format!(
                "await chrome.tabs.move({hidden_id}, {{windowId: {controlled_window}, index: -1}}); \
                 return true;"
            ),
        ),
        (
            "move into a new window",
            format!("await chrome.windows.create({{tabId: {hidden_id}, focused: false}}); return true;"),
        ),
    ] {
        assert_eq!(in_extension(&mut raw, &worker, &script).await, true, "{step}");
        for session in [&hidden_session, &controlled_session] {
            raw.wait_until(session, LAYOUT).await;
        }
        for tab in [&hidden_tab, &controlled_tab] {
            let shot = patched.gateway().screenshot(&tab.tab_id).await.unwrap();
            assert_eq!(png_size(&shot.png), (1440, 900), "{step}");
        }
    }
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: single viewport owner {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn ownership_ledger_reports_agent_tabs_in_their_group() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let mut patched = Patched::launch(&executable, "ledger-session").await;
    let user = patched
        .browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.opened_by == OpenedBy::External)
        .expect("initial browser tab");
    let agent = patched
        .browser
        .create_agent_tab("ledger-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &agent).await;
    let opened = gateway_event(patched.gateway(), |event| event.kind == "tab.opened").await;
    assert_eq!(opened.session_id.as_deref(), Some("ledger-session"));
    assert_eq!(opened.payload["tab"]["cdp_target_id"], agent.id.as_str());
    // The group join precedes the announcement, so the event already names the group.
    assert_eq!(opened.payload["tab"]["group_title"], "Agent Tabs");

    let ledger = patched.gateway().tabs().await.unwrap();
    assert!(ledger.through_event_seq >= opened.seq);
    let agent_tab = ledger
        .tabs
        .iter()
        .find(|tab| tab.cdp_target_id.as_deref() == Some(agent.id.as_str()))
        .unwrap();
    assert_eq!(agent_tab.owner, GatewayOwner::Agent);
    assert_eq!(agent_tab.opened_by, GatewayOwner::Agent);
    assert_eq!(agent_tab.session_id.as_deref(), Some("ledger-session"));
    assert_eq!(agent_tab.subagent_id, None);
    assert!(agent_tab.background);
    assert_eq!(agent_tab.group_title.as_deref(), Some("Agent Tabs"));
    assert_eq!(agent_tab.url, probe.url);
    let user_tab = ledger
        .tabs
        .iter()
        .find(|tab| tab.cdp_target_id.as_deref() == Some(user.id.as_str()))
        .unwrap();
    assert_eq!(user_tab.owner, GatewayOwner::External);
    assert_eq!(user_tab.session_id, None);
    assert_eq!(user_tab.group_title, None);

    let labelled = patched
        .gateway()
        .set_label(&agent_tab.tab_id, "research")
        .await
        .unwrap();
    assert_eq!(labelled.label, "research");
    assert!(matches!(
        patched.gateway().set_label(&user_tab.tab_id, "mine").await,
        Err(openside_browser::BrowserError::Gateway { .. })
    ));

    patched
        .browser
        .close_agent_tab("ledger-session", &agent.id)
        .await
        .unwrap();
    event(
        &mut patched.events,
        |event| matches!(event, BrowserEvent::TabClosed { tab, .. } if tab.id == agent.id),
    )
    .await;
    let closed = gateway_event(patched.gateway(), |event| event.kind == "tab.closed").await;
    assert_eq!(closed.tab_id.as_deref(), Some(agent_tab.tab_id.as_str()));
    assert!(patched
        .gateway()
        .tab_for_target(&agent.id)
        .await
        .unwrap()
        .is_none());
    patched.browser.shutdown().await.unwrap();
    eprintln!("patched timing: ownership ledger {:?}", started.elapsed());
}

/// An isolated space's agent tab lives in its own CDP browser context, yet the patched build
/// still files it under the session in the Agent Tabs group, beside no user tab.
#[tokio::test]
async fn space_tabs_are_agent_tabs_in_their_group() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let probe = probe_server().await;
    let patched = Patched::launch(&executable, "space-session").await;
    let context = patched.browser.open_space("space-session").await.unwrap();
    let agent = patched
        .browser
        .create_agent_tab("space-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &agent).await;
    assert_eq!(agent.browser_context_id.as_deref(), Some(context.as_str()));
    gateway_event(patched.gateway(), |event| {
        event.kind == "tab.opened" && event.payload["tab"]["cdp_target_id"] == agent.id.as_str()
    })
    .await;
    let tab = patched.gateway_tab(&agent.id).await;
    assert_eq!(tab.owner, GatewayOwner::Agent);
    assert_eq!(tab.session_id.as_deref(), Some("space-session"));
    assert_eq!(tab.group_title.as_deref(), Some("Agent Tabs"));
    assert!(patched.browser.close_space("space-session").await.unwrap());
    assert!(patched
        .browser
        .tabs()
        .await
        .iter()
        .all(|tab| tab.id != agent.id));
    assert_eq!(patched.browser.prune_spaces().await.unwrap(), 0);
    patched.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn agent_viewport_is_native_1440x900_and_hidden_tabs_keep_rendering() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch(&executable, "viewport-session").await;
    let mut raw = RawCdp::connect(&patched.browser).await;

    // Created and inspected only through the raw client, so the controller sets no
    // override: the browser alone lays these pages out at 1440x900.
    let (windowed, windowed_session) = raw.open(&probe, true).await;
    let (hidden, hidden_session) = raw.open(&probe, false).await;
    for session in [&windowed_session, &hidden_session] {
        // The initial about:blank is complete and already emulated too; wait for the probe.
        raw.wait_until(
            session,
            "document.readyState === 'complete' && typeof window.frames_drawn === 'number'",
        )
        .await;
        raw.wait_until(session, "innerWidth === 1440 && innerHeight === 900")
            .await;
    }
    assert_eq!(
        raw.eval(&hidden_session, "document.visibilityState").await,
        "hidden"
    );
    let frames = |value: Value| {
        value
            .as_u64()
            .unwrap_or_else(|| panic!("frame counter is not a number: {value}"))
    };
    let before = frames(raw.eval(&hidden_session, "window.frames_drawn").await);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let after = frames(raw.eval(&hidden_session, "window.frames_drawn").await);
    assert!(
        after >= before + 20,
        "hidden agent tab stopped rendering: {before} -> {after} frames in 1s"
    );

    for target in [&windowed, &hidden] {
        let tab = patched.gateway_tab(target).await;
        assert_eq!(tab.owner, GatewayOwner::Agent);
        let shot = patched.gateway().screenshot(&tab.tab_id).await.unwrap();
        assert_eq!((shot.width, shot.height), (1440, 900));
        assert_eq!(png_size(&shot.png), (1440, 900));
    }
    // The controller's screenshot routes owned tabs through the native hidden capture.
    let png = patched.browser.screenshot(&hidden).await.unwrap();
    assert_eq!(png_size(&png), (1440, 900));
    assert_eq!(
        raw.eval(&hidden_session, "document.visibilityState").await,
        "hidden"
    );
    assert_eq!(
        raw.eval(&hidden_session, "document.hasFocus()").await,
        false
    );
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: viewport and hidden rendering {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn automation_artifacts_are_absent() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch(&executable, "artifact-session").await;
    patched.browser.record_protocol(true);
    let agent = patched
        .browser
        .create_agent_tab("artifact-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &agent).await;
    patched.browser.snapshot(&agent.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let page = patched
        .browser
        .evaluate(
            &agent.id,
            "({webdriver: navigator.webdriver, ua: navigator.userAgent, \
              brands: (navigator.userAgentData?.brands ?? []).map(b => b.brand), \
              inspector: window.inspector_seen})",
        )
        .await
        .unwrap();
    assert_eq!(page["webdriver"], false);
    let ua = page["ua"].as_str().unwrap();
    assert!(ua.contains("Chrome/"), "unexpected UA {ua}");
    assert!(!ua.contains("Headless"), "UA advertises headless: {ua}");
    assert!(
        !page["brands"].to_string().contains("Headless"),
        "client hints advertise headless: {}",
        page["brands"]
    );
    assert_eq!(page["inspector"], false);
    let command_line =
        std::fs::read(format!("/proc/{}/cmdline", patched.browser.process_id())).unwrap();
    let command_line = String::from_utf8_lossy(&command_line);
    assert!(!command_line
        .split('\0')
        .any(|arg| arg == "--enable-automation"));
    let status = patched
        .gateway()
        .request("browser.status", json!({}))
        .await
        .unwrap();
    assert_eq!(status["automation_infobar_count"], 0, "{status}");
    let methods: Vec<String> = patched
        .browser
        .take_protocol_log()
        .into_iter()
        .map(|(method, _)| method)
        .collect();
    assert!(methods
        .iter()
        .any(|method| method == "Target.attachToTarget"));
    assert!(
        !methods.iter().any(|method| method == "Runtime.enable"),
        "gateway mode enabled the Runtime domain: {methods:?}"
    );
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: automation artifacts {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn native_ux_defaults_are_enabled() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let patched = Patched::launch(&executable, "native-ux-session").await;
    let status = patched
        .gateway()
        .request("browser.status", json!({}))
        .await
        .unwrap();
    for feature in ["vertical_tabs", "split_view", "bookmarks_side_panel"] {
        assert_eq!(status["ux"][feature], true, "{feature}: {status}");
    }
    eprintln!("patched native UX: {}", status["ux"]);
    patched.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn browser_prefs_read_and_write_the_profile_through_the_gateway() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let person = Person::launch(&executable, &[], Vec::new()).await;
    let gateway = &person.gateway;
    let before = gateway.prefs().await.unwrap();
    assert!(before.spell_check, "{before:?}");
    assert!(!before.accept_languages.is_empty(), "{before:?}");
    assert_eq!(before.default_zoom_percent, Some(100));
    // A fresh profile has not loaded its engines; the browser loads them first.
    assert!(!before.search_engine.options.is_empty(), "{before:?}");
    assert!(
        before
            .search_engine
            .options
            .iter()
            .all(|engine| engine.id > 0 && !engine.name.is_empty()),
        "{before:?}"
    );

    let after = gateway
        .set_pref(&BrowserPrefUpdate::SpellCheck(false))
        .await
        .unwrap();
    assert!(!after.spell_check);
    let after = gateway
        .set_pref(&BrowserPrefUpdate::AcceptLanguages(vec![
            "ko".into(),
            "en-US".into(),
        ]))
        .await
        .unwrap();
    assert_eq!(after.accept_languages, ["ko", "en-US"]);
    // Flip whatever the build defaults to (ungoogled-chromium blocks).
    let (cookies, cookie_mode) = match before.third_party_cookies {
        CookiePolicy::Block => (CookiePolicy::Allow, 2),
        CookiePolicy::Allow => (CookiePolicy::Block, 1),
    };
    let after = gateway
        .set_pref(&BrowserPrefUpdate::ThirdPartyCookies(cookies))
        .await
        .unwrap();
    assert_eq!(after.third_party_cookies, cookies);
    let after = gateway
        .set_pref(&BrowserPrefUpdate::AutoPictureInPicture(false))
        .await
        .unwrap();
    assert_eq!(after.auto_picture_in_picture, Some(false));
    let after = gateway
        .set_pref(&BrowserPrefUpdate::DefaultZoomPercent(125))
        .await
        .unwrap();
    assert_eq!(after.default_zoom_percent, Some(125));
    if let Some(other) = before
        .search_engine
        .options
        .iter()
        .find(|engine| Some(engine.id) != before.search_engine.current)
    {
        let after = gateway
            .set_pref(&BrowserPrefUpdate::SearchEngine(other.id))
            .await
            .unwrap();
        assert_eq!(after.search_engine.current, Some(other.id));
    }

    // chrome://settings reads the same profile values the gateway wrote.
    let mut seen = Vec::new();
    for key in [
        "browser.enable_spellchecking",
        "intl.accept_languages",
        "profile.cookie_controls_mode",
    ] {
        seen.push(settings_pref(&person.browser, key).await);
    }
    assert_eq!(seen, [json!(false), json!("ko,en-US"), json!(cookie_mode)]);

    for (update, code) in [
        (
            BrowserPrefUpdate::DefaultZoomPercent(101),
            "invalid_request",
        ),
        (
            BrowserPrefUpdate::AcceptLanguages(vec!["ko,en".into()]),
            "invalid_request",
        ),
        (
            BrowserPrefUpdate::AcceptLanguages(vec!["ko".into(), "KO".into()]),
            "invalid_request",
        ),
        (BrowserPrefUpdate::SearchEngine(999_999), "invalid_request"),
    ] {
        let result = gateway.set_pref(&update).await;
        assert!(
            matches!(&result, Err(BrowserError::Gateway { code: refused, .. }) if refused == code),
            "{update:?}: {result:?}; before {before:?}"
        );
    }
    let unknown = gateway
        .request(
            "prefs.set",
            json!({"name":"download.default_directory","value":"/tmp"}),
        )
        .await;
    assert!(
        matches!(&unknown, Err(BrowserError::Gateway { code, .. }) if code == "invalid_request"),
        "{unknown:?}"
    );
    person.browser.shutdown().await.unwrap();
}

fn gateway_code<T: std::fmt::Debug>(result: &Result<T, BrowserError>) -> &str {
    match result {
        Err(BrowserError::Gateway { code, .. }) => code,
        other => panic!("expected a gateway refusal, got {other:?}"),
    }
}

/// An unpacked extension carrying the openside extension's manifest key, so it gets the pinned
/// id the gateway looks for, and its global `mini-popup` command without a suggested key.
fn mini_popup_extension() -> TempDir {
    let protocol = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/desktopNotifyProtocol.ts"),
    )
    .unwrap();
    let key = protocol
        .split("EXTENSION_KEY =")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .expect("EXTENSION_KEY in web/src/desktopNotifyProtocol.ts");
    let extension = tempfile::tempdir().unwrap();
    std::fs::write(
        extension.path().join("manifest.json"),
        json!({"manifest_version":3,"name":"mini popup probe","version":"1","key":key,
            "background":{"service_worker":"sw.js"},
            "commands":{"mini-popup":{"description":"Open the mini popup","global":true}}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(extension.path().join("sw.js"), "// mini popup probe\n").unwrap();
    extension
}

/// An unpacked extension whose browser-only `hold` command suggests Alt+Space, which Chromium
/// grants it on install.
fn alt_space_extension() -> TempDir {
    let extension = tempfile::tempdir().unwrap();
    std::fs::write(
        extension.path().join("manifest.json"),
        json!({"manifest_version":3,"name":"alt space holder","version":"1",
            "background":{"service_worker":"sw.js"},
            "commands":{"hold":{"description":"Hold Alt+Space",
                "suggested_key":{"default":"Alt+Space"}}}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(extension.path().join("sw.js"), "// alt space holder\n").unwrap();
    extension
}

/// A command of the extension named `extension` as chrome://extensions/shortcuts in `browser`
/// reads it.
async fn shortcuts_page_command(browser: &Browser, extension: &str, command: &str) -> Value {
    let mut raw = RawCdp::connect(browser).await;
    let (_, session) = raw.open("chrome://extensions/shortcuts", false).await;
    let expression = format!(
        r#"(async () => {{
        for (let i = 0; i < 100 && !(window.chrome && chrome.developerPrivate); i++) {{
            await new Promise(r => setTimeout(r, 100));
        }}
        const info = (await chrome.developerPrivate.getExtensionsInfo()).find(e => e.name === {extension:?});
        const command = info.commands.find(c => c.name === {command:?});
        return {{keybinding: command.keybinding, scope: command.scope, active: command.isActive}};
    }})()"#
    );
    // The evaluation can start in the blank document the navigation then replaces.
    for _ in 0..20 {
        let reply = raw
            .reply(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
            )
            .await;
        if reply.get("error").is_none() && reply["result"]["exceptionDetails"].is_null() {
            return reply["result"]["result"]["value"].clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("chrome://extensions/shortcuts never answered");
}

#[tokio::test]
async fn browser_prefs_bind_the_mini_popup_shortcut_of_the_openside_extension() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    // Without the openside extension there is no mini popup to bind; a malformed value is
    // still refused as malformed.
    let bare = Person::launch(&executable, &[], Vec::new()).await;
    assert_eq!(bare.gateway.prefs().await.unwrap().mini_popup, None);
    assert_eq!(
        gateway_code(
            &bare
                .gateway
                .set_pref(&BrowserPrefUpdate::MiniPopup(true))
                .await
        ),
        "unsupported"
    );
    assert_eq!(
        gateway_code(
            &bare
                .gateway
                .request("prefs.set", json!({"name":"mini_popup","value":"on"}))
                .await
        ),
        "invalid_request"
    );
    bare.browser.shutdown().await.unwrap();

    let extension = mini_popup_extension();
    let person = Person::launch(&executable, &[], vec![extension.path().to_owned()]).await;
    let gateway = &person.gateway;
    // The unpacked extension finishes loading after the gateway is up. A fresh profile never
    // binds the command by itself: the toggle starts off.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let prefs = gateway.prefs().await.unwrap();
        if let Some(enabled) = prefs.mini_popup {
            assert!(!enabled, "{prefs:?}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the extension never loaded: {prefs:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let on = gateway
        .set_pref(&BrowserPrefUpdate::MiniPopup(true))
        .await
        .unwrap();
    assert_eq!(on.mini_popup, Some(true));
    assert_eq!(gateway.prefs().await.unwrap().mini_popup, Some(true));
    assert_eq!(
        shortcuts_page_command(&person.browser, "mini popup probe", "mini-popup").await,
        json!({"keybinding":"Alt+Space","scope":"GLOBAL","active":true})
    );
    // Turning it on again is idempotent.
    let again = gateway
        .set_pref(&BrowserPrefUpdate::MiniPopup(true))
        .await
        .unwrap();
    assert_eq!(again.mini_popup, Some(true));

    let off = gateway
        .set_pref(&BrowserPrefUpdate::MiniPopup(false))
        .await
        .unwrap();
    assert_eq!(off.mini_popup, Some(false));
    assert_eq!(gateway.prefs().await.unwrap().mini_popup, Some(false));
    assert_eq!(
        shortcuts_page_command(&person.browser, "mini popup probe", "mini-popup").await
            ["keybinding"],
        json!("")
    );
    person.browser.shutdown().await.unwrap();

    // Another extension holding Alt+Space keeps it: turning the mini popup on is refused and
    // the setting stays off.
    let holder = alt_space_extension();
    let person = Person::launch(
        &executable,
        &[],
        vec![extension.path().to_owned(), holder.path().to_owned()],
    )
    .await;
    let gateway = &person.gateway;
    let held = json!({"keybinding":"Alt+Space","scope":"CHROME","active":true});
    assert_eq!(
        shortcuts_page_command(&person.browser, "alt space holder", "hold").await,
        held
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while gateway.prefs().await.unwrap().mini_popup.is_none() {
        assert!(Instant::now() < deadline, "the extension never loaded");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let refused = gateway.set_pref(&BrowserPrefUpdate::MiniPopup(true)).await;
    assert_eq!(gateway_code(&refused), "unsupported");
    assert_eq!(gateway.prefs().await.unwrap().mini_popup, Some(false));
    assert_eq!(
        shortcuts_page_command(&person.browser, "alt space holder", "hold").await,
        held
    );
    assert_eq!(
        shortcuts_page_command(&person.browser, "mini popup probe", "mini-popup").await
            ["keybinding"],
        json!("")
    );
    person.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn split_view_keys_open_side_by_side_and_stacked_splits() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let probe = probe_server().await;
    let extension = tabs_extension();
    let person = Person::launch(&executable, &[], vec![extension.path().to_owned()]).await;
    let mut raw = RawCdp::connect(&person.browser).await;
    let worker = extension_worker(&mut raw).await;
    const VIEWPORT: &str = "[innerWidth, innerHeight]";
    // Ctrl+Shift+\ splits side by side, Ctrl+Shift+- stacked; the page's viewport halves along
    // that axis. The key reaches the page first, which leaves it unhandled.
    for (key, code, key_code, stacked) in
        [("|", "Backslash", 220, false), ("_", "Minus", 189, true)]
    {
        let (target, session) = raw.open(&probe, true).await;
        let window = window_of(&mut raw, &target).await;
        let activated = in_extension(
            &mut raw,
            &worker,
            &format!(
                "const [tab] = await chrome.tabs.query({{windowId: {window}}}); \
                 await chrome.tabs.update(tab.id, {{active: true}}); \
                 await chrome.windows.update({window}, {{focused: true}}); return true;"
            ),
        )
        .await;
        assert_eq!(activated, true, "{activated}");
        raw.wait_until(&session, "document.visibilityState === 'visible'")
            .await;
        let before = raw.eval(&session, VIEWPORT).await;
        for kind in ["rawKeyDown", "keyUp"] {
            raw.call(
                Some(&session),
                "Input.dispatchKeyEvent",
                json!({"type":kind,"modifiers":2 | 8,"key":key,"code":code,
                    "windowsVirtualKeyCode":key_code,"nativeVirtualKeyCode":key_code}),
            )
            .await;
        }
        let split = format!(
            "const tabs = await chrome.tabs.query({{windowId: {window}}}); \
             return tabs.length === 2 && tabs.every(tab => tab.splitViewId > 0) \
                 && tabs[0].splitViewId === tabs[1].splitViewId;"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while in_extension(&mut raw, &worker, &split).await != true {
            assert!(Instant::now() < deadline, "{code}: no split opened");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // The split's own frame takes a few pixels on both axes; only the split axis halves.
        let (width, height) = (before[0].as_i64().unwrap(), before[1].as_i64().unwrap());
        let (halved, kept) = if stacked {
            (("innerHeight", height), ("innerWidth", width))
        } else {
            (("innerWidth", width), ("innerHeight", height))
        };
        raw.wait_until(
            &session,
            &format!(
                "{} < {} * 0.6 && {} > {} * 0.9",
                halved.0, halved.1, kept.0, kept.1
            ),
        )
        .await;
    }
    person.browser.shutdown().await.unwrap();
}

/// One `test.tab_rename` round trip. Every answer has the title the tab strip renders, whether
/// the tab has a name, the tab context menu's command ids, whether Reset is enabled, and the open
/// editor's text.
async fn tab_rename(gateway: &Gateway, payload: Value) -> Value {
    gateway.request("test.tab_rename", payload).await.unwrap()
}

/// Renames through the real menu item and editor bubble: opens it from the context menu, checks
/// it is prefilled with the current title, types `text` over the selection and presses Enter.
async fn rename_in_editor(gateway: &Gateway, tab_id: &str, text: &str) -> Value {
    let before = tab_rename(gateway, json!({"tab_id":tab_id,"action":"get"})).await;
    let open = tab_rename(gateway, json!({"tab_id":tab_id,"action":"open_editor"})).await;
    assert_eq!(open["editor"], before["title"], "{open}");
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"edit","text":text,"key":"enter"}),
    )
    .await
}

/// Polls until the tab strip shows `expected`; the page title can arrive after the load event.
async fn tab_strip_title(gateway: &Gateway, tab_id: &str, expected: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let answer = tab_rename(gateway, json!({"tab_id":tab_id,"action":"get"})).await;
            if answer["title"] == expected {
                return answer;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("tab strip never showed {expected:?}"))
}

/// The commands right after Duplicate (2), up to Pin (6), in menu order.
fn rename_items(answer: &Value) -> Vec<i64> {
    let menu: Vec<i64> = answer["menu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_i64().unwrap())
        .collect();
    let duplicate = menu.iter().position(|&id| id == 2).expect("Duplicate item");
    let pin = menu.iter().position(|&id| id == 6).expect("Pin item");
    menu[duplicate + 1..pin].to_vec()
}

/// Polls a CDP target until its page has committed `url`.
async fn target_url(raw: &mut RawCdp, target: &str, url: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let info = raw
                .call(None, "Target.getTargetInfo", json!({"targetId":target}))
                .await;
            if info["targetInfo"]["url"] == url {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("page never committed {url:?}"));
}

/// Polls until the tab has (or has no) name; for navigations that give no other signal.
async fn tab_strip_custom(gateway: &Gateway, tab_id: &str, custom: bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while tab_rename(gateway, json!({"tab_id":tab_id,"action":"get"})).await["custom"] != custom
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the tab's name never became custom={custom}"));
}

/// Polls a CDP target until its page reports `title`.
async fn target_title(raw: &mut RawCdp, target: &str, title: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let info = raw
                .call(None, "Target.getTargetInfo", json!({"targetId":target}))
                .await;
            if info["targetInfo"]["title"] == title {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("page title never became {title:?}"));
}

/// Tab rename (S20, UX-06) through the real tab context menu and editor bubble. The menu offers
/// Rename tab (29) right after Duplicate, and Reset tab name (30) only while the tab has a name.
/// Esc or an untouched field changes nothing; Enter and losing focus commit; an empty name and
/// Reset clear it. The name replaces the title the tab strip renders, survives page retitles,
/// same-origin navigations and reloads, and is dropped by a cross-origin navigation; the page and
/// its CDP target keep the page title.
#[tokio::test]
async fn tab_rename_survives_same_origin_and_clears_cross_origin() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let probe = probe_server().await;
    // The test method exists only behind its switch.
    let plain = Patched::launch(&executable, "rename-off").await;
    let tab = plain
        .browser
        .create_agent_tab("rename-off", &probe)
        .await
        .unwrap();
    let tab_id = plain.gateway_tab(&tab.id).await.tab_id;
    assert_eq!(
        gateway_code(
            &plain
                .gateway()
                .request("test.tab_rename", json!({"tab_id":tab_id,"action":"get"}))
                .await
        ),
        "invalid_request"
    );
    plain.browser.shutdown().await.unwrap();

    let patched = Patched::launch_args(
        &executable,
        "rename-session",
        None,
        &["--openside-test-tab-rename".to_owned()],
    )
    .await;
    let tab = patched
        .browser
        .create_agent_tab("rename-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &tab).await;
    let tab_id = patched.gateway_tab(&tab.id).await.tab_id;
    let gateway = patched.gateway();
    let get = json!({"tab_id":tab_id,"action":"get"});

    let unnamed = tab_strip_title(gateway, &tab_id, "Probe").await;
    assert_eq!(unnamed["custom"], false);
    assert_eq!(rename_items(&unnamed), [29]);
    assert_eq!(unnamed["reset_enabled"], false);
    assert_eq!(unnamed["editor"], Value::Null);
    assert_eq!(
        gateway_code(
            &gateway
                .request("test.tab_rename", json!({"tab_id":tab_id,"action":"reset"}))
                .await
        ),
        "unavailable"
    );

    // The restore path takes only "<tuple origin>\n<one-line UTF-8 name>" whose origin is the
    // page being restored, and gives the name the same normalization and cap as typing it.
    let origin = probe.url.split('/').take(3).collect::<Vec<_>>().join("/");
    let restore = |bytes: Vec<u8>| {
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        tab_rename(
            gateway,
            json!({"tab_id":tab_id,"action":"restore","extra_hex":hex}),
        )
    };
    let restored = restore(format!("{origin}\n  Restored   name ").into_bytes()).await;
    assert_eq!(restored["title"], "Restored name");
    assert_eq!(restored["custom"], true);
    let capped = restore(format!("{origin}\n{}", "\u{1d11e}".repeat(300)).into_bytes()).await;
    assert_eq!(capped["title"], "\u{1d11e}".repeat(256));
    let stale = origin.replace("127.0.0.1", "localhost");
    let mut invalid_utf8 = format!("{origin}\n").into_bytes();
    invalid_utf8.extend([0xff, b'A']);
    for malformed in [
        b"Restored".to_vec(),
        b"\nRestored".to_vec(),
        b"not a url\nRestored".to_vec(),
        b"null\nRestored".to_vec(),
        format!("{origin}/\nRestored").into_bytes(),
        format!("{stale}\nRestored").into_bytes(),
        format!("{origin}\nRestored\nname").into_bytes(),
        format!("{origin}\n").into_bytes(),
        format!("{origin}\n   ").into_bytes(),
        invalid_utf8,
    ] {
        let answer = restore(malformed.clone()).await;
        assert_eq!(
            answer["custom"],
            false,
            "{:?}",
            String::from_utf8_lossy(&malformed)
        );
        assert_eq!(answer["title"], "Probe");
    }

    // Esc, and Enter on the untouched prefilled title, change nothing and close the editor.
    let open = tab_rename(gateway, json!({"tab_id":tab_id,"action":"open_editor"})).await;
    assert_eq!(open["editor"], "Probe");
    let escaped = tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"edit","text":"Ignored","key":"escape"}),
    )
    .await;
    assert_eq!(escaped["custom"], false);
    assert_eq!(escaped["editor"], Value::Null);
    tab_rename(gateway, json!({"tab_id":tab_id,"action":"open_editor"})).await;
    let untouched = tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"edit","key":"enter"}),
    )
    .await;
    assert_eq!(untouched["custom"], false);
    assert_eq!(untouched["editor"], Value::Null);

    // Enter commits; whitespace runs collapse as they do in a tab title.
    let named = rename_in_editor(gateway, &tab_id, "  Release   notes ").await;
    assert_eq!(named["title"], "Release notes");
    assert_eq!(named["custom"], true);
    assert_eq!(named["editor"], Value::Null);
    assert_eq!(rename_items(&named), [29, 30]);
    assert_eq!(named["reset_enabled"], true);
    assert_eq!(named["saved"], true);
    assert_eq!(
        patched
            .browser
            .evaluate(&tab.id, "document.title")
            .await
            .unwrap(),
        "Probe"
    );
    let mut raw = RawCdp::connect(&patched.browser).await;
    target_title(&mut raw, &tab.id, "Probe").await;
    // The gateway's tab records keep the page title too.
    assert_eq!(patched.gateway_tab(&tab.id).await.title, "Probe");

    // Losing focus commits too.
    let open = tab_rename(gateway, json!({"tab_id":tab_id,"action":"open_editor"})).await;
    assert_eq!(open["editor"], "Release notes");
    let blurred = tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"edit","text":"Docs","key":"blur"}),
    )
    .await;
    assert_eq!(blurred["title"], "Docs");
    assert_eq!(blurred["editor"], Value::Null);

    // The page retitling itself keeps the name once the browser has the new page title.
    patched
        .browser
        .evaluate(&tab.id, "document.title = 'Retitled'")
        .await
        .unwrap();
    target_title(&mut raw, &tab.id, "Retitled").await;
    assert_eq!(tab_rename(gateway, get.clone()).await["title"], "Docs");
    // So do a same-origin navigation and a reload.
    patched
        .browser
        .navigate("rename-session", &tab.id, &probe.child())
        .await
        .unwrap();
    target_title(&mut raw, &tab.id, "Child").await;
    assert_eq!(tab_rename(gateway, get.clone()).await["title"], "Docs");
    patched
        .browser
        .evaluate(
            &tab.id,
            "window.before_reload = true; location.reload(); true",
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let fresh = patched
                .browser
                .evaluate(
                    &tab.id,
                    "window.before_reload === undefined && document.readyState === 'complete'",
                )
                .await;
            if matches!(fresh, Ok(Value::Bool(true))) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("reload never finished");
    let reloaded = tab_rename(gateway, get.clone()).await;
    assert_eq!(reloaded["title"], "Docs");
    assert_eq!(reloaded["custom"], true);

    // Another origin (same port, other host) drops it; the page title shows again.
    let other_origin = probe.url.replace("127.0.0.1", "localhost");
    patched
        .browser
        .navigate("rename-session", &tab.id, &other_origin)
        .await
        .unwrap();
    let cleared = tab_strip_title(gateway, &tab_id, "Probe").await;
    assert_eq!(cleared["custom"], false);
    assert_eq!(rename_items(&cleared), [29]);

    // Reset is the menu item; an empty name clears too.
    rename_in_editor(gateway, &tab_id, "Draft").await;
    let reset = tab_rename(gateway, json!({"tab_id":tab_id,"action":"reset"})).await;
    assert_eq!(reset["title"], "Probe");
    assert_eq!(reset["custom"], false);
    assert_eq!(rename_items(&reset), [29]);
    assert_eq!(reset["reset_enabled"], false);
    rename_in_editor(gateway, &tab_id, "Draft").await;
    let emptied = rename_in_editor(gateway, &tab_id, "   ").await;
    assert_eq!(emptied["title"], "Probe");
    assert_eq!(emptied["custom"], false);

    // Names are capped at 256 grapheme clusters: characters outside the BMP, a combining
    // sequence and an emoji ZWJ sequence at the boundary stay whole, and a cut right after a
    // space drops that space.
    let astral = rename_in_editor(gateway, &tab_id, &"\u{1d11e}".repeat(256)).await;
    assert_eq!(astral["title"], "\u{1d11e}".repeat(256));
    let long = rename_in_editor(gateway, &tab_id, &"\u{1d11e}".repeat(257)).await;
    assert_eq!(long["title"], "\u{1d11e}".repeat(256));
    assert_eq!(long["custom"], true);
    let combining = format!("{}e\u{301}", "a".repeat(255));
    let named = rename_in_editor(gateway, &tab_id, &format!("{combining}tail")).await;
    assert_eq!(named["title"], combining.as_str());
    let family = format!(
        "{}\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
        "a".repeat(255)
    );
    let named = rename_in_editor(gateway, &tab_id, &format!("{family}x")).await;
    assert_eq!(named["title"], family.as_str());
    let cut = rename_in_editor(gateway, &tab_id, &format!("{} tail", "a".repeat(255))).await;
    assert_eq!(cut["title"], "a".repeat(255));

    // A name on a page that is not http or https (here a file: page) shows but is not saved
    // with the session, since restore would never accept it.
    let local_dir = TempDir::new().unwrap();
    let local = local_dir.path().join("local.html");
    std::fs::write(&local, "<title>Local</title>").unwrap();
    let local_url = format!("file://{}", local.display());
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"navigate","url":local_url}),
    )
    .await;
    target_url(&mut raw, &tab.id, &local_url).await;
    tab_strip_title(gateway, &tab_id, "Local").await;
    let local_name = rename_in_editor(gateway, &tab_id, "Notes").await;
    assert_eq!(local_name["custom"], true);
    assert_eq!(local_name["saved"], false);

    // about:blank the page opens itself inherits its origin and keeps the name; one the browser
    // loads (typed, or here the gateway) has no origin and drops it.
    patched
        .browser
        .evaluate(
            &tab.id,
            "setTimeout(() => { location = 'about:blank'; }); true",
        )
        .await
        .unwrap();
    target_url(&mut raw, &tab.id, "about:blank").await;
    assert_eq!(tab_rename(gateway, get.clone()).await["custom"], true);
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"navigate","url":"about:blank"}),
    )
    .await;
    tab_strip_custom(gateway, &tab_id, false).await;

    // An error page counts as the URL it failed to load: a same-origin failure keeps the name,
    // another origin's drops it.
    patched
        .browser
        .navigate("rename-session", &tab.id, &probe.url)
        .await
        .unwrap();
    tab_strip_title(gateway, &tab_id, "Probe").await;
    rename_in_editor(gateway, &tab_id, "Docs").await;
    let session = raw
        .call(
            None,
            "Target.attachToTarget",
            json!({"targetId":tab.id,"flatten":true}),
        )
        .await["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let offline = |offline: bool| json!({"offline":offline,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1});
    raw.call(Some(&session), "Network.enable", json!({})).await;
    raw.call(
        Some(&session),
        "Network.emulateNetworkConditions",
        offline(true),
    )
    .await;
    let failed = format!("{}?offline", probe.child());
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"navigate","url":failed}),
    )
    .await;
    target_url(&mut raw, &tab.id, &failed).await;
    let kept = tab_rename(gateway, get.clone()).await;
    assert_eq!(kept["title"], "Docs");
    assert_eq!(kept["custom"], true);
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"navigate",
            "url":failed.replace("127.0.0.1", "localhost")}),
    )
    .await;
    tab_strip_custom(gateway, &tab_id, false).await;
    raw.call(
        Some(&session),
        "Network.emulateNetworkConditions",
        offline(false),
    )
    .await;

    // Closing a tab while its editor is open closes the editor without a crash.
    let other = patched
        .browser
        .create_agent_tab("rename-session", &probe)
        .await
        .unwrap();
    ready(&patched.browser, &other).await;
    let other_id = patched.gateway_tab(&other.id).await.tab_id;
    let open = tab_rename(gateway, json!({"tab_id":other_id,"action":"open_editor"})).await;
    assert_eq!(open["editor"], "Probe");
    raw.call(None, "Target.closeTarget", json!({"targetId":other.id}))
        .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while tab_rename(gateway, get.clone()).await["editor"] != Value::Null {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the editor outlived its tab");

    patched.browser.shutdown().await.unwrap();
}

/// A tab's name is saved with the session: after a restart that restores the last session the
/// tab shows it again, and the cross-origin rule still holds against the origin it was given on.
#[tokio::test]
async fn tab_rename_survives_a_restart_with_session_restore() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let probe = probe_server().await;
    let profile = TempDir::new().unwrap();
    let switch = "--openside-test-tab-rename".to_owned();
    let first = Patched::launch_profile(
        &executable,
        "rename-restart",
        None,
        std::slice::from_ref(&switch),
        Some(profile.path()),
    )
    .await;
    let tab = first
        .browser
        .create_agent_tab("rename-restart", &probe)
        .await
        .unwrap();
    ready(&first.browser, &tab).await;
    let tab_id = first.gateway_tab(&tab.id).await.tab_id;
    tab_strip_title(first.gateway(), &tab_id, "Probe").await;
    let named = rename_in_editor(first.gateway(), &tab_id, "Release notes").await;
    assert_eq!(named["title"], "Release notes");
    // Chromium writes session commands on a 2.5 s timer; SIGTERM then quits it the way a person
    // closing the browser does, which saves the session.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let pid = first.browser.process_id();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    tokio::time::timeout(Duration::from_secs(20), async {
        // The test process is Chromium's parent and reaps it only in `shutdown`: until then an
        // exited browser is a zombie.
        while std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        }) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("browser did not exit on SIGTERM");
    let _ = tokio::time::timeout(Duration::from_secs(10), first.browser.shutdown()).await;

    let second = Patched::launch_profile(
        &executable,
        "rename-restart",
        None,
        &[switch, "--restore-last-session".to_owned()],
        Some(profile.path()),
    )
    .await;
    let mut raw = RawCdp::connect(&second.browser).await;
    let target = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let targets = raw.call(None, "Target.getTargets", json!({})).await;
            if let Some(found) = targets["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .find(|info| info["type"] == "page" && info["url"] == probe.url.as_str())
            {
                return found["targetId"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the named tab was not restored");
    let tab_id = second.gateway_tab(&target).await.tab_id;
    let gateway = second.gateway();
    let restored = tab_strip_title(gateway, &tab_id, "Release notes").await;
    assert_eq!(restored["custom"], true);
    assert_eq!(rename_items(&restored), [29, 30]);

    // The restored name belongs to the origin it was given on: a same-origin page keeps it, and
    // another origin drops it.
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"navigate","url":probe.child()}),
    )
    .await;
    target_title(&mut raw, &target, "Child").await;
    let kept = tab_rename(gateway, json!({"tab_id":tab_id,"action":"get"})).await;
    assert_eq!(kept["title"], "Release notes");
    assert_eq!(kept["custom"], true);
    tab_rename(
        gateway,
        json!({"tab_id":tab_id,"action":"navigate",
            "url":probe.url.replace("127.0.0.1", "localhost")}),
    )
    .await;
    let cleared = tab_strip_title(gateway, &tab_id, "Probe").await;
    assert_eq!(cleared["custom"], false);

    second.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn browser_prefs_wait_for_search_engines_is_bounded() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let person = Person::launch(
        &executable,
        &["--openside-test-search-engines-never-load".to_owned()],
        Vec::new(),
    )
    .await;
    let gateway = &person.gateway;
    // A malformed request is refused at once; it never waits for the engines.
    let started = Instant::now();
    let malformed = gateway
        .request(
            "prefs.set",
            json!({"name":"default_zoom_percent","value":101}),
        )
        .await;
    assert_eq!(gateway_code(&malformed), "invalid_request");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    let started = Instant::now();
    let results = futures_util::future::join_all((0..10).map(|_| gateway.prefs())).await;
    let mut codes: Vec<&str> = results.iter().map(gateway_code).collect();
    codes.sort_unstable();
    // Eight wait and expire; the rest are refused instead of queueing.
    assert_eq!(
        codes,
        [vec!["resource_exhausted"; 2], vec!["timeout"; 8]].concat()
    );
    assert!(
        started.elapsed() < Duration::from_secs(11),
        "{:?}",
        started.elapsed()
    );
    // Expired waits are released: a later request waits again, it is not refused.
    let again = gateway
        .set_pref(&BrowserPrefUpdate::SpellCheck(false))
        .await;
    assert_eq!(gateway_code(&again), "timeout");
    person.browser.shutdown().await.unwrap();
}

/// Two patched browsers side by side: the agent browser and the person's own. Settings reach the
/// person's startup profile only; the agent gateway refuses them and its profile is untouched.
#[tokio::test]
async fn browser_prefs_hit_the_person_profile_never_the_agent_profile() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let agent = Patched::launch(&executable, "prefs-agent").await;
    let person = Person::launch(&executable, &[], Vec::new()).await;
    assert!(!agent.gateway().supports_prefs());
    assert_eq!(
        gateway_code(&agent.gateway().request("prefs.get", json!({})).await),
        "role_denied"
    );
    assert_eq!(
        gateway_code(
            &agent
                .gateway()
                .request("prefs.set", json!({"name":"spell_check","value":false}))
                .await
        ),
        "role_denied"
    );
    assert_eq!(
        gateway_code(
            &agent
                .gateway()
                .request("native.open", json!({"tool":"task_manager"}))
                .await
        ),
        "role_denied"
    );
    let written = person
        .gateway
        .set_pref(&BrowserPrefUpdate::SpellCheck(false))
        .await
        .unwrap();
    assert!(!written.spell_check);
    assert_eq!(
        settings_pref(&person.browser, "browser.enable_spellchecking").await,
        json!(false)
    );
    assert_eq!(
        settings_pref(&agent.browser, "browser.enable_spellchecking").await,
        json!(true)
    );
    person.browser.shutdown().await.unwrap();
    agent.browser.shutdown().await.unwrap();
}

/// An unpacked MV3 extension allowed to take over the person's privacy settings.
fn privacy_extension() -> TempDir {
    let extension = tempfile::tempdir().unwrap();
    std::fs::write(
        extension.path().join("manifest.json"),
        json!({"manifest_version":3,"name":"privacy probe","version":"1",
            "permissions":["privacy"],"background":{"service_worker":"sw.js"},
            // An engine the person's browser gained from an extension: never listed.
            "chrome_settings_overrides":{"search_provider":{"name":"Probe search",
                "keyword":"probe","search_url":"https://probe.example/?q={searchTerms}",
                "favicon_url":"https://probe.example/favicon.ico","encoding":"UTF-8",
                "is_default":false}}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(extension.path().join("sw.js"), "// privacy probe\n").unwrap();
    extension
}

#[tokio::test]
async fn browser_prefs_refuse_controlled_settings_and_never_expose_custom_engines() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let extension = privacy_extension();
    let person = Person::launch(&executable, &[], vec![extension.path().to_owned()]).await;
    let gateway = &person.gateway;
    let listed = gateway.request("prefs.get", json!({})).await.unwrap();
    assert!(
        listed["search_engine"]["options"]
            .as_array()
            .is_some_and(|options| !options.is_empty()),
        "{listed}"
    );
    let text = listed.to_string();
    for leak in [
        "Ask openside",
        "openside routines",
        "Probe search",
        "probe",
        "127.0.0.1",
        "searchTerms",
        "http",
    ] {
        assert!(!text.contains(leak), "{leak} leaked: {text}");
    }

    let extra = gateway
        .request(
            "prefs.set",
            json!({"name":"spell_check","value":false,"profile":"Other"}),
        )
        .await;
    assert_eq!(gateway_code(&extra), "invalid_request");

    // An extension taking over third-party cookies makes the setting not the person's to change.
    let mut raw = RawCdp::connect(&person.browser).await;
    let worker = extension_worker(&mut raw).await;
    raw.eval(
        &worker,
        "chrome.privacy.websites.thirdPartyCookiesAllowed.set({value: true}); true",
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let result = gateway
            .set_pref(&BrowserPrefUpdate::ThirdPartyCookies(CookiePolicy::Block))
            .await;
        if result.is_err() {
            assert_eq!(gateway_code(&result), "policy_denied");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "extension never took control: {result:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A malformed value is refused as malformed even for a controlled setting.
    let invalid = gateway
        .request(
            "prefs.set",
            json!({"name":"third_party_cookies","value":"sometimes"}),
        )
        .await;
    assert_eq!(gateway_code(&invalid), "invalid_request");
    assert_eq!(
        gateway.prefs().await.unwrap().third_party_cookies,
        CookiePolicy::Allow
    );
    person.browser.shutdown().await.unwrap();
}

/// The person's browser serves settings and native tools only; the browser itself refuses every
/// tab and agent method, and never reports the person's tabs as events.
#[tokio::test]
async fn person_role_refuses_every_tab_and_agent_method() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let probe = probe_server().await;
    let person = Person::launch(&executable, &[], Vec::new()).await;
    let mut capabilities = person.gateway.capabilities().to_vec();
    capabilities.sort();
    assert_eq!(capabilities, ["browser_prefs", "native_tools"]);
    // A tab the person opens is never adopted or reported.
    let mut raw = RawCdp::connect(&person.browser).await;
    let (target, _) = raw.open(&probe.url, false).await;
    for (kind, payload) in [
        ("tabs.list", json!({})),
        ("browser.status", json!({})),
        ("focus.request", json!({"tab_id":target,"grant_id":null})),
        (
            "screenshot.capture",
            json!({"tab_id":target,"format":"png"}),
        ),
        ("tab.set_label", json!({"tab_id":target,"label":"x"})),
        (
            "autofill.fields",
            json!({"tab_id":target,"origin":"http://127.0.0.1"}),
        ),
        (
            "autofill.fill",
            json!({"tab_id":target,"handle":"h","origin":"o","value":"v"}),
        ),
        ("tab.open", json!({"url":"https://example.com/"})),
        ("tab.close", json!({"tab_id":target})),
        ("dialog.respond", json!({"tab_id":target,"accept":true})),
        ("download.cancel", json!({"download_id":"1"})),
        ("no.such.method", json!({})),
    ] {
        assert_eq!(
            gateway_code(&person.gateway.request(kind, payload).await),
            "role_denied",
            "{kind}"
        );
    }
    let quiet = tokio::time::timeout(Duration::from_secs(1), person.gateway.next_event()).await;
    assert!(quiet.is_err(), "person gateway sent an event: {quiet:?}");
    person.browser.shutdown().await.unwrap();
}

/// `--incognito` makes the person's windows off the record, never the profile the gateway serves:
/// settings are read and written on the regular startup profile, and native tools need one of its
/// normal windows.
#[tokio::test]
async fn person_role_under_incognito_serves_the_regular_profile_only() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let person = Person::launch(&executable, &["--incognito".to_owned()], Vec::new()).await;
    let gateway = &person.gateway;
    assert!(gateway.prefs().await.unwrap().spell_check);
    let written = gateway
        .set_pref(&BrowserPrefUpdate::SpellCheck(false))
        .await
        .unwrap();
    assert!(!written.spell_check);
    assert!(!gateway.prefs().await.unwrap().spell_check);
    assert_eq!(
        gateway_code(
            &gateway
                .request("native.open", json!({"tool":"devtools"}))
                .await
        ),
        "unsupported"
    );
    person.browser.shutdown().await.unwrap();
}

/// A startup profile outside the user data dir is never the person's: nothing is read, written
/// or opened, and a malformed request is still malformed.
#[tokio::test]
async fn person_role_without_a_valid_startup_profile_serves_nothing() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    // Without its startup profile loaded the browser refuses CDP context management, so it runs
    // bare here: only the person gateway is needed.
    let runtime = private_runtime_dir();
    let user_data = tempfile::tempdir().unwrap();
    let mut browser = tokio::process::Command::new(&executable)
        .args([
            "--headless=new".to_owned(),
            "--no-first-run".to_owned(),
            "--no-default-browser-check".to_owned(),
            format!("--user-data-dir={}", user_data.path().display()),
            "--openside-gateway-instance-id=person-0123456789abcdef".to_owned(),
            format!(
                "--openside-gateway-runtime-dir={}",
                runtime.path().display()
            ),
            "--openside-gateway-role=person".to_owned(),
            "--profile-directory=nested/Other".to_owned(),
            "about:blank".to_owned(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let gateway = loop {
        if let Some(gateway) = Gateway::connect_person(runtime.path(), Duration::from_secs(10))
            .await
            .unwrap()
        {
            break gateway;
        }
        assert!(
            Instant::now() < deadline,
            "no person gateway: {:?}",
            walk(runtime.path())
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let gateway = &gateway;
    assert_eq!(gateway_code(&gateway.prefs().await), "unsupported");
    assert_eq!(
        gateway_code(
            &gateway
                .set_pref(&BrowserPrefUpdate::SpellCheck(false))
                .await
        ),
        "unsupported"
    );
    for payload in [
        json!({"name":"default_zoom_percent","value":101}),
        json!({"name":"spell_check","value":"no"}),
        json!({"name":"download.default_directory","value":"/tmp"}),
    ] {
        assert_eq!(
            gateway_code(&gateway.request("prefs.set", payload.clone()).await),
            "invalid_request",
            "{payload}"
        );
    }
    assert_eq!(
        gateway_code(
            &gateway
                .request("native.open", json!({"tool":"task_manager"}))
                .await
        ),
        "unsupported"
    );
    browser.kill().await.unwrap();
}

/// native.open reaches the browser's own Task manager and DevTools for the person's profile.
#[tokio::test]
async fn person_native_tools_open_task_manager_and_devtools() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let probe = probe_server().await;
    let person = Person::launch(&executable, &[], Vec::new()).await;
    let mut raw = RawCdp::connect(&person.browser).await;
    raw.open(&probe.url, false).await;
    let devtools_targets = |targets: &Value| {
        targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|target| {
                target["url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with("devtools://devtools/"))
            })
            .count()
    };
    let before = devtools_targets(&raw.call(None, "Target.getTargets", json!({})).await);
    person
        .gateway
        .open_native_tool(openside_browser::NativeTool::DevTools)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while devtools_targets(&raw.call(None, "Target.getTargets", json!({})).await) <= before {
        assert!(Instant::now() < deadline, "no DevTools window opened");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The browser answers opened only after chrome::OpenTaskManager ran for the person's window.
    person
        .gateway
        .open_native_tool(openside_browser::NativeTool::TaskManager)
        .await
        .unwrap();
    for payload in [
        json!({}),
        json!({"tool":"bookmarks"}),
        json!({"tool":"devtools","tab_id":1}),
    ] {
        assert_eq!(
            gateway_code(&person.gateway.request("native.open", payload).await),
            "invalid_request"
        );
    }
    assert!(person.browser.is_connected());
    person.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn daemon_ui_new_tab_and_routines_open_existing_web_ui() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    assert!(
        dist.join("index.html").is_file(),
        "build web/dist before patched UI smoke"
    );
    let index = std::fs::read_to_string(dist.join("index.html")).unwrap();
    let app = Router::new()
        .route(
            "/",
            get({
                let index = index.clone();
                move || {
                    let index = index.clone();
                    async move { Html(index) }
                }
            }),
        )
        .route(
            "/routines",
            get(move || {
                let index = index.clone();
                async move { Html(index) }
            }),
        )
        .route(
            "/assets/{file}",
            get(
                move |axum::extract::Path(file): axum::extract::Path<String>| {
                    let dist = dist.clone();
                    async move {
                        assert!(!file.contains('/') && !file.contains(".."));
                        let bytes = std::fs::read(dist.join("assets").join(&file)).unwrap();
                        let mime = if file.ends_with(".css") {
                            "text/css"
                        } else {
                            "text/javascript"
                        };
                        ([(axum::http::header::CONTENT_TYPE, mime)], bytes)
                    }
                },
            ),
        )
        .route(
            "/v1/routines",
            get(|| async {
                axum::Json(json!({"routines":[{
                    "id":"smoke-routine","name":"Local smoke routine",
                    "schedule":{"type":"interval","secs":3600},
                    "action":{"type":"new_session","prompt":"Local fixture"},
                    "enabled":true,"catch_up":false,"created_at":"2026-10-12T00:00:00Z"
                }]}))
            }),
        )
        .route(
            "/v1/routines/suggestions",
            get(|| async { axum::Json(json!({"suggestions":[]})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ui = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let _server = Probe {
        url: ui.clone(),
        task,
    };
    let patched = Patched::launch_args(
        &executable,
        "ui-session",
        None,
        &[format!("--openside-daemon-ui-url={ui}")],
    )
    .await;
    let tab = patched
        .browser
        .create_agent_tab("ui-session", "chrome://newtab/")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if patched
                .browser
                .evaluate(&tab.id, "location.href")
                .await
                .unwrap()
                == ui
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let status = patched
        .gateway()
        .request("browser.status", json!({}))
        .await
        .unwrap();
    assert_eq!(status["ui"], json!({"ask":true,"routines":true}));
    // A synthetic device token models normal UI pairing, not a browser privilege.
    patched
        .browser
        .evaluate(
            &tab.id,
            "localStorage.setItem('openside.token','fixture-device'); true",
        )
        .await
        .unwrap();
    patched
        .browser
        .navigate("ui-session", &tab.id, &format!("{ui}routines"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if patched
                .browser
                .evaluate(
                    &tab.id,
                    "document.body.innerText.includes('Local smoke routine')",
                )
                .await
                .unwrap()
                == true
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let mut inspector = RawCdp::connect(&patched.browser).await;
    let context = inspector
        .call(
            None,
            "Target.createBrowserContext",
            json!({"disposeOnDetach":true}),
        )
        .await["browserContextId"]
        .clone();
    let target = inspector
        .call(
            None,
            "Target.createTarget",
            json!({"url":"chrome://newtab/","browserContextId":context,"background":true}),
        )
        .await["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let incognito = inspector.attach(&target).await;
    inspector
        .wait_until(&incognito, "document.readyState === 'complete'")
        .await;
    assert!(inspector
        .eval(&incognito, "location.href")
        .await
        .as_str()
        .unwrap()
        .starts_with("chrome://"));
    inspector
        .call(
            None,
            "Target.disposeBrowserContext",
            json!({"browserContextId":context}),
        )
        .await;
    eprintln!(
        "patched daemon UI: NTP, ask keyword, routines keyword and routine API list observed"
    );
    patched.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_autofill_checks_origin_scope_and_document_without_cdp_values() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let server = Server::start().await;
    let patched = Patched::launch(&executable, "fill-session").await;
    let tab = patched
        .browser
        .create_agent_tab("fill-session", &server.page("login"))
        .await
        .unwrap();
    ready(&patched.browser, &tab).await;
    let gateway_tab = patched.gateway_tab(&tab.id).await;
    let origin = patched.browser.origin(&tab.id).await.unwrap();
    let snapshot = patched.browser.snapshot(&tab.id).await.unwrap();
    let reference = snapshot
        .nodes
        .iter()
        .find(|node| node.name == "Password")
        .unwrap()
        .reference
        .as_deref()
        .unwrap();
    let node = patched
        .browser
        .resolve_ref("fill-session", &tab.id, reference)
        .await
        .unwrap();
    let request = json!({"tab_id":gateway_tab.tab_id,"backend_node_id":node.backend_node_id});
    let fields = patched
        .gateway()
        .request("autofill.fields", request.clone())
        .await
        .unwrap();
    let password = fields["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["kind"] == "password")
        .expect("password field");
    let payload = json!({"tab_id":gateway_tab.tab_id,"handle":password["handle"],"origin":"https://invalid.example","value":"native-canary-482"});
    let error = patched
        .gateway()
        .request("autofill.fill", payload)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("native-canary"));
    assert_eq!(
        patched
            .browser
            .evaluate(
                &tab.id,
                "document.querySelector('input[type=password]').value"
            )
            .await
            .unwrap(),
        ""
    );
    let fields = patched
        .gateway()
        .request("autofill.fields", request.clone())
        .await
        .unwrap();
    let password = fields["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["kind"] == "password")
        .unwrap();
    patched.browser.record_protocol(true);
    patched
        .gateway()
        .fill_secret_node(
            &tab.id,
            password["backend_node_id"].as_u64().unwrap(),
            &origin,
            "native-canary-482",
        )
        .await
        .unwrap();
    let trace = patched.browser.take_protocol_log();
    assert!(!serde_json::to_string(&trace)
        .unwrap()
        .contains("native-canary-482"));
    assert!(!trace.iter().any(|(method, _)| method == "Input.insertText"
        || method == "Runtime.enable"
        || method == "Network.enable"));
    assert_eq!(
        patched
            .browser
            .evaluate(
                &tab.id,
                "document.querySelector('input[type=password]').value"
            )
            .await
            .unwrap(),
        "native-canary-482"
    );
    let fields = patched
        .gateway()
        .request("autofill.fields", request.clone())
        .await
        .unwrap();
    let handle = fields["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["kind"] == "password")
        .unwrap()["handle"]
        .clone();
    let fill = json!({"tab_id":gateway_tab.tab_id,"handle":handle,"origin":origin,"value":"native-canary-482"});
    assert_eq!(
        patched
            .gateway()
            .request("autofill.fill", fill.clone())
            .await
            .unwrap(),
        json!({"filled":true})
    );
    let replay = patched
        .gateway()
        .request("autofill.fill", fill)
        .await
        .unwrap_err();
    assert!(!replay.to_string().contains("native-canary-482"));
    let fields = patched
        .gateway()
        .request("autofill.fields", request)
        .await
        .unwrap();
    let handle = fields["fields"][0]["handle"].clone();
    patched
        .browser
        .navigate("fill-session", &tab.id, &server.page("welcome"))
        .await
        .unwrap();
    assert!(patched.gateway().request("autofill.fill", json!({"tab_id":gateway_tab.tab_id,"handle":handle,"origin":origin,"value":"native-canary-482"})).await.is_err());
    let external = patched
        .gateway()
        .tabs()
        .await
        .unwrap()
        .tabs
        .into_iter()
        .find(|tab| tab.owner != GatewayOwner::Agent)
        .unwrap();
    assert!(patched
        .gateway()
        .request("autofill.fields", json!({"tab_id":external.tab_id}))
        .await
        .is_err());
    eprintln!("patched native autofill: value-free CDP, origin denial, one-use denial, stale-document denial and external-tab denial observed");
    patched.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn popup_dialog_and_download_events_reach_the_gateway() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let server = Server::start().await;
    let mut patched = Patched::launch(&executable, "events-session").await;

    let opener = patched
        .browser
        .create_agent_tab("events-session", &server.page("popup"))
        .await
        .unwrap();
    ready(&patched.browser, &opener).await;
    let opener_tab = patched.gateway_tab(&opener.id).await;
    patched
        .browser
        .evaluate(&opener.id, "document.getElementById('popup').click();true")
        .await
        .unwrap();
    let popup = gateway_event(patched.gateway(), |event| event.kind == "popup.opened").await;
    assert_eq!(popup.payload["opener_tab_id"], opener_tab.tab_id.as_str());
    assert_eq!(popup.payload["blocked"], false);
    assert_eq!(popup.payload["child_tab"]["opened_by"], "external");
    assert_eq!(popup.payload["child_tab"]["owner"], "external");
    let child = popup.payload["child_tab"]["cdp_target_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The popup opened from an agent tab must not have taken focus either.
    event(
        &mut patched.events,
        |event| matches!(event, BrowserEvent::PopupOpened { tab, .. } if tab.id == child),
    )
    .await;
    patched.browser.attach(&child).await.unwrap();
    assert_eq!(
        patched
            .browser
            .evaluate(&child, "document.hasFocus()")
            .await
            .unwrap(),
        false
    );

    let dialog = patched
        .browser
        .create_agent_tab("events-session", &server.page("dialog"))
        .await
        .unwrap();
    ready(&patched.browser, &dialog).await;
    let dialog_tab = patched.gateway_tab(&dialog.id).await;
    let (triggered, ()) = tokio::join!(
        patched
            .browser
            .evaluate(&dialog.id, "document.getElementById('dialog').click();true"),
        async {
            let opened =
                gateway_event(patched.gateway(), |event| event.kind == "dialog.opened").await;
            assert_eq!(opened.tab_id.as_deref(), Some(dialog_tab.tab_id.as_str()));
            assert_eq!(opened.session_id.as_deref(), Some("events-session"));
            assert_eq!(opened.payload["dialog_type"], "alert");
            assert_eq!(opened.payload["message_redacted"], true);
            assert_eq!(opened.payload["origin"], server.url.as_str());
            assert!(!opened.payload.to_string().contains("Local fixture dialog"));
            patched
                .browser
                .handle_dialog(&dialog.id, true, None)
                .await
                .unwrap();
            let closed =
                gateway_event(patched.gateway(), |event| event.kind == "dialog.closed").await;
            assert_eq!(closed.payload["dialog_id"], opened.payload["dialog_id"]);
            assert_eq!(closed.payload["accepted"], true);
        }
    );
    triggered.unwrap();

    let download = patched
        .browser
        .create_agent_tab("events-session", &server.page("download"))
        .await
        .unwrap();
    ready(&patched.browser, &download).await;
    let download_tab = patched.gateway_tab(&download.id).await;
    patched
        .browser
        .evaluate(
            &download.id,
            "document.getElementById('download').click();true",
        )
        .await
        .unwrap();
    let started_event =
        gateway_event(patched.gateway(), |event| event.kind == "download.started").await;
    assert_eq!(
        started_event.tab_id.as_deref(),
        Some(download_tab.tab_id.as_str())
    );
    assert_eq!(started_event.payload["suggested_filename_redacted"], true);
    assert!(!started_event.payload.to_string().contains("fixture.txt"));
    let download_id = started_event.payload["download_id"].clone();
    let finished = gateway_event(patched.gateway(), |event| {
        event.kind == "download.updated" && event.payload["download_id"] == download_id
    })
    .await;
    assert_eq!(finished.payload["state"], "complete");
    event(&mut patched.events, |event| {
        matches!(event, BrowserEvent::DownloadFinished { .. })
    })
    .await;
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: popup/dialog/download events {:?}",
        started.elapsed()
    );
}

/// Live processes whose command line names `needle` (the test's unique profile path).
fn processes_mentioning(needle: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline"))
                .map(|cmdline| String::from_utf8_lossy(&cmdline).contains(needle))
                .unwrap_or(false)
        })
        .collect()
}

/// A detached launch (the daemon's mode) whose gateway fails after Chromium started must not
/// leave an unowned browser or gateway files behind.
#[tokio::test]
async fn failed_gateway_launch_kills_a_detached_browser() {
    let _serial = serial().await;
    let started = Instant::now();
    let mut cases = Vec::new();
    if let Some(stock) = common::browser_config() {
        // Stock Chromium never publishes a gateway, so a required one fails after spawn.
        cases.push(("stock", stock.executable, None));
    }
    if let Some(patched) = patched_executable() {
        // A shared `openside` directory makes the patched browser refuse to start its gateway.
        cases.push(("patched", patched, Some(0o755)));
    }
    for (name, executable, shared_mode) in cases {
        let runtime = private_runtime_dir();
        if let Some(mode) = shared_mode {
            let shared = runtime.path().join("openside");
            std::fs::create_dir(&shared).unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let profile = tempfile::tempdir().unwrap();
        let mut gateway = GatewayConfig::new("fault-session");
        gateway.runtime_dir = Some(runtime.path().to_owned());
        gateway.discovery_timeout = Duration::from_millis(500);
        let config = BrowserConfig {
            gateway: Some(gateway),
            profile_dir: Some(profile.path().join("browser-profile")),
            detached: true,
            ..BrowserConfig::new(&executable)
        };
        let needle = profile.path().display().to_string();
        let Err(error) = Browser::launch(config).await else {
            panic!("{name}: launch without a usable gateway succeeded");
        };
        assert!(
            matches!(error, openside_browser::BrowserError::InvalidEndpoint(_)),
            "{name}: {error:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let alive = processes_mentioning(&needle);
            if alive.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{name}: browser processes survived a failed launch: {alive:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let browser_dir = runtime.path().join("openside/browser");
        let leftovers = std::fs::read_dir(&browser_dir)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "{name}: gateway files survived");
    }
    eprintln!(
        "patched timing: failed gateway launch cleanup {:?}",
        started.elapsed()
    );
}

#[test]
fn guarded_build_accepts_only_ninja_target_names() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../browser/guarded-build.sh");
    // A missing source tree guarantees nothing is built even if validation regressed; the
    // message tells the two failures apart.
    let missing = tempfile::tempdir().unwrap().path().join("missing-src");
    for args in [
        &["chrome", "-j99"][..],
        &["-C", "elsewhere"][..],
        &["--jobs=99"][..],
        &["chrome;true"][..],
    ] {
        let output = std::process::Command::new("bash")
            .arg(&script)
            .args(args)
            .env_remove("OPENSIDE_BROWSER_BUILD_DIR")
            .env("OPENSIDE_CHROMIUM_SRC", &missing)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains("only target names are accepted"),
            "{args:?}: {stderr}"
        );
    }
}

fn dom_node_with_id<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if node["attributes"].as_array().is_some_and(|attributes| {
        attributes
            .windows(2)
            .any(|pair| pair[0] == "id" && pair[1] == id)
    }) {
        return Some(node);
    }
    match node {
        Value::Object(values) => values
            .values()
            .find_map(|value| dom_node_with_id(value, id)),
        Value::Array(values) => values.iter().find_map(|value| dom_node_with_id(value, id)),
        _ => None,
    }
}

#[tokio::test]
async fn native_autofill_refuses_same_renderer_cross_origin_iframe_nodes() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let child_app = Router::new().route("/", get(|| async {
        ([(axum::http::header::HeaderName::from_static("origin-agent-cluster"), "?0")],
         Html("<!doctype html><title>Foreign field</title><form><input id=cross-origin-password type=password aria-label='Cross-origin Password'></form>"))
    }));
    let child_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let child_url = format!(
        "http://localhost:{}/",
        child_listener.local_addr().unwrap().port()
    );
    let child_task =
        tokio::spawn(async move { axum::serve(child_listener, child_app).await.unwrap() });
    let _child = Probe {
        url: child_url.clone(),
        task: child_task,
    };
    let html = format!("<!doctype html><title>Main frame</title><form><input type=password aria-label='Main Password'></form><iframe id=cross-origin-frame src='{child_url}' onload=\"this.dataset.ready='true'\"></iframe>");
    let main_app = Router::new().route(
        "/",
        get(move || {
            let html = html.clone();
            async move {
                (
                    [(
                        axum::http::header::HeaderName::from_static("origin-agent-cluster"),
                        "?0",
                    )],
                    Html(html),
                )
            }
        }),
    );
    let main_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let main_url = format!(
        "http://localhost:{}/",
        main_listener.local_addr().unwrap().port()
    );
    assert_ne!(main_url, child_url);
    let main_task =
        tokio::spawn(async move { axum::serve(main_listener, main_app).await.unwrap() });
    let _main = Probe {
        url: main_url.clone(),
        task: main_task,
    };
    let patched = Patched::launch(&executable, "iframe-fill-session").await;
    let tab = patched
        .browser
        .create_agent_tab("iframe-fill-session", &main_url)
        .await
        .unwrap();
    let gateway_tab = patched.gateway_tab(&tab.id).await;
    let mut inspector = RawCdp::connect(&patched.browser).await;
    let session = inspector.attach(&tab.id).await;
    inspector
        .wait_until(
            &session,
            "document.querySelector('iframe')?.dataset.ready === 'true'",
        )
        .await;
    assert_eq!(
        inspector
            .eval(
                &session,
                "document.querySelector('iframe').contentDocument === null"
            )
            .await,
        true
    );
    let document = inspector
        .call(
            Some(&session),
            "DOM.getDocument",
            json!({"depth":-1,"pierce":true}),
        )
        .await;
    let iframe = dom_node_with_id(&document, "cross-origin-frame").unwrap();
    // InspectorDOMAgent includes contentDocument only for a local, same-renderer frame.
    let local_document = iframe
        .get("contentDocument")
        .expect("fixture must actually share the main renderer");
    let field = dom_node_with_id(local_document, "cross-origin-password").unwrap();
    let backend = field["backendNodeId"].as_u64().unwrap();
    let targets = inspector.call(None, "Target.getTargets", json!({})).await;
    assert!(!targets["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["type"] == "iframe" && target["url"] == child_url));
    let extraction = patched
        .gateway()
        .request(
            "autofill.fields",
            json!({"tab_id":gateway_tab.tab_id,"backend_node_id":backend}),
        )
        .await
        .unwrap_err();
    assert!(!extraction.to_string().contains("iframe-secret-canary"));
    assert!(patched.gateway().request("autofill.fill", json!({"tab_id":gateway_tab.tab_id,"handle":"unminted-foreign-handle","origin":main_url.trim_end_matches('/'),"value":"iframe-secret-canary"})).await.is_err());
    let resolved = inspector
        .call(
            Some(&session),
            "DOM.resolveNode",
            json!({"backendNodeId":backend}),
        )
        .await;
    let value = inspector.call(Some(&session), "Runtime.callFunctionOn", json!({"objectId":resolved["object"]["objectId"],"functionDeclaration":"function() { return this.value; }","returnByValue":true})).await;
    assert_eq!(value["result"]["value"], "");
    eprintln!("patched iframe autofill: cross-origin field in verified shared renderer refused; canary absent");
    patched.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn native_autofill_handles_stay_revoked_after_bfcache_restore() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let app = Router::new()
        .route("/a", get(|| async { Html("<!doctype html><title>Cacheable form</title><form><input type=password aria-label='Cached Password'></form><script>window.restored=false;window.nonce=Math.random();addEventListener('pageshow',e=>window.restored=e.persisted)</script>") }))
        .route("/b", get(|| async { Html("<!doctype html><title>Away</title>Away") }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let _server = Probe {
        url: origin.clone(),
        task,
    };
    let patched = Patched::launch(&executable, "bfcache-fill-session").await;
    let tab = patched
        .browser
        .create_agent_tab("bfcache-fill-session", &format!("{origin}/a"))
        .await
        .unwrap();
    ready(&patched.browser, &tab).await;
    let gateway_tab = patched.gateway_tab(&tab.id).await;
    let snapshot = patched.browser.snapshot(&tab.id).await.unwrap();
    let reference = snapshot
        .nodes
        .iter()
        .find(|node| node.name == "Cached Password")
        .unwrap()
        .reference
        .as_deref()
        .unwrap();
    let node = patched
        .browser
        .resolve_ref("bfcache-fill-session", &tab.id, reference)
        .await
        .unwrap();
    let fields = patched
        .gateway()
        .request(
            "autofill.fields",
            json!({"tab_id":gateway_tab.tab_id,"backend_node_id":node.backend_node_id}),
        )
        .await
        .unwrap();
    let handle = fields["fields"][0]["handle"].clone();
    let mut inspector = RawCdp::connect(&patched.browser).await;
    let session = inspector.attach(&tab.id).await;
    let nonce = inspector.eval(&session, "window.nonce").await;
    let history = inspector
        .call(Some(&session), "Page.getNavigationHistory", json!({}))
        .await;
    let entry =
        history["entries"][history["currentIndex"].as_u64().unwrap() as usize]["id"].clone();
    patched
        .browser
        .navigate("bfcache-fill-session", &tab.id, &format!("{origin}/b"))
        .await
        .unwrap();
    inspector
        .call(
            Some(&session),
            "Page.navigateToHistoryEntry",
            json!({"entryId":entry}),
        )
        .await;
    inspector
        .wait_until(
            &session,
            "location.pathname === '/a' && window.restored === true",
        )
        .await;
    assert_eq!(inspector.eval(&session, "window.nonce").await, nonce);
    let rejected = patched.gateway().request("autofill.fill", json!({"tab_id":gateway_tab.tab_id,"handle":handle,"origin":origin,"value":"bfcache-secret-canary"})).await.unwrap_err();
    assert!(!rejected.to_string().contains("bfcache-secret-canary"));
    assert_eq!(
        inspector
            .eval(&session, "document.querySelector('input').value")
            .await,
        ""
    );
    eprintln!("patched autofill: actual BFCache restoration retained document nonce but original handle stayed revoked");
    patched.browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn browser_status_never_returns_sensitive_keywords_or_other_profile_state() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let server = probe_server().await;
    let profile = TempDir::new().unwrap();
    let args = [format!(
        "--openside-daemon-ui-url={}/",
        server.url.trim_end_matches("/probe")
    )];
    let first = Patched::launch_profile(
        &executable,
        "profile-status-session",
        None,
        &args,
        Some(profile.path()),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = first
                .gateway()
                .request("browser.status", json!({}))
                .await
                .unwrap();
            if status["ui"] == json!({"ask":true,"routines":true}) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    first.browser.shutdown().await.unwrap();
    let default = profile.path().join("Default");
    let db = rusqlite::Connection::open(default.join("Web Data")).unwrap();
    assert_eq!(
        db.execute(
            "UPDATE keywords SET url=? WHERE keyword='ask'",
            ["http://127.0.0.1/?token=status-primary-canary"]
        )
        .unwrap(),
        1
    );
    let other_keywords: Vec<(String, String)> = db
        .prepare(
            "SELECT keyword,url FROM keywords WHERE keyword NOT IN ('ask','routines') ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!other_keywords.is_empty());
    drop(db);
    let prefs_path = default.join("Preferences");
    let mut prefs: Value =
        serde_json::from_str(&std::fs::read_to_string(&prefs_path).unwrap()).unwrap();
    prefs["vertical_tabs"]["enabled"] = json!(false);
    std::fs::write(&prefs_path, serde_json::to_vec(&prefs).unwrap()).unwrap();
    let other = profile.path().join("Other");
    std::fs::create_dir(&other).unwrap();
    std::fs::copy(default.join("Web Data"), other.join("Web Data")).unwrap();
    prefs["vertical_tabs"]["enabled"] = json!(true);
    let second_url = server.url.replace("/probe", "/child");
    prefs["session"]["restore_on_startup"] = json!(4);
    prefs["session"]["startup_urls"] = json!([second_url]);
    std::fs::write(
        other.join("Preferences"),
        serde_json::to_vec(&prefs).unwrap(),
    )
    .unwrap();
    let db = rusqlite::Connection::open(other.join("Web Data")).unwrap();
    assert_eq!(
        db.execute(
            "UPDATE keywords SET url=? WHERE keyword='ask'",
            ["http://127.0.0.1/?token=status-other-canary"]
        )
        .unwrap(),
        1
    );
    drop(db);
    let state_path = profile.path().join("Local State");
    let mut state: Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    let mut attributes = state["profile"]["info_cache"]["Default"].clone();
    assert!(attributes.is_object());
    attributes["name"] = json!("Secondary fixture");
    state["profile"]["info_cache"]["Other"] = attributes;
    state["profile"]["last_used"] = json!("Default");
    state["profile"]["last_active_profiles"] = json!(["Default", "Other"]);
    state["profile"]["show_picker_on_startup"] = json!(false);
    std::fs::write(state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let patched = Patched::launch_profile(
        &executable,
        "profile-status-session",
        None,
        &args,
        Some(profile.path()),
    )
    .await;
    let mut inspector = RawCdp::connect(&patched.browser).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let targets = inspector.call(None, "Target.getTargets", json!({})).await;
            if targets["targetInfos"]
                .as_array()
                .unwrap()
                .iter()
                .any(|target| target["url"] == second_url)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let status = patched
        .gateway()
        .request("browser.status", json!({}))
        .await
        .unwrap();
    assert_eq!(status["ui"], json!({"ask":false,"routines":true}));
    assert_eq!(
        status["ux"]["vertical_tabs"], false,
        "second profile must not change primary-profile status"
    );
    assert!(!status.to_string().contains("canary"));
    patched.browser.shutdown().await.unwrap();
    let db = rusqlite::Connection::open(default.join("Web Data")).unwrap();
    let after: Vec<(String, String)> = db
        .prepare(
            "SELECT keyword,url FROM keywords WHERE keyword NOT IN ('ask','routines') ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(after, other_keywords);
    assert_eq!(
        db.query_row("SELECT url FROM keywords WHERE keyword='ask'", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        "http://127.0.0.1/?token=status-primary-canary"
    );
    drop(db);
    let mut prefs: Value =
        serde_json::from_str(&std::fs::read_to_string(&prefs_path).unwrap()).unwrap();
    prefs["vertical_tabs"]
        .as_object_mut()
        .unwrap()
        .remove("enabled");
    std::fs::write(&prefs_path, serde_json::to_vec(&prefs).unwrap()).unwrap();
    let inherited = Patched::launch_profile(
        &executable,
        "profile-status-session",
        None,
        &args,
        Some(profile.path()),
    )
    .await;
    assert_eq!(
        inherited
            .gateway()
            .request("browser.status", json!({}))
            .await
            .unwrap()["ux"]["vertical_tabs"],
        true
    );
    inherited.browser.shutdown().await.unwrap();
    eprintln!("patched status: existing sensitive keyword preserved but never returned; live second profile excluded; explicit/default prefs respected");
}

#[tokio::test]
async fn daemon_ui_rejects_credentials_query_and_nonroot_switch_values() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let server = probe_server().await;
    let base = server.url.trim_end_matches("/probe");
    for invalid in [
        format!("{base}/?token=invalid-ui-canary"),
        format!("{base}/not-the-root"),
        base.replacen("http://", "http://invalid-ui-canary@", 1) + "/",
    ] {
        let patched = Patched::launch_args(
            &executable,
            "invalid-ui-session",
            None,
            &[format!("--openside-daemon-ui-url={invalid}")],
        )
        .await;
        let tab = patched
            .browser
            .create_agent_tab("invalid-ui-session", "chrome://newtab/")
            .await
            .unwrap();
        let mut inspector = RawCdp::connect(&patched.browser).await;
        let session = inspector.attach(&tab.id).await;
        inspector
            .wait_until(&session, "document.readyState === 'complete'")
            .await;
        assert!(inspector
            .eval(&session, "location.href")
            .await
            .as_str()
            .unwrap()
            .starts_with("chrome://"));
        let status = patched
            .gateway()
            .request("browser.status", json!({}))
            .await
            .unwrap();
        assert_eq!(status["ui"], json!({"ask":false,"routines":false}));
        assert!(!status.to_string().contains("invalid-ui-canary"));
        patched.browser.shutdown().await.unwrap();
    }
    eprintln!(
        "patched daemon UI: credential-bearing, query-bearing and non-root switch values refused"
    );
}

/// Starts the patched browser outside `Browser::launch` with an explicit environment, so the
/// runtime-directory source is the only variable.
fn raw_gateway_browser(
    executable: &Path,
    instance: &str,
    switch: Option<&Path>,
    xdg_runtime_dir: &Path,
    profile: &Path,
) -> tokio::process::Child {
    let mut command = tokio::process::Command::new(executable);
    command
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--password-store=basic",
            "--disable-extensions",
            "--disable-sync",
            "--remote-debugging-port=0",
            "--openside-agent-session-id=runtime-session",
            "about:blank",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--openside-gateway-instance-id={instance}"))
        .env("XDG_RUNTIME_DIR", xdg_runtime_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    common::supervise(&mut command);
    if let Some(switch) = switch {
        command.arg(format!(
            "--openside-gateway-runtime-dir={}",
            switch.display()
        ));
    }
    command.spawn().unwrap()
}

async fn stop_raw_browser(mut child: tokio::process::Child) {
    let group = nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap());
    let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
    let _ = child.wait().await;
}

/// Native-messaging hosts inherit the browser's environment, so the gateway's directory
/// travels as a switch and the browser keeps the user's XDG_RUNTIME_DIR. The switch wins
/// over the environment and never falls back to it when it names an unusable directory.
#[tokio::test]
async fn gateway_runtime_dir_comes_from_the_switch_not_the_environment() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let user_runtime = private_runtime_dir();
    let gateway_runtime = private_runtime_dir();
    let profile = tempfile::tempdir().unwrap();
    let instance = "runtime-switch-0001";
    let child = raw_gateway_browser(
        &executable,
        instance,
        Some(gateway_runtime.path()),
        user_runtime.path(),
        profile.path(),
    );
    let discovery = gateway_runtime
        .path()
        .join("openside/browser")
        .join(instance)
        .join("gateway.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !discovery.exists() {
        assert!(
            Instant::now() < deadline,
            "no gateway in the switch directory; user runtime holds {:?}",
            walk(user_runtime.path())
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !user_runtime.path().join("openside").exists(),
        "the gateway also wrote into XDG_RUNTIME_DIR"
    );
    stop_raw_browser(child).await;

    // A shared, sticky, symlinked, empty, relative or overlong switch value is refused
    // outright, even with a private XDG_RUNTIME_DIR available, and nothing is created.
    let shared = tempfile::tempdir().unwrap();
    std::fs::set_permissions(shared.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let sticky = tempfile::tempdir().unwrap();
    std::fs::set_permissions(sticky.path(), std::fs::Permissions::from_mode(0o1700)).unwrap();
    let target = private_runtime_dir();
    let links = tempfile::tempdir().unwrap();
    let symlink = links.path().join("runtime");
    std::os::unix::fs::symlink(target.path(), &symlink).unwrap();
    let long_root = private_runtime_dir();
    let overlong = long_root.path().join("r".repeat(64));
    std::fs::create_dir(&overlong).unwrap();
    std::fs::set_permissions(&overlong, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (name, switch) in [
        ("shared", shared.path().to_owned()),
        ("sticky", sticky.path().to_owned()),
        ("symlink", symlink.clone()),
        ("empty", PathBuf::new()),
        ("relative", PathBuf::from("relative-runtime")),
        ("overlong", overlong.clone()),
    ] {
        let profile = tempfile::tempdir().unwrap();
        let child = raw_gateway_browser(
            &executable,
            instance,
            Some(&switch),
            user_runtime.path(),
            profile.path(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while !profile.path().join("DevToolsActivePort").exists() {
            assert!(Instant::now() < deadline, "{name}: browser never started");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // The valid switch above publishes well within this window of a running browser.
        tokio::time::sleep(Duration::from_secs(1)).await;
        for root in [
            shared.path(),
            sticky.path(),
            target.path(),
            overlong.as_path(),
            user_runtime.path(),
        ] {
            assert!(
                !root.join("openside").exists(),
                "{name}: gateway started in {}",
                root.display()
            );
        }
        stop_raw_browser(child).await;
    }
    eprintln!("patched timing: runtime dir switch {:?}", started.elapsed());
}

/// Tiling and Wayland compositors own the toplevel size: a maximized dedicated agent window
/// never reaches 1440x900, so the page is emulated at 1440x900 inside it and the native
/// capture succeeds. The user's own window in the same browser keeps its real size.
#[tokio::test]
async fn maximized_agent_window_is_emulated_at_1440x900() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch_args(
        &executable,
        "maximized-session",
        None,
        &["--start-maximized".to_owned()],
    )
    .await;
    let mut raw = RawCdp::connect(&patched.browser).await;
    let user_target = patched
        .browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.opened_by == OpenedBy::External)
        .expect("initial browser tab")
        .id;
    let (windowed, windowed_session) = raw.open(&probe, true).await;
    let window = raw
        .call(
            None,
            "Browser.getWindowForTarget",
            json!({"targetId":windowed}),
        )
        .await;
    assert_eq!(
        window["bounds"]["windowState"], "maximized",
        "precondition: the agent window must be maximized: {window}"
    );
    raw.wait_until(&windowed_session, "document.readyState === 'complete'")
        .await;
    raw.wait_until(
        &windowed_session,
        "innerWidth === 1440 && innerHeight === 900",
    )
    .await;
    let tab = patched.gateway_tab(&windowed).await;
    let shot = patched.gateway().screenshot(&tab.tab_id).await.unwrap();
    assert_eq!((shot.width, shot.height), (1440, 900));
    assert_eq!(png_size(&shot.png), (1440, 900));

    let user_session = raw.attach(&user_target).await;
    let user_window = raw
        .call(
            None,
            "Browser.getWindowForTarget",
            json!({"targetId":user_target}),
        )
        .await;
    let user_size = raw.eval(&user_session, "[innerWidth, innerHeight]").await;
    assert_ne!(
        user_size,
        json!([1440, 900]),
        "the user's window was emulated too ({user_window})"
    );
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: maximized agent window emulation {:?} (agent window {}, user page {user_size})",
        started.elapsed(),
        window["bounds"]
    );
}

/// Window id shared by CDP and the extension APIs (both use the session id).
async fn window_of(raw: &mut RawCdp, target: &str) -> i64 {
    raw.call(
        None,
        "Browser.getWindowForTarget",
        json!({"targetId":target}),
    )
    .await["windowId"]
        .as_i64()
        .unwrap()
}

/// Activation authority follows the requesting actor, not only the target window.
/// Extensions act for the user: a window holding only agent tabs is never raised, resized,
/// put into another state, or flashed by them, while a window that holds a user tab (agent
/// tabs in it or not) is still theirs to manage. DevTools clients are the agent in this
/// browser: they activate no tab and change no window, the user's included. Native
/// window-manager activation by the user is not covered by any automated test.
#[tokio::test]
async fn window_activation_follows_the_requesting_actor() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let extension = tabs_extension();
    let patched = Patched::launch_with(
        &executable,
        "window-focus-session",
        None,
        &[],
        None,
        vec![extension.path().to_owned()],
    )
    .await;
    let mut raw = RawCdp::connect(&patched.browser).await;
    let user_target = patched
        .browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.opened_by == OpenedBy::External)
        .expect("initial browser tab")
        .id;
    let (agent, agent_session) = raw.open(&probe, true).await;
    // A background agent tab in the user's window makes that window mixed.
    let (_hidden, hidden_session) = raw.open(&probe, false).await;
    for session in [&agent_session, &hidden_session] {
        raw.wait_until(session, "document.readyState === 'complete'")
            .await;
    }
    let agent_tab = patched.gateway_tab(&agent).await;
    let user_tab = patched.gateway_tab(&user_target).await;
    let agent_window = window_of(&mut raw, &agent).await;
    let user_window = window_of(&mut raw, &user_target).await;
    assert_ne!(agent_window, user_window);
    let worker = extension_worker(&mut raw).await;

    // Extension requests against the window of agent tabs: refused before any of the update
    // runs, and reported.
    for (what, update) in [
        ("focused", "{focused: true}"),
        ("state", "{state: 'maximized'}"),
        ("bounds", "{width: 800, height: 600}"),
        ("drawAttention", "{drawAttention: true}"),
    ] {
        let reply = in_extension(
            &mut raw,
            &worker,
            &format!("await chrome.windows.update({agent_window}, {update}); return 'applied';"),
        )
        .await;
        assert!(
            reply["error"]
                .as_str()
                .is_some_and(|message| message.contains("openside:")),
            "windows.update {what} on the agent window: {reply}"
        );
        let denied = gateway_event(patched.gateway(), |event| {
            event.kind == "focus.decided" && event.tab_id.as_deref() == Some(&agent_tab.tab_id)
        })
        .await;
        assert_eq!(denied.payload["allowed"], false, "{what}");
    }
    let state = in_extension(
        &mut raw,
        &worker,
        &format!(
            "const w = await chrome.windows.get({agent_window}); return [w.state, w.focused];"
        ),
    )
    .await;
    assert_eq!(state, json!(["normal", false]));
    assert_eq!(raw.eval(&agent_session, "document.hasFocus()").await, false);

    // The mixed window holds the user's tab: the extension still manages it for the user.
    let mixed = in_extension(
        &mut raw,
        &worker,
        &format!(
            "await chrome.windows.update({user_window}, {{width: 900, height: 700, focused: true}}); \
             return (await chrome.windows.getLastFocused()).id === {user_window};"
        ),
    )
    .await;
    assert_eq!(
        mixed, true,
        "the extension lost control of the user's window"
    );

    // The agent, over CDP, neither changes the user's (mixed) window nor activates the
    // user's tab.
    let message = raw
        .refused(
            None,
            "Browser.setWindowBounds",
            json!({"windowId":user_window,"bounds":{"width":1000,"height":800}}),
        )
        .await;
    assert!(message.contains("never resizes"), "{message}");
    raw.call(
        None,
        "Target.activateTarget",
        json!({"targetId":user_target}),
    )
    .await;
    let denied = gateway_event(patched.gateway(), |event| {
        event.kind == "focus.decided" && event.tab_id.as_deref() == Some(&user_tab.tab_id)
    })
    .await;
    assert_eq!(denied.payload["allowed"], false);

    // A fresh window the extension opens for the user is activated as usual.
    let fresh = in_extension(
        &mut raw,
        &worker,
        "const created = await chrome.windows.create({url: 'about:blank', focused: true}); \
         return (await chrome.windows.getLastFocused()).id === created.id;",
    )
    .await;
    assert_eq!(
        fresh, true,
        "an extension window for the user was not activated"
    );

    // Moving the agent tab into a new focused window: shown inactive, and reported.
    let moved = in_extension(
        &mut raw,
        &worker,
        &format!(
            "const [tab] = await chrome.tabs.query({{windowId: {agent_window}}}); \
             const created = await chrome.windows.create({{tabId: tab.id, focused: true}}); \
             return (await chrome.windows.get(created.id)).focused;"
        ),
    )
    .await;
    assert_eq!(
        moved, false,
        "windows.create focused the agent tab's new window"
    );
    gateway_event(patched.gateway(), |event| {
        event.kind == "focus.decided" && event.tab_id.as_deref() == Some(&agent_tab.tab_id)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(raw.eval(&agent_session, "document.hasFocus()").await, false);
    assert_eq!(patched.gateway_tab(&agent).await.owner, GatewayOwner::Agent);
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: window activation by actor {:?}",
        started.elapsed()
    );
}

/// A DevTools session that leaves resets the page-wide device metrics. With the window already
/// at 1440x900 that reset changes only the scale factor and mobile mode, so neither a resize nor
/// a capture announces it; the browser must notice the departing session itself.
#[tokio::test]
async fn agent_viewport_is_restored_when_another_client_leaves_without_a_resize() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    // A 2x screen makes a page that lost the override visibly different at the same size.
    let patched = Patched::launch_args(
        &executable,
        "detach-session",
        None,
        &["--force-device-scale-factor=2".to_owned()],
    )
    .await;
    let mut raw = RawCdp::connect(&patched.browser).await;
    let (target, session) = raw.open(&probe, true).await;
    let laid_out = "typeof window.frames_drawn === 'number' && innerWidth === 1440 \
                    && innerHeight === 900 && devicePixelRatio === 1";
    raw.wait_until(&session, laid_out).await;
    assert_eq!(
        patched.gateway_tab(&target).await.owner,
        GatewayOwner::Agent
    );

    for leave_by_closing in [false, true] {
        let mut other = RawCdp::connect(&patched.browser).await;
        let other_session = other.attach(&target).await;
        other
            .call(
                Some(&other_session),
                "Emulation.setDeviceMetricsOverride",
                json!({"width":1440,"height":900,"deviceScaleFactor":3,"mobile":true}),
            )
            .await;
        // Precondition: the other session's override really reached the page.
        other
            .wait_until(&other_session, "devicePixelRatio === 3")
            .await;
        if leave_by_closing {
            drop(other);
        } else {
            other
                .call(
                    None,
                    "Target.detachFromTarget",
                    json!({"sessionId":other_session}),
                )
                .await;
        }
        raw.wait_until(&session, laid_out).await;
    }
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: viewport after another client leaves {:?}",
        started.elapsed()
    );
}

/// A renderer whose main thread is busy answers no protocol command, so the viewport override
/// is never acknowledged. Captures must fail within their deadline and give their slots back,
/// and the same connection must capture again once the page is responsive.
#[tokio::test]
async fn capture_slots_come_back_when_the_renderer_stops_answering() {
    let _serial = serial().await;
    let Some(executable) = patched_executable() else {
        return;
    };
    let started = Instant::now();
    let probe = probe_server().await;
    let patched = Patched::launch(&executable, "hang-session").await;
    let mut raw = RawCdp::connect(&patched.browser).await;
    let (target, session) = raw.open(&probe, false).await;
    raw.wait_until(
        &session,
        "typeof window.frames_drawn === 'number' && innerWidth === 1440 && innerHeight === 900",
    )
    .await;
    let tab = patched.gateway_tab(&target).await;
    let gateway = patched.gateway();
    let code = |result: Result<GatewayScreenshot, BrowserError>| match result {
        Err(BrowserError::Gateway { code, .. }) => code,
        other => panic!("expected a gateway error, got {other:?}"),
    };

    raw.send(
        Some(&session),
        "Runtime.evaluate",
        json!({"expression":"for (;;) {}"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let hung = Instant::now();
    let (first, second, third) = tokio::join!(
        gateway.screenshot(&tab.tab_id),
        gateway.screenshot(&tab.tab_id),
        async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            gateway.screenshot(&tab.tab_id).await
        }
    );
    assert_eq!(code(third), "resource_exhausted");
    for result in [first, second] {
        let code = code(result);
        assert!(
            code == "surface_unavailable" || code == "timeout",
            "unexpected capture failure {code}"
        );
    }
    assert!(
        hung.elapsed() < Duration::from_secs(10),
        "captures outlived their deadline: {:?}",
        hung.elapsed()
    );
    // Both slots are free again on the same connection, while the page still hangs.
    assert_ne!(
        code(gateway.screenshot(&tab.tab_id).await),
        "resource_exhausted"
    );

    raw.reply(Some(&session), "Runtime.terminateExecution", json!({}))
        .await;
    raw.wait_until(&session, "innerWidth === 1440 && innerHeight === 900")
        .await;
    let shot = gateway.screenshot(&tab.tab_id).await.unwrap();
    assert_eq!((shot.width, shot.height), (1440, 900));
    drop(raw);
    patched.browser.shutdown().await.unwrap();
    eprintln!(
        "patched timing: capture after a hung renderer {:?}",
        started.elapsed()
    );
}
