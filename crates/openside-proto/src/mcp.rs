use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How openside reaches an external MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpServerTransport {
    /// A local process speaking JSON-RPC over stdin/stdout.
    Stdio,
    /// The Streamable HTTP transport (JSON or server-sent-event responses).
    Http,
}

/// Connection state of a configured MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpServerStatus {
    /// Turned off; nothing is started.
    Disabled,
    /// Starting or initializing.
    Connecting,
    /// Initialized; `tools` is current.
    Connected,
    /// Its secrets are in the Vault, which is locked.
    VaultLocked,
    /// Start, handshake or tool listing failed; see `error`.
    Failed,
}

/// One tool an MCP server offers, as agents see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerTool {
    /// The server's own tool name.
    pub name: String,
    /// Name agents call: `mcp__<server id>__<tool>`, reduced to `[A-Za-z0-9_-]`, at most 64.
    pub exposed_name: String,
    /// The server's description, if any.
    pub description: Option<String>,
    /// The server marks the tool as read-only. A hint only: every call still follows the
    /// session's approval rules.
    pub read_only_hint: bool,
}

/// A configured MCP server. Secret values are never returned; `secrets` lists their names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServer {
    /// Stable id: lowercase letters, digits, `-`, `_`.
    pub id: String,
    /// Display name.
    pub label: String,
    /// Disabled servers are kept but not started.
    pub enabled: bool,
    /// Transport.
    pub transport: McpServerTransport,
    /// stdio: program and arguments.
    pub command: Option<Vec<String>>,
    /// stdio: working directory.
    pub cwd: Option<String>,
    /// http: endpoint.
    pub url: Option<String>,
    /// http: private, CGNAT and unique-local addresses are reachable too.
    pub allow_private_network: bool,
    /// Names of Vault-held environment variables (stdio) or headers (http).
    pub secrets: Vec<String>,
    /// Connection state.
    pub status: McpServerStatus,
    /// Why the last start, handshake or listing failed; redacted, no secret values.
    pub error: Option<String>,
    /// Name the server reported during the handshake.
    pub server_name: Option<String>,
    /// Tools agents can call; empty unless `status` is `connected`.
    pub tools: Vec<McpServerTool>,
}

/// Response of `GET /v1/mcp/servers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerList {
    /// Servers in configuration order.
    pub servers: Vec<McpServer>,
}

/// `POST /v1/mcp/servers` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMcpServerRequest {
    /// Stable id; derived from `label` when absent.
    #[serde(default)]
    pub id: Option<String>,
    /// Display name, 1-80 characters.
    pub label: String,
    /// Transport.
    pub transport: McpServerTransport,
    /// stdio: program and arguments, run without a shell.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// stdio: absolute working directory.
    #[serde(default)]
    pub cwd: Option<String>,
    /// http: `https://` endpoint, or `http://` on a loopback host.
    #[serde(default)]
    pub url: Option<String>,
    /// http: also reach private (10/8, 172.16/12, 192.168/16), CGNAT (100.64/10) and
    /// unique-local IPv6 addresses; link-local and cloud metadata stay refused. Default false.
    #[serde(default)]
    pub allow_private_network: bool,
    /// Environment variables (stdio) or headers (http) to store in the Vault, by name.
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    /// Defaults to true.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `PATCH /v1/mcp/servers/{id}` body; absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMcpServerRequest {
    /// Display name.
    pub label: Option<String>,
    /// Start or stop the server.
    pub enabled: Option<bool>,
    /// stdio: replacement program and arguments.
    pub command: Option<Vec<String>>,
    /// stdio: replacement working directory; an empty string clears it.
    pub cwd: Option<String>,
    /// http: replacement endpoint.
    pub url: Option<String>,
    /// http: allow or refuse private, CGNAT and unique-local addresses.
    pub allow_private_network: Option<bool>,
    /// Secrets to set (a value) or remove (null), by name; others are unchanged.
    pub secrets: Option<BTreeMap<String, Option<String>>>,
}
