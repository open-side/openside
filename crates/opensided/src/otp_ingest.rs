//! `POST /v1/otp`: forwarded SMS one-time codes (human-step §4.2).
//!
//! Only senders listed in `otp.senders` are accepted. `sender` is what the forwarding
//! device reports; the trust boundary is that device's `otp_ingest` token, not SMS
//! provenance. Each `otp.sites` entry maps an exact origin to a regex; the first capture
//! group (else the whole match) is the code. A message that yields a code for more than one
//! site is refused (409 `ambiguous_otp`): one SMS must never fill two sites.
//!
//! Codes and tickets meet in one place, [`reconcile`], serialized by one lock: ingest
//! stores the code and then reconciles, and the listener reconciles when a `secret_input`
//! ticket opens or the event stream lagged. Whichever runs second sees the other's state,
//! so no wakeup is lost, and a code is taken under the lock before it fills a ticket, so
//! it fills at most one. A filled ticket goes through the same path as
//! `POST /v1/tickets/{id}/resolve` (origin re-check, redaction, zeroize). Unused codes live
//! in memory for `otp.ttl_secs` of monotonic time and are zeroized by a sweep at expiry.
//! Codes are never persisted, logged, audited or returned.

use std::time::{Duration, Instant};

use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use openside_proto::{
    EventKind, OtpIngestRequest, OtpIngestResponse, Ticket, TicketKind, TicketStatus,
};
use serde_json::json;
use zeroize::Zeroizing;

use crate::{api::HttpError, engine::Shared};

struct Code {
    origin: String,
    value: Zeroizing<String>,
    deadline: Instant,
}

/// Unconsumed codes, memory only. The async lock is the reconciliation lock.
#[derive(Default)]
pub(crate) struct Codes(tokio::sync::Mutex<Vec<Code>>);

fn extract(pattern: &str, text: &str) -> Option<Zeroizing<String>> {
    let regex = match regex::Regex::new(pattern) {
        Ok(regex) => regex,
        Err(_) => {
            crate::diag!("opensided: otp.sites pattern does not compile; ignoring it");
            return None;
        }
    };
    let captures = regex.captures(text)?;
    let code = captures.get(1).or_else(|| captures.get(0))?.as_str().trim();
    (!code.is_empty()).then(|| Zeroizing::new(code.to_owned()))
}

fn waiting(ticket: &Ticket) -> Option<&str> {
    (ticket.kind == TicketKind::SecretInput
        && ticket.status == TicketStatus::Pending
        && ticket.expires_at > Utc::now())
    .then_some(())?;
    ticket.target.as_ref()?.origin.as_deref()
}

/// Fill pending `secret_input` tickets from stored codes, newest ticket per site first.
/// Callers hold the codes lock, which serializes every store and every fill.
async fn reconcile(s: &Shared, codes: &mut Vec<Code>) -> Vec<String> {
    let now = s.remote.instant();
    codes.retain(|code| code.deadline > now);
    if codes.is_empty() {
        return Vec::new();
    }
    let Ok(mut pending) = s.tickets().await else {
        return Vec::new();
    };
    pending.sort_by_key(|ticket| std::cmp::Reverse(ticket.created_at));
    let mut filled = Vec::new();
    for ticket in &pending {
        let Some(origin) = waiting(ticket) else {
            continue;
        };
        let Some(index) = codes.iter().position(|code| code.origin == origin) else {
            continue;
        };
        let code = codes.remove(index).value;
        let ok = crate::api::resolve_as_human_step(s, ticket.id.as_str(), code)
            .await
            .is_ok();
        crate::pairing::audit(
            s,
            "otp",
            "otp_consumed",
            json!({"ticket_id": ticket.id, "origin": origin, "filled": ok}),
        )
        .await;
        if ok {
            filled.push(ticket.id.as_str().to_owned());
        }
    }
    filled
}

pub(crate) async fn ingest(
    State(s): State<Shared>,
    Json(request): Json<OtpIngestRequest>,
) -> Result<Json<OtpIngestResponse>, HttpError> {
    let text = Zeroizing::new(request.text);
    let config = &s.config.loaded.config.otp;
    if !config
        .senders
        .iter()
        .any(|sender| sender == request.sender.trim())
    {
        crate::pairing::audit(
            &s,
            "otp",
            "otp_rejected",
            json!({"reason": "sender_not_allowed"}),
        )
        .await;
        return Err(HttpError(StatusCode::FORBIDDEN, "sender_not_allowed"));
    }
    let matched: Vec<(String, Zeroizing<String>)> = config
        .sites
        .iter()
        .filter_map(|(origin, pattern)| extract(pattern, &text).map(|code| (origin.clone(), code)))
        .collect();
    let matched_sites: Vec<String> = matched.iter().map(|(origin, _)| origin.clone()).collect();
    if matched.len() > 1 {
        crate::pairing::audit(
            &s,
            "otp",
            "otp_rejected",
            json!({"reason": "ambiguous", "matched_sites": matched_sites}),
        )
        .await;
        return Err(HttpError(StatusCode::CONFLICT, "ambiguous_otp"));
    }
    let mut codes = s.remote.otp.0.lock().await;
    let deadline = s.remote.instant() + Duration::from_secs(u64::from(config.ttl_secs));
    for (origin, value) in matched {
        // Only the newest code of a site is useful; an older one is superseded.
        codes.retain(|code| code.origin != origin);
        codes.push(Code {
            origin,
            value,
            deadline,
        });
    }
    let resolved_tickets = reconcile(&s, &mut codes).await;
    let stored = codes
        .iter()
        .filter(|code| matched_sites.contains(&code.origin))
        .count() as u32;
    drop(codes);
    let response = OtpIngestResponse {
        matched_sites,
        resolved_tickets,
        stored,
    };
    crate::pairing::audit(
        &s,
        "otp",
        "otp_ingested",
        json!({"matched_sites": response.matched_sites, "resolved_tickets": response.resolved_tickets, "stored": response.stored}),
    )
    .await;
    Ok(Json(response))
}

/// Reconcile when a `secret_input` ticket opens or events were missed, and zeroize codes
/// at their deadline.
pub(crate) fn start(shared: &Shared) {
    let s = shared.clone();
    let mut events = shared.events.subscribe();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut sweep = tokio::time::interval(Duration::from_secs(1));
        loop {
            let reconcile_now = tokio::select! {
                _ = shutdown.recv() => break,
                _ = sweep.tick() => {
                    let now = s.remote.instant();
                    s.remote.otp.0.lock().await.retain(|code| code.deadline > now);
                    false
                }
                event = events.recv() => match event {
                    Ok(event) => matches!(
                        &event.kind,
                        EventKind::TicketOpened { ticket } if ticket.kind == TicketKind::SecretInput
                    ),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            };
            if reconcile_now {
                let mut codes = s.remote.otp.0.lock().await;
                reconcile(&s, &mut codes).await;
            }
        }
        s.remote.otp.0.lock().await.clear();
    });
}

#[cfg(test)]
impl Codes {
    /// Hold the reconciliation lock, as an ingest between its store and its fill does.
    pub(crate) async fn hold(&self) -> tokio::sync::MutexGuard<'_, impl Sized> {
        self.0.lock().await
    }
}
