//! Freedesktop Secret Service (libsecret/GNOME Keyring/KWallet) backend over D-Bus.
//!
//! Only the 32-byte wrapping key is stored, as one item tagged `application=openside`,
//! `openside-vault=<vault id>` in the `default` collection. The `plain` transfer algorithm
//! is used: the key crosses the user's private session bus unencrypted, as with libsecret's
//! default. Locked items and collections that need an interactive prompt fail closed with
//! [`Error::Unavailable`]; this backend never displays or answers prompts.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;
use zeroize::Zeroize;
use zvariant::{OwnedObjectPath, OwnedValue, Type, Value};

use super::{PlatformLock, WrappingKey};
use crate::Error;

const DESTINATION: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const SESSION: &str = "org.freedesktop.Secret.Session";
const NO_PROMPT: &str = "/";

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

/// Wrapping key in the Secret Service of a D-Bus session bus.
pub struct SecretServiceLock {
    address: Option<String>,
    vault_id: String,
    unlocked: AtomicBool,
}

impl std::fmt::Debug for SecretServiceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretServiceLock")
    }
}

impl SecretServiceLock {
    /// Use the current user's session bus (`DBUS_SESSION_BUS_ADDRESS`).
    pub fn session(vault_id: impl Into<String>) -> Self {
        Self {
            address: None,
            vault_id: vault_id.into(),
            unlocked: AtomicBool::new(false),
        }
    }

    /// Use an explicit D-Bus address, e.g. a private test bus.
    pub fn with_address(address: impl Into<String>, vault_id: impl Into<String>) -> Self {
        Self {
            address: Some(address.into()),
            vault_id: vault_id.into(),
            unlocked: AtomicBool::new(false),
        }
    }

    fn connect(&self) -> Result<Connection, Error> {
        match &self.address {
            Some(address) => zbus::blocking::connection::Builder::address(address.as_str())
                .and_then(|builder| builder.build()),
            None => Connection::session(),
        }
        .map_err(|_| Error::Unavailable)
    }

    /// Whether a Secret Service answers on the bus or can be D-Bus-activated there, without
    /// touching any item or showing a prompt. A reachable service may still be locked.
    pub fn reachable(&self) -> bool {
        let Ok(connection) = self.connect() else {
            return false;
        };
        let has_owner = connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &DESTINATION,
            )
            .ok()
            .and_then(|reply| reply.body().deserialize::<bool>().ok())
            .unwrap_or(false);
        has_owner
            || connection
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "ListActivatableNames",
                    &(),
                )
                .ok()
                .and_then(|reply| reply.body().deserialize::<Vec<String>>().ok())
                .is_some_and(|names| names.iter().any(|name| name == DESTINATION))
    }

    fn fetch(
        &self,
        connection: &Connection,
        session: &OwnedObjectPath,
        create: bool,
    ) -> Result<WrappingKey, Error> {
        let attributes = HashMap::from([
            ("application", "openside"),
            ("openside-vault", self.vault_id.as_str()),
        ]);
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = call(
            connection,
            SERVICE_PATH,
            SERVICE,
            "SearchItems",
            &attributes,
        )?;
        let item = match (unlocked.into_iter().next(), locked.into_iter().next()) {
            (Some(item), _) => Some(item),
            (None, Some(item)) => {
                let (done, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = call(
                    connection,
                    SERVICE_PATH,
                    SERVICE,
                    "Unlock",
                    &vec![item.clone()],
                )?;
                if prompt.as_str() != NO_PROMPT || !done.contains(&item) {
                    return Err(Error::Unavailable);
                }
                Some(item)
            }
            (None, None) => None,
        };
        if let Some(item) = item {
            let secret: WireSecret = call(connection, item.as_str(), ITEM, "GetSecret", session)?;
            return WrappingKey::from_bytes(&secret.value);
        }
        if !create {
            return Err(Error::Unavailable);
        }
        let collection: OwnedObjectPath =
            call(connection, SERVICE_PATH, SERVICE, "ReadAlias", &"default")?;
        if collection.as_str() == NO_PROMPT {
            return Err(Error::Unavailable);
        }
        let key = WrappingKey::generate()?;
        let owned: HashMap<String, String> = attributes
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        let properties = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                Value::from("openside vault wrapping key"),
            ),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(owned)),
        ]);
        let secret = WireSecret {
            session: session.clone(),
            parameters: Vec::new(),
            value: key.bytes().to_vec(),
            content_type: "application/octet-stream".into(),
        };
        let (_item, prompt): (OwnedObjectPath, OwnedObjectPath) = call(
            connection,
            collection.as_str(),
            COLLECTION,
            "CreateItem",
            &(properties, &secret, true),
        )?;
        if prompt.as_str() != NO_PROMPT {
            return Err(Error::Unavailable);
        }
        Ok(key)
    }
}

impl SecretServiceLock {
    /// Whether the wrapping-key item currently sits in the service's unlocked set. Any
    /// failure to establish that counts as locked.
    fn item_unlocked(&self) -> bool {
        let Ok(connection) = self.connect() else {
            return false;
        };
        let attributes = HashMap::from([
            ("application", "openside"),
            ("openside-vault", self.vault_id.as_str()),
        ]);
        let found: Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>), Error> = call(
            &connection,
            SERVICE_PATH,
            SERVICE,
            "SearchItems",
            &attributes,
        );
        found.is_ok_and(|(unlocked, _locked)| !unlocked.is_empty())
    }
}

impl PlatformLock for SecretServiceLock {
    fn unlock(&mut self) -> Result<(), Error> {
        self.unlocked.store(true, Ordering::Release);
        Ok(())
    }
    fn lock(&mut self) {
        self.unlocked.store(false, Ordering::Release);
    }
    /// Asks the service on every call, so locking the keyring while the vault stays open
    /// revokes `WhileUnlocked` use. An observed lock is sticky until the vault is reopened.
    fn is_unlocked(&self) -> bool {
        if !self.unlocked.load(Ordering::Acquire) {
            return false;
        }
        if self.item_unlocked() {
            return true;
        }
        self.unlocked.store(false, Ordering::Release);
        false
    }
    fn wrapping_key(&mut self, create: bool) -> Result<Option<WrappingKey>, Error> {
        let connection = self.connect()?;
        let (_output, session): (OwnedValue, OwnedObjectPath) = call(
            &connection,
            SERVICE_PATH,
            SERVICE,
            "OpenSession",
            &("plain", Value::from("")),
        )?;
        let key = self.fetch(&connection, &session, create);
        let _closed: Result<(), Error> = call(&connection, session.as_str(), SESSION, "Close", &());
        key.map(Some)
    }
}

fn call<B, R>(
    connection: &Connection,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Result<R, Error>
where
    B: Serialize + Type,
    R: for<'de> Deserialize<'de> + Type,
{
    connection
        .call_method(Some(DESTINATION), path, Some(interface), method, body)
        .map_err(|_| Error::Unavailable)?
        .body()
        .deserialize::<R>()
        .map_err(|_| Error::Unavailable)
}
