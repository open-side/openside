//! Encrypted credential storage for trusted host-side fill operations.
//!
//! # Threat model
//! The model, web page and tool-output consumer must never receive a vault or a
//! plaintext accessor. Only trusted Rust host code constructs entries and supplies
//! a fill callback after independently obtaining the browser's current origin.
//! This crate verifies that origin, not the browser's navigation state: the host
//! must prevent navigation races during fill. A malicious callback, compromised
//! process, debugger, swap/core dumps and filesystem rollback are out of scope.
//! Secret allocations and derived key buffers are zeroized; cryptographic library
//! internals, compiler/register copies and caller-owned strings cannot be promised
//! erased.
//!
//! # File format
//! Binary: `OSVAULT\0` (8 bytes), big-endian version u32, 16-byte salt, 24-byte
//! XChaCha20 nonce, authenticated ciphertext/tag. The whole 52-byte header is AEAD
//! associated data. The decrypted payload is UTF-8 JSON containing all entries,
//! including metadata, passkeys and per-item policy overrides.
//! - Version 1 (passphrase): key = Argon2id v19 (m=19456 KiB, t=2, p=1, 32 bytes) of
//!   the passphrase and salt.
//! - Version 2 (platform keyring): key = HKDF-SHA256(ikm = 32-byte wrapping key held by a
//!   [`PlatformLock`] backend, salt, info `openside-vault v2 file key`). The wrapping key
//!   lives only in the OS secret store; neither key is written to disk.
//!
//! Salt is random per vault, nonce random per write. Unknown versions and
//! authentication failures are rejected without plaintext diagnostics. Files are
//! capped at 16 MiB, atomically replaced, and created with private permissions. The
//! containing directory must be trusted; callers must serialize writers.
//!
//! # Policy and audit
//! Each item has an optional policy override; otherwise the vault default applies.
//! `Always` still needs a decrypted vault; `WhileUnlocked` additionally checks the
//! platform lock; `Never` never releases. Full lock drops all decrypted entries/key.
//! Audit JSONL is append-only through this API, not tamper-proof against the local
//! account. Every decision is logged with caller, session and tool-call ids before a
//! callback can run, and completion adds a second record. Records never contain
//! values: labels are validated, origins normalized (no query/path/credentials), and
//! live-secret redaction is applied to serialized lines.
//!
//! Redaction is defense in depth, not permission to emit secrets: encoded/partial
//! leaks outside the documented transforms and screenshots require host controls.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod audit;
mod channel;
mod origin;
mod passkey;
mod platform;
mod provider;
mod secret;
mod subscription;
mod sync;

pub use audit::{read_audit, AuditAction, AuditContext, AuditQuery, AuditRecord};
pub use channel::ChannelSecretInfo;
pub use origin::AllowedOrigin;
pub use passkey::{PasskeyCredential, PasskeyRelease, SignCount};
#[cfg(any(target_os = "macos", windows))]
pub use platform::native::NativeKeyringLock;
#[cfg(target_os = "linux")]
pub use platform::secret_service::SecretServiceLock;
pub use platform::{keyring_reachable, PassphraseLock, PlatformLock, WrappingKey};
pub use provider::ProviderKeyInfo;
pub use secret::{parse_base32_seed, Redactor, Secret, Totp, TotpAlgorithm, TotpParams, TotpSeed};
pub use subscription::SubscriptionInfo;
#[cfg(target_os = "macos")]
pub use subscription::{delete_cli_keychain, read_cli_keychain, write_cli_keychain};
pub use sync::PreparedImport;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    fmt,
    fs::{File, OpenOptions},
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"OSVAULT\0";
const HEADER: usize = 52;
const MAX_FILE: u64 = 16 * 1024 * 1024;
const VERSION_PASSPHRASE: u32 = 1;
const VERSION_KEYRING: u32 = 2;

/// Deliberately value-free errors, safe to expose to tool consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Filesystem operation failed; paths and operating-system messages are hidden.
    Io,
    /// Audit write failed; a callback is not started without a decision record.
    Audit,
    /// The operation took effect but its completion audit record could not be written.
    AuditIncomplete,
    /// File header or decrypted payload is malformed or unsupported.
    Format,
    /// Passphrase/key is incorrect, the wrong unlock method was used, or ciphertext was modified.
    Authentication,
    /// Cryptographic operation or random source failed.
    Crypto,
    /// No decrypted vault is available.
    Locked,
    /// Origin is not a supported strict HTTP(S) DNS origin.
    InvalidOrigin,
    /// TOTP seed is not canonical base32, or its `otpauth://` URI or parameters are invalid.
    InvalidSeed,
    /// Entry id is invalid or an entry contains no allowed origins.
    InvalidEntry,
    /// A new vault cannot overwrite an existing file.
    AlreadyExists,
    /// A trusted fill callback failed without exposing its diagnostics.
    FillFailed,
    /// The platform secret store is unreachable, locked behind a prompt, or holds no key.
    Unavailable,
    /// No item has this id.
    UnknownItem,
    /// An item with this id already exists.
    ItemExists,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Io => "vault I/O failed",
            Self::Audit => "vault audit failed",
            Self::AuditIncomplete => "vault operation done but its audit record failed",
            Self::Format => "invalid vault format",
            Self::Authentication => "vault authentication failed",
            Self::Crypto => "vault cryptography failed",
            Self::Locked => "vault locked",
            Self::InvalidOrigin => "invalid origin",
            Self::InvalidSeed => "invalid TOTP seed or parameters",
            Self::InvalidEntry => "invalid vault entry",
            Self::AlreadyExists => "vault already exists",
            Self::FillFailed => "vault fill failed",
            Self::Unavailable => "platform secret store unavailable",
            Self::UnknownItem => "unknown vault item",
            Self::ItemExists => "vault item already exists",
        })
    }
}
impl std::error::Error for Error {}

/// Host policy governing secret release, never bypassing file encryption.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub enum AccessPolicy {
    /// Permit release while the vault is decrypted.
    Always,
    /// Additionally require an unlocked platform backend.
    WhileUnlocked,
    /// Never release this entry.
    Never,
}

impl AccessPolicy {
    /// Stable lowercase name used in audit records and APIs.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::WhileUnlocked => "while_unlocked",
            Self::Never => "never",
        }
    }
}

/// Why nothing was released. Value-free and safe to show to the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Denial {
    /// The vault file is not decrypted.
    Locked,
    /// No item has the requested id.
    UnknownSite,
    /// The item's effective policy is `Never`.
    PolicyNever,
    /// The item's effective policy is `WhileUnlocked` and the platform is locked.
    PlatformLocked,
    /// The current page origin is not in the item's allowlist.
    OriginMismatch,
    /// A passkey's RP ID does not belong to the current page origin.
    RpMismatch,
    /// The item has no passkey usable from the current origin.
    NoPasskey,
    /// An item accepting the origin already holds an authenticator; it is never replaced.
    AlreadyRegistered,
}

impl Denial {
    /// Stable snake_case reason.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::UnknownSite => "unknown_site",
            Self::PolicyNever => "policy_never",
            Self::PlatformLocked => "platform_locked",
            Self::OriginMismatch => "origin_mismatch",
            Self::RpMismatch => "rp_mismatch",
            Self::NoPasskey => "no_passkey",
            Self::AlreadyRegistered => "already_registered",
        }
    }
}

/// Result of [`Vault::totp_site_for`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TotpMatch {
    /// No item accepting the origin holds an authenticator.
    None,
    /// Exactly one does; its item id.
    One(String),
    /// Several do (several accounts on one site); the caller must not pick one.
    Ambiguous,
}

/// An encrypted entry. Username, password, optional TOTP seed and passkey private
/// keys are secrets; TOTP parameters are not.
pub struct Entry {
    site: String,
    origins: Vec<AllowedOrigin>,
    username: Secret,
    password: Secret,
    totp: Option<Totp>,
    policy: Option<AccessPolicy>,
    passkeys: Vec<PasskeyCredential>,
    /// Operation id of the [`Vault::register_totp`] call that created this item, so a host
    /// that lost the outcome of that call can tell its own save from any other at the origin.
    registration: Option<String>,
    provider_key: Option<ProviderKeyInfo>,
    channel_secret: Option<ChannelSecretInfo>,
    subscription: Option<SubscriptionInfo>,
    subscription_tokens: Vec<Secret>,
}
impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Entry {
    /// Construct an entry with a non-secret `[a-z0-9:_-]{1,64}` site identifier and an
    /// explicit policy override. Build `totp` with [`Totp::parse`] (an `otpauth://` URI or a
    /// bare base32 seed) or [`Totp::new`] (explicit parameters).
    pub fn new(
        site: String,
        origins: Vec<AllowedOrigin>,
        username: Secret,
        password: Secret,
        totp: Option<Totp>,
        policy: AccessPolicy,
    ) -> Result<Self, Error> {
        Self::build(
            site,
            origins,
            username,
            password,
            totp,
            Some(policy),
            Vec::new(),
        )
    }

    /// Like [`Self::new`], but `None` stores no per-item override so the vault default
    /// applies.
    pub fn with_policy_override(
        site: String,
        origins: Vec<AllowedOrigin>,
        username: Secret,
        password: Secret,
        totp: Option<Totp>,
        policy: Option<AccessPolicy>,
    ) -> Result<Self, Error> {
        Self::build(site, origins, username, password, totp, policy, Vec::new())
    }

    fn build(
        site: String,
        origins: Vec<AllowedOrigin>,
        username: Secret,
        password: Secret,
        totp: Option<Totp>,
        policy: Option<AccessPolicy>,
        passkeys: Vec<PasskeyCredential>,
    ) -> Result<Self, Error> {
        if !valid_label(&site) || origins.is_empty() {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            site,
            origins,
            username,
            password,
            totp,
            policy,
            passkeys,
            registration: None,
            provider_key: None,
            channel_secret: None,
            subscription: None,
            subscription_tokens: Vec::new(),
        })
    }

    /// Masking filter for every secret of this entry. Install it before the entry is saved
    /// (see [`Vault::add_item`]), so no committed outcome leaves its values unmasked.
    pub fn redactor(&self) -> Redactor {
        Redactor::new(self.secrets())
    }

    fn secrets(&self) -> impl Iterator<Item = &Secret> {
        [&self.username, &self.password]
            .into_iter()
            .chain(self.totp.iter().map(|totp| &totp.seed))
            .chain(self.passkeys.iter().map(|passkey| &passkey.private_key))
            .chain(self.subscription_tokens.iter())
    }
}

/// Value-free item metadata for listings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemInfo {
    /// Item id (site label).
    pub site: String,
    /// Allowed origins in normalized `scheme://[*.]host:port` form.
    pub origins: Vec<String>,
    /// Per-item override; `None` inherits the vault default.
    pub policy: Option<AccessPolicy>,
    /// Policy actually enforced.
    pub effective_policy: AccessPolicy,
    /// Whether a TOTP seed is stored.
    pub has_totp: bool,
    /// RP IDs of stored passkeys.
    pub passkey_rp_ids: Vec<String>,
}

/// A saved sign-in accepting an origin ([`Vault::logins_for`]); no value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginAccount {
    /// Item id.
    pub site: String,
    /// The username with all but its first character (and an email's domain) masked.
    pub username_hint: String,
    /// Policy actually enforced for the item.
    pub effective_policy: AccessPolicy,
}

/// What [`Vault::save_login`] kept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedLogin {
    /// Item id.
    pub site: String,
    /// A new item, rather than a new password for an existing one.
    pub created: bool,
}

/// `j***@naver.com` for an email address, `j***` otherwise; empty for an empty username.
/// Nothing past the first character of the account name is shown.
fn username_hint(username: &str) -> String {
    let Some(first) = username.chars().next() else {
        return String::new();
    };
    match username.rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {
            format!("{first}***@{domain}")
        }
        _ => format!("{first}***"),
    }
}

/// Borrowed secrets available solely during the trusted, verified fill callback.
/// The host must not log, clone into output, or retain these values.
pub struct FillSecrets<'a> {
    /// Username to fill.
    pub username: &'a str,
    /// Password to fill.
    pub password: &'a str,
    /// Optional TOTP factor, to generate a code inside the trusted callback.
    pub totp: Option<&'a Totp>,
}
impl fmt::Debug for FillSecrets<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl FillSecrets<'_> {
    /// Generate the item's RFC 6238 code (its stored digits, period and algorithm) and lend
    /// it only to a trusted fill callback. The code and decoded seed are zeroized afterwards.
    pub async fn fill_totp(
        &self,
        unix_seconds: u64,
        fill: impl for<'code> FnOnce(
            &'code str,
        )
            -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'code>>,
    ) -> Result<(), Error> {
        let code = self.totp.ok_or(Error::InvalidSeed)?.code(unix_seconds)?;
        fill(&code.0).await.map_err(|()| Error::FillFailed)
    }
}

/// A value-free result suitable for tool output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginOutcome {
    /// The callback completed successfully (filled, or passkey registered/used).
    Filled,
    /// No secret was released.
    Denied(Denial),
    /// The callback ran but reported failure without an error payload.
    FillFailed,
}

/// Future returned by a passkey login callback: observed signature counters.
pub type PasskeyUse<'f> = Pin<Box<dyn Future<Output = Result<Vec<SignCount>, ()>> + Send + 'f>>;
/// Future returned by a passkey registration callback: newly created credentials.
pub type PasskeyCapture<'f> =
    Pin<Box<dyn Future<Output = Result<Vec<PasskeyCredential>, ()>> + Send + 'f>>;

struct State {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; 16],
    version: u32,
    entries: Vec<Entry>,
}

/// How an existing vault file is unlocked, read from its unauthenticated header only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoredFormat {
    /// Version 1: passphrase (Argon2id).
    Passphrase,
    /// Version 2: wrapping key in a platform keyring.
    Keyring,
}

/// The unlock method of the vault file at `path`, `None` when no file exists. Reads the
/// first 12 bytes only; nothing is decrypted or authenticated. Unknown magic or versions
/// are [`Error::Format`].
pub fn stored_format(path: impl AsRef<Path>) -> Result<Option<StoredFormat>, Error> {
    let mut file = match File::open(path.as_ref()) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Io),
    };
    let mut header = [0u8; 12];
    file.read_exact(&mut header).map_err(|_| Error::Format)?;
    if &header[..8] != MAGIC {
        return Err(Error::Format);
    }
    match u32::from_be_bytes([header[8], header[9], header[10], header[11]]) {
        VERSION_PASSPHRASE => Ok(Some(StoredFormat::Passphrase)),
        VERSION_KEYRING => Ok(Some(StoredFormat::Keyring)),
        _ => Err(Error::Format),
    }
}

/// Encrypted vault handle, with optional decrypted state and an audit destination.
/// Debug hides paths, entries and backend state, even for custom backends.
pub struct Vault<L: PlatformLock = PassphraseLock> {
    path: PathBuf,
    audit_path: PathBuf,
    platform: L,
    default_policy: AccessPolicy,
    state: Option<State>,
}
impl<L: PlatformLock> fmt::Debug for Vault<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl<L: PlatformLock> Drop for Vault<L> {
    fn drop(&mut self) {
        self.lock();
    }
}

impl<L: PlatformLock> Vault<L> {
    /// Create a new passphrase-encrypted (version 1) vault without overwriting files.
    pub fn create(
        path: impl AsRef<Path>,
        audit_path: impl AsRef<Path>,
        passphrase: &Secret,
        platform: L,
    ) -> Result<Self, Error> {
        Self::create_inner(
            path.as_ref(),
            audit_path.as_ref(),
            platform,
            Some(passphrase),
        )
    }

    /// Create a new version-2 vault whose key derives from a wrapping key the platform
    /// backend generates and stores in its secret store.
    pub fn create_with_platform_key(
        path: impl AsRef<Path>,
        audit_path: impl AsRef<Path>,
        platform: L,
    ) -> Result<Self, Error> {
        Self::create_inner(path.as_ref(), audit_path.as_ref(), platform, None)
    }

    fn create_inner(
        path: &Path,
        audit_path: &Path,
        mut platform: L,
        passphrase: Option<&Secret>,
    ) -> Result<Self, Error> {
        if path == audit_path {
            return Err(Error::InvalidEntry);
        }
        let mut salt = [0u8; 16];
        OsRng.try_fill_bytes(&mut salt).map_err(|_| Error::Crypto)?;
        let (version, key) = match passphrase {
            Some(passphrase) => (VERSION_PASSPHRASE, derive_key(passphrase, &salt)?),
            None => {
                let wrapping = platform.wrapping_key(true)?.ok_or(Error::Unavailable)?;
                (VERSION_KEYRING, hkdf_key(&wrapping, &salt)?)
            }
        };
        let mut vault = Self {
            path: path.to_owned(),
            audit_path: audit_path.to_owned(),
            platform,
            default_policy: AccessPolicy::WhileUnlocked,
            state: Some(State {
                key,
                salt,
                version,
                entries: Vec::new(),
            }),
        };
        vault.platform.unlock()?;
        vault.persist(None, true)?;
        Ok(vault)
    }

    /// Open and authenticate an existing version-1 vault using the supplied passphrase.
    pub fn open(
        path: impl AsRef<Path>,
        audit_path: impl AsRef<Path>,
        passphrase: &Secret,
        platform: L,
    ) -> Result<Self, Error> {
        let mut vault = Self::closed(path.as_ref(), audit_path.as_ref(), platform)?;
        vault.unlock(passphrase)?;
        Ok(vault)
    }

    /// Open an existing version-2 vault with the platform backend's wrapping key.
    pub fn open_with_platform_key(
        path: impl AsRef<Path>,
        audit_path: impl AsRef<Path>,
        platform: L,
    ) -> Result<Self, Error> {
        let mut vault = Self::closed(path.as_ref(), audit_path.as_ref(), platform)?;
        vault.unlock_with_platform_key()?;
        Ok(vault)
    }

    fn closed(path: &Path, audit_path: &Path, platform: L) -> Result<Self, Error> {
        if path == audit_path {
            return Err(Error::InvalidEntry);
        }
        Ok(Self {
            path: path.to_owned(),
            audit_path: audit_path.to_owned(),
            platform,
            default_policy: AccessPolicy::WhileUnlocked,
            state: None,
        })
    }

    /// Authenticate a version-1 file and reload it; failure leaves the handle fully locked.
    pub fn unlock(&mut self, passphrase: &Secret) -> Result<(), Error> {
        self.load(Some(passphrase))
    }

    /// Reload a version-2 file with the platform wrapping key; failure leaves it locked.
    pub fn unlock_with_platform_key(&mut self) -> Result<(), Error> {
        self.load(None)
    }

    fn load(&mut self, passphrase: Option<&Secret>) -> Result<(), Error> {
        self.lock();
        let mut bytes = Vec::new();
        File::open(&self.path)
            .map_err(|_| Error::Io)?
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Io)?;
        if bytes.len() < HEADER + 16 || bytes.len() as u64 > MAX_FILE || &bytes[..8] != MAGIC {
            return Err(Error::Format);
        }
        let version = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let mut salt = [0; 16];
        salt.copy_from_slice(&bytes[12..28]);
        let key = match (version, passphrase) {
            (VERSION_PASSPHRASE, Some(passphrase)) => derive_key(passphrase, &salt)?,
            (VERSION_KEYRING, None) => {
                let wrapping = self
                    .platform
                    .wrapping_key(false)?
                    .ok_or(Error::Unavailable)?;
                hkdf_key(&wrapping, &salt)?
            }
            (VERSION_PASSPHRASE | VERSION_KEYRING, _) => return Err(Error::Authentication),
            _ => return Err(Error::Format),
        };
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| Error::Crypto)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&bytes[28..52]),
                    Payload {
                        msg: &bytes[HEADER..],
                        aad: &bytes[..HEADER],
                    },
                )
                .map_err(|_| Error::Authentication)?,
        );
        let wire: Vec<ReadEntry> = serde_json::from_slice(&plaintext).map_err(|_| Error::Format)?;
        let mut entries = Vec::with_capacity(wire.len());
        for item in wire {
            let origins = item
                .origins
                .iter()
                .map(|origin| AllowedOrigin::parse(origin))
                .collect::<Result<Vec<_>, _>>()?;
            if entries.iter().any(|entry: &Entry| entry.site == item.site) {
                return Err(Error::Format);
            }
            let passkeys = item
                .passkeys
                .into_iter()
                .map(|passkey| {
                    PasskeyCredential::new(
                        passkey.credential_id,
                        passkey.rp_id,
                        passkey.user_handle,
                        passkey.private_key.0,
                        passkey.sign_count,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if [
                item.provider_key.is_some(),
                item.channel_secret.is_some(),
                item.subscription.is_some(),
            ]
            .into_iter()
            .filter(|present| *present)
            .count()
                > 1
            {
                return Err(Error::Format);
            }
            let mut entry = if let Some(info) = item.provider_key {
                if !origins.is_empty() || !passkeys.is_empty() || item.totp_seed.is_some() {
                    return Err(Error::Format);
                }
                let mut entry = Entry::provider_key(
                    &info.provider,
                    &info.key_id,
                    &info.label,
                    item.password.0,
                    info.created_at_ms,
                )?;
                if info.reserve_percent > 99 {
                    return Err(Error::Format);
                }
                let metadata = entry.provider_key.as_mut().ok_or(Error::Format)?;
                metadata.enabled = info.enabled;
                metadata.reserve_percent = info.reserve_percent;
                entry
            } else if let Some(info) = item.channel_secret {
                if !origins.is_empty() || !passkeys.is_empty() || item.totp_seed.is_some() {
                    return Err(Error::Format);
                }
                let signing = (!item.username.0 .0.is_empty()).then_some(item.username.0);
                Entry::channel_secret(
                    &info.kind,
                    &info.account_id,
                    item.password.0,
                    signing,
                    info.created_at_ms,
                )?
            } else if let Some(info) = item.subscription {
                if !origins.is_empty() || !passkeys.is_empty() || item.totp_seed.is_some() {
                    return Err(Error::Format);
                }
                Entry::subscription(info, item.password.0)?
            } else {
                Entry::build(
                    item.site.clone(),
                    origins,
                    item.username.0,
                    item.password.0,
                    ReadEntry::totp(item.totp_seed, item.totp_params)?,
                    item.policy,
                    passkeys,
                )?
            };
            if entry.site != item.site {
                return Err(Error::Format);
            }
            entry.registration = item.registration;
            entries.push(entry);
        }
        if let Err(error) = self.platform.unlock() {
            self.platform.lock();
            return Err(error);
        }
        self.state = Some(State {
            key,
            salt,
            version,
            entries,
        });
        Ok(())
    }

    /// Zeroize decrypted entries/key and revoke the backend's unlock state.
    pub fn lock(&mut self) {
        self.state = None;
        self.platform.lock();
    }

    /// Whether the file is currently decrypted (independent of platform policy).
    pub fn is_unlocked(&self) -> bool {
        self.state.is_some()
    }

    /// Access the trusted platform hook, e.g. for an OS screen-lock notification.
    pub fn platform_mut(&mut self) -> &mut L {
        &mut self.platform
    }

    /// Policy for items without an override.
    pub fn default_policy(&self) -> AccessPolicy {
        self.default_policy
    }

    /// Set the policy for items without an override (host configuration; not persisted).
    pub fn set_default_policy(&mut self, policy: AccessPolicy) {
        self.default_policy = policy;
    }

    /// Insert or replace an entry, atomically saving before changing in-memory state.
    pub fn put(&mut self, entry: Entry) -> Result<(), Error> {
        self.persist(Some(&entry), false)?;
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        if let Some(existing) = state
            .entries
            .iter_mut()
            .find(|existing| existing.site == entry.site)
        {
            *existing = entry;
        } else {
            state.entries.push(entry);
        }
        Ok(())
    }

    /// Value-free metadata of every item; audited as a `get`, a locked vault as a denial.
    pub fn items(&self, ctx: &AuditContext<'_>) -> Result<Vec<ItemInfo>, Error> {
        let Some(state) = self.state.as_ref() else {
            self.audit(ctx, AuditAction::Get, None, None, "denied", Some("locked"))?;
            return Err(Error::Locked);
        };
        self.audit(ctx, AuditAction::Get, None, None, "allowed", None)?;
        Ok(state
            .entries
            .iter()
            .filter(|entry| {
                entry.provider_key.is_none()
                    && entry.channel_secret.is_none()
                    && entry.subscription.is_none()
            })
            .map(|entry| self.info(entry))
            .collect())
    }

    /// Change one item's policy override (`None` inherits the default). The decision is
    /// audited before anything changes, so an unwritable audit log can never loosen a
    /// policy; rejected requests are audited too. If the change is saved but its completion
    /// record cannot be written, [`Error::AuditIncomplete`] reports that it did take effect.
    pub fn set_policy(
        &mut self,
        ctx: &AuditContext<'_>,
        site: &str,
        policy: Option<AccessPolicy>,
    ) -> Result<ItemInfo, Error> {
        let requested = policy.map_or("default", AccessPolicy::wire);
        let index = match self.state.as_ref() {
            None => {
                self.audit(
                    ctx,
                    AuditAction::PolicyUpdate,
                    Some(site),
                    None,
                    "denied",
                    Some("locked"),
                )?;
                return Err(Error::Locked);
            }
            Some(state) => state.entries.iter().position(|entry| entry.site == site),
        };
        let Some(index) = index else {
            self.audit(
                ctx,
                AuditAction::PolicyUpdate,
                Some(site),
                None,
                "denied",
                Some("unknown_item"),
            )?;
            return Err(Error::UnknownItem);
        };
        self.audit(
            ctx,
            AuditAction::PolicyUpdate,
            Some(site),
            None,
            "allowed",
            Some(requested),
        )?;
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let previous = std::mem::replace(&mut state.entries[index].policy, policy);
        if let Err(error) = self.persist(None, false) {
            if let Some(state) = self.state.as_mut() {
                state.entries[index].policy = previous;
            }
            let _ = self.audit(
                ctx,
                AuditAction::PolicyUpdate,
                Some(site),
                None,
                "failed",
                Some(requested),
            );
            return Err(error);
        }
        self.audit(
            ctx,
            AuditAction::PolicyUpdate,
            Some(site),
            None,
            "updated",
            Some(requested),
        )
        .map_err(|_| Error::AuditIncomplete)?;
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        Ok(self.info(&state.entries[index]))
    }

    /// Which saved authenticator applies to `origin`. Value-free, so not audited (like
    /// [`Self::item_ids`]); the later fill is. Several accounts can share an origin, so two or
    /// more matching items are reported as [`TotpMatch::Ambiguous`] rather than picking one.
    pub fn totp_site_for(&self, origin: &str) -> Result<TotpMatch, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let mut matching = state.entries.iter().filter(|entry| {
            entry.totp.is_some() && entry.origins.iter().any(|rule| rule.matches(origin))
        });
        Ok(match (matching.next(), matching.next()) {
            (None, _) => TotpMatch::None,
            (Some(entry), None) => TotpMatch::One(entry.site.clone()),
            (Some(_), Some(_)) => TotpMatch::Ambiguous,
        })
    }

    /// Whether the item saved by the [`Self::register_totp`] or [`Self::save_login`] call with
    /// this `operation` id is in the vault. Value-free, so not audited. Any other item at the
    /// same origin, added by a person or by another call, does not count.
    pub fn registered_by(&self, operation: &str) -> Result<bool, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        Ok(state
            .entries
            .iter()
            .any(|entry| entry.registration.as_deref() == Some(operation)))
    }

    /// The saved sign-ins (items with a password) that accept `origin`, with a masked account
    /// name such as `j***@naver.com`. Not audited, like [`Self::totp_site_for`]: no value is
    /// released and the later fill is audited.
    pub fn logins_for(&self, origin: &str) -> Result<Vec<LoginAccount>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        Ok(state
            .entries
            .iter()
            .filter(|entry| {
                !entry.password.0.is_empty()
                    && entry.origins.iter().any(|rule| rule.matches(origin))
            })
            .map(|entry| LoginAccount {
                site: entry.site.clone(),
                username_hint: username_hint(&entry.username.0),
                effective_policy: entry.policy.unwrap_or(self.default_policy),
            })
            .collect())
    }

    /// Whether [`Self::save_login`] for `origin` would be refused by policy: the vault default
    /// or an item accepting the origin says `Never`, or `WhileUnlocked` with a locked platform.
    pub fn login_save_refusal(&self, origin: &str) -> Result<Option<Denial>, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        if state.entries.iter().any(|entry| {
            entry.origins.iter().any(|rule| rule.matches(origin))
                && entry.policy.unwrap_or(self.default_policy) == AccessPolicy::Never
        }) {
            return Ok(Some(Denial::PolicyNever));
        }
        Ok(match self.default_policy {
            AccessPolicy::Never => Some(Denial::PolicyNever),
            AccessPolicy::WhileUnlocked if !self.platform.is_unlocked() => {
                Some(Denial::PlatformLocked)
            }
            _ => None,
        })
    }

    /// Keep a sign-in a person typed into a `login` ticket, bound to exactly `origin`. An
    /// item bound to exactly that origin with the same username gets the new password (and
    /// nothing else changes); otherwise a new item `login-<host>` is created with the default
    /// policy, never joining an item with wildcard or extra origins. Refused like
    /// [`Self::login_save_refusal`]. Audited as `login_save` before any change and again once
    /// saved (`saved` or `updated`); [`Error::AuditIncomplete`] means it *was* saved. Callers
    /// install a redactor for both values first. `operation` is kept on the item in the same
    /// write, for [`Self::registered_by`].
    pub fn save_login(
        &mut self,
        ctx: &AuditContext<'_>,
        origin: &str,
        username: Secret,
        password: Secret,
        operation: &str,
    ) -> Result<Result<SavedLogin, Denial>, Error> {
        let action = AuditAction::LoginSave;
        if !valid_label(operation) || password.0.is_empty() {
            return Err(Error::InvalidEntry);
        }
        let rule = AllowedOrigin::parse(origin)?;
        let Some(state) = self.state.as_ref() else {
            self.audit(ctx, action, None, Some(origin), "denied", Some("locked"))?;
            return Err(Error::Locked);
        };
        let existing = state
            .entries
            .iter()
            .position(|entry| entry.origins == [rule.clone()] && entry.username.0 == username.0);
        let site = match existing {
            Some(index) => state.entries[index].site.clone(),
            None => new_site(&state.entries, "login", origin),
        };
        if let Some(denial) = self.login_save_refusal(origin)? {
            self.audit(
                ctx,
                action,
                Some(&site),
                Some(origin),
                "denied",
                Some(denial.wire()),
            )?;
            return Ok(Err(denial));
        }
        self.audit(ctx, action, Some(&site), Some(origin), "allowed", None)?;
        let saved = match existing {
            Some(index) => {
                let state = self.state.as_mut().ok_or(Error::Locked)?;
                let entry = &mut state.entries[index];
                let previous = (
                    std::mem::replace(&mut entry.password, password),
                    entry.registration.replace(operation.to_owned()),
                );
                let written = self.persist(None, false);
                if written.is_err() {
                    if let Some(state) = self.state.as_mut() {
                        let entry = &mut state.entries[index];
                        entry.password = previous.0;
                        entry.registration = previous.1;
                    }
                }
                written
            }
            None => Entry::build(
                site.clone(),
                vec![rule],
                username,
                password,
                None,
                None,
                Vec::new(),
            )
            .and_then(|mut entry| {
                entry.registration = Some(operation.to_owned());
                self.put(entry)
            }),
        };
        if let Err(error) = saved {
            let _ = self.audit(ctx, action, Some(&site), Some(origin), "failed", None);
            return Err(error);
        }
        let created = existing.is_none();
        self.audit(
            ctx,
            action,
            Some(&site),
            Some(origin),
            if created { "saved" } else { "updated" },
            None,
        )
        .map_err(|_| Error::AuditIncomplete)?;
        Ok(Ok(SavedLogin { site, created }))
    }

    /// Store an authenticator a person approved for `origin` as a new TOTP-only item (empty
    /// username and password, default policy) bound to exactly that origin, so the factor never
    /// inherits another item's wildcard or extra origins. Nothing is ever replaced: when an item
    /// accepting `origin` already holds a TOTP the request is refused with
    /// [`Denial::AlreadyRegistered`], which also makes a stale second ticket for the same origin
    /// a no-op. An item accepting `origin` whose policy is `Never` refuses it too. Audited as
    /// `otp_register` before any change and again once it is saved; [`Error::AuditIncomplete`]
    /// means the factor *was* saved. Callers install [`Totp::redactor`] before calling.
    /// `operation` (a `[a-z0-9:_-]{1,64}` id, not secret) is saved on the item in the same
    /// write, so [`Self::registered_by`] can later tell whether exactly this call took
    /// effect.
    pub fn register_totp(
        &mut self,
        ctx: &AuditContext<'_>,
        origin: &str,
        totp: Totp,
        operation: &str,
    ) -> Result<Result<String, Denial>, Error> {
        let action = AuditAction::OtpRegister;
        if !valid_label(operation) {
            return Err(Error::InvalidEntry);
        }
        let rule = AllowedOrigin::parse(origin)?;
        let Some(state) = self.state.as_ref() else {
            self.audit(ctx, action, None, Some(origin), "denied", Some("locked"))?;
            return Err(Error::Locked);
        };
        let matching: Vec<&Entry> = state
            .entries
            .iter()
            .filter(|entry| entry.origins.iter().any(|rule| rule.matches(origin)))
            .collect();
        let refusal = if let Some(entry) = matching.iter().find(|entry| entry.totp.is_some()) {
            Some((entry.site.clone(), Denial::AlreadyRegistered))
        } else {
            matching
                .iter()
                .find(|entry| entry.policy.unwrap_or(self.default_policy) == AccessPolicy::Never)
                .map(|entry| (entry.site.clone(), Denial::PolicyNever))
        };
        let site = new_site(&state.entries, "otp", origin);
        let refusal = refusal.or(match self.default_policy {
            AccessPolicy::Never => Some((site.clone(), Denial::PolicyNever)),
            AccessPolicy::WhileUnlocked if !self.platform.is_unlocked() => {
                Some((site.clone(), Denial::PlatformLocked))
            }
            _ => None,
        });
        if let Some((site, denial)) = refusal {
            self.audit(
                ctx,
                action,
                Some(&site),
                Some(origin),
                "denied",
                Some(denial.wire()),
            )?;
            return Ok(Err(denial));
        }
        self.audit(ctx, action, Some(&site), Some(origin), "allowed", None)?;
        let saved = Entry::build(
            site.clone(),
            vec![rule],
            Secret::new(String::new()),
            Secret::new(String::new()),
            Some(totp),
            None,
            Vec::new(),
        )
        .and_then(|mut entry| {
            entry.registration = Some(operation.to_owned());
            self.put(entry)
        });
        if let Err(error) = saved {
            let _ = self.audit(ctx, action, Some(&site), Some(origin), "failed", None);
            return Err(error);
        }
        self.audit(ctx, action, Some(&site), Some(origin), "registered", None)
            .map_err(|_| Error::AuditIncomplete)?;
        Ok(Ok(site))
    }

    /// Add an item a person entered (admin API or CLI). An existing id is refused with
    /// [`Error::ItemExists`] rather than replaced, so a typo cannot overwrite a login. The
    /// decision is audited (`item_add`) before saving; a brand-new id is not yet known to the
    /// vault, so that record names `unknown-site` and the `added` record names the item.
    pub fn add_item(&mut self, ctx: &AuditContext<'_>, entry: Entry) -> Result<ItemInfo, Error> {
        let action = AuditAction::ItemAdd;
        let site = entry.site.clone();
        let Some(state) = self.state.as_ref() else {
            self.audit(ctx, action, Some(&site), None, "denied", Some("locked"))?;
            return Err(Error::Locked);
        };
        if state.entries.iter().any(|existing| existing.site == site) {
            self.audit(
                ctx,
                action,
                Some(&site),
                None,
                "denied",
                Some("item_exists"),
            )?;
            return Err(Error::ItemExists);
        }
        self.audit(ctx, action, Some(&site), None, "allowed", None)?;
        if let Err(error) = self.put(entry) {
            let _ = self.audit(ctx, action, Some(&site), None, "failed", None);
            return Err(error);
        }
        self.audit(ctx, action, Some(&site), None, "added", None)
            .map_err(|_| Error::AuditIncomplete)?;
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.site == site)
            .ok_or(Error::UnknownItem)?;
        Ok(self.info(entry))
    }

    /// Delete an item on a person's request (admin API or CLI), saving before the in-memory
    /// copy is dropped. Audited as `item_delete`; once the item is gone the `deleted` record
    /// names `unknown-site`, so the preceding `allowed` record carries the id.
    pub fn delete_item(&mut self, ctx: &AuditContext<'_>, site: &str) -> Result<(), Error> {
        let action = AuditAction::ItemDelete;
        let index = match self.state.as_ref() {
            None => {
                self.audit(ctx, action, Some(site), None, "denied", Some("locked"))?;
                return Err(Error::Locked);
            }
            Some(state) => state.entries.iter().position(|entry| entry.site == site),
        };
        let Some(index) = index else {
            self.audit(
                ctx,
                action,
                Some(site),
                None,
                "denied",
                Some("unknown_item"),
            )?;
            return Err(Error::UnknownItem);
        };
        self.audit(ctx, action, Some(site), None, "allowed", None)?;
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let removed = state.entries.remove(index);
        if let Err(error) = self.persist(None, false) {
            if let Some(state) = self.state.as_mut() {
                state.entries.insert(index, removed);
            }
            let _ = self.audit(ctx, action, Some(site), None, "failed", None);
            return Err(error);
        }
        self.audit(ctx, action, Some(site), None, "deleted", None)
            .map_err(|_| Error::AuditIncomplete)
    }

    fn info(&self, entry: &Entry) -> ItemInfo {
        ItemInfo {
            site: entry.site.clone(),
            origins: entry.origins.iter().map(AllowedOrigin::wire).collect(),
            policy: entry.policy,
            effective_policy: entry.policy.unwrap_or(self.default_policy),
            has_totp: entry.totp.is_some(),
            passkey_rp_ids: entry
                .passkeys
                .iter()
                .map(|passkey| passkey.rp_id.clone())
                .collect(),
        }
    }

    /// Build a zeroizing redaction filter from all live secrets, including passkey keys.
    /// The caller owns the filter and must drop it when its output pipeline closes.
    pub fn redactor(&self) -> Result<Redactor, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        Ok(Redactor::new(
            state.entries.iter().flat_map(|entry| entry.secrets()),
        ))
    }

    /// Redaction filter for one item, e.g. after a passkey was added to it.
    pub fn redactor_for(&self, site: &str) -> Result<Redactor, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.site == site)
            .ok_or(Error::UnknownItem)?;
        Ok(Redactor::new(entry.secrets()))
    }

    /// Release secrets only to a trusted callback for an allowed exact origin.
    /// Every decision is audited; audit failure before release prevents the callback.
    pub fn vault_login(
        &mut self,
        ctx: &AuditContext<'_>,
        action: AuditAction,
        site: &str,
        origin: &str,
        fill: impl FnOnce(FillSecrets<'_>) -> Result<(), ()>,
    ) -> Result<LoginOutcome, Error> {
        let entry = match self.authorized_entry(ctx, action, site, origin)? {
            Ok(entry) => entry,
            Err(denial) => return Ok(LoginOutcome::Denied(denial)),
        };
        let result = fill(Self::fill_secrets(entry));
        self.complete(ctx, action, site, origin, result.is_ok())
    }

    /// Async counterpart of `vault_login` for trusted CDP fill operations.
    /// Authorization and audit happen before any borrowed secret is released.
    pub async fn vault_login_async<'a>(
        &'a mut self,
        ctx: &AuditContext<'_>,
        action: AuditAction,
        site: &str,
        origin: &str,
        fill: impl FnOnce(FillSecrets<'a>) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>,
    ) -> Result<LoginOutcome, Error> {
        let entry = match self.authorized_entry(ctx, action, site, origin)? {
            Ok(entry) => entry,
            Err(denial) => return Ok(LoginOutcome::Denied(denial)),
        };
        let result = fill(Self::fill_secrets(entry)).await;
        self.complete(ctx, action, site, origin, result.is_ok())
    }

    /// Lend zeroizing copies of the item's passkeys whose RP ID belongs to `origin` to a
    /// trusted callback that installs them into a virtual authenticator, then store the
    /// observed signature counters.
    pub async fn passkey_login<'f>(
        &mut self,
        ctx: &AuditContext<'_>,
        site: &str,
        origin: &str,
        use_keys: impl FnOnce(Vec<PasskeyRelease>) -> PasskeyUse<'f>,
    ) -> Result<LoginOutcome, Error> {
        let action = AuditAction::PasskeyLogin;
        let releases = match self.decide(site, origin) {
            Err(denial) => return self.deny(ctx, action, site, origin, denial),
            Ok(entry) => entry
                .passkeys
                .iter()
                .filter(|passkey| origin::rp_id_allowed(&passkey.rp_id, origin))
                .map(|passkey| PasskeyRelease {
                    credential_id: passkey.credential_id.clone(),
                    rp_id: passkey.rp_id.clone(),
                    user_handle: passkey.user_handle.clone(),
                    private_key: Zeroizing::new(passkey.private_key.0.to_string()),
                    sign_count: passkey.sign_count,
                })
                .collect::<Vec<_>>(),
        };
        if releases.is_empty() {
            return self.deny(ctx, action, site, origin, Denial::NoPasskey);
        }
        self.audit(ctx, action, Some(site), Some(origin), "allowed", None)?;
        let Ok(counts) = use_keys(releases).await else {
            self.audit(ctx, action, Some(site), Some(origin), "failed", None)?;
            return Ok(LoginOutcome::FillFailed);
        };
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let mut changed = false;
        if let Some(entry) = state.entries.iter_mut().find(|entry| entry.site == site) {
            for count in counts {
                if let Some(passkey) = entry
                    .passkeys
                    .iter_mut()
                    .find(|passkey| passkey.credential_id == count.credential_id)
                {
                    if count.sign_count > passkey.sign_count {
                        passkey.sign_count = count.sign_count;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.persist(None, false)?;
        }
        self.audit(ctx, action, Some(site), Some(origin), "completed", None)?;
        Ok(LoginOutcome::Filled)
    }

    /// Authorize a passkey registration for `origin`, run the trusted capture callback
    /// (which reads the new credential back from the virtual authenticator) and store
    /// the credentials in the item. Credentials for another RP are discarded.
    pub async fn passkey_register<'f>(
        &mut self,
        ctx: &AuditContext<'_>,
        site: &str,
        origin: &str,
        capture: impl FnOnce() -> PasskeyCapture<'f>,
    ) -> Result<LoginOutcome, Error> {
        let action = AuditAction::PasskeyRegister;
        if let Err(denial) = self.authorized_entry(ctx, action, site, origin)? {
            return Ok(LoginOutcome::Denied(denial));
        }
        let credentials = match capture().await {
            Ok(credentials) if !credentials.is_empty() => credentials,
            _ => {
                self.audit(ctx, action, Some(site), Some(origin), "failed", None)?;
                return Ok(LoginOutcome::FillFailed);
            }
        };
        if credentials
            .iter()
            .any(|credential| !origin::rp_id_allowed(&credential.rp_id, origin))
        {
            return self.deny(ctx, action, site, origin, Denial::RpMismatch);
        }
        let state = self.state.as_mut().ok_or(Error::Locked)?;
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| entry.site == site)
            .ok_or(Error::UnknownItem)?;
        for credential in credentials {
            match entry
                .passkeys
                .iter_mut()
                .find(|passkey| passkey.credential_id == credential.credential_id)
            {
                Some(existing) => *existing = credential,
                None => entry.passkeys.push(credential),
            }
        }
        self.persist(None, false)?;
        self.audit(ctx, action, Some(site), Some(origin), "completed", None)?;
        Ok(LoginOutcome::Filled)
    }

    fn decide(&self, site: &str, origin: &str) -> Result<&Entry, Denial> {
        let state = self.state.as_ref().ok_or(Denial::Locked)?;
        let entry = state
            .entries
            .iter()
            .find(|entry| entry.site == site)
            .ok_or(Denial::UnknownSite)?;
        match entry.policy.unwrap_or(self.default_policy) {
            AccessPolicy::Never => return Err(Denial::PolicyNever),
            AccessPolicy::WhileUnlocked if !self.platform.is_unlocked() => {
                return Err(Denial::PlatformLocked)
            }
            _ => {}
        }
        if !entry.origins.iter().any(|rule| rule.matches(origin)) {
            return Err(Denial::OriginMismatch);
        }
        Ok(entry)
    }

    fn authorized_entry(
        &self,
        ctx: &AuditContext<'_>,
        action: AuditAction,
        site: &str,
        origin: &str,
    ) -> Result<Result<&Entry, Denial>, Error> {
        let decision = self.decide(site, origin);
        self.audit(
            ctx,
            action,
            Some(site),
            Some(origin),
            if decision.is_ok() {
                "allowed"
            } else {
                "denied"
            },
            decision.as_ref().err().map(|denial| denial.wire()),
        )?;
        Ok(decision)
    }

    fn deny(
        &self,
        ctx: &AuditContext<'_>,
        action: AuditAction,
        site: &str,
        origin: &str,
        denial: Denial,
    ) -> Result<LoginOutcome, Error> {
        self.audit(
            ctx,
            action,
            Some(site),
            Some(origin),
            "denied",
            Some(denial.wire()),
        )?;
        Ok(LoginOutcome::Denied(denial))
    }

    fn complete(
        &self,
        ctx: &AuditContext<'_>,
        action: AuditAction,
        site: &str,
        origin: &str,
        ok: bool,
    ) -> Result<LoginOutcome, Error> {
        self.audit(
            ctx,
            action,
            Some(site),
            Some(origin),
            if ok { "filled" } else { "fill-failed" },
            None,
        )?;
        Ok(if ok {
            LoginOutcome::Filled
        } else {
            LoginOutcome::FillFailed
        })
    }

    fn fill_secrets(entry: &Entry) -> FillSecrets<'_> {
        FillSecrets {
            username: &entry.username.0,
            password: &entry.password.0,
            totp: entry.totp.as_ref(),
        }
    }

    fn persist(&self, replacement: Option<&Entry>, create: bool) -> Result<(), Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let entries = state
            .entries
            .iter()
            .filter(|entry| replacement.map_or(true, |new| entry.site != new.site))
            .chain(replacement);
        let wire: Vec<_> = entries.map(WriteEntry::from).collect();
        let mut plaintext = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *plaintext, &wire).map_err(|_| Error::Format)?;
        if plaintext.len() as u64 + HEADER as u64 + 16 > MAX_FILE {
            return Err(Error::Format);
        }
        let mut header = [0; HEADER];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&state.version.to_be_bytes());
        header[12..28].copy_from_slice(&state.salt);
        OsRng
            .try_fill_bytes(&mut header[28..])
            .map_err(|_| Error::Crypto)?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(state.key.as_ref()).map_err(|_| Error::Crypto)?;
        let encrypted = cipher
            .encrypt(
                XNonce::from_slice(&header[28..]),
                Payload {
                    msg: &plaintext,
                    aad: &header,
                },
            )
            .map_err(|_| Error::Crypto)?;
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Io)?;
        file.write_all(&header)
            .and_then(|()| file.write_all(&encrypted))
            .and_then(|()| file.as_file().sync_all())
            .map_err(|_| Error::Io)?;
        if create {
            file.persist_noclobber(&self.path).map_err(|err| {
                if err.error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::AlreadyExists
                } else {
                    Error::Io
                }
            })?;
        } else {
            file.persist(&self.path).map_err(|_| Error::Io)?;
        }
        Ok(())
    }

    fn audit(
        &self,
        ctx: &AuditContext<'_>,
        action: AuditAction,
        site: Option<&str>,
        origin: Option<&str>,
        decision: &'static str,
        detail: Option<&str>,
    ) -> Result<(), Error> {
        let known_site = |site: &str| {
            self.state
                .as_ref()
                .is_some_and(|state| state.entries.iter().any(|entry| entry.site == site))
        };
        let origin = origin.map_or_else(
            || "-".to_owned(),
            |origin| {
                AllowedOrigin::parse(origin)
                    .ok()
                    .filter(|rule| rule.matches(origin))
                    .map_or_else(|| "invalid-origin".into(), |rule| rule.wire())
            },
        );
        let record = AuditRecord {
            when: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| Error::Audit)?
                .as_secs(),
            who: if valid_label(ctx.who) {
                ctx.who
            } else {
                "invalid-caller"
            }
            .to_owned(),
            what: action.wire().to_owned(),
            site_id: match site {
                None => "*".to_owned(),
                Some(site) if known_site(site) && valid_label(site) => site.to_owned(),
                Some(_) => "unknown-site".to_owned(),
            },
            origin,
            decision: decision.to_owned(),
            session_id: audit::identifier(ctx.session_id).map(str::to_owned),
            tool_call_id: audit::identifier(ctx.tool_call_id).map(str::to_owned),
            detail: detail.map(str::to_owned),
        };
        let raw = Zeroizing::new(serde_json::to_string(&record).map_err(|_| Error::Audit)?);
        let mut line = if self.state.is_some() {
            self.redactor()?.redact(&raw)
        } else {
            raw.to_string()
        };
        // A secret can itself be JSON punctuation. Fail closed rather than append
        // a malformed record or bypass redaction to preserve the audit schema.
        serde_json::from_str::<serde_json::Value>(&line).map_err(|_| Error::Audit)?;
        line.push('\n');
        append_audit(&self.audit_path, &line)
    }
}

/// Append a value-free lifecycle record (`setup`, `unlock`, `lock`) for the vault whose audit
/// log is `audit_path`. Lifecycle steps run while no [`Vault`] is open (a failed unlock, a
/// setup that never created one), so this needs none; the record names no item (`*`) and no
/// origin (`-`), and `detail` is a fixed reason code, never a value.
pub fn audit_lifecycle(
    audit_path: impl AsRef<Path>,
    ctx: &AuditContext<'_>,
    action: AuditAction,
    decision: &'static str,
    detail: Option<&'static str>,
) -> Result<(), Error> {
    let record = AuditRecord {
        when: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Audit)?
            .as_secs(),
        who: if valid_label(ctx.who) {
            ctx.who
        } else {
            "invalid-caller"
        }
        .to_owned(),
        what: action.wire().to_owned(),
        site_id: "*".to_owned(),
        origin: "-".to_owned(),
        decision: decision.to_owned(),
        session_id: audit::identifier(ctx.session_id).map(str::to_owned),
        tool_call_id: audit::identifier(ctx.tool_call_id).map(str::to_owned),
        detail: detail.map(str::to_owned),
    };
    let mut line = serde_json::to_string(&record).map_err(|_| Error::Audit)?;
    line.push('\n');
    append_audit(audit_path.as_ref(), &line)
}

fn append_audit(path: &Path, line: &str) -> Result<(), Error> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| {
            file.write_all(line.as_bytes())
                .and_then(|()| file.sync_data())
        })
        .map_err(|_| Error::Audit)
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b":_-".contains(&b))
}
/// `<prefix>-<host>[-<port>]`, made unique among `entries`, for an item the vault creates
/// itself (`otp` for a TOTP-only item, `login` for a saved sign-in).
fn new_site(entries: &[Entry], prefix: &str, origin: &str) -> String {
    let host: String = origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    let base = format!("{prefix}-{}", host.trim_matches('-'));
    let taken = |site: &str| entries.iter().any(|entry| entry.site == site);
    (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|site| !taken(site))
        .unwrap_or(base)
}
fn derive_key(passphrase: &Secret, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, Error> {
    let params = Params::new(19456, 2, 1, Some(32)).map_err(|_| Error::Crypto)?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.0.as_bytes(), salt, key.as_mut())
        .map_err(|_| Error::Crypto)?;
    Ok(key)
}
// The wrapping key is uniformly random, so a single HKDF expand block suffices; no
// password-hardening is needed.
fn hkdf_key(wrapping: &WrappingKey, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, Error> {
    let mut extract = <Hmac<Sha256> as Mac>::new_from_slice(salt).map_err(|_| Error::Crypto)?;
    extract.update(wrapping.bytes());
    let mut prk = Zeroizing::new([0u8; 32]);
    prk.copy_from_slice(&extract.finalize().into_bytes());
    let mut expand =
        <Hmac<Sha256> as Mac>::new_from_slice(prk.as_ref()).map_err(|_| Error::Crypto)?;
    expand.update(b"openside-vault v2 file key");
    expand.update(&[1]);
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&expand.finalize().into_bytes());
    Ok(key)
}

#[derive(Serialize)]
struct WriteEntry<'a> {
    site: &'a str,
    origins: Vec<String>,
    username: &'a str,
    password: &'a str,
    totp_seed: Option<&'a str>,
    // Written only when not the defaults, so files of items without non-default parameters
    // stay readable by builds that predate this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    totp_params: Option<TotpParams>,
    policy: Option<AccessPolicy>,
    passkeys: Vec<WritePasskey<'a>>,
    // Local to this vault file (never exported for sync) and written only when set, so files
    // without registered authenticators stay readable by builds that predate this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    registration: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_key: Option<&'a ProviderKeyInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_secret: Option<&'a ChannelSecretInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscription: Option<&'a SubscriptionInfo>,
}
#[derive(Serialize)]
struct WritePasskey<'a> {
    credential_id: &'a str,
    rp_id: &'a str,
    user_handle: Option<&'a str>,
    private_key: &'a str,
    sign_count: u32,
}
impl<'a> From<&'a Entry> for WriteEntry<'a> {
    fn from(entry: &'a Entry) -> Self {
        Self {
            site: &entry.site,
            origins: entry.origins.iter().map(AllowedOrigin::wire).collect(),
            username: &entry.username.0,
            password: &entry.password.0,
            totp_seed: entry.totp.as_ref().map(|totp| totp.seed.0.as_str()),
            totp_params: entry
                .totp
                .as_ref()
                .map(Totp::params)
                .filter(|params| !params.is_default()),
            policy: entry.policy,
            passkeys: entry
                .passkeys
                .iter()
                .map(|passkey| WritePasskey {
                    credential_id: &passkey.credential_id,
                    rp_id: &passkey.rp_id,
                    user_handle: passkey.user_handle.as_deref(),
                    private_key: &passkey.private_key.0,
                    sign_count: passkey.sign_count,
                })
                .collect(),
            registration: entry.registration.as_deref(),
            provider_key: entry.provider_key.as_ref(),
            channel_secret: entry.channel_secret.as_ref(),
            subscription: entry.subscription.as_ref(),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadEntry {
    site: String,
    origins: Vec<String>,
    username: ReadSecret,
    password: ReadSecret,
    totp_seed: Option<ReadSecret>,
    /// Absent in items written before TOTP parameters existed: they get the defaults
    /// (6 digits, 30 seconds, SHA-1).
    #[serde(default)]
    totp_params: Option<TotpParams>,
    #[serde(default)]
    policy: Option<AccessPolicy>,
    #[serde(default)]
    passkeys: Vec<ReadPasskey>,
    #[serde(default)]
    registration: Option<String>,
    #[serde(default)]
    provider_key: Option<ProviderKeyInfo>,
    #[serde(default)]
    channel_secret: Option<ChannelSecretInfo>,
    #[serde(default)]
    subscription: Option<SubscriptionInfo>,
}
impl ReadEntry {
    fn totp(seed: Option<ReadSecret>, params: Option<TotpParams>) -> Result<Option<Totp>, Error> {
        match (seed, params) {
            (Some(seed), params) => Totp::new(seed.0, params.unwrap_or_default()).map(Some),
            (None, None) => Ok(None),
            (None, Some(_)) => Err(Error::Format),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadPasskey {
    credential_id: String,
    rp_id: String,
    user_handle: Option<String>,
    private_key: ReadSecret,
    sign_count: u32,
}
struct ReadSecret(Secret);
impl<'de> Deserialize<'de> for ReadSecret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = ReadSecret;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a secret string")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(ReadSecret(Secret::new(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(ReadSecret(Secret::new(value)))
            }
        }
        deserializer.deserialize_string(Visitor)
    }
}

#[cfg(test)]
mod tests;
