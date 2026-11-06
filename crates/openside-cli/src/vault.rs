//! `openside vault status|init|unlock|lock|add|rm|items|policy|audit|settings|export|import|
//! clear|recovery-key|recover`: vault administration through `/v1/vault/*`. Passphrases,
//! usernames, passwords, authenticator keys and recovery keys are read at prompts (hidden,
//! except the username) or one per line from piped stdin, never from argv, and only sent to
//! the daemon. Only `export` (to a new 0600 file) and `recovery-key` (printed once) put values
//! anywhere, and both need `--yes`.

use std::io::{Read, Write};

use clap::{Subcommand, ValueEnum};
use openside_proto::{
    CreateVaultItemRequest, ImportCategoryReport, ImportPasswordsRequest,
    UpdateVaultAutofillSettings, UpdateVaultItemRequest, UpdateVaultSettingsRequest,
    VaultAuditList, VaultAutoLock, VaultBackend, VaultClearResult, VaultConfirmRequest,
    VaultExport, VaultItem, VaultItemList, VaultPolicy, VaultPolicyOverride, VaultRecoverRequest,
    VaultRecoveryKey, VaultSettings, VaultState, VaultStatus, VaultUnlockRequest,
};
use zeroize::Zeroize;

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
    /// Show password settings, or change the ones given.
    Settings {
        /// Lock the vault after this long without use.
        #[arg(long, value_enum, value_name = "TIME")]
        auto_lock: Option<AutoLockArg>,
        /// Require a fingerprint or Touch ID before a keyring vault unlocks.
        #[arg(long, value_name = "BOOL")]
        biometric_unlock: Option<bool>,
        /// Fill saved sign-ins at all.
        #[arg(long, value_name = "BOOL")]
        autofill: Option<bool>,
        /// Press the sign-in button after filling.
        #[arg(long, value_name = "BOOL")]
        auto_submit: Option<bool>,
        /// Also fill on plain-http pages (localhost always counts as secure).
        #[arg(long, value_name = "BOOL")]
        allow_insecure: Option<bool>,
        /// Never fill on this domain or its subdomains; repeatable.
        #[arg(long = "exclude-domain", value_name = "DOMAIN")]
        exclude: Vec<String>,
        /// Remove a domain from the excluded list; repeatable.
        #[arg(long = "include-domain", value_name = "DOMAIN")]
        include: Vec<String>,
        /// Access policy for items without their own.
        #[arg(long, value_enum, value_name = "POLICY")]
        default_policy: Option<DefaultPolicyArg>,
    },
    /// Write every saved sign-in, unencrypted, to a new CSV file (asks for the passphrase).
    Export {
        /// The file to create; an existing file is never overwritten.
        #[arg(long, short, value_name = "FILE")]
        output: std::path::PathBuf,
        /// Confirm that the file will hold your passwords in plain text.
        #[arg(long)]
        yes: bool,
    },
    /// Add sign-ins from a browser or password-manager CSV export.
    Import { file: std::path::PathBuf },
    /// Delete every saved sign-in, authenticator and passkey.
    Clear {
        /// Confirm the deletion.
        #[arg(long)]
        yes: bool,
    },
    /// Create a recovery key and print it once; any earlier key stops working.
    RecoveryKey {
        /// Confirm replacing any earlier recovery key.
        #[arg(long)]
        yes: bool,
    },
    /// Open a locked vault with its recovery key and choose a new passphrase.
    Recover,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PolicyArg {
    Always,
    #[value(name = "while_unlocked")]
    WhileUnlocked,
    Never,
    Default,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum DefaultPolicyArg {
    Always,
    #[value(name = "while_unlocked")]
    WhileUnlocked,
    Never,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum AutoLockArg {
    #[value(name = "immediately")]
    Immediately,
    #[value(name = "1m")]
    OneMinute,
    #[value(name = "5m")]
    FiveMinutes,
    #[value(name = "15m")]
    FifteenMinutes,
    #[value(name = "1h")]
    OneHour,
    #[value(name = "4h")]
    FourHours,
    #[value(name = "1d")]
    OneDay,
    #[value(name = "1w")]
    OneWeek,
    #[value(name = "never")]
    Never,
}

impl AutoLockArg {
    fn wire(self) -> VaultAutoLock {
        match self {
            Self::Immediately => VaultAutoLock::Immediately,
            Self::OneMinute => VaultAutoLock::OneMinute,
            Self::FiveMinutes => VaultAutoLock::FiveMinutes,
            Self::FifteenMinutes => VaultAutoLock::FifteenMinutes,
            Self::OneHour => VaultAutoLock::OneHour,
            Self::FourHours => VaultAutoLock::FourHours,
            Self::OneDay => VaultAutoLock::OneDay,
            Self::OneWeek => VaultAutoLock::OneWeek,
            Self::Never => VaultAutoLock::Never,
        }
    }
}

fn auto_lock_name(value: VaultAutoLock) -> &'static str {
    match value {
        VaultAutoLock::Immediately => "immediately",
        VaultAutoLock::OneMinute => "1 minute",
        VaultAutoLock::FiveMinutes => "5 minutes",
        VaultAutoLock::FifteenMinutes => "15 minutes",
        VaultAutoLock::OneHour => "1 hour",
        VaultAutoLock::FourHours => "4 hours",
        VaultAutoLock::OneDay => "1 day",
        VaultAutoLock::OneWeek => "1 week",
        VaultAutoLock::Never => "never",
    }
}

fn print_settings<W: Write>(stdout: &mut W, settings: &VaultSettings) -> Result<(), String> {
    let on = |value: bool| if value { "on" } else { "off" };
    let excluded = if settings.autofill.excluded_domains.is_empty() {
        "none".to_owned()
    } else {
        settings.autofill.excluded_domains.join(", ")
    };
    let biometrics = match (&settings.biometrics.mechanism, &settings.biometrics.reason) {
        (Some(mechanism), None) if settings.biometrics.available => mechanism.clone(),
        (_, Some(reason)) => format!("unavailable ({reason})"),
        _ => "unavailable".to_owned(),
    };
    let recovery = match (settings.recovery.configured, settings.recovery.current) {
        (true, true) => "set",
        (true, false) => "outdated; create a new one",
        _ => "not set",
    };
    writeln!(
        stdout,
        "auto-lock\t{}\nbiometric unlock\t{} ({biometrics})\nautofill\t{}\nauto-submit\t{}\nhttp pages\t{}\nexcluded domains\t{excluded}\ndefault policy\t{}\nincognito\tnever filled\nrecovery key\t{recovery}",
        auto_lock_name(settings.auto_lock),
        on(settings.biometric_unlock),
        on(settings.autofill.enabled),
        on(settings.autofill.auto_submit),
        on(settings.autofill.allow_insecure),
        policy_name(settings.default_policy),
    )
    .map_err(output_error)
}

/// Fresh authentication for export and recovery keys: the passphrase of a passphrase vault,
/// or the OS biometric prompt when a keyring vault has biometric unlock on.
async fn confirm_request<R: Read, E: Write>(
    client: &Client,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<VaultConfirmRequest, String> {
    let status: VaultStatus = client.get("/v1/vault").await?;
    if status.state != VaultState::Unlocked {
        return Err("the vault is locked; run `openside vault unlock` first".to_owned());
    }
    let mut request = VaultConfirmRequest {
        confirm: true,
        ..VaultConfirmRequest::default()
    };
    if status.backend == Some(VaultBackend::Passphrase) {
        request.passphrase = Some(hidden("Vault passphrase", stdin, stderr)?);
    } else {
        let settings: VaultSettings = client.get("/v1/vault/settings").await?;
        request.biometric = settings.biometric_unlock;
    }
    Ok(request)
}

/// Text of a CSV export: UTF-8 (with or without a byte-order mark) or UTF-16 with its
/// byte-order mark, which some password managers on Windows write.
fn decode_export(bytes: &[u8]) -> Option<String> {
    let utf16 = |bytes: &[u8], read: fn([u8; 2]) -> u16| {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units = zeroize::Zeroizing::new(
            bytes
                .chunks_exact(2)
                .map(|pair| read([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        );
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => std::str::from_utf8(bytes).ok().map(str::to_owned),
    }
}

fn write_new_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("could not write {}: {error}", path.display()));
    }
    Ok(())
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
            let keyring = status
                .backend
                .is_some_and(|backend| backend != VaultBackend::Passphrase);
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
                    // The daemon runs the biometric check only when biometric unlock is on.
                    Some(&VaultUnlockRequest {
                        passphrase,
                        biometric: keyring,
                    }),
                )
                .await?;
            print_status(stdout, json, &status)
        }
        VaultCommand::Unlock => {
            let status: VaultStatus = client.get("/v1/vault").await?;
            if status.state == VaultState::Unlocked {
                return print_status(stdout, json, &status);
            }
            let keyring = status
                .backend
                .is_some_and(|backend| backend != VaultBackend::Passphrase);
            let passphrase = (!keyring && status.backend.is_some())
                .then(|| hidden("Vault passphrase", stdin, stderr))
                .transpose()?;
            // A keyring vault with biometric unlock on asks for the OS prompt; otherwise the
            // daemon ignores the flag.
            let status: VaultStatus = client
                .json(
                    "POST",
                    "/v1/vault/unlock",
                    Some(&VaultUnlockRequest {
                        passphrase,
                        biometric: keyring,
                    }),
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
        VaultCommand::Settings {
            auto_lock,
            biometric_unlock,
            autofill,
            auto_submit,
            allow_insecure,
            exclude,
            include,
            default_policy,
        } => {
            let mut request = UpdateVaultSettingsRequest {
                auto_lock: auto_lock.map(AutoLockArg::wire),
                biometric_unlock: *biometric_unlock,
                default_policy: default_policy.map(|policy| match policy {
                    DefaultPolicyArg::Always => VaultPolicy::Always,
                    DefaultPolicyArg::WhileUnlocked => VaultPolicy::WhileUnlocked,
                    DefaultPolicyArg::Never => VaultPolicy::Never,
                }),
                ..UpdateVaultSettingsRequest::default()
            };
            let mut fill = UpdateVaultAutofillSettings {
                enabled: *autofill,
                auto_submit: *auto_submit,
                allow_insecure: *allow_insecure,
                excluded_domains: None,
            };
            if !exclude.is_empty() || !include.is_empty() {
                let current: VaultSettings = client.get("/v1/vault/settings").await?;
                let mut domains = current.autofill.excluded_domains;
                domains.retain(|domain| {
                    !include
                        .iter()
                        .any(|removed| removed.trim().eq_ignore_ascii_case(domain))
                });
                domains.extend(exclude.iter().cloned());
                fill.excluded_domains = Some(domains);
            }
            if fill != UpdateVaultAutofillSettings::default() {
                request.autofill = Some(fill);
            }
            let settings: VaultSettings = if request == UpdateVaultSettingsRequest::default() {
                client.get("/v1/vault/settings").await?
            } else {
                client
                    .json("PATCH", "/v1/vault/settings", Some(&request))
                    .await?
            };
            if json {
                return emit(stdout, &settings);
            }
            print_settings(stdout, &settings)
        }
        VaultCommand::Export { output, yes } => {
            if !*yes {
                return Err(
                    "the file will hold your passwords unencrypted; pass --yes to export"
                        .to_owned(),
                );
            }
            if output.exists() {
                return Err(format!("{} already exists", output.display()));
            }
            let request = confirm_request(client, stdin, stderr).await?;
            let mut export: VaultExport = client
                .json("POST", "/v1/vault/export", Some(&request))
                .await?;
            let written = write_new_private(output, export.content.as_bytes());
            export.content.zeroize();
            written?;
            if json {
                return emit(
                    stdout,
                    &serde_json::json!({
                        "path": output,
                        "exported": export.exported,
                        "skipped": export.skipped,
                    }),
                );
            }
            writeln!(
                stdout,
                "Exported {} sign-ins to {} (unencrypted; delete it once imported){}",
                export.exported,
                output.display(),
                if export.skipped > 0 {
                    format!("; {} passkeys cannot be exported", export.skipped)
                } else {
                    String::new()
                },
            )
            .map_err(output_error)
        }
        VaultCommand::Import { file } => {
            let bytes = zeroize::Zeroizing::new(
                std::fs::read(file)
                    .map_err(|error| format!("could not read {}: {error}", file.display()))?,
            );
            let csv = decode_export(&bytes).ok_or_else(|| {
                format!(
                    "{} is not UTF-8 or UTF-16 text; export it again as CSV",
                    file.display()
                )
            })?;
            let mut request = ImportPasswordsRequest { csv };
            let sent = client
                .json::<ImportCategoryReport, _>("POST", "/v1/import/passwords", Some(&request))
                .await;
            request.csv.zeroize();
            let report = sent?;
            if json {
                return emit(stdout, &report);
            }
            writeln!(
                stdout,
                "Imported {} of {} sign-ins ({} skipped)",
                report.imported, report.read, report.skipped
            )
            .map_err(output_error)?;
            if let Some(note) = &report.note {
                writeln!(stdout, "{note}").map_err(output_error)?;
            }
            Ok(())
        }
        VaultCommand::Clear { yes } => {
            if !*yes {
                return Err(
                    "this deletes every saved sign-in, authenticator and passkey; pass --yes"
                        .to_owned(),
                );
            }
            let request = confirm_request(client, stdin, stderr).await?;
            let result: VaultClearResult = client
                .json("POST", "/v1/vault/clear", Some(&request))
                .await?;
            if json {
                return emit(stdout, &result);
            }
            writeln!(stdout, "Deleted {} items", result.removed).map_err(output_error)
        }
        VaultCommand::RecoveryKey { yes } => {
            if !*yes {
                return Err(
                    "a new recovery key replaces any earlier one; pass --yes to create it"
                        .to_owned(),
                );
            }
            let request = confirm_request(client, stdin, stderr).await?;
            let mut key: VaultRecoveryKey = client
                .json("POST", "/v1/vault/recovery-key", Some(&request))
                .await?;
            let printed = if json {
                emit(stdout, &key)
            } else {
                writeln!(
                    stdout,
                    "{}\nKeep this key somewhere safe. It is shown only once and opens your vault if you forget how to unlock it.",
                    key.recovery_key
                )
                .map_err(output_error)
            };
            key.recovery_key.zeroize();
            printed
        }
        VaultCommand::Recover => {
            let status: VaultStatus = client.get("/v1/vault").await?;
            if status.state == VaultState::Unlocked {
                return Err("the vault is already unlocked".to_owned());
            }
            let recovery_key = hidden("Recovery key", stdin, stderr)?;
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
            let request = VaultRecoverRequest {
                recovery_key,
                passphrase,
            };
            let status: VaultStatus = client
                .json("POST", "/v1/vault/recover", Some(&request))
                .await?;
            print_status(stdout, json, &status)
        }
    }
}
