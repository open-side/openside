//! Standard browser locations are kept together so profile discovery never accepts network paths.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Available, BrowserKind, Source};

const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

/// A locally discovered profile; directory paths remain host-side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProfile {
    /// Collision-safe opaque selector, stable across discovery calls.
    pub id: String,
    /// Parser family independent of the browser's display brand.
    pub kind: BrowserKind,
    /// Stable brand identifier for Safe Storage lookup and UI icons.
    pub brand: String,
    /// Browser-provided profile display name, with directory-name fallback.
    pub name: String,
    /// Source account label from Local State when present.
    pub email: Option<String>,
    /// Browser-local avatar identifier, never a remote image URL.
    pub avatar: Option<String>,
    /// Read-only source directory, never accepted from a network request.
    pub dir: PathBuf,
    /// Presence of known category source files, not a successful decrypt guarantee.
    pub available: Available,
    /// Safe permission hints suitable for the GUI.
    pub warnings: Vec<String>,
}

impl DiscoveredProfile {
    /// Retain the parser family and read-only directory for host-side import.
    pub fn source(&self) -> Source {
        Source {
            kind: self.kind,
            dir: self.dir.clone(),
        }
    }
}

#[derive(Clone, Copy)]
struct BrowserLocation {
    brand: &'static str,
    kind: BrowserKind,
    linux: &'static [&'static str],
    mac: &'static [&'static str],
    windows: &'static [&'static str],
}

const LOCATIONS: &[BrowserLocation] = &[
    BrowserLocation {
        brand: "chrome",
        kind: BrowserKind::Chrome,
        linux: &[
            "config/google-chrome",
            "config/google-chrome-beta",
            "config/google-chrome-unstable",
            "home/.var/app/com.google.Chrome/config/google-chrome",
        ],
        mac: &[
            "home/Library/Application Support/Google/Chrome",
            "home/Library/Application Support/Google/Chrome Beta",
            "home/Library/Application Support/Google/Chrome Canary",
        ],
        windows: &[
            "local/Google/Chrome/User Data",
            "local/Google/Chrome Beta/User Data",
            "local/Google/Chrome SxS/User Data",
        ],
    },
    BrowserLocation {
        brand: "chromium",
        kind: BrowserKind::Chromium,
        linux: &[
            "config/chromium",
            "home/snap/chromium/common/chromium",
            "home/.var/app/org.chromium.Chromium/config/chromium",
        ],
        mac: &["home/Library/Application Support/Chromium"],
        windows: &["local/Chromium/User Data"],
    },
    BrowserLocation {
        brand: "brave",
        kind: BrowserKind::Chromium,
        linux: &[
            "config/BraveSoftware/Brave-Browser",
            "home/.var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
        ],
        mac: &["home/Library/Application Support/BraveSoftware/Brave-Browser"],
        windows: &["local/BraveSoftware/Brave-Browser/User Data"],
    },
    BrowserLocation {
        brand: "edge",
        kind: BrowserKind::Chromium,
        linux: &[
            "config/microsoft-edge",
            "config/microsoft-edge-beta",
            "config/microsoft-edge-dev",
        ],
        mac: &["home/Library/Application Support/Microsoft Edge"],
        windows: &["local/Microsoft/Edge/User Data"],
    },
    BrowserLocation {
        brand: "vivaldi",
        kind: BrowserKind::Chromium,
        linux: &["config/vivaldi", "config/vivaldi-snapshot"],
        mac: &["home/Library/Application Support/Vivaldi"],
        windows: &["local/Vivaldi/User Data"],
    },
    BrowserLocation {
        brand: "opera",
        kind: BrowserKind::Chromium,
        linux: &[
            "config/opera",
            "config/opera-developer",
            "config/opera-beta",
        ],
        mac: &[
            "home/Library/Application Support/com.operasoftware.Opera",
            "home/Library/Application Support/com.operasoftware.OperaGX",
        ],
        windows: &[
            "roaming/Opera Software/Opera Stable",
            "roaming/Opera Software/Opera GX Stable",
        ],
    },
    BrowserLocation {
        brand: "arc",
        kind: BrowserKind::Chromium,
        linux: &["config/Arc"],
        mac: &["home/Library/Application Support/Arc/User Data"],
        windows: &[
            "local/Packages/TheBrowserCompany.Arc_ttt1ap7aakyb4/LocalCache/Local/Arc/User Data",
            "local/Arc/User Data",
        ],
    },
    BrowserLocation {
        brand: "aside",
        kind: BrowserKind::Chromium,
        linux: &["config/aside", "config/Aside", "config/aside/User Data"],
        mac: &[
            "home/Library/Application Support/Aside",
            "home/Library/Application Support/Aside/User Data",
        ],
        windows: &["local/Aside/User Data"],
    },
    BrowserLocation {
        brand: "firefox",
        kind: BrowserKind::Firefox,
        linux: &[
            "home/.mozilla/firefox",
            "home/snap/firefox/common/.mozilla/firefox",
            "home/.var/app/org.mozilla.firefox/.mozilla/firefox",
        ],
        mac: &["home/Library/Application Support/Firefox"],
        windows: &["roaming/Mozilla/Firefox"],
    },
    BrowserLocation {
        brand: "librewolf",
        kind: BrowserKind::Firefox,
        linux: &[
            "home/.librewolf",
            "home/.var/app/io.gitlab.librewolf-community/.librewolf",
        ],
        mac: &["home/Library/Application Support/LibreWolf"],
        windows: &["roaming/librewolf"],
    },
    BrowserLocation {
        brand: "zen",
        kind: BrowserKind::Firefox,
        linux: &["home/.zen", "home/.var/app/app.zen_browser.zen/.zen"],
        mac: &["home/Library/Application Support/zen"],
        windows: &["roaming/zen"],
    },
    BrowserLocation {
        brand: "floorp",
        kind: BrowserKind::Firefox,
        linux: &["home/.floorp", "home/.var/app/one.ablaze.floorp/.floorp"],
        mac: &["home/Library/Application Support/Floorp"],
        windows: &["roaming/Floorp"],
    },
    BrowserLocation {
        brand: "safari",
        kind: BrowserKind::Safari,
        linux: &[],
        mac: &[
            "home/Library/Safari",
            "home/Library/Containers/com.apple.Safari/Data/Library/Safari",
        ],
        windows: &[],
    },
];

/// Discover using the daemon process environment and platform-standard location table.
pub fn discover_system() -> Vec<DiscoveredProfile> {
    discover(&|key| std::env::var_os(key))
}

/// Discover with supplied environment values so fixtures never touch real user profiles.
pub fn discover(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<DiscoveredProfile> {
    discover_platform(
        env,
        if cfg!(target_os = "macos") {
            "mac"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        },
    )
}

fn discover_platform(
    env: &dyn Fn(&str) -> Option<OsString>,
    platform: &str,
) -> Vec<DiscoveredProfile> {
    let Some(home) = env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    let config = env("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(".config"), PathBuf::from);
    let local = env("LOCALAPPDATA").map_or_else(|| home.join("AppData/Local"), PathBuf::from);
    let roaming = env("APPDATA").map_or_else(|| home.join("AppData/Roaming"), PathBuf::from);
    let mut found = Vec::new();
    for location in LOCATIONS {
        let paths = match platform {
            "mac" => location.mac,
            "windows" => location.windows,
            _ => location.linux,
        };
        for path in paths {
            let Some((base, suffix)) = path.split_once('/') else {
                continue;
            };
            let root = match base {
                "config" => &config,
                "local" => &local,
                "roaming" => &roaming,
                _ => &home,
            }
            .join(suffix);
            match location.kind {
                BrowserKind::Firefox => firefox_profiles(location, &root, &mut found),
                BrowserKind::Safari => safari_profiles(location, &root, &mut found),
                _ => chromium_profiles(location, &root, &mut found),
            }
            if location.brand == "arc" {
                let sidebar_root = if root.file_name().is_some_and(|name| name == "User Data") {
                    root.parent().unwrap_or(&root)
                } else {
                    &root
                };
                let sidebar = crate::db::Item {
                    browser: location.kind,
                    what: "Arc sidebar",
                    path: sidebar_root.join("StorableSidebar.json"),
                };
                if is_real_dir(sidebar_root) && sidebar.probe().unwrap_or(false) {
                    push(&mut found, location, "All Spaces (shared sidebar)".to_owned(), sidebar_root.to_owned(), None, None,
                        vec!["Arc keeps Favorites and Spaces in one shared sidebar. This source includes links from every Space.".to_owned()]);
                }
            }
        }
    }
    found
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn read_index(path: &Path) -> Option<Vec<u8>> {
    crate::db::Item {
        browser: BrowserKind::Chromium,
        what: "profile index",
        path: path.to_owned(),
    }
    .read_bounded(MAX_INDEX_BYTES)
    .ok()
}

fn push(
    found: &mut Vec<DiscoveredProfile>,
    location: &BrowserLocation,
    name: String,
    dir: PathBuf,
    email: Option<String>,
    avatar: Option<String>,
    warnings: Vec<String>,
) {
    if found.iter().any(|profile| profile.dir == dir) {
        return;
    }
    let Some(directory) = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let digest = Sha256::digest(dir.as_os_str().as_encoded_bytes());
    let mut id = format!("{}:{directory}:", location.brand);
    for byte in &digest[..8] {
        id.push_str(&format!("{byte:02x}"));
    }
    let source = Source {
        kind: location.kind,
        dir: dir.clone(),
    };
    found.push(DiscoveredProfile {
        id,
        kind: location.kind,
        brand: location.brand.to_owned(),
        name,
        email,
        avatar,
        available: Available::probe(&source),
        dir,
        warnings,
    });
}

fn chromium_profiles(location: &BrowserLocation, root: &Path, found: &mut Vec<DiscoveredProfile>) {
    if !is_real_dir(root) {
        return;
    }
    let state: Option<Value> =
        read_index(&root.join("Local State")).and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let cache = state
        .as_ref()
        .and_then(|state| state["profile"]["info_cache"].as_object());
    let mut directories = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.filter_map(std::result::Result::ok) {
            let directory = entry.file_name().to_string_lossy().into_owned();
            if is_real_dir(&entry.path())
                && (directory == "Default"
                    || directory.starts_with("Profile ")
                    || cache.is_some_and(|cache| cache.contains_key(&directory))
                    || ["History", "Bookmarks", "Preferences", "Network/Cookies"]
                        .iter()
                        .any(|file| entry.path().join(file).is_file()))
            {
                directories.push(directory);
            }
        }
    }
    if let Some(cache) = cache {
        for directory in cache.keys().filter(|directory| safe_relative(directory)) {
            if is_real_dir(&root.join(directory)) && !directories.contains(directory) {
                directories.push(directory.clone());
            }
        }
    }
    directories.sort_by_key(|directory| {
        (
            directory != "Default",
            directory
                .strip_prefix("Profile ")
                .and_then(|number| number.parse::<u32>().ok())
                .unwrap_or(u32::MAX),
            directory.clone(),
        )
    });
    if [
        "History",
        "Bookmarks",
        "Preferences",
        "Cookies",
        "Network/Cookies",
    ]
    .iter()
    .any(|file| root.join(file).is_file())
    {
        directories.insert(0, String::new());
    }
    for directory in directories {
        if !directory.is_empty() && !safe_relative(&directory) {
            continue;
        }
        let metadata = cache.and_then(|cache| {
            cache.get(if directory.is_empty() {
                "Default"
            } else {
                directory.as_str()
            })
        });
        let string = |key: &str| {
            metadata
                .and_then(|value| value[key].as_str())
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let name = string("name").unwrap_or_else(|| {
            if directory.is_empty() {
                "Default".to_owned()
            } else {
                directory.clone()
            }
        });
        let avatar = string("avatar_icon")
            .filter(|icon| icon.starts_with("chrome://theme/") || icon.starts_with("avatar"));
        push(
            found,
            location,
            name,
            root.join(&directory),
            string("user_name").or_else(|| string("gaia_email")),
            avatar,
            Vec::new(),
        );
    }
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && !path.contains('\\')
}

fn firefox_profiles(location: &BrowserLocation, root: &Path, found: &mut Vec<DiscoveredProfile>) {
    let mut indexed = Vec::new();
    if let Some(bytes) = read_index(&root.join("profiles.ini")) {
        let text = String::from_utf8_lossy(&bytes);
        let mut active = false;
        let (mut name, mut path, mut relative) = (None::<String>, None::<String>, true);
        let mut flush = |name: &mut Option<String>, path: &mut Option<String>, relative: bool| {
            let name = name.take();
            if let Some(path) = path.take() {
                if relative && !safe_relative(&path) {
                    return;
                }
                let dir = if relative {
                    root.join(&path)
                } else {
                    PathBuf::from(&path)
                };
                if is_real_dir(&dir) {
                    indexed.push((name.unwrap_or(path), dir));
                }
            }
        };
        for line in text.lines().map(str::trim) {
            if let Some(section) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                if active {
                    flush(&mut name, &mut path, relative);
                }
                active = section.starts_with("Profile");
                relative = true;
                continue;
            }
            if active {
                match line.split_once('=') {
                    Some(("Name", value)) => name = Some(value.to_owned()),
                    Some(("Path", value)) => path = Some(value.to_owned()),
                    Some(("IsRelative", value)) => relative = value != "0",
                    _ => {}
                }
            }
        }
        if active {
            flush(&mut name, &mut path, relative);
        }
    }
    for directory in [root.to_owned(), root.join("Profiles")] {
        if let Ok(entries) = fs::read_dir(directory) {
            let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                if is_real_dir(&entry.path())
                    && ["places.sqlite", "cookies.sqlite", "logins.json", "prefs.js"]
                        .iter()
                        .any(|file| entry.path().join(file).is_file())
                    && !indexed.iter().any(|(_, dir)| *dir == entry.path())
                {
                    indexed.push((
                        entry.file_name().to_string_lossy().into_owned(),
                        entry.path(),
                    ));
                }
            }
        }
    }
    for (name, dir) in indexed {
        push(found, location, name, dir, None, None, Vec::new());
    }
}

fn safari_profiles(location: &BrowserLocation, root: &Path, found: &mut Vec<DiscoveredProfile>) {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let source = Source { kind: BrowserKind::Safari, dir: root.to_owned() };
            let warnings = crate::safari::permission_warning(&source).map(|warning| vec![warning.to_owned()]).unwrap_or_else(|| if fs::read_dir(root).is_err() { vec!["Safari needs Full Disk Access. Allow openside in System Settings > Privacy & Security > Full Disk Access, then try again.".to_owned()] } else { Vec::new() });
            push(found, location, "Safari".to_owned(), root.to_owned(), None, None, warnings);
            if let Ok(entries) = fs::read_dir(root.join("Profiles")) {
                for entry in entries.filter_map(std::result::Result::ok).filter(|entry| is_real_dir(&entry.path())) {
                    push(found, location, entry.file_name().to_string_lossy().into_owned(), entry.path(), None, None, Vec::new());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => push(found, location, "Safari".to_owned(), root.to_owned(), None, None, vec!["Safari needs Full Disk Access. Allow openside in System Settings > Privacy & Security > Full Disk Access, then try again.".to_owned()]),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_brand_and_custom_profile_is_discovered() {
        let home = tempfile::tempdir().unwrap();
        for location in LOCATIONS
            .iter()
            .filter(|location| !location.linux.is_empty())
        {
            let relative = location.linux[0];
            let (base, suffix) = relative.split_once('/').unwrap();
            let root = if base == "config" {
                home.path().join(".config")
            } else {
                home.path().to_owned()
            }
            .join(suffix);
            let dir = root.join("Work");
            fs::create_dir_all(&dir).unwrap();
            if location.kind == BrowserKind::Firefox {
                fs::write(
                    root.join("profiles.ini"),
                    "[Profile0]\nName=Work\nPath=Work\nIsRelative=1\n",
                )
                .unwrap();
            } else {
                fs::write(root.join("Local State"), r#"{"profile":{"info_cache":{"Work":{"name":"Office","user_name":"person@example.test","avatar_icon":"chrome://theme/IDR_PROFILE_AVATAR_1"}}}}"#).unwrap();
            }
        }
        let found = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "linux",
        );
        for location in LOCATIONS
            .iter()
            .filter(|location| !location.linux.is_empty())
        {
            assert!(found.iter().any(|profile| profile.brand == location.brand));
        }
        let chrome = found
            .iter()
            .find(|profile| profile.brand == "chrome")
            .unwrap();
        assert_eq!(chrome.name, "Office");
        assert_eq!(chrome.email.as_deref(), Some("person@example.test"));
    }

    #[test]
    fn duplicate_profile_names_do_not_drop_profiles() {
        let home = tempfile::tempdir().unwrap();
        for root in [".config/google-chrome", ".config/google-chrome-beta"] {
            fs::create_dir_all(home.path().join(root).join("Default")).unwrap();
        }
        let found = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "linux",
        );
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].id, found[1].id);
        let beta_id = found[1].id.clone();
        fs::remove_dir_all(home.path().join(".config/google-chrome")).unwrap();
        let remaining = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "linux",
        );
        assert_eq!(remaining[0].id, beta_id);
    }

    #[test]
    fn mac_and_windows_tables_are_portable() {
        for platform in ["mac", "windows"] {
            let home = tempfile::tempdir().unwrap();
            let root = if platform == "mac" {
                "Library/Application Support/Google/Chrome"
            } else {
                "AppData/Local/Google/Chrome/User Data"
            };
            fs::create_dir_all(home.path().join(root).join("Default")).unwrap();
            let found = discover_platform(
                &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
                platform,
            );
            assert!(found.iter().any(|profile| profile.brand == "chrome"));
        }
    }

    #[test]
    fn firefox_index_accepts_all_relative_and_absolute_profiles() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".mozilla/firefox");
        let outside = home.path().join("outside-profile");
        fs::create_dir_all(root.join("nested/relative")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("profiles.ini"), format!("[Profile0]\nName=Relative\nIsRelative=1\nPath=nested/relative\n[Profile1]\nName=Absolute\nIsRelative=0\nPath={}\n", outside.display())).unwrap();
        let found = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "linux",
        );
        assert_eq!(
            found
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            ["Relative", "Absolute"]
        );
        assert_eq!(found[1].dir, outside);
    }

    #[cfg(unix)]
    #[test]
    fn safari_remains_visible_when_full_disk_access_is_needed() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("Library/Safari");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
        let protected = fs::read_dir(&root).is_err();
        let found = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "mac",
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(found.iter().any(|profile| profile.brand == "safari"));
        if protected {
            assert!(found[0]
                .warnings
                .iter()
                .any(|warning| warning.contains("Full Disk Access")));
        }
    }

    #[test]
    fn direct_opera_profile_does_not_offer_network_directory_as_another_profile() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".config/opera");
        fs::create_dir_all(root.join("Network")).unwrap();
        fs::write(root.join("Network/Cookies"), b"fixture").unwrap();
        fs::write(
            root.join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Opera personal"}}}}"#,
        )
        .unwrap();
        let found = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "linux",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].brand, "opera");
        assert_eq!(found[0].name, "Opera personal");
        assert_eq!(found[0].dir, root);
    }

    #[cfg(unix)]
    #[test]
    fn arc_sidebar_is_a_separate_shared_source_from_chromium_profiles() {
        let home = tempfile::tempdir().unwrap();
        let arc = home.path().join("Library/Application Support/Arc");
        fs::create_dir_all(arc.join("User Data/Default")).unwrap();
        fs::write(
            arc.join("User Data/Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Personal"}}}}"#,
        )
        .unwrap();
        fs::write(
            arc.join("StorableSidebar.json"),
            r#"{"sidebar":{"containers":[{"items":[]}]}}"#,
        )
        .unwrap();
        let profiles = discover_platform(
            &|key| (key == "HOME").then(|| home.path().as_os_str().to_owned()),
            "mac",
        );
        assert_eq!(profiles.len(), 2);
        let profile = profiles
            .iter()
            .find(|profile| profile.name == "Personal")
            .unwrap();
        assert!(!profile.available.bookmarks && !profile.available.tabs);
        let shared = profiles
            .iter()
            .find(|profile| profile.name == "All Spaces (shared sidebar)")
            .unwrap();
        assert_eq!(shared.dir, arc);
        assert!(shared.available.bookmarks && shared.available.tabs);
        assert!(shared
            .warnings
            .iter()
            .any(|warning| warning.contains("every Space")));
    }
}
