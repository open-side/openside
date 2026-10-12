use chrono::{DateTime, Utc};
use openside_core::{MessageId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::PolicyMode;

/// When a routine fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutineSchedule {
    /// POSIX five-field cron expression (minute hour day-of-month month day-of-week).
    Cron {
        /// Cron expression; `@hourly`/`@daily`-style aliases are accepted.
        expr: String,
        /// IANA time zone name; UTC when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    /// Fixed period anchored at the moment the routine was created or last rescheduled.
    Interval {
        /// Seconds between runs; at least one.
        secs: u64,
    },
}

/// What a routine does when it fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutineAction {
    /// Create a fresh session and submit the prompt as its first message.
    NewSession {
        /// Prompt submitted to the new session.
        prompt: String,
        /// Permission mode override, or the daemon default when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<PolicyMode>,
        /// Absolute workspace path, canonicalized by the daemon when the run starts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
        /// Model override, or the configured default when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Inject a follow-up (queue lane) message into an existing session.
    Heartbeat {
        /// Session that receives the message.
        session_id: SessionId,
        /// Message text.
        prompt: String,
    },
}

/// Why a routine is not firing although the user did not disable it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutinePauseReason {
    /// The heartbeat target session no longer exists.
    TargetMissing,
}

/// A scheduled routine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Routine {
    /// Opaque routine identifier assigned by the daemon.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Firing schedule.
    pub schedule: RoutineSchedule,
    /// Action performed on each run.
    pub action: RoutineAction,
    /// Whether the scheduler fires this routine; automatic pauses clear it too.
    pub enabled: bool,
    /// Set when the daemon paused the routine on its own; cleared by resuming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<RoutinePauseReason>,
    /// Run once for the latest occurrence missed while the daemon was down.
    pub catch_up: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Next scheduled occurrence; absent while the routine is not enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
}

/// Routine listing response, sorted by creation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutineList {
    /// All routines.
    pub routines: Vec<Routine>,
}

/// Routine creation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRoutineRequest {
    /// Human-readable name; must not be blank.
    pub name: String,
    /// Firing schedule.
    pub schedule: RoutineSchedule,
    /// Action performed on each run.
    pub action: RoutineAction,
    /// Start enabled; defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Run once for the latest occurrence missed while the daemon was down; defaults to false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up: Option<bool>,
}

/// Partial routine update; omitted fields are unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoutineRequest {
    /// New name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New schedule; the next run is recomputed from now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<RoutineSchedule>,
    /// New action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<RoutineAction>,
    /// Pause (`false`) or resume (`true`); resuming clears `paused_reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// New catch-up policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up: Option<bool>,
}

/// Disposition of one scheduled or manual run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The prompt was handed to a session that has not settled yet.
    Started,
    /// The session finished the run's message successfully.
    Completed,
    /// The run could not start, or its message failed, was aborted or dropped.
    Failed,
    /// The previous run was still in progress, so this one was skipped.
    SkippedOverlap,
    /// The heartbeat target no longer exists; the routine was paused.
    PausedTargetMissing,
    /// The daemon was not running at the scheduled time and the run was not replayed.
    Missed,
}

/// One entry of a routine's run history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunRecord {
    /// Monotonic run identifier, unique across routines.
    pub id: u64,
    /// Routine that produced the run.
    pub routine_id: String,
    /// Occurrence the run was scheduled for; the request time for manual runs.
    pub scheduled_at: DateTime<Utc>,
    /// When the prompt was dispatched; absent for runs that never started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the run reached a terminal status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// Current disposition.
    pub status: RunStatus,
    /// Session that received the prompt, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Message that carried the prompt, used to decide when the run has settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    /// Short sanitized explanation, e.g. why a run was skipped or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Run history response, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunList {
    /// Runs, newest first.
    pub runs: Vec<RunRecord>,
}

/// A repeated task found in the user's messages; never persisted until the user creates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutineDraft {
    /// Suggested routine name.
    pub name: String,
    /// Most recent wording of the repeated prompt.
    pub prompt: String,
    /// Guessed schedule: cron when the spacing fits one, otherwise an interval.
    pub schedule: RoutineSchedule,
    /// Number of occurrences the guess is based on.
    pub occurrences: u32,
    /// Sessions the occurrences came from.
    pub session_ids: Vec<SessionId>,
    /// First occurrence.
    pub first_seen: DateTime<Utc>,
    /// Latest occurrence.
    pub last_seen: DateTime<Utc>,
}

/// Repetition candidates response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutineSuggestionList {
    /// Draft routines, most frequent first; none of them exists yet.
    pub suggestions: Vec<RoutineDraft>,
}
