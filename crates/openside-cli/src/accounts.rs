use std::io::Write;

use clap::Subcommand;
use openside_proto::{AccountsResponse, SubscriptionAccount};
use serde_json::json;

use crate::{client::Client, emit, output_error};
#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// Create an isolated CLI home and print the external sign-in command.
    Add {
        source: String,
        #[arg(long)]
        label: String,
    },
    /// Show login readiness and the exact external sign-in command for this account.
    Show { source: String, account_id: String },
    /// Enable one existing subscription account.
    Enable { source: String, account_id: String },
    /// Disable one existing subscription account.
    Disable { source: String, account_id: String },
    /// Remove an openside account; never delete the default user CLI home.
    Rm { source: String, account_id: String },
}

pub(crate) async fn run<W: Write>(
    command: Option<&AccountCommand>,
    client: &Client,
    json_output: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let Some(command) = command else {
        return list(client, json_output, stdout).await;
    };
    let (source, id) = match command {
        AccountCommand::Add { source, .. } => (source, None),
        AccountCommand::Show { source, account_id }
        | AccountCommand::Enable { source, account_id }
        | AccountCommand::Disable { source, account_id }
        | AccountCommand::Rm { source, account_id } => (source, Some(account_id)),
    };
    if !matches!(source.as_str(), "claude-code" | "codex-cli") {
        return Err("source must be claude-code or codex-cli".into());
    }
    let mut path = format!("/v1/settings/subscriptions/{source}");
    if let Some(id) = id {
        path.push('/');
        path.push_str(crate::models::component(id)?);
    }
    let account: SubscriptionAccount = match command {
        AccountCommand::Add { label, .. } => {
            client
                .json("POST", &path, Some(&json!({"label":label})))
                .await?
        }
        AccountCommand::Show { .. } => client.get(&path).await?,
        AccountCommand::Enable { .. } => {
            client
                .json("PATCH", &path, Some(&json!({"enabled":true})))
                .await?
        }
        AccountCommand::Disable { .. } => {
            client
                .json("PATCH", &path, Some(&json!({"enabled":false})))
                .await?
        }
        AccountCommand::Rm { .. } => {
            client.send("DELETE", &path).await?;
            if !json_output {
                writeln!(stdout, "Subscription account removed").map_err(output_error)?;
            }
            return Ok(());
        }
    };
    if json_output {
        emit(stdout, &account)
    } else {
        writeln!(
            stdout,
            "{}\t{}\t{}\tinstalled={}\tsigned_in={}\tenabled={}\n{}\nLog in: {}",
            account.source,
            account.account_id,
            account.label,
            account.installed,
            account.logged_in,
            account.enabled,
            account.reason,
            account.login_command
        )
        .map_err(output_error)
    }
}

async fn list<W: Write>(client: &Client, json: bool, stdout: &mut W) -> Result<(), String> {
    let response: AccountsResponse = client.get("/v1/accounts").await?;
    if json {
        return emit(stdout, &response);
    }
    if let Some(ui) = crate::ui::pretty() {
        return write!(
            stdout,
            "{}",
            crate::ui::accounts(ui, &response, crate::ui::now_secs())
        )
        .map_err(output_error);
    }
    writeln!(
        stdout,
        "SOURCE\tACCOUNT\tPROVIDER\tMODELS\tUSAGE\tBLOCKED UNTIL (MS)"
    )
    .map_err(output_error)?;
    for source in &response.sources {
        if source.accounts.is_empty() {
            writeln!(stdout, "{}\t{}\t-\t-\t-\t-", source.name, source.reason)
                .map_err(output_error)?;
        }
        for account in &source.accounts {
            let usage = account
                .usage_windows
                .iter()
                .map(|window| match window.used_percent {
                    Some(used) => format!("{}: {used:.1}%", window.name),
                    None => format!("{}: unknown", window.name),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let blocked = account
                .blocked_until_ms
                .map(|value| format!("{value:.0}"))
                .unwrap_or_else(|| "-".into());
            writeln!(
                stdout,
                "{}\t{}\t{}\t{}\t{}\t{}",
                source.name,
                account.label,
                account.provider,
                account.models.join(", "),
                if usage.is_empty() { "-" } else { &usage },
                blocked
            )
            .map_err(output_error)?;
        }
    }
    if response.truncated {
        writeln!(
            stdout,
            "Account metadata truncated at the documented limits"
        )
        .map_err(output_error)?;
    }
    Ok(())
}
