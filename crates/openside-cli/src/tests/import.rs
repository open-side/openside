use super::*;
use openside_import::testing::{
    BookmarkEntry, ChromeProfile, CookieSpec, Encryption, FirefoxProfile, HistoryEntry,
};
use std::path::Path;

async fn cli(args: &[&str]) -> (u8, String, String) {
    let mut argv = vec![OsString::from("openside")];
    argv.extend(args.iter().map(OsString::from));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_with_input(argv, &mut std::io::empty(), &mut stdout, &mut stderr).await;
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

/// A temp HOME/XDG tree whose configuration points the data directory inside it.
fn data_dir(environment: &Environment) -> PathBuf {
    let root = environment.directory.path();
    let data = root.join("data");
    std::fs::create_dir_all(root.join("openside")).unwrap();
    std::fs::write(
        root.join("openside/config.toml"),
        format!("[daemon]\ndata_dir = {:?}\n", data.to_str().unwrap()),
    )
    .unwrap();
    data
}

fn chrome(dir: &Path, encryption: Encryption) {
    ChromeProfile {
        history: vec![
            HistoryEntry {
                url: "http://127.0.0.1:1/".into(),
                title: Some("Home".into()),
                visits_unix_ms: vec![1_700_000_000_000],
            },
            HistoryEntry {
                url: "about:blank".into(),
                title: None,
                visits_unix_ms: vec![1_700_000_001_000],
            },
        ],
        bookmarks: vec![BookmarkEntry {
            folder: vec!["Dev".into()],
            title: "Docs".into(),
            url: "https://docs.rs/".into(),
        }],
        cookies: vec![
            CookieSpec::new("127.0.0.1", "sid", "abc123"),
            CookieSpec::new("example.test", "pref", "dark"),
        ],
        encryption,
        ..ChromeProfile::default()
    }
    .write(dir);
}

#[tokio::test]
async fn import_command_writes_history_and_bookmarks_and_reports_counts() {
    let environment = Environment::new().await;
    let data = data_dir(&environment);
    let source = environment.directory.path().join("Default");
    chrome(&source, Encryption::V10);
    let from = format!("chrome:{}", source.display());

    let (code, out, err) = cli(&[
        "import",
        "--from",
        &from,
        "--into",
        "work",
        "--what",
        "history,bookmarks",
    ])
    .await;
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("Imported from chrome into profile `work`:"),
        "{out}"
    );
    assert!(out.contains("history: 1 imported, 1 skipped"), "{out}");
    assert!(out.contains("bookmarks: 1 imported"), "{out}");
    assert!(!out.contains("cookies"), "{out}");
    let imports = data.join("browser-imports/work");
    assert!(imports.join("history.chrome.jsonl").is_file());
    assert!(imports.join("bookmarks.chrome.json").is_file());
    assert!(!data.join("browser-imports/other").exists());

    let (code, out, err) = cli(&[
        "--json",
        "import",
        "--from",
        &from,
        "--into",
        "work",
        "--what",
        "bookmarks",
        "--dry-run",
    ])
    .await;
    assert_eq!(code, 0, "{err}");
    let report: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["profile"], "work");
    assert_eq!(report["bookmarks"]["imported"], 1);
    assert!(report.get("history").is_none());
}

#[tokio::test]
async fn dry_run_decrypts_cookies_with_a_keyring_command_and_writes_nothing() {
    let environment = Environment::new().await;
    let data = data_dir(&environment);
    let source = environment.directory.path().join("Default");
    chrome(&source, Encryption::V11("keyring-secret".into()));
    let from = format!("chromium:{}", source.display());
    let base = [
        "import",
        "--from",
        &from,
        "--into",
        "p",
        "--what",
        "cookies",
        "--dry-run",
    ];

    let (code, _, err) = cli(&base).await;
    assert_eq!(code, 1);
    assert!(
        err.contains("v11") && err.contains("--keyring-password-cmd"),
        "{err}"
    );

    let mut wrong = base.to_vec();
    wrong.extend(["--keyring-password-cmd", "echo wrong-password"]);
    let (code, out, err) = cli(&wrong).await;
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("keyring password is wrong"), "{err}");
    assert!(
        !err.contains("wrong-password"),
        "the password must not be echoed: {err}"
    );

    let mut right = base.to_vec();
    right.extend(["--keyring-password-cmd", "echo keyring-secret"]);
    let (code, out, err) = cli(&right).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("Dry run, nothing written."), "{out}");
    assert!(out.contains("cookies: 2 imported"), "{out}");
    assert!(!out.contains("keyring-secret") && !out.contains("abc123"));
    assert!(!data.exists());
}

#[tokio::test]
async fn failures_name_the_cause() {
    let environment = Environment::new().await;
    let data = data_dir(&environment);
    let root = environment.directory.path().to_owned();
    let run = |from: String, what: &'static str| async move {
        cli(&[
            "import",
            "--from",
            &from,
            "--into",
            "t",
            "--what",
            what,
            "--dry-run",
        ])
        .await
    };

    let (code, _, err) = run(
        format!("chrome:{}", root.join("absent").display()),
        "history",
    )
    .await;
    assert_eq!(code, 1);
    assert!(err.contains("profile directory not found"), "{err}");

    let corrupt = root.join("corrupt");
    chrome(&corrupt, Encryption::V10);
    std::fs::write(
        corrupt.join("History"),
        b"garbage that is not sqlite, just plain text",
    )
    .unwrap();
    let (code, _, err) = run(format!("chrome:{}", corrupt.display()), "history").await;
    assert_eq!(code, 1);
    assert!(err.contains("corrupt or not a SQLite file"), "{err}");

    let future = root.join("future");
    chrome(&future, Encryption::Raw(b"v20 app-bound blob".to_vec()));
    let (code, _, err) = run(format!("chrome:{}", future.display()), "cookies").await;
    assert_eq!(code, 1);
    assert!(err.contains("unknown encryption version `v20`"), "{err}");

    let firefox = root.join("firefox");
    FirefoxProfile::default().write(&firefox);
    std::fs::remove_file(firefox.join("cookies.sqlite")).unwrap();
    let (code, _, err) = run(format!("firefox:{}", firefox.display()), "cookies").await;
    assert_eq!(code, 1);
    assert!(err.contains("Firefox cookies not found"), "{err}");

    let (code, _, err) = run("safari:/x".into(), "history").await;
    assert_eq!(code, 1);
    assert!(err.contains("expected <chrome|chromium|firefox>"), "{err}");

    let (code, _, err) = cli(&["import", "--from", "chrome:/x", "--into", "../x"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("invalid profile name"), "{err}");

    let (code, _, err) = cli(&[
        "import",
        "--from",
        "chrome:/x",
        "--into",
        "t",
        "--what",
        "passwords",
    ])
    .await;
    assert_eq!(code, 2, "{err}");
    assert!(!data.exists());
}
