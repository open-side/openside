use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::future::BoxFuture;
use openside_browser::Action as BrowserAction;
use openside_config::{
    StoredVault, VaultAutoLock, VaultAutofillConfig, VaultBackend, VaultConfig, VaultPolicySetting,
    VaultResolveError,
};
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use openside_proto::{VaultState, VaultStatus};
use openside_vault::{
    AccessPolicy, AuditAction, AuditContext, AutofillRules, BiometricError, BiometricMechanism,
    LoginOutcome, PassphraseLock, PlatformLock, Secret, StoredFormat, TotpMatch, Vault,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{io::AsyncReadExt, sync::Mutex};
use zeroize::Zeroizing;

use super::{
    browser::{browser_error, done, Controller},
    parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec,
};
use crate::engine::Shared;

mod ceremony;

pub(crate) type DaemonVault = Vault<Box<dyn PlatformLock + Send>>;

pub(crate) struct Service {
    pub(crate) vault: Mutex<Option<DaemonVault>>,
    available: AtomicBool,
    /// Unlock method of the open vault, and why the last unlock attempt failed.
    state: parking_lot::Mutex<(Option<VaultBackend>, Option<&'static str>)>,
    throttle: parking_lot::Mutex<UnlockThrottle>,
    /// Password settings changed at runtime; `None` until first read from the config.
    live: parking_lot::RwLock<Option<LiveSettings>>,
    /// Last vault use on the daemon's (test-settable) monotonic clock, for auto-lock.
    last_activity: parking_lot::Mutex<Option<Instant>>,
    /// Bumped each time a vault is installed, so a step authenticated against one opening
    /// can tell that the vault was locked and opened again before it ran.
    opened: std::sync::atomic::AtomicU64,
    /// Tests: pause the next export, clear or recovery-key step after its authentication
    /// (signal the first, wait for the second).
    #[cfg(test)]
    pub(crate) after_reauth:
        parking_lot::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    /// When a person at this computer last opened the vault (setup, unlock, recover), for
    /// steps that need fresh authentication. The daemon-start unlock never sets it.
    person_unlock: parking_lot::Mutex<Option<Instant>>,
    biometrics: parking_lot::RwLock<Arc<dyn Biometrics>>,
}

/// The `[vault]` settings the Passwords settings change without a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveSettings {
    pub(crate) auto_lock: VaultAutoLock,
    pub(crate) biometric_unlock: bool,
    pub(crate) autofill: VaultAutofillConfig,
    pub(crate) default_policy: VaultPolicySetting,
}

impl LiveSettings {
    fn from_config(config: &VaultConfig) -> Self {
        Self {
            auto_lock: config.auto_lock,
            biometric_unlock: config.biometric_unlock,
            autofill: config.autofill.clone(),
            default_policy: config.default_policy,
        }
    }

    /// Write these into a `[vault]` section, keeping its other keys.
    pub(crate) fn apply_to(&self, config: &mut VaultConfig) {
        config.auto_lock = self.auto_lock;
        config.biometric_unlock = self.biometric_unlock;
        config.autofill = self.autofill.clone();
        config.default_policy = self.default_policy;
    }

    /// Vault-side autofill rules. The config loader and the settings API refuse entries
    /// that are not domain names; should one get through anyway, autofill turns off rather
    /// than dropping an exclusion.
    pub(crate) fn autofill_rules(&self) -> AutofillRules {
        let excluded = self
            .autofill
            .excluded_domains
            .iter()
            .map(|domain| openside_vault::normalize_domain(domain))
            .collect::<Result<Vec<_>, _>>();
        excluded
            .and_then(|excluded| {
                AutofillRules::new(
                    self.autofill.enabled,
                    self.autofill.allow_insecure,
                    &excluded,
                )
            })
            .unwrap_or_else(|_| {
                crate::diag!(
                    "opensided: vault.autofill.excluded_domains is invalid; autofill is off"
                );
                AutofillRules::new(false, false, &[]).unwrap_or_default()
            })
    }
}

/// How long a person's unlock counts as fresh authentication for export and recovery keys.
pub(crate) const FRESH_UNLOCK: Duration = Duration::from_secs(5 * 60);
/// How long a biometric prompt waits for the person.
const BIOMETRIC_TIMEOUT: Duration = Duration::from_secs(30);
/// The shortest idle time before auto-lock, so `immediately` never cuts into a use that
/// recorded itself but has not yet taken the vault.
const IMMEDIATE_GRACE: Duration = Duration::from_secs(1);

/// The OS biometric check, replaceable in tests.
pub(crate) trait Biometrics: Send + Sync {
    fn probe(&self) -> Result<BiometricMechanism, BiometricError>;
    fn verify(&self, reason: &str) -> Result<(), BiometricError>;
}

struct OsBiometrics;

impl Biometrics for OsBiometrics {
    fn probe(&self) -> Result<BiometricMechanism, BiometricError> {
        openside_vault::biometric_probe()
    }
    fn verify(&self, reason: &str) -> Result<(), BiometricError> {
        openside_vault::biometric_verify(reason, BIOMETRIC_TIMEOUT)
    }
}

/// What the daemon observed about the vault file and keyring, without unlocking anything.
pub(crate) struct Probe {
    stored: Option<StoredVault>,
    keyring_reachable: bool,
    pub(crate) backend: Result<VaultBackend, &'static str>,
}

/// A lifecycle request that failed, as an HTTP status code and stable error code.
pub(crate) type LifecycleError = (u16, &'static str);

/// Why `POST /v1/vault/unlock` did not unlock.
pub(crate) enum UnlockRefusal {
    Failed(LifecycleError),
    /// Too many wrong passphrases (or an attempt already running) for this caller, or across
    /// paired devices: no key derivation ran. Retry after this many seconds.
    Throttled(u64),
}

/// Wrong passphrases a caller may submit before each further attempt waits.
const FREE_FAILURES: u32 = 5;
/// The first wait; it doubles with every further wrong passphrase, up to [`MAX_COOLDOWN`].
const BASE_COOLDOWN: Duration = Duration::from_secs(30);
const MAX_COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// Wrong passphrases from all paired devices together within [`REMOTE_WINDOW`] before every
/// device waits; the local socket is exempt, so a person at the computer can always recover.
const REMOTE_FAILURES: usize = 20;
const REMOTE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Paired-device unlock attempts queued at once (each runs Argon2id under the vault lock).
const REMOTE_IN_FLIGHT: usize = 2;

/// Failed-passphrase bookkeeping, in memory only (a restart forgets it, as it forgets the
/// unlocked vault). Callers are keyed by their audit label (`device:<id>` or `local`), never
/// by a client-supplied address.
#[derive(Default)]
struct UnlockThrottle {
    failures: HashMap<String, (u32, Instant)>,
    remote: VecDeque<Instant>,
    in_flight: HashSet<String>,
}

impl UnlockThrottle {
    /// Admit one attempt, or the time until one would be admitted.
    fn admit(&mut self, caller: &str, remote: bool, now: Instant) -> Result<(), Duration> {
        if let Some(&(count, last)) = self.failures.get(caller) {
            if count >= FREE_FAILURES {
                let doublings = (count - FREE_FAILURES).min(16);
                let wait = BASE_COOLDOWN
                    .saturating_mul(1 << doublings)
                    .min(MAX_COOLDOWN);
                let until = last + wait;
                if now < until {
                    return Err(until - now);
                }
            }
        }
        if remote {
            while self
                .remote
                .front()
                .is_some_and(|failed| now.duration_since(*failed) >= REMOTE_WINDOW)
            {
                self.remote.pop_front();
            }
            if self.remote.len() >= REMOTE_FAILURES {
                let oldest = self.remote.front().copied().unwrap_or(now);
                return Err((oldest + REMOTE_WINDOW).saturating_duration_since(now));
            }
            let queued = self
                .in_flight
                .iter()
                .filter(|key| key.as_str() != "local")
                .count();
            if queued >= REMOTE_IN_FLIGHT {
                return Err(Duration::from_secs(1));
            }
        }
        if !self.in_flight.insert(caller.to_owned()) {
            return Err(Duration::from_secs(1));
        }
        Ok(())
    }
}

/// One admitted unlock attempt; dropping it ends the attempt even if the request is cancelled.
struct Attempt<'a> {
    throttle: &'a parking_lot::Mutex<UnlockThrottle>,
    caller: &'a str,
    remote: bool,
    /// `Some(failed)` once the passphrase was checked.
    outcome: Option<bool>,
    now: Instant,
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        let mut throttle = self.throttle.lock();
        throttle.in_flight.remove(self.caller);
        match self.outcome {
            Some(true) => {
                let entry = throttle
                    .failures
                    .entry(self.caller.to_owned())
                    .or_insert((0, self.now));
                *entry = (entry.0.saturating_add(1), self.now);
                if self.remote {
                    throttle.remote.push_back(self.now);
                }
            }
            Some(false) => {
                throttle.failures.remove(self.caller);
            }
            None => {}
        }
    }
}

/// Record a value-free lifecycle step in the vault audit log.
pub(crate) fn lifecycle_audit(
    shared: &Shared,
    who: &str,
    action: AuditAction,
    decision: &'static str,
    detail: Option<&str>,
) -> Result<(), ()> {
    let ctx = AuditContext {
        who,
        session_id: None,
        tool_call_id: None,
    };
    openside_vault::audit_lifecycle(Service::audit_path(shared), &ctx, action, decision, detail)
        .map_err(|_| crate::diag!("opensided: vault {} audit failed", decision))
}

impl Service {
    pub(crate) fn new() -> Self {
        Self {
            vault: Mutex::new(None),
            available: AtomicBool::new(false),
            state: parking_lot::Mutex::new((None, None)),
            throttle: parking_lot::Mutex::new(UnlockThrottle::default()),
            live: parking_lot::RwLock::new(None),
            last_activity: parking_lot::Mutex::new(None),
            opened: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            after_reauth: parking_lot::Mutex::new(None),
            person_unlock: parking_lot::Mutex::new(None),
            biometrics: parking_lot::RwLock::new(Arc::new(OsBiometrics)),
        }
    }

    /// The password settings in effect.
    pub(crate) fn settings(&self, shared: &Shared) -> LiveSettings {
        if let Some(live) = self.live.read().as_ref() {
            return live.clone();
        }
        let initial = LiveSettings::from_config(&shared.config.loaded.config.vault);
        self.live.write().get_or_insert(initial).clone()
    }

    /// Replace the password settings (already saved by the caller) and apply them to the open
    /// vault at once.
    pub(crate) async fn replace_settings(&self, settings: LiveSettings) {
        let mut guard = self.vault.lock().await;
        if let Some(vault) = guard.as_mut() {
            vault.set_default_policy(access_policy(settings.default_policy));
            vault.set_autofill(settings.autofill_rules());
        }
        *self.live.write() = Some(settings);
    }

    /// Record vault use: it postpones auto-lock. With `immediately`, a lock is queued for
    /// when this use is over: it waits [`IMMEDIATE_GRACE`] and then for the vault itself, which
    /// the use holds while it reads values.
    pub(crate) fn touch(&self, shared: &Shared) {
        *self.last_activity.lock() = Some(shared.remote.instant());
        if self.settings(shared).auto_lock == VaultAutoLock::Immediately {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let shared = shared.clone();
                runtime.spawn(async move {
                    tokio::time::sleep(IMMEDIATE_GRACE).await;
                    crate::vault_manage::auto_lock(&shared).await;
                });
            }
        }
    }

    /// Whether the open vault has been idle for its auto-lock time. `immediately` still
    /// waits [`IMMEDIATE_GRACE`] so a use that touched but has not yet taken the vault is not
    /// locked out; that use queues its own lock.
    pub(crate) fn idle_due(&self, shared: &Shared) -> bool {
        let Some(idle) = self.settings(shared).auto_lock.idle() else {
            return false;
        };
        let idle = idle.max(IMMEDIATE_GRACE);
        self.available()
            && self
                .last_activity
                .lock()
                .is_some_and(|last| shared.remote.instant().saturating_duration_since(last) >= idle)
    }

    /// Auto-lock: drop the decrypted vault if it is still idle once its lock is held, and
    /// record `lock` with detail `auto_lock`, attributed to the daemon. True when it locked.
    pub(crate) async fn lock_if_idle(&self, shared: &Shared) -> bool {
        let mut guard = self.vault.lock().await;
        if guard.is_none() || !self.idle_due(shared) {
            return false;
        }
        self.available.store(false, Ordering::Release);
        *guard = None;
        self.state.lock().1 = None;
        *self.person_unlock.lock() = None;
        let _ = lifecycle_audit(
            shared,
            "opensided",
            AuditAction::Lock,
            "locked",
            Some("auto_lock"),
        );
        true
    }

    /// A person at this computer opened the vault just now.
    fn person_opened(&self, shared: &Shared) {
        *self.person_unlock.lock() = Some(shared.remote.instant());
    }

    /// Whether a person at this computer opened the vault within [`FRESH_UNLOCK`].
    pub(crate) fn recently_opened(&self, shared: &Shared) -> bool {
        self.person_unlock
            .lock()
            .is_some_and(|at| shared.remote.instant().saturating_duration_since(at) < FRESH_UNLOCK)
    }

    /// Which opening of the vault is current; see `opened`.
    pub(crate) fn opening(&self) -> u64 {
        self.opened.load(Ordering::SeqCst)
    }

    /// Unlock method of the open (or last opened) vault.
    pub(crate) fn backend(&self) -> Option<VaultBackend> {
        self.state.lock().0
    }

    #[cfg(test)]
    pub(crate) fn set_biometrics(&self, biometrics: Arc<dyn Biometrics>) {
        *self.biometrics.write() = biometrics;
    }

    #[cfg(test)]
    pub(crate) fn last_activity(&self) -> Option<Instant> {
        *self.last_activity.lock()
    }

    /// Ask the OS whether a biometric check could run now, without prompting.
    pub(crate) async fn biometric_probe(&self) -> Result<BiometricMechanism, BiometricError> {
        let biometrics = self.biometrics.read().clone();
        match tokio::time::timeout(
            Duration::from_secs(5),
            tokio::task::spawn_blocking(move || biometrics.probe()),
        )
        .await
        {
            Ok(Ok(result)) => result,
            _ => Err(BiometricError::Unavailable("service_unavailable")),
        }
    }

    /// Prompt the person for a biometric check.
    pub(crate) async fn biometric_verify(
        &self,
        reason: &'static str,
    ) -> Result<(), BiometricError> {
        let biometrics = self.biometrics.read().clone();
        match tokio::time::timeout(
            BIOMETRIC_TIMEOUT + Duration::from_secs(5),
            tokio::task::spawn_blocking(move || biometrics.verify(reason)),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(BiometricError::Failed),
            Err(_) => Err(BiometricError::Timeout),
        }
    }

    /// Check a re-entered passphrase against the open passphrase vault, throttled like
    /// unlock (per caller; this is a local-only step, so no device-wide limit applies).
    /// `Ok(false)` is a wrong passphrase.
    pub(crate) async fn check_passphrase(
        &self,
        shared: &Shared,
        who: &str,
        passphrase: &Secret,
    ) -> Result<bool, UnlockRefusal> {
        let now = shared.remote.instant();
        if let Err(wait) = self.throttle.lock().admit(who, false, now) {
            return Err(UnlockRefusal::Throttled(
                wait.as_secs() + u64::from(wait.subsec_nanos() > 0),
            ));
        }
        let mut attempt = Attempt {
            throttle: &self.throttle,
            caller: who,
            remote: false,
            outcome: None,
            now,
        };
        let guard = self.vault.lock().await;
        let vault = guard
            .as_ref()
            .ok_or(UnlockRefusal::Failed((409, "vault_unavailable")))?;
        let matched = vault
            .verify_passphrase(passphrase)
            .map_err(|_| UnlockRefusal::Failed((409, "vault_unavailable")))?;
        attempt.outcome = Some(!matched);
        Ok(matched)
    }
    /// Unlock attempts admitted and not yet finished.
    #[cfg(test)]
    pub(crate) fn throttle_in_flight(&self) -> usize {
        self.throttle.lock().in_flight.len()
    }
    pub(crate) fn available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
    pub(crate) fn audit_path(shared: &Shared) -> PathBuf {
        openside_config::data_layout::DataPath::VaultAudit.path(&shared.config.data_dir)
    }
    pub(crate) fn file(shared: &Shared) -> PathBuf {
        shared
            .config
            .loaded
            .config
            .vault
            .file(&shared.config.data_dir)
    }

    /// Read the file header and ask whether this OS's keyring answers; blocking D-Bus work
    /// runs off the async runtime and gives up after a few seconds.
    pub(crate) async fn probe(shared: &Shared) -> Probe {
        let path = Self::file(shared);
        let setting = shared.config.loaded.config.vault.unlock;
        let observed = tokio::task::spawn_blocking(move || {
            let stored = match openside_vault::stored_format(&path) {
                Ok(None) => Some(StoredVault::Missing),
                Ok(Some(StoredFormat::Passphrase)) => Some(StoredVault::Passphrase),
                Ok(Some(StoredFormat::Keyring)) => Some(StoredVault::Keyring),
                Err(_) => None,
            };
            (stored, openside_vault::keyring_reachable())
        });
        let Ok(Ok((stored, keyring_reachable))) =
            tokio::time::timeout(Duration::from_secs(5), observed).await
        else {
            return Probe {
                stored: None,
                keyring_reachable: false,
                backend: Err("probe_timeout"),
            };
        };
        let backend = match stored {
            None => Err("unreadable"),
            Some(stored) => setting
                .resolve(stored, keyring_reachable)
                .map_err(VaultResolveError::wire),
        };
        Probe {
            stored,
            keyring_reachable,
            backend,
        }
    }

    /// Daemon start: open a keyring vault, or a passphrase vault whose `passphrase_cmd` is
    /// configured. Anything else stays locked until a person unlocks or sets it up.
    pub(crate) async fn unlock(&self, shared: &Shared) {
        let probe = Self::probe(shared).await;
        let mut guard = self.vault.lock().await;
        let backend = match (probe.stored, probe.backend) {
            (Some(StoredVault::Missing), _) => return,
            (_, Err(reason)) => {
                self.state.lock().1 = Some(reason);
                crate::diag!("opensided: vault cannot be opened ({reason}); see `openside doctor`");
                return;
            }
            (_, Ok(backend)) => backend,
        };
        let path = Self::file(shared);
        let audit = Self::audit_path(shared);
        let opened = if backend.is_keyring() {
            // Biometric unlock means a person touches the sensor first; nobody does at boot.
            if self.settings(shared).biometric_unlock {
                self.state.lock().1 = Some("biometric_required");
                return;
            }
            open_with_keyring(backend, path, audit).await
        } else {
            match shared.config.loaded.config.vault.passphrase_cmd.as_deref() {
                Some(argv) => open_with_passphrase_cmd(argv, path, audit)
                    .await
                    .map_err(|()| "passphrase_cmd_failed"),
                None => return,
            }
        };
        match opened {
            // Fail closed: no unlock without its audit record.
            Ok(vault) => {
                if lifecycle_audit(shared, "opensided", AuditAction::Unlock, "unlocked", None)
                    .is_err()
                {
                    self.state.lock().1 = Some("audit_unavailable");
                    return;
                }
                self.install(shared, &mut guard, backend, vault);
            }
            Err(reason) => {
                let _ = lifecycle_audit(
                    shared,
                    "opensided",
                    AuditAction::Unlock,
                    "failed",
                    Some(reason),
                );
                self.state.lock().1 = Some(reason);
                crate::diag!(
                    "opensided: configured vault could not be unlocked ({reason}); vault tools are hidden"
                );
            }
        }
    }

    pub(crate) fn install(
        &self,
        shared: &Shared,
        slot: &mut Option<DaemonVault>,
        backend: VaultBackend,
        mut vault: DaemonVault,
    ) {
        let settings = self.settings(shared);
        vault.set_default_policy(access_policy(settings.default_policy));
        vault.set_autofill(settings.autofill_rules());
        let Ok(redactor) = vault.redactor() else {
            return;
        };
        // Filters outlive a lock on purpose (pages and logs read while unlocked may still echo
        // the values), so an unlock of an unchanged vault must not add another plaintext copy.
        let mut redactors = shared.registry.redactors.write();
        if !redactor.covered_by(&redactors) {
            redactors.push(redactor);
        }
        drop(redactors);
        *slot = Some(vault);
        self.opened.fetch_add(1, Ordering::SeqCst);
        *self.state.lock() = (Some(backend), None);
        self.available.store(true, Ordering::Release);
        self.touch(shared);
    }

    /// Value-free lifecycle state for `GET /v1/vault`.
    pub(crate) async fn status(&self, shared: &Shared) -> VaultStatus {
        let default_policy = wire_policy(self.settings(shared).default_policy);
        if self.vault.lock().await.is_some() {
            return VaultStatus {
                state: VaultState::Unlocked,
                backend: self.state.lock().0.map(wire_backend),
                reason: None,
                default_policy,
            };
        }
        let probe = Self::probe(shared).await;
        let (state, backend, reason) = match (probe.stored, probe.backend) {
            (_, Err(reason)) => (VaultState::Unavailable, None, Some(reason)),
            (Some(StoredVault::Missing), Ok(backend))
                if backend.is_keyring() && !probe.keyring_reachable =>
            {
                (
                    VaultState::Unavailable,
                    Some(backend),
                    Some("keyring_unreachable"),
                )
            }
            (Some(StoredVault::Missing), Ok(backend)) => {
                (VaultState::Uninitialized, Some(backend), None)
            }
            (_, Ok(backend)) if backend.is_keyring() && !probe.keyring_reachable => (
                VaultState::Locked,
                Some(backend),
                Some("keyring_unreachable"),
            ),
            (_, Ok(backend)) => (VaultState::Locked, Some(backend), self.state.lock().1),
        };
        VaultStatus {
            state,
            backend: backend.map(wire_backend),
            reason: reason.map(str::to_owned),
            default_policy,
        }
    }

    /// Create the vault file (`POST /v1/vault/setup`, or the first item of a keyring vault).
    /// The caller holds the vault lock, so setup cannot race another setup or unlock. Audited
    /// (`setup`) with `who`; a vault created whose record then fails stays set up and unlocked
    /// and is reported as `vault_set_up_audit_incomplete`. With biometric unlock on, a new
    /// keyring vault opens only for a person at this computer who passes the biometric check
    /// (`biometric`), as an unlock would.
    pub(crate) async fn setup(
        &self,
        shared: &Shared,
        slot: &mut Option<DaemonVault>,
        who: &str,
        passphrase: Option<Secret>,
        biometric: bool,
    ) -> Result<(), LifecycleError> {
        let created = self
            .create(shared, slot.is_some(), who, passphrase, biometric)
            .await;
        let (backend, vault) = match created {
            Ok(created) => created,
            Err(error) => {
                let _ = lifecycle_audit(shared, who, AuditAction::Setup, "failed", Some(error.1));
                return Err(error);
            }
        };
        let audited = lifecycle_audit(shared, who, AuditAction::Setup, "completed", None);
        self.install(shared, slot, backend, vault);
        if who == "local" {
            self.person_opened(shared);
        }
        audited.map_err(|()| (500, "vault_set_up_audit_incomplete"))
    }

    async fn create(
        &self,
        shared: &Shared,
        open: bool,
        who: &str,
        passphrase: Option<Secret>,
        biometric: bool,
    ) -> Result<(VaultBackend, DaemonVault), LifecycleError> {
        let probe = Self::probe(shared).await;
        if open || probe.stored != Some(StoredVault::Missing) {
            return Err((409, "vault_already_set_up"));
        }
        let backend = probe.backend.map_err(|_| (503, "vault_unsupported"))?;
        let path = Self::file(shared);
        let audit = Self::audit_path(shared);
        let created = if backend.is_keyring() {
            if passphrase.is_some() {
                return Err((400, "passphrase_not_used"));
            }
            if self.settings(shared).biometric_unlock {
                if who != "local" || !biometric {
                    return Err((403, "biometric_required"));
                }
                if self
                    .biometric_verify("set up your saved passwords")
                    .await
                    .is_err()
                {
                    return Err((403, "biometric_failed"));
                }
            }
            if !probe.keyring_reachable {
                return Err((503, "keyring_unavailable"));
            }
            let platform = keyring_lock(backend, &path).ok_or((503, "vault_unsupported"))?;
            blocking(move || Vault::create_with_platform_key(path, audit, platform)).await
        } else {
            let passphrase = passphrase.ok_or((400, "passphrase_required"))?;
            blocking(move || {
                let platform: Box<dyn PlatformLock + Send> = Box::new(PassphraseLock::default());
                Vault::create(path, audit, &passphrase, platform)
            })
            .await
        };
        let vault = created.map_err(|error| match error {
            Some(openside_vault::Error::AlreadyExists) => (409, "vault_already_set_up"),
            Some(openside_vault::Error::Unavailable) => (503, "keyring_unavailable"),
            _ => (500, "internal_error"),
        })?;
        Ok((backend, vault))
    }

    /// Decrypt an existing vault on a person's request (`POST /v1/vault/unlock`). `who` is the
    /// caller's audit label (`device:<id>`, or `local` for the Unix socket, which `remote` is
    /// false for). Each caller gets [`FREE_FAILURES`] wrong passphrases, then waits before
    /// every further attempt; paired devices together also share a limit. A throttled attempt
    /// is refused before any key derivation or vault lock and is not audited (guessing must
    /// not flood the log); every attempt that runs is. An unlock whose record cannot be
    /// written does not happen. With `biometric_unlock` on, a keyring vault opens only for a
    /// local caller that asks for (`biometric`) and passes the OS biometric check.
    pub(crate) async fn unlock_now(
        &self,
        shared: &Shared,
        who: &str,
        remote: bool,
        passphrase: Option<Secret>,
        biometric: bool,
    ) -> Result<(), UnlockRefusal> {
        let now = shared.remote.instant();
        let admitted = self.throttle.lock().admit(who, remote, now);
        if let Err(wait) = admitted {
            return Err(UnlockRefusal::Throttled(
                wait.as_secs() + u64::from(wait.subsec_nanos() > 0),
            ));
        }
        let mut attempt = Attempt {
            throttle: &self.throttle,
            caller: who,
            remote,
            outcome: None,
            now,
        };
        // Admit before credential serialization so concurrent guesses are refused, not queued.
        let _channel_change = shared.channels.change_credentials().await;
        let _model_change = shared.models.change_credentials().await;
        let mut guard = self.vault.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let refuse = |error: LifecycleError| {
            let _ = lifecycle_audit(shared, who, AuditAction::Unlock, "failed", Some(error.1));
            UnlockRefusal::Failed(error)
        };
        let probe = Self::probe(shared).await;
        if probe.stored == Some(StoredVault::Missing) {
            return Err(refuse((409, "vault_not_set_up")));
        }
        let backend = probe
            .backend
            .map_err(|_| refuse((503, "vault_unsupported")))?;
        let path = Self::file(shared);
        let audit = Self::audit_path(shared);
        let opened = if backend.is_keyring() {
            if passphrase.is_some() {
                return Err(refuse((400, "passphrase_not_used")));
            }
            if self.settings(shared).biometric_unlock {
                if remote || !biometric {
                    return Err(refuse((403, "biometric_required")));
                }
                if let Err(error) = self.biometric_verify("unlock your saved passwords").await {
                    let _ = lifecycle_audit(
                        shared,
                        who,
                        AuditAction::Unlock,
                        "denied",
                        Some(error.wire()),
                    );
                    return Err(UnlockRefusal::Failed((403, "biometric_failed")));
                }
            }
            open_with_keyring(backend, path, audit)
                .await
                .map_err(|reason| match reason {
                    "unreadable" => (500, "internal_error"),
                    _ => (503, "keyring_unavailable"),
                })
        } else {
            let passphrase = passphrase.ok_or_else(|| refuse((400, "passphrase_required")))?;
            let opened = blocking(move || {
                let platform: Box<dyn PlatformLock + Send> = Box::new(PassphraseLock::default());
                Vault::open(path, audit, &passphrase, platform)
            })
            .await;
            attempt.outcome = Some(matches!(
                opened,
                Err(Some(openside_vault::Error::Authentication))
            ));
            opened.map_err(|error| match error {
                Some(openside_vault::Error::Authentication) => (400, "wrong_passphrase"),
                _ => (500, "internal_error"),
            })
        };
        let vault = match opened {
            Ok(vault) => vault,
            Err(error) if error.1 == "wrong_passphrase" => {
                let _ = lifecycle_audit(
                    shared,
                    who,
                    AuditAction::Unlock,
                    "denied",
                    Some("wrong_passphrase"),
                );
                return Err(UnlockRefusal::Failed(error));
            }
            Err(error) => return Err(refuse(error)),
        };
        // Fail closed: the decrypted vault is dropped unless its record is written.
        lifecycle_audit(shared, who, AuditAction::Unlock, "unlocked", None)
            .map_err(|()| UnlockRefusal::Failed((500, "audit_unavailable")))?;
        self.install(shared, &mut guard, backend, vault);
        if !remote {
            self.person_opened(shared);
        }
        Ok(())
    }

    /// Open a locked vault with its recovery key and give it a new unlock key
    /// (`POST /v1/vault/recover`, local only): `passphrase` for a passphrase vault, the
    /// keyring's key for a keyring vault. Wrong keys count against the unlock throttle like
    /// wrong passphrases. Audited (`recover`): `recovered`, `denied` with
    /// `wrong_recovery_key`, or `failed` with the code. The re-keyed vault is installed even
    /// when its record fails (the file already changed), reported as
    /// `vault_recovered_audit_incomplete`.
    pub(crate) async fn recover(
        &self,
        shared: &Shared,
        who: &str,
        recovery_key: &str,
        passphrase: Option<Secret>,
    ) -> Result<(), UnlockRefusal> {
        let now = shared.remote.instant();
        if let Err(wait) = self.throttle.lock().admit(who, false, now) {
            return Err(UnlockRefusal::Throttled(
                wait.as_secs() + u64::from(wait.subsec_nanos() > 0),
            ));
        }
        let mut attempt = Attempt {
            throttle: &self.throttle,
            caller: who,
            remote: false,
            outcome: None,
            now,
        };
        let _channel_change = shared.channels.change_credentials().await;
        let _model_change = shared.models.change_credentials().await;
        let mut guard = self.vault.lock().await;
        let refuse = |error: LifecycleError| {
            let _ = lifecycle_audit(shared, who, AuditAction::Recover, "failed", Some(error.1));
            UnlockRefusal::Failed(error)
        };
        if guard.is_some() {
            return Err(refuse((409, "vault_already_unlocked")));
        }
        let probe = Self::probe(shared).await;
        if probe.stored == Some(StoredVault::Missing) {
            return Err(refuse((409, "vault_not_set_up")));
        }
        let backend = probe
            .backend
            .map_err(|_| refuse((503, "vault_unsupported")))?;
        let path = Self::file(shared);
        let audit = Self::audit_path(shared);
        match openside_vault::recovery_info(&path) {
            Ok(Some(info)) if info.current => {}
            Ok(_) => return Err(refuse((409, "recovery_not_configured"))),
            Err(_) => return Err(refuse((500, "internal_error"))),
        }
        let platform: Box<dyn PlatformLock + Send> = if backend.is_keyring() {
            if passphrase.is_some() {
                return Err(refuse((400, "passphrase_not_used")));
            }
            keyring_lock(backend, &path).ok_or_else(|| refuse((503, "vault_unsupported")))?
        } else {
            if passphrase.is_none() {
                return Err(refuse((400, "passphrase_required")));
            }
            Box::new(PassphraseLock::default())
        };
        let wrong_key = |attempt: &mut Attempt<'_>| {
            attempt.outcome = Some(true);
            let _ = lifecycle_audit(
                shared,
                who,
                AuditAction::Recover,
                "denied",
                Some("wrong_recovery_key"),
            );
            UnlockRefusal::Failed((403, "wrong_recovery_key"))
        };
        let Ok(key) = openside_vault::RecoveryKey::parse(recovery_key) else {
            return Err(wrong_key(&mut attempt));
        };
        let recovered =
            blocking(move || Vault::recover(path, audit, &key, passphrase.as_ref(), platform))
                .await;
        let vault = match recovered {
            Ok(vault) => vault,
            Err(Some(openside_vault::Error::Authentication)) => {
                return Err(wrong_key(&mut attempt));
            }
            Err(Some(openside_vault::Error::Unavailable)) => {
                return Err(refuse((503, "keyring_unavailable")));
            }
            Err(_) => return Err(refuse((500, "internal_error"))),
        };
        attempt.outcome = Some(false);
        let audited = lifecycle_audit(shared, who, AuditAction::Recover, "recovered", None);
        self.install(shared, &mut guard, backend, vault);
        self.person_opened(shared);
        audited.map_err(|()| UnlockRefusal::Failed((500, "vault_recovered_audit_incomplete")))
    }

    /// Drop the decrypted vault (`POST /v1/vault/lock`); vault tools disappear with it. The
    /// lock always happens; only then is it audited (`lock`), and a failed record is reported
    /// as `vault_locked_audit_incomplete` with the vault already locked. Output filters stay
    /// installed (see [`Self::install`]), so their copies of the values outlive the lock.
    pub(crate) async fn lock(&self, shared: &Shared, who: &str) -> Result<(), LifecycleError> {
        let mut guard = self.vault.lock().await;
        self.available.store(false, Ordering::Release);
        *guard = None;
        self.state.lock().1 = None;
        *self.person_unlock.lock() = None;
        // Still under the vault lock, so lifecycle records land in the order the steps ran.
        let audited = lifecycle_audit(shared, who, AuditAction::Lock, "locked", None);
        drop(guard);
        audited.map_err(|()| (500, "vault_locked_audit_incomplete"))
    }
}

pub(crate) fn access_policy(setting: VaultPolicySetting) -> AccessPolicy {
    match setting {
        VaultPolicySetting::Always => AccessPolicy::Always,
        VaultPolicySetting::WhileUnlocked => AccessPolicy::WhileUnlocked,
        VaultPolicySetting::Never => AccessPolicy::Never,
    }
}

pub(crate) fn wire_policy(setting: VaultPolicySetting) -> openside_proto::VaultPolicy {
    match setting {
        VaultPolicySetting::Always => openside_proto::VaultPolicy::Always,
        VaultPolicySetting::WhileUnlocked => openside_proto::VaultPolicy::WhileUnlocked,
        VaultPolicySetting::Never => openside_proto::VaultPolicy::Never,
    }
}

pub(crate) fn wire_backend(backend: VaultBackend) -> openside_proto::VaultBackend {
    match backend {
        VaultBackend::Passphrase => openside_proto::VaultBackend::Passphrase,
        VaultBackend::SecretService => openside_proto::VaultBackend::SecretService,
        VaultBackend::Native => openside_proto::VaultBackend::Native,
    }
}

/// Run blocking vault work (Argon2id, D-Bus, file I/O) off the runtime, bounded to 20s.
/// `None` means it panicked or timed out.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, openside_vault::Error> + Send + 'static,
) -> Result<T, Option<openside_vault::Error>> {
    match tokio::time::timeout(Duration::from_secs(20), tokio::task::spawn_blocking(work)).await {
        Ok(Ok(result)) => result.map_err(Some),
        _ => Err(None),
    }
}

async fn open_with_keyring(
    backend: VaultBackend,
    path: PathBuf,
    audit: PathBuf,
) -> Result<DaemonVault, &'static str> {
    let platform = keyring_lock(backend, &path).ok_or("unsupported_platform")?;
    blocking(move || Vault::open_with_platform_key(path, audit, platform))
        .await
        .map_err(|error| match error {
            Some(openside_vault::Error::Format) => "unreadable",
            Some(openside_vault::Error::Authentication) => "keyring_key_mismatch",
            _ => "keyring_locked",
        })
}

async fn open_with_passphrase_cmd(
    argv: &[String],
    path: PathBuf,
    audit: PathBuf,
) -> Result<DaemonVault, ()> {
    let program = argv
        .first()
        .filter(|program| !program.is_empty())
        .ok_or(())?;
    let result = async {
        let mut command = tokio::process::Command::new(program);
        command
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| ())?;
        let stdout = child.stdout.take().ok_or(())?;
        let mut output = Zeroizing::new(Vec::new());
        stdout
            .take(65_537)
            .read_to_end(&mut output)
            .await
            .map_err(|_| ())?;
        if output.len() > 65_536 || !child.wait().await.map_err(|_| ())?.success() {
            return Err(());
        }
        let mut text = match String::from_utf8(std::mem::take(&mut *output)) {
            Ok(text) => Zeroizing::new(text),
            Err(error) => {
                let _bytes = Zeroizing::new(error.into_bytes());
                return Err(());
            }
        };
        let length = text.trim_end_matches(['\r', '\n']).len();
        text.truncate(length);
        let passphrase = Secret::new(std::mem::take(&mut *text));
        blocking(move || {
            let platform: Box<dyn PlatformLock + Send> = Box::new(PassphraseLock::default());
            Vault::open(path, audit, &passphrase, platform)
        })
        .await
        .map_err(|_| ())
    };
    tokio::time::timeout(Duration::from_secs(20), result)
        .await
        .map_err(|_| ())?
}

/// Keyring item identity: one wrapping key per vault file path.
fn vault_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[allow(clippy::needless_return)]
pub(crate) fn keyring_lock(
    backend: VaultBackend,
    path: &Path,
) -> Option<Box<dyn PlatformLock + Send>> {
    let id = vault_id(path);
    match backend {
        VaultBackend::SecretService => {
            #[cfg(target_os = "linux")]
            return Some(Box::new(openside_vault::SecretServiceLock::session(id)));
            #[cfg(not(target_os = "linux"))]
            return None;
        }
        VaultBackend::Native => {
            #[cfg(any(target_os = "macos", windows))]
            return Some(Box::new(openside_vault::NativeKeyringLock::new(id)));
            #[cfg(not(any(target_os = "macos", windows)))]
            return None;
        }
        VaultBackend::Passphrase => None,
    }
}

fn audit_ctx(ctx: &ToolEnv) -> AuditContext<'_> {
    AuditContext {
        who: "opensided",
        session_id: Some(ctx.session.as_str()),
        tool_call_id: Some(ctx.tool_call_id.as_str()),
    }
}

fn denied(outcome: LoginOutcome) -> Option<ToolExec> {
    match outcome {
        LoginOutcome::Denied(denial) => Some(ToolExec::Denied {
            reason: format!("vault_{}", denial.wire()),
            hint: Some(
                "The vault refused this item for the current page; the refusal is audited.".into(),
            ),
        }),
        _ => None,
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoginArgs {
    tab_id: String,
    site: String,
    username_ref: String,
    password_ref: String,
    submit_ref: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OtpArgs {
    tab_id: String,
    site: String,
    field_ref: String,
}
#[derive(Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PasskeyAction {
    /// Create a new passkey for the site and save it in the vault.
    Register,
    /// Sign in with a saved passkey.
    Login,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PasskeyArgs {
    tab_id: String,
    site: String,
    action: PasskeyAction,
    /// Button that makes the page call navigator.credentials.create (register) or .get (login).
    submit_ref: String,
}

/// Policy actions shared by tools that click a page control after authorization.
async fn vault_actions(
    ctx: &ToolEnv,
    tab_id: &str,
    submit_ref: Option<&str>,
) -> std::result::Result<Vec<Action>, ToolError> {
    let browser = ctx
        .shared
        .registry
        .browser
        .for_tab(&ctx.shared, &ctx.session, tab_id)
        .await?;
    let origin = Controller::origin(&browser, &ctx.session, tab_id).await?;
    let mut actions = vec![Action::new(Capability::VaultUse, &origin)];
    if let Some(reference) = submit_ref {
        let snapshot = browser.snapshot(tab_id).await.map_err(browser_error)?;
        let node = snapshot
            .nodes
            .iter()
            .find(|node| node.reference.as_deref() == Some(reference))
            .ok_or_else(|| ToolError("Unknown submit_ref; take a fresh snapshot".into()))?;
        if super::browser::sensitive(&node.name, &node.role) {
            actions.push(Action::new(Capability::Sensitive, origin));
        }
    }
    Ok(actions)
}

pub(crate) struct Login;
impl DaemonTool for Login {
    fn name(&self) -> &'static str {
        "vault_login"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<LoginArgs>(self.name(), "Fill saved username/password into browser refs after exact-origin vault authorization. Values never enter model output. Optional submit_ref clicks only after fill.", ApprovalTier::Write, true)
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available() && ctx.shared.registry.vault.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: LoginArgs = parse(args)?;
            if args.site.is_empty() || args.username_ref.is_empty() || args.password_ref.is_empty()
            {
                return Err(ToolError("site and field refs must not be empty".into()));
            }
            vault_actions(ctx, &args.tab_id, args.submit_ref.as_deref()).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"site":args["site"],"username_ref":args["username_ref"],"password_ref":args["password_ref"],"submit_ref":args["submit_ref"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: LoginArgs = parse(&args)?;
                let controller = &ctx.shared.registry.browser;
                let browser = controller.for_tab(&ctx.shared, &ctx.session, &args.tab_id).await?;
                let origin = Controller::origin(&browser, &ctx.session, &args.tab_id).await?;
                check_origin(ctx, &origin)?;
                let _ = crate::connections::begin_login(&ctx.shared, &browser, &ctx.session, &args.tab_id, &origin).await;
                let document = browser.document_lifetime_id(&args.tab_id).await.ok();
                let browser = &browser;
                let args = &args;
                // Auto-submit off: fill only; the person (or a later click) submits.
                let submit = args.submit_ref.as_deref().filter(|_| {
                    ctx.shared.registry.vault.settings(&ctx.shared).autofill.auto_submit
                });
                let mut guard = ctx.shared.registry.vault.vault.lock().await;
                let vault = guard.as_mut().ok_or_else(|| ToolError("Vault is unavailable".into()))?;
                ctx.shared.registry.vault.touch(&ctx.shared);
                let origin_check = origin.clone();
                let audit = audit_ctx(ctx);
                let outcome = vault.vault_login_async(&audit, AuditAction::Fill, &args.site, &origin, |secrets| Box::pin(async move {
                    controller.mark_secret(&ctx.shared, browser, &args.tab_id).await?;
                    if browser.origin(&args.tab_id).await.map_err(|_| ())? != origin_check { return Err(()); }
                    if let Some(gateway) = browser.gateway() {
                        let username = browser.resolve_ref(ctx.session.as_str(), &args.tab_id, &args.username_ref).await.map_err(|_| ())?;
                        let password = browser.resolve_ref(ctx.session.as_str(), &args.tab_id, &args.password_ref).await.map_err(|_| ())?;
                        gateway.fill_secret_node(&args.tab_id, username.backend_node_id, &origin_check, secrets.username).await.map_err(|_| ())?;
                        gateway.fill_secret_node(&args.tab_id, password.backend_node_id, &origin_check, secrets.password).await.map_err(|_| ())?;
                    } else {
                        browser.insert_text(ctx.session.as_str(), &args.tab_id, &args.username_ref, secrets.username).await.map_err(|_| ())?;
                        if browser.origin(&args.tab_id).await.map_err(|_| ())? != origin_check { return Err(()); }
                        browser.insert_text(ctx.session.as_str(), &args.tab_id, &args.password_ref, secrets.password).await.map_err(|_| ())?;
                    }
                    if let Some(reference) = submit { browser.act(ctx.session.as_str(), &args.tab_id, reference, BrowserAction::Click).await.map_err(|_| ())?; }
                    Ok(())
                })).await.map_err(|_| ToolError("Vault fill or audit failed".into()))?;
                drop(guard);
                controller.persist(&ctx.shared).await.map_err(|_| ToolError("Browser state could not be saved".into()))?;
                if outcome == LoginOutcome::Filled {
                    crate::connections::remember_login_item(&ctx.shared, &ctx.session, &args.tab_id, &args.site).await;
                }
                if outcome == LoginOutcome::Filled && submit.is_some() {
                    super::login::complete_vault_login(&ctx.shared, browser, &ctx.session, &args.tab_id, document.as_deref(), &args.site).await;
                }
                if outcome == LoginOutcome::Filled && args.submit_ref.is_some() && submit.is_none() {
                    return Ok(done(json!({"status":"filled","submitted":false,"note":"Auto-submit is off in the password settings, so the form was filled but not submitted."})));
                }
                Ok(denied(outcome).unwrap_or_else(|| done(json!({"status":if outcome == LoginOutcome::Filled { "filled" } else { "fill_failed" }}))))
            }.await;
            result.unwrap_or_else(|_: ToolError| done(json!({"status":"fill_failed"})))
        })
    }
}

pub(crate) struct Otp;
impl DaemonTool for Otp {
    fn name(&self) -> &'static str {
        "vault_otp"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<OtpArgs>(self.name(), "Generate a saved TOTP in the daemon and insert it into a browser ref after exact-origin vault authorization. No code or seed is returned.", ApprovalTier::Write, false)
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available() && ctx.shared.registry.vault.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: OtpArgs = parse(args)?;
            if args.site.is_empty() || args.field_ref.is_empty() {
                return Err(ToolError("site and field_ref must not be empty".into()));
            }
            vault_actions(ctx, &args.tab_id, None).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"site":args["site"],"field_ref":args["field_ref"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: OtpArgs = parse(&args)?;
                match fill_saved_totp(ctx, &args.tab_id, &args.field_ref, Some(&args.site)).await? {
                    SavedTotp::Used(exec) => Ok(*exec),
                    SavedTotp::Missing | SavedTotp::Ambiguous => {
                        Err(ToolError("OTP fill failed".into()))
                    }
                }
            }
            .await;
            result.unwrap_or_else(|error: ToolError| ToolExec::Denied {
                reason: error.0,
                hint: None,
            })
        })
    }
}

/// What [`fill_saved_totp`] did.
pub(super) enum SavedTotp {
    /// The vault decided: filled, or a value-free refusal.
    Used(Box<ToolExec>),
    /// No item accepting the tab's origin holds an authenticator; nothing was touched.
    Missing,
    /// Several items (accounts) do; none is picked and nothing was touched.
    Ambiguous,
}

/// Generate the TOTP of `site` (with `None`: of the one item holding one for the tab's
/// origin) inside the vault callback and insert it into `field`, spread over the split code
/// boxes that follow it if there are any; the code never leaves the daemon.
pub(super) async fn fill_saved_totp(
    ctx: &ToolEnv,
    tab_id: &str,
    field: &str,
    site: Option<&str>,
) -> std::result::Result<SavedTotp, ToolError> {
    let controller = &ctx.shared.registry.browser;
    let browser = controller
        .for_tab(&ctx.shared, &ctx.session, tab_id)
        .await?;
    let origin = Controller::origin(&browser, &ctx.session, tab_id).await?;
    check_origin(ctx, &origin)?;
    let mut vault = ctx.shared.registry.vault.vault.lock().await;
    let vault = vault
        .as_mut()
        .ok_or_else(|| ToolError("Vault is unavailable".into()))?;
    ctx.shared.registry.vault.touch(&ctx.shared);
    let site = match site {
        Some(site) => site.to_owned(),
        None => match vault.totp_site_for(&origin) {
            Ok(TotpMatch::One(site)) => site,
            Ok(TotpMatch::Ambiguous) => return Ok(SavedTotp::Ambiguous),
            _ => return Ok(SavedTotp::Missing),
        },
    };
    let owner = ctx.session.as_str().to_owned();
    let tab = tab_id.to_owned();
    // The field and any split boxes are pinned before a code exists.
    let node = browser
        .resolve_ref(&owner, &tab, field)
        .await
        .map_err(browser_error)?;
    if node.origin != origin {
        return Err(ToolError("Tab origin changed".into()));
    }
    let boxes = browser
        .resolve_otp_group(&owner, &tab, &node)
        .await
        .map_err(browser_error)?;
    let unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ToolError("Clock unavailable".into()))?
        .as_secs();
    controller
        .mark_secret(&ctx.shared, &browser, &tab)
        .await
        .map_err(|_| ToolError("Browser document unavailable".into()))?;
    let fill_controller = controller.clone();
    let origin_check = origin.clone();
    let audit = audit_ctx(ctx);
    let _ =
        crate::connections::begin_login(&ctx.shared, &browser, &ctx.session, tab_id, &origin).await;
    let outcome = vault
        .vault_login_async(&audit, AuditAction::Otp, &site, &origin, |secrets| {
            Box::pin(async move {
                secrets
                    .fill_totp(unix_seconds, move |code| {
                        Box::pin(async move {
                            fill_controller.redact_secret(code);
                            if browser.origin(&tab).await.map_err(|_| ())? != origin_check {
                                return Err(());
                            }
                            let Some(gateway) = browser.gateway() else {
                                return browser
                                    .insert_otp_nodes(&owner, &tab, &node, &boxes, code)
                                    .await
                                    .map_err(|_| ());
                            };
                            if boxes.is_empty() {
                                return gateway
                                    .fill_secret_node(
                                        &tab,
                                        node.backend_node_id,
                                        &origin_check,
                                        code,
                                    )
                                    .await
                                    .map_err(|_| ());
                            }
                            // The patched browser fills one box per autofill call. Before each,
                            // the pinned boxes must still be the visible consecutive run.
                            let chars: Vec<char> = code.chars().collect();
                            if chars.len() != boxes.len() + 1 {
                                return Err(());
                            }
                            let all: Vec<&openside_browser::NodeHandle> =
                                std::iter::once(&node).chain(&boxes).collect();
                            for (handle, ch) in all.into_iter().zip(chars) {
                                let current = browser
                                    .resolve_otp_group(&owner, &tab, &node)
                                    .await
                                    .map_err(|_| ())?;
                                if current != boxes {
                                    return Err(());
                                }
                                let mut buffer = [0; 4];
                                gateway
                                    .fill_secret_node(
                                        &tab,
                                        handle.backend_node_id,
                                        &origin_check,
                                        ch.encode_utf8(&mut buffer),
                                    )
                                    .await
                                    .map_err(|_| ())?;
                            }
                            Ok(())
                        })
                    })
                    .await
                    .map_err(|_| ())
            })
        })
        .await
        .map_err(|_| ToolError("Vault OTP or audit failed".into()))?;
    controller
        .persist(&ctx.shared)
        .await
        .map_err(|_| ToolError("Browser state could not be saved".into()))?;
    if let Some(refusal) = denied(outcome) {
        return Ok(SavedTotp::Used(Box::new(refusal)));
    }
    match outcome {
        LoginOutcome::Filled => {
            crate::connections::remember_login_item(&ctx.shared, &ctx.session, tab_id, &site).await;
            super::otp::mark_filled(&ctx.session, tab_id, &origin);
            Ok(SavedTotp::Used(Box::new(done(json!({"status":"filled"})))))
        }
        _ => Err(ToolError("OTP fill failed".into())),
    }
}

pub(crate) struct Passkey;
impl DaemonTool for Passkey {
    fn name(&self) -> &'static str {
        "vault_passkey"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<PasskeyArgs>(self.name(), "Register or sign in with a vault-held software passkey (WebAuthn). The daemon installs a virtual authenticator on the tab, clicks submit_ref (the page's create/sign-in button) and saves or uses the credential after exact-origin vault authorization. Keys never enter model output.", ApprovalTier::Write, false)
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available() && ctx.shared.registry.vault.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: PasskeyArgs = parse(args)?;
            if args.site.is_empty() || args.submit_ref.is_empty() {
                return Err(ToolError("site and submit_ref must not be empty".into()));
            }
            vault_actions(ctx, &args.tab_id, Some(&args.submit_ref)).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"site":args["site"],"action":args["action"],"submit_ref":args["submit_ref"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: PasskeyArgs = parse(&args)?;
                let controller = &ctx.shared.registry.browser;
                let browser = controller
                    .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
                    .await?;
                let origin = Controller::origin(&browser, &ctx.session, &args.tab_id).await?;
                check_origin(ctx, &origin)?;
                // The ceremony runs in a tab of its own that loads this page again, and finds
                // the control there by role and name; both are read before the vault lock so
                // that a page still loading is noticed here.
                let url = browser.url(&args.tab_id).await.map_err(browser_error)?;
                let snapshot = browser
                    .snapshot(&args.tab_id)
                    .await
                    .map_err(browser_error)?;
                let node = snapshot
                    .nodes
                    .iter()
                    .find(|node| node.reference.as_deref() == Some(args.submit_ref.as_str()))
                    .ok_or_else(|| ToolError("Unknown submit_ref; take a fresh snapshot".into()))?;
                let lookalikes = snapshot
                    .nodes
                    .iter()
                    .filter(|other| other.role == node.role && other.name == node.name)
                    .count();
                if lookalikes != 1 {
                    return Err(ToolError(
                        "submit_ref is ambiguous: another control has the same role and name"
                            .into(),
                    ));
                }
                let (submit_role, submit_name) = (node.role.clone(), node.name.clone());
                if args.action == PasskeyAction::Login {
                    let _ = crate::connections::begin_login(
                        &ctx.shared,
                        &browser,
                        &ctx.session,
                        &args.tab_id,
                        &origin,
                    )
                    .await;
                }
                let mut guard = ctx.shared.registry.vault.vault.lock().await;
                let vault = guard
                    .as_mut()
                    .ok_or_else(|| ToolError("Vault is unavailable".into()))?;
                ctx.shared.registry.vault.touch(&ctx.shared);
                let port = Arc::new(ceremony::BrowserPort::new(
                    browser.clone(),
                    ctx.session.as_str().to_owned(),
                    url,
                    submit_role,
                    submit_name,
                ));
                let audit = audit_ctx(ctx);
                let ceremony_origin = origin.clone();
                let outcome = match args.action {
                    PasskeyAction::Register => {
                        vault
                            .passkey_register(&audit, &args.site, &origin, move || {
                                Box::pin(async move {
                                    let job = ceremony::Job::Register;
                                    match ceremony::run(
                                        port,
                                        ceremony_origin,
                                        Default::default(),
                                        job,
                                    )
                                    .await
                                    {
                                        Ok(ceremony::Outcome::Registered(keys)) => Ok(keys),
                                        _ => Err(()),
                                    }
                                })
                            })
                            .await
                    }
                    PasskeyAction::Login => {
                        vault
                            .passkey_login(&audit, &args.site, &origin, move |keys| {
                                Box::pin(async move {
                                    let job = ceremony::Job::Login(keys);
                                    match ceremony::run(
                                        port,
                                        ceremony_origin,
                                        Default::default(),
                                        job,
                                    )
                                    .await
                                    {
                                        Ok(ceremony::Outcome::SignedIn(counts)) => Ok(counts),
                                        _ => Err(()),
                                    }
                                })
                            })
                            .await
                    }
                }
                .map_err(|_| ToolError("Vault passkey operation or audit failed".into()))?;
                if outcome == LoginOutcome::Filled && args.action == PasskeyAction::Register {
                    // The new private key is now a known secret for every output path.
                    if let Ok(redactor) = vault.redactor_for(&args.site) {
                        ctx.shared.registry.redactors.write().push(redactor);
                    }
                }
                drop(guard);
                controller
                    .persist(&ctx.shared)
                    .await
                    .map_err(|_| ToolError("Browser state could not be saved".into()))?;
                if let Some(refusal) = denied(outcome) {
                    return Ok(refusal);
                }
                if outcome == LoginOutcome::Filled && args.action == PasskeyAction::Login {
                    crate::connections::remember_login_item(
                        &ctx.shared,
                        &ctx.session,
                        &args.tab_id,
                        &args.site,
                    )
                    .await;
                    let _ = crate::connections::complete_login(
                        &ctx.shared,
                        &browser,
                        &ctx.session,
                        &args.tab_id,
                        openside_proto::ConnectionSource::Vault,
                        Some(&args.site),
                    )
                    .await;
                }
                Ok(done(json!({"status": match (outcome, args.action) {
                    (LoginOutcome::Filled, PasskeyAction::Register) => "registered",
                    (LoginOutcome::Filled, PasskeyAction::Login) => "signed_in",
                    _ => "failed",
                }})))
            }
            .await;
            result.unwrap_or_else(|error: ToolError| ToolExec::Denied {
                reason: error.0,
                hint: None,
            })
        })
    }
}

fn check_origin(ctx: &ToolEnv, origin: &str) -> std::result::Result<(), ToolError> {
    if ctx
        .approved_actions
        .iter()
        .any(|action| action.capability == Capability::VaultUse && action.target == origin)
    {
        Ok(())
    } else {
        Err(ToolError(
            "Page origin changed; take a fresh snapshot".into(),
        ))
    }
}
