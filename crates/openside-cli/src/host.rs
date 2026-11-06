//! `openside host profiles|list|use|status` and `openside logout`.
//!
//! A session host is the daemon a command talks to: `local` (this computer's daemon socket) or
//! a remote daemon saved by `openside login`, named by its saved name or its device id.

use std::io::Write;

use clap::Subcommand;
use serde_json::json;

use crate::{
    client::Client,
    emit, output_error,
    remotes::{self, Transport},
    settings::{self, ProfileExt, LOCAL_HOST},
};

#[derive(Subcommand)]
pub(crate) enum HostCommand {
    /// List browser profiles for the selected host and account
    Profiles,
    /// List available session hosts
    List,
    /// Set the default session host
    Use {
        #[arg(value_name = "host")]
        host: String,
    },
    /// Show the default session host
    Status,
}

/// A host argument as a saved profile name, or `None` for the local daemon.
pub(crate) fn canonical(host: &str) -> Result<Option<String>, String> {
    if host == LOCAL_HOST {
        Ok(None)
    } else {
        remotes::find(host).map(Some)
    }
}

fn transport(transport: Transport) -> &'static str {
    match transport {
        Transport::Direct => "direct",
        Transport::Relay => "relay",
    }
}

/// Commands that read only local files. `selected` is the host this invocation resolved to
/// (`None` = local) and `profile` the browser profile it resolved to.
pub(crate) fn local<W: Write>(
    command: &HostCommand,
    selected: Option<&str>,
    profile: openside_proto::CliProfile,
    json_output: bool,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        HostCommand::Profiles => {
            let host = selected.unwrap_or(LOCAL_HOST);
            for candidate in openside_proto::CliProfile::ALL {
                let current = candidate == profile;
                if json_output {
                    emit(
                        stdout,
                        &json!({"host": host, "profile_id": candidate.id(), "default": current,
                                "description": candidate.description()}),
                    )?;
                } else {
                    let mark = if current { '*' } else { ' ' };
                    writeln!(
                        stdout,
                        "{mark} {}\t{}",
                        candidate.id(),
                        candidate.description()
                    )
                    .map_err(output_error)?;
                }
            }
            Ok(())
        }
        HostCommand::List => {
            let default = settings::load()?.host;
            let is_default = |name: &str| default.as_deref().unwrap_or(LOCAL_HOST) == name;
            if json_output {
                emit(
                    stdout,
                    &json!({"name": LOCAL_HOST, "transport": "socket", "default": is_default(LOCAL_HOST)}),
                )?;
            } else {
                let mark = if is_default(LOCAL_HOST) { '*' } else { ' ' };
                writeln!(stdout, "{mark} {LOCAL_HOST}\tsocket\tthis computer")
                    .map_err(output_error)?;
            }
            for (name, profile) in remotes::saved()? {
                if json_output {
                    emit(
                        stdout,
                        &json!({
                            "name": name, "transport": profile.transport, "url": profile.url,
                            "device_id": profile.device_id, "scopes": profile.scopes,
                            "token_store": profile.token_store, "default": is_default(&name),
                        }),
                    )?;
                } else {
                    let mark = if is_default(&name) { '*' } else { ' ' };
                    writeln!(
                        stdout,
                        "{mark} {name}\t{}\t{}\t{}",
                        transport(profile.transport),
                        profile.url,
                        profile.device_id
                    )
                    .map_err(output_error)?;
                }
            }
            Ok(())
        }
        HostCommand::Use { host } => {
            let name = canonical(host)?;
            let mut saved = settings::load()?;
            saved.host = name.clone();
            settings::save(&saved)?;
            let name = name.as_deref().unwrap_or(LOCAL_HOST);
            if json_output {
                emit(stdout, &json!({"default_host": name}))
            } else {
                writeln!(stdout, "Default host: {name}").map_err(output_error)
            }
        }
        HostCommand::Status => unreachable!("host status asks the daemon"),
    }
}

/// `host status`: the selected host and whether its daemon answers.
pub(crate) async fn status<W: Write>(
    selected: Option<&str>,
    client: &Client,
    json_output: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let online = client
        .get::<openside_proto::Health>("/v1/health")
        .await
        .map(|health| health.ok);
    let state = match &online {
        Ok(true) => "online".to_owned(),
        Ok(false) => "starting".to_owned(),
        Err(error) => format!("offline ({error})"),
    };
    let (name, transport_name, url) = match selected {
        None => (LOCAL_HOST.to_owned(), "socket", String::new()),
        Some(name) => {
            let (profile, _) = remotes::load(name)?;
            (name.to_owned(), transport(profile.transport), profile.url)
        }
    };
    if json_output {
        return emit(
            stdout,
            &json!({"host": name, "transport": transport_name, "url": url,
                    "online": online.unwrap_or(false)}),
        );
    }
    writeln!(stdout, "{name}\t{transport_name}\t{url}\t{state}").map_err(output_error)
}

/// `openside logout`: revoke this computer's device on the host, then forget the host.
pub(crate) async fn logout<W: Write, E: Write>(
    selected: Option<&str>,
    json_output: bool,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    let name = selected.ok_or(
        "the local daemon is reached through this computer's socket; there is nothing to sign out of (pass --host NAME)",
    )?;
    let (profile, _) = remotes::load(name)?;
    let revoked = match Client::remote(name) {
        Ok(client) => client
            .send(
                "DELETE",
                &format!(
                    "/v1/devices/{}",
                    crate::client::encode_segment(&profile.device_id)
                ),
            )
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };
    remotes::forget(name)?;
    let mut saved = settings::load()?;
    if saved.host.as_deref() == Some(name) {
        saved.host = None;
        settings::save(&saved)?;
    }
    if let Err(error) = &revoked {
        writeln!(
            stderr,
            "openside: warning: could not revoke device {} on {name} ({error}); revoke it there with `openside device revoke {}`",
            profile.device_id, profile.device_id
        )
        .map_err(output_error)?;
    }
    if json_output {
        return emit(
            stdout,
            &json!({"host": name, "device_id": profile.device_id, "revoked": revoked.is_ok()}),
        );
    }
    writeln!(stdout, "Signed out of {name}").map_err(output_error)
}
