use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::{env_value, Config, Paths};

/// Sidecar command, its project directory, and actionable discovery problems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCommand {
    /// Executable followed by arguments; invoke directly without shell parsing.
    pub argv: Vec<String>,
    /// Discovered or configured Python project directory.
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

/// Resolve configured argv or a portable uv/Python module invocation.
pub fn agent_command(config: &Config, _paths: &Paths) -> AgentCommand {
    agent_command_with(
        config,
        &|key| std::env::var_os(key),
        std::env::current_exe().ok().as_deref(),
    )
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
            .find(|dir| dir.join("pyproject.toml").is_file());
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
    if agent_dir.is_none() {
        problems.push("Agent project not found; set agent.dir or OPENSIDE_AGENT_DIR, or install openside_agent and set agent.command.".into());
    }
    let argv = if find_on_path_with("uv", env).is_some() {
        if let Some(dir) = &agent_dir {
            vec![
                "uv".into(),
                "run".into(),
                "--project".into(),
                dir.to_string_lossy().into_owned(),
                "python".into(),
                "-m".into(),
                "openside_agent".into(),
            ]
        } else {
            problems.push("Cannot build uv project command without agent.dir; using the installed Python module candidate.".into());
            vec!["python3".into(), "-m".into(), "openside_agent".into()]
        }
    } else {
        problems.push("uv not found on PATH; install uv or set agent.command to a Python environment with openside_agent installed.".into());
        vec!["python3".into(), "-m".into(), "openside_agent".into()]
    };
    AgentCommand {
        argv,
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
