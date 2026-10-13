use chrono::{Duration, Utc};
use openside_core::{TicketId, TicketResolution, ToolCall};
use openside_proto::{
    EventKind, SessionId, SessionStatus, Ticket, TicketDeferral, TicketKind, TicketOrigin,
    TicketStatus, TicketTool,
};
use serde_json::{json, Value};

use crate::{
    engine::{Action, Shared},
    persistence::{random_id, Result},
    tools::TicketRequest,
};

pub(crate) fn lifetime(kind: TicketKind, config: &openside_config::TicketsConfig) -> Duration {
    Duration::seconds(i64::from(match kind {
        TicketKind::Confirm => config.confirm_ttl_secs,
        // A sign-in form and a page's choice are bound to one page like a secret field.
        TicketKind::SecretInput | TicketKind::Login | TicketKind::Choice => {
            config.secret_input_ttl_secs
        }
        TicketKind::DeviceApproval => config.device_approval_ttl_secs,
        TicketKind::Takeover => config.takeover_ttl_secs,
        TicketKind::Info | TicketKind::Hire => config.info_ttl_secs,
    }))
}

pub(crate) fn proposal_key(draft: &openside_proto::HireDraft) -> String {
    use unicode_normalization::UnicodeNormalization;
    let normalized: String = draft
        .persona
        .role
        .nfkc()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut words = normalized.split_whitespace().peekable();
    if words
        .peek()
        .is_some_and(|word| matches!(*word, "a" | "an" | "the"))
    {
        words.next();
    }
    words.collect::<Vec<_>>().join(" ")
}

pub(crate) async fn propose_hire(
    ctx: &crate::tools::ToolEnv,
    draft: openside_proto::HireDraft,
) -> crate::tools::ToolExec {
    if ctx.agent_id.as_deref() != Some("chief") || ctx.subagent.is_some() {
        return crate::tools::ToolExec::Denied {
            reason: "chief_only".into(),
            hint: None,
        };
    }
    crate::tools::ToolExec::Defer(Box::new(TicketRequest {
        kind: TicketKind::Hire,
        summary: format!("Hire {}", draft.name),
        instructions: None,
        options: vec![],
        target: None,
        resume_when: None,
        otp: None,
        otp_register: None,
        login: None,
        choice: None,
        approval: None,
        form: None,
        hire: Some(draft),
        pins: vec![],
    }))
}

pub(crate) async fn resolve_hire(
    shared: &Shared,
    ticket: &Ticket,
    value: Value,
    edited: Option<openside_proto::HireDraft>,
) -> std::result::Result<Value, crate::api::HttpError> {
    use crate::api::HttpError;
    use axum::http::StatusCode;
    let original = ticket
        .hire
        .as_ref()
        .ok_or(HttpError(StatusCode::BAD_REQUEST, "invalid_ticket"))?;
    let decision: openside_proto::HireDecision = serde_json::from_value(value)
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_value"))?;
    let key = proposal_key(original);
    let result = match decision {
        openside_proto::HireDecision::Hire => {
            let mut draft = edited.unwrap_or_else(|| original.clone());
            draft.proposal_key = original.proposal_key.clone();
            let agent = crate::team::hire(shared, &draft, ticket.id.as_str()).await?;
            json!({"status":"hired","agent_id":agent.id,"name":agent.name,"role":agent.persona.role})
        }
        openside_proto::HireDecision::NotNow | openside_proto::HireDecision::Never => {
            if edited.is_some() {
                return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_value"));
            }
            json!({"status":if decision == openside_proto::HireDecision::Never { "declined" } else { "later" }})
        }
    };
    let status = match decision {
        openside_proto::HireDecision::Hire => "hire",
        openside_proto::HireDecision::NotNow => "not_now",
        openside_proto::HireDecision::Never => "never",
    };
    if decision == openside_proto::HireDecision::Never {
        shared
            .agents
            .suppress_proposal(&key, &original.persona.role, Utc::now())?;
    } else {
        shared.agents.put_proposal(&key, status, Utc::now())?;
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn open(
    shared: &Shared,
    session: &SessionId,
    call: &ToolCall,
    deferral: TicketDeferral,
    request: TicketRequest,
    tool: Option<TicketTool>,
    approval_binding: Option<[u8; 32]>,
    device_id: Option<&str>,
) -> Result<TicketId> {
    let _gate = shared.ticket_gate.lock().await;
    if let Some(subagent) = &call.subagent {
        // Cancellation ends a child under this gate, so no ticket can outlive its child.
        if !crate::subagents::is_live(shared, session, subagent).await? {
            return Err("subagent_finished".into());
        }
    }
    let state = shared.db.state(session).await?;
    if let Some(ticket) = state.tickets.values().find(|ticket| {
        ticket.tool_call_id == call.tool_call_id
            && ticket.status == TicketStatus::Pending
            && ticket.deferral == deferral
            && ticket.origin.subagent_id == call.subagent
    }) {
        if let Some(binding) = approval_binding {
            let session = session.clone();
            let id = ticket.id.clone();
            if !shared
                .db
                .store(move |store| Ok(store.approval_matches(&session, &id, &binding)?))
                .await?
            {
                return Err("approval_args_mismatch".into());
            }
        }
        return Ok(ticket.id.clone());
    }
    if request.kind == TicketKind::Hire {
        let draft = request.hire.as_ref().ok_or("missing_hire_draft")?;
        let chief = shared.agents.get("chief")?.ok_or("chief_missing")?;
        if chief.home_session_id != *session || call.subagent.is_some() {
            return Err("chief_only".into());
        }
        let mut preview = chief.clone();
        preview.kind = openside_proto::AgentKind::Teammate;
        preview.name = draft.name.clone();
        preview.initials = draft
            .initials
            .clone()
            .unwrap_or_else(|| draft.name.chars().take(2).collect());
        preview.color = draft
            .color
            .clone()
            .unwrap_or_else(|| openside_proto::ACCENT_COLORS[0].into());
        preview.persona = draft.persona.clone();
        crate::agents::validate(&preview)?;
        if shared
            .tickets()
            .await?
            .iter()
            .any(|ticket| ticket.kind == TicketKind::Hire && ticket.status == TicketStatus::Pending)
        {
            return Err("proposal_pending".into());
        }
    }
    if request.kind == TicketKind::SecretInput
        && state
            .tickets
            .values()
            .filter(|ticket| {
                ticket.kind == TicketKind::SecretInput && ticket.target == request.target
            })
            .count()
            >= 3
    {
        return Err("secret input target request limit reached".into());
    }
    let id = TicketId::new(random_id());
    let checked = match &request.target {
        Some(target) => {
            // A one-time-code ticket (asking for a code, or registering an authenticator whose
            // code goes into the setup page) also pins split code boxes.
            let otp_boxes = request.otp.is_some() || request.otp_register.is_some();
            shared
                .registry
                .browser
                .capture_ticket(shared, &id, session, target, otp_boxes, request.pins)
                .await?
        }
        None => Default::default(),
    };
    let now = Utc::now();
    let mut ticket = Ticket {
        hire: request.hire,
        id: id.clone(),
        session_id: session.clone(),
        tool_call_id: call.tool_call_id.clone(),
        deferral,
        kind: request.kind,
        summary: request.summary,
        instructions: request.instructions,
        options: request.options,
        origin: TicketOrigin {
            subagent_id: call.subagent.clone(),
            device_id: device_id.map(str::to_owned),
            driver_ids: state.driver_ids.iter().cloned().collect(),
        },
        tool,
        target: request.target,
        resume_when: request.resume_when,
        created_at: now,
        expires_at: now + lifetime(request.kind, &shared.config.loaded.config.tickets),
        status: TicketStatus::Pending,
        resolution: None,
        otp: request.otp,
        otp_register: request.otp_register,
        login: request.login,
        choice: request.choice,
        approval: request.approval,
        card: None,
        form: request.form,
    };
    // Target strings are kept verbatim only when the daemon checked them against the live
    // browser; a caller-supplied tab id, origin or field ref is redacted like any other text.
    let mut keep = crate::tools::TICKET_KEEP.to_vec();
    if crate::tools::connections::is_grant_ticket(&ticket) {
        keep.extend_from_slice(crate::tools::connections::GRANT_KEEP);
    }
    let [tab_id, origin, field_ref] = crate::tools::TICKET_TARGET_KEEP;
    keep.extend(
        [
            (checked.tab_id, tab_id),
            (checked.origin, origin),
            (checked.field_ref, field_ref),
        ]
        .into_iter()
        .filter_map(|(checked, path)| checked.then_some(path)),
    );
    ticket = serde_json::from_value(
        shared
            .registry
            .redact_except(serde_json::to_value(ticket)?, &keep)?,
    )?;
    if let Some(draft) = &ticket.hire {
        let key = proposal_key(draft);
        let aliases: Vec<_> = state
            .tickets
            .values()
            .filter_map(|ticket| ticket.hire.as_ref())
            .map(|old| {
                let legacy = old
                    .proposal_key
                    .as_deref()
                    .filter(|key| !key.trim().is_empty())
                    .unwrap_or(&old.persona.role)
                    .trim()
                    .to_lowercase();
                (legacy, proposal_key(old))
            })
            .collect();
        shared.agents.migrate_proposal_keys(&aliases)?;
        let stored = shared.agents.get_proposal(&key)?;
        let person_fallback = if call.name == "team_hire" {
            let args: Value = serde_json::from_str(&call.arguments_json)?;
            let message = args["request_message_id"].as_str().ok_or("not_requested")?;
            crate::team::hire_tainted(shared, session, message)
                .await
                .map_err(|error| error.1)?
        } else {
            false
        };
        if !person_fallback {
            if stored.as_ref().is_some_and(|(status, _)| status == "never") {
                return Err("proposal_suppressed".into());
            }
            if shared
                .agents
                .latest_proposal_decline()?
                .is_some_and(|at| Utc::now() < at + Duration::days(7))
            {
                return Err("proposal_global_cooldown".into());
            }
        }
        let previous_ticket = state
            .tickets
            .values()
            .filter(|ticket| {
                ticket.kind == TicketKind::Hire
                    && ticket
                        .hire
                        .as_ref()
                        .is_some_and(|old| proposal_key(old) == key)
            })
            .map(|ticket| ticket.created_at)
            .max();
        let last_proposal = match stored {
            Some((status, cleared_at)) if status == "cleared" => {
                previous_ticket.filter(|at| *at > cleared_at)
            }
            Some((_, at)) => Some(at),
            None => previous_ticket,
        };
        if !person_fallback && last_proposal.is_some_and(|at| Utc::now() < at + Duration::days(30))
        {
            return Err("proposal_cooldown".into());
        }
    }
    if let Some(binding) = approval_binding {
        let session = session.clone();
        let id = id.clone();
        shared
            .db
            .store(move |store| Ok(store.bind_approval(&session, &id, &binding)?))
            .await?;
    }
    shared
        .emit(
            session,
            None,
            EventKind::TicketOpened {
                ticket: Box::new(ticket),
            },
        )
        .await?;
    Ok(id)
}

pub(crate) async fn get(shared: &Shared, id: &TicketId) -> Result<Option<Ticket>> {
    Ok(shared
        .tickets()
        .await?
        .into_iter()
        .find(|ticket| &ticket.id == id))
}

pub(crate) fn resolution(ticket: &Ticket) -> TicketResolution {
    if ticket.deferral == TicketDeferral::Approval {
        if ticket.status == TicketStatus::Resolved
            && ticket
                .resolution
                .as_ref()
                .is_some_and(|value| value["approved"] == true)
        {
            TicketResolution::Approved
        } else {
            let reason = match ticket.status {
                TicketStatus::Expired => "expired",
                _ => ticket
                    .resolution
                    .as_ref()
                    .and_then(|value| value["reason"].as_str())
                    .unwrap_or("rejected"),
            };
            TicketResolution::Denied {
                reason: reason.into(),
            }
        }
    } else {
        let value = if ticket.status == TicketStatus::Expired {
            json!({"status":"expired"})
        } else {
            ticket
                .resolution
                .clone()
                .unwrap_or_else(|| json!({"status":"rejected"}))
        };
        TicketResolution::Result {
            for_model_json: value.to_string(),
        }
    }
}

pub(crate) async fn delivery_resolution(
    shared: &Shared,
    ticket: &Ticket,
) -> Result<TicketResolution> {
    let session = ticket.session_id.clone();
    let id = ticket.id.clone();
    let fresh = shared
        .db
        .store(move |store| Ok(store.mark_ticket_delivered(&session, &id)?))
        .await?;
    if fresh {
        return Ok(resolution(ticket));
    }
    shared
        .emit(
            &ticket.session_id,
            None,
            EventKind::runtime_error(
                "interrupted",
                "The previous resumption was interrupted; verify state before retrying.",
            ),
        )
        .await?;
    Ok(if ticket.deferral == TicketDeferral::Approval {
        TicketResolution::Denied {
            reason: "interrupted_after_resume".into(),
        }
    } else {
        TicketResolution::Result {
            for_model_json: json!({
                "status": "interrupted",
                "message": "The previous resumption was interrupted; verify state before retrying."
            })
            .to_string(),
        }
    })
}

pub(crate) async fn expire(shared: &Shared) -> Result<()> {
    let expired = {
        let _gate = shared.ticket_gate.lock().await;
        let expired: Vec<_> = shared
            .tickets()
            .await?
            .into_iter()
            .filter(|ticket| {
                ticket.status == TicketStatus::Pending && ticket.expires_at <= Utc::now()
            })
            .collect();
        for ticket in &expired {
            close_unanswered(shared, ticket, None).await?;
        }
        expired
    };
    for ticket in expired {
        if shared.db.state(&ticket.session_id).await?.status != SessionStatus::Stopped
            && !shared.auth_failed(&ticket.session_id).await?
        {
            // A transient startup failure does not undo the durable expiry; lazy
            // reopening replays terminal resolutions from ready.pending_deferred.
            let _ = shared
                .command(&ticket.session_id, Action::Resume(ticket.id))
                .await;
        }
    }
    Ok(())
}

pub(crate) async fn reject_open(shared: &Shared, session: &SessionId, reason: &str) -> Result<()> {
    let _gate = shared.ticket_gate.lock().await;
    let state = shared.db.state(session).await?;
    for ticket in state
        .tickets
        .values()
        .filter(|ticket| ticket.status == TicketStatus::Pending)
    {
        close_unanswered(shared, ticket, Some(reason)).await?;
    }
    Ok(())
}

/// End a pending ticket without the person's answer: rejected with `rejected` as the reason
/// (an explicit rejection, a session stop, a subagent cancellation), or expired when `None`.
/// The caller holds `ticket_gate`. A registration whose interrupted attempt saved its factor,
/// or whose outcome cannot be checked, is published as that outcome instead
/// ([`crate::tools::otp::interrupted_registration`]), so no terminal result denies a factor
/// the vault holds.
pub(crate) async fn close_unanswered(
    shared: &Shared,
    ticket: &Ticket,
    rejected: Option<&str>,
) -> Result<()> {
    let (end, mut decision) = match rejected {
        Some(_) => (openside_proto::TakeoverEndReason::Rejected, "rejected"),
        None => (openside_proto::TakeoverEndReason::Expired, "expired"),
    };
    crate::takeover::finish(shared, ticket, end).await;
    let event = match crate::tools::otp::interrupted_registration(shared, ticket).await {
        Some(resolution) => {
            decision = "resolved";
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.id.clone(),
                resolution,
            }
        }
        None => match rejected {
            Some(reason) => EventKind::TicketRejected {
                card: None,
                ticket_id: ticket.id.clone(),
                reason: reason.into(),
            },
            None => EventKind::TicketExpired {
                card: None,
                ticket_id: ticket.id.clone(),
            },
        },
    };
    crate::tools::otp::discard(ticket);
    shared.emit(&ticket.session_id, None, event).await?;
    audit(
        shared,
        &ticket.session_id,
        &ticket.tool_call_id,
        "ticket",
        decision,
        0,
    )
    .await?;
    if matches!(ticket.kind, TicketKind::SecretInput | TicketKind::Login)
        || ticket.otp_register.is_some()
    {
        // The ticket is no longer pending, so a stale intent can never block anything.
        let _ = shared
            .registry
            .browser
            .clear_fill_intent(shared, &ticket.id)
            .await;
    }
    Ok(())
}

pub(crate) async fn audit(
    shared: &Shared,
    session: &SessionId,
    call: &str,
    tool: &str,
    decision: &str,
    duration_ms: u128,
) -> Result<()> {
    let record = json!({"ts":Utc::now(),"session":session,"tool_call_id":call,"tool":tool,"decision":decision,"duration_ms":duration_ms});
    audit_record(shared, record).await
}

/// Append one redacted record to the private `audit.jsonl` (0600, no symlinks, fsynced).
pub(crate) async fn audit_record(shared: &Shared, record: Value) -> Result<()> {
    let path = openside_config::data_layout::DataPath::Audit.path(&shared.config.data_dir);
    // These members are daemon-generated labels and ids; anything else a caller adds (pairing
    // names, details) is scrubbed.
    let record = shared
        .registry
        .redact_except(
            record,
            &[
                &["ts"],
                &["session"],
                &["tool_call_id"],
                &["tool"],
                &["decision"],
                &["device"],
            ],
        )?
        .to_string();
    tokio::task::spawn_blocking(move || -> Result<()> {
        use fs2::FileExt;
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        file.lock_exclusive()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        writeln!(file, "{record}")?;
        file.sync_data()?;
        Ok(())
    })
    .await?
}

pub(crate) fn approved(value: &Value) -> bool {
    value == true || value == "approve"
}
