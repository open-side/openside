//! Arc stores every Space in one sidebar, so discovery exposes it separately from profiles.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{db::Item, sanitize_import_url, Bookmark, ImportError, RestoreTab, Result, Source};

const MAX_SIDEBAR_BYTES: u64 = 32 * 1024 * 1024;

fn sidebar(source: &Source, what: &'static str) -> Result<Value> {
    let item = Item {
        browser: source.kind,
        what,
        path: source.dir.join("StorableSidebar.json"),
    };
    let bytes = item.read_bounded(MAX_SIDEBAR_BYTES)?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|_| ImportError::Corrupt {
        what,
        path: item.path.clone(),
        format: "Arc sidebar JSON",
        detail: "invalid sidebar document".into(),
    })?;
    if !document["sidebar"]["containers"].is_array()
        && !document["sidebar"]["containers"].is_object()
    {
        return Err(item.unsupported("Arc sidebar has no supported containers"));
    }
    Ok(document)
}

fn items(document: &Value) -> Vec<&Value> {
    let mut pending = vec![&document["sidebar"]["containers"]];
    if let Some(index) = document.get("items") {
        pending.push(index);
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    while let Some(value) = pending.pop() {
        if let Some(id) = value["id"].as_str().filter(|_| value["data"].is_object()) {
            if seen.insert(id) {
                result.push(value);
            }
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().rev()),
            Value::Object(values) => pending.extend(values.values().rev()),
            _ => {}
        }
    }
    result
}

fn folder(item: &Value, index: &BTreeMap<&str, &Value>) -> Vec<String> {
    let mut names = Vec::new();
    let mut visited = BTreeSet::new();
    let mut parent = item["parentID"].as_str();
    while let Some(id) = parent {
        if !visited.insert(id) {
            break;
        }
        let Some(value) = index.get(id) else {
            break;
        };
        let data = &value["data"];
        let fallback = if data.get("list").is_some() {
            Some("Folder")
        } else if data.get("tabGroup").is_some() {
            Some("Tab group")
        } else if data.get("itemContainer").is_some() {
            Some("Favorites and tabs")
        } else {
            None
        };
        if let Some(fallback) = fallback {
            let title = value["title"]
                .as_str()
                .filter(|title| !title.is_empty())
                .or_else(|| {
                    data["tabGroup"]["title"]
                        .as_str()
                        .filter(|title| !title.is_empty())
                })
                .unwrap_or(fallback);
            names.push(title.to_owned());
        }
        parent = value["parentID"].as_str();
    }
    names.reverse();
    if names.is_empty() {
        names.push("Arc sidebar".into());
    }
    names
}

pub(crate) fn read_bookmarks(source: &Source) -> Result<(Vec<Bookmark>, u64)> {
    let document = sidebar(source, "bookmarks")?;
    let nodes = items(&document);
    let index = nodes
        .iter()
        .filter_map(|value| value["id"].as_str().map(|id| (id, *value)))
        .collect();
    let mut bookmarks = Vec::new();
    let mut skipped = 0;
    for item in nodes {
        let Some(tab) = item["data"].get("tab") else {
            continue;
        };
        let Some(address) = tab["savedURL"].as_str() else {
            skipped += 1;
            continue;
        };
        let Some(url) = sanitize_import_url(address) else {
            skipped += 1;
            continue;
        };
        let title = tab["savedTitle"]
            .as_str()
            .filter(|title| *title != address)
            .unwrap_or(&url)
            .to_owned();
        bookmarks.push(Bookmark {
            url,
            title,
            folder: folder(item, &index),
            added_at_ms: None,
        });
    }
    Ok((bookmarks, skipped))
}

pub(crate) fn read_tabs(source: &Source) -> Result<Vec<RestoreTab>> {
    let document = sidebar(source, "tabs")?;
    let mut tabs = Vec::new();
    let mut seen = BTreeSet::new();
    for item in items(&document) {
        let tab = &item["data"]["tab"];
        let Some(address) = tab["savedURL"].as_str() else {
            continue;
        };
        let Some(url) = sanitize_import_url(address).filter(|url| crate::model::is_web_url(url))
        else {
            continue;
        };
        if !seen.insert(address) {
            continue;
        }
        let title = tab["savedTitle"]
            .as_str()
            .filter(|title| *title != address)
            .unwrap_or(&url)
            .to_owned();
        tabs.push(RestoreTab { url, title });
    }
    Ok(tabs)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::BrowserKind;

    fn source(dir: &std::path::Path) -> Source {
        Source {
            kind: BrowserKind::Chromium,
            dir: dir.to_owned(),
        }
    }

    fn write_fixture(dir: &std::path::Path) {
        std::fs::write(dir.join("StorableSidebar.json"), serde_json::to_vec(&serde_json::json!({
            "sidebar": {"containers": ["container-key", {"items": [
                "root", {"id":"root", "title":"Work", "data":{"itemContainer":{}}},
                "outer", {"id":"outer", "parentID":"root", "title":"Projects", "data":{"list":{}}},
                "group", {"id":"group", "parentID":"outer", "data":{"tabGroup":{"title":"Research"}}},
                "tab1", {"id":"tab1", "parentID":"group", "data":{"tab":{"savedURL":"https://arc.example.test/", "savedTitle":"Research page"}}},
                "tab2", {"id":"tab2", "parentID":"root", "data":{"tab":{"savedURL":"file:///tmp/arc-fixture.pdf", "savedTitle":"Local PDF"}}},
                "tab3", {"id":"tab3", "parentID":"root", "data":{"tab":{"savedURL":"ftp://arc.example.test/", "savedTitle":"Files"}}},
                "internal", {"id":"internal", "parentID":"root", "data":{"tab":{"savedURL":"arc://newtab/"}}},
                "bad", {"id":"bad", "data":{"tab":{"savedTitle":"Missing URL"}}},
                {"id":"tab1", "data":{"tab":{"savedURL":"https://duplicate.example.test/"}}}
            ]}]}
        })).unwrap()).unwrap();
    }

    #[test]
    fn shared_sidebar_retains_nested_folders_supported_bookmarks_and_restore_list() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());
        let before = std::fs::read(dir.path().join("StorableSidebar.json")).unwrap();
        let (bookmarks, skipped) = read_bookmarks(&source(dir.path())).unwrap();
        assert_eq!(bookmarks.len(), 3);
        assert_eq!(skipped, 2);
        assert_eq!(bookmarks[0].folder, ["Work", "Projects", "Research"]);
        assert_eq!(bookmarks[0].title, "Research page");
        let tabs = read_tabs(&source(dir.path())).unwrap();
        assert_eq!(
            tabs,
            [RestoreTab {
                url: "https://arc.example.test/".into(),
                title: "Research page".into()
            }]
        );
        assert_eq!(
            std::fs::read(dir.path().join("StorableSidebar.json")).unwrap(),
            before
        );
    }

    #[test]
    fn references_missing_parents_and_cycles_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("StorableSidebar.json"), r#"{"sidebar":{"containers":[{"items":["a","b","tab"]}]},"items":{"a":{"id":"a","parentID":"b","title":"A","data":{"list":{}}},"b":{"id":"b","parentID":"a","title":"B","data":{"list":{}}},"tab":{"id":"tab","parentID":"a","data":{"tab":{"savedURL":"https://arc.example.test/"}}}}}"#).unwrap();
        let (bookmarks, skipped) = read_bookmarks(&source(dir.path())).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].folder, ["B", "A"]);
        assert_eq!(skipped, 0);
        let missing = serde_json::json!({"id":"tab", "parentID":"absent"});
        assert_eq!(folder(&missing, &BTreeMap::new()), ["Arc sidebar"]);
    }

    #[test]
    fn malformed_oversized_and_linked_sidebar_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let item = source(dir.path());
        std::fs::write(dir.path().join("StorableSidebar.json"), b"{not-json").unwrap();
        assert!(matches!(
            read_bookmarks(&item),
            Err(ImportError::Corrupt { .. })
        ));
        std::fs::write(dir.path().join("StorableSidebar.json"), b"{}").unwrap();
        assert!(matches!(
            read_tabs(&item),
            Err(ImportError::UnsupportedVersion { .. })
        ));
        std::fs::File::create(dir.path().join("StorableSidebar.json"))
            .unwrap()
            .set_len(MAX_SIDEBAR_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_bookmarks(&item),
            Err(ImportError::UnsupportedVersion { .. })
        ));
        std::fs::remove_file(dir.path().join("StorableSidebar.json")).unwrap();
        let other = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(other.path(), dir.path().join("StorableSidebar.json")).unwrap();
        assert!(matches!(
            read_tabs(&item),
            Err(ImportError::Unreadable { .. })
        ));
    }
}
