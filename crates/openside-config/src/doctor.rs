use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::discovery::{
    agent_command_with, discover_browser_with, executable, find_on_path_with,
    platform_browser_locations,
};
use crate::{parse_config, validate, Loaded, StoredVault, VaultBackend, VaultResolveError};

/// Doctor inputs the configuration crate cannot probe itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct DoctorOptions {
    /// Vault file format and keyring reachability, probed by the caller that links the
    /// vault crate; `None` reports the vault file without its unlock method.
    pub vault: Option<VaultProbe>,
}

/// What the caller observed about the vault, without unlocking anything.
#[derive(Clone, Copy, Debug)]
pub struct VaultProbe {
    /// File format from the header; `None` when the file is unreadable or not a vault.
    pub stored: Option<StoredVault>,
    /// Whether this operating system's keyring answers (or can be activated).
    pub keyring_reachable: bool,
}

/// Result severity, serialized as `ok`, `warn`, or `fail`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The checked dependency is available and correctly configured.
    Ok,
    /// Optional dependency or setup issue, with a next step.
    Warn,
    /// A required dependency or configuration is unavailable.
    Fail,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

/// One availability or configuration check; never contains credential values.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Check {
    /// Stable machine-readable check identifier.
    pub id: String,
    /// Availability severity.
    pub status: Status,
    /// Human-readable, sanitized result.
    pub summary: String,
    /// Action to take when the check is not successful.
    pub next_step: Option<String>,
}

/// Serializable doctor response used by both JSON and plain-text CLI output.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct DoctorReport {
    /// All checks, in deterministic presentation order.
    pub checks: Vec<Check>,
}

impl DoctorReport {
    /// Render readable diagnostics, including remediation under every problem.
    pub fn render_text(&self) -> String {
        let mut text = String::new();
        for check in &self.checks {
            let _ = writeln!(
                text,
                "[{}] {}: {}",
                check.status.label(),
                check.id,
                check.summary
            );
            if let Some(next) = &check.next_step {
                let _ = writeln!(text, "  Next: {next}");
            }
        }
        text
    }

    fn add(
        &mut self,
        id: impl Into<String>,
        status: Status,
        summary: impl Into<String>,
        next_step: Option<String>,
    ) {
        self.checks.push(Check {
            id: id.into(),
            status,
            summary: summary.into(),
            next_step,
        });
    }
}

/// Check installation state without creating directories or exposing credentials.
///
/// The sidecar receives only credential-source configuration and model settings
/// through `OPENSIDE_DOCTOR_CONFIG`; stdout must be one JSON object within 60s.
pub async fn run(loaded: &Loaded, opts: DoctorOptions) -> DoctorReport {
    let env = |key: &str| std::env::var_os(key);
    run_with(
        loaded,
        &env,
        std::env::current_exe().ok().as_deref(),
        &platform_browser_locations(&env),
        opts,
    )
    .await
}

async fn run_with(
    loaded: &Loaded,
    env: &impl Fn(&str) -> Option<OsString>,
    current_exe: Option<&Path>,
    platform_locations: &[PathBuf],
    opts: DoctorOptions,
) -> DoctorReport {
    let mut report = DoctorReport::default();
    if let Some(path) = &loaded.source {
        match std::fs::read_to_string(path)
            .ok()
            .and_then(|text| parse_config(path, &text).ok())
            .and_then(|config| validate(&config, path).ok())
        {
            Some(_) => report.add(
                "config",
                Status::Ok,
                format!("{} is valid", path.display()),
                None,
            ),
            None => report.add(
                "config",
                Status::Fail,
                format!("{} is missing, unreadable, or invalid", path.display()),
                Some("Fix the TOML configuration file and reload it.".into()),
            ),
        }
    } else {
        report.add(
            "config",
            Status::Ok,
            format!(
                "{} is absent; using defaults and environment overrides",
                loaded.paths.config_file.display()
            ),
            None,
        );
    }
    for (index, warning) in loaded.warnings.iter().enumerate() {
        report.add(
            format!("config.warning.{index}"),
            Status::Warn,
            warning,
            Some("Review the configuration and remove the reported ambiguity.".into()),
        );
    }
    for (id, path, private) in [
        ("paths.data", &loaded.paths.data_dir, true),
        ("paths.state", &loaded.paths.state_dir, false),
        ("paths.runtime", &loaded.paths.runtime_dir, true),
    ] {
        check_directory(&mut report, id, path, private);
    }
    vault_check(&mut report, loaded, opts.vault);
    for (name, required) in [("python3", true), ("uv", false)] {
        match find_on_path_with(name, env) {
            Some(path) => report.add(format!("executable.{name}"), Status::Ok, format!("{} found at {}", name, path.display()), None),
            None => report.add(format!("executable.{name}"), Status::Warn,
                if required { "python3 not found on PATH; an explicit agent.command may supply Python" } else { "uv not found on PATH; automatic launch falls back to python3" },
                Some(if required { "Install Python or configure agent.command with your Python environment." } else { "Install uv for reproducible sidecar dependencies, or configure agent.command." }.into())),
        }
    }
    let agent = agent_command_with(&loaded.config, env, current_exe);
    for (index, problem) in agent.problems.iter().enumerate() {
        report.add(
            format!("agent.discovery.{index}"),
            Status::Warn,
            problem,
            Some(
                "Set agent.dir and install its dependencies with uv sync, or set agent.command."
                    .into(),
            ),
        );
    }
    probe_sidecar(&mut report, loaded, &agent.argv).await;
    match discover_browser_with(&loaded.config, env, platform_locations) {
        Ok(path) => {
            report.add(
                "browser",
                Status::Ok,
                format!("Chromium-compatible browser found at {}", path.display()),
                None,
            );
            crate::native_messaging::warn_extension_loading(&mut report, loaded, &path);
            crate::drm::doctor_check(&mut report, Some(&path));
        }
        Err(err) => {
            report.add(
                "browser",
                Status::Warn,
                err.to_string(),
                Some(
                    "Install Chromium or Google Chrome, or set browser.executable / OPENSIDE_BROWSER."
                        .into(),
                ),
            );
            crate::drm::doctor_check(&mut report, None);
        }
    }
    if cfg!(unix) {
        let bwrap = loaded
            .config
            .sandbox
            .bwrap
            .clone()
            .filter(|path| executable(path))
            .or_else(|| {
                if loaded.config.sandbox.bwrap.is_none() {
                    find_on_path_with("bwrap", env)
                } else {
                    None
                }
            });
        match bwrap {
            Some(path) => report.add("sandbox.bwrap", Status::Ok, format!("bubblewrap found at {}", path.display()), None),
            None => report.add("sandbox.bwrap", if loaded.config.sandbox.required { Status::Fail } else { Status::Warn },
                if loaded.config.sandbox.required { "bubblewrap is required but missing; shell tools must stay disabled" } else { "bubblewrap is missing; only policy-based protection is available" },
                Some("Install bubblewrap or configure sandbox.bwrap; enable sandbox.required to forbid unsandboxed shell tools.".into())),
        }
    } else {
        // The daemon serves only a Unix socket here, so the bubblewrap and Unix-socket listener
        // checks would describe a daemon that cannot start.
        report.add(
            "daemon.transport",
            Status::Fail,
            "The daemon transport is unsupported on this OS: opensided serves only a Unix socket and the CLI has no other endpoint",
            Some("Run opensided and openside on Linux or macOS; see docs/PLATFORMS.md.".into()),
        );
    }
    report.add(
        "network.allow",
        Status::Ok,
        if loaded.config.network.allow.is_empty() {
            "Outbound allowlist is empty: the sidecar, channels and model fetch reach only loopback and their configured endpoints; other destinations are refused.".into()
        } else {
            format!(
                "Outbound allowlist enforced by the sidecar, channels and model fetch (plus loopback and configured endpoints): {}",
                loaded.config.network.allow.join(", ")
            )
        },
        None,
    );
    crate::memory::doctor_check(&mut report, loaded);
    if cfg!(unix) {
        crate::remote::doctor_check(&mut report, loaded);
    }
    crate::native_messaging::doctor_check(&mut report, loaded, env);
    crate::crash_reports::doctor_check(&mut report, loaded);
    report
}

/// The vault's unlock method and the next step for it; never unlocks or runs
/// `vault.passphrase_cmd`.
fn vault_check(report: &mut DoctorReport, loaded: &Loaded, probe: Option<VaultProbe>) {
    let vault = &loaded.config.vault;
    let path = vault.file(&loaded.paths.data_dir);
    let shown = path.display();
    let Some(probe) = probe else {
        let state = if path.is_file() {
            "exists"
        } else {
            "is not created yet"
        };
        report.add(
            "vault",
            Status::Ok,
            format!("Vault file {shown} {state}"),
            None,
        );
        return;
    };
    let Some(stored) = probe.stored else {
        report.add(
            "vault",
            Status::Fail,
            format!("{shown} is unreadable or not an openside vault file"),
            Some(
                "Restore the file, or point vault.path / OPENSIDE_VAULT_PATH at the vault.".into(),
            ),
        );
        return;
    };
    let setting = serde_json::to_value(vault.unlock)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let backend = match vault.unlock.resolve(stored, probe.keyring_reachable) {
        Ok(backend) => backend,
        Err(VaultResolveError::UnsupportedPlatform) => {
            report.add(
                "vault",
                Status::Fail,
                format!("vault.unlock = \"{setting}\" is not available on this operating system"),
                Some("Set vault.unlock = \"auto\" to use this system's keyring.".into()),
            );
            return;
        }
        Err(VaultResolveError::FormatMismatch) => {
            let created = if stored == StoredVault::Keyring {
                "an OS keyring"
            } else {
                "a passphrase"
            };
            report.add(
                "vault",
                Status::Fail,
                format!("{shown} was created with {created}, but vault.unlock = \"{setting}\""),
                Some(
                    "Set vault.unlock = \"auto\" so the file keeps the method it was created with."
                        .into(),
                ),
            );
            return;
        }
    };
    let store = backend.describe();
    let (status, summary, next) = match (stored, backend) {
        (StoredVault::Missing, VaultBackend::Passphrase) => (
            Status::Warn,
            if vault.unlock == crate::VaultUnlockSetting::Passphrase {
                "Vault is not set up; vault.unlock = \"passphrase\"".to_owned()
            } else {
                "Vault is not set up and no OS keyring is reachable, so it needs a passphrase".to_owned()
            },
            Some("Run `openside vault init` and choose a passphrase (it is never written to disk), or start a keyring service and re-run doctor.".to_owned()),
        ),
        (StoredVault::Missing, _) if probe.keyring_reachable => (
            Status::Ok,
            format!("Vault is ready to be created at {shown}; its key will live in {store}"),
            Some("Add the first item on the Vault page or with `openside vault add`, or run `openside vault init`.".to_owned()),
        ),
        (StoredVault::Missing, _) => (
            Status::Warn,
            format!("Vault is not set up and {store} is not reachable"),
            Some("Start a keyring service in this session (gnome-keyring-daemon, KWallet), or set vault.unlock = \"passphrase\" and run `openside vault init`.".to_owned()),
        ),
        (_, VaultBackend::Passphrase) if vault.passphrase_cmd.is_some() => (
            Status::Ok,
            format!("Vault at {shown} is unlocked with vault.passphrase_cmd at daemon start (doctor does not run it)"),
            None,
        ),
        (_, VaultBackend::Passphrase) => (
            Status::Ok,
            format!("Vault at {shown} is passphrase-protected and starts locked"),
            Some("After each daemon start, unlock it with `openside vault unlock` or on the Vault page, or set vault.passphrase_cmd.".to_owned()),
        ),
        (_, _) if probe.keyring_reachable => (
            Status::Ok,
            format!("Vault at {shown} is unlocked with {store} at daemon start"),
            None,
        ),
        (_, _) => (
            Status::Warn,
            format!("Vault at {shown} keeps its key in {store}, which is not reachable; the vault stays locked"),
            Some("Start or unlock the keyring in this session, then run `openside vault unlock`.".to_owned()),
        ),
    };
    report.add("vault", status, summary, next);
}

fn check_directory(report: &mut DoctorReport, id: &str, path: &Path, private: bool) {
    let Ok(metadata) = std::fs::metadata(path) else {
        report.add(
            id,
            Status::Warn,
            format!("{} does not exist or cannot be inspected", path.display()),
            Some(format!(
                "Create {} with owner-only access (0700 on Unix).",
                path.display()
            )),
        );
        return;
    };
    if !metadata.is_dir() {
        report.add(
            id,
            Status::Fail,
            format!("{} is not a directory", path.display()),
            Some("Choose a directory path and create it with appropriate permissions.".into()),
        );
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = metadata.permissions().mode() & 0o777;
        let owned = metadata.uid() == nix::unistd::getuid().as_raw();
        if !owned
            || (private && mode != 0o700)
            || (!private && (mode & 0o700 != 0o700 || mode & 0o022 != 0))
        {
            report.add(
                id,
                Status::Warn,
                format!(
                    "{} has permissions {mode:04o}; owner access or privacy needs attention",
                    path.display()
                ),
                Some(format!(
                    "Ensure {} belongs to your user and set permissions to 0700.",
                    path.display()
                )),
            );
            return;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = private;
        if metadata.permissions().readonly() {
            report.add(
                id,
                Status::Warn,
                format!("{} is read-only", path.display()),
                Some("Grant your user read/write access to this directory.".into()),
            );
            return;
        }
    }
    report.add(
        id,
        Status::Ok,
        format!("{} exists with suitable permissions", path.display()),
        None,
    );
}

#[derive(Deserialize)]
struct SidecarDoctor {
    python: String,
    packages: BTreeMap<String, String>,
    credentials: Vec<CredentialAvailability>,
    models: ModelAvailability,
}

#[derive(Deserialize)]
struct CredentialAvailability {
    provider: String,
    source: String,
    available: bool,
    reason: String,
    next_step: String,
}

#[derive(Deserialize)]
struct ModelAvailability {
    default: Option<String>,
    fallback: Vec<String>,
    providers_ok: bool,
}

async fn probe_sidecar(report: &mut DoctorReport, loaded: &Loaded, argv: &[String]) {
    let next = || {
        Some("Install sidecar dependencies with uv sync and check agent.command / agent.dir; then run doctor again.".into())
    };
    let Some(program) = argv.first().filter(|program| !program.is_empty()) else {
        report.add(
            "agent.sidecar",
            Status::Fail,
            "Agent command is empty",
            next(),
        );
        return;
    };
    let config = serde_json::json!({ "credentials": loaded.config.credentials, "models": loaded.config.models });
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .args(["doctor", "--json"])
        .env("OPENSIDE_DOCTOR_CONFIG", config.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(Duration::from_secs(60), command.output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(_)) => {
            report.add(
                "agent.sidecar",
                Status::Fail,
                "Sidecar doctor exited unsuccessfully (stderr intentionally not displayed)",
                next(),
            );
            return;
        }
        Ok(Err(_)) => {
            report.add(
                "agent.sidecar",
                Status::Fail,
                "Sidecar doctor could not be started",
                next(),
            );
            return;
        }
        Err(_) => {
            report.add(
                "agent.sidecar",
                Status::Fail,
                "Sidecar doctor exceeded its 60-second deadline",
                next(),
            );
            return;
        }
    };
    let sidecar: SidecarDoctor = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => {
            report.add(
                "agent.sidecar",
                Status::Fail,
                "Sidecar doctor did not return one valid doctor JSON object",
                next(),
            );
            return;
        }
    };
    report.add(
        "agent.sidecar",
        Status::Ok,
        format!("Sidecar doctor is available (Python {})", sidecar.python),
        None,
    );
    if sidecar.packages.is_empty() {
        report.add(
            "agent.packages",
            Status::Fail,
            "No sidecar dependency versions were reported",
            next(),
        );
    } else {
        report.add(
            "agent.packages",
            Status::Ok,
            sidecar
                .packages
                .into_iter()
                .map(|(name, version)| format!("{name} {version}"))
                .collect::<Vec<_>>()
                .join(", "),
            None,
        );
    }
    for (index, credential) in sidecar.credentials.into_iter().enumerate() {
        report.add(
            format!("credentials.{index}"),
            if credential.available {
                Status::Ok
            } else {
                Status::Warn
            },
            format!(
                "{} / {}: {}",
                credential.provider, credential.source, credential.reason
            ),
            if credential.available {
                None
            } else if credential.next_step.is_empty() {
                Some("Configure a credential source for this provider.".into())
            } else {
                Some(credential.next_step)
            },
        );
    }
    report.add("models", if sidecar.models.default.is_some() && sidecar.models.providers_ok { Status::Ok } else { Status::Warn },
        format!("Default model {}; {} fallback(s); providers {}", if sidecar.models.default.is_some() { "configured" } else { "not configured" },
            sidecar.models.fallback.len(), if sidecar.models.providers_ok { "available" } else { "unavailable" }),
        if sidecar.models.default.is_some() && sidecar.models.providers_ok { None } else { Some("Configure models.default (or select a model per session) and available credential sources.".into()) });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::tests::{env_lookup, fixture};
    use std::os::unix::fs::PermissionsExt;

    fn script(root: &Path, name: &str, contents: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, contents).unwrap_or_else(|err| panic!("script: {err}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|err| panic!("permissions: {err}"));
        path
    }

    #[tokio::test]
    async fn fake_sidecar_reports_availability_and_receives_only_config() {
        let (root, mut loaded) = fixture();
        for dir in [
            &loaded.paths.data_dir,
            &loaded.paths.state_dir,
            &loaded.paths.runtime_dir,
        ] {
            std::fs::create_dir_all(dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .unwrap_or_else(|err| panic!("permissions: {err}"));
        }
        let fake = script(
            root.path(),
            "doctor-ok",
            r#"#!/bin/sh
[ "$1" = doctor ] && [ "$2" = --json ] || exit 2
case "$OPENSIDE_DOCTOR_CONFIG" in *'"credentials":{}'*'"models":'* ) ;; *) exit 3 ;; esac
printf '%s\n' '{"python":"test-python","packages":{"pydantic-ai":"test-version"},"credentials":[{"provider":"test-provider","source":"env","available":false,"reason":"not set","next_step":"configure source"}],"models":{"default":null,"fallback":[],"providers_ok":false}}'
"#,
        );
        loaded.config.agent.command = Some(vec![fake.to_string_lossy().into_owned()]);
        let env = env_lookup(&[("PATH", root.path().as_os_str())]);
        let report = run_with(&loaded, &env, None, &[], DoctorOptions::default()).await;
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "agent.sidecar")
                .map(|check| check.status),
            Some(Status::Ok)
        );
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "credentials.0")
                .map(|check| check.status),
            Some(Status::Warn)
        );
        assert!(report
            .checks
            .iter()
            .filter(|check| check.status != Status::Ok)
            .all(|check| check.next_step.is_some()));
        assert!(report.render_text().contains("configure source"));
        let json = serde_json::to_value(&report).unwrap_or_else(|err| panic!("JSON: {err}"));
        assert_eq!(json["checks"][0]["status"], "ok");
    }

    #[tokio::test]
    async fn sidecar_failures_and_invalid_json_never_expose_output() {
        let (root, mut loaded) = fixture();
        let env = env_lookup(&[("PATH", root.path().as_os_str())]);
        for (name, contents) in [
            (
                "doctor-fail",
                "#!/bin/sh\nprintf 'secret-output'\nprintf 'secret-stderr' >&2\nexit 9\n",
            ),
            ("doctor-invalid", "#!/bin/sh\nprintf 'secret-output'\n"),
            ("doctor-multiple", "#!/bin/sh\nprintf '{} {}'\n"),
        ] {
            let fake = script(root.path(), name, contents);
            loaded.config.agent.command = Some(vec![fake.to_string_lossy().into_owned()]);
            let report = run_with(&loaded, &env, None, &[], DoctorOptions::default()).await;
            assert_eq!(
                report
                    .checks
                    .iter()
                    .find(|check| check.id == "agent.sidecar")
                    .map(|check| check.status),
                Some(Status::Fail)
            );
            assert!(!report.render_text().contains("secret-"));
        }
        loaded.config.agent.command = Some(vec![root
            .path()
            .join("missing")
            .to_string_lossy()
            .into_owned()]);
        let report = run_with(&loaded, &env, None, &[], DoctorOptions::default()).await;
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "agent.sidecar")
                .map(|check| check.status),
            Some(Status::Fail)
        );
    }

    #[tokio::test]
    async fn configuration_changed_after_load_is_revalidated() {
        let (root, mut loaded) = fixture();
        loaded.source = Some(loaded.paths.config_file.clone());
        loaded.config.agent.command = Some(vec![root
            .path()
            .join("missing-agent")
            .to_string_lossy()
            .into_owned()]);
        let env = env_lookup(&[("PATH", root.path().as_os_str())]);
        for (contents, status) in [
            ("[agent]\nstartup_timeout_ms = 0", Status::Fail),
            ("[agent]\nunknown = true", Status::Fail),
            ("[policy]\nmode = 'guard'", Status::Ok),
        ] {
            std::fs::write(&loaded.paths.config_file, contents)
                .unwrap_or_else(|err| panic!("write: {err}"));
            let report = run_with(&loaded, &env, None, &[], DoctorOptions::default()).await;
            assert_eq!(report.checks[0].status, status);
        }
    }

    #[tokio::test]
    async fn vault_check_reports_the_unlock_method_without_running_passphrase_command() {
        use crate::VaultUnlockSetting::{Auto, Passphrase, SecretService};
        let (root, mut loaded) = fixture();
        let marker = root.path().join("passphrase-command-ran");
        let command = script(
            root.path(),
            "unlock-command",
            &format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
        );
        loaded.config.agent.command = Some(vec![root
            .path()
            .join("missing-agent")
            .to_string_lossy()
            .into_owned()]);
        let env = env_lookup(&[("PATH", root.path().as_os_str())]);
        let default_path = crate::data_layout::DataPath::Vault.path(&loaded.paths.data_dir);
        let keyring = VaultBackend::platform_keyring().is_some();
        let cases = [
            // (unlock, stored, keyring reachable, passphrase_cmd, status, summary fragment, next step)
            (
                Auto,
                None,
                true,
                false,
                Status::Ok,
                "is not created yet",
                false,
            ),
            (
                Auto,
                Some(StoredVault::Missing),
                true,
                false,
                if keyring { Status::Ok } else { Status::Warn },
                if keyring {
                    "ready to be created"
                } else {
                    "needs a passphrase"
                },
                true,
            ),
            (
                Auto,
                Some(StoredVault::Missing),
                false,
                false,
                Status::Warn,
                "needs a passphrase",
                true,
            ),
            (
                Passphrase,
                Some(StoredVault::Missing),
                true,
                false,
                Status::Warn,
                "vault.unlock = \"passphrase\"",
                true,
            ),
            (
                Auto,
                Some(StoredVault::Passphrase),
                true,
                true,
                Status::Ok,
                "vault.passphrase_cmd at daemon start",
                false,
            ),
            (
                Auto,
                Some(StoredVault::Passphrase),
                true,
                false,
                Status::Ok,
                "starts locked",
                true,
            ),
            (
                Passphrase,
                Some(StoredVault::Keyring),
                true,
                false,
                Status::Fail,
                "created with an OS keyring",
                true,
            ),
        ];
        for (unlock, stored, reachable, with_cmd, status, fragment, next) in cases {
            loaded.config.vault.unlock = unlock;
            loaded.config.vault.passphrase_cmd =
                with_cmd.then(|| vec![command.to_string_lossy().into_owned()]);
            let opts = DoctorOptions {
                vault: stored.map(|stored| VaultProbe {
                    stored: Some(stored),
                    keyring_reachable: reachable,
                }),
            };
            let report = run_with(&loaded, &env, None, &[], opts).await;
            let checks: Vec<_> = report
                .checks
                .iter()
                .filter(|check| check.id == "vault")
                .collect();
            assert_eq!(checks.len(), 1);
            assert_eq!(
                checks[0].status, status,
                "{unlock:?} {stored:?}: {}",
                checks[0].summary
            );
            assert!(
                checks[0].summary.contains(fragment),
                "{}",
                checks[0].summary
            );
            assert_eq!(checks[0].next_step.is_some(), next, "{}", checks[0].summary);
            assert!(!report.render_text().contains("unlock-command"));
        }
        if VaultBackend::platform_keyring() == Some(VaultBackend::SecretService) {
            loaded.config.vault.unlock = SecretService;
            let opts = DoctorOptions {
                vault: Some(VaultProbe {
                    stored: Some(StoredVault::Keyring),
                    keyring_reachable: false,
                }),
            };
            let report = run_with(&loaded, &env, None, &[], opts).await;
            let check = report.checks.iter().find(|check| check.id == "vault");
            assert_eq!(check.map(|check| check.status), Some(Status::Warn));
            let summary = check
                .map(|check| check.summary.as_str())
                .unwrap_or_default();
            assert!(summary.contains("not reachable"), "{summary}");
            assert!(
                summary.contains(&default_path.display().to_string()),
                "{summary}"
            );
        }
        let opts = DoctorOptions {
            vault: Some(VaultProbe {
                stored: None,
                keyring_reachable: true,
            }),
        };
        let report = run_with(&loaded, &env, None, &[], opts).await;
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "vault")
                .map(|check| check.status),
            Some(Status::Fail)
        );
        assert!(
            !marker.exists(),
            "doctor must not execute the passphrase command"
        );
    }

    #[tokio::test]
    async fn required_sandbox_permissions_and_allowlist_are_reported() {
        let (root, mut loaded) = fixture();
        std::fs::create_dir(&loaded.paths.data_dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
        std::fs::set_permissions(
            &loaded.paths.data_dir,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap_or_else(|err| panic!("permissions: {err}"));
        loaded.config.agent.command = Some(vec![root
            .path()
            .join("missing-agent")
            .to_string_lossy()
            .into_owned()]);
        loaded.config.sandbox.required = true;
        loaded.config.network.allow = vec!["example.invalid".into()];
        let env = env_lookup(&[("PATH", root.path().as_os_str())]);
        let report = run_with(&loaded, &env, None, &[], DoctorOptions::default()).await;
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "paths.data")
                .map(|check| check.status),
            Some(Status::Warn)
        );
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "sandbox.bwrap")
                .map(|check| check.status),
            Some(Status::Fail)
        );
        let text = report.render_text();
        assert!(text.contains("example.invalid"));
        assert!(text.contains("enforced"));
        assert!(!text.contains("informational"));
        assert!(!text.contains("no network isolation"));
    }
}
