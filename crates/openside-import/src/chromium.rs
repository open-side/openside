//! Chrome and Chromium profile readers: `History` and `Cookies` SQLite, `Bookmarks` JSON.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::crypto::Decryptor;
use crate::db::{sqlite_text, Item};
use crate::error::{ImportError, Result};
use crate::model::{is_web_url, Bookmark, Cookie, SameSite, Visit};
use crate::BrowserKind;

/// Microseconds between 1601-01-01 (WebKit epoch) and 1970-01-01.
const WEBKIT_EPOCH_OFFSET_US: i64 = 11_644_473_600_000_000;

fn webkit_to_unix_ms(webkit_us: i64) -> Option<i64> {
    (webkit_us > 0).then(|| (webkit_us - WEBKIT_EPOCH_OFFSET_US) / 1000)
}

pub(crate) fn history_item(kind: BrowserKind, dir: &Path) -> Item {
    Item {
        browser: kind,
        what: "history",
        path: dir.join("History"),
    }
}

pub(crate) fn bookmarks_item(kind: BrowserKind, dir: &Path) -> Item {
    Item {
        browser: kind,
        what: "bookmarks",
        path: dir.join("Bookmarks"),
    }
}

/// Chromium moved the cookie store into `Network/` in version 96.
pub(crate) fn cookies_item(kind: BrowserKind, dir: &Path) -> Item {
    let network = dir.join("Network").join("Cookies");
    // Any answer but "absent" (including a permission error) selects the Network location,
    // so the later read reports the real cause instead of a misleading missing file.
    let path = if !matches!(fs::symlink_metadata(&network), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        network
    } else {
        dir.join("Cookies")
    };
    Item {
        browser: kind,
        what: "cookies",
        path,
    }
}

/// Stream every http(s) visit, oldest first; returns `(emitted, skipped_non_web)`.
pub(crate) fn read_history(
    item: &Item,
    sink: &mut dyn FnMut(Visit) -> Result<()>,
) -> Result<(u64, u64)> {
    let db = item.open()?;
    db.require(item, "urls", &["id", "url", "title"])?;
    db.require(item, "visits", &["url", "visit_time"])?;
    let mut statement = db
        .conn
        .prepare(
            "SELECT u.url, u.title, v.visit_time FROM visits v \
             JOIN urls u ON u.id = v.url WHERE u.url IS NOT NULL ORDER BY v.visit_time",
        )
        .map_err(|error| item.sqlite(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| item.sqlite(&error))?;
    let (mut emitted, mut skipped) = (0, 0);
    for row in rows {
        let (url, title, time) = row.map_err(|error| item.sqlite(&error))?;
        match webkit_to_unix_ms(time) {
            Some(visited_at_ms) if is_web_url(&url) => {
                sink(Visit {
                    url,
                    title: sqlite_text(title),
                    visited_at_ms,
                })?;
                emitted += 1;
            }
            _ => skipped += 1,
        }
    }
    Ok((emitted, skipped))
}

/// All bookmarks except `javascript:` bookmarklets, in file order.
pub(crate) fn read_bookmarks(item: &Item) -> Result<(Vec<Bookmark>, u64)> {
    let text = item.read()?;
    let root: Value = serde_json::from_slice(&text).map_err(|error| ImportError::Corrupt {
        what: item.what,
        path: item.path.clone(),
        format: "JSON",
        detail: error.to_string(),
    })?;
    let roots = root
        .get("roots")
        .and_then(Value::as_object)
        .ok_or_else(|| item.unsupported("no `roots` object"))?;
    let (mut bookmarks, mut skipped) = (Vec::new(), 0);
    for node in roots.values().filter(|node| node.is_object()) {
        walk(node, &mut Vec::new(), &mut bookmarks, &mut skipped);
    }
    Ok((bookmarks, skipped))
}

fn walk(node: &Value, folder: &mut Vec<String>, out: &mut Vec<Bookmark>, skipped: &mut u64) {
    let name = node.get("name").and_then(Value::as_str).unwrap_or_default();
    match node.get("type").and_then(Value::as_str) {
        Some("url") => {
            let Some(url) = node.get("url").and_then(Value::as_str) else {
                *skipped += 1;
                return;
            };
            if url.to_ascii_lowercase().starts_with("javascript:") {
                *skipped += 1;
                return;
            }
            out.push(Bookmark {
                url: url.to_owned(),
                title: name.to_owned(),
                folder: folder.clone(),
                added_at_ms: node
                    .get("date_added")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(webkit_to_unix_ms),
            });
        }
        Some("folder") => {
            folder.push(name.to_owned());
            for child in node
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                walk(child, folder, out, skipped);
            }
            folder.pop();
        }
        _ => *skipped += 1,
    }
}

/// Decrypt every cookie; the first undecryptable cookie aborts with a specific error.
/// Returns the cookies and the number of partitioned (CHIPS) cookies left out.
pub(crate) fn read_cookies(item: &Item, decryptor: &mut Decryptor) -> Result<(Vec<Cookie>, u64)> {
    let db = item.open()?;
    let columns = db.require(
        item,
        "cookies",
        &[
            "host_key",
            "name",
            "value",
            "encrypted_value",
            "path",
            "expires_utc",
            "is_secure",
            "is_httponly",
        ],
    )?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    let select = format!(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, \
         is_httponly, {}, {} FROM cookies",
        if has("samesite") { "samesite" } else { "NULL" },
        if has("top_frame_site_key") {
            "top_frame_site_key"
        } else {
            "NULL"
        },
    );
    let schema_version = db
        .conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM meta WHERE key = 'version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let mut statement = db
        .conn
        .prepare(&select)
        .map_err(|error| item.sqlite(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)? != 0,
                row.get::<_, i64>(7)? != 0,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .map_err(|error| item.sqlite(&error))?;
    let (mut cookies, mut partitioned) = (Vec::new(), 0);
    for row in rows {
        let (host, name, plain, encrypted, path, expires_utc, secure, http_only, same_site, top) =
            row.map_err(|error| item.sqlite(&error))?;
        if top.is_some_and(|site| !site.is_empty()) {
            partitioned += 1;
            continue;
        }
        let value = if encrypted.is_empty() {
            plain
        } else {
            decryptor.decrypt(&host, &name, &encrypted, schema_version)?
        };
        cookies.push(Cookie {
            host,
            name,
            value,
            path,
            expires: webkit_to_unix_ms(expires_utc).map(|ms| ms / 1000),
            secure,
            http_only,
            same_site: match same_site {
                Some(0) => Some(SameSite::None),
                Some(1) => Some(SameSite::Lax),
                Some(2) => Some(SameSite::Strict),
                _ => None,
            },
        });
    }
    Ok((cookies, partitioned))
}
