use crate::engine::Shared;
use chrono::{TimeZone, Utc};
use openside_core::{CredentialAuth, CredentialAuthKind};
use openside_proto::SubscriptionAccount;
use openside_vault::{AuditContext, Secret, SubscriptionInfo};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
mod keychain;
mod oauth;
mod storage;
#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidSource,
    InvalidValue,
    NotFound,
    VaultUnavailable,
    Io,
    AuthUnavailable,
    Stale,
}

#[derive(Debug)]
pub(crate) struct ResolvedCredential {
    pub auth: CredentialAuth,
    pub revision: u64,
}

type CredentialLocks = HashMap<(String, String), Arc<Mutex<()>>>;

pub(crate) struct Service {
    root: PathBuf,
    locks: Mutex<CredentialLocks>,
    client: Result<reqwest::Client, Error>,
    recent_mints: Mutex<HashMap<(String, String), (u64, std::time::Instant)>>,
    #[cfg(test)]
    pub(crate) token_endpoint: Option<String>,
}

fn context() -> AuditContext<'static> {
    AuditContext {
        who: "managed-subscription",
        session_id: None,
        tool_call_id: None,
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn validate(source: &str, id: &str) -> Result<(), Error> {
    if !matches!(source, "claude-code" | "codex-cli") {
        return Err(Error::InvalidSource);
    }
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::InvalidValue);
    }
    Ok(())
}
fn vault_error(error: openside_vault::Error) -> Error {
    match error {
        openside_vault::Error::UnknownItem => Error::NotFound,
        openside_vault::Error::InvalidEntry => Error::InvalidValue,
        _ => Error::VaultUnavailable,
    }
}
fn cli_installed(executable: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            #[cfg(windows)]
            {
                ["exe", "cmd", "bat", "com"].iter().any(|extension| {
                    directory
                        .join(executable)
                        .with_extension(extension)
                        .is_file()
                })
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                directory.join(executable).metadata().is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
            }
            #[cfg(not(any(unix, windows)))]
            {
                directory.join(executable).is_file()
            }
        })
    })
}

impl Service {
    pub(crate) fn new(data_dir: &Path) -> Self {
        let data_dir = data_dir
            .canonicalize()
            .unwrap_or_else(|_| data_dir.to_path_buf());
        Self {
            root: data_dir.join("managed-subscriptions"),
            locks: Mutex::new(HashMap::new()),
            recent_mints: Mutex::new(HashMap::new()),
            #[cfg(test)]
            token_endpoint: None,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| Error::Io),
        }
    }
    async fn lock(
        &self,
        source: &str,
        id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, Error> {
        validate(source, id)?;
        let lock = self
            .locks
            .lock()
            .await
            .entry((source.into(), id.into()))
            .or_default()
            .clone();
        Ok(lock.lock_owned().await)
    }
    async fn state(
        &self,
        s: &Shared,
        source: &str,
        id: &str,
    ) -> Result<(SubscriptionInfo, Zeroizing<String>), Error> {
        let guard = s.registry.vault.vault.lock().await;
        guard
            .as_ref()
            .ok_or(Error::VaultUnavailable)?
            .with_subscription(&context(), source, id, |info, raw| {
                (info.clone(), Zeroizing::new(raw.to_owned()))
            })
            .map_err(vault_error)
    }
    async fn commit(
        &self,
        s: &Shared,
        info: SubscriptionInfo,
        raw: &str,
        expected: Option<u64>,
    ) -> Result<SubscriptionInfo, Error> {
        self.commit_checked(s, info, raw, expected, None).await
    }
    async fn commit_checked(
        &self,
        s: &Shared,
        info: SubscriptionInfo,
        raw: &str,
        expected: Option<u64>,
        observation: Option<(Option<String>, Option<u128>)>,
    ) -> Result<SubscriptionInfo, Error> {
        let change = s.models.change_credentials().await;
        let mut guard = s.registry.vault.vault.lock().await;
        if let Some(expected) = observation {
            let current = self.observe(&info)?;
            let actual = (
                current.as_ref().map(|o| o.digest.clone()),
                current.as_ref().map(|o| o.mtime),
            );
            if actual != expected {
                return Err(Error::Stale);
            }
        }
        let vault = guard.as_mut().ok_or(Error::VaultUnavailable)?;
        let entry = openside_vault::Entry::subscription(info.clone(), Secret::new(raw.to_owned()))
            .map_err(vault_error)?;
        s.registry.redactors.write().push(entry.redactor());
        let result = vault
            .set_subscription(&context(), info, Secret::new(raw.to_owned()), expected)
            .map_err(vault_error);
        drop(guard);
        drop(change);
        s.models.invalidate(s).await;
        result
    }
    fn location(&self, info: &SubscriptionInfo) -> Result<PathBuf, Error> {
        if info.managed {
            return storage::home(&self.root, &info.source, &info.account_id);
        }
        #[cfg(test)]
        {
            Ok(self.root.join("external-default").join(&info.source))
        }
        #[cfg(not(test))]
        {
            let var = if info.source == "claude-code" {
                "CLAUDE_CONFIG_DIR"
            } else {
                "CODEX_HOME"
            };
            if let Some(value) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                return Ok(PathBuf::from(value));
            }
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .ok_or(Error::Io)?;
            Ok(PathBuf::from(home).join(if info.source == "claude-code" {
                ".claude"
            } else {
                ".codex"
            }))
        }
    }
    fn observe(&self, info: &SubscriptionInfo) -> Result<Option<storage::Observed>, Error> {
        let home = self.location(info)?;
        #[cfg(target_os = "macos")]
        if info.source == "claude-code" && (!cfg!(test) || info.managed) {
            return keychain::read(&home, info.managed);
        }
        storage::read_file(&home.join(storage::filename(&info.source)))
    }
    fn materialize(&self, info: &SubscriptionInfo, raw: &str) -> Result<(), Error> {
        if !info.managed {
            return Ok(());
        }
        if let Some(observed) = self.observe(info)? {
            let canonical = observed.digest == storage::digest(raw.as_bytes());
            if canonical && observed.private {
                return Ok(());
            }
            if !canonical && info.external_digest.as_ref() != Some(&observed.digest) {
                return Err(Error::Stale);
            }
        }
        let home = self.location(info)?;
        #[cfg(target_os = "macos")]
        if info.source == "claude-code" {
            keychain::write(&home, raw)?;
            return self.check_materialized(info, raw);
        }
        storage::write_file(&home, storage::filename(&info.source), raw.as_bytes())?;
        self.check_materialized(info, raw)
    }
    fn check_materialized(&self, info: &SubscriptionInfo, raw: &str) -> Result<(), Error> {
        let observed = self.observe(info)?.ok_or(Error::Stale)?;
        if observed.digest != storage::digest(raw.as_bytes()) {
            return Err(Error::Stale);
        }
        Ok(())
    }
    fn account(&self, info: &SubscriptionInfo, raw: &str) -> Result<SubscriptionAccount, Error> {
        info.validate_label_credentials(raw).map_err(vault_error)?;
        let executable = if info.source == "claude-code" {
            "claude"
        } else {
            "codex"
        };
        let installed = cli_installed(executable);
        let logged_in = oauth::parse(&info.source, raw).is_ok_and(|parsed| {
            !parsed.expires_at_ms.is_some_and(|expiry| expiry <= now())
                || (info.managed && parsed.refreshable)
        });
        let available = installed && logged_in && info.enabled && !info.auth_invalid;
        let reason = if !info.enabled {
            "Account disabled"
        } else if info.auth_invalid {
            "Sign in again"
        } else if !installed {
            "CLI not installed"
        } else if !logged_in {
            "Awaiting CLI sign-in"
        } else {
            "Ready"
        };
        let login_command = if info.managed {
            let location = self.location(info)?;
            let path = location.to_str().ok_or(Error::Io)?;
            if cfg!(windows) {
                let home = format!("'{}'", path.replace('\'', "''"));
                if info.source == "claude-code" {
                    format!("$env:CLAUDE_CONFIG_DIR={home}; Remove-Item Env:CLAUDE_SECURESTORAGE_CONFIG_DIR,Env:CLAUDE_CODE_CUSTOM_OAUTH_URL -ErrorAction SilentlyContinue; claude auth login")
                } else {
                    format!("$env:CODEX_HOME={home}; codex -c 'cli_auth_credentials_store=\"file\"' login")
                }
            } else {
                let home = storage::shell_quote(path);
                if info.source == "claude-code" {
                    format!("env -u CLAUDE_SECURESTORAGE_CONFIG_DIR -u CLAUDE_CODE_CUSTOM_OAUTH_URL CLAUDE_CONFIG_DIR={home} claude auth login")
                } else {
                    format!(
                        "CODEX_HOME={home} codex -c 'cli_auth_credentials_store=\"file\"' login"
                    )
                }
            }
        } else if info.source == "claude-code" {
            "claude auth login".into()
        } else {
            "codex login".into()
        };
        Ok(SubscriptionAccount {
            source: info.source.clone(),
            provider: if info.source == "claude-code" {
                "anthropic"
            } else {
                "openai-codex"
            }
            .into(),
            account_id: info.account_id.clone(),
            label: info.label.clone(),
            managed: info.managed,
            installed,
            logged_in,
            enabled: info.enabled,
            available,
            reason: reason.into(),
            login_command,
            created_at: Utc
                .timestamp_millis_opt(info.created_at_ms as i64)
                .single()
                .ok_or(Error::Io)?,
            reserve_percent: info.reserve_percent,
        })
    }
    pub(crate) async fn create(
        &self,
        s: &Shared,
        source: &str,
        label: &str,
    ) -> Result<SubscriptionAccount, Error> {
        validate(source, "new")?;
        let id = format!("{:032x}", rand::random::<u128>());
        let info = SubscriptionInfo {
            source: source.into(),
            account_id: id.clone(),
            label: label.into(),
            managed: true,
            enabled: true,
            hidden: false,
            reserve_percent: 10,
            created_at_ms: now(),
            revision: 0,
            external_digest: None,
            external_mtime_ns: None,
            auth_invalid: false,
        };
        openside_vault::Entry::subscription(info.clone(), Secret::new(String::new()))
            .map_err(vault_error)?;
        let home = self.location(&info)?;
        storage::create(&home)?;
        if source == "codex-cli" {
            storage::write_file(
                &home,
                "config.toml",
                b"cli_auth_credentials_store = \"file\"\n",
            )?;
        }
        let info = match self.commit(s, info, "", None).await {
            Ok(info) => info,
            Err(error) => {
                // A completion-audit failure can follow a successful durable commit.
                if matches!(self.state(s, source, &id).await, Err(Error::NotFound)) {
                    let _ = storage::wipe(&home);
                }
                return Err(error);
            }
        };
        self.account(&info, "")
    }
    async fn adopt(
        &self,
        s: &Shared,
        mut info: SubscriptionInfo,
        mut raw: Zeroizing<String>,
    ) -> Result<(SubscriptionInfo, Zeroizing<String>), Error> {
        let Some(observed) = self.observe(&info)? else {
            return Ok((info, raw));
        };
        if !info.managed {
            if info.external_digest.as_ref() != Some(&observed.digest) {
                let revision = info.revision;
                info.external_digest = Some(observed.digest);
                info.external_mtime_ns = Some(observed.mtime);
                info.auth_invalid = false;
                info = self.commit(s, info, "", Some(revision)).await?;
            }
            return Ok((info, observed.bytes));
        }
        if observed.digest == storage::digest(raw.as_bytes()) {
            return Ok((info, raw));
        }
        if info.external_digest.as_ref() == Some(&observed.digest) {
            if !raw.is_empty() {
                self.materialize(&info, &raw)?;
            }
            return Ok((info, raw));
        }
        if observed.mtime != 0
            && info
                .external_mtime_ns
                .is_some_and(|mtime| observed.mtime < mtime)
        {
            return Err(Error::Stale);
        }
        oauth::parse(&info.source, &observed.bytes)?;
        let revision = info.revision;
        info.external_digest = Some(observed.digest);
        info.external_mtime_ns = Some(observed.mtime);
        info.auth_invalid = false;
        let observation = (info.external_digest.clone(), info.external_mtime_ns);
        info = self
            .commit_checked(s, info, &observed.bytes, Some(revision), Some(observation))
            .await?;
        raw = observed.bytes;
        self.materialize(&info, &raw)?;
        Ok((info, raw))
    }
    pub(crate) async fn get(
        &self,
        s: &Shared,
        source: &str,
        id: &str,
    ) -> Result<SubscriptionAccount, Error> {
        let _lock = self.lock(source, id).await?;
        let (info, raw) = self.state(s, source, id).await?;
        if info.hidden {
            return Err(Error::NotFound);
        }
        let fallback = info.clone();
        match self.adopt(s, info, raw).await {
            Ok((info, raw)) => self.account(&info, &raw),
            Err(Error::Io | Error::AuthUnavailable | Error::Stale) => {
                let mut account = self.account(&fallback, "")?;
                account.reason = "CLI credentials unavailable; sign in again".into();
                Ok(account)
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn list(&self, s: &Shared) -> Result<Vec<SubscriptionAccount>, Error> {
        for source in ["claude-code", "codex-cli"] {
            let _lock = self.lock(source, "default").await?;
            match self.state(s, source, "default").await {
                Ok(_) => (),
                Err(Error::NotFound) => {
                    let info = SubscriptionInfo {
                        source: source.into(),
                        account_id: "default".into(),
                        label: "Default CLI account".into(),
                        managed: false,
                        hidden: false,
                        enabled: s
                            .config
                            .loaded
                            .config
                            .credentials
                            .get("subscriptions")
                            .and_then(|v| v.get(source))
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(source == "codex-cli"),
                        reserve_percent: 10,
                        created_at_ms: now(),
                        revision: 0,
                        external_digest: None,
                        external_mtime_ns: None,
                        auth_invalid: false,
                    };
                    self.commit(s, info, "", None).await?;
                }
                Err(error) => return Err(error),
            }
        }
        let infos = {
            let guard = s.registry.vault.vault.lock().await;
            guard
                .as_ref()
                .ok_or(Error::VaultUnavailable)?
                .subscriptions(&context())
                .map_err(vault_error)?
        };
        let mut result = Vec::with_capacity(infos.len());
        for info in infos.into_iter().filter(|info| !info.hidden) {
            match self.get(s, &info.source, &info.account_id).await {
                Ok(account) => result.push(account),
                Err(Error::NotFound) => (),
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
    pub(crate) async fn update(
        &self,
        s: &Shared,
        source: &str,
        id: &str,
        label: Option<&str>,
        enabled: Option<bool>,
        reserve_percent: Option<u8>,
    ) -> Result<SubscriptionAccount, Error> {
        let _lock = self.lock(source, id).await?;
        let (mut info, raw) = self.state(s, source, id).await?;
        if info.hidden {
            return Err(Error::NotFound);
        }
        let raw = if info.managed {
            raw
        } else {
            self.observe(&info)?
                .map(|observed| observed.bytes)
                .unwrap_or_default()
        };
        let revision = info.revision;
        if let Some(label) = label {
            info.label = label.into();
        }
        if let Some(enabled) = enabled {
            info.enabled = enabled;
        }
        if let Some(reserve) = reserve_percent {
            info.reserve_percent = reserve;
        }
        info.validate_label_credentials(&raw).map_err(vault_error)?;
        let canonical = if info.managed { raw.as_str() } else { "" };
        let info = self.commit(s, info, canonical, Some(revision)).await?;
        self.account(&info, &raw)
    }
    pub(crate) async fn remove(&self, s: &Shared, source: &str, id: &str) -> Result<(), Error> {
        let _lock = self.lock(source, id).await?;
        let (mut info, raw) = self.state(s, source, id).await?;
        if info.hidden {
            return Err(Error::NotFound);
        }
        if !info.managed {
            let revision = info.revision;
            info.enabled = false;
            info.hidden = true;
            self.commit(s, info, "", Some(revision)).await?;
            return Ok(());
        }
        let revision = info.revision;
        info.enabled = false;
        let info = self.commit(s, info, &raw, Some(revision)).await?;
        let home = self.location(&info)?;
        #[cfg(target_os = "macos")]
        if source == "claude-code" {
            keychain::wipe(&home)?;
        }
        storage::wipe(&home)?;
        let change = s.models.change_credentials().await;
        let mut guard = s.registry.vault.vault.lock().await;
        let result = guard
            .as_mut()
            .ok_or(Error::VaultUnavailable)?
            .delete_subscription(&context(), source, id, info.revision)
            .map_err(vault_error);
        drop(guard);
        drop(change);
        s.models.invalidate(s).await;
        result
    }
    pub(crate) async fn watch(&self, s: &Shared) -> Result<(), Error> {
        self.list(s).await.map(|_| ())
    }
    pub(crate) async fn resolve(
        &self,
        s: &Shared,
        source: &str,
        id: &str,
        force_refresh: bool,
    ) -> Result<ResolvedCredential, Error> {
        let requested = self.state(s, source, id).await?.0.revision;
        let _lock = self.lock(source, id).await?;
        let (info, raw) = self.state(s, source, id).await?;
        if info.hidden {
            return Err(Error::NotFound);
        }
        let (mut info, mut raw) = self.adopt(s, info, raw).await?;
        if !info.enabled || info.auth_invalid {
            return Err(Error::AuthUnavailable);
        }
        let mut parsed = oauth::parse(source, &raw)?;
        let expired = parsed
            .expires_at_ms
            .is_some_and(|expiry| expiry <= now().saturating_add(30_000));
        let recently_minted = self
            .recent_mints
            .lock()
            .await
            .get(&(source.into(), id.into()))
            .is_some_and(|(revision, when)| {
                *revision == info.revision && when.elapsed() < Duration::from_secs(5)
            });
        if expired || (force_refresh && requested == info.revision && !recently_minted) {
            if !info.managed {
                return Err(Error::AuthUnavailable);
            }
            let client = self.client.as_ref().map_err(|_| Error::Io)?;
            let before = self.observe(&info)?.map(|o| (o.digest, o.mtime));
            #[cfg(not(test))]
            let refreshed = oauth::refresh(client, source, &raw).await;
            #[cfg(test)]
            let refreshed = match self.token_endpoint.as_deref() {
                Some(endpoint) => oauth::refresh_at(client, source, &raw, endpoint).await,
                None => oauth::refresh(client, source, &raw).await,
            };
            let latest = self.observe(&info)?.map(|o| (o.digest, o.mtime));
            if latest != before {
                let adopted = self.adopt(s, info, raw).await?;
                info = adopted.0;
                raw = adopted.1;
                parsed = oauth::parse(source, &raw)?;
            } else {
                match refreshed {
                    Ok(updated) => {
                        let revision = info.revision;
                        let observation = (
                            before.as_ref().map(|o| o.0.clone()),
                            before.as_ref().map(|o| o.1),
                        );
                        info.external_digest = observation.0.clone();
                        info.external_mtime_ns = observation.1;
                        info = self
                            .commit_checked(s, info, &updated, Some(revision), Some(observation))
                            .await?;
                        self.recent_mints.lock().await.insert(
                            (source.into(), id.into()),
                            (info.revision, std::time::Instant::now()),
                        );
                        raw = updated;
                        self.materialize(&info, &raw)?;
                        parsed = oauth::parse(source, &raw)?;
                    }
                    Err(Error::AuthUnavailable) => {
                        let revision = info.revision;
                        info.auth_invalid = true;
                        let observation = (
                            before.as_ref().map(|o| o.0.clone()),
                            before.as_ref().map(|o| o.1),
                        );
                        self.commit_checked(s, info, &raw, Some(revision), Some(observation))
                            .await?;
                        return Err(Error::AuthUnavailable);
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        if parsed.expires_at_ms.is_some_and(|expiry| expiry <= now()) {
            return Err(Error::AuthUnavailable);
        }
        self.materialize(&info, &raw)?;
        Ok(ResolvedCredential {
            auth: CredentialAuth {
                kind: if source == "claude-code" {
                    CredentialAuthKind::ClaudeCode
                } else {
                    CredentialAuthKind::CodexCli
                },
                value: parsed.access.to_string(),
                upstream_account_id: parsed.account_id.take(),
                expires_at_ms: parsed.expires_at_ms,
            },
            revision: info.revision,
        })
    }
}
