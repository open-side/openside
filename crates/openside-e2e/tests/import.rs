//! B-1: browser data import. Synthetic Chrome and Firefox profiles are imported with the
//! real `openside import` command into two openside profiles; the agent browser launched on
//! each profile must send exactly that profile's cookies to a 127.0.0.1 page.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Html;
use axum::routing::get;
use axum::Router;
use openside_browser::{Browser, BrowserConfig};
use openside_import::testing::{
    BookmarkEntry, ChromeProfile, CookieSpec, FirefoxProfile, HistoryEntry,
};

type Seen = Arc<Mutex<Vec<String>>>;

// Both tests rewrite process-wide HOME/XDG variables, so they cannot overlap.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn page(State(seen): State<Seen>, headers: HeaderMap) -> Html<&'static str> {
    let cookie = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    seen.lock().unwrap().push(cookie);
    Html("<!doctype html><title>cookie page</title><p>ok</p>")
}

/// Point HOME and every XDG directory at the temp root so nothing outside it is touched.
fn isolate(root: &Path, data: &Path, browser: &Path) {
    std::env::set_var("HOME", root);
    for (key, dir) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "xdg-data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        let path = root.join(dir);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    std::env::remove_var("OPENSIDE_DATA_DIR");
    std::env::remove_var("OPENSIDE_BROWSER");
    let config = root.join("config/openside");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        // The discovered browser is pinned: OPENSIDE_BROWSER is cleared above so the test's
        // configuration, not the caller's environment, decides what `import` launches.
        format!(
            "[daemon]\ndata_dir = {:?}\n[browser]\nexecutable = {:?}\n",
            data.to_str().unwrap(),
            browser.to_str().unwrap()
        ),
    )
    .unwrap();
}

async fn import(args: &[&str]) -> (u8, String, String) {
    let mut argv = vec!["openside"];
    argv.extend_from_slice(args);
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = openside_cli::run(argv, &mut stdout, &mut stderr).await;
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

async fn cookies_sent(executable: &Path, profile: &Path, url: &str, seen: &Seen) -> String {
    seen.lock().unwrap().clear();
    let mut config = BrowserConfig::new(executable);
    config.profile_dir = Some(profile.to_owned());
    let (browser, _events) = Browser::launch(config).await.unwrap();
    browser.create_agent_tab("import-e2e", url).await.unwrap();
    let mut cookie = None;
    for _ in 0..100 {
        cookie = seen.lock().unwrap().first().cloned();
        if cookie.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    browser.shutdown().await.unwrap();
    cookie.expect("the browser never requested the page")
}

fn history(url: &str) -> Vec<HistoryEntry> {
    vec![HistoryEntry {
        url: url.into(),
        title: Some("Imported page".into()),
        visits_unix_ms: vec![1_700_000_000_000],
    }]
}

#[tokio::test]
async fn imported_cookies_reach_only_their_own_openside_profile() {
    let _serial = SERIAL.lock().await;
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: browser import acceptance requires Chromium: {reason}");
            return;
        }
    };
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    isolate(root.path(), &data, &executable);

    let seen: Seen = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    let server = tokio::spawn({
        let router = Router::new()
            .route("/page", get(page))
            .with_state(seen.clone());
        async move { axum::serve(listener, router).await.unwrap() }
    });

    let chrome_dir = root.path().join("chrome-Default");
    let mut http_only = CookieSpec::new("127.0.0.1", "chrome_sid", "chrome-secret");
    http_only.http_only = true;
    let mut session = CookieSpec::new("127.0.0.1", "chrome_session", "only-this-session");
    session.expires_unix = None;
    ChromeProfile {
        history: history(&url),
        bookmarks: vec![BookmarkEntry {
            folder: vec!["Work".into()],
            title: "Local".into(),
            url: url.clone(),
        }],
        cookies: vec![
            http_only,
            session,
            CookieSpec::new("other.example.test", "elsewhere", "not-for-this-host"),
        ],
        ..ChromeProfile::default()
    }
    .write(&chrome_dir);
    let firefox_dir = root.path().join("firefox-profile");
    FirefoxProfile {
        history: history(&url),
        bookmarks: Vec::new(),
        cookies: vec![CookieSpec::new("127.0.0.1", "ff_sid", "firefox-secret")],
    }
    .write(&firefox_dir);

    let chrome_from = format!("chrome:{}", chrome_dir.display());
    let (code, out, err) = import(&["import", "--from", &chrome_from, "--into", "alpha"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("cookies: 3 imported"), "{out}");
    assert!(
        !out.contains("chrome-secret"),
        "cookie values are never printed"
    );
    let firefox_from = format!("firefox:{}", firefox_dir.display());
    let (code, out, err) = import(&["import", "--from", &firefox_from, "--into", "beta"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("cookies: 1 imported"), "{out}");

    let profiles = data.join("browser-profiles");
    let alpha = cookies_sent(&executable, &profiles.join("alpha"), &url, &seen).await;
    assert!(
        alpha.contains("chrome_sid=chrome-secret"),
        "alpha sent: {alpha}"
    );
    assert!(
        alpha.contains("chrome_session=only-this-session"),
        "session cookies survive as persistent ones: {alpha}"
    );
    assert!(
        !alpha.contains("ff_sid") && !alpha.contains("elsewhere"),
        "{alpha}"
    );

    let beta = cookies_sent(&executable, &profiles.join("beta"), &url, &seen).await;
    assert!(beta.contains("ff_sid=firefox-secret"), "beta sent: {beta}");
    assert!(
        !beta.contains("chrome_sid"),
        "profile A's cookie leaked into B: {beta}"
    );

    // A profile that was never imported into stays empty.
    let gamma = cookies_sent(&executable, &profiles.join("gamma"), &url, &seen).await;
    assert_eq!(gamma, "");

    // Chromium wrote the cookies with its own encryption: no plaintext value on disk.
    let database = std::fs::read(profiles.join("alpha/Default/Cookies"))
        .or_else(|_| std::fs::read(profiles.join("alpha/Default/Network/Cookies")))
        .unwrap();
    let needle = b"chrome-secret";
    assert!(
        !database
            .windows(needle.len())
            .any(|window| window == needle),
        "the target profile stores the cookie value in plaintext"
    );

    // History and bookmarks are scoped to the profile they were imported into.
    let imports = data.join("browser-imports");
    assert!(imports.join("alpha/history.chrome.jsonl").is_file());
    assert!(imports.join("alpha/bookmarks.chrome.json").is_file());
    assert!(imports.join("beta/history.firefox.jsonl").is_file());
    assert!(!imports.join("beta/history.chrome.jsonl").exists());
    assert!(!imports.join("gamma").exists());

    server.abort();
}

#[tokio::test]
async fn importing_cookies_into_a_profile_in_use_is_refused() {
    let _serial = SERIAL.lock().await;
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: browser import acceptance requires Chromium: {reason}");
            return;
        }
    };
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    isolate(root.path(), &data, &executable);
    let chrome_dir: PathBuf = root.path().join("chrome");
    ChromeProfile {
        cookies: vec![CookieSpec::new("127.0.0.1", "sid", "v")],
        ..ChromeProfile::default()
    }
    .write(&chrome_dir);
    let mut config = BrowserConfig::new(&executable);
    config.profile_dir = Some(data.join("browser-profiles/busy"));
    let (browser, _events) = Browser::launch(config).await.unwrap();

    let from = format!("chrome:{}", chrome_dir.display());
    let (code, _, err) = import(&[
        "import", "--from", &from, "--into", "busy", "--what", "cookies",
    ])
    .await;
    browser.shutdown().await.unwrap();
    assert_eq!(code, 1);
    assert!(
        err.contains("profile `busy` is in use by a running browser"),
        "{err}"
    );
}
