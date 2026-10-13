use std::io::Cursor;
use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::db::{sqlite_text, Item};
use crate::error::{ImportError, Result};
use crate::model::{
    is_web_url, AutofillAddress, Bookmark, Cookie, ExtensionMetadata, RestoreTab, SavedPassword,
    SearchEngine, SitePermission, Visit,
};
use crate::{sanitize_import_url, KeyringSource, Source};

const COCOA_EPOCH_SECONDS: f64 = 978_307_200.0;
const FULL_DISK_ACCESS: &str = "Safari needs Full Disk Access. Allow openside in System Settings > Privacy & Security > Full Disk Access, then try again.";
const PASSWORDS_UNSUPPORTED: &str = "Safari passwords are stored in Keychain and cannot be imported directly. Export passwords from Safari and use the password CSV import.";
const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_BOOKMARK_DEPTH: usize = 64;

fn item(source: &Source, what: &'static str, name: &str) -> Item {
    Item {
        browser: source.kind,
        what,
        path: source.dir.join(name),
    }
}

fn with_permission_note(error: ImportError) -> ImportError {
    match error {
        ImportError::Unreadable { what, path, reason } if permission_denied(&reason) => {
            ImportError::Unreadable {
                what,
                path,
                reason: FULL_DISK_ACCESS.into(),
            }
        }
        other => other,
    }
}

fn permission_denied(reason: &str) -> bool {
    reason.contains("Permission denied")
        || reason.contains("permission denied")
        || reason.contains("Operation not permitted")
        || reason.contains("operation not permitted")
        || reason == FULL_DISK_ACCESS
}

pub(crate) fn permission_warning(source: &Source) -> Option<&'static str> {
    let mut inputs = vec![
        item(source, "bookmarks", "Bookmarks.plist"),
        item(source, "history", "History.db"),
    ];
    match cookies_item(source) {
        Ok(input) => inputs.push(input),
        Err(ImportError::Unreadable { reason, .. }) if permission_denied(&reason) => {
            return Some(FULL_DISK_ACCESS)
        }
        _ => {}
    }
    for input in inputs {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(nix::libc::O_NOFOLLOW);
        }
        if options
            .open(input.path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
        {
            return Some(FULL_DISK_ACCESS);
        }
    }
    None
}

fn read_bytes(input: &Item) -> Result<Vec<u8>> {
    input
        .read_bounded(MAX_SOURCE_BYTES as u64)
        .map_err(with_permission_note)
}

pub(crate) fn read_bookmarks(source: &Source) -> Result<(Vec<Bookmark>, u64)> {
    let input = item(source, "bookmarks", "Bookmarks.plist");
    let bytes = read_bytes(&input)?;
    let value =
        plist::Value::from_reader(Cursor::new(bytes)).map_err(|_| ImportError::Corrupt {
            what: "bookmarks",
            path: input.path.clone(),
            format: "property list",
            detail: "Invalid Safari bookmarks property list.".into(),
        })?;
    let mut result = Vec::new();
    let mut skipped = 0;
    bookmark_nodes(
        &value,
        &mut Vec::new(),
        &mut result,
        &mut skipped,
        0,
        &input,
    )?;
    Ok((result, skipped))
}

fn bookmark_nodes(
    value: &plist::Value,
    folder: &mut Vec<String>,
    result: &mut Vec<Bookmark>,
    skipped: &mut u64,
    depth: usize,
    input: &Item,
) -> Result<()> {
    if depth > MAX_BOOKMARK_DEPTH {
        return Err(input.unsupported("Safari bookmark folders are nested too deeply."));
    }
    let Some(node) = value.as_dictionary() else {
        return Err(input.unsupported("Unsupported Safari bookmark node."));
    };
    let kind = node
        .get("WebBookmarkType")
        .and_then(plist::Value::as_string)
        .unwrap_or_default();
    if kind == "WebBookmarkTypeLeaf" {
        let raw = node
            .get("URLString")
            .and_then(plist::Value::as_string)
            .unwrap_or_default();
        if let Some(url) = sanitize_import_url(raw) {
            let title = node
                .get("URIDictionary")
                .and_then(plist::Value::as_dictionary)
                .and_then(|dictionary| dictionary.get("title"))
                .and_then(plist::Value::as_string)
                .or_else(|| node.get("Title").and_then(plist::Value::as_string))
                .unwrap_or_default();
            let title = if title == raw {
                url.clone()
            } else {
                title.to_owned()
            };
            result.push(Bookmark {
                url,
                title,
                folder: folder.clone(),
                added_at_ms: None,
            });
        } else {
            *skipped += 1;
        }
    } else if let Some(children) = node.get("Children").and_then(plist::Value::as_array) {
        let title = node
            .get("Title")
            .and_then(plist::Value::as_string)
            .unwrap_or_default();
        let include = !title.is_empty() && depth > 0;
        if include {
            folder.push(
                match title {
                    "BookmarksBar" => "Bookmarks Bar",
                    "BookmarksMenu" => "Bookmarks Menu",
                    other => other,
                }
                .to_owned(),
            );
        }
        for child in children {
            bookmark_nodes(child, folder, result, skipped, depth + 1, input)?;
        }
        if include {
            folder.pop();
        }
    }
    Ok(())
}

pub(crate) fn read_history(source: &Source) -> Result<Vec<Visit>> {
    let input = item(source, "history", "History.db");
    let db = input.open().map_err(with_permission_note)?;
    db.require(&input, "history_items", &["id", "url"])?;
    db.require(
        &input,
        "history_visits",
        &["history_item", "visit_time", "title"],
    )?;
    let mut statement = db.conn.prepare(
        "SELECT i.url,v.title,v.visit_time FROM history_visits v JOIN history_items i ON i.id=v.history_item WHERE i.url IS NOT NULL AND v.visit_time IS NOT NULL ORDER BY v.visit_time",
    ).map_err(|error| input.sqlite(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })
        .map_err(|error| input.sqlite(&error))?;
    let mut result = Vec::new();
    for row in rows {
        let (url, title, cocoa_seconds) = row.map_err(|error| input.sqlite(&error))?;
        if !is_web_url(&url) {
            continue;
        }
        let Some(clean) = sanitize_import_url(&url) else {
            continue;
        };
        let Some(visited_at_ms) = cocoa_milliseconds(cocoa_seconds) else {
            continue;
        };
        let title =
            sqlite_text(title).map(|title| if title == url { clean.clone() } else { title });
        result.push(Visit {
            url: clean,
            title,
            visited_at_ms,
        });
    }
    Ok(result)
}

fn cocoa_milliseconds(value: f64) -> Option<i64> {
    let ms = (value + COCOA_EPOCH_SECONDS) * 1000.0;
    if value.is_finite() && ms >= 0.0 && ms < i64::MAX as f64 {
        Some(ms.round() as i64)
    } else {
        None
    }
}

pub(crate) fn cookies_item(source: &Source) -> Result<Item> {
    let mut paths = vec![source.dir.join("Cookies.binarycookies")];
    if let Some(library) = source.dir.parent() {
        paths.push(library.join("Cookies/Cookies.binarycookies"));
        paths.push(
            library.join("Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"),
        );
    }
    for path in &paths {
        let candidate = Item {
            browser: source.kind,
            what: "cookies",
            path: path.clone(),
        };
        match candidate.probe().map_err(with_permission_note)? {
            true => return Ok(candidate),
            false => continue,
        }
    }
    Ok(Item {
        browser: source.kind,
        what: "cookies",
        path: paths
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("Cookies.binarycookies")),
    })
}

pub(crate) fn read_cookies(source: &Source) -> Result<Vec<Cookie>> {
    let input = cookies_item(source)?;
    let bytes = Zeroizing::new(read_bytes(&input)?);
    parse_binarycookies(&bytes).map_err(|detail| ImportError::Corrupt {
        what: "cookies",
        path: input.path,
        format: "Safari binary cookies",
        detail: detail.into(),
    })
}

pub(crate) fn cookie_count(source: &Source) -> Result<u64> {
    let input = cookies_item(source)?;
    let bytes = Zeroizing::new(read_bytes(&input)?);
    visit_binarycookies(&bytes, |_| Ok(())).map_err(|detail| ImportError::Corrupt {
        what: "cookies",
        path: input.path,
        format: "Safari binary cookies",
        detail: detail.into(),
    })
}

fn u32_at(bytes: &[u8], offset: usize, big_endian: bool) -> std::result::Result<u32, &'static str> {
    let raw: [u8; 4] = bytes
        .get(offset..offset.checked_add(4).ok_or("Invalid cookie offset.")?)
        .ok_or("Truncated Safari cookie data.")?
        .try_into()
        .map_err(|_| "Truncated Safari cookie data.")?;
    Ok(if big_endian {
        u32::from_be_bytes(raw)
    } else {
        u32::from_le_bytes(raw)
    })
}

fn f64_at(bytes: &[u8], offset: usize) -> std::result::Result<f64, &'static str> {
    Ok(f64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("Truncated Safari cookie date.")?
            .try_into()
            .map_err(|_| "Truncated Safari cookie date.")?,
    ))
}

fn cookie_string(bytes: &[u8], field_offset: usize) -> std::result::Result<&str, &'static str> {
    let offset = u32_at(bytes, field_offset, false)? as usize;
    if offset < 56 {
        return Err("Safari cookie string offset points into its header.");
    }
    let tail = bytes
        .get(offset..)
        .ok_or("Invalid Safari cookie string offset.")?;
    let end = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("Unterminated Safari cookie string.")?;
    std::str::from_utf8(&tail[..end]).map_err(|_| "Invalid Safari cookie string encoding.")
}

fn visit_binarycookies(
    bytes: &[u8],
    mut visit: impl FnMut(&[u8]) -> std::result::Result<(), &'static str>,
) -> std::result::Result<u64, &'static str> {
    if bytes.len() > MAX_SOURCE_BYTES || bytes.get(..4) != Some(b"cook".as_slice()) {
        return Err("Invalid Safari binary cookies header.");
    }
    let count = u32_at(bytes, 4, true)? as usize;
    let header_size = count
        .checked_mul(4)
        .and_then(|size| size.checked_add(8))
        .ok_or("Invalid Safari cookie page count.")?;
    if header_size > bytes.len() {
        return Err("Truncated Safari cookie page table.");
    }
    let mut start = header_size;
    let mut total = 0;
    for index in 0..count {
        let size = u32_at(bytes, 8 + index * 4, true)? as usize;
        let end = start
            .checked_add(size)
            .ok_or("Invalid Safari cookie page size.")?;
        let page = bytes
            .get(start..end)
            .ok_or("Truncated Safari cookie page.")?;
        start = end;
        if page.get(..4) != Some([0, 0, 1, 0].as_slice()) {
            return Err("Invalid Safari cookie page header.");
        }
        let record_count = u32_at(page, 4, false)? as usize;
        let table_end = record_count
            .checked_mul(4)
            .and_then(|size| size.checked_add(12))
            .ok_or("Invalid Safari cookie record count.")?;
        if table_end > page.len() {
            return Err("Truncated Safari cookie offset table.");
        }
        for index in 0..record_count {
            let offset = u32_at(page, 8 + index * 4, false)? as usize;
            if offset < table_end {
                return Err("Safari cookie offset points into its page header.");
            }
            let size = u32_at(page, offset, false)? as usize;
            if size < 56 {
                return Err("Truncated Safari cookie record.");
            }
            let end = offset
                .checked_add(size)
                .ok_or("Invalid Safari cookie record size.")?;
            visit(
                page.get(offset..end)
                    .ok_or("Safari cookie record exceeds its page.")?,
            )?;
            total += 1;
        }
    }
    // Safari appends checksum/trailer data after its pages; it is not another record.
    Ok(total)
}

fn parse_binarycookies(bytes: &[u8]) -> std::result::Result<Vec<Cookie>, &'static str> {
    let mut cookies = Vec::new();
    visit_binarycookies(bytes, |record| {
        let flags = u32_at(record, 8, false)?;
        let host = cookie_string(record, 16)?;
        let name = cookie_string(record, 20)?;
        let path = cookie_string(record, 24)?;
        let value = cookie_string(record, 28)?;
        let date = f64_at(record, 40)?;
        if !date.is_finite() {
            return Err("Invalid Safari cookie expiry date.");
        }
        let expires = if date <= 0.0 {
            None
        } else {
            Some(cocoa_milliseconds(date).ok_or("Invalid Safari cookie expiry date.")? / 1000)
        };
        cookies.push(Cookie {
            host: host.to_owned(),
            name: name.to_owned(),
            value: value.to_owned(),
            path: path.to_owned(),
            expires,
            secure: flags & 1 != 0,
            http_only: flags & 4 != 0,
            same_site: None,
            last_accessed_at_ms: None,
            created_at_ms: cocoa_milliseconds(f64_at(record, 48)?),
        });
        Ok(())
    })?;
    Ok(cookies)
}

pub(crate) fn password_count(source: &Source) -> Result<u64> {
    Err(item(source, "passwords", "Keychain").unsupported(PASSWORDS_UNSUPPORTED))
}

pub(crate) fn read_passwords(
    source: &Source,
    _keyring: &KeyringSource,
) -> Result<Vec<SavedPassword>> {
    Err(item(source, "passwords", "Keychain").unsupported(PASSWORDS_UNSUPPORTED))
}

macro_rules! unsupported_reader {
    ($name:ident, $record:ty, $what:literal, $reason:literal) => {
        pub(crate) fn $name(source: &Source) -> Result<Vec<$record>> {
            Err(item(source, $what, $what).unsupported($reason))
        }
    };
}

unsupported_reader!(
    read_autofill,
    AutofillAddress,
    "autofill",
    "Safari autofill addresses are not available in an importable profile file."
);
pub(crate) fn read_tabs(source: &Source) -> Result<Vec<RestoreTab>> {
    let input = item(source, "tabs", "LastSession.plist");
    let bytes = Zeroizing::new(read_bytes(&input)?);
    let value =
        plist::Value::from_reader(Cursor::new(&bytes[..])).map_err(|_| ImportError::Corrupt {
            what: "tabs",
            path: input.path.clone(),
            format: "property list",
            detail: "Invalid Safari session property list.".into(),
        })?;
    let windows = value
        .as_dictionary()
        .and_then(|root| root.get("SessionWindows"))
        .and_then(plist::Value::as_array)
        .ok_or_else(|| input.unsupported("Unsupported Safari session-window layout."))?;
    let mut result = Vec::new();
    for window in windows {
        let Some(window) = window.as_dictionary() else {
            continue;
        };
        if window
            .get("IsPrivateWindow")
            .and_then(plist::Value::as_boolean)
            == Some(true)
        {
            continue;
        }
        let Some(tabs) = window.get("TabStates").and_then(plist::Value::as_array) else {
            continue;
        };
        for tab in tabs {
            let Some(tab) = tab.as_dictionary() else {
                continue;
            };
            let raw = tab
                .get("TabURL")
                .and_then(plist::Value::as_string)
                .unwrap_or_default();
            if !is_web_url(raw) {
                continue;
            }
            let Some(url) = sanitize_import_url(raw) else {
                continue;
            };
            let title = tab
                .get("TabTitle")
                .and_then(plist::Value::as_string)
                .unwrap_or_default();
            let title = if title == raw {
                url.clone()
            } else {
                title.to_owned()
            };
            result.push(RestoreTab { url, title });
        }
    }
    Ok(result)
}
unsupported_reader!(
    read_search_engines,
    SearchEngine,
    "search_engines",
    "Safari search-engine settings are not available in a supported profile format."
);
unsupported_reader!(
    read_permissions,
    SitePermission,
    "permissions",
    "Safari site permissions are not available in a supported profile format."
);
unsupported_reader!(
    read_extensions,
    ExtensionMetadata,
    "extensions",
    "Safari extension metadata is not available in a supported profile format."
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserKind;

    fn source(dir: &std::path::Path) -> Source {
        Source {
            kind: BrowserKind::Safari,
            dir: dir.to_owned(),
        }
    }
    fn binarycookies() -> Vec<u8> {
        let mut record = vec![0u8; 56];
        record[8..12].copy_from_slice(&5u32.to_le_bytes());
        record[40..48].copy_from_slice(&1000.5f64.to_le_bytes());
        record[48..56].copy_from_slice(&500f64.to_le_bytes());
        for (offset, value) in [
            (16, ".fixture.test"),
            (20, "fixture-cookie"),
            (24, "/"),
            (28, "fixture-value"),
        ] {
            let position = record.len() as u32;
            record[offset..offset + 4].copy_from_slice(&position.to_le_bytes());
            record.extend(value.as_bytes());
            record.push(0);
        }
        let size = record.len() as u32;
        record[..4].copy_from_slice(&size.to_le_bytes());
        let mut page = vec![0, 0, 1, 0];
        page.extend(1u32.to_le_bytes());
        page.extend(16u32.to_le_bytes());
        page.extend([0u8; 4]);
        page.extend(record);
        let mut file = b"cook".to_vec();
        file.extend(1u32.to_be_bytes());
        file.extend((page.len() as u32).to_be_bytes());
        file.extend(page);
        file.extend([0u8; 8]);
        file
    }

    #[test]
    fn synthetic_binarycookies_preserve_attributes_and_epoch() {
        let cookies = parse_binarycookies(&binarycookies()).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].host, ".fixture.test");
        assert_eq!(cookies[0].value, "fixture-value");
        assert!(cookies[0].secure && cookies[0].http_only);
        assert_eq!(cookies[0].expires, Some(978_308_200));
        assert_eq!(cookies[0].last_accessed_at_ms, None);
        assert_eq!(cookies[0].created_at_ms, Some(978_307_700_000));
        assert!(!format!("{:?}", cookies[0]).contains("fixture-value"));
    }

    #[test]
    fn cookie_preview_counts_record_headers_without_decoding_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = binarycookies();
        let record_start = 12 + 16;
        bytes[record_start + 28..record_start + 32].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(dir.path().join("Cookies.binarycookies"), &bytes).unwrap();
        assert_eq!(cookie_count(&source(dir.path())).unwrap(), 1);
        assert!(read_cookies(&source(dir.path())).is_err());
    }

    #[test]
    fn malformed_binarycookies_never_read_outside_records() {
        let sample = binarycookies();
        for end in 0..sample.len() - 8 {
            assert!(parse_binarycookies(&sample[..end]).is_err());
        }
        let mut invalid = sample.clone();
        invalid[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse_binarycookies(&invalid).is_err());
        let mut invalid = sample;
        invalid[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_binarycookies(&invalid).is_err());
    }

    #[test]
    fn binary_and_xml_bookmarks_preserve_folder_hierarchy() {
        for binary in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let leaf = plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("WebBookmarkType", plist::Value::from("WebBookmarkTypeLeaf")),
                ("URLString", plist::Value::from("https://fixture.test/")),
                (
                    "URIDictionary",
                    plist::Value::Dictionary(plist::Dictionary::from_iter([(
                        "title",
                        plist::Value::from("Fixture page"),
                    )])),
                ),
            ]));
            let local = plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("WebBookmarkType", plist::Value::from("WebBookmarkTypeLeaf")),
                (
                    "URLString",
                    plist::Value::from("file:///synthetic/local-page.html"),
                ),
            ]));
            let other = |url: &str| {
                plist::Value::Dictionary(plist::Dictionary::from_iter([
                    ("WebBookmarkType", plist::Value::from("WebBookmarkTypeLeaf")),
                    ("URLString", plist::Value::from(url)),
                ]))
            };
            let folder = plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("WebBookmarkType", plist::Value::from("WebBookmarkTypeList")),
                ("Title", plist::Value::from("BookmarksBar")),
                (
                    "Children",
                    plist::Value::Array(vec![
                        leaf,
                        local,
                        other("ftp://fixture.test/page"),
                        other("about:blank"),
                        other("chrome://settings/"),
                        other("javascript:void(0)"),
                        other("data:text/plain,fixture"),
                    ]),
                ),
            ]));
            let root = plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("WebBookmarkType", plist::Value::from("WebBookmarkTypeList")),
                ("Children", plist::Value::Array(vec![folder])),
            ]));
            let path = dir.path().join("Bookmarks.plist");
            if binary {
                root.to_file_binary(path).unwrap();
            } else {
                root.to_file_xml(path).unwrap();
            }
            let (bookmarks, skipped) = read_bookmarks(&source(dir.path())).unwrap();
            assert_eq!(bookmarks.len(), 3);
            assert_eq!(skipped, 4);
            assert_eq!(bookmarks[1].url, "file:///synthetic/local-page.html");
            assert_eq!(bookmarks[2].url, "ftp://fixture.test/page");
            assert_eq!(bookmarks[0].folder, ["Bookmarks Bar"]);
            assert_eq!(bookmarks[0].title, "Fixture page");
        }
    }

    #[test]
    fn history_database_converts_cocoa_seconds_and_filters_internal_urls() {
        let dir = tempfile::tempdir().unwrap();
        let db = rusqlite::Connection::open(dir.path().join("History.db")).unwrap();
        db.execute_batch("CREATE TABLE history_items(id INTEGER,url TEXT); CREATE TABLE history_visits(history_item INTEGER,visit_time REAL,title TEXT); INSERT INTO history_items VALUES(1,'https://fixture.test/'),(2,'safari://internal'); INSERT INTO history_visits VALUES(1,1000.5,'Fixture'),(2,1001,'Internal');").unwrap();
        let visits = read_history(&source(dir.path())).unwrap();
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].visited_at_ms, 978_308_200_500);
    }

    #[test]
    fn full_disk_access_result_is_clear_and_not_a_missing_file() {
        let error = with_permission_note(ImportError::Unreadable {
            what: "history",
            path: PathBuf::from("synthetic/History.db"),
            reason: std::io::Error::from(std::io::ErrorKind::PermissionDenied).to_string(),
        });
        assert!(
            matches!(error, ImportError::Unreadable { reason, .. } if reason == FULL_DISK_ACCESS)
        );
        let missing = with_permission_note(ImportError::MissingFile {
            browser: "Safari",
            what: "history",
            path: PathBuf::from("synthetic/History.db"),
        });
        assert!(matches!(missing, ImportError::MissingFile { .. }));
    }

    #[test]
    fn saved_session_plists_import_only_public_web_tabs() {
        for binary in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let tab = |url: &str, title: &str| {
                plist::Value::Dictionary(plist::Dictionary::from_iter([
                    ("TabURL", plist::Value::from(url)),
                    ("TabTitle", plist::Value::from(title)),
                ]))
            };
            let public = plist::Value::Dictionary(plist::Dictionary::from_iter([(
                "TabStates",
                plist::Value::Array(vec![
                    tab("https://fixture.test/", "Fixture tab"),
                    tab("safari://internal", "Internal"),
                ]),
            )]));
            let private = plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("IsPrivateWindow", plist::Value::from(true)),
                (
                    "TabStates",
                    plist::Value::Array(vec![tab("https://private.test/", "Private")]),
                ),
            ]));
            let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
                ("SessionVersion", plist::Value::from("1.0")),
                ("SessionWindows", plist::Value::Array(vec![public, private])),
            ]));
            let path = dir.path().join("LastSession.plist");
            if binary {
                value.to_file_binary(path).unwrap();
            } else {
                value.to_file_xml(path).unwrap();
            }
            assert_eq!(
                read_tabs(&source(dir.path())).unwrap(),
                vec![RestoreTab {
                    url: "https://fixture.test/".into(),
                    title: "Fixture tab".into()
                }]
            );
        }
    }

    #[test]
    fn safari_keychain_passwords_are_explicitly_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path());
        assert!(
            matches!(password_count(&source), Err(ImportError::UnsupportedVersion { detail, .. }) if detail == PASSWORDS_UNSUPPORTED)
        );
        assert!(
            matches!(read_passwords(&source, &KeyringSource::None), Err(ImportError::UnsupportedVersion { detail, .. }) if detail == PASSWORDS_UNSUPPORTED)
        );
    }
}
