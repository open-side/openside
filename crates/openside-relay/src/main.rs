//! `openside-relay`: forward Noise-encrypted frames between openside daemons and devices.
//! CLI/MCP pin the daemon key from the pairing link; a relay-served PWA still trusts the
//! operator's code. Put TLS in front of it; it speaks plain HTTP on `--listen`.

use std::path::PathBuf;

use clap::Parser;

/// Self-hosted relay for reaching an openside daemon from outside its network.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8080", env = "OPENSIDE_RELAY_LISTEN")]
    listen: std::net::SocketAddr,
    /// File with one daemon registration token per line (blank lines and `#` comments ignored).
    #[arg(long, value_name = "PATH")]
    token_file: Option<PathBuf>,
    /// Built web app (`web/dist`) served to phones on the same origin.
    #[arg(long, value_name = "DIR", env = "OPENSIDE_RELAY_WEB_DIR")]
    web_dir: Option<PathBuf>,
    /// Device connections allowed per daemon.
    #[arg(long, default_value_t = 32)]
    max_clients: usize,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run(Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("openside-relay: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    let mut tokens: Vec<String> = std::env::var("OPENSIDE_RELAY_TOKENS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(path) = &args.token_file {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        tokens.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_owned),
        );
    }
    if tokens.is_empty() {
        return Err("no registration tokens: set OPENSIDE_RELAY_TOKENS or --token-file".into());
    }
    if tokens.iter().any(|token| token.len() < 16) {
        return Err("registration tokens must be at least 16 characters".into());
    }
    let config = openside_relay::server::ServerConfig {
        daemon_tokens: tokens,
        web_dir: args.web_dir,
        max_clients_per_daemon: args.max_clients.max(1),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|error| format!("cannot listen on {}: {error}", args.listen))?;
    eprintln!("openside-relay listening on {}", args.listen);
    let shutdown = async {
        // Containers stop with SIGTERM; PID 1 without a handler would only die after a timeout.
        #[cfg(unix)]
        {
            let mut term =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(term) => term,
                    Err(_) => return std::future::pending().await,
                };
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    };
    axum::serve(listener, openside_relay::server::router(config))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|error| error.to_string())
}
