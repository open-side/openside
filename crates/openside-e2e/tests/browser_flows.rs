//! Browser QA flows through the real daemon (public REPL route and event stream) on a
//! loopback fixture site: interaction, dialogs, popups, downloads, uploads, iframes,
//! navigation edge cases, several tabs, daemon restart with the browser alive and renderer
//! crashes. `OPENSIDE_FLOW_BROWSER` picks the Chromium build (the patched build for the
//! agent-tab group, stock otherwise). Every scenario ends in `Harness::kill`, which fails the
//! test when any owned process outlives it.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Path as AxPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Router,
};
use openside_e2e::Harness;
use parking_lot::Mutex;
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn executable() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("OPENSIDE_FLOW_BROWSER") {
        return Some(path.into());
    }
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: browser flows require Chromium: {reason}");
            None
        }
    }
}

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title></head><body>{body}</body></html>"
    ))
}

#[derive(Default)]
struct Seen {
    posts: Vec<String>,
    hits: Vec<String>,
}

type Shared = Arc<Mutex<Seen>>;

struct Site {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn route(
    State(seen): State<Shared>,
    AxPath(name): AxPath<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    seen.lock().hits.push(name.clone());
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    match name.as_str() {
        "form" => page(
            "Form",
            r##"<h1>Form</h1><form method="post" action="/submit">
<label>Name <input name="name" type="text"></label>
<label>Notes <textarea name="notes"></textarea></label>
<label>Colour <select name="colour"><option value="">Pick</option><option value="red">Red</option><option value="blue">Blue</option></select></label>
<label><input type="checkbox" name="agree" value="yes"> I agree</label>
<label><input type="radio" name="size" value="s"> Small</label><label><input type="radio" name="size" value="l"> Large</label>
<button type="submit">Send form</button></form>"##,
        )
        .into_response(),
        "long" => {
            let items: String = (0..300)
                .map(|n| format!("<li>Row {n}</li>"))
                .collect();
            page(
                "Long page",
                &format!(
                    "<h1>Long</h1><ul>{items}</ul><button id=\"bottom\" onclick=\"document.title='Bottom clicked'\">Bottom button</button><div style=\"height:3000px\"></div><button onclick=\"document.title='Far clicked'\">Far button</button>"
                ),
            )
            .into_response()
        }
        "dialogs" => page(
            "Dialogs",
            r##"<h1>Dialogs</h1>
<button onclick="alert('hello alert');document.title='after alert'">Show alert</button>
<button onclick="document.title='confirm:'+confirm('sure?')">Show confirm</button>
<button onclick="document.title='prompt:'+prompt('name?','dflt')">Show prompt</button>
<button onclick="window.onbeforeunload=()=>'leave?';document.title='armed'">Arm beforeunload</button>
<a href="/form">Leave page</a>"##,
        )
        .into_response(),
        "upload" => page(
            "Upload",
            r##"<h1>Upload</h1><input type="file" id="f" aria-label="Pick file" onchange="document.title='chosen:'+this.files.length">
<button onclick="document.title='clicked'">Plain button</button>"##,
        )
        .into_response(),
        "frames" => page(
            "Frames",
            &format!(
                r##"<h1>Frames</h1><iframe id="same" title="same" src="/inner" width="400" height="120"></iframe>
<iframe id="cross" title="cross" src="http://localhost:{}/inner" width="400" height="120"></iframe>
<iframe id="sandbox" title="sandboxed" sandbox="" srcdoc="<button>Sandboxed button</button>"></iframe>
<button onclick="document.title='top clicked'">Top button</button>"##,
                host.rsplit(':').next().unwrap_or("0")
            ),
        )
        .into_response(),
        "inner" => page(
            "Inner",
            r##"<p>Inner frame text</p><button onclick="parent.document.title='inner clicked'">Inner button</button>"##,
        )
        .into_response(),
        "popups" => page(
            "Popups",
            r##"<h1>Popups</h1>
<a href="/form" target="_blank">Blank link</a>
<button onclick="window.p=window.open('/popup-child','kid','width=400,height=300');document.title='open:'+(window.p?'ok':'null')">Open popup</button>
<button onclick="window.p&&window.p.close();document.title='closed:'+(window.p?window.p.closed:'none')">Close popup</button>
<button onclick="var w=window.open('/form');document.title='tab:'+(w?'ok':'null')">Open tab</button>
<button onclick="var n=0;for(let i=0;i<8;i++)if(window.open('/popup-child?'+i))n++;document.title='many:'+n">Open many</button>"##,
        )
        .into_response(),
        "popup-child" => page(
            "Popup child",
            r##"<h1>Child</h1><button onclick="window.close()">Close me</button>"##,
        )
        .into_response(),
        "download" => page(
            "Download",
            r##"<h1>Download</h1><a href="/file.csv" download>Get csv</a> <a href="/file.bin">Get binary</a> <button onclick="location.href='/file.csv'">Navigate to file</button>"##,
        )
        .into_response(),
        "file.csv" => (
            [
                (header::CONTENT_TYPE, "text/csv"),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"data.csv\""),
            ],
            "a,b\n1,2\n",
        )
            .into_response(),
        "file.bin" => (
            [(header::CONTENT_TYPE, "application/octet-stream")],
            vec![0u8; 4096],
        )
            .into_response(),
        "slow" => {
            tokio::time::sleep(Duration::from_secs(3)).await;
            page("Slow", "<h1>Slow page</h1>").into_response()
        }
        "never" => {
            tokio::time::sleep(Duration::from_secs(600)).await;
            page("Never", "<h1>Never</h1>").into_response()
        }
        "redirect" => Redirect::to("/form").into_response(),
        "loop" => Redirect::to("/loop").into_response(),
        "status" => (StatusCode::INTERNAL_SERVER_ERROR, page("Boom", "<h1>Server error</h1>"))
            .into_response(),
        "spa" => page(
            "SPA",
            r##"<h1>SPA</h1><div id="out">start</div><button onclick="history.pushState({}, '', '/spa/next');document.getElementById('out').textContent='routed'">Go next</button>
<button onclick="setTimeout(()=>{document.getElementById('out').innerHTML='<button>Late button</button>'},1200)">Load late</button>"##,
        )
        .into_response(),
        "keys" => page(
            "Keys",
            r##"<h1>Keys</h1><input id="k" aria-label="Key sink" onkeydown="document.title='key:'+event.key">
<div onmouseover="document.getElementById('menu').hidden=false" style="padding:20px">Hover me<div id="menu" hidden><button>Menu item</button></div></div>
<div contenteditable="true" role="textbox" aria-label="Rich">rich</div>"##,
        )
        .into_response(),
        "shadow" => page(
            "Shadow",
            r##"<h1>Shadow</h1><div id="host"></div><script>
const root=document.getElementById('host').attachShadow({mode:'open'});
root.innerHTML='<button onclick="document.title=\'shadow clicked\'">Shadow button</button>';</script>"##,
        )
        .into_response(),
        "crash" => page("Crash", "<h1>Crash target</h1>").into_response(),
        "title" => page(
            "Before",
            r##"<h1>Title</h1><button onclick="document.title='After retitle'">Retitle</button>"##,
        )
        .into_response(),
        "heavy" => {
            let nodes: String = (0..6000)
                .map(|n| format!("<div><span>cell {n}</span><button>b{n}</button></div>"))
                .collect();
            page("Heavy", &nodes).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn submit(State(seen): State<Shared>, body: String) -> impl IntoResponse {
    seen.lock().posts.push(body.clone());
    page(
        "Submitted",
        &format!("<h1>Submitted</h1><p id=\"echo\">{body}</p>"),
    )
}

impl Site {
    async fn start() -> Self {
        let seen = Shared::default();
        let app = Router::new()
            .route(
                "/",
                get(|| async { page("Home", "<h1>Home</h1><a href=\"/form\">Form</a>") }),
            )
            .route("/submit", post(submit))
            .route("/{name}", get(route))
            .route(
                "/spa/next",
                get(|| async { page("SPA next", "<h1>SPA next</h1>") }),
            )
            .with_state(seen);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { origin, task }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }
}

struct Flow {
    harness: Harness,
    site: Site,
}

impl Flow {
    async fn start() -> Option<Self> {
        let executable = executable()?;
        let site = Site::start().await;
        let mut harness = Harness::prepare(DAEMON, &json!({"responses":[]}))?;
        harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
        harness.launch().await;
        Some(Self { harness, site })
    }

    async fn session(&self) -> String {
        let created = self
            .harness
            .request(
                "POST",
                "/v1/sessions",
                Some(json!({"mode":"full","workspace":self.harness.workspace()})),
            )
            .await;
        assert_eq!(created.status, 201, "{}", created.body);
        created.body["id"].as_str().unwrap().to_owned()
    }

    /// One REPL call with a hang guard; returns the response body.
    async fn repl(&self, session: &str, tab: Option<&str>, code: &str) -> Value {
        let mut body = json!({"code":code});
        if let Some(tab) = tab {
            body["tab_id"] = json!(tab);
        }
        let started = std::time::Instant::now();
        let response = tokio::time::timeout(
            Duration::from_secs(90),
            self.harness
                .request("POST", &format!("/v1/sessions/{session}/repl"), Some(body)),
        )
        .await
        .unwrap_or_else(|_| panic!("REPL hung on: {code}"));
        eprintln!(
            "[repl {:>5}ms] {code}\n      -> {} {}",
            started.elapsed().as_millis(),
            response.status,
            response.body
        );
        response.body
    }

    async fn tabs(&self) -> Value {
        self.harness
            .request("GET", "/v1/browser/tabs", None)
            .await
            .body
    }
}

fn value(response: &Value) -> &Value {
    &response["result"]["return_value"]
}

fn tab_of(response: &Value) -> String {
    response["result"]["tab_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no tab in {response}"))
        .to_owned()
}

#[tokio::test]
async fn explore_interaction_and_navigation() {
    let Some(flow) = Flow::start().await else {
        return;
    };
    let s = flow.session().await;
    let r = flow
        .repl(
            &s,
            None,
            &format!(
                "await page.goto('{}'); page.title()",
                flow.site.url("/form")
            ),
        )
        .await;
    let tab = tab_of(&r);
    for code in [
        "await page.getByLabel('Name').fill('Ada'); await page.getByLabel('Notes').fill('line1\\nline2'); await page.getByLabel('Colour').selectOption('blue'); await page.getByLabel('I agree').check(); await page.getByLabel('Large').check(); await page.getByRole('button', {name:'Send form'}).click(); await page.waitForURL(/submit/); page.locator('#echo').textContent()",
        "await page.goBack(); page.url()",
        "await page.goForward(); page.title()",
        "await page.reload(); page.title()",
    ] {
        flow.repl(&s, Some(&tab), code).await;
    }
    let long = flow.site.url("/long");
    for code in [
        format!("await page.goto('{long}'); await page.getByRole('button', {{name:'Bottom button'}}).click(); page.title()"),
        "await page.getByRole('button', {name:'Far button'}).click(); page.title()".to_owned(),
        "await page.keyboard.press('End'); await page.keyboard.press('PageUp'); 1".to_owned(),
        format!("await page.goto('{}'); await page.getByRole('button', {{name:'Go next'}}).click(); [page.url(), await page.locator('#out').textContent()]", flow.site.url("/spa")),
        "await page.getByRole('button', {name:'Load late'}).click(); await page.getByRole('button', {name:'Late button'}).click(); 'late ok'".to_owned(),
        format!("await page.goto('{}'); page.title()", flow.site.url("/redirect")),
        format!("await page.goto('{}'); page.title()", flow.site.url("/status")),
        format!("await page.goto('{}'); page.title()", flow.site.url("/missing")),
        format!("await page.goto('{}'); page.title()", flow.site.url("/loop")),
        format!("await page.goto('{}'); page.title()", flow.site.url("/slow")),
        format!("await page.goto('{}', {{timeout: 2000}}); page.title()", flow.site.url("/never")),
        "await page.goto('http://127.0.0.1:1/'); page.title()".to_owned(),
        format!("await page.goto('{}'); await page.getByLabel('Key sink').press('Tab'); page.title()", flow.site.url("/keys")),
        "await page.getByText('Hover me').hover(); await page.getByRole('button', {name:'Menu item'}).click(); 'hover ok'".to_owned(),
        "await page.getByRole('textbox', {name:'Rich'}).fill('typed rich'); await page.getByRole('textbox', {name:'Rich'}).textContent()".to_owned(),
        format!("await page.goto('{}'); await page.getByRole('button', {{name:'Shadow button'}}).click(); page.title()", flow.site.url("/shadow")),
        format!("await page.goto('{}'); await page.getByRole('button', {{name:'Retitle'}}).click(); page.title()", flow.site.url("/title")),
        format!("await page.goto('{}'); (await page.snapshot()).length", flow.site.url("/heavy")),
    ] {
        flow.repl(&s, Some(&tab), &code).await;
    }
    eprintln!("tabs: {}", flow.tabs().await);
    let mut flow = flow;
    flow.harness.kill().await;
}

/// A click handler that opens a dialog blocks the input call until someone answers it; the
/// daemon answers for the agent (alert and leave-page accepted, confirm and prompt cancelled).
#[tokio::test]
async fn javascript_dialogs_are_answered_and_never_wedge_the_tab() {
    let Some(mut flow) = Flow::start().await else {
        return;
    };
    let s = flow.session().await;
    let opened = flow
        .repl(
            &s,
            None,
            &format!(
                "await page.goto('{}'); await page.title()",
                flow.site.url("/dialogs")
            ),
        )
        .await;
    let tab = tab_of(&opened);
    for (button, title) in [
        ("Show alert", "after alert"),
        ("Show confirm", "confirm:false"),
        ("Show prompt", "prompt:null"),
    ] {
        let result = flow
            .repl(&s, Some(&tab), &format!("await page.getByRole('button', {{name:'{button}'}}).click(); await page.title()"))
            .await;
        assert_eq!(value(&result), title, "{result}");
    }
    let left = flow
        .repl(&s, Some(&tab), "await page.getByRole('button', {name:'Arm beforeunload'}).click(); await page.getByRole('link', {name:'Leave page'}).click(); page.url()")
        .await;
    assert!(
        value(&left)
            .as_str()
            .is_some_and(|url| url.ends_with("/form")),
        "{left}"
    );
    flow.harness.kill().await;
}
