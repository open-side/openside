//! Platform unlock backends.
//!
//! A passphrase backend only tracks whether the passphrase-encrypted file (format 1) was
//! opened. Keyring backends hold a random 32-byte wrapping key in an OS secret store; the
//! vault file (format 2) key is derived from it with HKDF-SHA256 and the per-file salt, so
//! neither the wrapping key nor the file key is ever written to disk by this crate.

use std::fmt;

use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use crate::Error;

#[cfg(target_os = "linux")]
pub(crate) mod secret_service;

#[cfg(any(target_os = "macos", windows))]
pub(crate) mod native;

/// Whether this operating system's keyring can hold a wrapping key, without touching any
/// item or showing a prompt. Keychain and Credential Manager are part of the OS; a Secret
/// Service must be running or D-Bus-activatable on the session bus. Other systems have none.
#[allow(clippy::needless_return)]
pub fn keyring_reachable() -> bool {
    #[cfg(target_os = "linux")]
    return secret_service::SecretServiceLock::session("probe").reachable();
    #[cfg(any(target_os = "macos", windows))]
    return true;
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    return false;
}

/// Platform unlock hook and optional source of a keyring-held wrapping key.
///
/// `unlock` is called only after the vault file authenticated. Backends must fail closed
/// and must not log key material.
pub trait PlatformLock {
    /// Notify the backend of successful authentication, or refuse unlocking.
    fn unlock(&mut self) -> Result<(), Error>;
    /// Revoke the platform unlock state.
    fn lock(&mut self);
    /// Whether `WhileUnlocked` entries may be used. Keyring backends consult the live store
    /// on every call and fail closed, so locking the store revokes use without a vault lock.
    fn is_unlocked(&self) -> bool;
    /// The keyring-held wrapping key for a format-2 vault. With `create`, a missing key is
    /// generated and stored. Passphrase-only backends hold no key and return `Ok(None)`.
    fn wrapping_key(&mut self, create: bool) -> Result<Option<WrappingKey>, Error> {
        let _ = create;
        Ok(None)
    }
}

impl PlatformLock for Box<dyn PlatformLock + Send> {
    fn unlock(&mut self) -> Result<(), Error> {
        (**self).unlock()
    }
    fn lock(&mut self) {
        (**self).lock();
    }
    fn is_unlocked(&self) -> bool {
        (**self).is_unlocked()
    }
    fn wrapping_key(&mut self, create: bool) -> Result<Option<WrappingKey>, Error> {
        (**self).wrapping_key(create)
    }
}

/// Passphrase-only backend: tracks successful vault authentication in memory.
#[derive(Debug, Default)]
pub struct PassphraseLock {
    unlocked: bool,
}
impl PlatformLock for PassphraseLock {
    fn unlock(&mut self) -> Result<(), Error> {
        self.unlocked = true;
        Ok(())
    }
    fn lock(&mut self) {
        self.unlocked = false;
    }
    fn is_unlocked(&self) -> bool {
        self.unlocked
    }
}

/// A zeroizing 256-bit key held by a platform secret store.
pub struct WrappingKey(Zeroizing<[u8; 32]>);

impl fmt::Debug for WrappingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl WrappingKey {
    /// Generate a fresh key from the operating-system random source.
    pub fn generate() -> Result<Self, Error> {
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng
            .try_fill_bytes(key.as_mut())
            .map_err(|_| Error::Crypto)?;
        Ok(Self(key))
    }

    /// Accept exactly 32 bytes read back from a secret store.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut key = Zeroizing::new([0u8; 32]);
        if bytes.len() != key.len() {
            return Err(Error::Unavailable);
        }
        key.copy_from_slice(bytes);
        Ok(Self(key))
    }

    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
