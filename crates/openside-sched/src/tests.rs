//! Scheduler behaviour with a manual clock and a scripted host; nothing here sleeps.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use parking_lot::Mutex;
use tokio::sync::Notify;

use chrono::{DateTime, Duration, TimeZone, Utc};
use openside_proto::{
    CreateRoutineRequest, MessageId, Routine, RoutineAction, RoutinePauseReason, RoutineSchedule,
    RunRecord, RunStatus, SessionId, UpdateRoutineRequest,
};

use crate::{
    suggest::{self, UserMessage},
    Clock, Dispatch, HostError, ManualClock, NewSessionSpec, Progress, RoutineHost, Scheduler,
    Store,
};

/// A point inside a host call at which a test can stop the dispatch, inspect the world, and
/// then release (or abandon) it.
#[derive(Default)]
struct Checkpoint {
    entered: Notify,
    release: Notify,
}

impl Checkpoint {
    async fn pause(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

#[derive(Default)]
struct HostState {
    /// Existing sessions.
    sessions: HashSet<SessionId>,
    agents: HashMap<String, SessionId>,
    /// Where each queued message stands; absent means it was never queued.
    messages: HashMap<(SessionId, MessageId), Progress>,
    started: Vec<NewSessionSpec>,
    follow_ups: Vec<(SessionId, String)>,
    before_queue: Option<Arc<Checkpoint>>,
    after_queue: Option<Arc<Checkpoint>>,
    fail_after_queue: bool,
    reject: bool,
    after_exists: Option<Arc<Checkpoint>>,
}

/// Message-specific fake of the daemon. Clones share state, so a second [`Scheduler`] built
/// over the same host plays the role of a restarted daemon that kept its event log.
#[derive(Clone, Default)]
struct FakeHost(Arc<Mutex<HostState>>);

impl FakeHost {
    fn add_session(&self, id: &str) -> SessionId {
        let id = SessionId::new(id);
        self.0.lock().sessions.insert(id.clone());
        id
    }
    /// Complete every message of the session.
    fn settle(&self, id: &SessionId) {
        for ((session, _), progress) in self.0.lock().messages.iter_mut() {
            if session == id {
                *progress = Progress::Completed;
            }
        }
    }
    fn set_progress(&self, session: &SessionId, message: &MessageId, progress: Progress) {
        self.0
            .lock()
            .messages
            .insert((session.clone(), message.clone()), progress);
    }
    fn remove(&self, id: &SessionId) {
        self.0.lock().sessions.remove(id);
    }
    fn started(&self) -> usize {
        self.0.lock().started.len()
    }
    fn follow_ups(&self) -> Vec<(SessionId, String)> {
        self.0.lock().follow_ups.clone()
    }
    fn queued(&self) -> usize {
        self.0.lock().messages.len()
    }
    fn exists_checkpoint(&self, after: Option<Arc<Checkpoint>>) {
        self.0.lock().after_exists = after;
    }
    fn checkpoints(&self, before: Option<Arc<Checkpoint>>, after: Option<Arc<Checkpoint>>) {
        let mut state = self.0.lock();
        state.before_queue = before;
        state.after_queue = after;
    }
    async fn queue(&self, dispatch: &Dispatch) -> Result<(), HostError> {
        if self.0.lock().reject {
            return Err(HostError::Failed("boom".into()));
        }
        let before = self.0.lock().before_queue.clone();
        if let Some(checkpoint) = before {
            checkpoint.pause().await;
        }
        self.0.lock().messages.insert(
            (dispatch.session_id.clone(), dispatch.message_id.clone()),
            Progress::InProgress,
        );
        let after = self.0.lock().after_queue.clone();
        if let Some(checkpoint) = after {
            checkpoint.pause().await;
        }
        if self.0.lock().fail_after_queue {
            return Err(HostError::Failed("runtime failed to start".into()));
        }
        Ok(())
    }
}

impl RoutineHost for FakeHost {
    async fn start_session(
        &self,
        spec: &NewSessionSpec,
        dispatch: &Dispatch,
    ) -> Result<(), HostError> {
        {
            let mut state = self.0.lock();
            state.sessions.insert(dispatch.session_id.clone());
            state.started.push(spec.clone());
        }
        self.queue(dispatch).await
    }
    async fn follow_up(&self, dispatch: &Dispatch, prompt: &str) -> Result<(), HostError> {
        {
            let mut state = self.0.lock();
            if !state.sessions.contains(&dispatch.session_id) {
                return Err(HostError::TargetMissing);
            }
            state
                .follow_ups
                .push((dispatch.session_id.clone(), prompt.to_owned()));
        }
        self.queue(dispatch).await
    }
    async fn progress(&self, session: &SessionId, message: &MessageId) -> Progress {
        let state = self.0.lock();
        if !state.sessions.contains(session) {
            return Progress::Missing;
        }
        state
            .messages
            .get(&(session.clone(), message.clone()))
            .cloned()
            .unwrap_or(Progress::NotDispatched)
    }
    async fn session_exists(&self, session: &SessionId) -> bool {
        let exists = self.0.lock().sessions.contains(session);
        let checkpoint = self.0.lock().after_exists.clone();
        if let Some(checkpoint) = checkpoint {
            // The caller holds a possibly stale answer while the test changes the world.
            checkpoint.pause().await;
        }
        exists
    }
    async fn agent_session(&self, agent: &str) -> Option<SessionId> {
        self.0.lock().agents.get(agent).cloned()
    }
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

struct Fixture {
    clock: Arc<ManualClock>,
    host: FakeHost,
    scheduler: Scheduler<FakeHost>,
}

fn fixture(start: &str) -> Fixture {
    let clock = Arc::new(ManualClock::new(at(start)));
    let host = FakeHost::default();
    let scheduler = Scheduler::new(
        Store::open_in_memory().unwrap(),
        clock.clone(),
        host.clone(),
    );
    Fixture {
        clock,
        host,
        scheduler,
    }
}

fn cron(expr: &str, timezone: Option<&str>) -> RoutineSchedule {
    RoutineSchedule::Cron {
        expr: expr.into(),
        timezone: timezone.map(Into::into),
    }
}

fn new_session(prompt: &str) -> RoutineAction {
    RoutineAction::NewSession {
        prompt: prompt.into(),
        mode: None,
        workspace: None,
        model: None,
    }
}

fn request(schedule: RoutineSchedule, action: RoutineAction) -> CreateRoutineRequest {
    CreateRoutineRequest {
        name: "routine".into(),
        schedule,
        action,
        enabled: None,
        catch_up: None,
    }
}

fn statuses(runs: &[RunRecord]) -> Vec<RunStatus> {
    runs.iter().map(|run| run.status).collect()
}

// ① cron schedule

#[tokio::test]
async fn cron_routine_fires_when_the_clock_reaches_the_occurrence() {
    let f = fixture("2026-10-12T08:58:00Z");
    let routine = f
        .scheduler
        .create(request(
            cron("0 9 * * *", None),
            new_session("daily report"),
        ))
        .await
        .unwrap();
    assert_eq!(routine.next_run_at, Some(at("2026-10-12T09:00:00Z")));

    assert!(f.scheduler.tick().await.unwrap().is_empty(), "too early");
    f.clock.advance(Duration::minutes(1));
    assert!(f.scheduler.tick().await.unwrap().is_empty(), "still early");
    f.clock.advance(Duration::minutes(1));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&runs), [RunStatus::Started]);
    assert_eq!(runs[0].scheduled_at, at("2026-10-12T09:00:00Z"));
    assert_eq!(f.host.started(), 1);
    assert_eq!(f.host.0.lock().started[0].prompt, "daily report");

    assert!(
        f.scheduler.tick().await.unwrap().is_empty(),
        "no double fire"
    );
    assert_eq!(
        f.scheduler.get(&routine.id).unwrap().next_run_at,
        Some(at("2026-10-13T09:00:00Z"))
    );
}

#[tokio::test]
async fn cron_uses_posix_weekdays_and_the_requested_time_zone() {
    let f = fixture("2026-10-10T12:00:00Z"); // Saturday
                                             // 09:00 Mon-Fri in Seoul (UTC+9) is 00:00 UTC; Monday 2026-10-12 is next.
    let routine = f
        .scheduler
        .create(request(
            cron("0 9 * * 1-5", Some("Asia/Seoul")),
            new_session("standup"),
        ))
        .await
        .unwrap();
    assert_eq!(routine.next_run_at, Some(at("2026-10-12T00:00:00Z")));
}

#[tokio::test]
async fn bad_schedules_are_rejected_with_specific_codes() {
    let f = fixture("2026-10-12T00:00:00Z");
    for (schedule, needle) in [
        (cron("61 * * * *", None), "cron"),
        (cron("0 9 * * *", Some("Mars/Base")), "time zone"),
        (RoutineSchedule::Interval { secs: 0 }, "interval"),
        (cron("0 0 31 2 *", None), "never"),
    ] {
        let error = f
            .scheduler
            .create(request(schedule, new_session("x")))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(needle), "{needle}: {error}");
    }
}

#[tokio::test]
async fn interval_routine_fires_every_period_without_drift() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 600 },
            new_session("poll"),
        ))
        .await
        .unwrap();
    for minute in [10, 20, 30] {
        f.clock
            .set(at("2026-10-12T00:00:00Z") + Duration::minutes(minute) + Duration::seconds(3));
        let runs = f.scheduler.tick().await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].scheduled_at,
            at("2026-10-12T00:00:00Z") + Duration::minutes(minute)
        );
        // Settle so the next occurrence is not skipped as an overlap.
        f.host.settle(runs[0].session_id.as_ref().unwrap());
    }
    assert_eq!(
        f.scheduler.get(&routine.id).unwrap().next_run_at,
        Some(at("2026-10-12T00:40:00Z"))
    );
}

#[tokio::test]
async fn late_tick_fires_only_the_latest_occurrence_and_records_the_rest_as_missed() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 60 },
            new_session("poll"),
        ))
        .await
        .unwrap();
    f.clock.advance(Duration::seconds(60 * 5 + 10)); // five occurrences are due
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(
        statuses(&runs),
        [
            RunStatus::Missed,
            RunStatus::Missed,
            RunStatus::Missed,
            RunStatus::Missed,
            RunStatus::Started
        ]
    );
    assert_eq!(runs[4].scheduled_at, at("2026-10-12T00:05:00Z"));
    assert_eq!(f.host.started(), 1, "no burst replay");
    assert_eq!(
        f.scheduler.get(&routine.id).unwrap().next_run_at,
        Some(at("2026-10-12T00:06:00Z"))
    );
}

// ② heartbeat

#[tokio::test]
async fn heartbeat_queues_a_follow_up_in_the_existing_session() {
    let f = fixture("2026-10-12T00:00:00Z");
    let target = f.host.add_session("existing");
    f.scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 300 },
            RoutineAction::Heartbeat {
                session_id: target.clone(),
                prompt: "check in".into(),
            },
        ))
        .await
        .unwrap();
    f.clock.advance(Duration::minutes(5));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&runs), [RunStatus::Started]);
    assert_eq!(runs[0].session_id.as_ref(), Some(&target));
    assert_eq!(f.host.follow_ups(), [(target, "check in".to_owned())]);
    assert_eq!(f.host.started(), 0, "heartbeat never creates a session");
}

// ③ overlap

#[tokio::test]
async fn run_is_skipped_and_recorded_while_the_previous_one_is_still_in_progress() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 60 },
            new_session("slow job"),
        ))
        .await
        .unwrap();

    f.clock.advance(Duration::seconds(60));
    let first = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&first), [RunStatus::Started]);
    let running = first[0].session_id.clone().unwrap();

    f.clock.advance(Duration::seconds(60));
    let second = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&second), [RunStatus::SkippedOverlap]);
    assert_eq!(second[0].session_id.as_ref(), Some(&running));
    assert_eq!(
        f.host.started(),
        1,
        "overlapping run must not start a session"
    );

    f.host.settle(&running);
    f.clock.advance(Duration::seconds(60));
    let third = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&third), [RunStatus::Started]);
    assert_eq!(f.host.started(), 2);

    let history = f.scheduler.runs(&routine.id, 10).await.unwrap();
    assert_eq!(
        statuses(&history),
        [
            RunStatus::Started,
            RunStatus::SkippedOverlap,
            RunStatus::Completed
        ],
        "newest first; the first run settled"
    );
    assert!(history[2].finished_at.is_some());
}

// ④ target deleted

#[tokio::test]
async fn deleting_the_target_session_pauses_the_heartbeat_routine() {
    let f = fixture("2026-10-12T00:00:00Z");
    let target = f.host.add_session("target");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 60 },
            RoutineAction::Heartbeat {
                session_id: target.clone(),
                prompt: "ping".into(),
            },
        ))
        .await
        .unwrap();

    f.host.remove(&target);
    let records = f.scheduler.on_session_deleted(&target).await.unwrap();
    assert_eq!(statuses(&records), [RunStatus::PausedTargetMissing]);

    let paused = f.scheduler.get(&routine.id).unwrap();
    assert!(!paused.enabled);
    assert_eq!(
        paused.paused_reason,
        Some(RoutinePauseReason::TargetMissing)
    );
    assert_eq!(paused.next_run_at, None);

    f.clock.advance(Duration::minutes(10));
    assert!(
        f.scheduler.tick().await.unwrap().is_empty(),
        "paused routine stays quiet"
    );
    assert!(f.host.follow_ups().is_empty());

    // Resuming clears the reason; a still-missing target pauses it again at the next fire.
    let resumed = f
        .scheduler
        .update(
            &routine.id,
            UpdateRoutineRequest {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(resumed.enabled && resumed.paused_reason.is_none());
    f.clock.advance(Duration::minutes(1));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&runs), [RunStatus::PausedTargetMissing]);
    assert!(!f.scheduler.get(&routine.id).unwrap().enabled);
}

#[tokio::test]
async fn a_target_that_disappeared_unnoticed_pauses_the_routine_at_fire_time() {
    let f = fixture("2026-10-12T00:00:00Z");
    let target = f.host.add_session("vanishes-quietly");
    f.scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 60 },
            RoutineAction::Heartbeat {
                session_id: target.clone(),
                prompt: "ping".into(),
            },
        ))
        .await
        .unwrap();
    f.host.remove(&target); // no deletion hook ran
    f.clock.advance(Duration::seconds(60));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&runs), [RunStatus::PausedTargetMissing]);
    let routine = &f.scheduler.list().unwrap()[0];
    assert_eq!(
        routine.paused_reason,
        Some(RoutinePauseReason::TargetMissing)
    );
}

#[tokio::test]
async fn deleting_a_session_fails_runs_that_were_still_attached_to_it() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 60 },
            new_session("job"),
        ))
        .await
        .unwrap();
    f.clock.advance(Duration::seconds(60));
    let run = f.scheduler.tick().await.unwrap().remove(0);
    let session = run.session_id.unwrap();
    f.host.remove(&session);
    f.scheduler.on_session_deleted(&session).await.unwrap();
    let history = f.scheduler.runs(&routine.id, 5).await.unwrap();
    assert_eq!(statuses(&history), [RunStatus::Failed]);
    assert_eq!(history[0].detail.as_deref(), Some("session was deleted"));
}

// ⑤ run now

#[tokio::test]
async fn run_now_fires_immediately_without_moving_the_schedule() {
    let f = fixture("2026-10-12T08:00:00Z");
    let routine = f
        .scheduler
        .create(request(cron("0 9 * * *", None), new_session("report")))
        .await
        .unwrap();
    let run = f.scheduler.run_now(&routine.id).await.unwrap();
    assert_eq!(run.status, RunStatus::Started);
    assert_eq!(run.scheduled_at, at("2026-10-12T08:00:00Z"));
    assert_eq!(f.host.started(), 1);
    assert_eq!(
        f.scheduler.get(&routine.id).unwrap().next_run_at,
        Some(at("2026-10-12T09:00:00Z"))
    );

    // The overlap rule also covers manual runs.
    let again = f.scheduler.run_now(&routine.id).await.unwrap();
    assert_eq!(again.status, RunStatus::SkippedOverlap);
    assert_eq!(f.host.started(), 1);

    // A disabled routine can still be run on demand.
    f.host.settle(run.session_id.as_ref().unwrap());
    f.scheduler
        .update(
            &routine.id,
            UpdateRoutineRequest {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.scheduler.run_now(&routine.id).await.unwrap().status,
        RunStatus::Started
    );
    assert!(matches!(
        f.scheduler.run_now("missing").await,
        Err(crate::Error::UnknownRoutine(_))
    ));
}

// restart recovery

fn reopen(
    path: &std::path::Path,
    clock: &Arc<ManualClock>,
    host: &FakeHost,
) -> Scheduler<FakeHost> {
    Scheduler::open(path, clock.clone(), host.clone()).unwrap()
}

#[tokio::test]
async fn restart_records_missed_runs_without_replaying_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routines.db");
    let clock = Arc::new(ManualClock::new(at("2026-10-12T00:00:00Z")));
    let host = FakeHost::default();
    let id = {
        let scheduler = reopen(&path, &clock, &host);
        scheduler
            .create(request(
                RoutineSchedule::Interval { secs: 600 },
                new_session("poll"),
            ))
            .await
            .unwrap()
            .id
    };

    // The daemon is down for 35 minutes: occurrences at :10, :20, :30 are missed.
    clock.advance(Duration::minutes(35));
    let scheduler = reopen(&path, &clock, &host);
    assert_eq!(
        scheduler.list().unwrap().len(),
        1,
        "routines survive restart"
    );
    let records = scheduler.recover().await.unwrap();
    assert_eq!(statuses(&records), [RunStatus::Missed; 3]);
    assert_eq!(
        records.iter().map(|r| r.scheduled_at).collect::<Vec<_>>(),
        [
            at("2026-10-12T00:10:00Z"),
            at("2026-10-12T00:20:00Z"),
            at("2026-10-12T00:30:00Z")
        ]
    );
    assert_eq!(host.started(), 0, "missed runs are not replayed");
    assert_eq!(
        scheduler.get(&id).unwrap().next_run_at,
        Some(at("2026-10-12T00:40:00Z"))
    );

    // History is durable, and the schedule resumes normally.
    clock.advance(Duration::minutes(6));
    let fired = scheduler.tick().await.unwrap();
    assert_eq!(statuses(&fired), [RunStatus::Started]);
    drop(scheduler);
    let history = reopen(&path, &clock, &host).runs(&id, 10).await.unwrap();
    assert_eq!(history.len(), 4);
}

#[tokio::test]
async fn catch_up_runs_once_for_the_latest_missed_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routines.db");
    let clock = Arc::new(ManualClock::new(at("2026-10-12T00:00:00Z")));
    let host = FakeHost::default();
    let id = {
        let scheduler = reopen(&path, &clock, &host);
        let mut create = request(RoutineSchedule::Interval { secs: 600 }, new_session("poll"));
        create.catch_up = Some(true);
        scheduler.create(create).await.unwrap().id
    };
    clock.advance(Duration::minutes(35));
    let scheduler = reopen(&path, &clock, &host);
    let records = scheduler.recover().await.unwrap();
    assert_eq!(
        statuses(&records),
        [RunStatus::Missed, RunStatus::Missed, RunStatus::Started]
    );
    assert_eq!(records[2].scheduled_at, at("2026-10-12T00:30:00Z"));
    assert_eq!(host.started(), 1, "exactly one catch-up run");
    assert_eq!(
        scheduler.get(&id).unwrap().next_run_at,
        Some(at("2026-10-12T00:40:00Z"))
    );
}

#[tokio::test]
async fn a_huge_backlog_is_summarised_instead_of_flooding_the_history() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 1 },
            new_session("poll"),
        ))
        .await
        .unwrap();
    f.clock.advance(Duration::days(30));
    let records = f.scheduler.recover().await.unwrap();
    assert_eq!(records.len(), crate::MAX_MISSED_RECORDS);
    assert!(records
        .last()
        .unwrap()
        .detail
        .as_deref()
        .unwrap()
        .contains("not recorded individually"));
    assert_eq!(f.host.started(), 0);
    let next = f.scheduler.get(&routine.id).unwrap().next_run_at.unwrap();
    assert!(next > f.clock.now());
}

#[tokio::test]
async fn pausing_and_resuming_never_counts_the_paused_time_as_missed() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(
            RoutineSchedule::Interval { secs: 60 },
            new_session("poll"),
        ))
        .await
        .unwrap();
    let patch = |enabled| UpdateRoutineRequest {
        enabled: Some(enabled),
        ..Default::default()
    };
    f.scheduler.update(&routine.id, patch(false)).await.unwrap();
    f.clock.advance(Duration::hours(3));
    assert!(f.scheduler.recover().await.unwrap().is_empty());
    let resumed: Routine = f.scheduler.update(&routine.id, patch(true)).await.unwrap();
    assert!(resumed.next_run_at.unwrap() > f.clock.now());
    assert!(f.scheduler.tick().await.unwrap().is_empty());
}

// ⑥ repetition candidates

fn message(session: &str, id: &str, text: &str, when: DateTime<Utc>) -> UserMessage {
    UserMessage {
        session_id: SessionId::new(session),
        message_id: id.into(),
        text: text.into(),
        at: when,
    }
}

#[test]
fn a_daily_request_becomes_a_cron_draft_with_the_typical_time_of_day() {
    let day0 = Utc.with_ymd_and_hms(2026, 10, 1, 9, 5, 0).unwrap();
    let wobble = [0, 7, -4, 11, 3, -9]; // minutes of human jitter
    let mut messages: Vec<UserMessage> = wobble
        .iter()
        .enumerate()
        .map(|(day, jitter)| {
            message(
                &format!("s{day}"),
                &format!("m{day}"),
                &format!("Summarize my unread email from today {}", day + 1),
                day0 + Duration::days(day as i64) + Duration::minutes(*jitter),
            )
        })
        .collect();
    // Unrelated one-off chatter must not produce suggestions.
    messages.push(message("s0", "x1", "fix the flaky login test in ci", day0));
    messages.push(message("s1", "x2", "continue", day0 + Duration::hours(30)));

    let drafts = suggest::suggest(&messages, &[]);
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    let draft = &drafts[0];
    assert_eq!(draft.occurrences, 6);
    assert_eq!(draft.prompt, "Summarize my unread email from today 6");
    assert_eq!(draft.session_ids.len(), 6);
    let RoutineSchedule::Cron { expr, timezone } = &draft.schedule else {
        panic!("expected cron draft: {:?}", draft.schedule);
    };
    assert_eq!(timezone.as_deref(), Some("UTC"));
    assert_eq!(expr, "8 9 * * *", "median time of day of the occurrences");
}

#[test]
fn weekly_spacing_picks_the_weekday() {
    let first = Utc.with_ymd_and_hms(2026, 9, 7, 18, 0, 0).unwrap(); // Monday
    let messages: Vec<_> = (0..4)
        .map(|week| {
            message(
                "s",
                &format!("w{week}"),
                "Draft the weekly status update for the team",
                first + Duration::weeks(week) + Duration::minutes(week * 3),
            )
        })
        .collect();
    let drafts = suggest::suggest(&messages, &[]);
    let RoutineSchedule::Cron { expr, .. } = &drafts[0].schedule else {
        panic!("expected cron");
    };
    assert_eq!(expr, "6 18 * * 1");
}

#[test]
fn irregular_or_too_few_repetitions_are_not_suggested() {
    let start = Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap();
    let text = "Check the build dashboard for failures";
    // Two occurrences are not enough.
    let two = [
        message("s", "a", text, start),
        message("s", "b", text, start + Duration::days(1)),
    ];
    assert!(suggest::suggest(&two, &[]).is_empty());
    // Three occurrences with wildly different gaps are not a routine.
    let erratic = [
        message("s", "a", text, start),
        message("s", "b", text, start + Duration::hours(2)),
        message("s", "c", text, start + Duration::days(9)),
    ];
    assert!(suggest::suggest(&erratic, &[]).is_empty());
    // Re-sending within minutes is one occurrence, not three.
    let retries = [
        message("s", "a", text, start),
        message("s", "b", text, start + Duration::seconds(30)),
        message("s", "c", text, start + Duration::seconds(90)),
    ];
    assert!(suggest::suggest(&retries, &[]).is_empty());
}

#[test]
fn routine_sent_prompts_and_existing_routines_are_not_suggested_again() {
    let start = Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap();
    let text = "Check the build dashboard for failures";
    let from_routine: Vec<_> = (0..4)
        .map(|day| {
            message(
                "s",
                &format!("routine-{day}"),
                text,
                start + Duration::days(day),
            )
        })
        .collect();
    assert!(suggest::suggest(&from_routine, &[]).is_empty());

    let typed: Vec<_> = (0..4)
        .map(|day| message("s", &format!("u{day}"), text, start + Duration::days(day)))
        .collect();
    assert_eq!(suggest::suggest(&typed, &[]).len(), 1);
    assert!(
        suggest::suggest(&typed, &["check the BUILD dashboard for failures!".into()]).is_empty()
    );
}

#[tokio::test]
async fn scheduler_suggestions_skip_prompts_that_already_have_a_routine() {
    let f = fixture("2026-10-12T00:00:00Z");
    let start = Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap();
    let text = "Check the build dashboard for failures";
    let messages: Vec<_> = (0..4)
        .map(|day| message("s", &format!("u{day}"), text, start + Duration::days(day)))
        .collect();
    assert_eq!(f.scheduler.suggestions(&messages).unwrap().len(), 1);
    f.scheduler
        .create(request(cron("0 9 * * *", None), new_session(text)))
        .await
        .unwrap();
    assert!(f.scheduler.suggestions(&messages).unwrap().is_empty());
    assert_eq!(
        f.scheduler.list().unwrap().len(),
        1,
        "suggesting creates nothing"
    );
}

// five-field grammar

#[tokio::test]
async fn only_five_field_cron_expressions_are_accepted() {
    let f = fixture("2026-10-12T00:00:00Z");
    for expr in [
        "0 0 9 * * *",      // leading seconds
        "0 9 * * * 2027",   // trailing year
        "0 0 9 * * * 2027", // both
        "* * * *",
        "",
    ] {
        let error = f
            .scheduler
            .create(request(cron(expr, None), new_session("x")))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cron"),
            "{expr:?} must be rejected as a cron error: {error}"
        );
    }
    for expr in [
        "0 9 * * *",
        "*/15 * * * 1-5",
        "@daily",
        "0 9 * * MON",
        "0 9 * * 7",
    ] {
        f.scheduler
            .create(request(cron(expr, None), new_session("x")))
            .await
            .unwrap_or_else(|error| panic!("{expr:?} must be accepted: {error}"));
    }
}

// daylight saving time (America/New_York: spring forward 2026-03-08, fall back 2026-11-01)

const NEW_YORK: Option<&str> = Some("America/New_York");

#[test]
fn a_fixed_time_inside_the_spring_gap_runs_at_the_first_instant_after_it() {
    let schedule = cron("30 2 * * *", NEW_YORK);
    let day_before = at("2026-03-07T12:00:00Z");
    let gap_day = crate::schedule::next_after(&schedule, day_before, day_before)
        .unwrap()
        .unwrap();
    // 02:30 does not exist on 8 March; the job runs when 03:00 EDT (07:00Z) arrives.
    assert_eq!(gap_day, at("2026-03-08T07:00:00Z"));
    let next = crate::schedule::next_after(&schedule, gap_day, gap_day)
        .unwrap()
        .unwrap();
    assert_eq!(
        next,
        at("2026-03-09T06:30:00Z"),
        "back to 02:30 EDT the next day"
    );
}

#[test]
fn a_fixed_time_inside_the_fall_fold_runs_once_at_the_earlier_instant() {
    let schedule = cron("30 1 * * *", NEW_YORK);
    let before = at("2026-10-31T12:00:00Z");
    let fold_day = crate::schedule::next_after(&schedule, before, before)
        .unwrap()
        .unwrap();
    assert_eq!(
        fold_day,
        at("2026-11-01T05:30:00Z"),
        "01:30 EDT, not the repeated 01:30 EST"
    );
    let next = crate::schedule::next_after(&schedule, fold_day, fold_day)
        .unwrap()
        .unwrap();
    assert_eq!(
        next,
        at("2026-11-02T06:30:00Z"),
        "the repeated 01:30 EST is skipped"
    );
}

#[test]
fn an_hourly_job_runs_on_both_instants_of_the_repeated_hour() {
    let schedule = cron("0 * * * *", NEW_YORK);
    let mut at_time = at("2026-11-01T04:30:00Z");
    let mut seen = Vec::new();
    for _ in 0..3 {
        at_time = crate::schedule::next_after(&schedule, at_time, at_time)
            .unwrap()
            .unwrap();
        seen.push(at_time);
    }
    assert_eq!(
        seen,
        [
            at("2026-11-01T05:00:00Z"), // 01:00 EDT
            at("2026-11-01T06:00:00Z"), // 01:00 EST
            at("2026-11-01T07:00:00Z"), // 02:00 EST
        ]
    );
}

#[tokio::test]
async fn the_scheduler_fires_once_across_the_spring_gap_and_the_fall_fold() {
    // Spring: nothing fires at the nonexistent 02:30, one run at 03:00 EDT, none duplicated.
    let f = fixture("2026-03-07T12:00:00Z");
    let spring = f
        .scheduler
        .create(request(cron("30 2 * * *", NEW_YORK), new_session("daily")))
        .await
        .unwrap();
    f.clock.set(at("2026-03-08T06:59:59Z"));
    assert!(f.scheduler.tick().await.unwrap().is_empty());
    f.clock.set(at("2026-03-08T07:00:00Z"));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&runs), [RunStatus::Started]);
    assert_eq!(runs[0].scheduled_at, at("2026-03-08T07:00:00Z"));
    f.host.settle(runs[0].session_id.as_ref().unwrap());
    f.clock.set(at("2026-03-08T08:00:00Z"));
    assert!(f.scheduler.tick().await.unwrap().is_empty());
    assert_eq!(
        f.scheduler.get(&spring.id).unwrap().next_run_at,
        Some(at("2026-03-09T06:30:00Z"))
    );

    // Fall: one run at 01:30 EDT; the repeated 01:30 EST does not fire again.
    let f = fixture("2026-10-31T12:00:00Z");
    f.scheduler
        .create(request(cron("30 1 * * *", NEW_YORK), new_session("daily")))
        .await
        .unwrap();
    f.clock.set(at("2026-11-01T05:30:00Z"));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(statuses(&runs), [RunStatus::Started]);
    f.host.settle(runs[0].session_id.as_ref().unwrap());
    f.clock.set(at("2026-11-01T06:30:00Z"));
    assert!(
        f.scheduler.tick().await.unwrap().is_empty(),
        "01:30 EST must not repeat the run"
    );
    f.clock.set(at("2026-11-02T06:30:00Z"));
    assert_eq!(
        statuses(&f.scheduler.tick().await.unwrap()),
        [RunStatus::Started]
    );
}

#[tokio::test]
async fn recovery_across_the_fall_fold_counts_the_folded_day_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routines.db");
    let clock = Arc::new(ManualClock::new(at("2026-10-31T12:00:00Z")));
    let host = FakeHost::default();
    {
        let mut create = request(cron("30 1 * * *", NEW_YORK), new_session("daily"));
        create.catch_up = Some(true);
        reopen(&path, &clock, &host).create(create).await.unwrap();
    }
    clock.set(at("2026-11-02T07:00:00Z"));
    let records = reopen(&path, &clock, &host).recover().await.unwrap();
    assert_eq!(
        records
            .iter()
            .map(|run| (run.status, run.scheduled_at))
            .collect::<Vec<_>>(),
        [
            (RunStatus::Missed, at("2026-11-01T05:30:00Z")),
            (RunStatus::Started, at("2026-11-02T06:30:00Z")),
        ]
    );
    assert_eq!(host.started(), 1);
}

// concurrency: the gate serialises every mutation with firing

type Shared = (Arc<ManualClock>, FakeHost, Arc<Scheduler<FakeHost>>);

fn shared(start: &str) -> Shared {
    let clock = Arc::new(ManualClock::new(at(start)));
    let host = FakeHost::default();
    let scheduler = Arc::new(Scheduler::new(
        Store::open_in_memory().unwrap(),
        clock.clone(),
        host.clone(),
    ));
    (clock, host, scheduler)
}

/// Let every runnable task make progress until it blocks.
async fn quiesce() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

fn interval(secs: u64) -> RoutineSchedule {
    RoutineSchedule::Interval { secs }
}

#[tokio::test]
async fn pause_and_delete_wait_for_a_dispatch_that_is_in_flight() {
    let (clock, host, scheduler) = shared("2026-10-12T00:00:00Z");
    let first = scheduler
        .create(request(interval(60), new_session("one")))
        .await
        .unwrap();
    let second = scheduler
        .create(request(interval(60), new_session("two")))
        .await
        .unwrap();
    let third = scheduler
        .create(request(interval(60), new_session("three")))
        .await
        .unwrap();
    let stop = Arc::new(Checkpoint::default());
    host.checkpoints(Some(stop.clone()), None);

    clock.advance(Duration::seconds(60));
    let tick = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.tick().await }
    });
    stop.entered.notified().await; // the first routine's dispatch is now suspended
    let pause = tokio::spawn({
        let (scheduler, id) = (scheduler.clone(), second.id.clone());
        async move {
            scheduler
                .update(
                    &id,
                    UpdateRoutineRequest {
                        enabled: Some(false),
                        ..Default::default()
                    },
                )
                .await
        }
    });
    let delete = tokio::spawn({
        let (scheduler, id) = (scheduler.clone(), third.id.clone());
        async move { scheduler.delete(&id).await }
    });
    quiesce().await;
    assert!(
        !pause.is_finished(),
        "pause must wait for the in-flight tick"
    );
    assert!(
        !delete.is_finished(),
        "delete must wait for the in-flight tick"
    );

    host.checkpoints(None, None);
    stop.release.notify_one();
    let runs = tick.await.unwrap().unwrap();
    // The tick was linearised before both edits, so all three fired in it.
    assert_eq!(runs.len(), 3);
    let paused = pause.await.unwrap().unwrap();
    delete.await.unwrap().unwrap();

    // After the edits returned, nothing acts on the old state.
    assert!(!paused.enabled && paused.next_run_at.is_none());
    assert!(matches!(
        scheduler.get(&third.id),
        Err(crate::Error::UnknownRoutine(_))
    ));
    let started = host.started();
    clock.advance(Duration::hours(1));
    let later = scheduler.tick().await.unwrap();
    let fired: Vec<_> = later
        .iter()
        .filter(|run| run.status != RunStatus::Missed)
        .collect();
    assert_eq!(
        fired
            .iter()
            .map(|run| (run.routine_id.clone(), run.status))
            .collect::<Vec<_>>(),
        [(first.id.clone(), RunStatus::SkippedOverlap)],
        "only the untouched routine is still scheduled (skipped as an overlap)"
    );
    assert!(
        later.iter().all(|run| run.routine_id == first.id),
        "no record for the paused or deleted routines"
    );
    assert_eq!(host.started(), started);
    assert!(scheduler.runs(&second.id, 10).await.unwrap().len() == 1);
}

#[tokio::test]
async fn changing_the_schedule_during_a_dispatch_is_not_overwritten_by_the_tick() {
    let (clock, host, scheduler) = shared("2026-10-12T00:00:00Z");
    let routine = scheduler
        .create(request(interval(60), new_session("job")))
        .await
        .unwrap();
    let stop = Arc::new(Checkpoint::default());
    host.checkpoints(Some(stop.clone()), None);
    clock.advance(Duration::seconds(60));
    let tick = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.tick().await }
    });
    stop.entered.notified().await;
    let edit = tokio::spawn({
        let (scheduler, id) = (scheduler.clone(), routine.id.clone());
        async move {
            scheduler
                .update(
                    &id,
                    UpdateRoutineRequest {
                        schedule: Some(interval(3600)),
                        ..Default::default()
                    },
                )
                .await
        }
    });
    quiesce().await;
    assert!(!edit.is_finished());
    host.checkpoints(None, None);
    stop.release.notify_one();
    tick.await.unwrap().unwrap();
    let edited = edit.await.unwrap().unwrap();
    assert_eq!(
        scheduler.get(&routine.id).unwrap().next_run_at,
        edited.next_run_at,
        "the edit's next_run_at is the final word"
    );
    assert_eq!(edited.next_run_at, Some(at("2026-10-12T01:01:00Z")));
}

#[tokio::test]
async fn a_second_tick_and_run_now_cannot_overlap_a_dispatch_in_flight() {
    let (clock, host, scheduler) = shared("2026-10-12T00:00:00Z");
    let routine = scheduler
        .create(request(interval(60), new_session("job")))
        .await
        .unwrap();
    let stop = Arc::new(Checkpoint::default());
    host.checkpoints(Some(stop.clone()), None);
    clock.advance(Duration::seconds(60));
    let first = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.tick().await }
    });
    stop.entered.notified().await;
    let second = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.tick().await }
    });
    let manual = tokio::spawn({
        let (scheduler, id) = (scheduler.clone(), routine.id.clone());
        async move { scheduler.run_now(&id).await }
    });
    quiesce().await;
    assert!(!second.is_finished() && !manual.is_finished());
    host.checkpoints(None, None);
    stop.release.notify_one();
    assert_eq!(
        statuses(&first.await.unwrap().unwrap()),
        [RunStatus::Started]
    );
    assert!(second.await.unwrap().unwrap().is_empty());
    assert_eq!(
        manual.await.unwrap().unwrap().status,
        RunStatus::SkippedOverlap
    );
    assert_eq!(host.queued(), 1, "exactly one prompt was ever queued");
    assert_eq!(host.started(), 1);
}

#[tokio::test]
async fn deleting_the_target_during_a_dispatch_pauses_after_the_dispatch_settles() {
    let (clock, host, scheduler) = shared("2026-10-12T00:00:00Z");
    let target = host.add_session("target");
    let routine = scheduler
        .create(request(
            interval(60),
            RoutineAction::Heartbeat {
                session_id: target.clone(),
                prompt: "ping".into(),
            },
        ))
        .await
        .unwrap();
    let stop = Arc::new(Checkpoint::default());
    host.checkpoints(Some(stop.clone()), None);
    clock.advance(Duration::seconds(60));
    let tick = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.tick().await }
    });
    stop.entered.notified().await;
    host.remove(&target);
    let hook = tokio::spawn({
        let (scheduler, target) = (scheduler.clone(), target.clone());
        async move { scheduler.on_session_deleted(&target).await }
    });
    quiesce().await;
    assert!(
        !hook.is_finished(),
        "the deletion hook must not interleave with the dispatch"
    );
    host.checkpoints(None, None);
    stop.release.notify_one();
    tick.await.unwrap().unwrap();
    hook.await.unwrap().unwrap();

    let history = scheduler.runs(&routine.id, 10).await.unwrap();
    assert_eq!(
        statuses(&history),
        [RunStatus::PausedTargetMissing, RunStatus::Failed]
    );
    assert_eq!(history[1].detail.as_deref(), Some("session was deleted"));
    let paused = scheduler.get(&routine.id).unwrap();
    assert_eq!(
        paused.paused_reason,
        Some(RoutinePauseReason::TargetMissing)
    );
}

#[tokio::test]
async fn a_deleted_routine_can_never_receive_history_again() {
    let store = Store::open_in_memory().unwrap();
    let clock = ManualClock::new(at("2026-10-12T00:00:00Z"));
    let scheduler = Scheduler::new(
        Store::open_in_memory().unwrap(),
        Arc::new(clock),
        FakeHost::default(),
    );
    let routine = scheduler
        .create(request(interval(60), new_session("job")))
        .await
        .unwrap();
    let run = RunRecord {
        id: 0,
        routine_id: routine.id.clone(),
        scheduled_at: at("2026-10-12T00:01:00Z"),
        started_at: None,
        finished_at: None,
        status: RunStatus::Started,
        session_id: None,
        message_id: None,
        detail: None,
    };
    store.insert_routine(&routine).unwrap();
    let stored = store.insert_run(&run).unwrap();
    assert!(store.delete_routine(&routine.id).unwrap());
    assert!(
        store.runs(&routine.id, 10).unwrap().is_empty(),
        "history goes with the routine"
    );
    assert!(
        store.insert_run(&run).is_err(),
        "no run for a deleted routine"
    );
    assert!(
        store.save_run(&stored).is_err(),
        "no update of a deleted run"
    );
    assert!(matches!(
        store.set_next_run_at(&routine.id, None),
        Err(crate::Error::UnknownRoutine(_))
    ));
}

// durable reservation: a crash at any point leaves a run that names its message

fn heartbeat_to(target: &SessionId) -> RoutineAction {
    RoutineAction::Heartbeat {
        session_id: target.clone(),
        prompt: "ping".into(),
    }
}

#[tokio::test]
async fn a_crash_after_queueing_and_before_recording_still_blocks_overlap_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routines.db");
    let clock = Arc::new(ManualClock::new(at("2026-10-12T00:00:00Z")));
    let host = FakeHost::default();
    let target = host.add_session("target");
    let first = Arc::new(reopen(&path, &clock, &host));
    let id = first
        .create(request(interval(600), heartbeat_to(&target)))
        .await
        .unwrap()
        .id;
    let stop = Arc::new(Checkpoint::default());
    host.checkpoints(None, Some(stop.clone()));
    let doomed = tokio::spawn({
        let (first, id) = (first.clone(), id.clone());
        async move { first.run_now(&id).await }
    });
    stop.entered.notified().await; // queued in the host, host call not yet returned
    doomed.abort(); // the daemon dies here
    let _ = doomed.await;
    drop(first);
    host.checkpoints(None, None);

    let second = reopen(&path, &clock, &host);
    assert!(second.recover().await.unwrap().is_empty());
    let again = second.run_now(&id).await.unwrap();
    assert_eq!(
        again.status,
        RunStatus::SkippedOverlap,
        "the queued prompt is still the active run"
    );
    assert_eq!(host.queued(), 1);

    let open = second.runs(&id, 10).await.unwrap();
    let message = open
        .iter()
        .find(|run| run.status == RunStatus::Started)
        .and_then(|run| run.message_id.clone())
        .expect("the reservation names its message");
    host.set_progress(&target, &message, Progress::Completed);
    assert_eq!(
        second.run_now(&id).await.unwrap().status,
        RunStatus::Started
    );
    assert_eq!(host.queued(), 2);
}

#[tokio::test]
async fn a_crash_before_queueing_is_reported_as_never_delivered_and_the_next_run_goes_ahead() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routines.db");
    let clock = Arc::new(ManualClock::new(at("2026-10-12T00:00:00Z")));
    let host = FakeHost::default();
    let first = Arc::new(reopen(&path, &clock, &host));
    let id = first
        .create(request(interval(600), new_session("job")))
        .await
        .unwrap()
        .id;
    let stop = Arc::new(Checkpoint::default());
    host.checkpoints(Some(stop.clone()), None);
    let doomed = tokio::spawn({
        let (first, id) = (first.clone(), id.clone());
        async move { first.run_now(&id).await }
    });
    stop.entered.notified().await;
    doomed.abort();
    let _ = doomed.await;
    drop(first);
    host.checkpoints(None, None);

    let second = reopen(&path, &clock, &host);
    second.recover().await.unwrap();
    let history = second.runs(&id, 10).await.unwrap();
    assert_eq!(statuses(&history), [RunStatus::Failed]);
    assert!(history[0]
        .detail
        .as_deref()
        .unwrap()
        .contains("never queued"));
    assert_eq!(host.queued(), 0);
    assert_eq!(
        second.run_now(&id).await.unwrap().status,
        RunStatus::Started
    );
    assert_eq!(host.queued(), 1, "one active run, not two");
}

#[tokio::test]
async fn a_host_error_after_the_prompt_was_queued_keeps_the_run_active() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(interval(600), new_session("job")))
        .await
        .unwrap();
    f.host.0.lock().fail_after_queue = true;
    let run = f.scheduler.run_now(&routine.id).await.unwrap();
    assert_eq!(
        run.status,
        RunStatus::Started,
        "the message is queued, so the run is live"
    );
    assert!(run.detail.as_deref().unwrap().contains("queued"));
    assert_eq!(
        f.scheduler.run_now(&routine.id).await.unwrap().status,
        RunStatus::SkippedOverlap
    );
    assert_eq!(f.host.queued(), 1);
}

#[tokio::test]
async fn a_host_error_before_anything_was_queued_fails_the_run_and_frees_the_slot() {
    let f = fixture("2026-10-12T00:00:00Z");
    let routine = f
        .scheduler
        .create(request(interval(600), new_session("job")))
        .await
        .unwrap();
    f.host.0.lock().reject = true;
    let run = f.scheduler.run_now(&routine.id).await.unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.session_id.is_none() && run.message_id.is_none());
    assert_eq!(f.host.queued(), 0);
    f.host.0.lock().reject = false;
    assert_eq!(
        f.scheduler.run_now(&routine.id).await.unwrap().status,
        RunStatus::Started
    );
}

#[tokio::test]
async fn recovery_pauses_heartbeats_whose_target_vanished_without_a_deletion_hook() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routines.db");
    let clock = Arc::new(ManualClock::new(at("2026-10-12T00:00:00Z")));
    let host = FakeHost::default();
    let target = host.add_session("target");
    let id = {
        let scheduler = reopen(&path, &clock, &host);
        scheduler
            .create(request(interval(600), heartbeat_to(&target)))
            .await
            .unwrap()
            .id
    };
    // The session was tombstoned, then the daemon died before the hook ran.
    host.remove(&target);
    let scheduler = reopen(&path, &clock, &host);
    let records = scheduler.recover().await.unwrap();
    assert_eq!(statuses(&records), [RunStatus::PausedTargetMissing]);
    let routine = scheduler.get(&id).unwrap();
    assert!(!routine.enabled);
    assert_eq!(
        routine.paused_reason,
        Some(RoutinePauseReason::TargetMissing)
    );
    clock.advance(Duration::hours(1));
    assert!(scheduler.tick().await.unwrap().is_empty());
    assert!(host.follow_ups().is_empty());
}

// heartbeat target validation happens under the gate

#[tokio::test]
async fn a_heartbeat_to_a_missing_session_is_rejected_on_create_and_on_retarget() {
    let f = fixture("2026-10-12T00:00:00Z");
    let ghost = SessionId::new("ghost");
    let error = f
        .scheduler
        .create(request(interval(60), heartbeat_to(&ghost)))
        .await
        .unwrap_err();
    assert_eq!(
        match error {
            crate::Error::Invalid(invalid) => invalid.code(),
            other => panic!("{other}"),
        },
        "target_session_missing"
    );
    assert!(
        f.scheduler.list().unwrap().is_empty(),
        "nothing was persisted"
    );

    let routine = f
        .scheduler
        .create(request(interval(60), new_session("job")))
        .await
        .unwrap();
    let retarget = UpdateRoutineRequest {
        action: Some(heartbeat_to(&ghost)),
        ..Default::default()
    };
    assert!(f.scheduler.update(&routine.id, retarget).await.is_err());
    assert!(
        matches!(
            f.scheduler.get(&routine.id).unwrap().action,
            RoutineAction::NewSession { .. }
        ),
        "the rejected retarget changed nothing"
    );
}

#[tokio::test]
async fn a_target_deleted_between_the_check_and_the_create_is_paused_by_the_deletion_hook() {
    let (_, host, scheduler) = shared("2026-10-12T00:00:00Z");
    let target = host.add_session("target");
    let stop = Arc::new(Checkpoint::default());
    host.exists_checkpoint(Some(stop.clone()));
    let creating = tokio::spawn({
        let (scheduler, target) = (scheduler.clone(), target.clone());
        async move {
            scheduler
                .create(request(interval(60), heartbeat_to(&target)))
                .await
        }
    });
    stop.entered.notified().await; // the target was seen alive; the routine is not stored yet
    host.exists_checkpoint(None);
    host.remove(&target);
    let hook = tokio::spawn({
        let (scheduler, target) = (scheduler.clone(), target.clone());
        async move { scheduler.on_session_deleted(&target).await }
    });
    quiesce().await;
    assert!(
        !hook.is_finished(),
        "the hook waits for the create that is mid-validation"
    );
    stop.release.notify_one();
    let routine = creating.await.unwrap().unwrap();
    hook.await.unwrap().unwrap();

    let stored = scheduler.get(&routine.id).unwrap();
    assert!(
        !stored.enabled,
        "the hook saw the committed routine and paused it"
    );
    assert_eq!(
        stored.paused_reason,
        Some(RoutinePauseReason::TargetMissing)
    );
    assert_eq!(
        statuses(&scheduler.runs(&routine.id, 5).await.unwrap()),
        [RunStatus::PausedTargetMissing]
    );
}

#[tokio::test]
async fn a_target_deleted_between_the_check_and_a_retarget_is_paused_by_the_deletion_hook() {
    let (_, host, scheduler) = shared("2026-10-12T00:00:00Z");
    let target = host.add_session("target");
    let routine = scheduler
        .create(request(interval(60), new_session("job")))
        .await
        .unwrap();
    let stop = Arc::new(Checkpoint::default());
    host.exists_checkpoint(Some(stop.clone()));
    let retargeting = tokio::spawn({
        let (scheduler, id, target) = (scheduler.clone(), routine.id.clone(), target.clone());
        async move {
            scheduler
                .update(
                    &id,
                    UpdateRoutineRequest {
                        action: Some(heartbeat_to(&target)),
                        ..Default::default()
                    },
                )
                .await
        }
    });
    stop.entered.notified().await;
    host.exists_checkpoint(None);
    host.remove(&target);
    let hook = tokio::spawn({
        let (scheduler, target) = (scheduler.clone(), target.clone());
        async move { scheduler.on_session_deleted(&target).await }
    });
    quiesce().await;
    assert!(!hook.is_finished());
    stop.release.notify_one();
    retargeting.await.unwrap().unwrap();
    hook.await.unwrap().unwrap();

    let stored = scheduler.get(&routine.id).unwrap();
    assert!(matches!(stored.action, RoutineAction::Heartbeat { .. }));
    assert!(!stored.enabled);
    assert_eq!(
        stored.paused_reason,
        Some(RoutinePauseReason::TargetMissing)
    );
}

#[tokio::test]
async fn team_routine_targets_home_and_retirement_pauses_without_fallback() {
    let f = fixture("2026-10-01T00:00:00Z");
    let home = f.host.add_session("mina-home");
    f.host.0.lock().agents.insert("mina".into(), home.clone());
    let routine = f
        .scheduler
        .create(CreateRoutineRequest {
            name: "Prices".into(),
            schedule: RoutineSchedule::Interval { secs: 60 },
            action: RoutineAction::Agent {
                agent_id: "mina".into(),
                prompt: "Compare".into(),
            },
            enabled: None,
            catch_up: None,
        })
        .await
        .unwrap();
    assert_eq!(routine.owner_agent_id.as_deref(), Some("mina"));
    f.clock.advance(Duration::seconds(60));
    let runs = f.scheduler.tick().await.unwrap();
    assert_eq!(runs[0].session_id.as_ref(), Some(&home));
    f.host.settle(&home);
    f.host.0.lock().agents.remove("mina");
    f.clock.advance(Duration::seconds(60));
    f.scheduler.tick().await.unwrap();
    let paused = f.scheduler.get(&routine.id).unwrap();
    assert!(!paused.enabled);
    assert_eq!(paused.paused_reason, Some(RoutinePauseReason::AgentRetired));
    assert_eq!(f.host.follow_ups().len(), 1);
}
