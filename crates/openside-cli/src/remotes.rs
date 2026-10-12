//! Remote daemon profiles: `openside connect`, `openside remote profiles list|rm`, and
//! the `--remote NAME` / `OPENSIDE_REMOTE` connection every command accepts.
//!
//! A profile records how to reach the daemon (`direct` HTTPS such as Tailscale Serve or a
//! Cloudflare Tunnel, or `relay` with the daemon's routing id and X25519 key) in
//! `<config dir>/remotes.toml` (0600). The device token goes to the OS keyring when one is
//! available — the login keychain on macOS, Credential Manager on Windows, the Secret
//! Service through `secret-tool` on Linux — and otherwise into the same 0600 file.
//! `OPENSIDE_REMOTE_TOKEN_STORE=file` forces the file (headless machines, tests).

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Subcommand};
use openside_proto::{ExchangePairingRequest, PairDeviceResponse};
use serde::{Deserialize, Serialize};

use crate::{client::Client, emit, output_error, prompt};

const KEYRING_SERVICE: &str = "openside-remote";

/// How a profile reaches its daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Transport {
    /// HTTPS to the daemon's public URL (Tailscale Serve, Cloudflare Tunnel, a TLS proxy).
    Direct,
    /// CLI/MCP Noise transport to the pairing-pinned `daemon_key`; relay-hosted PWA code is trusted separately.
    Relay,
}

/// One saved remote daemon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub(crate) transport: Transport,
    /// Daemon origin (`direct`) or relay origin (`relay`).
    pub(crate) url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) daemon_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) daemon_key: Option<String>,
    pub(crate) device_id: String,
    #[serde(default)]
    pub(crate) scopes: Vec<String>,
    /// Where the token is: `keyring` or `file`.
    pub(crate) token_store: String,
    /// The device token when `token_store = "file"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemotesFile {
    #[serde(default)]
    remote: BTreeMap<String, Profile>,
}

/// A parsed pairing link (the QR code of `openside pair`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairingLink {
    /// `https://host[:port]` the link points at: the daemon (direct) or the relay.
    pub(crate) origin: String,
    pub(crate) code: String,
    /// Relay links: daemon routing id and static public key.
    pub(crate) relay: Option<(String, [u8; 32])>,
}

/// Parse `https://host/pair#<code>` (direct) or
/// `https://relay/pair#c=<code>&t=relay&d=<daemon id>&k=<daemon key>` (relay).
pub(crate) fn parse_link(text: &str) -> Result<PairingLink, String> {
    let invalid = || "not an openside pairing link (expected https://…/pair#…)".to_owned();
    let url = url::Url::parse(text.trim()).map_err(|_| invalid())?;
    if url.path() != "/pair" || url.query().is_some() {
        return Err(invalid());
    }
    let fragment = url
        .fragment()
        .filter(|f| !f.is_empty())
        .ok_or_else(invalid)?;
    let origin = url.origin().ascii_serialization();
    // Validates https / loopback-only http the same way the connection will.
    openside_relay::net::Origin::parse(&origin).map_err(|error| error.to_string())?;
    if !fragment.contains('=') {
        return Ok(PairingLink {
            origin,
            code: fragment.to_owned(),
            relay: None,
        });
    }
    let fields: BTreeMap<String, String> = url::form_urlencoded::parse(fragment.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let code = fields
        .get("c")
        .filter(|c| !c.is_empty())
        .ok_or_else(invalid)?;
    match fields.get("t").map(String::as_str) {
        None => Ok(PairingLink {
            origin,
            code: code.clone(),
            relay: None,
        }),
        Some("relay") => {
            let id = fields
                .get("d")
                .ok_or("relay pairing link has no daemon id")?;
            let key = fields
                .get("k")
                .and_then(|key| openside_relay::noise::decode_key(key))
                .ok_or("relay pairing link has no valid daemon key")?;
            if openside_relay::noise::daemon_id(&key) != *id {
                return Err("relay pairing link: daemon id does not match its key".into());
            }
            Ok(PairingLink {
                origin,
                code: code.clone(),
                relay: Some((id.clone(), key)),
            })
        }
        Some(other) => Err(format!("unsupported transport {other:?} in pairing link")),
    }
}

/// `<config dir>/remotes.toml`, beside `config.toml`.
pub(crate) fn file_path() -> Result<PathBuf, String> {
    let paths = openside_config::Paths::discover();
    paths
        .config_file
        .parent()
        .map(|dir| dir.join("remotes.toml"))
        .ok_or_else(|| "cannot resolve the configuration directory".to_owned())
}

fn read_file(path: &Path) -> Result<RemotesFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|_| format!("{}: invalid remote profile configuration", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RemotesFile::default()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn write_file(path: &Path, file: &RemotesFile) -> Result<(), String> {
    let text = toml::to_string(file).map_err(|_| "cannot encode remotes.toml")?;
    let dir = path
        .parent()
        .ok_or("remotes.toml has no parent directory")?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    // Write to a private temporary file and rename, so a token is never briefly world-readable
    // and a crash cannot leave a half-written file.
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|error| format!("cannot write in {}: {error}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    temp.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    temp.persist(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(())
}

fn file_store_forced() -> bool {
    std::env::var("OPENSIDE_REMOTE_TOKEN_STORE").is_ok_and(|value| value == "file")
}

mod keyring_store {
    //! Thin wrappers over the platform secret store; every error means "not available".

    #[cfg(any(target_os = "macos", windows))]
    pub(super) fn set(name: &str, token: &str) -> bool {
        keyring::Entry::new(super::KEYRING_SERVICE, name)
            .and_then(|entry| entry.set_password(token))
            .is_ok()
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(super) fn get(name: &str) -> Option<String> {
        keyring::Entry::new(super::KEYRING_SERVICE, name)
            .and_then(|entry| entry.get_password())
            .ok()
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(super) fn delete(name: &str) {
        if let Ok(entry) = keyring::Entry::new(super::KEYRING_SERVICE, name) {
            let _ = entry.delete_credential();
        }
    }

    // libsecret's CLI keeps D-Bus and its native libraries out of this binary; without it
    // (or without a running Secret Service) the token goes to the private file instead.
    #[cfg(not(any(target_os = "macos", windows)))]
    fn secret_tool(args: &[&str], input: Option<&str>) -> Option<String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let program = openside_config::find_on_path("secret-tool")?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(input.as_bytes()).ok()?;
        }
        let output = child.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(super) fn set(name: &str, token: &str) -> bool {
        let label = format!("openside remote {name}");
        secret_tool(
            &[
                "store",
                "--label",
                &label,
                "service",
                super::KEYRING_SERVICE,
                "account",
                name,
            ],
            Some(token),
        )
        .is_some()
            && get(name).as_deref() == Some(token)
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(super) fn get(name: &str) -> Option<String> {
        secret_tool(
            &["lookup", "service", super::KEYRING_SERVICE, "account", name],
            None,
        )
        .map(|value| value.trim_end_matches('\n').to_owned())
        .filter(|value| !value.is_empty())
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(super) fn delete(name: &str) {
        let _ = secret_tool(
            &["clear", "service", super::KEYRING_SERVICE, "account", name],
            None,
        );
    }
}

/// The profile and its device token.
pub(crate) fn load(name: &str) -> Result<(Profile, String), String> {
    let path = file_path()?;
    let file = read_file(&path)?;
    let profile = file.remote.get(name).cloned().ok_or_else(|| {
        format!("no remote named {name:?}; pair one with `openside connect` (see `openside remote profiles list`)")
    })?;
    let token = match profile.token_store.as_str() {
        "file" => profile.token.clone(),
        _ => keyring_store::get(name),
    }
    .ok_or_else(|| {
        format!("the device token for remote {name:?} is missing; run `openside connect` again")
    })?;
    Ok((profile, token))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Store a newly paired profile; the token goes to the keyring when it can.
pub(crate) fn save(
    name: &str,
    mut profile: Profile,
    token: &str,
    force: bool,
) -> Result<String, String> {
    let path = file_path()?;
    let mut file = read_file(&path)?;
    if file.remote.contains_key(name) && !force {
        return Err(format!("a remote named {name:?} already exists; pass --force to replace it or --name to choose another"));
    }
    if !file_store_forced() && keyring_store::set(name, token) {
        profile.token_store = "keyring".into();
        profile.token = None;
    } else {
        profile.token_store = "file".into();
        profile.token = Some(token.to_owned());
    }
    let store = profile.token_store.clone();
    file.remote.insert(name.to_owned(), profile);
    write_file(&path, &file)?;
    Ok(store)
}

#[derive(Args)]
pub(crate) struct ConnectArgs {
    /// Pairing link; omit to read from stdin or a hidden prompt (recommended).
    link: Option<String>,
    /// Profile name used with `--remote NAME`; default: the host name's first label.
    #[arg(long)]
    name: Option<String>,
    /// Replace an existing profile with the same name.
    #[arg(long)]
    force: bool,
}

#[derive(Subcommand)]
pub(crate) enum RemotesCommand {
    /// Saved remote daemons (tokens are never shown).
    List,
    /// Forget a remote and delete its stored token. The daemon keeps the device until you
    /// run `openside device revoke <id>` there.
    Rm {
        /// Profile name.
        name: String,
    },
}

fn default_name(link: &PairingLink) -> String {
    let host = url::Url::parse(&link.origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let label = host.split('.').next().unwrap_or_default();
    if valid_name(label) {
        label.to_owned()
    } else {
        "remote".to_owned()
    }
}

fn read_pairing_link<R: Read, E: Write>(
    link: Option<&str>,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<PairingLink, String> {
    let text = match link {
        Some(link) => {
            writeln!(stderr, "Warning: a pairing link in command arguments may appear in shell history and process listings. Prefer `openside connect` and enter it at the hidden prompt or pipe it through stdin.")
                .map_err(output_error)?;
            link.to_owned()
        }
        None => prompt::read_hidden("Pairing link", stdin, stderr)?,
    };
    parse_link(&text)
}

/// Exchange the one-time code and save the profile without requiring credentials in argv.
pub(crate) async fn connect<R: Read, W: Write, E: Write>(
    args: &ConnectArgs,
    json: bool,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    let link = read_pairing_link(args.link.as_deref(), stdin, stderr)?;
    let name = args.name.clone().unwrap_or_else(|| default_name(&link));
    if !valid_name(&name) {
        return Err("--name may use letters, digits, '-', '_' and '.' (at most 64)".into());
    }
    if !args.force && read_file(&file_path()?)?.remote.contains_key(&name) {
        return Err(format!("a remote named {name:?} already exists; pass --force to replace it or --name to choose another"));
    }
    let client = Client::pairing(&link)?;
    let paired: PairDeviceResponse = client
        .json(
            "POST",
            "/v1/pairing/exchange",
            Some(&ExchangePairingRequest {
                code: link.code.clone(),
            }),
        )
        .await?;
    if paired.kind == openside_proto::DeviceKind::PersonDevice {
        writeln!(stderr, "This link pairs a person's device, not an agent client. For CLI/MCP self-approval protection, ask the daemon's operator for a new link from openside pair --agent.").map_err(output_error)?;
    }
    let scopes: Vec<String> = paired
        .scopes
        .iter()
        .filter_map(|scope| {
            serde_json::to_value(scope)
                .ok()?
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    let profile = Profile {
        transport: if link.relay.is_some() {
            Transport::Relay
        } else {
            Transport::Direct
        },
        url: link.origin.clone(),
        daemon_id: link.relay.as_ref().map(|(id, _)| id.clone()),
        daemon_key: link
            .relay
            .as_ref()
            .map(|(_, key)| openside_relay::noise::encode_key(key)),
        device_id: paired.device_id.clone(),
        scopes: scopes.clone(),
        token_store: String::new(),
        token: None,
    };
    let store = save(&name, profile.clone(), &paired.token, args.force)?;
    if json {
        return emit(
            stdout,
            &serde_json::json!({
                "name": name, "transport": profile.transport, "url": profile.url,
                "device_id": profile.device_id, "scopes": scopes, "token_store": store,
                "kind": paired.kind,
            }),
        );
    }
    writeln!(
        stdout,
        "Paired as device {} ({}). Token stored in the {}.",
        paired.device_id,
        scopes.join(", "),
        if store == "keyring" {
            "OS keyring"
        } else {
            "private remotes.toml file"
        }
    )
    .map_err(output_error)?;
    writeln!(
        stdout,
        "Use it with: openside --remote {name} <command>   (or OPENSIDE_REMOTE={name})"
    )
    .map_err(output_error)
}

pub(crate) fn remotes<W: Write>(
    command: &RemotesCommand,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let path = file_path()?;
    match command {
        RemotesCommand::List => {
            let file = read_file(&path)?;
            for (name, profile) in &file.remote {
                if json {
                    emit(
                        stdout,
                        &serde_json::json!({
                            "name": name, "transport": profile.transport, "url": profile.url,
                            "device_id": profile.device_id, "scopes": profile.scopes,
                            "token_store": profile.token_store,
                        }),
                    )?;
                } else {
                    let transport = match profile.transport {
                        Transport::Direct => "direct",
                        Transport::Relay => "relay",
                    };
                    writeln!(
                        stdout,
                        "{name}\t{transport}\t{}\t{}",
                        profile.url, profile.device_id
                    )
                    .map_err(output_error)?;
                }
            }
            Ok(())
        }
        RemotesCommand::Rm { name } => {
            let mut file = read_file(&path)?;
            let profile = file
                .remote
                .remove(name)
                .ok_or_else(|| format!("no remote named {name:?}"))?;
            if profile.token_store == "keyring" {
                keyring_store::delete(name);
            }
            write_file(&path, &file)?;
            if json {
                return emit(
                    stdout,
                    &serde_json::json!({"removed": name, "device_id": profile.device_id}),
                );
            }
            writeln!(
                stdout,
                "Removed {name}. The daemon still lists device {}; revoke it there with `openside device revoke {}`.",
                profile.device_id, profile.device_id
            )
            .map_err(output_error)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn stdin_pairing_keeps_the_credential_out_of_diagnostics() {
        let mut input = &b"https://daemon.test/pair#non-secret-sentinel\n"[..];
        let mut stderr = Vec::new();
        let link = read_pairing_link(None, &mut input, &mut stderr).unwrap();
        assert_eq!(link.code, "non-secret-sentinel");
        assert!(!String::from_utf8(stderr)
            .unwrap()
            .contains("non-secret-sentinel"));
        let mut stderr = Vec::new();
        read_pairing_link(
            Some("https://daemon.test/pair#argv-sentinel"),
            &mut &b""[..],
            &mut stderr,
        )
        .unwrap();
        let diagnostic = String::from_utf8(stderr).unwrap();
        assert!(diagnostic.contains("process listings"));
        assert!(!diagnostic.contains("argv-sentinel"));
    }

    #[test]
    fn malformed_profile_diagnostics_do_not_contain_source_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.toml");
        std::fs::write(&path, "token = 'diagnostic-sentinel'\nbroken").unwrap();
        let diagnostic = read_file(&path).unwrap_err();
        assert!(!diagnostic.contains("diagnostic-sentinel"));
    }
}
