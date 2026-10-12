//! Device-side merge: detect local edits, pull and merge remote records, push winners.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    backend::write_atomic,
    crypto::random_device_id,
    record::{Payload, PAYLOAD_VERSION},
    AccountId, Backend, Error, Kind, Pushed, Stamp, SyncKey,
};

/// Pull/push rounds before giving up when other devices keep winning compare-and-swap.
pub(crate) const MAX_ROUNDS: usize = 8;
/// Highest Lamport counter accepted from other devices and issued by this one; one bound for
/// both, so every stamp a device publishes is admissible everywhere. A remote stamp above it
/// (only an authenticated but misbehaving device can produce one) is rejected. Once the next
/// stamp would exceed it, runs fail with [`Error::Exhausted`] and local edits stay pending.
pub(crate) const MAX_LAMPORT: u64 = 1 << 62;

/// Local view of one record: its winning stamp, a keyed digest of the body it
/// corresponds to (never the body itself), and the backend entity tag last seen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Record kind.
    pub kind: Kind,
    /// Record id within its kind.
    pub id: String,
    /// Current winning version.
    pub stamp: Stamp,
    /// Keyed digest of the local body for live records; `None` for tombstones.
    pub digest: Option<String>,
    /// Entity tag of the last blob seen on the backend for this record.
    pub remote_etag: Option<String>,
    /// The local version has not been stored on the backend yet.
    pub dirty: bool,
}

/// Per-device sync state persisted next to the daemon data. Contains no record bodies
/// and no key material.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientState {
    /// Account (hex).
    pub account: String,
    /// This device's id; it breaks ties between equal Lamport counters.
    pub device_id: String,
    /// Key check from the pairing code, compared after deriving the key.
    pub check: String,
    /// Highest Lamport counter seen or issued.
    pub lamport: u64,
    /// Unix seconds of the last completed sync.
    pub last_sync: Option<u64>,
    /// Records keyed by `<kind>/<id>`.
    pub records: BTreeMap<String, Meta>,
    /// Local changes the last run found but could not stamp (counter exhausted).
    #[serde(default)]
    pub unstamped: u64,
}

impl ClientState {
    /// Fresh state for this device with a random device id.
    pub fn new(key: &SyncKey) -> Result<Self, Error> {
        Ok(Self {
            account: key.account().hex(),
            device_id: random_device_id()?,
            check: crate::crypto::hex(&key.pairing_code().check),
            lamport: 0,
            last_sync: None,
            records: BTreeMap::new(),
            unstamped: 0,
        })
    }

    /// Account of this state.
    pub fn account_id(&self) -> Result<AccountId, Error> {
        AccountId::parse(&self.account)
    }

    /// Whether `key` was derived from this account's passphrase.
    pub fn accepts(&self, key: &SyncKey) -> bool {
        key.account().hex() == self.account
            && crate::crypto::hex(&key.pairing_code().check) == self.check
    }

    /// Load the state file; `None` when sync was never set up.
    pub fn load(path: &Path) -> Result<Option<Self>, Error> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| Error::Format),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Error::Io),
        }
    }

    /// Atomically replace the state file (mode 0600, parent created 0700).
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|_| Error::Io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| Error::Io)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| Error::Format)?;
        write_atomic(path, &bytes)
    }

    /// Local changes waiting to be pushed, including ones that could not be stamped.
    pub fn pending(&self) -> usize {
        self.records.values().filter(|m| m.dirty).count()
            + usize::try_from(self.unstamped).unwrap_or(usize::MAX)
    }

    /// Whether the Lamport counter reached [`MAX_LAMPORT`]: no further local edit can sync.
    pub fn exhausted(&self) -> bool {
        self.lamport >= MAX_LAMPORT
    }

    fn next_stamp(&mut self, deleted: bool) -> Result<Stamp, Error> {
        self.lamport = self
            .lamport
            .checked_add(1)
            .filter(|n| *n <= MAX_LAMPORT)
            .ok_or(Error::Exhausted)?;
        Ok(Stamp {
            lamport: self.lamport,
            deleted,
            device: self.device_id.clone(),
        })
    }
}

/// The device's own data, read and written by the sync run.
pub trait LocalStore {
    /// Whether `kind` can be read and written now (a locked vault is not); unavailable
    /// kinds are neither diffed (absence is not deletion) nor applied (they wait).
    fn available(&self, kind: Kind) -> bool;
    /// Current bodies of every record of an available kind, keyed by id.
    fn snapshot(
        &mut self,
        kind: Kind,
    ) -> impl Future<Output = Result<BTreeMap<String, Zeroizing<Vec<u8>>>, Error>> + Send;
    /// Insert/replace (`Some`) or delete (`None`) one record. Deleting a missing record
    /// succeeds. [`Error::Format`] means the body was rejected and the record is skipped;
    /// [`Applied::Declined`] keeps the local data (see there).
    fn apply(
        &mut self,
        kind: Kind,
        id: &str,
        body: Option<&[u8]>,
    ) -> impl Future<Output = Result<Applied, Error>> + Send;
}

/// Outcome of [`LocalStore::apply`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The remote version is now the local one.
    Applied,
    /// The store keeps local data that is outside the synced selection (e.g. a memory whose
    /// sync tag was removed here). A remote deletion is settled without deleting it; a
    /// remote edit is answered with a newer deletion, as if the local record were gone.
    Declined,
}

/// What one run did. Counts only; record contents are never reported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Blobs stored on the backend.
    pub pushed: u32,
    /// Remote blobs authenticated and decrypted.
    pub pulled: u32,
    /// Remote versions that won and were applied locally.
    pub applied: u32,
    /// Unsynced local edits replaced by a winning remote version, plus remote versions the
    /// store declined to protect local data outside the synced selection.
    pub conflicts: u32,
    /// Remote blobs that failed authentication or validation and were not applied.
    pub rejected: u32,
    /// Remote versions of unavailable kinds left for a later run.
    pub deferred: u32,
}

fn meta_key(kind: Kind, id: &str) -> String {
    format!("{}/{id}", kind.wire())
}

/// Stamp every local change of `kind`; returns how many changes could not be stamped
/// because the counter is exhausted (they stay unrecorded, so the next run finds them again).
fn detect(
    state: &mut ClientState,
    key: &SyncKey,
    kind: Kind,
    snapshot: &BTreeMap<String, Zeroizing<Vec<u8>>>,
) -> Result<u64, Error> {
    let mut unstamped = 0;
    for (id, body) in snapshot {
        let digest = key.digest(kind, id, body);
        let entry = meta_key(kind, id);
        let unchanged = state
            .records
            .get(&entry)
            .is_some_and(|m| !m.stamp.deleted && m.digest.as_deref() == Some(digest.as_str()));
        if unchanged {
            continue;
        }
        let Ok(stamp) = state.next_stamp(false) else {
            unstamped += 1;
            continue;
        };
        let remote_etag = state
            .records
            .get(&entry)
            .and_then(|m| m.remote_etag.clone());
        state.records.insert(
            entry,
            Meta {
                kind,
                id: id.clone(),
                stamp,
                digest: Some(digest),
                remote_etag,
                dirty: true,
            },
        );
    }
    let removed: Vec<String> = state
        .records
        .iter()
        .filter(|(_, m)| m.kind == kind && !m.stamp.deleted && !snapshot.contains_key(&m.id))
        .map(|(entry, _)| entry.clone())
        .collect();
    for entry in removed {
        let Ok(stamp) = state.next_stamp(true) else {
            unstamped += 1;
            continue;
        };
        if let Some(meta) = state.records.get_mut(&entry) {
            meta.stamp = stamp;
            meta.digest = None;
            meta.dirty = true;
        }
    }
    Ok(unstamped)
}

/// Saves the state durably; called by [`sync`] before anything that depends on it
/// becomes visible elsewhere.
pub type Checkpoint<'a> = dyn FnMut(&ClientState) -> Result<(), Error> + Send + 'a;

/// One sync run: record local edits as new stamps, pull and merge, push local winners,
/// and repeat while compare-and-swap reports concurrent writers.
///
/// `checkpoint` is the write-ahead boundary: it runs after new stamps are issued, after
/// remote counters are absorbed (before applying anything) and before every push, so a
/// crash can never reissue a published stamp for different content. The caller persists
/// `state` once more afterwards (also after an error).
///
/// The caller must keep every local writer of the synced data out for the whole run, so
/// that nothing changes between a snapshot and an apply.
pub async fn sync<B: Backend, S: LocalStore + Send>(
    state: &mut ClientState,
    key: &SyncKey,
    backend: &B,
    store: &mut S,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Report, Error> {
    let mut report = Report::default();
    let mut snapshots = BTreeMap::new();
    state.unstamped = 0;
    for kind in Kind::ALL {
        if store.available(kind) {
            let snapshot = store.snapshot(kind).await?;
            state.unstamped += detect(state, key, kind, &snapshot)?;
            snapshots.insert(kind, snapshot);
        }
    }
    checkpoint(state)?;
    // Nothing is exchanged once an edit cannot be stamped: pushing the rest would publish a
    // state that silently drops it.
    if state.unstamped > 0 {
        return Err(Error::Exhausted);
    }
    let mut applied = BTreeSet::new();
    let mut settled = false;
    for _ in 0..MAX_ROUNDS {
        pull(
            state,
            key,
            backend,
            store,
            &mut snapshots,
            &mut applied,
            &mut report,
            checkpoint,
        )
        .await?;
        checkpoint(state)?;
        if push(state, key, backend, &snapshots, &mut report).await? {
            settled = true;
            break;
        }
    }
    // Digests must describe the store's own serialization of applied records, or the next
    // run would mistake a re-encoded body for a local edit. Local writers are held off for
    // the whole run, so this snapshot is exactly the applied version.
    for kind in Kind::ALL {
        if !applied.iter().any(|(k, _)| *k == kind) {
            continue;
        }
        let snapshot = store.snapshot(kind).await?;
        for (_, id) in applied.iter().filter(|(k, _)| *k == kind) {
            if let (Some(body), Some(meta)) =
                (snapshot.get(id), state.records.get_mut(&meta_key(kind, id)))
            {
                if !meta.stamp.deleted {
                    meta.digest = Some(key.digest(kind, id, body));
                }
            }
        }
    }
    if !settled {
        return Err(Error::Contention);
    }
    state.last_sync = Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    );
    Ok(report)
}

/// One authenticated, structurally valid remote version.
struct Remote {
    etag: String,
    kind: Kind,
    id: String,
    stamp: Stamp,
    body: Option<Zeroizing<Vec<u8>>>,
    digest: Option<String>,
}

/// Authenticate and validate one fetched blob; `None` means rejected.
fn decode(key: &SyncKey, record_key: &str, etag: String, blob: &[u8]) -> Option<Remote> {
    let plaintext = key.open(record_key, blob).ok()?;
    let mut payload = serde_json::from_slice::<Payload>(&plaintext).ok()?;
    // The AEAD binds the blob to its opaque key; the key must also be the one this kind and
    // id map to, or a validly sealed record is being replayed under another identity.
    if payload.v != PAYLOAD_VERSION
        || key.record_key(payload.kind, &payload.id) != record_key
        || payload.stamp.deleted != payload.body.is_none()
        || payload.stamp.lamport > MAX_LAMPORT
    {
        return None;
    }
    let body = match payload.body.take().map(Zeroizing::new) {
        Some(encoded) => Some(Zeroizing::new(STANDARD.decode(encoded.as_bytes()).ok()?)),
        None => None,
    };
    let digest = body
        .as_ref()
        .map(|b| key.digest(payload.kind, &payload.id, b));
    Some(Remote {
        etag,
        kind: payload.kind,
        id: payload.id.clone(),
        stamp: payload.stamp.clone(),
        body,
        digest,
    })
}

#[allow(clippy::too_many_arguments)]
async fn pull<B: Backend, S: LocalStore>(
    state: &mut ClientState,
    key: &SyncKey,
    backend: &B,
    store: &mut S,
    snapshots: &mut BTreeMap<Kind, BTreeMap<String, Zeroizing<Vec<u8>>>>,
    applied: &mut BTreeSet<(Kind, String)>,
    report: &mut Report,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<(), Error> {
    // Records whose blob has not changed since this device last saw it are skipped; the
    // rest (new, changed, deferred or previously rejected) are fetched and checked.
    let known: BTreeMap<String, (String, Option<String>)> = state
        .records
        .iter()
        .map(|(entry, m)| {
            (
                key.record_key(m.kind, &m.id),
                (entry.clone(), m.remote_etag.clone()),
            )
        })
        .collect();
    let listing = backend.list().await?;
    // A record this device stored that is gone from the backend (deleted by someone with
    // write access, or a restored share) is uploaded again rather than retried forever
    // against an entity tag that no longer exists.
    let listed: BTreeSet<&str> = listing.iter().map(|l| l.key.as_str()).collect();
    for (record_key, (entry, remote_etag)) in &known {
        if remote_etag.is_some() && !listed.contains(record_key.as_str()) {
            if let Some(meta) = state.records.get_mut(entry) {
                meta.remote_etag = None;
                meta.dirty = true;
            }
        }
    }
    let mut remotes = Vec::new();
    for listed in listing {
        if known
            .get(&listed.key)
            .is_some_and(|(_, tag)| tag.as_deref() == Some(listed.etag.as_str()))
        {
            continue;
        }
        let Some((blob, etag)) = backend.get(&listed.key).await? else {
            continue;
        };
        match decode(key, &listed.key, etag, &blob) {
            Some(remote) => {
                state.lamport = state.lamport.max(remote.stamp.lamport);
                remotes.push(remote);
            }
            None => report.rejected += 1,
        }
    }
    // Absorbed counters are durable before any remote version reaches the local store, so
    // a crash mid-apply makes the next run stamp what it finds above everything seen.
    checkpoint(state)?;
    for remote in remotes {
        report.pulled += 1;
        let kind = remote.kind;
        let entry = meta_key(kind, &remote.id);
        let local = state.records.get(&entry).cloned();
        // Stamps order versions; the keyed body digest breaks the (normally impossible) tie
        // of one stamp naming two bodies, so every device still picks the same winner.
        let remote_wins = local.as_ref().map_or(true, |m| {
            (&remote.stamp, &remote.digest) > (&m.stamp, &m.digest)
        });
        if !remote_wins {
            if let Some(meta) = state.records.get_mut(&entry) {
                meta.remote_etag = Some(remote.etag);
                // Equal versions mean the backend already has ours; an older remote is
                // overwritten by pushing ours again.
                meta.dirty = (&remote.stamp, &remote.digest) != (&meta.stamp, &meta.digest);
            }
            continue;
        }
        if !store.available(kind) {
            // The entity tag is not recorded, so the next run fetches it again.
            report.deferred += 1;
            continue;
        }
        let outcome = match store
            .apply(kind, &remote.id, remote.body.as_ref().map(|b| b.as_slice()))
            .await
        {
            Ok(outcome) => outcome,
            Err(Error::Format) => {
                report.rejected += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        if outcome == Applied::Declined {
            report.conflicts += 1;
            let meta = match remote.body {
                // The remote deletion is settled; the local data simply is not synced.
                None => Meta {
                    kind,
                    id: remote.id.clone(),
                    stamp: remote.stamp,
                    digest: None,
                    remote_etag: Some(remote.etag),
                    dirty: false,
                },
                // Answer the edit with a newer deletion: here the record is out of the set.
                Some(_) => Meta {
                    kind,
                    id: remote.id.clone(),
                    stamp: state.next_stamp(true)?,
                    digest: None,
                    remote_etag: Some(remote.etag),
                    dirty: true,
                },
            };
            snapshots.entry(kind).or_default().remove(&remote.id);
            state.records.insert(entry, meta);
            continue;
        }
        if local.as_ref().is_some_and(|m| m.dirty) {
            report.conflicts += 1;
        }
        report.applied += 1;
        let snapshot = snapshots.entry(kind).or_default();
        match &remote.body {
            Some(body) => snapshot.insert(remote.id.clone(), body.clone()),
            None => snapshot.remove(&remote.id),
        };
        applied.insert((kind, remote.id.clone()));
        state.records.insert(
            entry,
            Meta {
                kind,
                id: remote.id,
                stamp: remote.stamp,
                digest: remote.digest,
                remote_etag: Some(remote.etag),
                dirty: false,
            },
        );
    }
    Ok(())
}

/// Push every dirty record whose body is available. Returns false if any write lost a
/// compare-and-swap race and another pull is needed.
async fn push<B: Backend>(
    state: &mut ClientState,
    key: &SyncKey,
    backend: &B,
    snapshots: &BTreeMap<Kind, BTreeMap<String, Zeroizing<Vec<u8>>>>,
    report: &mut Report,
) -> Result<bool, Error> {
    let dirty: Vec<String> = state
        .records
        .iter()
        .filter(|(_, m)| m.dirty)
        .map(|(entry, _)| entry.clone())
        .collect();
    let mut settled = true;
    for entry in dirty {
        let Some(meta) = state.records.get(&entry).cloned() else {
            continue;
        };
        let body = if meta.stamp.deleted {
            None
        } else {
            // A live record of a kind that is unavailable now stays dirty for later.
            match snapshots.get(&meta.kind).and_then(|s| s.get(&meta.id)) {
                Some(body) => Some(STANDARD.encode(body.as_slice())),
                None => continue,
            }
        };
        let payload = Payload {
            v: PAYLOAD_VERSION,
            kind: meta.kind,
            id: meta.id.clone(),
            stamp: meta.stamp.clone(),
            body,
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&payload).map_err(|_| Error::Format)?);
        let record_key = key.record_key(meta.kind, &meta.id);
        let blob = key.seal(&record_key, &plaintext)?;
        match backend
            .put(&record_key, meta.remote_etag.as_deref(), &blob)
            .await?
        {
            Pushed::Stored(etag) => {
                if let Some(meta) = state.records.get_mut(&entry) {
                    // Without an authoritative tag the next pull fetches the record once
                    // and recognizes it as ours.
                    meta.remote_etag = etag;
                    meta.dirty = false;
                }
                report.pushed += 1;
            }
            Pushed::Conflict => settled = false,
        }
    }
    Ok(settled)
}
