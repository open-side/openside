//! Record identity, ordering and the sealed payload format.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// What a record holds. The kind is only visible inside the sealed payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A pinned tab (`url`, `title`); id is derived from the URL.
    PinnedTab,
    /// A vault item in its exported form; id is the vault site label.
    VaultItem,
    /// A settings document; id names it (the live policy is `policy`).
    Setting,
    /// A memory from the user-selected subset; id is the id it got on its first device.
    Memory,
}

impl Kind {
    /// Every kind, in a fixed order.
    pub const ALL: [Self; 4] = [
        Self::PinnedTab,
        Self::VaultItem,
        Self::Setting,
        Self::Memory,
    ];

    /// Stable wire label, also mixed into the opaque record key.
    pub fn wire(self) -> &'static str {
        match self {
            Self::PinnedTab => "pinned_tab",
            Self::VaultItem => "vault_item",
            Self::Setting => "setting",
            Self::Memory => "memory",
        }
    }
}

/// Version of one record. Derived ordering is the conflict rule: a higher Lamport
/// counter wins; on a tie a tombstone (`deleted = true`) wins over an edit; a remaining
/// tie is broken by the lexicographically greater device id. The order is total, so the
/// maximum is the same on every device whatever order records arrive in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Stamp {
    /// Lamport counter: greater than every stamp the writing device had seen.
    pub lamport: u64,
    /// Deletion marker; tombstones are kept so a deletion propagates.
    pub deleted: bool,
    /// Writing device.
    pub device: String,
}

/// Plaintext sealed into a blob. `body` is base64 of the kind-specific JSON, absent for
/// tombstones.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Payload {
    pub(crate) v: u8,
    pub(crate) kind: Kind,
    pub(crate) id: String,
    pub(crate) stamp: Stamp,
    pub(crate) body: Option<String>,
}

impl Drop for Payload {
    fn drop(&mut self) {
        if let Some(body) = self.body.as_mut() {
            body.zeroize();
        }
    }
}

pub(crate) const PAYLOAD_VERSION: u8 = 1;
