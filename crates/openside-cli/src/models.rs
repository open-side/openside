use crate::{client::Client, emit, output_error, prompt};
use clap::Subcommand;
use openside_proto::{
    ModelAccountRef, ModelCatalog, ModelSelection, ProviderKeyInfo, ProviderKeyList, ProviderPool,
    ProviderPoolList, SessionAccountSelection,
};
use serde::Serialize;
use serde_json::json;
use std::io::{Read, Write};
use zeroize::Zeroizing;

#[derive(Subcommand)]
pub(crate) enum KeyCommand {
    /// List labelled provider accounts, masked suffixes and enabled state, never values.
    List,
    /// Create a labelled account or replace --key-id; reads a hidden stdin line only.
    Set {
        provider: String,
        #[arg(long, required_unless_present = "key_id")]
        label: Option<String>,
        #[arg(long)]
        key_id: Option<String>,
    },
    /// Change account metadata without reading or replacing the key.
    Update {
        provider: String,
        key_id: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        reserve_percent: Option<u8>,
    },
    /// Remove only this provider account.
    Rm { provider: String, key_id: String },
}
#[derive(Subcommand)]
pub(crate) enum ModelCommand {
    /// Set reasoning/fast models and fallbacks; Team homes update on their next turn.
    Set {
        model: String,
        #[arg(long = "fallback")]
        fallbacks: Vec<String>,
        /// Fast front-agent model, or none to use the reasoning model.
        #[arg(long)]
        fast: Option<String>,
    },
    /// List or replace explicit ordered provider account pools.
    Pool {
        #[command(subcommand)]
        command: PoolCommand,
    },
    /// Prefer an authorized account for subsequent requests; omit account to clear.
    Account {
        session: String,
        account: Option<String>,
    },
}
#[derive(Subcommand)]
pub(crate) enum PoolCommand {
    /// List provider pools without credential values.
    List,
    /// Replace membership with source:account-id references; empty disables routing.
    Set {
        provider: String,
        accounts: Vec<String>,
    },
}

pub(crate) async fn list<W: Write>(
    client: &Client,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let catalog: ModelCatalog = client.get("/v1/settings/models").await?;
    if json {
        return emit(stdout, &catalog);
    }
    if let Some(ui) = crate::ui::pretty() {
        return write!(stdout, "{}", crate::ui::models(ui, &catalog)).map_err(output_error);
    }
    writeln!(
        stdout,
        "Reasoning model: {}\nFast model: {}",
        catalog.default_model.as_deref().unwrap_or("not configured"),
        catalog.fast_model.as_deref().unwrap_or("same as reasoning"),
    )
    .map_err(output_error)?;
    writeln!(
        stdout,
        "PROVIDER\tMODEL\tAVAILABLE\tSOURCE\tACCOUNT\tREASON\tREMAINING USAGE\tCATALOG"
    )
    .map_err(output_error)?;
    for provider in &catalog.providers {
        for model in &provider.models {
            writeln!(
                stdout,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                provider.provider,
                model.id,
                model.available,
                model.source.as_deref().unwrap_or("-"),
                model.account_id.as_deref().unwrap_or("-"),
                model.reason,
                remaining_usage(model),
                if model.in_catalog {
                    "current"
                } else {
                    "Not in current catalog"
                },
            )
            .map_err(output_error)?;
        }
        writeln!(
            stdout,
            "Pool {}: {}",
            provider.provider,
            provider
                .pool
                .accounts
                .iter()
                .map(|account| format!("{}:{}", account.source, account.account_id))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .map_err(output_error)?;
    }
    Ok(())
}

pub(crate) fn remaining_usage(model: &openside_proto::CatalogModel) -> String {
    let accounts: Vec<_> = model
        .accounts
        .iter()
        .filter(|account| account.in_pool)
        .enumerate()
        .map(|(index, account)| {
            let label = if account.label.is_empty() {
                format!("Account {}", index + 1)
            } else {
                account.label.clone()
            };
            let windows: Vec<_> = account
                .usage_windows
                .iter()
                .map(|window| {
                    let remaining = window
                        .used_percent
                        .map(|used| format!("{:.0}% remaining", (100.0 - used).clamp(0.0, 100.0)))
                        .unwrap_or_else(|| "unknown".into());
                    let reset = window
                        .reset_at_ms
                        .map(|ms| format!("; resets at {ms:.0} Unix ms"))
                        .unwrap_or_default();
                    format!("{}: {remaining}{reset}", window.name)
                })
                .collect();
            format!(
                "{label}: {}",
                if windows.is_empty() {
                    "unknown".into()
                } else {
                    windows.join(", ")
                }
            )
        })
        .collect();
    if accounts.is_empty() {
        "unknown".into()
    } else {
        accounts.join("; ")
    }
}

pub(crate) fn component(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err("invalid account/provider/session identifier".into());
    }
    Ok(value)
}
fn account(value: &str) -> Result<ModelAccountRef, String> {
    let (source, id) = value
        .split_once(':')
        .ok_or("account must be source:account-id")?;
    if ![
        "omp-broker",
        "vault",
        "env",
        "command",
        "claude-code",
        "codex-cli",
        "builtin",
    ]
    .contains(&source)
    {
        return Err("unknown credential source".into());
    }
    Ok(ModelAccountRef {
        source: source.into(),
        account_id: component(id)?.into(),
    })
}
pub(crate) async fn run<W: Write>(
    command: Option<&ModelCommand>,
    client: &Client,
    json_output: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let Some(command) = command else {
        return list(client, json_output, stdout).await;
    };
    match command {
        ModelCommand::Set {
            model,
            fallbacks,
            fast,
        } => {
            let value: ModelSelection = client
                .json(
                    "PUT",
                    "/v1/settings/models",
                    Some(&json!({"default_model":model,"fallback_models":fallbacks,
                        "fast_model":fast.as_deref().filter(|id| *id != "none")})),
                )
                .await?;
            if json_output {
                emit(stdout, &value)
            } else {
                writeln!(stdout,"Reasoning model: {}; Fast model: {} (new conversations and Team homes on their next turn); fallbacks: {}",value.default_model,value.fast_model.as_deref().unwrap_or("same as reasoning"),value.fallback_models.join(", ")).map_err(output_error)
            }
        }
        ModelCommand::Pool {
            command: PoolCommand::List,
        } => {
            let value: ProviderPoolList = client.get("/v1/settings/pools").await?;
            if json_output {
                return emit(stdout, &value);
            }
            for pool in value.pools {
                writeln!(
                    stdout,
                    "{}\t{}",
                    pool.provider,
                    pool.accounts
                        .iter()
                        .map(|a| format!("{}:{}", a.source, a.account_id))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        ModelCommand::Pool {
            command: PoolCommand::Set { provider, accounts },
        } => {
            let accounts = accounts
                .iter()
                .map(|value| account(value))
                .collect::<Result<Vec<_>, _>>()?;
            let value: ProviderPool = client
                .json(
                    "PUT",
                    &format!("/v1/settings/pools/{}", component(provider)?),
                    Some(&json!({"accounts":accounts})),
                )
                .await?;
            if json_output {
                emit(stdout, &value)
            } else {
                writeln!(stdout, "Provider pool updated: {}", value.provider).map_err(output_error)
            }
        }
        ModelCommand::Account {
            session,
            account: preference,
        } => {
            let account = preference.as_deref().map(account).transpose()?;
            let value: SessionAccountSelection = client
                .json(
                    "PUT",
                    &format!("/v1/sessions/{}/account", component(session)?),
                    Some(&json!({"account":account})),
                )
                .await?;
            if json_output {
                emit(stdout, &value)
            } else {
                writeln!(stdout, "Account preference applies to the next request")
                    .map_err(output_error)
            }
        }
    }
}

#[derive(Serialize)]
struct KeyBody<'a> {
    key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
}
fn provider_path(provider: &str) -> Result<String, String> {
    if !matches!(provider, "anthropic" | "openai" | "google") {
        return Err("provider must be anthropic, openai or google".into());
    }
    Ok(format!("/v1/settings/keys/{provider}"))
}
fn show<W: Write>(info: &ProviderKeyInfo, json: bool, stdout: &mut W) -> Result<(), String> {
    if json {
        emit(stdout, info)
    } else {
        writeln!(
            stdout,
            "{}\t{}\t{}\t…{}\tenabled={}\t{}",
            info.provider, info.key_id, info.label, info.last_four, info.enabled, info.created_at
        )
        .map_err(output_error)
    }
}
pub(crate) async fn key<R: Read, W: Write, E: Write>(
    command: &KeyCommand,
    client: &Client,
    json_output: bool,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    match command {
        KeyCommand::List => {
            let list: ProviderKeyList = client.get("/v1/settings/keys").await?;
            if json_output {
                emit(stdout, &list)
            } else if let Some(ui) = crate::ui::pretty() {
                write!(stdout, "{}", crate::ui::key_list(ui, &list)).map_err(output_error)
            } else {
                for info in &list.keys {
                    show(info, false, stdout)?;
                }
                Ok(())
            }
        }
        KeyCommand::Set {
            provider,
            label,
            key_id,
        } => {
            let mut path = provider_path(provider)?;
            let method = if let Some(id) = key_id {
                path.push('/');
                path.push_str(component(id)?);
                "PUT"
            } else {
                "POST"
            };
            let key = Zeroizing::new(prompt::read_hidden("Provider API key", stdin, stderr)?);
            let info: ProviderKeyInfo = client
                .json(
                    method,
                    &path,
                    Some(&KeyBody {
                        key: &key,
                        label: label.as_deref(),
                    }),
                )
                .await?;
            show(&info, json_output, stdout)
        }
        KeyCommand::Update {
            provider,
            key_id,
            label,
            enabled,
            reserve_percent,
        } => {
            let path = format!("{}/{}", provider_path(provider)?, component(key_id)?);
            let info: ProviderKeyInfo = client
                .json(
                    "PATCH",
                    &path,
                    Some(
                        &json!({"label":label,"enabled":enabled,"reserve_percent":reserve_percent}),
                    ),
                )
                .await?;
            show(&info, json_output, stdout)
        }
        KeyCommand::Rm { provider, key_id } => {
            client
                .send(
                    "DELETE",
                    &format!("{}/{}", provider_path(provider)?, component(key_id)?),
                )
                .await?;
            if !json_output {
                writeln!(stdout, "Provider account removed").map_err(output_error)?;
            }
            Ok(())
        }
    }
}
