use super::*;
use axum::{response::Html, routing::get, Router};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use openside_proto::{PreviewEndReason, PreviewPauseReason, PreviewServerMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const PAGE: &str = r#"<!doctype html><style>body{margin:0;background:#e00000}</style><title>Red</title><input id="name" autofocus>"#;
const BLUE: &str =
    r#"<!doctype html><style>body{margin:0;background:#0000e0}</style><title>Blue</title>"#;
const NOISE: &str = r#"<!doctype html><canvas id="c" width="1440" height="900"></canvas><script>const ctx=c.getContext('2d'),img=ctx.createImageData(1440,900);setInterval(()=>{const d=new Uint32Array(img.data.buffer);for(let i=0;i<d.length;i++)d[i]=(Math.random()*0xffffffff)|0xff000000;ctx.putImageData(img,0,0)},30)</script>"#;

struct Fixture {
    daemon: Daemon,
    session: SessionId,
    tab: String,
    origin: String,
    tcp: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    pid: u32,
    _root: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.pid).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

async fn fixture() -> Option<Fixture> {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    match test_browser(root.path()) {
        Ok(browser) => config.loaded.config.browser.executable = Some(browser),
        Err(reason) => {
            eprintln!("SKIP preview browser tests: {reason}");
            return None;
        }
    }
    let site = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", site.local_addr().unwrap().port());
    let site_task = tokio::spawn(async move {
        axum::serve(
            site,
            Router::new()
                .route("/", get(|| async { Html(PAGE) }))
                .route("/blue", get(|| async { Html(BLUE) }))
                .route("/noise", get(|| async { Html(NOISE) })),
        )
        .await
        .unwrap();
    });
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let session = create_session(
        &daemon,
        &crate::persistence::random_id(),
        PolicyMode::Full,
        None,
    )
    .await;
    let tab = result(
        host(&daemon, &session)
            .await
            .call(call("browser_open_tab", json!({"url":origin}), "open"))
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
        session,
        tab,
        origin,
        tcp,
        tasks: vec![site_task, api_task],
        pid,
        _root: root,
    })
}
fn result(outcome: ToolOutcome) -> Value {
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("tool failed: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn connect(f: &Fixture, session: &SessionId, token: &str) -> Result<Socket, (u16, Value)> {
    let mut request = format!("ws://{}/v1/sessions/{session}/browser/preview", f.tcp)
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("openside.v1, openside.bearer.{token}")
            .parse()
            .unwrap(),
    );
    request
        .headers_mut()
        .insert("origin", format!("http://{}", f.tcp).parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, response)) => {
            assert_eq!(response.headers()["sec-websocket-protocol"], "openside.v1");
            Ok(socket)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err((
            response.status().as_u16(),
            response
                .body()
                .as_ref()
                .and_then(|body| serde_json::from_slice(body).ok())
                .unwrap_or(Value::Null),
        )),
        Err(error) => panic!("preview socket: {error}"),
    }
}
async fn message(socket: &mut Socket) -> PreviewServerMessage {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Message::Text(body) = socket.next().await.unwrap().unwrap() {
                return serde_json::from_str(&body).unwrap();
            }
        }
    })
    .await
    .expect("preview message deadline")
}
async fn frame(socket: &mut Socket) -> (String, String) {
    loop {
        if let PreviewServerMessage::Frame {
            data,
            width,
            height,
            tab_id,
        } = message(socket).await
        {
            assert_eq!((width, height), (720, 450));
            return (data, tab_id);
        }
    }
}
fn jpeg_size(data: &str) -> (u16, u16) {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert_eq!(&bytes[..2], &[0xff, 0xd8]);
    let mut at = 2;
    while at + 8 < bytes.len() {
        if bytes[at] != 0xff {
            at += 1;
            continue;
        }
        let marker = bytes[at + 1];
        at += 2;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if marker == 0x00 || marker == 0xff || (0xd0..=0xd8).contains(&marker) {
            continue;
        }
        let len = usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]));
        if [0xc0, 0xc1, 0xc2].contains(&marker) {
            return (
                u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]),
                u16::from_be_bytes([bytes[at + 3], bytes[at + 4]]),
            );
        }
        at += len;
    }
    panic!("JPEG has no size");
}
async fn token(f: &Fixture, scope: CapabilityScope) -> String {
    f.daemon.shared.db.pair(vec![scope]).await.unwrap().1
}

#[tokio::test]
async fn preview_frames_follow_agent_navigation_and_tab_switch_with_bounded_size_and_rate() {
    let Some(f) = fixture().await else {
        return;
    };
    let token = token(&f, CapabilityScope::Browse).await;
    let mut socket = connect(&f, &f.session, &token).await.unwrap();
    let (red, tab) = frame(&mut socket).await;
    assert_eq!(tab, f.tab);
    assert_eq!(jpeg_size(&red), (720, 450));
    result(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/blue",f.origin)}),
                "nav",
            ))
            .await,
    );
    loop {
        let (data, _) = frame(&mut socket).await;
        if data != red {
            break;
        }
    }
    let second = result(
        host(&f.daemon, &f.session)
            .await
            .call(call("browser_open_tab", json!({"url":f.origin}), "second"))
            .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    loop {
        if frame(&mut socket).await.1 == second {
            break;
        }
    }
    result(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/noise",f.origin)}),
                "noise",
            ))
            .await,
    );
    loop {
        if frame(&mut socket).await.1 == f.tab {
            break;
        }
    }
    let start = tokio::time::Instant::now();
    for _ in 0..4 {
        frame(&mut socket).await;
    }
    assert!(
        start.elapsed() >= Duration::from_millis(1450),
        "preview exceeded 2 fps"
    );
    socket.close(None).await.unwrap();
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_ignores_input_and_withholds_secret_tainted_tabs_after_navigation() {
    let Some(f) = fixture().await else {
        return;
    };
    let token = token(&f, CapabilityScope::Browse).await;
    let mut socket = connect(&f, &f.session, &token).await.unwrap();
    frame(&mut socket).await;
    socket
        .send(Message::Text(
            json!({"type":"text","text":"must-not-land"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    socket
        .send(Message::Text("x".repeat(1024).into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let browser = f
        .daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&f.tab, "document.querySelector('#name').value")
            .await
            .unwrap(),
        ""
    );
    f.daemon
        .shared
        .registry
        .browser
        .mark_secret(&f.daemon.shared, &browser, &f.tab)
        .await
        .unwrap();
    browser
        .page_command(
            f.session.as_str(),
            &f.tab,
            "Input.insertText",
            json!({"text":"VISIBLE-SECRET-PREVIEW-CANARY"}),
        )
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&f.tab, "document.querySelector('#name').value")
            .await
            .unwrap(),
        "VISIBLE-SECRET-PREVIEW-CANARY"
    );
    loop {
        if matches!(
            message(&mut socket).await,
            PreviewServerMessage::Withheld { .. }
        ) {
            break;
        }
    }
    no_frames(&mut socket, Duration::from_millis(1100), "visible secret").await;
    result(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/blue",f.origin)}),
                "clean-nav",
            ))
            .await,
    );
    for _ in 0..3 {
        assert!(
            matches!(message(&mut socket).await,PreviewServerMessage::Withheld { code, tab_id } if code == "screenshot_unavailable_after_secret_fill" && tab_id == f.tab)
        );
    }
    socket.close(None).await.unwrap();
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_closes_messages_over_one_kib_without_forwarding_any_input() {
    let Some(f) = fixture().await else {
        return;
    };
    let token = token(&f, CapabilityScope::Browse).await;
    let mut socket = connect(&f, &f.session, &token).await.unwrap();
    frame(&mut socket).await;
    socket
        .send(Message::Text("x".repeat(1025).into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(message) = socket.next().await {
            if matches!(message, Err(_) | Ok(Message::Close(_))) {
                break;
            }
        }
    })
    .await
    .expect("oversized preview input was not disconnected");
    let browser = f
        .daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&f.tab, "document.querySelector('#name').value")
            .await
            .unwrap(),
        ""
    );
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_requires_browse_refuses_private_sessions_and_bounds_viewers() {
    let Some(f) = fixture().await else {
        return;
    };
    let read = token(&f, CapabilityScope::Read).await;
    assert_eq!(connect(&f, &f.session, &read).await.err().unwrap().0, 403);
    let browse = token(&f, CapabilityScope::Browse).await;
    let response = api::router(f.daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            "/v1/sessions",
            json!({"incognito":true,"model":std::any::type_name::<TestRuntime>()}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let private = SessionId::new(json_body(response).await["id"].as_str().unwrap());
    assert_eq!(
        connect(&f, &private, &browse).await.err().unwrap(),
        (
            409,
            json!({"code":"private_session","message":"private session"})
        )
    );
    let mut sockets = Vec::new();
    for _ in 0..8 {
        sockets.push(connect(&f, &f.session, &browse).await.unwrap());
    }
    let (code, error) = connect(&f, &f.session, &browse).await.err().unwrap();
    assert_eq!(code, 429);
    assert_eq!(error["code"], "too_many_viewers");
    for socket in &mut sockets {
        frame(socket).await;
        socket.close(None).await.unwrap();
    }
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn takeover_preempts_preview_in_both_connection_orders_and_preview_resumes() {
    let Some(f) = fixture().await else {
        return;
    };
    let browse = token(&f, CapabilityScope::Browse).await;
    let human = token(&f, CapabilityScope::HumanStep).await;
    let mut preview = connect(&f, &f.session, &browse).await.unwrap();
    frame(&mut preview).await;
    for iteration in 0..2 {
        let ToolOutcome::Deferred { ticket, .. } = host(&f.daemon, &f.session)
            .await
            .call(call(
                "request_human_step",
                json!({"kind":"takeover","summary":"Human step","tab_id":f.tab}),
                &format!("takeover-{iteration}"),
            ))
            .await
        else {
            panic!("ticket");
        };
        let mut upgrade_request = format!("ws://{}/v1/tickets/{ticket}/takeover", f.tcp)
            .into_client_request()
            .unwrap();
        upgrade_request
            .headers_mut()
            .insert("authorization", format!("Bearer {human}").parse().unwrap());
        let (mut control, _) = tokio_tungstenite::connect_async(upgrade_request)
            .await
            .unwrap();
        if iteration == 1 {
            preview = connect(&f, &f.session, &browse).await.unwrap();
        }
        loop {
            if matches!(message(&mut preview).await,PreviewServerMessage::Paused { reason: PreviewPauseReason::Takeover, tab_id } if tab_id == f.tab)
            {
                break;
            }
        }
        assert!(matches!(
            control.next().await.unwrap().unwrap(),
            Message::Text(_)
        ));
        control
            .send(Message::Text(
                json!({"type":"text","text":"TAKEOVER-PREVIEW-CANARY"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let browser = f
            .daemon
            .shared
            .registry
            .browser
            .existing()
            .await
            .map_err(|error| error.0)
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !browser
                .evaluate(
                    &f.tab,
                    "document.querySelector('#name').value.includes('TAKEOVER-PREVIEW-CANARY')",
                )
                .await
                .unwrap()
                .as_bool()
                .unwrap()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("takeover input did not land");
        no_frames(
            &mut preview,
            Duration::from_millis(1100),
            "takeover with typed canary",
        )
        .await;
        let response = api::router(f.daemon.shared.clone(), true)
            .oneshot(request(
                "POST",
                &format!("/v1/tickets/{ticket}/done"),
                json!({}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        frame(&mut preview).await;
        control.close(None).await.ok();
        if iteration == 0 {
            preview.close(None).await.unwrap();
        }
    }
    preview.close(None).await.unwrap();
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_ends_on_its_device_revocation_session_stop_and_deletion() {
    let Some(f) = fixture().await else {
        return;
    };
    let (device, revocable) = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Browse])
        .await
        .unwrap();
    let browse = token(&f, CapabilityScope::Browse).await;
    let mut revoked = connect(&f, &f.session, &revocable).await.unwrap();
    let mut retained = connect(&f, &f.session, &browse).await.unwrap();
    frame(&mut revoked).await;
    frame(&mut retained).await;
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
    loop {
        if let PreviewServerMessage::Ended { reason } = message(&mut revoked).await {
            assert_eq!(reason, PreviewEndReason::Revoked);
            break;
        }
    }
    result(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/blue",f.origin)}),
                "retained-nav",
            ))
            .await,
    );
    frame(&mut retained).await;
    let response = api::router(f.daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/sessions/{}/stop", f.session),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_success());
    loop {
        if let PreviewServerMessage::Ended { reason } = message(&mut retained).await {
            assert_eq!(reason, PreviewEndReason::SessionStopped);
            break;
        }
    }
    let second = create_session(&f.daemon, "delete-preview", PolicyMode::Full, None).await;
    let mut deleted = connect(&f, &second, &browse).await.unwrap();
    let response = api::router(f.daemon.shared.clone(), true)
        .oneshot(request(
            "DELETE",
            &format!("/v1/sessions/{second}"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_success());
    loop {
        if let PreviewServerMessage::Ended { reason } = message(&mut deleted).await {
            assert_eq!(reason, PreviewEndReason::SessionDeleted);
            break;
        }
    }
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_drops_a_stalled_reader_with_a_bounded_write_deadline() {
    let Some(f) = fixture().await else {
        return;
    };
    result(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/noise",f.origin)}),
                "noise",
            ))
            .await,
    );
    let token = token(&f, CapabilityScope::Browse).await;
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut stream = socket.connect(f.tcp.parse().unwrap()).await.unwrap();
    stream.write_all(format!("GET /v1/sessions/{}/browser/preview HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n",f.session,f.tcp).as_bytes()).await.unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(stream.read_u8().await.unwrap());
    }
    assert!(headers.starts_with(b"HTTP/1.1 101"));
    tokio::time::sleep(Duration::from_secs(25)).await;
    // Once the writer's deadline closes TCP, draining buffered frames reaches EOF/reset.
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut buf = [0; 65536];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await
    .expect("stalled preview was not disconnected");
    let mut fresh = connect(&f, &f.session, &token).await.unwrap();
    frame(&mut fresh).await;
    fresh.close(None).await.unwrap();
    f.daemon.shutdown().await;
}

async fn no_frames(socket: &mut Socket, duration: Duration, boundary: &str) {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        match tokio::time::timeout_at(deadline, socket.next()).await {
            Err(_) => return,
            Ok(Some(Ok(Message::Text(body)))) => {
                let message: PreviewServerMessage = serde_json::from_str(&body).unwrap();
                assert!(
                    !matches!(message, PreviewServerMessage::Frame { .. }),
                    "frame after {boundary}"
                );
            }
            Ok(Some(Ok(_))) => {}
            other => panic!("preview closed during {boundary}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn preview_latches_stop_across_immediate_session_resume() {
    let Some(f) = fixture().await else {
        return;
    };
    let token = token(&f, CapabilityScope::Browse).await;
    let mut old = connect(&f, &f.session, &token).await.unwrap();
    frame(&mut old).await;
    let router = api::router(f.daemon.shared.clone(), true);
    let stopped = router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/v1/sessions/{}/stop", f.session),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert!(stopped.status().is_success());
    let resumed = router
        .oneshot(request(
            "POST",
            &format!("/v1/sessions/{}/messages", f.session),
            json!({"message_id":"resume-preview","text":"Resume","mode":"now"}),
            None,
        ))
        .await
        .unwrap();
    assert!(resumed.status().is_success());
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.daemon.shared.db.state(&f.session).await.unwrap().status == SessionStatus::Stopped {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        message(&mut old).await,
        PreviewServerMessage::Ended {
            reason: PreviewEndReason::SessionStopped
        }
    );
    let mut new = connect(&f, &f.session, &token).await.unwrap();
    frame(&mut new).await;
    new.close(None).await.unwrap();
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_never_discloses_foreign_owned_tabs_even_if_current_tab_is_forged() {
    let Some(f) = fixture().await else {
        return;
    };
    let foreign = create_session(
        &f.daemon,
        &crate::persistence::random_id(),
        PolicyMode::Full,
        None,
    )
    .await;
    let token = token(&f, CapabilityScope::Browse).await;
    f.daemon.shared.registry.browser.note_tab(&foreign, &f.tab);
    let mut socket = connect(&f, &foreign, &token).await.unwrap();
    assert_eq!(message(&mut socket).await, PreviewServerMessage::NoTab);
    no_frames(
        &mut socket,
        Duration::from_millis(1100),
        "foreign-owned tab",
    )
    .await;
    let owned = result(
        host(&f.daemon, &foreign)
            .await
            .call(call(
                "browser_open_tab",
                json!({"url":format!("{}/blue", f.origin)}),
                "foreign-open",
            ))
            .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(frame(&mut socket).await.1, owned);
    socket.close(None).await.unwrap();
    f.daemon.shutdown().await;
}

#[tokio::test]
async fn preview_writer_allows_one_admitted_pretransition_frame_and_discards_queued_stale_frames() {
    use crate::preview::hooks::{Barrier, Stage, BARRIERS};
    let Some(f) = fixture().await else {
        return;
    };
    result(
        host(&f.daemon, &f.session)
            .await
            .call(call(
                "browser_navigate",
                json!({"tab_id":f.tab,"url":format!("{}/noise", f.origin)}),
                "animated",
            ))
            .await,
    );
    let (device, token) = f
        .daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Browse])
        .await
        .unwrap();
    for (stage, expected_frames) in [(Stage::Before, 0), (Stage::After, 1)] {
        let mut socket = connect(&f, &f.session, &token).await.unwrap();
        frame(&mut socket).await;
        let barrier = Arc::new(Barrier::default());
        BARRIERS
            .lock()
            .insert(device.clone(), (stage, Arc::clone(&barrier)));
        tokio::time::timeout(Duration::from_secs(15), barrier.reached.notified())
            .await
            .unwrap();
        let reservation = crate::takeover::reserve(
            &f.daemon.shared,
            &f.session,
            "request_human_step",
            &json!({"kind":"takeover","tab_id":f.tab}),
        )
        .await
        .unwrap();
        barrier.release.notify_one();
        let mut frames = 0;
        loop {
            match message(&mut socket).await {
                PreviewServerMessage::Frame { .. } => frames += 1,
                PreviewServerMessage::Paused { .. } => break,
                _ => {}
            }
        }
        assert_eq!(frames, expected_frames);
        no_frames(
            &mut socket,
            Duration::from_millis(1100),
            "admission barrier",
        )
        .await;
        drop(reservation);
        frame(&mut socket).await;
        socket.close(None).await.unwrap();
    }
    f.daemon.shutdown().await;
}
