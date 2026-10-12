use std::{future::Future, pin::Pin, time::Instant};

use openside_core::{DeferKind, HostToolHandler, ToolCall, ToolOutcome, ToolSpec};
use openside_policy::{Action, Capability, Effect};
use openside_proto::{EventKind, TicketDeferral, TicketKind, TicketTarget, TicketTool};
use openside_sessions::{ApprovalClaim, SessionState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    engine::Shared,
    persistence::Result,
    tools::{self, TicketRequest, ToolEnv, ToolExec},
};

pub(crate) struct Host {
    pub(crate) env: ToolEnv,
    caller: Option<String>,
}
impl Host {
    pub(crate) fn new(shared: Shared, state: &SessionState) -> Self {
        Self {
            env: ToolEnv::new(shared, state),
            caller: None,
        }
    }
    pub(crate) fn for_api(
        shared: Shared,
        state: &SessionState,
        caller: &crate::api::Caller,
    ) -> Self {
        Self {
            env: ToolEnv::new(shared, state),
            caller: Some(caller.id().to_owned()),
        }
    }
    pub(crate) fn specs(&self) -> Vec<ToolSpec> {
        self.env.shared.registry.specs(&self.env)
    }

    async fn execute(
        &self,
        call: &ToolCall,
        cancellation: &tokio_util::sync::CancellationToken,
        browser_probe: &Option<crate::browser_state::ProbeBinding>,
    ) -> Result<(ToolOutcome, Option<Value>)> {
        let shared = &self.env.shared;
        if cancellation.is_cancelled() {
            return Ok((denied("subagent_finished", "The subagent has ended"), None));
        }
        let Some(tool) = shared.registry.get(&call.name) else {
            return Ok((
                denied("Unknown tool", "Use a tool from the exposed allowlist"),
                None,
            ));
        };
        if !tools::visible(tool.as_ref(), &self.env) {
            return Ok((
                denied(
                    "Tool is not exposed in this session",
                    "Use an exposed read-only tool or change the session mode",
                ),
                None,
            ));
        }
        if let Some((reason, hint)) = crate::tasks::front_denial(shared, call) {
            return Ok((denied(reason, hint), None));
        }
        if let Some(subagent) = &call.subagent {
            if let Some(reason) =
                crate::subagents::guard(shared, &self.env.session, subagent, &call.name).await?
            {
                return Ok((denied(reason, "Use a tool granted to this subagent"), None));
            }
        }
        // B-10: incognito sessions never reach the Vault (unknown state counts as incognito).
        // A secret_input human step is the Vault's other entry point (policy treats it as
        // `VaultUse`): the secret is filled through the same sink, so it is refused too, as are
        // the one-time-code tools and request_login, which read or store vault items.
        let vault_entry = call.name.starts_with("vault_")
            || matches!(
                call.name.as_str(),
                "request_otp" | "register_authenticator" | "request_login"
            )
            || (call.name == "request_human_step"
                && serde_json::from_str::<Value>(&call.arguments_json)
                    .is_ok_and(|args| args["kind"] == "secret_input"));
        if vault_entry
            && shared
                .db
                .state(&self.env.session)
                .await
                .map_or(true, |state| state.incognito)
        {
            return Ok((
                denied(
                    "vault_incognito",
                    "The Vault is unavailable in incognito sessions",
                ),
                None,
            ));
        }
        let args: Value = match serde_json::from_str(&call.arguments_json) {
            Ok(args) => args,
            Err(_) => {
                return Ok((
                    denied(
                        "Invalid JSON arguments",
                        "Send a JSON object matching the tool schema",
                    ),
                    None,
                ))
            }
        };
        // HS-6: opening a takeover first waits out the agent's in-flight browser operation.
        let _takeover = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok((denied("subagent_finished", "The subagent has ended"), None)),
            takeover = crate::takeover::reserve(shared, &self.env.session, &call.name, &args) => takeover,
        };
        // Keep daemon navigation/fill operations serialized through policy and execution.
        let mut browser_gate = if tools::touches_browser(&call.name) {
            Some(tokio::select! {
                biased;
                () = cancellation.cancelled() => return Ok((denied("subagent_finished", "The subagent has ended"), None)),
                gate = shared.registry.browser.gate.lock() => gate,
            })
        } else {
            None
        };
        // HS-6: a tab under human takeover belongs to the person until the ticket ends.
        if let Some((reason, hint)) =
            crate::takeover::blocked(shared, &self.env.session, &call.name, &args).await
        {
            return Ok((denied(reason, &hint), None));
        }
        let admission = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok((denied("subagent_finished", "The subagent has ended"), None)),
            actions = tool.actions(&args, &self.env) => actions,
        };
        let actions = match admission {
            Ok(actions) => actions,
            Err(error) => {
                return Ok((
                    denied(&error.0, "Correct the arguments using the tool schema"),
                    None,
                ))
            }
        };
        // Live daemon policy is re-read on every call so PUT /v1/policy reaches running sessions.
        let policy = crate::policy_api::effective(shared, &self.env.policy);
        let effect = actions
            .iter()
            .map(|action| policy.decide(action).effect)
            .max()
            .unwrap_or(Effect::Allow);
        if effect == Effect::Deny {
            return Ok((
                denied(
                    "Policy denied this action; approval cannot override Deny",
                    "Choose a permitted action",
                ),
                None,
            ));
        }
        let mut _navigation_grant = None;
        // Even an otherwise allowed action cannot carry a forged approval reference.
        if let Some(ticket) = &call.approval_ticket {
            let session = self.env.session.clone();
            let ticket = ticket.clone();
            let tool_call_id = call.tool_call_id.clone();
            let name = call.name.clone();
            let subagent = call.subagent.clone();
            let binding = approval_binding(&args, &actions)?;
            let claim = shared
                .db
                .store(move |store| {
                    Ok(store.claim_approval(
                        &session,
                        &ticket,
                        &tool_call_id,
                        &name,
                        subagent.as_deref(),
                        &binding,
                    )?)
                })
                .await?;
            match claim {
                // The approved Ask origins are reachable only while this call runs.
                ApprovalClaim::Claimed => {
                    _navigation_grant = Some(
                        shared.registry.browser.grant_navigation(
                            &self.env.session,
                            actions
                                .iter()
                                .filter(|action| {
                                    action.capability == Capability::Browse
                                        && policy.decide(action).effect == Effect::Ask
                                })
                                .filter_map(|action| openside_policy::origin_of(&action.target))
                                .collect(),
                        ),
                    );
                }
                ApprovalClaim::AlreadyUsed => {
                    return Ok((
                        denied(
                            "approval_already_used",
                            "Request a new approval before retrying",
                        ),
                        None,
                    ))
                }
                ApprovalClaim::ArgsMismatch => {
                    return Ok((
                        denied(
                            "approval_args_mismatch",
                            "Use exactly the approved arguments and action targets",
                        ),
                        None,
                    ))
                }
                ApprovalClaim::Invalid => return Ok((
                    denied(
                        "Approval ticket does not authorize this tool call",
                        "Use the resolved approved ticket for this session, tool and tool_call_id",
                    ),
                    None,
                )),
            }
        } else if effect == Effect::Ask {
            let safe = shared.registry.redact(tool.redact_args(&args))?;
            // Lock order is ticket_gate → browser gate (see `Controller::gate`): an HTTP answer
            // holds ticket_gate while it waits for the browser gate, so this call must not
            // wait for ticket_gate with the browser gate held. Nothing ran in the browser yet.
            drop(browser_gate.take());
            let ticket = crate::tickets::open(
                shared,
                &self.env.session,
                call,
                TicketDeferral::Approval,
                TicketRequest {
                    kind: TicketKind::Confirm,
                    summary: format!("Approve {}", tool.name()),
                    instructions: None,
                    options: Vec::new(),
                    // D-4: the approval object names the origin it would act on.
                    target: actions
                        .iter()
                        .find_map(|action| openside_policy::origin_of(&action.target))
                        .map(|origin| TicketTarget {
                            tab_id: None,
                            origin: Some(origin),
                            field_ref: None,
                        }),
                    resume_when: None,
                    otp: None,
                    otp_register: None,
                    login: None,
                    choice: None,
                    approval: None,
                    pins: Vec::new(),
                },
                Some(TicketTool {
                    name: call.name.clone(),
                    args: safe,
                }),
                Some(approval_binding(&args, &actions)?),
                self.caller.as_deref(),
            )
            .await?;
            return Ok((
                ToolOutcome::Deferred {
                    ticket,
                    kind: DeferKind::Approval,
                },
                None,
            ));
        }
        // Policy and the browser gate may have waited; a child cancelled meanwhile must not act.
        if cancellation.is_cancelled() {
            return Ok((denied("subagent_finished", "The subagent has ended"), None));
        }
        if let Some(subagent) = &call.subagent {
            if !crate::subagents::is_live(shared, &self.env.session, subagent).await? {
                return Ok((denied("subagent_finished", "The subagent has ended"), None));
            }
            // A newly opened tab can acquire its creator while this call waits for the
            // browser gate; never act on an earlier reservation without rechecking it.
            if let Some((reason, hint)) =
                crate::browser_state::tab_denial(shared, &self.env.session, call).await?
            {
                return Ok((denied(reason, hint), None));
            }
        }
        let env = ToolEnv {
            shared: shared.clone(),
            session: self.env.session.clone(),
            mode: self.env.mode,
            workspace: self.env.workspace.clone(),
            policy,
            approved_actions: actions,
            tool_call_id: call.tool_call_id.clone(),
            subagent: call.subagent.clone(),
            cancellation: cancellation.clone(),
            browser_probe: browser_probe.clone(),
        };
        let tracker = crate::artifacts::Tracker::before(&env, &call.name).await;
        let used_tab = args
            .get("tab_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        // Dropping a future does not cancel an issued CDP command or blocking filesystem I/O.
        // Cooperative tools stop themselves; all others settle before terminal publication.
        let executed = tool.call(args, &env).await;
        let result = if cancellation.is_cancelled() {
            ToolExec::Failed("Task tool cancelled".into())
        } else {
            executed
        };
        match result {
            ToolExec::Done { for_model, for_app } => {
                if call.name.starts_with("browser_") {
                    if let Some(tab) = for_model
                        .get("tab_id")
                        .and_then(Value::as_str)
                        .or(used_tab.as_deref())
                    {
                        shared.registry.browser.note_tab(&env.session, tab);
                    }
                }
                tracker.record(&env, &call.name).await;
                let for_model = shared.registry.redact_except(for_model, &[&["tab_id"]])?;
                let for_app = for_app
                    .map(|value| shared.registry.redact(value))
                    .transpose()?;
                Ok((
                    ToolOutcome::Done {
                        for_model_json: limit_output(for_model),
                    },
                    for_app,
                ))
            }
            ToolExec::DoneWithParts {
                for_model,
                for_app,
                parts,
            } => {
                if call.name.starts_with("browser_") {
                    if let Some(tab) = for_model
                        .get("tab_id")
                        .and_then(Value::as_str)
                        .or(used_tab.as_deref())
                    {
                        shared.registry.browser.note_tab(&env.session, tab);
                    }
                }
                tracker.record(&env, &call.name).await;
                let for_model = shared.registry.redact_except(for_model, &[&["tab_id"]])?;
                let for_app = for_app
                    .map(|value| shared.registry.redact(value))
                    .transpose()?;
                Ok((
                    ToolOutcome::DoneWithParts {
                        for_model_json: limit_output(for_model),
                        parts,
                    },
                    for_app,
                ))
            }
            ToolExec::Defer(request) => {
                // Opening a ticket pins its field under the browser gate itself
                // (`Controller::capture_ticket`); a deferring browser tool (request_otp,
                // register_authenticator) has finished its own browser work by now.
                drop(browser_gate.take());
                let ticket = crate::tickets::open(
                    shared,
                    &self.env.session,
                    call,
                    TicketDeferral::External,
                    *request,
                    None,
                    None,
                    self.caller.as_deref(),
                )
                .await?;
                Ok((
                    ToolOutcome::Deferred {
                        ticket,
                        kind: DeferKind::External,
                    },
                    None,
                ))
            }
            ToolExec::Denied { reason, hint } => Ok((ToolOutcome::Denied { reason, hint }, None)),
            ToolExec::Failed(message) => Ok((ToolOutcome::Failed { message }, None)),
        }
    }

    async fn recorded(&self, call: ToolCall) -> Result<ToolOutcome> {
        let shared = &self.env.shared;
        let started = Instant::now();
        let args = shared
            .registry
            .get(&call.name)
            .and_then(|tool| {
                serde_json::from_str(&call.arguments_json)
                    .ok()
                    .map(|args| tool.redact_args(&args))
            })
            .unwrap_or(Value::Null);
        let args = shared.registry.redact(args)?;
        shared
            .emit(
                &self.env.session,
                None,
                EventKind::ToolStarted {
                    tool_call_id: call.tool_call_id.clone(),
                    name: call.name.clone(),
                    args,
                },
            )
            .await?;
        let active = call
            .subagent
            .as_ref()
            .map(|task| shared.host_calls.enter(&self.env.session, task));
        let cancellation = active
            .as_ref()
            .map(|active| active.token())
            .unwrap_or_default();
        let mut stamp = crate::browser_state::before(shared, &self.env.session, &call).await;
        let browser_probe = (call.name == "browser_repl"
            && serde_json::from_str::<Value>(&call.arguments_json)
                .is_ok_and(|args| args["tab_id"].as_str().is_none()))
        .then(|| std::sync::Arc::new(parking_lot::Mutex::new(stamp.take())));
        let (mut outcome, for_app) = self
            .execute(&call, &cancellation, &browser_probe)
            .await
            .unwrap_or_else(|_| {
                (
                    ToolOutcome::Failed {
                        message: "Tool execution or durable recording failed".into(),
                    },
                    None,
                )
            });
        match &mut outcome {
            ToolOutcome::Denied { reason, hint } => {
                *reason = shared
                    .registry
                    .redact(Value::String(std::mem::take(reason)))?
                    .as_str()
                    .unwrap_or_default()
                    .into();
                if let Some(hint) = hint {
                    *hint = shared
                        .registry
                        .redact(Value::String(std::mem::take(hint)))?
                        .as_str()
                        .unwrap_or_default()
                        .into();
                }
            }
            ToolOutcome::Failed { message } => {
                *message = shared
                    .registry
                    .redact(Value::String(std::mem::take(message)))?
                    .as_str()
                    .unwrap_or_default()
                    .into();
            }
            _ => {}
        }
        let (result, is_error, decision) = match &outcome {
            ToolOutcome::Done { for_model_json }
            | ToolOutcome::DoneWithParts { for_model_json, .. } => (
                match for_app {
                    Some(value) => value,
                    None => serde_json::from_str(for_model_json)?,
                },
                false,
                "allowed",
            ),
            ToolOutcome::Deferred { ticket, kind } => (
                json!({"status":"deferred","ticket_id":ticket,"kind":kind}),
                false,
                "deferred",
            ),
            ToolOutcome::Denied { reason, hint } => (
                json!({"status":"denied","reason":reason,"hint":hint}),
                true,
                "denied",
            ),
            ToolOutcome::Failed { message } => {
                (json!({"status":"error","message":message}), true, "failed")
            }
        };
        let result = shared.registry.redact(result)?;
        shared
            .emit(
                &self.env.session,
                None,
                EventKind::ToolEnded {
                    tool_call_id: call.tool_call_id.clone(),
                    result,
                    is_error,
                },
            )
            .await?;
        if let Some(binding) = browser_probe {
            stamp = binding.lock().take();
        }
        crate::browser_state::after(shared, &self.env.session, &call, &outcome, stamp);
        crate::tickets::audit(
            shared,
            &self.env.session,
            &call.tool_call_id,
            &call.name,
            decision,
            started.elapsed().as_millis(),
        )
        .await?;
        Ok(outcome)
    }
}

fn approval_binding(args: &Value, actions: &[Action]) -> Result<[u8; 32]> {
    let actions: Vec<_> = actions
        .iter()
        .map(|action| {
            let capability = match action.capability {
                Capability::FsRead => "fs_read",
                Capability::FsWrite => "fs_write",
                Capability::Exec => "exec",
                Capability::Browse => "browse",
                Capability::Fetch => "fetch",
                Capability::VaultUse => "vault_use",
                Capability::Sensitive => "sensitive",
            };
            json!({"capability": capability, "target": action.target})
        })
        .collect();
    let mut full = json!({"args": args, "actions": actions});
    // Do not depend on serde_json's map backend: nested keys must also be canonical.
    full.sort_all_objects();
    Ok(Sha256::digest(serde_json::to_vec(&full)?).into())
}

impl HostToolHandler for Host {
    fn call(&self, call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        Box::pin(async move {
            self.recorded(call)
                .await
                .unwrap_or_else(|_| ToolOutcome::Failed {
                    message: "Tool event or audit recording failed".into(),
                })
        })
    }

    fn context_compacted(&self, subagent: Option<String>) {
        tools::browser::context_compacted(&self.env.session, subagent.as_deref());
    }
}

fn denied(reason: &str, hint: &str) -> ToolOutcome {
    ToolOutcome::Denied {
        reason: reason.into(),
        hint: Some(hint.into()),
    }
}

pub(crate) const OUTPUT_LIMIT: usize = 32 * 1024;
pub(crate) fn limit_output(value: Value) -> String {
    let serialized = value.to_string();
    if serialized.len() <= OUTPUT_LIMIT {
        return serialized;
    }
    let note =
        "Output truncated. Use read offset/limit or narrow the tool request to retrieve more.";
    let mut preview = String::new();
    // Budget escaped JSON bytes, not Unicode characters or raw UTF-8 alone.
    let overhead = json!({"truncated":true,"preview":"","how_to_get_more":note})
        .to_string()
        .len();
    let budget = OUTPUT_LIMIT.saturating_sub(overhead);
    let mut used = 0;
    for ch in serialized.chars() {
        let size = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' => 2,
            ch if ch < '\u{20}' => 6,
            ch => ch.len_utf8(),
        };
        if used + size > budget {
            break;
        }
        used += size;
        preview.push(ch);
    }
    json!({"truncated":true,"preview":preview,"how_to_get_more":note}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_binding_canonicalizes_nested_keys_and_covers_every_action() {
        let a: Value =
            serde_json::from_str(r#"{"outer":{"z":1,"a":2},"list":[{"y":3,"b":4}]}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{"list":[{"b":4,"y":3}],"outer":{"a":2,"z":1}}"#).unwrap();
        let actions = [Action::new(Capability::FsWrite, "/approved")];
        assert_eq!(
            approval_binding(&a, &actions).unwrap(),
            approval_binding(&b, &actions).unwrap()
        );
        assert_ne!(
            approval_binding(&a, &actions).unwrap(),
            approval_binding(
                &json!({
                    "outer": {"z":1, "a":9}, "list":[{"y":3,"b":4}]
                }),
                &actions
            )
            .unwrap()
        );
        assert_ne!(
            approval_binding(&a, &actions).unwrap(),
            approval_binding(&a, &[Action::new(Capability::FsRead, "/approved")]).unwrap()
        );
        assert_ne!(
            approval_binding(&a, &actions).unwrap(),
            approval_binding(&a, &[Action::new(Capability::FsWrite, "/unapproved")]).unwrap()
        );
    }
}
