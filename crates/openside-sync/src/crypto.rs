//! Key derivation, opaque record keys, pairing codes and record sealing.

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{Error, Kind};

const BLOB_VERSION: u8 = 1;
const NONCE: usize = 24;
const PAIRING_PREFIX: &str = "osync1-";

/// Random 128-bit account identifier; it salts the key derivation and names the account
/// on the backend. Not secret, but it never appears with the passphrase.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AccountId([u8; 16]);

impl AccountId {
    /// Fresh random account.
    pub fn generate() -> Result<Self, Error> {
        let mut bytes = [0; 16];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| Error::Crypto)?;
        Ok(Self(bytes))
    }

    /// Parse the 32-character lowercase hex form.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let bytes = hex_decode(value).ok_or(Error::Format)?;
        Ok(Self(bytes.try_into().map_err(|_| Error::Format)?))
    }

    /// Lowercase hex form used in paths and URLs.
    pub fn hex(&self) -> String {
        hex(&self.0)
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

/// What a second device needs besides the passphrase: the account and a 32-bit key check
/// that rejects a mistyped passphrase before anything is merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingCode {
    /// Account to join.
    pub account: AccountId,
    /// First four bytes of the key-check MAC.
    pub check: [u8; 4],
}

impl PairingCode {
    /// `osync1-` followed by base32 (no padding) of account id and check.
    pub fn encode(&self) -> String {
        let mut bytes = [0u8; 20];
        bytes[..16].copy_from_slice(&self.account.0);
        bytes[16..].copy_from_slice(&self.check);
        format!(
            "{PAIRING_PREFIX}{}",
            BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
        )
    }

    /// Parse a code, ignoring case, surrounding whitespace and `-`/space grouping.
    pub fn parse(code: &str) -> Result<Self, Error> {
        let code = code.trim().to_ascii_lowercase();
        let body = code.strip_prefix(PAIRING_PREFIX).ok_or(Error::Format)?;
        let body: String = body
            .chars()
            .filter(|c| *c != '-' && *c != ' ')
            .collect::<String>()
            .to_ascii_uppercase();
        let bytes = BASE32_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| Error::Format)?;
        if bytes.len() != 20 {
            return Err(Error::Format);
        }
        let mut account = [0; 16];
        account.copy_from_slice(&bytes[..16]);
        let mut check = [0; 4];
        check.copy_from_slice(&bytes[16..]);
        Ok(Self {
            account: AccountId(account),
            check,
        })
    }
}

/// Keys derived from the sync passphrase. Held only in memory; never written to disk.
pub struct SyncKey {
    account: AccountId,
    seal: Zeroizing<[u8; 32]>,
    ids: Zeroizing<[u8; 32]>,
    check: [u8; 4],
}

impl fmt::Debug for SyncKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl SyncKey {
    /// Argon2id (default parameters: 19 MiB, 2 passes) over the passphrase with a salt
    /// bound to the account, then HMAC-SHA256 subkeys per purpose.
    pub fn derive(passphrase: &[u8], account: AccountId) -> Result<Self, Error> {
        if passphrase.is_empty() {
            return Err(Error::WrongPassphrase);
        }
        let mut salt_hash = Sha256::new();
        salt_hash.update(b"openside-sync v1 salt\0");
        salt_hash.update(account.0);
        let salt = salt_hash.finalize();
        let mut master = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
            .hash_password_into(passphrase, &salt[..16], master.as_mut())
            .map_err(|_| Error::Crypto)?;
        let sub = |label: &[u8]| -> Result<Zeroizing<[u8; 32]>, Error> {
            let mut out = Zeroizing::new([0u8; 32]);
            out.copy_from_slice(&mac(master.as_ref(), &[b"openside-sync v1 ", label])?);
            Ok(out)
        };
        let check_full = sub(b"check")?;
        let mut check = [0; 4];
        check.copy_from_slice(&check_full[..4]);
        Ok(Self {
            account,
            seal: sub(b"seal")?,
            ids: sub(b"ids")?,
            check,
        })
    }

    /// Account the key belongs to.
    pub fn account(&self) -> AccountId {
        self.account
    }

    /// Code a second device enters (with the same passphrase) to join this account.
    pub fn pairing_code(&self) -> PairingCode {
        PairingCode {
            account: self.account,
            check: self.check,
        }
    }

    /// Whether this key matches a pairing code's account and key check.
    pub fn matches(&self, code: &PairingCode) -> bool {
        self.account == code.account && self.check == code.check
    }

    /// Opaque backend key of a record: HMAC of kind and id, so the backend cannot
    /// learn which item or URL a blob belongs to.
    pub fn record_key(&self, kind: Kind, id: &str) -> String {
        mac(
            self.ids.as_ref(),
            &[b"key\0", kind.wire().as_bytes(), b"\0", id.as_bytes()],
        )
        .map(|m| hex(&m[..16]))
        .unwrap_or_default()
    }

    /// Keyed digest of a local body, used to notice local edits without storing bodies.
    pub fn digest(&self, kind: Kind, id: &str, body: &[u8]) -> String {
        mac(
            self.ids.as_ref(),
            &[
                b"digest\0",
                kind.wire().as_bytes(),
                b"\0",
                id.as_bytes(),
                b"\0",
                body,
            ],
        )
        .map(|m| hex(&m[..16]))
        .unwrap_or_default()
    }

    /// `version(1) || nonce(24) || ciphertext+tag` with a fresh random nonce. Associated
    /// data binds the blob to the format version, the account and the record key.
    pub fn seal(&self, record_key: &str, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        let cipher =
            XChaCha20Poly1305::new_from_slice(self.seal.as_ref()).map_err(|_| Error::Crypto)?;
        let mut nonce = [0u8; NONCE];
        OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| Error::Crypto)?;
        let aad = self.aad(record_key);
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Crypto)?;
        let mut blob = Vec::with_capacity(1 + NONCE + ciphertext.len());
        blob.push(BLOB_VERSION);
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ciphertext);
        Ok(blob)
    }

    /// Authenticate and decrypt; any modification, a blob moved to another record key or
    /// account, or a different passphrase fails with [`Error::Crypto`].
    pub fn open(&self, record_key: &str, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
        if blob.len() < 1 + NONCE + 16 || blob[0] != BLOB_VERSION {
            return Err(Error::Crypto);
        }
        let cipher =
            XChaCha20Poly1305::new_from_slice(self.seal.as_ref()).map_err(|_| Error::Crypto)?;
        let aad = self.aad(record_key);
        cipher
            .decrypt(
                XNonce::from_slice(&blob[1..1 + NONCE]),
                Payload {
                    msg: &blob[1 + NONCE..],
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| Error::Crypto)
    }

    fn aad(&self, record_key: &str) -> Vec<u8> {
        let mut aad = Vec::with_capacity(64);
        aad.extend_from_slice(b"openside-sync v1\0");
        aad.push(BLOB_VERSION);
        aad.extend_from_slice(&self.account.0);
        aad.extend_from_slice(record_key.as_bytes());
        aad
    }
}

fn mac(key: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], Error> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| Error::Crypto)?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0
        || !value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).ok())
        .collect()
}

/// Short random device id.
pub(crate) fn random_device_id() -> Result<String, Error> {
    let mut bytes = [0u8; 8];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| Error::Crypto)?;
    Ok(hex(&bytes))
}
