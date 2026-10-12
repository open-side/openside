//! `openside routine ...`: manage scheduled routines through `/v1/routines`.

use std::{io::Write, path::PathBuf};

use clap::{Args, Subcommand};
use openside_proto::{
    CreateRoutineRequest, Routine, RoutineAction, RoutineDraft, RoutineList, RoutinePauseReason,
    RoutineSchedule, RoutineSuggestionList, RunList, RunRecord, RunStatus, SessionId,
    UpdateRoutineRequest,
};
use serde_json::Value;

use crate::{client::Client, emit, output_error, Mode};

#[derive(Subcommand)]
pub(crate) enum RoutineCommand {
    /// List routines.
    List,
    /// Show one routine.
    Show { id: String },
    /// Create a routine that starts a new session or sends a heartbeat to an existing one.
    Add(AddArgs),
    /// Run a routine now, outside its schedule.
    Run { id: String },
    /// Show recent runs of a routine, newest first.
    Runs {
        id: String,
        #[arg(long, value_name = "N", help = "Maximum number of runs to show")]
        limit: Option<usize>,
    },
    /// Stop scheduling a routine until it is resumed.
    Pause { id: String },
    /// Resume a paused routine; the next run is computed from now.
    Resume { id: String },
    /// Delete a routine and its run history.
    Rm { id: String },
    /// Show routine drafts found in repeated requests; nothing is created.
    Suggest,
}

#[derive(Args)]
pub(crate) struct AddArgs {
    #[arg(long, value_name = "NAME")]
    name: String,
    #[arg(
        long,
        value_name = "EXPR",
        conflicts_with = "interval_secs",
        required_unless_present = "interval_secs",
        help = "Five-field cron expression"
    )]
    cron: Option<String>,
    #[arg(
        long,
        value_name = "ZONE",
        requires = "cron",
        help = "IANA time zone for --cron (default UTC)"
    )]
    timezone: Option<String>,
    #[arg(long, value_name = "SECS", help = "Fixed interval instead of --cron")]
    interval_secs: Option<u64>,
    #[arg(long, value_name = "TEXT", help = "Prompt sent on every run")]
    prompt: String,
    #[arg(
        long,
        value_name = "SESSION",
        help = "Send the prompt to this existing session instead of starting a new one"
    )]
    heartbeat: Option<String>,
    #[arg(long, value_name = "MODEL", conflicts_with = "heartbeat")]
    model: Option<String>,
    #[arg(long, value_enum, conflicts_with = "heartbeat")]
    mode: Option<Mode>,
    #[arg(long, value_name = "PATH", conflicts_with = "heartbeat")]
    workspace: Option<PathBuf>,
    #[arg(
        long,
        help = "Run once for the latest occurrence missed while the daemon was down"
    )]
    catch_up: bool,
    #[arg(long, help = "Create the routine paused")]
    disabled: bool,
}

pub(crate) async fn execute<W: Write>(
    command: &RoutineCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        RoutineCommand::List => {
            let list: RoutineList = client.get("/v1/routines").await?;
            if json {
                return emit(stdout, &list);
            }
            for routine in &list.routines {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}",
                    routine.id,
                    state(routine),
                    schedule_text(&routine.schedule),
                    routine.name
                )
                .map_err(output_error)?;
            }
            Ok(())
        }
        RoutineCommand::Show { id } => {
            let routine: Routine = client.get(&routine_path(id, "")).await?;
            print_routine(json, stdout, &routine)
        }
        RoutineCommand::Add(add) => {
            let routine: Routine = client
                .json("POST", "/v1/routines", Some(&create_request(add)?))
                .await?;
            print_routine(json, stdout, &routine)
        }
        RoutineCommand::Run { id } => {
            let run: RunRecord = client
                .json::<_, Value>("POST", &routine_path(id, "run"), None)
                .await?;
            if json {
                return emit(stdout, &run);
            }
            print_run(stdout, &run)
        }
        RoutineCommand::Runs { id, limit } => {
            let path = match limit {
                Some(limit) => format!("{}?limit={limit}", routine_path(id, "runs")),
                None => routine_path(id, "runs"),
            };
            let list: RunList = client.get(&path).await?;
            if json {
                return emit(stdout, &list);
            }
            for run in &list.runs {
                print_run(stdout, run)?;
            }
            Ok(())
        }
        RoutineCommand::Pause { id } => set_enabled(json, client, stdout, id, false).await,
        RoutineCommand::Resume { id } => set_enabled(json, client, stdout, id, true).await,
        RoutineCommand::Rm { id } => {
            let routine: Routine = client
                .json::<_, Value>("DELETE", &routine_path(id, ""), None)
                .await?;
            if json {
                return emit(stdout, &routine);
            }
            writeln!(stdout, "Deleted routine {} ({})", routine.id, routine.name)
                .map_err(output_error)
        }
        RoutineCommand::Suggest => {
            let found: RoutineSuggestionList = client.get("/v1/routines/suggestions").await?;
            if json {
                return emit(stdout, &found);
            }
            for draft in &found.suggestions {
                print_draft(stdout, draft)?;
            }
            Ok(())
        }
    }
}

async fn set_enabled<W: Write>(
    json: bool,
    client: &Client,
    stdout: &mut W,
    id: &str,
    enabled: bool,
) -> Result<(), String> {
    let routine: Routine = client
        .json(
            "PATCH",
            &routine_path(id, ""),
            Some(&UpdateRoutineRequest {
                enabled: Some(enabled),
                ..UpdateRoutineRequest::default()
            }),
        )
        .await?;
    print_routine(json, stdout, &routine)
}

fn create_request(add: &AddArgs) -> Result<CreateRoutineRequest, String> {
    let schedule = match (&add.cron, add.interval_secs) {
        (Some(expr), None) => RoutineSchedule::Cron {
            expr: expr.clone(),
            timezone: add.timezone.clone(),
        },
        (None, Some(_)) if add.timezone.is_some() => {
            return Err("--timezone only applies to --cron".into())
        }
        (None, Some(secs)) => RoutineSchedule::Interval { secs },
        _ => return Err("exactly one of --cron or --interval-secs is required".into()),
    };
    let action = match &add.heartbeat {
        Some(session) => RoutineAction::Heartbeat {
            session_id: SessionId::new(session.as_str()),
            prompt: add.prompt.clone(),
        },
        None => RoutineAction::NewSession {
            prompt: add.prompt.clone(),
            mode: add.mode.map(Into::into),
            workspace: add
                .workspace
                .as_ref()
                .map(|path| {
                    path.canonicalize()
                        .map_err(|_| "could not canonicalize --workspace".to_owned())?
                        .into_os_string()
                        .into_string()
                        .map_err(|_| "--workspace must be valid UTF-8".to_owned())
                })
                .transpose()?,
            model: add.model.clone(),
        },
    };
    Ok(CreateRoutineRequest {
        name: add.name.clone(),
        schedule,
        action,
        enabled: add.disabled.then_some(false),
        catch_up: add.catch_up.then_some(true),
    })
}

fn routine_path(id: &str, suffix: &str) -> String {
    let encoded = crate::client::encode_segment(id);
    if suffix.is_empty() {
        format!("/v1/routines/{encoded}")
    } else {
        format!("/v1/routines/{encoded}/{suffix}")
    }
}

fn state(routine: &Routine) -> &'static str {
    match (routine.enabled, routine.paused_reason) {
        (true, _) => "enabled",
        (false, Some(RoutinePauseReason::TargetMissing)) => "paused:target_missing",
        (false, None) => "paused",
    }
}

fn schedule_text(schedule: &RoutineSchedule) -> String {
    match schedule {
        RoutineSchedule::Cron { expr, timezone } => {
            format!("cron {expr} {}", timezone.as_deref().unwrap_or("UTC"))
        }
        RoutineSchedule::Interval { secs } => format!("every {secs}s"),
    }
}

fn print_routine<W: Write>(json: bool, stdout: &mut W, routine: &Routine) -> Result<(), String> {
    if json {
        return emit(stdout, routine);
    }
    let target = match &routine.action {
        RoutineAction::NewSession { .. } => "new session".to_owned(),
        RoutineAction::Heartbeat { session_id, .. } => format!("heartbeat to {session_id}"),
    };
    let next = routine
        .next_run_at
        .map_or_else(|| "-".to_owned(), |at| at.to_rfc3339());
    writeln!(
        stdout,
        "{}\t{}\t{}\t{}\tnext {}\t{}",
        routine.id,
        state(routine),
        schedule_text(&routine.schedule),
        target,
        next,
        routine.name
    )
    .map_err(output_error)
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

fn print_run<W: Write>(stdout: &mut W, run: &RunRecord) -> Result<(), String> {
    writeln!(
        stdout,
        "{}\t{}\t{}\t{}\t{}",
        run.id,
        run.scheduled_at.to_rfc3339(),
        status_text(run.status),
        run.session_id.as_ref().map_or("-", |id| id.as_str()),
        run.detail.as_deref().unwrap_or("")
    )
    .map_err(output_error)
}

fn print_draft<W: Write>(stdout: &mut W, draft: &RoutineDraft) -> Result<(), String> {
    writeln!(
        stdout,
        "{}\t{}x\t{}\t{}",
        draft.name,
        draft.occurrences,
        schedule_text(&draft.schedule),
        draft.prompt
    )
    .map_err(output_error)
}
