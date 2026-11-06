use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::discovery::{agent_command_with, discover_browser_with, find_on_path_with};
use crate::*;

pub(crate) fn fixture() -> (TempDir, Loaded) {
    let root = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
    let paths = Paths {
        config_file: root.path().join("config.toml"),
        data_dir: root.path().join("data"),
        state_dir: root.path().join("state"),
        runtime_dir: root.path().join("runtime"),
    };
    (
        root,
        Loaded {
            config: Config::default(),
            paths,
            source: None,
            warnings: Vec::new(),
        },
    )
}

pub(crate) fn env_lookup(entries: &[(&str, &OsStr)]) -> impl Fn(&str) -> Option<OsString> {
    let entries: std::collections::BTreeMap<String, OsString> = entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.to_os_string()))
        .collect();
    move |key| entries.get(key).cloned()
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap_or_else(|err| panic!("write: {err}"));
}

fn binary(path: &Path) {
    write_executable(path, "fake executable");
}

/// Writes an executable file without this process ever holding a write fd to it. Such an fd
/// is inherited by children that parallel tests fork until they exec, and exec'ing the file
/// meanwhile fails with ETXTBSY; `cp` holds the only write fd, in its own process.
pub(crate) fn write_executable(path: &Path, contents: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let source = path.with_extension("source");
        write(&source, contents);
        let status = std::process::Command::new("cp")
            .arg(&source)
            .arg(path)
            .status()
            .unwrap_or_else(|err| panic!("cp: {err}"));
        assert!(status.success(), "cp {}: {status}", path.display());
        std::fs::remove_file(&source).unwrap_or_else(|err| panic!("remove: {err}"));
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|err| panic!("permissions: {err}"));
    }
    #[cfg(not(unix))]
    write(path, contents);
}

#[test]
fn min_bun_matches_the_sidecar_engines_bun() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../agent/package.json"))
            .unwrap_or_else(|err| panic!("agent/package.json: {err}"));
    let (major, minor, patch) = crate::discovery::MIN_BUN;
    assert_eq!(
        crate::discovery::MIN_BUN_TEXT,
        format!("{major}.{minor}.{patch}")
    );
    assert_eq!(
        manifest["engines"]["bun"],
        format!(">={}", crate::discovery::MIN_BUN_TEXT)
    );
}

#[test]
fn file_values_and_all_environment_overrides_have_defined_precedence() {
    let (root, loaded) = fixture();
    write(
        &loaded.paths.config_file,
        r#"
[daemon]
data_dir = "file-data"
socket = "file.sock"
listen = "127.0.0.1:1"
[vault]
path = "file-vault"
passphrase_cmd = ["unlock-fixture", "--no-shell"]
[agent]
command = ["file-bun"]
dir = "file-agent"
bun = "file-bun-executable"
startup_timeout_ms = 123
[models]
default = "file-selection"
fallback = ["fallback-selection"]
[credentials.any_source]
opaque_setting = "not-a-credential"
[browser]
executable = "file-browser"
[policy]
mode = "read-only"
"#,
    );
    let env = env_lookup(&[
        ("OPENSIDE_DATA_DIR", OsStr::new("env-data")),
        ("OPENSIDE_SOCKET", OsStr::new("env.sock")),
        ("OPENSIDE_LISTEN", OsStr::new("127.0.0.1:2")),
        (
            "OPENSIDE_AGENT_COMMAND",
            OsStr::new(r#"["env-bun","run","env-agent/src/main.ts"]"#),
        ),
        ("OPENSIDE_AGENT_DIR", OsStr::new("env-agent")),
        ("OPENSIDE_BUN", OsStr::new("env-bun-executable")),
        ("OPENSIDE_MODEL", OsStr::new("env-selection")),
        ("OPENSIDE_BROWSER", OsStr::new("env-browser")),
        ("OPENSIDE_VAULT_PATH", OsStr::new("env-vault")),
    ]);
    let parsed = load_with(None, loaded.paths, &env).unwrap_or_else(|err| panic!("load: {err}"));
    assert_eq!(
        parsed.config.daemon.data_dir,
        Some(PathBuf::from("env-data"))
    );
    assert_eq!(parsed.paths.data_dir, PathBuf::from("env-data"));
    assert_eq!(parsed.config.daemon.socket, Some(PathBuf::from("env.sock")));
    assert_eq!(parsed.config.daemon.listen.map(|addr| addr.port()), Some(2));
    assert_eq!(parsed.config.vault.path, Some(PathBuf::from("env-vault")));
    assert_eq!(
        parsed.config.vault.passphrase_cmd,
        Some(vec!["unlock-fixture".into(), "--no-shell".into()])
    );
    assert_eq!(
        parsed.config.agent.command,
        Some(vec![
            "env-bun".into(),
            "run".into(),
            "env-agent/src/main.ts".into()
        ])
    );
    assert_eq!(parsed.config.agent.dir, Some(PathBuf::from("env-agent")));
    assert_eq!(
        parsed.config.agent.bun,
        Some(PathBuf::from("env-bun-executable"))
    );
    assert_eq!(parsed.config.agent.startup_timeout_ms, 123);
    assert_eq!(
        parsed.config.models.default.as_deref(),
        Some("env-selection")
    );
    assert_eq!(parsed.config.models.fallback, ["fallback-selection"]);
    assert_eq!(
        parsed.config.browser.executable,
        Some(PathBuf::from("file-browser"))
    );
    assert_eq!(parsed.config.policy.mode, PolicyModeSetting::ReadOnly);
    assert_eq!(parsed.source, Some(root.path().join("config.toml")));
    assert_eq!(
        parsed.config.credentials["any_source"]["opaque_setting"],
        "not-a-credential"
    );
}

#[test]
fn ticket_lifetimes_default_and_reject_zero() {
    let path = Path::new("config.toml");
    let default = parse_config(path, "").unwrap_or_else(|err| panic!("default: {err}"));
    assert_eq!(default.tickets.confirm_ttl_secs, 600);
    assert_eq!(default.tickets.secret_input_ttl_secs, 300);
    assert_eq!(default.tickets.device_approval_ttl_secs, 1800);
    assert_eq!(default.tickets.takeover_ttl_secs, 1800);
    assert_eq!(default.tickets.info_ttl_secs, 86400);
    let short = parse_config(path, "[tickets]\nconfirm_ttl_secs = 2")
        .unwrap_or_else(|err| panic!("short: {err}"));
    assert_eq!(short.tickets.confirm_ttl_secs, 2);
    for key in [
        "confirm_ttl_secs",
        "secret_input_ttl_secs",
        "device_approval_ttl_secs",
        "info_ttl_secs",
        "takeover_ttl_secs",
    ] {
        assert!(parse_config(path, &format!("[tickets]\n{key} = 0")).is_err());
    }
}

#[test]
fn agent_runtime_options_reach_the_sidecar_and_keep_must_stay_below_max() {
    let path = Path::new("config.toml");
    let tuned = parse_config(
        path,
        "[agent]\ntool_retries = 1\ncompaction_max_messages = 40\ncompaction_keep_messages = 39\nprofile = \"browser-agent\"",
    )
    .unwrap_or_else(|err| panic!("tuned: {err}"));
    assert_eq!(
        tuned.agent.runtime_options(),
        serde_json::json!({"tool_retries": 1, "compaction_max_messages": 40, "compaction_max_tokens": 64000, "compaction_keep_messages": 39, "profile": "browser-agent", "orchestration": "front"})
    );
    validate(&tuned, path).unwrap_or_else(|err| panic!("tuned validation: {err}"));
    let equal = parse_config(
        path,
        "[agent]\ncompaction_max_messages = 20\ncompaction_keep_messages = 20",
    )
    .unwrap_or_else(|err| panic!("equal: {err}"));
    let error = validate(&equal, path)
        .err()
        .unwrap_or_else(|| panic!("keep == max must be rejected"));
    assert_eq!(error.key, "agent.compaction_keep_messages");
    let defaults = AgentConfig::default().runtime_options();
    assert!(defaults["compaction_max_messages"].is_null());
    assert_eq!(defaults["compaction_max_tokens"], 64_000);
    for source in [
        "[agent]\ncompaction_max_tokens = 0",
        "[agent]\ncompaction_keep_messages = 1",
    ] {
        let invalid = parse_config(path, source)
            .unwrap_or_else(|err| panic!("parse invalid thresholds: {err}"));
        assert!(validate(&invalid, path).is_err());
    }
    let budget = parse_config(path, "[agent]\ncompaction_max_tokens = 32000")
        .unwrap_or_else(|err| panic!("token budget: {err}"));
    assert_eq!(
        budget.agent.runtime_options()["compaction_max_tokens"],
        32_000
    );
}

#[test]
fn missing_default_is_ok_but_explicit_file_and_openside_config_are_required() {
    let (_root, loaded) = fixture();
    let default = load_with(None, loaded.paths.clone(), &|_| None)
        .unwrap_or_else(|err| panic!("load: {err}"));
    assert!(default.source.is_none());
    assert_eq!(default.config.agent.startup_timeout_ms, 30_000);
    assert!(default.config.credentials.is_object());
    let explicit = load_with(
        Some(&loaded.paths.config_file),
        loaded.paths.clone(),
        &|_| None,
    );
    assert!(matches!(explicit, Err(ConfigError { key, .. }) if key == "$"));
    let selected = loaded.paths.config_file.as_os_str();
    let env = env_lookup(&[("OPENSIDE_CONFIG", selected)]);
    assert!(load_with(None, loaded.paths, &env).is_err());
}

#[test]
fn unknown_sections_and_keys_report_file_and_exact_key() {
    let (_root, loaded) = fixture();
    for (text, expected) in [
        ("unknown = true", "unknown"),
        ("[agent]\nunknown = true", "agent.unknown"),
        ("[browser]\nexecutible = 'misspelled'", "browser.executible"),
        ("[vault]\nunknown = true", "vault.unknown"),
    ] {
        write(&loaded.paths.config_file, text);
        let err = load_with(None, loaded.paths.clone(), &|_| None)
            .err()
            .unwrap_or_else(|| panic!("must reject unknown key"));
        assert_eq!(err.path, loaded.paths.config_file);
        assert_eq!(err.key, expected);
        assert!(err
            .to_string()
            .contains(&loaded.paths.config_file.display().to_string()));
    }
}

#[test]
fn browser_gateway_defaults_to_auto_and_accepts_only_auto_or_off() {
    let (_root, loaded) = fixture();
    let default = load_with(None, loaded.paths.clone(), &|_| None)
        .unwrap_or_else(|err| panic!("load: {err}"));
    assert_eq!(default.config.browser.gateway, BrowserGatewaySetting::Auto);
    write(&loaded.paths.config_file, "[browser]\ngateway = 'off'");
    let off = load_with(None, loaded.paths.clone(), &|_| None)
        .unwrap_or_else(|err| panic!("load: {err}"));
    assert_eq!(off.config.browser.gateway, BrowserGatewaySetting::Off);
    write(&loaded.paths.config_file, "[browser]\ngateway = 'required'");
    let err = load_with(None, loaded.paths.clone(), &|_| None)
        .err()
        .unwrap_or_else(|| panic!("must reject an unknown gateway mode"));
    assert_eq!(err.key, "browser.gateway");
}

#[test]
fn browser_language_accepts_only_language_codes() {
    let (_root, loaded) = fixture();
    for tag in ["ko", "en", "fil", "pt-BR", "zh_Hant"] {
        write(
            &loaded.paths.config_file,
            &format!("[browser]\nlanguage = '{tag}'"),
        );
        let config = load_with(None, loaded.paths.clone(), &|_| None)
            .unwrap_or_else(|err| panic!("load {tag}: {err}"));
        assert_eq!(config.config.browser.language.as_deref(), Some(tag));
    }
    for tag in ["", "k", "KO", "ko-", "ko-KR-x", "ko;rm", "ko KR", "korean"] {
        write(
            &loaded.paths.config_file,
            &format!("[browser]\nlanguage = '{tag}'"),
        );
        let err = load_with(None, loaded.paths.clone(), &|_| None)
            .err()
            .unwrap_or_else(|| panic!("must reject {tag:?}"));
        assert_eq!(err.key, "browser.language");
    }
}

#[test]
fn invalid_types_values_and_env_report_keys_without_raw_values() {
    let (_root, loaded) = fixture();
    for (text, expected) in [
        (
            "[agent]\nstartup_timeout_ms = 0",
            "agent.startup_timeout_ms",
        ),
        ("[agent]\ncommand = []", "agent.command"),
        ("[agent]\ncommand = ['']", "agent.command"),
        ("[models]\ndefault = ''", "models.default"),
        ("[policy]\nmode = 'secret-canary'", "policy.mode"),
        (
            "[agent]\nstartup_timeout_ms = 'secret-canary'",
            "agent.startup_timeout_ms",
        ),
        ("[vault]\npassphrase_cmd = []", "vault.passphrase_cmd"),
        ("[vault]\npath = ''", "vault.path"),
    ] {
        write(&loaded.paths.config_file, text);
        let err = load_with(None, loaded.paths.clone(), &|_| None)
            .err()
            .unwrap_or_else(|| panic!("must reject invalid value"));
        assert_eq!(err.key, expected);
        assert!(!err.to_string().contains("secret-canary"));
    }
    write(
        &loaded.paths.config_file,
        "[credentials]\nvalue = 'secret-canary'\ninvalid = [",
    );
    let err = load_with(None, loaded.paths.clone(), &|_| None)
        .err()
        .unwrap_or_else(|| panic!("must reject syntax"));
    assert!(!err.to_string().contains("secret-canary"));
    write(&loaded.paths.config_file, "");
    for (name, key) in [
        ("OPENSIDE_LISTEN", "daemon.listen"),
        ("OPENSIDE_AGENT_COMMAND", "agent.command"),
    ] {
        let env = env_lookup(&[(name, OsStr::new("secret-canary"))]);
        let err = load_with(None, loaded.paths.clone(), &env)
            .err()
            .unwrap_or_else(|| panic!("must reject env"));
        assert_eq!(err.key, key);
        assert!(!err.to_string().contains("secret-canary"));
    }
}

#[test]
fn explicit_file_overrides_discovered_path() {
    let (root, loaded) = fixture();
    let explicit = root.path().join("explicit.toml");
    write(&explicit, "[policy]\nmode = 'full'");
    let parsed = load_with(Some(&explicit), loaded.paths, &|_| None)
        .unwrap_or_else(|err| panic!("load: {err}"));
    assert_eq!(parsed.source, Some(explicit.clone()));
    assert_eq!(parsed.paths.config_file, explicit);
    assert_eq!(parsed.config.policy.mode, PolicyModeSetting::Full);
}

#[test]
fn config_debug_redacts_opaque_credentials() {
    let config = Config {
        credentials: serde_json::json!({"opaque": "secret-canary"}),
        ..Config::default()
    };
    assert!(!format!("{config:?}").contains("secret-canary"));
    let vault = VaultConfig {
        path: None,
        passphrase_cmd: Some(vec!["secret-canary".into()]),
        ..VaultConfig::default()
    };
    assert!(!format!("{vault:?}").contains("secret-canary"));
}

#[test]
fn example_and_schema_cover_every_known_key() {
    let path = Path::new("example.toml");
    let config = parse_config(path, &example_toml()).unwrap_or_else(|err| panic!("example: {err}"));
    validate(&config, path).unwrap_or_else(|err| panic!("validation: {err}"));
    let schema = config_schema();
    for section in [
        "daemon",
        "vault",
        "agent",
        "models",
        "credentials",
        "browser",
        "sandbox",
        "policy",
        "network",
        "ui",
    ] {
        assert!(schema["properties"].get(section).is_some(), "{section}");
    }
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["credentials"]["type"], "object");
    for key in [
        "data_dir",
        "socket",
        "listen",
        "path",
        "passphrase_cmd",
        "command",
        "dir",
        "bun",
        "startup_timeout_ms",
        "default",
        "fallback",
        "executable",
        "bundle_dir",
        "required",
        "bwrap",
        "mode",
        "allow",
    ] {
        assert!(example_toml().contains(key), "missing example key {key}");
    }
}

#[test]
fn browser_discovery_prefers_config_env_bundle_path_and_platform_in_order() {
    let (root, loaded) = fixture();
    let mut config = loaded.config;
    let configured = root.path().join("configured");
    let environment = root.path().join("environment");
    let bundle = root.path().join("bundle");
    let path_dir = root.path().join("bin");
    let platform = root.path().join("platform-browser");
    std::fs::create_dir(&bundle).unwrap_or_else(|err| panic!("mkdir: {err}"));
    std::fs::create_dir(&path_dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
    for path in [
        &configured,
        &environment,
        &bundle.join("chrome"),
        &bundle.join("chromium"),
        &path_dir.join("chromium"),
        &platform,
    ] {
        binary(path);
    }
    config.browser.executable = Some(configured.clone());
    config.browser.bundle_dir = Some(bundle.clone());
    let env = env_lookup(&[
        ("OPENSIDE_BROWSER", environment.as_os_str()),
        ("PATH", path_dir.as_os_str()),
    ]);
    assert_eq!(
        discover_browser_with(&config, &env, std::slice::from_ref(&platform)),
        Ok(configured.clone())
    );
    std::fs::remove_file(&configured).unwrap_or_else(|err| panic!("remove fixture: {err}"));
    assert_eq!(
        discover_browser_with(&config, &env, std::slice::from_ref(&platform)),
        Ok(environment.clone())
    );
    std::fs::remove_file(&environment).unwrap_or_else(|err| panic!("remove fixture: {err}"));
    assert_eq!(
        discover_browser_with(&config, &env, std::slice::from_ref(&platform)),
        Ok(bundle.join("chrome"))
    );
    std::fs::remove_file(bundle.join("chrome"))
        .unwrap_or_else(|err| panic!("remove fixture: {err}"));
    assert_eq!(
        discover_browser_with(&config, &env, std::slice::from_ref(&platform)),
        Ok(bundle.join("chromium"))
    );
    std::fs::remove_file(bundle.join("chromium"))
        .unwrap_or_else(|err| panic!("remove fixture: {err}"));
    assert_eq!(
        discover_browser_with(&config, &env, std::slice::from_ref(&platform)),
        Ok(path_dir.join("chromium"))
    );
    std::fs::remove_file(path_dir.join("chromium"))
        .unwrap_or_else(|err| panic!("remove fixture: {err}"));
    assert_eq!(
        discover_browser_with(&config, &env, std::slice::from_ref(&platform)),
        Ok(platform)
    );
}

#[test]
fn browser_path_names_have_stable_precedence_and_failure_lists_candidates() {
    let (root, loaded) = fixture();
    let names = [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
    ];
    for name in names {
        binary(&root.path().join(name));
    }
    let env = env_lookup(&[("PATH", root.path().as_os_str())]);
    for name in names {
        assert_eq!(
            discover_browser_with(&loaded.config, &env, &[]),
            Ok(root.path().join(name))
        );
        std::fs::remove_file(root.path().join(name))
            .unwrap_or_else(|err| panic!("remove fixture: {err}"));
    }
    let err = discover_browser_with(&loaded.config, &env, &[])
        .err()
        .unwrap_or_else(|| panic!("must fail"));
    assert_eq!(err.tried, names.map(|name| format!("PATH:{name}")));
}

#[cfg(unix)]
#[test]
fn path_discovery_rejects_non_executable_files_and_directories() {
    let (root, _loaded) = fixture();
    write(&root.path().join("not-executable"), "not executable");
    std::fs::create_dir(root.path().join("directory")).unwrap_or_else(|err| panic!("mkdir: {err}"));
    let env = env_lookup(&[("PATH", root.path().as_os_str())]);
    assert_eq!(find_on_path_with("not-executable", &env), None);
    assert_eq!(find_on_path_with("directory", &env), None);
}

fn agent_package(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|err| panic!("mkdir: {err}"));
    write(
        &dir.join("package.json"),
        r#"{"name":"openside-agent","version":"0.2.0"}"#,
    );
}

/// Executable that answers `--version` like Bun; a shell script, hence unix-only tests.
#[cfg(unix)]
fn fake_bun(path: &Path, version: &str) {
    write_executable(path, &format!("#!/bin/sh\necho {version}\n"));
}

#[cfg(unix)]
#[test]
fn agent_command_explicit_wins_and_default_runs_bun_on_main_ts() {
    let (root, loaded) = fixture();
    let mut config = loaded.config;
    let dir = root.path().join("agent");
    agent_package(&dir);
    config.agent.dir = Some(dir.clone());
    config.agent.command = Some(vec!["explicit-command".into(), "argument".into()]);
    let env = env_lookup(&[("PATH", root.path().as_os_str())]);
    assert_eq!(
        agent_command_with(&config, &env, None).argv,
        ["explicit-command", "argument"]
    );
    config.agent.command = None;
    let main = dir.join("src/main.ts").to_string_lossy().into_owned();
    let without_bun = agent_command_with(&config, &env, None);
    assert_eq!(
        without_bun.argv,
        ["bun".to_string(), "run".into(), main.clone()]
    );
    assert!(without_bun
        .problems
        .iter()
        .any(|problem| problem.contains("Bun executable bun not found")));
    fake_bun(&root.path().join("bun"), "1.4.2");
    let too_old = agent_command_with(&config, &env, None);
    assert_eq!(too_old.argv[0], "bun");
    assert!(
        too_old
            .problems
            .iter()
            .any(|problem| problem.contains("Bun 1.4.2")
                && problem.contains("older than the required 1.4.4")),
        "{:?}; direct exec: {:?}",
        too_old.problems,
        std::process::Command::new(root.path().join("bun"))
            .arg("--version")
            .output()
            .map(|output| output.status)
    );
    fake_bun(&root.path().join("bun"), "1.4.4");
    let with_bun = agent_command_with(&config, &env, None);
    assert_eq!(with_bun.argv, ["bun".to_string(), "run".into(), main]);
    assert!(with_bun.problems.is_empty(), "{:?}", with_bun.problems);
}

#[cfg(unix)]
#[test]
fn agent_bun_config_wins_over_openside_bun_env_over_path() {
    let (root, loaded) = fixture();
    let mut config = loaded.config;
    let dir = root.path().join("agent");
    agent_package(&dir);
    config.agent.dir = Some(dir.clone());
    let (configured, environment) = (
        root.path().join("configured-bun"),
        root.path().join("env-bun"),
    );
    fake_bun(&configured, "1.5.0");
    fake_bun(&environment, "1.4.5-canary.3+abc");
    let env = env_lookup(&[
        ("PATH", root.path().join("empty").as_os_str()),
        ("OPENSIDE_BUN", environment.as_os_str()),
    ]);
    let from_env = agent_command_with(&config, &env, None);
    assert_eq!(from_env.argv[0], environment.to_string_lossy());
    assert!(from_env.problems.is_empty());
    config.agent.bun = Some(configured.clone());
    let from_config = agent_command_with(&config, &env, None);
    assert_eq!(from_config.argv[0], configured.to_string_lossy());
    assert!(from_config.problems.is_empty());
    config.agent.bun = Some(root.path().join("missing-bun"));
    assert!(agent_command_with(&config, &env, None)
        .problems
        .iter()
        .any(|problem| problem.contains("missing-bun not found")));
    // load() maps OPENSIDE_BUN onto agent.bun, so file values lose to the environment.
    write(&loaded.paths.config_file, "[agent]\nbun = 'file-bun'\n");
    let parsed = load_with(Some(&loaded.paths.config_file), loaded.paths.clone(), &env)
        .unwrap_or_else(|err| panic!("load: {err}"));
    assert_eq!(parsed.config.agent.bun, Some(environment));
}

#[test]
fn agent_directory_search_prefers_env_then_installed_then_cargo_layout() {
    let (root, loaded) = fixture();
    let exe = root.path().join("target/debug/opensided");
    std::fs::create_dir_all(root.path().join("target/debug"))
        .unwrap_or_else(|err| panic!("mkdir: {err}"));
    let cargo_agent = root.path().join("agent");
    let installed_agent = root.path().join("target/share/openside/agent");
    for dir in [&cargo_agent, &installed_agent] {
        agent_package(dir);
    }
    let env = env_lookup(&[("PATH", root.path().as_os_str())]);
    let selected = agent_command_with(&loaded.config, &env, Some(&exe));
    assert_eq!(
        selected.agent_dir,
        Some(root.path().join("target/debug/../share/openside/agent"))
    );
    // A package.json with another name is not the sidecar project.
    write(
        &installed_agent.join("package.json"),
        r#"{"name":"some-other-package"}"#,
    );
    assert_eq!(
        agent_command_with(&loaded.config, &env, Some(&exe)).agent_dir,
        Some(root.path().join("target/debug/../../agent"))
    );
    let explicit_env = env_lookup(&[
        ("OPENSIDE_AGENT_DIR", cargo_agent.as_os_str()),
        ("PATH", root.path().as_os_str()),
    ]);
    assert_eq!(
        agent_command_with(&loaded.config, &explicit_env, Some(&exe)).agent_dir,
        Some(cargo_agent)
    );
    assert!(!agent_command_with(&loaded.config, &env, None)
        .problems
        .is_empty());
}

#[test]
fn channels_default_off_accept_numeric_ids_and_fail_closed_without_allow_lists() {
    let path = Path::new("config.toml");
    let default = parse_config(path, "").unwrap_or_else(|err| panic!("default: {err}"));
    assert!(default.channels.accounts.is_empty());
    assert_eq!(default.channels.token_env, "OPENSIDE_TOKEN");

    let numeric = parse_config(
        path,
        "[channels.telegram]\nenabled = true\ntoken_env = \"T\"\nallowed_chats = [-1001, \"42\"]\nallowed_users = [7]",
    )
    .unwrap_or_else(|err| panic!("numeric: {err}"));
    assert_eq!(
        numeric.channels.accounts[0].options.allowed_chats,
        ["-1001", "42"]
    );
    assert_eq!(numeric.channels.accounts[0].options.allowed_users, ["7"]);

    let (_root, loaded) = fixture();
    for (text, key) in [
        (
            "[channels.telegram]\nenabled = true\ntoken_env = \"T\"",
            "channels.telegram",
        ),
        ("[channels.slack]\nenabled = true", "channels.slack"),
        ("[channels.discord]\nenabled = true", "channels.discord"),
        (
            "[channels]\nedit_interval_ms = 10",
            "channels.edit_interval_ms",
        ),
        ("[channels]\nstt_cmd = []", "channels.stt_cmd"),
        (
            "[channels]\nstt_timeout_secs = 0",
            "channels.stt_timeout_secs",
        ),
        ("[channels]\ntoken_env = \" \"", "channels.token_env"),
    ] {
        write(&loaded.paths.config_file, text);
        let err = load_with(None, loaded.paths.clone(), &|_| None)
            .err()
            .unwrap_or_else(|| panic!("must reject: {text}"));
        assert!(err.key.starts_with(key), "{text}: got {}", err.key);
    }
    let unknown = parse_config(path, "[channels]\nfoo = 1")
        .err()
        .unwrap_or_else(|| panic!("unknown"));
    assert_eq!(unknown.key, "channels.foo");
    let unknown = parse_config(path, "[channels.slack]\nbogus = 1")
        .err()
        .unwrap_or_else(|| panic!("unknown"));
    assert_eq!(unknown.key, "channels.slack.bogus");
}

#[test]
fn channels_example_parses_with_every_adapter_disabled() {
    let config = parse_config(Path::new("config.toml"), &example_toml())
        .unwrap_or_else(|err| panic!("example: {err}"));
    assert!(config
        .channels
        .accounts
        .iter()
        .all(|account| !account.enabled));
}

#[test]
fn vault_unlock_auto_keeps_the_file_method_and_never_falls_back_silently() {
    use crate::{
        StoredVault as S, VaultBackend as B, VaultResolveError as E, VaultUnlockSetting as U,
    };
    let keyring = B::platform_keyring();
    let missing = U::Auto.resolve(S::Missing, true);
    assert_eq!(missing, Ok(keyring.unwrap_or(B::Passphrase)));
    assert_eq!(U::Auto.resolve(S::Missing, false), Ok(B::Passphrase));
    assert_eq!(U::Auto.resolve(S::Passphrase, true), Ok(B::Passphrase));
    assert_eq!(
        U::Auto.resolve(S::Keyring, false),
        keyring.ok_or(E::UnsupportedPlatform)
    );
    assert_eq!(
        U::Passphrase.resolve(S::Keyring, true),
        Err(E::FormatMismatch)
    );
    if let Some(keyring) = keyring {
        let explicit = if keyring == B::SecretService {
            U::SecretService
        } else {
            U::Native
        };
        assert_eq!(explicit.resolve(S::Missing, false), Ok(keyring));
        assert_eq!(
            explicit.resolve(S::Passphrase, true),
            Err(E::FormatMismatch)
        );
        let other = if keyring == B::SecretService {
            U::Native
        } else {
            U::SecretService
        };
        assert_eq!(other.resolve(S::Missing, true), Err(E::UnsupportedPlatform));
    }
    assert_eq!(
        toml::from_str::<crate::Config>("[vault]\nunlock = \"secret-service\"")
            .map(|parsed| parsed.vault.unlock),
        Ok(U::SecretService)
    );
    assert!(toml::from_str::<crate::Config>("[vault]\nunlock = \"passphrase-cmd\"").is_err());
    assert_eq!(crate::Config::default().vault.unlock, U::Auto);
}

#[test]
fn front_is_default_without_a_separate_model_and_single_remains_explicit() {
    let path = Path::new("config.toml");
    let default = parse_config(path, "").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(default.agent.orchestration, crate::Orchestration::Front);
    assert_eq!(default.models.front, None);
    let configured = parse_config(
        path,
        "[agent]\norchestration = \"single\"\n[models]\nfront = \"provider:fast\"",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(configured.agent.orchestration, crate::Orchestration::Single);
    assert_eq!(configured.models.front.as_deref(), Some("provider:fast"));
}

#[test]
fn team_concurrency_defaults_and_rejects_zero() {
    let default: Config = toml::from_str("").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(default.team.max_running, 3);
    assert_eq!(default.team.max_active_teammates, 5);
    let capped: Config = toml::from_str("[team]\nmax_active_teammates=2\n")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(capped.team.max_active_teammates, 2);
    let disabled: Config = toml::from_str("[team]\nmax_active_teammates=0\n")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(disabled.team.max_active_teammates, 0);
    assert!(toml::from_str::<Config>("[team]\nmax_active_teammates=-1\n").is_err());
    let configured: Config =
        toml::from_str("[team]\nmax_running=2\n").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(configured.team.max_running, 2);
    assert!(toml::from_str::<Config>("[team]\nmax_running=0\n").is_err());
    assert!(toml::from_str::<Config>("[team]\nmax_running=-1\n").is_err());
    let serialized = toml::to_string(&configured).unwrap_or_else(|error| panic!("{error}"));
    let reparsed = parse_config(Path::new("test-config.toml"), &serialized)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(reparsed.team.max_running, 2);
    assert!(parse_config(Path::new("test-config.toml"), "[team]\nunknown=2\n").is_err());
}

#[test]
fn vault_password_settings_parse_with_defaults_and_reject_bad_domains() {
    let path = Path::new("config.toml");
    let default = parse_config(path, "").unwrap_or_else(|err| panic!("default: {err}"));
    assert_eq!(default.vault.auto_lock, VaultAutoLock::OneWeek);
    assert!(!default.vault.biometric_unlock);
    assert!(default.vault.autofill.enabled && default.vault.autofill.auto_submit);
    assert!(!default.vault.autofill.allow_insecure);
    for (text, value, idle) in [
        ("immediately", VaultAutoLock::Immediately, Some(0)),
        ("one_minute", VaultAutoLock::OneMinute, Some(60)),
        ("five_minutes", VaultAutoLock::FiveMinutes, Some(300)),
        ("fifteen_minutes", VaultAutoLock::FifteenMinutes, Some(900)),
        ("one_hour", VaultAutoLock::OneHour, Some(3600)),
        ("four_hours", VaultAutoLock::FourHours, Some(4 * 3600)),
        ("one_day", VaultAutoLock::OneDay, Some(86400)),
        ("one_week", VaultAutoLock::OneWeek, Some(7 * 86400)),
        ("never", VaultAutoLock::Never, None),
    ] {
        let config = parse_config(path, &format!("[vault]\nauto_lock = \"{text}\""))
            .unwrap_or_else(|err| panic!("{text}: {err}"));
        assert_eq!(config.vault.auto_lock, value);
        assert_eq!(value.idle().map(|idle| idle.as_secs()), idle);
    }
    assert!(parse_config(path, "[vault]\nauto_lock = \"two_minutes\"").is_err());
    let tuned = parse_config(
        path,
        "[vault]\nbiometric_unlock = true\n[vault.autofill]\nenabled = false\nauto_submit = false\nallow_insecure = true\nexcluded_domains = [\"bank.example\"]",
    )
    .unwrap_or_else(|err| panic!("tuned: {err}"));
    assert!(tuned.vault.biometric_unlock);
    assert_eq!(tuned.vault.autofill.excluded_domains, ["bank.example"]);
    assert!(!tuned.vault.autofill.enabled && tuned.vault.autofill.allow_insecure);
    for bad in [
        "\"\"",
        "\"bank example\"",
        "\"https://user:pw@bank.example/\"",
        "\"intranet\"",
        "\"-bank.example\"",
        "\"10.0.0.1\"",
    ] {
        let parsed = parse_config(
            path,
            &format!("[vault.autofill]\nexcluded_domains = [{bad}]"),
        );
        assert!(
            parsed.and_then(|config| validate(&config, path)).is_err(),
            "{bad}"
        );
    }
    assert!(parse_config(path, "[vault.autofill]\nunknown = true").is_err());
}

#[test]
fn search_settings_accept_engine_templates_and_api_endpoints_only_over_http() {
    let path = Path::new("config.toml");
    let check = |text: &str| {
        let config = parse_config(path, text)?;
        validate(&config, path).map(|()| config)
    };
    let default = check("").unwrap_or_else(|err| panic!("default: {err}"));
    assert_eq!(default.search.provider, SearchProvider::Browser);
    assert!(default.search.engine.is_none() && default.search.api_url.is_none());
    let tuned = check(
        "[search]\nprovider = \"brave\"\nengine = \"https://search.example/?q={query}\"\napi_url = \"http://localhost:9000/v1\"",
    )
    .unwrap_or_else(|err| panic!("tuned: {err}"));
    assert_eq!(tuned.search.provider, SearchProvider::Brave);
    for bad in [
        "provider = \"google\"",
        "engine = \"https://search.example/?q=fixed\"",
        "engine = \"ftp://search.example/{query}\"",
        "engine = \"https://user:pw@search.example/?q={query}\"",
        "engine = \"{query}\"",
        "api_url = \"file:///etc/passwd\"",
        "api_url = \"https://key:secret@api.example/\"",
        "unknown = true",
    ] {
        assert!(check(&format!("[search]\n{bad}")).is_err(), "{bad}");
    }
}

#[test]
fn task_role_and_image_keys_round_trip_through_the_settings_file() {
    let path = Path::new("config.toml");
    let text = "[models]\ndefault = \"a:chat\"\nstandard = \"a:sub\"\ndeep = \"a:deep\"\nvisual = \"a:vision\"\nimage_off = true";
    let config = parse_document(path, text).unwrap_or_else(|err| panic!("roles: {err}"));
    assert_eq!(config.models.standard.as_deref(), Some("a:sub"));
    assert_eq!(config.models.visual.as_deref(), Some("a:vision"));
    assert!(config.models.image_off && config.models.image.is_none());
    // Settings writes go through serde and back through the same parser.
    let written = toml::to_string(&config).unwrap();
    let reread = parse_document(path, &written).unwrap_or_else(|err| panic!("reread: {err}"));
    assert!(reread.models.image_off);
    assert_eq!(reread.models.deep.as_deref(), Some("a:deep"));
    // Unset stays out of the file, so automatic is the default.
    let auto = parse_document(path, "[models]\nimage = \"a:picture\"").unwrap();
    assert!(!toml::to_string(&auto).unwrap().contains("image_off"));
    assert!(parse_document(path, "[models]\nimage = \"off\"").is_err());
}
