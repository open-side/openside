use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Most servers a person can connect; each one is a process or a remote connection.
pub const MAX_MCP_SERVERS: usize = 32;

/// External MCP servers whose tools agents can call (`docs/specs/mcp.md` §MCP client).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct McpConfig {
    /// Connected servers, in display order.
    pub servers: Vec<McpServerConfig>,
}

/// How openside reaches an MCP server.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// A local process speaking JSON-RPC over stdin/stdout.
    Stdio,
    /// The Streamable HTTP transport (JSON or SSE responses) at `url`.
    Http,
}

/// One server, without credential values: `secrets` lists names whose values live in the
/// Vault and are injected as environment variables (stdio) or headers (http).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// Stable id: lowercase letters, digits, `-`, `_`; at most 32 characters. Tool names
    /// shown to the model are `mcp__<id>__<tool>`.
    pub id: String,
    /// Name shown in Settings.
    pub label: String,
    /// Disabled servers keep their settings and secrets but are not started.
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Transport.
    pub transport: McpTransport,
    /// stdio: program and arguments, run without a shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// stdio: working directory (default: the daemon's data directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// http: endpoint; `https://`, or `http://` on a loopback host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// http: also allow private (10/8, 172.16/12, 192.168/16), shared CGNAT (100.64/10, used
    /// by Tailscale) and unique-local IPv6 addresses. Link-local, cloud metadata and other
    /// special-purpose addresses stay refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_private_network: bool,
    /// Names of Vault-held values: environment variables (stdio) or HTTP headers (http).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
}

fn enabled() -> bool {
    true
}

/// Server id syntax shared by config validation and the API.
pub fn valid_mcp_server_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Header or environment variable name syntax for a server secret.
pub fn valid_mcp_secret_name(transport: McpTransport, name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && match transport {
            McpTransport::Stdio => {
                !name.as_bytes()[0].is_ascii_digit()
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            }
            McpTransport::Http => name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        }
}

/// Remote endpoints must use TLS unless they never leave the machine.
pub fn valid_mcp_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    // Credentials belong in Vault-held headers: the URL is shown to every settings reader.
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return false;
    }
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => match url.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

impl McpServerConfig {
    /// First problem as `(key, reason)`, with `index` naming the array position.
    pub fn validate(&self, index: usize) -> Result<(), (String, &'static str)> {
        let key = |field: &str| format!("mcp.servers[{index}].{field}");
        if !valid_mcp_server_id(&self.id) {
            return Err((
                key("id"),
                "use 1-32 lowercase letters, digits, hyphens or underscores",
            ));
        }
        if self.label.trim().is_empty()
            || self.label.chars().count() > 80
            || self.label.chars().any(char::is_control)
        {
            return Err((
                key("label"),
                "use a name of 1-80 characters without control characters",
            ));
        }
        match self.transport {
            McpTransport::Stdio => {
                let argv_ok = self.command.as_ref().is_some_and(|argv| {
                    !argv.is_empty()
                        && !argv[0].trim().is_empty()
                        && argv.len() <= 64
                        && argv
                            .iter()
                            .all(|arg| arg.len() <= 4096 && !arg.contains('\0'))
                });
                if !argv_ok {
                    return Err((key("command"), "stdio servers need a program to run"));
                }
                if self.url.is_some() {
                    return Err((key("url"), "stdio servers take a command, not a URL"));
                }
                if self.allow_private_network {
                    return Err((
                        key("allow_private_network"),
                        "only http servers connect over a network",
                    ));
                }
                if self.cwd.as_ref().is_some_and(|cwd| !cwd.is_absolute()) {
                    return Err((key("cwd"), "use an absolute directory"));
                }
            }
            McpTransport::Http => {
                if !self.url.as_deref().is_some_and(valid_mcp_url) {
                    return Err((key("url"), "use an https:// URL, or http:// on localhost"));
                }
                if self.command.is_some() || self.cwd.is_some() {
                    return Err((key("command"), "http servers take a URL, not a command"));
                }
            }
        }
        if self.secrets.len() > 32 {
            return Err((key("secrets"), "at most 32 secrets per server"));
        }
        for (position, name) in self.secrets.iter().enumerate() {
            if !valid_mcp_secret_name(self.transport, name) {
                return Err((
                    key("secrets"),
                    "use environment variable names (stdio) or header names (http)",
                ));
            }
            if self.secrets[..position]
                .iter()
                .any(|other| other.eq_ignore_ascii_case(name))
            {
                return Err((key("secrets"), "secret names must be unique"));
            }
        }
        Ok(())
    }
}

impl McpConfig {
    /// Reject invalid servers, duplicate ids and too many servers.
    pub(crate) fn validate(&self) -> Result<(), (String, &'static str)> {
        if self.servers.len() > MAX_MCP_SERVERS {
            return Err(("mcp.servers".into(), "at most 32 servers"));
        }
        for (index, server) in self.servers.iter().enumerate() {
            server.validate(index)?;
            if self.servers[..index]
                .iter()
                .any(|other| other.id == server.id)
            {
                return Err((
                    format!("mcp.servers[{index}].id"),
                    "server ids must be unique",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{valid_mcp_url, McpConfig};

    #[test]
    fn private_network_opt_in_is_http_only_and_omitted_when_off() {
        let config: McpConfig = toml::from_str(
            r#"[[servers]]
id = "lan"
label = "LAN"
transport = "http"
url = "https://nas.lan/mcp"
allow_private_network = true

[[servers]]
id = "public"
label = "Public"
transport = "http"
url = "https://mcp.example.com/mcp"
"#,
        )
        .unwrap();
        assert!(config.validate().is_ok());
        assert!(config.servers[0].allow_private_network);
        assert!(!config.servers[1].allow_private_network);
        let written = toml::to_string(&config).unwrap();
        assert_eq!(
            written.matches("allow_private_network").count(),
            1,
            "{written}"
        );

        let stdio: McpConfig = toml::from_str(
            r#"[[servers]]
id = "local"
label = "Local"
transport = "stdio"
command = ["server"]
allow_private_network = true
"#,
        )
        .unwrap();
        assert_eq!(
            stdio.validate().unwrap_err().0,
            "mcp.servers[0].allow_private_network"
        );
    }

    #[test]
    fn urls_carry_no_credentials_and_leave_the_machine_only_over_tls() {
        for ok in [
            "https://mcp.example.com/mcp",
            "http://127.0.0.1:3845/mcp",
            "http://localhost:9/mcp",
            "http://[::1]:9/mcp",
        ] {
            assert!(valid_mcp_url(ok), "{ok}");
        }
        for bad in [
            "https://mcp.example.com/mcp?api_key=secret",
            "https://user:pass@mcp.example.com/mcp",
            "https://mcp.example.com/mcp#frag",
            "http://mcp.example.com/mcp",
            "http://192.168.1.2/mcp",
            "ftp://mcp.example.com/",
        ] {
            assert!(!valid_mcp_url(bad), "{bad}");
        }
    }
}
