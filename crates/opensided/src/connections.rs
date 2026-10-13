use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
    time::Instant,
};

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use openside_browser::CookieKey;
use openside_config::data_layout::DataPath;
use openside_proto::*;
use rusqlite::{params, OptionalExtension};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::persistence::Result;

pub(crate) mod api;
pub(crate) mod navigation;
pub(crate) mod spaces;

pub(crate) struct Service {
    store: parking_lot::Mutex<Store>,
    pub(crate) gate: tokio::sync::Mutex<()>,
    last_sweep: parking_lot::Mutex<Option<Instant>>,
    pending: tokio::sync::Mutex<BTreeMap<(SessionId, String), (Instant, LoginSnapshot)>>,
    pub(crate) navigation: navigation::Navigation,
}

pub(crate) struct Record<'a> {
    pub site: &'a str,
    pub keys: &'a [CookieKey],
    pub source: ConnectionSource,
    pub status: ConnectionStatus,
    pub identity: Option<&'a str>,
    pub identity_source: ConnectionIdentitySource,
    pub home: Option<&'a str>,
    pub session: Option<&'a SessionId>,
    pub agent: &'a str,
    pub vault_item: Option<&'a str>,
    pub account_space: Option<&'a str>,
}

struct Store {
    db: rusqlite::Connection,
    key: Zeroizing<[u8; 32]>,
    audit: std::fs::File,
}

/// PSL includes private suffixes so unrelated hosted tenants never share a connection.
pub(crate) fn site(host: &str) -> Option<String> {
    let host = host
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host == "localhost" {
        return Some(host);
    }
    psl::domain_str(&host).map(str::to_owned)
}

pub(crate) fn site_url(value: &str) -> Option<(String, String)> {
    let mut url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let site = site(url.host_str()?)?;
    // Callback queries and fragments commonly carry tokens; they are never persisted.
    url.set_query(None);
    url.set_fragment(None);
    Some((site, url.into()))
}

fn wire<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .ok_or("invalid connection enum")?
        .to_owned())
}

fn mask(identity: &str) -> Option<String> {
    let text = identity.trim();
    if text.is_empty() {
        return None;
    }
    let first = text.chars().next()?;
    if let Some((_, domain)) = text.rsplit_once('@') {
        let domain = url::Host::parse(domain).ok()?.to_string();
        Some(format!("{first}•••@{domain}"))
    } else {
        Some(format!("{first}•••"))
    }
}

fn identity_key(key: &[u8], identity: Option<&str>) -> Result<(String, Option<String>)> {
    let Some(identity) = identity.filter(|identity| !identity.trim().is_empty()) else {
        return Ok(("default".into(), None));
    };
    let normalized = Zeroizing::new(identity.trim().to_lowercase());
    let mut mac = Hmac::<Sha256>::new_from_slice(key)?;
    mac.update(normalized.as_bytes());
    let hash = mac.finalize().into_bytes();
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(64);
    for byte in hash {
        write!(&mut encoded, "{byte:02x}")?;
    }
    Ok((encoded, mask(identity)))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    options
}

impl Service {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let key_path = DataPath::ConnectionsKey.path(root);
        let mut key = Zeroizing::new([0u8; 32]);
        match private_options()
            .write(true)
            .create_new(true)
            .open(&key_path)
        {
            Ok(mut file) => {
                rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, key.as_mut());
                file.write_all(key.as_ref())?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut file = private_options().read(true).open(&key_path)?;
                if !file.metadata()?.is_file() || file.metadata()?.len() != 32 {
                    return Err("invalid connection identity key".into());
                }
                file.read_exact(key.as_mut())?;
            }
            Err(error) => return Err(error.into()),
        }
        let path = DataPath::Connections.path(root);
        let file = private_options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        drop(file);
        let db = rusqlite::Connection::open(path)?;
        db.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA secure_delete=ON;",
        )?;
        let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 2 {
            return Err("unsupported connections schema".into());
        }
        if version == 0 {
            db.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE connections (
                    id TEXT PRIMARY KEY, site TEXT NOT NULL,
                    profile TEXT NOT NULL CHECK(profile IN ('agent','user')),
                    account_key TEXT NOT NULL, identity_masked TEXT, identity_source TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('candidate','unverified','active','expired','needs_login')),
                    source TEXT NOT NULL, fingerprint TEXT NOT NULL CHECK(json_valid(fingerprint)),
                    favicon TEXT, home_url TEXT, vault_item_id TEXT, jar_group TEXT, account_space TEXT,
                    created_at TEXT NOT NULL, verified_at TEXT, used_at TEXT, used_by TEXT, expires_at TEXT,
                    UNIQUE(site,profile,account_key));
                CREATE TABLE connection_grants (
                    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL, access TEXT NOT NULL CHECK(access IN ('allowed','ask','denied')),
                    granted_at TEXT NOT NULL, PRIMARY KEY(connection_id,agent_id));
                CREATE TABLE dismissed_sites (site TEXT PRIMARY KEY);
                PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE connection_origins(site TEXT NOT NULL, origin TEXT NOT NULL, PRIMARY KEY(site,origin)); PRAGMA user_version=2; COMMIT;")?;
        }
        let audit = private_options()
            .append(true)
            .create(true)
            .open(DataPath::ConnectionsAudit.path(root))?;
        Ok(Self {
            store: parking_lot::Mutex::new(Store { db, key, audit }),
            gate: Default::default(),
            last_sweep: Default::default(),
            pending: Default::default(),
            navigation: Default::default(),
        })
    }

    pub(crate) fn list(&self) -> Result<Vec<Connection>> {
        self.store.lock().rows(None)
    }
    pub(crate) fn get(&self, id: &str) -> Result<Option<Connection>> {
        Ok(self.store.lock().rows(Some(id))?.pop())
    }

    pub(crate) fn shared_site(&self, site: &str) -> Result<Vec<Connection>> {
        let store = self.store.lock();
        let mut query=store.db.prepare("SELECT id FROM connections WHERE site=?1 AND profile='agent' AND account_space IS NULL ORDER BY id")?;
        let mut rows = Vec::new();
        for id in query.query_map([site], |row| row.get::<_, String>(0))? {
            if let Some(row) = store.rows(Some(&id?))?.pop() {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    pub(crate) fn allow_agent(
        &self,
        rows: &[Connection],
        agent: &str,
        session: &SessionId,
    ) -> Result<()> {
        let mut store = self.store.lock();
        let now = Utc::now().to_rfc3339();
        let tx = store.db.transaction()?;
        for row in rows {
            tx.execute("INSERT INTO connection_grants VALUES(?1,?2,'allowed',?3) ON CONFLICT(connection_id,agent_id) DO UPDATE SET access='allowed',granted_at=excluded.granted_at",
                params![row.id,agent,now])?;
        }
        tx.commit()?;
        for row in rows {
            store.audit(
                &row.id,
                &row.site,
                Some(session),
                agent,
                ConnectionAuditAction::Grant,
            )?;
        }
        Ok(())
    }

    pub(crate) fn record(&self, record: Record<'_>) -> Result<Connection> {
        let Record {
            site,
            keys,
            source,
            status,
            identity,
            identity_source,
            home,
            session,
            agent,
            vault_item,
            account_space,
        } = record;
        let mut store = self.store.lock();
        let (account_key, identity) = identity_key(store.key.as_ref(), identity)?;
        let account_key = account_space.map_or(account_key, |space| format!("space:{space}"));
        let identity_source = if identity.is_some() {
            identity_source
        } else {
            ConnectionIdentitySource::None
        };
        if matches!(source, ConnectionSource::Import | ConnectionSource::Sweep) {
            if let Some(row) = store.rows(None)?.into_iter().find(|row| {
                row.site == site
                    && row.account_space.is_none()
                    && row.status == ConnectionStatus::Active
            }) {
                return Ok(row);
            }
        }
        let mut existing: Option<String> = store
            .db
            .query_row(
                "SELECT id FROM connections WHERE site=?1 AND profile='agent' AND account_key=?2",
                params![site, account_key],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_none() && identity.is_some() && account_space.is_none() {
            existing = store.db.query_row("SELECT id FROM connections WHERE site=?1 AND profile='agent' AND account_key='default'", [site], |row| row.get(0)).optional()?;
        }
        let had_existing = existing.is_some();
        let id = existing.unwrap_or_else(|| format!("c_{:016x}", rand::random::<u64>()));
        let now = Utc::now();
        let fingerprint: Vec<_> = keys
            .iter()
            .filter(|key| self::site(&key.domain).as_deref() == Some(site))
            .collect();
        let expires = expiry(fingerprint.iter().copied());
        let home = home
            .and_then(site_url)
            .filter(|(home_site, _)| home_site == site)
            .map(|(_, url)| url);
        let tx = store.db.transaction()?;
        if had_existing && identity.is_some() {
            tx.execute(
                "UPDATE connections SET account_key=?2 WHERE id=?1 AND account_key='default'",
                params![id, account_key],
            )?;
        }
        tx.execute("INSERT INTO connections(id,site,profile,account_key,identity_masked,identity_source,status,source,fingerprint,home_url,vault_item_id,jar_group,created_at,verified_at,expires_at,account_space)
            VALUES(?1,?2,'agent',?3,?4,?5,?6,?7,?8,?9,?10,COALESCE(?14,?2),?11,?12,?13,?14)
            ON CONFLICT(site,profile,account_key) DO UPDATE SET
            identity_masked=COALESCE(excluded.identity_masked,connections.identity_masked),
            identity_source=CASE WHEN excluded.identity_masked IS NULL THEN connections.identity_source ELSE excluded.identity_source END,
            status=excluded.status,source=excluded.source,fingerprint=excluded.fingerprint,
            home_url=COALESCE(excluded.home_url,connections.home_url),vault_item_id=COALESCE(excluded.vault_item_id,connections.vault_item_id),
            verified_at=COALESCE(excluded.verified_at,connections.verified_at),expires_at=excluded.expires_at",
            params![id, site, account_key, identity, wire(&identity_source)?, wire(&status)?, wire(&source)?,
                serde_json::to_string(&fingerprint)?, home, vault_item, now.to_rfc3339(),
                (status == ConnectionStatus::Active).then(|| now.to_rfc3339()), expires.map(|value| value.to_rfc3339()), account_space])?;
        if let Some(home) = &home {
            let origin = url::Url::parse(home)?.origin().ascii_serialization();
            tx.execute(
                "INSERT OR IGNORE INTO connection_origins VALUES(?1,?2)",
                params![site, origin],
            )?;
        }
        for who in ["chief", agent] {
            if matches!(source, ConnectionSource::Import | ConnectionSource::Sweep) {
                tx.execute(
                    "INSERT OR IGNORE INTO connection_grants VALUES(?1,?2,'allowed',?3)",
                    params![id, who, now.to_rfc3339()],
                )?;
            } else {
                tx.execute("INSERT INTO connection_grants VALUES(?1,?2,'allowed',?3) ON CONFLICT(connection_id,agent_id) DO UPDATE SET access='allowed',granted_at=excluded.granted_at", params![id, who, now.to_rfc3339()])?;
            }
        }
        tx.commit()?;
        store.audit(&id, site, session, agent, ConnectionAuditAction::Detected)?;
        store
            .rows(Some(&id))?
            .pop()
            .ok_or_else(|| "connection disappeared".into())
    }

    pub(crate) fn sweep(&self, cookies: &[CookieKey], visited: &[String]) -> Result<()> {
        let now = Utc::now().timestamp() as f64;
        let mut store = self.store.lock();
        let rows = store.rows(None)?;
        for row in &rows {
            if row.account_space.is_some() {
                continue;
            }
            let fingerprint: String = store.db.query_row(
                "SELECT fingerprint FROM connections WHERE id=?1",
                [&row.id],
                |row| row.get(0),
            )?;
            let fingerprint: Vec<CookieKey> = serde_json::from_str(&fingerprint)?;
            if fingerprint.is_empty() {
                continue;
            }
            let live: Vec<_> = cookies
                .iter()
                .filter(|cookie| cookie.expires <= 0.0 || cookie.expires > now)
                .filter(|cookie| fingerprint.iter().any(|key| same_key(cookie, key)))
                .collect();
            let expires = expiry(live.iter().copied()).map(|value| value.to_rfc3339());
            store.db.execute(
                "UPDATE connections SET expires_at=?2 WHERE id=?1",
                params![row.id, expires],
            )?;
            if live.is_empty() && row.status != ConnectionStatus::Expired {
                store.db.execute(
                    "UPDATE connections SET status='expired' WHERE id=?1",
                    [&row.id],
                )?;
                store.audit(
                    &row.id,
                    &row.site,
                    None,
                    "chief",
                    ConnectionAuditAction::Expired,
                )?;
            }
        }
        let mut groups: BTreeMap<String, Vec<CookieKey>> = BTreeMap::new();
        for cookie in cookies
            .iter()
            .filter(|cookie| cookie.http_only && cookie.secure && cookie.expires > now)
        {
            let Some(site) = site(&cookie.domain) else {
                continue;
            };
            if rows.iter().any(|row| row.site == site) || !visited.contains(&site) {
                continue;
            }
            if store
                .db
                .query_row(
                    "SELECT 1 FROM dismissed_sites WHERE site=?1",
                    [&site],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                continue;
            }
            groups.entry(site).or_default().push(cookie.clone());
        }
        drop(store);
        for (site, keys) in groups {
            self.record(Record {
                site: &site,
                keys: &keys,
                source: ConnectionSource::Sweep,
                status: ConnectionStatus::Candidate,
                identity: None,
                identity_source: ConnectionIdentitySource::None,
                home: None,
                session: None,
                agent: "chief",
                vault_item: None,
                account_space: None,
            })?;
        }
        Ok(())
    }

    pub(crate) fn update(&self, id: &str, request: UpdateConnectionRequest) -> Result<()> {
        let mut store = self.store.lock();
        let tx = store.db.transaction()?;
        if let Some(grants) = request.grants {
            for grant in grants {
                tx.execute("INSERT INTO connection_grants VALUES(?1,?2,?3,?4) ON CONFLICT(connection_id,agent_id) DO UPDATE SET access=excluded.access,granted_at=excluded.granted_at", params![id, grant.agent_id, wire(&grant.access)?, Utc::now().to_rfc3339()])?;
            }
        }
        if let Some(home) = request.home_url {
            tx.execute(
                "UPDATE connections SET home_url=?2 WHERE id=?1",
                params![id, home],
            )?;
        }
        tx.commit()?;
        let site: String =
            store
                .db
                .query_row("SELECT site FROM connections WHERE id=?1", [id], |row| {
                    row.get(0)
                })?;
        store.audit(id, &site, None, "chief", ConnectionAuditAction::Grant)
    }

    pub(crate) fn remove(&self, row: &Connection, sign_out: bool, dismiss: bool) -> Result<()> {
        let mut store = self.store.lock();
        let tx = store.db.transaction()?;
        if dismiss || !sign_out {
            tx.execute(
                "INSERT OR IGNORE INTO dismissed_sites VALUES(?1)",
                [&row.site],
            )?;
        }
        tx.execute("DELETE FROM connections WHERE id=?1", [&row.id])?;
        tx.commit()?;
        store.audit(
            &row.id,
            &row.site,
            None,
            "chief",
            if sign_out {
                ConnectionAuditAction::SignOut
            } else {
                ConnectionAuditAction::Forget
            },
        )
    }

    pub(crate) fn mark(&self, row: &Connection, status: ConnectionStatus) -> Result<()> {
        let mut store = self.store.lock();
        store.db.execute(
            "UPDATE connections SET status=?2,verified_at=?3 WHERE id=?1",
            params![row.id, wire(&status)?, Utc::now().to_rfc3339()],
        )?;
        store.audit(
            &row.id,
            &row.site,
            None,
            "chief",
            if status == ConnectionStatus::NeedsLogin {
                ConnectionAuditAction::NeedsLogin
            } else {
                ConnectionAuditAction::Verified
            },
        )
    }

    pub(crate) fn fingerprint_expired(
        &self,
        row: &Connection,
        cookies: &[CookieKey],
    ) -> Result<bool> {
        let store = self.store.lock();
        let raw: String = store.db.query_row(
            "SELECT fingerprint FROM connections WHERE id=?1",
            [&row.id],
            |row| row.get(0),
        )?;
        let fingerprint: Vec<CookieKey> = serde_json::from_str(&raw)?;
        let now = Utc::now().timestamp() as f64;
        Ok(!fingerprint.is_empty()
            && !cookies.iter().any(|cookie| {
                (cookie.expires <= 0.0 || cookie.expires > now)
                    && fingerprint.iter().any(|key| same_key(cookie, key))
            }))
    }

    pub(crate) fn confirm_identity(&self, row: &Connection, identity: &str) -> Result<()> {
        let store = self.store.lock();
        let (key, masked) = identity_key(store.key.as_ref(), Some(identity))?;
        // Separate account spaces keep their stable account key when their label changes.
        if row.account_space.is_some() {
            store.db.execute(
                "UPDATE connections SET identity_masked=?2,identity_source='agent' WHERE id=?1",
                params![row.id, masked],
            )?;
        } else {
            store.db.execute("UPDATE connections SET account_key=?2,identity_masked=?3,identity_source='agent' WHERE id=?1", params![row.id, key, masked])?;
        }
        Ok(())
    }

    pub(crate) fn record_use(
        &self,
        row: &Connection,
        session: &SessionId,
        agent: &str,
        action: ConnectionAuditAction,
        detail: Option<&str>,
    ) -> Result<()> {
        let mut store = self.store.lock();
        let at = Utc::now();
        store.db.execute(
            "UPDATE connections SET used_at=?2,used_by=?3 WHERE id=?1",
            params![row.id, at.to_rfc3339(), agent],
        )?;
        let record = ConnectionAuditRecord {
            at,
            connection_id: row.id.clone(),
            site: row.site.clone(),
            agent_id: agent.into(),
            session_id: Some(session.clone()),
            action,
            detail: detail.map(str::to_owned),
        };
        serde_json::to_writer(&mut store.audit, &record)?;
        store.audit.write_all(b"\n")?;
        store.audit.sync_data()?;
        Ok(())
    }

    fn favicon(&self, id: &str, favicon: Option<String>) -> Result<()> {
        self.store.lock().db.execute(
            "UPDATE connections SET favicon=?2 WHERE id=?1",
            params![id, favicon],
        )?;
        Ok(())
    }

    pub(crate) fn remember_tracked_origin(&self, value: &str) -> Result<()> {
        if let Some((site, safe)) = site_url(value) {
            let origin = url::Url::parse(&safe)?.origin().ascii_serialization();
            self.store.lock().db.execute(
                "INSERT OR IGNORE INTO connection_origins SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM connections WHERE site=?1 AND account_space IS NULL)",
                params![site, origin],
            )?;
        }
        Ok(())
    }

    fn remember_origin(&self, value: &str) -> Result<()> {
        if let Some((site, safe)) = site_url(value) {
            let origin = url::Url::parse(&safe)?.origin().ascii_serialization();
            self.store.lock().db.execute(
                "INSERT OR IGNORE INTO connection_origins VALUES(?1,?2)",
                params![site, origin],
            )?;
        }
        Ok(())
    }

    pub(crate) fn origins(&self, site: &str) -> Result<Vec<String>> {
        let store = self.store.lock();
        let mut query = store
            .db
            .prepare("SELECT origin FROM connection_origins WHERE site=?1")?;
        let origins = query
            .query_map([site], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(origins)
    }

    pub(crate) fn identify(&self, row: &Connection, identities: &[String]) -> Result<()> {
        let mut store = self.store.lock();
        if row.account_space.is_some() {
            if let Some(identity) = identities.first() {
                let (_, masked) = identity_key(store.key.as_ref(), Some(identity))?;
                store.db.execute(
                    "UPDATE connections SET identity_masked=?2,identity_source='probe' WHERE id=?1",
                    params![row.id, masked],
                )?;
            }
            return Ok(());
        }
        for (index, identity) in identities.iter().take(16).enumerate() {
            let (key, masked) = identity_key(store.key.as_ref(), Some(identity))?;
            let existing: Option<String> = store.db.query_row("SELECT id FROM connections WHERE site=?1 AND profile='agent' AND account_key=?2", params![row.site, key], |row| row.get(0)).optional()?;
            if let Some(id) = existing {
                store.db.execute(
                    "UPDATE connections SET identity_masked=?2,identity_source='probe' WHERE id=?1",
                    params![id, masked],
                )?;
            } else if index == 0 && row.identity.is_none() {
                store.db.execute("UPDATE connections SET account_key=?2,identity_masked=?3,identity_source='probe' WHERE id=?1", params![row.id, key, masked])?;
            } else {
                let id = format!("c_{:016x}", rand::random::<u64>());
                let tx = store.db.transaction()?;
                tx.execute("INSERT INTO connections SELECT ?2,site,profile,?3,?4,'probe',status,source,fingerprint,favicon,home_url,vault_item_id,jar_group,account_space,created_at,verified_at,used_at,used_by,expires_at FROM connections WHERE id=?1", params![row.id, id, key, masked])?;
                tx.execute(
                    "INSERT INTO connection_grants VALUES(?1,'chief','allowed',?2)",
                    params![id, Utc::now().to_rfc3339()],
                )?;
                tx.commit()?;
            }
        }
        Ok(())
    }
}

fn same_key(a: &CookieKey, b: &CookieKey) -> bool {
    a.domain == b.domain && a.name == b.name && a.path == b.path
}

fn expiry<'a>(keys: impl Iterator<Item = &'a CookieKey>) -> Option<DateTime<Utc>> {
    let mut expires = None;
    for key in keys {
        if key.expires <= 0.0 {
            return None;
        }
        expires = Some(expires.map_or(key.expires as i64, |old: i64| old.max(key.expires as i64)));
    }
    expires.and_then(|expires| DateTime::from_timestamp(expires, 0))
}

impl Store {
    fn rows(&self, id: Option<&str>) -> Result<Vec<Connection>> {
        let mut query = self.db.prepare("SELECT json_object('id',id,'site',site,'profile',profile,'identity',identity_masked,
            'identity_source',identity_source,'status',status,'source',source,'favicon',favicon,'home_url',home_url,
            'vault_item_id',vault_item_id,'jar_group',jar_group,'account_space',account_space,'accounts_on_site',
            (SELECT count(*) FROM connections other WHERE other.site=connections.site AND other.profile=connections.profile),
            'created_at',created_at,'verified_at',verified_at,'used_at',used_at,'used_by',used_by,'expires_at',expires_at,
            'grants',json('[]')) FROM connections WHERE (?1 IS NULL OR id=?1) ORDER BY site,id")?;
        let strings = query
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rows = Vec::with_capacity(strings.len());
        let mut grants = self.db.prepare("SELECT agent_id,access FROM connection_grants WHERE connection_id=?1 ORDER BY agent_id")?;
        for string in strings {
            let mut row: Connection = serde_json::from_str(&string)?;
            for grant in grants.query_map([&row.id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (agent_id, access) = grant?;
                row.grants.push(ConnectionGrant {
                    agent_id,
                    access: serde_json::from_value(serde_json::Value::String(access))?,
                });
            }
            rows.push(row);
        }
        Ok(rows)
    }

    fn audit(
        &mut self,
        id: &str,
        site: &str,
        session: Option<&SessionId>,
        agent: &str,
        action: ConnectionAuditAction,
    ) -> Result<()> {
        let record = ConnectionAuditRecord {
            at: Utc::now(),
            connection_id: id.into(),
            site: site.into(),
            agent_id: agent.into(),
            session_id: session.cloned(),
            action,
            detail: None,
        };
        serde_json::to_writer(&mut self.audit, &record)?;
        self.audit.write_all(b"\n")?;
        self.audit.sync_data()?;
        Ok(())
    }
}

pub(crate) struct LoginSnapshot {
    site: String,
    baseline: openside_browser::CookieBaseline,
    vault_item_id: Option<String>,
}

pub(crate) async fn before_login(
    shared: &crate::engine::Shared,
    browser: &openside_browser::Browser,
    session: &SessionId,
    origin: &str,
) -> Result<Option<LoginSnapshot>> {
    let state = shared.db.state(session).await?;
    if state.incognito || state.browser_space != BrowserSpace::Shared {
        return Ok(None);
    }
    let Some((site, _)) = site_url(origin) else {
        return Ok(None);
    };
    shared.connections.remember_origin(origin)?;
    let baseline = browser.cookie_baseline().await?;
    Ok(Some(LoginSnapshot {
        site,
        baseline,
        vault_item_id: None,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn after_login(
    shared: &crate::engine::Shared,
    browser: &openside_browser::Browser,
    session: &SessionId,
    tab_id: &str,
    before: Option<LoginSnapshot>,
    source: ConnectionSource,
    identity: Option<&str>,
    vault_item_id: Option<&str>,
) -> Result<()> {
    let Some(before) = before else {
        return Ok(());
    };
    let _gate = shared.connections.gate.lock().await;
    let state = shared.db.state(session).await?;
    if state.incognito || state.browser_space != BrowserSpace::Shared {
        return Ok(());
    }
    let tab = browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.id == tab_id)
        .ok_or("connection sign-in tab missing")?;
    let Some((site, home)) = site_url(&tab.url) else {
        return Ok(());
    };
    if site != before.site {
        return Ok(());
    }
    let account_space = shared.registry.browser.account_space_for_tab(tab_id).await;
    let snapshot = browser.snapshot(tab_id).await?;
    let (kind, _) = crate::browser_state::classify(&snapshot.nodes, &tab.url, &tab.title);
    if matches!(
        kind,
        PageKind::Login | PageKind::Otp | PageKind::Captcha | PageKind::Error
    ) {
        return Ok(());
    }
    let mut keys: Vec<_> = browser
        .changed_cookie_keys(&before.baseline)
        .await?
        .into_iter()
        .filter(|key| self::site(&key.domain).as_deref() == Some(site.as_str()))
        .collect();
    if keys.iter().any(|key| key.http_only && key.secure) {
        keys.retain(|key| key.http_only && key.secure);
    }
    if keys.is_empty() {
        let store = shared.connections.store.lock();
        let old: Option<String> = store.db.query_row("SELECT fingerprint FROM connections WHERE site=?1 AND profile='agent' AND account_space IS ?2 ORDER BY created_at LIMIT 1", params![site, account_space], |row| row.get(0)).optional()?;
        if let Some(old) = old {
            keys = serde_json::from_str(&old)?;
        }
    }
    if keys.is_empty() {
        keys = browser
            .site_cookie_keys()
            .await?
            .into_iter()
            .filter(|key| {
                self::site(&key.domain).as_deref() == Some(site.as_str())
                    && key.http_only
                    && key.secure
                    && key.expires > Utc::now().timestamp() as f64
            })
            .collect();
    }
    let probed = if identity.is_none() {
        probe(browser, session.as_str(), tab_id).await.ok()
    } else {
        None
    };
    let identity_source = if probed
        .as_ref()
        .is_some_and(|probe| !probe.identities.is_empty())
    {
        ConnectionIdentitySource::Probe
    } else {
        match source {
            ConnectionSource::Vault => ConnectionIdentitySource::Vault,
            ConnectionSource::Login => ConnectionIdentitySource::LoginTicket,
            _ => ConnectionIdentitySource::None,
        }
    };
    let identity = identity.or_else(|| {
        probed
            .as_ref()
            .and_then(|probe| probe.identities.first().map(String::as_str))
    });
    let row = shared.connections.record(Record {
        site: &site,
        keys: &keys,
        source,
        status: ConnectionStatus::Active,
        identity,
        identity_source,
        home: Some(&home),
        session: Some(session),
        agent: state.agent_id.as_deref().unwrap_or("chief"),
        vault_item: vault_item_id.or(before.vault_item_id.as_deref()),
        account_space: account_space.as_deref(),
    })?;
    if let Some(probe) = probed.filter(|_| account_space.is_none()) {
        shared.connections.identify(&row, &probe.identities)?;
    }
    if let Ok(icon) = browser.favicon(session.as_str(), tab_id).await {
        shared.connections.favicon(&row.id, icon)?;
    }
    let row = shared
        .connections
        .get(&row.id)?
        .ok_or("connection disappeared")?;
    shared
        .emit(
            session,
            None,
            EventKind::ConnectionSaved {
                connection_id: row.id,
                site: row.site,
                identity: row.identity,
            },
        )
        .await?;
    Ok(())
}

pub(crate) async fn imported<'a>(
    shared: &crate::engine::Shared,
    imported: impl IntoIterator<Item = (&'a str, &'a str, &'a str)> + Send,
) -> Result<()> {
    let imported: std::collections::BTreeSet<_> = imported.into_iter().collect();
    if imported.is_empty() {
        return Ok(());
    }
    let _gate = shared.connections.gate.lock().await;
    let browser = shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|_| "connection browser unavailable")?;
    let now = Utc::now().timestamp() as f64;
    let cookies = browser.site_cookie_keys().await?;
    let mut groups: BTreeMap<String, Vec<CookieKey>> = BTreeMap::new();
    for key in cookies
        .into_iter()
        .filter(|key| key.http_only && key.expires > now)
    {
        if !imported.contains(&(key.domain.as_str(), key.name.as_str(), key.path.as_str())) {
            continue;
        }
        if let Some(site) = site(&key.domain) {
            groups.entry(site).or_default().push(key);
        }
    }
    for (site, keys) in groups {
        shared.connections.record(Record {
            site: &site,
            keys: &keys,
            source: ConnectionSource::Import,
            status: ConnectionStatus::Unverified,
            identity: None,
            identity_source: ConnectionIdentitySource::None,
            home: None,
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: None,
        })?;
    }
    Ok(())
}

pub(crate) async fn begin_login(
    shared: &crate::engine::Shared,
    browser: &openside_browser::Browser,
    session: &SessionId,
    tab: &str,
    origin: &str,
) -> Result<()> {
    let mut pending = shared.connections.pending.lock().await;
    pending.retain(|_, (at, _)| at.elapsed() < std::time::Duration::from_secs(600));
    let key = (session.clone(), tab.to_owned());
    if let std::collections::btree_map::Entry::Vacant(entry) = pending.entry(key) {
        if let Some(snapshot) = before_login(shared, browser, session, origin).await? {
            entry.insert((Instant::now(), snapshot));
        }
    }
    Ok(())
}

pub(crate) async fn complete_login(
    shared: &crate::engine::Shared,
    browser: &openside_browser::Browser,
    session: &SessionId,
    tab: &str,
    source: ConnectionSource,
    vault_item_id: Option<&str>,
) -> Result<()> {
    let current = browser
        .tabs()
        .await
        .into_iter()
        .find(|current| current.id == tab)
        .ok_or("connection sign-in tab missing")?;
    let snapshot = browser.snapshot(tab).await?;
    let (kind, _) = crate::browser_state::classify(&snapshot.nodes, &current.url, &current.title);
    if matches!(
        kind,
        PageKind::Login | PageKind::Otp | PageKind::Captcha | PageKind::Error
    ) {
        return Ok(());
    }
    let snapshot = shared
        .connections
        .pending
        .lock()
        .await
        .remove(&(session.clone(), tab.to_owned()))
        .filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(600))
        .map(|(_, snapshot)| snapshot);
    after_login(
        shared,
        browser,
        session,
        tab,
        snapshot,
        source,
        None,
        vault_item_id,
    )
    .await
}

pub(crate) async fn remember_login_item(
    shared: &crate::engine::Shared,
    session: &SessionId,
    tab: &str,
    item: &str,
) {
    if let Some((_, snapshot)) = shared
        .connections
        .pending
        .lock()
        .await
        .get_mut(&(session.clone(), tab.to_owned()))
    {
        snapshot.vault_item_id = Some(item.to_owned());
    }
}

struct Probe {
    signed_in: bool,
    identities: Zeroizing<Vec<String>>,
}

async fn probe(browser: &openside_browser::Browser, owner: &str, tab: &str) -> Result<Probe> {
    let mut value = browser.evaluate_isolated(owner, tab, r#"(() => {
        const controls = [...document.querySelectorAll('button,a,[role="button"],[role="link"]')];
        const name = e => (e.getAttribute('aria-label') || e.textContent || '').trim().slice(0,512);
        const signed_in = controls.some(e => /^(sign out|log out|logout)\b/i.test(name(e)));
        const landmarks = [...document.querySelectorAll('header,nav,[role="banner"],[role="navigation"]')];
        const candidates = [...landmarks, ...controls.filter(e => /account|profile|signed in as/i.test(name(e)))];
        const identities = [...new Set(candidates.flatMap(e => name(e).match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/ig) || []))].slice(0,16);
        return {signed_in, identities};
    })()"#).await.map_err(|_| "connection identity probe unavailable")?;
    let identities: Vec<String> = serde_json::from_value(value["identities"].take())?;
    Ok(Probe {
        signed_in: value["signed_in"] == true,
        identities: Zeroizing::new(identities),
    })
}

pub(crate) async fn refresh(shared: &crate::engine::Shared) -> Result<()> {
    let Ok(browser) = shared.registry.browser.existing().await else {
        return Ok(());
    };
    let _browser_gate = shared.registry.browser.gate.lock().await;
    let _gate = shared.connections.gate.lock().await;
    if shared
        .connections
        .last_sweep
        .lock()
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(60))
    {
        return Ok(());
    }
    let cookies = browser.site_cookie_keys().await?;
    for tab in browser
        .tabs()
        .await
        .into_iter()
        .filter(|tab| tab.browser_context_id.is_none())
    {
        shared.connections.remember_origin(&tab.url)?;
    }
    let sites: std::collections::BTreeSet<_> = cookies
        .iter()
        .filter(|key| key.http_only && key.secure && key.expires > Utc::now().timestamp() as f64)
        .filter_map(|key| site(&key.domain))
        .collect();
    let mut visited = Vec::new();
    for site in sites {
        let query = site.clone();
        let history = shared
            .memory
            .run(move |memory| memory.search(&query, Some(MemoryKind::History), 100))
            .await?;
        for hit in &history.hits {
            if let Some(url) = hit
                .memory
                .url
                .as_deref()
                .filter(|url| site_url(url).is_some_and(|(value, _)| value == site))
            {
                shared.connections.remember_origin(url)?;
            }
        }
        if history.hits.iter().any(|hit| {
            hit.memory
                .url
                .as_deref()
                .and_then(site_url)
                .is_some_and(|(value, _)| value == site)
        }) {
            visited.push(site);
        }
    }
    shared.connections.sweep(&cookies, &visited)?;
    *shared.connections.last_sweep.lock() = Some(Instant::now());
    Ok(())
}

pub(crate) fn start(shared: &crate::engine::Shared) {
    navigation::Navigation::start(shared);
    let shared = shared.clone();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut cleanup = tokio::time::interval(std::time::Duration::from_secs(30));
        cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = cleanup.tick() => {
                    shared.connections.pending.lock().await.retain(|_, (at, _)| at.elapsed() < std::time::Duration::from_secs(600));
                    shared.connections.navigation.expire();
                }
                _ = interval.tick() => { let _ = refresh(&shared).await; }
            }
        }
    });
}
