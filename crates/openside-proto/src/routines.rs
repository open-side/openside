use chrono::{DateTime, Utc};
use openside_core::{MessageId, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::PolicyMode;

/// Wall-clock or elapsed-time trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutineSchedule {
    /// POSIX minute/hour/day/month/weekday.
    Cron {
        /// Accepts @hourly/@daily aliases.
        expr: String,
        /// IANA time zone name; UTC when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    /// Anchored at creation or last reschedule.
    Interval {
        /// Seconds; >=1.
        secs: u64,
    },
}

/// Dispatch destination and payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutineAction {
    /// First message in a fresh session.
    NewSession {
        /// Initial user message.
        prompt: String,
        /// Omitted: daemon default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<PolicyMode>,
        /// Absolute path; canonicalized at dispatch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
        /// Omitted: configured model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Active owner's permanent home thread.
    Agent {
        /// Resolved at dispatch; never falls back to a session.
        agent_id: String,
        /// Queued only while owner is active.
        prompt: String,
    },
    /// Queue a follow-up in an existing session.
    Heartbeat {
        /// Existing destination.
        session_id: SessionId,
        /// Queued user message.
        prompt: String,
    },
}

impl RoutineAction {
    /// Derive ownership from dispatch intent instead of a second stored assignment.
    pub fn owner_agent_id(&self) -> Option<&str> {
        match self {
            Self::Agent { agent_id, .. } => Some(agent_id),
            _ => None,
        }
    }
}

/// Why a routine is not firing although the user did not disable it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutinePauseReason {
    /// The heartbeat target session no longer exists.
    TargetMissing,
    /// The assigned agent is retired or no longer exists.
    AgentRetired,
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
    /// Derived from the action; not a separately stored ownership record.
    #[serde(default)]
    pub owner_agent_id: Option<String>,
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

/// Schedule and dispatch configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRoutineRequest {
    /// Nonblank display name.
    pub name: String,
    /// Determines next occurrence.
    pub schedule: RoutineSchedule,
    /// Runs on each occurrence.
    pub action: RoutineAction,
    /// Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Replay latest missed occurrence after downtime; default false.
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
