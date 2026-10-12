//! Daemon-level acceptance for B-3 (diff-only repeat snapshots, warnings instead of silent
//! truncation) and B-4 (coordinate input gated like `browser_act`). Real Chromium and loopback
//! fixtures only.

use super::*;
use crate::tools::browser::Controller;
use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserEvent, DownloadStatus, SnapshotPatch};

const LIST: &str = r#"<!doctype html><title>List</title><h1>Tasks</h1>
<button onclick="const li = document.createElement('li'); li.textContent = 'Task ' + (document.querySelectorAll('li').length + 1); document.getElementById('list').append(li)">Add row</button>
<ul id="list"><li>Task 1</li></ul><a href="/">Home</a>"#;

const CANVAS: &str = r#"<!doctype html><title>Canvas</title><body style="margin:0;overflow:hidden">
<canvas id="c" width="1440" height="900" style="display:block"></canvas><script>
const g = document.getElementById('c').getContext('2d');
let clicked = false;
function draw() {
  g.fillStyle = '#ffffff'; g.fillRect(0, 0, 1440, 900);
  g.fillStyle = clicked ? '#00c000' : '#d00000'; g.fillRect(900, 520, 160, 60);
}
draw();
document.getElementById('c').addEventListener('click', (event) => {
  if (event.offsetX >= 900 && event.offsetX < 1060 && event.offsetY >= 520 && event.offsetY < 580) {
    clicked = true; document.title = 'Clicked'; draw();
  }
});
</script></body>"#;

const BUY: &str = r#"<!doctype html><title>Shop</title><body style="margin:0">
<button style="position:absolute;left:100px;top:100px;width:200px;height:50px" onclick="window.bought = true">Buy now</button>
<script>window.bought = false</script></body>"#;

const SWAP: &str = r#"<!doctype html><title>Swap</title><body style="margin:0">
<button id="b" style="position:absolute;left:100px;top:100px;width:200px;height:50px" onclick="window.advanced = true">Next step</button>
<script>window.advanced = false; window.deleted = false;
const b = document.getElementById('b');
b.addEventListener('mousemove', () => { b.textContent = 'Delete account'; b.onclick = () => { window.deleted = true }; }, { once: true });
</script></body>"#;

const COVER: &str = r#"<!doctype html><title>Cover</title><body style="margin:0">
<button style="position:absolute;left:100px;top:100px;width:200px;height:50px" onclick="window.bought = true">Buy now</button>
<div role="note" aria-label="Info" style="position:absolute;left:0;top:0;width:600px;height:400px;pointer-events:none;z-index:5"></div>
<script>window.bought = false</script></body>"#;

const FOCUS: &str = r#"<!doctype html><title>Focus</title><body style="margin:0">
<input id="a" aria-label="Search" style="position:absolute;left:100px;top:100px;width:200px;height:30px">
<input id="b" aria-label="Card number" style="position:absolute;left:100px;top:300px;width:200px;height:30px">
<script>document.getElementById('a').addEventListener('focus', () => document.getElementById('b').focus());</script></body>"#;

// Same fields, but the page also makes `document.activeElement` claim the approved field is
// focused while the native focus is on the other one.
const SPOOF: &str = r#"<!doctype html><title>Spoof</title><body style="margin:0">
<input id="a" aria-label="Search" style="position:absolute;left:100px;top:100px;width:200px;height:30px">
<input id="b" aria-label="Card number" style="position:absolute;left:100px;top:300px;width:200px;height:30px">
<script>
const a = document.getElementById('a'), b = document.getElementById('b');
Object.defineProperty(document, 'activeElement', {get: () => a});
a.addEventListener('focus', () => b.focus());
</script></body>"#;

/// A cross-site child frame (`localhost` inside `127.0.0.1`) renders in another process, so
/// the tab only sees its iframe element.
const FRAMED: &str = r#"<!doctype html><title>Framed</title><body style="margin:0">
<iframe id="f" style="position:absolute;left:0;top:0;width:400px;height:300px;border:0"></iframe>
<script>document.getElementById('f').src = 'http://localhost:' + location.port + '/child';</script></body>"#;

fn child_page(title: &str) -> String {
    format!("<!doctype html><title>{title}</title><body style=\"margin:0\"><button style=\"width:400px;height:300px\">{title}</button></body>")
}

/// The same child, same-site and same-origin: it renders in this tab's process and the point
/// lands inside the child document.
const NEAR: &str = r#"<!doctype html><title>Near</title><body style="margin:0">
<iframe id="f" src="/child" style="position:absolute;left:0;top:0;width:400px;height:300px;border:0"></iframe></body>"#;

fn quotes_page() -> String {
    let mut page = String::from("<!doctype html><title>Quotes</title>");
    for index in 0..30 {
        let label = "\\\"q".repeat(700);
        page.push_str(&format!("<button aria-label='{index} {label}'>x</button><section aria-label='Region {index} {label}'><p>{index}</p><a href='/{index}'>go {index}</a></section>"));
    }
    page
}

/// One accessible name alone is several results long: quotes and backslashes double again in
/// the tool's JSON, and the multi-byte tail makes chunk edges land inside UTF-8 sequences.
fn huge_page() -> String {
    let label = "\\\"é日".repeat(8000);
    format!("<!doctype html><title>Huge</title><h1>Top</h1><button aria-label='{label}'>x</button><a href='/'>Home</a>")
}

fn large_page() -> String {
    let mut page = String::from("<!doctype html><title>Large</title><h1>Directory</h1>");
    for section in 0..20 {
        page.push_str(&format!(
            "<section aria-label=\"Section {section}\"><h2>Section {section}</h2><ul>"
        ));
        for item in 0..100 {
            page.push_str(&format!(
                "<li><a href=\"/entry/{section}/{item}\">Entry {section}-{item} with a descriptive link label</a></li>"
            ));
        }
        page.push_str("</ul></section>");
    }
    page
}

async fn fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let large = large_page();
    let router = Router::new()
        .route("/", get(|| async { Html(LIST) }))
        .route("/canvas", get(|| async { Html(CANVAS) }))
        .route("/buy", get(|| async { Html(BUY) }))
        .route("/swap", get(|| async { Html(SWAP) }))
        .route("/cover", get(|| async { Html(COVER) }))
        .route("/focus", get(|| async { Html(FOCUS) }))
        .route("/spoof", get(|| async { Html(SPOOF) }))
        .route("/quotes", get(|| async { Html(quotes_page()) }))
        .route("/huge", get(|| async { Html(huge_page()) }))
        .route("/framed", get(|| async { Html(FRAMED) }))
        .route("/near", get(|| async { Html(NEAR) }))
        .route("/child", get(|| async { Html(child_page("Child one")) }))
        .route("/child2", get(|| async { Html(child_page("Child two")) }))
        .route("/large", get(move || async move { Html(large) }));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

fn browser_config(root: &Path) -> Option<Resolved> {
    let mut config = resolved_config(root);
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(executable) => {
            config.loaded.config.browser.executable = Some(executable);
            Some(config)
        }
        Err(error) => {
            eprintln!("SKIP daemon browser snapshot/pointer tests: {error}");
            None
        }
    }
}

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

async fn tool(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> ToolOutcome {
    host(daemon, session).await.call(call(name, args, id)).await
}

fn model(outcome: ToolOutcome) -> Value {
    let (ToolOutcome::Done { for_model_json } | ToolOutcome::DoneWithParts { for_model_json, .. }) =
        outcome
    else {
        panic!("expected completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

async fn snapshot(daemon: &Daemon, session: &SessionId, args: Value, id: &str) -> Value {
    model(tool(daemon, session, "browser_snapshot", args, id).await)
}

async fn start(
    daemon: &Daemon,
    session: &SessionId,
    url: &str,
) -> (String, Arc<Browser>, BrowserCleanup) {
    let tab = model(
        tool(
            daemon,
            session,
            "browser_open_tab",
            json!({"url":url}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let cleanup = BrowserCleanup(browser.process_id());
    (tab, browser, cleanup)
}

const DIFF_HEADER: &str = "Diff since this tab's previous snapshot";

/// `(snapshot_id, body)` of a plain-text result: complete trees and diffs.
fn split(result: &Value) -> (String, String) {
    let text = result
        .as_str()
        .unwrap_or_else(|| panic!("expected plain text: {result}"));
    let rest = text
        .strip_prefix("[snapshot_id ")
        .unwrap_or_else(|| panic!("{text}"));
    let (id, rest) = rest.split_once([' ', ']']).unwrap();
    let body = if let Some(diff) = rest.strip_prefix("since ") {
        diff.split_once("] ").unwrap().1
    } else {
        rest.strip_prefix('\n').unwrap()
    };
    (id.to_owned(), body.to_owned())
}

fn apply(base: &str, diff: &str) -> String {
    let (header, body) = diff.split_once('\n').unwrap();
    assert!(header.starts_with(DIFF_HEADER), "{diff}");
    SnapshotPatch::parse(body)
        .expect("diff parses")
        .apply(base)
        .expect("diff applies to the base snapshot")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_snapshot_since_returns_applicable_diffs_with_stable_refs_until_navigation() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "snap", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &origin).await;

    let (first_id, first) = split(&snapshot(&daemon, &session, json!({"tab_id":tab}), "s1").await);
    assert!(
        first.contains("Task 1") && !first.starts_with(DIFF_HEADER),
        "{first}"
    );
    let add = browser
        .snapshot(&tab)
        .await
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.role == "button" && node.name == "Add row")
        .and_then(|node| node.reference)
        .unwrap();
    assert!(first.contains(&format!("[{add}]")), "{first}");

    // Without `since` the caller gets the whole tree: no diff against a base it may not hold.
    let (_, again) = split(&snapshot(&daemon, &session, json!({"tab_id":tab}), "s1b").await);
    assert_eq!(again, first);
    let (unchanged_id, unchanged) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":first_id}),
            "s2",
        )
        .await,
    );
    assert_eq!(
        unchanged,
        "No changes since this tab's previous snapshot.\n"
    );

    for click in ["add-1", "add-2"] {
        model(
            tool(
                &daemon,
                &session,
                "browser_act",
                json!({"tab_id":tab,"target":add,"action":"click"}),
                click,
            )
            .await,
        );
    }
    // A diff result is itself a valid base: base + diff is what the caller now holds.
    let (diff_id, diff) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":unchanged_id}),
            "s3",
        )
        .await,
    );
    assert!(
        diff.starts_with(DIFF_HEADER) && diff.contains("Task 3"),
        "{diff}"
    );
    assert!(
        !diff.contains("Task 1") && !diff.contains("Home"),
        "only changed lines: {diff}"
    );
    let (_, full) = split(&snapshot(&daemon, &session, json!({"tab_id":tab}), "s4").await);
    assert_eq!(
        apply(&first, &diff),
        full,
        "applying the diff yields the full snapshot"
    );
    assert!(
        full.contains(&format!("[{add}]")) && !diff.contains(&format!("[{add}]")),
        "{full}"
    );
    model(
        tool(
            &daemon,
            &session,
            "browser_act",
            json!({"tab_id":tab,"target":add,"action":"click"}),
            "add-3",
        )
        .await,
    );
    let (_, next) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":diff_id}),
            "s5",
        )
        .await,
    );
    assert!(
        next.starts_with(DIFF_HEADER) && next.contains("Task 4"),
        "{next}"
    );

    // A navigation starts a new document: `since` is ignored and the whole tree comes back.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/")}),
            "nav",
        )
        .await,
    );
    let (_, fresh) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":diff_id}),
            "s6",
        )
        .await,
    );
    assert!(
        !fresh.starts_with(DIFF_HEADER) && fresh.contains("Add row"),
        "{fresh}"
    );
    assert!(!fresh.contains("Task 2"), "{fresh}");

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_snapshot_over_budget_pages_one_capture_and_never_diffs_against_a_partial_view() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "large", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &format!("{origin}/large")).await;
    let whole = browser.snapshot(&tab).await.unwrap().text;
    assert!(
        whole.len() > 64 * 1024,
        "fixture must exceed one result: {}",
        whole.len()
    );

    let mut pages = Vec::new();
    let mut cursor = None::<(String, u64)>;
    loop {
        let mut args = json!({"tab_id":tab});
        if let Some((id, line)) = &cursor {
            args["snapshot_id"] = json!(id);
            args["from_line"] = json!(line);
        }
        let result = snapshot(&daemon, &session, args, &format!("page-{}", pages.len())).await;
        assert!(serde_json::to_string(&result).unwrap().len() < 32 * 1024);
        let warning = &result["warning"];
        assert_eq!(warning["code"], "snapshot_over_budget", "{result}");
        assert!(warning["message"]
            .as_str()
            .unwrap()
            .contains("Nothing was dropped"));
        assert!(warning["omitted_lines"].as_u64().unwrap() > 0);
        assert_eq!(
            warning["total_lines"].as_u64().unwrap(),
            whole.lines().count() as u64
        );
        let id = result["snapshot_id"].as_str().unwrap().to_owned();
        let text = result["snapshot"].as_str().unwrap();
        pages.push(text.to_owned());
        if pages.len() == 1 {
            let scopes = warning["scopes"].as_array().unwrap();
            let scope = scopes.last().unwrap();
            let section = scope["name"]
                .as_str()
                .unwrap()
                .strip_prefix("Section ")
                .unwrap();
            assert!(!text.contains(&format!("Entry {section}-0 ")), "{warning}");
            let scoped = snapshot(
                &daemon,
                &session,
                json!({"tab_id":tab,"snapshot_id":id,"ref":scope["ref"]}),
                "scoped",
            )
            .await;
            let (scoped_id, scoped) = split(&scoped);
            assert_eq!(scoped_id, id, "a scope reads the same capture");
            assert!(scoped.contains(&format!("Entry {section}-99 with a descriptive link label")));
            assert_eq!(
                scoped.lines().count() as u64,
                scope["lines"].as_u64().unwrap()
            );
            // Neither a first page nor a scope is a whole tree the caller holds: no diff.
            let retry =
                snapshot(&daemon, &session, json!({"tab_id":tab,"since":id}), "retry").await;
            assert!(retry["warning"].is_object(), "{retry}");
            // Mutating the page between pages must not shift this capture's cursor.
            browser
                .evaluate(&tab, "document.querySelector('section').remove(); true")
                .await
                .unwrap();
        }
        match warning["next_from_line"].as_u64() {
            Some(line) => cursor = Some((id, line)),
            None => break,
        }
    }
    assert!(pages.len() >= 3, "{}", pages.len());
    assert_eq!(
        pages.concat(),
        whole,
        "pages of one capture join to exactly that capture"
    );

    // Once the document is gone, its capture can no longer be paged.
    let (id, line) = cursor.unwrap_or_else(|| ("missing".into(), 1));
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/")}),
            "nav",
        )
        .await,
    );
    let stale = tool(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab,"snapshot_id":id,"from_line":line}),
        "stale",
    )
    .await;
    assert!(!matches!(stale, ToolOutcome::Done { .. }), "{stale:?}");

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_snapshot_results_with_heavy_escaping_stay_structured_under_the_output_limit() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "quotes", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &format!("{origin}/quotes")).await;
    let whole = browser.snapshot(&tab).await.unwrap().text;
    let mut pages = Vec::new();
    let mut cursor = None::<(String, u64)>;
    loop {
        let mut args = json!({"tab_id":tab});
        if let Some((id, line)) = &cursor {
            args["snapshot_id"] = json!(id);
            args["from_line"] = json!(line);
        }
        let result = snapshot(&daemon, &session, args, &format!("q-{}", pages.len())).await;
        let size = serde_json::to_string(&result).unwrap().len();
        assert!(size < 32 * 1024, "{size}");
        assert!(
            result.get("truncated").is_none(),
            "the host never cuts a snapshot"
        );
        let warning = &result["warning"];
        for scope in warning["scopes"].as_array().unwrap() {
            assert!(
                scope["name"].as_str().unwrap().chars().count() <= 81,
                "{scope}"
            );
        }
        pages.push(result["snapshot"].as_str().unwrap().to_owned());
        match warning["next_from_line"].as_u64() {
            Some(line) => cursor = Some((result["snapshot_id"].as_str().unwrap().into(), line)),
            None => break,
        }
    }
    assert_eq!(pages.concat(), whole);

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

/// Inclusive pixel bounds `(x0, y0, x1, y1)`.
type PixelBox = (u32, u32, u32, u32);

/// Width, height and bounding box of the red fixture button in a screenshot, in PNG pixels.
fn button_box(path: &Path) -> (u32, u32, Option<PixelBox>) {
    let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    let channels = frame.color_type.samples();
    assert_eq!(frame.bit_depth, png::BitDepth::Eight);
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for y in 0..frame.height {
        for x in 0..frame.width {
            let at = (y * frame.width + x) as usize * channels;
            let (r, g, b) = (pixels[at], pixels[at + 1], pixels[at + 2]);
            if r > 180 && g < 60 && b < 60 {
                found = Some(match found {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    (frame.width, frame.height, found)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_pointer_clicks_canvas_from_screenshot_and_gates_sensitive_targets() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "pointer", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &format!("{origin}/canvas")).await;

    // Nothing on the canvas is addressable through the accessibility snapshot.
    let tree = snapshot(&daemon, &session, json!({"tab_id":tab}), "canvas-tree").await;
    assert!(!tree.as_str().unwrap().contains("button"), "{tree}");

    let outcome = tool(
        &daemon,
        &session,
        "browser_screenshot",
        json!({"tab_id":tab}),
        "shot-1",
    )
    .await;
    // B-4: the capture reaches vision models as an image part referencing the stored file.
    let ToolOutcome::DoneWithParts { parts, .. } = &outcome else {
        panic!("screenshot without image part: {outcome:?}")
    };
    assert_eq!(
        (
            parts.len(),
            parts[0].kind,
            parts[0].mime.as_str(),
            parts[0].artifact_ref.as_deref()
        ),
        (
            1,
            openside_core::ContentKind::Image,
            "image/png",
            Some("shot-1.png")
        )
    );
    let image = parts[0].data.clone().unwrap();
    assert!(
        image.starts_with(b"\x89PNG") && image.len() <= crate::artifacts::MODEL_IMAGE_MAX_BYTES
    );
    let shot = model(outcome);
    assert_eq!(shot["saved"], true);
    let artifacts = daemon
        .shared
        .config
        .data_dir
        .join(format!("artifacts/{session}"));
    let (width, height, found) = button_box(&artifacts.join("shot-1.png"));
    assert_eq!(
        (width, height),
        (1440, 900),
        "screenshots use the 1440x900 viewport"
    );
    let (x0, y0, x1, y1) = found.expect("the red canvas button is visible");
    let (x, y) = (f64::from(x0 + x1) / 2.0, f64::from(y0 + y1) / 2.0);

    let outside = tool(
        &daemon,
        &session,
        "browser_pointer",
        json!({"tab_id":tab,"action":"click","x":1440,"y":10}),
        "outside",
    )
    .await;
    assert!(matches!(outside, ToolOutcome::Denied { .. }), "{outside:?}");

    let clicked = model(
        tool(
            &daemon,
            &session,
            "browser_pointer",
            json!({"tab_id":tab,"action":"click","x":x,"y":y}),
            "click",
        )
        .await,
    );
    assert_eq!(clicked["navigated"], false, "{clicked}");
    assert_eq!(
        browser.evaluate(&tab, "document.title").await.unwrap(),
        "Clicked"
    );
    model(
        tool(
            &daemon,
            &session,
            "browser_screenshot",
            json!({"tab_id":tab}),
            "shot-2",
        )
        .await,
    );
    let (_, _, after) = button_box(&artifacts.join("shot-2.png"));
    assert_eq!(after, None, "the canvas button turned green");

    // A DOM target under the point gets browser_act's Sensitive gate.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/buy")}),
            "buy-page",
        )
        .await,
    );
    let args = json!({"tab_id":tab,"action":"click","x":200,"y":125});
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::Approval,
    } = tool(&daemon, &session, "browser_pointer", args.clone(), "buy").await
    else {
        panic!("a sensitive target under the point must ask");
    };
    assert_eq!(
        browser.evaluate(&tab, "window.bought").await.unwrap(),
        false
    );
    let response = person_resolve(&daemon, &ticket, json!(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut approved = call("browser_pointer", args, "buy");
    approved.approval_ticket = Some(ticket);
    let done = model(host(&daemon, &session).await.call(approved).await);
    assert_eq!(
        done["hit"],
        json!({"role":"button","name":"Buy now"}),
        "{done}"
    );
    assert_eq!(browser.evaluate(&tab, "window.bought").await.unwrap(), true);

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_pointer_rechecks_every_event_ignores_covers_and_types_only_into_the_hit_element() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "recheck", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &format!("{origin}/swap")).await;

    // Approved as "Next step"; the mousemove handler relabels it "Delete account" before the
    // press, so the press must not be sent.
    let swapped = tool(
        &daemon,
        &session,
        "browser_pointer",
        json!({"tab_id":tab,"action":"click","x":200,"y":125}),
        "swap",
    )
    .await;
    assert!(
        matches!(&swapped, ToolOutcome::Denied { reason, .. } if reason.contains("changed under that point")),
        "{swapped:?}"
    );
    assert_eq!(
        browser.evaluate(&tab, "window.deleted").await.unwrap(),
        false
    );
    assert_eq!(
        browser.evaluate(&tab, "window.advanced").await.unwrap(),
        false
    );

    // A pointer-events:none cover does not receive the click, so it cannot launder the
    // sensitive button underneath.
    let navigate = |path: &'static str, id: &'static str| {
        let args = json!({"tab_id":tab,"url":format!("{origin}{path}")});
        let daemon = &daemon;
        let session = &session;
        async move { model(tool(daemon, session, "browser_navigate", args, id).await) }
    };
    navigate("/cover", "cover").await;
    let covered = tool(
        &daemon,
        &session,
        "browser_pointer",
        json!({"tab_id":tab,"action":"click","x":200,"y":125}),
        "covered",
    )
    .await;
    assert!(
        matches!(
            covered,
            ToolOutcome::Deferred {
                kind: DeferKind::Approval,
                ..
            }
        ),
        "{covered:?}"
    );
    assert_eq!(
        browser.evaluate(&tab, "window.bought").await.unwrap(),
        false
    );

    // A focus handler that moves focus to another field gets no text.
    navigate("/focus", "focus").await;
    let redirected = tool(
        &daemon,
        &session,
        "browser_pointer",
        json!({"tab_id":tab,"action":"type","x":200,"y":115,"text":"4111 1111"}),
        "type",
    )
    .await;
    assert!(
        matches!(&redirected, ToolOutcome::Denied { reason, .. } if reason.contains("focus")),
        "{redirected:?}"
    );
    for field in ["a", "b"] {
        assert_eq!(
            browser
                .evaluate(&tab, &format!("document.getElementById('{field}').value"))
                .await
                .unwrap(),
            ""
        );
    }

    // Page script cannot vouch for the focus: `document.activeElement` is overridden to name
    // the approved field, but the native focus is on the other field and gets no text.
    navigate("/spoof", "spoof").await;
    let spoofed = tool(
        &daemon,
        &session,
        "browser_pointer",
        json!({"tab_id":tab,"action":"type","x":200,"y":115,"text":"4111 1111"}),
        "spoof-type",
    )
    .await;
    assert!(
        matches!(&spoofed, ToolOutcome::Denied { reason, .. } if reason.contains("focus")),
        "{spoofed:?}"
    );
    assert_eq!(
        browser
            .evaluate(&tab, "document.activeElement.id")
            .await
            .unwrap(),
        "a",
        "the fixture's page-visible claim"
    );
    assert_eq!(
        browser
            .evaluate(
                &tab,
                "Object.getOwnPropertyDescriptor(Document.prototype, 'activeElement').get.call(document).id"
            )
            .await
            .unwrap(),
        "b",
        "native focus is on the other field"
    );
    for field in ["a", "b"] {
        assert_eq!(
            browser
                .evaluate(&tab, &format!("document.getElementById('{field}').value"))
                .await
                .unwrap(),
            ""
        );
    }

    // `submit` is not part of the coordinate tool: no implicit form submission.
    let submit = tool(
        &daemon,
        &session,
        "browser_pointer",
        json!({"tab_id":tab,"action":"type","x":200,"y":315,"text":"x","submit":true}),
        "submit",
    )
    .await;
    assert!(!matches!(submit, ToolOutcome::Done { .. }), "{submit:?}");

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_snapshot_since_after_context_compaction_returns_the_full_tree() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "compaction", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &origin).await;
    let add = browser
        .snapshot(&tab)
        .await
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.role == "button" && node.name == "Add row")
        .and_then(|node| node.reference)
        .unwrap();
    let click = |id: &'static str| {
        let args = json!({"tab_id":tab,"target":add,"action":"click"});
        let (daemon, session) = (&daemon, &session);
        async move { model(tool(daemon, session, "browser_act", args, id).await) }
    };

    let (full_id, _) = split(&snapshot(&daemon, &session, json!({"tab_id":tab}), "full").await);
    click("add-1").await;
    let (diff_id, diff) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":full_id}),
            "diff",
        )
        .await,
    );
    assert!(
        diff.starts_with(DIFF_HEADER),
        "before compaction `since` yields a diff: {diff}"
    );

    // The sidecar summarized the conversation: it still names both ids, but the trees behind
    // them (the full snapshot and the diff onto it) are gone from the model's context.
    host(&daemon, &session).await.context_compacted(None);
    click("add-2").await;
    let current = browser.snapshot(&tab).await.unwrap().text;
    for (base, id) in [(&full_id, "since-full"), (&diff_id, "since-diff")] {
        let (_, body) =
            split(&snapshot(&daemon, &session, json!({"tab_id":tab,"since":base}), id).await);
        assert!(!body.starts_with(DIFF_HEADER), "{id}: {body}");
        assert_eq!(body, current, "{id}: the whole current tree");
    }

    // The next complete tree is a base again.
    let (fresh_id, _) = split(&snapshot(&daemon, &session, json!({"tab_id":tab}), "fresh").await);
    click("add-3").await;
    let (_, diff) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":fresh_id}),
            "after",
        )
        .await,
    );
    assert!(
        diff.starts_with(DIFF_HEADER) && diff.contains("Task 4"),
        "{diff}"
    );

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_snapshot_continues_inside_a_line_larger_than_one_result_from_the_same_capture() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "huge", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &format!("{origin}/huge")).await;
    let whole = browser.snapshot(&tab).await.unwrap().text;
    let huge_line = whole.lines().position(|line| line.len() > 60 * 1024);
    let huge_line = huge_line.expect("one accessible name alone exceeds two results") + 1;

    let mut pages = Vec::new();
    let mut inside = 0;
    let mut cursor = None::<(String, u64, Option<u64>)>;
    loop {
        let mut args = json!({"tab_id":tab});
        if let Some((id, line, chars)) = &cursor {
            args["snapshot_id"] = json!(id);
            args["from_line"] = json!(line);
            if let Some(chars) = chars {
                args["from_char"] = json!(chars);
            }
        }
        let result = snapshot(&daemon, &session, args, &format!("huge-{}", pages.len())).await;
        assert!(serde_json::to_string(&result).unwrap().len() < 32 * 1024);
        assert!(
            result.get("truncated").is_none(),
            "the host never cuts a snapshot"
        );
        let warning = &result["warning"];
        assert_eq!(warning["code"], "snapshot_over_budget", "{result}");
        pages.push(result["snapshot"].as_str().unwrap().to_owned());
        if pages.len() == 1 {
            // The page changes under the capture between chunk fetches.
            browser
                .evaluate(
                    &tab,
                    "document.querySelector('button').setAttribute('aria-label', 'mutated'); true",
                )
                .await
                .unwrap();
        }
        let id = result["snapshot_id"].as_str().unwrap().to_owned();
        match (
            warning["next_from_line"].as_u64(),
            warning["next_from_char"].as_u64(),
        ) {
            (Some(line), Some(chars)) => {
                inside += 1;
                assert_eq!(line, huge_line as u64, "{warning}");
                assert!(warning["message"].as_str().unwrap().contains("from_char"));
                cursor = Some((id, line, Some(chars)));
            }
            (Some(line), None) => cursor = Some((id, line, None)),
            (None, _) => break,
        }
    }
    assert!(
        inside >= 3,
        "the huge line took {inside} continuation parts"
    );
    assert_eq!(
        pages.concat(),
        whole,
        "chunks reassemble the immutable capture exactly"
    );
    assert!(!whole.contains("mutated") && !pages.concat().contains("mutated"));

    let (id, _, _) = cursor.unwrap_or_else(|| ("missing".into(), 1, None));
    let past_end = tool(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab,"snapshot_id":id,"from_line":huge_line,"from_char":10_000_000}),
        "past-end",
    )
    .await;
    assert!(
        !matches!(past_end, ToolOutcome::Done { .. }),
        "{past_end:?}"
    );

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restored_tab_download_and_close_reach_its_owner_through_a_late_event_consumer() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "restored", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &origin).await;
    let owner = session.as_str().to_owned();

    // What recovery does with a saved tab list: adopt, and seed the consumer's ledger from the
    // tabs that were actually adopted.
    let ledger = Controller::adopt_saved(
        &browser,
        [
            (tab.clone(), owner.clone()),
            ("gone-tab".into(), owner.clone()),
        ],
    )
    .await;
    assert_eq!(ledger, HashMap::from([(tab.clone(), owner.clone())]));

    // The restored tab starts a download and closes at once: by the time any consumer reads
    // the events, the live tab list has already forgotten it.
    let info = browser
        .tabs()
        .await
        .into_iter()
        .find(|item| item.id == tab)
        .unwrap();
    browser.close_agent_tab(&owner, &tab).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while browser.tabs().await.iter().any(|item| item.id == tab) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let queue = |guid: &str| {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut closed = info.clone();
        closed.owner_session_id = None;
        for event in [
            BrowserEvent::DownloadStarted {
                guid: guid.into(),
                url: format!("{origin}/file"),
                suggested_filename: "file.txt".into(),
                tab_id: Some(tab.clone()),
            },
            BrowserEvent::DownloadFinished {
                guid: guid.into(),
                status: DownloadStatus::Completed,
            },
            BrowserEvent::TabClosed { tab: closed },
        ] {
            sender.send(event).unwrap();
        }
        (sender, receiver)
    };
    let controller = &daemon.shared.registry.browser;
    let (_keep_seeded, seeded) = queue("dl-seeded");
    controller.events(&daemon.shared, seeded, ledger);
    let (_keep_blind, blind) = queue("dl-blind");
    controller.events(&daemon.shared, blind, HashMap::new());

    let seen = |guid: &'static str| {
        let (daemon, session) = (&daemon, &session);
        async move {
            let events = daemon.shared.db.events(session, 0).await.unwrap();
            let started = events.iter().any(|event| {
                matches!(&event.kind, EventKind::ToolUpdated { tool_call_id, .. } if tool_call_id == guid)
            });
            let finished = events.iter().any(|event| {
                matches!(&event.kind, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == guid)
            });
            (started, finished)
        }
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while seen("dl-seeded").await != (true, true) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the restored tab's download start and finish reach its owner");
    // Without the ledger the same events have no owner to go to.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen("dl-blind").await, (false, false));

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_pointer_binds_to_the_child_document_and_never_acts_inside_a_cross_process_frame() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "child-frame", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &format!("{origin}/near")).await;
    let owner = session.as_str();
    let click = openside_browser::PointerAction::Click {
        x: 200.0,
        y: 150.0,
        clicks: 1,
    };
    let hit_until = |done: Box<dyn Fn(&openside_browser::PointHit) -> bool + Send>| {
        let (browser, tab) = (browser.clone(), tab.clone());
        async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok(hit) = browser.hit_test(owner, &tab, 200.0, 150.0).await {
                        if done(&hit) {
                            return hit;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("the expected hit-test result never appeared")
        }
    };

    // A same-site child document: the point lands inside it and the approval records which
    // child document that was.
    let approved = hit_until(Box::new(|hit| {
        hit.labels.iter().any(|label| label.name == "Child one")
    }))
    .await;
    browser
        .pointer(owner, &tab, click.clone(), &approved)
        .await
        .unwrap();
    browser
        .evaluate(&tab, "document.getElementById('f').src = '/child2'; true")
        .await
        .unwrap();
    let replaced = {
        let approved = approved.clone();
        hit_until(Box::new(move |hit| {
            hit != &approved && hit.labels.iter().any(|label| label.name == "Child two")
        }))
        .await
    };
    let refused = browser.pointer(owner, &tab, click.clone(), &approved).await;
    assert!(
        refused.is_err(),
        "the approval of the old child document must not apply"
    );
    browser
        .pointer(owner, &tab, click.clone(), &replaced)
        .await
        .unwrap();

    // A cross-process child (`localhost` inside `127.0.0.1`) cannot be inspected from this tab:
    // the point is refused outright, so no approval exists that a navigating child could
    // outlive.
    browser
        .navigate(owner, &tab, &format!("{origin}/framed"))
        .await
        .unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Err(error) = browser.hit_test(owner, &tab, 200.0, 150.0).await {
                return error;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a point over a cross-process frame is never described");
    assert!(
        matches!(refused, openside_browser::BrowserError::InvalidResponse(_)),
        "{refused:?}"
    );
    assert!(browser
        .pointer(owner, &tab, click, &replaced)
        .await
        .is_err());

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}

/// `(snapshot_id, body)` of a complete viewport result: `[snapshot_id <id> viewport]` + tree.
fn viewport_parts(result: &Value) -> (String, String) {
    let text = result
        .as_str()
        .unwrap_or_else(|| panic!("expected plain text: {result}"));
    let (header, body) = text.split_once('\n').unwrap();
    let id = header
        .strip_prefix("[snapshot_id ")
        .and_then(|rest| rest.strip_suffix(" viewport]"))
        .unwrap_or_else(|| panic!("{header}"));
    (id.to_owned(), body.to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_snapshot_scope_viewport_is_partial_never_a_diff_base_and_keeps_refs() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "viewport", PolicyMode::Guard, None).await;
    let (tab, browser, _cleanup) = start(&daemon, &session, &origin).await;
    let owner = session.as_str().to_owned();

    // A page that fits the screen: the viewport capture is the whole tree, but it says it is
    // a viewport capture and `since` never turns it into a diff.
    let (full_id, full) = split(&snapshot(&daemon, &session, json!({"tab_id":tab}), "v1").await);
    let result = snapshot(
        &daemon,
        &session,
        json!({"tab_id":tab,"scope":"viewport","since":full_id}),
        "v2",
    )
    .await;
    let (viewport_id, viewport) = viewport_parts(&result);
    assert_eq!(viewport, full);
    // Partial by definition, so never the base of a later diff: the caller gets a whole tree.
    let (_, again) = split(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"since":viewport_id}),
            "v3",
        )
        .await,
    );
    assert_eq!(again, full);
    // The capture is paged and scoped by ref like any other, and says it is a viewport capture.
    let add = viewport
        .lines()
        .find(|line| line.contains("Add row"))
        .and_then(|line| line.trim().strip_prefix('['))
        .and_then(|line| line.split(']').next())
        .unwrap()
        .to_owned();
    let subtree = snapshot(
        &daemon,
        &session,
        json!({"tab_id":tab,"snapshot_id":viewport_id,"ref":add}),
        "v4",
    )
    .await;
    assert!(subtree.as_str().unwrap().contains("Add row"), "{subtree}");
    // `scope` belongs to a new capture; a read keeps the scope it was taken with.
    let refused = tool(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab,"snapshot_id":viewport_id,"scope":"viewport"}),
        "v5",
    )
    .await;
    assert!(
        matches!(&refused, ToolOutcome::Denied { reason, .. } if reason.contains("new capture")),
        "{refused:?}"
    );
    // Refs of a viewport capture act as usual.
    model(
        tool(
            &daemon,
            &session,
            "browser_act",
            json!({"tab_id":tab,"target":add,"action":"click"}),
            "v6",
        )
        .await,
    );

    // A long page: the whole tree needs paging, the viewport capture is one small result, and
    // after scrolling it is a different part of the page.
    browser
        .navigate(&owner, &tab, &format!("{origin}/large"))
        .await
        .unwrap();
    let paged = snapshot(&daemon, &session, json!({"tab_id":tab}), "v7").await;
    assert!(paged["warning"].is_object(), "the full page is over budget");
    let full_bytes = paged.to_string().len();
    let (_, top) = viewport_parts(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"scope":"viewport"}),
            "v8",
        )
        .await,
    );
    assert!(
        top.contains("Entry 0-0") && !top.contains("Entry 19-99"),
        "{top}"
    );
    assert!(top.len() * 10 < full_bytes, "{} vs {full_bytes}", top.len());
    browser
        .evaluate(&tab, "window.scrollTo(0, 30000); true")
        .await
        .unwrap();
    let (_, scrolled) = viewport_parts(
        &snapshot(
            &daemon,
            &session,
            json!({"tab_id":tab,"scope":"viewport"}),
            "v9",
        )
        .await,
    );
    assert!(
        scrolled.contains("Entry ") && !scrolled.contains("Entry 0-0"),
        "{scrolled}"
    );

    drop(browser);
    daemon.shutdown().await;
    server.abort();
}
