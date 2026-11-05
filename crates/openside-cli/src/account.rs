//! `openside account list|status|use`: CLI accounts are the daemon's team agents.
//!
//! `u0` is the chief and `u1`, `u2`, ... are the active teammates in roster order; an agent id
//! works too. A session started with an account runs as that agent (`agent_id` on
//! `POST /v1/sessions`). Without one, a session is an ordinary conversation, which is what
//! `u0` means when nothing was chosen.

use std::io::Write;

use clap::Subcommand;
use openside_proto::{AgentList, AgentStatus, AgentSummary};
use serde_json::json;

use crate::{
    client::Client,
    emit, output_error,
    settings::{self, AccountRef, AccountSelector},
    ui::clean,
};

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// List local openside accounts.
    List,
    /// Show the active openside account.
    Status {
        /// Account to show instead of the active one.
        #[arg(value_name = "id")]
        id: Option<String>,
    },
    /// Set the default openside CLI account.
    Use {
        #[arg(value_name = "id")]
        id: String,
    },
}

/// Active agents in roster order (the daemon lists the chief first).
async fn roster(client: &Client) -> Result<Vec<AgentSummary>, String> {
    let list: AgentList = client.get("/v1/agents").await?;
    Ok(list
        .agents
        .into_iter()
        .filter(|row| row.agent.status == AgentStatus::Active)
        .collect())
}

fn find<'a>(
    rows: &'a [AgentSummary],
    account: &AccountRef,
) -> Result<(usize, &'a AgentSummary), String> {
    let found = match account {
        AccountRef::Index(index) => rows.get(*index).map(|row| (*index, row)),
        AccountRef::Id(id) => rows.iter().enumerate().find(|(_, row)| row.agent.id == *id),
    };
    found.ok_or_else(|| {
        let name = match account {
            AccountRef::Index(index) => format!("u{index}"),
            AccountRef::Id(id) => clean(id),
        };
        format!("no account {name}; see `openside account list`")
    })
}

/// The agent id an account selector names, checked against the daemon's roster.
pub(crate) async fn resolve(client: &Client, selector: &AccountSelector) -> Result<String, String> {
    let rows = roster(client).await?;
    let (_, row) = find(&rows, &selector.account)?;
    Ok(row.agent.id.clone())
}

pub(crate) async fn execute<W: Write>(
    command: &AccountCommand,
    client: &Client,
    json_output: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let mut saved = settings::load()?;
    let rows = roster(client).await?;
    let current = match saved.account.as_deref() {
        Some(id) => rows.iter().position(|row| row.agent.id == id),
        None => Some(0),
    };
    match command {
        AccountCommand::List => {
            if json_output {
                let accounts: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        json!({
                            "selector": format!("u{index}"), "id": row.agent.id,
                            "name": row.agent.name, "current": current == Some(index),
                        })
                    })
                    .collect();
                return emit(stdout, &json!({ "accounts": accounts }));
            }
            for (index, row) in rows.iter().enumerate() {
                let mark = if current == Some(index) { '*' } else { ' ' };
                writeln!(
                    stdout,
                    "{mark} u{index}\t{}\t{}",
                    clean(&row.agent.id),
                    clean(&row.agent.name)
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        AccountCommand::Status { id } => {
            let (index, row) = match id {
                Some(id) => find(&rows, &AccountSelector::parse(id)?.account)?,
                None => {
                    let index = current.ok_or_else(|| {
                        "the default account is no longer on the team; choose one with `openside account use <id>`".to_owned()
                    })?;
                    find(&rows, &AccountRef::Index(index))?
                }
            };
            let agent = &row.agent;
            if json_output {
                return emit(
                    stdout,
                    &json!({
                        "selector": format!("u{index}"), "current": current == Some(index),
                        "id": agent.id, "name": agent.name, "kind": agent.kind,
                        "model": agent.model, "mode": agent.mode,
                        "home_session_id": agent.home_session_id,
                        "session_status": row.session_status,
                    }),
                );
            }
            writeln!(
                stdout,
                "u{index}\t{}\t{}\nmodel\t{}\nhome session\t{}",
                clean(&agent.id),
                clean(&agent.name),
                agent.model.as_deref().map_or("default".into(), clean),
                agent.home_session_id,
            )
            .map_err(output_error)
        }
        AccountCommand::Use { id } => {
            let selector = AccountSelector::parse(id)?;
            let (index, row) = find(&rows, &selector.account)?;
            saved.account = Some(row.agent.id.clone());
            if selector.profile.is_some() {
                saved.profile = selector.profile;
            }
            settings::save(&saved)?;
            if json_output {
                return emit(
                    stdout,
                    &json!({"selector": format!("u{index}"), "id": row.agent.id}),
                );
            }
            writeln!(
                stdout,
                "Default account: u{index} ({})",
                clean(&row.agent.name)
            )
            .map_err(output_error)
        }
    }
}
