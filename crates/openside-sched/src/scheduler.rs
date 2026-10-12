use std::{future::Future, path::Path, sync::Arc};

use chrono::{DateTime, Utc};
use openside_proto::{
    CreateRoutineRequest, MessageId, PolicyMode, Routine, RoutineAction, RoutineDraft,
    RoutinePauseReason, RunRecord, RunStatus, SessionId, UpdateRoutineRequest,
};

use crate::{
    schedule,
    suggest::{self, UserMessage},
    Clock, Error, Invalid, Result, Store, ROUTINE_MESSAGE_PREFIX,
};

/// Most `missed` records written for one routine in one catch-up pass.
pub const MAX_MISSED_RECORDS: usize = 50;
/// Default and maximum number of runs returned by history queries.
pub const DEFAULT_RUN_LIMIT: usize = 100;

/// Parameters of the session a `new_session` routine creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionSpec {
    /// Routine name, used as the session title.
    pub title: String,
    /// First message of the session.
    pub prompt: String,
    /// Permission mode override.
    pub mode: Option<PolicyMode>,
    /// Absolute workspace path.
    pub workspace: Option<String>,
    /// Model override.
    pub model: Option<String>,
}

/// Identity of one run's prompt, chosen and persisted by the scheduler before the host acts.
///
/// The host must use exactly these ids: a repeated call with the same ids is a no-op, and
/// the scheduler finds the work again after a crash by asking about this message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    /// Session that receives the prompt: the heartbeat target, or the session to create.
    pub session_id: SessionId,
    /// Message that carries it.
    pub message_id: MessageId,
}

/// Why the host could not deliver a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The heartbeat target session does not exist (any more).
    TargetMissing,
    /// Delivery failed for another, sanitized reason.
    Failed(String),
}

/// Where a previously dispatched message stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Still queued, running, or waiting on a human.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Failed, was aborted or dropped.
    Failed(String),
    /// The session exists but never received the message (the dispatch was interrupted).
    NotDispatched,
    /// Its session does not exist.
    Missing,
}

/// The daemon-side effects of a routine run.
pub trait RoutineHost: Send + Sync + 'static {
    /// Create the session `dispatch.session_id` and queue the prompt in it as
    /// `dispatch.message_id`.
    fn start_session(
        &self,
        spec: &NewSessionSpec,
        dispatch: &Dispatch,
    ) -> impl Future<Output = std::result::Result<(), HostError>> + Send;

    /// Queue `prompt` as `dispatch.message_id` in the existing session `dispatch.session_id`.
    fn follow_up(
        &self,
        dispatch: &Dispatch,
        prompt: &str,
    ) -> impl Future<Output = std::result::Result<(), HostError>> + Send;

    /// Report whether the message of an earlier dispatch has settled.
    fn progress(
        &self,
        session: &SessionId,
        message: &MessageId,
    ) -> impl Future<Output = Progress> + Send;

    /// Whether the session exists (is not deleted).
    fn session_exists(&self, session: &SessionId) -> impl Future<Output = bool> + Send;
}

#[derive(Clone, Copy)]
enum Trigger {
    Schedule,
    Manual,
}

/// Fires routines against a [`RoutineHost`] using an injected [`Clock`].
///
/// Scheduling is pull-based: the owner calls [`Scheduler::tick`] periodically (the daemon
/// does so every second). All firing is serialised, so a manual run and a scheduled run
/// of one routine can never both pass the overlap check.
pub struct Scheduler<H> {
    store: Store,
    clock: Arc<dyn Clock>,
    host: H,
    gate: tokio::sync::Mutex<()>,
}

fn blank(value: &str) -> bool {
    value.trim().is_empty()
}

fn validate_action(action: &RoutineAction) -> std::result::Result<(), Invalid> {
    match action {
        RoutineAction::NewSession {
            prompt, workspace, ..
        } => {
            if blank(prompt) {
                return Err(Invalid::Action("prompt must not be blank".into()));
            }
            if workspace
                .as_deref()
                .is_some_and(|path| !Path::new(path).is_absolute())
            {
                return Err(Invalid::Action("workspace must be an absolute path".into()));
            }
        }
        RoutineAction::Heartbeat { session_id, prompt } => {
            if blank(prompt) || blank(session_id.as_str()) {
                return Err(Invalid::Action(
                    "heartbeat needs a session id and a prompt".into(),
                ));
            }
        }
    }
    Ok(())
}

impl<H: RoutineHost> Scheduler<H> {
    /// Build a scheduler over an opened store.
    pub fn new(store: Store, clock: Arc<dyn Clock>, host: H) -> Self {
        Self {
            store,
            clock,
            host,
            gate: tokio::sync::Mutex::new(()),
        }
    }

    /// Open (or create) `routines.db` at `path`.
    pub fn open(path: impl AsRef<Path>, clock: Arc<dyn Clock>, host: H) -> Result<Self> {
        Ok(Self::new(Store::open(path)?, clock, host))
    }

    /// The injected host.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// A heartbeat is only meaningful for a session that exists right now. Checked while
    /// holding the gate, so a deletion either ran its hook before this (target gone: rejected)
    /// or runs it after the routine is persisted (and then pauses it).
    async fn require_target(&self, action: &RoutineAction) -> Result<()> {
        if let RoutineAction::Heartbeat { session_id, .. } = action {
            if !self.host.session_exists(session_id).await {
                return Err(Invalid::TargetMissing.into());
            }
        }
        Ok(())
    }

    /// Create a routine; an enabled one computes its first occurrence from now.
    ///
    /// Serialised with firing and with [`Scheduler::on_session_deleted`].
    pub async fn create(&self, request: CreateRoutineRequest) -> Result<Routine> {
        let _gate = self.gate.lock().await;
        if blank(&request.name) {
            return Err(Invalid::Name.into());
        }
        schedule::validate(&request.schedule, self.clock.now())?;
        validate_action(&request.action)?;
        self.require_target(&request.action).await?;
        let now = self.clock.now();
        let enabled = request.enabled.unwrap_or(true);
        let routine = Routine {
            id: self.store.new_id()?,
            name: request.name.trim().to_owned(),
            next_run_at: if enabled {
                schedule::next_after(&request.schedule, now, now)?
            } else {
                None
            },
            schedule: request.schedule,
            action: request.action,
            enabled,
            paused_reason: None,
            catch_up: request.catch_up.unwrap_or(false),
            created_at: now,
        };
        self.store.insert_routine(&routine)?;
        Ok(routine)
    }

    /// Load one routine.
    pub fn get(&self, id: &str) -> Result<Routine> {
        self.store
            .routine(id)?
            .ok_or_else(|| Error::UnknownRoutine(id.to_owned()))
    }

    /// All routines in creation order.
    pub fn list(&self) -> Result<Vec<Routine>> {
        self.store.routines()
    }

    /// Apply a partial update. Resuming clears an automatic pause and recomputes the next
    /// run from now, so time spent paused is never treated as missed.
    ///
    /// Serialised with firing: once this returns, no in-flight tick acts on the old state.
    pub async fn update(&self, id: &str, request: UpdateRoutineRequest) -> Result<Routine> {
        let _gate = self.gate.lock().await;
        let mut routine = self.get(id)?;
        let now = self.clock.now();
        let mut reschedule = false;
        if let Some(name) = request.name {
            if blank(&name) {
                return Err(Invalid::Name.into());
            }
            routine.name = name.trim().to_owned();
        }
        if let Some(new_schedule) = request.schedule {
            schedule::validate(&new_schedule, now)?;
            routine.schedule = new_schedule;
            reschedule = true;
        }
        if let Some(action) = request.action {
            validate_action(&action)?;
            self.require_target(&action).await?;
            routine.action = action;
        }
        if let Some(catch_up) = request.catch_up {
            routine.catch_up = catch_up;
        }
        if let Some(enabled) = request.enabled {
            if enabled && !routine.enabled {
                routine.paused_reason = None;
                reschedule = true;
            }
            routine.enabled = enabled;
        }
        if !routine.enabled {
            routine.next_run_at = None;
        } else if reschedule || routine.next_run_at.is_none() {
            routine.next_run_at = schedule::next_after(&routine.schedule, now, now)?;
        }
        self.store.save_routine(&routine)?;
        Ok(routine)
    }

    /// Delete a routine and its history, returning the deleted routine. Serialised with
    /// firing, so an in-flight tick cannot start it or re-create history afterwards.
    pub async fn delete(&self, id: &str) -> Result<Routine> {
        let _gate = self.gate.lock().await;
        let routine = self.get(id)?;
        self.store.delete_routine(id)?;
        Ok(routine)
    }

    /// Run history, newest first, with in-progress runs refreshed against the host.
    pub async fn runs(&self, id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        self.get(id)?;
        {
            let _gate = self.gate.lock().await;
            self.reconcile(Some(id)).await?;
        }
        self.store.runs(id, limit.clamp(1, 500))
    }

    /// Repetition candidates from `messages`, excluding prompts that already have a routine.
    pub fn suggestions(&self, messages: &[UserMessage]) -> Result<Vec<RoutineDraft>> {
        let existing: Vec<String> = self
            .store
            .routines()?
            .into_iter()
            .map(|routine| match routine.action {
                RoutineAction::NewSession { prompt, .. }
                | RoutineAction::Heartbeat { prompt, .. } => prompt,
            })
            .collect();
        Ok(suggest::suggest(messages, &existing))
    }

    /// Fire everything due at the clock's current time and settle finished runs.
    ///
    /// Several occurrences due at once (a late tick) fire only the latest; the older ones
    /// are recorded as `missed` rather than replayed in a burst.
    pub async fn tick(&self) -> Result<Vec<RunRecord>> {
        let _gate = self.gate.lock().await;
        let now = self.clock.now();
        self.reconcile(None).await?;
        let mut produced = Vec::new();
        for listed in self.store.routines()? {
            // Re-read: earlier iterations awaited the host, and the snapshot may be stale.
            let Some(routine) = self.store.routine(&listed.id)? else {
                continue;
            };
            let Some(first) = routine
                .next_run_at
                .filter(|due| routine.enabled && *due <= now)
            else {
                continue;
            };
            self.process_due(
                &routine,
                first,
                now,
                true,
                "scheduler was late; a newer occurrence ran instead",
                &mut produced,
            )
            .await?;
        }
        Ok(produced)
    }

    /// Reconcile persisted state after a daemon start.
    ///
    /// Occurrences that came due while the daemon was down are recorded as `missed` and
    /// never replayed in a burst. A routine with `catch_up` runs once for its most recent
    /// missed occurrence. Returns every record written.
    pub async fn recover(&self) -> Result<Vec<RunRecord>> {
        let _gate = self.gate.lock().await;
        let now = self.clock.now();
        self.reconcile(None).await?;
        let mut produced = Vec::new();
        self.reconcile_targets(&mut produced).await?;
        for listed in self.store.routines()? {
            let Some(routine) = self.store.routine(&listed.id)? else {
                continue;
            };
            if !routine.enabled {
                continue;
            }
            match routine.next_run_at {
                Some(first) if first <= now => {
                    self.process_due(
                        &routine,
                        first,
                        now,
                        routine.catch_up,
                        "daemon was not running at the scheduled time",
                        &mut produced,
                    )
                    .await?;
                }
                Some(_) => {}
                None => {
                    let next = schedule::next_after(&routine.schedule, now, routine.created_at)?;
                    self.store.set_next_run_at(&routine.id, next)?;
                }
            }
        }
        Ok(produced)
    }

    /// Run a routine immediately, outside its schedule. The next scheduled occurrence is
    /// unchanged. The overlap rule applies, so the returned record may be `skipped_overlap`.
    pub async fn run_now(&self, id: &str) -> Result<RunRecord> {
        let _gate = self.gate.lock().await;
        let routine = self.get(id)?;
        let now = self.clock.now();
        self.execute(&routine, now, Trigger::Manual).await
    }

    /// Hook for session deletion: pause heartbeat routines that target the session and
    /// fail runs still attached to it. Returns the `paused_target_missing` records.
    ///
    /// Serialised with firing, so it cannot interleave with a dispatch or reconciliation.
    pub async fn on_session_deleted(&self, session: &SessionId) -> Result<Vec<RunRecord>> {
        let _gate = self.gate.lock().await;
        let now = self.clock.now();
        let mut records = Vec::new();
        for routine in self.store.routines()? {
            let targets = matches!(&routine.action,
                RoutineAction::Heartbeat { session_id, .. } if session_id == session);
            if !targets || routine.paused_reason == Some(RoutinePauseReason::TargetMissing) {
                continue;
            }
            records.push(self.pause_for_missing_target(&routine, session)?);
        }
        for mut run in self.store.open_runs(None)? {
            if run.session_id.as_ref() == Some(session) {
                run.status = RunStatus::Failed;
                run.finished_at = Some(now);
                run.detail = Some("session was deleted".into());
                self.store.save_run(&run)?;
            }
        }
        Ok(records)
    }

    async fn process_due(
        &self,
        routine: &Routine,
        first: DateTime<Utc>,
        now: DateTime<Utc>,
        fire_latest: bool,
        reason: &str,
        produced: &mut Vec<RunRecord>,
    ) -> Result<()> {
        let due = schedule::due(&routine.schedule, first, now, MAX_MISSED_RECORDS)?;
        // Persist the next occurrence before executing: a crash in between loses one
        // run (at most once) instead of firing it twice.
        self.store.set_next_run_at(&routine.id, due.next)?;
        let latest_is_listed = !due.truncated;
        let missed_count = if fire_latest && latest_is_listed {
            due.listed.len() - 1
        } else {
            due.listed.len()
        };
        for (index, scheduled_at) in due.listed.iter().take(missed_count).enumerate() {
            let last = index + 1 == missed_count;
            let detail = if last && due.truncated {
                format!("{reason}; further missed occurrences were not recorded individually")
            } else {
                reason.to_owned()
            };
            produced.push(self.store.insert_run(&RunRecord {
                id: 0,
                routine_id: routine.id.clone(),
                scheduled_at: *scheduled_at,
                started_at: None,
                finished_at: Some(now),
                status: RunStatus::Missed,
                session_id: None,
                message_id: None,
                detail: Some(detail),
            })?);
        }
        if fire_latest {
            produced.push(self.execute(routine, due.latest, Trigger::Schedule).await?);
        }
        Ok(())
    }

    async fn execute(
        &self,
        routine: &Routine,
        scheduled_at: DateTime<Utc>,
        trigger: Trigger,
    ) -> Result<RunRecord> {
        self.reconcile(Some(&routine.id)).await?;
        let mut run = RunRecord {
            id: 0,
            routine_id: routine.id.clone(),
            scheduled_at,
            started_at: None,
            finished_at: None,
            status: RunStatus::Started,
            session_id: None,
            message_id: None,
            detail: matches!(trigger, Trigger::Manual).then(|| "manual run".to_owned()),
        };
        if let Some(previous) = self.store.open_runs(Some(&routine.id))?.into_iter().last() {
            run.status = RunStatus::SkippedOverlap;
            run.finished_at = Some(self.clock.now());
            run.session_id = previous.session_id;
            run.detail = Some(format!("previous run {} has not settled yet", previous.id));
            return self.store.insert_run(&run);
        }
        // The run's identity is chosen and stored before the host is touched, so a crash at
        // any later point leaves an open run that names the exact message to look for.
        let dispatch = Dispatch {
            session_id: match &routine.action {
                RoutineAction::Heartbeat { session_id, .. } => session_id.clone(),
                RoutineAction::NewSession { .. } => SessionId::new(self.store.new_id()?),
            },
            message_id: MessageId::new(format!("{ROUTINE_MESSAGE_PREFIX}{}", self.store.new_id()?)),
        };
        run.started_at = Some(self.clock.now());
        run.session_id = Some(dispatch.session_id.clone());
        run.message_id = Some(dispatch.message_id.clone());
        let mut run = self.store.insert_run(&run)?;
        let dispatched = match &routine.action {
            RoutineAction::NewSession {
                prompt,
                mode,
                workspace,
                model,
            } => {
                self.host
                    .start_session(
                        &NewSessionSpec {
                            title: routine.name.clone(),
                            prompt: prompt.clone(),
                            mode: *mode,
                            workspace: workspace.clone(),
                            model: model.clone(),
                        },
                        &dispatch,
                    )
                    .await
            }
            RoutineAction::Heartbeat { prompt, .. } => self.host.follow_up(&dispatch, prompt).await,
        };
        let error = match dispatched {
            Ok(()) => return Ok(run),
            Err(error) => error,
        };
        if let HostError::Failed(detail) = &error {
            // The host may have queued the prompt before failing (for example while starting
            // its runtime). A queued message still counts as the active run.
            if matches!(
                self.host
                    .progress(&dispatch.session_id, &dispatch.message_id)
                    .await,
                Progress::InProgress
            ) {
                run.detail = Some(format!(
                    "{detail}; the prompt is queued and will be delivered"
                ));
                self.store.save_run(&run)?;
                return Ok(run);
            }
        }
        run.finished_at = Some(self.clock.now());
        match error {
            HostError::TargetMissing => {
                run.status = RunStatus::PausedTargetMissing;
                run.message_id = None;
                run.detail = Some("target session does not exist; routine paused".into());
                self.store
                    .auto_pause(&routine.id, RoutinePauseReason::TargetMissing)?;
            }
            HostError::Failed(detail) => {
                run.status = RunStatus::Failed;
                run.message_id = None;
                if matches!(routine.action, RoutineAction::NewSession { .. }) {
                    run.session_id = None;
                }
                run.detail = Some(detail);
            }
        }
        self.store.save_run(&run)?;
        Ok(run)
    }

    /// Move `started` runs whose message has settled to a terminal status.
    async fn reconcile(&self, routine: Option<&str>) -> Result<()> {
        for mut run in self.store.open_runs(routine)? {
            let progress = match (&run.session_id, &run.message_id) {
                (Some(session), Some(message)) => self.host.progress(session, message).await,
                _ => Progress::Failed("run has no message".into()),
            };
            let (status, detail) = match progress {
                Progress::InProgress => continue,
                Progress::Completed => (RunStatus::Completed, None),
                Progress::Failed(reason) => (RunStatus::Failed, Some(reason)),
                Progress::NotDispatched => (
                    RunStatus::Failed,
                    Some("the prompt was never queued (interrupted before delivery)".to_owned()),
                ),
                Progress::Missing => (
                    RunStatus::Failed,
                    Some("session does not exist (deleted or never created)".to_owned()),
                ),
            };
            run.status = status;
            run.finished_at = Some(self.clock.now());
            if detail.is_some() {
                run.detail = detail;
            }
            self.store.save_run(&run)?;
        }
        Ok(())
    }

    /// Disable `routine` because its heartbeat target is gone and record why.
    fn pause_for_missing_target(
        &self,
        routine: &Routine,
        session: &SessionId,
    ) -> Result<RunRecord> {
        let now = self.clock.now();
        self.store
            .auto_pause(&routine.id, RoutinePauseReason::TargetMissing)?;
        self.store.insert_run(&RunRecord {
            id: 0,
            routine_id: routine.id.clone(),
            scheduled_at: now,
            started_at: None,
            finished_at: Some(now),
            status: RunStatus::PausedTargetMissing,
            session_id: Some(session.clone()),
            message_id: None,
            detail: Some("target session was deleted; routine paused".into()),
        })
    }

    /// Pause enabled heartbeat routines whose target no longer exists. Recovery runs this so a
    /// deletion whose hook never ran (crash right after the tombstone) is still honoured.
    async fn reconcile_targets(&self, produced: &mut Vec<RunRecord>) -> Result<()> {
        for routine in self.store.routines()? {
            let RoutineAction::Heartbeat { session_id, .. } = &routine.action else {
                continue;
            };
            if routine.enabled && !self.host.session_exists(session_id).await {
                produced.push(self.pause_for_missing_target(&routine, session_id)?);
            }
        }
        Ok(())
    }
}
