//! Sign-in and the person's decisions on a page. `request_login` rebuilds the tab's login
//! form as a `login` ticket (fields, the page's other sign-in options, saved accounts) or,
//! when exactly one usable login is saved for the origin, signs in from the vault without
//! asking. `request_choice` turns a choice the page asks for (which account, which
//! verification method, trust this device) into a `choice` ticket. Every node is found and
//! pinned by the daemon in an isolated world when the ticket opens; an answer writes or clicks
//! only those nodes, only in that document, and never brings the tab to the front. After a
//! submit or a click the page is classified into the next step the agent should take.
//! Values typed into a login ticket are written by the daemon, never handed to the model: their
//! redactors are installed first and every result, event and ticket is masked. Page scripts the
//! agent runs with `browser_repl` can still read the filled page, so the tab is tainted for
//! screenshots like any other secret fill.

use std::{
    collections::HashMap,
    sync::LazyLock,
    time::{Duration, Instant},
};

use axum::http::StatusCode;
use futures_util::future::BoxFuture;
use openside_browser::{Browser, BrowserError, NodeHandle};
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use openside_proto::{
    LoginAlternativeKind, LoginFieldKind, LoginProvider, SessionId, Ticket, TicketApproval,
    TicketChoice, TicketChoiceOption, TicketKind, TicketLogin, TicketLoginAccount,
    TicketLoginAlternative, TicketLoginField, TicketTarget,
};
use openside_vault::{AccessPolicy, AuditAction, AuditContext, LoginOutcome, Redactor, Secret};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::{
    browser::{browser_error, done, Controller},
    otp::{approved_origin, origin_actions},
    parse, spec, DaemonTool, SecretSinkError, TicketRequest, ToolEnv, ToolError, ToolExec,
};
use crate::{api::HttpError, engine::Shared};

/// How long the page may take after a submit or click before the next step is reported.
const NEXT_WAIT: Duration = Duration::from_secs(10);
const NEXT_POLL: Duration = Duration::from_millis(300);
/// A sign-in form that stays this long without an error message is reported as `unknown`.
const LOGIN_SETTLE: Duration = Duration::from_secs(3);
const MAX_OPTIONS: usize = 12;

/// The account name a person typed on the first page of a two-step sign-in, held in memory
/// only, so the password page's ticket can still keep the login in the vault. Keyed by session
/// and origin; dropped after [`ACCOUNT_STEP_LIMIT`] or when the login is saved.
type AccountSteps = HashMap<(SessionId, String), (Zeroizing<String>, Instant)>;
static ACCOUNT_STEP: LazyLock<parking_lot::Mutex<AccountSteps>> = LazyLock::new(Default::default);
const ACCOUNT_STEP_LIMIT: Duration = Duration::from_secs(10 * 60);

fn account_step(session: &SessionId, origin: &str) -> Option<Zeroizing<String>> {
    let mut steps = ACCOUNT_STEP.lock();
    steps.retain(|_, (_, at)| at.elapsed() < ACCOUNT_STEP_LIMIT);
    steps
        .get(&(session.clone(), origin.to_owned()))
        .map(|(name, _)| name.clone())
}

/// Shared page helpers for the isolated-world scripts. Labels are page text: trimmed, capped,
/// and never read from an input's value (which may hold what someone typed).
const COMMON_JS: &str = r#"
const clean = (s, n) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, n);
const visible = (el) => {
    if (!el || !el.isConnected) return false;
    const s = getComputedStyle(el);
    if (s.visibility === 'hidden' || s.display === 'none' || Number(s.opacity) === 0) return false;
    const r = el.getBoundingClientRect();
    return r.width > 1 && r.height > 1;
};
const usable = (el) => visible(el) && !el.disabled && !el.readOnly && el.getAttribute('aria-disabled') !== 'true';
const labelledby = (el) => (el.getAttribute('aria-labelledby') || '').split(/\s+/)
    .map((id) => { const t = id && document.getElementById(id); return t ? t.innerText : ''; }).join(' ');
const fieldLabel = (el) => clean((el.labels && el.labels[0] && el.labels[0].innerText)
    || el.getAttribute('aria-label') || labelledby(el) || el.getAttribute('placeholder')
    || el.getAttribute('title') || '', 80);
const lines = (el) => {
    const own = el.tagName === 'INPUT' ? ((el.labels && el.labels[0] && el.labels[0].innerText)
        || (['button', 'submit', 'reset'].includes(el.type) ? el.value : '')) : el.innerText;
    const text = String(el.getAttribute('aria-label') || labelledby(el) || own || el.getAttribute('title')
        || (el.querySelector && el.querySelector('img[alt]') ? el.querySelector('img[alt]').alt : '') || '');
    return text.split('\n').map((s) => s.replace(/\s+/g, ' ').trim()).filter(Boolean);
};
const nameOf = (el) => clean(lines(el)[0] || '', 80);
const LOGINISH = /log ?in|sign ?in|continue|next|submit|로그인|다음|계속|확인/i;
const PROVIDERS = [['google', /google|구글/i], ['apple', /apple|애플/i], ['kakao', /kakao|카카오/i],
    ['naver', /naver|네이버/i], ['facebook', /facebook|페이스북/i], ['github', /github|깃허브/i],
    ['microsoft', /microsoft|마이크로소프트|outlook/i]];
const passwordField = () => {
    const all = [...document.querySelectorAll('input[type=password]')].filter(usable);
    return all.find((el) => (el.autocomplete || '').includes('current-password'))
        || all.find((el) => !(el.autocomplete || '').includes('new-password')) || null;
};
const ID_HINT = /user|login|e-?mail|phone|mobile|account|(^|[^a-z])id([^a-z]|$)|아이디|전화|휴대|이메일|계정/i;
const textLike = (el) => el.tagName === 'INPUT' && ['text', 'email', 'tel'].includes(el.type) && usable(el);
const idScore = (el) => {
    const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
    if (/username|email|tel/.test(ac)) return 3;
    if (el.type === 'email' || el.type === 'tel') return 2;
    const hints = [el.name, el.id, el.getAttribute('placeholder'), el.getAttribute('aria-label'), fieldLabel(el)].join(' ');
    return ID_HINT.test(hints) ? 2 : 1;
};
const OTP_HINT = /otp|one.?time|verification|verify|security code|(^|[^a-z])code([^a-z]|$)|인증|보안 ?코드|확인 ?번호/i;
const otpField = (el) => {
    if (el.tagName !== 'INPUT' || !['text', 'tel', 'number'].includes(el.type) || !usable(el)) return false;
    if ((el.getAttribute('autocomplete') || '').includes('one-time-code')) return true;
    const hints = [el.name, el.id, el.getAttribute('placeholder'), el.getAttribute('aria-label'), fieldLabel(el)].join(' ');
    return OTP_HINT.test(hints) && !ID_HINT.test(hints);
};
const headingBefore = (scope, el) => {
    const set = el.closest('fieldset');
    const legend = set && set.querySelector('legend');
    if (legend && clean(legend.innerText, 160)) return clean(legend.innerText, 160);
    const dialog = el.closest('dialog,[role=dialog],[role=alertdialog]');
    if (dialog) {
        const named = labelledby(dialog) || dialog.getAttribute('aria-label');
        if (clean(named, 160)) return clean(named, 160);
    }
    let best = '';
    for (const h of (dialog || scope).querySelectorAll('h1,h2,h3,h4,[role=heading],legend')) {
        if (!visible(h) || !(h.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING)) continue;
        const text = clean(h.innerText, 160);
        if (text) best = text;
    }
    return best;
};
const CHOOSY = /choose|select|pick|which|account|method|verify|trust|device|allow|access|permission|consent|terms|agree|wants to|선택|계정|방법|인증|신뢰|기기|허용|권한|동의|약관/i;
const findChoice = () => {
    const dialogs = [...document.querySelectorAll('dialog[open],[role=dialog],[role=alertdialog],[aria-modal=true]')].filter(visible);
    const scope = dialogs.length ? dialogs[dialogs.length - 1] : (document.querySelector('main') || document.body);
    if (!scope) return null;
    const option = (el, click) => {
        const text = lines(el);
        return {label: clean(text[0] || '', 80), description: clean(text.slice(1).join(' '), 120), el: click || el};
    };
    const done = (items, first) => {
        const options = items.filter((o) => o.label).slice(0, 12);
        return options.length >= 2 ? {question: headingBefore(scope, first), options} : null;
    };
    const radios = new Map();
    for (const input of scope.querySelectorAll('input[type=radio]')) {
        const label = input.labels && input.labels[0];
        if (input.disabled || !(visible(input) || (label && visible(label)))) continue;
        const key = input.name || '';
        if (!radios.has(key)) radios.set(key, []);
        radios.get(key).push(option(label || input, visible(input) ? input : label));
    }
    for (const [, items] of radios) {
        const found = done(items, items[0].el);
        if (found) return found;
    }
    for (const [group, member] of [['[role=radiogroup]', '[role=radio]'], ['[role=listbox]', '[role=option]'], ['[role=menu]', '[role=menuitem]']]) {
        for (const g of scope.querySelectorAll(group)) {
            if (!visible(g)) continue;
            const items = [...g.querySelectorAll(member)].filter(usable).map((el) => option(el));
            const found = done(items, g);
            if (found) return found;
        }
    }
    const clickable = 'a[href],button,[role=button],[role=link],input[type=button],input[type=submit]';
    if (dialogs.length) {
        const items = [...scope.querySelectorAll(clickable)].filter(usable).map((el) => option(el));
        const found = done(items, scope);
        if (found) return found;
    }
    for (const list of scope.querySelectorAll('ul,ol,[role=list]')) {
        if (!visible(list) || !(dialogs.length || list.closest('form') || CHOOSY.test(headingBefore(scope, list)))) continue;
        const rows = [...list.children].filter(visible);
        if (rows.length < 2 || rows.length > 12) continue;
        const items = [];
        for (const row of rows) {
            const controls = [...row.querySelectorAll(clickable)].filter(usable);
            if (controls.length !== 1) { items.length = 0; break; }
            items.push(option(controls[0]));
        }
        const found = done(items, list);
        if (found) return found;
    }
    // A short row of buttons under a heading that asks for a decision (consent, terms, trust
    // this device), outside any sign-in form.
    const forms = [...scope.querySelectorAll('form')].filter(visible);
    for (const group of [...forms, scope]) {
        if (group.querySelector('input[type=password]') || [...group.querySelectorAll('input')].some(textLike)) continue;
        const buttons = [...group.querySelectorAll('button,input[type=submit],input[type=button],[role=button]')].filter(usable);
        if (buttons.length < 2 || buttons.length > 4 || !CHOOSY.test(headingBefore(scope, buttons[0]))) continue;
        const found = done(buttons.map((el) => option(el)), buttons[0]);
        if (found) return found;
    }
    return null;
};
const alerts = () => [...document.querySelectorAll('[role=alert],[aria-live=assertive],[aria-live=polite],.error,.error_message,[class*="error" i],[id*="error" i],[class*="err_" i]')]
    .filter(visible).map((el) => clean(el.innerText, 200)).filter(Boolean);
// The first step of a two-step sign-in: one account-name field and a continue button.
const identifierStep = () => {
    if (passwordField()) return false;
    const fields = [...document.querySelectorAll('input')].filter(textLike).filter((el) => !otpField(el));
    if (fields.length !== 1 || idScore(fields[0]) < 2) return false;
    const form = fields[0].form || fields[0].closest('form') || document.body;
    return [...form.querySelectorAll('button,input[type=submit],[role=button]')].some((el) => usable(el) && LOGINISH.test(nameOf(el)));
};
const PUSH = /approve (the |this |your )?(sign[- ]?in|log ?in|request)|check your (phone|device)|on your (phone|device|mobile)|sent (a |you a )?(notification|push|request)|tap (yes|approve|allow|the number)|open the .{1,40} app to (approve|confirm|sign in)|waiting for (your )?approval|휴대폰에서 .{0,12}(승인|확인)|앱에서 .{0,12}(승인|확인)|알림을 (확인|눌러)/i;
const enrollment = (text) => {
    let otpauth = /otpauth:\/\//i.test(text);
    for (const el of document.querySelectorAll('a[href],img[src],img[alt],[data-otpauth],input[type=hidden]')) {
        if (otpauth) break;
        for (const a of el.attributes) if (/^otpauth:\/\//i.test(a.value)) { otpauth = true; break; }
    }
    if (otpauth) return true;
    if (!/authenticator|2-?step|two-?factor|2fa|setup key|secret key|scan (the |this )?(qr )?code|인증 ?앱|OTP 앱|보안 ?키/i.test(text)) return false;
    const key = /(^|\s)[A-Z2-7]{4}( ?[A-Z2-7]{4}){3,}(\s|$)/.test(text);
    const qr = [...document.querySelectorAll('img,canvas,svg')].some((el) => {
        if (!visible(el)) return false;
        const r = el.getBoundingClientRect();
        return r.width >= 96 && Math.abs(r.width - r.height) <= 8;
    });
    return key || qr;
};
const siteName = () => {
    const meta = (name) => { const m = document.querySelector(`meta[property="${name}"],meta[name="${name}"]`); return m ? clean(m.content, 60) : ''; };
    const title = clean(document.title, 60);
    return meta('og:site_name') || meta('application-name') || (title.length <= 40 ? title : '') || location.hostname;
};
"#;

/// Finds the login form; resolves to `{data, nodes}` for [`Browser::locate`].
const LOGIN_FORM_JS: &str = r#"
const nodes = [];
const pin = (el) => nodes.push(el) - 1;
const password = passwordField();
const scopeOf = (el) => (el && (el.form || el.closest('form'))) || document;
let identifier = null;
{
    const within = scopeOf(password);
    const candidates = [...within.querySelectorAll('input')].filter(textLike)
        .filter((el) => !password || (el.compareDocumentPosition(password) & Node.DOCUMENT_POSITION_FOLLOWING))
        .filter((el) => !otpField(el));
    let best = 0;
    for (const el of candidates) { const s = idScore(el); if (s >= best) { best = s; identifier = el; } }
    if (!password && best < 2) identifier = null;
}
if (!password && !identifier) return {data: {found: false}, nodes: []};
const root = (password || identifier).form || (password || identifier).closest('form') || document.body;
const fields = [];
const ac = (el, fallback) => {
    const token = (el.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/).pop();
    return ['username', 'email', 'tel', 'current-password'].includes(token) ? token : fallback;
};
const required = (el) => el.required || el.getAttribute('aria-required') === 'true';
const placeholder = (el) => clean(el.getAttribute('placeholder'), 80) || null;
if (identifier) fields.push({kind: 'identifier', label: fieldLabel(identifier) || 'Username', placeholder: placeholder(identifier),
    autocomplete: identifier.type === 'email' ? 'email' : identifier.type === 'tel' ? 'tel' : ac(identifier, 'username'), masked: false, required: true, node: pin(identifier)});
if (password) fields.push({kind: 'password', label: fieldLabel(password) || 'Password', placeholder: placeholder(password),
    autocomplete: 'current-password', masked: true, required: true, node: pin(password)});
const KEEP = /keep|stay|remember|signed in|로그인 ?상태|자동 ?로그인|기억/i;
for (const box of root.querySelectorAll('input[type=checkbox],[role=checkbox]')) {
    const label = box.labels && box.labels[0];
    if (box.disabled || !(visible(box) || (label && visible(label)))) continue;
    const text = clean((label && label.innerText) || box.getAttribute('aria-label') || labelledby(box), 80);
    if (!KEEP.test(text)) continue;
    fields.push({kind: 'checkbox', label: text, placeholder: null, autocomplete: null, masked: false, required: false, node: pin(box)});
    if (fields.filter((f) => f.kind === 'checkbox').length >= 2) break;
}
const providerOf = (text) => {
    const host = location.hostname.toLowerCase();
    const hit = PROVIDERS.find(([name, re]) => re.test(text) && !host.includes(name));
    return hit ? hit[0] : null;
};
let submit = null, score = 0;
for (const el of root.querySelectorAll('button,input[type=submit],input[type=image],[role=button]')) {
    if (!usable(el) || el.type === 'reset') continue;
    const text = nameOf(el);
    if (providerOf(text + ' ' + el.className + ' ' + el.id)) continue;
    const s = (el.type === 'submit' ? 1 : 0) + (LOGINISH.test(text) ? 2 : 0);
    if (s > score) { score = s; submit = el; }
}
const taken = new Set([identifier, password, submit]);
const alternatives = [];
const seen = new Set();
for (const el of document.querySelectorAll('a[href],button,[role=button],[role=tab],[role=link]')) {
    if (alternatives.length >= 8) break;
    if (taken.has(el) || !usable(el) || el.getAttribute('aria-selected') === 'true') continue;
    if ([identifier, password].some((f) => f && el.contains(f))) continue;
    const label = clean(nameOf(el), 60);
    const hints = label + ' ' + (typeof el.className === 'string' ? el.className : '') + ' ' + el.id;
    let kind = null, provider = providerOf(hints);
    if (provider && label.length <= 40) kind = 'provider';
    else if (/passkey|패스키|security key|보안 ?키/i.test(hints)) kind = 'passkey';
    else if (/(^|[^a-z])qr([^a-z]|$)|qr ?코드/i.test(hints)) kind = 'qr';
    else if (/one-?time (number|code|password)|일회용|magic link|email me a link|휴대폰 ?로그인|phone login/i.test(hints)) kind = 'other';
    if (!kind) continue;
    if (kind !== 'provider') provider = null;
    const key = kind + '|' + provider + '|' + label;
    if (seen.has(key)) continue;
    seen.add(key);
    alternatives.push({label: label || provider || kind, kind, provider, node: pin(el)});
}
const errors = alerts();
const data = {found: true, site_name: siteName(), fields, alternatives,
    error_text: errors.length ? errors[0] : null,
    submit: submit ? {label: nameOf(submit) || 'Log in', node: pin(submit)} : null};
return {data, nodes};
"#;

/// Finds the page's choice; resolves to `{data, nodes}` for [`Browser::locate`].
const CHOICE_JS: &str = r#"
const found = findChoice();
const site = {site_name: siteName()};
if (!found) return {data: Object.assign({found: false}, site), nodes: []};
return {data: Object.assign({found: true, question: found.question,
    options: found.options.map((o) => ({label: o.label, description: o.description || null}))}, site),
    nodes: found.options.map((o) => o.el)};
"#;

/// Only the page's name, for a choice built from the model's refs.
const SITE_JS: &str = r#"
return {data: {site_name: siteName()}, nodes: []};
"#;

/// Classifies the page after a submit or click. `login` is a sign-in form (password, or an
/// identifier-only first step) without an error message; `none` is a page that asks for
/// nothing. A setup page for an authenticator (an otpauth link, or a QR code or text key next
/// to authenticator wording) is `enroll_totp`, ahead of `otp`, since it also asks for a code.
const NEXT_STEP_JS: &str = r#"
const captcha = [...document.querySelectorAll('iframe[src*="recaptcha"],iframe[src*="hcaptcha"],iframe[src*="turnstile"],iframe[src*="captcha"],iframe[src*="arkoselabs"],.g-recaptcha,.h-captcha,.cf-turnstile,[id*="captcha" i],[name*="captcha" i],img[alt*="captcha" i],img[src*="captcha" i]')].some(visible);
if (captcha) return {step: 'captcha'};
const text = document.body ? String(document.body.innerText || '').slice(0, 20000) : '';
const inputs = [...document.querySelectorAll('input')];
const codeBox = inputs.some(otpField) || inputs.filter((el) => usable(el) && el.maxLength === 1).length >= 4;
if (enrollment(text)) return {step: 'enroll_totp'};
if (codeBox) return {step: 'otp'};
const password = passwordField();
if (password || identifierStep()) {
    const errors = alerts();
    return errors.length ? {step: 'error', error_text: errors[0]} : {step: 'login'};
}
// A page that only reports a failure (a denied or timed-out approval, a locked account).
const failure = [...document.querySelectorAll('[role=alert],[aria-live=assertive]')].filter(visible)
    .map((el) => clean(el.innerText, 200)).find(Boolean);
if (failure) return {step: 'error', error_text: failure};
if (!inputs.some((el) => textLike(el)) && PUSH.test(text)) return {step: 'push_approval'};
if (findChoice()) return {step: 'choice'};
return {step: 'none'};
"#;

/// Reads a phone-approval wait page for its card: the heading and instruction, and the
/// number the page asks the person to match on the phone.
const APPROVAL_JS: &str = r#"
const text = document.body ? String(document.body.innerText || '').slice(0, 20000) : '';
if ([...document.querySelectorAll('input')].some((el) => textLike(el)) || !PUSH.test(text))
    return {data: {found: false}, nodes: []};
const scope = document.querySelector('main') || document.body;
const heading = [...scope.querySelectorAll('h1,h2,h3,[role=heading]')].filter(visible).map((h) => clean(h.innerText, 120)).find(Boolean) || '';
const line = text.split('\n').map((s) => clean(s, 200)).find((s) => PUSH.test(s) && s !== heading) || '';
let number = null;
for (const el of scope.querySelectorAll('*')) {
    if (el.children.length || !visible(el)) continue;
    const t = clean(el.innerText, 8);
    if (/^\d{1,3}$/.test(t) && parseFloat(getComputedStyle(el).fontSize) >= 24) { number = t; break; }
}
return {data: {found: true, site_name: siteName(),
    message: clean([heading, line].filter(Boolean).join(' — '), 240), number}, nodes: []};
"#;

/// Reads an option the model named: visible, enabled, and the page's own text for it.
const OPTION_FN: &str = r#"function() {
    const r = this.getBoundingClientRect();
    const s = getComputedStyle(this);
    const shown = r.width > 1 && r.height > 1 && s.visibility !== 'hidden' && s.display !== 'none';
    const disabled = this.disabled === true || this.getAttribute('aria-disabled') === 'true';
    const by = (this.getAttribute('aria-labelledby') || '').split(/\s+/).map((id) => { const t = id && document.getElementById(id); return t ? t.innerText : ''; }).join(' ');
    const own = this.tagName === 'INPUT' ? ((this.labels && this.labels[0] && this.labels[0].innerText) || (['button', 'submit'].includes(this.type) ? this.value : '')) : this.innerText;
    const text = String(this.getAttribute('aria-label') || by || own || this.getAttribute('title') || '')
        .split('\n').map((t) => t.replace(/\s+/g, ' ').trim()).filter(Boolean);
    return {ok: shown && !disabled, label: (text[0] || '').slice(0, 80), description: text.slice(1).join(' ').slice(0, 120)};
}"#;

fn located_script(body: &str) -> String {
    format!("(async () => {{ {COMMON_JS}\n{body} }})()")
}

fn sync_script(body: &str) -> String {
    format!("(() => {{ {COMMON_JS}\n{body} }})()")
}

/// What the page asks for after a submit or click.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    Done,
    /// Another sign-in page: the password step after an account-name step, or a provider's.
    Login,
    Otp,
    /// The site waits for the person to approve the sign-in on their phone.
    PushApproval,
    /// The site offers to set up an authenticator (QR code or setup key, then a code).
    EnrollTotp,
    Choice,
    Captcha,
    Error,
    Unknown,
}

impl Next {
    fn wire(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Login => "login",
            Self::Otp => "otp",
            Self::PushApproval => "push_approval",
            Self::EnrollTotp => "enroll_totp",
            Self::Choice => "choice",
            Self::Captcha => "captcha",
            Self::Error => "error",
            Self::Unknown => "unknown",
        }
    }
}

/// The tab's document before an action, so [`next_step`] can tell a new sign-in page from
/// the same form still showing.
async fn document_before(browser: &Browser, tab: &str) -> Option<String> {
    browser.document_lifetime_id(tab).await.ok()
}

/// Poll the page until its next step holds still (two equal readings), for at most
/// [`NEXT_WAIT`]. Readings fail while a navigation is in flight; those are skipped. A sign-in
/// form in a document other than `before` is the next sign-in step; in the same document it
/// is the form the action left in place, reported as `unknown` once it settles. The caller
/// holds the browser gate, so no daemon operation moves the tab meanwhile.
async fn next_step(
    browser: &Browser,
    owner: &str,
    tab: &str,
    before: Option<&str>,
) -> (Next, Option<String>) {
    let script = sync_script(NEXT_STEP_JS);
    let started = Instant::now();
    let mut last: Option<(String, Option<String>)> = None;
    let mut login_since: Option<Instant> = None;
    while started.elapsed() < NEXT_WAIT {
        tokio::time::sleep(NEXT_POLL).await;
        let Ok(reading) = browser.evaluate_isolated(owner, tab, &script).await else {
            last = None;
            continue;
        };
        let step = reading["step"].as_str().unwrap_or("none").to_owned();
        let text = reading["error_text"].as_str().map(str::to_owned);
        if step == "login" {
            let moved = match before {
                None => true,
                Some(before) => document_before(browser, tab)
                    .await
                    .is_some_and(|now| now != before),
            };
            if !moved {
                let since = *login_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= LOGIN_SETTLE {
                    return (Next::Unknown, None);
                }
                last = None;
                continue;
            }
        } else {
            login_since = None;
        }
        let current = (step, text);
        if last.as_ref() == Some(&current) {
            let next = match current.0.as_str() {
                "captcha" => Next::Captcha,
                "enroll_totp" => Next::EnrollTotp,
                "otp" => Next::Otp,
                "error" => Next::Error,
                "login" => Next::Login,
                "push_approval" => Next::PushApproval,
                "choice" => Next::Choice,
                _ => Next::Done,
            };
            return (next, current.1.filter(|_| next == Next::Error));
        }
        last = Some(current);
    }
    (Next::Unknown, None)
}

fn with_next(mut result: Value, (next, error_text): (Next, Option<String>)) -> Value {
    result["next"] = json!(next.wire());
    if let Some(text) = error_text {
        result["error_text"] = json!(text);
    }
    result
}

/// A field of the located form, with the index of its pinned node.
#[derive(Deserialize)]
struct FoundField {
    kind: LoginFieldKind,
    label: String,
    placeholder: Option<String>,
    autocomplete: Option<String>,
    masked: bool,
    required: bool,
    node: usize,
}

#[derive(Deserialize)]
struct FoundSubmit {
    label: String,
    node: usize,
}

#[derive(Deserialize)]
struct FoundAlternative {
    label: String,
    kind: LoginAlternativeKind,
    provider: Option<LoginProvider>,
    node: usize,
}

#[derive(Deserialize)]
struct FoundForm {
    found: bool,
    #[serde(default)]
    site_name: String,
    #[serde(default)]
    fields: Vec<FoundField>,
    #[serde(default)]
    alternatives: Vec<FoundAlternative>,
    #[serde(default)]
    submit: Option<FoundSubmit>,
    #[serde(default)]
    error_text: Option<String>,
}

/// The pinned nodes of a login form, by role.
#[derive(Default)]
struct Form {
    identifier: Option<NodeHandle>,
    password: Option<NodeHandle>,
    submit: Option<NodeHandle>,
}

impl Form {
    fn from_pins(login: &TicketLogin, pins: &[(String, NodeHandle)]) -> Self {
        let pin = |id: &str| {
            pins.iter()
                .find(|(name, _)| name == id)
                .map(|(_, node)| node.clone())
        };
        let field = |kind: LoginFieldKind| {
            login
                .fields
                .iter()
                .find(|field| field.kind == kind)
                .and_then(|field| pin(&field.id))
        };
        Self {
            identifier: field(LoginFieldKind::Identifier),
            password: field(LoginFieldKind::Password),
            submit: pin("submit"),
        }
    }
}

/// A built login ticket, the pins by ticket id, and the pinned nodes by role.
type BuiltLogin = (TicketLogin, Vec<(String, NodeHandle)>, Form);

fn build_login(found: FoundForm, nodes: &[NodeHandle], origin: &str) -> Option<BuiltLogin> {
    let node = |index: usize| nodes.get(index).cloned();
    let mut pins = Vec::new();
    let mut form = Form::default();
    let mut fields = Vec::new();
    for (index, field) in found.fields.into_iter().enumerate() {
        let id = format!("f{index}");
        let handle = node(field.node)?;
        match field.kind {
            LoginFieldKind::Identifier => form.identifier = Some(handle.clone()),
            LoginFieldKind::Password => form.password = Some(handle.clone()),
            _ => {}
        }
        pins.push((id.clone(), handle));
        fields.push(TicketLoginField {
            id,
            kind: field.kind,
            label: field.label,
            placeholder: field.placeholder.filter(|text| !text.is_empty()),
            autocomplete: field.autocomplete.filter(|token| {
                ["username", "email", "tel", "current-password"].contains(&token.as_str())
            }),
            suggestions: Vec::new(),
            masked: field.masked,
            required: field.required,
        });
    }
    let submit_label = match found.submit {
        Some(submit) => {
            let handle = node(submit.node)?;
            form.submit = Some(handle.clone());
            pins.push(("submit".into(), handle));
            submit.label
        }
        None => "Log in".into(),
    };
    let mut alternatives = Vec::new();
    for (index, alternative) in found.alternatives.into_iter().enumerate() {
        let id = format!("a{index}");
        pins.push((id.clone(), node(alternative.node)?));
        alternatives.push(TicketLoginAlternative {
            id,
            label: alternative.label,
            kind: alternative.kind,
            provider: alternative
                .provider
                .filter(|_| alternative.kind == LoginAlternativeKind::Provider),
        });
    }
    if form.identifier.is_none() && form.password.is_none() {
        return None;
    }
    let site_name = if found.site_name.trim().is_empty() {
        origin.to_owned()
    } else {
        found.site_name
    };
    Some((
        TicketLogin {
            origin: origin.to_owned(),
            site_name,
            icon: None,
            fields,
            alternatives,
            accounts: None,
            submit_label,
            can_save: false,
            error_text: found.error_text.filter(|text| !text.trim().is_empty()),
        },
        pins,
        form,
    ))
}

/// Submit the form: click its pinned sign-in button, or press Enter in its last text field
/// when the page has no recognizable one.
async fn submit(
    browser: &Browser,
    owner: &str,
    tab: &str,
    form: &Form,
) -> Result<(), BrowserError> {
    match (
        &form.submit,
        form.password.as_ref().or(form.identifier.as_ref()),
    ) {
        (Some(button), _) => browser.click_node(owner, tab, button).await,
        (None, Some(field)) => browser.press_enter_node(owner, tab, field).await,
        (None, None) => Err(BrowserError::InvalidAction("form has nothing to submit")),
    }
}

/// Fill a saved login into the form inside the vault's callback and submit it. The tab is
/// tainted before any value exists; a refusal releases nothing.
#[allow(clippy::too_many_arguments)]
async fn fill_account(
    shared: &Shared,
    browser: &Browser,
    owner: &SessionId,
    tab: &str,
    origin: &str,
    form: &Form,
    site: &str,
    audit: &AuditContext<'_>,
) -> Result<LoginOutcome, ToolError> {
    let controller = &shared.registry.browser;
    let mut guard = shared.registry.vault.vault.lock().await;
    let vault = guard
        .as_mut()
        .ok_or_else(|| ToolError("Vault is unavailable".into()))?;
    if let Ok(redactor) = vault.redactor_for(site) {
        shared.registry.redactors.write().push(redactor);
    }
    vault
        .vault_login_async(audit, AuditAction::Fill, site, origin, |secrets| {
            Box::pin(async move {
                controller.mark_secret(shared, browser, tab).await?;
                let owner = owner.as_str();
                if let Some(node) = &form.identifier {
                    Controller::fill_node(browser, owner, tab, node, secrets.username)
                        .await
                        .map_err(|_| ())?;
                }
                if let Some(node) = &form.password {
                    Controller::fill_node(browser, owner, tab, node, secrets.password)
                        .await
                        .map_err(|_| ())?;
                }
                submit(browser, owner, tab, form).await.map_err(|_| ())
            })
        })
        .await
        .map_err(|_| ToolError("Vault fill or audit failed".into()))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginRequestArgs {
    /// Tab showing the site's sign-in page.
    tab_id: String,
    /// Short note for the person on why signing in is needed. Never a password.
    reason: Option<String>,
}

pub(crate) struct RequestLogin;
impl DaemonTool for RequestLogin {
    fn name(&self) -> &'static str {
        "request_login"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<LoginRequestArgs>(
            self.name(),
            "Use whenever a page asks the person to sign in. The daemon reads the tab's login form itself. With exactly one login saved in the vault for the page it signs in at once; otherwise the person gets a sign-in card on their own device, mirroring the page's fields and its other sign-in options (Google, Apple, passkey, QR …), and may keep the login in the vault. Values go straight into the page and never reach you; the tab is never brought to the screen. Result {\"status\":\"submitted\"|\"filled_from_vault\"|\"alternative\",\"next\":…}. Then follow `next`: login → call request_login again (the next sign-in page), otp → take a fresh snapshot for the current code-field ref, then request_otp, push_approval → wait_for_approval, enroll_totp → register_authenticator, choice → request_choice, captcha → tell the person and offer takeover, error → report error_text and call request_login again, done → continue, unknown → take a snapshot. login_form_not_found means inspect the page, not that sign-in succeeded: profile, account, trust-device and terms screens need request_choice. Never ask for passwords in chat or ask_user and never type them yourself.",
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
            let args: LoginRequestArgs = parse(args)?;
            if args.tab_id.trim().is_empty() {
                return Err(ToolError("tab_id must not be empty".into()));
            }
            origin_actions(ctx, &args.tab_id).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"reason":args["reason"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Self::call_with_vault(args, ctx, true)
    }
}

impl RequestLogin {
    fn call_with_vault(args: Value, ctx: &ToolEnv, allow_vault: bool) -> BoxFuture<'_, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: LoginRequestArgs = parse(&args)?;
                let origin = approved_origin(ctx)?;
                let controller = &ctx.shared.registry.browser;
                let browser = controller.for_tab(&ctx.shared, &ctx.session, &args.tab_id).await?;
                let owner = ctx.session.as_str();
                let tab = args.tab_id.as_str();
                if Controller::origin(&browser, &ctx.session, tab).await? != origin {
                    return Err(ToolError("Page origin changed; take a fresh snapshot".into()));
                }
                let (found, nodes) = browser
                    .locate(owner, tab, &located_script(LOGIN_FORM_JS))
                    .await
                    .map_err(browser_error)?;
                let found: FoundForm = serde_json::from_value(found)
                    .map_err(|_| ToolError("Login form could not be read".into()))?;
                if !found.found {
                    return Ok(ToolExec::Denied {
                        reason: "login_form_not_found".into(),
                        hint: Some("No sign-in form is visible on this tab. Take a snapshot; if the page shows a choice, use request_choice.".into()),
                    });
                }
                let (mut login, pins, form) = build_login(found, &nodes, &origin)
                    .ok_or_else(|| ToolError("Login form could not be read".into()))?;
                login.icon = crate::favicons::for_tab(&browser, owner, tab, &origin).await;
                let root = ctx.shared.config.data_dir.clone();
                let addresses = tokio::task::spawn_blocking(move || crate::import_delivery::source_addresses(&root))
                    .await.map_err(|_| ToolError("Imported address suggestions could not be read".into()))?
                    .map_err(|_| ToolError("Imported address suggestions could not be read".into()))?;
                for field in &mut login.fields {
                    if !field.masked {
                        field.suggestions = super::imports::address_suggestions(&addresses, field.autocomplete.as_deref().unwrap_or_default());
                    }
                }
                let (accounts, refusal) = {
                    let guard = ctx.shared.registry.vault.vault.lock().await;
                    match guard.as_ref() {
                        Some(vault) => (
                            vault.logins_for(&origin).unwrap_or_default(),
                            vault.login_save_refusal(&origin).ok().flatten(),
                        ),
                        None => (Vec::new(), Some(openside_vault::Denial::PolicyNever)),
                    }
                };
                let usable: Vec<_> = accounts
                    .into_iter()
                    .filter(|account| account.effective_policy != AccessPolicy::Never)
                    .collect();
                let _ = crate::connections::begin_login(&ctx.shared, &browser, &ctx.session, tab, &origin).await;
                if let [account] = if allow_vault { usable.as_slice() } else { &[] } {
                    let audit = AuditContext {
                        who: "opensided",
                        session_id: Some(owner),
                        tool_call_id: Some(ctx.tool_call_id.as_str()),
                    };
                    let before = document_before(&browser, tab).await;
                    let outcome = fill_account(
                        &ctx.shared,
                        &browser,
                        &ctx.session,
                        tab,
                        &origin,
                        &form,
                        &account.site,
                        &audit,
                    )
                    .await?;
                    let _ = controller.persist(&ctx.shared).await;
                    return Ok(match outcome {
                        LoginOutcome::Filled => {
                            crate::connections::remember_login_item(&ctx.shared, &ctx.session, tab, &account.site).await;
                            let next = next_step(&browser, owner, tab, before.as_deref()).await;
                            if next.0 == Next::Done {
                                let _ = crate::connections::complete_login(&ctx.shared, &browser, &ctx.session, tab, openside_proto::ConnectionSource::Vault, Some(&account.site)).await;
                            }
                            done(with_next(json!({"status":"filled_from_vault","item_id":account.site}), next))
                        },
                        LoginOutcome::Denied(denial) => ToolExec::Denied {
                            reason: format!("vault_{}", denial.wire()),
                            hint: Some("The vault refused the saved login for this page; the refusal is audited.".into()),
                        },
                        LoginOutcome::FillFailed => done(json!({"status":"fill_failed"})),
                    });
                }
                if usable.len() > 1 {
                    login.accounts = Some(
                        usable
                            .into_iter()
                            .map(|account| TicketLoginAccount {
                                item_id: account.site,
                                username_hint: account.username_hint,
                            })
                            .collect(),
                    );
                }
                // The password page of a two-step sign-in can keep the login with the account
                // name typed on the page before.
                login.can_save = refusal.is_none()
                    && form.password.is_some()
                    && (form.identifier.is_some() || account_step(&ctx.session, &origin).is_some());
                // Notifications identify the verified authority, not the page's claimed name.
                let authority = origin.split_once("://").map_or(origin.as_str(), |(_, host)| host);
                let summary = format!("Sign in to {authority}");
                let mut instructions = format!("Enter your sign-in details for {authority}. Values go straight into the page; saving to the vault is optional.");
                if let Some(reason) = args.reason.filter(|reason| !reason.trim().is_empty()) {
                    instructions.push_str("\nWhy this was requested: ");
                    instructions.push_str(&reason);
                }
                Ok(ToolExec::Defer(Box::new(TicketRequest { hire: None, form: None, kind: TicketKind::Login,
                summary,
                instructions: Some(instructions),
                options: Vec::new(),
                target: Some(TicketTarget {
                    tab_id: Some(args.tab_id),
                    origin: Some(origin),
                    field_ref: None,
                }),
                resume_when: None,
                otp: None,
                otp_register: None,
                login: Some(login),
                choice: None,
                approval: None,
                pins, })))
            }
            .await;
            result.unwrap_or_else(|error: ToolError| ToolExec::Denied {
                reason: error.0,
                hint: None,
            })
        })
    }
}

pub(crate) async fn relogin_continuation(ctx: &ToolEnv, tab: &str, next: &str) -> ToolExec {
    let detected = if matches!(next, "unknown" | "error") {
        match ctx
            .shared
            .registry
            .browser
            .for_tab(&ctx.shared, &ctx.session, tab)
            .await
        {
            Ok(browser) => browser
                .evaluate_isolated(ctx.session.as_str(), tab, &sync_script(NEXT_STEP_JS))
                .await
                .ok()
                .and_then(|value| value["step"].as_str().map(str::to_owned)),
            Err(_) => None,
        }
    } else {
        None
    };
    let outcome = match detected.as_deref().unwrap_or(next) {
        "login" | "error" => RequestLogin::call_with_vault(json!({"tab_id":tab}), ctx, false).await,
        "push_approval" => WaitForApproval.call(json!({"tab_id":tab}), ctx).await,
        "otp" => {
            let found = async {
                let browser = ctx.shared.registry.browser.for_tab(&ctx.shared, &ctx.session, tab).await?;
                let owner = ctx.session.as_str();
                let snapshot = browser.snapshot(tab).await.map_err(browser_error)?;
                let probe = format!("function() {{ {COMMON_JS}\nreturn otpField(this) || (usable(this) && this.maxLength === 1 && [...document.querySelectorAll('input')].filter((el) => usable(el) && el.maxLength === 1).length >= 4); }}");
                for node in &snapshot.nodes {
                    if !matches!(node.role.as_str(), "textbox" | "spinbutton") {
                        continue;
                    }
                    let Some(reference) = &node.reference else { continue; };
                    let pinned = browser.resolve_ref(owner, tab, reference).await.map_err(browser_error)?;
                    if browser.inspect_node(owner, tab, &pinned, &probe).await.map_err(browser_error)? == Value::Bool(true) {
                        return Ok::<_, ToolError>(reference.clone());
                    }
                }
                Err(ToolError("otp_field_not_found".into()))
            }.await;
            match found {
                Ok(field) => {
                    super::otp::RequestOtp::call_with_vault(
                        json!({"tab_id":tab,"field_ref":field}),
                        ctx,
                        false,
                    )
                    .await
                }
                Err(error) => ToolExec::Denied {
                    reason: error.0,
                    hint: None,
                },
            }
        }
        _ => ToolExec::Denied {
            reason: "connection_takeover_required".into(),
            hint: None,
        },
    };
    match outcome {
        ToolExec::Denied { ref reason, .. }
            if matches!(
                reason.as_str(),
                "otp_field_not_found" | "login_form_not_found"
            ) =>
        {
            ToolExec::Denied {
                reason: "connection_takeover_required".into(),
                hint: None,
            }
        }
        outcome => outcome,
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChoiceOptionArg {
    /// Ref of the option's control in your latest snapshot.
    #[serde(rename = "ref")]
    reference: String,
    /// Ignored for display: the person always sees the page's own text for the control.
    #[allow(dead_code)]
    label: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChoiceRequestArgs {
    /// Tab showing the choice.
    tab_id: String,
    /// The question, used only when the page has no heading for it.
    question: Option<String>,
    /// The options' refs; omit to let the daemon find the page's choice itself.
    options: Option<Vec<ChoiceOptionArg>>,
}

/// Browse on the tab's live origin: a choice clicks the page but releases no secret.
async fn browse_action(ctx: &ToolEnv, tab: &str) -> std::result::Result<Vec<Action>, ToolError> {
    let browser = ctx
        .shared
        .registry
        .browser
        .for_tab(&ctx.shared, &ctx.session, tab)
        .await?;
    let origin = Controller::origin(&browser, &ctx.session, tab).await?;
    Ok(vec![Action::new(Capability::Browse, origin)])
}

#[derive(Deserialize)]
struct FoundOption {
    label: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct FoundChoice {
    found: bool,
    #[serde(default)]
    site_name: String,
    #[serde(default)]
    question: String,
    #[serde(default)]
    options: Vec<FoundOption>,
}

pub(crate) struct RequestChoice;
impl DaemonTool for RequestChoice {
    fn name(&self) -> &'static str {
        "request_choice"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<ChoiceRequestArgs>(
            self.name(),
            "Ask the person to make a decision the page presents (which account, which verification method, trust this device, …) instead of clicking it yourself. Omit options to let the daemon find the page's choice, or pass refs from your latest snapshot; the person always sees the page's own labels. Their pick is clicked in the tab without bringing it to the screen. Result {\"status\":\"chosen\",\"option\":…,\"next\":…}; follow `next` as for request_login.",
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
            let args: ChoiceRequestArgs = parse(args)?;
            if args.tab_id.trim().is_empty() {
                return Err(ToolError("tab_id must not be empty".into()));
            }
            if args.options.as_ref().is_some_and(|options| {
                options.len() < 2
                    || options.len() > MAX_OPTIONS
                    || options
                        .iter()
                        .any(|option| option.reference.trim().is_empty())
            }) {
                return Err(ToolError(format!(
                    "options must name 2 to {MAX_OPTIONS} refs"
                )));
            }
            browse_action(ctx, &args.tab_id).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"],"question":args["question"],"options":args["options"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: ChoiceRequestArgs = parse(&args)?;
                let origin = ctx
                    .approved_actions
                    .iter()
                    .find(|action| action.capability == Capability::Browse)
                    .map(|action| action.target.clone())
                    .ok_or_else(|| ToolError("Missing approved tool target".into()))?;
                let browser = ctx.shared.registry.browser.for_tab(&ctx.shared, &ctx.session, &args.tab_id).await?;
                let owner = ctx.session.as_str();
                let tab = args.tab_id.as_str();
                if Controller::origin(&browser, &ctx.session, tab).await? != origin {
                    return Err(ToolError("Page origin changed; take a fresh snapshot".into()));
                }
                let (found, nodes) = match &args.options {
                    None => {
                        let (data, nodes) = browser
                            .locate(owner, tab, &located_script(CHOICE_JS))
                            .await
                            .map_err(browser_error)?;
                        let found: FoundChoice = serde_json::from_value(data)
                            .map_err(|_| ToolError("Choice could not be read".into()))?;
                        (found, nodes)
                    }
                    Some(options) => {
                        let (data, _) = browser
                            .locate(owner, tab, &located_script(SITE_JS))
                            .await
                            .map_err(browser_error)?;
                        let document = browser.document_id(tab).await.map_err(browser_error)?;
                        let mut found = Vec::new();
                        let mut nodes = Vec::new();
                        for option in options {
                            let node = browser
                                .resolve_ref(owner, tab, &option.reference)
                                .await
                                .map_err(browser_error)?;
                            if node.document_id != document || node.origin != origin {
                                return Err(ToolError("Page changed; take a fresh snapshot".into()));
                            }
                            let read: Value = browser
                                .inspect_node(owner, tab, &node, OPTION_FN)
                                .await
                                .map_err(browser_error)?;
                            let label = read["label"].as_str().unwrap_or_default().to_owned();
                            if read["ok"] != true || label.is_empty() {
                                return Err(ToolError(format!(
                                    "{} is not a visible, enabled control with a label",
                                    option.reference
                                )));
                            }
                            found.push(FoundOption {
                                label,
                                description: read["description"].as_str().map(str::to_owned),
                            });
                            nodes.push(node);
                        }
                        (
                            FoundChoice {
                                found: true,
                                site_name: data["site_name"].as_str().unwrap_or_default().to_owned(),
                                question: String::new(),
                                options: found,
                            },
                            nodes,
                        )
                    }
                };
                if !found.found || found.options.len() < 2 || nodes.len() != found.options.len() {
                    return Ok(ToolExec::Denied {
                        reason: "choice_not_found".into(),
                        hint: Some("No choice with at least two options is visible. Take a snapshot and pass the options' refs.".into()),
                    });
                }
                let mut options = Vec::new();
                let mut pins = Vec::new();
                for (index, (option, node)) in found
                    .options
                    .into_iter()
                    .zip(nodes)
                    .take(MAX_OPTIONS)
                    .enumerate()
                {
                    let id = format!("o{index}");
                    pins.push((id.clone(), node));
                    options.push(TicketChoiceOption {
                        id,
                        label: option.label,
                        description: option.description.filter(|text| !text.trim().is_empty()),
                    });
                }
                let question = Some(found.question)
                    .filter(|text| !text.trim().is_empty())
                    .or(args.question.filter(|text| !text.trim().is_empty()))
                    .unwrap_or_else(|| "Choose an option".into());
                let site_icon = crate::favicons::for_tab(&browser, owner, tab, &origin).await;
                let site_name = if found.site_name.trim().is_empty() {
                    origin.clone()
                } else {
                    found.site_name
                };
                Ok(ToolExec::Defer(Box::new(TicketRequest { hire: None, form: None, kind: TicketKind::Choice,
                summary: question.clone(),
                instructions: None,
                options: Vec::new(),
                target: Some(TicketTarget {
                    tab_id: Some(args.tab_id),
                    origin: Some(origin.clone()),
                    field_ref: None,
                }),
                resume_when: None,
                otp: None,
                otp_register: None,
                login: None,
                choice: Some(TicketChoice {
                    origin,
                    site_name,
                    icon: site_icon,
                    question,
                    options,
                    multiple: false,
                }),
                approval: None,
                pins, })))
            }
            .await;
            result.unwrap_or_else(|error: ToolError| ToolExec::Denied {
                reason: error.0,
                hint: None,
            })
        })
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalWaitArgs {
    /// Tab showing the site's "approve the sign-in on your phone" page.
    tab_id: String,
}

#[derive(Deserialize)]
struct FoundApproval {
    found: bool,
    #[serde(default)]
    site_name: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    number: Option<String>,
}

pub(crate) struct WaitForApproval;
impl DaemonTool for WaitForApproval {
    fn name(&self) -> &'static str {
        "wait_for_approval"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<ApprovalWaitArgs>(
            self.name(),
            "Use when a sign-in page waits for the person to approve it on their phone (a push notification in the site's app, \"check your phone\", a number to tap). The person sees an \"Approve on your phone\" card that waits live; the daemon watches the page and returns once it moves on, so do not poll, reload or click anything meanwhile. Result {\"status\":\"done\",\"next\":…} (follow `next` as for request_login; a denied or timed-out approval is next=error with error_text), {\"status\":\"rejected\"} when the person cancelled, or {\"status\":\"expired\"}.",
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
            let args: ApprovalWaitArgs = parse(args)?;
            if args.tab_id.trim().is_empty() {
                return Err(ToolError("tab_id must not be empty".into()));
            }
            browse_action(ctx, &args.tab_id).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"tab_id":args["tab_id"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: ApprovalWaitArgs = parse(&args)?;
                let origin = ctx
                    .approved_actions
                    .iter()
                    .find(|action| action.capability == Capability::Browse)
                    .map(|action| action.target.clone())
                    .ok_or_else(|| ToolError("Missing approved tool target".into()))?;
                let browser = ctx.shared.registry.browser.for_tab(&ctx.shared, &ctx.session, &args.tab_id).await?;
                let owner = ctx.session.as_str();
                let tab = args.tab_id.as_str();
                if Controller::origin(&browser, &ctx.session, tab).await? != origin {
                    return Err(ToolError("Page origin changed; take a fresh snapshot".into()));
                }
                let (data, _) = browser
                    .locate(owner, tab, &located_script(APPROVAL_JS))
                    .await
                    .map_err(browser_error)?;
                let found: FoundApproval = serde_json::from_value(data)
                    .map_err(|_| ToolError("Approval page could not be read".into()))?;
                if !found.found {
                    return Ok(ToolExec::Denied {
                        reason: "approval_not_found".into(),
                        hint: Some("This page does not ask to approve the sign-in on a phone. Take a snapshot; for a code field use request_otp, for a choice request_choice.".into()),
                    });
                }
                let site_icon = crate::favicons::for_tab(&browser, owner, tab, &origin).await;
                let site_name = if found.site_name.trim().is_empty() {
                    origin.clone()
                } else {
                    found.site_name
                };
                Ok(ToolExec::Defer(Box::new(TicketRequest { hire: None, form: None, kind: TicketKind::DeviceApproval,
                summary: format!("Approve the sign-in to {} on your phone", origin.split_once("://").map_or(origin.as_str(), |(_, host)| host)),
                instructions: Some(format!("Approve the sign-in for {} in the site's app on your phone. openside continues by itself once the page moves on.", origin.split_once("://").map_or(origin.as_str(), |(_, host)| host))),
                options: Vec::new(),
                target: Some(TicketTarget {
                    tab_id: Some(args.tab_id),
                    origin: Some(origin.clone()),
                    field_ref: None,
                }),
                resume_when: None,
                otp: None,
                otp_register: None,
                login: None,
                choice: None,
                approval: Some(TicketApproval {
                    origin,
                    site_name,
                    icon: site_icon,
                    message: found.message,
                    number: found.number.filter(|number| {
                        !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                    }),
                }),
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

/// The outcome of a phone-approval wait once its page moved on, for the browser poll; `None`
/// while the page still waits (or cannot be read mid-navigation). The caller holds the
/// browser gate.
pub(crate) async fn approval_outcome(
    shared: &Shared,
    browser: &Browser,
    session: &SessionId,
    tab: &str,
) -> Option<Value> {
    let owner = session.as_str();
    let reading = browser
        .evaluate_isolated(owner, tab, &sync_script(NEXT_STEP_JS))
        .await
        .ok()?;
    if reading["step"] == "push_approval" {
        return None;
    }
    let next = next_step(browser, owner, tab, None).await;
    if next.0 == Next::Done {
        let _ = crate::connections::complete_login(
            shared,
            browser,
            session,
            tab,
            openside_proto::ConnectionSource::Login,
            None,
        )
        .await;
    }
    Some(with_next(json!({"status":"done"}), next))
        .filter(|result| result["next"] != "push_approval")
}

/// A person's answer to a `login` ticket.
pub(crate) enum LoginAnswer {
    /// Typed values by field id; text values are already masked everywhere.
    Values {
        text: Vec<(String, Zeroizing<String>)>,
        checked: Vec<(String, bool)>,
        save: bool,
    },
    /// A saved account offered on the ticket.
    Account(String),
    /// An alternative sign-in control on the ticket.
    Alternative(String),
}

/// Parse a `login` answer against its ticket. Every text value gets a redactor before
/// anything else happens, so not even an error path can echo it.
pub(crate) fn parse_login_answer(
    shared: &Shared,
    ticket: &Ticket,
    value: Value,
) -> Result<LoginAnswer, HttpError> {
    let login = ticket.login.as_ref().ok_or_else(HttpError::invalid_value)?;
    let Value::Object(mut object) = value else {
        return Err(HttpError::invalid_value());
    };
    if let Some(Value::Object(values)) = object.get("values") {
        let secrets: Vec<Secret> = values
            .values()
            .filter_map(|value| value.as_str())
            .filter(|text| !text.is_empty())
            .map(|text| Secret::new(text.to_owned()))
            .collect();
        if !secrets.is_empty() {
            shared
                .registry
                .redactors
                .write()
                .push(Redactor::new(&secrets));
        }
    }
    if object.len() == 1 {
        if let Some(Value::String(item)) = object.get("account") {
            if !login
                .accounts
                .iter()
                .flatten()
                .any(|account| &account.item_id == item)
            {
                return Err(HttpError::invalid_value());
            }
            return Ok(LoginAnswer::Account(item.clone()));
        }
        if let Some(Value::String(id)) = object.get("alternative") {
            if !login
                .alternatives
                .iter()
                .any(|alternative| &alternative.id == id)
            {
                return Err(HttpError::invalid_value());
            }
            return Ok(LoginAnswer::Alternative(id.clone()));
        }
    }
    let save = match object.remove("save") {
        None => false,
        Some(Value::Bool(save)) => save,
        Some(_) => return Err(HttpError::invalid_value()),
    };
    let (Some(Value::Object(values)), true) = (object.remove("values"), object.is_empty()) else {
        return Err(HttpError::invalid_value());
    };
    let mut text = Vec::new();
    let mut checked = Vec::new();
    for (id, value) in values {
        let field = login
            .fields
            .iter()
            .find(|field| field.id == id)
            .ok_or_else(HttpError::invalid_value)?;
        match (field.kind, value) {
            (LoginFieldKind::Checkbox, Value::Bool(on)) => checked.push((id, on)),
            (LoginFieldKind::Checkbox, _) | (_, Value::Bool(_)) => {
                return Err(HttpError::invalid_value());
            }
            (_, Value::String(value)) => text.push((id, Zeroizing::new(value))),
            _ => return Err(HttpError::invalid_value()),
        }
    }
    let filled = |kind: LoginFieldKind| {
        login
            .fields
            .iter()
            .filter(|field| field.kind == kind)
            .all(|field| {
                text.iter()
                    .any(|(id, value)| id == &field.id && !value.is_empty())
            })
    };
    if !filled(LoginFieldKind::Identifier) || !filled(LoginFieldKind::Password) {
        return Err(HttpError::invalid_value());
    }
    if save && !login.can_save {
        return Err(HttpError::invalid_value());
    }
    Ok(LoginAnswer::Values {
        text,
        checked,
        save,
    })
}

/// The outcome of a `login` ticket whose earlier answer kept the login in the vault and then
/// never committed: the sign-in was submitted before the save, so it is reported as such.
/// `None` when nothing was saved (the caller then refuses a blind retry while an intent is
/// left) or nothing can be checked.
pub(crate) async fn interrupted(shared: &Shared, ticket: &Ticket) -> Option<Value> {
    let operation = shared
        .registry
        .browser
        .fill_intent(shared, &ticket.id)
        .await
        .ok()??
        .registration?;
    let guard = shared.registry.vault.vault.lock().await;
    guard
        .as_ref()?
        .registered_by(&operation)
        .ok()?
        .then(|| json!({"status":"submitted","saved":true,"next":"unknown"}))
}

/// Error codes of a `login` or `choice` answer: a page that moved on is `target_unavailable`
/// (or `origin_mismatch`), so the card can tell the person to ask the agent again; the rest
/// map as for secret input.
fn refused(code: &str) -> HttpError {
    match code {
        "target_unavailable" | "target_missing" => {
            HttpError(StatusCode::CONFLICT, "target_unavailable")
        }
        "takeover_active" => HttpError(StatusCode::CONFLICT, "takeover_active"),
        code => crate::api::refused(code),
    }
}

fn http(error: SecretSinkError) -> HttpError {
    refused(error.code)
}

/// Where a `login` or `choice` answer acts: the ticket's tab, still showing the pinned
/// document to this session without a takeover, and its pinned nodes. The caller holds the
/// browser gate.
pub(super) async fn pinned_target(
    shared: &Shared,
    ticket: &Ticket,
) -> Result<(std::sync::Arc<Browser>, String, Vec<(String, NodeHandle)>), HttpError> {
    let controller = &shared.registry.browser;
    let target = ticket
        .target
        .as_ref()
        .ok_or_else(HttpError::invalid_value)?;
    controller
        .ticket_target_current(shared, &ticket.id, &ticket.session_id, target)
        .await
        .map_err(http)?;
    let pins = controller
        .pinned(&ticket.id, &ticket.session_id, target)
        .await
        .ok_or_else(|| refused("target_unavailable"))?;
    let tab = target.tab_id.clone().ok_or_else(HttpError::invalid_value)?;
    let browser = controller
        .target_browser(shared, &ticket.session_id, &tab)
        .await
        .map_err(http)?;
    Ok((browser, tab, pins))
}

/// A browser error before anything was written (the node or document is gone) leaves the
/// ticket pending with a clean slate; any other may have reached the page.
fn untouched(error: &BrowserError) -> bool {
    matches!(
        error,
        BrowserError::StaleReference
            | BrowserError::OriginMismatch
            | BrowserError::InvalidAction(_)
    )
}

/// Apply a `login` answer. The caller holds the ticket gate, checked the scopes and the live
/// policy, and found no interrupted earlier answer. Errors leave the ticket pending.
pub(crate) async fn resolve_login(
    shared: &Shared,
    ticket: &Ticket,
    answer: LoginAnswer,
    who: &str,
) -> Result<Value, HttpError> {
    let login = ticket.login.as_ref().ok_or_else(HttpError::invalid_value)?;
    let target = ticket
        .target
        .as_ref()
        .ok_or_else(HttpError::invalid_value)?;
    let controller = &shared.registry.browser;
    let _gate = controller.gate.lock().await;
    let (browser, tab, pins) = pinned_target(shared, ticket).await?;
    let owner = ticket.session_id.as_str();
    let form = Form::from_pins(login, &pins);
    let before = document_before(&browser, &tab).await;
    let result = match answer {
        LoginAnswer::Alternative(id) => {
            let node = pins
                .iter()
                .find(|(name, _)| name == &id)
                .map(|(_, node)| node)
                .ok_or_else(|| refused("target_unavailable"))?;
            browser
                .click_node(owner, &tab, node)
                .await
                .map_err(|_| refused("target_unavailable"))?;
            json!({"status":"alternative"})
        }
        LoginAnswer::Account(site) => {
            controller
                .mark_ticket_secret(shared, &ticket.id, &ticket.session_id, target)
                .await
                .map_err(http)?;
            controller
                .record_fill_intent(shared, &ticket.id, None)
                .await?;
            let audit = AuditContext {
                who,
                session_id: Some(owner),
                tool_call_id: Some(&ticket.tool_call_id),
            };
            let outcome = fill_account(
                shared,
                &browser,
                &ticket.session_id,
                &tab,
                &login.origin,
                &form,
                &site,
                &audit,
            )
            .await;
            match outcome {
                Ok(LoginOutcome::Filled) => json!({"status":"filled_from_vault","item_id":site}),
                Ok(LoginOutcome::Denied(_)) => {
                    let _ = controller.clear_fill_intent(shared, &ticket.id).await;
                    return Err(refused("vault_denied"));
                }
                Ok(LoginOutcome::FillFailed) | Err(_) => {
                    return Err(refused("fill_failed"));
                }
            }
        }
        LoginAnswer::Values {
            text,
            checked,
            save,
        } => {
            controller
                .mark_ticket_secret(shared, &ticket.id, &ticket.session_id, target)
                .await
                .map_err(http)?;
            // The save comes after the submit; its id lets a recovery tell whether it happened.
            let operation = crate::persistence::random_id();
            controller
                .record_fill_intent(shared, &ticket.id, save.then_some(operation.as_str()))
                .await?;
            let mut written = false;
            let filled = async {
                for (id, value) in &text {
                    let node = pins
                        .iter()
                        .find(|(name, _)| name == id)
                        .map(|(_, node)| node)
                        .ok_or(BrowserError::StaleReference)?;
                    Controller::fill_node(&browser, owner, &tab, node, value).await?;
                    written = true;
                }
                for (id, on) in &checked {
                    if let Some((_, node)) = pins.iter().find(|(name, _)| name == id) {
                        browser.set_checked_node(owner, &tab, node, *on).await?;
                    }
                }
                submit(&browser, owner, &tab, &form).await
            }
            .await;
            if let Err(error) = filled {
                if !written && untouched(&error) {
                    let _ = controller.clear_fill_intent(shared, &ticket.id).await;
                    return Err(refused("target_unavailable"));
                }
                let _ = controller.persist(shared).await;
                return Err(refused("fill_failed"));
            }
            let next = next_step(&browser, owner, &tab, before.as_deref()).await;
            if form.password.is_none() && next.0 == Next::Login {
                let typed = login
                    .fields
                    .iter()
                    .find(|field| field.kind == LoginFieldKind::Identifier)
                    .and_then(|field| text.iter().find(|(id, _)| id == &field.id));
                if let Some((_, name)) = typed {
                    ACCOUNT_STEP.lock().insert(
                        (ticket.session_id.clone(), login.origin.clone()),
                        (name.clone(), Instant::now()),
                    );
                }
            }
            // An unchanged form, CAPTCHA or another login page does not prove acceptance.
            let (saved, item) = if save
                && matches!(
                    next.0,
                    Next::Done | Next::Otp | Next::Choice | Next::PushApproval | Next::EnrollTotp
                ) {
                save_login(shared, ticket, login, &text, &operation, who).await
            } else {
                (false, None)
            };
            let _ = controller.persist(shared).await;
            if let Some(item) = &item {
                crate::connections::remember_login_item(shared, &ticket.session_id, &tab, item)
                    .await;
            }
            if next.0 == Next::Done {
                let _ = crate::connections::complete_login(
                    shared,
                    &browser,
                    &ticket.session_id,
                    &tab,
                    openside_proto::ConnectionSource::Login,
                    item.as_deref(),
                )
                .await;
            }
            let mut result = with_next(json!({"status":"submitted"}), next);
            if save {
                result["saved"] = json!(saved);
            }
            return Ok(result);
        }
    };
    let _ = controller.persist(shared).await;
    if let Some(item) = result.get("item_id").and_then(Value::as_str) {
        crate::connections::remember_login_item(shared, &ticket.session_id, &tab, item).await;
    }
    let next = next_step(&browser, owner, &tab, before.as_deref()).await;
    if next.0 == Next::Done {
        let _ = crate::connections::complete_login(
            shared,
            &browser,
            &ticket.session_id,
            &tab,
            openside_proto::ConnectionSource::Login,
            result.get("item_id").and_then(Value::as_str),
        )
        .await;
    }
    Ok(with_next(result, next))
}

pub(super) async fn complete_vault_login(
    shared: &Shared,
    browser: &Browser,
    session: &SessionId,
    tab: &str,
    before: Option<&str>,
    item: &str,
) {
    if next_step(browser, session.as_str(), tab, before).await.0 == Next::Done {
        let _ = crate::connections::complete_login(
            shared,
            browser,
            session,
            tab,
            openside_proto::ConnectionSource::Vault,
            Some(item),
        )
        .await;
    }
}

/// An incomplete completion audit still saved the item, but does not return its id.
async fn save_login(
    shared: &Shared,
    ticket: &Ticket,
    login: &TicketLogin,
    text: &[(String, Zeroizing<String>)],
    operation: &str,
    who: &str,
) -> (bool, Option<String>) {
    let value = |kind: LoginFieldKind| {
        login
            .fields
            .iter()
            .find(|field| field.kind == kind)
            .and_then(|field| text.iter().find(|(id, _)| id == &field.id))
            .map(|(_, value)| Secret::new(value.as_str().to_owned()))
    };
    let step = (ticket.session_id.clone(), login.origin.clone());
    let (Some(username), Some(password)) = (
        value(LoginFieldKind::Identifier).or_else(|| {
            account_step(&ticket.session_id, &login.origin)
                .map(|name| Secret::new(name.as_str().to_owned()))
        }),
        value(LoginFieldKind::Password),
    ) else {
        return (false, None);
    };
    ACCOUNT_STEP.lock().remove(&step);
    let mut guard = shared.registry.vault.vault.lock().await;
    let Some(vault) = guard.as_mut() else {
        return (false, None);
    };
    let audit = AuditContext {
        who,
        session_id: Some(ticket.session_id.as_str()),
        tool_call_id: Some(&ticket.tool_call_id),
    };
    match vault.save_login(&audit, &login.origin, username, password, operation) {
        Ok(Ok(saved)) => (true, Some(saved.site)),
        Err(openside_vault::Error::AuditIncomplete) => (true, None),
        Ok(Err(_)) | Err(_) => (false, None),
    }
}

/// Apply a `choice` answer: click the picked option's pinned node. The caller holds the
/// ticket gate and checked the live policy. Errors leave the ticket pending.
pub(crate) async fn resolve_choice(
    shared: &Shared,
    ticket: &Ticket,
    value: Value,
) -> Result<Value, HttpError> {
    let choice = ticket
        .choice
        .as_ref()
        .ok_or_else(HttpError::invalid_value)?;
    let Value::Object(object) = value else {
        return Err(HttpError::invalid_value());
    };
    let (Some(Value::String(id)), 1) = (object.get("option"), object.len()) else {
        return Err(HttpError::invalid_value());
    };
    if !choice.options.iter().any(|option| &option.id == id) {
        return Err(HttpError::invalid_value());
    }
    let _gate = shared.registry.browser.gate.lock().await;
    let (browser, tab, pins) = pinned_target(shared, ticket).await?;
    let owner = ticket.session_id.as_str();
    let node = pins
        .iter()
        .find(|(name, _)| name == id)
        .map(|(_, node)| node)
        .ok_or_else(|| refused("target_unavailable"))?;
    browser
        .click_node(owner, &tab, node)
        .await
        .map_err(|_| refused("target_unavailable"))?;
    let next = next_step(&browser, owner, &tab, None).await;
    if next.0 == Next::Done {
        let _ = crate::connections::complete_login(
            shared,
            &browser,
            &ticket.session_id,
            &tab,
            openside_proto::ConnectionSource::Login,
            None,
        )
        .await;
    }
    Ok(with_next(json!({"status":"chosen","option":id}), next))
}
