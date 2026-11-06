//! Daemon-level acceptance for agent-browser downloads (`docs/specs/artifacts.md`
//! § Browser downloads): a click in an agent tab is imported into the session's artifacts,
//! listed by `browser_downloads`, published in lifecycle order, quarantined by name, cancelled
//! in transfer past the per-file ceiling, refused by a read-only policy and bounded per
//! session. Real Chromium and a loopback fixture only.

use super::*;
use axum::{
    body::Bytes,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use openside_browser::Browser;

const PAGE: &str = "<!doctype html><title>Downloads</title>\
<a id=file href=/file.txt>Report</a> <a id=tool href=/tool.sh>Tool</a> \
<a id=big href=/big>Big</a>";
const REPORT: &[u8] = b"downloaded bytes\n";
const SCRIPT: &[u8] = b"#!/bin/sh\necho downloaded\n";

fn attachment(name: &str, body: Body) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        body,
    )
}

async fn site() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route(
            "/file.txt",
            get(|| async { attachment("report.txt", Body::from(REPORT)) }),
        )
        .route(
            "/tool.sh",
            get(|| async { attachment("install.sh", Body::from(SCRIPT)) }),
        )
        // 2 MiB without a length, slowly enough that progress is reported mid-transfer.
        .route(
            "/big",
            get(|| async {
                let chunks = futures_util::stream::unfold(0u32, |sent| async move {
                    if sent == 32 {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Some((
                        Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 64 * 1024])),
                        sent + 1,
                    ))
                });
                attachment("big.bin", Body::from_stream(chunks))
            }),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

fn browser_config(root: &Path) -> Option<Resolved> {
    let mut config = resolved_config(root);
    match test_browser() {
        Ok(executable) => {
            config.loaded.config.browser.executable = Some(executable);
            Some(config)
        }
        Err(error) => {
            eprintln!("SKIP daemon browser download tests: {error}");
            None
        }
    }
}

async fn tool(daemon: &Daemon, session: &SessionId, name: &str, args: Value, id: &str) -> Value {
    let outcome = host(daemon, session).await.call(call(name, args, id)).await;
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected completed {name}: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

async fn open_tab(daemon: &Daemon, session: &SessionId, url: &str, id: &str) -> String {
    tool(daemon, session, "browser_open_tab", json!({"url":url}), id).await["tab_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn shared_browser(daemon: &Daemon) -> Arc<Browser> {
    daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap()
}

/// Clicks a fixture link and returns the guid of the download it started.
async fn download(
    daemon: &Daemon,
    session: &SessionId,
    tab: &str,
    link: &str,
    path: &str,
) -> String {
    download_in(
        &*shared_browser(daemon).await,
        daemon,
        session,
        tab,
        link,
        path,
    )
    .await
}

async fn download_in(
    browser: &Browser,
    daemon: &Daemon,
    session: &SessionId,
    tab: &str,
    link: &str,
    path: &str,
) -> String {
    browser
        .evaluate(
            tab,
            &format!("document.getElementById('{link}').click(); true"),
        )
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
                .find(|download| download.url.ends_with(path))
            {
                return found.guid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("clicking {link} started no download of {path}"))
}

/// Chromium's own file of a download (whole or partial) is gone from the profile.
fn raw_gone(browser: &Browser, guid: &str) -> bool {
    let folder = browser.profile_path().join("downloads");
    [guid.to_owned(), format!("{guid}.crdownload")]
        .iter()
        .all(|name| folder.join(name).symlink_metadata().is_err())
}

/// The settled record of one download, through the tool.
async fn settled(daemon: &Daemon, session: &SessionId, guid: &str, id: &str) -> Value {
    let listed = tool(
        daemon,
        session,
        "browser_downloads",
        json!({"download_id":guid,"wait_seconds":20}),
        id,
    )
    .await;
    assert_eq!(listed["pending"], false, "{listed}");
    let entries = listed["downloads"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{listed}");
    entries[0].clone()
}

/// Lifecycle steps logged on a download's guid, in log order.
async fn lifecycle(daemon: &Daemon, session: &SessionId, guid: &str) -> Vec<String> {
    daemon
        .shared
        .db
        .events(session, 0)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ToolUpdated {
                tool_call_id,
                update,
            } if tool_call_id == guid => update["browser_event"].as_str().map(str::to_owned),
            EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == guid => {
                Some("tool.ended".into())
            }
            EventKind::ArtifactSaved {
                tool_call_id, tool, ..
            } if tool_call_id == guid => Some(format!("artifact.saved:{tool}")),
            _ => None,
        })
        .collect()
}

/// The last lifecycle update is published just after the record settles.
async fn logged(daemon: &Daemon, session: &SessionId, guid: &str, last: &str) -> Vec<String> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let steps = lifecycle(daemon, session, guid).await;
            if steps.last().is_some_and(|step| step == last) {
                return steps;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{guid} never logged {last}"))
}

struct Kill(Vec<u32>);
impl Drop for Kill {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(i32::try_from(*pid).unwrap()),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

#[tokio::test]
async fn agent_downloads_are_imported_listed_quarantined_and_bounded() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = site().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "downloads", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/"), "open").await;
    let _kill = Kill(vec![shared_browser(&daemon).await.process_id()]);

    // A plain-text attachment is saved under the session's artifacts, privately.
    let guid = download(&daemon, &session, &tab, "file", "/file.txt").await;
    let entry = settled(&daemon, &session, &guid, "list-report").await;
    let artifact = format!("downloads/{guid}/report.txt");
    assert_eq!(entry["state"], "saved", "{entry}");
    assert_eq!(entry["artifact"], artifact.as_str());
    assert_eq!(entry["file_name"], "report.txt");
    assert_eq!(entry["size"], REPORT.len());
    assert_eq!(entry["mime"], "text/plain");
    assert_eq!(entry["quarantined"], false);
    assert_eq!(entry["url"], format!("{origin}/file.txt"));
    assert_eq!(entry["tab_id"], tab.as_str());
    let stored = crate::artifacts::session_dir(&daemon.shared, &session).join(&artifact);
    assert_eq!(std::fs::read(&stored).unwrap(), REPORT);
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let read = tool(
        &daemon,
        &session,
        "browser_downloads",
        json!({"download_id":guid,"read_text":true}),
        "read-report",
    )
    .await;
    assert_eq!(read["downloads"][0]["text"], "downloaded bytes\n", "{read}");
    assert_eq!(read["downloads"][0]["truncated"], false);
    assert_eq!(read["downloads"][0]["untrusted"], true);
    assert_eq!(read["downloads"][0]["authority"], "data_only");
    assert_eq!(read["untrusted"], true);
    assert_eq!(read["authority"], "data_only");
    assert_eq!(
        logged(&daemon, &session, &guid, "download.saved").await,
        [
            "download.started",
            "tool.ended",
            "artifact.saved:browser_download",
            "download.saved"
        ]
    );
    // Chromium's own file is gone once the import settled; saving the download still works,
    // from the imported copy.
    let browser = shared_browser(&daemon).await;
    assert!(raw_gone(&browser, &guid));
    let saved = browser
        .save_download(session.as_str(), &guid, Some("copy.txt"))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), REPORT);
    drop(browser);
    // The REPL's saveAs, once the import has released the file, copies the imported artifact.
    let repl = tool(
        &daemon,
        &session,
        "browser_repl",
        json!({"tab_id": tab, "code": "const [download] = await Promise.all([page.waitForEvent('download'), page.click('#file')]);\nawait page.waitForTimeout(1500);\nawait download.saveAs('via-repl.txt')"}),
        "repl-save",
    )
    .await;
    let path = repl["return_value"]
        .as_str()
        .unwrap_or_else(|| panic!("{repl}"));
    assert!(path.ends_with("via-repl.txt"), "{repl}");
    assert_eq!(std::fs::read(path).unwrap(), REPORT);
    let repl_guid = daemon
        .shared
        .registry
        .browser
        .downloads
        .of(&session)
        .last()
        .unwrap()
        .guid
        .clone();
    assert_ne!(repl_guid, guid);
    assert_eq!(
        settled(&daemon, &session, &repl_guid, "list-repl").await["state"],
        "saved"
    );
    assert!(raw_gone(&*shared_browser(&daemon).await, &repl_guid));

    // A script is kept but offered for download only.
    let script = download(&daemon, &session, &tab, "tool", "/tool.sh").await;
    let entry = settled(&daemon, &session, &script, "list-script").await;
    assert_eq!(entry["state"], "saved", "{entry}");
    assert_eq!(entry["file_name"], "install.sh");
    assert_eq!(entry["quarantined"], true);
    let listed = tool(
        &daemon,
        &session,
        "browser_downloads",
        json!({}),
        "list-all",
    )
    .await;
    assert_eq!(listed["downloads"].as_array().unwrap().len(), 3, "{listed}");

    // Past the per-file ceiling the browser cancels the transfer and nothing is kept.
    shared_browser(&daemon).await.set_download_limit(Some(1000));
    let big = download(&daemon, &session, &tab, "big", "/big").await;
    let entry = settled(&daemon, &session, &big, "list-big").await;
    assert_eq!(entry["state"], "failed", "{entry}");
    assert_eq!(entry["error"], "too_large");
    assert_eq!(
        logged(&daemon, &session, &big, "download.failed").await,
        ["download.started", "tool.ended", "download.failed"]
    );
    assert!(!crate::artifacts::session_dir(&daemon.shared, &session)
        .join("downloads")
        .join(&big)
        .exists());
    assert!(raw_gone(&*shared_browser(&daemon).await, &big));
    shared_browser(&daemon)
        .await
        .set_download_limit(Some(crate::downloads::MAX_FILE_BYTES));

    // A read-only session may browse but not write its downloads folder.
    let read_only = create_session(&daemon, "read-only", PolicyMode::ReadOnly, None).await;
    let viewer = open_tab(&daemon, &read_only, &format!("{origin}/"), "open-ro").await;
    let refused = download(&daemon, &read_only, &viewer, "file", "/file.txt").await;
    let entry = settled(&daemon, &read_only, &refused, "list-ro").await;
    assert_eq!(entry["state"], "failed", "{entry}");
    assert_eq!(entry["error"], "policy");
    assert!(!crate::artifacts::session_dir(&daemon.shared, &read_only)
        .join("downloads")
        .exists());
    assert!(raw_gone(&*shared_browser(&daemon).await, &refused));

    // The per-session total counts the files already imported.
    let used = (2 * REPORT.len() + SCRIPT.len()) as u64;
    let over = crate::artifacts::store_download(
        &daemon.shared,
        &session,
        "quota",
        "downloads/quota/extra.txt",
        vec![b'x'; 10],
        "text/plain",
        used + 9,
    )
    .await;
    assert!(matches!(
        over,
        Err(crate::artifacts::DownloadRefusal::SessionQuota)
    ));
    let fits = crate::artifacts::store_download(
        &daemon.shared,
        &session,
        "quota",
        "downloads/quota/extra.txt",
        vec![b'x'; 10],
        "text/plain",
        used + 10,
    )
    .await;
    assert!(matches!(fits, Ok(10)));

    daemon.shutdown().await;
    fixture.abort();
}

/// A private chat's download is imported like any other, and Chromium's file leaves the
/// temporary profile as soon as the import settles, not only when the profile is deleted.
#[tokio::test]
async fn private_downloads_leave_no_file_in_the_temporary_profile() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = site().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let private = SessionId::new("private-downloads");
    let stored = private.clone();
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session_with_privacy(
                stored,
                "Private",
                std::any::type_name::<TestRuntime>(),
                PolicyMode::Full,
                None,
                true,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let tab = open_tab(&daemon, &private, &format!("{origin}/"), "open").await;
    let incognito = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &private)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _kill = Kill(vec![incognito.process_id()]);

    let guid = download_in(&incognito, &daemon, &private, &tab, "file", "/file.txt").await;
    let entry = settled(&daemon, &private, &guid, "list-private").await;
    assert_eq!(entry["state"], "saved", "{entry}");
    assert!(raw_gone(&incognito, &guid));
    assert!(incognito.profile_path().join("downloads").is_dir());
    let saved = crate::artifacts::session_dir(&daemon.shared, &private)
        .join(format!("downloads/{guid}/report.txt"));
    assert_eq!(std::fs::read(saved).unwrap(), REPORT);

    incognito.set_download_limit(Some(1000));
    let big = download_in(&incognito, &daemon, &private, &tab, "big", "/big").await;
    let entry = settled(&daemon, &private, &big, "list-private-big").await;
    assert_eq!(entry["error"], "too_large", "{entry}");
    assert!(raw_gone(&incognito, &big));

    drop(incognito);
    daemon.shutdown().await;
    fixture.abort();
}

/// A download that settles after its session was deleted leaves no record and no event: the
/// lifecycle update is ordered against deletion by the artifact gate.
#[tokio::test]
async fn downloads_settling_after_deletion_leave_no_record_or_event() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "doomed", PolicyMode::Full, None).await;
    let started = openside_browser::BrowserEvent::DownloadStarted {
        guid: "g-live".into(),
        url: "https://files.example/a.txt".into(),
        suggested_filename: "a.txt".into(),
        tab_id: None,
    };
    crate::downloads::observe(&daemon.shared, &session, None, &started).await;
    assert_eq!(
        daemon.shared.registry.browser.downloads.of(&session).len(),
        1
    );

    daemon
        .shared
        .command(&session, crate::engine::Action::Tombstone)
        .await
        .unwrap();
    crate::session_delete::on_session_deleted(&daemon.shared, &session).await;
    assert!(daemon
        .shared
        .registry
        .browser
        .downloads
        .of(&session)
        .is_empty());

    let finished = openside_browser::BrowserEvent::DownloadFinished {
        guid: "g-live".into(),
        status: openside_browser::DownloadStatus::Canceled,
    };
    crate::downloads::observe(&daemon.shared, &session, None, &finished).await;
    crate::downloads::observe(&daemon.shared, &session, None, &started).await;
    assert!(daemon
        .shared
        .registry
        .browser
        .downloads
        .of(&session)
        .is_empty());
    let events = daemon
        .shared
        .db
        .events(&session, 0)
        .await
        .unwrap_or_default();
    assert!(
        !events.iter().any(|event| matches!(
            &event.kind,
            EventKind::ToolUpdated { tool_call_id, .. } if tool_call_id == "g-live"
        )),
        "no download event may follow deletion"
    );
    daemon.shutdown().await;
}

/// Only the first terminal report settles a download: a repeated `completed` starts no second
/// import, and a `canceled` after `completed` neither overwrites the outcome nor publishes.
#[tokio::test]
async fn repeated_or_late_terminal_reports_change_nothing() {
    use openside_browser::{BrowserEvent, DownloadStatus};
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "twice", PolicyMode::Full, None).await;
    let guid = "g-twice";
    let finished = |status| BrowserEvent::DownloadFinished {
        guid: guid.into(),
        status,
    };
    let started = BrowserEvent::DownloadStarted {
        guid: guid.into(),
        url: "https://files.example/a.txt".into(),
        suggested_filename: "a.txt".into(),
        tab_id: None,
    };
    let shared = &daemon.shared;
    crate::downloads::observe(shared, &session, None, &started).await;
    // No browser: the one import this starts fails as `unavailable`.
    crate::downloads::observe(shared, &session, None, &finished(DownloadStatus::Completed)).await;
    crate::downloads::observe(shared, &session, None, &finished(DownloadStatus::Canceled)).await;
    crate::downloads::observe(shared, &session, None, &finished(DownloadStatus::Completed)).await;
    let state = || {
        let list = shared.registry.browser.downloads.of(&session);
        assert_eq!(list.len(), 1);
        crate::downloads::describe(&list[0], None)
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while state()["state"] != "failed" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the import settles");
    crate::downloads::observe(shared, &session, None, &finished(DownloadStatus::Canceled)).await;
    crate::downloads::observe(shared, &session, None, &started).await;
    assert_eq!(state()["state"], "failed");
    assert_eq!(state()["error"], "unavailable", "{}", state());
    let events = shared.db.events(&session, 0).await.unwrap();
    let updates: Vec<&Value> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolUpdated {
                tool_call_id,
                update,
            } if tool_call_id == guid => Some(update),
            _ => None,
        })
        .collect();
    assert_eq!(updates.len(), 1, "{updates:?}");
    assert_eq!(updates[0]["browser_event"], "download.failed");
    assert_eq!(updates[0]["error"], "unavailable");
    daemon.shutdown().await;
}
