use crate::{client::Client, emit, output_error, prompt};
use clap::Subcommand;
use openside_proto::{
    AddChannelAccountRequest, ChannelAccount, ChannelAccountList, ChannelAccountOptions,
    ChannelAccountTest, ChannelCredentials,
};
use std::io::{Read, Write};

// Clap derives this enum from the CLI shape; boxing its fields would obscure the arguments.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub(crate) enum ChannelCommand {
    /// List account labels and effective status, never credentials.
    List,
    /// Add a channel identity; credentials are read from hidden stdin only.
    Add {
        kind: String,
        id: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        enabled: bool,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        api_base: Option<String>,
        #[arg(long)]
        listen: Option<String>,
        #[arg(long)]
        public_key: Option<String>,
        #[arg(long = "allow-chat")]
        allowed_chats: Vec<String>,
        #[arg(long = "allow-user")]
        allowed_users: Vec<String>,
        #[arg(long = "allow-workspace")]
        allowed_workspaces: Vec<String>,
        #[arg(long = "allow-channel")]
        allowed_channels: Vec<String>,
        #[arg(long = "allow-guild")]
        allowed_guilds: Vec<String>,
    },
    /// Remove only this account, stopping its adapter and deleting its Vault credentials.
    Rm { kind: String, id: String },
    /// Perform a read-only connectivity check, without sending a chat message.
    Test { kind: String, id: String },
}
fn path(kind: &str, id: Option<&str>) -> Result<String, String> {
    if !matches!(kind, "telegram" | "slack" | "discord") {
        return Err("channel kind must be telegram, slack or discord".into());
    }
    if id.is_some_and(|id| !openside_config::valid_channel_id(id)) {
        return Err("invalid channel account id".into());
    }
    Ok(format!(
        "/v1/settings/channels/{kind}/accounts{}",
        id.map(|id| format!("/{id}")).unwrap_or_default()
    ))
}
fn show<W: Write>(a: &ChannelAccount, json: bool, stdout: &mut W) -> Result<(), String> {
    if json {
        emit(stdout, a)
    } else {
        writeln!(
            stdout,
            "{}\t{}\t{}\t{:?}\t{}",
            a.kind,
            a.id,
            a.label,
            a.status,
            a.agent_id.as_deref().unwrap_or("-")
        )
        .map_err(output_error)
    }
}
pub(crate) async fn run<R: Read, W: Write, E: Write>(
    command: &ChannelCommand,
    client: &Client,
    json: bool,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    match command {
        ChannelCommand::List => {
            let rows: ChannelAccountList = client.get("/v1/settings/channels").await?;
            if json {
                emit(stdout, &rows)
            } else {
                for a in &rows.accounts {
                    show(a, false, stdout)?;
                }
                Ok(())
            }
        }
        ChannelCommand::Add {
            kind,
            id,
            label,
            enabled,
            agent,
            api_base,
            listen,
            public_key,
            allowed_chats,
            allowed_users,
            allowed_workspaces,
            allowed_channels,
            allowed_guilds,
        } => {
            let endpoint = path(kind, None)?;
            path(kind, Some(id))?;
            let credentials = ChannelCredentials {
                token: prompt::read_hidden("Bot token", stdin, stderr)?,
                signing_secret: if kind == "slack" {
                    Some(prompt::read_hidden("Slack signing secret", stdin, stderr)?)
                } else {
                    None
                },
            };
            let request = AddChannelAccountRequest {
                id: id.clone(),
                label: label.clone(),
                enabled: *enabled,
                agent_id: agent.clone(),
                credentials,
                options: ChannelAccountOptions {
                    api_base: api_base.clone(),
                    listen: listen.clone(),
                    public_key: public_key.clone(),
                    allowed_chats: allowed_chats.clone(),
                    allowed_users: allowed_users.clone(),
                    allowed_workspaces: allowed_workspaces.clone(),
                    allowed_channels: allowed_channels.clone(),
                    allowed_guilds: allowed_guilds.clone(),
                    poll_timeout_secs: None,
                },
            };
            let row: ChannelAccount = client.json("POST", &endpoint, Some(&request)).await?;
            show(&row, json, stdout)
        }
        ChannelCommand::Rm { kind, id } => {
            client.send("DELETE", &path(kind, Some(id))?).await?;
            if !json {
                writeln!(stdout, "Channel account removed").map_err(output_error)?;
            }
            Ok(())
        }
        ChannelCommand::Test { kind, id } => {
            let result: ChannelAccountTest = client
                .json::<ChannelAccountTest, ()>(
                    "POST",
                    &format!("{}/test", path(kind, Some(id))?),
                    None,
                )
                .await?;
            if json {
                emit(stdout, &result)?;
            } else {
                writeln!(
                    stdout,
                    "{}\t{}\t{:?}\t{}",
                    kind, id, result.status, result.code
                )
                .map_err(output_error)?;
            }
            if !result.ok {
                return Err(result.code);
            }
            Ok(())
        }
    }
}
