use parking_lot::RwLock;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, ToolSpec};
use openside_policy::{Action, Capability, Policy};
use openside_proto::{PolicyMode, SessionId, TicketId, TicketKind, TicketOption, TicketTarget};
use openside_sessions::SessionState;
use openside_vault::Redactor;
use serde::de::DeserializeOwned;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::{engine::Shared, persistence::Result};

pub(crate) mod browser;
mod files;
mod human;
pub(crate) mod memory;
pub(crate) mod otp;
pub(crate) mod repl;
mod shell;
pub(crate) mod vault;

pub(crate) struct ToolEnv {
    pub(crate) shared: Shared,
    pub(crate) session: SessionId,
    pub(crate) mode: PolicyMode,
    pub(crate) workspace: Option<PathBuf>,
    pub(crate) policy: Policy,
    pub(crate) tool_call_id: String,
    pub(crate) approved_actions: Vec<Action>,
    /// The subagent running this call; `None` for the session's parent agent.
    pub(crate) subagent: Option<String>,
}

impl ToolEnv {
    pub(crate) fn new(shared: Shared, state: &SessionState) -> Self {
        let workspace = state.workspace.as_ref().map(PathBuf::from);
        let policy = Policy {
            mode: state.mode.into(),
            approved_roots: workspace.iter().cloned().collect(),
            // Session-level rules only; host::execute layers the live daemon policy beneath.
            layers: Vec::new(),
            origins: Default::default(),
        };
        Self {
            shared,
            session: state.id.clone(),
            mode: state.mode,
            workspace,
            policy,
            approved_actions: Vec::new(),
            tool_call_id: String::new(),
            subagent: None,
        }
    }
    pub(crate) fn target(&self, capability: Capability) -> std::result::Result<&Path, ToolError> {
        let path = self
            .approved_actions
            .iter()
            .find(|a| a.capability == capability)
            .map(|action| Path::new(&action.target))
            .ok_or_else(|| ToolError("Missing approved tool target".into()))?;
        if matches!(capability, Capability::FsRead | Capability::FsWrite)
            && self.shared.credential_paths.is_protected(path)
        {
            return Err(ToolError("Credential source files are protected".into()));
        }
        Ok(path)
    }
}

pub(crate) struct ToolError(pub(crate) String);

pub(crate) trait DaemonTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn spec(&self) -> ToolSpec;
    fn visible(&self, ctx: &ToolEnv) -> bool;
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>>;
    fn redact_args(&self, args: &Value) -> Value;
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec>;
}

pub(crate) enum ToolExec {
    Done {
        for_model: Value,
        for_app: Option<Value>,
    },
    /// Done, with binary content handed to the model next to `for_model` (screenshots).
    DoneWithParts {
        for_model: Value,
        for_app: Option<Value>,
        parts: Vec<openside_core::ContentPart>,
    },
    Defer(TicketRequest),
    Denied {
        reason: String,
        hint: Option<String>,
    },
    Failed(String),
}

pub(crate) struct TicketRequest {
    pub(crate) kind: TicketKind,
    pub(crate) summary: String,
    pub(crate) instructions: Option<String>,
    pub(crate) options: Vec<TicketOption>,
    pub(crate) target: Option<TicketTarget>,
    pub(crate) resume_when: Option<Value>,
    pub(crate) otp: Option<openside_proto::TicketOtp>,
    pub(crate) otp_register: Option<openside_proto::TicketOtpRegister>,
}

/// Trusted browser insertion boundary: implementations must validate session ownership
/// and current origin before filling, and must never include the value in errors.
pub(crate) trait SecretSink: Send + Sync {
    fn fill<'a>(
        &'a self,
        ticket: &'a TicketId,
        session: &'a SessionId,
        target: &'a TicketTarget,
        value: Zeroizing<String>,
    ) -> BoxFuture<'a, std::result::Result<(), SecretSinkError>>;
    /// [`Self::fill`] for a caller that already holds the browser gate (`Controller::gate`),
    /// so it can take the vault after the gate and fill inside the vault callback without
    /// waiting for the gate again. Sinks without a gate fill as usual.
    fn fill_gate_held<'a>(
        &'a self,
        ticket: &'a TicketId,
        session: &'a SessionId,
        target: &'a TicketTarget,
        value: Zeroizing<String>,
    ) -> BoxFuture<'a, std::result::Result<(), SecretSinkError>> {
        self.fill(ticket, session, target, value)
    }
}

pub(crate) struct SecretSinkError {
    pub(crate) code: &'static str,
}

pub(crate) struct Registry {
    tools: BTreeMap<&'static str, Arc<dyn DaemonTool>>,
    pub(crate) secret_sink: RwLock<Option<Arc<dyn SecretSink>>>,
    pub(crate) redactors: Arc<RwLock<Vec<Redactor>>>,
    pub(crate) browser: Arc<browser::Controller>,
    pub(crate) vault: vault::Service,
}

impl Registry {
    pub(crate) fn new(config: &crate::config::Resolved) -> Self {
        let tools: Vec<Arc<dyn DaemonTool>> = vec![
            Arc::new(human::RequestHumanStep),
            Arc::new(human::AskUser),
            Arc::new(files::Read),
            Arc::new(files::Write),
            Arc::new(files::Edit),
            Arc::new(shell::Bash),
            Arc::new(browser::BrowserTool(browser::Kind::Open)),
            Arc::new(browser::BrowserTool(browser::Kind::Tabs)),
            Arc::new(browser::BrowserTool(browser::Kind::Snapshot)),
            Arc::new(browser::BrowserTool(browser::Kind::Navigate)),
            Arc::new(browser::BrowserTool(browser::Kind::Act)),
            Arc::new(browser::BrowserTool(browser::Kind::Screenshot)),
            Arc::new(browser::BrowserTool(browser::Kind::Close)),
            Arc::new(browser::cua::BrowserPointer),
            Arc::new(vault::Login),
            Arc::new(vault::Otp),
            Arc::new(otp::RequestOtp),
            Arc::new(otp::RegisterAuthenticator),
            Arc::new(vault::Passkey),
            Arc::new(memory::MemorySearch),
            Arc::new(memory::MemoryWrite),
            Arc::new(crate::skills::SkillTool),
            Arc::new(repl::BrowserRepl),
            Arc::new(crate::subagents::Delegate),
        ];
        let redactors = Arc::new(RwLock::new(Vec::new()));
        let browser = Arc::new(browser::Controller::new(
            &config.loaded.config,
            redactors.clone(),
        ));
        let sink: Arc<dyn SecretSink> = browser.clone();
        let secret_sink = browser.available().then_some(sink);
        Self {
            tools: tools.into_iter().map(|tool| (tool.name(), tool)).collect(),
            secret_sink: RwLock::new(secret_sink),
            redactors,
            browser,
            vault: vault::Service::new(),
        }
    }
    pub(crate) fn get(&self, name: &str) -> Option<&Arc<dyn DaemonTool>> {
        self.tools.get(name)
    }
    /// Every registered tool, so tests can prove properties for the whole registry instead
    /// of a hand-picked list.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.tools.keys().copied().collect()
    }
    pub(crate) fn specs(&self, ctx: &ToolEnv) -> Vec<ToolSpec> {
        self.tools
            .values()
            .filter_map(|tool| {
                if !tool.visible(ctx)
                    || !crate::profiles::allows(crate::profiles::current(&ctx.shared), tool.name())
                {
                    return None;
                }
                let spec = tool.spec();
                (ctx.mode != PolicyMode::ReadOnly || spec.approval == ApprovalTier::Read)
                    .then_some(spec)
            })
            .collect()
    }
    pub(crate) fn redact(&self, value: Value) -> Result<Value> {
        self.redact_except(value, &[])
    }
    /// Redact everything except what `keep` names: JSON paths (`*` matches any member or array
    /// element) into a typed protocol value whose content the daemon generated or the schema
    /// constrains (ids, enum tags, timestamps, origins). A string at the end of a path is kept
    /// verbatim, and the member names of every object a path passes through are kept, since
    /// they are field names of the schema. Live secrets of any length are masked as
    /// substrings, so a one-character answer would otherwise rewrite identifiers and break
    /// deserialization; exemptions are chosen by schema location, never by what a string
    /// looks like, so page and model text are always scrubbed.
    pub(crate) fn redact_except(&self, mut value: Value, keep: &[&[&str]]) -> Result<Value> {
        let redactors = self.redactors.read();
        if redactors.is_empty() {
            return Ok(value);
        }
        fn redact(text: &mut String, redactors: &[Redactor]) {
            for redactor in redactors {
                *text = redactor.redact(text);
            }
        }
        fn below<'p>(keep: &[&'p [&'p str]], key: &str) -> Vec<&'p [&'p str]> {
            keep.iter()
                .filter_map(|path| match path.split_first() {
                    Some((first, rest)) if *first == key || *first == "*" => Some(rest),
                    _ => None,
                })
                .collect()
        }
        fn apply(value: &mut Value, redactors: &[Redactor], keep: &[&[&str]]) {
            match value {
                Value::String(text) => {
                    if !keep.iter().any(|path| path.is_empty()) {
                        redact(text, redactors);
                    }
                }
                Value::Array(array) => {
                    for (index, item) in array.iter_mut().enumerate() {
                        apply(item, redactors, &below(keep, &index.to_string()));
                    }
                }
                Value::Object(object) => {
                    let schema = keep.iter().any(|path| !path.is_empty());
                    let old = std::mem::take(object);
                    for (mut key, mut value) in old {
                        let inner = below(keep, &key);
                        if !schema {
                            redact(&mut key, redactors);
                        }
                        apply(&mut value, redactors, &inner);
                        object.insert(key, value);
                    }
                }
                _ => {}
            }
        }
        apply(&mut value, &redactors, keep);
        Ok(value)
    }
}

/// Schema locations of a [`openside_proto::Ticket`] that [`Registry::redact_except`] keeps.
/// Only daemon-generated or daemon-validated values qualify: ids, enum tags, timestamps, the
/// origin a registration derived from the tab. Model-chosen text such as `options[*].value`
/// stays redacted even though the schema names it. Target strings are kept only per ticket,
/// for the parts `tickets::open` checked against the live browser ([`TICKET_TARGET_KEEP`]).
pub(crate) const TICKET_KEEP: &[&[&str]] = &[
    &["id"],
    &["session_id"],
    &["tool_call_id"],
    &["deferral"],
    &["kind"],
    &["created_at"],
    &["expires_at"],
    &["status"],
    &["origin", "subagent_id"],
    &["tool", "name"],
    &["otp", "channel"],
    &["otp_register", "origin"],
    // Resolution statuses are daemon-chosen constants; free answers stay redacted.
    &["resolution", "status"],
];

/// `target.tab_id`, `target.origin`, `target.field_ref`: kept by `tickets::open` only when
/// `Controller::capture_ticket` vouched for them (an owned tab, its live origin, a resolved
/// field ref); otherwise they are redacted like any caller text.
pub(crate) const TICKET_TARGET_KEEP: [&[&str]; 3] = [
    &["target", "tab_id"],
    &["target", "origin"],
    &["target", "field_ref"],
];

/// Schema locations of a serialized [`openside_proto::EventKind`] that are kept, by its
/// `kind` tag; the ticket of `ticket.opened` is kept as in [`TICKET_KEEP`].
pub(crate) fn event_keep(tag: &str) -> Vec<Vec<&'static str>> {
    let fields: &[&[&'static str]] = match tag {
        "message.queued" => &[
            &["message_id"],
            &["mode"],
            &["attachments", "*", "kind"],
            &["attachments", "*", "mime"],
            &["attachments", "*", "sha256"],
        ],
        "message.edited" | "message.promoted" | "message.moved" | "message.injected" => {
            &[&["message_id"]]
        }
        "message.completed" => &[&["message_id"], &["outcome"]],
        "message.dropped" => &[&["message_id"], &["reason"]],
        "usage" => &[&["model"]],
        "tool.started" => &[&["tool_call_id"], &["name"]],
        "tool.updated" | "tool.ended" => &[&["tool_call_id"]],
        // Paths are chosen by the model or the person, so they stay redacted.
        "artifact.saved" => &[&["mime"], &["tool_call_id"], &["tool"]],
        "ticket.resolved" => &[&["ticket_id"], &["resolution", "status"]],
        "ticket.rejected" | "ticket.expired" => &[&["ticket_id"]],
        "subagent.started" => &[
            &["subagent_id"],
            &["parent_tool_call_id"],
            &["model"],
            &["tools", "*"],
        ],
        "subagent.ended" | "subagent.lifecycle" => &[&["subagent_id"], &["status"]],
        "subagent.progress" => &[&["subagent_id"]],
        "tab.opened" | "tab.closed" | "tab.asleep" => &[&["tab_id"]],
        "session.created" => &[&["model"], &["mode"], &["workspace"]],
        "runtime.started" => &[&["tools", "*"]],
        "runtime.error" => &[&["code"]],
        _ => &[],
    };
    let mut keep: Vec<Vec<&'static str>> = vec![vec!["kind"]];
    keep.extend(fields.iter().map(|path| path.to_vec()));
    if tag == "ticket.opened" {
        // `tickets::open`, the only producer, already redacted every target string it could
        // not vouch for; the rest must survive verbatim, since the pinned field is matched
        // against the published target.
        keep.extend(
            TICKET_KEEP
                .iter()
                .chain(TICKET_TARGET_KEEP.iter())
                .map(|path| {
                    let mut full = vec!["ticket"];
                    full.extend_from_slice(path);
                    full
                }),
        );
    }
    keep
}

/// Tools that read or write a browser tab: they hold the browser gate for the whole call and
/// are refused on a tab under human takeover. One list for both guards, so a new browser tool
/// cannot be serialized by one and forgotten by the other.
pub(crate) fn touches_browser(name: &str) -> bool {
    name.starts_with("browser_")
        || name.starts_with("vault_")
        || matches!(name, "request_otp" | "register_authenticator")
}

pub(crate) fn visible(tool: &dyn DaemonTool, ctx: &ToolEnv) -> bool {
    (ctx.mode != PolicyMode::ReadOnly || tool.spec().approval == ApprovalTier::Read)
        && tool.visible(ctx)
        && crate::profiles::allows(crate::profiles::current(&ctx.shared), tool.name())
}

pub(crate) fn parse<T: DeserializeOwned>(args: &Value) -> std::result::Result<T, ToolError> {
    serde_json::from_value(args.clone())
        .map_err(|_| ToolError("Arguments do not match the tool schema".into()))
}

pub(crate) fn spec<T: schemars::JsonSchema>(
    name: &str,
    description: &str,
    approval: ApprovalTier,
    deferrable: bool,
) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters_json: schemars::schema_for!(T).to_value().to_string(),
        approval,
        deferrable,
    }
}
