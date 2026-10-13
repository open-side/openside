//! Chromium-family "Safe Storage" keys, fetched through the OS credential APIs.
//!
//! The browser's own keyring entry is the only source: Secret Service on Linux (the same
//! D-Bus protocol the openside Vault backend speaks), the login Keychain on macOS, and the
//! DPAPI-wrapped `os_crypt.encrypted_key` of `Local State` on Windows. Preview never calls
//! these APIs. An explicit import may display a macOS Keychain access request; Linux never
//! requests Unlock or answers a prompt. Locked/refused access is a typed GUI error.
//! Returned secrets are zeroizing and never included in logs, Debug or error messages.

use std::path::Path;

use base64::Engine as _;
use zeroize::Zeroizing;

use crate::db::Item;
use crate::error::{ImportError, Result};
use crate::BrowserKind;

/// Where one browser keeps its Safe Storage password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SafeStorage {
    /// Keychain service and keyring item label.
    pub(crate) service: &'static str,
    /// Keychain account name.
    pub(crate) account: &'static str,
    /// Secret Service `application` attribute the browser tags its item with.
    pub(crate) application: &'static str,
}

/// Safe Storage names by brand id (the ids of the import contract). Browsers that keep no
/// Safe Storage key (Firefox family, Safari) are absent.
pub(crate) fn safe_storage(brand: &str) -> Option<SafeStorage> {
    let (service, account, application) = match brand {
        "chrome" => ("Chrome Safe Storage", "Chrome", "chrome"),
        "chromium" => ("Chromium Safe Storage", "Chromium", "chromium"),
        "brave" => ("Brave Safe Storage", "Brave", "brave"),
        "edge" => (
            "Microsoft Edge Safe Storage",
            "Microsoft Edge",
            "microsoft-edge",
        ),
        "vivaldi" => ("Vivaldi Safe Storage", "Vivaldi", "vivaldi"),
        "opera" => ("Opera Safe Storage", "Opera", "opera"),
        "arc" => ("Arc Safe Storage", "Arc", "arc"),
        "aside" => ("Aside Safe Storage", "Aside", "aside"),
        _ => return None,
    };
    Some(SafeStorage {
        service,
        account,
        application,
    })
}

fn unavailable(brand: &str, reason: &'static str) -> ImportError {
    ImportError::KeyringUnavailable {
        browser: brand.to_owned(),
        reason,
    }
}

fn locked(brand: &str) -> ImportError {
    ImportError::KeyringLocked {
        browser: brand.to_owned(),
    }
}

/// The Safe Storage password of `brand` from this OS's keyring.
pub(crate) fn keyring_password(brand: &str) -> Result<Zeroizing<String>> {
    let storage = safe_storage(brand)
        .ok_or_else(|| unavailable(brand, "this browser has no known Safe Storage entry"))?;
    platform::password(brand, &storage)
}

#[cfg(target_os = "linux")]
mod platform {
    use super::SafeStorage;
    use crate::error::Result;
    use zeroize::Zeroizing;

    pub(super) fn password(brand: &str, storage: &SafeStorage) -> Result<Zeroizing<String>> {
        super::secret_service::password(None, brand, storage)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{locked, unavailable, SafeStorage};
    use crate::error::Result;
    use zeroize::Zeroizing;

    /// Generic password `<Browser> Safe Storage` / `<Browser>`. Reading another app's item
    /// makes Keychain ask the person for access; a denial or a locked keychain is `locked`.
    pub(super) fn password(brand: &str, storage: &SafeStorage) -> Result<Zeroizing<String>> {
        match keyring::Entry::new(storage.service, storage.account)
            .and_then(|entry| entry.get_password())
        {
            Ok(password) => Ok(Zeroizing::new(password)),
            Err(keyring::Error::NoEntry) => Err(unavailable(brand, "no Keychain item was found")),
            Err(keyring::Error::BadEncoding(_)) => {
                Err(unavailable(brand, "the Keychain item is not text"))
            }
            Err(_) => Err(locked(brand)),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::{unavailable, SafeStorage};
    use crate::error::Result;
    use zeroize::Zeroizing;

    pub(super) fn password(brand: &str, _storage: &SafeStorage) -> Result<Zeroizing<String>> {
        Err(unavailable(
            brand,
            "this platform keeps the key in Local State, not in a keyring entry",
        ))
    }
}

/// Freedesktop Secret Service client, read-only: search by the browser's `application`
/// attribute, unlock without a prompt, fetch the secret over the `plain` transfer algorithm
/// (as libsecret does by default; the key crosses only the user's private session bus).
#[cfg(target_os = "linux")]
pub(crate) mod secret_service {
    use std::collections::HashMap;

    use serde::{Deserialize, Serialize};
    use zbus::blocking::Connection;
    use zeroize::{Zeroize, Zeroizing};
    use zvariant::{OwnedObjectPath, OwnedValue, Type, Value};

    use super::{locked, unavailable, SafeStorage};
    use crate::error::Result;

    const DESTINATION: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE: &str = "org.freedesktop.Secret.Service";
    const ITEM: &str = "org.freedesktop.Secret.Item";
    const SESSION: &str = "org.freedesktop.Secret.Session";

    #[derive(Deserialize, Serialize, Type)]
    struct WireSecret {
        session: OwnedObjectPath,
        parameters: Vec<u8>,
        value: Vec<u8>,
        content_type: String,
    }

    impl Drop for WireSecret {
        fn drop(&mut self) {
            self.value.zeroize();
        }
    }

    fn call<B, R>(
        connection: &Connection,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Option<R>
    where
        B: Serialize + Type,
        R: for<'de> Deserialize<'de> + Type,
    {
        connection
            .call_method(Some(DESTINATION), path, Some(interface), method, body)
            .ok()?
            .body()
            .deserialize::<R>()
            .ok()
    }

    /// The Safe Storage password; `address` selects a private bus (tests), `None` the
    /// session bus.
    pub(crate) fn password(
        address: Option<&str>,
        brand: &str,
        storage: &SafeStorage,
    ) -> Result<Zeroizing<String>> {
        let connection = match address {
            Some(address) => zbus::blocking::connection::Builder::address(address)
                .and_then(|builder| builder.build()),
            None => Connection::session(),
        }
        .map_err(|_| unavailable(brand, "no Secret Service is reachable on the session bus"))?;
        let (_output, session): (OwnedValue, OwnedObjectPath) = call(
            &connection,
            SERVICE_PATH,
            SERVICE,
            "OpenSession",
            &("plain", Value::from("")),
        )
        .ok_or_else(|| unavailable(brand, "the Secret Service refused a session"))?;
        let found = fetch(&connection, &session, brand, storage);
        let _closed: Option<()> = call(&connection, session.as_str(), SESSION, "Close", &());
        found
    }

    fn fetch(
        connection: &Connection,
        session: &OwnedObjectPath,
        brand: &str,
        storage: &SafeStorage,
    ) -> Result<Zeroizing<String>> {
        let attributes = HashMap::from([("application", storage.application)]);
        let (unlocked, locked_items): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = call(
            connection,
            SERVICE_PATH,
            SERVICE,
            "SearchItems",
            &attributes,
        )
        .ok_or_else(|| unavailable(brand, "the Secret Service could not be searched"))?;
        let item = match (unlocked.into_iter().next(), locked_items.into_iter().next()) {
            (Some(item), _) => item,
            (None, Some(_)) => return Err(locked(brand)),
            (None, None) => return Err(unavailable(brand, "no Safe Storage item was found")),
        };
        let secret: WireSecret = call(connection, item.as_str(), ITEM, "GetSecret", session)
            .ok_or_else(|| locked(brand))?;
        let text = std::str::from_utf8(&secret.value)
            .map_err(|_| unavailable(brand, "the Safe Storage item is not text"))?;
        Ok(Zeroizing::new(text.to_owned()))
    }
}

/// The 32-byte AES-GCM key of a Windows profile: `Local State` → `os_crypt.encrypted_key`
/// (base64 of `DPAPI` + a DPAPI blob), unwrapped for the current Windows user.
pub(crate) fn os_crypt_key(
    brand: &str,
    kind: BrowserKind,
    local_state: &Path,
) -> Result<Zeroizing<[u8; 32]>> {
    let item = Item {
        browser: kind,
        what: "Local State",
        path: local_state.to_owned(),
    };
    let json = Zeroizing::new(item.read()?);
    let wrapped = parse_encrypted_key(&json)
        .ok_or_else(|| unavailable(brand, "Local State holds no os_crypt key"))?;
    let key = unprotect(&wrapped).ok_or_else(|| locked(brand))?;
    let key: [u8; 32] = key
        .as_slice()
        .try_into()
        .map_err(|_| unavailable(brand, "the os_crypt key has an unexpected length"))?;
    Ok(Zeroizing::new(key))
}

/// The DPAPI blob inside `os_crypt.encrypted_key`.
fn parse_encrypted_key(local_state: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let value: serde_json::Value = serde_json::from_slice(local_state).ok()?;
    let encoded = value.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
        .map(Zeroizing::new)?;
    bytes
        .strip_prefix(b"DPAPI")
        .map(|blob| Zeroizing::new(blob.to_vec()))
}

#[cfg(not(windows))]
fn unprotect(_blob: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    None
}

/// `CryptUnprotectData` without UI. The OS-allocated plaintext is wiped before it is freed.
#[cfg(windows)]
#[allow(unsafe_code)]
fn unprotect(blob: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    const UI_FORBIDDEN: u32 = 0x1;
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(blob.len()).ok()?,
        pbData: blob.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` points at `blob` for the duration of the call; the optional pointers
    // are null as documented; `output` is a valid out-parameter.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return None;
    }
    let len = usize::try_from(output.cbData).ok()?;
    // SAFETY: on success the OS returns `cbData` readable bytes at `pbData`, which stay
    // valid until `LocalFree`; they are copied, zeroed, then freed exactly once.
    let plain = unsafe {
        let plain = Zeroizing::new(std::slice::from_raw_parts(output.pbData, len).to_vec());
        std::ptr::write_bytes(output.pbData, 0, len);
        LocalFree(output.pbData.cast());
        plain
    };
    Some(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chromium_brand_has_a_distinct_safe_storage_entry() {
        let brands = [
            "chrome", "chromium", "brave", "edge", "vivaldi", "opera", "arc", "aside",
        ];
        let mut services = std::collections::HashSet::new();
        for brand in brands {
            let storage = safe_storage(brand).unwrap();
            assert!(storage.service.ends_with(" Safe Storage"), "{brand}");
            assert!(services.insert(storage.service), "{brand}");
        }
        assert_eq!(
            safe_storage("chrome").map(|s| (s.service, s.account)),
            Some(("Chrome Safe Storage", "Chrome"))
        );
        assert!(safe_storage("firefox").is_none());
        assert!(safe_storage("safari").is_none());
    }

    #[test]
    fn unknown_browsers_report_unavailable_without_touching_a_keyring() {
        let error = keyring_password("firefox").unwrap_err();
        assert!(
            matches!(&error, ImportError::KeyringUnavailable { browser, .. } if browser == "firefox"),
            "{error}"
        );
    }

    #[test]
    fn local_state_key_is_the_dpapi_blob_after_the_marker() {
        let wrapped = base64::engine::general_purpose::STANDARD.encode(b"DPAPI\x01\x02\x03");
        let json = format!(r#"{{"os_crypt":{{"encrypted_key":"{wrapped}"}}}}"#);
        assert_eq!(
            parse_encrypted_key(json.as_bytes())
                .as_deref()
                .map(Vec::as_slice),
            Some(&[1u8, 2, 3][..])
        );
        let unmarked = base64::engine::general_purpose::STANDARD.encode(b"nope");
        let json = format!(r#"{{"os_crypt":{{"encrypted_key":"{unmarked}"}}}}"#);
        assert!(parse_encrypted_key(json.as_bytes()).is_none());
        assert!(parse_encrypted_key(b"{}").is_none());
        assert!(parse_encrypted_key(b"not json").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn dpapi_is_a_locked_keyring_error_off_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Local State");
        let wrapped = base64::engine::general_purpose::STANDARD.encode(b"DPAPI\x01");
        std::fs::write(
            &path,
            format!(r#"{{"os_crypt":{{"encrypted_key":"{wrapped}"}}}}"#),
        )
        .unwrap();
        let error = os_crypt_key("chrome", BrowserKind::Chrome, &path).unwrap_err();
        assert!(
            matches!(error, ImportError::KeyringLocked { .. }),
            "{error}"
        );
        let missing = os_crypt_key("chrome", BrowserKind::Chrome, &dir.path().join("absent"));
        assert!(matches!(missing, Err(ImportError::MissingFile { .. })));
    }

    #[cfg(target_os = "linux")]
    mod secret_service {
        use std::collections::HashMap;
        use std::io::{BufRead, BufReader};
        use std::process::{Child, Command, Stdio};
        use std::sync::Arc;

        use parking_lot::Mutex;

        use serde::{Deserialize, Serialize};
        use zvariant::{OwnedObjectPath, OwnedValue, Type, Value};

        use super::*;
        use crate::safe_storage::secret_service::password;

        const ITEM: &str = "/org/freedesktop/secrets/collection/login/1";
        const SESSION: &str = "/org/freedesktop/secrets/session/1";
        const PROMPT: &str = "/org/freedesktop/secrets/prompt/1";

        struct Store {
            application: &'static str,
            secret: Vec<u8>,
            locked: bool,
            // The unlock answer: `true` completes silently, `false` demands a prompt.
            unlock_without_prompt: bool,
            deny_secret: bool,
            unlock_calls: u32,
        }

        #[derive(Deserialize, Serialize, Type)]
        struct WireSecret {
            session: OwnedObjectPath,
            parameters: Vec<u8>,
            value: Vec<u8>,
            content_type: String,
        }

        fn path(value: &str) -> OwnedObjectPath {
            OwnedObjectPath::try_from(value).unwrap()
        }

        struct Service(Arc<Mutex<Store>>);
        #[zbus::interface(name = "org.freedesktop.Secret.Service")]
        impl Service {
            fn open_session(
                &self,
                algorithm: &str,
                _input: OwnedValue,
            ) -> (OwnedValue, OwnedObjectPath) {
                assert_eq!(algorithm, "plain");
                (
                    OwnedValue::try_from(Value::from("")).unwrap(),
                    path(SESSION),
                )
            }
            fn search_items(
                &self,
                attributes: HashMap<String, String>,
            ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
                let store = self.0.lock();
                let matches = attributes.len() == 1
                    && attributes.get("application").map(String::as_str) == Some(store.application);
                match (matches, store.locked) {
                    (false, _) => (Vec::new(), Vec::new()),
                    (true, false) => (vec![path(ITEM)], Vec::new()),
                    (true, true) => (Vec::new(), vec![path(ITEM)]),
                }
            }
            fn unlock(
                &self,
                objects: Vec<OwnedObjectPath>,
            ) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
                let mut store = self.0.lock();
                store.unlock_calls += 1;
                if store.unlock_without_prompt {
                    store.locked = false;
                    (objects, path("/"))
                } else {
                    (Vec::new(), path(PROMPT))
                }
            }
        }

        struct Item(Arc<Mutex<Store>>);
        #[zbus::interface(name = "org.freedesktop.Secret.Item")]
        impl Item {
            fn get_secret(&self, session: OwnedObjectPath) -> zbus::fdo::Result<WireSecret> {
                let store = self.0.lock();
                if store.deny_secret {
                    return Err(zbus::fdo::Error::AccessDenied("denied".into()));
                }
                Ok(WireSecret {
                    session,
                    parameters: Vec::new(),
                    value: store.secret.clone(),
                    content_type: "text/plain".into(),
                })
            }
        }

        struct Session;
        #[zbus::interface(name = "org.freedesktop.Secret.Session")]
        impl Session {
            fn close(&self) {}
        }

        struct Bus(Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// A private `dbus-daemon`; the user's session bus and keyring are never contacted.
        fn private_bus(dir: &std::path::Path) -> Option<(Bus, String)> {
            let config = dir.join("bus.conf");
            std::fs::write(
                &config,
                format!(
                    "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n<busconfig><type>session</type><listen>unix:path={}</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>\n",
                    dir.join("bus.sock").display()
                ),
            )
            .unwrap();
            let child = match Command::new("dbus-daemon")
                .arg(format!("--config-file={}", config.display()))
                .args(["--nofork", "--print-address"])
                .env_remove("DBUS_SESSION_BUS_ADDRESS")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    eprintln!("SKIP: Safe Storage lookup test needs dbus-daemon: {error}");
                    return None;
                }
            };
            let mut bus = Bus(child);
            let mut address = String::new();
            BufReader::new(bus.0.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            Some((bus, address.trim().to_owned()))
        }

        fn serve(address: &str, store: &Arc<Mutex<Store>>) -> zbus::blocking::Connection {
            zbus::blocking::connection::Builder::address(address)
                .unwrap()
                .serve_at("/org/freedesktop/secrets", Service(store.clone()))
                .unwrap()
                .serve_at(ITEM, Item(store.clone()))
                .unwrap()
                .serve_at(SESSION, Session)
                .unwrap()
                .name("org.freedesktop.secrets")
                .unwrap()
                .build()
                .unwrap()
        }

        fn store(locked: bool, unlock_without_prompt: bool, deny_secret: bool) -> Store {
            Store {
                application: "chrome",
                secret: b"synthetic-safe-storage-secret".to_vec(),
                locked,
                unlock_without_prompt,
                deny_secret,
                unlock_calls: 0,
            }
        }

        #[test]
        fn lookup_distinguishes_ready_locked_denied_and_missing_keyrings() {
            let dir = tempfile::tempdir().unwrap();
            let Some((_bus, address)) = private_bus(dir.path()) else {
                return;
            };
            let chrome = safe_storage("chrome").unwrap();
            let brave = safe_storage("brave").unwrap();

            // Nothing serves org.freedesktop.secrets yet.
            let error = password(Some(&address), "chrome", &chrome).unwrap_err();
            assert!(
                matches!(error, ImportError::KeyringUnavailable { .. }),
                "{error}"
            );

            let shared = Arc::new(Mutex::new(store(false, false, false)));
            let _service = serve(&address, &shared);
            let found = password(Some(&address), "chrome", &chrome).unwrap();
            assert_eq!(&*found, "synthetic-safe-storage-secret");

            // The item belongs to another application attribute.
            let error = password(Some(&address), "brave", &brave).unwrap_err();
            assert!(
                matches!(error, ImportError::KeyringUnavailable { .. }),
                "{error}"
            );

            // Locked and needing an interactive prompt: typed, never answered.
            *shared.lock() = store(true, false, false);
            let error = password(Some(&address), "chrome", &chrome).unwrap_err();
            assert!(
                matches!(error, ImportError::KeyringLocked { .. }),
                "{error}"
            );

            // Even a silent unlock would mutate the keyring without the person's action.
            *shared.lock() = store(true, true, false);
            let error = password(Some(&address), "chrome", &chrome).unwrap_err();
            assert!(
                matches!(error, ImportError::KeyringLocked { .. }),
                "{error}"
            );
            assert!(shared.lock().locked);
            assert_eq!(shared.lock().unlock_calls, 0);

            // The service refuses to hand the secret over.
            *shared.lock() = store(false, false, true);
            let error = password(Some(&address), "chrome", &chrome).unwrap_err();
            assert!(
                matches!(error, ImportError::KeyringLocked { .. }),
                "{error}"
            );
            assert!(!error.to_string().contains("synthetic-safe-storage-secret"));
        }
    }
}
