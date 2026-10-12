//! `openside sync setup|pair|now|status`: end-to-end encrypted sync of pinned tabs and
//! vault items through `/v1/sync/*`. The passphrase never passes through the CLI; the
//! daemon reads it from `sync.passphrase_cmd`.

use std::io::Write;

use clap::Subcommand;
use openside_proto::{PairSyncRequest, SyncBackendKind, SyncReport, SyncSetupResponse, SyncStatus};
use serde_json::Value;

use crate::{client::Client, emit, output_error};

#[derive(Subcommand)]
pub(crate) enum SyncCommand {
    /// Create a sync account on this device and print the pairing code for the next one.
    Setup,
    /// Join the account of a pairing code (same passphrase in sync.passphrase_cmd).
    Pair {
        /// `osync1-…` code printed by `openside sync setup`.
        code: String,
    },
    /// Sync now: push local changes, pull and merge the other devices' changes.
    Now,
    /// Show whether sync is configured, set up and unlocked, and what is pending.
    Status,
}

fn print_status<W: Write>(stdout: &mut W, status: &SyncStatus) -> Result<(), String> {
    let backend = match status.backend {
        Some(SyncBackendKind::Webdav) => "webdav",
        Some(SyncBackendKind::Dir) => "dir",
        None => "none",
    };
    writeln!(
        stdout,
        "backend\t{backend}\nset_up\t{}\nunlocked\t{}\naccount\t{}\ndevice\t{}\nlast_sync\t{}\nrecords\t{}\npending\t{}\nexhausted\t{}",
        status.set_up,
        status.unlocked,
        status.account_id.as_deref().unwrap_or("-"),
        status.device_id.as_deref().unwrap_or("-"),
        status
            .last_sync
            .map_or_else(|| "-".to_owned(), |at| at.to_rfc3339()),
        status.records,
        status.pending,
        status.exhausted,
    )
    .map_err(output_error)
}

pub(crate) async fn execute<W: Write>(
    command: &SyncCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        SyncCommand::Setup => {
            let response: SyncSetupResponse = client
                .json::<_, Value>("POST", "/v1/sync/setup", None)
                .await?;
            if json {
                return emit(stdout, &response);
            }
            writeln!(
                stdout,
                "pairing code\t{}\nOn the other device, with the same sync passphrase: openside sync pair {}",
                response.pairing_code, response.pairing_code
            )
            .map_err(output_error)
        }
        SyncCommand::Pair { code } => {
            let status: SyncStatus = client
                .json(
                    "POST",
                    "/v1/sync/pair",
                    Some(&PairSyncRequest {
                        pairing_code: code.clone(),
                    }),
                )
                .await?;
            if json {
                return emit(stdout, &status);
            }
            print_status(stdout, &status)
        }
        SyncCommand::Now => {
            let report: SyncReport = client
                .json::<_, Value>("POST", "/v1/sync/run", None)
                .await?;
            if json {
                return emit(stdout, &report);
            }
            writeln!(
                stdout,
                "pushed\t{}\npulled\t{}\napplied\t{}\nconflicts\t{}\nrejected\t{}\ndeferred\t{}\nvault\t{}",
                report.pushed,
                report.pulled,
                report.applied,
                report.conflicts,
                report.rejected,
                report.deferred,
                if report.vault_available { "synced" } else { "locked (items wait)" },
            )
            .map_err(output_error)
        }
        SyncCommand::Status => {
            let status: SyncStatus = client.get("/v1/sync").await?;
            if json {
                return emit(stdout, &status);
            }
            print_status(stdout, &status)
        }
    }
}
