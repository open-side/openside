use std::{collections::BTreeMap, sync::LazyLock};

use parking_lot::Mutex;
use proptest::prelude::*;
use zeroize::Zeroizing;

use crate::*;

static KEY: LazyLock<SyncKey> = LazyLock::new(|| {
    SyncKey::derive(
        b"correct horse battery staple",
        AccountId::parse("00112233445566778899aabbccddeeff").unwrap(),
    )
    .unwrap()
});

fn shared_key() -> &'static SyncKey {
    &KEY
}

/// In-memory backend with WebDAV semantics: per-record entity tags and conditional writes.
#[derive(Default)]
struct MemoryBackend {
    inner: Mutex<BTreeMap<String, Vec<u8>>>,
}

fn tag(blob: &[u8]) -> String {
    crate::backend::etag(blob)
}

impl Backend for MemoryBackend {
    async fn list(&self) -> Result<Vec<Listed>, Error> {
        Ok(self
            .inner
            .lock()
            .iter()
            .map(|(key, blob)| Listed {
                key: key.clone(),
                etag: tag(blob),
            })
            .collect())
    }
    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, Error> {
        Ok(self
            .inner
            .lock()
            .get(key)
            .map(|blob| (blob.clone(), tag(blob))))
    }
    async fn put(&self, key: &str, if_match: Option<&str>, blob: &[u8]) -> Result<Pushed, Error> {
        let mut inner = self.inner.lock();
        if inner.get(key).map(|b| tag(b)).as_deref() != if_match {
            return Ok(Pushed::Conflict);
        }
        inner.insert(key.to_owned(), blob.to_vec());
        Ok(Pushed::Stored(Some(tag(blob))))
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
struct MemoryStore {
    items: BTreeMap<(Kind, String), Vec<u8>>,
    vault_locked: bool,
    /// Local records outside the synced selection: hidden from snapshots, and remote
    /// versions of them are declined.
    unselected: BTreeMap<(Kind, String), Vec<u8>>,
}

impl LocalStore for MemoryStore {
    fn available(&self, kind: Kind) -> bool {
        !(kind == Kind::VaultItem && self.vault_locked)
    }
    async fn snapshot(
        &mut self,
        kind: Kind,
    ) -> Result<BTreeMap<String, Zeroizing<Vec<u8>>>, Error> {
        Ok(self
            .items
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .map(|((_, id), body)| (id.clone(), Zeroizing::new(body.clone())))
            .collect())
    }
    async fn apply(&mut self, kind: Kind, id: &str, body: Option<&[u8]>) -> Result<Applied, Error> {
        if body == Some(b"invalid".as_slice()) {
            return Err(Error::Format);
        }
        if self.unselected.contains_key(&(kind, id.to_owned())) {
            return Ok(Applied::Declined);
        }
        match body {
            Some(body) => self.items.insert((kind, id.to_owned()), body.to_vec()),
            None => self.items.remove(&(kind, id.to_owned())),
        };
        Ok(Applied::Applied)
    }
}

struct Device {
    state: ClientState,
    store: MemoryStore,
    /// The state as of the last checkpoint: what a restart after a crash would load.
    saved: ClientState,
}

impl Device {
    fn new(id: &str) -> Self {
        let mut state = ClientState::new(shared_key()).unwrap();
        state.device_id = id.to_owned();
        Self {
            saved: state.clone(),
            state,
            store: MemoryStore::default(),
        }
    }
    fn set(&mut self, kind: Kind, id: &str, body: &[u8]) {
        self.store
            .items
            .insert((kind, id.to_owned()), body.to_vec());
    }
    fn delete(&mut self, kind: Kind, id: &str) {
        self.store.items.remove(&(kind, id.to_owned()));
    }
    async fn try_sync<B: Backend>(&mut self, backend: &B) -> Result<Report, Error> {
        let mut saved = None;
        let result = sync(
            &mut self.state,
            shared_key(),
            backend,
            &mut self.store,
            &mut |state| {
                saved = Some(state.clone());
                Ok(())
            },
        )
        .await;
        if let Some(saved) = saved {
            self.saved = saved;
        }
        if result.is_ok() {
            self.saved = self.state.clone();
        }
        result
    }
    async fn sync<B: Backend>(&mut self, backend: &B) -> Report {
        self.try_sync(backend).await.unwrap()
    }
    /// Crash and restart: everything after the last checkpoint is lost.
    fn restart(&mut self) {
        self.state = self.saved.clone();
    }
}

fn get(device: &Device, kind: Kind, id: &str) -> Option<Vec<u8>> {
    device.store.items.get(&(kind, id.to_owned())).cloned()
}

#[test]
fn pairing_code_round_trips_and_rejects_garbage() {
    let code = shared_key().pairing_code();
    let text = code.encode();
    assert!(text.starts_with("osync1-"));
    assert_eq!(PairingCode::parse(&text).unwrap(), code);
    assert_eq!(
        PairingCode::parse(&format!("  {}  ", text.to_uppercase())).unwrap(),
        code
    );
    assert_eq!(PairingCode::parse("osync1-abc"), Err(Error::Format));
    assert_eq!(PairingCode::parse("nonsense"), Err(Error::Format));
}

#[test]
fn wrong_passphrase_fails_check_and_cannot_open_blobs() {
    let key = shared_key();
    let wrong = SyncKey::derive(b"wrong passphrase", key.account()).unwrap();
    assert!(!wrong.matches(&key.pairing_code()));
    assert!(key.matches(&key.pairing_code()));
    let record = key.record_key(Kind::VaultItem, "bank");
    let blob = key.seal(&record, b"secret body").unwrap();
    assert_eq!(&*key.open(&record, &blob).unwrap(), b"secret body");
    assert_eq!(wrong.open(&record, &blob), Err(Error::Crypto));
    // The record key derivation is keyed as well: the wrong key maps ids elsewhere.
    assert_ne!(wrong.record_key(Kind::VaultItem, "bank"), record);
}

#[test]
fn sealed_blob_is_bound_to_its_record_and_detects_tampering() {
    let key = shared_key();
    let record = key.record_key(Kind::PinnedTab, "a");
    let other = key.record_key(Kind::PinnedTab, "b");
    let blob = key.seal(&record, b"https://example.test/").unwrap();
    assert_eq!(key.open(&other, &blob), Err(Error::Crypto));
    for index in [0, 5, blob.len() - 1] {
        let mut tampered = blob.clone();
        tampered[index] ^= 1;
        assert_eq!(key.open(&record, &tampered), Err(Error::Crypto));
    }
    let again = key.seal(&record, b"https://example.test/").unwrap();
    assert_ne!(blob, again, "fresh nonce per seal");
    assert!(!blob.windows(5).any(|w| w == b"https"));
}

#[tokio::test]
async fn two_devices_exchange_pins_and_vault_items_and_deletions() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.set(Kind::PinnedTab, "t1", b"{\"url\":\"https://one.test/\"}");
    a.set(Kind::VaultItem, "bank", b"{\"password\":\"p\"}");
    let report = a.sync(&backend).await;
    assert_eq!(report.pushed, 2);
    let report = b.sync(&backend).await;
    assert_eq!(report.applied, 2);
    assert_eq!(a.store.items, b.store.items);
    b.delete(Kind::PinnedTab, "t1");
    b.sync(&backend).await;
    a.sync(&backend).await;
    assert_eq!(get(&a, Kind::PinnedTab, "t1"), None);
    assert_eq!(a.store.items, b.store.items);
    let idle = a.sync(&backend).await;
    assert_eq!(
        (idle.pushed, idle.applied),
        (0, 0),
        "settled state is a fixpoint"
    );
}

#[tokio::test]
async fn concurrent_edits_resolve_by_lamport_then_device_id() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.set(Kind::VaultItem, "bank", b"base");
    a.sync(&backend).await;
    b.sync(&backend).await;
    // Same Lamport counter on both sides: the greater device id ("b") wins everywhere.
    a.set(Kind::VaultItem, "bank", b"from-a");
    b.set(Kind::VaultItem, "bank", b"from-b");
    let report = b.sync(&backend).await;
    assert_eq!((report.conflicts, report.pushed), (0, 1), "b pushes first");
    let report = a.sync(&backend).await;
    assert_eq!(
        report.conflicts, 1,
        "a's unsynced edit was replaced and reported"
    );
    assert_eq!(report.pushed, 0);
    let report = b.sync(&backend).await;
    assert_eq!(
        (report.conflicts, report.applied),
        (0, 0),
        "b's own edit stands"
    );
    assert_eq!(
        get(&a, Kind::VaultItem, "bank").as_deref(),
        Some(&b"from-b"[..])
    );
    assert_eq!(
        get(&b, Kind::VaultItem, "bank").as_deref(),
        Some(&b"from-b"[..])
    );
    // A device that has seen more history issues a higher counter and wins.
    a.set(Kind::VaultItem, "bank", b"a-later");
    a.state.lamport += 5;
    b.set(Kind::VaultItem, "bank", b"b-older");
    b.sync(&backend).await;
    a.sync(&backend).await;
    b.sync(&backend).await;
    assert_eq!(
        get(&b, Kind::VaultItem, "bank").as_deref(),
        Some(&b"a-later"[..])
    );
    assert_eq!(a.store, b.store);
}

#[tokio::test]
async fn tombstone_wins_a_tie_with_a_concurrent_edit() {
    let backend = MemoryBackend::default();
    // "z" > "a": without the tombstone rule z's edit would win the device tiebreak.
    let mut a = Device::new("a");
    let mut z = Device::new("z");
    a.set(Kind::PinnedTab, "t", b"v1");
    a.sync(&backend).await;
    z.sync(&backend).await;
    a.delete(Kind::PinnedTab, "t");
    z.set(Kind::PinnedTab, "t", b"v2");
    z.sync(&backend).await;
    a.sync(&backend).await;
    z.sync(&backend).await;
    assert_eq!(get(&a, Kind::PinnedTab, "t"), None);
    assert_eq!(get(&z, Kind::PinnedTab, "t"), None);
}

#[tokio::test]
async fn tampered_or_invalid_records_are_rejected_not_applied() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.set(Kind::VaultItem, "bank", b"good");
    a.set(Kind::PinnedTab, "bad", b"invalid");
    a.sync(&backend).await;
    {
        let mut inner = backend.inner.lock();
        let key = shared_key().record_key(Kind::VaultItem, "bank");
        let blob = inner.get_mut(&key).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x40;
    }
    let report = b.sync(&backend).await;
    assert_eq!(report.rejected, 2, "{report:?}");
    assert_eq!(report.applied, 0);
    assert!(b.store.items.is_empty());
}

#[tokio::test]
async fn locked_vault_defers_without_deleting() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.set(Kind::VaultItem, "bank", b"one");
    a.sync(&backend).await;
    b.sync(&backend).await;
    a.set(Kind::VaultItem, "bank", b"two");
    a.sync(&backend).await;
    // While b's vault is locked its items are invisible: that must not become deletions,
    // and a's newer version must wait instead of being skipped for good.
    b.store.vault_locked = true;
    let report = b.sync(&backend).await;
    assert_eq!((report.deferred, report.pushed), (1, 0));
    b.store.vault_locked = false;
    let report = b.sync(&backend).await;
    assert_eq!(report.applied, 1);
    assert_eq!(
        get(&b, Kind::VaultItem, "bank").as_deref(),
        Some(&b"two"[..])
    );
    a.sync(&backend).await;
    assert_eq!(
        get(&a, Kind::VaultItem, "bank").as_deref(),
        Some(&b"two"[..])
    );
}

const CANARIES: [&[u8]; 8] = [
    b"CANARY-SECRET-77",
    b"canary-url",
    b"CanaryTitle",
    b"canary-site",
    b"CanaryMemory",
    b"canary-policy",
    b"pinned_tab",
    b"vault_item",
];

fn canary_records(device: &mut Device) {
    device.set(
        Kind::PinnedTab,
        "t",
        b"{\"url\":\"https://canary-url.test/\",\"title\":\"CanaryTitle\"}",
    );
    device.set(
        Kind::VaultItem,
        "canary-site",
        b"{\"password\":\"CANARY-SECRET-77\"}",
    );
    device.set(Kind::Memory, "m1", b"{\"content\":\"CanaryMemory\"}");
    device.set(
        Kind::Setting,
        "policy",
        b"{\"allow_origins\":[\"https://canary-policy.test\"]}",
    );
}

/// Every file under `dir`, recursively, must be free of plaintext canaries.
fn assert_ciphertext_only(dir: &std::path::Path) -> usize {
    let mut files = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files += assert_ciphertext_only(&path);
            continue;
        }
        files += 1;
        let bytes = std::fs::read(&path).unwrap();
        let name = path.to_string_lossy().into_owned();
        for canary in CANARIES {
            assert!(
                !bytes.windows(canary.len()).any(|w| w == canary)
                    && !name.as_bytes().windows(canary.len()).any(|w| w == canary),
                "plaintext {} in backend storage",
                String::from_utf8_lossy(canary)
            );
        }
    }
    files
}

#[tokio::test]
async fn webdav_backend_stores_only_ciphertext_and_needs_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("dav");
    let (addr, server) = fake_webdav::spawn(
        "127.0.0.1:0".parse().unwrap(),
        storage.clone(),
        Some(("alice", "dav-pw")),
    )
    .await
    .unwrap();
    let base = format!("http://{addr}/remote.php/dav");
    let key = shared_key();
    let credentials = Some(("alice".to_owned(), Zeroizing::new("dav-pw".to_owned())));
    let dav_a = WebDavBackend::new(&base, key.account(), credentials.clone()).unwrap();
    let dav_b = WebDavBackend::new(&base, key.account(), credentials).unwrap();
    let anonymous = WebDavBackend::new(&base, key.account(), None).unwrap();
    assert_eq!(anonymous.ensure().await, Err(Error::Unauthorized));
    dav_a.ensure().await.unwrap();
    dav_b.ensure().await.unwrap();
    assert!(matches!(anonymous.list().await, Err(Error::Unauthorized)));

    let mut a = Device::new("a");
    let mut b = Device::new("b");
    canary_records(&mut a);
    assert_eq!(a.sync(&dav_a).await.pushed, 4);
    b.sync(&dav_b).await;
    assert_eq!(a.store.items, b.store.items);
    // Concurrent writers: b's stale If-Match loses (412), b merges and retries.
    a.set(Kind::PinnedTab, "t", b"a2");
    b.set(Kind::PinnedTab, "t", b"b2");
    a.sync(&dav_a).await;
    b.sync(&dav_b).await;
    a.sync(&dav_a).await;
    assert_eq!(a.store.items, b.store.items);
    assert_eq!(get(&a, Kind::PinnedTab, "t").as_deref(), Some(&b"b2"[..]));
    // Only blobs whose entity tag changed are fetched again.
    let idle = a.sync(&dav_a).await;
    assert_eq!((idle.pulled, idle.pushed), (0, 0));

    assert_eq!(assert_ciphertext_only(&storage), 4);
    // A device holding the wrong passphrase reads the share but can open nothing.
    let wrong = SyncKey::derive(b"not the passphrase", key.account()).unwrap();
    let mut state = ClientState::new(&wrong).unwrap();
    let mut store = MemoryStore::default();
    let report = sync(&mut state, &wrong, &dav_a, &mut store, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!((report.rejected, report.applied), (4, 0));
    assert!(store.items.is_empty());
    server.abort();
}

#[tokio::test]
async fn directory_backend_syncs_without_a_server_and_restores_removed_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let backend = DirBackend::open(dir.path(), shared_key().account()).unwrap();
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    canary_records(&mut a);
    a.sync(&backend).await;
    b.set(Kind::VaultItem, "v", b"vault");
    b.sync(&backend).await;
    a.sync(&backend).await;
    assert_eq!(a.store.items, b.store.items);
    assert_eq!(a.store.items.len(), 5);
    assert_eq!(assert_ciphertext_only(dir.path()), 5);
    // A blob removed from the share behind the devices' backs is uploaded again by the next
    // device that holds it, instead of wedging that record's conditional writes.
    let removed = dir.path().join(shared_key().account().hex()).join(format!(
        "{}.blob",
        shared_key().record_key(Kind::PinnedTab, "t")
    ));
    std::fs::remove_file(&removed).unwrap();
    assert_eq!(a.sync(&backend).await.pushed, 1);
    assert!(removed.exists());
    let idle = b.sync(&backend).await;
    assert_eq!((idle.pushed, idle.applied), (0, 0));
}

/// A validly sealed record with an arbitrary stamp, as another key holder could write it.
fn seal_record(kind: Kind, id: &str, body: Option<&[u8]>, lamport: u64, device: &str) -> Vec<u8> {
    use base64::Engine as _;
    let payload = crate::record::Payload {
        v: crate::record::PAYLOAD_VERSION,
        kind,
        id: id.to_owned(),
        stamp: Stamp {
            lamport,
            deleted: body.is_none(),
            device: device.to_owned(),
        },
        body: body.map(|b| base64::engine::general_purpose::STANDARD.encode(b)),
    };
    let key = shared_key();
    key.seal(
        &key.record_key(kind, id),
        &serde_json::to_vec(&payload).unwrap(),
    )
    .unwrap()
}

/// Backend wrapper for fault injection: fails ("crashes") after a number of successful
/// writes, or lets a competing device write right before this device's next write.
struct Faulty<'a> {
    inner: &'a MemoryBackend,
    crash_after_puts: Mutex<Option<usize>>,
    interleave: Mutex<Option<(String, Vec<u8>)>>,
    always_conflict: bool,
}

impl<'a> Faulty<'a> {
    fn new(inner: &'a MemoryBackend) -> Self {
        Self {
            inner,
            crash_after_puts: Mutex::new(None),
            interleave: Mutex::new(None),
            always_conflict: false,
        }
    }
}

impl Backend for Faulty<'_> {
    async fn list(&self) -> Result<Vec<Listed>, Error> {
        self.inner.list().await
    }
    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>, Error> {
        self.inner.get(key).await
    }
    async fn put(&self, key: &str, if_match: Option<&str>, blob: &[u8]) -> Result<Pushed, Error> {
        if self.always_conflict {
            return Ok(Pushed::Conflict);
        }
        let competing = self.interleave.lock().take();
        if let Some((other_key, other_blob)) = competing {
            // Another device's write lands between this device's pull and its push.
            let current = self.inner.get(&other_key).await?.map(|(_, tag)| tag);
            assert!(matches!(
                self.inner
                    .put(&other_key, current.as_deref(), &other_blob)
                    .await?,
                Pushed::Stored(_)
            ));
        }
        let stored = self.inner.put(key, if_match, blob).await?;
        let mut crash = self.crash_after_puts.lock();
        if let Some(left) = crash.as_mut() {
            if *left == 0 {
                return Err(Error::Backend("crashed".into()));
            }
            *left -= 1;
        }
        Ok(stored)
    }
}

#[tokio::test]
async fn crash_between_put_and_state_save_never_reuses_a_published_stamp() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut b = Device::new("b");
    a.set(Kind::Setting, "policy", b"X");
    let faulty = Faulty::new(&backend);
    *faulty.crash_after_puts.lock() = Some(0);
    // The blob is stored, then the device dies before saving its state.
    assert!(a.try_sync(&faulty).await.is_err());
    assert_eq!(backend.inner.lock().len(), 1);
    a.restart();
    b.sync(&backend).await;
    assert_eq!(get(&b, Kind::Setting, "policy").as_deref(), Some(&b"X"[..]));
    // After the restart an edit must get a stamp newer than the published one.
    a.set(Kind::Setting, "policy", b"Y");
    a.sync(&backend).await;
    b.sync(&backend).await;
    assert_eq!(get(&b, Kind::Setting, "policy").as_deref(), Some(&b"Y"[..]));
    assert_eq!(a.store.items, b.store.items);
}

#[tokio::test]
async fn equal_stamps_naming_different_bodies_still_converge() {
    // Two devices sharing an id issue the same stamp for different bodies: the keyed
    // digest decides, identically everywhere.
    let backend = MemoryBackend::default();
    let mut a = Device::new("same");
    let mut b = Device::new("same");
    a.set(Kind::PinnedTab, "t", b"from-a");
    b.set(Kind::PinnedTab, "t", b"from-b");
    a.sync(&backend).await;
    b.sync(&backend).await;
    a.sync(&backend).await;
    assert_eq!(a.store.items, b.store.items);
    let idle = b.sync(&backend).await;
    assert_eq!((idle.pushed, idle.applied), (0, 0));
}

#[tokio::test]
async fn stale_conditional_write_loses_then_merges_and_retries() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut z = Device::new("z");
    a.set(Kind::PinnedTab, "t", b"v1");
    a.sync(&backend).await;
    z.sync(&backend).await;
    // z edits; between z's pull and push, a's newer edit is stored.
    a.set(Kind::PinnedTab, "t", b"a-edit");
    let blob = seal_record(Kind::PinnedTab, "t", Some(b"a-edit"), 50, "a");
    z.set(Kind::PinnedTab, "t", b"z-edit");
    let faulty = Faulty::new(&backend);
    *faulty.interleave.lock() = Some((shared_key().record_key(Kind::PinnedTab, "t"), blob));
    let report = z.sync(&faulty).await;
    assert_eq!((report.conflicts, report.applied), (1, 1), "{report:?}");
    assert_eq!(
        get(&z, Kind::PinnedTab, "t").as_deref(),
        Some(&b"a-edit"[..])
    );
    a.sync(&backend).await;
    assert_eq!(a.store.items, z.store.items);

    let mut exhausted = Faulty::new(&backend);
    exhausted.always_conflict = true;
    z.set(Kind::PinnedTab, "t", b"never stored");
    assert_eq!(z.try_sync(&exhausted).await, Err(Error::Contention));
    assert_eq!(
        z.state.pending(),
        1,
        "the edit stays pending for a later run"
    );
}

#[tokio::test]
async fn one_lamport_ceiling_bounds_both_accepting_and_issuing_stamps() {
    use crate::engine::MAX_LAMPORT;
    let backend = MemoryBackend::default();
    let key = |id: &str| shared_key().record_key(Kind::PinnedTab, id);
    let at = seal_record(Kind::PinnedTab, "at", Some(b"remote"), MAX_LAMPORT, "m");
    backend.inner.lock().insert(key("at"), at.clone());
    let above = seal_record(
        Kind::PinnedTab,
        "above",
        Some(b"remote"),
        MAX_LAMPORT + 1,
        "m",
    );
    backend.inner.lock().insert(key("above"), above);

    // A accepts the record exactly at the ceiling and rejects the one above it.
    let mut a = Device::new("a");
    let report = a.sync(&backend).await;
    assert_eq!((report.applied, report.rejected), (1, 1), "{report:?}");
    assert!(a.state.exhausted());
    // A's next edit cannot be stamped: an explicit error, still pending, also after a restart.
    a.set(Kind::PinnedTab, "at", b"local edit");
    assert_eq!(a.try_sync(&backend).await, Err(Error::Exhausted));
    assert_eq!(a.state.pending(), 1);
    a.restart();
    assert_eq!((a.state.lamport, a.state.pending()), (MAX_LAMPORT, 1));
    assert_eq!(a.try_sync(&backend).await, Err(Error::Exhausted));

    // B never receives an inadmissible record: A published nothing, the share is unchanged.
    assert_eq!(backend.inner.lock().get(&key("at")), Some(&at));
    let mut b = Device::new("b");
    let report = b.sync(&backend).await;
    assert_eq!((report.applied, report.rejected), (1, 1), "{report:?}");
    assert_eq!(
        get(&b, Kind::PinnedTab, "at").as_deref(),
        Some(&b"remote"[..])
    );

    // Issuing exactly the ceiling works and that stamp is admissible on a peer.
    let other = MemoryBackend::default();
    let mut c = Device::new("c");
    c.state.lamport = MAX_LAMPORT - 1;
    c.set(Kind::PinnedTab, "t", b"last");
    c.sync(&other).await;
    c.restart();
    assert_eq!((c.state.lamport, c.state.pending()), (MAX_LAMPORT, 0));
    let mut d = Device::new("d");
    assert_eq!(d.sync(&other).await.applied, 1);
    assert_eq!(get(&d, Kind::PinnedTab, "t").as_deref(), Some(&b"last"[..]));
}

#[tokio::test]
async fn declined_records_keep_local_data_outside_the_selection() {
    let backend = MemoryBackend::default();
    let mut a = Device::new("a");
    let mut z = Device::new("z");
    a.set(Kind::Memory, "m", b"note");
    a.sync(&backend).await;
    z.sync(&backend).await;
    // a takes m out of the selection; z deletes it concurrently. Whichever tombstone wins,
    // a keeps its unselected copy.
    let body = a
        .store
        .items
        .remove(&(Kind::Memory, "m".to_owned()))
        .unwrap();
    a.store
        .unselected
        .insert((Kind::Memory, "m".to_owned()), body.clone());
    z.delete(Kind::Memory, "m");
    z.sync(&backend).await;
    a.sync(&backend).await;
    z.sync(&backend).await;
    a.sync(&backend).await;
    assert_eq!(
        a.store.unselected.get(&(Kind::Memory, "m".to_owned())),
        Some(&body)
    );
    assert_eq!(get(&z, Kind::Memory, "m"), None);
    // A later remote edit of the same id is answered with a deletion, not applied.
    z.set(Kind::Memory, "m", b"revived");
    z.state.lamport += 5;
    z.sync(&backend).await;
    let report = a.sync(&backend).await;
    assert_eq!(report.conflicts, 1);
    z.sync(&backend).await;
    assert_eq!(get(&z, Kind::Memory, "m"), None);
    assert_eq!(
        a.store.unselected.get(&(Kind::Memory, "m".to_owned())),
        Some(&body)
    );
}

#[test]
fn multistatus_parsing_handles_real_shaped_responses() {
    let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
 <d:response><d:href>/remote.php/dav/files/u/openside/acc/</d:href>
  <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>/remote.php/dav/files/u/openside/acc/0123456789abcdef0123456789abcdef.blob</d:href>
  <d:propstat><d:prop><d:getetag xmlns:x="urn:x">&quot;5f1e&quot;</d:getetag></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><oc:size/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
 </d:response>
 <D:response xmlns:D="DAV:"><D:href>/x/fedcba9876543210fedcba9876543210.blob</D:href>
  <D:propstat><D:prop><D:getetag/></D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
 </D:response>
</d:multistatus>"#;
    assert_eq!(
        crate::backend::multistatus(xml).unwrap(),
        vec![(
            "/remote.php/dav/files/u/openside/acc/0123456789abcdef0123456789abcdef.blob".to_owned(),
            "5f1e".to_owned()
        )]
    );
    let weak = xml.replace("&quot;5f1e&quot;", "W/&quot;5f1e&quot;");
    assert!(matches!(
        crate::backend::multistatus(&weak),
        Err(Error::Backend(_))
    ));
}

#[derive(Clone, Debug)]
enum Op {
    Set(bool, Kind, u8, u8),
    Delete(bool, Kind, u8),
    Sync(bool),
    Lock(bool, bool),
    /// Sync that dies after storing this many blobs, then restarts from the checkpoint.
    Crash(bool, u8),
}

fn op() -> impl Strategy<Value = Op> {
    let kind = prop_oneof![
        Just(Kind::PinnedTab),
        Just(Kind::VaultItem),
        Just(Kind::Setting),
        Just(Kind::Memory)
    ];
    prop_oneof![
        (any::<bool>(), kind.clone(), 0u8..3, any::<u8>())
            .prop_map(|(d, k, i, v)| Op::Set(d, k, i, v)),
        (any::<bool>(), kind, 0u8..3).prop_map(|(d, k, i)| Op::Delete(d, k, i)),
        any::<bool>().prop_map(Op::Sync),
        (any::<bool>(), any::<bool>()).prop_map(|(d, l)| Op::Lock(d, l)),
        (any::<bool>(), 0u8..3).prop_map(|(d, n)| Op::Crash(d, n)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    /// Any interleaving of edits, deletions, vault locking, syncs and syncs that crash after
    /// a partial upload (restarting from the last checkpoint) on two devices ends
    /// in the same state on both once each has synced with the vault unlocked, and a
    /// further sync changes nothing.
    #[test]
    fn any_interleaving_converges(ops in proptest::collection::vec(op(), 0..48)) {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            let backend = MemoryBackend::default();
            let mut devices = [Device::new("a"), Device::new("b")];
            for op in ops {
                match op {
                    Op::Set(d, kind, id, value) => {
                        let device = &mut devices[usize::from(d)];
                        if device.store.available(kind) {
                            device.set(kind, &id.to_string(), &[value]);
                        }
                    }
                    Op::Delete(d, kind, id) => {
                        let device = &mut devices[usize::from(d)];
                        if device.store.available(kind) {
                            device.delete(kind, &id.to_string());
                        }
                    }
                    Op::Sync(d) => {
                        devices[usize::from(d)].sync(&backend).await;
                    }
                    Op::Lock(d, locked) => devices[usize::from(d)].store.vault_locked = locked,
                    Op::Crash(d, puts) => {
                        let faulty = Faulty::new(&backend);
                        *faulty.crash_after_puts.lock() = Some(usize::from(puts));
                        let device = &mut devices[usize::from(d)];
                        let _ = device.try_sync(&faulty).await;
                        device.restart();
                    }
                }
            }
            for device in &mut devices {
                device.store.vault_locked = false;
            }
            for _ in 0..2 {
                for device in &mut devices {
                    device.sync(&backend).await;
                }
            }
            prop_assert_eq!(&devices[0].store.items, &devices[1].store.items);
            for device in &mut devices {
                let report = device.sync(&backend).await;
                prop_assert_eq!((report.pushed, report.applied), (0, 0));
                prop_assert_eq!(device.state.pending(), 0);
            }
            let stamps = |d: &Device| d.state.records.values().map(|m| m.stamp.clone()).collect::<Vec<_>>();
            prop_assert_eq!(stamps(&devices[0]), stamps(&devices[1]));
            Ok(())
        })?;
    }
}
