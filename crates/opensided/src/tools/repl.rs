//! `browser_repl`: Playwright-syntax JavaScript against a session-owned agent tab.
//!
//! Every navigation and page access inside the snippet is checked against the session policy
//! by a synchronous guard. A sensitive interaction (the `browser_act` keyword heuristic) that
//! policy asks about stops the snippet before it happens and is reported as an approval
//! descriptor naming the operation, target, origin and tab. Calling again with that
//! descriptor added to `approvals` makes it a policy action of the call, so the host opens a
//! confirm ticket bound to the code, tab and every descriptor; the ticket's arguments list the
//! descriptors. After approval the snippet reruns from the start and each approved sensitive
//! descriptor permits exactly one matching operation. The descriptors live in the call
//! arguments, so nothing about a pending approval is kept in daemon memory.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use futures_util::future::BoxFuture;
use openside_browser::OpenedBy;
use openside_core::{ApprovalTier, HostToolHandler, ToolCall, ToolOutcome};
use openside_policy::{Action, Capability, Effect, Policy};
use openside_proto::{ReplRequest, ReplResponse, ReplStatus, SessionId};
use openside_repl::{Frame, Gate, Guard, Limits, Stop, Target, Verdict};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    browser::{browser_error, sensitive, url_origin, Controller},
    parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec,
};
use crate::{
    api::{Caller, HttpError},
    engine::Shared,
    host::Host,
    persistence::random_id,
};

/// Tool call ids of `/repl` runs, a subset of [`super::API_CALL_PREFIX`].
const API_CALL_PREFIX: &str = "api-repl-";
const NAME: &str = "browser_repl";
const APPROVAL_REQUIRED: &str = "approval_required";
// The hint ends with the exact `approvals` value to send back; the API route parses it.
const APPROVALS_MARKER: &str = "\"approvals\": ";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplArgs {
    /// JavaScript using Playwright's `page`, `expect`, and `console`. Top-level `await` works;
    /// the last expression statement is returned. `page.snapshot()` returns the
    /// browser_snapshot text, and `page.locator('aria-ref=eN')` targets its refs.
    code: String,
    /// Agent tab to drive. Optional when the session owns exactly one tab (or none: a blank
    /// tab is opened and its id returned).
    tab_id: Option<String>,
    /// Operations to request human approval for, copied verbatim from an `approval_required`
    /// hint. Each approved sensitive entry permits one matching operation in this call.
    #[serde(default)]
    approvals: Vec<Approval>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct Approval {
    /// `browse` (an origin) or `sensitive` (one operation).
    capability: ApprovalKind,
    /// The descriptor exactly as reported.
    target: String,
}

#[derive(Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ApprovalKind {
    Browse,
    Sensitive,
}

impl Approval {
    /// The policy action this approval stands for. A sensitive descriptor is decided as the
    /// sensitive capability on its real origin, so origin Deny rules apply to it; the
    /// descriptor itself stays bound through the call arguments.
    fn action(&self) -> Action {
        match self.capability {
            ApprovalKind::Browse => Action::new(Capability::Browse, &self.target),
            ApprovalKind::Sensitive => Action::new(
                Capability::Sensitive,
                descriptor_origin(&self.target).unwrap_or(&self.target),
            ),
        }
    }

    fn descriptor(&self) -> Action {
        let capability = match self.capability {
            ApprovalKind::Browse => Capability::Browse,
            ApprovalKind::Sensitive => Capability::Sensitive,
        };
        Action::new(capability, &self.target)
    }

    fn of(action: &Action) -> Option<Self> {
        let capability = match action.capability {
            Capability::Browse => ApprovalKind::Browse,
            Capability::Sensitive => ApprovalKind::Sensitive,
            _ => return None,
        };
        Some(Self {
            capability,
            target: action.target.clone(),
        })
    }
}

/// The origin of a descriptor built by [`PolicyGuard::descriptor`]; the quoted name may hold
/// any text, so the fixed suffix is split from the right.
fn descriptor_origin(descriptor: &str) -> Option<&str> {
    let (head, _tab) = descriptor.rsplit_once(" in tab ")?;
    let (_, origin) = head.rsplit_once("\" on ")?;
    Some(origin)
}

pub(crate) struct BrowserRepl;

async fn select_tab(
    browser: &openside_browser::Browser,
    ctx: &ToolEnv,
    requested: Option<&str>,
) -> Result<Option<String>, ToolError> {
    if let Some(tab) = requested {
        Controller::owned(browser, &ctx.session, tab).await?;
        return Ok(Some(tab.to_owned()));
    }
    let mut owned = browser.tabs().await.into_iter().filter(|tab| {
        tab.opened_by == OpenedBy::Agent
            && tab.owner_session_id.as_deref() == Some(ctx.session.as_str())
    });
    match (owned.next(), owned.next()) {
        (None, _) => Ok(None),
        (Some(tab), None) => Ok(Some(tab.id)),
        _ => Err(ToolError(
            "tab_id is required when the session owns several tabs; list them with browser_tabs"
                .into(),
        )),
    }
}

/// Session policy as seen from inside a snippet.
struct PolicyGuard {
    policy: Policy,
    tab: String,
    /// Approved browse origins stay valid for the run; approved sensitive descriptors are
    /// removed as they are used, so each permits one operation.
    approved: parking_lot::Mutex<Vec<Action>>,
    requested: parking_lot::Mutex<Option<Action>>,
    /// Canonical folders a page may receive files from.
    upload_roots: Vec<std::path::PathBuf>,
    /// The session's download folder; `None` when the session cannot receive downloads.
    downloads: Option<std::path::PathBuf>,
}

impl PolicyGuard {
    /// `policy` is decided first, so Deny always wins; only an Ask may be satisfied by an
    /// approval, which is matched by `approval` (the descriptor for sensitive operations).
    fn decide(&self, policy: Action, approval: Action) -> Verdict {
        match self.policy.decide(&policy).effect {
            Effect::Allow => Verdict::Allow,
            Effect::Deny => Verdict::Deny(format!(
                "policy denies {} on {}",
                capability(policy.capability),
                policy.target
            )),
            Effect::Ask => {
                let mut approved = self.approved.lock();
                if let Some(position) = approved.iter().position(|known| *known == approval) {
                    if approval.capability == Capability::Sensitive {
                        approved.remove(position);
                    }
                    return Verdict::Allow;
                }
                drop(approved);
                let description = approval.target.clone();
                *self.requested.lock() = Some(approval);
                Verdict::Approval(description)
            }
        }
    }

    /// Opaque origins (about:blank) hold nothing to protect. A frame is judged on its own origin
    /// as well as its top frame's, and denied if either is.
    fn browse_all(&self, origin: &str, frame: Option<Frame<'_>>) -> Verdict {
        for checked in std::iter::once(origin).chain(frame.map(|frame| frame.origin)) {
            if checked != "null" {
                let browse = self.browse(checked);
                if !matches!(browse, Verdict::Allow) {
                    return browse;
                }
            }
        }
        Verdict::Allow
    }

    fn browse(&self, origin: &str) -> Verdict {
        let action = Action::new(Capability::Browse, origin);
        self.decide(action.clone(), action)
    }

    /// The approval descriptor of one sensitive operation; binding the tab keeps an approval
    /// from carrying over to another tab, including an implicitly selected one.
    fn descriptor(&self, target: &Target<'_>, origin: &str) -> String {
        format!(
            "{} {} \"{}\" on {origin} in tab {}",
            target.action, target.role, target.name, self.tab
        )
    }
}

impl PolicyGuard {
    /// A file may be handed to a page only from the session's own folders: the workspace,
    /// message attachments and what the session downloaded. FullAccess does not widen this,
    /// because the page, not the user, would choose what leaves the machine. A Deny rule still
    /// wins inside the allow-list.
    fn upload(&self, path: &str, origin: &str, frame: Option<Frame<'_>>) -> Verdict {
        let browse = self.browse_all(origin, frame);
        if !matches!(browse, Verdict::Allow) {
            return browse;
        }
        let target = std::path::Path::new(path);
        if !self
            .upload_roots
            .iter()
            .any(|root| openside_policy::path_within(root, target))
        {
            return Verdict::Deny(format!(
                "{path} is outside the session workspace and attachments; uploads are limited to those folders"
            ));
        }
        match self
            .policy
            .decide(&Action::new(Capability::FsRead, path))
            .effect
        {
            Effect::Deny => Verdict::Deny(format!("policy denies reading {path}")),
            _ => Verdict::Allow,
        }
    }

    /// A download is saved into the session's download folder, so it needs the page's origin
    /// and the file's origin to be browsable and the mode to allow writing there.
    fn download(&self, url: &str, origin: &str) -> Verdict {
        let browse = self.browse_all(origin, None);
        if !matches!(browse, Verdict::Allow) {
            return browse;
        }
        // blob: and data: downloads come from the page itself, which was just judged.
        if let Ok(source) = url_origin(url) {
            let browse = self.browse_all(&source, None);
            if !matches!(browse, Verdict::Allow) {
                return browse;
            }
        }
        let Some(folder) = &self.downloads else {
            return Verdict::Deny("this session has no download folder".into());
        };
        let folder = folder.to_string_lossy();
        match self
            .policy
            .decide(&Action::new(Capability::FsWrite, folder.as_ref()))
            .effect
        {
            Effect::Deny => Verdict::Deny(format!("policy denies writing downloads to {folder}")),
            _ => Verdict::Allow,
        }
    }
}

fn capability(capability: Capability) -> &'static str {
    match capability {
        Capability::Browse => "browsing",
        Capability::Sensitive => "a sensitive action",
        _ => "this action",
    }
}

impl Guard for PolicyGuard {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        match gate {
            Gate::Navigate { url } => {
                if *url == "about:blank" {
                    return Verdict::Allow;
                }
                match url_origin(url) {
                    Ok(origin) => self.browse(&origin),
                    Err(error) => Verdict::Deny(error.0),
                }
            }
            Gate::Page {
                origin,
                frame,
                target,
            } => {
                let browse = self.browse_all(origin, *frame);
                if !matches!(browse, Verdict::Allow) {
                    return browse;
                }
                let frame_origin = frame.map(|frame| frame.origin);
                let acted_on = frame_origin.unwrap_or(origin);
                match target {
                    Some(target) if sensitive(target.name, target.role) => self.decide(
                        Action::new(Capability::Sensitive, acted_on),
                        Action::new(Capability::Sensitive, self.descriptor(target, acted_on)),
                    ),
                    _ => Verdict::Allow,
                }
            }
            // Page script is judged on the origins it runs in; there is no per-call approval.
            Gate::Script { origin, frame, .. } => self.browse_all(origin, *frame),
            Gate::Upload {
                path,
                origin,
                frame,
            } => self.upload(path, origin, *frame),
            Gate::Download {
                url,
                suggested_name: _,
                origin,
            } => self.download(url, origin),
        }
    }
}

impl DaemonTool for BrowserRepl {
    fn name(&self) -> &'static str {
        NAME
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<ReplArgs>(
            NAME,
            "Run Playwright-syntax JavaScript (page.goto, page.getByRole(...).click(), locator.fill, expect(...).toHaveText, ...) in a session-owned tab and return {return_value, console, error?, tab_id}. Page script is allowed once the page's origin passes policy: page.evaluate/evaluateHandle, page.$eval/$$eval, locator.evaluate/evaluateAll, page.addInitScript (removed when the call ends) and raw page.cdp(method, params); what page script does inside the page is not checked step by step. Frames (frameLocator), page.mouse/keyboard, dialogs, popups, downloads and setInputFiles (absolute paths inside the workspace or session attachments) work; network routes (page.route) and page.request throw NotSupported. Each call starts a fresh JavaScript context. Operations whose target looks sensitive (the same keyword heuristic as browser_act, e.g. pay, purchase, checkout, delete) need human approval: the call stops before that operation with approval_required and a hint carrying an `approvals` value; call again with that value added to request approval. Once approved, the whole snippet reruns from the start and each approved entry permits one operation.",
            ApprovalTier::Write,
            true,
        )
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let parsed: ReplArgs = parse(args)?;
            if parsed.code.trim().is_empty() {
                return Err(ToolError("code is empty".into()));
            }
            // Only requested descriptors are bound into an approval; the starting origin is
            // not, because the run being approved may itself navigate.
            Ok(parsed.approvals.iter().map(Approval::action).collect())
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({
            "tab_id": args.get("tab_id").cloned().unwrap_or(Value::Null),
            "code_length": args.get("code").and_then(Value::as_str).map(str::len).unwrap_or(0),
            "approvals": args.get("approvals").cloned().unwrap_or_else(|| json!([])),
        })
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            match self.execute(args, ctx).await {
                Ok(result) => result,
                Err(error) => ToolExec::Denied {
                    reason: error.0,
                    hint: Some("Verify tab ownership with browser_tabs".into()),
                },
            }
        })
    }
}

impl BrowserRepl {
    async fn execute(&self, args: Value, ctx: &ToolEnv) -> Result<ToolExec, ToolError> {
        let parsed: ReplArgs = parse(&args)?;
        let controller = &ctx.shared.registry.browser;
        controller.lost_tab(&ctx.session, &args).await?;
        let browser = controller.for_session(&ctx.shared, &ctx.session).await?;
        if ctx.cancellation.is_cancelled() {
            return Err(ToolError("Task tool cancelled".into()));
        }
        let tab = match select_tab(&browser, ctx, parsed.tab_id.as_deref()).await? {
            Some(tab) => tab,
            None => {
                let tab = browser
                    .create_agent_tab_cancellable(
                        ctx.session.as_str(),
                        "about:blank",
                        ctx.cancellation.cancelled(),
                    )
                    .await
                    .map_err(browser_error)?;
                tab.id
            }
        };
        let selected = openside_core::ToolCall {
            tool_call_id: ctx.tool_call_id.clone(),
            name: NAME.into(),
            arguments_json: json!({"tab_id":tab}).to_string(),
            subagent: ctx.subagent.clone(),
            approval_ticket: None,
        };
        if let Some((reason, hint)) =
            crate::browser_state::tab_denial(&ctx.shared, &ctx.session, &selected)
                .await
                .map_err(|_| ToolError("Task tab state is unavailable".into()))?
        {
            return Ok(ToolExec::Denied {
                reason: reason.into(),
                hint: Some(hint.into()),
            });
        }
        if let Some(binding) = &ctx.browser_probe {
            let stamp = crate::browser_state::before(&ctx.shared, &ctx.session, &selected).await;
            *binding.lock() = stamp;
        }
        super::otp::protect_setup_page(&ctx.shared, &browser, ctx.session.as_str(), &tab).await?;
        if controller.tab_tainted(&tab).await {
            return Ok(ToolExec::Denied {
                reason: "repl_unavailable_after_secret_fill".into(),
                hint: Some(
                    "a secret was filled in this tab; open a new tab with browser_open_tab and use the REPL there".into(),
                ),
            });
        }
        // The host approved the policy actions of these args; the snippet then matches each
        // approval by its exact descriptor.
        let approved = parsed
            .approvals
            .iter()
            .filter(|approval| ctx.approved_actions.contains(&approval.action()))
            .map(Approval::descriptor)
            .collect();
        let mut upload_roots = ctx.policy.approved_roots.clone();
        upload_roots
            .push(crate::artifacts::session_dir(&ctx.shared, &ctx.session).join("attachments"));
        let downloads = browser.session_downloads(ctx.session.as_str()).await.ok();
        upload_roots.extend(downloads.clone());
        let guard = Arc::new(PolicyGuard {
            policy: ctx.policy.clone(),
            tab: tab.clone(),
            approved: parking_lot::Mutex::new(approved),
            requested: parking_lot::Mutex::new(None),
            upload_roots,
            downloads,
        });
        let output = openside_repl::run_cancellable(
            Arc::clone(&browser),
            ctx.session.as_str().to_owned(),
            tab.clone(),
            parsed.code,
            guard.clone(),
            Limits::default(),
            ctx.cancellation.cancelled(),
        )
        .await;
        if output.stop.is_none() && browser.tabs().await.iter().any(|known| known.id == tab) {
            super::otp::protect_setup_page(&ctx.shared, &browser, ctx.session.as_str(), &tab)
                .await?;
            if controller.tab_tainted(&tab).await {
                return Ok(ToolExec::Denied {
                    reason: "repl_unavailable_after_secret_fill".into(),
                    hint: Some("The snippet reached an authenticator setup page. Use register_authenticator and browser_snapshot instead.".into()),
                });
            }
        }
        controller
            .persist(&ctx.shared)
            .await
            .map_err(|_| ToolError("Browser state could not be saved".into()))?;
        match output.stop {
            Some(Stop::Approval(description)) => {
                // Earlier boundaries stay requested so the next ticket covers the whole rerun.
                let mut approvals = parsed.approvals;
                approvals.extend(
                    guard
                        .requested
                        .lock()
                        .take()
                        .as_ref()
                        .and_then(Approval::of),
                );
                let approvals = serde_json::to_string(&approvals)
                    .map_err(|_| ToolError("approval descriptor could not be encoded".into()))?;
                Ok(ToolExec::Denied {
                    reason: format!("{APPROVAL_REQUIRED}: {description}"),
                    hint: Some(format!(
                        "Nothing past that step ran. To request human approval, call browser_repl again with the same code and tab_id plus this argument; once approved, the whole snippet reruns from the start, so keep earlier steps safe to repeat. {APPROVALS_MARKER}{approvals}"
                    )),
                })
            }
            Some(Stop::Denied(reason)) => Ok(ToolExec::Denied {
                reason: format!("policy_denied: {reason}"),
                hint: Some("Choose a permitted action".into()),
            }),
            None => {
                let mut result = json!({
                    "return_value": output.return_value,
                    "console": output.console,
                    "tab_id": tab,
                });
                if let Some(error) = output.error {
                    result["error"] = json!(error);
                }
                // Structured page URL after the snippet ran, so citation checks never have to
                // scrape page text; absent when the snippet closed the tab.
                if let Ok(url) = browser.url(&tab).await {
                    result["url"] = json!(crate::browser::public_url(&url));
                }
                Ok(ToolExec::Done {
                    for_model: result,
                    for_app: None,
                })
            }
        }
    }
}

/// The `approvals` value an `approval_required` hint asks the caller to send back.
fn requested_approvals(hint: Option<&str>) -> Option<Value> {
    let hint = hint?;
    let start = hint.rfind(APPROVALS_MARKER)? + APPROVALS_MARKER.len();
    serde_json::from_str(&hint[start..]).ok()
}

/// `POST /v1/sessions/{id}/repl`: the `browser_repl` tool through the same host pipeline
/// (policy, approval tickets, redaction, tool events, audit) as an agent call.
pub(crate) async fn route(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<ReplRequest>,
) -> Result<Json<ReplResponse>, HttpError> {
    let session = SessionId::new(id);
    let state = shared
        .db
        .state(&session)
        .await
        .map_err(|_| HttpError::not_found())?;
    caller.record_driver(&shared, &session).await?;
    let mut args = json!({"code": request.code});
    if let Some(tab) = &request.tab_id {
        args["tab_id"] = json!(tab);
    }
    let tool_call_id = match &request.approval_ticket {
        Some(ticket) => {
            let ticket = crate::tickets::get(&shared, ticket)
                .await
                .map_err(|_| HttpError::internal())?
                .ok_or_else(HttpError::not_found)?;
            if ticket.session_id != session || !ticket.tool_call_id.starts_with(API_CALL_PREFIX) {
                return Err(HttpError(StatusCode::CONFLICT, "invalid_transition"));
            }
            // The approved descriptors are part of the bound arguments; the durable ticket
            // carries them, so a rerun after a daemon restart still matches.
            if let Some(approvals) = ticket.tool.as_ref().map(|tool| &tool.args["approvals"]) {
                if approvals.as_array().is_some_and(|list| !list.is_empty()) {
                    args["approvals"] = approvals.clone();
                }
            }
            ticket.tool_call_id
        }
        None => format!("{API_CALL_PREFIX}{}", random_id()),
    };
    let host = Host::for_api(shared.clone(), &state, &caller);
    let call = |tool_call_id: String, args: &Value, approval_ticket| ToolCall {
        tool_call_id,
        name: NAME.into(),
        arguments_json: args.to_string(),
        subagent: None,
        approval_ticket,
    };
    let mut call_id = tool_call_id.clone();
    let mut outcome = host
        .call(call(tool_call_id, &args, request.approval_ticket.clone()))
        .await;
    // An API caller has no model loop to repeat the call, so request the approval right away;
    // this also covers a later boundary reached by an approved rerun.
    if let ToolOutcome::Denied {
        reason,
        hint: Some(hint),
    } = &outcome
    {
        if let Some(approvals) = reason
            .starts_with(APPROVAL_REQUIRED)
            .then(|| requested_approvals(Some(hint)))
            .flatten()
        {
            args["approvals"] = approvals;
            call_id = format!("{API_CALL_PREFIX}{}", random_id());
            outcome = host.call(call(call_id.clone(), &args, None)).await;
        }
    }
    let mut response = ReplResponse {
        tool_call_id: call_id,
        status: ReplStatus::Failed,
        result: None,
        ticket_id: None,
        reason: None,
        hint: None,
    };
    match outcome {
        // REPL callers get the JSON result; binary parts are only for model input.
        ToolOutcome::Done { for_model_json }
        | ToolOutcome::DoneWithParts { for_model_json, .. } => {
            response.status = ReplStatus::Done;
            response.result = serde_json::from_str(&for_model_json).ok();
        }
        ToolOutcome::Deferred { ticket, .. } => {
            response.status = ReplStatus::Deferred;
            response.ticket_id = Some(ticket);
        }
        ToolOutcome::Denied { reason, hint } => {
            response.status = ReplStatus::Denied;
            response.reason = Some(reason);
            response.hint = hint;
        }
        ToolOutcome::Failed { message } => response.reason = Some(message),
    }
    Ok(Json(response))
}

#[cfg(test)]
mod guard_tests {
    use std::path::PathBuf;

    use openside_policy::Mode;

    use super::*;

    fn guard(mode: Mode, downloads: Option<&str>) -> PolicyGuard {
        let roots = vec![PathBuf::from("/work/space")];
        PolicyGuard {
            policy: Policy {
                mode,
                approved_roots: roots.clone(),
                ..Policy::default()
            },
            tab: "t1".into(),
            approved: parking_lot::Mutex::new(Vec::new()),
            requested: parking_lot::Mutex::new(None),
            upload_roots: roots
                .into_iter()
                .chain([PathBuf::from("/data/artifacts/s1/attachments")])
                .collect(),
            downloads: downloads.map(PathBuf::from),
        }
    }

    fn upload(guard: &PolicyGuard, path: &str) -> Verdict {
        guard.check(&Gate::Upload {
            path,
            origin: "https://app.example",
            frame: None,
        })
    }

    fn download(guard: &PolicyGuard, url: &str) -> Verdict {
        guard.check(&Gate::Download {
            url,
            suggested_name: "report.csv",
            origin: "https://app.example",
        })
    }

    #[test]
    fn uploads_are_limited_to_workspace_and_attachments() {
        for mode in [Mode::ReadOnly, Mode::Guard, Mode::FullAccess] {
            let guard = guard(mode, None);
            assert!(matches!(
                upload(&guard, "/work/space/a/b.txt"),
                Verdict::Allow
            ));
            assert!(matches!(
                upload(&guard, "/data/artifacts/s1/attachments/m1/0-a.png"),
                Verdict::Allow
            ));
            for outside in [
                "/etc/passwd",
                "/work/spacex/a",
                "/work/space/../../etc/passwd",
                "relative.txt",
            ] {
                assert!(
                    matches!(upload(&guard, outside), Verdict::Deny(_)),
                    "{mode:?} must refuse {outside}"
                );
            }
        }
    }

    #[test]
    fn downloads_need_a_folder_and_a_writable_mode() {
        let guard_with_folder = guard(Mode::Guard, Some("/data/downloads/s1"));
        assert!(matches!(
            download(&guard_with_folder, "https://files.example/report.csv"),
            Verdict::Allow
        ));
        assert!(matches!(
            download(&guard_with_folder, "blob:https://app.example/1234"),
            Verdict::Allow
        ));
        assert!(matches!(
            download(
                &guard(Mode::Guard, None),
                "https://files.example/report.csv"
            ),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            download(
                &guard(Mode::ReadOnly, Some("/data/downloads/s1")),
                "https://files.example/report.csv"
            ),
            Verdict::Deny(_)
        ));
    }
}
