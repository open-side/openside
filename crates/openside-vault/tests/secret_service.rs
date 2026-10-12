//! C-6: the Secret Service backend against a fake `org.freedesktop.secrets` on a private
//! `dbus-daemon`. The user's session bus and keyring are never contacted: the private bus
//! has no service directories, so nothing can be auto-activated on it.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::Arc,
};

use base64::Engine as _;
use openside_vault::{
    AccessPolicy, AllowedOrigin, AuditAction, AuditContext, Entry, Error, LoginOutcome, Secret,
    SecretServiceLock, Vault,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use zvariant::{OwnedObjectPath, OwnedValue, Type, Value};

const ITEM: &str = "/org/freedesktop/secrets/collection/login/1";
const COLLECTION: &str = "/org/freedesktop/secrets/collection/login";
const SESSION: &str = "/org/freedesktop/secrets/session/1";

#[derive(Default)]
struct Store {
    key: Option<Vec<u8>>,
    attributes: HashMap<String, String>,
    locked: bool,
    sessions: usize,
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
    fn open_session(&self, algorithm: &str, _input: OwnedValue) -> (OwnedValue, OwnedObjectPath) {
        assert_eq!(algorithm, "plain");
        self.0.lock().sessions += 1;
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
        let matches = store.key.is_some()
            && attributes
                .iter()
                .all(|(name, value)| store.attributes.get(name) == Some(value));
        match (matches, store.locked) {
            (false, _) => (Vec::new(), Vec::new()),
            (true, false) => (vec![path(ITEM)], Vec::new()),
            (true, true) => (Vec::new(), vec![path(ITEM)]),
        }
    }
    fn unlock(&self, _objects: Vec<OwnedObjectPath>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
        // A locked keyring needs an interactive prompt; the backend must refuse it.
        (Vec::new(), path("/org/freedesktop/secrets/prompt/1"))
    }
    fn read_alias(&self, name: &str) -> OwnedObjectPath {
        assert_eq!(name, "default");
        path(COLLECTION)
    }
}

struct Collection(Arc<Mutex<Store>>);
#[zbus::interface(name = "org.freedesktop.Secret.Collection")]
impl Collection {
    fn create_item(
        &self,
        properties: HashMap<String, OwnedValue>,
        secret: WireSecret,
        _replace: bool,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        let attributes: HashMap<String, String> = properties
            ["org.freedesktop.Secret.Item.Attributes"]
            .try_clone()
            .unwrap()
            .try_into()
            .unwrap();
        let mut store = self.0.lock();
        store.attributes = attributes;
        store.key = Some(secret.value.clone());
        (path(ITEM), path("/"))
    }
}

struct Item(Arc<Mutex<Store>>);
#[zbus::interface(name = "org.freedesktop.Secret.Item")]
impl Item {
    fn get_secret(&self, session: OwnedObjectPath) -> WireSecret {
        WireSecret {
            session,
            parameters: Vec::new(),
            value: self.0.lock().key.clone().unwrap_or_default(),
            content_type: "application/octet-stream".into(),
        }
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
            eprintln!("SKIP: secret-service backend test needs dbus-daemon: {error}");
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

fn ctx() -> AuditContext<'static> {
    AuditContext {
        who: "test",
        session_id: None,
        tool_call_id: None,
    }
}

#[test]
fn secret_service_holds_wrapping_key_and_master_key_never_on_disk() {
    let bus_dir = tempfile::tempdir().unwrap();
    let Some((_bus, address)) = private_bus(bus_dir.path()) else {
        return;
    };
    assert!(!SecretServiceLock::with_address(address.clone(), "probe").reachable());
    let store = Arc::new(Mutex::new(Store::default()));
    let _service = zbus::blocking::connection::Builder::address(address.as_str())
        .unwrap()
        .serve_at("/org/freedesktop/secrets", Service(store.clone()))
        .unwrap()
        .serve_at(COLLECTION, Collection(store.clone()))
        .unwrap()
        .serve_at(ITEM, Item(store.clone()))
        .unwrap()
        .serve_at(SESSION, Session)
        .unwrap()
        .name("org.freedesktop.secrets")
        .unwrap()
        .build()
        .unwrap();
    assert!(SecretServiceLock::with_address(address.clone(), "probe").reachable());
    assert_eq!(
        openside_vault::stored_format(bus_dir.path().join("absent")),
        Ok(None)
    );

    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("vault.db");
    let audit = data.path().join("vault-audit.jsonl");
    const PASSWORD: &str = "secret-service-canary-password-55";
    let mut vault = Vault::create_with_platform_key(
        &path,
        &audit,
        SecretServiceLock::with_address(address.clone(), "vault-under-test"),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "item".into(),
                vec![AllowedOrigin::parse("https://example.com").unwrap()],
                Secret::new("user".into()),
                Secret::new(PASSWORD.into()),
                None,
                AccessPolicy::WhileUnlocked,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);

    let key = store
        .lock()
        .key
        .clone()
        .expect("wrapping key stored in keyring");
    assert_eq!(key.len(), 32);
    assert_eq!(
        store.lock().attributes,
        HashMap::from([
            ("application".to_owned(), "openside".to_owned()),
            ("openside-vault".to_owned(), "vault-under-test".to_owned()),
        ])
    );
    let needles = [
        key.clone(),
        data_encoding::HEXLOWER.encode(&key).into_bytes(),
        data_encoding::HEXUPPER.encode(&key).into_bytes(),
        base64::engine::general_purpose::STANDARD
            .encode(&key)
            .into_bytes(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(&key)
            .into_bytes(),
        PASSWORD.as_bytes().to_vec(),
    ];
    let mut scanned = 0;
    let mut pending = vec![data.path().to_owned()];
    while let Some(entry) = pending.pop() {
        if entry.is_dir() {
            pending.extend(
                std::fs::read_dir(&entry)
                    .unwrap()
                    .map(|e| e.unwrap().path()),
            );
        } else {
            let bytes = std::fs::read(&entry).unwrap();
            scanned += 1;
            for needle in &needles {
                assert!(
                    !bytes.windows(needle.len()).any(|window| window == needle),
                    "key material on disk in {}",
                    entry.display()
                );
            }
        }
    }
    assert_eq!(
        scanned, 1,
        "only the encrypted vault file exists before any fill"
    );

    let mut reopened = Vault::open_with_platform_key(
        &path,
        &audit,
        SecretServiceLock::with_address(address.clone(), "vault-under-test"),
    )
    .unwrap();
    let mut filled = false;
    assert_eq!(
        reopened
            .vault_login(
                &ctx(),
                AuditAction::Fill,
                "item",
                "https://example.com",
                |secrets| {
                    filled = secrets.password == PASSWORD;
                    Ok(())
                }
            )
            .unwrap(),
        LoginOutcome::Filled
    );
    assert!(filled);
    drop(reopened);
    assert!(store.lock().sessions >= 2);

    // A different vault id has no key; a keyring that needs an unlock prompt fails closed.
    assert_eq!(
        Vault::open_with_platform_key(
            &path,
            &audit,
            SecretServiceLock::with_address(address.clone(), "other-vault"),
        )
        .unwrap_err(),
        Error::Unavailable
    );
    store.lock().locked = true;
    assert_eq!(
        Vault::open_with_platform_key(
            &path,
            &audit,
            SecretServiceLock::with_address(address.clone(), "vault-under-test"),
        )
        .unwrap_err(),
        Error::Unavailable
    );
    // No Secret Service on the bus at all.
    drop(_service);
    store.lock().locked = false;
    assert_eq!(
        Vault::open_with_platform_key(
            &path,
            &audit,
            SecretServiceLock::with_address(address, "vault-under-test"),
        )
        .unwrap_err(),
        Error::Unavailable
    );
}

#[test]
fn while_unlocked_follows_the_live_keyring_lock_of_an_open_vault() {
    let bus_dir = tempfile::tempdir().unwrap();
    let Some((_bus, address)) = private_bus(bus_dir.path()) else {
        return;
    };
    let store = Arc::new(Mutex::new(Store::default()));
    let service = zbus::blocking::connection::Builder::address(address.as_str())
        .unwrap()
        .serve_at("/org/freedesktop/secrets", Service(store.clone()))
        .unwrap()
        .serve_at(COLLECTION, Collection(store.clone()))
        .unwrap()
        .serve_at(ITEM, Item(store.clone()))
        .unwrap()
        .serve_at(SESSION, Session)
        .unwrap()
        .name("org.freedesktop.secrets")
        .unwrap()
        .build()
        .unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut vault = Vault::create_with_platform_key(
        data.path().join("vault.db"),
        data.path().join("vault-audit.jsonl"),
        SecretServiceLock::with_address(address.clone(), "live-lock"),
    )
    .unwrap();
    for (site, policy) in [
        ("while", AccessPolicy::WhileUnlocked),
        ("always", AccessPolicy::Always),
    ] {
        vault
            .put(
                Entry::new(
                    site.into(),
                    vec![AllowedOrigin::parse("https://example.com").unwrap()],
                    Secret::new("user".into()),
                    Secret::new("live-lock-password".into()),
                    None,
                    policy,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let fill = |vault: &mut Vault<SecretServiceLock>, site: &str| {
        vault
            .vault_login(
                &ctx(),
                AuditAction::Fill,
                site,
                "https://example.com",
                |_| Ok(()),
            )
            .unwrap()
    };
    assert_eq!(fill(&mut vault, "while"), LoginOutcome::Filled);

    // The keyring locks while the vault stays open and the daemon keeps running.
    store.lock().locked = true;
    assert_eq!(
        fill(&mut vault, "while"),
        LoginOutcome::Denied(openside_vault::Denial::PlatformLocked)
    );
    // `Always` only needs the decrypted vault, as documented.
    assert_eq!(fill(&mut vault, "always"), LoginOutcome::Filled);
    // An observed lock is sticky: unlocking the store does not revive the session.
    store.lock().locked = false;
    assert_eq!(
        fill(&mut vault, "while"),
        LoginOutcome::Denied(openside_vault::Denial::PlatformLocked)
    );

    // A reopened vault authenticates again; losing the service itself also fails closed.
    drop(vault);
    let mut vault = Vault::open_with_platform_key(
        data.path().join("vault.db"),
        data.path().join("vault-audit.jsonl"),
        SecretServiceLock::with_address(address, "live-lock"),
    )
    .unwrap();
    assert_eq!(fill(&mut vault, "while"), LoginOutcome::Filled);
    drop(service);
    assert_eq!(
        fill(&mut vault, "while"),
        LoginOutcome::Denied(openside_vault::Denial::PlatformLocked)
    );
}
