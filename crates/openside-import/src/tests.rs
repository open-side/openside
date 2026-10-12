use std::fs;
use std::path::{Path, PathBuf};

use crate::crypto::{Decryptor, KeyringSource};
use crate::testing::{
    read_jsonl, v10_keyring, BookmarkEntry, ChromeProfile, CookieSpec, Encryption, FirefoxProfile,
    HistoryEntry,
};
use crate::*;

const T0: i64 = 1_700_000_000_000;

fn history() -> Vec<HistoryEntry> {
    vec![
        HistoryEntry {
            url: "http://127.0.0.1:1/a".into(),
            title: Some("Page A".into()),
            visits_unix_ms: vec![T0, T0 + 5_000],
        },
        HistoryEntry {
            url: "https://example.test/b".into(),
            title: None,
            visits_unix_ms: vec![T0 + 1_000],
        },
        HistoryEntry {
            url: "chrome://settings/".into(),
            title: Some("Settings".into()),
            visits_unix_ms: vec![T0 + 2_000],
        },
    ]
}

fn bookmarks() -> Vec<BookmarkEntry> {
    vec![
        BookmarkEntry {
            folder: vec![],
            title: "Top".into(),
            url: "https://top.test/".into(),
        },
        BookmarkEntry {
            folder: vec!["Dev".into(), "Rust".into()],
            title: "Docs".into(),
            url: "https://docs.rs/".into(),
        },
        BookmarkEntry {
            folder: vec![],
            title: "Bookmarklet".into(),
            url: "JavaScript:alert(1)".into(),
        },
    ]
}

fn cookies() -> Vec<CookieSpec> {
    let mut session = CookieSpec::new("example.test", "session_only", "s3ss");
    session.expires_unix = None;
    session.secure = true;
    session.http_only = true;
    session.samesite = 1;
    vec![
        CookieSpec::new("127.0.0.1", "sid", "abc123"),
        CookieSpec::new(".example.test", "pref", "dark mode;x=1"),
        session,
    ]
}

fn chrome(encryption: Encryption) -> ChromeProfile {
    ChromeProfile {
        history: history(),
        bookmarks: bookmarks(),
        cookies: cookies(),
        encryption,
        ..ChromeProfile::default()
    }
}

fn isolate_home(dir: &Path) {
    std::env::set_var("HOME", dir);
}

fn decrypted(profile: &Path, keyring: KeyringSource) -> Result<Vec<Cookie>> {
    let item = crate::chromium::cookies_item(BrowserKind::Chrome, profile);
    let mut decryptor = Decryptor::new(keyring);
    crate::chromium::read_cookies(&item, &mut decryptor).map(|(cookies, _)| cookies)
}

fn options(source: Source, target: ProfileLayout) -> ImportOptions {
    ImportOptions {
        source,
        target,
        what: What {
            cookies: false,
            ..What::ALL
        },
        skip_missing: false,
        dry_run: false,
        keyring: v10_keyring(),
        browser: None,
    }
}

fn tree(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push((path.clone(), fs::read(path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn chrome_v10_cookies_decrypt_in_both_schema_generations_and_locations() {
    let home = tempfile::tempdir().unwrap();
    isolate_home(home.path());
    for (schema, network_dir) in [(24, true), (20, false)] {
        let dir = home.path().join(format!("chrome-{schema}"));
        ChromeProfile {
            cookie_schema: schema,
            network_dir,
            ..chrome(Encryption::V10)
        }
        .write(&dir);
        let read = decrypted(&dir, v10_keyring()).unwrap();
        assert_eq!(read.len(), 3, "schema {schema}");
        assert_eq!(read[0].host, "127.0.0.1");
        assert_eq!(read[0].value, "abc123");
        assert_eq!(read[1].value, "dark mode;x=1");
        assert!(read[2].expires.is_none() && read[2].secure && read[2].http_only);
        assert_eq!(read[2].same_site, Some(SameSite::Lax));
        assert_eq!(read[0].expires, Some(4_102_444_800));
    }
}

#[test]
fn chrome_plaintext_value_column_is_used_when_nothing_is_encrypted() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("p");
    chrome(Encryption::Plain).write(&dir);
    let read = decrypted(&dir, KeyringSource::None).unwrap();
    assert_eq!(read[0].value, "abc123");
}

#[test]
fn v11_cookies_need_the_keyring_password_and_reject_a_wrong_one() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("p");
    chrome(Encryption::V11("hunter2-keyring".into())).write(&dir);

    let missing = decrypted(&dir, KeyringSource::None).unwrap_err();
    assert!(matches!(
        missing,
        ImportError::KeyringPasswordRequired { .. }
    ));
    assert!(missing.to_string().contains("--keyring-password-cmd"));

    let wrong = decrypted(&dir, KeyringSource::password("not-the-password")).unwrap_err();
    assert!(
        matches!(wrong, ImportError::WrongKeyringPassword { .. }),
        "{wrong}"
    );
    assert!(wrong.to_string().contains("keyring password is wrong"));
    assert!(!wrong.to_string().contains("not-the-password"));

    let right = decrypted(&dir, KeyringSource::password("hunter2-keyring")).unwrap();
    assert_eq!(right[0].value, "abc123");

    // The password may come from an explicit command; the command is run only for v11.
    let via_command =
        decrypted(&dir, KeyringSource::Command("echo hunter2-keyring".into())).unwrap();
    assert_eq!(via_command[1].value, "dark mode;x=1");
    let failing = decrypted(&dir, KeyringSource::Command("false".into())).unwrap_err();
    assert!(matches!(failing, ImportError::KeyringCommand(_)));
}

#[test]
fn unknown_cookie_versions_and_corrupt_values_are_reported_by_cookie() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("p");
    chrome(Encryption::Raw(
        b"v20\x01\x02\x03garbage-blob-bytes".to_vec(),
    ))
    .write(&dir);
    let error = decrypted(&dir, KeyringSource::None).unwrap_err();
    assert!(
        matches!(&error, ImportError::UnknownCookieVersion { version, .. } if version == "v20"),
        "{error}"
    );
    assert!(error.to_string().contains("127.0.0.1"));
    assert!(error.to_string().contains("sid"));

    let dir = home.path().join("q");
    chrome(Encryption::Raw(
        b"v10 not a multiple of block size".to_vec(),
    ))
    .write(&dir);
    let error = decrypted(&dir, v10_keyring()).unwrap_err();
    assert!(
        matches!(error, ImportError::CorruptCookie { .. }),
        "{error}"
    );
}

#[test]
fn partitioned_chrome_cookies_are_left_out_and_counted() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("p");
    chrome(Encryption::V10).write(&dir);
    let conn = rusqlite::Connection::open(dir.join("Network").join("Cookies")).unwrap();
    conn.execute(
        "UPDATE cookies SET top_frame_site_key = 'https://top.test' WHERE name = 'pref'",
        [],
    )
    .unwrap();
    drop(conn);
    let item = crate::chromium::cookies_item(BrowserKind::Chrome, &dir);
    let (read, partitioned) =
        crate::chromium::read_cookies(&item, &mut Decryptor::new(v10_keyring())).unwrap();
    assert_eq!((read.len(), partitioned), (2, 1));
}

#[tokio::test]
async fn chrome_history_and_bookmarks_land_in_the_profile_store() {
    let home = tempfile::tempdir().unwrap();
    isolate_home(home.path());
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "work").unwrap();
    let report = import(options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target.clone(),
    ))
    .await
    .unwrap();
    let history = report.history.unwrap();
    assert_eq!((history.imported, history.skipped), (3, 1));
    let lines = read_jsonl(&target.history_path(BrowserKind::Chrome));
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["url"], "http://127.0.0.1:1/a");
    assert_eq!(lines[0]["title"], "Page A");
    assert_eq!(lines[0]["visited_at_ms"], T0);
    assert!(lines[1]["title"].is_null());
    assert_eq!(lines[2]["visited_at_ms"], T0 + 5_000);
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(target.bookmarks_path(BrowserKind::Chrome)).unwrap())
            .unwrap();
    let bookmarks = report.bookmarks.unwrap();
    assert_eq!(
        (bookmarks.imported, bookmarks.skipped),
        (2, 1),
        "bookmarklet is skipped"
    );
    let saved = document["bookmarks"].as_array().unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0]["folder"], serde_json::json!(["Bookmarks bar"]));
    assert_eq!(
        saved[1]["folder"],
        serde_json::json!(["Bookmarks bar", "Dev", "Rust"])
    );
    assert_eq!(saved[1]["url"], "https://docs.rs/");
}

#[tokio::test]
async fn firefox_history_bookmarks_and_cookies_are_read() {
    let home = tempfile::tempdir().unwrap();
    isolate_home(home.path());
    let source = home.path().join("ff");
    let mut milliseconds = CookieSpec::new("ms.test", "ms", "1");
    milliseconds.expires_unix = Some(4_102_444_800_000);
    FirefoxProfile {
        history: history(),
        bookmarks: bookmarks(),
        cookies: [cookies(), vec![milliseconds]].concat(),
    }
    .write(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "ff-profile").unwrap();
    let mut options = options(
        Source {
            kind: BrowserKind::Firefox,
            dir: source.clone(),
        },
        target.clone(),
    );
    let report = import(options.clone()).await.unwrap();
    assert_eq!(report.history.unwrap().imported, 3);
    let bookmarks = report.bookmarks.unwrap();
    // The `place:` smart bookmark and the bookmarklet are not pages; the tag association
    // below `tags________` is not a second bookmark.
    assert_eq!((bookmarks.imported, bookmarks.skipped), (2, 2));
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(target.bookmarks_path(BrowserKind::Firefox)).unwrap())
            .unwrap();
    assert_eq!(
        document["bookmarks"][1]["folder"],
        serde_json::json!(["Bookmarks Toolbar", "Dev", "Rust"])
    );

    let item = crate::firefox::cookies_item(&source);
    let (read, isolated) = crate::firefox::read_cookies(&item).unwrap();
    assert_eq!(isolated, 1, "container cookie is left out");
    assert_eq!(read.len(), 4);
    assert_eq!(read[0].value, "abc123");
    assert_eq!(read[2].expires, None);
    assert_eq!(
        read[3].expires,
        Some(4_102_444_800),
        "millisecond expiry is normalized"
    );

    options.dry_run = true;
    options.what.cookies = true;
    let dry = import(options).await.unwrap();
    assert_eq!(dry.cookies.unwrap().read, 4);
}

#[tokio::test]
async fn dry_run_reads_everything_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "dry").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target,
    );
    opts.dry_run = true;
    opts.what = What::ALL;
    let report = import(opts).await.unwrap();
    assert!(report.dry_run);
    assert_eq!(report.history.unwrap().imported, 3);
    assert_eq!(report.bookmarks.unwrap().imported, 2);
    let cookies = report.cookies.unwrap();
    assert_eq!((cookies.read, cookies.imported), (3, 3));
    assert!(
        !data.exists(),
        "a dry run must not create the data directory"
    );
}

#[tokio::test]
async fn import_never_modifies_the_source_and_reimport_does_not_duplicate() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    let before = tree(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "idem").unwrap();
    let opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source.clone(),
        },
        target.clone(),
    );
    import(opts.clone()).await.unwrap();
    import(opts).await.unwrap();
    assert_eq!(tree(&source), before, "source profile files are untouched");
    assert_eq!(
        read_jsonl(&target.history_path(BrowserKind::Chrome)).len(),
        3
    );
    let entries: Vec<_> = fs::read_dir(&target.import_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries.len(), 2, "no temp files left behind: {entries:?}");
}

#[tokio::test]
async fn importing_into_one_profile_leaves_another_untouched() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    let data = home.path().join("data");
    let a = ProfileLayout::new(&data, "a").unwrap();
    let b = ProfileLayout::new(&data, "b").unwrap();
    import(options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        a.clone(),
    ))
    .await
    .unwrap();
    assert!(a.history_path(BrowserKind::Chrome).is_file());
    assert!(!b.import_dir.exists() && !b.browser_dir.exists());
    assert_ne!(a.import_dir, b.import_dir);
    assert_ne!(a.browser_dir, b.browser_dir);
}

#[tokio::test]
async fn missing_corrupt_and_unsupported_sources_fail_with_specific_messages() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "t").unwrap();
    let run = |dir: PathBuf, kind: BrowserKind| {
        let mut opts = options(Source { kind, dir }, target.clone());
        opts.what = What {
            history: true,
            bookmarks: false,
            cookies: false,
        };
        async move { import(opts).await.unwrap_err() }
    };

    let error = run(home.path().join("nope"), BrowserKind::Chrome).await;
    assert!(
        error.to_string().contains("profile directory not found"),
        "{error}"
    );

    let empty = home.path().join("empty");
    fs::create_dir(&empty).unwrap();
    let error = run(empty.clone(), BrowserKind::Chrome).await;
    assert!(
        matches!(
            error,
            ImportError::MissingFile {
                what: "history",
                ..
            }
        ),
        "{error}"
    );
    let error = run(empty, BrowserKind::Firefox).await;
    assert!(
        error.to_string().contains("Firefox history not found"),
        "{error}"
    );

    let corrupt = home.path().join("corrupt");
    chrome(Encryption::V10).write(&corrupt);
    fs::write(
        corrupt.join("History"),
        b"this is not a sqlite database at all, just text",
    )
    .unwrap();
    let error = run(corrupt, BrowserKind::Chrome).await;
    assert!(
        matches!(
            error,
            ImportError::Corrupt {
                what: "history",
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("corrupt"));

    let bad_json = home.path().join("badjson");
    chrome(Encryption::V10).write(&bad_json);
    fs::write(bad_json.join("Bookmarks"), b"{ not json").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: bad_json,
        },
        target.clone(),
    );
    opts.what.history = false;
    let error = import(opts).await.unwrap_err();
    assert!(error.to_string().contains("not a JSON file"), "{error}");

    let unsupported = home.path().join("unsupported");
    chrome(Encryption::V10).write(&unsupported);
    rusqlite::Connection::open(unsupported.join("History"))
        .unwrap()
        .execute_batch("DROP TABLE visits")
        .unwrap();
    let error = run(unsupported, BrowserKind::Chrome).await;
    assert!(
        matches!(
            error,
            ImportError::UnsupportedVersion {
                what: "history",
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("table `visits` is missing"));

    let old_layout = home.path().join("old");
    FirefoxProfile::default().write(&old_layout);
    rusqlite::Connection::open(old_layout.join("places.sqlite"))
        .unwrap()
        .execute_batch("ALTER TABLE moz_historyvisits RENAME COLUMN visit_date TO date")
        .unwrap();
    let error = run(old_layout, BrowserKind::Firefox).await;
    assert!(
        error.to_string().contains("no column `visit_date`"),
        "{error}"
    );
}

#[tokio::test]
async fn failed_cookie_decryption_leaves_the_target_profile_untouched() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V11("pw".into())).write(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target,
    );
    opts.what = What::ALL;
    // v10 fixtures decrypt through v10_keyring(); only an empty source can exercise the
    // missing-password path for v11.
    opts.keyring = KeyringSource::None;
    opts.dry_run = false;
    opts.browser = Some(PathBuf::from("/nonexistent/browser"));
    let error = import(opts).await.unwrap_err();
    assert!(
        matches!(error, ImportError::KeyringPasswordRequired { .. }),
        "{error}"
    );
    assert!(!data.exists());
}

#[tokio::test]
async fn cookie_import_without_a_browser_is_a_specific_error() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target,
    );
    opts.what = What::ALL;
    let error = import(opts).await.unwrap_err();
    assert!(matches!(error, ImportError::BrowserRequired(_)), "{error}");
}

#[tokio::test]
async fn omitted_categories_skip_absent_files_but_explicit_ones_fail() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    fs::remove_file(source.join("Bookmarks")).unwrap();
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target,
    );
    opts.skip_missing = true;
    let report = import(opts.clone()).await.unwrap();
    assert!(report
        .bookmarks
        .unwrap()
        .note
        .unwrap()
        .contains("bookmarks not found"));
    assert_eq!(report.history.unwrap().imported, 3);
    opts.skip_missing = false;
    assert!(import(opts)
        .await
        .unwrap_err()
        .to_string()
        .contains("bookmarks not found"));
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_source_files_report_the_operating_system_error() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    fs::set_permissions(source.join("History"), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(source.join("History")).is_ok() {
        eprintln!("skip: running with privileges that bypass file permissions");
        return;
    }
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let error = import(options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target,
    ))
    .await
    .unwrap_err();
    assert!(
        matches!(
            error,
            ImportError::Unreadable {
                what: "history",
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("History"));
}

#[test]
fn locked_databases_are_reported_as_locked() {
    let item = crate::chromium::history_item(BrowserKind::Chrome, Path::new("/profile"));
    let busy = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(5), None);
    let error = item.sqlite(&busy);
    assert!(matches!(error, ImportError::Locked { .. }));
    assert!(error.to_string().contains("locked; close Chrome"));
}

#[test]
fn source_and_profile_names_are_validated() {
    assert_eq!(
        "firefox:/some/dir".parse::<Source>().unwrap(),
        Source {
            kind: BrowserKind::Firefox,
            dir: PathBuf::from("/some/dir")
        }
    );
    for bad in ["", "chrome", "chrome:", "safari:/x", "/x"] {
        assert!(matches!(
            bad.parse::<Source>().unwrap_err(),
            ImportError::InvalidSource(_)
        ));
    }
    let data = Path::new("/d");
    assert_eq!(
        ProfileLayout::new(data, "default").unwrap().browser_dir,
        data.join("browser-profile")
    );
    assert_eq!(
        ProfileLayout::new(data, "work-2").unwrap().browser_dir,
        data.join("browser-profiles").join("work-2")
    );
    for bad in ["", "..", "a/b", "A", "-x", "x y", &"a".repeat(65)] {
        assert!(
            matches!(
                ProfileLayout::new(data, bad).unwrap_err(),
                ImportError::InvalidProfileName(_)
            ),
            "{bad}"
        );
    }
}

#[test]
fn cookie_debug_never_prints_the_value() {
    let cookie = CookieSpec::new("a.test", "n", "super-secret-value");
    let cookie = Cookie {
        host: cookie.host,
        name: cookie.name,
        value: cookie.value,
        path: "/".into(),
        expires: None,
        secure: false,
        http_only: false,
        same_site: None,
    };
    assert!(!format!("{cookie:?}").contains("super-secret-value"));
}

fn set_encrypted(profile: &Path, name: &str, blob: &[u8]) {
    let conn = rusqlite::Connection::open(profile.join("Network").join("Cookies")).unwrap();
    conn.execute(
        "UPDATE cookies SET encrypted_value = ?1 WHERE name = ?2",
        rusqlite::params![blob, name],
    )
    .unwrap();
}

#[test]
fn damaged_v11_cookies_are_corrupt_not_a_wrong_password() {
    let home = tempfile::tempdir().unwrap();
    let password = "keyring-pw";

    // Not whole AES blocks: damaged regardless of the password supplied.
    let dir = home.path().join("short");
    chrome(Encryption::V11(password.into())).write(&dir);
    set_encrypted(&dir, "sid", b"v11abc");
    for supplied in [password, "some-other-password"] {
        let error = decrypted(&dir, KeyringSource::password(supplied)).unwrap_err();
        assert!(
            matches!(error, ImportError::CorruptCookie { .. }),
            "{error}"
        );
    }

    // Right password already proven on an earlier cookie, then one whole-block garbage blob.
    let dir = home.path().join("later");
    chrome(Encryption::V11(password.into())).write(&dir);
    let mut garbage = b"v11".to_vec();
    garbage.extend_from_slice(&[0x5a; 32]);
    set_encrypted(&dir, "pref", &garbage);
    let error = decrypted(&dir, KeyringSource::password(password)).unwrap_err();
    assert!(
        matches!(error, ImportError::CorruptCookie { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("pref"));

    // The same garbage as the very first cookie is indistinguishable from a wrong password.
    set_encrypted(&dir, "sid", &garbage);
    let error = decrypted(&dir, KeyringSource::password(password)).unwrap_err();
    assert!(
        matches!(error, ImportError::WrongKeyringPassword { .. }),
        "{error}"
    );
}

#[test]
fn cookie_schema_version_stored_as_an_integer_is_still_read() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("p");
    chrome(Encryption::V10).write(&dir);
    // No type affinity: SQLite keeps 24 as INTEGER instead of converting it to text.
    rusqlite::Connection::open(dir.join("Network").join("Cookies"))
        .unwrap()
        .execute_batch(
            "DROP TABLE meta; CREATE TABLE meta(key, value); INSERT INTO meta VALUES('version', 24);",
        )
        .unwrap();
    let read = decrypted(&dir, v10_keyring()).unwrap();
    assert_eq!(
        read[0].value, "abc123",
        "the domain hash prefix must be stripped"
    );
}

#[tokio::test]
async fn firefox_rows_with_null_urls_or_dates_do_not_abort_the_import() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("ff");
    FirefoxProfile {
        history: history(),
        ..FirefoxProfile::default()
    }
    .write(&source);
    let conn = rusqlite::Connection::open(source.join("places.sqlite")).unwrap();
    conn.execute_batch(
        "INSERT INTO moz_places(id,url,title) VALUES(900,NULL,'no url');
         INSERT INTO moz_historyvisits(place_id,visit_date,visit_type) VALUES(900,1700000000000000,1);
         INSERT INTO moz_historyvisits(place_id,visit_date,visit_type) VALUES(1,NULL,1);",
    )
    .unwrap();
    drop(conn);
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Firefox,
            dir: source,
        },
        target,
    );
    opts.what = What {
        history: true,
        bookmarks: false,
        cookies: false,
    };
    assert_eq!(import(opts).await.unwrap().history.unwrap().imported, 3);
}

#[tokio::test]
async fn visits_still_in_an_uncheckpointed_wal_are_imported() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    // Like a running browser: WAL mode, the connection stays open, nothing is checkpointed.
    let live = rusqlite::Connection::open(source.join("History")).unwrap();
    live.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
        .unwrap();
    live.execute_batch("PRAGMA wal_autocheckpoint=0;").unwrap();
    live.execute_batch(
        "INSERT INTO urls(url,title,last_visit_time) VALUES('https://wal.test/',NULL,13400000000000000);
         INSERT INTO visits(url,visit_time) VALUES(last_insert_rowid(),13400000000000000);",
    )
    .unwrap();
    assert!(source.join("History-wal").metadata().unwrap().len() > 0);
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target.clone(),
    );
    opts.what = What {
        history: true,
        bookmarks: false,
        cookies: false,
    };
    assert_eq!(import(opts).await.unwrap().history.unwrap().imported, 4);
    let urls: Vec<_> = read_jsonl(&target.history_path(BrowserKind::Chrome))
        .iter()
        .map(|visit| visit["url"].as_str().unwrap().to_owned())
        .collect();
    assert!(urls.contains(&"https://wal.test/".to_owned()), "{urls:?}");
    drop(live);
}

#[cfg(unix)]
#[tokio::test]
async fn source_files_are_copied_private_and_symlinks_are_not_followed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("ff");
    FirefoxProfile {
        cookies: cookies(),
        ..FirefoxProfile::default()
    }
    .write(&source);
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Firefox,
            dir: source.clone(),
        },
        target,
    );
    opts.dry_run = true;
    opts.what = What {
        history: false,
        bookmarks: false,
        cookies: true,
    };
    // A world-readable, read-only source (a backup copy) still imports; the scratch copy is 0600.
    fs::set_permissions(
        source.join("cookies.sqlite"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    assert_eq!(import(opts.clone()).await.unwrap().cookies.unwrap().read, 3);

    let item = crate::firefox::cookies_item(&source);
    let copy = item.open().unwrap();
    let scratch = copy.conn.path().unwrap().to_owned();
    assert_eq!(
        fs::metadata(&scratch).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(copy);

    // A symlinked database in an untrusted profile is refused, never followed.
    let outside = home.path().join("outside.sqlite");
    fs::rename(source.join("cookies.sqlite"), &outside).unwrap();
    symlink(&outside, source.join("cookies.sqlite")).unwrap();
    let error = import(opts).await.unwrap_err();
    assert!(
        matches!(&error, ImportError::Unreadable { reason, .. } if reason.contains("symbolic link")),
        "{error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_profile_directory_is_reported_as_unreadable_not_missing() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    fs::set_permissions(source.join("Network"), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(source.join("Network")).is_ok() {
        eprintln!("skip: running with privileges that bypass file permissions");
        return;
    }
    let target = ProfileLayout::new(&home.path().join("data"), "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source.clone(),
        },
        target,
    );
    opts.dry_run = true;
    opts.what = What {
        history: false,
        bookmarks: false,
        cookies: true,
    };
    let error = import(opts).await.unwrap_err();
    fs::set_permissions(source.join("Network"), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(
            error,
            ImportError::Unreadable {
                what: "cookies",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn nothing_is_created_when_a_later_category_fails() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    fs::write(
        source.join("History"),
        b"corrupt corrupt corrupt corrupt corrupt",
    )
    .unwrap();
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "t").unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target,
    );
    // Bookmarks are read fine before the corrupt history is hit.
    opts.what = What {
        history: true,
        bookmarks: true,
        cookies: false,
    };
    assert!(matches!(
        import(opts).await.unwrap_err(),
        ImportError::Corrupt { .. }
    ));
    assert!(
        !data.exists(),
        "a failed import must not create the profile store"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn profile_store_is_private_and_never_follows_planted_temp_files() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    chrome(Encryption::V10).write(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "t").unwrap();
    fs::create_dir_all(&target.import_dir).unwrap();
    let victim = home.path().join("victim");
    fs::write(&victim, "keep").unwrap();
    for planted in ["history.chrome.jsonl.tmp", "bookmarks.chrome.json.tmp"] {
        symlink(&victim, target.import_dir.join(planted)).unwrap();
    }
    import(options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target.clone(),
    ))
    .await
    .unwrap();
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&target.import_dir), 0o700);
    assert_eq!(mode(target.import_dir.parent().unwrap()), 0o700);
    assert_eq!(mode(&target.history_path(BrowserKind::Chrome)), 0o600);
    assert_eq!(mode(&target.bookmarks_path(BrowserKind::Chrome)), 0o600);
}

#[cfg(unix)]
#[tokio::test]
async fn a_live_singleton_lock_refuses_cookie_import_even_without_cookies() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("chrome");
    ChromeProfile {
        history: history(),
        ..ChromeProfile::default()
    }
    .write(&source);
    let data = home.path().join("data");
    let target = ProfileLayout::new(&data, "busy").unwrap();
    fs::create_dir_all(&target.browser_dir).unwrap();
    // A Chromium without a debugging port: only its lock symlink shows it is running.
    let lock = target.browser_dir.join("SingletonLock");
    symlink(format!("somehost-{}", std::process::id()), &lock).unwrap();
    let mut opts = options(
        Source {
            kind: BrowserKind::Chrome,
            dir: source,
        },
        target.clone(),
    );
    opts.what = What::ALL;
    opts.browser = Some(PathBuf::from("/nonexistent/browser"));
    let error = import(opts.clone()).await.unwrap_err();
    assert!(
        matches!(error, ImportError::ProfileInUse(ref name) if name == "busy"),
        "{error}"
    );
    assert!(!target.import_dir.exists(), "nothing was written");

    // A lock left by a dead process is stale; with nothing to apply no browser is launched.
    fs::remove_file(&lock).unwrap();
    let mut finished = std::process::Command::new("true").spawn().unwrap();
    finished.wait().unwrap();
    symlink(format!("somehost-{}", finished.id()), &lock).unwrap();
    assert_eq!(import(opts).await.unwrap().cookies.unwrap().imported, 0);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn discovery_lists_standard_profiles_by_id_with_their_available_data() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".config");
    let chrome = config.join("google-chrome");
    ChromeProfile {
        history: history(),
        bookmarks: bookmarks(),
        cookies: cookies(),
        ..ChromeProfile::default()
    }
    .write(&chrome.join("Default"));
    // A second profile with only a history database, named in `Local State`.
    fs::create_dir_all(chrome.join("Profile 2")).unwrap();
    fs::copy(
        chrome.join("Default/History"),
        chrome.join("Profile 2/History"),
    )
    .unwrap();
    fs::write(
        chrome.join("Local State"),
        r#"{"profile":{"info_cache":{"Profile 2":{"name":"Work"}}}}"#,
    )
    .unwrap();
    fs::create_dir_all(chrome.join("Crashpad")).unwrap();

    let firefox = home.path().join(".mozilla/firefox");
    FirefoxProfile {
        history: history(),
        ..FirefoxProfile::default()
    }
    .write(&firefox.join("abcd.default-release"));
    fs::write(
        firefox.join("profiles.ini"),
        "[Install4F96]\nDefault=Profiles/ignored\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=abcd.default-release\n\n[Profile1]\nName=gone\nIsRelative=1\nPath=missing\n",
    )
    .unwrap();

    let home_dir = home.path().to_owned();
    let env = move |key: &str| match key {
        "HOME" => Some(home_dir.clone().into_os_string()),
        _ => None,
    };
    let found = discover(&env);
    let summary: Vec<_> = found
        .iter()
        .map(|profile| (profile.id.as_str(), profile.name.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("chrome:Default", "Default"),
            ("chrome:Profile 2", "Work"),
            ("firefox:abcd.default-release", "default-release"),
        ]
    );
    let flags = |profile: &DiscoveredProfile| {
        (
            profile.available.history,
            profile.available.bookmarks,
            profile.available.cookies,
        )
    };
    assert_eq!(flags(&found[0]), (true, true, true));
    assert_eq!(flags(&found[1]), (true, false, false));
    assert_eq!(flags(&found[2]), (true, true, true));
    assert_eq!(found[2].source().kind, BrowserKind::Firefox);
    // Without HOME nothing is guessed.
    assert!(discover(&|_| None).is_empty());
}
