use crate::doctor::{Check, DoctorReport, Status};
use std::path::{Path, PathBuf};

const LIMITATION: &str = "The patched Chromium build does not bundle or license Widevine. Finding a CDM library does not prove DRM playback, codec support, browser certification or a streaming-service entitlement.";

fn library(root: &Path, depth: u8) -> Option<PathBuf> {
    if depth == 0
        || std::fs::symlink_metadata(root)
            .ok()?
            .file_type()
            .is_symlink()
    {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    scan_library(
        entries
            .take(128)
            .flatten()
            .map(|entry| (entry.path(), entry.file_type())),
        depth,
    )
}

fn scan_library(
    entries: impl IntoIterator<Item = (PathBuf, std::io::Result<std::fs::FileType>)>,
    depth: u8,
) -> Option<PathBuf> {
    for (path, kind) in entries {
        let Ok(kind) = kind else {
            continue;
        };
        if kind.is_file()
            && matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("libwidevinecdm.so" | "widevinecdm.dll" | "libwidevinecdm.dylib")
            )
        {
            return Some(path);
        }
        if kind.is_dir() {
            if let Some(found) = library(&path, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

fn versioned_library(root: &Path, nested: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .take(64)
        .flatten()
        .find_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.starts_with(|c: char| c.is_ascii_digit())
                || !name.chars().all(|c| c.is_ascii_digit() || c == '.')
            {
                return None;
            }
            library(&entry.path().join(nested), 5)
        })
}

fn browser_library(browser: &Path) -> Option<PathBuf> {
    let browser = std::fs::canonicalize(browser).ok()?;
    for parent in browser.ancestors().skip(1).take(5) {
        if let Some(found) = library(&parent.join("WidevineCdm"), 5) {
            return Some(found);
        }
        if let Some(found) = versioned_library(parent, Path::new("WidevineCdm")) {
            return Some(found);
        }
        // Distribution wrappers live in bin while Chromium's bundled CDM lives in lib.
        if parent.file_name().is_some_and(|name| name == "bin") {
            if let Some(name @ ("chromium" | "chromium-browser")) =
                browser.file_name().and_then(|name| name.to_str())
            {
                if let Some(found) = parent.parent().and_then(|prefix| {
                    library(&prefix.join("lib").join(name).join("WidevineCdm"), 5)
                }) {
                    return Some(found);
                }
            }
        }
        if parent.file_name().is_some_and(|name| name == "Contents") {
            let frameworks = parent.join("Frameworks");
            if let Ok(entries) = std::fs::read_dir(frameworks) {
                for framework in entries.take(32).flatten() {
                    if framework
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with("Framework.framework"))
                    {
                        if let Some(found) = versioned_library(
                            &framework.path().join("Versions"),
                            Path::new("Libraries/WidevineCdm"),
                        ) {
                            return Some(found);
                        }
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn doctor_check(report: &mut DoctorReport, browser: Option<&Path>) {
    let found = browser.and_then(browser_library);
    report.checks.push(Check {
        id: "browser.widevine".into(),
        status: if found.is_some() { Status::Ok } else { Status::Warn },
        summary: match found {
            Some(path) => format!("WidevineCdm library found at {}. {LIMITATION}", path.display()),
            None => format!("No WidevineCdm library found beside the configured browser. {LIMITATION}"),
        },
        next_step: Some("For DRM playback, use an officially supported browser and its licensed Widevine installation. Do not copy proprietary CDMs into the openside distribution. See docs/specs/drm.md.".into()),
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn unrelated_entry_metadata_error_does_not_hide_a_cdm() {
        let root = tempfile::tempdir().unwrap();
        let cdm = root.path().join("libwidevinecdm.so");
        std::fs::write(&cdm, "fixture").unwrap();
        let kind = std::fs::metadata(&cdm).unwrap().file_type();
        let entries = [
            (
                root.path().join("unreadable-sibling"),
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            ),
            (cdm.clone(), Ok(kind)),
        ];
        assert_eq!(scan_library(entries, 5), Some(cdm));
    }
    #[test]
    fn widevine_reports_library_presence_and_patched_build_limitations() {
        for name in [
            "libwidevinecdm.so",
            "widevinecdm.dll",
            "libwidevinecdm.dylib",
        ] {
            let root = tempfile::tempdir().unwrap();
            let browser = root.path().join("chrome");
            std::fs::write(&browser, "fixture").unwrap();
            let cdm = root.path().join("WidevineCdm/1.2/_platform_specific/test");
            std::fs::create_dir_all(&cdm).unwrap();
            let mut report = DoctorReport::default();
            doctor_check(&mut report, Some(&browser));
            assert_eq!(
                report.checks[0].status,
                Status::Warn,
                "an empty directory is not a CDM"
            );
            std::fs::write(cdm.join(name), "fixture").unwrap();
            report.checks.clear();
            doctor_check(&mut report, Some(&browser));
            assert_eq!(report.checks[0].status, Status::Ok);
            assert!(report.checks[0].summary.contains(name));
            assert!(report.checks[0]
                .summary
                .contains("does not bundle or license Widevine"));
            assert!(report.checks[0]
                .summary
                .contains("does not prove DRM playback"));
        }
    }
    #[test]
    fn widevine_finds_distribution_windows_and_macos_install_layouts() {
        for (executable, module) in [
            ("usr/bin/chromium", "usr/lib/chromium/WidevineCdm/libwidevinecdm.so"),
            ("Chrome/Application/chrome.exe", "Chrome/Application/123.0.1/WidevineCdm/widevinecdm.dll"),
            ("Chrome.app/Contents/MacOS/Chrome", "Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/123.0.1/Libraries/WidevineCdm/libwidevinecdm.dylib"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let browser = root.path().join(executable);
            let cdm = root.path().join(module);
            std::fs::create_dir_all(browser.parent().unwrap()).unwrap();
            std::fs::create_dir_all(cdm.parent().unwrap()).unwrap();
            std::fs::write(&browser, "fixture").unwrap();
            std::fs::write(&cdm, "fixture").unwrap();
            let mut report = DoctorReport::default();
            doctor_check(&mut report, Some(&browser));
            assert_eq!(report.checks[0].status, Status::Ok, "{module}");
            let reported_cdm = report.checks[0]
                .summary
                .strip_prefix("WidevineCdm library found at ")
                .unwrap()
                .strip_suffix(&format!(". {LIMITATION}"))
                .unwrap();
            assert_eq!(
                std::fs::canonicalize(reported_cdm).unwrap(),
                std::fs::canonicalize(&cdm).unwrap()
            );
        }
    }
    #[test]
    fn missing_browser_has_an_actionable_drm_warning() {
        let mut report = DoctorReport::default();
        doctor_check(&mut report, None);
        assert_eq!(report.checks[0].status, Status::Warn);
        assert!(report.checks[0]
            .next_step
            .as_ref()
            .unwrap()
            .contains("officially supported browser"));
    }
}
