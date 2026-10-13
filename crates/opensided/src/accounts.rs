use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use axum::{extract::State, Json};
use openside_proto::AccountsResponse;
use tokio::{io::AsyncReadExt, process::Command, sync::Mutex};

use crate::{api::HttpError, engine::Shared};

const TTL: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: u64 = 1024 * 1024;
#[cfg(test)]
const MIN_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct Service {
    cached: Mutex<Option<(Instant, u64, AccountsResponse)>>,
    refresh: Mutex<()>,
}

impl Service {
    async fn get(&self, shared: &Shared) -> Result<AccountsResponse, HttpError> {
        let cache = self.cached.lock().await;
        if let Some((at, generation, value)) = &*cache {
            if at.elapsed() < TTL && *generation == shared.models.credentials_generation() {
                return Ok(value.clone());
            }
        }
        drop(cache);
        let _refresh = self.refresh.lock().await;
        let cache = self.cached.lock().await;
        if let Some((at, generation, value)) = &*cache {
            if at.elapsed() < TTL && *generation == shared.models.credentials_generation() {
                return Ok(value.clone());
            }
        }
        drop(cache);
        let generation = shared.models.credentials_generation();
        let value = crate::model_accounts::diagnostics(shared).await?;
        if generation == shared.models.credentials_generation() {
            *self.cached.lock().await = Some((Instant::now(), generation, value.clone()));
        }
        Ok(value)
    }
    pub(crate) async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }
}

pub(crate) async fn list(
    State(shared): State<Shared>,
) -> Result<Json<AccountsResponse>, HttpError> {
    shared.accounts.get(&shared).await.map(Json)
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

#[cfg(test)]
async fn probe(
    argv: &[String],
    config: &serde_json::Value,
    env: &[(String, String)],
    deadline: Duration,
) -> Result<AccountsResponse, ()> {
    let value = diagnostic(argv, config, env, deadline, "accounts").await?;
    let mut response: AccountsResponse = serde_json::from_value(value).map_err(|_| ())?;
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
                        || account
                            .reason
                            .as_ref()
                            .is_some_and(|reason| reason.len() > 256)
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
}

pub(crate) async fn diagnostic(
    argv: &[String],
    config: &serde_json::Value,
    env: &[(String, String)],
    deadline: Duration,
    kind: &str,
) -> Result<serde_json::Value, ()> {
    let program = argv.first().ok_or(())?;
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .args([kind, "--json"])
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
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
    let result = tokio::time::timeout(deadline, async {
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
        serde_json::from_slice(&bytes).map_err(|_| ())
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
            assert!(probe(&args, &serde_json::json!({}), &[], MIN_DEADLINE)
                .await
                .is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn accounts_diagnostics_bound_stdout() {
        let args = vec!["sh".into(), "-c".into(), "yes x".into()];
        assert!(probe(&args, &serde_json::json!({}), &[], MIN_DEADLINE)
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn accounts_diagnostics_still_stop_at_their_deadline() {
        let args = vec!["sh".into(), "-c".into(), "sleep 1".into()];
        assert!(probe(
            &args,
            &serde_json::json!({}),
            &[],
            Duration::from_millis(20),
        )
        .await
        .is_err());
    }
}
