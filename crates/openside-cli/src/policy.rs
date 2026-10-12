//! `openside policy show|set|test`: the live daemon policy (`/v1/policy`).

use std::{io::Write, path::PathBuf};

use clap::Subcommand;
use openside_proto::{PolicyCapability, PolicyDecision, PolicySettings, PolicyTestRequest};

use crate::{client::Client, emit, output_error, Mode};

#[derive(Subcommand)]
pub(crate) enum PolicyCommand {
    /// Print the live policy as JSON.
    Show,
    /// Replace the live policy with a JSON document (see `policy show`); applies immediately.
    Set {
        /// File holding the policy JSON.
        file: PathBuf,
    },
    /// Evaluate one action without running it.
    Test {
        /// URL, origin or absolute path the action touches.
        target: String,
        /// fs_read, fs_write, exec, browse, fetch, vault_use or sensitive.
        #[arg(value_parser = parse_capability)]
        capability: PolicyCapability,
        /// Session mode to evaluate under (default: the policy's default_mode).
        #[arg(long, value_enum)]
        mode: Option<Mode>,
        /// Approved workspace for guard mode.
        #[arg(long, value_name = "PATH")]
        workspace: Option<PathBuf>,
    },
}

fn parse_capability(value: &str) -> Result<PolicyCapability, String> {
    serde_json::from_value(serde_json::Value::String(value.replace('-', "_")))
        .map_err(|_| format!("unknown capability {value:?}"))
}

pub(crate) async fn execute<W: Write>(
    command: &PolicyCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        PolicyCommand::Show => {
            let settings: PolicySettings = client.get("/v1/policy").await?;
            print_settings(json, stdout, &settings)
        }
        PolicyCommand::Set { file } => {
            let text =
                std::fs::read(file).map_err(|_| "could not read the policy file".to_owned())?;
            let settings: PolicySettings = serde_json::from_slice(&text)
                .map_err(|error| format!("invalid policy JSON: {error}"))?;
            let applied: PolicySettings = client.json("PUT", "/v1/policy", Some(&settings)).await?;
            print_settings(json, stdout, &applied)
        }
        PolicyCommand::Test {
            target,
            capability,
            mode,
            workspace,
        } => {
            let workspace = workspace
                .as_ref()
                .map(|path| {
                    path.canonicalize()
                        .map_err(|_| "could not canonicalize --workspace".to_owned())?
                        .into_os_string()
                        .into_string()
                        .map_err(|_| "--workspace must be valid UTF-8".to_owned())
                })
                .transpose()?;
            let decision: PolicyDecision = client
                .json(
                    "POST",
                    "/v1/policy/test",
                    Some(&PolicyTestRequest {
                        target: target.clone(),
                        capability: *capability,
                        mode: mode.map(Into::into),
                        workspace,
                    }),
                )
                .await?;
            if json {
                return emit(stdout, &decision);
            }
            let effect = serde_json::to_value(decision.effect).map_err(|_| "invalid decision")?;
            writeln!(
                stdout,
                "{}\t{}",
                effect.as_str().unwrap_or_default(),
                decision.reason
            )
            .map_err(output_error)
        }
    }
}

fn print_settings<W: Write>(
    json: bool,
    stdout: &mut W,
    settings: &PolicySettings,
) -> Result<(), String> {
    if json {
        return emit(stdout, settings);
    }
    let text = serde_json::to_string_pretty(settings).map_err(|_| "invalid policy")?;
    writeln!(stdout, "{text}").map_err(output_error)
}
