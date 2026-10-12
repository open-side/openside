//! macOS Keychain / Windows Credential Manager backend through the `keyring` crate.
//! Compiled only on those targets; not built or tested in this repository's CI (Linux).

use std::sync::atomic::{AtomicBool, Ordering};

use super::{PlatformLock, WrappingKey};
use crate::Error;

/// Wrapping key stored as a generic password/credential (service `openside-vault`,
/// account = vault id) in the operating system's native credential store.
pub struct NativeKeyringLock {
    vault_id: String,
    unlocked: AtomicBool,
}

impl std::fmt::Debug for NativeKeyringLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeKeyringLock")
    }
}

impl NativeKeyringLock {
    /// Use the current user's native credential store.
    pub fn new(vault_id: impl Into<String>) -> Self {
        Self {
            vault_id: vault_id.into(),
            unlocked: AtomicBool::new(false),
        }
    }

    /// The native stores expose no lock-state query, so readability of the wrapping key is
    /// the probe: a locked or vanished store fails the read and counts as locked.
    fn key_readable(&self) -> bool {
        keyring::Entry::new("openside-vault", &self.vault_id)
            .and_then(|entry| entry.get_secret())
            .map(zeroize::Zeroizing::new)
            .is_ok()
    }
}

impl PlatformLock for NativeKeyringLock {
    fn unlock(&mut self) -> Result<(), Error> {
        self.unlocked.store(true, Ordering::Release);
        Ok(())
    }
    fn lock(&mut self) {
        self.unlocked.store(false, Ordering::Release);
    }
    fn is_unlocked(&self) -> bool {
        if !self.unlocked.load(Ordering::Acquire) {
            return false;
        }
        if self.key_readable() {
            return true;
        }
        self.unlocked.store(false, Ordering::Release);
        false
    }
    fn wrapping_key(&mut self, create: bool) -> Result<Option<WrappingKey>, Error> {
        let entry = keyring::Entry::new("openside-vault", &self.vault_id)
            .map_err(|_| Error::Unavailable)?;
        match entry.get_secret() {
            Ok(bytes) => {
                let bytes = zeroize::Zeroizing::new(bytes);
                WrappingKey::from_bytes(&bytes).map(Some)
            }
            Err(keyring::Error::NoEntry) if create => {
                let key = WrappingKey::generate()?;
                entry
                    .set_secret(key.bytes())
                    .map_err(|_| Error::Unavailable)?;
                Ok(Some(key))
            }
            Err(_) => Err(Error::Unavailable),
        }
    }
}
