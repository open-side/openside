use crate::doctor::{Check, DoctorReport, Status};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

const HOSTS: &[(&str, &[&str])] = &[
    (
        "1password",
        &[
            "com.1password.1password.json",
            "com.agilebits.1password.json",
        ],
    ),
    ("bitwarden", &["com.8bit.bitwarden.json"]),
];

fn user_dirs(platform: &str, env: &impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let home = env("HOME").map(PathBuf::from);
    let base = match platform {
        "linux" => env("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|path| path.join(".config"))),
        "macos" => home.map(|path| path.join("Library/Application Support")),
        _ => None,
    };
    let Some(base) = base else { return Vec::new() };
    let browsers: &[&str] = if platform == "linux" {
        &[
            "chromium",
            "google-chrome",
            "google-chrome-for-testing",
            "BraveSoftware/Brave-Browser",
            "microsoft-edge",
        ]
    } else {
        // Brave shares Chrome's native-host directories on macOS, not its profile directory.
        &[
            "Chromium",
            "Google/Chrome",
            "Google/ChromeForTesting",
            "Microsoft Edge",
        ]
    };
    browsers
        .iter()
        .map(|browser| base.join(browser).join("NativeMessagingHosts"))
        .collect()
}

fn system_dirs(platform: &str) -> Vec<PathBuf> {
    let paths: &[&str] = match platform {
        "linux" => &[
            "/etc/chromium/native-messaging-hosts",
            "/etc/opt/chrome/native-messaging-hosts",
            "/etc/opt/chrome_for_testing/native-messaging-hosts",
            "/etc/opt/edge/native-messaging-hosts",
        ],
        "macos" => &[
            "/Library/Application Support/Chromium/NativeMessagingHosts",
            "/Library/Google/Chrome/NativeMessagingHosts",
            "/Library/Google/ChromeForTesting/NativeMessagingHosts",
            "/Library/Microsoft/Edge/NativeMessagingHosts",
        ],
        _ => &[],
    };
    paths.iter().map(PathBuf::from).collect()
}

fn sandbox_dirs(platform: &str, env: &impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    if platform != "linux" {
        return Vec::new();
    }
    let Some(home) = env("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        ".var/app/org.chromium.Chromium/config/chromium/NativeMessagingHosts",
        ".var/app/com.google.Chrome/config/google-chrome/NativeMessagingHosts",
        ".var/app/com.microsoft.Edge/config/microsoft-edge/NativeMessagingHosts",
        ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser/NativeMessagingHosts",
        "snap/chromium/common/chromium/NativeMessagingHosts",
    ]
    .into_iter()
    .map(|path| home.join(path))
    .collect()
}

pub(crate) fn warn_extension_loading(
    report: &mut DoctorReport,
    loaded: &crate::Loaded,
    executable: &Path,
) {
    if loaded.config.browser.extensions.is_empty() {
        return;
    }
    let path = executable
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('\\', "/");
    if (path.contains("google-chrome") && !path.contains("for-testing"))
        || path.contains("google chrome.app")
        || path.contains("/google/chrome/")
    {
        report.checks.push(Check {
            id: "browser.extensions.branded_chrome".into(), status: Status::Warn,
            summary: "Official Google Chrome 137+ ignores --load-extension; browser.extensions requires Chromium or Chrome for Testing.".into(),
            next_step: Some("Select Chromium or Chrome for Testing with browser.executable. In a separately opened Google Chrome, use chrome://extensions > Developer mode > Load unpacked instead; that browser is not the daemon's managed profile.".into()),
        });
    }
}
pub(crate) fn doctor_check(
    report: &mut DoctorReport,
    loaded: &crate::Loaded,
    env: &impl Fn(&str) -> Option<OsString>,
) {
    let platform = std::env::consts::OS;
    inspect_profile(report, loaded, env, platform, system_dirs(platform));
}

fn inspect_profile(
    report: &mut DoctorReport,
    loaded: &crate::Loaded,
    env: &impl Fn(&str) -> Option<OsString>,
    platform: &str,
    system: Vec<PathBuf>,
) {
    let mut user = user_dirs(platform, env);
    user.extend(sandbox_dirs(platform, env));
    if platform != "windows" {
        let managed = loaded
            .paths
            .data_dir
            .join("browser-profile/NativeMessagingHosts");
        let found = HOSTS
            .iter()
            .any(|(_, names)| names.iter().any(|name| managed.join(name).is_file()));
        report.checks.push(Check {
            id: "native_messaging.managed_profile".into(), status: if found { Status::Ok } else { Status::Warn },
            summary: format!("Managed browser NativeMessagingHosts: {} ({}); standard user profiles do not prove integration in this profile", managed.display(), if found { "known filename present" } else { "no known filename present" }),
            next_step: Some("Use the vendor's supported native integration setup for this disposable managed profile. Flatpak/Snap confinement or native-messaging portals may need package-specific setup; filename presence alone proves neither host execution nor a working connection. Doctor does not copy or read manifests.".into()),
        });
        user.push(managed);
    }
    inspect(report, platform, user, system);
}

fn inspect(report: &mut DoctorReport, platform: &str, user: Vec<PathBuf>, system: Vec<PathBuf>) {
    let guidance = "Install the vendor desktop app and Chromium extension, enable its browser integration, then verify the native connection in a disposable profile. Doctor never reads manifests, creates links, or runs native hosts.";
    if platform == "windows" {
        for browser in [
            "Chromium",
            "Google\\Chrome",
            "BraveSoftware\\Brave-Browser",
            "Microsoft\\Edge",
        ] {
            report.checks.push(Check { id: format!("native_messaging.registry.{browser}"), status: Status::Ok,
                summary: format!("Windows registry (not probed): HKCU and HKLM \\Software\\{browser}\\NativeMessagingHosts; also check the 32-bit registry view."), next_step: Some(guidance.into()) });
        }
        return;
    }
    for (index, dir) in user.iter().chain(&system).enumerate() {
        report.checks.push(Check {
            id: format!("native_messaging.path.{index}"),
            status: Status::Ok,
            summary: format!(
                "NativeMessagingHosts: {} ({})",
                dir.display(),
                if dir.is_dir() { "present" } else { "absent" }
            ),
            next_step: Some(guidance.into()),
        });
    }
    for (vendor, names) in HOSTS {
        // Checking fixed basenames avoids reading user-controlled manifest contents or commands.
        let found: Vec<_> = user
            .iter()
            .chain(&system)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .filter(|path| path.is_file())
            .collect();
        report.checks.push(Check { id: format!("native_messaging.{vendor}"), status: if found.is_empty() { Status::Warn } else { Status::Ok },
            summary: if found.is_empty() { format!("{vendor}: no known host manifest filename found; extension installation and native connection are not verified") }
                else { format!("{vendor}: manifest filename present at {}; contents and native connection are not verified", found.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")) },
            next_step: Some(guidance.into()) });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn doctor_native_hosts_use_temp_home_and_xdg_without_reading_manifests() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let xdg = root.path().join("xdg");
        let env = |key: &str| match key {
            "HOME" => Some(home.clone().into_os_string()),
            "XDG_CONFIG_HOME" => Some(xdg.clone().into_os_string()),
            _ => None,
        };
        let dirs = user_dirs("linux", &env);
        assert_eq!(dirs.len(), 5);
        assert!(dirs.iter().all(|dir| dir.starts_with(&xdg)));
        std::fs::create_dir_all(&dirs[0]).unwrap();
        std::fs::write(
            dirs[0].join("com.1password.1password.json"),
            "not JSON; private-command-secret",
        )
        .unwrap();
        let mut report = DoctorReport::default();
        inspect(&mut report, "linux", dirs.clone(), Vec::new());
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "native_messaging.1password")
                .unwrap()
                .status,
            Status::Ok
        );
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "native_messaging.bitwarden")
                .unwrap()
                .status,
            Status::Warn
        );
        assert!(!report.render_text().contains("private-command-secret"));
        assert!(report.render_text().contains("disposable profile"));
        std::fs::create_dir_all(&dirs[4]).unwrap();
        std::fs::write(dirs[4].join("com.8bit.bitwarden.json"), "invalid JSON").unwrap();
        let mut report = DoctorReport::default();
        inspect(&mut report, "linux", dirs, Vec::new());
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "native_messaging.bitwarden")
                .unwrap()
                .status,
            Status::Ok
        );
        let fallback = user_dirs("linux", &|key| {
            (key == "HOME").then(|| home.clone().into_os_string())
        });
        assert!(fallback[0].starts_with(home.join(".config")));
    }

    #[test]
    fn doctor_native_hosts_macos_paths_and_windows_registry_guidance() {
        let home = PathBuf::from("fixture-home");
        let mac = user_dirs("macos", &|key| {
            (key == "HOME").then(|| home.clone().into_os_string())
        });
        assert!(!mac
            .iter()
            .any(|dir| dir.to_string_lossy().contains("BraveSoftware")));
        assert_eq!(
            mac[1],
            home.join("Library/Application Support/Google/Chrome/NativeMessagingHosts")
        );
        let mut report = DoctorReport::default();
        inspect(&mut report, "windows", Vec::new(), Vec::new());
        assert_eq!(report.checks.len(), 4);
        assert!(report.checks.iter().all(|check| check.status == Status::Ok
            && check.summary.contains("HKCU and HKLM")
            && check.summary.contains("not probed")));
    }

    #[test]
    fn doctor_managed_profile_sandbox_candidates_and_branded_chrome_are_explicit() {
        let (root, mut loaded) = crate::tests::fixture();
        let env = |key: &str| (key == "HOME").then(|| root.path().join("home").into_os_string());
        let sandbox = sandbox_dirs("linux", &env);
        assert_eq!(sandbox.len(), 5);
        assert!(sandbox
            .iter()
            .any(|dir| dir.ends_with("snap/chromium/common/chromium/NativeMessagingHosts")));
        assert!(!system_dirs("linux")
            .iter()
            .any(|dir| dir.to_string_lossy().contains("/etc/brave/")));
        let managed = loaded
            .paths
            .data_dir
            .join("browser-profile/NativeMessagingHosts");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(
            managed.join("com.8bit.bitwarden.json"),
            "invalid private manifest contents",
        )
        .unwrap();
        let mut report = DoctorReport::default();
        inspect_profile(&mut report, &loaded, &env, "linux", Vec::new());
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "native_messaging.managed_profile")
                .unwrap()
                .status,
            Status::Ok
        );
        assert!(report
            .render_text()
            .contains(&managed.display().to_string()));
        assert!(!report
            .render_text()
            .contains("invalid private manifest contents"));
        loaded
            .config
            .browser
            .extensions
            .push(root.path().to_path_buf());
        warn_extension_loading(&mut report, &loaded, Path::new("google-chrome-stable"));
        assert!(report
            .checks
            .iter()
            .any(|check| check.id == "browser.extensions.branded_chrome"
                && check.status == Status::Warn));
        let mut supported = DoctorReport::default();
        warn_extension_loading(
            &mut supported,
            &loaded,
            Path::new("google-chrome-for-testing"),
        );
        assert!(supported.checks.is_empty());
    }
}
