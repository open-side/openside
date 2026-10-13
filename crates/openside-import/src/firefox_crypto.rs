use std::collections::BTreeMap;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::{Digest, Sha1};
use zeroize::Zeroizing;

use crate::db::Item;
use crate::error::Result;

const PBES2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 5, 13];
const PBKDF2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 5, 12];
const LEGACY_PBE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 12, 5, 1, 3];
const AES256: &[u8] = &[0x60, 0x86, 0x48, 1, 0x65, 3, 4, 1, 42];
const DES3: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 3, 7];
const HMAC_SHA1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 2, 7];
const HMAC_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 2, 9];
const PRIMARY_PASSWORD: &str =
    "Firefox needs a primary password; passwords were skipped. Other data can still be imported.";
const MALFORMED: &str = "Firefox password encryption data is damaged or unsupported.";

pub(crate) struct LoginKeys(BTreeMap<Vec<u8>, Zeroizing<Vec<u8>>>);

impl LoginKeys {
    pub(crate) fn load(item: &Item) -> Result<Self> {
        let db = item.open()?;
        db.require(item, "metadata", &["id", "item1", "item2"])?;
        db.require(item, "nssPrivate", &["a11", "a102"])?;
        let (salt, check) = db
            .conn
            .query_row(
                "SELECT item1,item2 FROM metadata WHERE id='password'",
                [],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .map_err(|error| item.sqlite(&error))?;
        // Empty-password validation is the only attempt. A protected profile is never
        // unlocked, prompted, modified, or retried with another password.
        let pass_key: Zeroizing<[u8; 20]> = Zeroizing::new(Sha1::digest(&salt).into());
        let verified =
            decrypt_pbe(&check, &pass_key[..]).map_err(|detail| item.unsupported(detail))?;
        if verified.as_ref().map(|plain| plain.as_slice()) != Some(b"password-check".as_slice()) {
            return Err(item.unsupported(PRIMARY_PASSWORD));
        }
        let mut statement = db
            .conn
            .prepare("SELECT a102,a11 FROM nssPrivate WHERE a102 IS NOT NULL AND a11 IS NOT NULL")
            .map_err(|error| item.sqlite(&error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(|error| item.sqlite(&error))?;
        let mut keys = BTreeMap::new();
        for row in rows {
            let (id, encrypted) = row.map_err(|error| item.sqlite(&error))?;
            let key = decrypt_pbe(&encrypted, &pass_key[..])
                .map_err(|detail| item.unsupported(detail))?
                .ok_or_else(|| item.unsupported(MALFORMED))?;
            if key.len() == 24 || key.len() == 32 {
                keys.insert(id, key);
            }
        }
        if keys.is_empty() {
            return Err(item.unsupported("Firefox has no supported saved-password encryption key."));
        }
        Ok(Self(keys))
    }

    pub(crate) fn decrypt(
        &self,
        encoded: &str,
    ) -> std::result::Result<Zeroizing<String>, &'static str> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| MALFORMED)?;
        let mut outer = sequence(&bytes)?;
        let id = outer.take(4)?.data;
        let mut algorithm = outer.take(0x30)?.reader();
        let oid = algorithm.take(6)?.data;
        let iv = algorithm.take(4)?.data;
        algorithm.finish()?;
        let ciphertext = outer.take(4)?.data;
        outer.finish()?;
        let key = self
            .0
            .get(id)
            .ok_or("Firefox saved-password key identifier is unsupported.")?;
        let plain = decrypt_cbc(oid, key, iv, ciphertext)?.ok_or(MALFORMED)?;
        let value = std::str::from_utf8(&plain).map_err(|_| MALFORMED)?;
        Ok(Zeroizing::new(value.to_owned()))
    }
}

#[derive(Clone, Copy)]
struct Node<'a> {
    tag: u8,
    data: &'a [u8],
    encoded: &'a [u8],
}

impl<'a> Node<'a> {
    fn reader(self) -> Der<'a> {
        Der(self.data)
    }
    fn integer(self) -> std::result::Result<u32, &'static str> {
        if self.tag != 2 || self.data.is_empty() || self.data.len() > 5 || self.data[0] & 0x80 != 0
        {
            return Err(MALFORMED);
        }
        self.data.iter().try_fold(0u32, |value, byte| {
            value
                .checked_mul(256)
                .and_then(|value| value.checked_add(u32::from(*byte)))
                .ok_or(MALFORMED)
        })
    }
}

struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    fn node(&mut self) -> std::result::Result<Node<'a>, &'static str> {
        let bytes = self.0;
        let tag = *bytes.first().ok_or(MALFORMED)?;
        let first = *bytes.get(1).ok_or(MALFORMED)?;
        let mut start = 2usize;
        let length = if first & 0x80 == 0 {
            usize::from(first)
        } else {
            let count = usize::from(first & 0x7f);
            if count == 0 || count > 4 {
                return Err(MALFORMED);
            }
            let encoded = bytes.get(2..2 + count).ok_or(MALFORMED)?;
            if encoded[0] == 0 {
                return Err(MALFORMED);
            }
            start += count;
            let length = encoded.iter().try_fold(0usize, |value, byte| {
                value
                    .checked_mul(256)
                    .and_then(|value| value.checked_add(usize::from(*byte)))
                    .ok_or(MALFORMED)
            })?;
            if length < 128 {
                return Err(MALFORMED);
            }
            length
        };
        let end = start.checked_add(length).ok_or(MALFORMED)?;
        let data = bytes.get(start..end).ok_or(MALFORMED)?;
        self.0 = bytes.get(end..).ok_or(MALFORMED)?;
        Ok(Node {
            tag,
            data,
            encoded: &bytes[..end],
        })
    }
    fn take(&mut self, tag: u8) -> std::result::Result<Node<'a>, &'static str> {
        let node = self.node()?;
        if node.tag != tag {
            return Err(MALFORMED);
        }
        Ok(node)
    }
    fn finish(&self) -> std::result::Result<(), &'static str> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(MALFORMED)
        }
    }
}

fn sequence(bytes: &[u8]) -> std::result::Result<Der<'_>, &'static str> {
    let mut input = Der(bytes);
    let sequence = input.take(0x30)?.reader();
    input.finish()?;
    Ok(sequence)
}

fn decrypt_cbc(
    oid: &[u8],
    key: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
) -> std::result::Result<Option<Zeroizing<Vec<u8>>>, &'static str> {
    let mut plain = Zeroizing::new(ciphertext.to_vec());
    let length = if oid == AES256 && key.len() == 32 && iv.len() == 16 {
        cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
            .map_err(|_| MALFORMED)?
            .decrypt_padded_mut::<Pkcs7>(&mut plain)
            .ok()
            .map(<[u8]>::len)
    } else if oid == DES3 && key.len() == 24 && iv.len() == 8 {
        cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv)
            .map_err(|_| MALFORMED)?
            .decrypt_padded_mut::<Pkcs7>(&mut plain)
            .ok()
            .map(<[u8]>::len)
    } else {
        return Err("Firefox saved-password cipher is unsupported.");
    };
    Ok(length.map(|length| {
        plain.truncate(length);
        plain
    }))
}

fn decrypt_pbe(
    bytes: &[u8],
    pass_key: &[u8],
) -> std::result::Result<Option<Zeroizing<Vec<u8>>>, &'static str> {
    let mut outer = sequence(bytes)?;
    let mut algorithm = outer.take(0x30)?.reader();
    let oid = algorithm.take(6)?.data;
    let mut params = algorithm.take(0x30)?.reader();
    algorithm.finish()?;
    let ciphertext = outer.take(4)?.data;
    outer.finish()?;
    if oid == LEGACY_PBE {
        let salt = params.take(4)?.data;
        let iterations = params.take(2)?.integer()?;
        params.finish()?;
        let (key, iv) = legacy_key(pass_key, salt, iterations)?;
        return decrypt_cbc(DES3, &key[..], &iv, ciphertext);
    }
    if oid != PBES2 {
        return Err("Firefox password key encryption is unsupported.");
    }
    let mut kdf = params.take(0x30)?.reader();
    if kdf.take(6)?.data != PBKDF2 {
        return Err(MALFORMED);
    }
    let mut derivation = kdf.take(0x30)?.reader();
    kdf.finish()?;
    let salt = derivation.take(4)?.data;
    let iterations = derivation.take(2)?.integer()?;
    if iterations == 0 || iterations > 1_000_000 {
        return Err(MALFORMED);
    }
    if derivation.0.first() == Some(&2) && derivation.take(2)?.integer()? != 32 {
        return Err(MALFORMED);
    }
    let prf = if derivation.0.is_empty() {
        HMAC_SHA1
    } else {
        let mut prf = derivation.take(0x30)?.reader();
        let oid = prf.take(6)?.data;
        if !prf.0.is_empty() && !prf.take(5)?.data.is_empty() {
            return Err(MALFORMED);
        }
        prf.finish()?;
        oid
    };
    derivation.finish()?;
    let mut cipher = params.take(0x30)?.reader();
    if cipher.take(6)?.data != AES256 {
        return Err(MALFORMED);
    }
    let iv = cipher.take(4)?;
    cipher.finish()?;
    params.finish()?;
    // NSS historically uses the complete DER OCTET STRING as its AES IV (04 0e
    // plus fourteen random bytes), not just the fourteen-byte string payload.
    let iv = if iv.data.len() == 14 {
        iv.encoded
    } else {
        iv.data
    };
    let mut key = Zeroizing::new([0u8; 32]);
    if prf == HMAC_SHA256 {
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pass_key, salt, iterations, &mut key[..]);
    } else if prf == HMAC_SHA1 {
        pbkdf2::pbkdf2_hmac::<Sha1>(pass_key, salt, iterations, &mut key[..]);
    } else {
        return Err("Firefox password key derivation is unsupported.");
    }
    decrypt_cbc(AES256, &key[..], iv, ciphertext)
}

fn legacy_key(
    pass_key: &[u8],
    salt: &[u8],
    iterations: u32,
) -> std::result::Result<(Zeroizing<[u8; 24]>, [u8; 8]), &'static str> {
    if iterations == 0 || iterations > 1_000_000 || salt.len() > 1024 {
        return Err(MALFORMED);
    }
    let mut hash = Sha1::new();
    hash.update(pass_key);
    hash.update(salt);
    let mut initial: Zeroizing<[u8; 20]> = Zeroizing::new(hash.finalize().into());
    for _ in 1..iterations {
        let next: Zeroizing<[u8; 20]> = Zeroizing::new(Sha1::digest(&initial[..]).into());
        initial.copy_from_slice(&next[..]);
    }
    let mut state = Zeroizing::new(vec![0u8; salt.len().max(20)]);
    state[..salt.len()].copy_from_slice(salt);
    let mut bits = Zeroizing::new([0u8; 40]);
    for chunk in bits.chunks_exact_mut(20) {
        let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&initial[..]).map_err(|_| MALFORMED)?;
        mac.update(&state);
        mac.update(salt);
        chunk.copy_from_slice(&mac.finalize().into_bytes());
        let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&initial[..]).map_err(|_| MALFORMED)?;
        mac.update(&state);
        let next: Zeroizing<[u8; 20]> = Zeroizing::new(mac.finalize().into_bytes().into());
        state[..20].copy_from_slice(&next[..]);
        state[20..].fill(0);
        state.truncate(20);
    }
    let mut key = Zeroizing::new([0u8; 24]);
    key.copy_from_slice(&bits[..24]);
    let mut iv = [0u8; 8];
    iv.copy_from_slice(&bits[32..]);
    Ok((key, iv))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::BrowserKind;
    use aes::cipher::BlockEncryptMut;
    use std::path::Path;

    fn der(tag: u8, data: &[u8]) -> Vec<u8> {
        let mut result = vec![tag];
        if data.len() < 128 {
            result.push(data.len() as u8);
        } else if data.len() <= 255 {
            result.extend([0x81, data.len() as u8]);
        } else {
            result.extend([0x82, (data.len() >> 8) as u8, data.len() as u8]);
        }
        result.extend_from_slice(data);
        result
    }
    fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
        der(0x30, &parts.concat())
    }
    fn pbe(plain: &[u8], pass_key: &[u8], legacy: bool) -> Vec<u8> {
        let salt = [0x34u8; 20];
        if legacy {
            let (key, iv) = legacy_key(pass_key, &salt, 1).unwrap();
            let encrypted = cbc::Encryptor::<des::TdesEde3>::new_from_slices(&key[..], &iv)
                .unwrap()
                .encrypt_padded_vec_mut::<Pkcs7>(plain);
            return seq(&[
                seq(&[der(6, LEGACY_PBE), seq(&[der(4, &salt), der(2, &[1])])]),
                der(4, &encrypted),
            ]);
        }
        let mut key = Zeroizing::new([0u8; 32]);
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pass_key, &salt, 1, &mut key[..]);
        let iv_der = der(4, &[0x56u8; 14]);
        let encrypted = cbc::Encryptor::<aes::Aes256>::new_from_slices(&key[..], &iv_der)
            .unwrap()
            .encrypt_padded_vec_mut::<Pkcs7>(plain);
        let params = seq(&[
            seq(&[
                der(6, PBKDF2),
                seq(&[
                    der(4, &salt),
                    der(2, &[1]),
                    der(2, &[32]),
                    seq(&[der(6, HMAC_SHA256), der(5, &[])]),
                ]),
            ]),
            seq(&[der(6, AES256), iv_der]),
        ]);
        seq(&[seq(&[der(6, PBES2), params]), der(4, &encrypted)])
    }
    pub(crate) fn write_profile(dir: &Path, legacy: bool, protected: bool) {
        let salt = [0x12u8; 20];
        let mut hash = Sha1::new();
        hash.update(salt);
        if protected {
            hash.update(b"fixture primary password");
        }
        let pass_key = Zeroizing::new(hash.finalize().to_vec());
        let key = [0x78u8; 24];
        let id = [0x90u8; 16];
        let db = rusqlite::Connection::open(dir.join("key4.db")).unwrap();
        db.execute_batch("CREATE TABLE metadata(id TEXT PRIMARY KEY,item1 BLOB,item2 BLOB); CREATE TABLE nssPrivate(a11 BLOB,a102 BLOB);").unwrap();
        db.execute(
            "INSERT INTO metadata VALUES('password',?1,?2)",
            rusqlite::params![salt.to_vec(), pbe(b"password-check", &pass_key, legacy)],
        )
        .unwrap();
        db.execute(
            "INSERT INTO nssPrivate VALUES(?1,?2)",
            rusqlite::params![pbe(&key, &pass_key, legacy), id.to_vec()],
        )
        .unwrap();
        let encrypt = |value: &[u8]| {
            let iv = [0x9au8; 8];
            let encrypted = cbc::Encryptor::<des::TdesEde3>::new_from_slices(&key, &iv)
                .unwrap()
                .encrypt_padded_vec_mut::<Pkcs7>(value);
            base64::engine::general_purpose::STANDARD.encode(seq(&[
                der(4, &id),
                seq(&[der(6, DES3), der(4, &iv)]),
                der(4, &encrypted),
            ]))
        };
        let logins = serde_json::json!({"logins":[{"hostname":"https://fixture.example.test","encryptedUsername":encrypt(b"fixture-user"),"encryptedPassword":encrypt(b"fixture-password")}],"version":3});
        std::fs::write(
            dir.join("logins.json"),
            serde_json::to_vec(&logins).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn empty_primary_password_pbes2_and_legacy_fixtures() {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            write_profile(dir.path(), legacy, false);
            let item = Item {
                browser: BrowserKind::Firefox,
                what: "passwords",
                path: dir.path().join("key4.db"),
            };
            let keys = LoginKeys::load(&item).unwrap();
            let json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("logins.json")).unwrap())
                    .unwrap();
            let login = &json["logins"][0];
            assert_eq!(
                &*keys
                    .decrypt(login["encryptedUsername"].as_str().unwrap())
                    .unwrap(),
                "fixture-user"
            );
            assert_eq!(
                &*keys
                    .decrypt(login["encryptedPassword"].as_str().unwrap())
                    .unwrap(),
                "fixture-password"
            );
        }
    }

    #[test]
    fn protected_primary_password_is_not_retried() {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            write_profile(dir.path(), legacy, true);
            let item = Item {
                browser: BrowserKind::Firefox,
                what: "passwords",
                path: dir.path().join("key4.db"),
            };
            let error = LoginKeys::load(&item).err().unwrap();
            assert!(
                matches!(error, crate::ImportError::UnsupportedVersion { detail, .. } if detail == PRIMARY_PASSWORD)
            );
        }
    }

    #[test]
    fn malformed_der_is_rejected_without_panicking() {
        for bytes in [
            &b""[..],
            &[0x30, 0x80],
            &[0x30, 0x84, 0xff, 0xff, 0xff, 0xff],
            &[0x30, 0x81, 1, 0],
        ] {
            assert!(decrypt_pbe(bytes, &[0u8; 20]).is_err());
        }
    }
}
