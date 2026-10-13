use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sanitize_import_document_urls;
use crate::url_safety::SanitizedRecords;
use crate::{
    arc, chromium, chromium_extra, chromium_passwords, crypto, db, firefox, firefox_extra, safari,
    store, AutofillAddress, Bookmark, BrowserKind, CategoryReport, Cookie, DiscoveredProfile,
    ExtensionMetadata, ImportError, KeyringSource, ProfileLayout, RestoreTab, Result,
    SavedPassword, SearchEngine, SitePermission, Source, Visit,
};

/// Independently previewable/migratable categories; source failure is isolated by category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    /// Web visits, not daemon conversation memory.
    History,
    /// Web bookmarks with source folder paths.
    Bookmarks,
    /// Authenticated sessions applied through Chromium itself.
    Cookies,
    /// Sign-ins transported only into encrypted Vault.
    Passwords,
    /// Non-payment address/form fields for trusted autofill.
    Autofill,
    /// Current tabs offered for explicit restoration.
    Tabs,
    /// Provider metadata without changing search defaults.
    SearchEngines,
    /// Site preference inventory, not new openside policy grants.
    Permissions,
    /// Name/store-link inventory without installing binaries.
    Extensions,
}

impl DataKind {
    /// Default onboarding selection, in stable report order.
    pub const ALL: [Self; 9] = [
        Self::History,
        Self::Bookmarks,
        Self::Cookies,
        Self::Passwords,
        Self::Autofill,
        Self::Tabs,
        Self::SearchEngines,
        Self::Permissions,
        Self::Extensions,
    ];
    /// Stable key used in persisted snapshots and API reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::Bookmarks => "bookmarks",
            Self::Cookies => "cookies",
            Self::Passwords => "passwords",
            Self::Autofill => "autofill",
            Self::Tabs => "tabs",
            Self::SearchEngines => "search_engines",
            Self::Permissions => "permissions",
            Self::Extensions => "extensions",
        }
    }
}

/// Source-file presence only; encrypted or protected data may still be unreadable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Available {
    /// A known history source exists.
    pub history: bool,
    /// A known bookmark source exists.
    pub bookmarks: bool,
    /// A known cookie source exists.
    pub cookies: bool,
    /// A password record source exists, not necessarily decryptable.
    pub passwords: bool,
    /// A known non-payment autofill source exists.
    pub autofill: bool,
    /// An open-session snapshot exists.
    pub tabs: bool,
    /// A known provider metadata source exists.
    pub search_engines: bool,
    /// A known site preference source exists.
    pub permissions: bool,
    /// Extension inventory metadata exists.
    pub extensions: bool,
}

impl Available {
    pub(crate) fn probe(source: &Source) -> Self {
        let has = |name: &str| {
            db::Item {
                browser: source.kind,
                what: "profile data",
                path: source.dir.join(name),
            }
            .probe()
            .unwrap_or(false)
        };
        match source.kind {
            BrowserKind::Chrome | BrowserKind::Chromium => Self {
                history: has("History"),
                bookmarks: has("Bookmarks") || has("StorableSidebar.json"),
                cookies: has("Network/Cookies") || has("Cookies"),
                passwords: has("Login Data") || has("Login Data For Account"),
                autofill: has("Web Data"),
                tabs: has("StorableSidebar.json")
                    || std::fs::symlink_metadata(source.dir.join("Sessions"))
                        .is_ok_and(|metadata| metadata.file_type().is_dir())
                    || has("Current Session")
                    || has("Last Session"),
                search_engines: has("Web Data"),
                permissions: has("Preferences"),
                extensions: has("Preferences") || has("Secure Preferences"),
            },
            BrowserKind::Firefox => Self {
                history: has("places.sqlite"),
                bookmarks: has("places.sqlite"),
                cookies: has("cookies.sqlite"),
                passwords: has("logins.json"),
                autofill: has("autofill-profiles.json") || has("formhistory.sqlite"),
                tabs: has("sessionstore.jsonlz4")
                    || has("sessionstore-backups/recovery.jsonlz4")
                    || has("sessionstore-backups/recovery.baklz4")
                    || has("sessionstore-backups/previous.jsonlz4"),
                search_engines: has("search.json.mozlz4"),
                permissions: has("permissions.sqlite"),
                extensions: has("extensions.json"),
            },
            BrowserKind::Safari => Self {
                history: has("History.db"),
                bookmarks: has("Bookmarks.plist"),
                cookies: safari::cookies_item(source)
                    .is_ok_and(|item| item.probe().unwrap_or(false)),
                passwords: false,
                autofill: false,
                tabs: has("LastSession.plist"),
                search_engines: false,
                permissions: false,
                extensions: false,
            },
        }
    }
}

/// Host-side records; secret categories must never be serialized with this aggregate.
#[derive(Debug, Default)]
pub struct ProfileData {
    /// Visits filtered to supported web URLs.
    pub history: Vec<Visit>,
    /// Folder-aware web bookmarks.
    pub bookmarks: Vec<Bookmark>,
    /// Decrypted cookies with redacted Debug and zeroized values.
    pub cookies: Vec<Cookie>,
    /// Decrypted sign-ins, never written into the side store.
    pub passwords: Vec<SavedPassword>,
    /// Non-payment address and form metadata.
    pub autofill: Vec<AutofillAddress>,
    /// Current tab navigations, never automatically opened.
    pub tabs: Vec<RestoreTab>,
    /// Source search-provider definitions.
    pub search_engines: Vec<SearchEngine>,
    /// Source site preference inventory.
    pub permissions: Vec<SitePermission>,
    /// Extension labels and official store links.
    pub extensions: Vec<ExtensionMetadata>,
}

/// One source snapshot plus value-free per-category read outcomes.
#[derive(Debug, Default)]
pub struct ProfileRead {
    /// Host-only data, including secret categories pending trusted persistence.
    pub data: ProfileData,
    /// Counts/notes safe to convert into the public report.
    pub reports: BTreeMap<DataKind, CategoryReport>,
}

/// Map failures into safe GUI guidance, deliberately dropping filesystem paths and values.
pub fn safe_note(error: &ImportError) -> String {
    match error {
        ImportError::KeyringPasswordRequired { .. } | ImportError::WrongKeyringPassword { .. } | ImportError::KeyringCommand(_) | ImportError::KeyringLocked { .. } | ImportError::KeyringUnavailable { .. } | ImportError::WrongPasswordKey { .. } => "Unlock your desktop keyring or login Keychain and try again. Other data can still be imported.".to_owned(),
        ImportError::MissingFile { .. } => "Not found in this profile.".to_owned(),
        ImportError::SourceTooLarge { .. } => "This browser data file is too large to import safely. Other data can still be imported.".to_owned(),
        ImportError::Unreadable { reason, .. } if reason.starts_with("Safari needs Full Disk Access.") => reason.clone(),
        ImportError::Unreadable { .. } => "This browser data could not be read. Check permission to read the profile and try again.".to_owned(),
        ImportError::Locked { .. } => "This browser data is changing or locked. Try again after the browser finishes saving.".to_owned(),
        ImportError::SourceLocked { browser, .. } => format!("Close {browser} and retry importing this browser data."),
        ImportError::UnsupportedVersion { detail, .. } if detail.starts_with("Firefox needs a primary password") || detail.starts_with("Safari ") => detail.clone(),
        ImportError::UnsupportedVersion { .. } | ImportError::UnknownCookieVersion { .. } | ImportError::UnsupportedPasswordEncryption { .. } => "This browser protects this data with an unsupported format. Other data can still be imported; saved passwords can be imported from an exported CSV file.".to_owned(),
        ImportError::Corrupt { .. } | ImportError::CorruptCookie { .. } | ImportError::CorruptPassword { .. } => "This browser data is damaged or could not be decrypted. Other data can still be imported.".to_owned(),
        ImportError::ProfileInUse(_) => "The openside browser is in use. Finish its current browser task, then try again.".to_owned(),
        ImportError::BrowserRequired(_) => "The openside browser is unavailable. Other data can still be imported.".to_owned(),
        _ => "This data could not be imported. Other data can still be imported.".to_owned(),
    }
}

fn capture<T>(
    result: Result<(Vec<T>, u64)>,
    reports: &mut BTreeMap<DataKind, CategoryReport>,
    kind: DataKind,
) -> Vec<T> {
    match result {
        Ok((items, skipped)) => {
            let count = u64::try_from(items.len()).unwrap_or(u64::MAX);
            reports.insert(
                kind,
                CategoryReport {
                    read: count,
                    imported: count,
                    skipped,
                    ..CategoryReport::default()
                },
            );
            items
        }
        Err(error) => {
            reports.insert(
                kind,
                CategoryReport {
                    note: Some(safe_note(&error)),
                    ..CategoryReport::default()
                },
            );
            Vec::new()
        }
    }
}

fn extra<T>(
    source: &Source,
    chrome: fn(&Source) -> Result<Vec<T>>,
    gecko: fn(&Source) -> Result<Vec<T>>,
    apple: fn(&Source) -> Result<Vec<T>>,
) -> Result<(Vec<T>, u64)> {
    let parser = match source.kind {
        BrowserKind::Chrome | BrowserKind::Chromium => chrome,
        BrowserKind::Firefox => gecko,
        BrowserKind::Safari => apple,
    };
    parser(source).map(|items| (items, 0))
}

/// Select one unexpired cookie per host/name/path across source profiles.
/// Last use wins, with creation as a fallback/tiebreaker; exact ties retain the first source.
/// Returns affected sites only, never values, and adjusts each source's preview/write counts.
pub fn select_recent_cookies(reads: &mut [ProfileRead], now_secs: i64) -> Vec<String> {
    let mut selected = BTreeMap::<(String, String, String), (usize, Cookie)>::new();
    let mut conflicts = BTreeSet::new();
    let rank = |cookie: &Cookie| {
        (
            cookie
                .last_accessed_at_ms
                .or(cookie.created_at_ms)
                .unwrap_or(i64::MIN),
            cookie.created_at_ms.unwrap_or(i64::MIN),
        )
    };
    for (index, read) in reads.iter_mut().enumerate() {
        if let Some(report) = read.reports.get_mut(&DataKind::Cookies) {
            report.imported = 0;
            report.skipped = report
                .skipped
                .saturating_add(read.data.cookies.len() as u64);
        }
        for cookie in read.data.cookies.drain(..) {
            if cookie.expires.is_some_and(|at| at <= now_secs) {
                continue;
            }
            let key = (
                cookie.host.clone(),
                cookie.name.clone(),
                cookie.path.clone(),
            );
            match selected.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((index, cookie));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().0 != index {
                        conflicts.insert(cookie.host.trim_start_matches('.').to_owned());
                    }
                    if rank(&cookie) > rank(&entry.get().1) {
                        entry.insert((index, cookie));
                    }
                }
            }
        }
    }
    for (index, cookie) in selected.into_values() {
        let read = &mut reads[index];
        read.data.cookies.push(cookie);
        if let Some(report) = read.reports.get_mut(&DataKind::Cookies) {
            report.imported = report.imported.saturating_add(1);
            report.skipped = report.skipped.saturating_sub(1);
        }
    }
    conflicts.into_iter().collect()
}

/// Count password records without decrypting them or requiring a configured target Vault.
pub fn password_count(source: &Source) -> Result<u64> {
    match source.kind {
        BrowserKind::Chrome | BrowserKind::Chromium => chromium_extra::password_count(source),
        BrowserKind::Firefox => firefox_extra::password_count(source),
        BrowserKind::Safari => safari::password_count(source),
    }
}

fn cookie_count(source: &Source) -> Result<u64> {
    let (item, table) = match source.kind {
        BrowserKind::Chrome | BrowserKind::Chromium => {
            (chromium::cookies_item(source.kind, &source.dir), "cookies")
        }
        BrowserKind::Firefox => (firefox::cookies_item(&source.dir), "moz_cookies"),
        BrowserKind::Safari => return safari::cookie_count(source),
    };
    let db = item.open()?;
    db.require(&item, table, &[])?;
    db.conn
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(|error| item.sqlite(&error))
}

/// Read selected categories independently; preview only counts cookies/passwords without key access.
pub fn read_profile(
    profile: &DiscoveredProfile,
    kinds: &[DataKind],
    keyring: KeyringSource,
    preview: bool,
) -> ProfileRead {
    let source = profile.source();
    let mut result = ProfileRead::default();
    for kind in kinds {
        match kind {
            DataKind::History => {
                let mut visits = Vec::new();
                let mut sink = |visit| {
                    visits.push(visit);
                    Ok(())
                };
                let read = match source.kind {
                    BrowserKind::Chrome | BrowserKind::Chromium => chromium::read_history(
                        &chromium::history_item(source.kind, &source.dir),
                        &mut sink,
                    )
                    .map(|(_, skipped)| (visits, skipped)),
                    BrowserKind::Firefox => firefox::read_history(
                        &firefox::places_item(&source.dir, "history"),
                        &mut sink,
                    )
                    .map(|(_, skipped)| (visits, skipped)),
                    BrowserKind::Safari => safari::read_history(&source).map(|items| (items, 0)),
                };
                result.data.history = capture(read, &mut result.reports, *kind);
            }
            DataKind::Bookmarks => {
                let read = match source.kind {
                    BrowserKind::Chrome | BrowserKind::Chromium
                        if profile.brand == "arc"
                            && profile.dir.join("StorableSidebar.json").is_file() =>
                    {
                        arc::read_bookmarks(&source)
                    }
                    BrowserKind::Chrome | BrowserKind::Chromium => chromium::read_bookmarks(
                        &chromium::bookmarks_item(source.kind, &source.dir),
                    ),
                    BrowserKind::Firefox => {
                        firefox::read_bookmarks(&firefox::places_item(&source.dir, "bookmarks"))
                    }
                    BrowserKind::Safari => safari::read_bookmarks(&source),
                };
                result.data.bookmarks = capture(read, &mut result.reports, *kind);
            }
            DataKind::Cookies if preview => {
                let report = match cookie_count(&source) {
                    Ok(count) => CategoryReport { read: count, imported: count, note: (count > 0).then(|| "Cookies will be read during import. Your system may ask for access then.".to_owned()), ..CategoryReport::default() },
                    Err(error) => CategoryReport { note: Some(safe_note(&error)), ..CategoryReport::default() },
                };
                result.reports.insert(*kind, report);
            }
            DataKind::Cookies => {
                let mut decryptor = crypto::Decryptor::new(keyring.clone());
                let read = match source.kind {
                    BrowserKind::Chrome | BrowserKind::Chromium => chromium::read_cookies(
                        &chromium::cookies_item(source.kind, &source.dir),
                        &mut decryptor,
                    ),
                    BrowserKind::Firefox => {
                        firefox::read_cookies(&firefox::cookies_item(&source.dir))
                    }
                    BrowserKind::Safari => safari::read_cookies(&source).map(|items| (items, 0)),
                };
                let now = crate::now_secs();
                let read = read.map(|(mut items, skipped)| {
                    let before = items.len();
                    items.retain(|cookie| !cookie.expires.is_some_and(|at| at <= now));
                    let expired = u64::try_from(before - items.len()).unwrap_or(u64::MAX);
                    (items, skipped.saturating_add(expired))
                });
                result.data.cookies = capture(read, &mut result.reports, *kind);
                if let Some(report) = result
                    .reports
                    .get_mut(kind)
                    .filter(|report| report.note.is_some())
                {
                    report.read = cookie_count(&source).unwrap_or_default();
                    report.skipped = report.read;
                }
            }
            DataKind::Passwords if preview => {
                let report = match password_count(&source) { Ok(count) => CategoryReport { read: count, imported: count, note: (count > 0).then(|| "Passwords will be read during import. Your system may ask for access then.".to_owned()), ..CategoryReport::default() }, Err(error) => CategoryReport { note: Some(safe_note(&error)), ..CategoryReport::default() } };
                result.reports.insert(*kind, report);
            }
            DataKind::Passwords => {
                let read = match source.kind {
                    BrowserKind::Chrome | BrowserKind::Chromium => {
                        chromium_passwords::read_passwords(&source, &keyring)
                    }
                    BrowserKind::Firefox => firefox_extra::read_passwords(&source, &keyring),
                    BrowserKind::Safari => safari::read_passwords(&source, &keyring),
                };
                result.data.passwords =
                    capture(read.map(|items| (items, 0)), &mut result.reports, *kind);
                if let Some(report) = result
                    .reports
                    .get_mut(kind)
                    .filter(|report| report.note.is_some())
                {
                    report.read = password_count(&source).unwrap_or_default();
                    report.skipped = report.read;
                }
            }
            DataKind::Autofill => {
                result.data.autofill = capture(
                    extra(
                        &source,
                        chromium_extra::read_autofill,
                        firefox_extra::read_autofill,
                        safari::read_autofill,
                    ),
                    &mut result.reports,
                    *kind,
                )
            }
            DataKind::Tabs
                if profile.brand == "arc" && profile.dir.join("StorableSidebar.json").is_file() =>
            {
                result.data.tabs = capture(
                    arc::read_tabs(&source).map(|items| (items, 0)),
                    &mut result.reports,
                    *kind,
                )
            }
            DataKind::Tabs => {
                result.data.tabs = capture(
                    extra(
                        &source,
                        chromium_extra::read_tabs,
                        firefox_extra::read_tabs,
                        safari::read_tabs,
                    ),
                    &mut result.reports,
                    *kind,
                )
            }
            DataKind::SearchEngines => {
                result.data.search_engines = capture(
                    extra(
                        &source,
                        chromium_extra::read_search_engines,
                        firefox_extra::read_search_engines,
                        safari::read_search_engines,
                    ),
                    &mut result.reports,
                    *kind,
                )
            }
            DataKind::Permissions => {
                result.data.permissions = capture(
                    extra(
                        &source,
                        chromium_extra::read_permissions,
                        firefox_extra::read_permissions,
                        safari::read_permissions,
                    ),
                    &mut result.reports,
                    *kind,
                )
            }
            DataKind::Extensions => {
                result.data.extensions = capture(
                    extra(
                        &source,
                        chromium_extra::read_extensions,
                        firefox_extra::read_extensions,
                        safari::read_extensions,
                    ),
                    &mut result.reports,
                    *kind,
                )
            }
        }
    }
    result
}

fn source_path(target: &ProfileLayout, id: &str) -> PathBuf {
    let digest = Sha256::digest(id.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    target
        .import_dir
        .join("sources")
        .join(format!("{name}.json"))
}

struct Snapshot<'a> {
    profile: &'a DiscoveredProfile,
    read: &'a ProfileRead,
    previous: Option<serde_json::Value>,
}

impl Serialize for Snapshot<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("version", &1)?;
        map.serialize_entry("id", &self.profile.id)?;
        map.serialize_entry("brand", &self.profile.brand)?;
        map.serialize_entry("name", &self.profile.name)?;
        if let Some(previous) = self
            .previous
            .as_ref()
            .and_then(serde_json::Value::as_object)
        {
            for (key, value) in previous {
                let replaced = self
                    .read
                    .reports
                    .iter()
                    .any(|(kind, report)| kind.label() == key && report.note.is_none());
                if !replaced
                    && !matches!(
                        key.as_str(),
                        "version" | "id" | "brand" | "name" | "cookies" | "passwords"
                    )
                {
                    map.serialize_entry(key, value)?;
                }
            }
        }
        for (kind, report) in &self.read.reports {
            if report.note.is_some() {
                continue;
            }
            match kind {
                DataKind::History => {
                    map.serialize_entry(kind.label(), &SanitizedRecords(&self.read.data.history))?
                }
                DataKind::Bookmarks => {
                    map.serialize_entry(kind.label(), &SanitizedRecords(&self.read.data.bookmarks))?
                }
                DataKind::Autofill => {
                    map.serialize_entry(kind.label(), &self.read.data.autofill)?
                }
                DataKind::Tabs => {
                    map.serialize_entry(kind.label(), &SanitizedRecords(&self.read.data.tabs))?
                }
                DataKind::SearchEngines => map.serialize_entry(
                    kind.label(),
                    &SanitizedRecords(&self.read.data.search_engines),
                )?,
                DataKind::Permissions => map.serialize_entry(
                    kind.label(),
                    &SanitizedRecords(&self.read.data.permissions),
                )?,
                DataKind::Extensions => map
                    .serialize_entry(kind.label(), &SanitizedRecords(&self.read.data.extensions))?,
                DataKind::Cookies | DataKind::Passwords => {}
            }
        }
        map.end()
    }
}

/// Replace only selected non-secret snapshot fields; the daemon handles native cookie delivery.
pub fn write_profile(
    target: &ProfileLayout,
    profile: &DiscoveredProfile,
    read: &ProfileRead,
) -> Result<()> {
    let path = source_path(target, &profile.id);
    let directory = target.import_dir.join("sources");
    let replaces_all = DataKind::ALL
        .iter()
        .filter(|kind| !matches!(kind, DataKind::Cookies | DataKind::Passwords))
        .all(|kind| {
            read.reports
                .get(kind)
                .is_some_and(|report| report.note.is_none())
        });
    let previous = if !replaces_all && path.exists() {
        let item = db::Item {
            browser: profile.kind,
            what: "imported profile",
            path: path.clone(),
        };
        let mut previous =
            serde_json::from_slice::<serde_json::Value>(&item.read_bounded(256 * 1024 * 1024)?)
                .map_err(|_| item.unsupported("saved import data is invalid"))?;
        if !previous.is_object() {
            return Err(item.unsupported("saved import data is invalid"));
        }
        sanitize_import_document_urls(&mut previous);
        Some(previous)
    } else {
        None
    };
    store::write_document(
        &directory,
        &path,
        &Snapshot {
            profile,
            read,
            previous,
        },
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testing::{BookmarkEntry, ChromeProfile, CookieSpec, Encryption, HistoryEntry};

    #[test]
    fn category_failure_does_not_block_other_data_and_reimport_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("source");
        ChromeProfile {
            history: vec![HistoryEntry {
                url: "https://example.test/".to_owned(),
                title: None,
                visits_unix_ms: vec![1],
            }],
            encryption: Encryption::Plain,
            ..ChromeProfile::default()
        }
        .write(&dir);
        std::fs::write(dir.join("Network/Cookies"), b"not sqlite").unwrap();
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: dir.clone(),
        };
        let profile = DiscoveredProfile {
            id: "chrome:Default".to_owned(),
            kind: source.kind,
            brand: "chrome".to_owned(),
            name: "Personal".to_owned(),
            email: None,
            avatar: None,
            available: Available::probe(&source),
            dir,
            warnings: Vec::new(),
        };
        let target = ProfileLayout::new(root.path(), "default").unwrap();
        for _ in 0..2 {
            let read = read_profile(
                &profile,
                &[DataKind::History, DataKind::Cookies],
                KeyringSource::None,
                false,
            );
            assert_eq!(read.reports[&DataKind::History].read, 1);
            assert!(read.reports[&DataKind::Cookies].note.is_some());
            write_profile(&target, &profile, &read).unwrap();
        }
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source_path(&target, &profile.id)).unwrap())
                .unwrap();
        assert_eq!(saved["history"].as_array().unwrap().len(), 1);
        assert!(saved.get("cookies").is_none());
        assert!(saved.get("passwords").is_none());
    }

    #[test]
    fn unavailable_keyring_counts_skipped_cookies_but_imports_history() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("source");
        ChromeProfile {
            history: vec![HistoryEntry {
                url: "https://example.test/".to_owned(),
                title: None,
                visits_unix_ms: vec![1],
            }],
            cookies: vec![CookieSpec::new(
                "example.test",
                "sid",
                "synthetic-cookie-canary",
            )],
            encryption: Encryption::V11("fixture-storage-key".to_owned()),
            ..ChromeProfile::default()
        }
        .write(&dir);
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: dir.clone(),
        };
        let profile = DiscoveredProfile {
            id: "fixture".to_owned(),
            kind: source.kind,
            brand: "chrome".to_owned(),
            name: "Personal".to_owned(),
            email: None,
            avatar: None,
            available: Available::probe(&source),
            dir,
            warnings: Vec::new(),
        };
        let read = read_profile(
            &profile,
            &[DataKind::History, DataKind::Cookies],
            KeyringSource::None,
            false,
        );
        assert_eq!(read.reports[&DataKind::History].imported, 1);
        let cookies = &read.reports[&DataKind::Cookies];
        assert_eq!((cookies.read, cookies.imported, cookies.skipped), (1, 0, 1));
        assert!(cookies
            .note
            .as_deref()
            .unwrap()
            .starts_with("Unlock your desktop keyring"));
        assert!(read.data.cookies.is_empty());
        let target = ProfileLayout::new(root.path(), "default").unwrap();
        write_profile(&target, &profile, &read).unwrap();
        assert!(!std::fs::read_to_string(source_path(&target, &profile.id))
            .unwrap()
            .contains("synthetic-cookie-canary"));
    }

    #[test]
    fn credential_preview_counts_rows_without_running_keyring_commands_or_decrypting() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("source");
        ChromeProfile {
            cookies: vec![CookieSpec::new(
                "example.test",
                "sid",
                "preview-cookie-canary",
            )],
            encryption: Encryption::V11("fixture-key".to_owned()),
            ..ChromeProfile::default()
        }
        .write(&dir);
        crate::testing::write_saved_passwords(
            &dir,
            &[("https://example.test/", "person", "preview-password-canary")],
        );
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: dir.clone(),
        };
        let profile = DiscoveredProfile {
            id: "fixture".to_owned(),
            kind: source.kind,
            brand: "chrome".to_owned(),
            name: "Personal".to_owned(),
            email: None,
            avatar: None,
            available: Available::probe(&source),
            dir,
            warnings: Vec::new(),
        };
        let marker = root.path().join("keyring-was-called");
        let command = format!("touch {}", marker.display());
        let preview = read_profile(
            &profile,
            &[DataKind::Cookies, DataKind::Passwords],
            KeyringSource::Command(command),
            true,
        );
        for kind in [DataKind::Cookies, DataKind::Passwords] {
            assert_eq!(
                (preview.reports[&kind].read, preview.reports[&kind].imported),
                (1, 1)
            );
            assert!(preview.reports[&kind]
                .note
                .as_deref()
                .unwrap()
                .contains("during import"));
        }
        assert!(preview.data.cookies.is_empty() && preview.data.passwords.is_empty());
        assert!(!marker.exists());
    }

    #[test]
    fn selecting_history_again_preserves_unselected_bookmarks() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("source");
        ChromeProfile {
            bookmarks: vec![BookmarkEntry {
                folder: vec!["Work".to_owned()],
                title: "Saved".to_owned(),
                url: "https://example.test/saved".to_owned(),
            }],
            ..ChromeProfile::default()
        }
        .write(&dir);
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: dir.clone(),
        };
        let profile = DiscoveredProfile {
            id: "fixture".to_owned(),
            kind: source.kind,
            brand: "chrome".to_owned(),
            name: "Personal".to_owned(),
            email: None,
            avatar: None,
            available: Available::probe(&source),
            dir,
            warnings: Vec::new(),
        };
        let target = ProfileLayout::new(root.path(), "default").unwrap();
        let initial = read_profile(
            &profile,
            &[DataKind::History, DataKind::Bookmarks],
            KeyringSource::None,
            false,
        );
        write_profile(&target, &profile, &initial).unwrap();
        let history = read_profile(&profile, &[DataKind::History], KeyringSource::None, false);
        write_profile(&target, &profile, &history).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source_path(&target, &profile.id)).unwrap())
                .unwrap();
        assert_eq!(saved["bookmarks"][0]["url"], "https://example.test/saved");
    }

    #[test]
    fn arc_shared_source_previews_and_reimports_sidebar_without_duplication() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("Arc");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("StorableSidebar.json"), r#"{"sidebar":{"containers":[{"items":[{"id":"tab","data":{"tab":{"savedURL":"https://arc.example.test/","savedTitle":"Arc page"}}},{"id":"internal","data":{"tab":{"savedURL":"arc://newtab/"}}}]}]}}"#).unwrap();
        let source = Source {
            kind: BrowserKind::Chromium,
            dir: dir.clone(),
        };
        let profile = DiscoveredProfile {
            id: "arc:shared".to_owned(),
            kind: source.kind,
            brand: "arc".to_owned(),
            name: "All Spaces (shared sidebar)".to_owned(),
            email: None,
            avatar: None,
            available: Available::probe(&source),
            dir,
            warnings: Vec::new(),
        };
        let target = ProfileLayout::new(root.path(), "default").unwrap();
        let preview = read_profile(
            &profile,
            &[DataKind::Bookmarks, DataKind::Tabs],
            KeyringSource::None,
            true,
        );
        assert_eq!(
            (
                preview.reports[&DataKind::Bookmarks].imported,
                preview.reports[&DataKind::Bookmarks].skipped
            ),
            (1, 1)
        );
        assert_eq!(preview.reports[&DataKind::Tabs].imported, 1);
        assert!(!target.import_dir.exists());
        for _ in 0..2 {
            let read = read_profile(
                &profile,
                &[DataKind::Bookmarks, DataKind::Tabs],
                KeyringSource::None,
                false,
            );
            write_profile(&target, &profile, &read).unwrap();
        }
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source_path(&target, &profile.id)).unwrap())
                .unwrap();
        assert_eq!(saved["bookmarks"].as_array().unwrap().len(), 1);
        assert_eq!(saved["tabs"].as_array().unwrap().len(), 1);
    }

    fn cookie_read(
        value: &str,
        access: Option<i64>,
        created: Option<i64>,
        expires: Option<i64>,
    ) -> ProfileRead {
        let mut read = ProfileRead::default();
        read.data.cookies.push(Cookie {
            host: ".example.test".to_owned(),
            name: "sid".to_owned(),
            value: value.to_owned(),
            path: "/".to_owned(),
            expires,
            secure: true,
            http_only: true,
            same_site: None,
            last_accessed_at_ms: access,
            created_at_ms: created,
        });
        read.reports.insert(
            DataKind::Cookies,
            CategoryReport {
                read: 1,
                imported: 1,
                ..CategoryReport::default()
            },
        );
        read
    }

    #[test]
    fn cookie_conflicts_choose_recent_use_independent_of_profile_order() {
        for recent_first in [false, true] {
            let old = cookie_read("older-cookie-canary", Some(10), Some(5), None);
            let recent = cookie_read("recent-cookie-canary", Some(20), Some(4), None);
            let mut reads = if recent_first {
                vec![recent, old]
            } else {
                vec![old, recent]
            };
            let sites = select_recent_cookies(&mut reads, 0);
            assert_eq!(sites, ["example.test"]);
            let winner = usize::from(!recent_first);
            assert_eq!(reads[winner].data.cookies[0].value, "recent-cookie-canary");
            assert_eq!(reads[winner].reports[&DataKind::Cookies].imported, 1);
            assert!(reads[1 - winner].data.cookies.is_empty());
            assert_eq!(reads[1 - winner].reports[&DataKind::Cookies].skipped, 1);
            assert!(!format!("{sites:?}").contains("cookie-canary"));
        }
    }

    #[test]
    fn cookie_conflicts_use_creation_and_ignore_expired_candidates() {
        let mut reads = vec![
            cookie_read("older-creation", Some(20), Some(4), None),
            cookie_read("newer-creation", Some(20), Some(8), None),
            cookie_read("expired-newest", Some(100), Some(90), Some(1)),
        ];
        select_recent_cookies(&mut reads, 2);
        assert_eq!(reads[1].data.cookies[0].value, "newer-creation");
        assert!(reads[2].data.cookies.is_empty());
        let mut reads = vec![
            cookie_read("older-access", Some(20), Some(4), None),
            cookie_read("creation-only", None, Some(30), None),
        ];
        select_recent_cookies(&mut reads, 2);
        assert_eq!(reads[1].data.cookies[0].value, "creation-only");
    }

    #[test]
    fn mixed_bookmarks_keep_supported_links_and_count_internal_items() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("source");
        let urls = [
            "https://example.test/",
            "file:///tmp/local-page.html",
            "ftp://example.test/file",
            "chrome://settings/",
            "about:blank",
            "data:text/plain,ignored",
        ];
        ChromeProfile {
            bookmarks: urls
                .iter()
                .map(|url| BookmarkEntry {
                    folder: Vec::new(),
                    title: "Source".to_owned(),
                    url: (*url).to_owned(),
                })
                .collect(),
            ..ChromeProfile::default()
        }
        .write(&dir);
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: dir.clone(),
        };
        let profile = DiscoveredProfile {
            id: "fixture".to_owned(),
            kind: source.kind,
            brand: "chrome".to_owned(),
            name: "Personal".to_owned(),
            email: None,
            avatar: None,
            available: Available::probe(&source),
            dir,
            warnings: Vec::new(),
        };
        let read = read_profile(&profile, &[DataKind::Bookmarks], KeyringSource::None, true);
        assert_eq!(
            (
                read.data.bookmarks.len(),
                read.reports[&DataKind::Bookmarks].skipped
            ),
            (3, 3)
        );
        assert!(read.reports[&DataKind::Bookmarks].note.is_none());
    }
}
