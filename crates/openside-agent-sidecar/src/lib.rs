//! Supervised agent sidecars over the versioned, daemon-owned JSONL protocol.
//!
//! [`Connection`] can use any asynchronous duplex transport. On Unix,
//! [`SidecarRuntime`] additionally owns a private socket and a child process group.
//! Session queues, policy decisions, tool execution, and crash recovery stay in the daemon.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::PathBuf;
use std::time::Duration;

mod connection;
mod credential_rpc;
mod framing;
#[cfg(unix)]
mod process;
mod protocol;
mod version;

pub use connection::{open_wire, Connection};
#[cfg(unix)]
pub use process::SidecarSession;
pub use version::{
    supported_agent_versions, MAX_AGENT_VERSION_EXCLUSIVE, MIN_AGENT_VERSION, PROTOCOL_VERSION,
};

/// Maximum JSON payload bytes in one incoming or outgoing line, excluding newline.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Reserves the rest of the wire frame for base64 attachments and envelope metadata.
pub const MAX_USER_TEXT_BYTES: usize = MAX_LINE_BYTES / 4;

/// Portable launch settings; no executable or machine-specific path is baked in.
#[derive(Clone)]
pub struct SidecarConfig {
    /// Executable followed by arguments, resolved by the configuration layer.
    pub argv: Vec<String>,
    /// Parent directory for per-session private socket directories.
    pub runtime_dir: PathBuf,
    /// Deadline for process spawning, hello authentication, and ready.
    pub startup_timeout: Duration,
    /// Complete approved runtime environment; the child never inherits the daemon environment.
    pub env: Vec<(String, String)>,
}

/// Factory for one supervised process per agent session.
#[derive(Clone)]
pub struct SidecarRuntime {
    config: SidecarConfig,
}

impl SidecarRuntime {
    /// Construct a runtime from already resolved configuration.
    pub fn new(config: SidecarConfig) -> Self {
        Self { config }
    }
}

#[cfg(not(unix))]
impl openside_core::AgentRuntime for SidecarRuntime {
    type Session = Connection;

    async fn open_session(
        &self,
        _spec: openside_core::SessionSpec,
        _tools: std::sync::Arc<dyn openside_core::HostToolHandler>,
    ) -> Result<Self::Session, openside_core::RuntimeError> {
        let _ = &self.config;
        Err(openside_core::RuntimeError::Unavailable(
            "Unix sidecar transport is unsupported on this platform".into(),
        ))
    }
}

#[cfg(all(test, unix))]
mod tests;
#[cfg(test)]
mod wire_contract_tests;
