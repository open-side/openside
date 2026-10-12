//! The web app keeps its own requests to the daemon answered across page loads.
//!
//! The daemon speaks HTTP/1.1, so Chromium allows six connections per host. Chromium keeps a page
//! it navigates away from in the back/forward cache with its open fetch streams and WebSockets,
//! so before the fix every full load of a conversation left one session event stream (and one
//! preview WebSocket) behind; after a few loads the pool was full and even `GET /v1/health` from
//! the page on screen never got an answer ("Loading conversations…" forever). The page now drops
//! its long-lived connections on `pagehide` (web/src/api/pageLifecycle.ts).
//!
//! Real daemon, scripted sidecar and two Chromiums: one agent tab live on a sign-in page with a
//! pending code request, three plain conversations, and the UI loaded ten times across routes
//! and phone/desktop viewports.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

const OWNER: &str = "web-connections-test";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const LOGIN: &str = "<!doctype html><title>Sign in</title><h1>Check your phone</h1><form><label>Verification code<input name='code' inputmode='numeric' autocomplete='one-time-code' maxlength='6'></label><button>Verify</button></form>";

struct Site(tokio::task::JoinHandle<()>);
impl Drop for Site {
    fn drop(&mut self) {
        self.0.abort();
    }
}
/// Reaps the UI Chromium even when an assertion fails first.
struct BrowserCleanup(u32);
impl Drop for BrowserCleanup {
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}
fn result<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
            } if tool_call_id == call => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {call}: {events:?}"))
}
fn field(snapshot: &Value, name: &str) -> String {
    let text = snapshot.as_str().expect("compact accessibility snapshot");
    let line = text
        .lines()
        .find(|line| line.contains(&format!("\"{name}\"")) && line.contains("textbox"))
        .unwrap_or_else(|| panic!("snapshot lacks {name}: {text}"));
    let start = line.find('[').unwrap() + 1;
    let end = line[start..].find(']').unwrap() + start;
    line[start..end].to_owned()
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
async fn create(harness: &Harness, title: &str) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":title,"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
/// Appends scripted turns; a fresh scripted sidecar resumes at the cursor, so the script can
/// use the tab id and field ref earlier real tool calls returned.
async fn extend_script(
    harness: &mut Harness,
    stream: &mut SseReader,
    responses: &mut Vec<Value>,
    additions: Vec<Value>,
) {
    responses.extend(additions);
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    stream.settled().await;
}
async fn wait(browser: &Browser, tab: &str, what: &str, predicate: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if browser.evaluate(tab, predicate).await.unwrap() == true {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Established TCP connections from the UI Chromium (any process of its group) to `port`.
fn ui_connections(port: &str, group: u32) -> Vec<String> {
    let out = std::process::Command::new("ss")
        .args([
            "-tnpH",
            "state",
            "established",
            &format!("( dport = :{port} )"),
        ])
        .output()
        .expect("ss");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| {
            line.split("pid=").skip(1).any(|rest| {
                let pid: String = rest.chars().take_while(char::is_ascii_digit).collect();
                std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                    // Field 5 (after the parenthesised command) is the process group.
                    stat.rsplit_once(')')
                        .and_then(|(_, tail)| tail.split_whitespace().nth(2).map(str::to_owned))
                        == Some(group.to_string())
                })
            })
        })
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn web_page_requests_stay_answered_across_page_loads_with_a_live_tab_and_pending_code() {
    // OPENSIDE_WEB_DIST serves another build, e.g. one from before the fix to watch this fail.
    let dist = std::env::var_os("OPENSIDE_WEB_DIST").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist"),
        PathBuf::from,
    );
    if !dist.join("index.html").is_file() {
        eprintln!("SKIP web connections: run pnpm -C web build");
        return;
    }
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(browser) => browser,
        Err(reason) => {
            eprintln!("SKIP web connections requires Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let _site = Site(tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/login", get(|| async { Html(LOGIN) })),
        )
        .await
        .unwrap();
    }));

    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{site}/login")}),
            "open",
        ),
        text("The sign-in page is open."),
    ];
    let Some(mut harness) = Harness::prepare(DAEMON, &json!({"responses":responses})) else {
        return;
    };
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[browser]\nexecutable = {}\n",
        json!(dist.canonicalize().unwrap()),
        json!(executable)
    ));
    harness.launch().await;

    // The sign-in conversation: a live agent tab, then a pending code request on it.
    let login = create(&harness, "Sign in").await;
    let mut stream = harness.events(&login, 0).await;
    submit(&harness, &login, "open").await;
    let events = stream.settled().await;
    let tab_id = result(&events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    extend_script(
        &mut harness,
        &mut stream,
        &mut responses,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab_id}), "fields"),
            text("Found the code field."),
        ],
    )
    .await;
    submit(&harness, &login, "fields").await;
    let events = stream.settled().await;
    let code_ref = field(result(&events, "fields"), "Verification code");
    extend_script(&mut harness, &mut stream, &mut responses, vec![tool("request_otp", json!({"tab_id":tab_id,"field_ref":code_ref,"channel":"sms","summary":"Code from the sign-in text"}), "otp")]).await;
    submit(&harness, &login, "otp").await;
    let ticket = loop {
        let event = stream.next().await;
        if let EventKind::TicketOpened { ticket } = event.kind {
            break ticket.id.to_string();
        }
        assert!(
            !matches!(event.kind, EventKind::SessionSettled {}),
            "settled without a ticket"
        );
    };

    // Plain conversations; each new sidecar starts this script at cursor 0.
    harness.replace_script(&json!({"responses":[text("Hello there.")]}));
    let mut plain = Vec::new();
    for title in ["Plans", "Notes", "Trip"] {
        let id = create(&harness, title).await;
        let mut events = harness.events(&id, 0).await;
        submit(&harness, &id, "hi").await;
        events.settled().await;
        plain.push(id);
    }

    let paired = harness
        .request("POST", "/v1/devices/pair", Some(json!({"name":"connections","scopes":["read","steer","approve","human_step","browse"]})))
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_owned();
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let origin = format!("http://127.0.0.1:{port}");
    let (ui, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let group = ui.process_id();
    let _cleanup = BrowserCleanup(group);
    let tab = ui.create_agent_tab(OWNER, &origin).await.unwrap().id;
    wait(
        &ui,
        &tab,
        "token screen",
        "!!document.querySelector('.token-entry')",
    )
    .await;
    ui.evaluate(
        &tab,
        &format!(
            "localStorage.setItem('openside.token', {}); true",
            json!(token)
        ),
    )
    .await
    .unwrap();
    // Every document this tab loads records the session streams and WebSockets it opens in
    // origin-wide storage and removes them when it closes or aborts them. A page Chromium keeps
    // in its back/forward cache is frozen, so a connection it never let go of stays on the list:
    // that is exactly what used to fill the six-per-host pool. (ss alone cannot tell: idle
    // keep-alive sockets, some in separate pools such as the credential-less manifest fetch, come
    // and go with every page load.)
    let ledger = r#"(() => {
        const key = '__openStreams';
        const edit = (change) => { const open = JSON.parse(localStorage.getItem(key) || '{}'); change(open); localStorage.setItem(key, JSON.stringify(open)); };
        const track = (url) => {
            const id = Math.random().toString(36).slice(2);
            edit((open) => { open[id] = url; });
            let done = false;
            return () => { if (!done) { done = true; edit((open) => { delete open[id]; }); } };
        };
        const NativeSocket = WebSocket;
        window.WebSocket = class extends NativeSocket {
            constructor(...args) {
                super(...args);
                const end = track(String(args[0]));
                this.addEventListener('close', end);
                const close = this.close.bind(this);
                this.close = (...reason) => { end(); close(...reason); };
            }
        };
        const nativeFetch = window.fetch;
        window.fetch = async (input, init) => {
            const url = String(input instanceof Request ? input.url : input);
            if (!url.endsWith('/events')) return nativeFetch(input, init);
            const end = track(url);
            init?.signal?.addEventListener('abort', end);
            try {
                const response = await nativeFetch(input, init);
                if (!response.body) { end(); return response; }
                const body = response.body.pipeThrough(new TransformStream({ flush: end }));
                return new Response(body, { status: response.status, statusText: response.statusText, headers: response.headers });
            } catch (error) { end(); throw error; }
        };
    })();"#;
    ui.page_cdp(
        OWNER,
        &tab,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": ledger }),
    )
    .await
    .unwrap();

    let routes = [
        format!("/sessions/{login}"),
        format!("/sessions/{}", plain[0]),
        format!("/sessions/{login}?ticket={ticket}"),
        "/".to_owned(),
        format!("/sessions/{}", plain[1]),
        format!("/sessions/{login}"),
        format!("/sessions/{}", plain[2]),
        format!("/sessions/{}", plain[0]),
        format!("/sessions/{login}"),
        format!("/sessions/{}", plain[1]),
    ];
    for (round, route) in routes.iter().enumerate() {
        let (width, height, scale, mobile) = if round % 2 == 0 {
            (390, 844, 2, true)
        } else {
            (1440, 900, 1, false)
        };
        ui.page_cdp(
            OWNER,
            &tab,
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":scale,"mobile":mobile}),
        )
        .await
        .unwrap();
        ui.navigate(OWNER, &tab, &format!("{origin}{route}"))
            .await
            .unwrap_or_else(|e| panic!("round {round} {route}: {e:?}"));
        let loaded = if route.starts_with("/sessions/") {
            "!!document.querySelector('.bubble.assistant')"
        } else {
            "!!document.querySelector('.shell')"
        };
        wait(&ui, &tab, &format!("round {round} {route}"), &format!("document.readyState === 'complete' && {loaded} && !document.body.innerText.includes('Loading conversations')")).await;
        if route.starts_with(&format!("/sessions/{login}")) {
            // The sign-in conversation streams its agent tab and shows the code request.
            wait(
                &ui,
                &tab,
                &format!("round {round} code form"),
                "!!document.querySelector('input[autocomplete=one-time-code]')",
            )
            .await;
        }

        let health = ui
            .evaluate(&tab, "(async () => { const t0 = performance.now(); const r = await Promise.race([fetch('/v1/health', { cache: 'no-store', headers: { Authorization: 'Bearer ' + localStorage.getItem('openside.token') } }).then((r) => r.status), new Promise((ok) => setTimeout(() => ok('no answer'), 5000))]); return { status: r, ms: performance.now() - t0 } })()")
            .await
            .unwrap();
        assert_eq!(health["status"], 200, "round {round} {route}: {health}");
        assert!(
            health["ms"].as_f64().unwrap() < 500.0,
            "round {round} {route}: {health}"
        );
        // The page on screen holds at most its session stream and its preview WebSocket.
        let open = ui
            .evaluate(
                &tab,
                "Object.values(JSON.parse(localStorage.getItem('__openStreams') || '{}'))",
            )
            .await
            .unwrap();
        let open = open.as_array().unwrap();
        assert!(open.len() <= 4, "round {round} {route}: {} long-lived connections held, ss shows {} sockets to the daemon:\n{}", open.len(), ui_connections(&port, group).len(), serde_json::to_string_pretty(open).unwrap());
    }

    ui.shutdown().await.unwrap();
    harness.kill().await;
}
