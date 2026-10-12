//! Routine scheduler: cron and interval routines that start sessions or send heartbeats.
//!
//! A [`Routine`](openside_proto::Routine) pairs a [`RoutineSchedule`](openside_proto::RoutineSchedule)
//! (exactly five-field POSIX cron with an optional IANA time zone, UTC by default, or a fixed
//! interval) with a [`RoutineAction`](openside_proto::RoutineAction): create a new session
//! with a prompt, or inject a follow-up into an existing session (heartbeat). The
//! [`Scheduler`] owns persistence in SQLite (`routines.db`) and all decisions; the
//! daemon supplies a [`RoutineHost`] that performs the side effects, and a [`Clock`]
//! so tests advance time without sleeping.
//!
//! Behaviour contract:
//! - Every mutation and every firing pass is serialised by one gate, so a pause, delete or
//!   schedule change never races a dispatch that is in flight.
//! - A run is stored, with the session and message ids it will use, before the host acts;
//!   after a crash the open run still names its message and keeps overlap rules intact.
//! - A run whose previous run has not settled is skipped and recorded `skipped_overlap`.
//! - A heartbeat whose target session is gone pauses the routine
//!   (`enabled = false`, `paused_reason = target_missing`) and records
//!   `paused_target_missing`; [`Scheduler::on_session_deleted`] does the same eagerly.
//! - [`Scheduler::run_now`] fires outside the schedule and leaves the next occurrence alone.
//! - [`Scheduler::recover`] runs at daemon start: occurrences missed while down are
//!   recorded `missed` and never replayed; `catch_up` allows one run for the latest.
//! - [`suggest::suggest`] drafts routines from messages the user keeps repeating.
//!
//! The cron engine is the `croner` crate (MIT).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod clock;
pub mod schedule;
mod scheduler;
mod store;
pub mod suggest;

pub use clock::{Clock, ManualClock, SystemClock};
pub use scheduler::{
    Dispatch, HostError, NewSessionSpec, Progress, RoutineHost, Scheduler, DEFAULT_RUN_LIMIT,
    MAX_MISSED_RECORDS,
};
pub use store::Store;
pub use suggest::UserMessage;

/// Message-id prefix of prompts sent by routines; they are not user repetition.
pub const ROUTINE_MESSAGE_PREFIX: &str = "routine-";

/// Why a routine definition was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Invalid {
    /// The name is blank.
    #[error("name must not be blank")]
    Name,
    /// The cron expression or interval is unusable.
    #[error("{0}")]
    Schedule(String),
    /// The time zone is not an IANA name.
    #[error("unknown time zone {0:?}")]
    Timezone(String),
    /// The action is incomplete.
    #[error("{0}")]
    Action(String),
    /// The heartbeat target session does not exist.
    #[error("heartbeat target session does not exist")]
    TargetMissing,
}

impl Invalid {
    /// Stable machine-readable code for API errors.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Name => "invalid_value",
            Self::Schedule(_) => "invalid_schedule",
            Self::Timezone(_) => "invalid_timezone",
            Self::Action(_) => "invalid_action",
            Self::TargetMissing => "target_session_missing",
        }
    }
}

/// Scheduler failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQLite open, query or durability failure.
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Stored routine JSON could not be encoded.
    #[error("routine JSON encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    /// No routine has this identifier.
    #[error("unknown routine {0}")]
    UnknownRoutine(String),
    /// The routine definition was rejected.
    #[error(transparent)]
    Invalid(#[from] Invalid),
}

/// Result of a scheduler operation.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests;
