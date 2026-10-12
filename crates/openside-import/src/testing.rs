//! Synthetic Chrome and Firefox profiles for tests: real table layouts, real `v10`/`v11`
//! encryption with the same key derivation the decryptor uses on this platform. Built from
//! scratch; no real profile is ever read.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::crypto::{derive_key, DOMAIN_HASH_VERSION, ITERATIONS};

const WEBKIT_EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;

/// One visited page; each entry in `visits_unix_ms` is one visit.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Address.
    pub url: String,
    /// Title.
    pub title: Option<String>,
    /// Visit times, Unix milliseconds.
    pub visits_unix_ms: Vec<i64>,
}

/// One bookmark placed in a folder path below the bookmarks bar / toolbar.
#[derive(Debug, Clone)]
pub struct BookmarkEntry {
    /// Folder names below the toolbar, outermost first; empty for the toolbar itself.
    pub folder: Vec<String>,
    /// Title.
    pub title: String,
    /// Address.
    pub url: String,
}

/// One cookie. `samesite`: -1 unspecified, 0 none, 1 lax, 2 strict.
#[derive(Debug, Clone)]
pub struct CookieSpec {
    /// Host key (leading dot for domain cookies).
    pub host: String,
    /// Name.
    pub name: String,
    /// Plaintext value.
    pub value: String,
    /// Path.
    pub path: String,
    /// Expiry in Unix seconds; `None` for a session cookie.
    pub expires_unix: Option<i64>,
    /// `Secure`.
    pub secure: bool,
    /// `HttpOnly`.
    pub http_only: bool,
    /// SameSite code.
    pub samesite: i64,
}

impl CookieSpec {
    /// A persistent host-only cookie that expires on 2100-01-01.
    pub fn new(host: &str, name: &str, value: &str) -> Self {
        Self {
            host: host.into(),
            name: name.into(),
            value: value.into(),
            path: "/".into(),
            expires_unix: Some(4_102_444_800),
            secure: false,
            http_only: false,
            samesite: -1,
        }
    }
}

/// How a Chrome fixture stores cookie values.
#[derive(Debug, Clone)]
pub enum Encryption {
    /// `v10` with the fixed Linux password (`peanuts`). On macOS the real `v10` key is
    /// Keychain-derived, so this variant encrypts with [`MAC_V10_PASSWORD`] instead and tests
    /// must supply it through [`v10_keyring`].
    V10,
    /// `v11` with this keyring password.
    V11(String),
    /// Unencrypted `value` column, empty `encrypted_value` (very old Chrome).
    Plain,
    /// The given `encrypted_value` bytes verbatim (unknown versions, corruption).
    Raw(Vec<u8>),
}

/// Stand-in Safe Storage password for `v10` fixtures on macOS, where the real key comes
/// from the login Keychain rather than the Linux `peanuts` constant.
#[cfg(target_os = "macos")]
pub const MAC_V10_PASSWORD: &str = "openside-test-safe-storage";

#[cfg(not(target_os = "macos"))]
const V10_PASSWORD: &[u8] = b"peanuts";

#[cfg(target_os = "macos")]
const V10_PASSWORD: &[u8] = MAC_V10_PASSWORD.as_bytes();

/// The keyring source that decrypts `Encryption::V10` fixtures on this platform: none on
/// Linux (fixed password), the Keychain stand-in on macOS.
pub fn v10_keyring() -> crate::KeyringSource {
    #[cfg(not(target_os = "macos"))]
    return crate::KeyringSource::None;
    #[cfg(target_os = "macos")]
    return crate::KeyringSource::password(MAC_V10_PASSWORD);
}

type CbcEnc = cbc::Encryptor<aes::Aes128>;

/// `v10`/`v11` blob for `value`, as Chromium writes it.
pub fn encrypt_cookie(
    prefix: &str,
    password: &[u8],
    host: &str,
    value: &str,
    schema: i64,
) -> Vec<u8> {
    let key = derive_key(password, ITERATIONS);
    let mut plain = Vec::new();
    if schema >= DOMAIN_HASH_VERSION {
        plain.extend_from_slice(Sha256::digest(host.as_bytes()).as_slice());
    }
    plain.extend_from_slice(value.as_bytes());
    let encrypted =
        CbcEnc::new(&key.into(), &[b' '; 16].into()).encrypt_padded_vec_mut::<Pkcs7>(&plain);
    let mut blob = prefix.as_bytes().to_vec();
    blob.extend_from_slice(&encrypted);
    blob
}

/// A Chrome/Chromium profile directory.
#[derive(Debug, Clone)]
pub struct ChromeProfile {
    /// History rows.
    pub history: Vec<HistoryEntry>,
    /// Bookmarks.
    pub bookmarks: Vec<BookmarkEntry>,
    /// Cookies.
    pub cookies: Vec<CookieSpec>,
    /// Cookie value storage.
    pub encryption: Encryption,
    /// `meta.version` of the cookie database.
    pub cookie_schema: i64,
    /// Put cookies in `Network/Cookies` (Chromium 96+) instead of `Cookies`.
    pub network_dir: bool,
}

impl Default for ChromeProfile {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            bookmarks: Vec::new(),
            cookies: Vec::new(),
            encryption: Encryption::V10,
            cookie_schema: 24,
            network_dir: true,
        }
    }
}

fn webkit_us(unix_ms: i64) -> i64 {
    (unix_ms + WEBKIT_EPOCH_OFFSET_MS) * 1000
}

impl ChromeProfile {
    /// Create the profile files in `dir` (created if needed).
    pub fn write(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.write_history(&dir.join("History"));
        self.write_bookmarks(&dir.join("Bookmarks"));
        let cookies = if self.network_dir {
            fs::create_dir_all(dir.join("Network")).unwrap();
            dir.join("Network").join("Cookies")
        } else {
            dir.join("Cookies")
        };
        self.write_cookies(&cookies);
    }

    fn write_history(&self, path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
             INSERT INTO meta VALUES('version','70');
             CREATE TABLE urls(id INTEGER PRIMARY KEY AUTOINCREMENT, url LONGVARCHAR, title LONGVARCHAR,
               visit_count INTEGER DEFAULT 0 NOT NULL, typed_count INTEGER DEFAULT 0 NOT NULL,
               last_visit_time INTEGER NOT NULL, hidden INTEGER DEFAULT 0 NOT NULL);
             CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER NOT NULL, visit_time INTEGER NOT NULL,
               from_visit INTEGER, transition INTEGER DEFAULT 0 NOT NULL, segment_id INTEGER,
               visit_duration INTEGER DEFAULT 0 NOT NULL,
               incremented_omnibox_typed_score BOOLEAN DEFAULT FALSE NOT NULL);",
        )
        .unwrap();
        for entry in &self.history {
            let last = entry.visits_unix_ms.iter().max().copied().unwrap_or(0);
            conn.execute(
                "INSERT INTO urls(url,title,visit_count,last_visit_time) VALUES(?1,?2,?3,?4)",
                params![
                    entry.url,
                    entry.title,
                    entry.visits_unix_ms.len(),
                    webkit_us(last)
                ],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for visit in &entry.visits_unix_ms {
                conn.execute(
                    "INSERT INTO visits(url,visit_time) VALUES(?1,?2)",
                    params![id, webkit_us(*visit)],
                )
                .unwrap();
            }
        }
    }

    fn write_bookmarks(&self, path: &Path) {
        let mut bar = json!({"type":"folder","name":"Bookmarks bar","children":[]});
        for bookmark in &self.bookmarks {
            let mut node = &mut bar;
            for name in &bookmark.folder {
                let children = node["children"].as_array_mut().unwrap();
                let position = children
                    .iter()
                    .position(|child| child["type"] == "folder" && child["name"] == name.as_str())
                    .unwrap_or_else(|| {
                        children.push(json!({"type":"folder","name":name,"children":[]}));
                        children.len() - 1
                    });
                node = &mut node["children"][position];
            }
            node["children"].as_array_mut().unwrap().push(json!({
                "type": "url", "name": bookmark.title, "url": bookmark.url,
                "date_added": webkit_us(1_700_000_000_000).to_string(),
            }));
        }
        let document = json!({
            "roots": {
                "bookmark_bar": bar,
                "other": {"type":"folder","name":"Other bookmarks","children":[]},
                "synced": {"type":"folder","name":"Mobile bookmarks","children":[]},
            },
            "version": 1,
        });
        fs::write(path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    }

    fn write_cookies(&self, path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
             CREATE TABLE cookies(creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
               top_frame_site_key TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
               encrypted_value BLOB NOT NULL, path TEXT NOT NULL, expires_utc INTEGER NOT NULL,
               is_secure INTEGER NOT NULL, is_httponly INTEGER NOT NULL, last_access_utc INTEGER NOT NULL,
               has_expires INTEGER NOT NULL DEFAULT 1, is_persistent INTEGER NOT NULL DEFAULT 1,
               priority INTEGER NOT NULL DEFAULT 1, samesite INTEGER NOT NULL DEFAULT -1,
               source_scheme INTEGER NOT NULL DEFAULT 0, source_port INTEGER NOT NULL DEFAULT -1,
               last_update_utc INTEGER NOT NULL DEFAULT 0, source_type INTEGER NOT NULL DEFAULT 0,
               has_cross_site_ancestor INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta VALUES('version', ?1)",
            [self.cookie_schema.to_string()],
        )
        .unwrap();
        for cookie in &self.cookies {
            let (plain, encrypted) = match &self.encryption {
                Encryption::V10 => (
                    String::new(),
                    encrypt_cookie(
                        "v10",
                        V10_PASSWORD,
                        &cookie.host,
                        &cookie.value,
                        self.cookie_schema,
                    ),
                ),
                Encryption::V11(password) => (
                    String::new(),
                    encrypt_cookie(
                        "v11",
                        password.as_bytes(),
                        &cookie.host,
                        &cookie.value,
                        self.cookie_schema,
                    ),
                ),
                Encryption::Plain => (cookie.value.clone(), Vec::new()),
                Encryption::Raw(bytes) => (String::new(), bytes.clone()),
            };
            conn.execute(
                "INSERT INTO cookies(creation_utc,host_key,top_frame_site_key,name,value,encrypted_value,
                   path,expires_utc,is_secure,is_httponly,last_access_utc,has_expires,is_persistent,samesite)
                 VALUES(?1,?2,'',?3,?4,?5,?6,?7,?8,?9,?1,?10,?10,?11)",
                params![
                    webkit_us(1_700_000_000_000),
                    cookie.host,
                    cookie.name,
                    plain,
                    encrypted,
                    cookie.path,
                    cookie.expires_unix.map_or(0, |at| webkit_us(at * 1000)),
                    cookie.secure,
                    cookie.http_only,
                    cookie.expires_unix.is_some(),
                    cookie.samesite,
                ],
            )
            .unwrap();
        }
    }
}

/// A Firefox profile directory.
#[derive(Debug, Clone, Default)]
pub struct FirefoxProfile {
    /// History rows.
    pub history: Vec<HistoryEntry>,
    /// Bookmarks (below the toolbar).
    pub bookmarks: Vec<BookmarkEntry>,
    /// Cookies; the expiry is written in seconds.
    pub cookies: Vec<CookieSpec>,
}

impl FirefoxProfile {
    /// Create `places.sqlite` and `cookies.sqlite` in `dir` (created if needed).
    pub fn write(&self, dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        self.write_places(&dir.join("places.sqlite"));
        self.write_cookies(&dir.join("cookies.sqlite"));
    }

    fn write_places(&self, path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR,
               rev_host LONGVARCHAR, visit_count INTEGER DEFAULT 0, hidden INTEGER DEFAULT 0 NOT NULL,
               typed INTEGER DEFAULT 0 NOT NULL, frecency INTEGER DEFAULT -1 NOT NULL,
               last_visit_date INTEGER, guid TEXT, foreign_count INTEGER DEFAULT 0 NOT NULL,
               url_hash INTEGER DEFAULT 0 NOT NULL);
             CREATE TABLE moz_historyvisits(id INTEGER PRIMARY KEY, from_visit INTEGER, place_id INTEGER,
               visit_date INTEGER, visit_type INTEGER, session INTEGER);
             CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER DEFAULT NULL,
               parent INTEGER, position INTEGER, title LONGVARCHAR, keyword_id INTEGER, folder_type TEXT,
               dateAdded INTEGER, lastModified INTEGER, guid TEXT,
               syncStatus INTEGER NOT NULL DEFAULT 0, syncChangeCounter INTEGER NOT NULL DEFAULT 1);
             INSERT INTO moz_bookmarks(id,type,parent,title,guid) VALUES
               (1,2,0,'','root________'),(2,2,1,'menu','menu________'),(3,2,1,'toolbar','toolbar_____'),
               (4,2,1,'tags','tags________'),(5,2,1,'unfiled','unfiled_____'),(6,2,1,'mobile','mobile______');",
        )
        .unwrap();
        for entry in &self.history {
            let last = entry.visits_unix_ms.iter().max().copied().unwrap_or(0);
            conn.execute(
                "INSERT INTO moz_places(url,title,visit_count,last_visit_date) VALUES(?1,?2,?3,?4)",
                params![
                    entry.url,
                    entry.title,
                    entry.visits_unix_ms.len(),
                    last * 1000
                ],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            for visit in &entry.visits_unix_ms {
                conn.execute(
                    "INSERT INTO moz_historyvisits(place_id,visit_date,visit_type) VALUES(?1,?2,1)",
                    params![id, visit * 1000],
                )
                .unwrap();
            }
        }
        for bookmark in &self.bookmarks {
            let mut parent = 3_i64;
            for name in &bookmark.folder {
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM moz_bookmarks WHERE type=2 AND parent=?1 AND title=?2",
                        params![parent, name],
                        |row| row.get(0),
                    )
                    .ok();
                parent = existing.unwrap_or_else(|| {
                    conn.execute(
                        "INSERT INTO moz_bookmarks(type,parent,title) VALUES(2,?1,?2)",
                        params![parent, name],
                    )
                    .unwrap();
                    conn.last_insert_rowid()
                });
            }
            conn.execute(
                "INSERT INTO moz_places(url,title) VALUES(?1,?2)",
                params![bookmark.url, bookmark.title],
            )
            .unwrap();
            let place = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO moz_bookmarks(type,fk,parent,title,dateAdded) VALUES(1,?1,?2,?3,?4)",
                params![place, parent, bookmark.title, 1_700_000_000_000_000_i64],
            )
            .unwrap();
        }
        // Firefox records a tag as a folder below `tags________` whose children point at the
        // tagged page; that is an association, not a second bookmark.
        let tagged: Option<i64> = conn
            .query_row(
                "SELECT fk FROM moz_bookmarks WHERE type=1 AND fk IS NOT NULL ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        if let Some(place) = tagged {
            conn.execute(
                "INSERT INTO moz_bookmarks(type,parent,title) VALUES(2,4,'rust')",
                [],
            )
            .unwrap();
            let tag = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO moz_bookmarks(type,fk,parent,title) VALUES(1,?1,?2,NULL)",
                params![place, tag],
            )
            .unwrap();
        }
        // A Firefox smart-bookmark query that must not be imported as a page.
        conn.execute(
            "INSERT INTO moz_places(url,title) VALUES('place:type=6&sort=14','Recent Tags')",
            [],
        )
        .unwrap();
        let place = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO moz_bookmarks(type,fk,parent,title) VALUES(1,?1,3,'Recent Tags')",
            [place],
        )
        .unwrap();
    }

    fn write_cookies(&self, path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_cookies(id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '',
               name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER,
               creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER, inBrowserElement INTEGER DEFAULT 0,
               sameSite INTEGER DEFAULT 0, rawSameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 0,
               CONSTRAINT moz_uniqueid UNIQUE (name, host, path, originAttributes));",
        )
        .unwrap();
        for cookie in &self.cookies {
            conn.execute(
                "INSERT INTO moz_cookies(name,value,host,path,expiry,isSecure,isHttpOnly,sameSite)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    cookie.name,
                    cookie.value,
                    cookie.host,
                    cookie.path,
                    cookie.expires_unix.unwrap_or(0),
                    cookie.secure,
                    cookie.http_only,
                    cookie.samesite.max(0),
                ],
            )
            .unwrap();
        }
        // A container-tab cookie: isolation the target profile does not have.
        conn.execute(
            "INSERT INTO moz_cookies(originAttributes,name,value,host,path,expiry,isSecure,isHttpOnly)
             VALUES('^userContextId=2','container','x','127.0.0.1','/',4102444800,0,0)",
            [],
        )
        .unwrap();
    }
}

/// Parse a JSON-lines file written by an import.
pub fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
