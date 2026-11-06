//! `/v1/mcp/servers`: add, change and remove external MCP servers. Mutations need local
//! access, because a stdio server runs a program on this computer and an http server
//! receives credentials.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_config::{
    valid_mcp_server_id, McpConfig, McpServerConfig, McpTransport, MAX_MCP_SERVERS,
};
use openside_proto::{
    CreateMcpServerRequest, McpServer, McpServerList, McpServerStatus, McpServerTransport,
    UpdateMcpServerRequest,
};
use openside_vault::{AuditContext, Entry, Error, Secret};

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    remote::LocalAuthority,
};

fn invalid() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_value")
}

fn unavailable() -> HttpError {
    HttpError(StatusCode::CONFLICT, "vault_unavailable")
}

fn not_found() -> HttpError {
    HttpError(StatusCode::NOT_FOUND, "mcp_server_not_found")
}

fn context(who: &str) -> AuditContext<'_> {
    AuditContext {
        who,
        session_id: None,
        tool_call_id: None,
    }
}

fn transport(kind: McpServerTransport) -> McpTransport {
    match kind {
        McpServerTransport::Stdio => McpTransport::Stdio,
        McpServerTransport::Http => McpTransport::Http,
    }
}

pub(crate) fn view(s: &Shared, config: &McpServerConfig) -> McpServer {
    let state = s.mcp.state(&config.id);
    let status = match &state {
        Some(state) => state.status,
        None if config.enabled => McpServerStatus::Connecting,
        None => McpServerStatus::Disabled,
    };
    McpServer {
        id: config.id.clone(),
        label: config.label.clone(),
        enabled: config.enabled,
        transport: match config.transport {
            McpTransport::Stdio => McpServerTransport::Stdio,
            McpTransport::Http => McpServerTransport::Http,
        },
        command: config.command.clone(),
        cwd: config.cwd.as_ref().map(|cwd| cwd.display().to_string()),
        url: config.url.clone(),
        allow_private_network: config.allow_private_network,
        secrets: config.secrets.clone(),
        status,
        error: state.as_ref().and_then(|state| state.error.clone()),
        server_name: state
            .as_ref()
            .and_then(|state| state.client.as_ref())
            .and_then(|client| client.server_name.clone()),
        tools: state.map(|state| state.tools).unwrap_or_default(),
    }
}

/// An id from the label: lowercase ASCII letters and digits joined by `-`, made unique.
fn derive_id(label: &str, taken: &McpConfig) -> String {
    let mut base = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    let mut base = base.trim_end_matches('-').to_owned();
    base.truncate(28);
    if base.is_empty() {
        base = "server".into();
    }
    let mut id = base.clone();
    let mut n = 2;
    while taken.servers.iter().any(|server| server.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

async fn persist(s: &Shared, config: McpConfig) -> Result<(), HttpError> {
    let mut stored = crate::settings_api::stored(s).await?;
    stored.mcp = config.clone();
    crate::settings_api::persist(
        s,
        serde_json::to_value(stored).map_err(|_| HttpError::internal())?,
    )
    .await?;
    s.mcp.set_config(config);
    Ok(())
}

/// Store secret values, setting up a keyring-backed Vault when none exists yet.
async fn save_secrets(
    s: &Shared,
    c: &Caller,
    server: &McpServerConfig,
    values: Vec<(String, String)>,
) -> Result<(), HttpError> {
    if values.is_empty() {
        return Ok(());
    }
    let now =
        u64::try_from(chrono::Utc::now().timestamp_millis()).map_err(|_| HttpError::internal())?;
    let entries = values
        .into_iter()
        .map(|(name, value)| Entry::mcp_secret(&server.id, &name, Secret::new(value), now))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid())?;
    let service = &s.registry.vault;
    let mut guard = service.vault.lock().await;
    if guard.is_none() {
        let probe = crate::tools::vault::Service::probe(s).await;
        if !probe.backend.is_ok_and(|b| b.is_keyring()) {
            return Err(unavailable());
        }
        service
            .setup(s, &mut guard, c.id(), None, false)
            .await
            .map_err(|error| match error {
                (403, "biometric_required") => crate::vault_api::lifecycle(error),
                _ => unavailable(),
            })?;
    }
    let vault = guard.as_mut().ok_or_else(unavailable)?;
    s.registry.vault.touch(s);
    for entry in entries {
        // Mask before saving, so no outcome after the write can echo the value.
        s.registry.redactors.write().push(entry.redactor());
        vault
            .set_mcp_secret(&context(c.id()), entry)
            .map_err(|error| match error {
                Error::Locked => unavailable(),
                Error::ItemExists => HttpError(StatusCode::CONFLICT, "mcp_secret_conflict"),
                _ => HttpError::internal(),
            })?;
    }
    Ok(())
}

async fn delete_secrets(
    s: &Shared,
    c: &Caller,
    server: &str,
    names: &[String],
) -> Result<(), HttpError> {
    if names.is_empty() {
        return Ok(());
    }
    let mut guard = s.registry.vault.vault.lock().await;
    let vault = guard.as_mut().ok_or_else(unavailable)?;
    s.registry.vault.touch(s);
    for name in names {
        match vault.delete_mcp_secret(&context(c.id()), server, name) {
            Ok(()) | Err(Error::UnknownItem) => {}
            Err(Error::Locked) => return Err(unavailable()),
            Err(_) => return Err(HttpError::internal()),
        }
    }
    Ok(())
}

fn cwd(value: Option<String>) -> Option<std::path::PathBuf> {
    value
        .filter(|cwd| !cwd.is_empty())
        .map(std::path::PathBuf::from)
}

pub(crate) async fn list(State(s): State<Shared>) -> Json<McpServerList> {
    let config = s.mcp.config();
    Json(McpServerList {
        servers: config
            .servers
            .iter()
            .map(|server| view(&s, server))
            .collect(),
    })
}

pub(crate) async fn get(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<McpServer>, HttpError> {
    let config = s.mcp.config();
    let server = config
        .servers
        .iter()
        .find(|server| server.id == id)
        .ok_or_else(not_found)?;
    Ok(Json(view(&s, server)))
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    Json(request): Json<CreateMcpServerRequest>,
) -> Result<(StatusCode, Json<McpServer>), HttpError> {
    local.require()?;
    // Detached: a dropped request must not leave the Vault and config.toml half-written.
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        let mut config = s.mcp.config();
        if config.servers.len() >= MAX_MCP_SERVERS {
            return Err(HttpError(StatusCode::CONFLICT, "mcp_server_limit"));
        }
        let id = match request.id {
            Some(id) if !valid_mcp_server_id(&id) => return Err(invalid()),
            Some(id) => id,
            None => derive_id(&request.label, &config),
        };
        if config.servers.iter().any(|server| server.id == id) {
            return Err(HttpError(StatusCode::CONFLICT, "mcp_server_exists"));
        }
        let server = McpServerConfig {
            id,
            label: request.label.trim().to_owned(),
            enabled: request.enabled.unwrap_or(true),
            transport: transport(request.transport),
            command: request.command,
            cwd: cwd(request.cwd),
            url: request.url,
            allow_private_network: request.allow_private_network,
            secrets: request.secrets.keys().cloned().collect(),
        };
        server
            .validate(config.servers.len())
            .map_err(|_| invalid())?;
        save_secrets(&s, &c, &server, request.secrets.into_iter().collect()).await?;
        config.servers.push(server.clone());
        persist(&s, config).await?;
        s.mcp.reconnect(&s, &server.id).await;
        Ok((StatusCode::CREATED, Json(view(&s, &server))))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    Path(id): Path<String>,
    Json(request): Json<UpdateMcpServerRequest>,
) -> Result<Json<McpServer>, HttpError> {
    local.require()?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        let mut config = s.mcp.config();
        let index = config
            .servers
            .iter()
            .position(|server| server.id == id)
            .ok_or_else(not_found)?;
        let server = &mut config.servers[index];
        if let Some(label) = request.label {
            server.label = label.trim().to_owned();
        }
        if let Some(enabled) = request.enabled {
            server.enabled = enabled;
        }
        if let Some(command) = request.command {
            server.command = Some(command);
        }
        if let Some(value) = request.cwd {
            server.cwd = cwd(Some(value));
        }
        if let Some(url) = request.url {
            server.url = Some(url);
        }
        if let Some(allow) = request.allow_private_network {
            server.allow_private_network = allow;
        }
        let mut set = Vec::new();
        let mut removed = Vec::new();
        for (name, value) in request.secrets.unwrap_or_default() {
            let known = server
                .secrets
                .iter()
                .position(|known| known.eq_ignore_ascii_case(&name));
            match (value, known) {
                (Some(value), Some(_)) => set.push((name, value)),
                (Some(value), None) => {
                    server.secrets.push(name.clone());
                    set.push((name, value));
                }
                (None, Some(position)) => removed.push(server.secrets.remove(position)),
                (None, None) => {}
            }
        }
        server.validate(index).map_err(|_| invalid())?;
        let server = server.clone();
        save_secrets(&s, &c, &server, set).await?;
        delete_secrets(&s, &c, &server.id, &removed).await?;
        persist(&s, config).await?;
        s.mcp.reconnect(&s, &server.id).await;
        Ok(Json(view(&s, &server)))
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn delete(
    State(s): State<Shared>,
    Extension(c): Extension<Caller>,
    Extension(local): Extension<LocalAuthority>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    local.require()?;
    tokio::spawn(async move {
        let _writer = crate::settings_api::WRITE.lock().await;
        let mut config = s.mcp.config();
        let index = config
            .servers
            .iter()
            .position(|server| server.id == id)
            .ok_or_else(not_found)?;
        let server = config.servers.remove(index);
        s.mcp.remove(&s, &server.id).await;
        delete_secrets(&s, &c, &server.id, &server.secrets).await?;
        persist(&s, config).await?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

pub(crate) async fn refresh(
    State(s): State<Shared>,
    // Refreshing a stdio server runs its program with its secrets, like creating one.
    Extension(local): Extension<LocalAuthority>,
    Path(id): Path<String>,
) -> Result<Json<McpServer>, HttpError> {
    local.require()?;
    let config = s.mcp.config();
    let server = config
        .servers
        .iter()
        .find(|server| server.id == id)
        .ok_or_else(not_found)?
        .clone();
    s.mcp.reconnect(&s, &server.id).await;
    Ok(Json(view(&s, &server)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_are_valid_and_unique() {
        let mut config = McpConfig::default();
        assert_eq!(derive_id("Figma (local)", &config), "figma-local");
        assert_eq!(derive_id("…", &config), "server");
        config.servers.push(McpServerConfig {
            id: "figma-local".into(),
            label: "x".into(),
            enabled: true,
            transport: McpTransport::Http,
            command: None,
            cwd: None,
            url: Some("http://127.0.0.1:3845/mcp".into()),
            allow_private_network: false,
            secrets: Vec::new(),
        });
        assert_eq!(derive_id("Figma local", &config), "figma-local-2");
        assert!(valid_mcp_server_id(&derive_id(&"A".repeat(80), &config)));
    }
}
