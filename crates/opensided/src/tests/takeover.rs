//! HS-6 remote takeover against real Chromium: frame stream, input forwarding to the
//! ticket's tab, the agent write lock and its ordering against in-flight agent work and
//! terminal transitions, secret withholding, stalled peers, scope/origin/single-controller
//! checks, expiry, revocation, and the post-takeover result of the original call.

use super::*;
use axum::{response::Html, routing::get, Router};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const PAGE: &str = r#"<!doctype html><html><head><title>Takeover fixture</title><style>
body{margin:0} #go{position:fixed;left:0;top:0;width:400px;height:200px}
#name{position:fixed;left:0;top:300px;width:400px;height:60px} #out{position:fixed;left:0;top:420px}
</style></head><body><button id="go" onclick="document.title='Clicked by human';out.textContent='Human clicked'">Go</button>
<input id="name" aria-label="Name" autocomplete="off"><div id="out">Waiting</div></body></html>"#;

/// Repaints the whole viewport with incompressible pixels so screencast frames are large and
/// never stop coming: a peer that does not read fills its socket within seconds.
const NOISE: &str = r#"<!doctype html><html><body style="margin:0"><canvas id="c" width="1440" height="900"></canvas><script>
const c=document.getElementById('c').getContext('2d');const img=c.createImageData(1440,900);
setInterval(()=>{const d=new Uint32Array(img.data.buffer);for(let i=0;i<d.length;i++)d[i]=(Math.random()*0xffffffff)|0xff000000;c.putImageData(img,0,0)},30);
</script></body></html>"#;

struct BrowserCleanup(u32);
impl Drop for BrowserCleanup {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

struct Fixture {
    daemon: Daemon,
    origin: String,
    runtime: TestRuntime,
    session: SessionId,
    tab: String,
    tcp: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    _cleanup: BrowserCleanup,
    _root: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn fixture(takeover_ttl_secs: u32) -> Option<Fixture> {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    match test_browser() {
        Ok(executable) => config.loaded.config.browser.executable = Some(executable),
        Err(error) => {
            eprintln!("SKIP takeover browser tests: {error}");
            return None;
        }
    }
    config.loaded.config.tickets.takeover_ttl_secs = takeover_ttl_secs;
    let site = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", site.local_addr().unwrap().port());
    let site_origin = origin.clone();
    let site_task = tokio::spawn(async move {
        let site_routes = Router::new()
            .route("/", get(|| async { Html(PAGE) }))
            .route(
                "/clean",
                get(|| async { Html("<!doctype html><title>Clean</title><p>Clean page</p>") }),
            )
            .route("/noise", get(|| async { Html(NOISE) }));
        axum::serve(site, site_routes).await.unwrap();
    });
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "takeover", PolicyMode::Full, None).await;
    let tab = model(
        host(&daemon, &session)
            .await
            .call(call("browser_open_tab", json!({"url":site_origin}), "open"))
            .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let pid = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap()
        .process_id();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp = listener.local_addr().unwrap().to_string();
    let router = api::router(daemon.shared.clone(), false);
    let api_task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Some(Fixture {
        daemon,
        origin,
        runtime,
        session,
        tab,
        tcp,
        tasks: vec![site_task, api_task],
        _cleanup: BrowserCleanup(pid),
        _root: root,
    })
}

fn model(outcome: ToolOutcome) -> Value {
    let (ToolOutcome::Done { for_model_json } | ToolOutcome::DoneWithParts { for_model_json, .. }) =
        outcome
    else {
        panic!("expected completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

async fn takeover(f: &Fixture, id: &str) -> TicketId {
    let ToolOutcome::Deferred { ticket, .. } = host(&f.daemon, &f.session)
        .await
        .call(call(
            "request_human_step",
            json!({"kind":"takeover","summary":"Solve the CAPTCHA","tab_id":f.tab}),
            id,
        ))
        .await
    else {
        panic!("expected takeover ticket");
    };
    ticket
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the takeover socket, or return the HTTP status and error code it was refused with.
async fn connect(
    f: &Fixture,
    ticket: &TicketId,
    token: Option<&str>,
    origin: Option<&str>,
) -> Result<Socket, (u16, Value)> {
    let mut request = format!("ws://{}/v1/tickets/{ticket}/takeover", f.tcp)
        .into_client_request()
        .unwrap();
    if let Some(token) = token {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => Ok(socket),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            let body = response
                .body()
                .as_ref()
                .and_then(|body| serde_json::from_slice(body).ok())
                .unwrap_or(Value::Null);
            Err((response.status().as_u16(), body))
        }
        Err(error) => panic!("takeover socket: {error}"),
    }
}

/// The web UI's handshake: no Authorization header (browsers cannot set it), the bearer
/// carried as a subprotocol, and the daemon's own origin. Returns the accepted protocol.
async fn connect_web(f: &Fixture, ticket: &TicketId, token: &str) -> Result<(Socket, String), u16> {
    let mut request = format!("ws://{}/v1/tickets/{ticket}/takeover", f.tcp)
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    headers.insert(
        "sec-websocket-protocol",
        format!("openside.v1, openside.bearer.{token}")
            .parse()
            .unwrap(),
    );
    headers.insert("origin", format!("http://{}", f.tcp).parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, response)) => {
            let protocol = response
                .headers()
                .get("sec-websocket-protocol")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default();
            Ok((socket, protocol))
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(error) => panic!("takeover socket: {error}"),
    }
}

async fn message(socket: &mut Socket) -> TakeoverServerMessage {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(15), socket.next())
            .await
            .expect("takeover message within 15s")
            .expect("socket open")
            .unwrap();
        if let Message::Text(text) = next {
            return serde_json::from_str(text.as_str()).unwrap();
        }
    }
}

async fn ended(socket: &mut Socket) -> TakeoverEndReason {
    loop {
        if let TakeoverServerMessage::Ended { reason } = message(socket).await {
            return reason;
        }
    }
}

async fn send(socket: &mut Socket, input: Value) {
    socket
        .send(Message::Text(input.to_string().into()))
        .await
        .unwrap();
}

async fn page(f: &Fixture, expression: &str) -> Value {
    f.daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap()
        .evaluate(&f.tab, expression)
        .await
        .unwrap()
}

async fn typed(f: &Fixture) -> u64 {
    page(f, "document.querySelector('#name').value.length")
        .await
        .as_u64()
        .unwrap()
}

async fn status(f: &Fixture, ticket: &TicketId) -> TicketStatus {
    crate::tickets::get(&f.daemon.shared, ticket)
        .await
        .unwrap()
        .unwrap()
        .status
}

/// Click the fixture's text field so `text` input lands in it.
async fn focus_name(socket: &mut Socket) {
    for event in ["down", "up"] {
        send(
            socket,
            json!({"type":"mouse","event":event,"x":100,"y":330}),
        )
        .await;
    }
}

/// A WebSocket controller that completes the handshake and then never reads, with a tiny
/// receive buffer so the daemon's writes back up quickly.
async fn stalled_peer(f: &Fixture, ticket: &TicketId, token: &str) -> tokio::net::TcpStream {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut stream = socket.connect(f.tcp.parse().unwrap()).await.unwrap();
    let request = format!(
        "GET /v1/tickets/{ticket}/takeover HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n",
        f.tcp
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
    }
    assert!(
        head.starts_with(b"HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    stream
}

/// Connect a controller as soon as the ticket's previous controller has been released.
async fn reacquire(f: &Fixture, ticket: &TicketId, token: &str, within: Duration) -> Socket {
    let deadline = std::time::Instant::now() + within;
    loop {
        match connect(f, ticket, Some(token), None).await {
            Ok(socket) => return socket,
            Err((409, body))
                if body["code"] == "takeover_in_use" && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("controller not released within {within:?}: {error:?}"),
        }
    }
}

fn refused(outcome: &ToolOutcome) -> bool {
    matches!(outcome, ToolOutcome::Denied { reason, .. } if reason == "takeover_active")
}

/// The subprotocol bearer is a credential only for a genuine WebSocket handshake to an
/// allowlisted WebSocket route, and only when no Authorization header is present.
#[tokio::test]
async fn subprotocol_bearer_counts_only_for_websocket_handshakes_without_authorization() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let (_, token) = daemon
        .shared
        .db
        .pair(vec![
            CapabilityScope::HumanStep,
            CapabilityScope::Read,
            CapabilityScope::Steer,
        ])
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    let protocol = format!("openside.v1, openside.bearer.{token}");
    let handshake = [
        ("connection", "Upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ("sec-websocket-protocol", protocol.as_str()),
    ];
    // The complete handshake with some headers replaced (`Some`) or dropped (`None`).
    let with = |replace: &[(&'static str, Option<&str>)]| -> Vec<(&'static str, String)> {
        let mut headers: Vec<(&'static str, String)> = handshake
            .iter()
            .filter(|(name, _)| !replace.iter().any(|(r, _)| r == name))
            .map(|(name, value)| (*name, (*value).to_owned()))
            .collect();
        headers.extend(
            replace
                .iter()
                .filter_map(|(name, value)| value.map(|v| (*name, v.to_owned()))),
        );
        headers
    };
    let status =
        |method: &'static str, path: &'static str, headers: Vec<(&'static str, String)>| {
            let router = router.clone();
            async move {
                let mut request = Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "127.0.0.1");
                for (name, value) in headers {
                    request = request.header(name, value);
                }
                router
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status()
            }
        };
    const TAKEOVER: &str = "/v1/tickets/missing/takeover";

    // A complete handshake to the takeover route gets past authentication.
    let accepted = status("GET", TAKEOVER, with(&[])).await;
    assert!(
        ![StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN].contains(&accepted),
        "{accepted}"
    );
    let refused = [
        (
            "protocol header alone",
            "GET",
            TAKEOVER,
            vec![("sec-websocket-protocol", protocol.clone())],
        ),
        ("non-WebSocket GET route", "GET", "/v1/sessions", with(&[])),
        (
            "non-WebSocket POST route",
            "POST",
            "/v1/sessions/session/messages",
            with(&[]),
        ),
        (
            "no Connection: upgrade",
            "GET",
            TAKEOVER,
            with(&[("connection", Some("keep-alive"))]),
        ),
        ("no Upgrade", "GET", TAKEOVER, with(&[("upgrade", None)])),
        (
            "old version",
            "GET",
            TAKEOVER,
            with(&[("sec-websocket-version", Some("8"))]),
        ),
        (
            "no key",
            "GET",
            TAKEOVER,
            with(&[("sec-websocket-key", None)]),
        ),
        (
            "short key",
            "GET",
            TAKEOVER,
            with(&[("sec-websocket-key", Some("c2hvcnQ="))]),
        ),
        (
            "malformed Authorization",
            "GET",
            TAKEOVER,
            with(&[("authorization", Some("Basic Zm9vOmJhcg=="))]),
        ),
        (
            "invalid bearer Authorization",
            "GET",
            TAKEOVER,
            with(&[("authorization", Some("Bearer not-a-token"))]),
        ),
    ];
    for (case, method, path, headers) in refused {
        assert_eq!(
            status(method, path, headers).await,
            StatusCode::UNAUTHORIZED,
            "{case}"
        );
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn takeover_streams_frames_forwards_input_locks_agent_and_resumes_with_fresh_page_hs6() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    let before = model(
        host(&f.daemon, &f.session)
            .await
            .call(call("browser_snapshot", json!({"tab_id":f.tab}), "before"))
            .await,
    );
    let before_id = before
        .as_str()
        .and_then(|text| text.strip_prefix("[snapshot_id "))
        .and_then(|text| text.split(']').next())
        .expect("snapshot id header")
        .to_owned();
    let go_ref = before
        .as_str()
        .unwrap()
        .lines()
        .find(|line| line.contains("\"Go\""))
        .and_then(|line| {
            line.split(|c: char| !c.is_ascii_alphanumeric())
                .find(|word| {
                    word.len() > 1 && word.starts_with('e') && word[1..].parse::<u32>().is_ok()
                })
        })
        .expect("Go button ref")
        .to_owned();
    let ticket = takeover(&f, "takeover").await;
    let human = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap()
        .1;
    let read = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap()
        .1;

    // Scope, credential and origin checks happen before any upgrade.
    assert_eq!(connect(&f, &ticket, None, None).await.err().unwrap().0, 401);
    assert_eq!(
        connect(&f, &ticket, Some(&read), None)
            .await
            .err()
            .unwrap()
            .0,
        403
    );
    let foreign = connect(&f, &ticket, Some(&human), Some("http://evil.example")).await;
    assert_eq!(
        foreign.err().unwrap(),
        (
            403,
            json!({"code":"origin_forbidden","message":"origin forbidden"})
        )
    );

    let mut socket = connect(
        &f,
        &ticket,
        Some(&human),
        Some(&format!("http://{}", f.tcp)),
    )
    .await
    .expect("same-origin controller connects");
    let TakeoverServerMessage::Frame {
        data,
        width,
        height,
    } = message(&mut socket).await
    else {
        panic!("first message must be a frame");
    };
    assert_eq!((width, height), (1440.0, 900.0));
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert_eq!(&jpeg[..2], b"\xff\xd8");
    let second = connect(&f, &ticket, Some(&human), None).await;
    assert_eq!(second.err().unwrap().1["code"], "takeover_in_use");

    // Meanwhile every agent tool that touches the tab is refused, reads included.
    for (name, args) in [
        (
            "browser_act",
            json!({"tab_id":f.tab,"target":"e1","action":"click"}),
        ),
        ("browser_snapshot", json!({"tab_id":f.tab})),
        ("browser_screenshot", json!({"tab_id":f.tab})),
        (
            "browser_navigate",
            json!({"tab_id":f.tab,"url":"http://localhost:9/"}),
        ),
        (
            "browser_pointer",
            json!({"tab_id":f.tab,"action":"click","x":100,"y":100}),
        ),
        ("browser_repl", json!({"code":"await page.click('#go')"})),
        ("browser_close_tab", json!({"tab_id":f.tab})),
    ] {
        let outcome = host(&f.daemon, &f.session)
            .await
            .call(call(name, args, name))
            .await;
        assert!(
            refused(&outcome),
            "{name} must be refused during takeover: {outcome:?}"
        );
    }
    assert!(model(
        host(&f.daemon, &f.session)
            .await
            .call(call("browser_tabs", json!({}), "tabs"))
            .await
    )
    .to_string()
    .contains(&f.tab));
    assert_eq!(
        page(&f, "out.textContent").await,
        "Waiting",
        "refused agent calls ran"
    );

    // The person's click and typing reach the page.
    for event in ["down", "up"] {
        send(
            &mut socket,
            json!({"type":"mouse","event":event,"x":100,"y":100}),
        )
        .await;
    }
    for event in ["down", "up"] {
        send(
            &mut socket,
            json!({"type":"mouse","event":event,"x":100,"y":330}),
        )
        .await;
    }
    send(&mut socket, json!({"type":"text","text":"typed by human"})).await;
    send(
        &mut socket,
        json!({"type":"mouse","event":"down","x":1440,"y":10}),
    )
    .await;
    send(&mut socket, json!({"type":"teleport"})).await;
    let mut errors = Vec::new();
    while errors.len() < 2 {
        if let TakeoverServerMessage::InputError { code } = message(&mut socket).await {
            errors.push(code);
        }
    }
    assert_eq!(errors, ["out_of_bounds", "invalid_input"]);
    assert_eq!(page(&f, "out.textContent").await, "Human clicked");
    assert_eq!(
        page(&f, "document.querySelector('#name').value").await,
        "typed by human"
    );

    // Browser shortcuts dispatched to the page do not reach browser chrome: no tab closes or
    // opens, the location does not change and devtools do not take the tab.
    let tabs = || async {
        model(
            host(&f.daemon, &f.session)
                .await
                .call(call("browser_tabs", json!({}), "tabs"))
                .await,
        )
        .to_string()
    };
    let (tabs_before, href) = (tabs().await, page(&f, "location.href").await);
    for (key, code, key_code) in [
        ("w", "KeyW", 87),
        ("t", "KeyT", 84),
        ("l", "KeyL", 76),
        ("Tab", "Tab", 9),
    ] {
        for event in ["down", "up"] {
            send(&mut socket, json!({"type":"key","event":event,"key":key,"code":code,"key_code":key_code,"modifiers":2})).await;
        }
    }
    for event in ["down", "up"] {
        send(
            &mut socket,
            json!({"type":"key","event":event,"key":"F12","code":"F12","key_code":123}),
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        tabs().await,
        tabs_before,
        "a shortcut changed the browser's tabs"
    );
    assert_eq!(page(&f, "location.href").await, href);
    // The daemon installs no logger, so dependency trace logging (which would include
    // received WebSocket text) has nowhere to go.
    assert_eq!(log::max_level(), log::LevelFilter::Off);
    // Typed text is input, never recorded.
    assert!(
        !serde_json::to_string(&f.daemon.shared.db.events(&f.session, 0).await.unwrap())
            .unwrap()
            .contains("typed by human")
    );

    // done ends the stream and resumes the original call with the page the person left.
    let done = api::router(f.daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{ticket}/done"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(ended(&mut socket).await, TakeoverEndReason::Done);
    until(|| !f.runtime.0.lock().resolved.is_empty()).await;
    let (resolved_ticket, call_id, resolution) = f.runtime.0.lock().resolved[0].clone();
    assert_eq!(
        (resolved_ticket, call_id.as_str()),
        (ticket.clone(), "takeover")
    );
    let TicketResolution::Result { for_model_json } = resolution else {
        panic!("takeover resolves with a result");
    };
    let result: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(result["status"], "done");
    assert_eq!(result["page_changed"], true);
    assert_eq!(result["tab_id"], f.tab.as_str());
    assert_eq!(result["title"], "Clicked by human");
    let note = result["note"].as_str().unwrap();
    assert!(note.contains("Start the continuation with a fresh browser_snapshot"));
    assert!(
        note.contains("inspect whether the requested outcome actually occurred before replying")
    );
    assert!(note.contains("not that sign-in or the task succeeded"));

    // Element references from before the takeover are void even for nodes that survived.
    let stale = host(&f.daemon, &f.session)
        .await
        .call(call(
            "browser_act",
            json!({"tab_id":f.tab,"target":go_ref,"action":"click"}),
            "stale",
        ))
        .await;
    assert!(
        matches!(&stale, ToolOutcome::Denied { reason, .. } if reason.contains("take a fresh snapshot")),
        "pre-takeover ref must be refused: {stale:?}"
    );
    // The agent writes again, and its pre-takeover capture is no longer a diff base.
    let after = model(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_snapshot",
                json!({"tab_id":f.tab,"since":before_id}),
                "after",
            ))
            .await,
    );
    let after = after.as_str().unwrap();
    assert!(
        !after.lines().next().unwrap().contains(" since "),
        "stale since base was used: {after}"
    );
    assert!(after.contains("Human clicked"));
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn takeover_stream_ends_on_ticket_expiry_and_on_token_revocation() {
    let Some(f) = fixture(4).await else {
        return;
    };
    let revoked = takeover(&f, "revoked").await;
    let (device, token) = f
        .daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let mut socket = connect(&f, &revoked, Some(&token), None).await.unwrap();
    assert!(matches!(
        message(&mut socket).await,
        TakeoverServerMessage::Frame { .. }
    ));
    let response = api::router(f.daemon.shared.clone(), true)
        .oneshot(request(
            "DELETE",
            &format!("/v1/devices/{device}"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(ended(&mut socket).await, TakeoverEndReason::Revoked);
    let expiring = takeover(&f, "expiring").await;
    let (_, token) = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    // A browser cannot send Authorization; a forged or non-token subprotocol is refused,
    // the real one is accepted and only `openside.v1` is echoed back.
    assert_eq!(
        connect_web(&f, &expiring, "not-a-token").await.err(),
        Some(401)
    );
    assert_eq!(
        connect_web(&f, &expiring, "bad token").await.err(),
        Some(401)
    );
    let (mut socket, protocol) = connect_web(&f, &expiring, &token).await.unwrap();
    assert_eq!(protocol, "openside.v1");
    assert!(matches!(
        message(&mut socket).await,
        TakeoverServerMessage::Frame { .. }
    ));
    // Input keeps arriving across the deadline; none of it may land once the ticket expired.
    focus_name(&mut socket).await;
    let (mut tx, mut rx) = socket.split();
    let typing = tokio::spawn(async move {
        while tx
            .send(Message::Text(
                json!({"type":"text","text":"y"}).to_string().into(),
            ))
            .await
            .is_ok()
        {
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    });
    // Read concurrently, as a client does, so `ended` is consumed as soon as it arrives.
    let reader = tokio::spawn(async move {
        loop {
            let next = tokio::time::timeout(Duration::from_secs(30), rx.next())
                .await
                .unwrap();
            match next.expect("socket closed without ended").unwrap() {
                Message::Text(text) => {
                    if let TakeoverServerMessage::Ended { reason } =
                        serde_json::from_str(text.as_str()).unwrap()
                    {
                        return reason;
                    }
                }
                Message::Close(_) => panic!("closed without ended"),
                _ => {}
            }
        }
    });
    let expiry = std::time::Instant::now() + Duration::from_secs(20);
    while status(&f, &expiring).await == TicketStatus::Pending {
        assert!(std::time::Instant::now() < expiry, "ticket never expired");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let at_expiry = typed(&f).await;
    assert!(at_expiry > 0, "input reached the page before expiry");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        typed(&f).await,
        at_expiry,
        "input landed after the ticket expired"
    );
    assert_eq!(reader.await.unwrap(), TakeoverEndReason::Expired);
    typing.abort();
    assert_eq!(
        connect(&f, &expiring, Some(&token), None)
            .await
            .err()
            .unwrap()
            .0,
        409
    );
    // The lock ended with the ticket.
    model(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_snapshot",
                json!({"tab_id":f.tab}),
                "unlocked",
            ))
            .await,
    );

    f.daemon.shutdown().await;
}

#[tokio::test]
async fn takeover_publication_waits_for_the_agent_operation_already_running() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    let agent = host(&f.daemon, &f.session).await;
    let tab = f.tab.clone();
    let running = tokio::spawn(async move {
        agent
            .call(call(
                "browser_repl",
                json!({"tab_id":tab,"code":"await page.waitForTimeout(1500); await page.click('#go'); 'agent wrote'"}),
                "slow",
            ))
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ticket = takeover(&f, "barrier").await;
    // The ticket was published only after the agent's operation finished, so the person never
    // shares the tab with it.
    assert!(
        running.is_finished(),
        "takeover published while an agent operation was running"
    );
    assert!(model(running.await.unwrap())
        .to_string()
        .contains("agent wrote"));
    assert_eq!(page(&f, "out.textContent").await, "Human clicked");
    assert_eq!(status(&f, &ticket).await, TicketStatus::Pending);
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn queued_input_never_lands_after_done_reject_or_revocation() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    const BURST: u64 = 2000;
    for (index, end) in ["done", "reject", "revoke"].into_iter().enumerate() {
        let ticket = takeover(&f, &format!("race-{index}")).await;
        let (device, token) = f
            .daemon
            .shared
            .db
            .pair_named(format!("phone-{index}"), vec![CapabilityScope::HumanStep])
            .await
            .unwrap();
        let mut socket = connect(&f, &ticket, Some(&token), None).await.unwrap();
        assert!(matches!(
            message(&mut socket).await,
            TakeoverServerMessage::Frame { .. }
        ));
        focus_name(&mut socket).await;
        let start = typed(&f).await;
        for _ in 0..BURST {
            socket
                .feed(Message::Text(
                    json!({"type":"text","text":"x"}).to_string().into(),
                ))
                .await
                .unwrap();
        }
        socket.flush().await.unwrap();
        let admin = api::router(f.daemon.shared.clone(), true);
        let at_end = match end {
            "done" | "reject" => {
                let response = admin
                    .oneshot(request(
                        "POST",
                        &format!("/v1/tickets/{ticket}/{end}"),
                        json!({}),
                        None,
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{end}");
                typed(&f).await
            }
            _ => {
                let response = admin
                    .oneshot(request(
                        "DELETE",
                        &format!("/v1/devices/{device}"),
                        json!({}),
                        None,
                    ))
                    .await
                    .unwrap();
                assert!(response.status().is_success());
                assert_eq!(ended(&mut socket).await, TakeoverEndReason::Revoked);
                typed(&f).await
            }
        };
        assert!(
            at_end - start < BURST,
            "{end}: the burst finished before the transition; no race exercised"
        );
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(
            typed(&f).await,
            at_end,
            "{end}: input landed after the takeover ended"
        );
        if end == "revoke" {
            // The ticket is still pending: another device may take over again.
            let (_, other) = f
                .daemon
                .shared
                .db
                .pair(vec![CapabilityScope::HumanStep])
                .await
                .unwrap();
            reacquire(&f, &ticket, &other, Duration::from_secs(3)).await;
        }
    }
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn a_tab_with_secret_fill_history_never_streams_frames_even_after_navigation() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    let controller = &f.daemon.shared.registry.browser;
    let browser = controller
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    controller
        .mark_secret(&f.daemon.shared, &browser, &f.tab)
        .await
        .unwrap();
    // The agent leaves the filled document for a clean one before the takeover opens.
    model(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/clean", f.origin)}),
                "clean",
            ))
            .await,
    );
    let ticket = takeover(&f, "tainted").await;
    let token = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap()
        .1;
    let mut socket = connect(&f, &ticket, Some(&token), None).await.unwrap();
    let mut withheld = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let Ok(next) = tokio::time::timeout(Duration::from_millis(300), message(&mut socket)).await
        else {
            continue;
        };
        match next {
            TakeoverServerMessage::Withheld { code } => {
                assert_eq!(code, "screenshot_unavailable_after_secret_fill");
                withheld += 1;
            }
            TakeoverServerMessage::Frame { .. } => panic!("a frame of a tainted tab was streamed"),
            _ => {}
        }
    }
    assert!(withheld > 0, "the stream must say why it shows nothing");
    // Input still works: the person can finish the step blind.
    send(
        &mut socket,
        json!({"type":"mouse","event":"move","x":10,"y":10}),
    )
    .await;
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn a_controller_that_stops_reading_is_dropped_and_the_ticket_can_be_taken_again() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    model(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/noise", f.origin)}),
                "noise",
            ))
            .await,
    );
    let ticket = takeover(&f, "stalled").await;
    let (_, first) = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    // A peer that never reads loses the controller once a write misses its deadline.
    let stalled = stalled_peer(&f, &ticket, &first).await;
    let mut socket = reacquire(&f, &ticket, &first, Duration::from_secs(20)).await;
    assert!(matches!(
        message(&mut socket).await,
        TakeoverServerMessage::Frame { .. }
    ));
    drop(socket);
    drop(stalled);
    // Revocation ends a stalled controller at once, without waiting for its write deadline.
    let (device, second) = f
        .daemon
        .shared
        .db
        .pair_named("stalled phone".into(), vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    // The controller just dropped is released by its stream task; wait for that.
    let probe = reacquire(&f, &ticket, &first, Duration::from_secs(3)).await;
    drop(probe);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _stalled = stalled_peer(&f, &ticket, &second).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let revoked_at = std::time::Instant::now();
    let response = api::router(f.daemon.shared.clone(), true)
        .oneshot(request(
            "DELETE",
            &format!("/v1/devices/{device}"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_success());
    let mut socket = reacquire(&f, &ticket, &first, Duration::from_secs(3)).await;
    assert!(revoked_at.elapsed() < Duration::from_secs(4));
    assert!(matches!(
        message(&mut socket).await,
        TakeoverServerMessage::Frame { .. }
    ));
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn revocation_returns_only_after_admitted_input_and_nothing_runs_after_it() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    let ticket = takeover(&f, "revoke-barrier").await;
    let (device, token) = f
        .daemon
        .shared
        .db
        .pair_named("barrier phone".into(), vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let mut socket = connect(&f, &ticket, Some(&token), None).await.unwrap();
    assert!(matches!(
        message(&mut socket).await,
        TakeoverServerMessage::Frame { .. }
    ));
    focus_name(&mut socket).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let start = typed(&f).await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    crate::takeover::hooks::PAUSE
        .lock()
        .insert(ticket.clone(), (reached.clone(), release.clone()));
    send(&mut socket, json!({"type":"text","text":"a"})).await;
    // The input passed admission and authorization and is about to be dispatched.
    tokio::time::timeout(Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    let shared = f.daemon.shared.clone();
    let revoke = tokio::spawn(async move {
        api::router(shared, true)
            .oneshot(request(
                "DELETE",
                &format!("/v1/devices/{device}"),
                json!({}),
                None,
            ))
            .await
            .unwrap()
            .status()
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !revoke.is_finished(),
        "DELETE returned while admitted input was still pending"
    );
    // Input sent now is behind the paused one and after the revocation mark.
    send(&mut socket, json!({"type":"text","text":"b"})).await;
    release.notify_one();
    assert!(revoke.await.unwrap().is_success());
    let at_revoke = typed(&f).await;
    assert_eq!(
        at_revoke,
        start + 1,
        "exactly the admitted input ran before DELETE returned"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        typed(&f).await,
        at_revoke,
        "input ran after DELETE returned"
    );
    assert_eq!(ended(&mut socket).await, TakeoverEndReason::Revoked);
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn one_takeover_opener_ending_keeps_another_openers_reservation() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "reserve", PolicyMode::Full, None).await;
    let shared = &daemon.shared;
    let open = json!({"kind":"takeover","summary":"CAPTCHA","tab_id":"tab-1"});
    let act = json!({"tab_id":"tab-1","target":"e1","action":"click"});
    let first = crate::takeover::reserve(shared, &session, "request_human_step", &open).await;
    let second = crate::takeover::reserve(shared, &session, "request_human_step", &open).await;
    assert!(first.is_some() && second.is_some());
    // The first opener is cancelled while the second is still before publication.
    drop(first);
    let refusal = crate::takeover::blocked(shared, &session, "browser_act", &act).await;
    assert_eq!(refusal.map(|(reason, _)| reason), Some("takeover_active"));
    drop(second);
    assert!(
        crate::takeover::blocked(shared, &session, "browser_act", &act)
            .await
            .is_none()
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn rejected_input_flood_cannot_hold_the_controller_past_its_idle_limit() {
    let Some(f) = fixture(1800).await else {
        return;
    };
    let ticket = takeover(&f, "flood").await;
    crate::takeover::hooks::IDLE
        .lock()
        .insert(ticket.clone(), Duration::from_secs(1));
    let token = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap()
        .1;
    let mut stream = tokio::net::TcpStream::connect(&f.tcp).await.unwrap();
    let request = format!(
        "GET /v1/tickets/{ticket}/takeover HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n",
        f.tcp
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
    }
    assert!(head.starts_with(b"HTTP/1.1 101"));
    let (mut read, mut write) = stream.into_split();
    // The peer reads everything, so no write ever blocks; only the idle limit can end it.
    let drain = tokio::spawn(async move {
        let mut buffer = vec![0; 1 << 16];
        while read.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
    });
    // Pre-encoded masked text frames (zero mask key) of rejected input, written far faster
    // than the daemon parses them, so its read side is never idle.
    let payload = br#"{"type":"teleport"}"#;
    let mut frame = vec![
        0x81,
        0x80 | u8::try_from(payload.len()).unwrap(),
        0,
        0,
        0,
        0,
    ];
    frame.extend_from_slice(payload);
    let chunk = frame.repeat(40_000);
    let flood = tokio::spawn(async move { while write.write_all(&chunk).await.is_ok() {} });
    let (_, other) = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let started = std::time::Instant::now();
    reacquire(&f, &ticket, &other, Duration::from_secs(4)).await;
    assert!(started.elapsed() < Duration::from_secs(4));
    flood.abort();
    drain.abort();
    crate::takeover::hooks::IDLE.lock().remove(&ticket);
    f.daemon.shutdown().await;
}
