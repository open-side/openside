//! `openside pair` and `openside device list|revoke` (F-3, WS-5).
//!
//! `pair` asks the daemon (over the local socket only) for a one-time pairing code valid
//! for three minutes and prints it as a terminal QR code of `remote.public_url/pair#code`.
//! The phone that opens the link exchanges the code for its own scoped token; the code
//! itself grants nothing after use or expiry. `device revoke` takes effect on the
//! device's next request and closes its open event streams.

use std::io::Write;

use clap::{Args, Subcommand};
use openside_proto::{CapabilityScope, CreatePairingRequest, DeviceList, PairingOffer};

use crate::{client::Client, emit, output_error, token::parse_scope};

#[derive(Args)]
pub(crate) struct PairArgs {
    /// Scope the new device gets; repeat for several. Default: read, approve, human_step.
    #[arg(long = "scope", value_name = "SCOPE", value_parser = parse_scope)]
    scopes: Vec<CapabilityScope>,
    /// Label shown in `openside device list`.
    #[arg(long, value_name = "NAME", default_value = "phone")]
    name: String,
}

#[derive(Subcommand)]
pub(crate) enum DeviceCommand {
    /// Paired devices with their scopes; tokens are never shown.
    List,
    /// Revoke a device: its next request is refused and its streams close.
    Revoke {
        /// Device id from `openside device list`.
        id: String,
    },
}

/// Terminal QR code, two modules per character row so it fits a normal window.
pub(crate) fn qr(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

pub(crate) async fn pair<W: Write>(
    args: &PairArgs,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    let mut scopes = if args.scopes.is_empty() {
        vec![
            CapabilityScope::Read,
            CapabilityScope::Approve,
            CapabilityScope::HumanStep,
        ]
    } else {
        args.scopes.clone()
    };
    scopes.dedup();
    let offer: PairingOffer = client
        .json(
            "POST",
            "/v1/pairing",
            Some(&CreatePairingRequest {
                name: args.name.clone(),
                scopes,
            }),
        )
        .await
        .map_err(|error| {
            if error.contains("local_socket_required") {
                "pairing codes can only be issued over the local Unix socket; run this on the \
                 daemon host without --url/--token"
                    .to_owned()
            } else {
                error
            }
        })?;
    if json {
        return emit(stdout, &offer);
    }
    let target = offer.url.clone().unwrap_or_else(|| offer.path.clone());
    if offer.url.is_some() {
        if let Some(code) = qr(&target) {
            writeln!(stdout, "{code}").map_err(output_error)?;
        }
        writeln!(stdout, "Scan with the phone, or open: {target}").map_err(output_error)?;
    } else {
        writeln!(
            stdout,
            "remote.public_url is not set: open the web app on the device and go to {target}"
        )
        .map_err(output_error)?;
    }
    writeln!(stdout, "Code: {}", offer.code).map_err(output_error)?;
    writeln!(stdout, "Valid once until {} (3 minutes).", offer.expires_at).map_err(output_error)
}

pub(crate) async fn device<W: Write>(
    command: &DeviceCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        DeviceCommand::List => {
            let list: DeviceList = client.get("/v1/devices").await?;
            if json {
                return emit(stdout, &list);
            }
            for device in &list.devices {
                let scopes: Vec<String> = device
                    .scopes
                    .iter()
                    .map(|scope| {
                        serde_json::to_value(scope)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    })
                    .collect();
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}",
                    device.device_id,
                    device.name,
                    scopes.join(","),
                    device
                        .created_at
                        .map(|at| at.to_string())
                        .unwrap_or_default()
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        DeviceCommand::Revoke { id } => {
            client
                .send(
                    "DELETE",
                    &format!("/v1/devices/{}", crate::client::encode_segment(id)),
                )
                .await?;
            if json {
                return emit(stdout, &serde_json::json!({"revoked": id}));
            }
            writeln!(stdout, "revoked {id}").map_err(output_error)
        }
    }
}
