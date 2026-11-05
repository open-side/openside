use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::{env_value, Config, Paths};

/// Sidecar command, its project directory, and actionable discovery problems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    /// Executable followed by arguments; invoke directly without shell parsing.
    pub argv: Vec<String>,
    /// Discovered or configured sidecar project directory.
    pub agent_dir: Option<PathBuf>,
    /// Non-fatal setup problems for doctor to display.
    pub problems: Vec<String>,
}

/// No executable candidate was usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserNotFound {
    /// Candidates in the exact order attempted.
    pub tried: Vec<String>,
}

impl fmt::Display for BrowserNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no chromium-compatible browser found (tried {})",
            self.tried.join(", ")
        )
    }
}

impl std::error::Error for BrowserNotFound {}

pub(crate) fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Find an executable on PATH using the host platform's executable conventions.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    find_on_path_with(name, &|key| std::env::var_os(key))
}

pub(crate) fn find_on_path_with(
    name: &str,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    if Path::new(name).components().count() > 1 {
        return executable(Path::new(name)).then(|| PathBuf::from(name));
    }
    let path = env("PATH")?;
    for dir in std::env::split_paths(&path) {
        for candidate in executable_candidates(dir.join(name), env) {
            if executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn executable_candidates(
    path: PathBuf,
    _env: &impl Fn(&str) -> Option<OsString>,
) -> impl Iterator<Item = PathBuf> {
    std::iter::once(path)
}

#[cfg(windows)]
fn executable_candidates(path: PathBuf, env: &impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    if path.extension().is_some() {
        return vec![path];
    }
    let extensions = env("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let mut candidates = vec![path.clone()];
    candidates.extend(
        extensions
            .to_string_lossy()
            .split(';')
            .filter(|value| !value.is_empty())
            .map(|extension| PathBuf::from(format!("{}{}", path.display(), extension))),
    );
    candidates
}

/// Package name in `<agent dir>/package.json` that marks a sidecar project directory.
pub(crate) const AGENT_PACKAGE_NAME: &str = "openside-agent";

/// Resolve configured argv or the default `bun run <agent dir>/src/main.ts` invocation.
pub fn agent_command(config: &Config, _paths: &Paths) -> AgentCommand {
    agent_command_with(
        config,
        &|key| std::env::var_os(key),
        std::env::current_exe().ok().as_deref(),
    )
}

pub(crate) fn is_agent_dir(dir: &Path) -> bool {
    std::fs::read(dir.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|package| package["name"] == AGENT_PACKAGE_NAME)
}

/// Configured (`agent.bun` / `OPENSIDE_BUN`) Bun executable, else the bare `bun` name.
pub(crate) fn bun_program(config: &Config, env: &impl Fn(&str) -> Option<OsString>) -> PathBuf {
    config
        .agent
        .bun
        .clone()
        .or_else(|| env_value(env, "OPENSIDE_BUN").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("bun"))
}

/// Bare names resolve through PATH; anything with a directory component is used as given.
pub(crate) fn resolve_program(
    program: &Path,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    if program.components().count() > 1 {
        executable(program).then(|| program.to_path_buf())
    } else {
        find_on_path_with(&program.to_string_lossy(), env)
    }
}

/// Oldest Bun the sidecar accepts; a unit test keeps it equal to `engines.bun` of
/// `agent/package.json`.
pub(crate) const MIN_BUN: (u64, u64, u64) = (1, 4, 4);
/// [`MIN_BUN`] as `X.Y.Z`.
pub(crate) const MIN_BUN_TEXT: &str = "1.4.4";

pub(crate) fn parse_bun_version(text: &str) -> Option<(u64, u64, u64)> {
    // `bun --version` prints `1.4.4`; canary builds append `-canary.N+sha`.
    let core = text.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// `<bun> --version`, bounded to five seconds so a wedged executable cannot stall daemon
/// startup or doctor; `None` when it does not answer with a version.
pub(crate) fn bun_version(bun: &Path) -> Option<(u64, u64, u64)> {
    use std::io::Read;
    let mut child = std::process::Command::new(bun)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut stdout = String::new();
    child.stdout.take()?.read_to_string(&mut stdout).ok()?;
    status
        .success()
        .then(|| parse_bun_version(&stdout))
        .flatten()
}

pub(crate) fn agent_command_with(
    config: &Config,
    env: &impl Fn(&str) -> Option<OsString>,
    current_exe: Option<&Path>,
) -> AgentCommand {
    let mut agent_dir = config
        .agent
        .dir
        .clone()
        .or_else(|| env_value(env, "OPENSIDE_AGENT_DIR").map(PathBuf::from));
    if agent_dir.is_none() {
        if let Some(exe_dir) = current_exe.and_then(Path::parent) {
            agent_dir = [
                exe_dir.join("../share/openside/agent"),
                exe_dir.join("../../agent"),
            ]
            .into_iter()
            .find(|dir| is_agent_dir(dir));
        }
    }
    if let Some(argv) = &config.agent.command {
        return AgentCommand {
            argv: argv.clone(),
            agent_dir,
            problems: Vec::new(),
        };
    }
    let mut problems = Vec::new();
    let bun = bun_program(config, env);
    match resolve_program(&bun, env) {
        None => problems.push(format!(
            "Bun executable {} not found; install Bun {} or newer, or set agent.bun / OPENSIDE_BUN.",
            bun.display(),
            MIN_BUN_TEXT
        )),
        Some(path) => match bun_version(&path) {
            Some(version) if version >= MIN_BUN => {}
            Some((major, minor, patch)) => problems.push(format!(
                "Bun {major}.{minor}.{patch} at {} is older than the required {}; set agent.bun / OPENSIDE_BUN to a newer Bun.",
                path.display(),
                MIN_BUN_TEXT
            )),
            None => problems.push(format!(
                "{} --version did not report a Bun version; set agent.bun / OPENSIDE_BUN to Bun {} or newer.",
                path.display(),
                MIN_BUN_TEXT
            )),
        },
    }
    // Without a project directory the entry point is relative and only works from the agent
    // directory; the problem below tells doctor users how to fix that.
    let main = match &agent_dir {
        Some(dir) => dir.join("src/main.ts"),
        None => {
            problems.push("Agent project not found; set agent.dir or OPENSIDE_AGENT_DIR to the directory whose package.json is named openside-agent, run `bun install --frozen-lockfile --production` there (docs/INSTALL.md), or set agent.command.".into());
            PathBuf::from("src/main.ts")
        }
    };
    AgentCommand {
        argv: vec![
            bun.to_string_lossy().into_owned(),
            "run".into(),
            main.to_string_lossy().into_owned(),
        ],
        agent_dir,
        problems,
    }
}

/// Search configured, environment, bundle, PATH, then platform browser locations.
pub fn discover_browser(config: &Config) -> Result<PathBuf, BrowserNotFound> {
    let env = |key: &str| std::env::var_os(key);
    discover_browser_with(config, &env, &platform_browser_locations(&env))
}

pub(crate) fn discover_browser_with(
    config: &Config,
    env: &impl Fn(&str) -> Option<OsString>,
    platform_locations: &[PathBuf],
) -> Result<PathBuf, BrowserNotFound> {
    let mut tried = Vec::new();
    let mut try_path = |path: PathBuf| {
        tried.push(path.to_string_lossy().into_owned());
        executable(&path).then_some(path)
    };
    for path in [
        config.browser.executable.clone(),
        env_value(env, "OPENSIDE_BROWSER").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(found) = try_path(path) {
            return Ok(found);
        }
    }
    if let Some(dir) = &config.browser.bundle_dir {
        for name in ["chrome", "chromium"] {
            for candidate in executable_candidates(dir.join(name), env) {
                if let Some(found) = try_path(candidate) {
                    return Ok(found);
                }
            }
        }
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
    ] {
        tried.push(format!("PATH:{name}"));
        if let Some(found) = find_on_path_with(name, env) {
            return Ok(found);
        }
    }
    for path in platform_locations {
        tried.push(path.to_string_lossy().into_owned());
        if executable(path) {
            return Ok(path.clone());
        }
    }
    Err(BrowserNotFound { tried })
}

pub(crate) fn platform_browser_locations(_env: &impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        ]
    }
    #[cfg(windows)]
    {
        ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"]
            .into_iter()
            .filter_map(|key| env_value(_env, key).map(PathBuf::from))
            .flat_map(|dir| {
                [
                    dir.join("Google/Chrome/Application/chrome.exe"),
                    dir.join("Chromium/Application/chrome.exe"),
                ]
            })
            .collect()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Vec::new()
    }
}
