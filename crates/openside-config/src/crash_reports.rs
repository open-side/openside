//! Local metadata-only panic reports. Payloads and backtraces can contain secrets.
use crate::doctor::{Check, DoctorReport, Status};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Persist only fixed metadata; callers cannot pass a panic payload or environment.
pub fn write_report(
    data_dir: &Path,
    source_file: Option<&str>,
    line: Option<u32>,
) -> io::Result<PathBuf> {
    let directory = crate::data_layout::DataPath::CrashReports.path(data_dir);
    std::fs::create_dir_all(&directory)?;
    if std::fs::symlink_metadata(&directory)?
        .file_type()
        .is_symlink()
    {
        return Err(io::Error::other("crash report directory is a symlink"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let path = directory.join(format!("panic-{timestamp}-{}.json", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(&path)?;
    let source = source_file
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str());
    let report = serde_json::json!({"format":"openside-crash/1","kind":"rust_panic","timestamp_unix_nanos":timestamp.to_string(),"pid":std::process::id(),"source_file":source,"line":line,"payload":"[redacted]","backtrace":"[omitted: may contain secrets]","uploaded":false});
    file.write_all(&serde_json::to_vec(&report)?)?;
    file.sync_all()?;
    Ok(path)
}

fn report_paths(directory: &Path) -> io::Result<Vec<String>> {
    if std::fs::symlink_metadata(directory)?
        .file_type()
        .is_symlink()
    {
        return Err(io::Error::other("crash report directory is a symlink"));
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("panic-") && name.ends_with(".json"))
        {
            paths.push(entry.path().display().to_string());
        }
    }
    Ok(paths)
}

pub(crate) fn doctor_check(report: &mut DoctorReport, loaded: &crate::Loaded) {
    let directory = crate::data_layout::DataPath::CrashReports.path(&loaded.paths.data_dir);
    let mut paths = match report_paths(&directory) {
        Ok(paths) => paths,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            report.checks.push(Check {
                id: "crash_reports".into(), status: Status::Warn,
                summary: format!("Cannot inspect local crash reports under {}: {error}", directory.display()),
                next_step: Some("Restore a private, readable crash-reports directory inside the configured data directory; do not use a symlink.".into()),
            });
            return;
        }
    };
    paths.sort();
    report.checks.push(Check {
        id:"crash_reports".into(), status: if paths.is_empty() { Status::Ok } else { Status::Warn },
        summary: format!("{} local redacted panic reports under {}{}; reports are never uploaded", paths.len(), directory.display(), if paths.is_empty() { String::new() } else { format!(": {}", paths.join(", ")) }),
        next_step: (!paths.is_empty()).then(|| "Inspect the metadata-only reports locally and delete them when no longer needed. SIGKILL/native Chromium minidumps are not Rust panic reports.".into()),
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn reports_are_private_redacted_and_doctor_lists_them() {
        let root = tempfile::tempdir().unwrap();
        let path = write_report(root.path(), Some("private/build/location.rs"), Some(42)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["source_file"], "location.rs");
        assert_eq!(value["payload"], "[redacted]");
        assert_eq!(value["uploaded"], false);
        assert!(!text.contains("private/build"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let loaded = crate::Loaded {
            config: crate::Config::default(),
            paths: crate::Paths {
                config_file: root.path().join("config.toml"),
                data_dir: root.path().to_path_buf(),
                state_dir: root.path().join("state"),
                runtime_dir: root.path().join("run"),
            },
            source: None,
            warnings: vec![],
        };
        let mut report = DoctorReport::default();
        doctor_check(&mut report, &loaded);
        assert_eq!(report.checks[0].status, Status::Warn);
        assert!(report.checks[0]
            .summary
            .contains(&path.display().to_string()));
    }
    #[test]
    fn doctor_inventory_errors_are_not_reported_as_no_crashes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("crash-reports"), "not a directory").unwrap();
        let loaded = crate::Loaded {
            config: crate::Config::default(),
            paths: crate::Paths {
                config_file: root.path().join("config.toml"),
                data_dir: root.path().to_path_buf(),
                state_dir: root.path().join("state"),
                runtime_dir: root.path().join("run"),
            },
            source: None,
            warnings: vec![],
        };
        let mut report = DoctorReport::default();
        doctor_check(&mut report, &loaded);
        assert_eq!(report.checks[0].status, Status::Warn);
        assert!(report.checks[0].summary.contains("Cannot inspect"));
        assert!(report.checks[0].next_step.is_some());
    }
    #[cfg(unix)]
    #[test]
    fn report_directory_symlink_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join("crash-reports")).unwrap();
        assert!(write_report(root.path(), None, None).is_err());
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    }
}
