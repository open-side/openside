//! Firefox profile readers: `places.sqlite` (history, bookmarks) and `cookies.sqlite`.

use std::collections::HashMap;
use std::path::Path;

use crate::db::{sqlite_text, Item};
use crate::error::Result;
use crate::model::{is_web_url, Bookmark, Cookie, SameSite, Visit};
use crate::BrowserKind;

const MAX_DEPTH: usize = 64;
const TAGS_ROOT: &str = "tags________";
/// Firefox stored cookie expiry in seconds, newer versions in milliseconds.
const MILLISECOND_EXPIRY_THRESHOLD: i64 = 100_000_000_000;

pub(crate) fn places_item(dir: &Path, what: &'static str) -> Item {
    Item {
        browser: BrowserKind::Firefox,
        what,
        path: dir.join("places.sqlite"),
    }
}

pub(crate) fn cookies_item(dir: &Path) -> Item {
    Item {
        browser: BrowserKind::Firefox,
        what: "cookies",
        path: dir.join("cookies.sqlite"),
    }
}

pub(crate) fn read_history(
    item: &Item,
    sink: &mut dyn FnMut(Visit) -> Result<()>,
) -> Result<(u64, u64)> {
    let db = item.open()?;
    db.require(item, "moz_places", &["id", "url", "title"])?;
    db.require(item, "moz_historyvisits", &["place_id", "visit_date"])?;
    let mut statement = db
        .conn
        .prepare(
            "SELECT p.url, p.title, v.visit_date FROM moz_historyvisits v \
             JOIN moz_places p ON p.id = v.place_id \
             WHERE p.url IS NOT NULL AND v.visit_date IS NOT NULL ORDER BY v.visit_date",
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
        let (url, title, micros) = row.map_err(|error| item.sqlite(&error))?;
        if micros > 0 && is_web_url(&url) {
            sink(Visit {
                url,
                title: sqlite_text(title),
                visited_at_ms: micros / 1000,
            })?;
            emitted += 1;
        } else {
            skipped += 1;
        }
    }
    Ok((emitted, skipped))
}

struct Node {
    parent: i64,
    title: Option<String>,
    guid: Option<String>,
}

fn root_title(guid: &str) -> Option<&'static str> {
    match guid {
        "menu________" => Some("Bookmarks Menu"),
        "toolbar_____" => Some("Bookmarks Toolbar"),
        "unfiled_____" => Some("Other Bookmarks"),
        "mobile______" => Some("Mobile Bookmarks"),
        _ => None,
    }
}

/// Bookmarks with a web-or-file address; Firefox `place:` queries and separators are skipped.
pub(crate) fn read_bookmarks(item: &Item) -> Result<(Vec<Bookmark>, u64)> {
    let db = item.open()?;
    db.require(item, "moz_places", &["id", "url"])?;
    let columns = db.require(
        item,
        "moz_bookmarks",
        &["id", "type", "fk", "parent", "title"],
    )?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    let select = format!(
        "SELECT b.id, b.type, b.parent, b.title, {}, {}, p.url FROM moz_bookmarks b \
         LEFT JOIN moz_places p ON p.id = b.fk ORDER BY b.id",
        if has("dateAdded") {
            "b.dateAdded"
        } else {
            "NULL"
        },
        if has("guid") { "b.guid" } else { "NULL" },
    );
    let mut statement = db
        .conn
        .prepare(&select)
        .map_err(|error| item.sqlite(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|error| item.sqlite(&error))?;
    let mut nodes = HashMap::new();
    let mut links = Vec::new();
    let mut skipped = 0;
    for row in rows {
        let (id, kind, parent, title, added, guid, url) =
            row.map_err(|error| item.sqlite(&error))?;
        nodes.insert(
            id,
            Node {
                parent,
                title,
                guid,
            },
        );
        match (kind, url) {
            (1, Some(url))
                if !url.starts_with("place:")
                    && !url.to_ascii_lowercase().starts_with("javascript:") =>
            {
                links.push((id, url, added));
            }
            (1, _) => skipped += 1,
            _ => {}
        }
    }
    let bookmarks = links
        .into_iter()
        .filter_map(|(id, url, added)| {
            let mut folder = Vec::new();
            let mut current = nodes.get(&id).map_or(0, |node| node.parent);
            while let Some(node) = nodes.get(&current) {
                if folder.len() >= MAX_DEPTH {
                    break;
                }
                let guid = node.guid.as_deref().unwrap_or_default();
                // Entries below the tags root only associate a tag with a page that is
                // already bookmarked elsewhere; importing them would duplicate it.
                if guid == TAGS_ROOT {
                    return None;
                }
                if guid == "root________" {
                    break;
                }
                let name = root_title(guid)
                    .map(str::to_owned)
                    .or_else(|| node.title.clone())
                    .unwrap_or_default();
                folder.push(name);
                current = node.parent;
            }
            folder.reverse();
            Some(Bookmark {
                title: nodes
                    .get(&id)
                    .and_then(|node| node.title.clone())
                    .unwrap_or_default(),
                url,
                folder,
                added_at_ms: added
                    .filter(|micros| *micros > 0)
                    .map(|micros| micros / 1000),
            })
        })
        .collect();
    Ok((bookmarks, skipped))
}

/// Plaintext cookies. Container and partitioned cookies (`originAttributes` set) belong to
/// isolation contexts the target profile does not have, so they are counted and left out.
pub(crate) fn read_cookies(item: &Item) -> Result<(Vec<Cookie>, u64)> {
    let db = item.open()?;
    let columns = db.require(
        item,
        "moz_cookies",
        &[
            "name",
            "value",
            "host",
            "path",
            "expiry",
            "isSecure",
            "isHttpOnly",
        ],
    )?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    let select = format!(
        "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, {}, {} FROM moz_cookies",
        if has("sameSite") { "sameSite" } else { "NULL" },
        if has("originAttributes") {
            "originAttributes"
        } else {
            "NULL"
        },
    );
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
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)? != 0,
                row.get::<_, i64>(6)? != 0,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(|error| item.sqlite(&error))?;
    let (mut cookies, mut isolated) = (Vec::new(), 0);
    for row in rows {
        let (host, name, value, path, expiry, secure, http_only, same_site, origin) =
            row.map_err(|error| item.sqlite(&error))?;
        if origin.is_some_and(|attributes| !attributes.is_empty()) {
            isolated += 1;
            continue;
        }
        let expires = match expiry {
            0 => None,
            value if value >= MILLISECOND_EXPIRY_THRESHOLD => Some(value / 1000),
            value => Some(value),
        };
        cookies.push(Cookie {
            host,
            name,
            value,
            path,
            expires,
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
    Ok((cookies, isolated))
}
