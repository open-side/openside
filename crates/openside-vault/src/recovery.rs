//! Recovery key: a second way to open the vault when its passphrase is forgotten or its
//! keyring key is lost.
//!
//! The key is 32 random bytes shown to the person once, as 52 base32 characters in groups of
//! four. Next to the vault file, `<vault>.recovery` (0600) holds the vault's file key sealed
//! with it: XChaCha20-Poly1305 under HKDF-SHA256(recovery key, random salt, info
//! `openside-vault recovery v1`). The whole 76-byte header is associated data and names the
//! vault version and salt the sealed key belongs to, so a recovery file left over from before
//! the vault was re-keyed is refused instead of trying a wrong key. Nobody can open the vault
//! from this file without the recovery key, which is never stored.
//!
//! Recovering opens the vault with the sealed key, gives it a new unlock key (a new
//! passphrase for a passphrase vault; the keyring's current wrapping key, created when
//! missing, for a keyring vault) and seals the new file key with the same recovery key, so
//! the person's saved key keeps working.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{
    derive_key, hkdf_key, AuditAction, AuditContext, Error, PlatformLock, Secret, Vault, HEADER,
    MAGIC, VERSION_KEYRING, VERSION_PASSPHRASE,
};

const RECOVERY_MAGIC: &[u8; 8] = b"OSVRECv1";
/// magic 8 | vault version 4 | vault salt 16 | salt 16 | nonce 24 | created (unix s) 8.
const RECOVERY_HEADER: usize = 76;
const RECOVERY_FILE: usize = RECOVERY_HEADER + 32 + 16;
const INFO: &[u8] = b"openside-vault recovery v1";

/// A vault recovery key. Debug output never shows it.
pub struct RecoveryKey(Zeroizing<[u8; 32]>);

impl std::fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl RecoveryKey {
    /// A new random key.
    pub fn generate() -> Result<Self, Error> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        OsRng
            .try_fill_bytes(bytes.as_mut())
            .map_err(|_| Error::Crypto)?;
        Ok(Self(bytes))
    }

    /// Read a key as [`Self::text`] writes it; case, spaces and dashes are ignored. Anything
    /// else is [`Error::Authentication`], without saying what was wrong.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let compact = Zeroizing::new(
            text.chars()
                .filter(|ch| !ch.is_whitespace() && *ch != '-')
                .map(|ch| ch.to_ascii_uppercase())
                .collect::<String>(),
        );
        let decoded = Zeroizing::new(
            data_encoding::BASE32_NOPAD
                .decode(compact.as_bytes())
                .map_err(|_| Error::Authentication)?,
        );
        let bytes: [u8; 32] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| Error::Authentication)?;
        Ok(Self(Zeroizing::new(bytes)))
    }

    /// The key as the person saves it: 52 base32 characters in dash-separated groups of four.
    /// Only for showing it once; drop it right after.
    pub fn text(&self) -> Zeroizing<String> {
        let encoded = Zeroizing::new(data_encoding::BASE32_NOPAD.encode(self.0.as_ref()));
        let mut grouped = Zeroizing::new(String::with_capacity(encoded.len() + encoded.len() / 4));
        for (index, ch) in encoded.chars().enumerate() {
            if index > 0 && index % 4 == 0 {
                grouped.push('-');
            }
            grouped.push(ch);
        }
        grouped
    }

    /// Output filter for the key as shown and as typed without dashes.
    pub fn redactor(&self) -> crate::Redactor {
        let grouped = Secret::new(self.text().to_string());
        let compact = Secret::new(data_encoding::BASE32_NOPAD.encode(self.0.as_ref()));
        crate::Redactor::new([&grouped, &compact])
    }

    fn kek(&self, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>, Error> {
        let mut extract = <Hmac<Sha256> as Mac>::new_from_slice(salt).map_err(|_| Error::Crypto)?;
        extract.update(self.0.as_ref());
        let mut prk = Zeroizing::new([0u8; 32]);
        prk.copy_from_slice(&extract.finalize().into_bytes());
        let mut expand =
            <Hmac<Sha256> as Mac>::new_from_slice(prk.as_ref()).map_err(|_| Error::Crypto)?;
        expand.update(INFO);
        expand.update(&[1]);
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&expand.finalize().into_bytes());
        Ok(key)
    }
}

/// Metadata of a recovery file, readable without any key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryInfo {
    /// When the key was generated (unix seconds).
    pub created_at: u64,
    /// Whether it seals the vault file's current key. A stale file (the vault was re-keyed
    /// since, e.g. restored from elsewhere) cannot recover it.
    pub current: bool,
}

/// `<vault file>.recovery`.
pub fn recovery_path(vault_path: impl AsRef<Path>) -> PathBuf {
    suffixed(vault_path.as_ref(), ".recovery")
}

/// `<vault file>.recovery.next`: the resealed recovery file written before a recovery
/// re-keys the vault and renamed over `.recovery` after. Whichever of the two matches the
/// vault header is the live one, so a crash between the steps never strands the key.
fn next_path(vault_path: &Path) -> PathBuf {
    suffixed(vault_path, ".recovery.next")
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

fn read_bounded(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, Error> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Io),
    };
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    Ok(Some(bytes))
}

/// The vault file's version and salt, from its header.
fn vault_header(vault_path: &Path) -> Result<(u32, [u8; 16]), Error> {
    let bytes = read_bounded(vault_path, HEADER)?.ok_or(Error::Io)?;
    if bytes.len() < HEADER || &bytes[..8] != MAGIC {
        return Err(Error::Format);
    }
    let version = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes[12..28]);
    Ok((version, salt))
}

fn parse_recovery(bytes: &[u8]) -> Result<(u32, [u8; 16], u64), Error> {
    if bytes.len() != RECOVERY_FILE || &bytes[..8] != RECOVERY_MAGIC {
        return Err(Error::Format);
    }
    let version = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes[12..28]);
    let mut created = [0u8; 8];
    created.copy_from_slice(&bytes[68..76]);
    Ok((version, salt, u64::from_be_bytes(created)))
}

/// The recovery file that seals the vault's current key: `.recovery`, or `.recovery.next`
/// left by an interrupted recovery. `Err(Some(stale))` when only a stale file exists.
fn live_recovery(vault_path: &Path) -> Result<Result<Vec<u8>, Option<Vec<u8>>>, Error> {
    let header = vault_header(vault_path).ok();
    let mut stale = None;
    for candidate in [recovery_path(vault_path), next_path(vault_path)] {
        let Some(bytes) = read_bounded(&candidate, RECOVERY_FILE)? else {
            continue;
        };
        let Ok((version, salt, _)) = parse_recovery(&bytes) else {
            continue;
        };
        if header == Some((version, salt)) {
            return Ok(Ok(bytes));
        }
        stale.get_or_insert(bytes);
    }
    Ok(Err(stale))
}

/// Whether a recovery key exists for the vault at `vault_path`, and whether it still applies.
pub fn recovery_info(vault_path: impl AsRef<Path>) -> Result<Option<RecoveryInfo>, Error> {
    let (bytes, current) = match live_recovery(vault_path.as_ref())? {
        Ok(bytes) => (bytes, true),
        Err(Some(bytes)) => (bytes, false),
        Err(None) => return Ok(None),
    };
    let (_, _, created_at) = parse_recovery(&bytes)?;
    Ok(Some(RecoveryInfo {
        created_at,
        current,
    }))
}

fn seal(
    key: &RecoveryKey,
    version: u32,
    vault_salt: &[u8; 16],
    file_key: &[u8; 32],
    created_at: u64,
) -> Result<Vec<u8>, Error> {
    let mut header = [0u8; RECOVERY_HEADER];
    header[..8].copy_from_slice(RECOVERY_MAGIC);
    header[8..12].copy_from_slice(&version.to_be_bytes());
    header[12..28].copy_from_slice(vault_salt);
    OsRng
        .try_fill_bytes(&mut header[28..68])
        .map_err(|_| Error::Crypto)?;
    header[68..76].copy_from_slice(&created_at.to_be_bytes());
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&header[28..44]);
    let kek = key.kek(&salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(kek.as_ref()).map_err(|_| Error::Crypto)?;
    let sealed = cipher
        .encrypt(
            XNonce::from_slice(&header[44..68]),
            Payload {
                msg: file_key,
                aad: &header,
            },
        )
        .map_err(|_| Error::Crypto)?;
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&sealed);
    Ok(bytes)
}

/// A 0600 temporary file next to `path` holding `bytes`, ready to replace it in one rename.
fn stage_private(path: &Path, bytes: &[u8]) -> Result<tempfile::NamedTempFile, Error> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Io)?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| Error::Io)?;
    Ok(file)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    stage_private(path, bytes)?
        .persist(path)
        .map_err(|_| Error::Io)?;
    sync_parent(path);
    Ok(())
}

/// Make a rename in `path`'s directory durable. Best effort: not every platform can open a
/// directory for syncing.
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl<L: PlatformLock> Vault<L> {
    /// Seal the open vault's file key with a new recovery key, replacing any earlier
    /// `<vault>.recovery` (the old key stops working), and return the key to show once.
    /// Audited (`recovery_key`: `allowed`, then `generated`). Both records are written before
    /// the file replaces the old one, so an unwritable log ([`Error::Audit`]) leaves the
    /// previous key in place and no key exists that the person was never shown.
    pub fn generate_recovery_key(&self, ctx: &AuditContext<'_>) -> Result<RecoveryKey, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        let key = RecoveryKey::generate()?;
        let sealed = seal(&key, state.version, &state.salt, &state.key, now())?;
        let path = recovery_path(&self.path);
        self.audit(ctx, AuditAction::RecoveryKey, None, None, "allowed", None)?;
        let staged = match stage_private(&path, &sealed) {
            Ok(staged) => staged,
            Err(error) => {
                let _ = self.audit(ctx, AuditAction::RecoveryKey, None, None, "failed", None);
                return Err(error);
            }
        };
        self.audit(ctx, AuditAction::RecoveryKey, None, None, "generated", None)?;
        if staged.persist(&path).is_err() {
            let _ = self.audit(ctx, AuditAction::RecoveryKey, None, None, "failed", None);
            return Err(Error::Io);
        }
        // A `.recovery.next` left by an interrupted recovery seals the old key; it must not
        // keep that key working.
        let _ = std::fs::remove_file(next_path(&self.path));
        sync_parent(&path);
        Ok(key)
    }

    /// Whether `passphrase` is the open passphrase vault's passphrase (constant-time key
    /// comparison). A keyring vault has no passphrase: always false.
    pub fn verify_passphrase(&self, passphrase: &Secret) -> Result<bool, Error> {
        let state = self.state.as_ref().ok_or(Error::Locked)?;
        if state.version != VERSION_PASSPHRASE {
            return Ok(false);
        }
        let derived = derive_key(passphrase, &state.salt)?;
        Ok(derived
            .iter()
            .zip(state.key.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0)
    }

    /// Open the vault at `path` with its recovery key and give it a new unlock key:
    /// `new_passphrase` for a passphrase vault (required), the keyring's wrapping key for a
    /// keyring vault (must be `None`; a missing keyring key is created). The re-keyed file
    /// replaces the old one atomically, then the recovery file is resealed with the same
    /// `key`, so the saved key keeps working. A wrong or stale key, or a passphrase given to
    /// a keyring vault (and the reverse), is [`Error::Authentication`]; nothing is written.
    /// Lifecycle records are the caller's ([`crate::audit_lifecycle`] with `recover`).
    pub fn recover(
        path: impl AsRef<Path>,
        audit_path: impl AsRef<Path>,
        key: &RecoveryKey,
        new_passphrase: Option<&Secret>,
        platform: L,
    ) -> Result<Self, Error> {
        let path = path.as_ref();
        let bytes = live_recovery(path)?.map_err(|_| Error::Authentication)?;
        let (version, _, _) = parse_recovery(&bytes)?;
        match (version, new_passphrase) {
            (VERSION_PASSPHRASE, Some(_)) | (VERSION_KEYRING, None) => {}
            (VERSION_PASSPHRASE | VERSION_KEYRING, _) => return Err(Error::Authentication),
            _ => return Err(Error::Format),
        }
        let mut wrap_salt = [0u8; 16];
        wrap_salt.copy_from_slice(&bytes[28..44]);
        let kek = key.kek(&wrap_salt)?;
        let cipher = XChaCha20Poly1305::new_from_slice(kek.as_ref()).map_err(|_| Error::Crypto)?;
        let opened = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&bytes[44..68]),
                    Payload {
                        msg: &bytes[RECOVERY_HEADER..],
                        aad: &bytes[..RECOVERY_HEADER],
                    },
                )
                .map_err(|_| Error::Authentication)?,
        );
        let mut file_key = Zeroizing::new([0u8; 32]);
        if opened.len() != 32 {
            return Err(Error::Format);
        }
        file_key.copy_from_slice(&opened);
        let mut vault = Self::closed(path, audit_path.as_ref(), platform)?;
        vault.load(crate::KeySource::File(file_key))?;
        let mut new_salt = [0u8; 16];
        OsRng
            .try_fill_bytes(&mut new_salt)
            .map_err(|_| Error::Crypto)?;
        let new_key = match new_passphrase {
            Some(passphrase) => derive_key(passphrase, &new_salt)?,
            None => {
                let wrapping = vault
                    .platform
                    .wrapping_key(true)?
                    .ok_or(Error::Unavailable)?;
                hkdf_key(&wrapping, &new_salt)?
            }
        };
        let resealed = seal(key, version, &new_salt, &new_key, now())?;
        let next = next_path(path);
        write_private(&next, &resealed)?;
        let state = vault.state.as_mut().ok_or(Error::Locked)?;
        state.salt = new_salt;
        state.key = new_key;
        if let Err(error) = vault.persist(None, false) {
            // The old vault file and `.recovery` are untouched; the staged file matches
            // neither and is only noise.
            let _ = std::fs::remove_file(&next);
            return Err(error);
        }
        // If this rename is lost, `.recovery.next` still matches the vault and is used.
        let _ = std::fs::rename(&next, recovery_path(path));
        sync_parent(path);
        Ok(vault)
    }
}
