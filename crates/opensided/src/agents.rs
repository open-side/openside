use crate::{engine::Shared, persistence::Result};
use chrono::{DateTime, Utc};
use openside_proto::*;
use rusqlite::{params, OptionalExtension};
use std::path::Path;

pub(crate) mod api;

pub(crate) const LEGACY_CHIEF_NAME: &str = "Juno";

pub(crate) struct Service {
    db: parking_lot::Mutex<rusqlite::Connection>,
    pub(crate) gate: tokio::sync::Mutex<()>,
}
impl Service {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let path = openside_config::data_layout::DataPath::Agents.path(root);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
        }
        drop(options.open(&path)?);
        let db = rusqlite::Connection::open(path)?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 5 {
            return Err("unsupported agents schema".into());
        }
        if version == 0 {
            db.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE agents(id TEXT PRIMARY KEY,name TEXT NOT NULL UNIQUE COLLATE NOCASE,home_session_id TEXT NOT NULL UNIQUE,data TEXT NOT NULL CHECK(json_valid(data)));
                CREATE TABLE handoffs(id TEXT PRIMARY KEY,from_agent TEXT NOT NULL,to_agent TEXT NOT NULL,dedupe_key TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN ('open','done','failed','cancelled')),data TEXT NOT NULL CHECK(json_valid(data)));
                CREATE UNIQUE INDEX handoff_open ON handoffs(dedupe_key) WHERE status='open';
                CREATE TABLE team_proposals(key TEXT PRIMARY KEY,status TEXT NOT NULL,decided_at TEXT NOT NULL);
                CREATE TABLE agent_transfers(ticket_id TEXT PRIMARY KEY,step INTEGER NOT NULL,payload TEXT NOT NULL CHECK(json_valid(payload)));
                CREATE TABLE agent_audit(seq INTEGER PRIMARY KEY AUTOINCREMENT,agent_id TEXT NOT NULL,operation TEXT NOT NULL,at TEXT NOT NULL);
                PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            db.execute_batch("BEGIN IMMEDIATE;
                ALTER TABLE agents ADD COLUMN channel_kind TEXT;
                ALTER TABLE agents ADD COLUMN channel_account TEXT;
                UPDATE agents SET channel_kind=json_extract(data,'$.channel.kind'),channel_account=json_extract(data,'$.channel.account_id');
                CREATE UNIQUE INDEX agent_channel ON agents(channel_kind,channel_account) WHERE channel_kind IS NOT NULL;
                PRAGMA user_version=2; COMMIT;")?;
        }
        if version < 3 {
            db.execute_batch("BEGIN IMMEDIATE; ALTER TABLE team_proposals ADD COLUMN label TEXT; PRAGMA user_version=3; COMMIT;")?;
        }
        if version < 4 {
            db.execute_batch("BEGIN IMMEDIATE; ALTER TABLE team_proposals ADD COLUMN last_declined_at TEXT;
                UPDATE team_proposals SET last_declined_at=decided_at WHERE status IN ('never','not_now','declined','cleared');
                PRAGMA user_version=4; COMMIT;")?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS agent_migrations(key TEXT PRIMARY KEY);")?;
        if !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_migrations WHERE key='chief_name')",
            [],
            |row| row.get::<_, bool>(0),
        )? {
            db.execute_batch("BEGIN IMMEDIATE;")?;
            let collision = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE id='chief' AND name=?1 COLLATE BINARY) AND EXISTS(SELECT 1 FROM agents WHERE id!='chief' AND name='openside')",
                [LEGACY_CHIEF_NAME],
                |row| row.get::<_, bool>(0),
            )?;
            if collision {
                crate::diag!(
                    "Chief default-name migration skipped: another teammate is named openside"
                );
            } else {
                db.execute(
                    "UPDATE agents SET name='openside',data=json_set(data,'$.name','openside','$.initials','o','$.persona.role',CASE WHEN json_extract(data,'$.persona.role')='Your chief assistant' THEN 'Your assistant' ELSE json_extract(data,'$.persona.role') END) WHERE id='chief' AND name=?1 COLLATE BINARY",
                    [LEGACY_CHIEF_NAME],
                )?;
                db.execute("INSERT INTO agent_migrations VALUES('chief_name')", [])?;
            }
            db.execute_batch("COMMIT;")?;
        }
        Ok(Self {
            db: parking_lot::Mutex::new(db),
            gate: Default::default(),
        })
    }
    pub(crate) fn chief_name_conflict(&self) -> Result<bool> {
        Ok(self.db.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id='chief' AND name=?1 COLLATE BINARY) AND EXISTS(SELECT 1 FROM agents WHERE id!='chief' AND name='openside')",
            [LEGACY_CHIEF_NAME],
            |row| row.get(0),
        )?)
    }
    pub(crate) fn get(&self, id: &str) -> Result<Option<Agent>> {
        let raw: Option<String> = self
            .db
            .lock()
            .query_row("SELECT data FROM agents WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }
    pub(crate) fn revision(&self) -> Result<u64> {
        Ok(self
            .db
            .lock()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM agent_audit", [], |row| {
                row.get(0)
            })?)
    }
    pub(crate) fn list(&self) -> Result<Vec<Agent>> {
        let db = self.db.lock();
        let mut statement = db.prepare("SELECT data FROM agents ORDER BY id")?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub(crate) fn put(&self, agent: &Agent, operation: &str) -> Result<()> {
        validate(agent).map_err(str::to_owned)?;
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        let previous: Option<String> = tx
            .query_row("SELECT data FROM agents WHERE id=?1", [&agent.id], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(previous) = previous {
            let previous: Agent = serde_json::from_str(&previous)?;
            if previous.kind != agent.kind || previous.home_session_id != agent.home_session_id {
                return Err("agent_identity_immutable".into());
            }
        }
        if (agent.id == "chief") != (agent.kind == AgentKind::Chief)
            || agent.kind == AgentKind::Chief && agent.status != AgentStatus::Active
        {
            return Err("chief_immutable".into());
        }
        tx.execute("INSERT INTO agents(id,name,home_session_id,data,channel_kind,channel_account) VALUES(?1,?2,?3,?4,?5,?6)
            ON CONFLICT(id) DO UPDATE SET name=excluded.name,data=excluded.data,channel_kind=excluded.channel_kind,channel_account=excluded.channel_account",
            params![agent.id, agent.name, agent.home_session_id.as_str(), serde_json::to_string(agent)?, agent.channel.as_ref().map(|c| &c.kind), agent.channel.as_ref().map(|c| &c.account_id)])?;
        tx.execute(
            "INSERT INTO agent_audit(agent_id,operation,at) VALUES(?1,?2,?3)",
            params![agent.id, operation, Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn channel_owner(&self, kind: &str, account_id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .lock()
            .query_row(
                "SELECT id FROM agents WHERE channel_kind=?1 AND channel_account=?2",
                params![kind, account_id],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub(crate) fn clear_channel_binding(&self, kind: &str, account_id: &str) -> Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM agents WHERE channel_kind=?1 AND channel_account=?2",
                params![kind, account_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            tx.execute("UPDATE agents SET channel_kind=NULL,channel_account=NULL,data=json_set(data,'$.channel',NULL,'$.persona_version',json_extract(data,'$.persona_version')+1) WHERE id=?1", [&id])?;
            tx.execute(
                "INSERT INTO agent_audit(agent_id,operation,at) VALUES(?1,'channel_cleared',?2)",
                params![id, Utc::now().to_rfc3339()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn get_transfer(&self, key: &str) -> Result<Option<(i64, serde_json::Value)>> {
        let row: Option<(i64, String)> = self
            .db
            .lock()
            .query_row(
                "SELECT step,payload FROM agent_transfers WHERE ticket_id=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(step, raw)| Ok((step, serde_json::from_str(&raw)?)))
            .transpose()
    }
    pub(crate) fn put_transfer(
        &self,
        key: &str,
        step: i64,
        payload: &serde_json::Value,
    ) -> Result<()> {
        self.db.lock().execute("INSERT INTO agent_transfers VALUES(?1,?2,?3) ON CONFLICT(ticket_id) DO UPDATE SET step=excluded.step,payload=excluded.payload", params![key,step,serde_json::to_string(payload)?])?;
        Ok(())
    }
    pub(crate) fn list_transfers(&self) -> Result<Vec<(String, i64, serde_json::Value)>> {
        let db = self.db.lock();
        let mut st =
            db.prepare("SELECT ticket_id,step,payload FROM agent_transfers ORDER BY ticket_id")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (key, step, raw) = row?;
            Ok((key, step, serde_json::from_str(&raw)?))
        })
        .collect()
    }
    pub(crate) fn get_proposal(&self, key: &str) -> Result<Option<(String, DateTime<Utc>)>> {
        let row: Option<(String, String)> = self
            .db
            .lock()
            .query_row(
                "SELECT status,decided_at FROM team_proposals WHERE key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(status, at)| Ok((status, at.parse()?)))
            .transpose()
    }
    pub(crate) fn put_proposal(&self, key: &str, status: &str, at: DateTime<Utc>) -> Result<()> {
        self.db.lock().execute("INSERT INTO team_proposals(key,status,decided_at,last_declined_at) VALUES(?1,?2,?3,CASE WHEN ?2 IN ('never','not_now','declined') THEN ?3 ELSE NULL END) ON CONFLICT(key) DO UPDATE SET status=excluded.status,decided_at=excluded.decided_at,last_declined_at=COALESCE(excluded.last_declined_at,team_proposals.last_declined_at)", params![key,status,at.to_rfc3339()])?;
        Ok(())
    }
    pub(crate) fn suppress_proposal(
        &self,
        key: &str,
        label: &str,
        at: DateTime<Utc>,
    ) -> Result<()> {
        self.db.lock().execute("INSERT INTO team_proposals(key,status,decided_at,label,last_declined_at) VALUES(?1,'never',?2,?3,?2) ON CONFLICT(key) DO UPDATE SET status='never',decided_at=excluded.decided_at,label=excluded.label,last_declined_at=excluded.last_declined_at", params![key,at.to_rfc3339(),label])?;
        Ok(())
    }
    pub(crate) fn suppressed_proposals(&self) -> Result<Vec<SuppressedProposal>> {
        let db = self.db.lock();
        let mut statement = db.prepare("SELECT key,label,status,decided_at FROM team_proposals WHERE status='never' ORDER BY decided_at DESC,key")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (key, label, status, at) = row?;
            Ok(SuppressedProposal {
                key,
                label,
                status,
                decided_at: at.parse()?,
            })
        })
        .collect()
    }
    pub(crate) fn migrate_proposal_keys(&self, aliases: &[(String, String)]) -> Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        if tx.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? >= 5 {
            return Ok(());
        }
        for (old, new) in aliases {
            if old == new {
                continue;
            }
            tx.execute("INSERT INTO team_proposals(key,status,decided_at,label,last_declined_at) SELECT ?2,status,decided_at,label,last_declined_at FROM team_proposals WHERE key=?1
                ON CONFLICT(key) DO UPDATE SET
                status=CASE WHEN team_proposals.status='never' OR excluded.status='never' THEN 'never' WHEN excluded.decided_at>team_proposals.decided_at THEN excluded.status ELSE team_proposals.status END,
                decided_at=MAX(team_proposals.decided_at,excluded.decided_at),label=COALESCE(team_proposals.label,excluded.label),
                last_declined_at=CASE WHEN team_proposals.last_declined_at IS NULL THEN excluded.last_declined_at WHEN excluded.last_declined_at IS NULL THEN team_proposals.last_declined_at ELSE MAX(team_proposals.last_declined_at,excluded.last_declined_at) END", params![old,new])?;
            tx.execute("DELETE FROM team_proposals WHERE key=?1", [old])?;
        }
        tx.pragma_update(None, "user_version", 5)?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn latest_proposal_decline(&self) -> Result<Option<DateTime<Utc>>> {
        let at: Option<String> = self.db.lock().query_row(
            "SELECT MAX(last_declined_at) FROM team_proposals",
            [],
            |row| row.get(0),
        )?;
        at.map(|at| at.parse().map_err(Into::into)).transpose()
    }
    pub(crate) fn clear_proposal(&self, key: &str) -> Result<()> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        // A tombstone prevents historical tickets from reinstating the cleared cooldown.
        let changed = tx.execute("UPDATE team_proposals SET status='cleared',decided_at=?2 WHERE key=?1 AND status='never'", params![key,Utc::now().to_rfc3339()])?;
        if changed != 0 {
            tx.execute("INSERT INTO agent_audit(agent_id,operation,at) VALUES('chief','proposal_cleared',?1)", [Utc::now().to_rfc3339()])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn get_handoff(&self, id: &str) -> Result<Option<Handoff>> {
        let raw: Option<String> = self
            .db
            .lock()
            .query_row("SELECT data FROM handoffs WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        raw.map(|raw| Ok(serde_json::from_str(&raw)?)).transpose()
    }
    pub(crate) fn list_handoffs(&self) -> Result<Vec<Handoff>> {
        let db = self.db.lock();
        let mut st = db.prepare("SELECT data FROM handoffs ORDER BY id")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub(crate) fn put_handoff(&self, handoff: &Handoff) -> Result<()> {
        let status = serde_json::to_value(handoff.status)?;
        self.db.lock().execute("INSERT INTO handoffs VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET status=excluded.status,data=excluded.data", params![handoff.id,handoff.from_agent,handoff.to_agent,handoff.dedupe_key,status.as_str(),serde_json::to_string(handoff)?])?;
        Ok(())
    }
}

pub(crate) fn persona(agent: &Agent) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\nRe-check sources for consequential decisions.",
        agent.name,
        agent.persona.role,
        agent.persona.voice,
        agent.persona.responsibilities.join("\n"),
        agent.persona.boundaries.join("\n")
    )
}
pub(crate) fn roster(agents: &[Agent]) -> Result<String> {
    let text = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Active)
        .map(|a| format!("{}: {}", a.name, a.persona.role))
        .collect::<Vec<_>>()
        .join("\n");
    if text.chars().count() > 2000 {
        return Err("roster_full".into());
    }
    Ok(text)
}
pub(crate) fn validate(agent: &Agent) -> std::result::Result<(), &'static str> {
    let bounded = |s: &str, max: usize| {
        !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control)
    };
    if !bounded(&agent.name, 24)
        || !bounded(&agent.initials, 3)
        || !ACCENT_COLORS.contains(&agent.color.as_str())
    {
        return Err("invalid_agent");
    }
    let p = &agent.persona;
    if !bounded(&p.role, 60)
        || p.voice.chars().count() > 100
        || p.responsibilities.len() > 6
        || p.boundaries.len() > 6
        || p.responsibilities
            .iter()
            .chain(&p.boundaries)
            .any(|s| !bounded(s, 100))
        || persona(agent).chars().count() > 1500
    {
        return Err("invalid_persona");
    }
    if agent.kind == AgentKind::Teammate {
        let first = p
            .role
            .trim()
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("");
        if p.responsibilities.is_empty()
            || ["general", "helper", "assistant"]
                .iter()
                .any(|word| first.eq_ignore_ascii_case(word))
        {
            return Err("invalid_persona");
        }
    }
    if agent.account_pool.as_ref().is_some_and(|pool| {
        pool.is_empty()
            || pool
                .iter()
                .any(|a| a.source.is_empty() || a.account_id.is_empty())
    }) {
        return Err("invalid_account_pool");
    }
    if agent.channel.as_ref().is_some_and(|c| {
        !["telegram", "slack", "discord"].contains(&c.kind.as_str())
            || !openside_config::valid_channel_id(&c.account_id)
    }) {
        return Err("invalid_agent");
    }
    Ok(())
}

pub(crate) fn for_session(
    s: &Shared,
    state: &openside_sessions::SessionState,
) -> Result<Option<Agent>> {
    state
        .agent_id
        .as_deref()
        .map(|id| s.agents.get(id)?.ok_or_else(|| "agent_not_found".into()))
        .transpose()
}

pub(crate) async fn ensure_home(s: &Shared, agent: &Agent) -> Result<()> {
    let id = agent.home_session_id.clone();
    let agent = agent.clone();
    let default = s.models.default_model();
    let event =
        s.db.store(move |store| {
            if let Some(state) = store.session(&id) {
                if state.agent_id.as_deref() != Some(agent.id.as_str()) {
                    return Err("agent_home_conflict".into());
                }
                let mode = (state.mode != agent.mode).then_some(agent.mode);
                let title = (state.title != agent.name).then(|| agent.name.clone());
                if mode.is_some() || title.is_some() {
                    return Ok(Some(store.append(
                        &id,
                        None,
                        EventKind::SessionUpdated {
                            mode,
                            title,
                            pinned: None,
                            archived: None,
                        },
                    )?));
                }
                return Ok(None);
            }
            Ok(Some(store.append(
                &id,
                None,
                EventKind::SessionCreated {
                    title: agent.name,
                    model: agent.model.or(default).unwrap_or_default(),
                    mode: agent.mode,
                    workspace: None,
                    incognito: false,
                    browser_space: BrowserSpace::Shared,
                    agent_id: Some(agent.id),
                },
            )?))
        })
        .await?;
    if let Some(event) = event {
        let _ = s.events.send(event);
    }
    Ok(())
}

pub(crate) async fn bootstrap(s: &Shared) -> Result<()> {
    let _gate = s.agents.gate.lock().await;
    let chief = match s.agents.get("chief")? {
        Some(agent) => agent,
        None => {
            let chief = Agent {
                id: "chief".into(),
                kind: AgentKind::Chief,
                name: "openside".into(),
                initials: "o".into(),
                color: ACCENT_COLORS[0].into(),
                persona: AgentPersona {
                    role: "Your assistant".into(),
                    voice: String::new(),
                    responsibilities: vec![],
                    boundaries: vec![],
                },
                skills: None,
                model: None,
                account_pool: None,
                channel: None,
                mode: s.policy.default_mode(),
                home_session_id: SessionId::new(crate::persistence::random_id()),
                status: AgentStatus::Active,
                hidden: false,
                pinned: true,
                created_at: Utc::now(),
                retired_at: None,
                persona_version: 1,
            };
            s.agents.put(&chief, "bootstrap")?;
            chief
        }
    };
    ensure_home(s, &chief).await?;
    for agent in s.agents.list()?.iter().filter(|agent| agent.id != "chief") {
        ensure_home(s, agent).await?;
    }
    Ok(())
}

pub(crate) fn prepare(
    s: &Shared,
    draft: &HireDraft,
    stable_id: &str,
) -> std::result::Result<Agent, crate::api::HttpError> {
    use crate::api::HttpError;
    use axum::http::StatusCode;
    if let Some(agent) = s.agents.get(stable_id)? {
        return Ok(agent);
    }
    let existing = s.agents.list()?;
    if existing
        .iter()
        .filter(|a| a.kind == AgentKind::Teammate && a.status == AgentStatus::Active)
        .count()
        >= s.config.loaded.config.team.max_active_teammates
    {
        return Err(HttpError(StatusCode::CONFLICT, "roster_full"));
    }
    let normalized_name = draft.name.to_lowercase();
    if existing
        .iter()
        .any(|a| a.name.to_lowercase() == normalized_name)
    {
        return Err(HttpError(StatusCode::CONFLICT, "name_taken"));
    }
    let chief = existing
        .iter()
        .find(|agent| agent.id == "chief")
        .ok_or_else(HttpError::internal)?;
    let default = s.policy.default_mode();
    let mode = draft.mode.unwrap_or_else(|| {
        if rank(chief.mode) < rank(default) {
            chief.mode
        } else {
            default
        }
    });
    if rank(mode) > rank(chief.mode) || rank(mode) > rank(default) {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
    let agent = Agent {
        id: stable_id.into(),
        kind: AgentKind::Teammate,
        name: draft.name.clone(),
        initials: draft
            .initials
            .clone()
            .unwrap_or_else(|| draft.name.chars().take(1).collect()),
        color: draft.color.clone().unwrap_or_else(|| {
            ACCENT_COLORS[stable_id
                .bytes()
                .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
                % 8]
            .into()
        }),
        persona: draft.persona.clone(),
        skills: draft.skills.clone(),
        model: draft.model.clone(),
        account_pool: draft.account_pool.clone(),
        channel: draft.channel.clone(),
        mode,
        home_session_id: SessionId::new(crate::persistence::random_id()),
        status: AgentStatus::Active,
        hidden: false,
        pinned: false,
        created_at: Utc::now(),
        retired_at: None,
        persona_version: 1,
    };
    validate(&agent).map_err(|code| HttpError(StatusCode::BAD_REQUEST, code))?;
    api::validate_channel(s, &agent)?;
    Ok(agent)
}

pub(crate) async fn create(
    s: &Shared,
    draft: &HireDraft,
    stable_id: &str,
) -> std::result::Result<Agent, crate::api::HttpError> {
    if let Some(agent) = s.agents.get(stable_id)? {
        ensure_home(s, &agent).await?;
        return Ok(agent);
    }
    let agent = prepare(s, draft, stable_id)?;
    s.agents.put(&agent, "created")?;
    ensure_home(s, &agent).await?;
    Ok(agent)
}

pub(crate) fn rank(mode: PolicyMode) -> u8 {
    match mode {
        PolicyMode::ReadOnly => 0,
        PolicyMode::Guard => 1,
        PolicyMode::Full => 2,
    }
}
pub(crate) fn ceiling(s: &Shared, agent: &Agent, requested: PolicyMode) -> Result<PolicyMode> {
    if agent.status == AgentStatus::Retired {
        return Ok(PolicyMode::ReadOnly);
    }
    let chief = s.agents.get("chief")?.ok_or("agent_not_found")?;
    Ok([requested, agent.mode, chief.mode, s.policy.default_mode()]
        .into_iter()
        .min_by_key(|mode| rank(*mode))
        .unwrap_or(PolicyMode::ReadOnly))
}
