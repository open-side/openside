use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use openside_proto::{TicketKind, TicketOption, TicketTarget};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{parse, spec, DaemonTool, TicketRequest, ToolEnv, ToolError, ToolExec};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HumanArgs {
    kind: TicketKind,
    summary: String,
    instructions: Option<String>,
    tab_id: Option<String>,
    origin: Option<String>,
    field_ref: Option<String>,
    resume_when: Option<super::browser::ResumeWhen>,
    options: Option<Vec<TicketOption>>,
}

pub(crate) struct RequestHumanStep;
impl DaemonTool for RequestHumanStep {
    fn name(&self) -> &'static str {
        "request_human_step"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<HumanArgs>(self.name(), "Pause for a human without bypassing authentication or security walls. Secret input must target a browser field; never request its value in chat.", ApprovalTier::Read, true)
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: HumanArgs = parse(args)?;
            if args.summary.trim().is_empty() {
                return Err(ToolError("summary must not be empty".into()));
            }
            let mut actions = Vec::new();
            if args.kind == TicketKind::SecretInput {
                if [
                    args.tab_id.as_deref(),
                    args.origin.as_deref(),
                    args.field_ref.as_deref(),
                ]
                .iter()
                .any(|value| value.map_or(true, |s| s.trim().is_empty()))
                {
                    return Err(ToolError(
                        "secret_input requires tab_id, origin and field_ref".into(),
                    ));
                }
                let url = url::Url::parse(args.origin.as_deref().unwrap_or_default())
                    .map_err(|_| ToolError("origin must be an HTTP(S) origin".into()))?;
                if !matches!(url.scheme(), "http" | "https")
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || url.path() != "/"
                {
                    return Err(ToolError("origin must be an HTTP(S) origin without credentials, path, query or fragment".into()));
                }
                // A secret released into this page is both browsing and secret use on it, so
                // origin lists, deny rules and the session mode apply when the step is opened
                // (and again when it is resolved, see api::resolve).
                let origin = openside_policy::origin_of(url.as_str())
                    .ok_or_else(|| ToolError("origin must be an HTTP(S) origin".into()))?;
                actions = vec![
                    Action::new(Capability::Browse, &origin),
                    Action::new(Capability::VaultUse, origin),
                ];
            }
            if let Some(condition) = &args.resume_when {
                condition.validate()?;
            }
            if args.kind == TicketKind::DeviceApproval {
                let condition = args
                    .resume_when
                    .as_ref()
                    .ok_or_else(|| ToolError("device_approval requires resume_when".into()))?;
                if !condition.manual()
                    && args
                        .tab_id
                        .as_deref()
                        .map_or(true, |id| id.trim().is_empty())
                {
                    return Err(ToolError(
                        "Automatic device_approval requires tab_id".into(),
                    ));
                }
            }
            Ok(actions)
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        let allowed = [
            "kind",
            "summary",
            "instructions",
            "tab_id",
            "origin",
            "field_ref",
            "resume_when",
            "options",
        ];
        let mut safe = args.clone();
        if let Some(object) = safe.as_object_mut() {
            object.retain(|key, _| allowed.contains(&key.as_str()));
        }
        safe
    }
    fn call<'a>(&'a self, args: Value, _: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            match parse::<HumanArgs>(&args) {
                Ok(args) => {
                    let target = if args.tab_id.is_some()
                        || args.origin.is_some()
                        || args.field_ref.is_some()
                    {
                        Some(TicketTarget {
                            tab_id: args.tab_id,
                            origin: args.origin,
                            field_ref: args.field_ref,
                        })
                    } else {
                        None
                    };
                    ToolExec::Defer(TicketRequest {
                        kind: args.kind,
                        summary: args.summary,
                        instructions: args.instructions,
                        options: args.options.unwrap_or_default(),
                        target,
                        resume_when: args
                            .resume_when
                            .map(|condition| serde_json::json!(condition)),
                        otp: None,
                        otp_register: None,
                    })
                }
                Err(error) => ToolExec::Denied {
                    reason: error.0,
                    hint: None,
                },
            }
        })
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    question: String,
    options: Option<Vec<String>>,
}

pub(crate) struct AskUser;
impl DaemonTool for AskUser {
    fn name(&self) -> &'static str {
        "ask_user"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<AskArgs>(self.name(), "Ask a non-secret question and pause until a human answers. Never ask for passwords, credentials or verification codes.", ApprovalTier::Read, true)
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: AskArgs = parse(args)?;
            if args.question.trim().is_empty() {
                return Err(ToolError("question must not be empty".into()));
            }
            Ok(Vec::new())
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        serde_json::json!({"question":args["question"],"options":args["options"]})
    }
    fn call<'a>(&'a self, args: Value, _: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            match parse::<AskArgs>(&args) {
                Ok(args) => ToolExec::Defer(TicketRequest {
                    kind: TicketKind::Info,
                    summary: args.question,
                    instructions: None,
                    options: args
                        .options
                        .unwrap_or_default()
                        .into_iter()
                        .map(|value| TicketOption {
                            label: value.clone(),
                            value,
                        })
                        .collect(),
                    target: None,
                    resume_when: None,
                    otp: None,
                    otp_register: None,
                }),
                Err(error) => ToolExec::Denied {
                    reason: error.0,
                    hint: None,
                },
            }
        })
    }
}
