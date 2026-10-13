//! Additional Chromium profile readers. Session data is a restore list, never opened.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde_json::Value;

use crate::db::{CopiedDb, Item};
use crate::model::is_web_url;
use crate::{
    sanitize_import_url, AutofillAddress, ExtensionMetadata, ImportError, RestoreTab, Result,
    SearchEngine, SitePermission, Source,
};

const MAX_PREFERENCES_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;

fn item(source: &Source, what: &'static str, filename: &str) -> Item {
    Item {
        browser: source.kind,
        what,
        path: source.dir.join(filename),
    }
}

pub(crate) fn password_count(source: &Source) -> Result<u64> {
    let mut total = 0;
    let mut found = false;
    for filename in ["Login Data", "Login Data For Account"] {
        let item = item(source, "passwords", filename);
        if !item.probe()? {
            continue;
        }
        found = true;
        let db = item.open()?;
        let columns = db.require(&item, "logins", &["origin_url", "password_value"])?;
        let blocked = if columns.iter().any(|column| column == "blacklisted_by_user") {
            " AND COALESCE(blacklisted_by_user, 0) = 0"
        } else {
            ""
        };
        // Preview must not fetch password blobs or require access to the desktop keyring.
        let count: u64 = db
            .conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM logins WHERE length(password_value) > 0 \
                     AND (substr(lower(origin_url), 1, 7) = 'http://' \
                     OR substr(lower(origin_url), 1, 8) = 'https://'){blocked}"
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|error| item.sqlite(&error))?;
        total += count;
    }
    if !found {
        return Err(item(source, "passwords", "Login Data").missing());
    }
    Ok(total)
}

pub(crate) fn read_autofill(source: &Source) -> Result<Vec<AutofillAddress>> {
    let item = item(source, "autofill", "Web Data");
    let db = item.open()?;
    let mut addresses: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut recognized = false;
    for table in [
        "address_type_tokens",
        "local_addresses_type_tokens",
        "contact_info_type_tokens",
    ] {
        if db.columns(&item, table)?.is_empty() {
            continue;
        }
        db.require(&item, table, &["guid", "type", "value"])?;
        recognized = true;
        let mut statement = db
            .conn
            .prepare(&format!(
                "SELECT guid, type, value FROM {table} ORDER BY guid, type"
            ))
            .map_err(|error| item.sqlite(&error))?;
        let mut rows = statement.query([]).map_err(|error| item.sqlite(&error))?;
        while let Some(row) = rows.next().map_err(|error| item.sqlite(&error))? {
            let (Ok(guid), Ok(kind), Ok(value)) = (
                row.get::<_, String>(0),
                row.get::<_, i64>(1),
                row.get::<_, String>(2),
            ) else {
                continue;
            };
            let Some(field) = address_field(kind) else {
                continue;
            };
            if !guid.is_empty() && !value.trim().is_empty() {
                addresses
                    .entry(guid)
                    .or_default()
                    .insert(field.to_owned(), value);
            }
        }
    }
    for table in ["addresses", "local_addresses", "contact_info"] {
        if !db.columns(&item, table)?.is_empty() {
            read_address_fields(
                &db,
                &item,
                table,
                &[("label", "label"), ("language_code", "language_code")],
                &mut addresses,
                false,
            )?;
        }
    }
    if !db.columns(&item, "autofill_profiles")?.is_empty() {
        recognized = true;
        read_address_fields(
            &db,
            &item,
            "autofill_profiles",
            &[
                ("company_name", "company_name"),
                ("street_address", "street_address"),
                ("dependent_locality", "dependent_locality"),
                ("city", "city"),
                ("state", "state"),
                ("zipcode", "postal_code"),
                ("sorting_code", "sorting_code"),
                ("country_code", "country_code"),
                ("language_code", "language_code"),
                ("label", "label"),
            ],
            &mut addresses,
            true,
        )?;
        for (table, fields) in [
            (
                "autofill_profile_names",
                &[
                    ("first_name", "first_name"),
                    ("middle_name", "middle_name"),
                    ("last_name", "last_name"),
                    ("full_name", "full_name"),
                    ("honorific_prefix", "honorific_prefix"),
                ][..],
            ),
            (
                "autofill_profile_addresses",
                &[
                    ("street_address", "street_address"),
                    ("street_name", "street_name"),
                    ("house_number", "house_number"),
                    ("subpremise", "subpremise"),
                    ("dependent_locality", "dependent_locality"),
                    ("city", "city"),
                    ("state", "state"),
                    ("zip_code", "postal_code"),
                    ("sorting_code", "sorting_code"),
                    ("country_code", "country_code"),
                    ("apartment_number", "apartment_number"),
                    ("floor", "floor"),
                ][..],
            ),
            ("autofill_profile_emails", &[("email", "email")][..]),
            ("autofill_profile_phones", &[("number", "phone")][..]),
        ] {
            if !db.columns(&item, table)?.is_empty() {
                read_address_fields(&db, &item, table, fields, &mut addresses, false)?;
            }
        }
    }
    if !recognized {
        return Err(item.unsupported("No supported autofill address tables were found"));
    }
    Ok(addresses
        .into_values()
        .filter(|fields| !fields.is_empty())
        .map(|fields| AutofillAddress { fields })
        .collect())
}

fn read_address_fields(
    db: &CopiedDb,
    item: &Item,
    table: &str,
    fields: &[(&str, &str)],
    addresses: &mut BTreeMap<String, BTreeMap<String, String>>,
    create: bool,
) -> Result<()> {
    let columns = db.require(item, table, &["guid"])?;
    let fields: Vec<_> = fields
        .iter()
        .filter(|(column, _)| columns.iter().any(|existing| existing == column))
        .collect();
    let mut select = String::from("SELECT guid");
    for (column, _) in &fields {
        select.push_str(", ");
        select.push_str(column);
    }
    select.push_str(" FROM ");
    select.push_str(table);
    select.push_str(" ORDER BY guid");
    let mut statement = db
        .conn
        .prepare(&select)
        .map_err(|error| item.sqlite(&error))?;
    let mut rows = statement.query([]).map_err(|error| item.sqlite(&error))?;
    while let Some(row) = rows.next().map_err(|error| item.sqlite(&error))? {
        let Ok(guid) = row.get::<_, String>(0) else {
            continue;
        };
        if guid.is_empty() {
            continue;
        }
        let address = if create {
            addresses.entry(guid).or_default()
        } else if let Some(address) = addresses.get_mut(&guid) {
            address
        } else {
            continue;
        };
        for (index, (_, field)) in fields.iter().enumerate() {
            if let Ok(value) = row.get::<_, String>(index + 1) {
                if !value.trim().is_empty() {
                    address.insert((*field).to_owned(), value);
                }
            }
        }
    }
    Ok(())
}

fn address_field(kind: i64) -> Option<&'static str> {
    Some(match kind {
        3 => "first_name",
        4 => "middle_name",
        5 => "last_name",
        6 => "middle_initial",
        7 => "full_name",
        8 => "name_suffix",
        9 => "email",
        10 => "phone_number",
        11 => "phone_city_code",
        12 => "phone_country_code",
        13 => "phone_city_and_number",
        14 => "phone",
        30 => "address_line1",
        31 => "address_line2",
        32 => "apartment_number",
        33 => "city",
        34 => "state",
        35 => "postal_code",
        36 => "country_code",
        60 => "company_name",
        77 => "street_address",
        79 => "sorting_code",
        81 => "dependent_locality",
        83 => "address_line3",
        93 => "phone_extension",
        103 => "street_name",
        104 => "house_number",
        105 => "subpremise",
        107 => "first_last_name",
        108 => "conjunction_last_name",
        109 => "second_last_name",
        110 => "honorific_prefix",
        114 => "formatted_address",
        115 => "formatted_address_with_name",
        116 => "floor",
        121 => "phone_city_code_with_trunk_prefix",
        122 => "phone_city_and_number_without_trunk_prefix",
        123 => "phone_number_prefix",
        124 => "phone_number_suffix",
        _ => return None,
    })
}

pub(crate) fn read_search_engines(source: &Source) -> Result<Vec<SearchEngine>> {
    let item = item(source, "search engines", "Web Data");
    let db = item.open()?;
    db.require(&item, "keywords", &["short_name", "keyword", "url"])?;
    let mut statement = db
        .conn
        .prepare("SELECT short_name, keyword, url FROM keywords ORDER BY keyword, short_name, url")
        .map_err(|error| item.sqlite(&error))?;
    let mut rows = statement.query([]).map_err(|error| item.sqlite(&error))?;
    let mut engines = Vec::new();
    while let Some(row) = rows.next().map_err(|error| item.sqlite(&error))? {
        let (Ok(name), Ok(keyword), Ok(url)) = (
            row.get::<_, String>(0),
            row.get::<_, String>(1),
            row.get::<_, String>(2),
        ) else {
            continue;
        };
        if !name.trim().is_empty() && !keyword.trim().is_empty() && is_web_url(&url) {
            let Some(url) = sanitize_import_url(&url) else {
                continue;
            };
            engines.push(SearchEngine { name, keyword, url });
        }
    }
    engines.dedup();
    Ok(engines)
}

fn preferences(source: &Source, what: &'static str) -> Result<Vec<Value>> {
    let mut documents = Vec::new();
    let mut failure = None;
    for filename in ["Preferences", "Secure Preferences"] {
        let item = item(source, what, filename);
        match item.read_bounded(MAX_PREFERENCES_BYTES).and_then(|bytes| {
            serde_json::from_slice::<Value>(&bytes).map_err(|error| ImportError::Corrupt {
                what,
                path: item.path.clone(),
                format: "JSON",
                detail: error.to_string(),
            })
        }) {
            Ok(document) if document.is_object() => documents.push(document),
            Ok(_) => {
                failure
                    .get_or_insert_with(|| item.unsupported("Preferences root is not an object"));
            }
            Err(ImportError::MissingFile { .. }) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    if documents.is_empty() {
        return Err(failure.unwrap_or_else(|| item(source, what, "Preferences").missing()));
    }
    Ok(documents)
}

pub(crate) fn read_extensions(source: &Source) -> Result<Vec<ExtensionMetadata>> {
    let mut extensions = BTreeMap::new();
    for document in preferences(source, "extensions")? {
        let Some(settings) = document
            .pointer("/extensions/settings")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (id, extension) in settings {
            if id.len() != 32 || !id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)) {
                continue;
            }
            if !extension.is_object() {
                continue;
            }
            let manifest = extension.get("manifest");
            let name = manifest
                .and_then(|manifest| manifest.get("name"))
                .or_else(|| extension.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(id);
            let update_url = manifest
                .and_then(|manifest| manifest.get("update_url"))
                .and_then(Value::as_str);
            let store_url = match update_url {
                Some("https://edge.microsoft.com/extensionwebstorebase/v1/crx") => Some(format!(
                    "https://microsoftedge.microsoft.com/addons/detail/{id}"
                )),
                Some("https://clients2.google.com/service/update2/crx")
                | Some("http://clients2.google.com/service/update2/crx") => {
                    Some(format!("https://chromewebstore.google.com/detail/{id}"))
                }
                _ if extension.get("from_webstore").and_then(Value::as_bool) == Some(true) => {
                    Some(format!("https://chromewebstore.google.com/detail/{id}"))
                }
                _ => None,
            };
            extensions.insert(
                id.clone(),
                ExtensionMetadata {
                    id: id.clone(),
                    name: name.to_owned(),
                    store_url,
                },
            );
        }
    }
    Ok(extensions.into_values().collect())
}

pub(crate) fn read_permissions(source: &Source) -> Result<Vec<SitePermission>> {
    let mut permissions = BTreeMap::new();
    for document in preferences(source, "permissions")? {
        let Some(exceptions) = document
            .pointer("/profile/content_settings/exceptions")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (permission, entries) in exceptions {
            let Some(entries) = entries.as_object() else {
                continue;
            };
            for (origin, record) in entries {
                let Some(setting) = record.get("setting").and_then(Value::as_i64) else {
                    continue;
                };
                let setting = match setting {
                    0 => "default",
                    1 => "allow",
                    2 => "block",
                    3 => "ask",
                    4 => "session_only",
                    _ => continue,
                };
                // Preserve both Chromium pattern components; dropping the secondary pattern
                // would turn an embedded-site exception into a broader first-party grant.
                let Some(origin) = crate::url_safety::sanitize_import_origin_pattern(origin) else {
                    continue;
                };
                permissions.insert(
                    (origin.clone(), permission.clone()),
                    SitePermission {
                        origin,
                        permission: permission.clone(),
                        setting: setting.to_owned(),
                    },
                );
            }
        }
    }
    Ok(permissions.into_values().collect())
}

pub(crate) fn read_tabs(source: &Source) -> Result<Vec<RestoreTab>> {
    let sessions = source.dir.join("Sessions");
    let mut candidates = Vec::new();
    match fs::symlink_metadata(&sessions) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let entries = fs::read_dir(&sessions)
                .map_err(|error| item(source, "tabs", "Sessions").unreadable(&error))?;
            let mut current = Vec::new();
            for entry in entries {
                let entry =
                    entry.map_err(|error| item(source, "tabs", "Sessions").unreadable(&error))?;
                let name = entry.file_name();
                let Some(timestamp) = name
                    .to_str()
                    .and_then(|name| name.strip_prefix("Session_"))
                    .and_then(|stamp| stamp.parse::<u64>().ok())
                else {
                    continue;
                };
                current.push((timestamp, entry.path()));
            }
            current.sort_by_key(|(timestamp, _)| std::cmp::Reverse(*timestamp));
            candidates.extend(current.into_iter().map(|(_, path)| path));
        }
        Ok(_) => {
            return Err(
                item(source, "tabs", "Sessions").unsupported("Sessions is not a regular directory")
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(item(source, "tabs", "Sessions").unreadable(&error)),
    }
    candidates.push(source.dir.join("Current Session"));
    candidates.push(source.dir.join("Last Session"));
    let mut failure = None;
    for path in candidates {
        let item = Item {
            browser: source.kind,
            what: "tabs",
            path,
        };
        match item.read_bounded(MAX_SESSION_BYTES) {
            Ok(bytes) => match parse_session(&bytes, &item) {
                Ok(tabs) => return Ok(tabs),
                Err(error @ ImportError::UnsupportedVersion { .. }) => return Err(error),
                Err(error) => {
                    failure.get_or_insert(error);
                }
            },
            Err(ImportError::MissingFile { .. }) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    Err(failure.unwrap_or_else(|| item(source, "tabs", "Sessions").missing()))
}

#[derive(Default)]
struct SessionTab {
    window: i32,
    position: i32,
    selected: Option<i32>,
    navigations: BTreeMap<i32, RestoreTab>,
}

struct Pickle<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Pickle<'a> {
    fn new(bytes: &'a [u8]) -> Option<Self> {
        let size = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        Some(Self {
            bytes: bytes.get(4..4_usize.checked_add(size)?)?,
            cursor: 0,
        })
    }

    fn i32(&mut self) -> Option<i32> {
        let end = self.cursor.checked_add(4)?;
        let value = i32::from_le_bytes(self.bytes.get(self.cursor..end)?.try_into().ok()?);
        self.cursor = end;
        Some(value)
    }

    fn data(&mut self, size: usize) -> Option<&'a [u8]> {
        let end = self.cursor.checked_add(size)?;
        let data = self.bytes.get(self.cursor..end)?;
        self.cursor = end.checked_add(3)? & !3;
        Some(data)
    }

    fn string(&mut self) -> Option<String> {
        let length = usize::try_from(self.i32()?).ok()?;
        std::str::from_utf8(self.data(length)?)
            .ok()
            .map(str::to_owned)
    }

    fn string16(&mut self) -> Option<String> {
        let length = usize::try_from(self.i32()?).ok()?.checked_mul(2)?;
        let words = self
            .data(length)?
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]));
        char::decode_utf16(words)
            .collect::<std::result::Result<String, _>>()
            .ok()
    }
}

fn raw_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn parse_session(bytes: &[u8], item: &Item) -> Result<Vec<RestoreTab>> {
    let corrupt = || ImportError::Corrupt {
        what: "tabs",
        path: item.path.clone(),
        format: "SNSS",
        detail: "Session header or command data is incomplete".into(),
    };
    if bytes.get(..4) != Some(b"SNSS") {
        return Err(corrupt());
    }
    let version = raw_i32(bytes, 4).ok_or_else(corrupt)?;
    if !matches!(version, 1 | 3) {
        return Err(item.unsupported("This session format is encrypted or is not supported"));
    }
    let mut tabs: BTreeMap<i32, SessionTab> = BTreeMap::new();
    let mut closed_windows = BTreeSet::new();
    let mut cursor = 8;
    let mut complete_command = false;
    while let Some(size_bytes) = bytes.get(cursor..cursor + 2) {
        let size = u16::from_le_bytes([size_bytes[0], size_bytes[1]]) as usize;
        cursor += 2;
        if size == 0 {
            break;
        }
        let Some(command) = bytes.get(cursor..cursor + size) else {
            break;
        };
        cursor += size;
        complete_command = true;
        let payload = &command[1..];
        let id = raw_i32(payload, 0);
        let index = raw_i32(payload, 4);
        match (command[0], id, index) {
            (0, Some(window), Some(tab)) => tabs.entry(tab).or_default().window = window,
            (2, Some(tab), Some(position)) => tabs.entry(tab).or_default().position = position,
            (5, Some(tab), Some(index)) if index >= 0 => {
                tabs.entry(tab)
                    .or_default()
                    .navigations
                    .retain(|key, _| *key < index);
            }
            (6, _, _) => {
                if let Some((tab, index, navigation)) = read_navigation(payload) {
                    tabs.entry(tab)
                        .or_default()
                        .navigations
                        .insert(index, navigation);
                }
            }
            (7, Some(tab), Some(index)) if index >= 0 => {
                tabs.entry(tab).or_default().selected = Some(index)
            }
            (9, Some(window), _) => {
                closed_windows.remove(&window);
            }
            (11, Some(tab), Some(count)) if count > 0 => {
                prune(tabs.entry(tab).or_default(), 0, count)
            }
            (16, Some(tab), _) => {
                tabs.remove(&tab);
            }
            (17, Some(window), _) => {
                closed_windows.insert(window);
            }
            (24, Some(tab), Some(index)) if index >= 0 => {
                if let Some(count) = raw_i32(payload, 8).filter(|count| *count > 0) {
                    prune(tabs.entry(tab).or_default(), index, count);
                }
            }
            _ => {}
        }
    }
    if !complete_command && bytes.len() > 8 {
        return Err(corrupt());
    }
    let mut tabs: Vec<_> = tabs.into_iter().collect();
    tabs.sort_by_key(|(id, tab)| (tab.window, tab.position, *id));
    Ok(tabs
        .into_iter()
        .filter(|(_, tab)| !closed_windows.contains(&tab.window))
        .filter_map(|(_, mut tab)| {
            let index = tab
                .selected
                .or_else(|| tab.navigations.last_key_value().map(|(index, _)| *index))?;
            let mut navigation = tab.navigations.remove(&index)?;
            if !is_web_url(&navigation.url) {
                return None;
            }
            let url = sanitize_import_url(&navigation.url)?;
            if navigation.title == navigation.url {
                navigation.title = url.clone();
            }
            navigation.url = url;
            Some(navigation)
        })
        .collect())
}

fn read_navigation(payload: &[u8]) -> Option<(i32, i32, RestoreTab)> {
    let mut pickle = Pickle::new(payload)?;
    let tab = pickle.i32()?;
    let index = pickle.i32()?;
    if index < 0 {
        return None;
    }
    let url = pickle.string()?;
    let title = pickle.string16()?;
    Some((tab, index, RestoreTab { url, title }))
}

fn prune(tab: &mut SessionTab, start: i32, count: i32) {
    let Some(end) = start.checked_add(count) else {
        return;
    };
    if let Some(selected) = tab.selected.as_mut() {
        if *selected >= end {
            *selected -= count;
        } else if *selected >= start {
            *selected = start - 1;
        }
    }
    let later = tab.navigations.split_off(&end);
    tab.navigations.retain(|index, _| *index < start);
    for (index, navigation) in later {
        tab.navigations.insert(index - count, navigation);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::BrowserKind;
    use rusqlite::{params, Connection};
    use serde_json::json;
    use std::path::Path;

    fn source(dir: &Path) -> Source {
        Source {
            kind: BrowserKind::Chrome,
            dir: dir.to_owned(),
        }
    }

    fn write_json(dir: &Path, name: &str, value: Value) {
        fs::write(dir.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn append_command(out: &mut Vec<u8>, id: u8, payload: &[u8]) {
        out.extend_from_slice(&u16::try_from(payload.len() + 1).unwrap().to_le_bytes());
        out.push(id);
        out.extend_from_slice(payload);
    }

    fn append_ints(out: &mut Vec<u8>, id: u8, values: &[i32]) {
        let payload: Vec<_> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        append_command(out, id, &payload);
    }

    fn append_navigation(out: &mut Vec<u8>, tab: i32, index: i32, url: &str, title: &str) {
        let mut payload = Vec::new();
        payload.extend_from_slice(&tab.to_le_bytes());
        payload.extend_from_slice(&index.to_le_bytes());
        payload.extend_from_slice(&i32::try_from(url.len()).unwrap().to_le_bytes());
        payload.extend_from_slice(url.as_bytes());
        payload.resize((payload.len() + 3) & !3, 0);
        let title: Vec<_> = title.encode_utf16().collect();
        payload.extend_from_slice(&i32::try_from(title.len()).unwrap().to_le_bytes());
        for word in title {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        payload.resize((payload.len() + 3) & !3, 0);
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice(&0_i32.to_le_bytes());
        let mut pickle = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        pickle.extend_from_slice(&payload);
        append_command(out, 6, &pickle);
    }

    fn session(version: i32) -> Vec<u8> {
        let mut out = b"SNSS".to_vec();
        out.extend_from_slice(&version.to_le_bytes());
        out
    }

    #[test]
    fn password_preview_counts_without_reading_or_decrypting_values() {
        let dir = tempfile::tempdir().unwrap();
        for filename in ["Login Data", "Login Data For Account"] {
            let db = Connection::open(dir.path().join(filename)).unwrap();
            db.execute_batch("CREATE TABLE logins (origin_url TEXT, password_value BLOB, blacklisted_by_user INTEGER);").unwrap();
            for version in [b"v10", b"v11", b"v20"] {
                db.execute(
                    "INSERT INTO logins VALUES ('https://example.test/', ?1, 0)",
                    params![version.as_slice()],
                )
                .unwrap();
            }
            db.execute_batch(
                "INSERT INTO logins VALUES ('https://example.test/', X'010203', 1),
                 ('https://example.test/', X'', 0), ('https://example.test/', NULL, 0),
                 ('android://app', X'010203', 0);",
            )
            .unwrap();
        }
        assert_eq!(password_count(&source(dir.path())).unwrap(), 6);
        fs::remove_file(dir.path().join("Login Data")).unwrap();
        assert_eq!(password_count(&source(dir.path())).unwrap(), 3);
    }

    #[test]
    fn modern_autofill_preserves_supported_fields_and_skips_bad_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join("Web Data")).unwrap();
        db.execute_batch(
            "CREATE TABLE addresses (guid TEXT, label TEXT, language_code TEXT);
             INSERT INTO addresses VALUES ('a', 'Home', 'en'), ('orphan', 'Ignore', 'en');
             CREATE TABLE address_type_tokens (guid TEXT, type INTEGER, value TEXT);
             INSERT INTO address_type_tokens VALUES
             ('a', 7, 'Ada Person'), ('a', 77, '12 Main St'), ('a', 35, '12345'),
             ('a', 36, 'US'), ('b', 9, 'second@example.test'),
             ('a', 75, 'must-not-import'), ('broken', 7, X'FFFF'),
             ('empty', 7, ''), (NULL, 7, 'missing');",
        )
        .unwrap();
        let addresses = read_autofill(&source(dir.path())).unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(addresses[0].fields["full_name"], "Ada Person");
        assert_eq!(addresses[0].fields["postal_code"], "12345");
        assert_eq!(addresses[0].fields["label"], "Home");
        assert_eq!(addresses[0].fields["language_code"], "en");
        assert_eq!(addresses[0].fields.len(), 6);
        assert_eq!(addresses[1].fields["email"], "second@example.test");
    }

    #[test]
    fn intermediate_autofill_tables_are_merged() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join("Web Data")).unwrap();
        db.execute_batch(
            "CREATE TABLE local_addresses_type_tokens (guid TEXT, type INTEGER, value TEXT);
             CREATE TABLE contact_info_type_tokens (guid TEXT, type INTEGER, value TEXT);
             INSERT INTO local_addresses_type_tokens VALUES ('local', 33, 'Paris');
             INSERT INTO contact_info_type_tokens VALUES ('account', 7, 'Account Person');",
        )
        .unwrap();
        let addresses = read_autofill(&source(dir.path())).unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(addresses[0].fields["full_name"], "Account Person");
        assert_eq!(addresses[1].fields["city"], "Paris");
    }

    #[test]
    fn legacy_autofill_joins_names_emails_phones_and_structured_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join("Web Data")).unwrap();
        db.execute_batch(
            "CREATE TABLE autofill_profiles (guid TEXT, city TEXT, zipcode TEXT, country_code TEXT);
             CREATE TABLE autofill_profile_names (guid TEXT, full_name TEXT);
             CREATE TABLE autofill_profile_addresses (guid TEXT, street_address TEXT, zip_code TEXT);
             CREATE TABLE autofill_profile_emails (guid TEXT, email TEXT);
             CREATE TABLE autofill_profile_phones (guid TEXT, number TEXT);
             INSERT INTO autofill_profiles VALUES ('a', 'London', 'old', 'GB'), ('b', NULL, NULL, NULL);
             INSERT INTO autofill_profile_names VALUES ('a', 'Ada Person'), ('orphan', 'Ignore');
             INSERT INTO autofill_profile_addresses VALUES ('a', '12 Main St', 'SW1A 1AA');
             INSERT INTO autofill_profile_emails VALUES ('a', 'ada@example.test');
             INSERT INTO autofill_profile_phones VALUES ('a', '+441234');"
        ).unwrap();
        let addresses = read_autofill(&source(dir.path())).unwrap();
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].fields["postal_code"], "SW1A 1AA");
        assert_eq!(addresses[0].fields["full_name"], "Ada Person");
        assert_eq!(addresses[0].fields["email"], "ada@example.test");
        assert_eq!(addresses[0].fields["phone"], "+441234");
    }

    #[test]
    fn search_templates_survive_and_corrupt_rows_do_not_abort() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join("Web Data")).unwrap();
        db.execute_batch(
            "CREATE TABLE keywords (short_name TEXT, keyword TEXT, url TEXT);
             INSERT INTO keywords VALUES
             ('Search', 's', 'https://search.example.test/?q={searchTerms}'),
             ('Search', 's', 'https://search.example.test/?q={searchTerms}'),
             ('History', '@history', 'chrome://history/?q={searchTerms}'),
             (NULL, 'bad', 'https://example.test/'),
             ('Bad', X'FFFF', 'https://example.test/');",
        )
        .unwrap();
        let engines = read_search_engines(&source(dir.path())).unwrap();
        assert_eq!(engines.len(), 1);
        assert_eq!(
            engines[0].url,
            "https://search.example.test/?q={searchTerms}"
        );
    }

    #[test]
    fn secure_preferences_win_and_only_official_store_links_are_emitted() {
        let dir = tempfile::tempdir().unwrap();
        let chrome = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let edge = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let local = "cccccccccccccccccccccccccccccccc";
        write_json(
            dir.path(),
            "Preferences",
            json!({"extensions":{"settings":{
                chrome: {"manifest":{"name":"Old name"},"from_webstore":true},
                edge: {"manifest":{"name":"Edge Addon", "update_url":"https://edge.microsoft.com/extensionwebstorebase/v1/crx"}},
                local: {"manifest":{"name":"Local", "homepage_url":"https://not-a-store.test/"}},
                "invalid": {"manifest":{"name":"Ignore"}}
            }}}),
        );
        write_json(
            dir.path(),
            "Secure Preferences",
            json!({"extensions":{"settings":{
                chrome: {"manifest":{"name":"New name","update_url":"https://clients2.google.com/service/update2/crx"}}
            }}}),
        );
        let extensions = read_extensions(&source(dir.path())).unwrap();
        assert_eq!(extensions.len(), 3);
        assert_eq!(extensions[0].name, "New name");
        assert_eq!(
            extensions[0].store_url.as_deref(),
            Some(format!("https://chromewebstore.google.com/detail/{chrome}").as_str())
        );
        assert_eq!(
            extensions[1].store_url.as_deref(),
            Some(format!("https://microsoftedge.microsoft.com/addons/detail/{edge}").as_str())
        );
        assert_eq!(extensions[2].store_url, None);
    }

    #[test]
    fn valid_secure_preferences_survive_corrupt_preferences_and_permission_rows() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Preferences"), b"{bad json").unwrap();
        write_json(
            dir.path(),
            "Secure Preferences",
            json!({
                "extensions":{"settings":{"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa":{"manifest":{"name":"Safe"}}}},
                "profile":{"content_settings":{"exceptions":{
                    "notifications":{"https://allowed.test:443,*":{"setting":1}, "broken":{"setting":{"bad":true}}},
                    "geolocation":{"https://embedded.test:443,https://parent.test:443":{"setting":2}},
                    "camera":{"https://ask.test:443,*":{"setting":3}, "future":{"setting":99}}
                }}}
            }),
        );
        assert_eq!(read_extensions(&source(dir.path())).unwrap().len(), 1);
        let permissions = read_permissions(&source(dir.path())).unwrap();
        assert_eq!(permissions.len(), 3);
        assert!(permissions.iter().any(|permission| permission.origin
            == "https://embedded.test:443,https://parent.test:443"
            && permission.setting == "block"));
    }

    #[test]
    fn session_replay_uses_selected_navigation_order_and_closed_state() {
        let dir = tempfile::tempdir().unwrap();
        for version in [1, 3] {
            let mut bytes = session(version);
            append_ints(&mut bytes, 0, &[10, 1]);
            append_ints(&mut bytes, 0, &[10, 2]);
            append_ints(&mut bytes, 2, &[1, 1]);
            append_ints(&mut bytes, 2, &[2, 0]);
            append_navigation(&mut bytes, 1, 0, "https://selected.test/", "Selected 🦀");
            append_navigation(&mut bytes, 1, 1, "https://forward.test/", "Forward");
            append_ints(&mut bytes, 7, &[1, 0]);
            append_navigation(&mut bytes, 2, 0, "https://first.test/", "First");
            append_navigation(&mut bytes, 3, 0, "https://closed.test/", "Closed");
            append_ints(&mut bytes, 16, &[3]);
            append_ints(&mut bytes, 0, &[20, 4]);
            append_navigation(&mut bytes, 4, 0, "https://window.test/", "Closed window");
            append_ints(&mut bytes, 17, &[20]);
            append_navigation(&mut bytes, 5, 0, "https://previous.test/", "Previous");
            append_navigation(&mut bytes, 5, 1, "chrome://settings/", "Internal");
            append_ints(&mut bytes, 7, &[5, 1]);
            append_command(&mut bytes, 255, &[]);
            append_command(&mut bytes, 6, b"broken");
            bytes.extend_from_slice(&[200, 0, 6, 1]);
            fs::write(dir.path().join("Current Session"), bytes).unwrap();
            let tabs = read_tabs(&source(dir.path())).unwrap();
            assert_eq!(tabs.len(), 2);
            assert_eq!(tabs[0].url, "https://first.test/");
            assert_eq!(tabs[1].url, "https://selected.test/");
            assert_eq!(tabs[1].title, "Selected 🦀");
        }
    }

    #[test]
    fn session_pruning_shifts_navigation_and_selected_indices() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = session(3);
        for index in 0..4 {
            append_navigation(
                &mut bytes,
                1,
                index,
                &format!("https://tab.test/{index}"),
                "",
            );
        }
        append_ints(&mut bytes, 7, &[1, 3]);
        append_ints(&mut bytes, 11, &[1, 1]);
        append_ints(&mut bytes, 24, &[1, 1, 1]);
        fs::write(dir.path().join("Last Session"), bytes).unwrap();
        assert_eq!(
            read_tabs(&source(dir.path())).unwrap()[0].url,
            "https://tab.test/3"
        );
    }

    #[test]
    fn modern_sessions_choose_newest_and_fall_back_only_from_malformed_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Sessions")).unwrap();
        let mut bytes = session(3);
        append_navigation(&mut bytes, 1, 0, "https://newest.test/", "Newest");
        fs::write(dir.path().join("Sessions/Session_9"), b"bad").unwrap();
        fs::write(dir.path().join("Sessions/Session_8"), &bytes).unwrap();
        fs::write(
            dir.path().join("Sessions/Tabs_10"),
            b"must not read closed tabs",
        )
        .unwrap();
        assert_eq!(
            read_tabs(&source(dir.path())).unwrap()[0].url,
            "https://newest.test/"
        );
        fs::write(dir.path().join("Sessions/Session_9"), session(3)).unwrap();
        assert!(read_tabs(&source(dir.path())).unwrap().is_empty());
        for version in [2, 4, 5, 99] {
            fs::write(dir.path().join("Sessions/Session_9"), session(version)).unwrap();
            assert!(matches!(
                read_tabs(&source(dir.path())),
                Err(ImportError::UnsupportedVersion { .. })
            ));
        }
    }

    #[test]
    fn parser_rejects_missing_schemas_bad_headers_and_linked_files() {
        let dir = tempfile::tempdir().unwrap();
        Connection::open(dir.path().join("Web Data")).unwrap();
        assert!(matches!(
            read_autofill(&source(dir.path())),
            Err(ImportError::UnsupportedVersion { .. })
        ));
        assert!(matches!(
            read_search_engines(&source(dir.path())),
            Err(ImportError::UnsupportedVersion { .. })
        ));
        for bytes in [b"SNSS".as_slice(), b"not SNSS".as_slice()] {
            fs::write(dir.path().join("Current Session"), bytes).unwrap();
            assert!(matches!(
                read_tabs(&source(dir.path())),
                Err(ImportError::Corrupt { .. })
            ));
        }
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("Preferences"), b"{}").unwrap();
        std::os::unix::fs::symlink(
            other.path().join("Preferences"),
            dir.path().join("Preferences"),
        )
        .unwrap();
        assert!(matches!(
            read_extensions(&source(dir.path())),
            Err(ImportError::Unreadable { .. })
        ));
        std::os::unix::fs::symlink(other.path(), dir.path().join("Sessions")).unwrap();
        assert!(matches!(
            read_tabs(&source(dir.path())),
            Err(ImportError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn oversized_source_documents_fail_without_reading_their_contents() {
        let dir = tempfile::tempdir().unwrap();
        fs::File::create(dir.path().join("Preferences"))
            .unwrap()
            .set_len(MAX_PREFERENCES_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_extensions(&source(dir.path())),
            Err(ImportError::UnsupportedVersion { .. })
        ));
        fs::File::create(dir.path().join("Current Session"))
            .unwrap()
            .set_len(MAX_SESSION_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_tabs(&source(dir.path())),
            Err(ImportError::UnsupportedVersion { .. })
        ));
    }
}
