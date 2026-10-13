//! Local browser migration across Chromium brands, Firefox forks and permitted Safari files.
//!
//! [`read_profile`] isolates failures by [`DataKind`]; [`select_recent_cookies`] resolves
//! conflicting sessions by source recency before any native write. Secret records remain
//! host-only: callers save passwords into encrypted Vault and apply cookies through Chromium.
//! [`write_profile`] persists only source-scoped non-secret snapshots. SQLite's online
//! backup API normally reads a `mode=ro`, `immutable=0` source, including committed WAL
//! pages; closed WAL databases have a verified-quiescent main-file copy fallback.
//! Advisory read locks may delay checkpoints; the stock VFS's path-reopen residual and
//! filesystem metadata effects are documented in `docs/PLATFORMS.md`.
//!
//! [`import`] retains the standalone named-profile history/bookmark/cookie CLI workflow.
//! Safe Storage can use ordinary OS keyring access or a caller-supplied key. See
//! `docs/specs/import.md` for the daemon's nine-kind onboarding and native user delivery.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod apply;
mod arc;
mod chromium;
mod chromium_extra;
mod chromium_passwords;
mod crypto;
mod db;
mod discover;
mod error;
mod everything;
mod firefox;
mod firefox_crypto;
mod firefox_extra;
mod model;
mod password_csv;
mod safari;
mod safe_storage;
mod store;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;
mod url_safety;

use std::path::PathBuf;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub use apply::cdp_cookie;
pub use crypto::KeyringSource;
pub use db::profile_storage_origins;
pub use discover::{discover, discover_system, DiscoveredProfile};
pub use error::{ImportError, Result};
pub use everything::{password_count, safe_note, select_recent_cookies};
pub use everything::{read_profile, write_profile, Available, DataKind, ProfileData, ProfileRead};
pub use model::{
    AutofillAddress, Bookmark, Cookie, ExtensionMetadata, RestoreTab, SameSite, SavedPassword,
    SearchEngine, SitePermission, Visit,
};
pub use password_csv::{parse_password_csv, CsvPasswords};
pub use store::{ProfileLayout, DEFAULT_PROFILE};
pub use url_safety::{sanitize_import_document_urls, sanitize_import_url};

/// Browser family of a source profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserKind {
    /// Google Chrome.
    Chrome,
    /// Chromium and its derivatives.
    Chromium,
    /// Mozilla Firefox.
    Firefox,
    /// Apple Safari.
    Safari,
}

impl BrowserKind {
    /// Lowercase name used on the command line and in file names.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }

    pub(crate) fn family(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Chromium => "Chromium",
            Self::Firefox => "Firefox",
            Self::Safari => "Safari",
        }
    }

    fn is_chromium_family(self) -> bool {
        matches!(self, Self::Chrome | Self::Chromium)
    }
}

/// A source profile: browser family plus its profile directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// Browser family.
    pub kind: BrowserKind,
    /// Profile directory (Chrome `Default`, Firefox `xxxx.default-release`).
    pub dir: PathBuf,
}

impl FromStr for Source {
    type Err = ImportError;

    /// Parse `<chrome|chromium|firefox>:<directory>`.
    fn from_str(value: &str) -> Result<Self> {
        let invalid = || ImportError::InvalidSource(value.to_owned());
        let (kind, dir) = value.split_once(':').ok_or_else(invalid)?;
        let kind = match kind {
            "chrome" => BrowserKind::Chrome,
            "chromium" => BrowserKind::Chromium,
            "firefox" => BrowserKind::Firefox,
            _ => return Err(invalid()),
        };
        if dir.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            kind,
            dir: PathBuf::from(dir),
        })
    }
}

/// Data categories to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct What {
    /// Page visits.
    pub history: bool,
    /// Bookmarks.
    pub bookmarks: bool,
    /// Cookies.
    pub cookies: bool,
}

impl What {
    /// Every category.
    pub const ALL: Self = Self {
        history: true,
        bookmarks: true,
        cookies: true,
    };
}

/// Parameters of one import.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Profile to read.
    pub source: Source,
    /// openside profile to write.
    pub target: ProfileLayout,
    /// Categories to import.
    pub what: What,
    /// When true, a category whose source file is absent is reported and skipped instead of
    /// failing the import (used when the caller did not name categories explicitly).
    pub skip_missing: bool,
    /// Read and decrypt everything, write nothing, launch nothing.
    pub dry_run: bool,
    /// Where `v11` (OS keyring) cookie passwords come from.
    pub keyring: KeyringSource,
    /// Chromium-compatible executable that writes cookies into the target profile.
    pub browser: Option<PathBuf>,
}

/// A cookie Chromium refused to store (no value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RejectedCookie {
    /// Cookie host or URL.
    pub host: String,
    /// Cookie name.
    pub name: String,
}

/// Outcome for one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CategoryReport {
    /// Entries read from the source (cookies: decrypted).
    pub read: u64,
    /// Entries written to the target (dry run: that would be written).
    pub imported: u64,
    /// Entries left out: non-web URLs, bookmarklets, expired or partitioned cookies, cookies
    /// Chromium refused.
    pub skipped: u64,
    /// Cookies Chromium refused, by host and name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<RejectedCookie>,
    /// Why the category was skipped entirely (missing source file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// True when nothing was written.
    pub dry_run: bool,
    /// Source family label.
    pub source: String,
    /// Target profile name.
    pub profile: String,
    /// History outcome when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<CategoryReport>,
    /// Bookmarks outcome when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarks: Option<CategoryReport>,
    /// Cookies outcome when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookies: Option<CategoryReport>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

struct Items {
    history: db::Item,
    bookmarks: db::Item,
    cookies: db::Item,
}

fn items(source: &Source) -> Items {
    let (kind, dir) = (source.kind, source.dir.as_path());
    if kind.is_chromium_family() {
        Items {
            history: chromium::history_item(kind, dir),
            bookmarks: chromium::bookmarks_item(kind, dir),
            cookies: chromium::cookies_item(kind, dir),
        }
    } else {
        Items {
            history: firefox::places_item(dir, "history"),
            bookmarks: firefox::places_item(dir, "bookmarks"),
            cookies: firefox::cookies_item(dir),
        }
    }
}

/// Run an import. Reads and decrypts everything before writing anything: a missing file,
/// corrupt database or undecryptable cookie leaves the target profile untouched.
pub async fn import(options: ImportOptions) -> Result<Report> {
    if !options.source.dir.is_dir() {
        return Err(ImportError::MissingFile {
            browser: options.source.kind.family(),
            what: "profile directory",
            path: options.source.dir.clone(),
        });
    }
    if options.what.cookies && !options.dry_run && options.browser.is_none() {
        return Err(ImportError::BrowserRequired(
            "no Chromium-compatible browser was found; set browser.executable or OPENSIDE_BROWSER"
                .into(),
        ));
    }
    if options.what.cookies && !options.dry_run {
        // Checked up front, even when the source turns out to have no cookies: the import
        // must not half-run against a profile another browser owns.
        apply::ensure_profile_free(&options.target.name, &options.target.browser_dir)?;
    }
    let Read {
        mut report,
        cookies,
        bookmarks,
        history,
    } = tokio::task::spawn_blocking({
        let options = options.clone();
        move || read_all(&options)
    })
    .await
    .map_err(|error| ImportError::Browser(format!("import worker failed: {error}")))??;
    if options.dry_run {
        return Ok(report);
    }
    let now = now_secs();
    if let (Some(cookie_report), Some(browser)) = (report.cookies.as_mut(), &options.browser) {
        let params: Vec<_> = cookies
            .iter()
            .filter_map(|cookie| apply::cdp_cookie(cookie, now))
            .collect();
        if !params.is_empty() {
            let applied = apply::apply(
                browser,
                &options.target.name,
                &options.target.browser_dir,
                params,
            )
            .await?;
            cookie_report.imported = applied.accepted;
            cookie_report.skipped += u64::try_from(applied.rejected.len()).unwrap_or(u64::MAX);
            cookie_report.rejected = applied
                .rejected
                .into_iter()
                .map(|(host, name)| RejectedCookie { host, name })
                .collect();
        }
    }
    if let Some(stage) = history {
        stage.commit(
            &options.target.history_path(options.source.kind),
            &options.target.import_dir,
        )?;
    }
    if let Some(bookmarks) = bookmarks {
        store::write_bookmarks(
            &options.target.bookmarks_path(options.source.kind),
            &options.target.import_dir,
            options.source.kind,
            now.saturating_mul(1000),
            &bookmarks,
        )?;
    }
    Ok(report)
}

struct Read {
    report: Report,
    cookies: Vec<Cookie>,
    bookmarks: Option<Vec<Bookmark>>,
    history: Option<store::HistoryStage>,
}

enum Located {
    Found(db::Item),
    Missing(ImportError),
}

/// An absent file fails the import unless the caller let categories default.
fn locate(item: db::Item, skip_missing: bool) -> Result<Located> {
    if item.probe()? {
        Ok(Located::Found(item))
    } else if skip_missing {
        Ok(Located::Missing(item.missing()))
    } else {
        Err(item.missing())
    }
}

fn skipped_category(error: &ImportError) -> CategoryReport {
    CategoryReport {
        note: Some(error.to_string()),
        ..CategoryReport::default()
    }
}

fn count(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

fn read_all(options: &ImportOptions) -> Result<Read> {
    let items = items(&options.source);
    let chromium_family = options.source.kind.is_chromium_family();
    let now = now_secs();
    let mut report = Report {
        dry_run: options.dry_run,
        source: options.source.kind.label().to_owned(),
        profile: options.target.name.clone(),
        ..Report::default()
    };
    let mut cookies = Vec::new();
    let mut bookmarks = None;
    let mut history = None;
    let mut first_missing = None;
    let mut found_any = false;
    if options.what.cookies {
        match locate(items.cookies, options.skip_missing)? {
            Located::Found(item) => {
                found_any = true;
                let mut decryptor = crypto::Decryptor::new(options.keyring.clone());
                let (read, partitioned) = if chromium_family {
                    chromium::read_cookies(&item, &mut decryptor)?
                } else {
                    firefox::read_cookies(&item)?
                };
                let expired = read
                    .iter()
                    .filter(|cookie| cookie.expires.is_some_and(|at| at <= now))
                    .count();
                report.cookies = Some(CategoryReport {
                    read: count(read.len()),
                    imported: if options.dry_run {
                        count(read.len() - expired)
                    } else {
                        0
                    },
                    skipped: partitioned + count(expired),
                    ..CategoryReport::default()
                });
                cookies = read;
            }
            Located::Missing(error) => {
                report.cookies = Some(skipped_category(&error));
                first_missing.get_or_insert(error);
            }
        }
    }
    if options.what.bookmarks {
        match locate(items.bookmarks, options.skip_missing)? {
            Located::Found(item) => {
                found_any = true;
                let (read, skipped) = if chromium_family {
                    chromium::read_bookmarks(&item)?
                } else {
                    firefox::read_bookmarks(&item)?
                };
                report.bookmarks = Some(CategoryReport {
                    read: count(read.len()),
                    imported: count(read.len()),
                    skipped,
                    ..CategoryReport::default()
                });
                bookmarks = Some(read);
            }
            Located::Missing(error) => {
                report.bookmarks = Some(skipped_category(&error));
                first_missing.get_or_insert(error);
            }
        }
    }
    if options.what.history {
        match locate(items.history, options.skip_missing)? {
            Located::Found(item) => {
                found_any = true;
                let mut writer = if options.dry_run {
                    None
                } else {
                    Some(store::HistoryStage::new()?)
                };
                let mut sink = |visit: Visit| match writer.as_mut() {
                    Some(writer) => writer.push(&visit),
                    None => Ok(()),
                };
                let (read, skipped) = if chromium_family {
                    chromium::read_history(&item, &mut sink)?
                } else {
                    firefox::read_history(&item, &mut sink)?
                };
                report.history = Some(CategoryReport {
                    read,
                    imported: read,
                    skipped,
                    ..CategoryReport::default()
                });
                history = writer;
            }
            Located::Missing(error) => {
                report.history = Some(skipped_category(&error));
                first_missing.get_or_insert(error);
            }
        }
    }
    match first_missing {
        Some(error) if !found_any => Err(error),
        _ => Ok(Read {
            report,
            cookies,
            bookmarks,
            history,
        }),
    }
}
