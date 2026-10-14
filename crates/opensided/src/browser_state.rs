//! Live per-tab browser state for the chat (`browser.state` events), the tab ledger that keeps
//! parallel Tasks out of each other's tabs, and browser prewarming.
//!
//! Every label in a state is read from the page's accessibility tree; nothing is model text,
//! so a card cannot claim progress the page does not show. Values are never read: a typed
//! code or password cannot reach the event log through this path.

use std::collections::HashMap;
use std::sync::Arc;

use openside_browser::{Browser, SnapshotNode};
use openside_core::{ToolCall, ToolOutcome};
use openside_proto::{
    BrowserActionState, BrowserPhase, BrowserState, EventKind, PageFacts, PageField, PageKind,
    SessionId, Subagent,
};
use serde_json::Value;

use crate::{engine::Shared, persistence::Result};

const MAX_FIELDS: usize = 8;
const MAX_LABEL_CHARS: usize = 80;
const MAX_ERROR_CHARS: usize = 200;
const LIST_MIN_ITEMS: usize = 5;
const ARTICLE_MIN_TEXT: usize = 1500;

/// Tools that change a tab. A Task may only use them on tabs it opened or that no other
/// live Task holds.
const ACTING_TOOLS: &[&str] = &[
    "browser_navigate",
    "browser_act",
    "browser_pointer",
    "browser_repl",
    "browser_close_tab",
    "browser_type",
    "connection_fetch",
    "vault_login",
    "vault_otp",
    "vault_passkey",
    "request_login",
    "request_choice",
    "request_otp",
    "register_authenticator",
    "request_human_step",
    "wait_for_approval",
];

#[derive(Default)]
struct Generation {
    epoch: u64,
    terminal: bool,
}

/// Which Task holds which agent tab, per session. A tab is released when its Task ends.
#[derive(Default)]
pub(crate) struct TabLedger {
    owners: parking_lot::Mutex<HashMap<(SessionId, String), String>>,
    // Admission is released at Task termination, but already queued events must not fall
    // through to the front agent when their consumer catches up.
    event_owners: parking_lot::Mutex<HashMap<(SessionId, String), String>>,
    download_owners: parking_lot::Mutex<HashMap<(SessionId, String), String>>,
    generations: tokio::sync::Mutex<HashMap<(SessionId, Option<String>), Generation>>,
    #[cfg(test)]
    pub(crate) probe_gate: tokio::sync::Mutex<()>,
    #[cfg(test)]
    pub(crate) probe_started: tokio::sync::Notify,
    #[cfg(test)]
    pub(crate) probe_finished: tokio::sync::Notify,
}

impl TabLedger {
    pub(crate) fn holder(&self, session: &SessionId, tab: &str) -> Option<String> {
        self.owners
            .lock()
            .get(&(session.clone(), tab.to_owned()))
            .cloned()
    }
    pub(crate) fn claim(&self, session: &SessionId, tab: &str, task: &str) {
        self.event_owners
            .lock()
            .insert((session.clone(), tab.to_owned()), task.to_owned());
        self.owners
            .lock()
            .insert((session.clone(), tab.to_owned()), task.to_owned());
    }
    fn release(&self, session: &SessionId, task: &str) -> Vec<String> {
        let mut ledger = self.owners.lock();
        let tabs: Vec<String> = ledger
            .iter()
            .filter(|((owner, _), holder)| owner == session && holder.as_str() == task)
            .map(|((_, tab), _)| tab.clone())
            .collect();
        for tab in &tabs {
            ledger.remove(&(session.clone(), tab.clone()));
        }
        tabs
    }
    /// A kept tab no longer belongs to an ended Task, so its later events must reach the
    /// front agent that now works on it instead of a Task that can no longer be notified.
    fn forget_event_owner(&self, session: &SessionId, tab: &str) {
        self.event_owners
            .lock()
            .remove(&(session.clone(), tab.to_owned()));
    }
    pub(crate) fn event_owner(
        &self,
        session: &SessionId,
        event: &openside_browser::BrowserEvent,
    ) -> Option<String> {
        use openside_browser::BrowserEvent;
        let key = |id: &str| (session.clone(), id.to_owned());
        match event {
            BrowserEvent::PopupOpened { tab, opener_id } => {
                let mut owners = self.event_owners.lock();
                let task = owners.get(&key(opener_id)).cloned()?;
                owners.insert(key(&tab.id), task.clone());
                Some(task)
            }
            BrowserEvent::TabClosed { tab, .. } => self.event_owners.lock().remove(&key(&tab.id)),
            BrowserEvent::DialogOpened { tab_id, .. } => {
                self.event_owners.lock().get(&key(tab_id)).cloned()
            }
            BrowserEvent::DownloadStarted { guid, tab_id, .. } => {
                let task = self
                    .event_owners
                    .lock()
                    .get(&key(tab_id.as_deref()?))
                    .cloned()?;
                self.download_owners.lock().insert(key(guid), task.clone());
                Some(task)
            }
            BrowserEvent::DownloadFinished { guid, .. } => {
                self.download_owners.lock().remove(&key(guid))
            }
            _ => None,
        }
    }
}

fn tab_arg(call: &ToolCall) -> Option<String> {
    serde_json::from_str::<Value>(&call.arguments_json)
        .ok()?
        .get("tab_id")?
        .as_str()
        .map(str::to_owned)
}

/// A Task acting on a tab another live Task holds is refused; the first Task to act on an
/// unheld tab claims it. The front agent and single-agent sessions are not limited here.
pub(crate) async fn tab_denial(
    shared: &Shared,
    session: &SessionId,
    call: &ToolCall,
) -> Result<Option<(&'static str, &'static str)>> {
    let (Some(task), true) = (&call.subagent, ACTING_TOOLS.contains(&call.name.as_str())) else {
        return Ok(None);
    };
    let Some(tab) = tab_arg(call) else {
        return Ok(None);
    };
    let ledger = &shared.registry.browser.task_tabs;
    let key = (session.clone(), tab.clone());
    loop {
        let observed = ledger.holder(session, &tab);
        if let Some(holder) = observed.as_ref().filter(|holder| *holder != task) {
            if crate::subagents::get(shared, session, holder)
                .await?
                .is_some_and(|owner| !owner.status.is_terminal())
            {
                return Ok(Some((
                    "tab_owned_by_task",
                    "Another Task is working in this tab; open your own tab with browser_open_tab",
                )));
            }
        }
        let claimed = {
            let mut held = ledger.owners.lock();
            // Reserve only the owner whose liveness was checked.
            if held.get(&key) == observed.as_ref() {
                held.insert(key.clone(), task.clone());
                true
            } else {
                false
            }
        };
        if claimed {
            shared.registry.browser.retire_warm(session, &tab);
            shared.registry.browser.persist(shared).await?;
            return Ok(None);
        }
    }
}

/// The Task whose card a call on `tab` belongs to: the caller, or for the front agent's
/// reads, the Task that holds the tab.
fn card_owner(shared: &Shared, session: &SessionId, call: &ToolCall, tab: &str) -> Option<String> {
    call.subagent
        .clone()
        .or_else(|| shared.registry.browser.task_tabs.holder(session, tab))
}

/// Close unreferenced agent tabs when a Task ends; keep evidence needed by Cards or people.
pub(crate) async fn release_ended(shared: &Shared, session: &SessionId, tasks: &[Subagent]) {
    for task in tasks.iter().filter(|task| task.status.is_terminal()) {
        let ledger = &shared.registry.browser.task_tabs;
        shared.host_calls.cancel(session, &task.id).await;
        let _gate = shared.registry.browser.gate.lock().await;
        let released = ledger.release(session, &task.id);
        if released.is_empty() {
            continue;
        }
        let stamp = finish(shared, session, &task.id).await;
        for tab in released {
            let Some(browser) = shared.registry.browser.tab_browser(session, &tab).await else {
                ledger.claim(session, &tab, &task.id);
                continue;
            };
            let Ok(session_state) = shared.db.state(session).await else {
                ledger.claim(session, &tab, &task.id);
                continue;
            };
            let referenced = session_state.tickets.values().any(|ticket| {
                ticket
                    .target
                    .as_ref()
                    .and_then(|target| target.tab_id.as_deref())
                    == Some(tab.as_str())
                    && (ticket.status == openside_proto::TicketStatus::Pending
                        || ticket.card.is_some()
                        || session_state
                            .cards
                            .values()
                            .any(|card| card.ticket_id.as_ref() == Some(&ticket.id)))
            });
            if !referenced {
                // The browser rejects person tabs even if a stale ledger claims ownership.
                if let Err(error) = browser
                    .close_agent_tab_silently(session.as_str(), &tab)
                    .await
                {
                    ledger.claim(session, &tab, &task.id);
                    crate::diag!("opensided: task tab cleanup failed: {error}");
                }
                continue;
            }
            ledger.forget_event_owner(session, &tab);
            if let Some(state) =
                observe(shared, &browser, session, &tab, Some(task.id.clone())).await
            {
                emit_current(
                    shared,
                    session,
                    &stamp,
                    BrowserState {
                        phase: BrowserPhase::Done,
                        ..state
                    },
                    true,
                )
                .await;
            }
        }
        let _ = shared.registry.browser.persist(shared).await;
    }
}

fn phrase(tool: &str) -> Option<(&'static str, BrowserPhase)> {
    Some(match tool {
        "browser_open_tab" => ("Opening", BrowserPhase::Loading),
        "browser_navigate" => ("Going to", BrowserPhase::Loading),
        "browser_snapshot" | "browser_screenshot" | "browser_read" => {
            ("Reading the page", BrowserPhase::Reading)
        }
        "browser_act" | "browser_pointer" | "browser_type" => ("Working on", BrowserPhase::Acting),
        "browser_repl" => ("Working in the page", BrowserPhase::Acting),
        "request_login" | "vault_login" | "vault_passkey" => ("Signing in", BrowserPhase::Acting),
        "request_otp" | "vault_otp" | "register_authenticator" => {
            ("Verifying", BrowserPhase::Acting)
        }
        "request_choice" => ("Choosing", BrowserPhase::Acting),
        "request_human_step" | "wait_for_approval" => {
            ("Waiting for you", BrowserPhase::WaitingHuman)
        }
        _ => return None,
    })
}
pub(crate) type ProbeBinding = Arc<parking_lot::Mutex<Option<ProbeStamp>>>;

pub(crate) struct ProbeStamp {
    task: Option<String>,
    generation: u64,
}

async fn advance(shared: &Shared, session: &SessionId, task: Option<String>) -> ProbeStamp {
    let mut generations = shared.registry.browser.task_tabs.generations.lock().await;
    let generation = generations
        .entry((session.clone(), task.clone()))
        .or_default();
    if !generation.terminal {
        generation.epoch += 1;
    }
    ProbeStamp {
        task,
        generation: generation.epoch,
    }
}

pub(crate) async fn finish(shared: &Shared, session: &SessionId, task: &str) -> ProbeStamp {
    let mut generations = shared.registry.browser.task_tabs.generations.lock().await;
    let task = Some(task.to_owned());
    let generation = generations
        .entry((session.clone(), task.clone()))
        .or_default();
    if !generation.terminal {
        generation.epoch += 1;
        generation.terminal = true;
    }
    ProbeStamp {
        task,
        generation: generation.epoch,
    }
}

/// The state shown while a browser call runs, for calls on a known tab. `None` for a call
/// that cannot publish any `browser.state` — its `after` returns early — so it must not
/// consume the probe generation either: an epoch taken without an emission fences out a
/// still-running `after` probe of the previous call, and nothing replaces the lost state.
pub(crate) async fn before(
    shared: &Shared,
    session: &SessionId,
    call: &ToolCall,
) -> Option<ProbeStamp> {
    let (verb, phase) = phrase(&call.name)?;
    let args: Value = serde_json::from_str(&call.arguments_json).unwrap_or(Value::Null);
    let target_host = args["url"]
        .as_str()
        .and_then(|url| url::Url::parse(url).ok())
        .and_then(|url| url.host_str().map(str::to_owned));
    let tab = args["tab_id"].as_str();
    let owner = tab
        .and_then(|tab| card_owner(shared, session, call, tab))
        .or_else(|| call.subagent.clone());
    if tab.is_none() && call.name != "browser_open_tab" {
        return None;
    }
    let stamp = advance(shared, session, owner.clone()).await;
    let Some(tab) = tab else {
        return Some(stamp);
    };
    let Some(browser) = shared.registry.browser.tab_browser(session, tab).await else {
        return Some(stamp);
    };
    let Some(mut state) = current(&browser, tab, owner).await else {
        return Some(stamp);
    };
    // The element label comes from the tab's last snapshot, so it is the page's own text.
    let target = match args["target"].as_str().or(args["ref"].as_str()) {
        Some(reference) => shared
            .registry
            .browser
            .label(tab, reference)
            .await
            .map(|label| clip(&label, MAX_LABEL_CHARS)),
        None => None,
    };
    state.phase = phase;
    state.action = Some(BrowserActionState {
        tool: call.name.clone(),
        tool_call_id: call.tool_call_id.clone(),
        phrase: match (&target_host, &target) {
            (Some(host), _) => format!("{verb} {host}"),
            (None, Some(label)) => format!("{verb} “{label}”"),
            (None, None) => verb.to_owned(),
        },
        target,
    });
    emit_current(shared, session, &stamp, state, false).await;
    Some(stamp)
}

/// The page after a browser call, classified. Runs off the call path: the tool result
/// never waits for the probe.
pub(crate) fn after(
    shared: &Shared,
    session: &SessionId,
    call: &ToolCall,
    outcome: &ToolOutcome,
    stamp: Option<ProbeStamp>,
) {
    let Some(stamp) = stamp else {
        return;
    };
    let Some((verb, _)) = phrase(&call.name) else {
        return;
    };
    let tab = match outcome {
        ToolOutcome::Done { for_model_json }
        | ToolOutcome::DoneWithParts { for_model_json, .. } => {
            serde_json::from_str::<Value>(for_model_json)
                .ok()
                .and_then(|result| result["tab_id"].as_str().map(str::to_owned))
                .or_else(|| tab_arg(call))
        }
        _ => tab_arg(call),
    };
    let Some(tab) = tab else {
        return;
    };
    let phase = match outcome {
        ToolOutcome::Deferred { .. } => BrowserPhase::WaitingHuman,
        ToolOutcome::Denied { .. } | ToolOutcome::Failed { .. } => BrowserPhase::Error,
        _ => BrowserPhase::Reading,
    };
    let error = match outcome {
        ToolOutcome::Failed { message } => Some(message.clone()),
        ToolOutcome::Denied { reason, .. } => Some(reason.clone()),
        _ => None,
    };
    let task = stamp.task.clone();
    let shared = shared.clone();
    let session = session.clone();
    let action = BrowserActionState {
        tool: call.name.clone(),
        tool_call_id: call.tool_call_id.clone(),
        phrase: verb.to_owned(),
        target: None,
    };
    tokio::spawn(async move {
        #[cfg(test)]
        let _probe_gate = {
            let ledger = &shared.registry.browser.task_tabs;
            ledger.probe_started.notify_one();
            ledger.probe_gate.lock().await
        };
        #[cfg(test)]
        let _finished = {
            struct Finished<'a>(&'a tokio::sync::Notify);
            impl Drop for Finished<'_> {
                fn drop(&mut self) {
                    self.0.notify_one();
                }
            }
            Finished(&shared.registry.browser.task_tabs.probe_finished)
        };
        let mut state = {
            let _gate = shared.registry.browser.gate.lock().await;
            if !shared.registry.browser.available() {
                return;
            }
            let Some(browser) = shared.registry.browser.tab_browser(&session, &tab).await else {
                return;
            };
            let Some(state) = observe(&shared, &browser, &session, &tab, task).await else {
                return;
            };
            state
        };
        state.phase = phase;
        state.action = Some(action);
        if let Some(error) = error {
            state
                .facts
                .error_text
                .get_or_insert(clip(&error, MAX_ERROR_CHARS));
        }
        state.thumbnail = crate::preview::capture_thumbnail(&shared, &session, &tab).await;
        emit_current(&shared, &session, &stamp, state, false).await;
    });
}

async fn emit_current(
    shared: &Shared,
    session: &SessionId,
    stamp: &ProbeStamp,
    state: BrowserState,
    terminal: bool,
) {
    if state.task_id != stamp.task {
        return;
    }
    let ledger = &shared.registry.browser.task_tabs;
    // Hold the generation fence through durable emission, not merely through validation.
    let generations = ledger.generations.lock().await;
    if !generations
        .get(&(session.clone(), stamp.task.clone()))
        .is_some_and(|generation| {
            generation.epoch == stamp.generation && generation.terminal == terminal
        })
    {
        return;
    }
    if let Some(task) = &stamp.task {
        let holder = ledger.holder(session, &state.tab_id);
        if terminal {
            if holder.is_some() {
                return;
            }
        } else if holder.as_ref() != Some(task)
            || !crate::subagents::is_live(shared, session, task)
                .await
                .unwrap_or(false)
        {
            return;
        }
    }
    emit(shared, session, state).await;
}

async fn emit(shared: &Shared, session: &SessionId, state: BrowserState) {
    // Page labels can echo a secret the person typed elsewhere; the registry's redactors
    // cover every event payload the same way.
    let Ok(value) = serde_json::to_value(&state) else {
        return;
    };
    let Ok(value) = shared.registry.redact(value) else {
        return;
    };
    let Ok(state) = serde_json::from_value::<BrowserState>(value) else {
        return;
    };
    let _ = shared
        .emit(session, None, EventKind::BrowserState(Box::new(state)))
        .await;
}

/// URL and title only; cheap enough for the state shown before a call.
async fn current(browser: &Arc<Browser>, tab: &str, task: Option<String>) -> Option<BrowserState> {
    let info = browser
        .tabs()
        .await
        .into_iter()
        .find(|info| info.id == tab)?;
    let url = crate::browser::display_url(&info.url);
    Some(BrowserState {
        task_id: task,
        tab_id: tab.to_owned(),
        host: host(&url),
        url,
        title: clip(&info.title, MAX_LABEL_CHARS),
        favicon: None,
        phase: BrowserPhase::Reading,
        action: None,
        page_kind: PageKind::Other,
        facts: PageFacts::default(),
        thumbnail: None,
    })
}

/// URL, title and the classified accessibility tree.
async fn observe(
    shared: &Shared,
    browser: &Arc<Browser>,
    session: &SessionId,
    tab: &str,
    task: Option<String>,
) -> Option<BrowserState> {
    let mut state = current(browser, tab, task).await?;
    if let Ok(url) = url::Url::parse(&state.url) {
        state.favicon = crate::favicons::for_tab(
            browser,
            session.as_str(),
            tab,
            &url.origin().ascii_serialization(),
        )
        .await;
    }
    if let Ok(snapshot) = browser.snapshot(tab).await {
        let (kind, facts) = classify(&snapshot.nodes, &state.url, &state.title);
        state.page_kind = kind;
        state.facts = facts;
        crate::tools::otp::observed(shared, browser, session, tab, &state.url, kind).await;
    }
    Some(state)
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

fn mentions(text: &str, words: &[&str]) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|word| text.contains(word))
}

const PASSWORD_WORDS: [&str; 6] = [
    "password",
    "passwort",
    "mot de passe",
    "contraseña",
    "비밀번호",
    "パスワード",
];
const CODE_WORDS: [&str; 9] = [
    "verification code",
    "one-time",
    "otp",
    "security code",
    "enter code",
    "6-digit",
    "인증번호",
    "인증 코드",
    "確認コード",
];
const CAPTCHA_WORDS: [&str; 5] = [
    "captcha",
    "verify you are human",
    "i'm not a robot",
    "are you a robot",
    "자동입력 방지",
];
const CHECKOUT_WORDS: [&str; 6] = [
    "card number",
    "cvc",
    "cvv",
    "place order",
    "카드 번호",
    "결제하기",
];
const ERROR_WORDS: [&str; 8] = [
    "404",
    "not found",
    "500",
    "something went wrong",
    "access denied",
    "page isn't working",
    "찾을 수 없",
    "오류가 발생",
];

fn input_kind(node: &SnapshotNode) -> Option<&'static str> {
    Some(match node.role.as_str() {
        "textbox" | "searchbox" => {
            if node.value == "<redacted>" || mentions(&node.name, &PASSWORD_WORDS) {
                "password"
            } else if mentions(&node.name, &CODE_WORDS) {
                "code"
            } else if mentions(&node.name, &["email", "e-mail", "이메일"]) {
                "email"
            } else if node.role == "searchbox" {
                "search"
            } else {
                "text"
            }
        }
        "spinbutton" => "number",
        "checkbox" | "switch" => "checkbox",
        "radio" => "radio",
        "combobox" | "listbox" => "select",
        _ => return None,
    })
}

/// Classify a page from its accessibility nodes, URL and title. Names only; never values.
pub(crate) fn classify(nodes: &[SnapshotNode], url: &str, title: &str) -> (PageKind, PageFacts) {
    let heading = nodes
        .iter()
        .find(|node| node.role == "heading" && !node.name.trim().is_empty())
        .map(|node| clip(&node.name, MAX_LABEL_CHARS));
    let inputs: Vec<(&SnapshotNode, &str)> = nodes
        .iter()
        .filter_map(|node| input_kind(node).map(|kind| (node, kind)))
        .collect();
    let fields: Vec<PageField> = inputs
        .iter()
        .filter(|(node, _)| !node.name.trim().is_empty())
        .take(MAX_FIELDS)
        .map(|(node, kind)| PageField {
            label: clip(&node.name, MAX_LABEL_CHARS),
            kind: (*kind).to_owned(),
        })
        .collect();
    let alert = nodes
        .iter()
        .find(|node| node.role == "alert" && !node.name.trim().is_empty())
        .map(|node| clip(&node.name, MAX_ERROR_CHARS));
    let headline = format!("{title}\n{}", heading.as_deref().unwrap_or_default());
    let all_names = || nodes.iter().map(|node| node.name.as_str());
    let has = |kind: &str| inputs.iter().any(|(_, found)| *found == kind);
    let text_inputs = inputs
        .iter()
        .filter(|(_, kind)| matches!(*kind, "text" | "email" | "password" | "code" | "number"))
        .count();
    let options = inputs.iter().filter(|(_, kind)| *kind == "radio").count()
        + nodes.iter().filter(|node| node.role == "option").count();
    let items = nodes
        .iter()
        .filter(|node| {
            matches!(
                node.role.as_str(),
                "listitem" | "row" | "article" | "gridcell"
            )
        })
        .count();
    let text_len: usize = nodes
        .iter()
        .filter(|node| matches!(node.role.as_str(), "StaticText" | "paragraph"))
        .map(|node| node.name.len())
        .sum();

    let kind = if all_names().any(|name| mentions(name, &CAPTCHA_WORDS)) {
        PageKind::Captcha
    } else if has("password") {
        PageKind::Login
    } else if has("code") || (text_inputs >= 4 && fields.is_empty()) {
        PageKind::Otp
    } else if mentions(&headline, &ERROR_WORDS) && text_inputs == 0 {
        PageKind::Error
    } else if all_names().any(|name| mentions(name, &CHECKOUT_WORDS))
        || mentions(url, &["/checkout", "/payment"])
    {
        PageKind::Checkout
    } else if options >= 2 && text_inputs == 0 {
        PageKind::Choice
    } else if text_inputs > 1 || (text_inputs == 1 && !has("search")) {
        PageKind::Form
    } else if items >= LIST_MIN_ITEMS {
        PageKind::List
    } else if nodes.iter().any(|node| node.role == "article") || text_len >= ARTICLE_MIN_TEXT {
        PageKind::Article
    } else {
        PageKind::Other
    };
    let error_text = match kind {
        PageKind::Error => alert.or_else(|| heading.clone()),
        _ => alert,
    };
    let fields = match kind {
        // Options are what the person picks between; name them all, up to the bound.
        PageKind::Choice => nodes
            .iter()
            .filter(|node| matches!(node.role.as_str(), "radio" | "option"))
            .filter(|node| !node.name.trim().is_empty())
            .take(MAX_FIELDS)
            .map(|node| PageField {
                label: clip(&node.name, MAX_LABEL_CHARS),
                kind: if node.role == "radio" {
                    "radio"
                } else {
                    "option"
                }
                .to_owned(),
            })
            .collect(),
        _ => fields,
    };
    let result_count = (kind == PageKind::List).then(|| u32::try_from(items).unwrap_or(u32::MAX));
    (
        kind,
        PageFacts {
            heading,
            fields,
            result_count,
            error_text,
        },
    )
}

/// Launch the browser ahead of a first message.
pub(crate) async fn prewarm_browser(shared: &Shared) {
    if shared.registry.browser.available() {
        let _ = shared.registry.browser.get(shared).await;
    }
}

/// Start the session's sidecar, launch its browser and park one blank agent tab that the
/// session's next `browser_open_tab` adopts.
pub(crate) async fn prewarm_session(shared: &Shared, session: &SessionId) {
    if !shared.db.state(session).await.is_ok_and(|state| {
        !state.deleted && !state.archived && state.status != openside_proto::SessionStatus::Stopped
    }) {
        return;
    }
    let runtime = crate::engine::prewarm_runtime(shared, session);
    let browser = shared.registry.browser.prewarm_tab(shared, session);
    let _ = tokio::join!(runtime, browser);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, name: &str) -> SnapshotNode {
        SnapshotNode {
            reference: None,
            role: role.into(),
            name: name.into(),
            value: String::new(),
            depth: 0,
        }
    }

    #[test]
    fn classifies_login_with_labels_but_never_values() {
        let mut password = node("textbox", "Password");
        password.value = "<redacted>".into();
        let mut email = node("textbox", "Email address");
        email.value = "person@example.test".into();
        let (kind, facts) = classify(
            &[
                node("heading", "Sign in"),
                email,
                password,
                node("button", "Continue"),
            ],
            "https://accounts.example.test/login",
            "Sign in",
        );
        assert_eq!(kind, PageKind::Login);
        assert_eq!(facts.heading.as_deref(), Some("Sign in"));
        assert_eq!(
            facts.fields,
            vec![
                PageField {
                    label: "Email address".into(),
                    kind: "email".into()
                },
                PageField {
                    label: "Password".into(),
                    kind: "password".into()
                },
            ]
        );
        assert!(!serde_json::to_string(&facts).unwrap().contains("person@"));
    }

    #[test]
    fn classifies_otp_from_a_code_field_or_split_boxes() {
        let (kind, _) = classify(
            &[
                node("heading", "Check your phone"),
                node("textbox", "Verification code"),
            ],
            "https://example.test/verify",
            "Verify",
        );
        assert_eq!(kind, PageKind::Otp);
        let boxes: Vec<_> = (0..6).map(|_| node("textbox", "")).collect();
        assert_eq!(
            classify(&boxes, "https://example.test/2fa", "").0,
            PageKind::Otp
        );
    }

    #[test]
    fn classifies_choice_with_every_option_named() {
        let (kind, facts) = classify(
            &[
                node("heading", "Choose an account"),
                node("radio", "Personal"),
                node("radio", "Work"),
                node("button", "Next"),
            ],
            "https://example.test/pick",
            "Pick",
        );
        assert_eq!(kind, PageKind::Choice);
        let labels: Vec<_> = facts
            .fields
            .iter()
            .map(|field| field.label.as_str())
            .collect();
        assert_eq!(labels, ["Personal", "Work"]);
    }

    #[test]
    fn classifies_list_with_a_result_count() {
        let mut nodes = vec![node("heading", "Results"), node("searchbox", "Search")];
        nodes.extend((0..7).map(|index| node("listitem", &format!("Item {index}"))));
        let (kind, facts) = classify(&nodes, "https://shop.example.test/search?q=x", "Search");
        assert_eq!(kind, PageKind::List);
        assert_eq!(facts.result_count, Some(7));
    }

    #[test]
    fn classifies_error_pages_with_their_text() {
        let (kind, facts) = classify(
            &[
                node("heading", "404 — Page not found"),
                node("link", "Home"),
            ],
            "https://example.test/missing",
            "Not found",
        );
        assert_eq!(kind, PageKind::Error);
        assert_eq!(facts.error_text.as_deref(), Some("404 — Page not found"));
        let (kind, facts) = classify(
            &[node("textbox", "Username"), node("alert", "Wrong username")],
            "https://example.test/form",
            "Form",
        );
        assert_eq!(kind, PageKind::Form);
        assert_eq!(facts.error_text.as_deref(), Some("Wrong username"));
    }

    #[test]
    fn ledger_keeps_tasks_out_of_each_others_tabs() {
        let ledger = TabLedger::default();
        let session = SessionId::new("s");
        ledger.claim(&session, "tab-a", "task-1");
        ledger.claim(&session, "tab-b", "task-2");
        assert_eq!(ledger.holder(&session, "tab-a").as_deref(), Some("task-1"));
        assert_eq!(ledger.release(&session, "task-1"), ["tab-a"]);
        assert_eq!(ledger.holder(&session, "tab-a"), None);
        assert_eq!(ledger.holder(&session, "tab-b").as_deref(), Some("task-2"));
    }
}
