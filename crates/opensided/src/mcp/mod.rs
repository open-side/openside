//! External MCP servers as a tool source (`docs/specs/mcp.md` §MCP client). The daemon,
//! not the agent sidecar, holds the connections: secrets stay in the daemon, and every call
//! goes through `host::execute` (policy, approval tickets, redaction, events, audit) like a
//! built-in tool.

pub(crate) mod api;
pub(crate) mod client;

use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    time::Duration,
};

use futures_util::future::BoxFuture;
use openside_config::{McpConfig, McpServerConfig};
use openside_core::{ApprovalTier, ToolSpec};
use openside_policy::{Action, Capability};
use openside_proto::{McpServerStatus, McpServerTool};
use openside_vault::AuditContext;
use serde_json::{json, Map, Value};

use crate::{
    engine::Shared,
    tools::{DaemonTool, ToolEnv, ToolError, ToolExec},
};
use client::{Client, ClientError};

/// Prefix of every tool name an MCP server contributes.
pub(crate) const TOOL_PREFIX: &str = "mcp__";
/// Prefix of the policy target of an MCP call: `mcp:<server id>/<tool>`.
pub(crate) const ACTION_PREFIX: &str = "mcp:";
const MAX_TOOL_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 2048;
const MAX_SCHEMA: usize = 64 * 1024;
const VAULT_RETRY: Duration = Duration::from_secs(30);

/// Connection state of one server.
#[derive(Clone)]
pub(crate) struct ServerState {
    pub(crate) status: McpServerStatus,
    pub(crate) error: Option<String>,
    pub(crate) client: Option<Arc<Client>>,
    pub(crate) tools: Vec<McpServerTool>,
    /// Input schema (JSON text) by the server's tool name.
    schemas: BTreeMap<String, String>,
}

impl ServerState {
    fn new(status: McpServerStatus) -> Self {
        Self {
            status,
            error: None,
            client: None,
            tools: Vec::new(),
            schemas: BTreeMap::new(),
        }
    }
}

/// Live MCP configuration and connections.
pub(crate) struct Hub {
    config: parking_lot::RwLock<McpConfig>,
    states: parking_lot::RwLock<BTreeMap<String, ServerState>>,
    /// Serializes connects so two refreshes of one server never race their clients.
    connect: tokio::sync::Mutex<()>,
}

impl Hub {
    pub(crate) fn new(config: &McpConfig) -> Self {
        Self {
            config: parking_lot::RwLock::new(config.clone()),
            states: parking_lot::RwLock::default(),
            connect: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn config(&self) -> McpConfig {
        self.config.read().clone()
    }

    pub(crate) fn set_config(&self, config: McpConfig) {
        *self.config.write() = config;
    }

    pub(crate) fn state(&self, id: &str) -> Option<ServerState> {
        self.states.read().get(id).cloned()
    }

    /// Connect one server (stopping its previous connection) and publish the tool set.
    pub(crate) async fn reconnect(&self, shared: &Shared, id: &str) {
        let _serial = self.connect.lock().await;
        let Some(config) = self.config().servers.into_iter().find(|s| s.id == id) else {
            self.states.write().remove(id);
            self.publish(shared);
            return;
        };
        if let Some(old) = self.states.write().remove(id) {
            drop(old);
        }
        self.publish(shared);
        let state = if config.enabled {
            self.states
                .write()
                .insert(id.to_owned(), ServerState::new(McpServerStatus::Connecting));
            connect(shared, &config).await
        } else {
            ServerState::new(McpServerStatus::Disabled)
        };
        if let Some(client) = &state.client {
            watch(shared, id, client);
        }
        self.states.write().insert(id.to_owned(), state);
        self.publish(shared);
    }

    /// Stop and forget a server.
    pub(crate) async fn remove(&self, shared: &Shared, id: &str) {
        let _serial = self.connect.lock().await;
        self.states.write().remove(id);
        self.publish(shared);
    }

    /// Stop every server (daemon shutdown).
    pub(crate) fn stop_all(&self, shared: &Shared) {
        self.states.write().clear();
        self.publish(shared);
    }

    /// Rebuild the registry's MCP tools from connected servers, in configuration order;
    /// a name already taken keeps its first owner.
    fn publish(&self, shared: &Shared) {
        let config = self.config();
        let states = self.states.read();
        let mut tools: BTreeMap<String, Arc<dyn DaemonTool>> = BTreeMap::new();
        for server in &config.servers {
            let Some(state) = states.get(&server.id) else {
                continue;
            };
            let Some(client) = &state.client else {
                continue;
            };
            for tool in &state.tools {
                if tools.contains_key(&tool.exposed_name) {
                    continue;
                }
                if let Some(schema) = state.schemas.get(&tool.name) {
                    let built = McpTool::new(server, tool, schema.clone(), client.clone());
                    tools.insert(tool.exposed_name.clone(), Arc::new(built));
                }
            }
        }
        drop(states);
        shared.registry.set_mcp_tools(tools);
    }
}

/// Connect enabled servers in the background, retry the ones waiting for the Vault, and
/// stop them all on shutdown.
pub(crate) fn start(shared: &Shared) {
    let shared = shared.clone();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let ids: Vec<String> = shared
            .mcp
            .config()
            .servers
            .into_iter()
            .map(|server| server.id)
            .collect();
        for id in &ids {
            shared.mcp.reconnect(&shared, id).await;
        }
        let mut tick = tokio::time::interval(VAULT_RETRY);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    shared.mcp.stop_all(&shared);
                    break;
                }
                _ = tick.tick() => {
                    let waiting: Vec<String> = shared
                        .mcp
                        .states
                        .read()
                        .iter()
                        .filter(|(_, state)| state.status == McpServerStatus::VaultLocked)
                        .map(|(id, _)| id.clone())
                        .collect();
                    if waiting.is_empty() || shared.registry.vault.vault.lock().await.is_none() {
                        continue;
                    }
                    for id in waiting {
                        shared.mcp.reconnect(&shared, &id).await;
                    }
                }
            }
        }
    });
}

/// Relist tools when the server says they changed, and mark the server failed when its
/// process exits.
fn watch(shared: &Shared, id: &str, client: &Arc<Client>) {
    let shared = shared.clone();
    let id = id.to_owned();
    let weak: Weak<Client> = Arc::downgrade(client);
    let changed = client.tools_changed.clone();
    tokio::spawn(async move {
        loop {
            let notified = changed.notified();
            let exited = async {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    match weak.upgrade() {
                        Some(client) if !client.is_closed() => {}
                        _ => break,
                    }
                }
            };
            tokio::select! {
                () = notified => {}
                () = exited => {
                    let Some(client) = weak.upgrade() else { return };
                    let _serial = shared.mcp.connect.lock().await;
                    let mut states = shared.mcp.states.write();
                    if let Some(state) = states.get_mut(&id) {
                        if state.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client)) {
                            *state = ServerState {
                                error: Some("server exited".into()),
                                ..ServerState::new(McpServerStatus::Failed)
                            };
                        }
                    }
                    drop(states);
                    shared.mcp.publish(&shared);
                    return;
                }
            }
            let Some(client) = weak.upgrade() else { return };
            // A failed relist keeps the change pending: retry until it lands or the client goes.
            let listed = loop {
                match client.list_tools().await {
                    Ok(listed) => break listed,
                    Err(_) if client.is_closed() => return,
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        let current = shared.mcp.states.read().get(&id).is_some_and(|state| {
                            state
                                .client
                                .as_ref()
                                .is_some_and(|c| Arc::ptr_eq(c, &client))
                        });
                        if !current {
                            return;
                        }
                    }
                }
            };
            let Some(config) = shared.mcp.config().servers.into_iter().find(|s| s.id == id) else {
                return;
            };
            let _serial = shared.mcp.connect.lock().await;
            {
                let mut states = shared.mcp.states.write();
                let Some(state) = states.get_mut(&id) else {
                    return;
                };
                if !state
                    .client
                    .as_ref()
                    .is_some_and(|c| Arc::ptr_eq(c, &client))
                {
                    return;
                }
                (state.tools, state.schemas) = describe(&shared, &config, &listed);
            }
            shared.mcp.publish(&shared);
        }
    });
}

fn audit() -> AuditContext<'static> {
    AuditContext {
        who: "opensided",
        session_id: None,
        tool_call_id: None,
    }
}

async fn connect(shared: &Shared, config: &McpServerConfig) -> ServerState {
    let secrets = if config.secrets.is_empty() {
        Vec::new()
    } else {
        let guard = shared.registry.vault.vault.lock().await;
        let Some(vault) = guard.as_ref() else {
            return ServerState::new(McpServerStatus::VaultLocked);
        };
        let found = vault.with_mcp_secrets(&audit(), &config.id, |pairs| {
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>()
        });
        match found {
            Ok(found) => found,
            Err(openside_vault::Error::Locked) => {
                return ServerState::new(McpServerStatus::VaultLocked)
            }
            Err(_) => return failed(shared, "could not read the server's secrets"),
        }
    };
    if let Some(missing) = config.secrets.iter().find(|name| {
        !secrets
            .iter()
            .any(|(stored, _)| stored.eq_ignore_ascii_case(name))
    }) {
        return failed(shared, &format!("secret {missing} is not set"));
    }
    let attempt = async {
        let client = Client::connect(config, secrets, &shared.config.data_dir).await?;
        let listed = client.list_tools().await?;
        Ok::<_, ClientError>((client, listed))
    };
    match attempt.await {
        Ok((mut client, listed)) => {
            // The server could have read its own secrets; nothing it reports is kept unmasked.
            client.server_name = client
                .server_name
                .take()
                .and_then(|name| shared.registry.redact(Value::String(name)).ok())
                .and_then(|name| name.as_str().map(single_line));
            let (tools, schemas) = describe(shared, config, &listed);
            ServerState {
                status: McpServerStatus::Connected,
                error: None,
                tools,
                schemas,
                client: Some(Arc::new(client)),
            }
        }
        Err(error) => failed(shared, &error.0),
    }
}

fn failed(shared: &Shared, message: &str) -> ServerState {
    let message = match shared.registry.redact(Value::String(message.to_owned())) {
        Ok(Value::String(message)) => message,
        _ => "connection failed".into(),
    };
    ServerState {
        error: Some(message),
        ..ServerState::new(McpServerStatus::Failed)
    }
}

/// `mcp__<server>__<tool>` reduced to the characters model APIs accept, at most 64.
pub(crate) fn exposed_name(server: &str, tool: &str) -> String {
    let mut name: String = format!("{TOOL_PREFIX}{server}__{tool}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    name.truncate(MAX_TOOL_NAME);
    name
}

fn describe(
    shared: &Shared,
    config: &McpServerConfig,
    listed: &[Value],
) -> (Vec<McpServerTool>, BTreeMap<String, String>) {
    let mut tools: Vec<McpServerTool> = Vec::new();
    let mut schemas = BTreeMap::new();
    // Names, descriptions and schemas are server-controlled and reach the API, the model and
    // approval prompts, so they are masked before anything is kept.
    let Ok(Value::Array(listed)) = shared.registry.redact(Value::Array(listed.to_vec())) else {
        return (tools, schemas);
    };
    for tool in &listed {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(schema) = tool_schema(tool) else {
            continue;
        };
        // The MCP tool-name alphabet; anything else (controls, `/`, spaces) could forge
        // policy targets or approval text.
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            continue;
        }
        let exposed = exposed_name(&config.id, name);
        if tools.iter().any(|known| known.exposed_name == exposed) {
            continue;
        }
        schemas.insert(name.to_owned(), schema);
        tools.push(McpServerTool {
            name: name.to_owned(),
            exposed_name: exposed,
            description: tool.get("description").and_then(Value::as_str).map(|text| {
                truncated(text, MAX_DESCRIPTION)
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect()
            }),
            read_only_hint: tool
                .pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    (tools, schemas)
}

/// Server-supplied text shown in the API, UI and approval prompts: one line, no controls.
fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn truncated(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

/// The tool's input schema when it is a bounded object schema.
fn tool_schema(tool: &Value) -> Option<String> {
    let schema = tool.get("inputSchema")?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    let text = schema.to_string();
    (text.len() <= MAX_SCHEMA).then_some(text)
}

/// One tool of a connected server, exposed to agents.
pub(crate) struct McpTool {
    exposed: String,
    server: String,
    label: String,
    tool: String,
    spec: ToolSpec,
    client: Arc<Client>,
}

impl McpTool {
    fn new(
        config: &McpServerConfig,
        tool: &McpServerTool,
        schema: String,
        client: Arc<Client>,
    ) -> Self {
        let description = format!(
            "[MCP: {}] {}",
            config.label,
            tool.description.as_deref().unwrap_or(&tool.name)
        );
        Self {
            exposed: tool.exposed_name.clone(),
            server: config.id.clone(),
            label: config.label.clone(),
            tool: tool.name.clone(),
            spec: ToolSpec {
                name: tool.exposed_name.clone(),
                description: truncated(&description, MAX_DESCRIPTION),
                parameters_json: schema,
                // Server-side effects are unknown; readOnlyHint is the server's claim, not a
                // guarantee, so every call is treated like running a program.
                approval: ApprovalTier::Exec,
                deferrable: false,
            },
            client,
        }
    }
}

impl DaemonTool for McpTool {
    fn name(&self) -> &str {
        &self.exposed
    }
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }
    fn visible(&self, _ctx: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        _args: &'a Value,
        _ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        let mut action = Action::new(
            Capability::Exec,
            format!("{ACTION_PREFIX}{}/{}", self.server, self.tool),
        );
        action.label = Some(self.label.clone());
        Box::pin(async move { Ok(vec![action]) })
    }
    fn redact_args(&self, args: &Value) -> Value {
        args.clone()
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let arguments = if args.is_null() {
                Value::Object(Map::new())
            } else {
                args
            };
            match self
                .client
                .call_tool(&self.tool, arguments, &ctx.cancellation)
                .await
            {
                Ok(result) => self.result(result),
                Err(error) => ToolExec::Failed(format!("MCP server {}: {error}", self.label)),
            }
        })
    }
}

impl McpTool {
    fn result(&self, result: Value) -> ToolExec {
        let mut text = Vec::new();
        let mut parts = Vec::new();
        for item in result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(value) = item.get("text").and_then(Value::as_str) {
                        text.push(value.to_owned());
                    }
                }
                Some("image") => match image(item) {
                    Some(part) => parts.push(part),
                    None => text.push("[image omitted]".into()),
                },
                _ => text.push(item.to_string()),
            }
        }
        let content = text.join("\n");
        let is_error = result.get("isError").and_then(Value::as_bool) == Some(true);
        // Server output is data from a third party, never instructions for the agent.
        let mut for_model = json!({
            "source": "mcp",
            "server": self.server,
            "tool": self.tool,
            "untrusted": true,
            "authority": "data_only",
            "content": content,
        });
        if let Some(structured) = result.get("structuredContent") {
            for_model["structured"] = structured.clone();
        }
        if is_error {
            // An error report is server output too: same envelope, failed status.
            for_model["is_error"] = json!(true);
            return ToolExec::Failed(for_model.to_string());
        }
        if parts.is_empty() {
            ToolExec::Done {
                for_model,
                for_app: None,
            }
        } else {
            ToolExec::DoneWithParts {
                for_model,
                for_app: None,
                parts,
            }
        }
    }
}

fn image(item: &Value) -> Option<openside_core::ContentPart> {
    use base64::Engine as _;
    let mime = item.get("mimeType").and_then(Value::as_str)?;
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return None;
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(item.get("data").and_then(Value::as_str)?)
        .ok()?;
    (data.len() <= crate::artifacts::MODEL_IMAGE_MAX_BYTES).then(|| openside_core::ContentPart {
        kind: openside_core::ContentKind::Image,
        mime: mime.to_owned(),
        data: Some(data),
        artifact_ref: None,
        name: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposed_names_are_model_safe_and_bounded() {
        assert_eq!(exposed_name("files", "read.file"), "mcp__files__read_file");
        assert_eq!(exposed_name("x", &"a".repeat(100)).len(), 64);
        assert!(exposed_name("x", "über tool").starts_with("mcp__x___ber_tool"));
    }
}
