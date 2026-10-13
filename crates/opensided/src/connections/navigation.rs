use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use openside_browser::{DocumentPermission, DocumentRequest};
use openside_core::ToolCall;
use openside_proto::*;
use serde_json::json;

use crate::{
    engine::Shared,
    persistence::{Database, Result},
    tools::TicketRequest,
};

#[derive(Clone)]
struct Request {
    session: SessionId,
    tab: String,
    agent: String,
    site: String,
    requesting_origin: String,
    connection_ids: Vec<String>,
    account_space: Option<String>,
}

type PermitKey = (SessionId, String, String, String);

pub(crate) struct Policy<'a> {
    pub service: &'a super::Service,
    pub db: &'a Database,
    pub agents: &'a crate::agents::Service,
    pub ttl_secs: u32,
    pub account_space: Option<&'a str>,
}

struct DocumentGrants {
    epoch: u64,
    connections: BTreeSet<String>,
}

pub(crate) struct Navigation {
    sender: tokio::sync::mpsc::UnboundedSender<Request>,
    receiver: tokio::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Request>>>,
    pub(crate) grant_gate: tokio::sync::Mutex<()>,
    pending: parking_lot::Mutex<BTreeMap<(String, String), DateTime<Utc>>>,
    permits: Arc<parking_lot::Mutex<BTreeMap<PermitKey, usize>>>,
    documents: parking_lot::Mutex<BTreeMap<(SessionId, String), DocumentGrants>>,
}

impl Default for Navigation {
    fn default() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            sender,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
            grant_gate: Default::default(),
            pending: Default::default(),
            permits: Default::default(),
            documents: Default::default(),
        }
    }
}

pub(crate) struct Permit {
    key: PermitKey,
    permits: Arc<parking_lot::Mutex<BTreeMap<PermitKey, usize>>>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut permits = self.permits.lock();
        if let Some(count) = permits.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                permits.remove(&self.key);
            }
        }
    }
}

pub(crate) fn access(row: &Connection, agent: &str) -> ConnectionAccess {
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

impl Navigation {
    pub(crate) fn authorize(
        &self,
        session: &SessionId,
        tab: &str,
        connection: &str,
        agent: &str,
    ) -> Permit {
        let key = (session.clone(), tab.into(), connection.into(), agent.into());
        *self.permits.lock().entry(key.clone()).or_default() += 1;
        Permit {
            key,
            permits: self.permits.clone(),
        }
    }

    pub(crate) fn decide(
        &self,
        policy: Policy<'_>,
        request: &DocumentRequest,
    ) -> DocumentPermission {
        let Policy {
            service,
            db,
            agents,
            ttl_secs,
            account_space,
        } = policy;
        let Some((site, _)) = super::site_url(&request.url) else {
            return DocumentPermission::Allowed;
        };
        let Ok(rows) = service.request_site(&site, account_space) else {
            return DocumentPermission::ConnectionDenied;
        };
        if rows.is_empty() {
            return DocumentPermission::Allowed;
        }
        let session = SessionId::new(&request.owner);
        let Ok(agent) = db.connection_document_principal(&session) else {
            return DocumentPermission::ConnectionDenied;
        };
        if agents
            .get(&agent)
            .ok()
            .flatten()
            .map_or(true, |agent| agent.status != AgentStatus::Active)
            || rows
                .iter()
                .any(|row| access(row, &agent) == ConnectionAccess::Denied)
        {
            return DocumentPermission::ConnectionDenied;
        }
        let missing: Vec<_> = {
            let permits = self.permits.lock();
            let documents = self.documents.lock();
            let document = documents
                .get(&(session.clone(), request.tab_id.clone()))
                .filter(|grant| grant.epoch == request.document_epoch);
            rows.iter()
                .filter(|row| {
                    access(row, &agent) == ConnectionAccess::Ask
                        && !permits.contains_key(&(
                            session.clone(),
                            request.tab_id.clone(),
                            row.id.clone(),
                            agent.clone(),
                        ))
                        && !document.is_some_and(|grant| grant.connections.contains(&row.id))
                })
                .map(|row| row.id.clone())
                .collect()
        };
        if missing.is_empty() {
            return DocumentPermission::Allowed;
        }
        if db
            .claim_connection_document_grant(&session, &agent, &request.tab_id, &missing)
            .unwrap_or(false)
        {
            self.activate(&session, &request.tab_id, request.document_epoch, &missing);
            self.release(&agent, &missing);
            return DocumentPermission::Allowed;
        }
        let ids: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
        let now = Utc::now();
        let mut pending = self.pending.lock();
        let queued = ids.iter().any(|id| {
            pending
                .get(&(agent.clone(), id.clone()))
                .is_some_and(|expires| *expires > now)
        });
        if !queued {
            let expires = now + chrono::Duration::seconds(i64::from(ttl_secs));
            for id in &ids {
                pending.insert((agent.clone(), id.clone()), expires);
            }
            if self
                .sender
                .send(Request {
                    session,
                    tab: request.tab_id.clone(),
                    agent: agent.clone(),
                    site: site.clone(),
                    requesting_origin: request.requesting_origin.clone(),
                    connection_ids: ids.clone(),
                    account_space: account_space.map(str::to_owned),
                })
                .is_err()
            {
                for id in &ids {
                    pending.remove(&(agent.clone(), id.clone()));
                }
                return DocumentPermission::ConnectionDenied;
            }
        }
        DocumentPermission::ConnectionGrantRequired {
            connection_id: missing[0].clone(),
            site,
        }
    }

    pub(crate) fn activate(&self, session: &SessionId, tab: &str, epoch: u64, ids: &[String]) {
        let mut documents = self.documents.lock();
        let grant = documents
            .entry((session.clone(), tab.into()))
            .or_insert_with(|| DocumentGrants {
                epoch,
                connections: BTreeSet::new(),
            });
        if grant.epoch != epoch {
            grant.epoch = epoch;
            grant.connections.clear();
        }
        grant.connections.extend(ids.iter().cloned());
    }

    pub(crate) fn document_allowed(
        &self,
        session: &SessionId,
        tab: &str,
        epoch: u64,
        ids: &[String],
    ) -> bool {
        self.documents
            .lock()
            .get(&(session.clone(), tab.into()))
            .is_some_and(|grant| {
                grant.epoch == epoch && ids.iter().all(|id| grant.connections.contains(id))
            })
    }

    pub(crate) fn close_tab(&self, session: &SessionId, tab: &str) {
        self.documents.lock().remove(&(session.clone(), tab.into()));
    }

    pub(crate) fn invalidate_tab(&self, session: &SessionId, tab: &str, ids: &[String]) {
        let mut documents = self.documents.lock();
        let key = (session.clone(), tab.to_owned());
        if let Some(grant) = documents.get_mut(&key) {
            grant.connections.retain(|id| !ids.contains(id));
            if grant.connections.is_empty() {
                documents.remove(&key);
            }
        }
    }

    pub(crate) fn protected_sites(policy: Policy<'_>, owner: &str, force: bool) -> Vec<String> {
        let Ok(agent) = policy
            .db
            .connection_document_principal(&SessionId::new(owner))
        else {
            return vec!["*".into()];
        };
        let active = policy
            .agents
            .get(&agent)
            .ok()
            .flatten()
            .is_some_and(|agent| agent.status == AgentStatus::Active);
        policy.service.list().map_or_else(
            |_| vec!["*".into()],
            |rows| {
                rows.into_iter()
                    .filter(|row| {
                        row.account_space.as_deref() == policy.account_space
                            && (force
                                || !active
                                || access(row, &agent) != ConnectionAccess::Allowed)
                    })
                    .map(|row| row.site)
                    .collect()
            },
        )
    }

    pub(crate) fn release(&self, agent: &str, ids: &[String]) {
        let mut pending = self.pending.lock();
        for id in ids {
            pending.remove(&(agent.to_owned(), id.clone()));
        }
    }

    pub(crate) fn expire(&self) {
        let now = Utc::now();
        self.pending.lock().retain(|_, expires| *expires > now);
    }

    pub(crate) fn ticket_ids(ticket: &Ticket) -> Option<(String, Vec<String>)> {
        let tool = ticket
            .tool
            .as_ref()
            .filter(|_| crate::tools::connections::is_grant_ticket(ticket))?;
        let agent = tool.args["agent_id"].as_str()?.to_owned();
        let ids = tool.args["connection_ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![tool.args["connection_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()]
            });
        Some((agent, ids))
    }

    pub(crate) fn start(shared: &Shared) {
        let shared = shared.clone();
        let mut shutdown = shared.shutdown.subscribe();
        tokio::spawn(async move {
            let Some(mut receiver) = shared.connections.navigation.receiver.lock().await.take()
            else {
                return;
            };
            loop {
                let request = tokio::select! {_=shutdown.recv()=>break,request=receiver.recv()=>match request{Some(request)=>request,None=>break}};
                if Self::open(&shared, &request).await.is_err() {
                    shared
                        .connections
                        .navigation
                        .release(&request.agent, &request.connection_ids);
                }
            }
        });
    }

    async fn open(shared: &Shared, request: &Request) -> Result<()> {
        let _grants = shared.connections.navigation.grant_gate.lock().await;
        let state = shared.db.state(&request.session).await?;
        if state.agent_id.as_deref().unwrap_or("chief") != request.agent {
            return Err("connection_denied".into());
        }
        let agent = shared
            .agents
            .get(&request.agent)?
            .filter(|agent| agent.status == AgentStatus::Active)
            .ok_or("connection_denied")?;
        let rows = shared
            .connections
            .request_site(&request.site, request.account_space.as_deref())?;
        if rows.is_empty() {
            shared
                .connections
                .navigation
                .release(&request.agent, &request.connection_ids);
            return Ok(());
        }
        if rows
            .iter()
            .any(|row| access(row, &request.agent) == ConnectionAccess::Denied)
        {
            return Err("connection_denied".into());
        }
        if rows
            .iter()
            .all(|row| access(row, &request.agent) == ConnectionAccess::Allowed)
        {
            shared
                .connections
                .navigation
                .release(&request.agent, &request.connection_ids);
            return Ok(());
        }
        let ids: Vec<_> = rows.iter().map(|row| row.id.clone()).collect();
        for ticket in shared.tickets().await? {
            if ticket.status == TicketStatus::Pending && ticket.expires_at > Utc::now() {
                if let Some((who, covered)) = Self::ticket_ids(&ticket).filter(|(who, covered)| {
                    who == &request.agent && covered.iter().any(|id| ids.contains(id))
                }) {
                    let mut pending = shared.connections.navigation.pending.lock();
                    for id in &ids {
                        pending.remove(&(who.clone(), id.clone()));
                    }
                    for id in covered.iter().filter(|id| ids.contains(id)) {
                        pending.insert((who.clone(), id.clone()), ticket.expires_at);
                    }
                    return Ok(());
                }
            }
        }
        let requesting_origin = if request.requesting_origin == "null" {
            match shared.registry.browser.preview_tab(&request.session).await {
                Some((browser, tab)) if tab != request.tab => browser
                    .origin(&tab)
                    .await
                    .unwrap_or_else(|_| "no open page".into()),
                _ => "no open page".into(),
            }
        } else {
            request.requesting_origin.clone()
        };
        let accounts = rows
            .iter()
            .map(|row| {
                row.identity
                    .as_deref()
                    .unwrap_or("Account not identified yet")
            })
            .collect::<Vec<_>>()
            .join("; ");
        let primary = rows
            .iter()
            .find(|row| access(row, &request.agent) == ConnectionAccess::Ask)
            .ok_or("connection_denied")?
            .id
            .clone();
        // This is a host-generated navigation ticket, not an unresolved sidecar tool call.
        let call = ToolCall {
            tool_call_id: format!(
                "{}connection-document-{}",
                crate::tools::API_CALL_PREFIX,
                crate::persistence::random_id()
            ),
            name: "connection_open".into(),
            arguments_json: json!({"connection_id":primary}).to_string(),
            subagent: None,
            approval_ticket: None,
        };
        let tool = TicketTool {
            name: "connection_open".into(),
            args: json!({"grant":true,"connection_id":primary,"connection_ids":ids,
            "agent_id":request.agent,"requesting_origin":requesting_origin,"navigation_tab_id":request.tab}),
        };
        let binding = crate::tools::connections::grant_binding(&tool);
        crate::tickets::open(shared,&request.session,&call,TicketDeferral::External,TicketRequest {
            kind:TicketKind::Confirm,summary:format!("{} wants to use your sign-in at {} while on {}",agent.name,request.site,requesting_origin),
            instructions:Some(format!("Accounts on this site: {accounts}. Allow once for this tab's current document, including its requests and redirects, or always for this agent. Retry after approval; another top-level document needs a new grant.")),
            options:vec![TicketOption{value:"once".into(),label:"Allow once".into()},TicketOption{value:"always".into(),label:"Always for this agent".into()},TicketOption{value:"reject".into(),label:"Not now".into()}],
            target:None,resume_when:None,otp:None,otp_register:None,login:None,choice:None,approval:None,form:None,hire:None,pins:vec![],
        },Some(tool),Some(binding),None).await?;
        let _=shared.command(&request.session,crate::engine::Action::BrowserNotice(
            format!("connection_grant_required: waiting for the person to allow access to {}. Retry after approval.",request.site))).await;
        Ok(())
    }
}
