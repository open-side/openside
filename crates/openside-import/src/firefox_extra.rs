use std::collections::BTreeMap;

use serde_json::Value;
use zeroize::Zeroizing;

use crate::db::Item;
use crate::error::{ImportError, Result};
use crate::firefox_crypto::LoginKeys;
use crate::model::{
    is_web_url, AutofillAddress, ExtensionMetadata, RestoreTab, SavedPassword, SearchEngine,
    SitePermission,
};
use crate::{sanitize_import_url, KeyringSource, Source};

const MAX_JSON_BYTES: usize = 128 * 1024 * 1024;
const MOZLZ4_MAGIC: &[u8; 8] = b"mozLz40\0";

fn item(source: &Source, what: &'static str, name: &str) -> Item {
    Item {
        browser: source.kind,
        what,
        path: source.dir.join(name),
    }
}

fn json<T: serde::de::DeserializeOwned>(item: &Item, compressed: bool) -> Result<T> {
    let bytes = Zeroizing::new(item.read_bounded(MAX_JSON_BYTES as u64)?);
    let decoded;
    let bytes = if compressed {
        decoded = Zeroizing::new(decode_mozlz4(&bytes).map_err(|detail| item.unsupported(detail))?);
        &decoded[..]
    } else {
        &bytes[..]
    };
    serde_json::from_slice(bytes).map_err(|_| ImportError::Corrupt {
        what: item.what,
        path: item.path.clone(),
        format: "JSON",
        detail: "Invalid browser data JSON.".into(),
    })
}

fn array<'a>(value: &'a Value, key: &str, item: &Item) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| item.unsupported("The browser data layout is unsupported."))
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn decode_mozlz4(bytes: &[u8]) -> std::result::Result<Vec<u8>, &'static str> {
    if bytes.get(..8) != Some(MOZLZ4_MAGIC.as_slice()) {
        return Err("Invalid Mozilla compressed-data header.");
    }
    let size = u32::from_le_bytes(
        bytes
            .get(8..12)
            .ok_or("Truncated Mozilla compressed data.")?
            .try_into()
            .map_err(|_| "Truncated Mozilla compressed data.")?,
    ) as usize;
    if size > MAX_JSON_BYTES {
        return Err("Mozilla compressed data is too large to import safely.");
    }
    let decoded = lz4_flex::block::decompress(&bytes[12..], size)
        .map_err(|_| "Mozilla compressed data is damaged.")?;
    if decoded.len() != size {
        return Err("Mozilla compressed-data size does not match its header.");
    }
    Ok(decoded)
}

pub(crate) fn password_count(source: &Source) -> Result<u64> {
    let input = item(source, "passwords", "logins.json");
    if !input.probe()? {
        return Ok(0);
    }
    let value = json(&input, false)?;
    Ok(array(&value, "logins", &input)?
        .iter()
        .filter(|login| is_web_url(&text(login, "hostname")))
        .count() as u64)
}

pub(crate) fn read_passwords(
    source: &Source,
    _keyring: &KeyringSource,
) -> Result<Vec<SavedPassword>> {
    let input = item(source, "passwords", "logins.json");
    let value = json(&input, false)?;
    let logins = array(&value, "logins", &input)?;
    if logins.is_empty() {
        return Ok(Vec::new());
    }
    let keys = LoginKeys::load(&item(source, "passwords", "key4.db"))?;
    let mut result = Vec::new();
    for login in logins {
        let origin = text(login, "hostname");
        if !is_web_url(&origin) {
            continue;
        }
        let username = keys
            .decrypt(
                login
                    .get("encryptedUsername")
                    .and_then(Value::as_str)
                    .ok_or_else(|| input.unsupported("Firefox saved username is missing."))?,
            )
            .map_err(|detail| input.unsupported(detail))?;
        let password = keys
            .decrypt(
                login
                    .get("encryptedPassword")
                    .and_then(Value::as_str)
                    .ok_or_else(|| input.unsupported("Firefox saved password is missing."))?,
            )
            .map_err(|detail| input.unsupported(detail))?;
        result.push(SavedPassword {
            origin,
            username,
            password,
        });
    }
    Ok(result)
}

const ADDRESS_FIELDS: &[&str] = &[
    "name",
    "given-name",
    "additional-name",
    "family-name",
    "organization",
    "street-address",
    "address-line1",
    "address-line2",
    "address-line3",
    "address-level1",
    "address-level2",
    "address-level3",
    "address-level4",
    "city",
    "state",
    "country",
    "country-name",
    "postal-code",
    "zip",
    "email",
    "tel",
    "tel-country-code",
    "tel-national",
    "tel-area-code",
    "tel-local",
];

pub(crate) fn read_autofill(source: &Source) -> Result<Vec<AutofillAddress>> {
    let mut result = Vec::new();
    let profiles = item(source, "autofill", "autofill-profiles.json");
    let profiles_present = profiles.probe()?;
    if profiles_present {
        let value = json(&profiles, false)?;
        for address in array(&value, "addresses", &profiles)? {
            let fields: BTreeMap<String, String> = ADDRESS_FIELDS
                .iter()
                .filter_map(|field| {
                    address
                        .get(*field)
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(|value| ((*field).to_owned(), value.to_owned()))
                })
                .collect();
            if !fields.is_empty() {
                result.push(AutofillAddress { fields });
            }
        }
    }
    let history = item(source, "autofill", "formhistory.sqlite");
    let history_present = history.probe()?;
    if history_present {
        let db = history.open()?;
        db.require(&history, "moz_formhistory", &["fieldname", "value"])?;
        let mut statement = db.conn.prepare("SELECT fieldname,value FROM moz_formhistory WHERE fieldname IS NOT NULL AND value IS NOT NULL ORDER BY fieldname,value")
            .map_err(|error| history.sqlite(&error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| history.sqlite(&error))?;
        for row in rows {
            let (field, value) = row.map_err(|error| history.sqlite(&error))?;
            if ADDRESS_FIELDS.contains(&field.as_str()) && !value.is_empty() {
                // Form history has no address grouping; joining unrelated rows would
                // invent a person's identity, so preserve each field independently.
                result.push(AutofillAddress {
                    fields: BTreeMap::from([(field, value)]),
                });
            }
        }
    }
    if !profiles_present && !history_present {
        return Err(profiles.missing());
    }
    Ok(result)
}

// Recovery files also contain form contents and session cookies. Typed decoding
// ignores those fields instead of copying unrelated secrets into a JSON tree.
#[derive(serde::Deserialize)]
struct SessionState {
    windows: Vec<SessionWindow>,
}
#[derive(serde::Deserialize)]
struct SessionWindow {
    tabs: Vec<SessionTab>,
}
#[derive(serde::Deserialize)]
struct SessionTab {
    index: Option<u64>,
    #[serde(default)]
    entries: Vec<SessionEntry>,
}
#[derive(serde::Deserialize)]
struct SessionEntry {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
}

pub(crate) fn read_tabs(source: &Source) -> Result<Vec<RestoreTab>> {
    let mut selected = None;
    for name in [
        "sessionstore-backups/recovery.jsonlz4",
        "sessionstore-backups/recovery.baklz4",
        "sessionstore-backups/previous.jsonlz4",
        "sessionstore.jsonlz4",
    ] {
        let candidate = item(source, "tabs", name);
        if candidate.probe()? {
            selected = Some(candidate);
            break;
        }
    }
    let input =
        selected.unwrap_or_else(|| item(source, "tabs", "sessionstore-backups/recovery.jsonlz4"));
    let value: SessionState = json(&input, true)?;
    let mut result = Vec::new();
    for window in value.windows {
        for tab in window.tabs {
            let index = tab
                .index
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| usize::try_from(index).ok())
                .unwrap_or_else(|| tab.entries.len().saturating_sub(1));
            let Some(entry) = tab.entries.into_iter().nth(index) else {
                continue;
            };
            if !is_web_url(&entry.url) {
                continue;
            }
            let Some(url) = sanitize_import_url(&entry.url) else {
                continue;
            };
            let title = if entry.title == entry.url {
                url.clone()
            } else {
                entry.title
            };
            result.push(RestoreTab { url, title });
        }
    }
    Ok(result)
}

fn query_component(input: &str) -> String {
    let mut output = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~{}".contains(&byte) {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

pub(crate) fn read_search_engines(source: &Source) -> Result<Vec<SearchEngine>> {
    let input = item(source, "search_engines", "search.json.mozlz4");
    let value = json(&input, true)?;
    let mut result = Vec::new();
    for engine in array(&value, "engines", &input)? {
        if engine.pointer("/_metaData/hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(urls) = engine.get("_urls").and_then(Value::as_array) else {
            continue;
        };
        let selected = urls.iter().find(|url| {
            url.get("type").and_then(Value::as_str) == Some("text/html")
                && url
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .eq_ignore_ascii_case("GET")
        });
        let Some(selected) = selected else {
            continue;
        };
        let mut url = text(selected, "template");
        if !is_web_url(&url) {
            continue;
        }
        if let Some(params) = selected
            .get("params")
            .or_else(|| selected.get("_params"))
            .and_then(Value::as_array)
        {
            for param in params {
                let name = text(param, "name");
                let value = text(param, "value");
                if name.is_empty() {
                    continue;
                }
                url.push(if url.contains('?') { '&' } else { '?' });
                url.push_str(&query_component(&name));
                url.push('=');
                url.push_str(&query_component(&value));
            }
        }
        let Some(url) = sanitize_import_url(&url) else {
            continue;
        };
        result.push(SearchEngine {
            name: text(engine, "_name"),
            keyword: engine
                .pointer("/_metaData/alias")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            url,
        });
    }
    Ok(result)
}

pub(crate) fn read_permissions(source: &Source) -> Result<Vec<SitePermission>> {
    let input = item(source, "permissions", "permissions.sqlite");
    let db = input.open()?;
    db.require(&input, "moz_perms", &["origin", "type", "permission"])?;
    let mut statement = db.conn.prepare("SELECT origin,type,permission FROM moz_perms WHERE origin IS NOT NULL AND type IS NOT NULL ORDER BY origin,type")
        .map_err(|error| input.sqlite(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| input.sqlite(&error))?;
    let mut result = Vec::new();
    for row in rows {
        let (origin, permission, setting) = row.map_err(|error| input.sqlite(&error))?;
        // Firefox container/private origins cannot retain isolation in the target profile.
        if is_web_url(&origin) && !origin.contains('^') {
            let Some(origin) = sanitize_import_url(&origin) else {
                continue;
            };
            result.push(SitePermission {
                origin,
                permission,
                setting: match setting {
                    1 => "allow".into(),
                    2 => "block".into(),
                    3 => "prompt".into(),
                    _ => "default".into(),
                },
            });
        }
    }
    Ok(result)
}

pub(crate) fn read_extensions(source: &Source) -> Result<Vec<ExtensionMetadata>> {
    let input = item(source, "extensions", "extensions.json");
    let value = json(&input, false)?;
    let mut result = Vec::new();
    for addon in array(&value, "addons", &input)? {
        if addon.get("type").and_then(Value::as_str) != Some("extension") {
            continue;
        }
        let id = text(addon, "id");
        if id.is_empty() {
            continue;
        }
        let name = addon
            .pointer("/defaultLocale/name")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        let listing = addon
            .pointer("/amoListingURL")
            .and_then(Value::as_str)
            .filter(|url| {
                url.starts_with("https://addons.mozilla.org/") && url.contains("/firefox/addon/")
            });
        let store_url = Some(listing.and_then(sanitize_import_url).unwrap_or_else(|| {
            format!(
                "https://addons.mozilla.org/firefox/search/?q={}",
                query_component(&id)
            )
        }));
        result.push(ExtensionMetadata {
            id,
            name,
            store_url,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BrowserKind;

    fn source(dir: &std::path::Path) -> Source {
        Source {
            kind: BrowserKind::Firefox,
            dir: dir.to_owned(),
        }
    }
    fn compressed(value: &Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(value).unwrap();
        let mut output = MOZLZ4_MAGIC.to_vec();
        output.extend(lz4_flex::block::compress_prepend_size(&bytes));
        output
    }

    #[test]
    fn moz_lz4_roundtrip_and_size_limits() {
        let value = serde_json::json!({"fixture":"synthetic","windows":[]});
        let bytes = compressed(&value);
        assert_eq!(
            serde_json::from_slice::<Value>(&decode_mozlz4(&bytes).unwrap()).unwrap(),
            value
        );
        assert!(decode_mozlz4(b"mozLz40\0").is_err());
        assert!(decode_mozlz4(b"wrong-header").is_err());
        let mut bomb = MOZLZ4_MAGIC.to_vec();
        bomb.extend(u32::MAX.to_le_bytes());
        assert!(decode_mozlz4(&bomb).is_err());
        let mut wrong_size = bytes;
        wrong_size[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode_mozlz4(&wrong_size).is_err());
    }

    #[test]
    fn session_tabs_use_current_entry_and_never_open_windows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sessionstore-backups")).unwrap();
        let value = serde_json::json!({"windows":[{"tabs":[
            {"index":1,"entries":[{"url":"https://selected.test/","title":"Selected"},{"url":"https://old.test/"}]},
            {"entries":[{"url":"about:config"}]}, {"index":9,"entries":[]}
        ]}]});
        std::fs::write(
            dir.path().join("sessionstore-backups/recovery.jsonlz4"),
            compressed(&value),
        )
        .unwrap();
        let tabs = read_tabs(&source(dir.path())).unwrap();
        assert_eq!(
            tabs,
            vec![RestoreTab {
                url: "https://selected.test/".into(),
                title: "Selected".into()
            }]
        );
    }

    #[test]
    fn synthetic_login_count_does_not_unlock_primary_password() {
        let dir = tempfile::tempdir().unwrap();
        crate::firefox_crypto::tests::write_profile(dir.path(), false, true);
        let source = source(dir.path());
        assert_eq!(password_count(&source).unwrap(), 1);
        assert!(read_passwords(&source, &KeyringSource::None).is_err());
        std::fs::write(dir.path().join("extensions.json"), br#"{"addons":[{"id":"fixture@example.test","type":"extension","defaultLocale":{"name":"Fixture"}}]}"#).unwrap();
        assert_eq!(read_extensions(&source).unwrap().len(), 1);
    }

    #[test]
    fn addresses_and_formhistory_do_not_import_credit_cards_or_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("autofill-profiles.json"), br#"{"addresses":[{"given-name":"Fixture","email":"fixture@example.test","guid":"ignored"}],"creditCards":[{"cc-number":"ignored"}]}"#).unwrap();
        let db = rusqlite::Connection::open(dir.path().join("formhistory.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE moz_formhistory(fieldname TEXT,value TEXT); INSERT INTO moz_formhistory VALUES('postal-code','12345'),('password','never import'),('search','irrelevant');").unwrap();
        let addresses = read_autofill(&source(dir.path())).unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(addresses[0].fields.len(), 2);
        assert_eq!(addresses[1].fields["postal-code"], "12345");
    }

    #[test]
    fn search_engines_preserve_parameters_and_skip_post_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let value = serde_json::json!({"engines":[
            {"_name":"Fixture","_metaData":{"alias":"fx"},"_urls":[{"type":"text/html","method":"GET","template":"https://search.test/","params":[{"name":"q","value":"{searchTerms}"},{"name":"src","value":"a b"}]}]},
            {"_name":"POST","_urls":[{"type":"text/html","method":"POST","template":"https://search.test/"}]},
            {"_name":"Hidden","_metaData":{"hidden":true},"_urls":[{"type":"text/html","template":"https://hidden.test/"}]}
        ]});
        std::fs::write(dir.path().join("search.json.mozlz4"), compressed(&value)).unwrap();
        let engines = read_search_engines(&source(dir.path())).unwrap();
        assert_eq!(engines.len(), 1);
        assert_eq!(
            engines[0].url,
            "https://search.test/?q={searchTerms}&src=a%20b"
        );
        assert_eq!(engines[0].keyword, "fx");
    }

    #[test]
    fn extension_metadata_never_keeps_unofficial_download_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("extensions.json"), br#"{"addons":[{"id":"fixture@example.test","type":"extension","sourceURI":"https://untrusted.test/fixture.xpi","defaultLocale":{"name":"Fixture"}},{"id":"theme","type":"theme"}]}"#).unwrap();
        let extensions = read_extensions(&source(dir.path())).unwrap();
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].name, "Fixture");
        assert!(extensions[0]
            .store_url
            .as_ref()
            .unwrap()
            .starts_with("https://addons.mozilla.org/"));
    }
}
