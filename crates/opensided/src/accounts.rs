use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use axum::{extract::State, http::StatusCode, Json};
use openside_proto::AccountsResponse;
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex};

use crate::{api::HttpError, config::Resolved, engine::Shared};

const TTL: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: u64 = 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct Service {
    cached: Mutex<Option<(Instant, Result<AccountsResponse, ()>)>>,
}

impl Service {
    async fn get(&self, config: &Resolved) -> Result<AccountsResponse, HttpError> {
        // Holding the lock across refresh gives concurrent readers one subprocess, not
        // one credential-source connection per HTTP request.
        let mut cached = self.cached.lock().await;
        if let Some((at, value)) = &*cached {
            if at.elapsed() < TTL {
                return value.clone().map_err(|_| unavailable());
            }
        }
        let config_json = serde_json::json!({
            "credentials": config.loaded.config.credentials,
            "models": config.loaded.config.models,
        });
        let value = probe(&config.argv, &config_json).await;
        *cached = Some((Instant::now(), value.clone()));
        value.map_err(|_| unavailable())
    }
}

fn unavailable() -> HttpError {
    HttpError(
        StatusCode::SERVICE_UNAVAILABLE,
        "account_sources_unavailable",
    )
}

pub(crate) async fn list(
    State(shared): State<Shared>,
) -> Result<Json<AccountsResponse>, HttpError> {
    shared.accounts.get(&shared.config).await.map(Json)
}

#[cfg(unix)]
struct ProcessGroup(Option<u32>);

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

async fn probe(argv: &[String], config: &serde_json::Value) -> Result<AccountsResponse, ()> {
    let program = argv.first().ok_or(())?;
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .args(["accounts", "--json"])
        .env("OPENSIDE_DOCTOR_CONFIG", config.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|_| ())?;
    // Cancellation must also kill wrapper descendants that keep stdout open.
    #[cfg(unix)]
    let _group = ProcessGroup(child.id());
    let result = tokio::time::timeout(DEADLINE, async {
        let stdout = child.stdout.take().ok_or(())?;
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| ())?;
        if bytes.len() as u64 > OUTPUT_LIMIT {
            return Err(());
        }
        if !child.wait().await.map_err(|_| ())?.success() {
            return Err(());
        }
        let mut response: AccountsResponse = serde_json::from_slice(&bytes).map_err(|_| ())?;
        if response.sources.len() > 32
            || response.sources.iter().any(|source| {
                source.name.len() > 64
                    || source.kind.len() > 64
                    || source.reason.len() > 256
                    || source.accounts.len() > 128
                    || source.accounts.iter().any(|account| {
                        account.id.len() > 128
                            || account.label.len() > 128
                            || account.provider.len() > 64
                            || account.models.len() > 64
                            || account.models.iter().any(|model| model.len() > 256)
                            || account.usage_windows.len() > 16
                            || account.usage_windows.iter().any(|window| {
                                window.name.len() > 64
                                    || window
                                        .used_percent
                                        .is_some_and(|used| !(0.0..=100.0).contains(&used))
                            })
                    })
            })
        {
            return Err(());
        }
        response.cached_for_ms = TTL.as_millis() as u32;
        Ok(response)
    })
    .await
    .unwrap_or(Err(()));
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn accounts_diagnostics_fail_closed_without_echoing_bad_output() {
        for output in [
            "canary-token",
            "{}",
            "{\"sources\":[],\"cached_for_ms\":5,\"truncated\":false,\"access\":\"canary\"}",
        ] {
            let args = vec![
                "sh".into(),
                "-c".into(),
                format!("printf '%s' '{}'", output),
            ];
            assert!(probe(&args, &serde_json::json!({})).await.is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn accounts_diagnostics_bound_stdout() {
        let args = vec!["sh".into(), "-c".into(), "yes x".into()];
        assert!(probe(&args, &serde_json::json!({})).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn accounts_refresh_is_single_flight_and_expired_cache_is_not_reused() {
        let root = tempfile::tempdir().unwrap();
        let count = root.path().join("count");
        let mut product = openside_config::Config::default();
        product.agent.command = Some(vec![
            "sh".into(), "-c".into(),
            format!("printf x >> '{}'; sleep 0.05; printf '%s' '{{\"sources\":[],\"cached_for_ms\":5,\"truncated\":false}}'", count.display()),
        ]);
        let loaded = openside_config::Loaded {
            config: product,
            paths: openside_config::Paths {
                config_file: root.path().join("config.toml"),
                data_dir: root.path().join("data"),
                state_dir: root.path().join("state"),
                runtime_dir: root.path().join("run"),
            },
            source: None,
            warnings: Vec::new(),
        };
        let config = crate::Config::default().resolve_loaded(loaded).unwrap();
        let service = Service::default();
        let (first, second) = tokio::join!(service.get(&config), service.get(&config));
        assert!(first.is_ok() && second.is_ok());
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "x");
        assert!(service.get(&config).await.is_ok());
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "x");
        service.cached.lock().await.as_mut().unwrap().0 = Instant::now() - TTL;
        assert!(service.get(&config).await.is_ok());
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "xx");
    }
}
