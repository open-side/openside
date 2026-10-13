use std::{path::Path, time::Duration};

use parking_lot::{Mutex, MutexGuard};

use chrono::{DateTime, SecondsFormat, Utc};
use openside_proto::{
    CreateRoutineRequest, MessageId, Routine, RoutineAction, RoutinePauseReason, RoutineSchedule,
    RunRecord, RunStatus, SessionId,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{Error, Result};

const ROUTINE_COLUMNS: &str =
    "id, name, schedule, action, enabled, paused_reason, catch_up, created_at, next_run_at";
const RUN_COLUMNS: &str =
    "id, routine_id, scheduled_at, started_at, finished_at, status, session_id, message_id, detail";

/// SQLite persistence for routines and their run history.
///
/// Every operation is a short synchronous statement; the connection is never held
/// across an await point by this crate.
pub struct Store(Mutex<Connection>);

fn text(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(index: usize, value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn parse_optional_time(
    index: usize,
    value: Option<String>,
) -> rusqlite::Result<Option<DateTime<Utc>>> {
    value.map(|value| parse_time(index, value)).transpose()
}

fn parse_json<T: serde::de::DeserializeOwned>(index: usize, value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn status_text(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Started => "started",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::SkippedOverlap => "skipped_overlap",
        RunStatus::PausedTargetMissing => "paused_target_missing",
        RunStatus::Missed => "missed",
    }
}

fn parse_status(index: usize, value: &str) -> rusqlite::Result<RunStatus> {
    Ok(match value {
        "started" => RunStatus::Started,
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "skipped_overlap" => RunStatus::SkippedOverlap,
        "paused_target_missing" => RunStatus::PausedTargetMissing,
        "missed" => RunStatus::Missed,
        other => return Err(unknown(index, other)),
    })
}

fn unknown(index: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("unknown stored value {value:?}").into(),
    )
}

fn routine_row(row: &Row<'_>) -> rusqlite::Result<Routine> {
    let schedule: String = row.get(2)?;
    let action: String = row.get(3)?;
    let paused: Option<String> = row.get(5)?;
    let action = parse_json::<RoutineAction>(3, &action)?;
    Ok(Routine {
        id: row.get(0)?,
        name: row.get(1)?,
        schedule: parse_json::<RoutineSchedule>(2, &schedule)?,
        owner_agent_id: action.owner_agent_id().map(str::to_owned),
        action,
        enabled: row.get(4)?,
        paused_reason: paused
            .map(|value| match value.as_str() {
                "target_missing" => Ok(RoutinePauseReason::TargetMissing),
                "agent_retired" => Ok(RoutinePauseReason::AgentRetired),
                other => Err(unknown(5, other)),
            })
            .transpose()?,
        catch_up: row.get(6)?,
        created_at: parse_time(7, row.get(7)?)?,
        next_run_at: parse_optional_time(8, row.get(8)?)?,
    })
}

fn run_row(row: &Row<'_>) -> rusqlite::Result<RunRecord> {
    let status: String = row.get(5)?;
    let id: i64 = row.get(0)?;
    Ok(RunRecord {
        id: id as u64,
        routine_id: row.get(1)?,
        scheduled_at: parse_time(2, row.get(2)?)?,
        started_at: parse_optional_time(3, row.get(3)?)?,
        finished_at: parse_optional_time(4, row.get(4)?)?,
        status: parse_status(5, &status)?,
        session_id: row.get::<_, Option<String>>(6)?.map(SessionId::new),
        message_id: row.get::<_, Option<String>>(7)?.map(MessageId::new),
        detail: row.get(8)?,
    })
}

fn pause_text(reason: Option<RoutinePauseReason>) -> Option<&'static str> {
    reason.map(|reason| match reason {
        RoutinePauseReason::TargetMissing => "target_missing",
        RoutinePauseReason::AgentRetired => "agent_retired",
    })
}

impl Store {
    /// Open or create the database file; the parent directory must exist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Private in-memory database for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS routines (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 schedule TEXT NOT NULL CHECK(json_valid(schedule)),
                 action TEXT NOT NULL CHECK(json_valid(action)),
                 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
                 paused_reason TEXT,
                 catch_up INTEGER NOT NULL CHECK(catch_up IN (0,1)),
                 created_at TEXT NOT NULL,
                 next_run_at TEXT
             );
             CREATE TABLE IF NOT EXISTS approved_routines (
                 ticket_id TEXT PRIMARY KEY,
                 routine_id TEXT NOT NULL UNIQUE,
                 request TEXT NOT NULL CHECK(json_valid(request))
             );
             CREATE TABLE IF NOT EXISTS runs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 routine_id TEXT NOT NULL REFERENCES routines(id) ON DELETE CASCADE,
                 scheduled_at TEXT NOT NULL,
                 started_at TEXT,
                 finished_at TEXT,
                 status TEXT NOT NULL,
                 session_id TEXT,
                 message_id TEXT,
                 detail TEXT
             );
             CREATE INDEX IF NOT EXISTS runs_by_routine ON runs(routine_id, id);
             CREATE INDEX IF NOT EXISTS runs_by_status ON runs(status);",
        )?;
        Ok(Self(Mutex::new(connection)))
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.0.lock()
    }

    /// A fresh 128-bit hexadecimal identifier from SQLite's CSPRNG.
    pub fn new_id(&self) -> Result<String> {
        Ok(self
            .connection()
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))?)
    }

    /// Receipts outlive routine deletion so replay cannot recreate a deleted approval.
    pub(crate) fn approved_routine(
        &self,
        ticket: &str,
    ) -> Result<Option<(String, CreateRoutineRequest)>> {
        Ok(self
            .connection()
            .query_row(
                "SELECT routine_id,request FROM approved_routines WHERE ticket_id=?1",
                [ticket],
                |row| {
                    let request: String = row.get(1)?;
                    Ok((row.get(0)?, parse_json(1, &request)?))
                },
            )
            .optional()?)
    }

    pub(crate) fn insert_approved_routine(
        &self,
        ticket: &str,
        request: &str,
        routine: &Routine,
    ) -> Result<()> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO routines(id,name,schedule,action,enabled,paused_reason,catch_up,created_at,next_run_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                routine.id,
                routine.name,
                serde_json::to_string(&routine.schedule)?,
                serde_json::to_string(&routine.action)?,
                routine.enabled,
                pause_text(routine.paused_reason),
                routine.catch_up,
                text(routine.created_at),
                routine.next_run_at.map(text),
            ],
        )?;
        transaction.execute(
            "INSERT INTO approved_routines(ticket_id,routine_id,request) VALUES(?1,?2,?3)",
            params![ticket, routine.id, request],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Persist a new routine.
    pub fn insert_routine(&self, routine: &Routine) -> Result<()> {
        self.connection().execute(
            "INSERT INTO routines(id,name,schedule,action,enabled,paused_reason,catch_up,created_at,next_run_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                routine.id,
                routine.name,
                serde_json::to_string(&routine.schedule)?,
                serde_json::to_string(&routine.action)?,
                routine.enabled,
                pause_text(routine.paused_reason),
                routine.catch_up,
                text(routine.created_at),
                routine.next_run_at.map(text),
            ],
        )?;
        Ok(())
    }

    /// Replace every mutable field of an existing routine.
    pub fn save_routine(&self, routine: &Routine) -> Result<()> {
        let changed = self.connection().execute(
            "UPDATE routines SET name=?2, schedule=?3, action=?4, enabled=?5, paused_reason=?6,
                 catch_up=?7, next_run_at=?8 WHERE id=?1",
            params![
                routine.id,
                routine.name,
                serde_json::to_string(&routine.schedule)?,
                serde_json::to_string(&routine.action)?,
                routine.enabled,
                pause_text(routine.paused_reason),
                routine.catch_up,
                routine.next_run_at.map(text),
            ],
        )?;
        if changed == 0 {
            return Err(Error::UnknownRoutine(routine.id.clone()));
        }
        Ok(())
    }

    /// Update only the next occurrence, so a concurrent edit of other fields survives.
    pub fn set_next_run_at(&self, id: &str, next: Option<DateTime<Utc>>) -> Result<()> {
        let changed = self.connection().execute(
            "UPDATE routines SET next_run_at=?2 WHERE id=?1",
            params![id, next.map(text)],
        )?;
        if changed == 0 {
            return Err(Error::UnknownRoutine(id.to_owned()));
        }
        Ok(())
    }

    /// Disable a routine on the daemon's own initiative and record why.
    pub fn auto_pause(&self, id: &str, reason: RoutinePauseReason) -> Result<()> {
        let changed = self.connection().execute(
            "UPDATE routines SET enabled=0, paused_reason=?2, next_run_at=NULL WHERE id=?1",
            params![id, pause_text(Some(reason))],
        )?;
        if changed == 0 {
            return Err(Error::UnknownRoutine(id.to_owned()));
        }
        Ok(())
    }

    /// Load one routine.
    pub fn routine(&self, id: &str) -> Result<Option<Routine>> {
        let sql = format!("SELECT {ROUTINE_COLUMNS} FROM routines WHERE id=?1");
        Ok(self
            .connection()
            .query_row(&sql, [id], routine_row)
            .optional()?)
    }

    /// All routines in creation order.
    pub fn routines(&self) -> Result<Vec<Routine>> {
        let sql = format!("SELECT {ROUTINE_COLUMNS} FROM routines ORDER BY rowid");
        let connection = self.connection();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map([], routine_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Delete a routine together with its run history; false when it did not exist.
    pub fn delete_routine(&self, id: &str) -> Result<bool> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM runs WHERE routine_id=?1", [id])?;
        let removed = transaction.execute("DELETE FROM routines WHERE id=?1", [id])?;
        transaction.commit()?;
        Ok(removed > 0)
    }

    /// Append a run; the returned record carries the assigned identifier.
    pub fn insert_run(&self, run: &RunRecord) -> Result<RunRecord> {
        let connection = self.connection();
        connection.execute(
            "INSERT INTO runs(routine_id,scheduled_at,started_at,finished_at,status,session_id,message_id,detail)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                run.routine_id,
                text(run.scheduled_at),
                run.started_at.map(text),
                run.finished_at.map(text),
                status_text(run.status),
                run.session_id.as_ref().map(|id| id.as_str()),
                run.message_id.as_ref().map(|id| id.as_str()),
                run.detail,
            ],
        )?;
        let mut stored = run.clone();
        stored.id = connection.last_insert_rowid() as u64;
        Ok(stored)
    }

    /// Persist a status transition of an existing run.
    pub fn save_run(&self, run: &RunRecord) -> Result<()> {
        let changed = self.connection().execute(
            "UPDATE runs SET started_at=?2, finished_at=?3, status=?4, session_id=?5, message_id=?6, detail=?7
             WHERE id=?1",
            params![
                run.id as i64,
                run.started_at.map(text),
                run.finished_at.map(text),
                status_text(run.status),
                run.session_id.as_ref().map(|id| id.as_str()),
                run.message_id.as_ref().map(|id| id.as_str()),
                run.detail,
            ],
        )?;
        if changed == 0 {
            return Err(Error::UnknownRoutine(run.routine_id.clone()));
        }
        Ok(())
    }

    /// Newest-first history of one routine.
    pub fn runs(&self, routine_id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        let sql =
            format!("SELECT {RUN_COLUMNS} FROM runs WHERE routine_id=?1 ORDER BY id DESC LIMIT ?2");
        let connection = self.connection();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![routine_id, i64::try_from(limit).unwrap_or(i64::MAX)],
            run_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Runs still in `started`, oldest first; optionally for a single routine.
    pub fn open_runs(&self, routine_id: Option<&str>) -> Result<Vec<RunRecord>> {
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM runs WHERE status='started' AND (?1 IS NULL OR routine_id=?1) ORDER BY id"
        );
        let connection = self.connection();
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map([routine_id], run_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
