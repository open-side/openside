use std::io::Write;

use clap::{Subcommand, ValueEnum};
use openside_proto::{
    Connection, ConnectionAccess, ConnectionAuditAction, ConnectionAuditList, ConnectionGrant,
    ConnectionList, ConnectionStatus, CreateAccountSpaceRequest, CreateAccountSpaceResult,
    ReloginConnectionRequest, ReloginResult, RevokeConnectionRequest, UpdateConnectionRequest,
};

use crate::{client::Client, emit, output_error, plain, Summary};

#[derive(Subcommand)]
pub(crate) enum ConnectionsCommand {
    /// List sign-ins and separately marked candidates (metadata only).
    List {
        /// Exact registrable site domain, filtered without changing the API object shape.
        #[arg(long)]
        site: Option<String>,
    },
    /// Show a connection and its explicit per-agent grants.
    Show { id: String },
    /// Check whether the browser is still signed in.
    Check { id: String },
    /// Sign in again, using the Vault or an existing human-step ticket.
    Relogin { id: String },
    /// Remove access and sign out every shared-profile account on the site.
    Revoke {
        id: String,
        /// Forget metadata and grants without clearing the shared browser's sign-in.
        #[arg(long)]
        keep_signed_in: bool,
    },
    /// Set a stable agent ID's connection access; ordinary action policy still applies.
    Grant {
        id: String,
        agent: String,
        #[arg(value_enum)]
        access: AccessArg,
    },
    /// Show newest-first metadata audit records.
    Audit {
        #[arg(long)]
        connection: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, value_enum)]
        action: Option<ActionArg>,
    },
    /// Manage opt-in separate account profiles.
    Spaces {
        #[command(subcommand)]
        command: SpacesCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum SpacesCommand {
    /// Create a separate account profile and start its sign-in flow.
    Add { site: String },
    /// Stop a separate account profile and delete its sign-in and profile directory.
    Remove { space_id: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum AccessArg {
    Allowed,
    Ask,
    Denied,
}

impl From<AccessArg> for ConnectionAccess {
    fn from(access: AccessArg) -> Self {
        match access {
            AccessArg::Allowed => Self::Allowed,
            AccessArg::Ask => Self::Ask,
            AccessArg::Denied => Self::Denied,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum ActionArg {
    Detected,
    Verified,
    Expired,
    NeedsLogin,
    Relogin,
    Open,
    Fetch,
    Grant,
    Forget,
    SignOut,
}

impl From<ActionArg> for ConnectionAuditAction {
    fn from(action: ActionArg) -> Self {
        match action {
            ActionArg::Detected => Self::Detected,
            ActionArg::Verified => Self::Verified,
            ActionArg::Expired => Self::Expired,
            ActionArg::NeedsLogin => Self::NeedsLogin,
            ActionArg::Relogin => Self::Relogin,
            ActionArg::Open => Self::Open,
            ActionArg::Fetch => Self::Fetch,
            ActionArg::Grant => Self::Grant,
            ActionArg::Forget => Self::Forget,
            ActionArg::SignOut => Self::SignOut,
        }
    }
}

fn action_name(action: ConnectionAuditAction) -> &'static str {
    match action {
        ConnectionAuditAction::Detected => "detected",
        ConnectionAuditAction::Verified => "verified",
        ConnectionAuditAction::Expired => "expired",
        ConnectionAuditAction::NeedsLogin => "needs_login",
        ConnectionAuditAction::Relogin => "relogin",
        ConnectionAuditAction::Open => "open",
        ConnectionAuditAction::Fetch => "fetch",
        ConnectionAuditAction::Grant => "grant",
        ConnectionAuditAction::Forget => "forget",
        ConnectionAuditAction::SignOut => "sign_out",
    }
}

fn path(id: &str, suffix: &str) -> String {
    format!(
        "/v1/connections/{}{suffix}",
        crate::client::encode_segment(id)
    )
}

fn print_connection<W: Write>(stdout: &mut W, connection: &Connection) -> Result<(), String> {
    let status = match connection.status {
        ConnectionStatus::Candidate => "candidate",
        ConnectionStatus::Unverified => "unverified",
        ConnectionStatus::Active => "active",
        ConnectionStatus::Expired => "expired",
        ConnectionStatus::NeedsLogin => "needs_login",
    };
    writeln!(
        stdout,
        "{}\t{}\t{status}\t{}",
        connection.id,
        plain(&connection.site),
        plain(
            connection
                .identity
                .as_deref()
                .unwrap_or("Account not identified yet")
        )
    )
    .map_err(output_error)?;
    for grant in &connection.grants {
        let access = match grant.access {
            ConnectionAccess::Allowed => "allowed",
            ConnectionAccess::Ask => "ask",
            ConnectionAccess::Denied => "denied",
        };
        writeln!(stdout, "  {}\t{access}", plain(&grant.agent_id)).map_err(output_error)?;
    }
    Ok(())
}

fn print_relogin<W: Write, E: Write>(
    result: &ReloginResult,
    json: bool,
    client: &Client,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    match result {
        ReloginResult::SignedIn if !json => writeln!(stdout, "Signed in").map_err(output_error),
        ReloginResult::NeedsYou { ticket } => {
            summary.session_id = Some(ticket.session_id.to_string());
            summary.status = "waiting_human";
            writeln!(
                stderr,
                "Ticket {} in session {}\nContinue with: {}",
                ticket.ticket_id,
                ticket.session_id,
                client.ticket_command(ticket.ticket_id.as_str(), "resolve", None)
            )
            .map_err(output_error)
        }
        _ => Ok(()),
    }
}

pub(crate) async fn execute<W: Write, E: Write>(
    command: &ConnectionsCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    match command {
        ConnectionsCommand::List { site } => {
            let mut list: ConnectionList = client.get("/v1/connections").await?;
            // The Admin list route has no site query; retain its candidate separation locally.
            if let Some(site) = site {
                list.connections
                    .retain(|connection| connection.site == *site);
                list.candidates
                    .retain(|connection| connection.site == *site);
            }
            if json {
                return emit(stdout, &list);
            }
            if !list.browser_available {
                writeln!(stdout, "Agent browser unavailable").map_err(output_error)?;
            }
            for connection in &list.connections {
                print_connection(stdout, connection)?;
            }
            if !list.candidates.is_empty() {
                writeln!(stdout, "Possibly signed in").map_err(output_error)?;
                for connection in &list.candidates {
                    print_connection(stdout, connection)?;
                }
            }
            Ok(())
        }
        ConnectionsCommand::Show { id } | ConnectionsCommand::Check { id } => {
            let connection: Connection = if matches!(command, ConnectionsCommand::Check { .. }) {
                client
                    .json::<_, serde_json::Value>("POST", &path(id, "/check"), None)
                    .await?
            } else {
                client.get(&path(id, "")).await?
            };
            if json {
                emit(stdout, &connection)
            } else {
                print_connection(stdout, &connection)
            }
        }
        ConnectionsCommand::Grant { id, agent, access } => {
            let connection: Connection = client
                .json(
                    "PATCH",
                    &path(id, ""),
                    Some(&UpdateConnectionRequest {
                        grants: Some(vec![ConnectionGrant {
                            agent_id: agent.clone(),
                            access: (*access).into(),
                        }]),
                        home_url: None,
                    }),
                )
                .await?;
            if json {
                emit(stdout, &connection)
            } else {
                print_connection(stdout, &connection)
            }
        }
        ConnectionsCommand::Relogin { id } => {
            let result: ReloginResult = client
                .json(
                    "POST",
                    &path(id, "/relogin"),
                    Some(&ReloginConnectionRequest::default()),
                )
                .await?;
            if json {
                emit(stdout, &result)?;
            }
            print_relogin(&result, json, client, stdout, stderr, summary)
        }
        ConnectionsCommand::Revoke { id, keep_signed_in } => {
            client
                .send_json(
                    "POST",
                    &path(id, "/revoke"),
                    &RevokeConnectionRequest {
                        sign_out: !keep_signed_in,
                        delete_vault_item: false,
                    },
                )
                .await?;
            if json {
                return Ok(());
            }
            writeln!(stdout, "Revoked {id}").map_err(output_error)
        }
        ConnectionsCommand::Audit {
            connection,
            agent,
            action,
        } => {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in [("connection", connection), ("agent", agent)] {
                if let Some(value) = value {
                    params.append_pair(key, value);
                }
            }
            if let Some(action) = action {
                params.append_pair("action", action_name((*action).into()));
            }
            let list: ConnectionAuditList = client
                .get(&format!("/v1/connections/audit?{}", params.finish()))
                .await?;
            if json {
                return emit(stdout, &list);
            }
            for record in &list.records {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    record.at.to_rfc3339(),
                    record.connection_id,
                    plain(&record.site),
                    plain(&record.agent_id),
                    action_name(record.action),
                    plain(record.detail.as_deref().unwrap_or(""))
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        ConnectionsCommand::Spaces {
            command: SpacesCommand::Add { site },
        } => {
            let space: CreateAccountSpaceResult = client
                .json(
                    "POST",
                    "/v1/connections/spaces",
                    Some(&CreateAccountSpaceRequest { site: site.clone() }),
                )
                .await?;
            if json {
                emit(stdout, &space)?;
            } else {
                writeln!(stdout, "Created account space {}", space.space_id)
                    .map_err(output_error)?;
            }
            print_relogin(&space.result, json, client, stdout, stderr, summary)
        }
        ConnectionsCommand::Spaces {
            command: SpacesCommand::Remove { space_id },
        } => {
            client
                .send(
                    "DELETE",
                    &format!(
                        "/v1/connections/spaces/{}",
                        crate::client::encode_segment(space_id)
                    ),
                )
                .await?;
            if json {
                return Ok(());
            }
            writeln!(stdout, "Removed account space {space_id}").map_err(output_error)
        }
    }
}
