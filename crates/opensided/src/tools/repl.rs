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
use openside_proto::{CapabilityScope, ReplRequest, ReplResponse, ReplStatus, SessionId};
use openside_repl::{Frame, Gate, Guard, Limits, Stop, Target, Verdict};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    browser::{browser_error, sensitive, url_origin, Controller},
    parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec,
};

use crate::{
    api::{Access, Caller, HttpError},
    engine::Shared,
    host::Host,
    persistence::random_id,
};
async fn protect_cookie_output(
    ctx: &ToolEnv,
    browser: &openside_browser::Browser,
    tab: &str,
) -> Result<(), ToolError> {
    browser
        .visit_tab_cookie_values(ctx.session.as_str(), tab, |value| {
            let Some(redactor) = openside_vault::Redactor::for_cookie(value) else {
                return;
            };
            let mut installed = ctx.shared.registry.redactors.write();
            if !redactor.covered_by(&installed) {
                installed.push(redactor);
            }
        })
        .await
        .map_err(|_| ToolError("Cookie output protection is unavailable".into()))
}

/// Tool call ids of `/repl` runs, a subset of [`super::API_CALL_PREFIX`].
const API_CALL_PREFIX: &str = "api-repl-";
const NAME: &str = "browser_repl";
const APPROVAL_REQUIRED: &str = "approval_required";
// The hint ends with the exact `approvals` value to send back; the API route parses it.
const APPROVALS_MARKER: &str = "\"approvals\": ";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReplArgs {
    /// Playwright page/expect/console; top-level await; returns last expression.
    /// page.snapshot() gives browser_snapshot text; page.locator('aria-ref=eN') targets refs.
    code: String,
    /// Omit for the only owned tab; if none, opens blank and returns its id.
    tab_id: Option<String>,
    /// Copy approval_required hint verbatim; each sensitive approval permits one operation.
    #[serde(default)]
    approvals: Vec<Approval>,
    /// The session's persistent context (`POST /repl` with `persistent`); not offered to
    /// agents, whose calls keep one fresh context each.
    #[serde(default)]
    #[schemars(skip)]
    persistent: bool,
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

/// Session policy as seen from inside a snippet, for one tab. Tabs of one persistent-context
/// execution share the approvals and the requested boundary.
#[derive(Clone)]
struct PolicyGuard {
    policy: Policy,
    tab: String,
    /// Approved browse origins stay valid for the run; approved sensitive descriptors are
    /// removed as they are used, so each permits one operation.
    approved: Arc<parking_lot::Mutex<Vec<Action>>>,
    requested: Arc<parking_lot::Mutex<Option<Action>>>,
    /// Canonical folders a page may receive files from.
    upload_roots: Vec<std::path::PathBuf>,
    /// The session's download folder; `None` when the session cannot receive downloads.
    downloads: Option<std::path::PathBuf>,
}

impl openside_repl::Guards for PolicyGuard {
    fn for_tab(&self, tab: &str) -> Arc<dyn Guard> {
        Arc::new(Self {
            tab: tab.to_owned(),
            ..self.clone()
        })
    }
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
        // Both spellings: a rule may name the configured data directory or where it really is.
        for folder in [folder.clone(), crate::artifacts::resolved(folder)] {
            let folder = folder.to_string_lossy();
            if self
                .policy
                .decide(&Action::new(Capability::FsWrite, folder.as_ref()))
                .effect
                == Effect::Deny
            {
                return Verdict::Deny(format!("policy denies writing downloads to {folder}"));
            }
        }
        Verdict::Allow
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
            "Run Playwright JavaScript in a session-owned tab; returns {return_value, console, error?, tab_id}. After origin policy approval, page.evaluate/evaluateHandle, $eval/$$eval, locator.evaluate/evaluateAll, page.addInitScript (call-scoped) and page.cdp(method, params) run without per-step checks inside page script. Supports frameLocator, mouse/keyboard, dialogs, popups, downloads and setInputFiles (absolute workspace/session-attachment paths); page.route and page.request throw NotSupported. Each call has a fresh JS context. Sensitive targets use browser_act's keyword heuristic (pay/purchase/checkout/delete): execution stops before the operation with approval_required. Resubmit the hint's exact approvals to request consent; approval reruns the whole snippet, allowing one operation per entry.",
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
        let mut redacted = json!({
            "tab_id": args.get("tab_id").cloned().unwrap_or(Value::Null),
            "code_length": args.get("code").and_then(Value::as_str).map(str::len).unwrap_or(0),
            "approvals": args.get("approvals").cloned().unwrap_or_else(|| json!([])),
        });
        if args["persistent"] == true {
            redacted["persistent"] = json!(true);
        }
        redacted
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
        // Agents, sidecars and `/tools/browser_repl` keep one fresh context per call; only the
        // `/repl` route sets `repl_context`, whatever the call id or arguments claim.
        if parsed.persistent && !ctx.repl_context {
            return Err(ToolError(
                "persistent is only available to POST /v1/sessions/{id}/repl".into(),
            ));
        }
        let browser = match parsed.tab_id.as_deref() {
            Some(tab) => controller.for_tab(&ctx.shared, &ctx.session, tab).await?,
            None => controller.for_session(&ctx.shared, &ctx.session).await?,
        };
        if ctx.cancellation.is_cancelled() {
            return Err(ToolError("Task tool cancelled".into()));
        }
        if parsed.persistent {
            return self.execute_persistent(parsed, ctx, browser).await;
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
        let selected = selected(ctx, &tab);
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
        protect_cookie_output(ctx, &browser, &tab).await?;
        let guard = policy_guard(ctx, &browser, &tab, &parsed.approvals).await;
        let limits = if ctx.subagent.is_some() {
            Limits {
                console_bytes: crate::tool_output::QUOTA,
                return_bytes: crate::tool_output::QUOTA,
                ..Limits::default()
            }
        } else {
            Limits::default()
        };
        let output = openside_repl::run_cancellable(
            Arc::clone(&browser),
            ctx.session.as_str().to_owned(),
            tab.clone(),
            parsed.code,
            Arc::new(guard.clone()),
            limits,
            ctx.cancellation.cancelled(),
        )
        .await;
        // Include rotated cookies before any result, console message or error is published.
        if browser.tabs().await.iter().any(|known| known.id == tab) {
            protect_cookie_output(ctx, &browser, &tab).await?;
        }
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
        if let Some(stopped) = stopped(output.stop, parsed.approvals, &guard)? {
            return Ok(stopped);
        }
        let mut result = json!({
            "return_value": output.return_value,
            "console": output.console,
            "tab_id": tab,
        });
        if let Some(error) = output.error {
            result["error"] = json!(error);
        }
        // Structured page URL after the snippet ran, so citation checks never have to scrape
        // page text; absent when the snippet closed the tab.
        if let Ok(url) = browser.url(&tab).await {
            result["url"] = json!(crate::browser::public_url(&url));
        }
        Ok(ToolExec::Done {
            for_model: result,
            for_app: None,
        })
    }

    /// A call in the session's persistent context: no tab is picked or opened implicitly, the
    /// snippet sees the tabs attached to the context, and each attached tab is admitted before
    /// and protected after the run like the single tab of a fresh-context call.
    async fn execute_persistent(
        &self,
        parsed: ReplArgs,
        ctx: &ToolEnv,
        browser: Arc<openside_browser::Browser>,
    ) -> Result<ToolExec, ToolError> {
        let controller = &ctx.shared.registry.browser;
        let select = parsed.tab_id.clone();
        if let Some(tab) = select.as_deref() {
            Controller::owned(&browser, &ctx.session, tab).await?;
        }
        let lease = match contexts::checkout(&ctx.session, &browser).await {
            Ok(lease) => lease,
            Err(contexts::Busy::Closed) => {
                return Err(ToolError("Browser is shutting down".into()))
            }
            Err(busy) => {
                return Ok(ToolExec::Denied {
                    reason: contexts::BUSY.into(),
                    hint: Some(busy.hint().into()),
                })
            }
        };
        let mut tabs = lease.tabs.clone();
        if let Some(tab) = &select {
            if !tabs.contains(tab) {
                tabs.push(tab.clone());
            }
        }
        let live = browser.tabs().await;
        tabs.retain(|tab| live.iter().any(|known| known.id == *tab));
        // Only tabs admitted here stay attached; the context drops any other.
        let mut keep = Vec::new();
        for tab in &tabs {
            if let Some((reason, hint)) =
                crate::browser_state::tab_denial(&ctx.shared, &ctx.session, &selected(ctx, tab))
                    .await
                    .map_err(|_| ToolError("Task tab state is unavailable".into()))?
            {
                return Ok(ToolExec::Denied {
                    reason: reason.into(),
                    hint: Some(hint.into()),
                });
            }
            super::otp::protect_setup_page(&ctx.shared, &browser, ctx.session.as_str(), tab)
                .await?;
            // The REPL never touches a tab that received a secret: a selected one refuses the
            // call, an attached one leaves the context.
            if controller.tab_tainted(tab).await {
                if select.as_deref() == Some(tab.as_str()) {
                    return Ok(ToolExec::Denied {
                        reason: "repl_unavailable_after_secret_fill".into(),
                        hint: Some(
                            "a secret was filled in this tab; open a new tab with openTab(url) and use the REPL there".into(),
                        ),
                    });
                }
                continue;
            }
            protect_cookie_output(ctx, &browser, tab).await?;
            keep.push(tab.clone());
        }
        if let Some(binding) = &ctx.browser_probe {
            if let Some(tab) = select
                .clone()
                .or_else(|| lease.page.clone())
                .filter(|tab| keep.contains(tab))
            {
                let stamp =
                    crate::browser_state::before(&ctx.shared, &ctx.session, &selected(ctx, &tab))
                        .await;
                *binding.lock() = stamp;
            }
        }
        let guard = policy_guard(ctx, &browser, "", &parsed.approvals).await;
        let output = lease
            .context
            .run(
                openside_repl::Request {
                    code: parsed.code,
                    select,
                    keep,
                    limits: Limits {
                        timeout: contexts::TIMEOUT,
                        ..Limits::default()
                    },
                },
                Arc::new(guard.clone()),
                ctx.cancellation.cancelled(),
            )
            .await;
        // A tab that reached a secret during the run is withheld and leaves the context.
        let live = browser.tabs().await;
        let mut unavailable = None;
        let mut attached = Vec::new();
        for tab in output
            .tabs
            .iter()
            .filter(|tab| live.iter().any(|known| known.id == **tab))
        {
            protect_cookie_output(ctx, &browser, tab).await?;
            if output.stop.is_none() {
                super::otp::protect_setup_page(&ctx.shared, &browser, ctx.session.as_str(), tab)
                    .await?;
            }
            if controller.tab_tainted(tab).await {
                unavailable = Some(ToolExec::Denied {
                    reason: "repl_unavailable_after_secret_fill".into(),
                    hint: Some("The snippet reached an authenticator setup page or a tab that received a secret; that tab left the REPL. Use register_authenticator and browser_snapshot instead.".into()),
                });
                continue;
            }
            attached.push(tab.clone());
        }
        let page = output.page.clone().filter(|tab| attached.contains(tab));
        lease.finish(attached.clone(), page.clone());
        controller
            .persist(&ctx.shared)
            .await
            .map_err(|_| ToolError("Browser state could not be saved".into()))?;
        if let Some(unavailable) = unavailable {
            return Ok(unavailable);
        }
        if let Some(stopped) = stopped(output.stop, parsed.approvals, &guard)? {
            return Ok(stopped);
        }
        let mut result = json!({
            "return_value": output.return_value,
            "console": output.console,
            "tabs": attached,
        });
        if let Some(error) = output.error {
            result["error"] = json!(error);
        }
        if let Some(tab) = page {
            if let Ok(url) = browser.url(&tab).await {
                result["url"] = json!(crate::browser::public_url(&url));
            }
            result["tab_id"] = json!(tab);
        }
        Ok(ToolExec::Done {
            for_model: result,
            for_app: None,
        })
    }
}

/// The tab-scoped call that browser state checks inspect for `tab`.
fn selected(ctx: &ToolEnv, tab: &str) -> ToolCall {
    ToolCall {
        tool_call_id: ctx.tool_call_id.clone(),
        name: NAME.into(),
        arguments_json: json!({ "tab_id": tab }).to_string(),
        subagent: ctx.subagent.clone(),
        approval_ticket: None,
    }
}

/// The guard of one call. The host approved the policy actions of these args; the snippet then
/// matches each approval by its exact descriptor.
async fn policy_guard(
    ctx: &ToolEnv,
    browser: &openside_browser::Browser,
    tab: &str,
    approvals: &[Approval],
) -> PolicyGuard {
    let approved = approvals
        .iter()
        .filter(|approval| ctx.approved_actions.contains(&approval.action()))
        .map(Approval::descriptor)
        .collect();
    let mut upload_roots = ctx.policy.approved_roots.clone();
    upload_roots.push(crate::artifacts::session_dir(&ctx.shared, &ctx.session).join("attachments"));
    let downloads = browser.session_downloads(ctx.session.as_str()).await.ok();
    upload_roots.extend(downloads.clone());
    PolicyGuard {
        policy: ctx.policy.clone(),
        tab: tab.to_owned(),
        approved: Arc::new(parking_lot::Mutex::new(approved)),
        requested: Arc::new(parking_lot::Mutex::new(None)),
        upload_roots,
        downloads,
    }
}

/// The tool result of a policy stop; `None` when the snippet was not stopped.
fn stopped(
    stop: Option<Stop>,
    mut approvals: Vec<Approval>,
    guard: &PolicyGuard,
) -> Result<Option<ToolExec>, ToolError> {
    Ok(match stop {
        Some(Stop::Approval(description)) => {
            // Earlier boundaries stay requested so the next ticket covers the whole rerun.
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
            Some(ToolExec::Denied {
                reason: format!("{APPROVAL_REQUIRED}: {description}"),
                hint: Some(format!(
                    "Nothing past that step ran. To request human approval, call browser_repl again with the same code and tab_id plus this argument; once approved, the whole snippet reruns from the start, so keep earlier steps safe to repeat. {APPROVALS_MARKER}{approvals}"
                )),
            })
        }
        Some(Stop::Denied(reason)) => Some(ToolExec::Denied {
            reason: format!("policy_denied: {reason}"),
            hint: Some("Choose a permitted action".into()),
        }),
        None => None,
    })
}

/// Stop the persistent contexts made for `browser` so their threads release it; part of
/// browser shutdown, which can only shut Chromium down once no handle is left. A running
/// snippet is cancelled and queued ones fail without running, even while callers hold them.
pub(crate) async fn close_contexts(browser: &Arc<openside_browser::Browser>) {
    contexts::close(browser);
    for context in contexts::CONTEXTS.remove_for(contexts::identity(browser)) {
        let _ = tokio::time::timeout(contexts::STOP_WAIT, context.shutdown()).await;
    }
}

#[cfg(test)]
pub(crate) fn has_context(session: &SessionId) -> bool {
    contexts::CONTEXTS.contains(session.as_str())
}

/// Persistent REPL contexts (`POST /repl` with `persistent`), one per session, bounded in
/// number, in calls waiting per context, and in idle lifetime.
mod contexts {
    use std::{collections::HashMap, sync::Arc, sync::LazyLock, time::Duration};

    use futures_util::future::BoxFuture;
    use openside_browser::Browser;
    use openside_proto::SessionId;
    use openside_repl::Context;
    use tokio::{sync::OwnedMutexGuard, time::Instant};

    /// Budget of one persistent-context call.
    pub(super) const TIMEOUT: Duration = Duration::from_secs(120);
    /// A context unused this long is dropped with its bindings; its tabs stay open.
    const IDLE: Duration = Duration::from_secs(600);
    /// Contexts alive at once; a new one evicts the least recently used idle context.
    const MAX_CONTEXTS: usize = 16;
    /// Calls of one context running or waiting at once.
    const MAX_CALLS: usize = 4;
    /// How long ending a context may wait for its thread (cancelled snippet cleanup).
    pub(super) const STOP_WAIT: Duration = Duration::from_secs(5);
    /// Denial reason of a call refused by these bounds; the API answers 429.
    pub(super) const BUSY: &str = "repl_busy";

    pub(super) static CONTEXTS: LazyLock<Arc<Registry<Context>>> =
        LazyLock::new(|| Arc::new(Registry::default()));

    /// A context's worker thread, stopped when the registry lets go of the context.
    pub(super) trait Worker: Send + Sync + 'static {
        fn shutdown(&self) -> BoxFuture<'_, ()>;
    }

    impl Worker for Context {
        fn shutdown(&self) -> BoxFuture<'_, ()> {
            Box::pin(Context::shutdown(self))
        }
    }

    /// Which browser a context drives. The context holds that browser, so the address
    /// cannot be reused while the entry exists.
    pub(super) fn identity(browser: &Arc<Browser>) -> usize {
        Arc::as_ptr(browser) as usize
    }

    /// Browsers being shut down; calls already waiting for the browser gate must not start a
    /// new context on them. A freed browser's entry is dead and ignored.
    static CLOSED: parking_lot::Mutex<Vec<std::sync::Weak<Browser>>> =
        parking_lot::const_mutex(Vec::new());

    pub(super) fn close(browser: &Arc<Browser>) {
        let mut closed = CLOSED.lock();
        closed.retain(|known| known.strong_count() > 0);
        closed.push(Arc::downgrade(browser));
    }

    /// The session's context for `browser`; a context made for another browser (a relaunch)
    /// is replaced.
    pub(super) async fn checkout(
        session: &SessionId,
        browser: &Arc<Browser>,
    ) -> Result<Lease<Context>, Busy> {
        let closing = {
            let mut closed = CLOSED.lock();
            closed.retain(|known| known.strong_count() > 0);
            closed
                .iter()
                .any(|known| std::ptr::eq(known.as_ptr(), Arc::as_ptr(browser)))
        };
        if closing {
            return Err(Busy::Closed);
        }
        CONTEXTS
            .checkout(session.as_str(), identity(browser), || {
                Context::new(Arc::clone(browser), session.as_str().to_owned())
            })
            .await
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum Busy {
        Calls,
        Contexts,
        /// The browser is shutting down.
        Closed,
    }

    impl Busy {
        pub(super) fn hint(&self) -> &'static str {
            match self {
                Self::Calls => {
                    "This session's REPL already has calls waiting; retry after they finish."
                }
                Self::Contexts => {
                    "Too many REPL sessions are running at once; retry after one finishes."
                }
                Self::Closed => "The browser is shutting down.",
            }
        }
    }

    struct Entry<C> {
        browser: usize,
        context: Arc<C>,
        /// Calls holding a lease: the one whose turn it is plus those waiting for theirs.
        calls: usize,
        /// Held from a call's admission checks through its recorded result, so the next call
        /// starts from the tabs the previous one left.
        turn: Arc<tokio::sync::Mutex<()>>,
        /// Attached tabs and the tab of `page` after the last finished call.
        tabs: Vec<String>,
        page: Option<String>,
        used: Instant,
    }

    pub(super) struct Registry<C> {
        entries: parking_lot::Mutex<HashMap<String, Entry<C>>>,
    }

    impl<C> Default for Registry<C> {
        fn default() -> Self {
            Self {
                entries: Default::default(),
            }
        }
    }

    /// One call's turn on a context; dropping it ends the turn and restarts the idle clock.
    pub(super) struct Lease<C: Worker> {
        registry: Arc<Registry<C>>,
        key: String,
        pub(super) context: Arc<C>,
        pub(super) tabs: Vec<String>,
        pub(super) page: Option<String>,
        _turn: Option<OwnedMutexGuard<()>>,
    }

    impl<C: Worker> Registry<C> {
        /// Admit a call and wait for its turn. An evicted context's worker has stopped before
        /// this returns, so no two workers beyond the bound ever run at once.
        pub(super) async fn checkout(
            self: &Arc<Self>,
            key: &str,
            browser: usize,
            make: impl FnOnce() -> C,
        ) -> Result<Lease<C>, Busy> {
            let (mut lease, turn, evicted) = {
                let mut entries = self.entries.lock();
                let mut evicted = Vec::new();
                let reuse = entries
                    .get(key)
                    .is_some_and(|entry| entry.browser == browser);
                if !reuse {
                    evicted.extend(entries.remove(key));
                    if entries.len() >= MAX_CONTEXTS {
                        let idle = entries
                            .iter()
                            .filter(|(_, entry)| entry.calls == 0)
                            .min_by_key(|(_, entry)| entry.used)
                            .map(|(key, _)| key.clone())
                            .ok_or(Busy::Contexts)?;
                        evicted.extend(entries.remove(&idle));
                    }
                    entries.insert(
                        key.to_owned(),
                        Entry {
                            browser,
                            context: Arc::new(make()),
                            calls: 0,
                            turn: Arc::default(),
                            tabs: Vec::new(),
                            page: None,
                            used: Instant::now(),
                        },
                    );
                }
                let Some(entry) = entries.get_mut(key) else {
                    return Err(Busy::Contexts);
                };
                if entry.calls >= MAX_CALLS {
                    return Err(Busy::Calls);
                }
                entry.calls += 1;
                entry.used = Instant::now();
                let lease = Lease {
                    registry: Arc::clone(self),
                    key: key.to_owned(),
                    context: Arc::clone(&entry.context),
                    tabs: Vec::new(),
                    page: None,
                    _turn: None,
                };
                (lease, Arc::clone(&entry.turn), evicted)
            };
            for entry in evicted {
                let _ = tokio::time::timeout(STOP_WAIT, entry.context.shutdown()).await;
            }
            // The lease already counts this call, so dropping this future while it waits
            // releases its place.
            lease._turn = Some(turn.lock_owned().await);
            if let Some(entry) = self.entries.lock().get(key) {
                if Arc::ptr_eq(&entry.context, &lease.context) {
                    lease.tabs.clone_from(&entry.tabs);
                    lease.page.clone_from(&entry.page);
                }
            }
            Ok(lease)
        }

        /// Remove every context of `browser`.
        pub(super) fn remove_for(&self, browser: usize) -> Vec<Arc<C>> {
            let mut entries = self.entries.lock();
            let keys: Vec<_> = entries
                .iter()
                .filter(|(_, entry)| entry.browser == browser)
                .map(|(key, _)| key.clone())
                .collect();
            keys.iter()
                .filter_map(|key| entries.remove(key))
                .map(|entry| entry.context)
                .collect()
        }

        fn expire_later(self: &Arc<Self>, key: String) {
            let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                return;
            };
            let registry = Arc::downgrade(self);
            runtime.spawn(async move {
                tokio::time::sleep(IDLE).await;
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                let expired = {
                    let mut entries = registry.entries.lock();
                    let idle = entries
                        .get(&key)
                        .is_some_and(|entry| entry.calls == 0 && entry.used.elapsed() >= IDLE);
                    idle.then(|| entries.remove(&key)).flatten()
                };
                if let Some(entry) = expired {
                    let _ = tokio::time::timeout(STOP_WAIT, entry.context.shutdown()).await;
                }
            });
        }

        #[cfg(test)]
        pub(super) fn contains(&self, key: &str) -> bool {
            self.entries.lock().contains_key(key)
        }
    }

    impl<C: Worker> Lease<C> {
        /// Record what the call left attached.
        pub(super) fn finish(&self, tabs: Vec<String>, page: Option<String>) {
            if let Some(entry) = self.registry.entries.lock().get_mut(&self.key) {
                if Arc::ptr_eq(&entry.context, &self.context) {
                    entry.tabs = tabs;
                    entry.page = page;
                }
            }
        }
    }

    impl<C: Worker> Drop for Lease<C> {
        fn drop(&mut self) {
            if let Some(entry) = self.registry.entries.lock().get_mut(&self.key) {
                if Arc::ptr_eq(&entry.context, &self.context) {
                    entry.calls -= 1;
                    entry.used = Instant::now();
                }
            }
            self.registry.expire_later(self.key.clone());
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicBool, Ordering};

        use super::*;

        /// A worker whose shutdown takes a moment and is observable.
        #[derive(Default)]
        struct Probe(Arc<AtomicBool>);

        impl Worker for Probe {
            fn shutdown(&self) -> BoxFuture<'_, ()> {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    self.0.store(true, Ordering::Release);
                })
            }
        }

        fn registry() -> Arc<Registry<Probe>> {
            Arc::new(Registry::default())
        }

        #[tokio::test]
        async fn calls_per_context_are_bounded_including_waiting_ones() {
            let registry = registry();
            let running = registry.checkout("s", 1, Probe::default).await.unwrap();
            let waiting: Vec<_> = (1..MAX_CALLS)
                .map(|_| {
                    let registry = Arc::clone(&registry);
                    tokio::spawn(async move { registry.checkout("s", 1, Probe::default).await })
                })
                .collect();
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(
                registry.checkout("s", 1, Probe::default).await.err(),
                Some(Busy::Calls)
            );
            drop(running);
            for waiter in waiting {
                assert!(waiter.await.unwrap().is_ok());
            }
            assert!(registry.checkout("s", 1, Probe::default).await.is_ok());
        }

        #[tokio::test]
        async fn a_queued_call_starts_from_the_previous_calls_result() {
            let registry = registry();
            let first = registry.checkout("s", 1, Probe::default).await.unwrap();
            let queued = tokio::spawn({
                let registry = Arc::clone(&registry);
                async move {
                    let lease = registry.checkout("s", 1, Probe::default).await.unwrap();
                    (lease.tabs.clone(), lease.page.clone())
                }
            });
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!queued.is_finished(), "the second call waits for its turn");
            first.finish(vec!["t1".into()], Some("t1".into()));
            drop(first);
            assert_eq!(
                queued.await.unwrap(),
                (vec!["t1".to_owned()], Some("t1".to_owned()))
            );
        }

        #[tokio::test]
        async fn the_evicted_worker_stops_before_its_replacement_is_admitted() {
            let registry = registry();
            let mut held = Vec::new();
            let mut stopped = Vec::new();
            for n in 0..MAX_CONTEXTS {
                let probe = Probe::default();
                stopped.push(Arc::clone(&probe.0));
                held.push(
                    registry
                        .checkout(&format!("s{n}"), 1, move || probe)
                        .await
                        .unwrap(),
                );
            }
            assert_eq!(
                registry.checkout("extra", 1, Probe::default).await.err(),
                Some(Busy::Contexts)
            );
            drop(held.remove(3));
            tokio::time::sleep(Duration::from_millis(5)).await;
            drop(held.remove(0));
            let extra = registry.checkout("extra", 1, Probe::default).await.unwrap();
            assert!(
                stopped[3].load(Ordering::Acquire),
                "the worker of the least recently used idle context stopped first"
            );
            assert!(!registry.contains("s3"));
            assert!(registry.contains("s0") && !stopped[0].load(Ordering::Acquire));
            drop((extra, held));
        }

        #[tokio::test]
        async fn another_browser_replaces_the_context() {
            let registry = registry();
            let first = registry.checkout("s", 1, Probe::default).await.unwrap();
            first.finish(vec!["t1".into()], Some("t1".into()));
            let first_context = Arc::clone(&first.context);
            drop(first);
            let again = registry.checkout("s", 1, Probe::default).await.unwrap();
            assert_eq!(again.tabs, ["t1"]);
            drop(again);
            let relaunched = registry.checkout("s", 2, Probe::default).await.unwrap();
            assert!(!Arc::ptr_eq(&relaunched.context, &first_context));
            assert!(first_context.0.load(Ordering::Acquire));
            assert!(relaunched.tabs.is_empty() && relaunched.page.is_none());
            assert_eq!(registry.remove_for(2).len(), 1);
            assert!(!registry.contains("s"));
        }

        #[tokio::test(start_paused = true)]
        async fn idle_contexts_expire_but_busy_ones_stay() {
            let registry = registry();
            drop(registry.checkout("idle", 1, Probe::default).await.unwrap());
            let busy = registry.checkout("busy", 1, Probe::default).await.unwrap();
            tokio::time::sleep(IDLE + Duration::from_secs(1)).await;
            assert!(!registry.contains("idle"));
            assert!(registry.contains("busy"));
            drop(busy);
            tokio::time::sleep(IDLE + Duration::from_secs(1)).await;
            assert!(!registry.contains("busy"));
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
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<ReplRequest>,
) -> Result<Json<ReplResponse>, HttpError> {
    // The authorization layer already requires `browse` for this route; checked here too so
    // the route never depends on its table entry alone.
    if !access.has(CapabilityScope::Browse) {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
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
    if request.persistent {
        args["persistent"] = json!(true);
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
    let mut host = Host::for_api(shared.clone(), &state, &caller);
    host.env.repl_context = true;
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
        ToolOutcome::Denied { reason, .. } if reason == contexts::BUSY => {
            return Err(HttpError(StatusCode::TOO_MANY_REQUESTS, contexts::BUSY));
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

    fn fixture_path(path: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:{path}"))
        } else {
            PathBuf::from(path)
        }
    }

    fn guard(mode: Mode, downloads: Option<&str>) -> PolicyGuard {
        let roots = vec![fixture_path("/work/space")];
        PolicyGuard {
            policy: Policy {
                mode,
                approved_roots: roots.clone(),
                ..Policy::default()
            },
            tab: "t1".into(),
            approved: Arc::default(),
            requested: Arc::default(),
            upload_roots: roots
                .into_iter()
                .chain([fixture_path("/data/artifacts/s1/attachments")])
                .collect(),
            downloads: downloads.map(fixture_path),
        }
    }

    fn upload(guard: &PolicyGuard, path: &str) -> Verdict {
        let path = if std::path::Path::new(path).has_root() {
            fixture_path(path).to_string_lossy().into_owned()
        } else {
            path.to_owned()
        };
        guard.check(&Gate::Upload {
            path: &path,
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
                "/work/spaces/a",
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

    /// A write rule on the real directory still covers a download folder reached through a
    /// symlinked data directory, and the configured spelling stays covered too.
    #[cfg(unix)]
    #[test]
    fn download_folder_rules_see_through_a_symlinked_data_directory() {
        use openside_policy::{Rule, RuleLayer};
        let root = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(root.path()).unwrap().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let configured = link.join("artifacts/s1/downloads");
        let actual = real.join("artifacts/s1/downloads");
        assert_eq!(crate::artifacts::resolved(&configured), actual);
        for pattern in [&actual, &configured] {
            let mut guard = guard(Mode::FullAccess, None);
            guard.downloads = Some(configured.clone());
            guard.policy.layers = vec![RuleLayer {
                rules: vec![Rule::new(
                    Effect::Deny,
                    Capability::FsWrite,
                    pattern.to_string_lossy(),
                )],
            }];
            assert!(
                matches!(
                    download(&guard, "https://files.example/report.csv"),
                    Verdict::Deny(_)
                ),
                "{pattern:?}"
            );
        }
    }
}
