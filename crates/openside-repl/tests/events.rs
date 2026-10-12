//! Page events and files of the REPL against real Chromium: JavaScript dialogs answered from
//! `page.on('dialog')` while the click that opened them is still pending, popups, downloads
//! saved into the session folder, file choosers, `setInputFiles` and the policy gates around
//! uploads and downloads.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use openside_browser::{Browser, BrowserConfig, BrowserEvent};
use openside_repl::{run, Gate, Guard, Limits, Output, Stop, Verdict};
use serde_json::{json, Value};
use tokio::{
    sync::{mpsc::UnboundedReceiver, Mutex},
    task::JoinHandle,
};

const OWNER: &str = "repl-events";
static SERIAL: Mutex<()> = Mutex::const_new(());

const PAGE: &str = r#"<!doctype html><title>Events</title>
<button id="alert" onclick="alert('hello')">Alert</button>
<button id="confirm" onclick="document.title = 'confirm:' + confirm('sure?')">Confirm</button>
<button id="prompt" onclick="document.title = 'prompt:' + prompt('name?', 'def')">Prompt</button>
<a id="get" href="/file.txt" download>Get</a>
<a id="missing" href="/stalled.txt" download>Stalled</a>
<button id="pop" onclick="window.open('/blank', 'popup-window')">Pop</button>
<button id="later" onclick="setTimeout(() => window.open('/blank', 'later-window'), 300)">Later</button>
<input id="file" type="file">
<input id="many" type="file" multiple>
<input id="chooser" type="file" style="position:fixed;right:0;bottom:0;width:100px;height:40px">
<button id="pick" onclick="document.getElementById('chooser').click()">Pick</button>
<input id="text" type="text">"#;

/// Allows everything except the transfers the test restricts, like the daemon's guard does for
/// uploads (a folder allow-list) and downloads.
struct FilesGuard {
    root: PathBuf,
    deny_downloads: bool,
    uploads: parking_lot::Mutex<Vec<String>>,
    downloads: parking_lot::Mutex<Vec<(String, String)>>,
}

impl Guard for FilesGuard {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        match gate {
            Gate::Upload { path, .. } => {
                self.uploads.lock().push((*path).to_owned());
                let path = Path::new(path);
                let plain = path.is_absolute()
                    && path.components().all(|part| part != Component::ParentDir);
                if plain && path.starts_with(&self.root) {
                    Verdict::Allow
                } else {
                    Verdict::Deny(format!("{} is outside the allowed folder", path.display()))
                }
            }
            Gate::Download {
                url,
                suggested_name,
                ..
            } => {
                self.downloads
                    .lock()
                    .push(((*url).to_owned(), (*suggested_name).to_owned()));
                if self.deny_downloads {
                    Verdict::Deny("downloads are off".into())
                } else {
                    Verdict::Allow
                }
            }
            _ => Verdict::Allow,
        }
    }
}

struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    base: String,
    root: PathBuf,
    guard: Arc<FilesGuard>,
    controller: UnboundedReceiver<BrowserEvent>,
    server: JoinHandle<()>,
}

fn routes() -> Router {
    Router::new()
        .route("/page", get(|| async { Html(PAGE) }))
        .route(
            "/blank",
            get(|| async { Html("<!doctype html><title>Blank</title>popup body") }),
        )
        .route(
            "/file.txt",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "application/octet-stream"),
                        (
                            header::CONTENT_DISPOSITION,
                            "attachment; filename=\"served.txt\"",
                        ),
                    ],
                    "served bytes\n",
                )
                    .into_response()
            }),
        )
        .fallback(|| async { StatusCode::NOT_FOUND })
}

impl Fixture {
    async fn start(name: &str, deny_downloads: bool) -> Option<Self> {
        let executable = match std::env::var_os("OPENSIDE_FLOW_BROWSER") {
            Some(path) => path.into(),
            None => match openside_config::discover_browser(&openside_config::Config::default()) {
                Ok(path) => path,
                Err(reason) => {
                    eprintln!("SKIP: REPL events require Chromium: {reason}");
                    return None;
                }
            },
        };
        let (browser, controller) = Browser::launch(BrowserConfig::new(executable))
            .await
            .expect("launch chromium");
        let browser = Arc::new(browser);
        let tab = browser
            .create_agent_tab(OWNER, "about:blank")
            .await
            .expect("agent tab")
            .id;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, routes()).await.unwrap();
        });
        let root =
            std::env::temp_dir().join(format!("openside-repl-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        Some(Self {
            browser,
            tab,
            base,
            guard: Arc::new(FilesGuard {
                root: root.clone(),
                deny_downloads,
                uploads: parking_lot::Mutex::default(),
                downloads: parking_lot::Mutex::default(),
            }),
            root,
            controller,
            server,
        })
    }

    async fn run(&self, code: &str) -> Output {
        run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            format!(
                "const BASE = {:?}; const ROOT = {:?};\n{code}",
                self.base,
                self.root.to_str().unwrap()
            ),
            self.guard.clone(),
            Limits::default(),
        )
        .await
    }

    async fn value(&self, code: &str) -> Value {
        let output = self.run(code).await;
        assert_eq!(output.error, None, "{code}\n{output:?}");
        output.return_value
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        std::fs::remove_dir_all(&self.root).ok();
    }
}

/// Dialogs of every kind are answered from a handler while the click that opened them is still
/// pending, and the handler sees the type, the text and a prompt's default.
#[tokio::test]
async fn dialogs_are_answered_from_the_handler_during_the_click() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("dialogs", false).await else {
        return;
    };
    let value = fixture
        .value(
            r#"
const seen = [];
page.on('dialog', async (dialog) => {
  seen.push([dialog.type(), dialog.message(), dialog.defaultValue()]);
  if (dialog.type() === 'prompt') await dialog.accept('Ada');
  else if (dialog.type() === 'confirm') await dialog.dismiss();
  else await dialog.accept();
});
await page.goto(BASE + '/page');
await page.click('#alert');
await page.click('#confirm');
const confirmed = await page.title();
await page.click('#prompt');
({ seen, confirmed, prompted: await page.title() })
"#,
        )
        .await;
    assert_eq!(
        value,
        json!({
            "seen": [["alert", "hello", ""], ["confirm", "sure?", ""], ["prompt", "name?", "def"]],
            "confirmed": "confirm:false",
            "prompted": "prompt:Ada",
        })
    );
}

/// A handler that answers nothing does not freeze the page: alerts are accepted and confirms
/// dismissed, as the daemon does without a script. `once` handlers run once and `off` removes
/// one; `waitForEvent` hands the dialog to the script, which answers it itself.
#[tokio::test]
async fn unanswered_dialogs_get_the_default_answer_and_waiters_get_the_dialog() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("dialog-defaults", false).await else {
        return;
    };
    let value = fixture
        .value(
            r#"
await page.goto(BASE + '/page');
let calls = 0;
const handler = () => { calls += 1; };
page.on('dialog', handler);
await page.click('#confirm');
const afterIgnored = await page.title();
await page.click('#alert');
page.off('dialog', handler);

const waiting = page.waitForEvent('dialog');
const click = page.click('#prompt');
const dialog = await waiting;
const kind = dialog.type();
await dialog.accept('Bob');
await click;
const waited = await page.title();
let twice;
try { await dialog.accept(); } catch (error) { twice = error.message; }

let timeout;
try { await page.waitForEvent('dialog', { timeout: 150 }); } catch (error) { timeout = error.name; }
({ calls, afterIgnored, kind, waited, twice, timeout })
"#,
        )
        .await;
    assert_eq!(value["calls"], 2);
    assert_eq!(value["afterIgnored"], "confirm:false");
    assert_eq!(value["kind"], "prompt");
    assert_eq!(value["waited"], "prompt:Bob");
    assert!(
        value["twice"].as_str().unwrap().contains("already handled"),
        "{value}"
    );
    assert_eq!(value["timeout"], "TimeoutError");
}

/// Once the script ends its claim on the tab's dialogs is gone, also when it failed.
#[tokio::test]
async fn the_dialog_claim_does_not_outlive_the_script() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("claim", false).await else {
        return;
    };
    for code in [
        "page.on('dialog', (d) => d.accept()); 1",
        "page.on('dialog', (d) => d.accept()); throw new Error('boom')",
        "page.waitForEvent('dialog', { timeout: 0 }); 1",
    ] {
        let output = fixture.run(code).await;
        assert!(output.stop.is_none(), "{output:?}");
        let claim = fixture.browser.claim_dialogs(OWNER, &fixture.tab).await;
        assert!(
            claim.is_ok(),
            "claim still held after {code:?}: {:?}",
            claim.err()
        );
    }
    // Unknown events fail loudly instead of silently never firing.
    let output = fixture.run("page.on('websocket', () => {})").await;
    assert!(
        output.error.unwrap().starts_with("NotSupported"),
        "websocket"
    );
}

/// A dialog nobody is waiting for blocks input and script commands; the script is told why
/// instead of hanging.
#[tokio::test]
async fn an_open_dialog_is_named_in_the_error_of_a_blocked_command() {
    let _serial = SERIAL.lock().await;
    let Some(mut fixture) = Fixture::start("blocked", false).await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/page')").await;
    let output = fixture
        .run(
            r#"
await page.evaluate(() => { setTimeout(() => alert('stuck'), 0); });
await page.waitForTimeout(300);
await page.title()
"#,
        )
        .await;
    let error = output
        .error
        .expect("title must fail while the dialog is open");
    assert!(error.starts_with("DialogError"), "{error}");
    assert!(error.contains("stuck"), "{error}");
    // The controller (the daemon in production) is the one that answers it.
    let mut seen = false;
    while let Ok(event) = fixture.controller.try_recv() {
        if matches!(event, BrowserEvent::DialogOpened { ref message, .. } if message == "stuck") {
            seen = true;
        }
    }
    assert!(seen, "an unclaimed dialog reaches the controller");
    fixture
        .browser
        .handle_dialog(&fixture.tab, true, None)
        .await
        .unwrap();
}

/// A popup is awaited next to the click that opens it, also when it opens later, and is
/// readable but is not a page the script can drive.
#[tokio::test]
async fn popups_are_waited_for_and_stay_read_only() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("popups", false).await else {
        return;
    };
    let value = fixture
        .value(
            r#"
await page.goto(BASE + '/page');
const [popup] = await Promise.all([page.waitForEvent('popup'), page.click('#pop')]);
const first = { url: popup.url().replace(BASE, ''), title: await popup.title() };
const [later] = await Promise.all([page.waitForEvent('popup'), page.click('#later')]);
let missing;
try { await page.waitForEvent('popup', { timeout: 200 }); } catch (error) { missing = error.name; }
let driven;
try { popup.click('a'); } catch (error) { driven = error.name; }
({ first, later: later.url().replace(BASE, ''), missing, driven })
"#,
        )
        .await;
    assert_eq!(value["first"], json!({"url": "/blank", "title": "Blank"}));
    assert_eq!(value["later"], "/blank");
    assert_eq!(value["missing"], "TimeoutError");
    assert_eq!(value["driven"], "NotSupported");
    let tabs = fixture.browser.tabs().await;
    let popups: Vec<_> = tabs
        .iter()
        .filter(|tab| tab.id != fixture.tab && tab.owner_session_id.as_deref() == Some(OWNER))
        .collect();
    assert_eq!(
        popups.len(),
        2,
        "popups belong to the opener's session: {tabs:?}"
    );
}

/// `saveAs` copies the download into the session's folder under a plain name; paths in the name
/// are refused, and a Gate denial stops the script before anything is saved.
#[tokio::test]
async fn downloads_are_saved_by_name_into_the_session_folder() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("downloads", false).await else {
        return;
    };
    let value = fixture
        .value(
            r#"
await page.goto(BASE + '/page');
const [download] = await Promise.all([page.waitForEvent('download'), page.click('#get')]);
const named = await download.saveAs('report.txt');
const failure = await download.failure();
const fallback = await download.path();
let escaped;
try { await download.saveAs('../escape.txt'); } catch (error) { escaped = error.message; }
({ suggested: download.suggestedFilename(), url: download.url().replace(BASE, ''), named, failure, fallback, escaped })
"#,
        )
        .await;
    let folder = fixture.browser.session_downloads(OWNER).await.unwrap();
    assert_eq!(value["suggested"], "served.txt");
    assert_eq!(value["url"], "/file.txt");
    assert_eq!(value["named"], folder.join("report.txt").to_str().unwrap());
    assert_eq!(value["failure"], Value::Null);
    assert_eq!(
        value["fallback"],
        folder.join("report.txt").to_str().unwrap()
    );
    assert!(value["escaped"].as_str().is_some(), "{value}");
    assert_eq!(
        std::fs::read(folder.join("report.txt")).unwrap(),
        b"served bytes\n"
    );
    assert!(!folder.parent().unwrap().join("escape.txt").exists());
    assert_eq!(
        fixture.guard.downloads.lock()[0].1,
        "served.txt",
        "the gate saw the suggested name"
    );
}

#[tokio::test]
async fn a_denied_download_stops_the_script() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("download-denied", true).await else {
        return;
    };
    let output = fixture
        .run(
            r#"
await page.goto(BASE + '/page');
const [download] = await Promise.all([page.waitForEvent('download'), page.click('#get')]);
await download.saveAs('report.txt');
"after"
"#,
        )
        .await;
    assert!(matches!(output.stop, Some(Stop::Denied(_))), "{output:?}");
    let folder = fixture.browser.session_downloads(OWNER).await.unwrap();
    assert!(!folder.join("report.txt").exists());
}

/// `setInputFiles` takes absolute paths inside the allowed folder; everything else is refused
/// before the page sees it.
#[tokio::test]
async fn set_input_files_is_confined_to_the_allowed_folder() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("upload", false).await else {
        return;
    };
    let one = fixture.root.join("one.txt");
    let two = fixture.root.join("two.txt");
    std::fs::write(&one, "1").unwrap();
    std::fs::write(&two, "22").unwrap();
    let value = fixture
        .value(
            r#"
await page.goto(BASE + '/page');
await page.setInputFiles('#file', ROOT + '/one.txt');
const single = await page.locator('#file').evaluate((el) => Array.from(el.files).map((f) => f.name + ':' + f.size));
await page.locator('#many').setInputFiles([ROOT + '/one.txt', ROOT + '/two.txt']);
const several = await page.locator('#many').evaluate((el) => Array.from(el.files).map((f) => f.name + ':' + f.size));
await page.setInputFiles('#many', []);
const cleared = await page.locator('#many').evaluate((el) => el.files.length);
const errors = {};
for (const [label, run] of [
  ['relative', () => page.setInputFiles('#file', 'one.txt')],
  ['missing', () => page.setInputFiles('#file', ROOT + '/none.txt')],
  ['folder', () => page.setInputFiles('#file', ROOT)],
  ['text', () => page.setInputFiles('#text', ROOT + '/one.txt')],
  ['buffer', () => page.setInputFiles('#file', { name: 'a.txt', mimeType: 'text/plain', buffer: 'x' })],
]) {
  try { await run(); errors[label] = 'no error'; } catch (error) { errors[label] = error.name; }
}
({ single, several, cleared, errors })
"#,
        )
        .await;
    assert_eq!(value["single"], json!(["one.txt:1"]));
    assert_eq!(value["several"], json!(["one.txt:1", "two.txt:2"]));
    assert_eq!(value["cleared"], 0);
    assert_eq!(value["errors"]["buffer"], "NotSupported");
    for label in ["relative", "missing", "folder", "text"] {
        let name = value["errors"][label].as_str().unwrap();
        assert!(
            name != "no error" && name != "NotSupported",
            "{label}: {name}"
        );
    }
}

#[cfg(unix)]
/// A path outside the allow-list, a `..` detour and a symlink that leaves the folder are all
/// denied by the Gate, and the denial stops the script.
#[tokio::test]
async fn uploads_outside_the_allowed_folder_are_denied() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("upload-denied", false).await else {
        return;
    };
    let outside = std::env::temp_dir().join(format!("openside-outside-{}.txt", std::process::id()));
    std::fs::write(&outside, "secret").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.root.join("link.txt")).unwrap();
    for attempt in [
        outside.to_str().unwrap().to_owned(),
        format!(
            "{}/../{}",
            fixture.root.display(),
            outside.file_name().unwrap().to_str().unwrap()
        ),
        format!("{}/link.txt", fixture.root.display()),
    ] {
        let output = fixture
            .run(&format!(
                "await page.goto(BASE + '/page'); await page.setInputFiles('#file', {attempt:?}); 'uploaded'"
            ))
            .await;
        assert!(
            matches!(output.stop, Some(Stop::Denied(_))),
            "{attempt} must be denied: {output:?}"
        );
        assert_ne!(output.return_value, json!("uploaded"));
    }
    let attached = fixture
        .value("await page.locator('#file').evaluate((el) => el.files.length)")
        .await;
    assert_eq!(attached, 0, "nothing reached the input");
    std::fs::remove_file(outside).ok();
}

/// A file chooser the page opens is handed to the script, which chooses the files; the script's
/// end turns interception off again.
#[tokio::test]
async fn file_choosers_are_answered_from_the_script() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start("chooser", false).await else {
        return;
    };
    std::fs::write(fixture.root.join("pick.txt"), "pick").unwrap();
    let value = fixture
        .value(
            r#"
await page.goto(BASE + '/page');
const [chooser] = await Promise.all([page.waitForEvent('filechooser'), page.click('#pick')]);
await chooser.setFiles(ROOT + '/pick.txt');
({ multiple: chooser.isMultiple(), files: await page.locator('#chooser').evaluate((el) => Array.from(el.files).map((f) => f.name)) })
"#,
        )
        .await;
    assert_eq!(value, json!({"multiple": false, "files": ["pick.txt"]}));
    let after = fixture
        .browser
        .page_command(
            OWNER,
            &fixture.tab,
            "Page.setInterceptFileChooserDialog",
            json!({"enabled": false}),
        )
        .await;
    assert!(after.is_ok());
}
