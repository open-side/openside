//! Saved logins of Chrome/Chromium-family profiles: `Login Data` (and, for signed-in
//! profiles, `Login Data For Account`) decrypted into [`SavedPassword`] records.
//!
//! The only consumer is the Vault writer, after the local person started the import. The
//! plaintext lives in zeroizing strings from decryption to hand-off; nothing here logs,
//! prints or serialises it, and every error names a site, never a username or password.
//!
//! Schemes (`os_crypt`): `v10`/`v11` are AES-128-CBC with a zero-IV of 16 spaces and a key
//! from PBKDF2-HMAC-SHA1(password, "saltysalt"): the fixed password `peanuts` (Linux `v10`,
//! 1 iteration), the keyring's Safe Storage password (Linux `v11`, 1 iteration) or the
//! Keychain's (macOS `v10`, 1003 iterations). Windows `v10` is AES-256-GCM
//! (`nonce[12] || ciphertext || tag[16]`) under the DPAPI-unwrapped `Local State` key.
//! Windows `v20` (app-bound) and DPAPI-only blobs are reported, never guessed at.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use zeroize::Zeroizing;

use crate::crypto::{decrypt_raw, derive_key, KeyringSource};
use crate::db::Item;
use crate::error::{ImportError, Result};
use crate::model::{is_web_url, SavedPassword};
use crate::{safe_storage, Source};

const LINUX_V10_PASSWORD: &[u8] = b"peanuts";
const GCM_NONCE_LEN: usize = 12;
const GCM_TAG_LEN: usize = 16;

/// The OS whose `os_crypt` scheme the profile was written under. Production always uses
/// [`Platform::CURRENT`]; tests pick each variant to exercise every scheme on one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    pub(crate) const CURRENT: Self = if cfg!(windows) {
        Self::Windows
    } else if cfg!(target_os = "macos") {
        Self::MacOs
    } else {
        Self::Linux
    };

    fn iterations(self) -> u32 {
        if self == Self::MacOs {
            1003
        } else {
            1
        }
    }
}

enum Cipher {
    Cbc {
        key: Zeroizing<[u8; 16]>,
        /// The key came from the keyring, so a first failure means a wrong password.
        from_keyring: bool,
    },
    Gcm(Zeroizing<[u8; 32]>),
}

/// Decryption keys for one profile, fetched on first use so a profile whose logins need no
/// keyring never touches it.
struct Keys<'a> {
    platform: Platform,
    source: &'a Source,
    keyring: &'a KeyringSource,
    keyring_cbc: Option<Zeroizing<[u8; 16]>>,
    gcm: Option<Zeroizing<[u8; 32]>>,
    // Set once a keyring-derived key decrypted something: later failures are then damage to
    // that entry rather than a wrong password.
    verified: bool,
}

impl<'a> Keys<'a> {
    fn new(platform: Platform, source: &'a Source, keyring: &'a KeyringSource) -> Self {
        Self {
            platform,
            source,
            keyring,
            keyring_cbc: None,
            gcm: None,
            verified: false,
        }
    }

    fn brand(&self) -> String {
        match self.keyring {
            KeyringSource::System { brand } => brand.clone(),
            _ => self.source.kind.label().to_owned(),
        }
    }

    fn keyring_key(&mut self) -> Result<Zeroizing<[u8; 16]>> {
        if let Some(key) = &self.keyring_cbc {
            return Ok(key.clone());
        }
        let Some(password) = self.keyring.resolve()? else {
            return Err(ImportError::KeyringUnavailable {
                browser: self.brand(),
                reason: "no keyring password was provided",
            });
        };
        let key = Zeroizing::new(derive_key(password.as_bytes(), self.platform.iterations()));
        self.keyring_cbc = Some(key.clone());
        Ok(key)
    }

    fn gcm_key(&mut self) -> Result<Zeroizing<[u8; 32]>> {
        if let Some(key) = &self.gcm {
            return Ok(key.clone());
        }
        let key = safe_storage::os_crypt_key(
            &self.brand(),
            self.source.kind,
            &local_state(&self.source.dir),
        )?;
        self.gcm = Some(key.clone());
        Ok(key)
    }

    fn cipher(&mut self, origin: &str, version: &[u8]) -> Result<Cipher> {
        match (self.platform, version) {
            (Platform::Linux, b"v10") => Ok(Cipher::Cbc {
                key: Zeroizing::new(derive_key(LINUX_V10_PASSWORD, 1)),
                from_keyring: false,
            }),
            (Platform::Linux, b"v11") | (Platform::MacOs, b"v10") => Ok(Cipher::Cbc {
                key: self.keyring_key()?,
                from_keyring: true,
            }),
            (Platform::Windows, b"v10") => Ok(Cipher::Gcm(self.gcm_key()?)),
            _ => Err(ImportError::UnsupportedPasswordEncryption {
                origin: origin.to_owned(),
                version: printable(version),
            }),
        }
    }

    fn decrypt(&mut self, origin: &str, blob: &[u8]) -> Result<Zeroizing<String>> {
        let Some((version, body)) = blob.split_at_checked(3) else {
            return Err(ImportError::UnsupportedPasswordEncryption {
                origin: origin.to_owned(),
                version: printable(blob),
            });
        };
        let cipher = self.cipher(origin, version)?;
        let from_keyring = matches!(
            cipher,
            Cipher::Cbc {
                from_keyring: true,
                ..
            }
        );
        let fail = |wrong_key_possible: bool| {
            if from_keyring && wrong_key_possible {
                ImportError::WrongPasswordKey {
                    origin: origin.to_owned(),
                }
            } else {
                ImportError::CorruptPassword {
                    origin: origin.to_owned(),
                }
            }
        };
        let wrong_key_possible = !self.verified;
        let plain: Zeroizing<Vec<u8>> = match &cipher {
            Cipher::Cbc { key, .. } => {
                // A wrong key cannot change the length: a blob that is not whole AES blocks
                // is damaged whatever the password.
                if body.is_empty() || body.len() % 16 != 0 {
                    return Err(fail(false));
                }
                Zeroizing::new(decrypt_raw(key, body).ok_or_else(|| fail(wrong_key_possible))?)
            }
            Cipher::Gcm(key) => {
                if body.len() < GCM_NONCE_LEN + GCM_TAG_LEN {
                    return Err(fail(false));
                }
                let (nonce, sealed) = body.split_at(GCM_NONCE_LEN);
                let plain = Aes256Gcm::new(key.as_slice().into())
                    .decrypt(Nonce::from_slice(nonce), sealed)
                    .map_err(|_| fail(false))?;
                Zeroizing::new(plain)
            }
        };
        let text = std::str::from_utf8(&plain).map_err(|_| fail(wrong_key_possible))?;
        let password = Zeroizing::new(text.to_owned());
        if from_keyring {
            self.verified = true;
        }
        Ok(password)
    }
}

fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(3)
        .map(|byte| {
            if byte.is_ascii_graphic() {
                char::from(*byte)
            } else {
                '?'
            }
        })
        .collect()
}

/// `Local State` sits in the user-data directory, the parent of a Chrome-style profile
/// directory and the profile directory itself for Opera.
fn local_state(profile: &Path) -> PathBuf {
    let beside = profile.parent().map(|parent| parent.join("Local State"));
    match beside {
        Some(path) if path.is_file() => path,
        _ => profile.join("Local State"),
    }
}

/// `scheme://host[:port]` of a web login URL; paths, queries and credentials dropped.
fn web_origin(url: &str) -> Option<String> {
    let url = url.trim();
    if !is_web_url(url) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    ))
}

fn items(source: &Source) -> [Item; 2] {
    let item = |what, file: &str| Item {
        browser: source.kind,
        what,
        path: source.dir.join(file),
    };
    [
        item("saved passwords", "Login Data"),
        item("saved passwords (account store)", "Login Data For Account"),
    ]
}

/// Every saved login of a Chromium-family profile, decrypted. The first entry that cannot be
/// decrypted aborts with a specific error so nothing is silently dropped. Entries Chromium
/// never stores a password for (blocklisted sites, federated sign-in) and non-web logins
/// (Android apps) are left out. The caller (the Vault writer) normalises and de-duplicates.
pub(crate) fn read_passwords(
    source: &Source,
    keyring: &KeyringSource,
) -> Result<Vec<SavedPassword>> {
    read_with(source, &mut Keys::new(Platform::CURRENT, source, keyring))
}

fn read_with(source: &Source, keys: &mut Keys<'_>) -> Result<Vec<SavedPassword>> {
    if !source.kind.is_chromium_family() {
        return Err(ImportError::InvalidSource(source.kind.label().to_owned()));
    }
    let [local, account] = items(source);
    let (local_present, account_present) = (local.probe()?, account.probe()?);
    if !local_present && !account_present {
        return Err(local.missing());
    }
    let mut out = Vec::new();
    if local_present {
        read_store(&local, keys, &mut out)?;
    }
    if account_present {
        // The account store mirrors synced logins; a login also kept locally counts once.
        let local_count = out.len();
        let mut synced = Vec::new();
        read_store(&account, keys, &mut synced)?;
        synced.retain(|login| {
            !out[..local_count]
                .iter()
                .any(|have| have.origin == login.origin && *have.username == *login.username)
        });
        out.append(&mut synced);
    }
    Ok(out)
}

fn read_store(item: &Item, keys: &mut Keys<'_>, out: &mut Vec<SavedPassword>) -> Result<()> {
    let db = item.open()?;
    let columns = db.require(
        item,
        "logins",
        &["origin_url", "username_value", "password_value"],
    )?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    let select = format!(
        "SELECT origin_url, {}, username_value, password_value, {} FROM logins",
        if has("signon_realm") {
            "signon_realm"
        } else {
            "NULL"
        },
        if has("blacklisted_by_user") {
            "blacklisted_by_user"
        } else {
            "0"
        },
    );
    let mut statement = db
        .conn
        .prepare(&select)
        .map_err(|error| item.sqlite(&error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?.map(Zeroizing::new),
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<i64>>(4)?.unwrap_or(0) != 0,
            ))
        })
        .map_err(|error| item.sqlite(&error))?;
    for row in rows {
        let (origin_url, realm, username, blob, blocklisted) =
            row.map_err(|error| item.sqlite(&error))?;
        let blob = blob.unwrap_or_default();
        if blocklisted || blob.is_empty() {
            continue;
        }
        let Some(origin) = origin_url
            .as_deref()
            .and_then(web_origin)
            .or_else(|| realm.as_deref().and_then(web_origin))
        else {
            continue;
        };
        let password = keys.decrypt(&origin, &blob)?;
        out.push(SavedPassword {
            origin,
            username: username.unwrap_or_default(),
            password,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use rusqlite::Connection;

    use super::*;
    use crate::BrowserKind;

    const PASSWORD: &str = "hunter2-canary-password";
    const USERNAME: &str = "canary.user@example.test";
    const KEYRING_PASSWORD: &str = "synthetic-keyring-password";

    fn cbc_blob(prefix: &[u8], password: &[u8], iterations: u32, plain: &str) -> Vec<u8> {
        let key = derive_key(password, iterations);
        let sealed = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &[b' '; 16].into())
            .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());
        [prefix, &sealed].concat()
    }

    fn gcm_blob(key: &[u8; 32], plain: &str) -> Vec<u8> {
        let nonce = [7u8; GCM_NONCE_LEN];
        let sealed = Aes256Gcm::new(key.into())
            .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
            .unwrap();
        [b"v10".as_slice(), &nonce, &sealed].concat()
    }

    struct Row<'a> {
        url: &'a str,
        username: &'a str,
        blob: Vec<u8>,
        blocklisted: bool,
    }

    fn row<'a>(url: &'a str, username: &'a str, blob: Vec<u8>) -> Row<'a> {
        Row {
            url,
            username,
            blob,
            blocklisted: false,
        }
    }

    fn write_login_data(path: &Path, rows: &[Row<'_>]) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR, \
             username_element VARCHAR, username_value VARCHAR, password_element VARCHAR, \
             password_value BLOB, signon_realm VARCHAR NOT NULL, blacklisted_by_user INTEGER NOT NULL)",
        )
        .unwrap();
        for row in rows {
            db.execute(
                "INSERT INTO logins (origin_url, username_value, password_value, signon_realm, \
                 blacklisted_by_user) VALUES (?1, ?2, ?3, ?1, ?4)",
                rusqlite::params![row.url, row.username, row.blob, i64::from(row.blocklisted)],
            )
            .unwrap();
        }
    }

    fn profile(rows: &[Row<'_>]) -> (tempfile::TempDir, Source) {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        std::fs::create_dir(&profile).unwrap();
        write_login_data(&profile.join("Login Data"), rows);
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: profile,
        };
        (dir, source)
    }

    fn read(
        platform: Platform,
        source: &Source,
        keyring: &KeyringSource,
    ) -> Result<Vec<SavedPassword>> {
        read_with(source, &mut Keys::new(platform, source, keyring))
    }

    #[test]
    fn linux_v10_decrypts_with_the_fixed_peanuts_password_and_needs_no_keyring() {
        let blob = cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/login?next=/a", USERNAME, blob)]);
        let found = read(Platform::Linux, &source, &KeyringSource::None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].origin, "https://example.test");
        assert_eq!(*found[0].username, USERNAME);
        assert_eq!(*found[0].password, PASSWORD);
    }

    #[test]
    fn linux_v11_uses_the_keyring_password_and_distinguishes_wrong_and_missing() {
        let blob = cbc_blob(b"v11", KEYRING_PASSWORD.as_bytes(), 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let found = read(
            Platform::Linux,
            &source,
            &KeyringSource::password(KEYRING_PASSWORD),
        )
        .unwrap();
        assert_eq!(*found[0].password, PASSWORD);

        let wrong = read(Platform::Linux, &source, &KeyringSource::password("not-it"));
        assert!(
            matches!(wrong, Err(ImportError::WrongPasswordKey { .. })),
            "{wrong:?}"
        );

        let none = read(Platform::Linux, &source, &KeyringSource::None);
        assert!(
            matches!(none, Err(ImportError::KeyringUnavailable { .. })),
            "{none:?}"
        );
    }

    #[test]
    fn a_locked_or_missing_system_keyring_is_a_typed_error_not_a_panic() {
        let blob = cbc_blob(b"v11", KEYRING_PASSWORD.as_bytes(), 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let keyring = KeyringSource::System {
            brand: "firefox".into(),
        };
        let error = read(Platform::Linux, &source, &keyring).unwrap_err();
        assert!(
            matches!(error, ImportError::KeyringUnavailable { ref browser, .. } if browser == "firefox"),
            "{error:?}"
        );
    }

    #[test]
    fn profiles_without_keyring_logins_never_touch_the_keyring() {
        let blob = cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let keyring = KeyringSource::System {
            brand: "firefox".into(),
        };
        assert_eq!(read(Platform::Linux, &source, &keyring).unwrap().len(), 1);
    }

    #[test]
    fn macos_v10_derives_with_1003_iterations_from_the_keychain_password() {
        let blob = cbc_blob(b"v10", KEYRING_PASSWORD.as_bytes(), 1003, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let found = read(
            Platform::MacOs,
            &source,
            &KeyringSource::password(KEYRING_PASSWORD),
        )
        .unwrap();
        assert_eq!(*found[0].password, PASSWORD);
        // The Linux iteration count must not decrypt a macOS blob.
        assert!(read(
            Platform::Linux,
            &source,
            &KeyringSource::password(KEYRING_PASSWORD)
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_keyring_command_supplies_the_password() {
        let blob = cbc_blob(b"v11", b"cmd-secret", 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let found = read(
            Platform::Linux,
            &source,
            &KeyringSource::Command("echo cmd-secret".into()),
        )
        .unwrap();
        assert_eq!(*found[0].password, PASSWORD);
    }

    #[test]
    fn windows_v10_is_aes_256_gcm_under_the_local_state_key() {
        let key = [0x42u8; 32];
        let (_dir, source) = profile(&[row(
            "https://example.test/",
            USERNAME,
            gcm_blob(&key, PASSWORD),
        )]);
        let keyring = KeyringSource::None;
        let mut keys = Keys::new(Platform::Windows, &source, &keyring);
        keys.gcm = Some(Zeroizing::new(key));
        let found = read_with(&source, &mut keys).unwrap();
        assert_eq!(*found[0].password, PASSWORD);

        let mut wrong = Keys::new(Platform::Windows, &source, &keyring);
        wrong.gcm = Some(Zeroizing::new([0x43u8; 32]));
        let error = read_with(&source, &mut wrong).unwrap_err();
        assert!(
            matches!(error, ImportError::CorruptPassword { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn windows_reports_a_missing_local_state_by_file() {
        let key = [0x42u8; 32];
        let (_dir, source) = profile(&[row(
            "https://example.test/",
            USERNAME,
            gcm_blob(&key, PASSWORD),
        )]);
        // No `Local State` beside the profile.
        let error = read(Platform::Windows, &source, &KeyringSource::None).unwrap_err();
        assert!(
            matches!(error, ImportError::MissingFile { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn unsupported_and_damaged_blobs_are_reported_by_site_only() {
        let key = [0x42u8; 32];
        for (platform, blob, expect) in [
            (
                Platform::Windows,
                b"v20abcdefghijklmnopqrstuvwxyz0123456789".to_vec(),
                "v20",
            ),
            (
                Platform::Linux,
                b"\x01\x00\x00\x00restOfDpapiBlob".to_vec(),
                "?..",
            ),
            (Platform::MacOs, b"v11aaaaaaaaaaaaaaaa".to_vec(), "v11"),
        ] {
            let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
            let keyring = KeyringSource::password(KEYRING_PASSWORD);
            let mut keys = Keys::new(platform, &source, &keyring);
            keys.gcm = Some(Zeroizing::new(key));
            let error = read_with(&source, &mut keys).unwrap_err();
            assert!(
                matches!(&error, ImportError::UnsupportedPasswordEncryption { origin, version }
                    if origin == "https://example.test" && version.starts_with(&expect[..1])),
                "{error:?}"
            );
        }

        let truncated = cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let truncated = truncated[..truncated.len() - 3].to_vec();
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, truncated)]);
        let error = read(Platform::Linux, &source, &KeyringSource::None).unwrap_err();
        assert!(
            matches!(error, ImportError::CorruptPassword { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_damaged_entry_after_a_verified_key_is_corrupt_not_a_wrong_password() {
        let good = cbc_blob(b"v11", KEYRING_PASSWORD.as_bytes(), 1, PASSWORD);
        let mut bad = cbc_blob(b"v11", KEYRING_PASSWORD.as_bytes(), 1, "other");
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        let (_dir, source) = profile(&[
            row("https://a.example.test/", USERNAME, good),
            row("https://b.example.test/", USERNAME, bad),
        ]);
        let error = read(
            Platform::Linux,
            &source,
            &KeyringSource::password(KEYRING_PASSWORD),
        )
        .unwrap_err();
        assert!(
            matches!(&error, ImportError::CorruptPassword { origin } if origin == "https://b.example.test"),
            "{error:?}"
        );
    }

    #[test]
    fn blocklisted_empty_and_non_web_logins_are_left_out() {
        let blob = || cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let mut blocked = row("https://blocked.example.test/", "", blob());
        blocked.blocklisted = true;
        let (_dir, source) = profile(&[
            blocked,
            row("https://federated.example.test/", USERNAME, Vec::new()),
            row("android://hash@com.example.app/", USERNAME, blob()),
            row(
                "https://User:pw@Shop.Example.Test:8443/a/b?c=d#e",
                USERNAME,
                blob(),
            ),
        ]);
        let found = read(Platform::Linux, &source, &KeyringSource::None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].origin, "https://shop.example.test:8443");
    }

    #[test]
    fn the_account_store_adds_logins_and_does_not_duplicate_local_ones() {
        let blob = || cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://local.example.test/", USERNAME, blob())]);
        write_login_data(
            &source.dir.join("Login Data For Account"),
            &[
                row("https://local.example.test/", USERNAME, blob()),
                row("https://synced.example.test/", USERNAME, blob()),
            ],
        );
        let found = read(Platform::Linux, &source, &KeyringSource::None).unwrap();
        let origins: Vec<_> = found.iter().map(|login| login.origin.as_str()).collect();
        assert_eq!(
            origins,
            ["https://local.example.test", "https://synced.example.test"]
        );
    }

    #[test]
    fn a_missing_login_database_and_a_foreign_schema_are_specific_errors() {
        let dir = tempfile::tempdir().unwrap();
        let source = Source {
            kind: BrowserKind::Chrome,
            dir: dir.path().to_owned(),
        };
        let missing = read(Platform::Linux, &source, &KeyringSource::None);
        assert!(matches!(missing, Err(ImportError::MissingFile { .. })));

        Connection::open(dir.path().join("Login Data"))
            .unwrap()
            .execute_batch("CREATE TABLE logins (origin_url TEXT)")
            .unwrap();
        let schema = read(Platform::Linux, &source, &KeyringSource::None);
        assert!(matches!(
            schema,
            Err(ImportError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn no_secret_reaches_debug_or_display_output() {
        let blob = cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let found = read(Platform::Linux, &source, &KeyringSource::None).unwrap();
        let debug = format!("{:?} {:?}", found[0], found);
        assert!(
            !debug.contains(PASSWORD) && !debug.contains(USERNAME),
            "{debug}"
        );

        let keyring = KeyringSource::password(KEYRING_PASSWORD);
        assert!(!format!("{keyring:?}").contains(KEYRING_PASSWORD));

        let blob = cbc_blob(b"v11", KEYRING_PASSWORD.as_bytes(), 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let wrong = KeyringSource::password("wrong-keyring-password");
        let error = read(Platform::Linux, &source, &wrong).unwrap_err();
        for text in [error.to_string(), format!("{error:?}")] {
            for secret in [
                PASSWORD,
                USERNAME,
                KEYRING_PASSWORD,
                "wrong-keyring-password",
            ] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    #[test]
    fn secrets_are_held_in_zeroize_on_drop_containers() {
        fn zeroizes<T: zeroize::ZeroizeOnDrop>(_: &T) {}
        let blob = cbc_blob(b"v10", b"peanuts", 1, PASSWORD);
        let (_dir, source) = profile(&[row("https://example.test/", USERNAME, blob)]);
        let found = read(Platform::Linux, &source, &KeyringSource::None).unwrap();
        zeroizes(&found[0].username);
        zeroizes(&found[0].password);
        let key: Zeroizing<[u8; 16]> = Zeroizing::new(derive_key(b"peanuts", 1));
        zeroizes(&key);
    }

    #[test]
    fn derived_keys_match_the_documented_linux_and_macos_values() {
        assert_eq!(
            derive_key(b"peanuts", 1),
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
                0x27, 0x78
            ]
        );
        assert_ne!(derive_key(b"peanuts", 1003), derive_key(b"peanuts", 1));
    }

    #[test]
    fn firefox_family_sources_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = Source {
            kind: BrowserKind::Firefox,
            dir: dir.path().to_owned(),
        };
        let error = read(Platform::Linux, &source, &KeyringSource::None).unwrap_err();
        assert!(matches!(error, ImportError::InvalidSource(_)), "{error:?}");
    }
}
