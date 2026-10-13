use url::Url;

/// Project an imported address without URL credentials or recognized bearer parameters.
/// Unknown parameters and path segments can still contain private data.
pub fn sanitize_import_url(input: &str) -> Option<String> {
    let mut url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "file" | "ftp") {
        return None;
    }
    // File URLs cannot contain userinfo; their setters reject even an empty username.
    if url.scheme() != "file" {
        url.set_username("").ok()?;
        url.set_password(None).ok()?;
    }
    if let Some(query) = url.query().and_then(without_secret_params) {
        url.set_query((!query.is_empty()).then_some(query.as_str()));
    }
    if let Some(fragment) = url.fragment() {
        if implicit_response(fragment) {
            url.set_fragment(None);
        } else if let Some((prefix, params)) = fragment.split_once('?').filter(|(prefix, _)| {
            !prefix.contains(['=', '&']) && !secret_param(&param_name(prefix))
        }) {
            if let Some(clean) = without_secret_params(params) {
                let fragment = if clean.is_empty() {
                    prefix.to_owned()
                } else {
                    format!("{prefix}?{clean}")
                };
                url.set_fragment((!fragment.is_empty()).then_some(fragment.as_str()));
            }
        } else if let Some(clean) = without_secret_params(fragment) {
            url.set_fragment((!clean.is_empty()).then_some(clean.as_str()));
        }
    }
    Some(url.into())
}

/// Remove recognized URL secrets from a previously persisted source snapshot.
pub fn sanitize_import_document_urls(document: &mut serde_json::Value) {
    for category in ["history", "bookmarks", "tabs", "search_engines"] {
        if let Some(records) = document
            .get_mut(category)
            .and_then(serde_json::Value::as_array_mut)
        {
            records.retain_mut(|record| {
                let Some(raw) = record.get("url").and_then(serde_json::Value::as_str) else {
                    return false;
                };
                let Some(clean) = sanitize_import_url(raw) else {
                    return false;
                };
                if record.get("title").and_then(serde_json::Value::as_str) == Some(raw) {
                    record["title"] = clean.clone().into();
                }
                record["url"] = clean.into();
                true
            });
        }
    }
    if let Some(records) = document
        .get_mut("permissions")
        .and_then(serde_json::Value::as_array_mut)
    {
        records.retain_mut(|record| {
            let Some(clean) = record
                .get("origin")
                .and_then(serde_json::Value::as_str)
                .and_then(sanitize_import_origin_pattern)
            else {
                return false;
            };
            record["origin"] = clean.into();
            true
        });
    }
    if let Some(records) = document
        .get_mut("extensions")
        .and_then(serde_json::Value::as_array_mut)
    {
        for record in records {
            if let Some(fields) = record.as_object_mut() {
                if let Some(link) = fields.get_mut("store_url") {
                    *link = link.as_str().and_then(sanitize_import_url).into();
                }
            }
        }
    }
}

pub(crate) fn sanitize_import_origin_pattern(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    for (index, component) in input.split(',').enumerate() {
        if index > 0 {
            output.push(',');
        }
        if component == "*" {
            output.push('*');
        } else if component.contains('*') {
            // Chromium match patterns are not URLs. Retain clean patterns without widening
            // their primary/secondary scope; credential-bearing malformed patterns fail closed.
            let (scheme, _) = component.split_once("://")?;
            if !["http", "https", "file", "ftp", "*"]
                .iter()
                .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
                || component.contains(['@', '?', '#'])
                || component.chars().any(char::is_whitespace)
            {
                return None;
            }
            output.push_str(component);
        } else {
            let parsed = Url::parse(component).ok()?;
            let clean = sanitize_import_url(component)?;
            output.push_str(if !component.contains('@') && clean == parsed.as_str() {
                component
            } else {
                &clean
            });
        }
    }
    Some(output)
}

pub(crate) struct SanitizedRecords<'a, T>(pub &'a [T]);

pub(crate) trait ImportUrlRecord: serde::Serialize {
    fn import_url(&self) -> &str;
    fn sanitized_url(&self) -> Option<String> {
        sanitize_import_url(self.import_url())
    }
    fn serialize_sanitized<S: serde::Serializer>(
        &self,
        url: &str,
        serializer: S,
    ) -> Result<S::Ok, S::Error>;
}

impl<T: ImportUrlRecord> serde::Serialize for SanitizedRecords<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        struct Record<'a, T> {
            record: &'a T,
            url: String,
        }
        impl<T: ImportUrlRecord> serde::Serialize for Record<'_, T> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.record.serialize_sanitized(&self.url, serializer)
            }
        }
        let mut seq = serializer.serialize_seq(None)?;
        for record in self.0 {
            if let Some(url) = record.sanitized_url() {
                seq.serialize_element(&Record { record, url })?;
            }
        }
        seq.end()
    }
}

macro_rules! url_record {
    ($record:ty, $source:ident, $clean:ident; $($field:ident => $value:expr),+) => {
        impl ImportUrlRecord for $record {
            fn import_url(&self) -> &str { &self.url }
            fn serialize_sanitized<S: serde::Serializer>(&self, $clean: &str, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let $source = self;
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("url", $clean)?;
                $(map.serialize_entry(stringify!($field), &$value)?;)+
                map.end()
            }
        }
    };
}
url_record!(crate::Visit, source, clean;
    title => source.title.as_deref().map(|title| if title == source.url { clean } else { title }),
    visited_at_ms => source.visited_at_ms);
url_record!(crate::Bookmark, source, clean;
    title => if source.title == source.url { clean } else { &source.title },
    folder => source.folder, added_at_ms => source.added_at_ms);
url_record!(crate::RestoreTab, source, clean;
    title => if source.title == source.url { clean } else { &source.title });
url_record!(crate::SearchEngine, source, clean;
    name => source.name, keyword => source.keyword);

impl ImportUrlRecord for crate::SitePermission {
    fn import_url(&self) -> &str {
        &self.origin
    }
    fn sanitized_url(&self) -> Option<String> {
        sanitize_import_origin_pattern(&self.origin)
    }
    fn serialize_sanitized<S: serde::Serializer>(
        &self,
        origin: &str,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("origin", origin)?;
        map.serialize_entry("permission", &self.permission)?;
        map.serialize_entry("setting", &self.setting)?;
        map.end()
    }
}

impl ImportUrlRecord for crate::ExtensionMetadata {
    fn import_url(&self) -> &str {
        self.store_url.as_deref().unwrap_or_default()
    }
    fn sanitized_url(&self) -> Option<String> {
        Some(
            self.store_url
                .as_deref()
                .and_then(sanitize_import_url)
                .unwrap_or_default(),
        )
    }
    fn serialize_sanitized<S: serde::Serializer>(
        &self,
        url: &str,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("store_url", &(!url.is_empty()).then_some(url))?;
        map.end()
    }
}

fn secret_param(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "token",
        "access_token",
        "id_token",
        "refresh_token",
        "code",
        "key",
        "api_key",
        "apikey",
        "sig",
        "signature",
        "auth",
        "authorization",
        "session",
        "sessionid",
        "sid",
        "password",
        "pass",
        "pwd",
        "secret",
    ];
    NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
        || name
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("x-amz-"))
        || name
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("x-goog-"))
}

fn param_name(param: &str) -> std::borrow::Cow<'_, str> {
    url::form_urlencoded::parse(param.split('=').next().unwrap_or_default().as_bytes())
        .next()
        .map_or_else(|| "".into(), |(name, _)| name)
}

fn implicit_response(fragment: &str) -> bool {
    ["access_token=", "id_token="].iter().any(|marker| {
        fragment
            .as_bytes()
            .windows(marker.len())
            .any(|part| part.eq_ignore_ascii_case(marker.as_bytes()))
    }) || fragment.split(['&', '?']).any(|param| {
        let name = param_name(param);
        name.eq_ignore_ascii_case("access_token") || name.eq_ignore_ascii_case("id_token")
    })
}

fn without_secret_params(params: &str) -> Option<String> {
    if !params
        .split('&')
        .any(|param| secret_param(&param_name(param)))
    {
        return None;
    }
    // Retain the original encoding of benign search-template markers and anchors.
    let mut output = String::with_capacity(params.len());
    let mut first = true;
    for param in params
        .split('&')
        .filter(|param| !secret_param(&param_name(param)))
    {
        if !first {
            output.push('&');
        }
        output.push_str(param);
        first = false;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_safety_rejects_internal_invalid_and_executable_addresses() {
        for input in [
            "chrome://settings/",
            "about:blank",
            "javascript:alert(1)",
            "data:text/plain,test",
            "not a URL",
            "https://",
            "https://[invalid]/",
        ] {
            assert_eq!(sanitize_import_url(input), None, "{input}");
        }
        for input in [
            "https://example.test/",
            "http://example.test/",
            "file:///tmp/page.html",
            "ftp://example.test/page",
        ] {
            assert_eq!(sanitize_import_url(input).as_deref(), Some(input));
        }
    }

    #[test]
    fn url_safety_strips_all_userinfo_even_without_sensitive_parameters() {
        for input in [
            "https://username-canary:password-canary@example.test/page",
            "http://username-canary@example.test/page",
            "ftp://:password-canary@example.test/page",
            "https://username-canary:@example.test/page",
        ] {
            let clean = sanitize_import_url(input).unwrap();
            let parsed = Url::parse(&clean).unwrap();
            assert!(parsed.username().is_empty());
            assert!(parsed.password().is_none());
            assert!(!clean.contains("canary"));
        }
    }

    #[test]
    fn url_safety_removes_every_sensitive_name_case_insensitively_and_percent_decoded() {
        for name in [
            "token",
            "access_token",
            "id_token",
            "refresh_token",
            "code",
            "key",
            "api_key",
            "apikey",
            "sig",
            "signature",
            "auth",
            "authorization",
            "session",
            "sessionid",
            "sid",
            "password",
            "pass",
            "pwd",
            "secret",
            "X-Amz-Credential",
            "X-Amz-Signature",
            "X-Goog-Algorithm",
            "X-Goog-Signature",
            "%74oken",
            "access%5Ftoken",
        ] {
            for name in [name.to_owned(), name.to_ascii_uppercase()] {
                let input = format!("https://example.test/page?q={{searchTerms}}&{name}=query-canary&src=a%20b#{name}=fragment-canary&section=kept");
                let clean = sanitize_import_url(&input).unwrap();
                assert!(!clean.contains("canary"), "{name}: {clean}");
                assert!(clean.contains("q={searchTerms}&src=a%20b"));
                if !name.eq_ignore_ascii_case("access_token")
                    && !name.eq_ignore_ascii_case("id_token")
                    && !name.eq_ignore_ascii_case("access%5Ftoken")
                {
                    assert!(clean.ends_with("#section=kept"));
                }
                assert_eq!(sanitize_import_url(&clean).as_deref(), Some(clean.as_str()));
            }
        }
    }

    #[test]
    fn url_safety_drops_whole_implicit_response_but_preserves_benign_route_fragments() {
        for fragment in [
            "access_token=canary&state=private-state",
            "state=private-state&ID_TOKEN=canary",
            "/callback?access%5Ftoken=canary&state=private-state",
            "/callback?foo=id_token=canary",
        ] {
            assert_eq!(
                sanitize_import_url(&format!("https://example.test/#{fragment}")).as_deref(),
                Some("https://example.test/")
            );
        }
        assert_eq!(
            sanitize_import_url("https://example.test/#/route?key=canary&tab=one").as_deref(),
            Some("https://example.test/#/route?tab=one")
        );
        assert_eq!(
            sanitize_import_url("https://example.test/?code=canary#section").as_deref(),
            Some("https://example.test/#section")
        );
        assert_eq!(
            sanitize_import_url("https://example.test/#/route?password=canary").as_deref(),
            Some("https://example.test/#/route")
        );
        assert_eq!(
            sanitize_import_url("https://example.test/#token=canary").as_deref(),
            Some("https://example.test/")
        );
        assert_eq!(
            sanitize_import_url("https://example.test/#secret=canary?tab=one&section=kept")
                .as_deref(),
            Some("https://example.test/#section=kept")
        );
        assert_eq!(
            sanitize_import_url("https://example.test/?monkey=kept&token_hint=kept#chapter-1")
                .as_deref(),
            Some("https://example.test/?monkey=kept&token_hint=kept#chapter-1")
        );
    }

    #[test]
    fn url_safety_permission_patterns_keep_both_scopes_without_credentials() {
        for pattern in [
            "https://example.test:443,*",
            "*://[*.]example.test:*,https://parent.test:443",
            "file://*,*",
        ] {
            assert_eq!(
                sanitize_import_origin_pattern(pattern).as_deref(),
                Some(pattern)
            );
        }
        let clean = sanitize_import_origin_pattern("https://user-canary:password-canary@example.test/?token=query-canary,https://parent.test:443").unwrap();
        assert_eq!(clean, "https://example.test/,https://parent.test:443");
        assert_eq!(
            sanitize_import_origin_pattern("https://@example.test:443,*").as_deref(),
            Some("https://example.test/,*")
        );
        for pattern in [
            "https://user-canary@*.example.test,*",
            "https://[*.]example.test?token=query-canary,*",
            "javascript:canary,*",
        ] {
            assert!(sanitize_import_origin_pattern(pattern).is_none());
        }
    }
}

#[cfg(all(test, unix))]
mod fixture_tests {
    use super::*;
    use crate::testing::{BookmarkEntry, ChromeProfile, FirefoxProfile, HistoryEntry};
    use crate::{
        Available, BrowserKind, DataKind, DiscoveredProfile, KeyringSource, ProfileLayout,
        ProfileRead, Source,
    };
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::fs;
    use std::path::Path;

    const RAW: &str = "https://url-user-canary:url-password-canary@example.test/page?q={searchTerms}&ACCESS_TOKEN=url-query-canary&X-Amz-Signature=url-aws-canary#section=kept&secret=url-fragment-canary";
    const CLEAN: &str = "https://example.test/page?q={searchTerms}#section=kept";

    fn entries() -> (Vec<HistoryEntry>, Vec<BookmarkEntry>) {
        (
            vec![HistoryEntry {
                url: RAW.into(),
                title: Some(RAW.into()),
                visits_unix_ms: vec![1_700_000_000_000],
            }],
            vec![BookmarkEntry {
                url: RAW.into(),
                title: RAW.into(),
                folder: vec!["Fixture".into()],
            }],
        )
    }

    fn profile(dir: &Path, kind: BrowserKind, brand: &str) -> DiscoveredProfile {
        let source = Source {
            kind,
            dir: dir.to_owned(),
        };
        DiscoveredProfile {
            id: format!("{brand}:synthetic"),
            kind,
            brand: brand.into(),
            name: "Synthetic profile".into(),
            email: None,
            avatar: None,
            dir: dir.to_owned(),
            available: Available::probe(&source),
            warnings: Vec::new(),
        }
    }

    fn assert_no_canaries(value: &Value) {
        assert!(
            !serde_json::to_string(value).unwrap().contains("canary"),
            "{value}"
        );
    }

    fn saved(target: &ProfileLayout) -> Value {
        let path = fs::read_dir(target.import_dir.join("sources"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn assert_source(
        source: &DiscoveredProfile,
        kinds: &[DataKind],
        expected: &[(DataKind, &str)],
    ) {
        let target_dir = tempfile::tempdir().unwrap();
        let target = ProfileLayout::new(target_dir.path(), "default").unwrap();
        for preview in [true, false] {
            let read = crate::read_profile(source, kinds, KeyringSource::None, preview);
            for kind in kinds {
                let report = &read.reports[kind];
                assert!(report.note.is_none(), "{kind:?}: {:?}", report.note);
                assert_eq!(report.imported, 1, "{kind:?}");
            }
            let projection = json!({
                "history": &read.data.history, "bookmarks": &read.data.bookmarks,
                "tabs": &read.data.tabs, "search_engines": &read.data.search_engines,
                "permissions": &read.data.permissions, "extensions": &read.data.extensions,
                "reports": &read.reports,
            });
            assert_no_canaries(&projection);
            for (kind, url) in expected {
                assert_eq!(projection[kind.label()][0]["url"], *url);
            }
            if !preview {
                crate::write_profile(&target, source, &read).unwrap();
                let document = saved(&target);
                assert_no_canaries(&document);
                for (kind, url) in expected {
                    assert_eq!(document[kind.label()][0]["url"], *url);
                }
            }
        }
    }

    fn chrome(dir: &Path) {
        let (history, bookmarks) = entries();
        ChromeProfile {
            history,
            bookmarks,
            ..ChromeProfile::default()
        }
        .write(dir);
        let mut payload = 1_i32.to_le_bytes().to_vec();
        payload.extend_from_slice(&0_i32.to_le_bytes());
        payload.extend_from_slice(&i32::try_from(RAW.len()).unwrap().to_le_bytes());
        payload.extend_from_slice(RAW.as_bytes());
        payload.resize((payload.len() + 3) & !3, 0);
        let title: Vec<_> = RAW.encode_utf16().collect();
        payload.extend_from_slice(&i32::try_from(title.len()).unwrap().to_le_bytes());
        for word in title {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        payload.resize((payload.len() + 3) & !3, 0);
        let mut pickle = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        pickle.extend_from_slice(&payload);
        let mut session = b"SNSS".to_vec();
        session.extend_from_slice(&1_i32.to_le_bytes());
        session.extend_from_slice(&u16::try_from(pickle.len() + 1).unwrap().to_le_bytes());
        session.push(6);
        session.extend_from_slice(&pickle);
        fs::write(dir.join("Last Session"), session).unwrap();
        let db = Connection::open(dir.join("Web Data")).unwrap();
        db.execute_batch("CREATE TABLE keywords (short_name TEXT, keyword TEXT, url TEXT)")
            .unwrap();
        db.execute(
            "INSERT INTO keywords VALUES ('Fixture', 'fixture', ?1)",
            [RAW],
        )
        .unwrap();
        fs::write(
            dir.join("Preferences"),
            serde_json::to_vec(&json!({"profile":{"content_settings":{"exceptions":{
                "notifications": {format!("{RAW},*"): {"setting":1}}
            }}}}))
            .unwrap(),
        )
        .unwrap();
    }

    fn compressed(value: &Value) -> Vec<u8> {
        let mut bytes = b"mozLz40\0".to_vec();
        bytes.extend(lz4_flex::block::compress_prepend_size(
            &serde_json::to_vec(value).unwrap(),
        ));
        bytes
    }

    fn firefox(dir: &Path) {
        let (history, bookmarks) = entries();
        FirefoxProfile {
            history,
            bookmarks,
            ..FirefoxProfile::default()
        }
        .write(dir);
        fs::write(
            dir.join("sessionstore.jsonlz4"),
            compressed(&json!({"windows":[{"tabs":[
                {"entries":[{"url":RAW,"title":RAW}]}
            ]}]})),
        )
        .unwrap();
        fs::write(dir.join("search.json.mozlz4"), compressed(&json!({"engines":[{
            "_name":"Fixture","_metaData":{"alias":"fixture"},"_urls":[{
                "type":"text/html","template":"https://url-user-canary:url-password-canary@example.test/search?token=url-query-canary",
                "params":[{"name":"q","value":"{searchTerms}"},{"name":"CODE","value":"url-code-canary"},{"name":"X-Goog-Signature","value":"url-google-canary"}]
            }]
        }]}))).unwrap();
        let db = Connection::open(dir.join("permissions.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE moz_perms(origin TEXT, type TEXT, permission INTEGER)")
            .unwrap();
        db.execute("INSERT INTO moz_perms VALUES (?1,'geo',1)", [RAW])
            .unwrap();
        fs::write(dir.join("extensions.json"), serde_json::to_vec(&json!({"addons":[{
            "id":"fixture@example.test","type":"extension","defaultLocale":{"name":"Fixture"},
            "amoListingURL":"https://addons.mozilla.org/firefox/addon/fixture?token=url-extension-canary#secret=url-extension-fragment-canary"
        }]})).unwrap()).unwrap();
    }

    fn safari(dir: &Path, binary: bool) {
        let db = Connection::open(dir.join("History.db")).unwrap();
        db.execute_batch("CREATE TABLE history_items(id INTEGER PRIMARY KEY,url TEXT); CREATE TABLE history_visits(history_item INTEGER,title TEXT,visit_time REAL)").unwrap();
        db.execute("INSERT INTO history_items VALUES (1,?1)", [RAW])
            .unwrap();
        db.execute("INSERT INTO history_visits VALUES (1,?1,1000.0)", [RAW])
            .unwrap();
        let bookmarks = plist::Value::Dictionary(plist::Dictionary::from_iter([
            ("WebBookmarkType", plist::Value::from("WebBookmarkTypeList")),
            (
                "Children",
                plist::Value::Array(vec![plist::Value::Dictionary(
                    plist::Dictionary::from_iter([
                        ("WebBookmarkType", plist::Value::from("WebBookmarkTypeLeaf")),
                        ("URLString", plist::Value::from(RAW)),
                        ("Title", plist::Value::from(RAW)),
                    ]),
                )]),
            ),
        ]));
        let session = plist::Value::Dictionary(plist::Dictionary::from_iter([(
            "SessionWindows",
            plist::Value::Array(vec![plist::Value::Dictionary(
                plist::Dictionary::from_iter([(
                    "TabStates",
                    plist::Value::Array(vec![plist::Value::Dictionary(
                        plist::Dictionary::from_iter([
                            ("TabURL", plist::Value::from(RAW)),
                            ("TabTitle", plist::Value::from(RAW)),
                        ]),
                    )]),
                )]),
            )]),
        )]));
        if binary {
            bookmarks
                .to_file_binary(dir.join("Bookmarks.plist"))
                .unwrap();
            session
                .to_file_binary(dir.join("LastSession.plist"))
                .unwrap();
        } else {
            bookmarks.to_file_xml(dir.join("Bookmarks.plist")).unwrap();
            session.to_file_xml(dir.join("LastSession.plist")).unwrap();
        }
    }

    #[test]
    fn url_safety_chromium_reader_and_source_store_canaries() {
        let dir = tempfile::tempdir().unwrap();
        chrome(dir.path());
        assert_source(
            &profile(dir.path(), BrowserKind::Chromium, "chromium"),
            &[
                DataKind::History,
                DataKind::Bookmarks,
                DataKind::Tabs,
                DataKind::SearchEngines,
                DataKind::Permissions,
            ],
            &[
                (DataKind::History, CLEAN),
                (DataKind::Bookmarks, CLEAN),
                (DataKind::Tabs, CLEAN),
                (DataKind::SearchEngines, CLEAN),
            ],
        );
    }

    #[test]
    fn url_safety_firefox_reader_and_source_store_canaries() {
        let dir = tempfile::tempdir().unwrap();
        firefox(dir.path());
        assert_source(
            &profile(dir.path(), BrowserKind::Firefox, "firefox"),
            &[
                DataKind::History,
                DataKind::Bookmarks,
                DataKind::Tabs,
                DataKind::SearchEngines,
                DataKind::Permissions,
                DataKind::Extensions,
            ],
            &[
                (DataKind::History, CLEAN),
                (DataKind::Bookmarks, CLEAN),
                (DataKind::Tabs, CLEAN),
                (
                    DataKind::SearchEngines,
                    "https://example.test/search?q={searchTerms}",
                ),
            ],
        );
    }

    #[test]
    fn url_safety_safari_reader_and_source_store_canaries() {
        for binary in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            safari(dir.path(), binary);
            assert_source(
                &profile(dir.path(), BrowserKind::Safari, "safari"),
                &[DataKind::History, DataKind::Bookmarks, DataKind::Tabs],
                &[
                    (DataKind::History, CLEAN),
                    (DataKind::Bookmarks, CLEAN),
                    (DataKind::Tabs, CLEAN),
                ],
            );
        }
    }

    #[test]
    fn url_safety_arc_fallback_titles_and_source_store_canaries() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("StorableSidebar.json"),
            serde_json::to_vec(&json!({
                "sidebar":{"containers":[{"items":[{"id":"tab","data":{"tab":{"savedURL":RAW}}}]}]}
            }))
            .unwrap(),
        )
        .unwrap();
        assert_source(
            &profile(dir.path(), BrowserKind::Chromium, "arc"),
            &[DataKind::Bookmarks, DataKind::Tabs],
            &[(DataKind::Bookmarks, CLEAN), (DataKind::Tabs, CLEAN)],
        );
        let read = crate::read_profile(
            &profile(dir.path(), BrowserKind::Chromium, "arc"),
            &[DataKind::Bookmarks, DataKind::Tabs],
            KeyringSource::None,
            true,
        );
        assert_eq!(read.data.bookmarks[0].title, CLEAN);
        assert_eq!(read.data.tabs[0].title, CLEAN);
    }

    #[test]
    fn url_safety_legacy_document_projection_and_partial_reimport_canaries() {
        let source_dir = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let target = ProfileLayout::new(target_dir.path(), "default").unwrap();
        let source = profile(source_dir.path(), BrowserKind::Chrome, "chrome");
        crate::write_profile(&target, &source, &ProfileRead::default()).unwrap();
        let path = fs::read_dir(target.import_dir.join("sources"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut document = json!({
            "id":source.id,"history":[{"url":RAW,"title":RAW},{"url":"javascript:legacy-canary"}],
            "bookmarks":[{"url":RAW,"title":RAW}],"tabs":[{"url":RAW,"title":RAW}],
            "search_engines":[{"url":RAW,"name":"Fixture","keyword":"fixture"}],
            "permissions":[{"origin":"*://[*.]example.test:*,https://parent.test:443","permission":"geo","setting":"allow"},
                {"origin":format!("{RAW},*"),"permission":"notifications","setting":"ask"}],
            "extensions":[{"id":"fixture","name":"Fixture","store_url":RAW},{"id":"local","name":"Local","store_url":"javascript:legacy-canary"}]
        });
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        sanitize_import_document_urls(&mut document);
        assert_no_canaries(&document);
        assert_eq!(document["history"].as_array().unwrap().len(), 1);
        assert_eq!(document["history"][0]["title"], CLEAN);
        assert_eq!(
            document["permissions"][0]["origin"],
            "*://[*.]example.test:*,https://parent.test:443"
        );
        assert!(document["extensions"][1]["store_url"].is_null());
        crate::write_profile(&target, &source, &ProfileRead::default()).unwrap();
        assert_no_canaries(&saved(&target));
        assert_eq!(saved(&target)["history"][0]["url"], CLEAN);
        let first = document.clone();
        sanitize_import_document_urls(&mut document);
        assert_eq!(document, first);
    }

    #[test]
    fn url_safety_persistence_defends_against_unsanitized_caller_records() {
        let source_dir = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let source = profile(source_dir.path(), BrowserKind::Chrome, "chrome");
        let target = ProfileLayout::new(target_dir.path(), "default").unwrap();
        let mut read = ProfileRead::default();
        for kind in [
            DataKind::History,
            DataKind::Bookmarks,
            DataKind::Tabs,
            DataKind::SearchEngines,
            DataKind::Permissions,
            DataKind::Extensions,
        ] {
            read.reports.insert(kind, crate::CategoryReport::default());
        }
        read.data.history.push(crate::Visit {
            url: RAW.into(),
            title: Some(RAW.into()),
            visited_at_ms: 1,
        });
        read.data.bookmarks.push(crate::Bookmark {
            url: RAW.into(),
            title: RAW.into(),
            folder: vec!["Fixture".into()],
            added_at_ms: None,
        });
        read.data.tabs.push(crate::RestoreTab {
            url: RAW.into(),
            title: RAW.into(),
        });
        read.data.search_engines.push(crate::SearchEngine {
            url: RAW.into(),
            name: "Fixture".into(),
            keyword: "fixture".into(),
        });
        read.data.permissions.push(crate::SitePermission {
            origin: format!("{RAW},*"),
            permission: "geo".into(),
            setting: "allow".into(),
        });
        read.data.extensions.push(crate::ExtensionMetadata {
            id: "fixture".into(),
            name: "Fixture".into(),
            store_url: Some(RAW.into()),
        });
        crate::write_profile(&target, &source, &read).unwrap();
        let document = saved(&target);
        assert_no_canaries(&document);
        assert_eq!(document["tabs"][0]["title"], CLEAN);
    }

    #[tokio::test]
    async fn url_safety_standalone_chromium_and_firefox_store_canaries() {
        for kind in [BrowserKind::Chromium, BrowserKind::Firefox] {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("source");
            let (history, bookmarks) = entries();
            match kind {
                BrowserKind::Chromium => ChromeProfile {
                    history,
                    bookmarks,
                    ..ChromeProfile::default()
                }
                .write(&source),
                BrowserKind::Firefox => FirefoxProfile {
                    history,
                    bookmarks,
                    ..FirefoxProfile::default()
                }
                .write(&source),
                _ => unreachable!(),
            }
            let target = ProfileLayout::new(root.path(), "default").unwrap();
            let report = crate::import(crate::ImportOptions {
                source: Source { kind, dir: source },
                target: target.clone(),
                what: crate::What {
                    history: true,
                    bookmarks: true,
                    cookies: false,
                },
                skip_missing: false,
                dry_run: false,
                keyring: KeyringSource::None,
                browser: None,
            })
            .await
            .unwrap();
            assert_no_canaries(&serde_json::to_value(report).unwrap());
            for record in crate::testing::read_jsonl(&target.history_path(kind)) {
                assert_no_canaries(&record);
            }
            let bookmarks: Value =
                serde_json::from_slice(&fs::read(target.bookmarks_path(kind)).unwrap()).unwrap();
            assert_no_canaries(&bookmarks);
        }
    }
}
