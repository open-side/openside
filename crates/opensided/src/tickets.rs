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
        TicketKind::SecretInput => config.secret_input_ttl_secs,
        TicketKind::DeviceApproval => config.device_approval_ttl_secs,
        TicketKind::Takeover => config.takeover_ttl_secs,
        TicketKind::Info => config.info_ttl_secs,
    }))
}

pub(crate) async fn open(
    shared: &Shared,
    session: &SessionId,
    call: &ToolCall,
    deferral: TicketDeferral,
    request: TicketRequest,
    tool: Option<TicketTool>,
    approval_binding: Option<[u8; 32]>,
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
                .capture_ticket(shared, &id, session, target, otp_boxes)
                .await?
        }
        None => Default::default(),
    };
    let now = Utc::now();
    let mut ticket = Ticket {
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
    };
    // Target strings are kept verbatim only when the daemon checked them against the live
    // browser; a caller-supplied tab id, origin or field ref is redacted like any other text.
    let mut keep = crate::tools::TICKET_KEEP.to_vec();
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
            EventKind::RuntimeError {
                code: "interrupted".into(),
                message: "The previous resumption was interrupted; verify state before retrying."
                    .into(),
            },
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
                ticket_id: ticket.id.clone(),
                resolution,
            }
        }
        None => match rejected {
            Some(reason) => EventKind::TicketRejected {
                ticket_id: ticket.id.clone(),
                reason: reason.into(),
            },
            None => EventKind::TicketExpired {
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
    if ticket.kind == TicketKind::SecretInput || ticket.otp_register.is_some() {
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
