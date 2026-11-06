//! Edges of agent-browser downloads the end-to-end acceptance test does not reach
//! (`docs/specs/artifacts.md` § Browser downloads): the per-session ceiling under concurrent
//! writers, files that grow or are swapped for links after Chromium finished them, policy
//! admission of the starting page, the source origin and the downloads folder separately,
//! downloads with no origin at all, how an imported file leaves through the authenticated
//! raw route, and how the artifact list marks quarantined ones. Most browser cases drive a
//! standalone headless Chromium whose events are handed to `downloads::observe` by the test,
//! so each import starts exactly when the test has prepared the file on disk; the page-origin
//! case runs through the daemon's own browser and event pump.

use super::*;
use axum::{
    http::header,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use openside_browser::{Browser, BrowserEvent, DownloadInfo, DownloadStatus, PageEvent};
use std::path::PathBuf;

use crate::downloads::{Failure, State};

const REPORT: &[u8] = b"downloaded bytes\n";
const HTML: &[u8] = b"<!doctype html><title>x</title><script>document.title='ran'</script>";

fn long_name() -> String {
    // Exactly at the 120-byte ceiling before the device-name prefix is added.
    format!("CON.{}.pdf", "x".repeat(112))
}

async fn site() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let attachment = |name: String| {
        (
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
            ],
            REPORT,
        )
            .into_response()
    };
    let router = Router::new()
        .route(
            "/",
            get(|| async {
                Html("<!doctype html><a id=file href=/file.txt>f</a> <a id=long href=/long>l</a>")
            }),
        )
        .route(
            "/file.txt",
            get(move || async move { attachment("report.txt".into()) }),
        )
        .route("/long", get(move || async move { attachment(long_name()) }));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

struct Kill(u32);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

async fn standalone_browser(root: &Path) -> Option<Arc<Browser>> {
    let executable = match test_browser() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("SKIP download gap browser tests: {error}");
            return None;
        }
    };
    let mut config = openside_browser::BrowserConfig::new(executable);
    config.profile_dir = Some(root.join("profile"));
    config.die_with_spawner = true;
    Some(Arc::new(Browser::launch(config).await.unwrap().0))
}

async fn loaded(browser: &Browser, tab: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while browser
            .evaluate(
                tab,
                "document.readyState === 'complete' && !!document.getElementById('long')",
            )
            .await
            .ok()
            != Some(json!(true))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("fixture page never loaded");
}

/// Clicks a fixture link and returns the browser's record once Chromium completed the file.
async fn fetched(browser: &Browser, tab: &str, link: &str) -> DownloadInfo {
    started_by(
        browser,
        tab,
        &format!("document.getElementById('{link}').click(); true"),
    )
    .await
}

/// Runs `script` in the tab and returns the record of the download it started, once completed.
async fn started_by(browser: &Browser, tab: &str, script: &str) -> DownloadInfo {
    let mut events = browser.page_events();
    browser.evaluate(tab, script).await.unwrap();
    let guid = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Ok(PageEvent::DownloadStarted(info)) => return info.guid,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("page events ended: {error}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{script} started no download"));
    let status = browser
        .wait_download(&guid, std::time::Instant::now() + Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(status, DownloadStatus::Completed);
    browser.download(&guid).unwrap()
}

fn raw_file(browser: &Browser, download: &DownloadInfo) -> PathBuf {
    browser
        .profile_path()
        .join("downloads")
        .join(&download.guid)
}

/// Hands the download's lifecycle to the daemon, as a pump would, and waits for its outcome.
async fn import(
    daemon: &Daemon,
    session: &SessionId,
    browser: &Arc<Browser>,
    download: &DownloadInfo,
) -> State {
    let started = BrowserEvent::DownloadStarted {
        guid: download.guid.clone(),
        url: download.url.clone(),
        suggested_filename: download.suggested_filename.clone(),
        tab_id: download.tab_id.clone(),
    };
    crate::downloads::observe(&daemon.shared, session, Some(browser.clone()), &started).await;
    let finished = BrowserEvent::DownloadFinished {
        guid: download.guid.clone(),
        status: DownloadStatus::Completed,
    };
    crate::downloads::observe(&daemon.shared, session, Some(browser.clone()), &finished).await;
    settle(daemon, session, &download.guid).await.state
}

/// The daemon's record of `guid` once it is saved or failed.
async fn settle(daemon: &Daemon, session: &SessionId, guid: &str) -> crate::downloads::Download {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(found) = daemon
                .shared
                .registry
                .browser
                .downloads
                .of(session)
                .into_iter()
                .find(|record| record.guid == guid)
            {
                if matches!(found.state, State::Saved(_) | State::Failed(_)) {
                    return found;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{guid} never settled"))
}

fn kept(daemon: &Daemon, session: &SessionId, guid: &str) -> bool {
    crate::artifacts::session_dir(&daemon.shared, session)
        .join(crate::downloads::ARTIFACT_DIR)
        .join(guid)
        .exists()
}

async fn set_policy(daemon: &Daemon, deny_origins: Value, rules: Value) {
    let settings = serde_json::from_value::<PolicySettings>(json!({
        "default_mode": "full",
        "allow_origins": [],
        "deny_origins": deny_origins,
        "rules": rules,
    }))
    .unwrap();
    crate::policy_api::replace_locked(&daemon.shared.policy, settings)
        .await
        .unwrap();
}

/// Admission checks the source origin and the downloads folder independently; the importer
/// reads at most one byte past the per-file ceiling and refuses a file that grew past it after
/// Chromium finished; a raw file swapped for a link is never followed; a page-chosen name at
/// the byte ceiling stays one safe component that keeps its extension.
#[tokio::test]
async fn imports_check_policy_size_and_links_on_the_file_as_it_is_on_disk() {
    let root = tempfile::tempdir().unwrap();
    let Some(browser) = standalone_browser(root.path()).await else {
        return;
    };
    let _kill = Kill(browser.process_id());
    let (origin, fixture) = site().await;
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "gaps", PolicyMode::Full, None).await;
    let tab = browser
        .create_agent_tab(session.as_str(), &format!("{origin}/"))
        .await
        .unwrap();
    loaded(&browser, &tab.id).await;
    let folder = crate::artifacts::session_dir(&daemon.shared, &session)
        .join(crate::downloads::ARTIFACT_DIR);

    // The source origin denied, the folder writable: refused before any byte is read.
    set_policy(&daemon, json!([origin]), json!([])).await;
    let denied_source = fetched(&browser, &tab.id, "file").await;
    assert_eq!(
        import(&daemon, &session, &browser, &denied_source).await,
        State::Failed(Failure::Policy)
    );
    assert!(!kept(&daemon, &session, &denied_source.guid));

    // The origin allowed, writing the session's downloads folder denied.
    set_policy(
        &daemon,
        json!([]),
        json!([{"effect":"deny","capability":"fs_write","pattern":folder.to_string_lossy()}]),
    )
    .await;
    let denied_folder = fetched(&browser, &tab.id, "file").await;
    assert_eq!(
        import(&daemon, &session, &browser, &denied_folder).await,
        State::Failed(Failure::Policy)
    );
    assert!(!kept(&daemon, &session, &denied_folder.guid));

    // Both allowed: saved.
    set_policy(&daemon, json!([]), json!([])).await;
    let allowed = fetched(&browser, &tab.id, "file").await;
    let State::Saved(saved) = import(&daemon, &session, &browser, &allowed).await else {
        panic!("an allowed download must be saved");
    };
    assert_eq!(
        saved.artifact,
        format!("downloads/{}/report.txt", allowed.guid)
    );
    assert_eq!(
        std::fs::read(folder.join(&allowed.guid).join("report.txt")).unwrap(),
        REPORT
    );

    // Chromium announced and wrote 17 bytes; the file on disk grew past the ceiling since.
    let grown = fetched(&browser, &tab.id, "file").await;
    assert_eq!(grown.received_bytes, REPORT.len() as u64);
    std::fs::OpenOptions::new()
        .write(true)
        .open(raw_file(&browser, &grown))
        .unwrap()
        .set_len(crate::downloads::MAX_FILE_BYTES + 1)
        .unwrap();
    assert_eq!(
        import(&daemon, &session, &browser, &grown).await,
        State::Failed(Failure::TooLarge)
    );
    assert!(!kept(&daemon, &session, &grown.guid));

    // The raw file swapped for a link to a file outside the profile: nothing is read.
    let outside = root.path().join("outside-secret.txt");
    std::fs::write(&outside, b"outside secret").unwrap();
    let swapped = fetched(&browser, &tab.id, "file").await;
    let raw = raw_file(&browser, &swapped);
    std::fs::remove_file(&raw).unwrap();
    std::os::unix::fs::symlink(&outside, &raw).unwrap();
    assert_eq!(
        import(&daemon, &session, &browser, &swapped).await,
        State::Failed(Failure::Unavailable)
    );
    assert!(!kept(&daemon, &session, &swapped.guid));
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside secret");

    // A 120-byte device name keeps the ceiling and its extension after the `_` prefix.
    let long = fetched(&browser, &tab.id, "long").await;
    assert_eq!(long.suggested_filename, long_name());
    let State::Saved(saved) = import(&daemon, &session, &browser, &long).await else {
        panic!("a long name must be saved under a safe name");
    };
    assert_eq!(saved.file_name.len(), 120, "{}", saved.file_name);
    assert!(
        saved.file_name.starts_with("_CON.") && saved.file_name.ends_with(".pdf"),
        "{}",
        saved.file_name
    );
    assert!(!saved.file_name.contains('/'));
    assert_eq!(
        std::fs::read(folder.join(&long.guid).join(&saved.file_name)).unwrap(),
        REPORT
    );

    daemon.shutdown().await;
    fixture.abort();
    if let Ok(browser) = Arc::try_unwrap(browser) {
        browser.shutdown().await.ok();
    }
}

/// A download with no origin of its own, started from a page with none either (`about:blank`),
/// leaves policy nothing to judge: the import fails closed even under a policy allowing all.
#[tokio::test]
async fn downloads_with_no_origin_to_judge_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let Some(browser) = standalone_browser(root.path()).await else {
        return;
    };
    let _kill = Kill(browser.process_id());
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "originless", PolicyMode::Full, None).await;
    let tab = browser
        .create_agent_tab(session.as_str(), "about:blank")
        .await
        .unwrap();
    let download = started_by(&browser, &tab.id, DATA_SCRIPT).await;
    assert!(download.url.starts_with("data:"), "{}", download.url);
    assert!(
        download
            .page_url
            .as_deref()
            .and_then(openside_policy::origin_of)
            .is_none(),
        "{:?}",
        download.page_url
    );
    assert_eq!(
        import(&daemon, &session, &browser, &download).await,
        State::Failed(Failure::Policy)
    );
    assert!(!kept(&daemon, &session, &download.guid));
    daemon.shutdown().await;
    if let Ok(browser) = Arc::try_unwrap(browser) {
        browser.shutdown().await.ok();
    }
}

// Function-scoped, so the same script can run again in one document.
const DATA_SCRIPT: &str = "(() => { const a = document.createElement('a'); \
    a.href = 'data:text/plain,hi'; a.download = 'd.txt'; document.body.appendChild(a); \
    a.click(); return true; })()";
const BLOB_SCRIPT: &str = "(() => { const a = document.createElement('a'); \
    a.href = URL.createObjectURL(new Blob(['x'], {type: 'text/plain'})); a.download = 'b.txt'; \
    document.body.appendChild(a); a.click(); return true; })()";

async fn made_here_site() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = Router::new().route(
        "/",
        get(|| async { Html("<!doctype html><title>Made here</title><p>page</p>") }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

async fn agent_tool(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> Value {
    let outcome = host(daemon, session).await.call(call(name, args, id)).await;
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected completed {name}: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

async fn agent_browser(daemon: &Daemon) -> Arc<Browser> {
    daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap()
}

/// Runs `script` in an agent tab and returns the guid of the download the daemon's pump
/// recorded for it.
async fn pumped(daemon: &Daemon, session: &SessionId, tab: &str, script: &str) -> String {
    let known: Vec<String> = daemon
        .shared
        .registry
        .browser
        .downloads
        .of(session)
        .into_iter()
        .map(|download| download.guid)
        .collect();
    agent_browser(daemon)
        .await
        .evaluate(tab, script)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(found) = daemon
                .shared
                .registry
                .browser
                .downloads
                .of(session)
                .into_iter()
                .find(|download| !known.contains(&download.guid))
            {
                return found.guid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{script} started no download"))
}

/// `blob:` and `data:` downloads have no origin of their own, so the page that made them is
/// judged: with that page's origin denied both fail as `policy`, allowed they are saved. Runs
/// through the daemon's own browser and event pump, which records the starting page.
#[tokio::test]
async fn blob_and_data_downloads_are_judged_by_the_page_that_made_them() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    match test_browser() {
        Ok(executable) => config.loaded.config.browser.executable = Some(executable),
        Err(error) => {
            eprintln!("SKIP page-origin download test: {error}");
            return;
        }
    }
    let (origin, fixture) = made_here_site().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "made-here", PolicyMode::Full, None).await;
    let tab = agent_tool(
        &daemon,
        &session,
        "browser_open_tab",
        json!({"url": format!("{origin}/")}),
        "open",
    )
    .await["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _kill = Kill(agent_browser(&daemon).await.process_id());
    let folder = crate::artifacts::session_dir(&daemon.shared, &session)
        .join(crate::downloads::ARTIFACT_DIR);

    // Denied after the page opened, so only the downloads are judged against it.
    set_policy(&daemon, json!([origin]), json!([])).await;
    for script in [BLOB_SCRIPT, DATA_SCRIPT] {
        let guid = pumped(&daemon, &session, &tab, script).await;
        let record = settle(&daemon, &session, &guid).await;
        assert_eq!(
            record.page_origin.as_deref(),
            Some(origin.as_str()),
            "{script}"
        );
        assert_eq!(record.state, State::Failed(Failure::Policy), "{script}");
        assert!(!kept(&daemon, &session, &guid), "{script}");
    }

    set_policy(&daemon, json!([]), json!([])).await;
    for (script, name, bytes) in [
        (BLOB_SCRIPT, "b.txt", &b"x"[..]),
        (DATA_SCRIPT, "d.txt", &b"hi"[..]),
    ] {
        let guid = pumped(&daemon, &session, &tab, script).await;
        let record = settle(&daemon, &session, &guid).await;
        let State::Saved(saved) = record.state else {
            panic!("{script}: {:?}", record.state);
        };
        assert_eq!(saved.file_name, name);
        assert_eq!(std::fs::read(folder.join(&guid).join(name)).unwrap(), bytes);
    }

    daemon.shutdown().await;
    fixture.abort();
}

/// Imports racing each other and another artifact writer: the lifecycle gate makes the
/// per-session ceiling exact among downloads, refused imports leave nothing behind, and other
/// artifact kinds are outside the download ceiling by design.
#[tokio::test]
async fn concurrent_imports_fill_the_session_ceiling_exactly() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "quota", PolicyMode::Full, None).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    let mut env = crate::tools::ToolEnv::new(daemon.shared.clone(), &state);
    env.tool_call_id = "writer".into();
    const LIMIT: u64 = 100;
    let store = |n: usize| {
        let shared = daemon.shared.clone();
        let session = session.clone();
        async move {
            let guid = format!("race-{n}");
            crate::artifacts::store_download(
                &shared,
                &session,
                &guid,
                &format!("downloads/{guid}/part.bin"),
                vec![b'x'; 30],
                "application/octet-stream",
                LIMIT,
            )
            .await
        }
    };
    let other = vec![b'y'; 4096];
    let (a, b, c, d, e, written) = tokio::join!(
        store(0),
        store(1),
        store(2),
        store(3),
        store(4),
        crate::artifacts::save_tool_output(&env, "write", "files/other.bin", other.clone()),
    );
    written.unwrap();
    let results = [a, b, c, d, e];
    let stored: Vec<usize> = (0..5).filter(|n| results[*n].is_ok()).collect();
    assert_eq!(stored.len(), 3, "30-byte imports into a 100-byte ceiling");
    for (n, result) in results.iter().enumerate() {
        match result {
            Ok(size) => assert_eq!(*size, 30),
            Err(crate::artifacts::DownloadRefusal::SessionQuota) => {
                assert!(!kept(&daemon, &session, &format!("race-{n}")));
            }
            Err(crate::artifacts::DownloadRefusal::Unavailable) => panic!("race-{n} unavailable"),
        }
    }
    let dir = crate::artifacts::session_dir(&daemon.shared, &session);
    let total: u64 = std::fs::read_dir(dir.join("downloads"))
        .unwrap()
        .flatten()
        .flat_map(|guid| std::fs::read_dir(guid.path()).unwrap().flatten())
        .map(|file| file.metadata().unwrap().len())
        .sum();
    assert_eq!(total, 90);
    assert_eq!(std::fs::read(dir.join("files/other.bin")).unwrap(), other);

    // The 4 KiB tool output does not count: the last 10 bytes still fit, one more does not.
    let last = |size: usize, guid: &'static str| {
        crate::artifacts::store_download(
            &daemon.shared,
            &session,
            guid,
            match guid {
                "fits" => "downloads/fits/last.bin",
                _ => "downloads/over/last.bin",
            },
            vec![b'z'; size],
            "application/octet-stream",
            LIMIT,
        )
    };
    assert!(matches!(
        last(11, "over").await,
        Err(crate::artifacts::DownloadRefusal::SessionQuota)
    ));
    assert!(!kept(&daemon, &session, "over"));
    assert!(matches!(last(10, "fits").await, Ok(10)));
    daemon.shutdown().await;
}

/// The authenticated raw route serves an imported download by its encoded path with the
/// sniffed type, refuses it without a token and refuses traversal; an `.html` download is never
/// served as HTML, with the same attachment headers an uploaded `.html` gets.
#[tokio::test]
async fn imported_downloads_leave_only_through_the_authenticated_raw_route_as_attachments() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "served", PolicyMode::Full, None).await;
    let other = create_session(&daemon, "neighbour", PolicyMode::Full, None).await;
    for (session, guid, rel, bytes) in [
        (&session, "g-text", "downloads/g-text/report.txt", REPORT),
        (&session, "g-html", "downloads/g-html/x.html", HTML),
        (&other, "g-secret", "downloads/g-secret/secret.txt", REPORT),
    ] {
        assert!(crate::artifacts::store_download(
            &daemon.shared,
            session,
            guid,
            rel,
            bytes.to_vec(),
            "text/plain",
            crate::downloads::MAX_SESSION_BYTES,
        )
        .await
        .is_ok());
    }
    let tcp = api::router(daemon.shared.clone(), false);
    let unix = api::router(daemon.shared.clone(), true);
    let read = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap()
        .1;
    let get = |router: &axum::Router, path: String, token: Option<&str>| {
        let request = request("GET", &path, json!({}), token);
        let router = router.clone();
        async move {
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let headers = response.headers().clone();
            let body = response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec();
            (status, headers, body)
        }
    };
    let raw = |session: &str, rel: &str| {
        format!(
            "/v1/sessions/{session}/artifacts/{}",
            rel.replace('/', "%2F")
        )
    };

    let (status, headers, body) = get(
        &tcp,
        raw("served", "downloads/g-text/report.txt"),
        Some(&read),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, REPORT);
    assert_eq!(headers["content-type"], "text/plain");
    let (status, _, _) = get(&tcp, raw("served", "downloads/g-text/report.txt"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for traversal in [
        "downloads%2F..%2F..%2Fneighbour%2Fdownloads%2Fg-secret%2Fsecret.txt",
        "downloads%2Fg-text%2F..%2F..%2F..%2Fneighbour%2Fdownloads%2Fg-secret%2Fsecret.txt",
        "..%2Fneighbour%2Fdownloads%2Fg-secret%2Fsecret.txt",
        "downloads%2F%2E%2E%2F%2E%2E%2Fneighbour%2Fdownloads%2Fg-secret%2Fsecret.txt",
    ] {
        let (status, _, body) = get(
            &tcp,
            format!("/v1/sessions/served/artifacts/{traversal}"),
            Some(&read),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{traversal}");
        assert_ne!(body, REPORT, "{traversal}");
    }

    // An uploaded `.html` with the same bytes is the reference for how such a file leaves.
    let response = unix
        .clone()
        .oneshot(request(
            "POST",
            "/v1/sessions/served/messages",
            json!({"message_id":"m1","text":"page","mode":"queue","attachments":[
                {"name":"x.html","data_base64":base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD, HTML)},
            ]}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let listed = json_body(
        unix.clone()
            .oneshot(request(
                "GET",
                "/v1/sessions/served/artifacts",
                json!({}),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let uploaded = listed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|artifact| {
            let path = artifact["path"].as_str()?;
            path.starts_with("attachments/").then(|| path.to_owned())
        })
        .unwrap_or_else(|| panic!("no uploaded attachment in {listed}"));
    let (_, upload_headers, _) = get(&tcp, raw("served", &uploaded), Some(&read)).await;

    let (status, headers, body) =
        get(&tcp, raw("served", "downloads/g-html/x.html"), Some(&read)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, HTML);
    let content_type = headers["content-type"].to_str().unwrap();
    assert!(!content_type.contains("html"), "{content_type}");
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(
        csp.starts_with("sandbox")
            && !csp.contains("allow-same-origin")
            && !csp.contains("allow-scripts"),
        "{csp}"
    );
    for name in [
        "content-type",
        "x-content-type-options",
        "content-security-policy",
        "cache-control",
    ] {
        assert_eq!(headers.get(name), upload_headers.get(name), "{name}");
    }
    assert!(upload_headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));

    // Nor previewed as HTML: the signed preview is typed by the same sniffing.
    let preview = crate::artifacts::signed_preview_url(
        &session,
        "downloads/g-html/x.html",
        Utc::now().timestamp() + 600,
    );
    let (status, headers, _) = get(&tcp, preview, None).await;
    assert!(
        status == StatusCode::UNSUPPORTED_MEDIA_TYPE
            || (status == StatusCode::OK
                && !headers["content-type"].to_str().unwrap().contains("html")),
        "{status} {headers:?}"
    );
    daemon.shutdown().await;
}

/// The artifact list re-derives quarantine from the stored name and sniffed type exactly as
/// the importer decided it, and offers no preview for a quarantined file; a file of the same
/// name outside `downloads/` is not a download and stays previewable.
#[tokio::test]
async fn the_artifact_list_marks_quarantined_downloads_and_offers_them_no_preview() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "listed", PolicyMode::Full, None).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    let mut env = crate::tools::ToolEnv::new(daemon.shared.clone(), &state);
    env.tool_call_id = "writer".into();
    let cases: [(&str, &[u8], bool); 5] = [
        ("install.sh", b"#!/bin/sh\necho hi\n", true),
        ("page.html", HTML, true),
        ("blob.dat", &[0, 159, 146, 150], true),
        ("report.txt", REPORT, false),
        ("report.pdf", b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n", false),
    ];
    for (n, (name, bytes, quarantined)) in cases.iter().enumerate() {
        let mime = crate::downloads::sniffed_mime(bytes);
        assert_eq!(
            crate::downloads::quarantined(name, mime),
            *quarantined,
            "{name}"
        );
        let guid = format!("g-{n}");
        assert!(crate::artifacts::store_download(
            &daemon.shared,
            &session,
            &guid,
            &format!("downloads/{guid}/{name}"),
            bytes.to_vec(),
            mime,
            crate::downloads::MAX_SESSION_BYTES,
        )
        .await
        .is_ok());
    }
    crate::artifacts::save_tool_output(&env, "write", "files/install.sh", b"echo hi\n".to_vec())
        .await
        .unwrap();
    let listed = json_body(
        api::router(daemon.shared.clone(), true)
            .oneshot(request(
                "GET",
                "/v1/sessions/listed/artifacts",
                json!({}),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let entry = |path: &str| {
        listed["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|artifact| artifact["path"] == path)
            .cloned()
            .unwrap_or_else(|| panic!("{path} not in {listed}"))
    };
    for (n, (name, _, quarantined)) in cases.iter().enumerate() {
        let artifact = entry(&format!("downloads/g-{n}/{name}"));
        assert_eq!(
            artifact["quarantined"].as_bool().unwrap_or(false),
            *quarantined,
            "{artifact}"
        );
        if *quarantined {
            assert!(artifact.get("preview_url").is_none(), "{artifact}");
        } else {
            assert!(artifact.get("quarantined").is_none(), "{artifact}");
            assert!(artifact["preview_url"].is_string(), "{artifact}");
        }
    }
    let written = entry("files/install.sh");
    assert!(written.get("quarantined").is_none(), "{written}");
    assert!(written["preview_url"].is_string(), "{written}");
    daemon.shutdown().await;
}
