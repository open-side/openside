//! `GET /v1/tools` and `POST /v1/sessions/{id}/tools/{name}`: the session's browser and
//! human-step tools for API clients (CLI, MCP, scripts), so an external caller has the same
//! browser powers as the in-app agent through the public API (G-5).
//!
//! A call goes through [`Host::call`] exactly like an agent call: profile visibility, mode,
//! incognito vault refusal, policy, approval tickets, redaction, tool events and audit. Its
//! tool call id starts with [`super::API_CALL_PREFIX`], so the tickets it opens are answered by
//! people in the usual places but never delivered to the agent runtime; an approval is claimed
//! by the caller's rerun with `approval_ticket`, and a human step's resolution is its result.
//! `browser_repl` keeps its own route (`/repl`) because it asks for approvals itself.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_core::{DeferKind, HostToolHandler, ToolCall, ToolOutcome};
use openside_proto::{
    CapabilityScope, SessionId, TicketDeferral, ToolCallRequest, ToolCallResponse, ToolCallStatus,
    ToolCatalog, ToolDescriptor,
};
use serde_json::Value;

use crate::{
    api::{Access, Caller, HttpError},
    engine::Shared,
    host::Host,
    persistence::random_id,
};

const CALL_PREFIX: &str = "api-tool-";

/// Directly callable tools and the scope each needs beyond the route's `browse`. Everything
/// that reaches the browser or asks a person is here; workspace, shell, memory, skills and
/// agent orchestration stay with the agent.
const TOOLS: &[(&str, CapabilityScope)] = &[
    ("ask_user", CapabilityScope::Browse),
    ("browser_act", CapabilityScope::Browse),
    ("browser_close_tab", CapabilityScope::Browse),
    ("browser_navigate", CapabilityScope::Browse),
    ("browser_open_tab", CapabilityScope::Browse),
    ("browser_pointer", CapabilityScope::Browse),
    ("browser_screenshot", CapabilityScope::Browse),
    ("browser_snapshot", CapabilityScope::Browse),
    ("browser_tabs", CapabilityScope::Browse),
    ("register_authenticator", CapabilityScope::Browse),
    ("request_choice", CapabilityScope::Browse),
    ("request_human_step", CapabilityScope::Browse),
    ("request_login", CapabilityScope::Browse),
    ("request_otp", CapabilityScope::Browse),
    ("vault_login", CapabilityScope::VaultUse),
    ("vault_otp", CapabilityScope::VaultUse),
    ("vault_passkey", CapabilityScope::VaultUse),
    ("wait_for_approval", CapabilityScope::Browse),
];

fn scope(name: &str) -> Option<CapabilityScope> {
    TOOLS
        .iter()
        .find(|(tool, _)| *tool == name)
        .map(|(_, scope)| *scope)
}

pub(crate) async fn catalog(State(shared): State<Shared>) -> Json<ToolCatalog> {
    let tools = TOOLS
        .iter()
        .filter_map(|(name, scope)| {
            let spec = shared.registry.get(name)?.spec();
            Some(ToolDescriptor {
                name: spec.name,
                description: spec.description,
                input_schema: serde_json::from_str(&spec.parameters_json).unwrap_or(Value::Null),
                scope: *scope,
                human: spec.deferrable,
            })
        })
        .collect();
    Json(ToolCatalog { tools })
}

pub(crate) async fn route(
    State(shared): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Path((id, name)): Path<(String, String)>,
    Json(request): Json<ToolCallRequest>,
) -> Result<Json<ToolCallResponse>, HttpError> {
    let required = scope(&name).ok_or_else(HttpError::not_found)?;
    if !access.has(required) {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
    if !request.args.is_object() {
        return Err(HttpError::invalid_value());
    }
    let session = SessionId::new(id);
    let state = shared
        .db
        .state(&session)
        .await
        .map_err(|_| HttpError::not_found())?;
    caller.record_driver(&shared, &session).await?;
    let tool_call_id = match &request.approval_ticket {
        Some(ticket) => {
            let ticket = crate::tickets::get(&shared, ticket)
                .await
                .map_err(|_| HttpError::internal())?
                .ok_or_else(HttpError::not_found)?;
            // Only this route's own approval for the same tool resumes here; the host then
            // checks that the arguments are the approved ones.
            if ticket.session_id != session
                || !ticket.tool_call_id.starts_with(CALL_PREFIX)
                || ticket.tool.as_ref().map(|tool| tool.name.as_str()) != Some(name.as_str())
            {
                return Err(HttpError(StatusCode::CONFLICT, "invalid_transition"));
            }
            ticket.tool_call_id
        }
        None => format!("{CALL_PREFIX}{}", random_id()),
    };
    let outcome = Host::for_api(shared.clone(), &state, &caller)
        .call(ToolCall {
            tool_call_id: tool_call_id.clone(),
            name,
            arguments_json: request.args.to_string(),
            subagent: None,
            approval_ticket: request.approval_ticket,
        })
        .await;
    let mut response = ToolCallResponse {
        tool_call_id,
        status: ToolCallStatus::Failed,
        result: None,
        ticket_id: None,
        deferral: None,
        artifacts: Vec::new(),
        reason: None,
        hint: None,
    };
    match outcome {
        ToolOutcome::Done { for_model_json } => {
            response.status = ToolCallStatus::Done;
            response.result = serde_json::from_str(&for_model_json).ok();
        }
        // Binary parts are model input; API callers fetch the saved artifact instead.
        ToolOutcome::DoneWithParts {
            for_model_json,
            parts,
        } => {
            response.status = ToolCallStatus::Done;
            response.result = serde_json::from_str(&for_model_json).ok();
            response.artifacts = parts
                .into_iter()
                .filter_map(|part| part.artifact_ref)
                .collect();
        }
        ToolOutcome::Deferred { ticket, kind } => {
            response.status = ToolCallStatus::Deferred;
            response.ticket_id = Some(ticket);
            response.deferral = Some(match kind {
                DeferKind::Approval => TicketDeferral::Approval,
                DeferKind::External => TicketDeferral::External,
            });
        }
        ToolOutcome::Denied { reason, hint } => {
            response.status = ToolCallStatus::Denied;
            response.reason = Some(reason);
            response.hint = hint;
        }
        ToolOutcome::Failed { message } => response.reason = Some(message),
    }
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::TOOLS;

    #[test]
    fn listed_tools_are_sorted_and_exclude_agent_only_tools() {
        let names: Vec<_> = TOOLS.iter().map(|(name, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        for agent_only in [
            "bash",
            "write",
            "edit",
            "read",
            "browser_repl",
            "task_start",
            "delegate",
        ] {
            assert!(!names.contains(&agent_only), "{agent_only}");
        }
    }
}
