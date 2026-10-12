//! Supervised `cloudflared` (remote.mode = "cloudflare").
//!
//! A quick tunnel runs `cloudflared tunnel --no-autoupdate --url http://<listen>` and its
//! public URL is the first `https://….trycloudflare.com` the process prints; it changes on
//! every start, so pairing links issued before a restart stop working. A named tunnel runs
//! `cloudflared tunnel --no-autoupdate run` with the token in `TUNNEL_TOKEN` (never argv,
//! which other local users can read) and is reachable at `https://<hostname>` once it logs a
//! registered connection. The process is restarted with backoff and killed on shutdown.

use std::{
    net::SocketAddr,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use openside_config::{CloudflareConfig, CloudflareTunnel};
use openside_proto::CloudflareStatus;
use parking_lot::Mutex;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use zeroize::Zeroizing;

const QUICK_SUFFIX: &str = ".trycloudflare.com";
const REGISTERED: &str = "Registered tunnel connection";
// A process that stayed up this long was healthy; its exit restarts the backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

pub(crate) struct Options {
    pub(crate) bin: String,
    pub(crate) listen: SocketAddr,
    /// `Some(hostname)` for a named tunnel.
    pub(crate) hostname: Option<String>,
    pub(crate) token: Option<Zeroizing<String>>,
    pub(crate) initial_backoff: Duration,
    pub(crate) max_backoff: Duration,
}

impl Options {
    pub(crate) fn from_config(
        config: &CloudflareConfig,
        listen: SocketAddr,
        token: Option<String>,
    ) -> Self {
        Self {
            bin: config.bin.clone(),
            listen,
            hostname: match config.tunnel {
                CloudflareTunnel::Quick => None,
                CloudflareTunnel::Named => config.hostname.clone(),
            },
            token: token.map(Zeroizing::new),
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
        }
    }
}

pub(crate) struct Supervisor {
    status: Arc<Mutex<CloudflareStatus>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Supervisor {
    pub(crate) fn status(&self) -> CloudflareStatus {
        self.status.lock().clone()
    }

    /// Kill `cloudflared` and wait until it has exited.
    pub(crate) async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        // The child has kill_on_drop, so aborting the task also ends the process.
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// The public URL in one line of `cloudflared` output, if any.
pub(crate) fn quick_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, ':' | '/' | '.' | '-')))
        .unwrap_or(rest.len());
    let url = &rest[..end];
    let host = url.strip_prefix("https://")?.trim_end_matches('/');
    (host.ends_with(QUICK_SUFFIX)
        && host.len() > QUICK_SUFFIX.len()
        && host != "api.trycloudflare.com"
        && !host.contains('/'))
    .then(|| format!("https://{host}"))
}

fn set(status: &Mutex<CloudflareStatus>, change: impl FnOnce(&mut CloudflareStatus)) {
    change(&mut status.lock());
}

fn forward_lines(
    reader: impl AsyncRead + Unpin + Send + 'static,
    lines: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(reader).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
}

pub(crate) fn spawn(options: Options, mut shutdown: broadcast::Receiver<()>) -> Supervisor {
    let status = Arc::new(Mutex::new(CloudflareStatus {
        state: "starting".into(),
        url: None,
        restarts: 0,
        last_error: None,
    }));
    let (stop_tx, mut stop) = oneshot::channel();
    let shared = Arc::clone(&status);
    let task = tokio::spawn(async move {
        let mut backoff = options.initial_backoff;
        let mut first = true;
        loop {
            if !first {
                set(&shared, |s| {
                    s.state = "restarting".into();
                    s.url = None;
                });
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = &mut stop => return,
                    _ = shutdown.recv() => return,
                }
                backoff = (backoff * 2).min(options.max_backoff);
                set(&shared, |s| s.restarts += 1);
            }
            first = false;
            let mut command = Command::new(&options.bin);
            command.args(["tunnel", "--no-autoupdate"]);
            match &options.hostname {
                None => {
                    command.args(["--url", &format!("http://{}", options.listen)]);
                }
                Some(_) => {
                    command.arg("run");
                    match &options.token {
                        Some(token) => {
                            command.env("TUNNEL_TOKEN", token.as_str());
                        }
                        None => {
                            set(&shared, |s| {
                                s.state = "failed".into();
                                s.last_error = Some(
                                    "named tunnel has no token (remote.cloudflare.token_file or OPENSIDE_CLOUDFLARE_TOKEN)".into(),
                                );
                            });
                            return;
                        }
                    }
                }
            }
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    set(&shared, |s| {
                        s.state = "failed".into();
                        s.last_error = Some(format!("cannot start {}: {error}", options.bin));
                    });
                    continue;
                }
            };
            let started = Instant::now();
            let (lines_tx, mut lines) = mpsc::unbounded_channel();
            if let Some(stdout) = child.stdout.take() {
                forward_lines(stdout, lines_tx.clone());
            }
            if let Some(stderr) = child.stderr.take() {
                forward_lines(stderr, lines_tx);
            }
            let exit = loop {
                tokio::select! {
                    line = lines.recv() => {
                        let Some(line) = line else { break child.wait().await; };
                        match &options.hostname {
                            None => if let Some(url) = quick_url(&line) {
                                set(&shared, |s| {
                                    if s.url.is_none() {
                                        crate::diag!("opensided: cloudflare quick tunnel at {url}");
                                        s.url = Some(url);
                                        s.state = "running".into();
                                        s.last_error = None;
                                    }
                                });
                            },
                            Some(hostname) => if line.contains(REGISTERED) {
                                set(&shared, |s| {
                                    s.url = Some(format!("https://{hostname}"));
                                    s.state = "running".into();
                                    s.last_error = None;
                                });
                            },
                        }
                    }
                    exit = child.wait() => break exit,
                    _ = &mut stop => {
                        let _ = child.kill().await;
                        return;
                    }
                    _ = shutdown.recv() => {
                        let _ = child.kill().await;
                        return;
                    }
                }
            };
            if started.elapsed() >= HEALTHY_RUN {
                backoff = options.initial_backoff;
            }
            let detail = match exit {
                Ok(status) => format!("cloudflared exited ({status})"),
                Err(error) => format!("cloudflared wait failed: {error}"),
            };
            set(&shared, |s| s.last_error = Some(detail));
        }
    });
    Supervisor {
        status,
        stop: Some(stop_tx),
        task: Some(task),
    }
}
