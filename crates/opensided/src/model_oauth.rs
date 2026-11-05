//! `pi-oauth` accounts: sign-in transactions driven by a one-shot helper process, plus
//! lease resolution and refresh. Tokens only ever travel helper stdout → vault → lease.
use crate::{engine::Shared, model_accounts::Account, tools::vault::DaemonVault};
use openside_core::{
    CredentialAuth, CredentialAuthKind, CredentialRequestMetadata, ModelAccountRef,
};
use openside_proto::{
    OAuthAccount, OAuthAuthorization, OAuthMethod, OAuthProviderInfo, OAuthTransaction,
    OAuthTransactionKind, OAuthTransactionStatus,
};
use openside_vault::{AuditContext, OAuthInfo, Secret};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot, Mutex},
};
use zeroize::{Zeroize, Zeroizing};

pub(crate) const SOURCE: &str = "pi-oauth";
const LINE_LIMIT: usize = 65_536;
const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);
const TERMINAL_RETENTION: Duration = Duration::from_secs(10 * 60);
const REFRESH_DEADLINE: Duration = Duration::from_secs(60);
const REFRESH_SKEW_MS: u64 = 60_000;
const MAX_LIVE: usize = 4;

pub(crate) struct ProviderSpec {
    pub provider: &'static str,
    name: &'static str,
    methods: &'static [OAuthMethod],
    manual_input: &'static [OAuthMethod],
    browser_login: Option<&'static str>,
    device_login: Option<&'static str>,
    kind: CredentialAuthKind,
    needs_account_id: bool,
    needs_project: bool,
}

pub(crate) const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        provider: "anthropic",
        name: "Claude (Pro/Max)",
        methods: &[OAuthMethod::Browser],
        manual_input: &[OAuthMethod::Browser],
        browser_login: Some("anthropic"),
        device_login: None,
        kind: CredentialAuthKind::ClaudeCode,
        needs_account_id: false,
        needs_project: false,
    },
    ProviderSpec {
        provider: "openai-codex",
        name: "ChatGPT (Codex)",
        methods: &[OAuthMethod::Browser, OAuthMethod::Device],
        manual_input: &[OAuthMethod::Browser],
        browser_login: Some("openai-codex"),
        device_login: Some("openai-codex-device"),
        kind: CredentialAuthKind::CodexCli,
        needs_account_id: true,
        needs_project: false,
    },
    ProviderSpec {
        provider: "google-gemini-cli",
        name: "Google Gemini CLI",
        methods: &[OAuthMethod::Browser],
        manual_input: &[OAuthMethod::Browser],
        browser_login: Some("google-gemini-cli"),
        device_login: None,
        kind: CredentialAuthKind::Bearer,
        needs_account_id: false,
        needs_project: true,
    },
    ProviderSpec {
        provider: "google-antigravity",
        name: "Google Antigravity",
        methods: &[OAuthMethod::Browser],
        manual_input: &[OAuthMethod::Browser],
        browser_login: Some("google-antigravity"),
        device_login: None,
        kind: CredentialAuthKind::Bearer,
        needs_account_id: false,
        needs_project: true,
    },
    ProviderSpec {
        provider: "github-copilot",
        name: "GitHub Copilot",
        methods: &[OAuthMethod::Device],
        manual_input: &[],
        browser_login: None,
        device_login: Some("github-copilot"),
        kind: CredentialAuthKind::Bearer,
        needs_account_id: false,
        needs_project: false,
    },
];

fn spec(provider: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|spec| spec.provider == provider)
}

pub(crate) fn providers() -> Vec<OAuthProviderInfo> {
    PROVIDERS
        .iter()
        .map(|spec| OAuthProviderInfo {
            provider: spec.provider.into(),
            name: spec.name.into(),
            methods: spec.methods.to_vec(),
            manual_input_methods: spec.manual_input.to_vec(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    ProviderUnknown,
    FlowUnsupported,
    InvalidValue,
    ManualInputNotAllowed,
    TransactionFinished,
    TransactionUnknown,
    Busy,
    UnknownAccount,
    VaultUnavailable,
}

/// Helper credential output; also the exact stored vault payload shape.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    access: String,
    refresh: String,
    expires: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enterprise_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_endpoint: Option<String>,
}

impl Drop for Payload {
    fn drop(&mut self) {
        self.access.zeroize();
        self.refresh.zeroize();
        for value in [
            &mut self.project_id,
            &mut self.account_id,
            &mut self.enterprise_url,
            &mut self.api_endpoint,
        ]
        .into_iter()
        .flatten()
        {
            value.zeroize();
        }
    }
}

impl Payload {
    fn parse(raw: &str) -> Option<Self> {
        let payload: Self = serde_json::from_str(raw).ok()?;
        (!payload.access.is_empty()).then_some(payload)
    }
    fn encode(&self) -> Option<Zeroizing<String>> {
        serde_json::to_string(self).ok().map(Zeroizing::new)
    }
    fn project(&self) -> Option<&str> {
        self.project_id.as_deref().filter(|project| {
            !project.is_empty()
                && project.len() <= 256
                && project
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        })
    }
    fn upstream_account(&self) -> Option<&str> {
        self.account_id
            .as_deref()
            .filter(|id| !id.is_empty() && id.len() <= 256)
    }
    fn complete_for(&self, spec: &ProviderSpec) -> bool {
        (!spec.needs_account_id || self.upstream_account().is_some())
            && (!spec.needs_project || self.project().is_some())
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum HelperEvent {
    Auth {
        url: String,
        #[serde(default)]
        launch_url: Option<String>,
        #[serde(default)]
        instructions: Option<String>,
        #[serde(default)]
        user_code: Option<String>,
    },
    ManualInputWanted,
    Progress {
        stage: String,
    },
    Credentials {
        credentials: Payload,
    },
    Error {
        code: String,
    },
}

enum Stop {
    Cancelled,
}

struct Transaction {
    view: OAuthTransaction,
    owner: Option<String>,
    label: String,
    join_pool: bool,
    deadline: tokio::time::Instant,
    finished: Option<Instant>,
    input: Option<mpsc::Sender<Zeroizing<String>>>,
    stop: Option<oneshot::Sender<Stop>>,
    /// The credentials are stored; only `Ready` may finish it now, so a cancel, delete or
    /// expiry that loses the race to the commit cannot report an outcome that did not happen.
    committed: bool,
}

impl Transaction {
    fn live(&self) -> bool {
        self.finished.is_none()
    }
    fn finish(&mut self, status: OAuthTransactionStatus, reason: Option<&str>) {
        if !self.live() || (self.committed && status != OAuthTransactionStatus::Ready) {
            return;
        }
        self.view.status = status;
        self.view.reason = reason.map(str::to_owned);
        self.view.authorization = None;
        self.view.manual_input = false;
        self.finished = Some(Instant::now());
        self.input = None;
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(Stop::Cancelled);
        }
    }
}

/// `(provider, account_id)`.
type AccountKey = (String, String);

pub(crate) struct Service {
    transactions: parking_lot::Mutex<HashMap<String, Transaction>>,
    locks: Mutex<HashMap<AccountKey, Arc<Mutex<()>>>>,
    /// `(provider, account_id) -> (revision, complete)`; see `Service::complete`.
    completeness: parking_lot::Mutex<HashMap<(String, String), (u64, bool)>>,
    /// Transaction lifetime in milliseconds; tests shorten it.
    pub(crate) ttl_ms: AtomicU64,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            transactions: parking_lot::Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            completeness: parking_lot::Mutex::new(HashMap::new()),
            ttl_ms: AtomicU64::new(DEFAULT_TTL.as_millis() as u64),
        }
    }
}

fn context() -> AuditContext<'static> {
    AuditContext {
        who: "model-oauth",
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

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_label(label: &str) -> bool {
    !label.trim().is_empty() && label.chars().count() <= 80 && !label.chars().any(char::is_control)
}

fn vault_error(error: openside_vault::Error) -> Error {
    match error {
        openside_vault::Error::UnknownItem => Error::UnknownAccount,
        openside_vault::Error::InvalidEntry => Error::InvalidValue,
        _ => Error::VaultUnavailable,
    }
}

fn readiness(info: &OAuthInfo, complete: bool) -> (bool, &'static str) {
    if !info.enabled {
        (false, "Account disabled")
    } else if info.auth_invalid {
        (false, "Sign in again")
    } else if !complete {
        (false, "Account is missing required sign-in details")
    } else {
        (true, "OAuth account ready")
    }
}

impl Service {
    /// Whether the stored payload carries every field the provider's lease needs. The answer
    /// changes only with the payload, so it is cached per revision: otherwise every inventory
    /// run would read (and audit) the tokens of every account that has required fields.
    fn complete(&self, vault: &DaemonVault, info: &OAuthInfo) -> bool {
        let Some(spec) = spec(&info.provider) else {
            return false;
        };
        if !spec.needs_account_id && !spec.needs_project {
            return true;
        }
        let key = (info.provider.clone(), info.account_id.clone());
        if let Some(&(revision, complete)) = self.completeness.lock().get(&key) {
            if revision == info.revision {
                return complete;
            }
        }
        let Ok((revision, complete)) = vault.with_oauth(
            &context(),
            &info.provider,
            &info.account_id,
            |stored, raw| {
                (
                    stored.revision,
                    Payload::parse(raw).is_some_and(|payload| payload.complete_for(spec)),
                )
            },
        ) else {
            return false;
        };
        self.completeness.lock().insert(key, (revision, complete));
        complete
    }

    fn remember(&self, provider: &str, id: &str, revision: u64, complete: bool) {
        self.completeness
            .lock()
            .insert((provider.into(), id.into()), (revision, complete));
    }

    /// A metadata-only change bumps the revision but keeps the payload, and so its completeness.
    fn carry(&self, provider: &str, id: &str, from: u64, to: u64) {
        let mut cache = self.completeness.lock();
        if let Some(entry) = cache.get_mut(&(provider.to_owned(), id.to_owned())) {
            if entry.0 == from {
                entry.0 = to;
            }
        }
    }

    /// Inventory rows for every known-provider OAuth account in an open vault.
    pub(crate) fn inventory(&self, vault: &DaemonVault) -> Vec<Account> {
        let infos = vault.oauth_accounts(&context()).unwrap_or_default();
        infos
            .into_iter()
            .filter(|info| spec(&info.provider).is_some())
            .map(|info| {
                let (ready, reason) = readiness(&info, self.complete(vault, &info));
                Account {
                    reference: ModelAccountRef {
                        source: SOURCE.into(),
                        account_id: info.account_id.clone(),
                    },
                    provider: info.provider,
                    label: info.label,
                    enabled: info.enabled,
                    ready,
                    reason: reason.into(),
                    models: vec![],
                    reserve: info.reserve_percent,
                    windows: vec![],
                    blocked_until: None,
                    model_blocks: vec![],
                    revision: info.revision,
                }
            })
            .collect()
    }
}

fn public(info: &OAuthInfo, complete: bool) -> OAuthAccount {
    let (available, reason) = readiness(info, complete);
    OAuthAccount {
        source: SOURCE.into(),
        provider: info.provider.clone(),
        account_id: info.account_id.clone(),
        label: info.label.clone(),
        enabled: info.enabled,
        reserve_percent: info.reserve_percent,
        available,
        reason: reason.into(),
        auth_invalid: info.auth_invalid,
        created_at_ms: info.created_at_ms,
    }
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

/// A running helper; dropping it kills the helper and its descendants.
struct Helper {
    stdin: ChildStdin,
    lines: mpsc::Receiver<Result<Zeroizing<Vec<u8>>, ()>>,
    // Declared before `_child` so killpg runs while the child is still unreaped and its pid
    // cannot have been reused.
    #[cfg(unix)]
    _group: ProcessGroup,
    _child: Child,
}

impl Helper {
    fn spawn(s: &Shared, mode: &str) -> Result<Self, ()> {
        let argv = &s.config.argv;
        let program = argv.first().ok_or(())?;
        let mut command = Command::new(program);
        command
            .args(&argv[1..])
            .arg(mode)
            .env_clear()
            .envs(openside_config::runtime_environment(
                &s.models.credentials_config(),
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|_| ())?;
        #[cfg(unix)]
        let group = ProcessGroup(child.id());
        let stdin = child.stdin.take().ok_or(())?;
        let stdout = child.stdout.take().ok_or(())?;
        let (sender, lines) = mpsc::channel(4);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Zeroizing::new(Vec::with_capacity(1024));
                let read = (&mut reader)
                    .take(LINE_LIMIT as u64 + 1)
                    .read_until(b'\n', &mut line)
                    .await;
                let item = match read {
                    Ok(0) => return,
                    // A line must end in a newline: a final unterminated line is a truncated
                    // write, and an oversize line never reaches its newline within the limit.
                    Ok(_) if line.last() == Some(&b'\n') => {
                        line.pop();
                        Ok(line)
                    }
                    Ok(_) => Err(()),
                    Err(_) => Err(()),
                };
                let failed = item.is_err();
                if sender.send(item).await.is_err() || failed {
                    return;
                }
            }
        });
        Ok(Self {
            stdin,
            lines,
            _child: child,
            #[cfg(unix)]
            _group: group,
        })
    }
    async fn send(&mut self, line: &str) -> Result<(), ()> {
        let mut bytes = Zeroizing::new(Vec::with_capacity(line.len() + 1));
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await.map_err(|_| ())?;
        self.stdin.flush().await.map_err(|_| ())
    }
    async fn next(&mut self) -> Option<Result<HelperEvent, ()>> {
        let line = self.lines.recv().await?;
        Some(line.and_then(|line| serde_json::from_slice(&line).map_err(|_| ())))
    }
}

fn json_string(value: &str) -> Zeroizing<String> {
    Zeroizing::new(serde_json::Value::String(value.to_owned()).to_string())
}

fn valid_authorization(
    url: &str,
    launch_url: Option<&str>,
    instructions: Option<&str>,
    user_code: Option<&str>,
) -> bool {
    let printable = |value: &str| !value.chars().any(char::is_control);
    url.starts_with("https://")
        && url.len() <= 8192
        && printable(url)
        && launch_url.map_or(true, |launch| {
            (launch.starts_with("http://127.0.0.1:") || launch.starts_with("http://localhost:"))
                && launch.len() <= 8192
                && printable(launch)
        })
        && instructions.map_or(true, |text| {
            text.len() <= 2048 && !text.chars().any(|c| c.is_control() && c != '\n')
        })
        && user_code.map_or(true, |code| {
            !code.is_empty()
                && code.len() <= 64
                && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn helper_reason(code: &str) -> (OAuthTransactionStatus, &'static str) {
    match code {
        "cancelled" => (OAuthTransactionStatus::Cancelled, "oauth_cancelled"),
        // The provider gave up waiting for the person, the same outcome as the daemon TTL.
        "timeout" => (OAuthTransactionStatus::Expired, "oauth_expired"),
        "callback_busy" => (OAuthTransactionStatus::Failed, "oauth_callback_busy"),
        "invalid_grant" => (OAuthTransactionStatus::Failed, "oauth_invalid_grant"),
        "unsupported" => (OAuthTransactionStatus::Failed, "oauth_flow_unsupported"),
        _ => (OAuthTransactionStatus::Failed, "oauth_failed"),
    }
}

enum RefreshError {
    InvalidGrant,
    Failed,
}

async fn refresh(s: &Shared, provider: &str, raw: &str) -> Result<Payload, RefreshError> {
    let mut helper = Helper::spawn(s, "auth-refresh").map_err(|_| RefreshError::Failed)?;
    let mut line = Zeroizing::new(String::with_capacity(raw.len() + 96));
    line.push_str(r#"{"type":"refresh","provider":"#);
    line.push_str(&json_string(provider));
    line.push_str(r#","credentials":"#);
    line.push_str(raw);
    line.push('}');
    let result = tokio::time::timeout(REFRESH_DEADLINE, async {
        helper.send(&line).await.map_err(|_| RefreshError::Failed)?;
        match helper.next().await {
            Some(Ok(HelperEvent::Credentials { credentials }))
                if !credentials.access.is_empty() =>
            {
                Ok(credentials)
            }
            Some(Ok(HelperEvent::Error { code })) if code == "invalid_grant" => {
                Err(RefreshError::InvalidGrant)
            }
            _ => Err(RefreshError::Failed),
        }
    })
    .await;
    result.unwrap_or(Err(RefreshError::Failed))
}

impl Service {
    fn ttl(&self) -> Duration {
        Duration::from_millis(self.ttl_ms.load(Ordering::Relaxed))
    }

    /// Mark overdue transactions expired and forget long-finished ones.
    fn sweep(transactions: &mut HashMap<String, Transaction>) {
        let current = tokio::time::Instant::now();
        for transaction in transactions.values_mut() {
            if transaction.live() && transaction.deadline <= current {
                transaction.finish(OAuthTransactionStatus::Expired, Some("oauth_expired"));
            }
        }
        transactions.retain(|_, transaction| {
            transaction
                .finished
                .map_or(true, |at| at.elapsed() < TERMINAL_RETENTION)
        });
    }

    fn modify(&self, id: &str, apply: impl FnOnce(&mut Transaction)) {
        if let Some(transaction) = self.transactions.lock().get_mut(id) {
            if transaction.live() {
                apply(transaction);
            }
        }
    }

    fn finish(&self, id: &str, status: OAuthTransactionStatus, reason: &str) {
        self.modify(id, |transaction| transaction.finish(status, Some(reason)));
    }

    fn visible(transaction: &Transaction, owner: Option<&str>) -> bool {
        owner.is_none() || transaction.owner.as_deref() == owner
    }

    pub(crate) fn transaction(
        &self,
        owner: Option<&str>,
        id: &str,
    ) -> Result<OAuthTransaction, Error> {
        let mut transactions = self.transactions.lock();
        Self::sweep(&mut transactions);
        transactions
            .get(id)
            .filter(|transaction| Self::visible(transaction, owner))
            .map(|transaction| transaction.view.clone())
            .ok_or(Error::TransactionUnknown)
    }

    pub(crate) async fn input(
        &self,
        owner: Option<&str>,
        id: &str,
        code: Zeroizing<String>,
    ) -> Result<OAuthTransaction, Error> {
        let sender = {
            let mut transactions = self.transactions.lock();
            Self::sweep(&mut transactions);
            let transaction = transactions
                .get_mut(id)
                .filter(|transaction| Self::visible(transaction, owner))
                .ok_or(Error::TransactionUnknown)?;
            if !transaction.live() {
                return Err(Error::TransactionFinished);
            }
            if !transaction.view.manual_input {
                return Err(Error::ManualInputNotAllowed);
            }
            if code.trim().is_empty() || code.chars().count() > 4096 {
                return Err(Error::InvalidValue);
            }
            let sender = transaction
                .input
                .clone()
                .ok_or(Error::TransactionFinished)?;
            transaction.view.manual_input = false;
            transaction.view.status = OAuthTransactionStatus::Exchanging;
            sender
        };
        sender
            .send(code)
            .await
            .map_err(|_| Error::TransactionFinished)?;
        self.transaction(owner, id)
    }

    pub(crate) fn cancel(&self, owner: Option<&str>, id: &str) -> Result<(), Error> {
        let mut transactions = self.transactions.lock();
        Self::sweep(&mut transactions);
        let transaction = transactions
            .get_mut(id)
            .filter(|transaction| Self::visible(transaction, owner))
            .ok_or(Error::TransactionUnknown)?;
        transaction.finish(OAuthTransactionStatus::Cancelled, Some("oauth_cancelled"));
        Ok(())
    }

    fn infos(vault: &DaemonVault) -> Result<Vec<OAuthInfo>, Error> {
        vault.oauth_accounts(&context()).map_err(vault_error)
    }

    async fn find(s: &Shared, provider: &str, id: &str) -> Result<Option<OAuthInfo>, Error> {
        let guard = s.registry.vault.vault.lock().await;
        let vault = guard.as_ref().ok_or(Error::VaultUnavailable)?;
        Ok(Self::infos(vault)?
            .into_iter()
            .find(|info| info.provider == provider && info.account_id == id))
    }

    fn checked<'a>(provider: &str, id: &str) -> Result<&'a ProviderSpec, Error> {
        let spec = spec(provider).ok_or(Error::ProviderUnknown)?;
        if !valid_id(id) {
            return Err(Error::UnknownAccount);
        }
        Ok(spec)
    }

    pub(crate) async fn list(&self, s: &Shared) -> Result<Vec<OAuthAccount>, Error> {
        let guard = s.registry.vault.vault.lock().await;
        let vault = guard.as_ref().ok_or(Error::VaultUnavailable)?;
        let mut accounts: Vec<_> = Self::infos(vault)?
            .into_iter()
            .filter(|info| spec(&info.provider).is_some())
            .map(|info| public(&info, self.complete(vault, &info)))
            .collect();
        accounts.sort_by(|a, b| {
            (&a.provider, a.created_at_ms, &a.account_id).cmp(&(
                &b.provider,
                b.created_at_ms,
                &b.account_id,
            ))
        });
        Ok(accounts)
    }

    pub(crate) async fn get(
        &self,
        s: &Shared,
        provider: &str,
        id: &str,
    ) -> Result<OAuthAccount, Error> {
        Self::checked(provider, id)?;
        let guard = s.registry.vault.vault.lock().await;
        let vault = guard.as_ref().ok_or(Error::VaultUnavailable)?;
        let info = Self::infos(vault)?
            .into_iter()
            .find(|info| info.provider == provider && info.account_id == id)
            .ok_or(Error::UnknownAccount)?;
        Ok(public(&info, self.complete(vault, &info)))
    }

    pub(crate) async fn update(
        &self,
        s: &Shared,
        provider: &str,
        id: &str,
        label: Option<&str>,
        enabled: Option<bool>,
        reserve_percent: Option<u8>,
    ) -> Result<OAuthAccount, Error> {
        Self::checked(provider, id)?;
        if label.is_some_and(|label| !valid_label(label))
            || reserve_percent.is_some_and(|reserve| reserve > 99)
        {
            return Err(Error::InvalidValue);
        }
        loop {
            let change = s.models.change_credentials().await;
            let mut guard = s.registry.vault.vault.lock().await;
            let vault = guard.as_mut().ok_or(Error::VaultUnavailable)?;
            let current = Self::infos(vault)?
                .into_iter()
                .find(|info| info.provider == provider && info.account_id == id)
                .ok_or(Error::UnknownAccount)?;
            let mut info = current.clone();
            if let Some(label) = label {
                info.label = label.into();
            }
            if let Some(enabled) = enabled {
                info.enabled = enabled;
            }
            if let Some(reserve) = reserve_percent {
                info.reserve_percent = reserve;
            }
            let result = vault.update_oauth_info(&context(), info, current.revision);
            let account = match result {
                Ok(info) => {
                    self.carry(provider, id, current.revision, info.revision);
                    Ok(public(&info, self.complete(vault, &info)))
                }
                // A concurrent refresh or sign-in bumped the revision; re-apply on top of it.
                Err(openside_vault::Error::UnknownItem) => {
                    drop(guard);
                    drop(change);
                    continue;
                }
                Err(error) => Err(vault_error(error)),
            };
            drop(guard);
            drop(change);
            s.models.invalidate(s).await;
            return account;
        }
    }

    pub(crate) async fn remove(&self, s: &Shared, provider: &str, id: &str) -> Result<(), Error> {
        Self::checked(provider, id)?;
        if Self::find(s, provider, id).await?.is_none() {
            return Err(Error::UnknownAccount);
        }
        {
            let mut transactions = self.transactions.lock();
            for transaction in transactions.values_mut().filter(|transaction| {
                transaction.live()
                    && transaction.view.kind == OAuthTransactionKind::Relogin
                    && transaction.view.provider == provider
                    && transaction.view.account_id == id
            }) {
                transaction.finish(
                    OAuthTransactionStatus::Failed,
                    Some("oauth_account_removed"),
                );
            }
        }
        loop {
            let _change = s.models.change_credentials().await;
            let mut guard = s.registry.vault.vault.lock().await;
            let vault = guard.as_mut().ok_or(Error::VaultUnavailable)?;
            let current = Self::infos(vault)?
                .into_iter()
                .find(|info| info.provider == provider && info.account_id == id)
                .ok_or(Error::UnknownAccount)?;
            match vault.delete_oauth(&context(), provider, id, current.revision) {
                Ok(()) => break,
                Err(openside_vault::Error::UnknownItem) => continue,
                Err(error) => return Err(vault_error(error)),
            }
        }
        let reference = ModelAccountRef {
            source: SOURCE.into(),
            account_id: id.into(),
        };
        // The record is already gone, so the delete stands; a ref left behind resolves to no
        // account and can be removed with `PUT /v1/settings/pools/{provider}`.
        if let Err(error) = crate::model_accounts::forget_account(s, &reference).await {
            crate::diag!(
                "opensided: removing a deleted OAuth account from pools failed: {}",
                error.1
            );
        }
        let key = (provider.to_owned(), id.to_owned());
        self.completeness.lock().remove(&key);
        self.locks.lock().await.remove(&key);
        s.models.invalidate(s).await;
        Ok(())
    }

    /// Start a sign-in. `existing` is the relogin target; `None` allocates a new account id.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start(
        self: &Arc<Self>,
        s: &Shared,
        owner: Option<String>,
        provider: &str,
        existing: Option<&str>,
        label: Option<&str>,
        join_pool: bool,
        method: Option<OAuthMethod>,
    ) -> Result<OAuthTransaction, Error> {
        let spec = spec(provider).ok_or(Error::ProviderUnknown)?;
        if existing.is_some_and(|id| !valid_id(id)) {
            return Err(Error::UnknownAccount);
        }
        let method = method.unwrap_or(spec.methods[0]);
        if !spec.methods.contains(&method) {
            return Err(Error::FlowUnsupported);
        }
        let login = match method {
            OAuthMethod::Browser => spec.browser_login,
            OAuthMethod::Device => spec.device_login,
        }
        .ok_or(Error::FlowUnsupported)?;
        if label.is_some_and(|label| !valid_label(label)) {
            return Err(Error::InvalidValue);
        }
        let (kind, account_id, label) = match existing {
            Some(id) => {
                let info = Self::find(s, provider, id)
                    .await?
                    .ok_or(Error::UnknownAccount)?;
                (OAuthTransactionKind::Relogin, info.account_id, info.label)
            }
            None => {
                // Fail before the person signs in when nothing could store the result.
                let guard = s.registry.vault.vault.lock().await;
                Self::infos(guard.as_ref().ok_or(Error::VaultUnavailable)?)?;
                (
                    OAuthTransactionKind::Create,
                    crate::models_api::opaque_id(),
                    label.ok_or(Error::InvalidValue)?.to_owned(),
                )
            }
        };
        let transaction_id = crate::models_api::opaque_id();
        let ttl = self.ttl();
        let deadline = tokio::time::Instant::now() + ttl;
        let (input, inputs) = mpsc::channel(1);
        let (stop, stopped) = oneshot::channel();
        let view = OAuthTransaction {
            transaction_id: transaction_id.clone(),
            kind,
            provider: provider.into(),
            account_id: account_id.clone(),
            method,
            status: OAuthTransactionStatus::Starting,
            reason: None,
            authorization: None,
            manual_input: false,
            expires_at_ms: now().saturating_add(ttl.as_millis() as u64),
            account: None,
        };
        {
            let mut transactions = self.transactions.lock();
            Self::sweep(&mut transactions);
            let live: Vec<_> = transactions.values().filter(|t| t.live()).collect();
            if live.len() >= MAX_LIVE
                || live
                    .iter()
                    .any(|t| t.view.provider == provider && t.view.account_id == account_id)
            {
                return Err(Error::Busy);
            }
            transactions.insert(
                transaction_id.clone(),
                Transaction {
                    view: view.clone(),
                    owner,
                    label,
                    join_pool,
                    deadline,
                    finished: None,
                    input: Some(input),
                    stop: Some(stop),
                    committed: false,
                },
            );
        }
        let service = self.clone();
        let shared = s.clone();
        tokio::spawn(async move {
            service
                .drive(
                    &shared,
                    &transaction_id,
                    spec,
                    method,
                    login,
                    inputs,
                    stopped,
                    deadline,
                )
                .await;
        });
        Ok(view)
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &self,
        s: &Shared,
        id: &str,
        spec: &'static ProviderSpec,
        method: OAuthMethod,
        login: &str,
        mut inputs: mpsc::Receiver<Zeroizing<String>>,
        mut stopped: oneshot::Receiver<Stop>,
        deadline: tokio::time::Instant,
    ) {
        let Ok(mut helper) = Helper::spawn(s, "auth-login") else {
            self.finish(
                id,
                OAuthTransactionStatus::Failed,
                "oauth_helper_unavailable",
            );
            return;
        };
        let first = format!(r#"{{"type":"login","provider":{}}}"#, *json_string(login));
        if helper.send(&first).await.is_err() {
            self.finish(
                id,
                OAuthTransactionStatus::Failed,
                "oauth_helper_unavailable",
            );
            return;
        }
        let manual_allowed = spec.manual_input.contains(&method);
        let mut heard = false;
        let mut progress_seen = false;
        loop {
            tokio::select! {
                biased;
                _ = &mut stopped => return,
                () = tokio::time::sleep_until(deadline) => {
                    self.finish(id, OAuthTransactionStatus::Expired, "oauth_expired");
                    return;
                }
                Some(code) = inputs.recv() => {
                    let mut line = Zeroizing::new(String::with_capacity(code.len() + 48));
                    line.push_str(r#"{"type":"manual_input","value":"#);
                    line.push_str(&json_string(&code));
                    line.push('}');
                    if helper.send(&line).await.is_err() {
                        self.finish(id, OAuthTransactionStatus::Failed, "oauth_failed");
                        return;
                    }
                }
                event = helper.next() => {
                    let Some(event) = event else {
                        let reason = if heard { "oauth_failed" } else { "oauth_helper_unavailable" };
                        self.finish(id, OAuthTransactionStatus::Failed, reason);
                        return;
                    };
                    heard = true;
                    match event {
                        Ok(HelperEvent::Auth { url, launch_url, instructions, user_code }) => {
                            if !valid_authorization(&url, launch_url.as_deref(), instructions.as_deref(), user_code.as_deref()) {
                                self.finish(id, OAuthTransactionStatus::Failed, "oauth_failed");
                                return;
                            }
                            self.modify(id, |transaction| {
                                if transaction.view.status == OAuthTransactionStatus::Starting
                                    || transaction.view.authorization.is_some()
                                {
                                    transaction.view.status = match method {
                                        OAuthMethod::Browser => OAuthTransactionStatus::AwaitingBrowser,
                                        OAuthMethod::Device => OAuthTransactionStatus::AwaitingCode,
                                    };
                                }
                                transaction.view.authorization = Some(OAuthAuthorization {
                                    url,
                                    launch_url,
                                    instructions,
                                    user_code,
                                });
                            });
                        }
                        Ok(HelperEvent::ManualInputWanted) => {
                            // Asking again before any exchange started means the last paste
                            // was rejected, so the person must be able to paste once more.
                            if manual_allowed && !progress_seen {
                                self.modify(id, |transaction| {
                                    if matches!(
                                        transaction.view.status,
                                        OAuthTransactionStatus::AwaitingBrowser
                                            | OAuthTransactionStatus::Exchanging
                                    ) {
                                        transaction.view.status = OAuthTransactionStatus::AwaitingBrowser;
                                        transaction.view.manual_input = true;
                                    }
                                });
                            }
                        }
                        Ok(HelperEvent::Progress { stage }) if stage == "exchanging" => {
                            progress_seen = true;
                            self.modify(id, |transaction| {
                                transaction.view.status = OAuthTransactionStatus::Exchanging;
                                transaction.view.manual_input = false;
                            });
                        }
                        Ok(HelperEvent::Credentials { credentials }) => {
                            drop(helper);
                            match self.commit(s, id, spec, credentials).await {
                                Ok(account) => self.modify(id, |transaction| {
                                    transaction.finish(OAuthTransactionStatus::Ready, None);
                                    transaction.view.account = Some(account);
                                }),
                                Err(reason) => self.finish(id, OAuthTransactionStatus::Failed, reason),
                            }
                            return;
                        }
                        Ok(HelperEvent::Error { code }) => {
                            let (status, reason) = helper_reason(&code);
                            self.finish(id, status, reason);
                            return;
                        }
                        Ok(HelperEvent::Progress { .. }) | Err(()) => {
                            self.finish(id, OAuthTransactionStatus::Failed, "oauth_failed");
                            return;
                        }
                    }
                }
            }
        }
    }

    async fn commit(
        &self,
        s: &Shared,
        id: &str,
        spec: &'static ProviderSpec,
        payload: Payload,
    ) -> Result<OAuthAccount, &'static str> {
        let (kind, provider, account_id, label, join_pool) = {
            let transactions = self.transactions.lock();
            let transaction = transactions
                .get(id)
                .filter(|transaction| transaction.live())
                .ok_or("oauth_cancelled")?;
            (
                transaction.view.kind,
                transaction.view.provider.clone(),
                transaction.view.account_id.clone(),
                transaction.label.clone(),
                transaction.join_pool,
            )
        };
        let raw = payload.encode().ok_or("oauth_failed")?;
        let complete = payload.complete_for(spec);
        drop(payload);
        let site = format!("oauth:{provider}:{account_id}");
        let info = loop {
            let change = s.models.change_credentials().await;
            let mut guard = s.registry.vault.vault.lock().await;
            let vault = guard.as_mut().ok_or("oauth_failed")?;
            // Held from the liveness check through the store, so a cancel or delete either
            // finishes the transaction first (and nothing is stored) or loses to the commit.
            let mut transactions = self.transactions.lock();
            let Some(transaction) = transactions.get_mut(id).filter(|t| t.live()) else {
                return Err(match kind {
                    OAuthTransactionKind::Relogin => "oauth_account_removed",
                    OAuthTransactionKind::Create => "oauth_cancelled",
                });
            };
            let (info, expected) = match kind {
                OAuthTransactionKind::Create => (
                    OAuthInfo {
                        provider: provider.clone(),
                        account_id: account_id.clone(),
                        label: label.clone(),
                        enabled: true,
                        reserve_percent: 10,
                        created_at_ms: now(),
                        revision: 0,
                        auth_invalid: false,
                    },
                    None,
                ),
                OAuthTransactionKind::Relogin => {
                    let mut current = vault
                        .oauth_accounts(&context())
                        .map_err(|_| "oauth_failed")?
                        .into_iter()
                        .find(|info| info.provider == provider && info.account_id == account_id)
                        .ok_or("oauth_account_removed")?;
                    let revision = current.revision;
                    current.auth_invalid = false;
                    (current, Some(revision))
                }
            };
            match vault.set_oauth(&context(), info, Secret::new(raw.to_string()), expected) {
                Ok(info) => {
                    transaction.committed = true;
                    drop(transactions);
                    self.remember(&provider, &account_id, info.revision, complete);
                    if let Ok(redactor) = vault.redactor_for(&site) {
                        s.registry.redactors.write().push(redactor);
                    }
                    drop(guard);
                    drop(change);
                    break info;
                }
                Err(openside_vault::Error::UnknownItem)
                    if kind == OAuthTransactionKind::Relogin =>
                {
                    continue
                }
                Err(_) => return Err("oauth_failed"),
            }
        };
        s.models.invalidate(s).await;
        if join_pool && kind == OAuthTransactionKind::Create {
            let reference = ModelAccountRef {
                source: SOURCE.into(),
                account_id: account_id.clone(),
            };
            if let Err(error) = crate::model_accounts::join_pool(s, &provider, reference).await {
                crate::diag!(
                    "opensided: adding a new OAuth account to its pool failed: {}",
                    error.1
                );
            }
        }
        Ok(public(&info, complete))
    }

    async fn lock(&self, provider: &str, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .locks
            .lock()
            .await
            .entry((provider.into(), id.into()))
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    fn current(vault: &DaemonVault, provider: &str, id: &str) -> Option<(OAuthInfo, Payload)> {
        vault
            .with_oauth(&context(), provider, id, |info, raw| {
                Payload::parse(raw).map(|payload| (info.clone(), payload))
            })
            .ok()
            .flatten()
    }

    async fn read(s: &Shared, provider: &str, id: &str) -> Result<(OAuthInfo, Payload), ()> {
        let guard = s.registry.vault.vault.lock().await;
        Self::current(guard.as_ref().ok_or(())?, provider, id).ok_or(())
    }

    /// Lease the account's access token, refreshing it first when it is about to expire or
    /// when the sidecar reported an auth failure against the revision it was leased.
    pub(crate) async fn resolve(
        &self,
        s: &Shared,
        provider: &str,
        id: &str,
        force: bool,
        leased_revision: u64,
    ) -> Result<(CredentialAuth, u64), ()> {
        let spec = spec(provider).ok_or(())?;
        let _single_flight = self.lock(provider, id).await;
        let (mut info, mut payload) = Self::read(s, provider, id).await?;
        if !info.enabled || info.auth_invalid {
            return Err(());
        }
        let stale = payload.expires <= now().saturating_add(REFRESH_SKEW_MS);
        if stale || (force && info.revision == leased_revision) {
            let raw = payload.encode().ok_or(())?;
            match refresh(s, provider, &raw).await {
                Ok(updated) => {
                    let change = s.models.change_credentials().await;
                    let mut guard = s.registry.vault.vault.lock().await;
                    let vault = guard.as_mut().ok_or(())?;
                    let mut expected = info.clone();
                    let outcome = loop {
                        let encoded = updated.encode().ok_or(())?;
                        let revision = expected.revision;
                        match vault.set_oauth(
                            &context(),
                            expected.clone(),
                            Secret::new(encoded.to_string()),
                            Some(revision),
                        ) {
                            Ok(stored) => {
                                self.remember(
                                    provider,
                                    id,
                                    stored.revision,
                                    updated.complete_for(spec),
                                );
                                if let Ok(redactor) =
                                    vault.redactor_for(&format!("oauth:{provider}:{id}"))
                                {
                                    s.registry.redactors.write().push(redactor);
                                }
                                break Some((stored, updated));
                            }
                            // Something bumped the revision while the helper ran. An unchanged
                            // refresh token means only metadata changed (rename, toggle), and the
                            // rotated tokens must still land or the next refresh would fail; a
                            // different one is a newer sign-in, which is never overwritten.
                            Err(openside_vault::Error::UnknownItem) => {
                                match Self::current(vault, provider, id) {
                                    Some((stored, current))
                                        if stored.revision != revision
                                            && current.refresh == payload.refresh =>
                                    {
                                        expected = stored;
                                    }
                                    Some((stored, current)) if stored.revision != revision => {
                                        break Some((stored, current));
                                    }
                                    _ => break None,
                                }
                            }
                            Err(_) => break None,
                        }
                    };
                    drop(guard);
                    drop(change);
                    s.models.invalidate(s).await;
                    let (stored, current) = outcome.ok_or(())?;
                    if !stored.enabled || stored.auth_invalid {
                        return Err(());
                    }
                    info = stored;
                    payload = current;
                }
                Err(RefreshError::InvalidGrant) => {
                    let change = s.models.change_credentials().await;
                    let mut guard = s.registry.vault.vault.lock().await;
                    if let Some(vault) = guard.as_mut() {
                        let mut target = info;
                        loop {
                            let revision = target.revision;
                            target.auth_invalid = true;
                            match vault.update_oauth_info(&context(), target, revision) {
                                Ok(stored) => {
                                    self.carry(provider, id, revision, stored.revision);
                                    break;
                                }
                                // Same rule as a successful refresh: only the sign-in whose
                                // refresh token was rejected is marked, never a newer one.
                                Err(openside_vault::Error::UnknownItem) => {
                                    match Self::current(vault, provider, id) {
                                        Some((stored, current))
                                            if stored.revision != revision
                                                && current.refresh == payload.refresh =>
                                        {
                                            target = stored;
                                        }
                                        _ => break,
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    drop(guard);
                    drop(change);
                    s.models.invalidate(s).await;
                    return Err(());
                }
                // A transient failure inside the skew window still leaves a usable token.
                Err(RefreshError::Failed) if !force && payload.expires > now() => {}
                Err(RefreshError::Failed) => return Err(()),
            }
        }
        if payload.expires <= now() || !payload.complete_for(spec) {
            return Err(());
        }
        let upstream_account_id = spec
            .needs_account_id
            .then(|| payload.upstream_account().map(str::to_owned))
            .flatten();
        let request_metadata = spec.needs_project.then(|| CredentialRequestMetadata {
            project_id: payload.project().map(str::to_owned),
        });
        Ok((
            CredentialAuth {
                kind: spec.kind,
                value: payload.access.clone(),
                upstream_account_id,
                expires_at_ms: Some(payload.expires),
                base_url: None,
                headers: None,
                error_rules: None,
                request_metadata,
            },
            info.revision,
        ))
    }
}
