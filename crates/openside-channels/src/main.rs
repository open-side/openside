//! `openside-channels`: run the configured chat adapters, mint a bot token, or check setup.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use openside_channels::{check, pair, process_env, start, state_dir};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "openside-channels",
    version,
    about = "Telegram, Slack and Discord adapters for the openside daemon"
)]
struct Cli {
    /// Configuration file (default: the XDG location, `OPENSIDE_CONFIG`).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run every enabled adapter until SIGINT/SIGTERM.
    Run,
    /// Verify configuration, secrets and the daemon connection without serving.
    Check,
    /// Mint a `read`+`steer`+`approve` device token through the daemon Unix socket.
    Pair {
        /// Device label shown in the daemon's device list.
        #[arg(long, default_value = "channel bot")]
        name: String,
        /// Token file to write (mode 0600); default `<state_dir>/daemon.token`.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Daemon Unix socket; default from the configuration.
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    if let Err(message) = run(Cli::parse()).await {
        eprintln!("openside-channels: {message}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let loaded = openside_config::load(cli.config.as_deref()).map_err(|error| error.to_string())?;
    match cli.command {
        Command::Run => {
            let shutdown = CancellationToken::new();
            let running = start(&loaded, &process_env, shutdown.clone())
                .await
                .map_err(|e| e.to_string())?;
            wait_for_signal().await;
            shutdown.cancel();
            running.join().await;
            Ok(())
        }
        Command::Check => {
            check(&loaded, &process_env)
                .await
                .map_err(|e| e.to_string())?;
            println!("ok: configuration, secrets and daemon connection are usable");
            Ok(())
        }
        Command::Pair { name, out, socket } => {
            let socket = socket.unwrap_or_else(|| loaded.socket_path());
            let out = out.unwrap_or_else(|| state_dir(&loaded).join("daemon.token"));
            let device = pair(&socket, &name, &out)
                .await
                .map_err(|e| e.to_string())?;
            println!("paired device {device}; token written to {}", out.display());
            println!(
                "set channels.token_file to that path; revoke with DELETE /v1/devices/{device}"
            );
            Ok(())
        }
    }
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
