//! One-time codes. `request_otp` is the one tool for "this page wants a verification code": a
//! TOTP saved in the vault for the tab's origin is generated and filled at once; otherwise a
//! `secret_input` ticket asks the person for the code, or lets them register the site's
//! authenticator so later calls need nobody. `register_authenticator` reads the setup key a
//! 2FA enrollment page shows (otpauth link, QR code or text key) and keeps it once the person
//! approves. Codes and setup keys never reach the model.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use openside_proto::{
    OtpChannel, SessionId, Ticket, TicketKind, TicketOtp, TicketOtpRegister, TicketTarget,
};
use openside_vault::{
    AuditAction, AuditContext, Denial, Error as VaultError, LoginOutcome, Redactor, Secret, Totp,
};
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    browser::Controller,
    parse, spec,
    vault::{fill_saved_totp, DaemonVault, SavedTotp},
    DaemonTool, SecretSink, SecretSinkError, TicketRequest, ToolEnv, ToolError, ToolExec,
};
use crate::engine::Shared;

type FilledLogins = HashMap<(SessionId, String), (String, Instant)>;

static FILLED_LOGINS: LazyLock<parking_lot::Mutex<FilledLogins>> = LazyLock::new(Default::default);

pub(crate) async fn filled(_shared: &Shared, ticket: &Ticket) {
    if ticket.otp.is_none() {
        return;
    }
    if let Some(target) = &ticket.target {
        if let (Some(tab), Some(origin)) = (&target.tab_id, &target.origin) {
            mark_filled(&ticket.session_id, tab, origin);
        }
    }
}

pub(super) fn mark_filled(session: &SessionId, tab: &str, origin: &str) {
    let mut pending = FILLED_LOGINS.lock();
    pending.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(600));
    pending.insert(
        (session.clone(), tab.to_owned()),
        (origin.to_owned(), Instant::now()),
    );
}

pub(crate) async fn observed(
    shared: &Shared,
    browser: &openside_browser::Browser,
    session: &SessionId,
    tab: &str,
    url: &str,
    kind: openside_proto::PageKind,
) {
    if matches!(
        kind,
        openside_proto::PageKind::Login | openside_proto::PageKind::Otp
    ) {
        return;
    }
    let origin = url::Url::parse(url)
        .ok()
        .map(|url| url.origin().ascii_serialization());
    let ready = {
        let mut pending = FILLED_LOGINS.lock();
        pending.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(600));
        let key = (session.clone(), tab.to_owned());
        if pending
            .get(&key)
            .is_some_and(|(expected, _)| Some(expected) == origin.as_ref())
        {
            pending.remove(&key);
            true
        } else {
            false
        }
    };
    if ready {
        let _ = crate::connections::complete_login(
            shared,
            browser,
            session,
            tab,
            openside_proto::ConnectionSource::Otp,
            None,
        )
        .await;
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OtpRequestArgs {
    tab_id: String,
    field_ref: String,
    /// Page-stated source; unknown if unspecified.
    channel: Option<OtpChannel>,
    /// Note to person; never a code.
    summary: Option<String>,
}

fn channel_text(channel: OtpChannel) -> &'static str {
    match channel {
        OtpChannel::Authenticator => {
            "Enter the code from your authenticator app, or register the authenticator so openside can fill codes for this site from now on."
        }
        OtpChannel::Sms => "Enter the code the site sent you by text message.",
        OtpChannel::Email => "Enter the code the site sent you by email.",
        OtpChannel::BackupCode => "Enter one of your backup codes for this site.",
        OtpChannel::Unknown => {
            "Enter the verification code this site asks for. If it comes from an authenticator app, you can register the authenticator instead."
        }
    }
}

/// Registering is offered only where a code can come from an authenticator, and only when a
/// vault exists to keep it.
fn can_register(channel: OtpChannel, vault_available: bool) -> bool {
    vault_available && matches!(channel, OtpChannel::Authenticator | OtpChannel::Unknown)
}

/// The tab's origin as approved for this call: the origin is the tab's, never the model's.
pub(super) fn approved_origin(ctx: &ToolEnv) -> std::result::Result<String, ToolError> {
    ctx.approved_actions
        .iter()
        .find(|action| action.capability == Capability::VaultUse)
        .map(|action| action.target.clone())
        .ok_or_else(|| ToolError("Missing approved tool target".into()))
}

/// Browse + VaultUse on the tab's live origin: a code or key is released only into the page
/// the person sees named on the ticket.
pub(super) async fn origin_actions(
    ctx: &ToolEnv,
    tab_id: &str,
) -> std::result::Result<Vec<Action>, ToolError> {
    let browser = ctx
        .shared
        .registry
        .browser
        .for_tab(&ctx.shared, &ctx.session, tab_id)
        .await?;
    let origin = Controller::origin(&browser, &ctx.session, tab_id).await?;
    Ok(vec![
        Action::new(Capability::Browse, &origin),
        Action::new(Capability::VaultUse, origin),
    ])
}

pub(crate) struct RequestOtp;
impl DaemonTool for RequestOtp {
    fn name(&self) -> &'static str {
        "request_otp"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<OtpRequestArgs>(
            self.name(),
            "For verification codes (TOTP/SMS/email/backup); use the first box for split codes. field_ref must come from a fresh browser_snapshot of this code page, never guessed or reused across navigation. The daemon fills from the origin's saved authenticator or asks the person privately to enter a code/register it; no code reaches you or brings the tab onscreen. status=filled means ready; status=registered with filled=false requires request_otp again. Await completion before clicking verify/continue; never batch that click with a pending request. Inspect the next page: account/profile/trust/terms need request_choice, not a success claim. Never ask codes in chat/ask_user; takeover only if requested.",
            ApprovalTier::Read,
            true,
        )
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: OtpRequestArgs = parse(args)?;
            if args.tab_id.trim().is_empty() || args.field_ref.trim().is_empty() {
                return Err(ToolError("tab_id and field_ref must not be empty".into()));
            }
            origin_actions(ctx, &args.tab_id).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"field_ref":args["field_ref"],"channel":args["channel"],"summary":args["summary"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Self::call_with_vault(args, ctx, true)
    }
}

impl RequestOtp {
    pub(super) fn call_with_vault(
        args: Value,
        ctx: &ToolEnv,
        allow_vault: bool,
    ) -> BoxFuture<'_, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: OtpRequestArgs = parse(&args)?;
                let origin = approved_origin(ctx)?;
                let browser = ctx
                    .shared
                    .registry
                    .browser
                    .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
                    .await?;
                if Controller::origin(&browser, &ctx.session, &args.tab_id).await? != origin {
                    return Err(ToolError(
                        "Page origin changed; take a fresh snapshot".into(),
                    ));
                }
                let _ = crate::connections::begin_login(
                    &ctx.shared,
                    &browser,
                    &ctx.session,
                    &args.tab_id,
                    &origin,
                )
                .await;
                let channel = args.channel.unwrap_or(OtpChannel::Unknown);
                let vault = ctx.shared.registry.vault.available();
                // Several saved accounts for one origin: the person types the code of the
                // account they mean, and registering another one is not offered.
                let mut ambiguous = false;
                if allow_vault
                    && vault
                    && matches!(channel, OtpChannel::Authenticator | OtpChannel::Unknown)
                {
                    match fill_saved_totp(ctx, &args.tab_id, &args.field_ref, None).await? {
                        SavedTotp::Used(exec) => return Ok(*exec),
                        SavedTotp::Ambiguous => ambiguous = true,
                        SavedTotp::Missing => {}
                    }
                }
                let summary = args
                    .summary
                    .filter(|summary| !summary.trim().is_empty())
                    .unwrap_or_else(|| format!("Verification code for {origin}"));
                Ok(ToolExec::Defer(Box::new(TicketRequest {
                    hire: None,
                    form: None,
                    kind: TicketKind::SecretInput,
                    summary,
                    instructions: Some(channel_text(channel).into()),
                    options: Vec::new(),
                    target: Some(TicketTarget {
                        tab_id: Some(args.tab_id),
                        origin: Some(origin),
                        field_ref: Some(args.field_ref),
                    }),
                    resume_when: None,
                    otp: Some(TicketOtp {
                        channel,
                        can_register: !ambiguous && can_register(channel, vault),
                    }),
                    otp_register: None,
                    login: None,
                    choice: None,
                    approval: None,
                    pins: Vec::new(),
                })))
            }
            .await;
            result.unwrap_or_else(|error: ToolError| ToolExec::Denied {
                reason: error.0,
                hint: None,
            })
        })
    }
}

/// The vault audit label of the person's device that approved a registration: `device:<id>`
/// for a paired device, `local` for the local socket. Ids outside the audit label alphabet are
/// recorded as `device` rather than rewritten.
pub(crate) fn approver(device: Option<&str>) -> String {
    match device {
        None => "local".into(),
        Some(id)
            if !id.is_empty()
                && id.len() <= 57
                && id
                    .bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')) =>
        {
            format!("device:{id}")
        }
        Some(_) => "device".into(),
    }
}

/// Keep `totp` in the vault for exactly `origin` (see `Vault::register_totp`), tagged with the
/// `operation` id the caller recorded in its fill intent. The seed is already a known secret
/// of every output path before the vault sees it. The error `audit_incomplete` means the
/// factor *was* saved but its completion record failed.
fn save_totp(
    shared: &Shared,
    vault: &mut DaemonVault,
    audit: &AuditContext<'_>,
    origin: &str,
    totp: Totp,
    operation: &str,
) -> std::result::Result<String, SecretSinkError> {
    shared.registry.redactors.write().push(totp.redactor());
    let refused = |code| SecretSinkError { code };
    match vault.register_totp(audit, origin, totp, operation) {
        Ok(Ok(site)) => Ok(site),
        Ok(Err(Denial::AlreadyRegistered)) => Err(refused("otp_already_registered")),
        Ok(Err(_)) => Err(refused("vault_denied")),
        Err(VaultError::AuditIncomplete) => {
            // The factor was saved; which site it went to is not reported, so no code is
            // generated from it here. The next `request_otp` finds it.
            Err(refused("audit_incomplete"))
        }
        Err(_) => Err(refused("vault_unavailable")),
    }
}

/// Generate the current code of `site` inside the vault callback and write it through `sink`
/// into the ticket's pinned field. The caller holds the browser gate, then the vault, in that
/// order (see `Controller::gate`). Error codes follow [`SecretSink::fill`].
async fn fill_registered(
    vault: &mut DaemonVault,
    audit: &AuditContext<'_>,
    site: &str,
    origin: &str,
    ticket: &Ticket,
    target: &TicketTarget,
    sink: Arc<dyn SecretSink>,
) -> std::result::Result<(), &'static str> {
    // fill_totp's callback must own what it captures.
    let failure = Arc::new(parking_lot::Mutex::new(None::<&'static str>));
    let failure_slot = failure.clone();
    let (ticket_id, session_id, target) =
        (ticket.id.clone(), ticket.session_id.clone(), target.clone());
    let unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "fill_failed")?
        .as_secs();
    let outcome = vault
        .vault_login_async(audit, AuditAction::Otp, site, origin, |secrets| {
            Box::pin(async move {
                secrets
                    .fill_totp(unix_seconds, move |code| {
                        Box::pin(async move {
                            sink.fill_gate_held(
                                &ticket_id,
                                &session_id,
                                &target,
                                Zeroizing::new(code.to_owned()),
                            )
                            .await
                            .map_err(|error| *failure_slot.lock() = Some(error.code))
                        })
                    })
                    .await
                    .map_err(|_| ())
            })
        })
        .await;
    let failed = failure.lock().take();
    match (outcome, failed) {
        (Ok(LoginOutcome::Filled), _) => Ok(()),
        (Ok(LoginOutcome::Denied(_)), _) => Err("vault_denied"),
        (_, Some(code)) => Err(code),
        _ => Err("fill_failed"),
    }
}

/// What registering an authenticator from a ticket did, once the vault accepted it.
pub(crate) enum Registered {
    /// The current code is in the ticket's field.
    Filled,
    /// The authenticator is saved but no code was written (no field, the page changed, or the
    /// completion audit failed); a later `request_otp` fills from the vault.
    Unfilled,
}

/// Resolve a `request_otp` ticket with an authenticator: keep it in the vault for `origin`,
/// then fill the current code into the ticket's pinned field. The caller has done everything
/// a plain code needs first (policy recheck, document taint, fill intent naming `operation`).
/// The pinned field is checked again under the browser gate before the save, because a daemon
/// navigation or a takeover may have overtaken the caller's checks while it waited for the
/// gate. An error means the vault kept nothing; once the authenticator is saved, a failed fill
/// is [`Registered::Unfilled`] so the ticket never stays open over a saved factor.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn register_and_fill(
    shared: &Shared,
    ticket: &Ticket,
    target: &TicketTarget,
    origin: &str,
    totp: Totp,
    operation: &str,
    sink: Arc<dyn SecretSink>,
    who: &str,
) -> std::result::Result<Registered, SecretSinkError> {
    let controller = &shared.registry.browser;
    let _gate = controller.gate.lock().await;
    controller
        .ticket_target_current(shared, &ticket.id, &ticket.session_id, target)
        .await?;
    let mut guard = shared.registry.vault.vault.lock().await;
    let vault = guard.as_mut().ok_or(SecretSinkError {
        code: "vault_unavailable",
    })?;
    let audit = AuditContext {
        who,
        session_id: Some(ticket.session_id.as_str()),
        tool_call_id: Some(&ticket.tool_call_id),
    };
    let site = match save_totp(shared, vault, &audit, origin, totp, operation) {
        Ok(site) => site,
        Err(error) if error.code == "audit_incomplete" => return Ok(Registered::Unfilled),
        Err(error) => return Err(error),
    };
    match fill_registered(vault, &audit, &site, origin, ticket, target, sink).await {
        Ok(()) => Ok(Registered::Filled),
        Err(_) => Ok(Registered::Unfilled),
    }
}

/// A setup key read by `register_authenticator`, waiting for the person's decision. Held only
/// in daemon memory: a restart loses it and the ticket then resolves as `setup_key_lost`.
struct PendingSetup {
    totp: Totp,
    tab: String,
    /// `Browser::document_lifetime_id` of the enrollment page the key was read from.
    document: String,
    read_at: Instant,
}

/// Keyed by session and tool call, which name the ticket before it has an id.
static PENDING: LazyLock<parking_lot::Mutex<HashMap<(SessionId, String), PendingSetup>>> =
    LazyLock::new(Default::default);

/// Longer than any confirm ticket lives; keys whose ticket never opened are dropped after it.
const PENDING_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

fn pending_key(ticket: &Ticket) -> (SessionId, String) {
    (ticket.session_id.clone(), ticket.tool_call_id.clone())
}

/// Drop the setup key of a `register_authenticator` ticket that ends without approval
/// (declined, rejected, expired, session stopped). A no-op for every other ticket.
pub(crate) fn discard(ticket: &Ticket) {
    if ticket.otp_register.is_some() {
        PENDING.lock().remove(&pending_key(ticket));
    }
}

/// The truthful result of a ticket that can register an authenticator (`register_authenticator`,
/// or `request_otp` offering registration) whose earlier attempt recorded an intent and never
/// committed (a failed ticket commit or a crash). The intent names the operation id the save
/// carried into the vault, so only that exact save counts, never another factor at the origin:
/// `{"status":"registered","filled":false}` when the vault holds it, and
/// `{"status":"registration_indeterminate"}` when that cannot be checked (vault locked or
/// unavailable, intent unreadable). `None` when there is nothing to recover: no intent, an
/// intent of a plain code fill, or a registration the vault shows never happened (that intent
/// is cleared, since nothing reached the vault or the page).
///
/// Every terminal path consults this — an answer, a rejection, expiry, a session stop and a
/// subagent cancellation — so none can report `declined`, `rejected` or `expired` over a saved
/// factor. Callers hold `ticket_gate`, so no attempt is still in flight.
/// A `login` ticket, whose answer may keep a sign-in, is answered by
/// [`super::login::interrupted`].
pub(crate) async fn interrupted_registration(shared: &Shared, ticket: &Ticket) -> Option<Value> {
    if ticket.login.is_some() {
        return super::login::interrupted(shared, ticket).await;
    }
    let registers =
        ticket.otp_register.is_some() || ticket.otp.as_ref().is_some_and(|otp| otp.can_register);
    if !registers {
        return None;
    }
    let indeterminate = || Some(json!({"status":"registration_indeterminate"}));
    let controller = &shared.registry.browser;
    let operation = match controller.fill_intent(shared, &ticket.id).await {
        Ok(Some(intent)) => intent.registration?,
        Ok(None) => return None,
        Err(_) => return indeterminate(),
    };
    let saved = {
        let guard = shared.registry.vault.vault.lock().await;
        guard.as_ref().map(|vault| vault.registered_by(&operation))
    };
    match saved {
        Some(Ok(true)) => {
            discard(ticket);
            Some(json!({"status":"registered","filled":false}))
        }
        Some(Ok(false)) => {
            // Nothing was saved, and a code is filled only after the save.
            if controller
                .clear_fill_intent(shared, &ticket.id)
                .await
                .is_err()
            {
                return indeterminate();
            }
            None
        }
        _ => indeterminate(),
    }
}

/// Whether the tab still shows, to this session and without a person in control, the exact
/// enrollment document a setup key was read from, at the ticket's origin.
async fn enrollment_unchanged(
    shared: &Shared,
    ticket: &Ticket,
    target: &TicketTarget,
    origin: &str,
    pending: &PendingSetup,
) -> std::result::Result<(), &'static str> {
    if target.tab_id.as_deref() != Some(pending.tab.as_str()) {
        return Err("target_unavailable");
    }
    let tab = json!({"tab_id":pending.tab});
    if crate::takeover::blocked(shared, &ticket.session_id, "register_authenticator", &tab)
        .await
        .is_some()
    {
        return Err("takeover_active");
    }
    let browser = shared
        .registry
        .browser
        .target_browser(shared, &ticket.session_id, &pending.tab)
        .await
        .map_err(|error| error.code)?;
    let tab_origin = Controller::origin(&browser, &ticket.session_id, &pending.tab)
        .await
        .map_err(|_| "target_unavailable")?;
    if tab_origin != origin {
        return Err("origin_mismatch");
    }
    let document = browser
        .document_lifetime_id(&pending.tab)
        .await
        .map_err(|_| "target_unavailable")?;
    if document != pending.document {
        return Err("target_unavailable");
    }
    Ok(())
}

/// Approve a `register_authenticator` ticket: keep the setup key for exactly the ticket's
/// origin and fill the site's confirmation code into the ticket's field, if it has one.
/// Returns the resolution; errors (codes as [`SecretSinkError`]) leave the ticket pending.
/// A fill intent recorded before the save stays until the ticket commits, so an approval
/// whose commit fails is answered by [`interrupted_registration`] on the next attempt.
pub(crate) async fn approve_registration(
    shared: &Shared,
    ticket: &Ticket,
    who: &str,
    sink: Option<Arc<dyn SecretSink>>,
) -> std::result::Result<Value, SecretSinkError> {
    let refused = |code| SecretSinkError { code };
    let register = ticket
        .otp_register
        .as_ref()
        .ok_or(refused("secret_input_failed"))?;
    let target = ticket
        .target
        .as_ref()
        .ok_or(refused("secret_input_failed"))?;
    let origin = register.origin.as_str();
    if target.origin.as_deref() != Some(origin) {
        return Err(refused("origin_mismatch"));
    }
    let key = pending_key(ticket);
    let Some(pending) = PENDING.lock().remove(&key) else {
        return Ok(json!({"status":"setup_key_lost"}));
    };
    let controller = &shared.registry.browser;
    // The gate, taken before the vault, keeps daemon browser operations from moving the tab
    // between the page check and the save.
    let _gate = controller.gate.lock().await;
    let keep = |pending: PendingSetup,
                code: &'static str|
     -> std::result::Result<Value, SecretSinkError> {
        PENDING.lock().insert(pending_key(ticket), pending);
        Err(refused(code))
    };
    if let Err(code) = enrollment_unchanged(shared, ticket, target, origin, &pending).await {
        return keep(pending, code);
    }
    if !shared.registry.vault.available() {
        return keep(pending, "vault_unavailable");
    }
    let mut guard = shared.registry.vault.vault.lock().await;
    let Some(vault) = guard.as_mut() else {
        drop(guard);
        return keep(pending, "vault_unavailable");
    };
    let operation = crate::persistence::random_id();
    if controller
        .record_fill_intent(shared, &ticket.id, Some(&operation))
        .await
        .is_err()
    {
        drop(guard);
        return keep(pending, "secret_input_failed");
    }
    let audit = AuditContext {
        who,
        session_id: Some(ticket.session_id.as_str()),
        tool_call_id: Some(&ticket.tool_call_id),
    };
    let site = match save_totp(shared, vault, &audit, origin, pending.totp, &operation) {
        Ok(site) => site,
        Err(error) if error.code == "audit_incomplete" => {
            return Ok(json!({"status":"registered","filled":false}));
        }
        Err(error) => {
            // Nothing was saved, so there is no outcome to recover.
            let _ = controller.clear_fill_intent(shared, &ticket.id).await;
            return Err(error);
        }
    };
    let (Some(sink), Some(_)) = (sink, target.field_ref.as_deref()) else {
        return Ok(json!({"status":"registered","filled":false}));
    };
    // Taint before the code exists, as for any secret fill.
    if controller
        .mark_ticket_secret(shared, &ticket.id, &ticket.session_id, target)
        .await
        .is_err()
    {
        return Ok(json!({"status":"registered","filled":false}));
    }
    let filled = fill_registered(vault, &audit, &site, origin, ticket, target, sink)
        .await
        .is_ok();
    let _ = controller.persist(shared).await;
    Ok(json!({"status":"registered","filled":filled}))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisterAuthenticatorArgs {
    /// Tab showing the site's 2FA setup page (QR code, otpauth link or text setup key).
    tab_id: String,
    /// Ref of the page's confirmation-code field, if it asks for one; for a code split over
    /// several boxes, the first box. The current code is filled there once approved.
    field_ref: Option<String>,
}

/// Collects otpauth links from attributes, form values and text, and scrolls the largest
/// square image/canvas/svg (the QR code candidate) into view for a screenshot. Runs in an
/// isolated world; returns `{uris, text, qr}`.
const SETUP_SOURCES: &str = r"(() => {
    const uris = new Set();
    const add = (s) => {
        if (typeof s !== 'string' || s.length > 4096) return;
        for (const m of s.matchAll(/otpauth:\/\/totp\/[^\s<>]+/gi)) uris.add(m[0]);
    };
    for (const el of document.querySelectorAll('*')) {
        for (const a of el.attributes) add(a.value);
        if ('value' in el && typeof el.value === 'string') add(el.value);
    }
    // Rendered setup keys may live only in readonly controls, not innerText.
    const values = [...document.querySelectorAll('input,textarea')].filter((el) => {
        const r = el.getBoundingClientRect(), s = getComputedStyle(el);
        return el.type !== 'password' && el.type !== 'hidden' && r.width > 1 && r.height > 1
            && s.display !== 'none' && s.visibility !== 'hidden' && Number(s.opacity) > 0;
    }).map((el) => String(el.value || '').slice(0, 4096));
    const text = [...values, document.body ? document.body.innerText : ''].join('\n').slice(0, 50000);
    add(text);
    let best = null, area = 0;
    for (const el of document.querySelectorAll('img, canvas, svg')) {
        const r = el.getBoundingClientRect();
        if (r.width < 48 || r.height < 48 || Math.abs(r.width - r.height) > r.width * 0.2) continue;
        if (r.width * r.height > area) { area = r.width * r.height; best = el; }
    }
    if (best) best.scrollIntoView({block: 'center', inline: 'center'});
    return {uris: [...uris].slice(0, 16), text, qr: !!best};
})()";

/// A setup key read from the page, with the non-secret label of its otpauth link.
struct SetupKey {
    totp: Totp,
    issuer: Option<String>,
    account: Option<String>,
}

/// Issuer and account from an otpauth link's label and `issuer` parameter. Only printable
/// text up to 64 characters is kept; it is shown to the person on the ticket.
fn label(uri: &str) -> (Option<String>, Option<String>) {
    let clean = |text: String| {
        let text = text.trim().to_owned();
        (!text.is_empty() && text.chars().count() <= 64 && !text.chars().any(char::is_control))
            .then_some(text)
    };
    let rest = &uri["otpauth://totp/".len().min(uri.len())..];
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let decode = |text: &str| {
        url::form_urlencoded::parse(format!("x={text}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    let path = decode(&path.replace('+', "%2B"));
    let (mut issuer, account) = match path.split_once(':') {
        Some((issuer, account)) => (clean(issuer.to_owned()), clean(account.to_owned())),
        None => (None, clean(path)),
    };
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key == "issuer" {
            issuer = clean(value.into_owned()).or(issuer);
        }
    }
    (issuer, account)
}

/// The factor a setup-key candidate (an otpauth link, or a base32 text key whose spaces are
/// ignored) describes, and whether it was a link.
fn parse_candidate(candidate: &str) -> Option<(Totp, bool)> {
    let link = candidate
        .get(..10)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("otpauth://"));
    let text = if link {
        candidate.to_owned()
    } else {
        candidate.chars().filter(|ch| *ch != ' ').collect()
    };
    Totp::parse(Secret::new(text)).ok().map(|totp| (totp, link))
}

/// One distinct key among `candidates`; `Err` when different keys are shown, `Ok(None)` when
/// none parses.
fn single_key(
    candidates: &[Zeroizing<String>],
) -> std::result::Result<Option<SetupKey>, &'static str> {
    let mut found: Option<SetupKey> = None;
    for candidate in candidates {
        let Some((totp, link)) = parse_candidate(candidate) else {
            continue;
        };
        if let Some(seen) = &found {
            if !seen.totp.same_key(&totp) {
                return Err("setup_key_ambiguous");
            }
            continue;
        }
        let (issuer, account) = if link { label(candidate) } else { (None, None) };
        found = Some(SetupKey {
            totp,
            issuer,
            account,
        });
    }
    Ok(found)
}

/// Base32 setup keys as enrollment pages print them for manual entry: groups of four, or one
/// run of at least 16 characters, in one case throughout.
static TEXT_KEY: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"\b(?:[A-Z2-7]{4}(?: [A-Z2-7]{4}){3,}|[a-z2-7]{4}(?: [a-z2-7]{4}){3,}|[A-Z2-7]{16,}|[a-z2-7]{16,})\b")
        .ok()
});

/// Text candidates that look like a setup key: a digit 2–7 (ordinary words have none) or,
/// without one, an upper-case run long enough not to be a word.
fn text_keys(text: &str) -> Vec<Zeroizing<String>> {
    let Some(pattern) = TEXT_KEY.as_ref() else {
        return Vec::new();
    };
    pattern
        .find_iter(text)
        .map(|found| found.as_str())
        .filter(|candidate| {
            let bare: String = candidate.chars().filter(|ch| *ch != ' ').collect();
            bare.bytes().any(|b| (b'2'..=b'7').contains(&b))
                || (bare.len() >= 26 && bare.bytes().all(|b| b.is_ascii_uppercase()))
        })
        .map(|candidate| Zeroizing::new(candidate.to_owned()))
        .collect()
}

/// otpauth links in the QR codes of a PNG screenshot.
fn qr_links(png: &[u8]) -> Vec<Zeroizing<String>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let Ok(mut reader) = decoder.read_info() else {
        return Vec::new();
    };
    let size = reader.output_buffer_size();
    let mut pixels = Zeroizing::new(vec![0; size]);
    let Ok(frame) = reader.next_frame(&mut pixels) else {
        return Vec::new();
    };
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Vec::new(),
    };
    let (width, height) = (frame.width as usize, frame.height as usize);
    let stride = frame.line_size;
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        let at = y * stride + x * channels;
        let luma = if channels >= 3 {
            (u32::from(pixels[at]) * 299
                + u32::from(pixels[at + 1]) * 587
                + u32::from(pixels[at + 2]) * 114)
                / 1000
        } else {
            u32::from(pixels[at])
        };
        // Transparent pixels render over white.
        let alpha = match channels {
            2 => u32::from(pixels[at + 1]),
            4 => u32::from(pixels[at + 3]),
            _ => 255,
        };
        u8::try_from((luma * alpha + 255 * (255 - alpha)) / 255).unwrap_or(u8::MAX)
    });
    image
        .detect_grids()
        .into_iter()
        .filter_map(|grid| {
            let mut out = Zeroizing::new(Vec::new());
            grid.decode_to(&mut *out).ok()?;
            let text = Zeroizing::new(String::from_utf8(out.to_vec()).ok()?);
            text.get(..15)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("otpauth://totp/"))
                .then_some(text)
        })
        .collect()
}

/// Every displayed form of a key read from the page, so none of them can reach the model.
fn displayed_forms(forms: &[Zeroizing<String>]) -> Redactor {
    let secrets: Vec<Secret> = forms
        .iter()
        .map(|form| Secret::new(form.to_string()))
        .collect();
    Redactor::new(&secrets)
}

/// Protect setup material before snapshots, history or images can expose it. The
/// caller holds the browser gate; this never registers or persists the factor.
pub(crate) async fn protect_setup_page(
    shared: &Shared,
    browser: &openside_browser::Browser,
    owner: &str,
    tab: &str,
) -> Result<(), ToolError> {
    let sources = browser
        .evaluate_isolated(owner, tab, openside_repl::SETUP_SCREENING.source)
        .await
        .map_err(|_| ToolError("Setup page could not be inspected".into()))?;
    let mut forms: Vec<Zeroizing<String>> = sources["uris"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|text| Zeroizing::new(text.to_owned()))
        .collect();
    forms.extend(text_keys(sources["text"].as_str().unwrap_or("")));
    let qr = sources["qr"] == true;
    drop(sources);
    let factors: Vec<Totp> = forms
        .iter()
        .filter_map(|form| parse_candidate(form).map(|(totp, _)| totp))
        .collect();
    {
        let mut redactors = shared.registry.redactors.write();
        if !forms.is_empty() {
            redactors.push(displayed_forms(&forms));
        }
        redactors.extend(factors.iter().map(Totp::redactor));
    }
    // A QR-only setup page is still secret even before its key can be decoded.
    if qr || !factors.is_empty() {
        shared
            .registry
            .browser
            .mark_secret(shared, browser, tab)
            .await
            .map_err(|_| ToolError("Browser document unavailable".into()))?;
    }
    Ok(())
}

pub(crate) struct RegisterAuthenticator;
impl DaemonTool for RegisterAuthenticator {
    fn name(&self) -> &'static str {
        "register_authenticator"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<RegisterAuthenticatorArgs>(
            self.name(),
            "Use on a site's 2FA / authenticator setup page (it shows a QR code, an otpauth link or a text setup key). The daemon reads the setup key itself and asks the person, on their own device, to approve keeping it in the vault for this site; nothing is shown to you and the tab is never brought to the screen. Pass field_ref when the page asks for a confirmation code, and the current code is filled there on approval. Result {\"status\":\"registered\",\"filled\":true|false} (then click the page's confirm button yourself), {\"approved\":false,\"status\":\"declined\"}, or {\"status\":\"setup_key_lost\"} (call again). Later code prompts on this site: request_otp fills them without asking. Never read, copy or screenshot the setup key yourself.",
            ApprovalTier::Read,
            true,
        )
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available() && ctx.shared.registry.vault.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: RegisterAuthenticatorArgs = parse(args)?;
            if args.tab_id.trim().is_empty()
                || args
                    .field_ref
                    .as_deref()
                    .is_some_and(|r| r.trim().is_empty())
            {
                return Err(ToolError("tab_id and field_ref must not be empty".into()));
            }
            origin_actions(ctx, &args.tab_id).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"field_ref":args["field_ref"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: RegisterAuthenticatorArgs = parse(&args)?;
                let origin = approved_origin(ctx)?;
                let controller = &ctx.shared.registry.browser;
                let browser = controller.for_tab(&ctx.shared, &ctx.session, &args.tab_id).await?;
                if Controller::origin(&browser, &ctx.session, &args.tab_id).await? != origin {
                    return Err(ToolError("Tab origin changed".into()));
                }
                {
                    let vault = ctx.shared.registry.vault.vault.lock().await;
                    let vault = vault
                        .as_ref()
                        .ok_or_else(|| ToolError("Vault is unavailable".into()))?;
                    if !matches!(
                        vault.totp_site_for(&origin),
                        Ok(openside_vault::TotpMatch::None)
                    ) {
                        return Ok(ToolExec::Denied {
                            reason: "otp_already_registered".into(),
                            hint: Some("The vault already holds an authenticator for this site; use request_otp for its codes. Replacing it is done by the person in the Vault page.".into()),
                        });
                    }
                }
                let document = browser
                    .document_lifetime_id(&args.tab_id)
                    .await
                    .map_err(|_| ToolError("Browser document unavailable".into()))?;
                let sources = browser
                    .evaluate_isolated(ctx.session.as_str(), &args.tab_id, SETUP_SOURCES)
                    .await
                    .map_err(|_| ToolError("Setup page could not be read".into()))?;
                let strings = |value: &Value| -> Vec<Zeroizing<String>> {
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|text| Zeroizing::new(text.to_owned()))
                        .collect()
                };
                let links = strings(&sources["uris"]);
                let text = Zeroizing::new(sources["text"].as_str().unwrap_or("").to_owned());
                let qr = sources["qr"] == true;
                drop(sources);
                // Sources in order of reliability; the first one that shows a key decides.
                let mut forms = links.clone();
                let mut key = single_key(&links);
                if qr && matches!(key, Ok(None)) {
                    let codes = qr_codes(&browser, &args.tab_id).await;
                    forms.extend(codes.iter().cloned());
                    key = single_key(&codes);
                }
                if matches!(key, Ok(None)) {
                    let texts = text_keys(&text);
                    forms.extend(texts.iter().cloned());
                    key = single_key(&texts);
                }
                // Everything that looked like a key, and every representation of each factor
                // parsed from it, is a secret of every output path before anything else
                // happens, whatever the outcome; a page showing any factor is tainted, so
                // screenshots and scripts are refused on it even when it is refused below.
                let factors: Vec<Totp> = forms
                    .iter()
                    .filter_map(|form| parse_candidate(form).map(|(totp, _)| totp))
                    .collect();
                {
                    let mut redactors = ctx.shared.registry.redactors.write();
                    if !forms.is_empty() {
                        redactors.push(displayed_forms(&forms));
                    }
                    redactors.extend(factors.iter().map(Totp::redactor));
                }
                if !factors.is_empty() {
                    controller
                        .mark_secret(&ctx.shared, &browser, &args.tab_id)
                        .await
                        .map_err(|_| ToolError("Browser document unavailable".into()))?;
                }
                drop(factors);
                let key = match key {
                    Ok(Some(key)) => key,
                    Ok(None) => {
                        return Ok(ToolExec::Denied {
                            reason: "setup_key_not_found".into(),
                            hint: Some("No authenticator setup key (QR code, otpauth link or text key) is visible on this page. Open the page's setup step (or its \"enter key manually\" option) and call again.".into()),
                        })
                    }
                    Err(reason) => {
                        return Ok(ToolExec::Denied {
                            reason: reason.into(),
                            hint: Some("The page shows more than one setup key; the person has to set this up themselves.".into()),
                        })
                    }
                };
                {
                    let mut pending = PENDING.lock();
                    pending.retain(|_, setup| setup.read_at.elapsed() < PENDING_LIMIT);
                    pending.insert(
                        (ctx.session.clone(), ctx.tool_call_id.clone()),
                        PendingSetup {
                            totp: key.totp,
                            tab: args.tab_id.clone(),
                            document,
                            read_at: Instant::now(),
                        },
                    );
                }
                let named = key
                    .issuer
                    .as_deref()
                    .map_or_else(|| origin.clone(), |issuer| format!("{issuer} ({origin})"));
                Ok(ToolExec::Defer(Box::new(TicketRequest { hire: None, form: None, kind: TicketKind::Confirm,
                summary: format!("Save the authenticator for {named} in the vault"),
                instructions: Some("Approve to keep this site's authenticator in openside's vault, so its codes are filled for you from now on. Decline to discard it.".into()),
                options: Vec::new(),
                target: Some(TicketTarget {
                    tab_id: Some(args.tab_id),
                    origin: Some(origin.clone()),
                    field_ref: args.field_ref,
                }),
                resume_when: None,
                otp: None,
                otp_register: Some(TicketOtpRegister {
                    origin,
                    issuer: key.issuer,
                    account: key.account,
                }),
                login: None,
                choice: None,
                approval: None,
                pins: Vec::new(), })))
            }
            .await;
            result.unwrap_or_else(|error: ToolError| ToolExec::Denied {
                reason: error.0,
                hint: None,
            })
        })
    }
}

/// otpauth links in the QR codes of a screenshot of the tab. The screenshot never leaves
/// this function.
async fn qr_codes(browser: &openside_browser::Browser, tab: &str) -> Vec<Zeroizing<String>> {
    let Ok(png) = browser.screenshot(tab).await else {
        return Vec::new();
    };
    let png = Zeroizing::new(png);
    tokio::task::spawn_blocking(move || qr_links(&png))
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A published RFC 6238 example seed, not anyone's secret.
    const SEED: &str = "JBSWY3DPEHPK3PXP";

    #[test]
    fn registration_is_offered_only_for_authenticator_codes_with_a_vault() {
        for channel in [OtpChannel::Authenticator, OtpChannel::Unknown] {
            assert!(can_register(channel, true));
            assert!(!can_register(channel, false));
        }
        for channel in [OtpChannel::Sms, OtpChannel::Email, OtpChannel::BackupCode] {
            assert!(!can_register(channel, true));
        }
    }

    #[test]
    fn arguments_are_strict_and_redaction_keeps_only_known_fields() {
        assert!(serde_json::from_value::<OtpRequestArgs>(
            json!({"tab_id":"t","field_ref":"e1","channel":"sms","summary":"Code"})
        )
        .is_ok());
        for bad in [
            json!({"tab_id":"t"}),
            json!({"tab_id":"t","field_ref":"e1","channel":"pigeon"}),
            json!({"tab_id":"t","field_ref":"e1","code":"123456"}),
            json!({"tab_id":"t","field_ref":"e1","origin":"https://evil.test"}),
        ] {
            assert!(serde_json::from_value::<OtpRequestArgs>(bad).is_err());
        }
        assert_eq!(
            RequestOtp.redact_args(&json!({"tab_id":"t","field_ref":"e1","code":"123456"})),
            json!({"tab_id":"t","field_ref":"e1","channel":null,"summary":null})
        );
        let spec = RequestOtp.spec();
        assert!(spec.deferrable);
        assert_eq!(spec.approval, ApprovalTier::Read);
        let schema: Value = serde_json::from_str(&spec.parameters_json).unwrap();
        let mut required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["field_ref", "tab_id"]);
    }

    #[test]
    fn register_authenticator_takes_no_key_or_origin_from_the_model() {
        assert!(serde_json::from_value::<RegisterAuthenticatorArgs>(json!({"tab_id":"t"})).is_ok());
        for bad in [
            json!({"tab_id":"t","secret":SEED}),
            json!({"tab_id":"t","origin":"https://evil.test"}),
            json!({"tab_id":"t","uri":"otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"}),
        ] {
            assert!(serde_json::from_value::<RegisterAuthenticatorArgs>(bad).is_err());
        }
        assert_eq!(
            RegisterAuthenticator.redact_args(&json!({"tab_id":"t","secret":SEED})),
            json!({"tab_id":"t","field_ref":null})
        );
        let spec = RegisterAuthenticator.spec();
        assert!(spec.deferrable);
        assert!(!spec.parameters_json.contains("secret"));
    }

    #[test]
    fn approvals_are_audited_with_the_approving_device() {
        assert_eq!(approver(None), "local");
        assert_eq!(approver(Some("phone-1")), "device:phone-1");
        assert_eq!(approver(Some("Phone 1")), "device");
        assert_eq!(approver(Some(&"a".repeat(80))), "device");
    }

    #[test]
    fn setup_keys_are_read_from_links_and_text_but_never_guessed() {
        let link = Zeroizing::new(format!(
            "otpauth://totp/Example%20Co:alice%40example.com?secret={SEED}&issuer=Example%20Co"
        ));
        let key = single_key(&[link.clone(), link]).unwrap().unwrap();
        assert_eq!(key.issuer.as_deref(), Some("Example Co"));
        assert_eq!(key.account.as_deref(), Some("alice@example.com"));

        let grouped = "Can't scan? Enter this key: JBSW Y3DP EHPK 3PXP and then the code.";
        let texts = text_keys(grouped);
        assert_eq!(texts.len(), 1);
        let key = single_key(&texts).unwrap().unwrap();
        assert!(key.issuer.is_none());
        let lower = text_keys("key: jbsw y3dp ehpk 3pxp");
        assert!(single_key(&lower)
            .unwrap()
            .unwrap()
            .totp
            .same_key(&key.totp));

        // Ordinary words, mixed case and short groups are not keys.
        for text in [
            "AUTHENTICATION REQUIRED FOR THIS",
            "Jbsw Y3dp Ehpk 3pxp",
            "ABCD EFGH",
            "Order 2024 3456 7777",
        ] {
            assert!(single_key(&text_keys(text)).unwrap().is_none(), "{text}");
        }
        let two = text_keys("first JBSW Y3DP EHPK 3PXP second KRSX G5CT MVRX EZLU");
        assert_eq!(single_key(&two).err(), Some("setup_key_ambiguous"));
    }

    #[test]
    fn qr_codes_on_a_screenshot_decode_to_their_otpauth_link() {
        let link = format!("otpauth://totp/Example:alice?secret={SEED}&issuer=Example");
        let code = qrcode::QrCode::new(link.as_bytes()).unwrap();
        let modules = code.width();
        let (scale, quiet) = (4, 4);
        let side = (modules + 2 * quiet) * scale;
        let colors = code.to_colors();
        let mut pixels = vec![255u8; side * side];
        for y in 0..modules {
            for x in 0..modules {
                if colors[y * modules + x] == qrcode::Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let (px, py) = ((x + quiet) * scale + dx, (y + quiet) * scale + dy);
                            pixels[py * side + px] = 0;
                        }
                    }
                }
            }
        }
        let mut png = Vec::new();
        {
            let side = u32::try_from(side).unwrap();
            let mut encoder = png::Encoder::new(&mut png, side, side);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
        }
        let links = qr_links(&png);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), link);
        assert!(qr_links(b"not a png").is_empty());
    }

    #[test]
    fn setup_keys_wait_only_for_their_own_ticket() {
        let session = SessionId::new("s-pending".to_owned());
        PENDING.lock().insert(
            (session.clone(), "call-1".into()),
            PendingSetup {
                totp: Totp::parse(Secret::new(SEED.to_owned())).unwrap(),
                tab: "t".into(),
                document: "d".into(),
                read_at: Instant::now(),
            },
        );
        let mut ticket: Ticket = serde_json::from_value(json!({
            "id":"tk","session_id":"s-pending","tool_call_id":"call-1","deferral":"external",
            "kind":"confirm","summary":"x","instructions":null,"options":[],
            "origin":{"subagent_id":null},"tool":null,"target":null,"resume_when":null,
            "created_at":"2026-01-01T00:00:00Z","expires_at":"2026-01-01T00:10:00Z",
            "status":"pending","resolution":null
        }))
        .unwrap();
        // A ticket that is not a registration never drops a key.
        discard(&ticket);
        assert!(PENDING
            .lock()
            .contains_key(&(session.clone(), "call-1".into())));
        ticket.otp_register = Some(TicketOtpRegister {
            origin: "https://example.com".into(),
            issuer: None,
            account: None,
        });
        discard(&ticket);
        assert!(!PENDING.lock().contains_key(&(session, "call-1".into())));
    }
}
