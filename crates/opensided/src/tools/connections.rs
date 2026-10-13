use std::sync::Arc;

use futures_util::future::BoxFuture;
use openside_browser::{Browser, OpenedBy};
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use openside_proto::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    browser::{url_origin, Controller},
    parse, DaemonTool, TicketRequest, ToolEnv, ToolError, ToolExec,
};
use crate::{connections, engine::Shared};

const BODY_CAP: usize = 64 * 1024;

fn spec<T: schemars::JsonSchema>(
    name: &str,
    description: &str,
    approval: ApprovalTier,
    deferrable: bool,
) -> openside_core::ToolSpec {
    let mut schema = schemars::schema_for!(T).to_value();
    if let Some(root) = schema.as_object_mut() {
        for annotation in ["$schema", "title", "description"] {
            root.remove(annotation);
        }
        if let Some(fields) = root.get_mut("properties").and_then(Value::as_object_mut) {
            for field in fields.values_mut().filter_map(Value::as_object_mut) {
                field.remove("description");
            }
        }
    }
    openside_core::ToolSpec {
        name: name.into(),
        description: description.into(),
        parameters_json: schema.to_string(),
        approval,
        deferrable,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    List,
    Open,
    Check,
    Confirm,
    Observe,
    Fetch,
}
pub(crate) struct ConnectionTool(pub(crate) Kind);

fn error(code: &str) -> ToolError {
    ToolError(code.into())
}
fn agent(ctx: &ToolEnv) -> Result<String, ToolError> {
    let id = ctx.agent_id.as_deref().unwrap_or("chief");
    let agent = ctx
        .shared
        .agents
        .get(id)
        .map_err(|_| error("connection_denied"))?
        .ok_or_else(|| error("connection_denied"))?;
    if agent.status != AgentStatus::Active {
        return Err(error("connection_denied"));
    }
    Ok(id.into())
}
fn access(row: &Connection, agent: &str) -> ConnectionAccess {
    row.grants
        .iter()
        .find(|grant| grant.agent_id == agent)
        .map(|grant| grant.access)
        .unwrap_or(if agent == "chief" {
            ConnectionAccess::Allowed
        } else {
            ConnectionAccess::Ask
        })
}
fn row(ctx: &ToolEnv, id: &str) -> Result<Connection, ToolError> {
    let row = ctx
        .shared
        .connections
        .get(id)
        .map_err(|_| error("connection_not_found"))?
        .ok_or_else(|| error("connection_not_found"))?;
    if row.profile != ConnectionProfile::Agent
        || access(&row, &agent(ctx)?) == ConnectionAccess::Denied
    {
        return Err(error("connection_denied"));
    }
    Ok(row)
}
fn grant_rows<'a>(
    ctx: &ToolEnv,
    row: &'a Connection,
) -> Result<std::borrow::Cow<'a, [Connection]>, ToolError> {
    let rows = if row.account_space.is_some() {
        std::borrow::Cow::Borrowed(std::slice::from_ref(row))
    } else {
        std::borrow::Cow::Owned(
            ctx.shared
                .connections
                .shared_site(&row.site)
                .map_err(|_| error("connection_denied"))?,
        )
    };
    let agent = agent(ctx)?;
    if rows
        .iter()
        .any(|row| access(row, &agent) == ConnectionAccess::Denied)
    {
        return Err(error("connection_denied"));
    }
    Ok(rows)
}
fn home(row: &Connection, requested: Option<&str>) -> Result<String, ToolError> {
    let target = requested
        .map(str::to_owned)
        .or_else(|| row.home_url.clone())
        .unwrap_or_else(|| format!("https://{}/", row.site));
    if connections::site_url(&target).map_or(true, |(site, _)| site != row.site) {
        return Err(error("connection_cross_site"));
    }
    Ok(target)
}
async fn browser(ctx: &ToolEnv, row: &Connection) -> Result<Arc<Browser>, ToolError> {
    let state = ctx
        .shared
        .db
        .state(&ctx.session)
        .await
        .map_err(|_| error("connection_denied"))?;
    if state.incognito || state.browser_space != BrowserSpace::Shared {
        return Err(error("connection_denied"));
    }
    if let Some(space) = &row.account_space {
        ctx.shared
            .registry
            .browser
            .account_browser(&ctx.shared, &row.site, space)
            .await
    } else {
        ctx.shared
            .registry
            .browser
            .for_session(&ctx.shared, &ctx.session)
            .await
            .map_err(|_| error("connection_browser_unavailable"))
    }
}
async fn fetch_target(
    ctx: &ToolEnv,
    args: &ConnectionFetchArgs,
    row: &Connection,
) -> Result<(Arc<Browser>, String, String), ToolError> {
    let state = ctx
        .shared
        .db
        .state(&ctx.session)
        .await
        .map_err(|_| error("connection_denied"))?;
    if state.incognito || state.browser_space != BrowserSpace::Shared {
        return Err(error("connection_denied"));
    }
    let browser = ctx
        .shared
        .registry
        .browser
        .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
        .await?;
    Controller::owned(&browser, &ctx.session, &args.tab_id).await?;
    if ctx
        .shared
        .registry
        .browser
        .account_space_for_tab(&args.tab_id)
        .await
        != row.account_space
    {
        return Err(error("connection_cross_site"));
    }
    let origin = browser
        .origin(&args.tab_id)
        .await
        .map_err(|_| error("connection_cross_site"))?;
    let base = url::Url::parse(&origin).map_err(|_| error("connection_cross_site"))?;
    if connections::site(base.host_str().unwrap_or_default()).as_deref() != Some(row.site.as_str())
    {
        return Err(error("connection_cross_site"));
    }
    let target = base
        .join(&args.path)
        .map_err(|_| error("connection_cross_site"))?;
    if target.origin() != base.origin()
        || !target.username().is_empty()
        || target.password().is_some()
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return Err(error("connection_cross_site"));
    }
    Ok((browser, origin, target.path().into()))
}
fn method(args: &ConnectionFetchArgs) -> Result<String, ToolError> {
    let method = args.method.to_ascii_uppercase();
    if method.is_empty()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    {
        return Err(error("connection_invalid_method"));
    }
    if matches!(method.as_str(), "GET" | "HEAD") && args.json_body.is_some() {
        return Err(error("connection_invalid_body"));
    }
    Ok(method)
}
pub(crate) fn write_descriptor(name: &str, args: &Value) -> Option<String> {
    if name != "connection_fetch" {
        return None;
    }
    let args: ConnectionFetchArgs = parse(args).ok()?;
    let method = method(&args).ok()?;
    (!matches!(method.as_str(), "GET" | "HEAD")).then(|| format!("{method} {}", args.path))
}

pub(crate) enum GrantAdmission {
    Allowed,
    Pending {
        ticket: TicketId,
        connection: String,
        site: String,
    },
    Ask {
        request: Box<TicketRequest>,
        tool: TicketTool,
        binding: [u8; 32],
    },
}
pub(crate) fn grant_binding(tool: &TicketTool) -> [u8; 32] {
    let mut value = json!({"tool":tool.name,"args":tool.args});
    value.sort_all_objects();
    Sha256::digest(value.to_string().as_bytes()).into()
}
pub(crate) fn is_grant_ticket(ticket: &Ticket) -> bool {
    ticket.deferral == TicketDeferral::External
        && ticket.kind == TicketKind::Confirm
        && ticket.tool.as_ref().is_some_and(|tool| {
            matches!(tool.name.as_str(), "connection_open" | "connection_fetch")
                && tool.args["grant"] == true
        })
        && ticket
            .options
            .iter()
            .map(|option| option.value.as_str())
            .eq(["once", "always", "reject"])
}

pub(crate) const GRANT_KEEP: &[&[&str]] = &[
    &["tool", "args", "grant"],
    &["tool", "args", "connection_id"],
    &["tool", "args", "agent_id"],
    &["tool", "args", "requesting_origin"],
    &["tool", "args", "connection_ids", "*"],
    &["tool", "args", "navigation_tab_id"],
    &["options", "0", "value"],
    &["options", "1", "value"],
    &["options", "2", "value"],
];

pub(crate) fn result_keep(name: &str) -> &'static [&'static [&'static str]] {
    match name {
        "create_task" | "delegate" => {
            &[&["subagent_id"], &["model"], &["tools", "*"], &["context"]]
        }
        "get_tasks" => &[&["tasks", "*", "task_id"]],
        "steer_task" | "cancel_task" => &[&["task_id"]],
        "connections_list" => &[
            &["*", "id"],
            &["*", "site"],
            &["*", "status"],
            &["*", "access"],
            &["*", "used_at"],
        ],
        "connection_check" | "connection_confirm" => &[
            &["id"],
            &["site"],
            &["profile"],
            &["identity_source"],
            &["status"],
            &["source"],
            &["vault_item_id"],
            &["jar_group"],
            &["account_space"],
            &["created_at"],
            &["verified_at"],
            &["used_at"],
            &["used_by"],
            &["expires_at"],
            &["grants", "*", "agent_id"],
            &["grants", "*", "access"],
        ],
        "connection_observe" => &[&["*", "request_id"]],
        "connection_open" => &[&["tab_id"], &["status"]],
        _ => &[&["tab_id"]],
    }
}

async fn grant_tool(
    ctx: &ToolEnv,
    name: &str,
    row: &Connection,
    rows: &[Connection],
    tab: Option<&str>,
) -> Result<(TicketTool, Option<TicketTarget>), ToolError> {
    let requesting = if let Some(tab) = tab {
        let browser = ctx
            .shared
            .registry
            .browser
            .for_tab(&ctx.shared, &ctx.session, tab)
            .await?;
        Some((
            tab.to_owned(),
            browser
                .origin(tab)
                .await
                .map_err(|_| error("connection_cross_site"))?,
        ))
    } else if let Some((browser, tab)) = ctx.shared.registry.browser.preview_tab(&ctx.session).await
    {
        Some((
            tab.clone(),
            browser
                .origin(&tab)
                .await
                .map_err(|_| error("connection_cross_site"))?,
        ))
    } else {
        None
    };
    let origin = requesting
        .as_ref()
        .map(|(_, origin)| origin.as_str())
        .unwrap_or("no open page");
    let target = requesting.as_ref().map(|(tab, origin)| TicketTarget {
        tab_id: Some(tab.clone()),
        origin: Some(origin.clone()),
        field_ref: None,
    });
    let ids: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
    Ok((
        TicketTool {
            name: name.into(),
            args: json!({"grant":true,"connection_id":row.id,"connection_ids":ids,"agent_id":agent(ctx)?,"requesting_origin":origin}),
        },
        target,
    ))
}
async fn once_ticket(
    ctx: &ToolEnv,
    tool: &TicketTool,
    subagent: Option<&str>,
    consume: bool,
) -> Result<bool, ToolError> {
    let binding = grant_binding(tool);
    let state = ctx
        .shared
        .db
        .state(&ctx.session)
        .await
        .map_err(|_| error("connection_denied"))?;
    for ticket in state
        .tickets
        .values()
        .filter(|ticket| is_grant_ticket(ticket) && ticket.tool.as_ref() == Some(tool))
    {
        let session = ctx.session.clone();
        let id = ticket.id.clone();
        let subagent = subagent.map(str::to_owned);
        let available = ctx
            .shared
            .db
            .store(move |store| {
                Ok(if consume {
                    store.claim_connection_grant(&session, &id, subagent.as_deref(), &binding)?
                } else {
                    store.connection_grant_available(
                        &session,
                        &id,
                        subagent.as_deref(),
                        &binding,
                    )?
                })
            })
            .await
            .map_err(|_| error("connection_denied"))?;
        if available {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn document_grant(
    ctx: &ToolEnv,
    args: &Value,
    rows: &[Connection],
) -> Result<bool, ToolError> {
    let Some(tab) = args["tab_id"].as_str() else {
        return Ok(false);
    };
    let browser = ctx
        .shared
        .registry
        .browser
        .for_tab(&ctx.shared, &ctx.session, tab)
        .await?;
    let epoch = browser
        .connection_document_epoch(ctx.session.as_str(), tab)
        .await
        .map_err(super::browser::browser_error)?;
    let ids: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
    Ok(ctx
        .shared
        .connections
        .navigation
        .document_allowed(&ctx.session, tab, epoch, &ids))
}
pub(crate) async fn admit_grant(
    ctx: &ToolEnv,
    name: &str,
    args: &Value,
    subagent: Option<&str>,
) -> Result<GrantAdmission, ToolError> {
    if !matches!(name, "connection_open" | "connection_fetch") {
        return Ok(GrantAdmission::Allowed);
    }
    match name {
        "connection_open" => {
            let _: ConnectionOpenArgs = parse(args)?;
        }
        "connection_fetch" => {
            let _: ConnectionFetchArgs = parse(args)?;
        }
        _ => {}
    }
    let id = args["connection_id"]
        .as_str()
        .ok_or_else(|| error("connection_not_found"))?;
    let row = row(ctx, id)?;
    let rows = grant_rows(ctx, &row)?;
    let agent_id = agent(ctx)?;
    if rows
        .iter()
        .all(|row| access(row, &agent_id) == ConnectionAccess::Allowed)
    {
        return Ok(GrantAdmission::Allowed);
    }
    if name == "connection_fetch" && document_grant(ctx, args, &rows).await? {
        return Ok(GrantAdmission::Allowed);
    }
    let (tool, target) = grant_tool(ctx, name, &row, &rows, args["tab_id"].as_str()).await?;
    if once_ticket(ctx, &tool, subagent, false).await? {
        return Ok(GrantAdmission::Allowed);
    }
    for ticket in ctx
        .shared
        .tickets()
        .await
        .map_err(|_| error("connection_denied"))?
    {
        if ticket.status == TicketStatus::Pending
            && ticket.expires_at > chrono::Utc::now()
            && connections::navigation::Navigation::ticket_ids(&ticket).is_some_and(|(who, ids)| {
                who == agent_id && rows.iter().any(|row| ids.contains(&row.id))
            })
        {
            return Ok(GrantAdmission::Pending {
                ticket: ticket.id,
                connection: row.id,
                site: row.site,
            });
        }
    }
    let agent = ctx
        .shared
        .agents
        .get(&agent_id)
        .map_err(|_| error("connection_denied"))?
        .ok_or_else(|| error("connection_denied"))?;
    let origin = tool.args["requesting_origin"]
        .as_str()
        .unwrap_or("no open page")
        .to_owned();
    let accounts = rows
        .iter()
        .map(|row| {
            row.identity
                .as_deref()
                .unwrap_or("Account not identified yet")
        })
        .collect::<Vec<_>>()
        .join("; ");
    Ok(GrantAdmission::Ask { binding:grant_binding(&tool), tool, request:Box::new(TicketRequest {
        kind:TicketKind::Confirm, summary:format!("{} wants to use your sign-in at {} while on {origin}", agent.name, row.site),
        instructions:Some(format!("Accounts on this site: {accounts}. Allow once, or always allow this agent to use these connections. Site actions still follow your approval rules.")),
        options:vec![TicketOption {value:"once".into(),label:"Allow once".into()}, TicketOption {value:"always".into(),label:"Always for this agent".into()}, TicketOption {value:"reject".into(),label:"Not now".into()}],
        target, resume_when:None, otp:None, otp_register:None, login:None, choice:None, approval:None, form:None, hire:None, pins:vec![],
    }) })
}
async fn consume_grant(
    ctx: &ToolEnv,
    name: &str,
    args: &Value,
    row: &Connection,
) -> Result<Option<TicketTool>, ToolError> {
    let agent = agent(ctx)?;
    let rows = grant_rows(ctx, row)?;
    if rows
        .iter()
        .all(|row| access(row, &agent) == ConnectionAccess::Allowed)
    {
        return Ok(None);
    }
    if name == "connection_fetch" && document_grant(ctx, args, &rows).await? {
        return Ok(None);
    }
    let (tool, _) = grant_tool(ctx, name, row, &rows, args["tab_id"].as_str()).await?;
    if once_ticket(ctx, &tool, ctx.subagent.as_deref(), true).await? {
        Ok(Some(tool))
    } else {
        Err(error("connection_denied"))
    }
}
pub(crate) async fn resolve_grant(
    shared: &Shared,
    ticket: &Ticket,
    value: Value,
) -> Result<Value, crate::api::HttpError> {
    use crate::api::HttpError;
    use axum::http::StatusCode;
    let answer = value
        .as_str()
        .filter(|answer| matches!(*answer, "once" | "always" | "reject"))
        .ok_or_else(HttpError::invalid_value)?;
    let tool = ticket
        .tool
        .as_ref()
        .filter(|_| is_grant_ticket(ticket))
        .ok_or_else(HttpError::invalid_value)?;
    let id = tool.args["connection_id"]
        .as_str()
        .ok_or_else(HttpError::invalid_value)?;
    let agent = tool.args["agent_id"]
        .as_str()
        .ok_or_else(HttpError::invalid_value)?;
    let state = shared
        .db
        .state(&ticket.session_id)
        .await
        .map_err(|_| HttpError::internal())?;
    if state.agent_id.as_deref().unwrap_or("chief") != agent
        || shared
            .agents
            .get(agent)
            .map_err(|_| HttpError::internal())?
            .map_or(true, |agent| agent.status != AgentStatus::Active)
    {
        return Err(HttpError(StatusCode::FORBIDDEN, "connection_denied"));
    }
    let _gate = shared.connections.gate.lock().await;
    let ids: Vec<String> = match tool.args["connection_ids"].as_array() {
        Some(ids) => ids
            .iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or_else(HttpError::invalid_value)
            })
            .collect::<Result<_, _>>()?,
        None => vec![id.into()],
    };
    let mut rows = Vec::with_capacity(ids.len());
    for id in &ids {
        let row = shared
            .connections
            .get(id)
            .map_err(|_| HttpError::internal())?
            .ok_or(HttpError(StatusCode::NOT_FOUND, "connection_not_found"))?;
        if access(&row, agent) == ConnectionAccess::Denied {
            return Err(HttpError(StatusCode::FORBIDDEN, "connection_denied"));
        }
        rows.push(row);
    }
    if answer == "always" {
        shared
            .connections
            .allow_agent(&rows, agent, &ticket.session_id)
            .map_err(|_| HttpError::internal())?;
        if let Some(browser) = shared.registry.browser.connection_instance(&rows[0]).await {
            browser
                .rearm_connection_requests(None, false)
                .await
                .map_err(|_| HttpError::internal())?;
        }
    }
    Ok(json!({"approved":answer != "reject","access":answer}))
}

impl DaemonTool for ConnectionTool {
    fn name(&self) -> &'static str {
        match self.0 {
            Kind::List => "connections_list",
            Kind::Open => "connection_open",
            Kind::Check => "connection_check",
            Kind::Confirm => "connection_confirm",
            Kind::Observe => "connection_observe",
            Kind::Fetch => "connection_fetch",
        }
    }
    fn spec(&self) -> openside_core::ToolSpec {
        match self.0 {
            Kind::List=>spec::<ConnectionsListArgs>(self.name(), "Metadata only. status=candidate: discovered sign-in, not verified; active: checked. access: agent permission, separate from status. used_at: last successful use.", ApprovalTier::Read, false),
            Kind::Open=>spec::<ConnectionOpenArgs>(self.name(), "Open a signed-in site in an owned tab. Ask grants need person approval; Browse policy applies. Use request_login on needs_login.", ApprovalTier::Read, true),
            Kind::Check=>spec::<ConnectionCheckArgs>(self.name(), "Refresh connection metadata in a background tab under Browse policy.", ApprovalTier::Read, true),
            Kind::Confirm=>spec::<ConnectionConfirmArgs>(self.name(), "Label the visible account; labels are masked and grant no authority.", ApprovalTier::Read, false),
            Kind::Observe=>spec::<ConnectionObserveArgs>(self.name(), "Read owned-tab fetch/XHR method, path, status, content type and header names, never values.", ApprovalTier::Read, false),
            Kind::Fetch=>spec::<ConnectionFetchArgs>(self.name(), "Fetch a same-origin path in an owned connection tab with browser credentials. GET/HEAD use Browse; other methods need a bound Confirm. reuse_headers_from keeps values private. Responses: redacted, 64 KiB cap, no screenshots.", ApprovalTier::Read, true),
        }
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        matches!(self.0, Kind::List | Kind::Confirm) || ctx.shared.registry.browser.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            agent(ctx)?;
            match self.0 {
                Kind::List => {
                    let _: ConnectionsListArgs = parse(args)?;
                    Ok(vec![])
                }
                Kind::Confirm => {
                    let args: ConnectionConfirmArgs = parse(args)?;
                    let row = row(ctx, &args.connection_id)?;
                    if access(&row, &agent(ctx)?) != ConnectionAccess::Allowed {
                        return Err(error("connection_denied"));
                    }
                    Ok(vec![])
                }
                Kind::Observe => {
                    let args: ConnectionObserveArgs = parse(args)?;
                    ctx.shared
                        .registry
                        .browser
                        .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
                        .await?;
                    Ok(vec![])
                }
                Kind::Open => {
                    let args: ConnectionOpenArgs = parse(args)?;
                    let row = row(ctx, &args.connection_id)?;
                    Ok(vec![Action::new(
                        Capability::Browse,
                        url_origin(&home(&row, args.url.as_deref())?)?,
                    )])
                }
                Kind::Check => {
                    let args: ConnectionCheckArgs = parse(args)?;
                    let row = row(ctx, &args.connection_id)?;
                    Ok(vec![Action::new(
                        Capability::Browse,
                        url_origin(&home(&row, None)?)?,
                    )])
                }
                Kind::Fetch => {
                    let args: ConnectionFetchArgs = parse(args)?;
                    let row = row(ctx, &args.connection_id)?;
                    let (_, origin, path) = fetch_target(ctx, &args, &row).await?;
                    let method = method(&args)?;
                    if !matches!(method.as_str(), "GET" | "HEAD")
                        && ctx.mode == PolicyMode::ReadOnly
                    {
                        return Err(error("connection_write_needs_approval"));
                    }
                    let mut browse = Action::new(Capability::Browse, &origin);
                    browse.label = Some(format!("{method} {path}"));
                    let mut actions = vec![browse];
                    if !matches!(method.as_str(), "GET" | "HEAD")
                        && openside_policy::sensitive_label(
                            &format!(
                                "{} {}",
                                path,
                                args.json_body
                                    .as_ref()
                                    .map(Value::to_string)
                                    .unwrap_or_default()
                            ),
                            "",
                        )
                    {
                        actions.push(Action::new(Capability::Sensitive, origin));
                    }
                    Ok(actions)
                }
            }
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        let mut safe = args.clone();
        if let Some(object) = safe.as_object_mut() {
            for key in ["identity", "query", "json_body"] {
                if object.contains_key(key) {
                    object.insert(key.into(), json!("[redacted]"));
                }
            }
            if let Some(Value::String(url)) = object.get_mut("url") {
                if let Some((_, clean)) = connections::site_url(url) {
                    *url = clean;
                } else {
                    *url = "[redacted]".into();
                }
            }
        }
        safe
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            match self.execute(&args, ctx).await {
                Ok(value) => ToolExec::Done {
                    for_model: value,
                    for_app: None,
                },
                Err(error) => ToolExec::Denied {
                    reason: error.0,
                    hint: None,
                },
            }
        })
    }
}

impl ConnectionTool {
    async fn execute(&self, args: &Value, ctx: &ToolEnv) -> Result<Value, ToolError> {
        let agent = agent(ctx)?;
        match self.0 {
            Kind::List => {
                let args: ConnectionsListArgs = parse(args)?;
                let rows = ctx
                    .shared
                    .connections
                    .list()
                    .map_err(|_| error("connection_not_found"))?;
                Ok(Value::Array(rows.into_iter().filter(|row|row.profile == ConnectionProfile::Agent && access(row,&agent) != ConnectionAccess::Denied && args.site.as_ref().map_or(true, |site|site == &row.site))
                    .map(|row|json!({"id":row.id,"site":row.site,"identity":row.identity,"status":row.status,"access":access(&row,&agent),"used_at":row.used_at,"accounts_on_site":row.accounts_on_site})).collect()))
            }
            Kind::Confirm => {
                let args: ConnectionConfirmArgs = parse(args)?;
                let identity = Zeroizing::new(args.identity);
                if identity.trim().is_empty() || identity.len() > 4096 {
                    return Err(error("connection_invalid_identity"));
                }
                let _gate = ctx.shared.connections.gate.lock().await;
                let row = row(ctx, &args.connection_id)?;
                if access(&row, &agent) != ConnectionAccess::Allowed {
                    return Err(error("connection_denied"));
                }
                ctx.shared
                    .connections
                    .confirm_identity(&row, &identity)
                    .map_err(|_| error("connection_invalid_identity"))?;
                Ok(json!(ctx
                    .shared
                    .connections
                    .get(&row.id)
                    .map_err(|_| error("connection_not_found"))?))
            }
            Kind::Observe => {
                let args: ConnectionObserveArgs = parse(args)?;
                let browser = ctx
                    .shared
                    .registry
                    .browser
                    .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
                    .await?;
                Ok(json!(browser
                    .connection_requests(ctx.session.as_str(), &args.tab_id)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?))
            }
            Kind::Check => {
                let args_t: ConnectionCheckArgs = parse(args)?;
                let _gate = ctx.shared.connections.gate.lock().await;
                let row = row(ctx, &args_t.connection_id)?;
                let browser = browser(ctx, &row).await?;
                let checked = connections::api::check_in_session(
                    &ctx.shared,
                    &row,
                    &ctx.session,
                    &browser,
                    None,
                    ctx.subagent.as_deref(),
                )
                .await
                .map_err(|error| super::ToolError(error.1.into()))?;
                Ok(json!(checked))
            }
            Kind::Open => {
                let args_t: ConnectionOpenArgs = parse(args)?;
                let _gate = ctx.shared.connections.gate.lock().await;
                let mut row = row(ctx, &args_t.connection_id)?;
                let authorization = consume_grant(ctx, self.name(), args, &row).await?;
                let authorized_ids: Vec<String> = authorization
                    .as_ref()
                    .and_then(|tool| tool.args["connection_ids"].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect();
                let target = home(&row, args_t.url.as_deref())?;
                let browser = browser(ctx, &row).await?;
                let reuse = browser.tabs().await.into_iter().find(|tab| {
                    tab.opened_by == OpenedBy::Agent
                        && tab.owner_session_id.as_deref() == Some(ctx.session.as_str())
                        && connections::site_url(&tab.url).is_some_and(|(site, _)| site == row.site)
                        && ctx
                            .shared
                            .registry
                            .browser
                            .task_tabs
                            .holder(&ctx.session, &tab.id)
                            == ctx.subagent
                });
                let tab = match reuse {
                    Some(tab) => tab,
                    None => browser
                        .create_agent_tab(ctx.session.as_str(), "about:blank")
                        .await
                        .map_err(|_| error("connection_browser_unavailable"))?,
                };
                if let Some(task) = &ctx.subagent {
                    ctx.shared
                        .registry
                        .browser
                        .task_tabs
                        .claim(&ctx.session, &tab.id, task);
                }
                let _authorization: Vec<_> = authorized_ids
                    .iter()
                    .map(|id| {
                        ctx.shared.connections.navigation.authorize(
                            &ctx.session,
                            &tab.id,
                            id,
                            &agent,
                        )
                    })
                    .collect();
                browser
                    .track_network(ctx.session.as_str(), &tab.id)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?;
                browser
                    .navigate(ctx.session.as_str(), &tab.id, &target)
                    .await
                    .map_err(super::browser::browser_error)?;
                if !authorized_ids.is_empty() {
                    let epoch = browser
                        .connection_document_epoch(ctx.session.as_str(), &tab.id)
                        .await
                        .map_err(super::browser::browser_error)?;
                    ctx.shared.connections.navigation.activate(
                        &ctx.session,
                        &tab.id,
                        epoch,
                        &authorized_ids,
                    );
                }
                if row.status == ConnectionStatus::Unverified {
                    row = connections::api::check_loaded(
                        &ctx.shared,
                        &row,
                        &ctx.session,
                        &browser,
                        &tab.id,
                    )
                    .await
                    .map_err(|error| super::ToolError(error.1.into()))?;
                }
                let url = browser
                    .url(&tab.id)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?;
                if connections::site_url(&url).map_or(true, |(site, _)| site != row.site) {
                    return Err(error("connection_cross_site"));
                }
                let snapshot = browser
                    .snapshot(&tab.id)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?;
                let cookies = browser
                    .site_cookie_keys()
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?;
                let expired = ctx
                    .shared
                    .connections
                    .fingerprint_expired(&row, &cookies)
                    .map_err(|_| error("connection_not_found"))?;
                let mut unauthorized = false;
                if let Some(frame) = browser
                    .frames(ctx.session.as_str(), &tab.id)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?
                    .iter()
                    .find(|frame| frame.parent.is_none())
                {
                    unauthorized = browser
                        .document_response(ctx.session.as_str(), &tab.id, &frame.loader)
                        .await
                        .map_err(|_| error("connection_browser_unavailable"))?
                        .is_some_and(|response| matches!(response.status, 401 | 403));
                }
                let needs_login = matches!(
                    crate::browser_state::classify(&snapshot.nodes, &url, "").0,
                    PageKind::Login | PageKind::Otp
                ) || expired
                    || unauthorized
                    || row.status == ConnectionStatus::Expired
                    || row
                        .expires_at
                        .is_some_and(|expiry| expiry <= chrono::Utc::now());
                if needs_login {
                    ctx.shared
                        .connections
                        .mark(&row, ConnectionStatus::NeedsLogin)
                        .map_err(|_| error("connection_not_found"))?;
                    row.status = ConnectionStatus::NeedsLogin;
                }
                ctx.shared.registry.browser.note_tab(&ctx.session, &tab.id);
                ctx.shared
                    .registry
                    .browser
                    .persist(&ctx.shared)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?;
                record_use(ctx, &row, ConnectionUseMode::Open, None, None).await?;
                let note = if needs_login {
                    Some("Call request_login on this tab to sign in again.".into())
                } else if row.accounts_on_site > 1 {
                    Some(format!(
                        "Select {} in the site's account switcher.",
                        row.identity.as_deref().unwrap_or("the intended account")
                    ))
                } else {
                    None::<String>
                };
                let note = if let Some(space) = &row.account_space {
                    Some(format!("{} Account space {space}; opening it may have closed another account space for this site. The shared sign-in is unchanged.",note.unwrap_or_default()))
                } else {
                    note
                };
                Ok(json!({"tab_id":tab.id,"status":row.status,"note":note}))
            }
            Kind::Fetch => {
                let args_t: ConnectionFetchArgs = parse(args)?;
                let _gate = ctx.shared.connections.gate.lock().await;
                let row = row(ctx, &args_t.connection_id)?;
                let (browser, origin, path) = fetch_target(ctx, &args_t, &row).await?;
                let method = method(&args_t)?;
                if !matches!(method.as_str(), "GET" | "HEAD")
                    && (!ctx.action_approved || ctx.mode == PolicyMode::ReadOnly)
                {
                    return Err(error("connection_write_needs_approval"));
                }
                let authorization = consume_grant(ctx, self.name(), args, &row).await?;
                let ids: Vec<_> = authorization
                    .as_ref()
                    .and_then(|tool| tool.args["connection_ids"].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect();
                if !ids.is_empty() {
                    let epoch = browser
                        .connection_document_epoch(ctx.session.as_str(), &args_t.tab_id)
                        .await
                        .map_err(super::browser::browser_error)?;
                    ctx.shared.connections.navigation.activate(
                        &ctx.session,
                        &args_t.tab_id,
                        epoch,
                        &ids,
                    );
                }
                ctx.shared
                    .registry
                    .browser
                    .mark_secret(&ctx.shared, &browser, &args_t.tab_id)
                    .await
                    .map_err(|_| error("connection_browser_unavailable"))?;
                let mut result=browser.connection_fetch(ctx.session.as_str(),&args_t.tab_id,&origin,
                    json!({"method":method,"path":path,"query":args_t.query,"json_body":args_t.json_body}),args_t.reuse_headers_from.as_deref(),
                    |value,is_cookie| {
                        let redactor=if is_cookie {
                            let Some(redactor)=openside_vault::Redactor::for_cookie(value) else{return;};
                            redactor
                        }else{
                            let secret=openside_vault::Secret::new(value.into());
                            openside_vault::Redactor::new([&secret])
                        };
                        let mut installed=ctx.shared.registry.redactors.write();
                        if !redactor.covered_by(&installed) {installed.push(redactor);}
                    }).await.map_err(|_|error("connection_browser_unavailable"))?;
                record_use(
                    ctx,
                    &row,
                    ConnectionUseMode::Fetch,
                    Some(&method),
                    Some(&path),
                )
                .await?;
                if matches!(result["status"].as_u64(), Some(401 | 403))
                    || result["needs_login"] == true
                {
                    ctx.shared
                        .connections
                        .mark(&row, ConnectionStatus::NeedsLogin)
                        .map_err(|_| error("connection_not_found"))?;
                    result["note"] = json!("Call request_login on this tab to sign in again.");
                }
                result = ctx
                    .shared
                    .registry
                    .redact(result)
                    .map_err(|_| error("connection_browser_unavailable"))?;
                if let Some(body) = result["body"].as_str() {
                    let mut body = body.to_owned();
                    cap_body(&mut body);
                    result["body"] = json!(body);
                }
                Ok(result)
            }
        }
    }
}
fn cap_body(body: &mut String) {
    if body.len() > BODY_CAP {
        let mut end = BODY_CAP;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        body.truncate(end);
    }
}
async fn record_use(
    ctx: &ToolEnv,
    row: &Connection,
    mode: ConnectionUseMode,
    method: Option<&str>,
    path: Option<&str>,
) -> Result<(), ToolError> {
    let detail = method
        .zip(path)
        .map(|(method, path)| {
            ctx.shared
                .registry
                .redact(Value::String(format!("{method} {path}")))
                .map(|value| value.as_str().unwrap_or_default().to_owned())
        })
        .transpose()
        .map_err(|_| error("connection_not_found"))?;
    ctx.shared
        .connections
        .record_use(
            row,
            &ctx.session,
            &agent(ctx)?,
            if mode == ConnectionUseMode::Open {
                ConnectionAuditAction::Open
            } else {
                ConnectionAuditAction::Fetch
            },
            detail.as_deref(),
        )
        .map_err(|_| error("connection_not_found"))?;
    ctx.shared
        .emit(
            &ctx.session,
            None,
            EventKind::ConnectionUsed {
                connection_id: row.id.clone(),
                site: row.site.clone(),
                mode,
                method: method.map(str::to_owned),
            },
        )
        .await
        .map_err(|_| error("connection_not_found"))?;
    Ok(())
}
