use std::path::PathBuf;

pub(crate) fn install(data_dir: PathBuf) {
    // The default hook prints the panic payload and can leak credentials to stderr.
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location();
        let result = openside_config::crash_reports::write_report(
            &data_dir,
            location.map(|value| value.file()),
            location.map(|value| value.line()),
        );
        crate::diag!(
            "opensided: panic payload redacted; local crash report {}",
            if result.is_ok() {
                "saved"
            } else {
                "unavailable"
            }
        );
    }));
}

#[cfg(test)]
mod tests {
    #[test]
    fn panic_hook_never_writes_payload_to_report_or_stderr() {
        const CHILD: &str = "OPENSIDE_PANIC_TEST_DIR";
        if let Some(path) = std::env::var_os(CHILD) {
            super::install(path.into());
            let _ = std::panic::catch_unwind(|| panic!("credential-canary-do-not-record"));
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "crash::tests::panic_hook_never_writes_payload_to_report_or_stderr",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("credential-canary-do-not-record")
        );
        let paths: Vec<_> = std::fs::read_dir(root.path().join("crash-reports"))
            .unwrap()
            .collect();
        assert_eq!(paths.len(), 1);
        let text = std::fs::read_to_string(paths[0].as_ref().unwrap().path()).unwrap();
        assert!(!text.contains("credential-canary-do-not-record"));
        assert!(text.contains("[redacted]"));
    }
}
