//! `openside remote setup tailscale|cloudflare|relay|off`, `openside remote status` and
//! `openside remote local-access`; reaching other daemons is `openside login` / `--host` (see
//! remotes.rs).
//!
//! Setup edits `config.toml` in place (comments kept) and then runs the doctor's remote
//! checks; the daemon picks the change up on its next start. Status asks the running daemon
//! which transports are up and which public URL each gives.

use std::{io::Write, path::PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use openside_config::doctor::{self, DoctorOptions, Status};
use openside_proto::RemoteStatus;

use crate::{client::Client, emit, output_error};

const DEFAULT_LISTEN: &str = "127.0.0.1:47820";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum SetupMode {
    /// Tailscale Serve: devices in your tailnet reach https://<machine>.<tailnet>.ts.net.
    Tailscale,
    /// Cloudflare Tunnel run by the daemon: a quick trycloudflare.com URL, or a named
    /// tunnel with a stable hostname (--named --hostname --token-file).
    Cloudflare,
    /// Your own openside-relay: no inbound port. CLI/MCP are end-to-end encrypted;
    /// the relay-served web app requires trust in its operator.
    Relay,
    /// Turn every managed transport off.
    Off,
}

#[derive(Args)]
pub(crate) struct SetupArgs {
    mode: SetupMode,
    /// Configuration file to edit; default: the discovered config.toml.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// tailscale: also run `tailscale serve` (otherwise the command is printed).
    #[arg(long)]
    apply: bool,
    /// tailscale: HTTPS port for openside's Serve handler (remote.tailscale.https_port;
    /// default 443). A port another app already serves is refused with a free one named.
    #[arg(long, value_name = "PORT", value_parser = clap::value_parser!(u16).range(1..))]
    https_port: Option<u16>,
    /// cloudflare: use a named tunnel instead of a quick tunnel.
    #[arg(long)]
    named: bool,
    /// cloudflare --named: the public hostname routed to the tunnel.
    #[arg(long, value_name = "HOST")]
    hostname: Option<String>,
    /// cloudflare --named: file with the tunnel token; relay: file with the registration token.
    #[arg(long, value_name = "PATH")]
    token_file: Option<PathBuf>,
    /// relay: the relay's https:// origin.
    #[arg(long, value_name = "URL")]
    relay_url: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum RemoteCommand {
    /// Enable a transport (or `off`) in config.toml and check it.
    Setup(SetupArgs),
    /// Transports the running daemon has up, with their public URLs.
    Status,
    /// Bootstrap the browser on this computer with local-person authority (local socket only).
    LocalAccess,
}

fn table<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    path: &[&str],
) -> Result<&'a mut toml_edit::Table, String> {
    let mut current = doc.as_table_mut();
    for key in path {
        let entry = current
            .entry(key)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        current = entry
            .as_table_mut()
            .ok_or_else(|| format!("config key {key} is not a table"))?;
    }
    Ok(current)
}

fn modes(doc: &toml_edit::DocumentMut) -> Vec<String> {
    match doc.get("remote").and_then(|remote| remote.get("mode")) {
        Some(item) if item.as_str() == Some("off") => Vec::new(),
        Some(item) => match (item.as_str(), item.as_array()) {
            (Some(one), _) => vec![one.to_owned()],
            (_, Some(list)) => list
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

fn set_modes(doc: &mut toml_edit::DocumentMut, modes: &[String]) -> Result<(), String> {
    let mut array = toml_edit::Array::new();
    for mode in modes {
        array.push(mode.as_str());
    }
    table(doc, &["remote"])?.insert("mode", toml_edit::value(array));
    Ok(())
}

/// What `openside remote setup tailscale` reads from the local Tailscale client.
struct TailscaleHost {
    /// MagicDNS name of this machine, without the trailing dot.
    dns_name: String,
    /// `tailscale serve status --json`.
    serve: serde_json::Value,
}

fn tailscale_json(bin: &std::path::Path, args: &[&str]) -> Result<serde_json::Value, String> {
    let output = std::process::Command::new(bin)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run tailscale: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`tailscale {}` failed; is Tailscale running and logged in?",
            args.join(" ")
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|_| format!("`tailscale {}` returned invalid JSON", args.join(" ")))
}

fn tailscale_host() -> Result<TailscaleHost, String> {
    let bin = openside_config::find_on_path("tailscale")
        .ok_or("tailscale is not installed or not on PATH")?;
    let status = tailscale_json(&bin, &["status", "--json"])?;
    let dns_name = status["Self"]["DNSName"]
        .as_str()
        .map(|name| name.trim_end_matches('.').to_owned())
        .filter(|name| !name.is_empty())
        .ok_or("tailscale status has no DNS name for this machine (MagicDNS off?)")?;
    let serve = tailscale_json(&bin, &["serve", "status", "--json"])?;
    Ok(TailscaleHost { dns_name, serve })
}

/// The `tailscale serve` argv that gives openside `port`, or `None` when it already has it.
fn serve_command(
    serve: &serde_json::Value,
    port: u16,
    address: &str,
) -> Result<Option<Vec<String>>, String> {
    use openside_config::tailscale::{free_https_port, serve_slot, ServeSlot};
    let target = format!("http://{address}");
    match serve_slot(serve, port, &target) {
        ServeSlot::Ours(_) => Ok(None),
        ServeSlot::Taken(holder) => Err(format!(
            "Tailscale Serve port {port} on this computer belongs to another app ({holder}); openside never replaces it.{}",
            free_https_port(serve)
                .map(|free| format!(" Run this again with --https-port {free} (free)."))
                .unwrap_or_default()
        )),
        ServeSlot::Free => Ok(Some(vec![
            "serve".into(),
            "--bg".into(),
            format!("--https={port}"),
            "--set-path=/".into(),
            target,
        ])),
    }
}

/// Run the `tailscale serve` argv (when given) and only then write `text` to `path`, so a
/// failed command leaves the config as it was; a failed write turns that handler off again.
fn serve_then_write(
    path: &std::path::Path,
    text: &str,
    serve: Option<(&std::path::Path, &[String])>,
) -> Result<(), String> {
    if let Some((bin, command)) = serve {
        let status = std::process::Command::new(bin)
            .args(command)
            .status()
            .map_err(|error| format!("cannot run tailscale serve: {error}"))?;
        if !status.success() {
            return Err(format!(
                "`tailscale {}` failed; {} was not changed",
                command.join(" "),
                path.display()
            ));
        }
    }
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text));
    if let Err(error) = written {
        if let Some((bin, command)) = serve {
            let https = command.iter().find(|arg| arg.starts_with("--https="));
            let undone = https.is_some_and(|https| {
                std::process::Command::new(bin)
                    .args(["serve", https.as_str(), "--set-path=/", "off"])
                    .status()
                    .is_ok_and(|status| status.success())
            });
            if !undone {
                return Err(format!(
                    "cannot write {}: {error}; turn the new handler off with `tailscale serve {} --set-path=/ off`",
                    path.display(),
                    https.map_or("--https=<port>", String::as_str)
                ));
            }
        }
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    Ok(())
}

/// Edit the config document for `args`; returns lines to print and, for Tailscale, the
/// `tailscale` argv `--apply` runs (`None` when openside already has its handler). Separated
/// from I/O so tests can drive it without Tailscale or a real config file.
fn apply(
    doc: &mut toml_edit::DocumentMut,
    args: &SetupArgs,
    tailscale: impl FnOnce() -> Result<TailscaleHost, String>,
) -> Result<(Vec<String>, Option<Vec<String>>), String> {
    let mut serve = None;
    let mut notes = Vec::new();
    let mut enabled = modes(doc);
    let listen =
        |doc: &mut toml_edit::DocumentMut, notes: &mut Vec<String>| -> Result<String, String> {
            let daemon = table(doc, &["daemon"])?;
            if let Some(listen) = daemon.get("listen").and_then(|v| v.as_str()) {
                return Ok(listen.to_owned());
            }
            daemon.insert("listen", toml_edit::value(DEFAULT_LISTEN));
            notes.push(format!(
                "Set daemon.listen = \"{DEFAULT_LISTEN}\" (loopback only)."
            ));
            Ok(DEFAULT_LISTEN.to_owned())
        };
    let name = match args.mode {
        SetupMode::Off => {
            set_modes(doc, &[])?;
            notes.push("Remote transports are off. Paired devices keep their tokens; revoke them with `openside device revoke`.".into());
            return Ok((notes, None));
        }
        SetupMode::Tailscale => {
            let address = listen(doc, &mut notes)?;
            let host = tailscale()?;
            let configured = doc
                .get("remote")
                .and_then(|remote| remote.get("tailscale"))
                .and_then(|tailscale| tailscale.get("https_port"))
                .and_then(toml_edit::Item::as_integer)
                .and_then(|port| u16::try_from(port).ok());
            let port = args
                .https_port
                .or(configured)
                .unwrap_or(openside_config::tailscale::DEFAULT_HTTPS_PORT);
            serve = serve_command(&host.serve, port, &address)?;
            if args.https_port.is_some() {
                table(doc, &["remote", "tailscale"])?
                    .insert("https_port", toml_edit::value(i64::from(port)));
            }
            let url = openside_config::tailscale::serve_origin(&host.dns_name, port);
            table(doc, &["remote"])?.insert("public_url", toml_edit::value(url.as_str()));
            notes.push(format!("Phones in your tailnet will use {url}"));
            match &serve {
                Some(command) => notes.push(format!(
                    "Tailscale Serve command: tailscale {}",
                    command.join(" ")
                )),
                None => notes.push(format!(
                    "Tailscale Serve already sends port {port} to openside."
                )),
            }
            "tailscale"
        }
        SetupMode::Cloudflare => {
            listen(doc, &mut notes)?;
            let cloudflare = table(doc, &["remote", "cloudflare"])?;
            if args.named {
                let hostname = args
                    .hostname
                    .as_deref()
                    .ok_or("--named needs --hostname (the public hostname routed to the tunnel)")?;
                cloudflare.insert("tunnel", toml_edit::value("named"));
                cloudflare.insert("hostname", toml_edit::value(hostname));
                if let Some(file) = &args.token_file {
                    cloudflare.insert("token_file", toml_edit::value(file.display().to_string()));
                } else {
                    notes.push(
                        "No --token-file: the daemon reads OPENSIDE_CLOUDFLARE_TOKEN.".into(),
                    );
                }
                notes.push(format!("Devices will use https://{hostname}"));
            } else {
                cloudflare.insert("tunnel", toml_edit::value("quick"));
                notes.push("Quick tunnel: the daemon prints a new https://*.trycloudflare.com address each time it starts; pair after it is up (`openside remote status`).".into());
            }
            "cloudflare"
        }
        SetupMode::Relay => {
            let url = args
                .relay_url
                .as_deref()
                .ok_or("relay needs --relay-url https://relay.example.com")?;
            openside_relay::net::Origin::parse(url).map_err(|error| error.to_string())?;
            let relay = table(doc, &["remote", "relay"])?;
            relay.insert("url", toml_edit::value(url.trim_end_matches('/')));
            if let Some(file) = &args.token_file {
                relay.insert("token_file", toml_edit::value(file.display().to_string()));
            } else {
                notes.push("No --token-file: the daemon reads OPENSIDE_RELAY_TOKEN.".into());
            }
            notes.push("Pair devices with `openside pair --via relay`; enter the link at `openside login` (hidden prompt or stdin). The CLI encrypts traffic to the daemon; trust the operator that serves the browser app.".into());
            "relay"
        }
    };
    if !enabled.iter().any(|mode| mode == name) {
        enabled.push(name.to_owned());
    }
    set_modes(doc, &enabled)?;
    Ok((notes, serve))
}

pub(crate) async fn setup<W: Write>(
    args: &SetupArgs,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let path = match &args.config {
        Some(path) => path.clone(),
        None => openside_config::Paths::discover().config_file,
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let (notes, serve) = apply(&mut doc, args, tailscale_host)?;
    let bin = match (args.apply, &serve) {
        (true, Some(_)) => Some(
            openside_config::find_on_path("tailscale")
                .ok_or("tailscale is not installed or not on PATH")?,
        ),
        _ => None,
    };
    serve_then_write(
        &path,
        &doc.to_string(),
        bin.as_deref().zip(serve.as_deref()),
    )?;
    let report = match openside_config::load(Some(&path)) {
        Ok(loaded) => doctor::run(&loaded, DoctorOptions { vault: None }).await,
        Err(error) => doctor::DoctorReport {
            checks: vec![doctor::Check {
                id: "config".into(),
                status: Status::Fail,
                summary: error.to_string(),
                next_step: Some("Fix the configuration file, then run the setup again.".into()),
            }],
        },
    };
    let checks: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.id.starts_with("remote.") || check.id == "config")
        .collect();
    if json {
        return emit(
            stdout,
            &serde_json::json!({"config": path, "notes": notes, "checks": checks}),
        );
    }
    writeln!(stdout, "Updated {}.", path.display()).map_err(output_error)?;
    for note in &notes {
        writeln!(stdout, "{note}").map_err(output_error)?;
    }
    for check in checks {
        let mark = match check.status {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        };
        writeln!(stdout, "[{mark}] {}: {}", check.id, check.summary).map_err(output_error)?;
        if let Some(next) = &check.next_step {
            writeln!(stdout, "       {next}").map_err(output_error)?;
        }
    }
    writeln!(
        stdout,
        "Restart the daemon to apply (`systemctl --user restart openside`)."
    )
    .map_err(output_error)
}

pub(crate) async fn status<W: Write>(
    client: &Client,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let status: RemoteStatus = client.get("/v1/remote/status").await?;
    if json {
        return emit(stdout, &status);
    }
    if !status.enabled {
        writeln!(
            stdout,
            "Remote Control is off: only this computer can reach the daemon; transport settings are kept (turn it on in Settings > Developers)."
        )
        .map_err(output_error)?;
    } else if status.modes.is_empty() {
        writeln!(
            stdout,
            "No remote transport is enabled (`openside remote setup <mode>`)."
        )
        .map_err(output_error)?;
    }
    if let Some(url) = &status.public_url {
        writeln!(stdout, "tailscale/direct\t{url}").map_err(output_error)?;
    }
    if let Some(cloudflare) = &status.cloudflare {
        writeln!(
            stdout,
            "cloudflare\t{}\t{}\trestarts={}{}",
            cloudflare.state,
            cloudflare.url.as_deref().unwrap_or("-"),
            cloudflare.restarts,
            cloudflare
                .last_error
                .as_deref()
                .map(|e| format!("\t{e}"))
                .unwrap_or_default()
        )
        .map_err(output_error)?;
    }
    if let Some(relay) = &status.relay {
        writeln!(
            stdout,
            "relay\t{}\t{}\tdevices={}{}",
            relay.state,
            relay.url,
            relay.clients,
            relay
                .last_error
                .as_deref()
                .map(|e| format!("\t{e}"))
                .unwrap_or_default()
        )
        .map_err(output_error)?;
    }
    Ok(())
}

pub(crate) async fn local_access<W: Write>(
    client: &Client,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let access: openside_proto::LocalAccess = client.get("/v1/remote/local-access").await?;
    if json {
        return emit(stdout, &access);
    }
    writeln!(
        stdout,
        "Local access secret (this daemon boot only): {}",
        access.secret
    )
    .map_err(output_error)?;
    match access.url {
        Some(url) => writeln!(stdout, "Open on this computer: {url}").map_err(output_error),
        None => writeln!(
            stdout,
            "Enable daemon.listen and restart the daemon before opening the web UI."
        )
        .map_err(output_error),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn args(mode: SetupMode) -> SetupArgs {
        SetupArgs {
            mode,
            config: None,
            apply: false,
            https_port: None,
            named: false,
            hostname: None,
            token_file: None,
            relay_url: None,
        }
    }

    fn host(serve: serde_json::Value) -> Result<TailscaleHost, String> {
        Ok(TailscaleHost {
            dns_name: "desk.tail1.ts.net".into(),
            serve,
        })
    }

    #[test]
    fn tailscale_setup_refuses_another_apps_port_and_writes_the_chosen_one() {
        let others = serde_json::json!({
            "TCP": {"443": {"HTTPS": true}, "8443": {"HTTPS": true}},
            "Web": {
                "desk.tail1.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:7317"}}},
                "desk.tail1.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:7878"}}}
            }
        });
        let mut doc = toml_edit::DocumentMut::new();
        let error = apply(&mut doc, &args(SetupMode::Tailscale), || {
            host(others.clone())
        })
        .unwrap_err();
        assert!(error.contains("--https-port 10000"), "{error}");

        let mut chosen = args(SetupMode::Tailscale);
        chosen.https_port = Some(10000);
        let (_, serve) = apply(&mut doc, &chosen, || host(others.clone())).unwrap();
        assert_eq!(
            serve.unwrap(),
            [
                "serve",
                "--bg",
                "--https=10000",
                "--set-path=/",
                "http://127.0.0.1:47820"
            ]
        );
        let config = parse(&doc.to_string());
        assert_eq!(config.remote.tailscale.https_port, 10000);
        assert_eq!(
            config.remote.public_url.as_deref(),
            Some("https://desk.tail1.ts.net:10000")
        );

        // A second run reads the saved port and finds openside's handler already there.
        let mut served = others;
        served["Web"]["desk.tail1.ts.net:10000"] =
            serde_json::json!({"Handlers": {"/": {"Proxy": "http://127.0.0.1:47820"}}});
        let (_, serve) = apply(&mut doc, &args(SetupMode::Tailscale), || host(served)).unwrap();
        assert_eq!(serve, None);
    }

    fn parse(text: &str) -> openside_config::Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        openside_config::load(Some(&path)).unwrap().config
    }

    #[test]
    fn setup_merges_transports_and_keeps_comments() {
        let mut doc: toml_edit::DocumentMut =
            "# mine\n[remote]\n# keep me\npush_contact = \"mailto:a@b.test\"\n"
                .parse()
                .unwrap();
        apply(&mut doc, &args(SetupMode::Tailscale), || {
            host(serde_json::json!({}))
        })
        .unwrap();
        let mut relay = args(SetupMode::Relay);
        relay.relay_url = Some("https://relay.example.test/".into());
        relay.token_file = Some("/tmp/relay-token".into());
        apply(&mut doc, &relay, || unreachable!()).unwrap();
        let text = doc.to_string();
        assert!(
            text.contains("# mine") && text.contains("# keep me"),
            "{text}"
        );
        let config = parse(&text);
        assert_eq!(
            config.remote.mode,
            vec![
                openside_config::RemoteMode::Tailscale,
                openside_config::RemoteMode::Relay
            ]
        );
        assert_eq!(
            config.remote.public_url.as_deref(),
            Some("https://desk.tail1.ts.net")
        );
        assert_eq!(
            config.remote.relay.url.as_deref(),
            Some("https://relay.example.test")
        );
        assert_eq!(
            config.daemon.listen.map(|l| l.to_string()).as_deref(),
            Some(DEFAULT_LISTEN)
        );

        let mut named = args(SetupMode::Cloudflare);
        named.named = true;
        assert!(apply(&mut doc, &named, || unreachable!()).is_err());
        named.hostname = Some("desk.example.test".into());
        apply(&mut doc, &named, || unreachable!()).unwrap();
        let config = parse(&doc.to_string());
        assert_eq!(config.remote.mode.len(), 3);
        assert_eq!(
            config.remote.cloudflare.tunnel,
            openside_config::CloudflareTunnel::Named
        );

        apply(&mut doc, &args(SetupMode::Off), || unreachable!()).unwrap();
        let config = parse(&doc.to_string());
        assert!(config.remote.mode.is_empty());
        assert_eq!(
            config.remote.relay.url.as_deref(),
            Some("https://relay.example.test")
        );

        let mut bad = args(SetupMode::Relay);
        bad.relay_url = Some("http://relay.example.test".into());
        assert!(apply(&mut doc, &bad, || unreachable!()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_the_config_only_after_serve_and_undoes_serve_when_the_write_fails() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = |name: &str, exit: u8| {
            let bin = dir.path().join(name);
            let log = dir.path().join(format!("{name}.log"));
            std::fs::write(
                &bin,
                format!(
                    "#!/bin/sh\necho \"$*\" >> '{}'\nexit {exit}\n",
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
            (bin, log)
        };
        let command: Vec<String> = [
            "serve",
            "--bg",
            "--https=10000",
            "--set-path=/",
            "http://127.0.0.1:47820",
        ]
        .map(String::from)
        .into();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# before\n").unwrap();

        let (failing, _) = fake("failing", 1);
        let error = serve_then_write(&path, "# after\n", Some((&failing, &command))).unwrap_err();
        assert!(error.contains("was not changed"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# before\n");

        let (working, log) = fake("working", 0);
        let unwritable = dir.path().join("config.toml/inside.toml");
        let error =
            serve_then_write(&unwritable, "# after\n", Some((&working, &command))).unwrap_err();
        assert!(error.starts_with("cannot write"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "serve --bg --https=10000 --set-path=/ http://127.0.0.1:47820\nserve --https=10000 --set-path=/ off\n"
        );

        serve_then_write(&path, "# after\n", Some((&working, &command))).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# after\n");
    }
}
