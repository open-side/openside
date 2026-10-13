//! HS-4 and HS-6 end to end: real daemon, real Python sidecar on a scripted model, real
//! Chromium and loopback fixture pages. Each test proves that the SAME deferred
//! `request_human_step` call receives its result and the run continues to settle.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use axum::{response::Html, routing::get, Router};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use openside_e2e::{Harness, SseReader};
use openside_proto::{
    Event, EventKind, TakeoverEndReason, TakeoverServerMessage, Ticket, TicketKind,
};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

const TAKEOVER_PAGE: &str = r#"<!doctype html><html><head><title>Captcha fixture</title><style>
body{margin:0} #go{position:fixed;left:0;top:0;width:400px;height:200px}
#name{position:fixed;left:0;top:300px;width:400px;height:60px} #out{position:fixed;left:0;top:420px}
</style></head><body><button id="go" onclick="document.title='Clicked by human';out.textContent='Human clicked'">I am not a robot</button>
<input id="name" aria-label="Name" autocomplete="off"><div id="out">Waiting</div></body></html>"#;
// The "other device" approves after a delay; nothing in openside touches this page meanwhile.
const DEVICE_PAGE: &str = r#"<!doctype html><title>Approve on your phone</title><p id="s">Check your phone</p>
<script>setTimeout(() => { document.getElementById('s').textContent = 'Device approved'; }, 3000);</script>"#;

struct Site {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Site {
    async fn start() -> Self {
        let app = Router::new()
            .route("/takeover", get(|| async { Html(TAKEOVER_PAGE) }))
            .route("/device", get(|| async { Html(DEVICE_PAGE) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { origin, task }
    }
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}
fn browser() -> Option<std::path::PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: takeover acceptance requires Chromium: {reason}");
            None
        }
    }
}
async fn create(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
async fn submit(harness: &Harness, session: &str, id: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":id,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}
fn ended_ok<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                card: _,
                tool_call_id,
                result,
                is_error: false,
            } if tool_call_id == call => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {call}: {events:?}"))
}
async fn opened(stream: &mut SseReader) -> Ticket {
    loop {
        let event = stream.next().await;
        if let EventKind::TicketOpened { ticket } = event.kind {
            return *ticket;
        }
        assert!(
            !matches!(event.kind, EventKind::SessionSettled {}),
            "settled without ticket"
        );
    }
}
/// Open a tab in a first turn, then give the sidecar the rest of the script; the sidecar
/// reloads it at the committed boundary and keeps its conversation.
async fn open_tab(
    harness: &mut Harness,
    stream: &mut SseReader,
    session: &str,
    url: &str,
    rest: Vec<Value>,
) -> String {
    submit(harness, session, "open-message").await;
    let events = stream.settled().await;
    let tab = ended_ok(&events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut responses = vec![
        tool("browser_open_tab", json!({"url":url}), "open"),
        text("opened"),
    ];
    responses.extend(
        rest.into_iter().map(|response| {
            serde_json::from_str(&response.to_string().replace("TAB", &tab)).unwrap()
        }),
    );
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    let boundary = stream.settled().await;
    assert!(boundary.iter().any(
        |event| matches!(&event.kind, EventKind::RuntimeError { code, .. } if code == "interrupted")
    ));
    tab
}
/// Events up to the settle that follows the ticket's resolution.
async fn resumed(stream: &mut SseReader, ticket: &Ticket) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = stream.next().await;
        let done = matches!(event.kind, EventKind::SessionSettled {})
            && events.iter().any(|event: &Event| {
                matches!(&event.kind, EventKind::TicketResolved { ticket_id, .. } if ticket_id == &ticket.id)
            });
        events.push(event);
        if done {
            return events;
        }
    }
}
/// The tool-return Pydantic AI recorded for `call` in the committed conversation.
fn tool_return(harness: &Harness, session: &str, call: &str) -> Value {
    harness.snapshot(session)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .find(|part| part["tool_call_id"] == call && part["part_kind"] == "tool-return")
        .cloned()
        .unwrap_or_else(|| panic!("no tool-return for {call}"))
}

async fn prepared(script: Vec<Value>) -> Option<(Harness, Site)> {
    let executable = browser()?;
    let site = Site::start().await;
    let mut harness = Harness::prepare(DAEMON, &json!({"responses":script}))?;
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;
    Some((harness, site))
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn message(socket: &mut Socket) -> TakeoverServerMessage {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(20), socket.next())
            .await
            .expect("takeover message within 20s")
            .expect("socket open")
            .unwrap();
        if let Message::Text(body) = next {
            return serde_json::from_str(body.as_str()).unwrap();
        }
    }
}

#[tokio::test]
async fn hs6_takeover_stream_drives_tab_while_agent_is_locked_out_and_same_call_resumes() {
    let Some((mut harness, site)) = prepared(Vec::new()).await else {
        return;
    };
    let url = format!("{}/takeover", site.origin);
    harness.replace_script(&json!({"responses":[tool("browser_open_tab", json!({"url":url}), "open"), text("opened")]}));
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let tab = open_tab(
        &mut harness,
        &mut stream,
        &session,
        &url,
        vec![
            tool(
                "request_human_step",
                json!({"kind":"takeover","summary":"Solve the CAPTCHA","tab_id":"TAB"}),
                "captcha",
            ),
            tool(
                "browser_snapshot",
                json!({"tab_id":"TAB"}),
                "after-snapshot",
            ),
            text("continued after takeover"),
        ],
    )
    .await;
    submit(&harness, &session, "captcha-message").await;
    let ticket = opened(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::Takeover);
    assert_eq!(ticket.tool_call_id, "captcha");
    let deferred = stream.settled().await;
    assert_eq!(ended_ok(&deferred, "captcha")["status"], "deferred");

    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"phone","scopes":["human_step"]})),
        )
        .await;
    let token = paired.body["token"].as_str().unwrap().to_owned();
    let reader = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"reader","scopes":["read"]})),
        )
        .await;
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
    let connect = |token: String| {
        let mut request = format!(
            "ws://127.0.0.1:{}/v1/tickets/{}/takeover",
            port.trim(),
            ticket.id
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        tokio_tungstenite::connect_async(request)
    };
    match connect(reader.body["token"].as_str().unwrap().to_owned()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), 403)
        }
        other => panic!("read token must be refused: {:?}", other.map(|_| ())),
    }
    let (mut socket, _) = connect(token.clone()).await.expect("human_step controller");
    let TakeoverServerMessage::Frame { data, .. } = message(&mut socket).await else {
        panic!("first message must be a frame");
    };
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert_eq!(&jpeg[..2], b"\xff\xd8");
    match connect(token.clone()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), 409)
        }
        other => panic!("second controller must be refused: {:?}", other.map(|_| ())),
    }

    // An external agent driving the same tab through the public REPL route is locked out.
    let repl = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/repl"),
            Some(json!({"code":"await page.locator('#go').click()","tab_id":tab})),
        )
        .await;
    assert_eq!(repl.body["status"], "denied", "{}", repl.body);
    assert_eq!(repl.body["reason"], "takeover_active");

    for (x, y) in [(100, 100), (100, 330)] {
        for event in ["down", "up"] {
            let input = json!({"type":"mouse","event":event,"x":x,"y":y});
            socket
                .send(Message::Text(input.to_string().into()))
                .await
                .unwrap();
        }
    }
    let typed = json!({"type":"text","text":"typed-by-human-canary"});
    socket
        .send(Message::Text(typed.to_string().into()))
        .await
        .unwrap();
    // Input is forwarded as it arrives; give Chromium a moment before ending the takeover.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let done = harness
        .request(
            "POST",
            &format!("/v1/tickets/{}/done", ticket.id),
            Some(json!({})),
        )
        .await;
    assert_eq!(done.status, 200, "{}", done.body);
    loop {
        if let TakeoverServerMessage::Ended { reason } = message(&mut socket).await {
            assert_eq!(reason, TakeoverEndReason::Done);
            break;
        }
    }
    let events = resumed(&mut stream, &ticket).await;
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == "continued after takeover")));
    // The SAME call got the post-takeover page, and the agent wrote to the tab again.
    let result = tool_return(&harness, &session, "captcha");
    let content = result["content"].to_string();
    assert!(content.contains("Clicked by human"), "{result}");
    assert!(content.contains("page_changed"), "{result}");
    let snapshot = ended_ok(&events, "after-snapshot").to_string();
    assert!(snapshot.contains("Human clicked"), "{snapshot}");
    assert!(
        snapshot.contains("typed-by-human-canary"),
        "the typed value is on the page"
    );
    // Raw input messages are not recorded: the audit log never sees the typed text (the page
    // itself, and so later snapshots, of course show it).
    let audit = std::fs::read_to_string(harness.data_dir.join("audit.jsonl")).unwrap_or_default();
    assert!(!audit.contains("typed-by-human-canary"));
    harness.kill().await;
}

#[tokio::test]
async fn hs4_device_approval_condition_resumes_same_call_without_a_human() {
    let Some((mut harness, site)) = prepared(Vec::new()).await else {
        return;
    };
    let url = format!("{}/device", site.origin);
    harness.replace_script(&json!({"responses":[tool("browser_open_tab", json!({"url":url}), "open"), text("opened")]}));
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    open_tab(
        &mut harness,
        &mut stream,
        &session,
        &url,
        vec![
            tool(
                "request_human_step",
                json!({"kind":"device_approval","summary":"Approve the sign-in on your phone","tab_id":"TAB","resume_when":{"text_visible":"Device approved"}}),
                "device",
            ),
            text("approved without me"),
        ],
    )
    .await;
    let started = Instant::now();
    submit(&harness, &session, "device-message").await;
    let ticket = opened(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::DeviceApproval);
    assert_eq!(ticket.tool_call_id, "device");
    // Nobody calls done or resolve: the daemon's poll sees the page condition.
    let events = resumed(&mut stream, &ticket).await;
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "resolved before the page condition could hold"
    );
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, resolution, .. } if ticket_id == &ticket.id && resolution["status"] == "done")));
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == "approved without me")));
    let result = tool_return(&harness, &session, "device");
    assert!(result["content"].to_string().contains("done"), "{result}");
    let state = harness
        .request("GET", &format!("/v1/tickets/{}", ticket.id), None)
        .await;
    assert_eq!(state.body["status"], "resolved");
    harness.kill().await;
}
