//! Finding browser profiles in their standard locations, so a caller can offer them by name
//! instead of taking a directory path from the network.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{items, BrowserKind, Source, What};

/// Largest `Local State` or `profiles.ini` read; both are a few kilobytes in practice.
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

/// A browser profile found on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProfile {
    /// `<browser>:<directory name>`; stable across calls, carries no path.
    pub id: String,
    /// Browser family.
    pub kind: BrowserKind,
    /// Name the browser shows for the profile (its directory name when none is recorded).
    pub name: String,
    /// The profile directory.
    pub dir: PathBuf,
    /// Categories whose source file exists.
    pub available: What,
}

impl DiscoveredProfile {
    /// The import source for this profile.
    pub fn source(&self) -> Source {
        Source {
            kind: self.kind,
            dir: self.dir.clone(),
        }
    }
}

/// Profiles under the standard locations of this OS, resolved from the process environment.
pub fn discover_system() -> Vec<DiscoveredProfile> {
    discover(&|key| std::env::var_os(key))
}

/// Profiles under the standard locations of this OS: Chrome, then Chromium, then Firefox,
/// each in the order the browser lists them. Directories that are symbolic links are skipped
/// like the importer skips linked source files.
pub fn discover(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<DiscoveredProfile> {
    let Some(home) = env("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    let (chrome, chromium, firefox) = roots(env, &home);
    let mut found = Vec::new();
    for root in chrome {
        chromium_profiles(BrowserKind::Chrome, &root, &mut found);
    }
    for root in chromium {
        chromium_profiles(BrowserKind::Chromium, &root, &mut found);
    }
    for root in firefox {
        firefox_profiles(&root, &mut found);
    }
    found
}

type Roots = (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>);

fn roots(env: &dyn Fn(&str) -> Option<OsString>, home: &Path) -> Roots {
    if cfg!(target_os = "macos") {
        let support = home.join("Library/Application Support");
        return (
            vec![support.join("Google/Chrome")],
            vec![support.join("Chromium")],
            vec![support.join("Firefox")],
        );
    }
    if cfg!(unix) {
        let config = env("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".config"), PathBuf::from);
        return (
            vec![config.join("google-chrome")],
            vec![
                config.join("chromium"),
                home.join("snap/chromium/common/chromium"),
            ],
            vec![
                home.join(".mozilla/firefox"),
                home.join("snap/firefox/common/.mozilla/firefox"),
                home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
            ],
        );
    }
    // Windows cookies cannot be decrypted (docs/specs/import.md), so nothing is offered.
    (Vec::new(), Vec::new(), Vec::new())
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn read_index(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_INDEX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_INDEX_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn push(found: &mut Vec<DiscoveredProfile>, kind: BrowserKind, name: String, dir: PathBuf) {
    let Some(directory) = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let id = format!("{}:{directory}", kind.label());
    if found.iter().any(|profile| profile.id == id) {
        return;
    }
    let items = items(&Source {
        kind,
        dir: dir.clone(),
    });
    let has = |item: crate::db::Item| matches!(item.probe(), Ok(true));
    found.push(DiscoveredProfile {
        id,
        kind,
        name,
        dir,
        available: What {
            history: has(items.history),
            bookmarks: has(items.bookmarks),
            cookies: has(items.cookies),
        },
    });
}

/// `Default`, then `Profile 1`, `Profile 2`, … Chrome keeps the display names in
/// `Local State` under `profile.info_cache`.
fn chromium_profiles(kind: BrowserKind, root: &Path, found: &mut Vec<DiscoveredProfile>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut directories: Vec<(u32, String)> = entries
        .filter_map(Result::ok)
        .filter(|entry| is_real_dir(&entry.path()))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rank = if name == "Default" {
                0
            } else {
                name.strip_prefix("Profile ")?
                    .parse::<u32>()
                    .ok()?
                    .checked_add(1)?
            };
            Some((rank, name))
        })
        .collect();
    directories.sort();
    let state: Option<Value> =
        read_index(&root.join("Local State")).and_then(|bytes| serde_json::from_slice(&bytes).ok());
    for (_, directory) in directories {
        let name = state
            .as_ref()
            .and_then(|state| state["profile"]["info_cache"][&directory]["name"].as_str())
            .filter(|name| !name.is_empty())
            .map_or_else(|| directory.clone(), str::to_owned);
        push(found, kind, name, root.join(&directory));
    }
}

/// Profiles listed in `profiles.ini`: `[ProfileN]` sections with `Name`, `Path` and
/// `IsRelative`.
fn firefox_profiles(root: &Path, found: &mut Vec<DiscoveredProfile>) {
    let Some(text) = read_index(&root.join("profiles.ini")) else {
        return;
    };
    let text = String::from_utf8_lossy(&text);
    let mut in_profile = false;
    let (mut name, mut path, mut relative) = (None::<String>, None::<String>, true);
    let mut flush = |name: &mut Option<String>, path: &mut Option<String>, relative: bool| {
        let (name, path) = (name.take(), path.take());
        let Some(path) = path else {
            return;
        };
        let dir = if relative {
            root.join(&path)
        } else {
            PathBuf::from(&path)
        };
        if is_real_dir(&dir) {
            push(
                found,
                BrowserKind::Firefox,
                name.unwrap_or_else(|| path.clone()),
                dir,
            );
        }
    };
    for line in text.lines().map(str::trim) {
        if let Some(section) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if in_profile {
                flush(&mut name, &mut path, relative);
            }
            in_profile = section.starts_with("Profile");
            relative = true;
            continue;
        }
        if !in_profile {
            continue;
        }
        match line.split_once('=') {
            Some(("Name", value)) => name = Some(value.to_owned()),
            Some(("Path", value)) => path = Some(value.to_owned()),
            Some(("IsRelative", value)) => relative = value != "0",
            _ => {}
        }
    }
    if in_profile {
        flush(&mut name, &mut path, relative);
    }
}
