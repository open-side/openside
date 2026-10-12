//! `openside vault status|init|unlock|lock|add|rm|items|policy|audit`: vault administration
//! through `/v1/vault/*`. Passphrases, usernames, passwords and authenticator keys are read
//! at prompts (hidden, except the username) or one per line from piped stdin, never from
//! argv, and only sent to the daemon. No command can print a stored value.

use std::io::{Read, Write};

use clap::{Subcommand, ValueEnum};
use openside_proto::{
    CreateVaultItemRequest, UpdateVaultItemRequest, VaultAuditList, VaultBackend, VaultItem,
    VaultItemList, VaultPolicy, VaultPolicyOverride, VaultState, VaultStatus, VaultUnlockRequest,
};

use crate::{client::Client, emit, output_error, prompt};

#[derive(Subcommand)]
pub(crate) enum VaultCommand {
    /// Show whether the vault is set up and unlocked, and how it is unlocked.
    Status,
    /// Create the vault: with an OS keyring nothing is asked; otherwise choose a passphrase.
    Init,
    /// Unlock the vault (asks for the passphrase of a passphrase vault).
    Unlock,
    /// Drop the decrypted vault from daemon memory until it is unlocked again.
    Lock,
    /// Add a login and/or an authenticator; values are asked for, never passed as arguments.
    Add {
        /// New item id (`[a-z0-9:_-]`, up to 64 characters).
        id: String,
        /// Allowed origin, e.g. `https://example.com` or `https://*.example.com`; repeatable.
        #[arg(long = "origin", value_name = "ORIGIN", required = true)]
        origins: Vec<String>,
        /// Ask for a username and password (the default unless only --totp is given).
        #[arg(long)]
        login: bool,
        /// Ask for an authenticator setup key or `otpauth://` link.
        #[arg(long)]
        totp: bool,
        /// Per-item access policy; `default` inherits vault.default_policy.
        #[arg(long, value_enum, default_value = "default")]
        policy: PolicyArg,
    },
    /// Delete an item.
    Rm { id: String },
    /// List item ids, allowed origins and access policies (never values).
    Items,
    /// Set an item's access policy, or `default` to inherit vault.default_policy.
    Policy {
        id: String,
        #[arg(value_enum)]
        policy: PolicyArg,
    },
    /// Show audit records (time, session, tool call, item, origin, result), oldest first.
    Audit {
        /// Only records at or after this RFC 3339 time.
        #[arg(long, value_name = "TIME")]
        since: Option<String>,
        #[arg(long, value_name = "ID")]
        item: Option<String>,
        #[arg(long, value_name = "SESSION")]
        session: Option<String>,
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PolicyArg {
    Always,
    #[value(name = "while_unlocked")]
    WhileUnlocked,
    Never,
    Default,
}

fn policy_name(policy: VaultPolicy) -> &'static str {
    match policy {
        VaultPolicy::Always => "always",
        VaultPolicy::WhileUnlocked => "while_unlocked",
        VaultPolicy::Never => "never",
    }
}

fn print_item<W: Write>(stdout: &mut W, item: &VaultItem) -> Result<(), String> {
    writeln!(
        stdout,
        "{}\t{}{}\t{}\ttotp={}\tpasskeys={}",
        item.id,
        policy_name(item.effective_policy),
        if item.policy.is_none() {
            " (default)"
        } else {
            ""
        },
        item.origins.join(","),
        if item.has_totp { "yes" } else { "no" },
        item.passkey_rp_ids.join(","),
    )
    .map_err(output_error)
}

fn backend_name(backend: Option<VaultBackend>) -> &'static str {
    match backend {
        Some(VaultBackend::Passphrase) => "passphrase",
        Some(VaultBackend::SecretService) => "secret_service",
        Some(VaultBackend::Native) => "native",
        None => "-",
    }
}

fn print_status<W: Write>(stdout: &mut W, json: bool, status: &VaultStatus) -> Result<(), String> {
    if json {
        return emit(stdout, status);
    }
    let state = match status.state {
        VaultState::Uninitialized => "uninitialized",
        VaultState::Locked => "locked",
        VaultState::Unlocked => "unlocked",
        VaultState::Unavailable => "unavailable",
    };
    writeln!(
        stdout,
        "{state}\t{}{}",
        backend_name(status.backend),
        status
            .reason
            .as_deref()
            .map(|reason| format!("\t{reason}"))
            .unwrap_or_default()
    )
    .map_err(output_error)
}

fn hidden<R: Read, E: Write>(label: &str, stdin: &mut R, stderr: &mut E) -> Result<String, String> {
    prompt::read_hidden(label, stdin, stderr)
}

pub(crate) async fn execute<R: Read, W: Write, E: Write>(
    command: &VaultCommand,
    json: bool,
    client: &Client,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    match command {
        VaultCommand::Status => {
            let status: VaultStatus = client.get("/v1/vault").await?;
            print_status(stdout, json, &status)
        }
        VaultCommand::Init => {
            let status: VaultStatus = client.get("/v1/vault").await?;
            if status.state != VaultState::Uninitialized {
                print_status(stdout, json, &status)?;
                return Err(match status.state {
                    VaultState::Unavailable => {
                        "the vault cannot be set up here; run `openside doctor`".to_owned()
                    }
                    _ => "the vault is already set up".to_owned(),
                });
            }
            let passphrase = if status.backend == Some(VaultBackend::Passphrase) {
                let first = hidden("New vault passphrase (8+ characters)", stdin, stderr)?;
                let again = hidden("Repeat the passphrase", stdin, stderr)?;
                if first != again {
                    return Err("the passphrases do not match".to_owned());
                }
                Some(first)
            } else {
                None
            };
            let status: VaultStatus = client
                .json(
                    "POST",
                    "/v1/vault/setup",
                    Some(&VaultUnlockRequest { passphrase }),
                )
                .await?;
            print_status(stdout, json, &status)
        }
        VaultCommand::Unlock => {
            let status: VaultStatus = client.get("/v1/vault").await?;
            if status.state == VaultState::Unlocked {
                return print_status(stdout, json, &status);
            }
            let passphrase = (status.backend == Some(VaultBackend::Passphrase))
                .then(|| hidden("Vault passphrase", stdin, stderr))
                .transpose()?;
            let status: VaultStatus = client
                .json(
                    "POST",
                    "/v1/vault/unlock",
                    Some(&VaultUnlockRequest { passphrase }),
                )
                .await?;
            print_status(stdout, json, &status)
        }
        VaultCommand::Lock => {
            let status: VaultStatus = client
                .json::<_, serde_json::Value>("POST", "/v1/vault/lock", None)
                .await?;
            print_status(stdout, json, &status)
        }
        VaultCommand::Add {
            id,
            origins,
            login,
            totp,
            policy,
        } => {
            let login = *login || !*totp;
            let (username, password) = if login {
                let username = prompt::read_visible(&format!("Username for {id}"), stdin, stderr)?;
                let password = hidden(&format!("Password for {id}"), stdin, stderr)?;
                (Some(username), Some(password))
            } else {
                (None, None)
            };
            let totp = totp
                .then(|| hidden("Authenticator setup key or otpauth:// link", stdin, stderr))
                .transpose()?;
            let request = CreateVaultItemRequest {
                id: id.clone(),
                origins: origins.clone(),
                username,
                password,
                totp,
                policy: match policy {
                    PolicyArg::Always => Some(VaultPolicy::Always),
                    PolicyArg::WhileUnlocked => Some(VaultPolicy::WhileUnlocked),
                    PolicyArg::Never => Some(VaultPolicy::Never),
                    PolicyArg::Default => None,
                },
            };
            let item: VaultItem = client
                .json("POST", "/v1/vault/items", Some(&request))
                .await?;
            if json {
                return emit(stdout, &item);
            }
            print_item(stdout, &item)
        }
        VaultCommand::Rm { id } => {
            let path = format!("/v1/vault/items/{}", crate::client::encode_segment(id));
            client.send("DELETE", &path).await?;
            if json {
                return emit(stdout, &serde_json::json!({"deleted": id}));
            }
            writeln!(stdout, "Deleted {id}").map_err(output_error)
        }
        VaultCommand::Items => {
            let list: VaultItemList = client.get("/v1/vault/items").await?;
            if json {
                return emit(stdout, &list);
            }
            if !list.available {
                return writeln!(stdout, "No unlocked vault").map_err(output_error);
            }
            for item in &list.items {
                print_item(stdout, item)?;
            }
            Ok(())
        }
        VaultCommand::Policy { id, policy } => {
            let request = UpdateVaultItemRequest {
                policy: match policy {
                    PolicyArg::Always => VaultPolicyOverride::Always,
                    PolicyArg::WhileUnlocked => VaultPolicyOverride::WhileUnlocked,
                    PolicyArg::Never => VaultPolicyOverride::Never,
                    PolicyArg::Default => VaultPolicyOverride::Default,
                },
            };
            let path = format!("/v1/vault/items/{}", crate::client::encode_segment(id));
            let item: VaultItem = client.json("PATCH", &path, Some(&request)).await?;
            if json {
                return emit(stdout, &item);
            }
            print_item(stdout, &item)
        }
        VaultCommand::Audit {
            since,
            item,
            session,
            limit,
        } => {
            let mut params = url::form_urlencoded::Serializer::new(String::new());
            for (key, value) in [("since", since), ("item", item), ("session", session)] {
                if let Some(value) = value {
                    params.append_pair(key, value);
                }
            }
            if let Some(limit) = limit {
                params.append_pair("limit", &limit.to_string());
            }
            let params = params.finish();
            let path = if params.is_empty() {
                "/v1/vault/audit".to_owned()
            } else {
                format!("/v1/vault/audit?{params}")
            };
            let list: VaultAuditList = client.get(&path).await?;
            if json {
                return emit(stdout, &list);
            }
            for record in &list.records {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}\t{}\t{}{}\t{}\t{}",
                    record.at.to_rfc3339(),
                    record.session_id.as_deref().unwrap_or("-"),
                    record.tool_call_id.as_deref().unwrap_or("-"),
                    record.what,
                    record.item,
                    record.result,
                    record
                        .detail
                        .as_deref()
                        .map(|detail| format!(" ({detail})"))
                        .unwrap_or_default(),
                    record.origin,
                    record.who,
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
    }
}
