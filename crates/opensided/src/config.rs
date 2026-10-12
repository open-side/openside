use std::{net::SocketAddr, path::PathBuf};

use clap::Parser;
use openside_config::{agent_command, find_on_path, Loaded, PolicyModeSetting};
use openside_proto::PolicyMode;

use crate::persistence::Result;

/// Command-line overrides for the portable openside configuration.
#[derive(Clone, Debug, Default, Parser)]
#[command(name = "opensided")]
pub struct Config {
    /// Explicit TOML configuration file.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Directory containing durable session, capability and audit records.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Private Unix socket path.
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// Optional loopback TCP listener; port zero selects an available port.
    #[arg(long)]
    pub listen: Option<SocketAddr>,
}

#[derive(Clone)]
pub(crate) struct Resolved {
    pub(crate) loaded: Loaded,
    pub(crate) data_dir: PathBuf,
    pub(crate) socket: PathBuf,
    pub(crate) listen: Option<SocketAddr>,
    pub(crate) argv: Vec<String>,
    pub(crate) bwrap: Option<PathBuf>,
}

impl Config {
    pub(crate) fn resolve(&self) -> Result<Resolved> {
        let loaded = openside_config::load(self.config.as_deref())?;
        self.resolve_loaded(loaded)
    }

    pub(crate) fn resolve_loaded(&self, loaded: Loaded) -> Result<Resolved> {
        let data_dir = self
            .data_dir
            .clone()
            .unwrap_or_else(|| loaded.paths.data_dir.clone());
        let socket = self.socket.clone().unwrap_or_else(|| loaded.socket_path());
        let listen = self.listen.or(loaded.config.daemon.listen);
        if listen.is_some_and(|address| !address.ip().is_loopback()) {
            return Err("TCP listener must be loopback".into());
        }
        let command = agent_command(&loaded.config, &loaded.paths);
        for notice in loaded.warnings.iter().chain(&command.problems) {
            crate::diag!("opensided: {notice}");
        }
        // Browser tools are registered separately; discovery never launches a browser.
        let _ = openside_config::discover_browser(&loaded.config);
        let bwrap = match &loaded.config.sandbox.bwrap {
            Some(path) => path.is_file().then(|| path.clone()),
            None => find_on_path("bwrap"),
        };
        Ok(Resolved {
            loaded,
            data_dir,
            socket,
            listen,
            argv: command.argv,
            bwrap,
        })
    }
}

impl Resolved {
    pub(crate) fn policy_mode(&self) -> PolicyMode {
        match self.loaded.config.policy.mode {
            PolicyModeSetting::ReadOnly => PolicyMode::ReadOnly,
            PolicyModeSetting::Guard => PolicyMode::Guard,
            PolicyModeSetting::Full => PolicyMode::Full,
        }
    }
}
