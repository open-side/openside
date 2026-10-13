use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, ToolSpec};
use openside_policy::Action;
use openside_proto::{CreateRoutineRequest, Ticket, TicketDeferral, TicketKind};
use serde_json::{json, Value};

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    tools::{self, DaemonTool, TicketRequest, ToolEnv, ToolError, ToolExec},
};

pub(crate) const PROPOSE_ROUTINE: &str = "propose_routine";
const PAYLOAD_KEEP: &[&[&str]] = &[
    &["schedule", "type"],
    &["action", "type"],
    &["action", "mode"],
    &["action", "agent_id"],
    &["action", "session_id"],
];

// These identifiers and enum tags have passed scheduler validation. The instructions are
// rendered only from the secret-screened payload, so repeated redaction cannot change consent.
pub(crate) const TICKET_KEEP: &[&[&str]] = &[
    &["instructions"],
    &["tool", "args", "schedule", "type"],
    &["tool", "args", "action", "type"],
    &["tool", "args", "action", "mode"],
    &["tool", "args", "action", "agent_id"],
    &["tool", "args", "action", "session_id"],
];

pub(crate) fn is_proposal(ticket: &Ticket) -> bool {
    ticket.kind == TicketKind::Confirm
        && ticket.deferral == TicketDeferral::External
        && ticket
            .tool
            .as_ref()
            .is_some_and(|tool| tool.name == PROPOSE_ROUTINE)
}

fn request(args: &Value) -> std::result::Result<CreateRoutineRequest, ToolError> {
    let mut request: CreateRoutineRequest = tools::parse(args)?;
    let name = request.name.trim();
    if name.len() != request.name.len() {
        request.name = name.to_owned();
    }
    request.enabled = Some(request.enabled.unwrap_or(true));
    request.catch_up = Some(request.catch_up.unwrap_or(false));
    Ok(request)
}

pub(crate) fn screen_payload(shared: &Shared, args: &Value) -> crate::persistence::Result<()> {
    if shared.registry.redact_except(args.clone(), PAYLOAD_KEEP)? != *args {
        return Err("routine_contains_secret".into());
    }
    Ok(())
}

pub(crate) struct ProposeRoutine;

impl DaemonTool for ProposeRoutine {
    fn name(&self) -> &'static str {
        PROPOSE_ROUTINE
    }

    fn spec(&self) -> ToolSpec {
        tools::spec::<CreateRoutineRequest>(
            self.name(),
            "Propose a scheduled routine for the person's approval. Supply its exact name, schedule and action (prompt and target). Nothing is scheduled until the confirmation is approved. Prefer an agent target for recurring chief/teammate work. Changed payloads require a new proposal; never claim a proposal is already running.",
            ApprovalTier::Read,
            false,
        )
    }

    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }

    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            request(args)?;
            Ok(Vec::new())
        })
    }

    fn redact_args(&self, args: &Value) -> Value {
        request(args)
            .ok()
            .and_then(|request| serde_json::to_value(request).ok())
            .unwrap_or(Value::Null)
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let request = match request(&args) {
                Ok(request) => request,
                Err(error) => return ToolExec::Failed(error.0),
            };
            let Some(scheduler) = ctx.shared.routines.get() else {
                return ToolExec::Denied {
                    reason: "routines_unavailable".into(),
                    hint: Some(
                        "Ask the person to restart the daemon before proposing a routine".into(),
                    ),
                };
            };
            if let Err(error) = scheduler.validate_request(&request).await {
                return ToolExec::Denied {
                    reason: match error {
                        openside_sched::Error::Invalid(invalid) => invalid.code().into(),
                        _ => "routine_validation_failed".into(),
                    },
                    hint: Some(
                        "Use a valid schedule, a nonempty prompt and an existing active target"
                            .into(),
                    ),
                };
            }
            let payload = match serde_json::to_value(&request) {
                Ok(payload) => payload,
                Err(_) => return ToolExec::Failed("Routine proposal could not be encoded".into()),
            };
            if screen_payload(&ctx.shared, &payload).is_err() {
                return ToolExec::Denied {
                    reason: "routine_contains_secret".into(),
                    hint: Some(
                        "Remove secret values from the routine; use a saved connection instead"
                            .into(),
                    ),
                };
            }
            let instructions = match serde_json::to_string_pretty(&payload) {
                Ok(payload) => format!(
                    "Approve this exact routine configuration. Cron times without a timezone use UTC.\n{payload}\nChanging the prompt, schedule or target requires a new proposal."
                ),
                Err(_) => return ToolExec::Failed("Routine proposal could not be encoded".into()),
            };
            ToolExec::Defer(Box::new(TicketRequest {
                kind: TicketKind::Confirm,
                summary: format!("Create routine: {}", request.name),
                instructions: Some(instructions),
                options: Vec::new(),
                target: None,
                resume_when: None,
                otp: None,
                otp_register: None,
                login: None,
                choice: None,
                approval: None,
                form: None,
                hire: None,
                pins: Vec::new(),
            }))
        })
    }
}

pub(crate) fn interrupted(shared: &Shared, ticket: &Ticket) -> Option<Value> {
    if !is_proposal(ticket) {
        return None;
    }
    let scheduler = shared.routines.get()?;
    let payload = &ticket.tool.as_ref()?.args;
    let Ok(request) = tools::parse::<CreateRoutineRequest>(payload) else {
        return Some(json!({"status": "indeterminate"}));
    };
    match scheduler.approved_routine(ticket.id.as_str(), &request) {
        Ok(Some(id)) => Some(json!({"approved": true, "status": "created", "routine_id": id})),
        Ok(None) => None,
        // Unknown persistence state cannot truthfully be presented as a rejection.
        Err(_) => Some(json!({"status": "indeterminate"})),
    }
}

/// Called under the existing ticket gate; no model-supplied approval payload is accepted.
pub(crate) async fn resolve(
    shared: &Shared,
    ticket: &Ticket,
    caller: &Caller,
    value: Value,
) -> std::result::Result<Value, HttpError> {
    if !is_proposal(ticket) {
        return Err(HttpError::invalid_value());
    }
    if value != true && value != false && value != "approve" && value != "reject" {
        return Err(HttpError::invalid_value());
    }
    if let Some(receipt) = interrupted(shared, ticket) {
        return Ok(receipt);
    }
    if !crate::tickets::approved(&value) {
        return Ok(json!({"approved": false, "status": "declined"}));
    }
    let payload = &ticket
        .tool
        .as_ref()
        .ok_or_else(HttpError::invalid_value)?
        .args;
    screen_payload(shared, payload)
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "routine_contains_secret"))?;
    let request =
        tools::parse::<CreateRoutineRequest>(payload).map_err(|_| HttpError::invalid_value())?;
    if let openside_proto::RoutineAction::NewSession {
        mode, workspace, ..
    } = &request.action
    {
        if caller.widens(shared, *mode, workspace.as_deref()) {
            return Err(HttpError(StatusCode::FORBIDDEN, "local_socket_required"));
        }
    }
    let scheduler = shared.routines.get().ok_or(HttpError(
        StatusCode::SERVICE_UNAVAILABLE,
        "routines_unavailable",
    ))?;
    let routine = scheduler
        .create_approved(ticket.id.as_str(), request)
        .await
        .map_err(|error| match error {
            openside_sched::Error::Invalid(invalid) => {
                HttpError(StatusCode::BAD_REQUEST, invalid.code())
            }
            openside_sched::Error::UnknownRoutine(_) => {
                HttpError(StatusCode::CONFLICT, "approved_routine_deleted")
            }
            _ => HttpError::internal(),
        })?;
    Ok(json!({"approved": true, "status": "created", "routine_id": routine.id}))
}
