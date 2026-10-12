use std::ffi::OsString;
use std::path::PathBuf;

use crate::env_value;

/// Resolved application locations; discovery does not create any directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// Default or environment-selected configuration file.
    pub config_file: PathBuf,
    /// Application data directory.
    pub data_dir: PathBuf,
    /// Persistent state directory.
    pub state_dir: PathBuf,
    /// Ephemeral sockets and process coordination directory.
    pub runtime_dir: PathBuf,
}

impl Paths {
    /// Resolve XDG variables, then platform directory conventions.
    pub fn discover() -> Self {
        Self::discover_with(&|key| std::env::var_os(key))
    }

    fn discover_with(env: &impl Fn(&str) -> Option<OsString>) -> Self {
        let config_home = env_value(env, "XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(dirs::config_dir)
            .unwrap_or_else(std::env::temp_dir);
        let data_home = env_value(env, "XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(dirs::data_dir)
            .unwrap_or_else(std::env::temp_dir);
        let data_dir = data_home.join("openside");
        let state_dir = env_value(env, "XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(dirs::state_dir)
            .map(|dir| dir.join("openside"))
            .unwrap_or_else(|| data_dir.clone());
        let runtime_dir = env_value(env, "XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(dirs::runtime_dir)
            .map(|dir| dir.join("openside"))
            .unwrap_or_else(runtime_fallback);
        let config_file = env_value(env, "OPENSIDE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| config_home.join("openside/config.toml"));
        Self {
            config_file,
            data_dir,
            state_dir,
            runtime_dir,
        }
    }
}

#[cfg(unix)]
fn runtime_fallback() -> PathBuf {
    std::env::temp_dir().join(format!("openside-{}", nix::unistd::getuid()))
}

#[cfg(not(unix))]
fn runtime_fallback() -> PathBuf {
    std::env::temp_dir().join("openside")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_and_explicit_config_are_resolved_without_process_env_mutation() {
        let root = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let env = |key: &str| Some(root.path().join(key).into_os_string());
        let paths = Paths::discover_with(&env);
        assert_eq!(paths.config_file, root.path().join("OPENSIDE_CONFIG"));
        assert_eq!(paths.data_dir, root.path().join("XDG_DATA_HOME/openside"));
        assert_eq!(paths.state_dir, root.path().join("XDG_STATE_HOME/openside"));
        assert_eq!(
            paths.runtime_dir,
            root.path().join("XDG_RUNTIME_DIR/openside")
        );
    }
}
