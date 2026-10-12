//! End-to-end encrypted sync of settings, pinned tabs, vault items and selected memories
//! between devices (B-9), with no openside server involved.
//!
//! Every record is sealed on the device with XChaCha20-Poly1305 under a key derived from
//! the user's sync passphrase (Argon2id, salted with the random account id). A backend
//! (a WebDAV share via [`WebDavBackend`] or a shared [`DirBackend`] folder) only stores
//! opaque record keys and sealed blobs; it never sees kinds, ids, URLs, titles or
//! secrets. Merging happens on the devices with a last-writer-wins register per record
//! ordered by [`Stamp`] (Lamport counter, tombstone flag, device id), which is a total
//! order, so any two devices that exchange their records converge to the same state.
//! The rules and the threat model are documented in `docs/specs/sync.md`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backend;
mod crypto;
mod engine;
pub mod fake_webdav;
mod record;

use std::fmt;

pub use backend::{Backend, DirBackend, Listed, Pushed, WebDavBackend};
pub use crypto::{AccountId, PairingCode, SyncKey};
pub use engine::{sync, Applied, Checkpoint, ClientState, LocalStore, Meta, Report};
pub use record::{Kind, Stamp};

/// Value-free sync errors; messages never contain record contents or credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// Key derivation, random source or encryption failed.
    Crypto,
    /// The passphrase does not match the account's pairing check.
    WrongPassphrase,
    /// A pairing code, account id or stored state is malformed.
    Format,
    /// The backend rejected the credentials.
    Unauthorized,
    /// The backend could not be reached or answered unexpectedly.
    Backend(String),
    /// Local filesystem operation failed.
    Io,
    /// The local store could not read or apply a record.
    Store(String),
    /// Other devices kept writing the same records; retry later.
    Contention,
    /// The Lamport counter reached its ceiling; local edits cannot be stamped any more and
    /// the account must be replaced (new setup, re-pair).
    Exhausted,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crypto => f.write_str("sync cryptography failed"),
            Self::WrongPassphrase => f.write_str("sync passphrase does not match this account"),
            Self::Format => f.write_str("malformed sync data"),
            Self::Unauthorized => f.write_str("sync backend rejected the credentials"),
            Self::Backend(reason) => write!(f, "sync backend unavailable: {reason}"),
            Self::Io => f.write_str("sync state could not be read or written"),
            Self::Store(reason) => write!(f, "local store failed: {reason}"),
            Self::Contention => f.write_str("sync did not settle; retry"),
            Self::Exhausted => f.write_str("sync counter exhausted; set up a new account"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests;
