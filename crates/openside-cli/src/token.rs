//! `openside token create --scope S...`: mint a scoped device token (PP-20).
//!
//! Minting is allowed only over the local Unix socket; the daemon answers 403
//! `local_socket_required` to TCP callers even when their token is admin. The token is
//! printed once on stdout for the caller to hand to a sandboxed tool; it is never logged.

use std::io::Write;

use clap::{Args, Subcommand};
use openside_proto::{CapabilityScope, PairDeviceRequest, PairDeviceResponse};

use crate::{client::Client, emit, output_error};

#[derive(Subcommand)]
pub(crate) enum TokenCommand {
    /// Create a device token limited to the given scopes.
    Create(CreateArgs),
}

#[derive(Args)]
pub(crate) struct CreateArgs {
    /// Scope to grant; repeat for several (read, steer, approve, human_step, browse,
    /// vault_use, otp_ingest, admin).
    #[arg(long = "scope", value_name = "SCOPE", required = true, value_parser = parse_scope)]
    scopes: Vec<CapabilityScope>,
    /// Label shown for the device.
    #[arg(long, value_name = "NAME", default_value = "cli token")]
    name: String,
}

pub(crate) fn parse_scope(value: &str) -> Result<CapabilityScope, String> {
    serde_json::from_value(serde_json::Value::String(value.replace('-', "_")))
        .map_err(|_| format!("unknown scope {value:?}"))
}

pub(crate) async fn execute<W: Write>(
    command: &TokenCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        TokenCommand::Create(args) => {
            let mut scopes = args.scopes.clone();
            scopes.dedup();
            let paired: PairDeviceResponse = client
                .json(
                    "POST",
                    "/v1/devices/pair",
                    Some(&PairDeviceRequest {
                        name: args.name.clone(),
                        scopes,
                    }),
                )
                .await
                .map_err(|error| {
                    if error.contains("local_socket_required") {
                        "tokens can only be created over the local Unix socket; run this command \
                         on the daemon host without --url/--token"
                            .to_owned()
                    } else {
                        error
                    }
                })?;
            if json {
                return emit(stdout, &paired);
            }
            writeln!(stdout, "{}\t{}", paired.device_id, paired.token).map_err(output_error)
        }
    }
}
