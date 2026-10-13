use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use openside_proto::{CreateSessionRequest, EventKind, Ticket, TicketKind, TicketStatus};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{log, ChannelError},
    messenger::Messenger,
    model::{AudioRef, Button, Inbound, MsgRef, Press, ThreadKey},
    store::{Card, Store},
    stt::Stt,
    text::{hex, truncate},
};

const HELP: &str = "openside bot\n\
Send a message to talk to the agent in this thread.\n\
/new - start a new session in this thread\n\
/stop - stop the running session\n\
/model <name> - model for the next session in this thread\n\
/sessions - list sessions\n\
/usage - token usage of this thread's session\n\
/answer <text> - answer a pending free-form question\n\
Where the platform reserves a leading /, use ! instead (for example !stop).";
const NO_STT: &str = "Voice messages need a local speech-to-text command (channels.stt_cmd in the openside config). Audio is never sent to a remote service. Please send text instead.";
const MAX_OPTIONS: usize = 20;

/// Platform-independent behavior knobs.
pub struct CoreOptions {
    /// Minimum delay between edits of a streaming reply.
    pub edit_interval: Duration,
    /// Local speech-to-text hook; `None` refuses voice messages politely.
    pub stt: Option<Stt>,
    /// `remote.public_url`: secret tickets link to their session in the web UI there.
    pub web_url: Option<String>,
}

pub(crate) struct Watcher {
    pub(crate) session: String,
    pub(crate) task: JoinHandle<()>,
}

/// Thread→session mapping, commands, streaming replies and approval cards for one
/// chat platform. Holds no session logic: every action is a public daemon API call.
pub struct Core<M: Messenger> {
    pub(crate) messenger: Arc<M>,
    pub(crate) daemon: crate::DaemonClient,
    pub(crate) store: Arc<Store>,
    pub(crate) options: CoreOptions,
    pub(crate) shutdown: CancellationToken,
    locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(crate) watchers: StdMutex<HashMap<String, Watcher>>,
    /// Card outcomes decided via a button, keyed by ticket id, while the API call is in flight.
    decisions: StdMutex<HashMap<String, String>>,
}

enum Command {
    New,
    Stop,
    Model(Option<String>),
    Sessions,
    Usage,
    Answer(String),
    Help,
}

enum Parsed {
    Command(Command),
    Text,
}

fn parse(text: &str) -> Parsed {
    let Some(prefix) = text.chars().next().filter(|c| matches!(c, '/' | '!')) else {
        return Parsed::Text;
    };
    let body = &text[1..];
    let (word, rest) = body
        .split_once(char::is_whitespace)
        .map_or((body, ""), |(word, rest)| (word, rest.trim()));
    // Telegram appends `@botname` to commands typed in groups.
    let word = word.split('@').next().unwrap_or_default().to_lowercase();
    let command = match word.as_str() {
        "new" => Command::New,
        "stop" => Command::Stop,
        "model" => Command::Model((!rest.is_empty()).then(|| rest.to_owned())),
        "sessions" => Command::Sessions,
        "usage" => Command::Usage,
        "answer" => Command::Answer(rest.to_owned()),
        "help" | "start" => Command::Help,
        // `!word` is ordinary chat; only `/word` is reserved for the bot.
        _ if prefix == '/' => Command::Help,
        _ => return Parsed::Text,
    };
    Parsed::Command(command)
}

fn describe(error: &ChannelError) -> String {
    match error {
        ChannelError::Daemon {
            status: 401 | 403, ..
        } => "The daemon refused the bot's token; it needs the read, steer and approve scopes."
            .into(),
        ChannelError::Daemon { code, .. } if code == "model_not_configured" => {
            "The daemon has no default model. Pick one with /model <name>, then send /new.".into()
        }
        ChannelError::Daemon { status: 404, .. } => {
            "That session or request no longer exists.".into()
        }
        ChannelError::Daemon { status: 409, .. } => {
            "The daemon cannot do that in the current state.".into()
        }
        ChannelError::Daemon { status, .. } => {
            format!("The daemon returned an error (HTTP {status}).")
        }
        ChannelError::Http(_) => "Could not reach the openside daemon.".into(),
        _ => "Something went wrong.".into(),
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Deterministic daemon message id: a redelivered platform update never creates a
/// second message (`SubmitMessageRequest.message_id` is an idempotency key).
fn message_id(thread: &ThreadKey, platform_message: &str) -> String {
    let digest = Sha256::digest(format!("{}|{platform_message}", thread.id()).as_bytes());
    format!("ch-{}", hex(&digest[..16]))
}

fn card(ticket: &Ticket, web_url: Option<&str>) -> (String, Vec<Button>) {
    let id = ticket.id.as_str();
    let button = |label: &str, data: String, danger: bool| Button {
        label: label.to_owned(),
        data,
        danger,
    };
    let web = || {
        web_url.map_or_else(
            || "the openside web UI".to_owned(),
            |base| {
                format!(
                    "{}/sessions/{}",
                    base.trim_end_matches('/'),
                    ticket.session_id.as_str()
                )
            },
        )
    };
    match ticket.kind {
        TicketKind::Info | TicketKind::SecretInput if ticket.form.is_some() => (
            format!(
                "Form requested\n{}\nFill this form privately in the openside web UI: open {}.",
                ticket.summary,
                web()
            ),
            Vec::new(),
        ),
        TicketKind::Hire => (
            format!("Teammate proposal\n{}\nReview and approve this proposal in the openside app: open {}.", ticket.summary, web()),
            Vec::new(),
        ),
        // Saving a site's authenticator is decided on the person's own device, never by
        // whoever can press a button in a shared chat.
        TicketKind::Confirm if ticket.otp_register.is_some() => (
            format!(
                "Authenticator setup\n{}\nThis can only be approved in the openside web UI: open {}.",
                ticket.summary,
                web()
            ),
            Vec::new(),
        ),
        TicketKind::Confirm => {
            let mut text = format!("Approval needed\n{}", ticket.summary);
            if let Some(tool) = &ticket.tool {
                text.push_str(&format!("\nTool: {} {}", tool.name, truncate(&tool.args.to_string(), 300)));
            }
            text.push_str(&format!("\nExpires {}", ticket.expires_at.to_rfc3339()));
            (
                text,
                vec![
                    button("Approve", format!("a:{id}"), false),
                    button("Reject", format!("r:{id}"), true),
                ],
            )
        }
        TicketKind::Info => {
            let mut text = format!("Question from the agent\n{}", ticket.summary);
            if let Some(instructions) = &ticket.instructions {
                text.push_str(&format!("\n{instructions}"));
            }
            let mut buttons: Vec<Button> = ticket
                .options
                .iter()
                .take(MAX_OPTIONS)
                .enumerate()
                .map(|(index, option)| button(&option.label, format!("o:{id}:{index}"), false))
                .collect();
            if ticket.options.is_empty() {
                text.push_str("\nReply with /answer <your answer>.");
            }
            buttons.push(button("Dismiss", format!("r:{id}"), true));
            (text, buttons)
        }
        // A deep link only: chat history is not private, so no secret is ever taken here.
        TicketKind::SecretInput => {
            let heading = if ticket.otp.is_some() {
                "Verification code requested"
            } else {
                "Secret input requested"
            };
            let web = web();
            (
                format!(
                    "{heading}\n{}\nChat history is not private, so this cannot be answered here. Open {web}, or run `openside ticket resolve {id} -` on the machine running the daemon.",
                    ticket.summary
                ),
                Vec::new(),
            )
        }
        // Credentials are never typed into chat: a deep link only, like a secret.
        TicketKind::Login => (
            format!(
                "Sign-in requested\n{}\nChat history is not private, so this cannot be answered here. Open {}.",
                ticket.summary,
                web()
            ),
            Vec::new(),
        ),
        // A page's choice releases no secret, so it may be picked from chat.
        TicketKind::Choice => {
            let options = ticket
                .choice
                .as_ref()
                .map(|choice| choice.options.as_slice())
                .unwrap_or_default();
            let mut buttons: Vec<Button> = options
                .iter()
                .take(MAX_OPTIONS)
                .enumerate()
                .map(|(index, option)| button(&option.label, format!("c:{id}:{index}"), false))
                .collect();
            buttons.push(button("Dismiss", format!("r:{id}"), true));
            (format!("Choice needed\n{}", ticket.summary), buttons)
        }
        TicketKind::DeviceApproval | TicketKind::Takeover => (
            format!(
                "Action needed outside chat\n{}\nComplete it in the openside web UI or at the desktop.",
                ticket.summary
            ),
            Vec::new(),
        ),
    }
}

impl<M: Messenger> Core<M> {
    /// Build the platform core. Nothing runs until messages arrive or
    /// [`Core::resume_all`] is called.
    pub fn new(
        messenger: Arc<M>,
        daemon: crate::DaemonClient,
        store: Arc<Store>,
        options: CoreOptions,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            messenger,
            daemon,
            store,
            options,
            shutdown,
            locks: StdMutex::new(HashMap::new()),
            watchers: StdMutex::new(HashMap::new()),
            decisions: StdMutex::new(HashMap::new()),
        })
    }

    fn thread_lock(&self, key: &ThreadKey) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(|p| p.into_inner());
        locks.entry(key.id()).or_default().clone()
    }

    /// Post `text` to the thread, split to the platform message limit.
    pub(crate) async fn say(&self, thread: &ThreadKey, text: &str) {
        let mut rest = text;
        while !rest.is_empty() {
            let cut = crate::text::split_index(rest, self.messenger.max_chars());
            let (head, tail) = rest.split_at(cut);
            if let Err(error) = self.messenger.send(thread, head, &[]).await {
                log(format_args!("{} send failed: {error}", M::PLATFORM));
                return;
            }
            rest = tail;
        }
    }

    /// Handle one allowed user message.
    pub async fn handle_inbound(self: &Arc<Self>, inbound: Inbound) {
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => {}
            _ = self.handle_inbound_active(inbound) => {}
        }
    }

    async fn handle_inbound_active(self: &Arc<Self>, inbound: Inbound) {
        let lock = self.thread_lock(&inbound.thread);
        let _guard = lock.lock().await;
        let thread = &inbound.thread;
        let mut text = inbound.text.trim().to_owned();
        let mut voice = false;
        if let Some(audio) = &inbound.audio {
            match self.transcribe(audio).await {
                Ok(transcript) => {
                    self.say(thread, &format!("Heard: {transcript}")).await;
                    text = if text.is_empty() {
                        transcript
                    } else {
                        format!("{text}\n{transcript}")
                    };
                    voice = true;
                }
                Err(refusal) => {
                    self.say(thread, &refusal).await;
                    if text.is_empty() {
                        return;
                    }
                }
            }
        }
        if text.is_empty() {
            return;
        }
        // A transcript is conversation, never a command.
        if !voice {
            if let Parsed::Command(command) = parse(&text) {
                self.command(thread, command).await;
                return;
            }
        }
        self.deliver(&inbound, &text).await;
    }

    async fn transcribe(&self, audio: &AudioRef) -> Result<String, String> {
        let Some(stt) = &self.options.stt else {
            return Err(NO_STT.into());
        };
        if audio.size.is_some_and(|size| size > stt.max_bytes) {
            return Err("That voice message is too large to transcribe.".into());
        }
        let directory = tempfile::Builder::new()
            .prefix("openside-stt-")
            .tempdir()
            .map_err(|_| "Could not prepare the voice message.".to_owned())?;
        let path = directory.path().join("audio");
        self.messenger
            .download_audio(audio, &path, stt.max_bytes)
            .await
            .map_err(|error| match error {
                ChannelError::Refused(_) => {
                    "That voice message is too large to transcribe.".to_owned()
                }
                _ => "Could not download the voice message.".to_owned(),
            })?;
        stt.transcribe(&path).await.map_err(|error| match error {
            ChannelError::Refused(reason) => {
                format!("Could not transcribe the voice message ({reason}).")
            }
            _ => "Could not run the local speech-to-text command.".to_owned(),
        })
    }

    /// Reuse a thread only while its immutable session owner still matches this identity.
    async fn ensure_session(self: &Arc<Self>, thread: &ThreadKey) -> Result<String, ChannelError> {
        let state = self.store.get(thread);
        let mut changed_owner = None;
        if let Some(id) = state.as_ref().and_then(|state| state.session_id.clone()) {
            if !self.daemon.has_owner() {
                return Ok(id);
            }
            match self.daemon.session(&id).await {
                Ok(session) if self.daemon.owns_session(&session) => return Ok(id),
                Ok(_) => {
                    self.stop_watcher(thread);
                    changed_owner = Some(self.daemon.owner_name().await?);
                }
                Err(error) if error.is_not_found() => {}
                Err(error) => return Err(error),
            }
            self.stop_watcher(thread);
        }
        let session = self
            .daemon
            .create_session(&CreateSessionRequest {
                agent_id: None,
                title: None,
                model: state.and_then(|state| state.model),
                mode: None,
                workspace: None,
                incognito: false,
                browser_space: Default::default(),
            })
            .await?;
        if !self.daemon.owns_session(&session) {
            return Err(ChannelError::Refused("channel_binding_conflict".into()));
        }
        let id = session.id.as_str().to_owned();
        self.store.update(thread, |state| {
            state.session_id = Some(id.clone());
            state.cursor = 0;
            state.cards.clear();
        })?;
        if let Some(name) = changed_owner {
            self.say(
                thread,
                &format!("This conversation now continues with {name}."),
            )
            .await;
        }
        self.ensure_watcher(thread, &id);
        Ok(id)
    }

    async fn deliver(self: &Arc<Self>, inbound: &Inbound, text: &str) {
        let thread = &inbound.thread;
        let id = message_id(thread, &inbound.message_id);
        for attempt in 0..2 {
            let session = match self.ensure_session(thread).await {
                Ok(session) => session,
                Err(error) => return self.say(thread, &describe(&error)).await,
            };
            match self.daemon.submit(&session, &id, text).await {
                Ok(_) => return self.ensure_watcher(thread, &session),
                // The session was deleted behind our back: forget it and start over once.
                Err(error) if error.is_not_found() && attempt == 0 => {
                    self.stop_watcher(thread);
                    let _ = self.store.update(thread, |state| {
                        state.session_id = None;
                        state.cursor = 0;
                        state.cards.clear();
                    });
                }
                Err(error) => return self.say(thread, &describe(&error)).await,
            }
        }
    }

    async fn command(self: &Arc<Self>, thread: &ThreadKey, command: Command) {
        let state = self.store.get(thread);
        let session = state.as_ref().and_then(|state| state.session_id.clone());
        match command {
            Command::Help => self.say(thread, HELP).await,
            Command::New => {
                self.stop_watcher(thread);
                let _ = self.store.update(thread, |state| {
                    state.session_id = None;
                    state.cursor = 0;
                    state.cards.clear();
                });
                match self.ensure_session(thread).await {
                    Ok(id) => {
                        self.say(thread, &format!("Started a new session ({}).", short(&id)))
                            .await
                    }
                    Err(error) => self.say(thread, &describe(&error)).await,
                }
            }
            Command::Stop => {
                let Some(session) = session else {
                    return self.say(thread, "No active session in this thread.").await;
                };
                match self.daemon.stop(&session).await {
                    Ok(_) => {
                        self.say(thread, "Stopped. Send a message to continue.")
                            .await
                    }
                    Err(error) => self.say(thread, &describe(&error)).await,
                }
            }
            Command::Model(None) => {
                let current = match state.and_then(|state| state.model) {
                    Some(model) => model,
                    None => match self.daemon.settings().await {
                        Ok(settings) => settings.default_model.map_or_else(
                            || "none configured".to_owned(),
                            |model| format!("{model} (daemon default)"),
                        ),
                        Err(error) => return self.say(thread, &describe(&error)).await,
                    },
                };
                self.say(thread, &format!("Model for the next session: {current}"))
                    .await;
            }
            Command::Model(Some(name)) => {
                if name.chars().count() > 200 || name.contains(char::is_whitespace) {
                    return self
                        .say(
                            thread,
                            "A model name has no spaces and is at most 200 characters.",
                        )
                        .await;
                }
                if self
                    .store
                    .update(thread, |state| state.model = Some(name.clone()))
                    .is_err()
                {
                    return self.say(thread, "Could not save the model choice.").await;
                }
                let next = if session.is_some() {
                    "A running session keeps its model; send /new to start one with it now."
                } else {
                    "Your next message starts a session with it."
                };
                self.say(thread, &format!("Model set to {name}. {next}"))
                    .await;
            }
            Command::Sessions => match self.daemon.sessions().await {
                Ok(list) => {
                    let mut lines = vec![format!("{} session(s)", list.sessions.len())];
                    for item in list.sessions.iter().take(20) {
                        let status = serde_json::to_value(item.status)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_default();
                        let here = if session.as_deref() == Some(item.id.as_str()) {
                            "  <- this thread"
                        } else {
                            ""
                        };
                        lines.push(format!(
                            "- {} {} [{status}]{here}",
                            short(item.id.as_str()),
                            truncate(&item.title, 40)
                        ));
                    }
                    self.say(thread, &lines.join("\n")).await;
                }
                Err(error) => self.say(thread, &describe(&error)).await,
            },
            Command::Usage => {
                let Some(session) = session else {
                    return self.say(thread, "No active session in this thread.").await;
                };
                match self.usage(&session).await {
                    Ok(text) => self.say(thread, &text).await,
                    Err(error) => self.say(thread, &describe(&error)).await,
                }
            }
            Command::Answer(answer) => {
                let Some(session) = session else {
                    return self.say(thread, "No active session in this thread.").await;
                };
                self.answer(thread, &session, &answer).await;
            }
        }
    }

    async fn usage(&self, session: &str) -> Result<String, ChannelError> {
        let last = self.daemon.session(session).await?.last_seq;
        let mut totals: std::collections::BTreeMap<String, (u64, u64, u64)> = Default::default();
        if last > 0 {
            let mut stream = self.daemon.events(session, 0).await?;
            let read = async {
                while let Some(event) = stream.next().await {
                    let event = event?;
                    if let EventKind::Usage {
                        model,
                        input_tokens,
                        output_tokens,
                    } = &event.kind
                    {
                        let entry = totals.entry(model.clone()).or_default();
                        entry.0 += 1;
                        entry.1 += input_tokens;
                        entry.2 += output_tokens;
                    }
                    if event.seq >= last {
                        break;
                    }
                }
                Ok::<(), ChannelError>(())
            };
            tokio::time::timeout(Duration::from_secs(15), read)
                .await
                .map_err(|_| ChannelError::Http("timeout".into()))??;
        }
        if totals.is_empty() {
            return Ok("No model usage recorded for this session yet.".into());
        }
        let mut lines = vec!["Usage for this session".to_owned()];
        let (mut requests, mut input, mut output) = (0, 0, 0);
        for (model, (count, i, o)) in &totals {
            lines.push(format!(
                "- {model}: {count} request(s), {i} input / {o} output tokens"
            ));
            requests += count;
            input += i;
            output += o;
        }
        lines.push(format!(
            "Total: {requests} request(s), {input} input / {output} output tokens"
        ));
        Ok(lines.join("\n"))
    }

    async fn answer(&self, thread: &ThreadKey, session: &str, answer: &str) {
        if answer.is_empty() {
            return self.say(thread, "Usage: /answer <your answer>").await;
        }
        let pending = match self.daemon.pending_tickets().await {
            Ok(list) => list.tickets,
            Err(error) => return self.say(thread, &describe(&error)).await,
        };
        let target = pending
            .into_iter()
            .filter(|ticket| {
                ticket.session_id.as_str() == session
                    && ticket.kind == TicketKind::Info
                    && ticket.options.is_empty()
            })
            .min_by_key(|ticket| ticket.created_at);
        let Some(ticket) = target else {
            return self
                .say(thread, "No question is waiting for a free-form answer.")
                .await;
        };
        match self.daemon.resolve(ticket.id.as_str(), json!(answer)).await {
            Ok(_) => self.say(thread, "Answer sent.").await,
            Err(error) => self.say(thread, &describe(&error)).await,
        }
    }

    /// Handle one allowed button press on an approval card.
    pub async fn handle_press(self: &Arc<Self>, press: Press) {
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => {}
            _ = self.handle_press_active(press) => {}
        }
    }

    async fn handle_press_active(self: &Arc<Self>, press: Press) {
        // Serialized per thread so two presses on one card cannot interleave: the loser's
        // failure must not clear the winner's recorded decision before the card settles.
        let lock = self.thread_lock(&press.thread);
        let _guard = lock.lock().await;
        let note = match self.press(&press).await {
            Ok(note) | Err(note) => note,
        };
        self.messenger.answer_press(&press, &note).await;
    }

    async fn press(&self, press: &Press) -> Result<String, String> {
        let mut parts = press.data.splitn(3, ':');
        let (action, id) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        let index = parts.next().and_then(|index| index.parse::<usize>().ok());
        let thread = &press.thread;
        let state = self
            .store
            .get(thread)
            .ok_or("This card is no longer active.")?;
        let session = state
            .session_id
            .as_deref()
            .ok_or("This card is no longer active.")?;
        // Only cards this bot posted in this very thread can be answered from it.
        let card = state
            .cards
            .get(id)
            .filter(|card| card.message == press.message.0);
        let Some(card) = card else {
            return Err("This card is no longer active.".into());
        };
        if card.done {
            return Err("Already handled.".into());
        }
        let ticket = self
            .daemon
            .ticket(id)
            .await
            .map_err(|error| describe(&error))?;
        if ticket.session_id.as_str() != session || ticket.status != TicketStatus::Pending {
            self.settle_card(thread, id, "No longer pending").await;
            return Err("That request is no longer pending.".into());
        }
        let outcome = match (action, ticket.kind) {
            // Authenticator setup is approved only on the person's own device.
            _ if ticket.otp_register.is_some() => {
                return Err("That request cannot be answered from chat.".into())
            }
            ("a", TicketKind::Confirm) => format!("Approved by user {}", press.user),
            ("r", TicketKind::Confirm | TicketKind::Info | TicketKind::Choice) => {
                format!("Rejected by user {}", press.user)
            }
            ("o", TicketKind::Info) => {
                let option = index
                    .and_then(|index| ticket.options.get(index))
                    .ok_or("That option no longer exists.")?;
                format!("Answered: {}", option.label)
            }
            ("c", TicketKind::Choice) => {
                let option = index
                    .and_then(|index| ticket.choice.as_ref()?.options.get(index))
                    .ok_or("That option no longer exists.")?;
                format!("Chose: {}", option.label)
            }
            // Secret, device and takeover tickets are never answerable from chat.
            _ => return Err("That request cannot be answered from chat.".into()),
        };
        self.decisions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_owned(), outcome.clone());
        let result = match action {
            "a" => self.daemon.resolve(id, json!(true)).await,
            "r" => {
                self.daemon
                    .reject(id, &format!("rejected from {}", M::PLATFORM))
                    .await
            }
            "c" => {
                let value = index
                    .and_then(|index| ticket.choice.as_ref()?.options.get(index))
                    .map(|option| json!({"option": option.id}));
                self.daemon.resolve(id, value.unwrap_or(Value::Null)).await
            }
            _ => {
                let value = index
                    .and_then(|index| ticket.options.get(index))
                    .map(|o| json!(o.value));
                self.daemon.resolve(id, value.unwrap_or(Value::Null)).await
            }
        };
        if let Err(error) = result {
            self.decisions
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(id);
            return Err(describe(&error));
        }
        self.settle_card(thread, id, &outcome).await;
        self.decisions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        Ok(match action {
            "a" => "Approved",
            "r" => "Rejected",
            _ => "Answered",
        }
        .to_owned())
    }

    /// Post a card for a newly opened ticket (once per ticket).
    pub(crate) async fn post_card(&self, thread: &ThreadKey, announced: Ticket) {
        let id = announced.id.as_str().to_owned();
        if self
            .store
            .get(thread)
            .is_some_and(|state| state.cards.contains_key(&id))
        {
            return;
        }
        // Replays announce tickets that were settled long ago; only pending ones need a card.
        let ticket = self.daemon.ticket(&id).await.unwrap_or(announced);
        if ticket.status != TicketStatus::Pending {
            return;
        }
        let (text, buttons) = card(&ticket, self.options.web_url.as_deref());
        let text = truncate(&text, self.messenger.max_chars());
        match self.messenger.send(thread, &text, &buttons).await {
            Ok(MsgRef(message)) => {
                let _ = self.store.update(thread, |state| {
                    state.cards.insert(
                        id,
                        Card {
                            message,
                            text,
                            done: false,
                        },
                    );
                });
            }
            Err(error) => log(format_args!("{} card send failed: {error}", M::PLATFORM)),
        }
    }

    /// Strip the buttons of a card and append the outcome.
    pub(crate) async fn settle_card(&self, thread: &ThreadKey, ticket: &str, outcome: &str) {
        // A decision made through a button wins over the generic event-driven outcome, so the
        // card names who decided regardless of which of the two arrives first.
        let decided = self
            .decisions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(ticket)
            .cloned();
        let outcome = decided.as_deref().unwrap_or(outcome);
        let Some(card) = self
            .store
            .get(thread)
            .and_then(|state| state.cards.get(ticket).cloned())
            .filter(|card| !card.done)
        else {
            return;
        };
        let text = truncate(
            &format!("{}\n\n{outcome}", card.text),
            self.messenger.max_chars(),
        );
        // Mark first: a concurrent button press or event must not edit the card twice.
        let claimed = self
            .store
            .update(thread, |state| match state.cards.get_mut(ticket) {
                Some(card) if !card.done => {
                    card.done = true;
                    true
                }
                _ => false,
            })
            .unwrap_or(false);
        if !claimed {
            return;
        }
        if let Err(error) = self
            .messenger
            .edit(thread, &MsgRef(card.message), &text, &[])
            .await
        {
            log(format_args!("{} card update failed: {error}", M::PLATFORM));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as RecordedMutex;

    fn otp_ticket() -> Ticket {
        serde_json::from_value(serde_json::json!({
            "id": "t-otp", "session_id": "s-1", "tool_call_id": "call",
            "deferral": "external", "kind": "secret_input",
            "summary": "Verification code for https://bank.test",
            "instructions": null, "options": [], "origin": {"subagent_id": null},
            "tool": null,
            "target": {"tab_id": "tab", "origin": "https://bank.test", "field_ref": "e1"},
            "resume_when": null,
            "created_at": "2026-01-01T00:00:00Z", "expires_at": "2026-01-01T00:10:00Z",
            "status": "pending", "resolution": null,
            "otp": {"channel": "authenticator", "can_register": true}
        }))
        .unwrap()
    }
    #[test]
    fn otp_cards_link_to_the_web_ui_and_never_offer_an_answer() {
        let ticket = otp_ticket();
        let (text, buttons) = card(&ticket, Some("https://host.example.ts.net/"));
        assert!(buttons.is_empty());
        assert!(text.starts_with("Verification code requested"), "{text}");
        assert!(
            text.contains("Open https://host.example.ts.net/sessions/s-1,"),
            "{text}"
        );
        assert!(text.contains("openside ticket resolve t-otp -"), "{text}");
        let (text, _) = card(&ticket, None);
        assert!(text.contains("Open the openside web UI"), "{text}");
    }

    #[test]
    fn form_cards_keep_imported_suggestions_out_of_channel_text_and_actions() {
        let canary = "IMPORTED_ADDRESS_CHANNEL_CANARY_73b1";
        let mut ticket = otp_ticket();
        ticket.otp = None;
        ticket.summary = "Fill fields for https://bank.test".into();
        let field: openside_proto::InputField = serde_json::from_value(json!({
            "id":"field_0","label":"Street address","kind":"text","secret":false,
            "otp":false,"card":null,"required":true,"autocomplete":"street-address",
            "placeholder":null,"validation":"text","max_length":160,"initial":null,
            "suggestions":[canary],"target_origin":"https://bank.test"
        }))
        .unwrap();
        ticket.form = Some(openside_proto::TicketForm {
            origin: "https://bank.test".into(),
            site_name: "Bank".into(),
            fields: vec![field.clone()],
            payment: false,
            submit_label: "Fill fields".into(),
        });
        ticket.card = Some(openside_proto::Card {
            id: format!("ticket:{}", ticket.id),
            kind: openside_proto::CardKind::Form,
            site: openside_proto::CardSite {
                origin: Some("https://bank.test".into()),
                name: "Bank".into(),
                favicon_ref: None,
            },
            title: "Fill fields".into(),
            subtitle: None,
            status: openside_proto::CardStatus::Pending,
            blocks: vec![openside_proto::Block::InputFields {
                id: "fields".into(),
                fields: vec![field],
            }],
            actions: Vec::new(),
            interaction: openside_proto::CardInteraction::Ask,
            ticket_id: Some(ticket.id.clone()),
            collapsed_summary: "Fill fields".into(),
        });
        assert!(serde_json::to_string(&ticket).unwrap().contains(canary));
        for kind in [TicketKind::Info, TicketKind::SecretInput] {
            ticket.kind = kind;
            let (text, buttons) = card(&ticket, Some("https://host.example.ts.net/"));
            assert!(!text.contains(canary));
            assert!(!text.contains("/answer"));
            assert!(text.contains("https://host.example.ts.net/sessions/s-1"));
            assert!(buttons.is_empty());
        }
    }

    fn login_ticket() -> Ticket {
        let mut ticket = otp_ticket();
        ticket.id = openside_proto::TicketId::new("t-login");
        ticket.kind = TicketKind::Login;
        ticket.otp = None;
        ticket.summary = "Sign in to https://bank.test".into();
        ticket.login = Some(
            serde_json::from_value(json!({
                "origin":"https://bank.test", "site_name":"Bank",
                "fields":[{"id":"private-field-canary","kind":"password",
                    "label":"Private password label canary","masked":true,"required":true}],
                "alternatives":[], "submit_label":"Private submit label canary", "can_save":true
            }))
            .unwrap(),
        );
        ticket
    }

    #[test]
    fn login_cards_are_link_only_and_choices_offer_page_labelled_buttons() {
        let ticket = login_ticket();
        let (text, buttons) = card(&ticket, Some("https://host.example.ts.net/"));
        assert!(buttons.is_empty(), "{buttons:?}");
        assert!(
            text.contains("Open https://host.example.ts.net/sessions/s-1."),
            "{text}"
        );
        assert!(text.contains("cannot be answered here"), "{text}");
        for forbidden in [
            "Private password",
            "private-field-canary",
            "Private submit",
            "/answer",
            "ticket resolve",
            "field_ref",
            "secret",
            "values",
        ] {
            assert!(
                !text.contains(forbidden),
                "login card exposed {forbidden}: {text}"
            );
        }
        assert!(card(&ticket, None).0.contains("Open the openside web UI."));
        let mut choice = ticket;
        choice.kind = TicketKind::Choice;
        choice.login = None;
        choice.choice = Some(
            serde_json::from_value(json!({
                "origin":"https://bank.test", "site_name":"Bank", "question":"How to verify?",
                "options":[{"id":"sms","label":"Text me a code"},{"id":"email","label":"Email"}],
                "multiple":false
            }))
            .unwrap(),
        );
        let (_, buttons) = card(&choice, None);
        assert_eq!(
            buttons,
            vec![
                Button {
                    label: "Text me a code".into(),
                    data: "c:t-login:0".into(),
                    danger: false
                },
                Button {
                    label: "Email".into(),
                    data: "c:t-login:1".into(),
                    danger: false
                },
                Button {
                    label: "Dismiss".into(),
                    data: "r:t-login".into(),
                    danger: true
                },
            ]
        );
    }

    #[derive(Default)]
    struct RecordedMessenger(RecordedMutex<Vec<String>>);

    impl Messenger for RecordedMessenger {
        const PLATFORM: &'static str = "telegram";
        fn max_chars(&self) -> usize {
            4096
        }
        async fn send(
            &self,
            _: &ThreadKey,
            text: &str,
            _: &[Button],
        ) -> crate::error::Result<MsgRef> {
            self.0.lock().push(text.into());
            Ok(MsgRef("message".into()))
        }
        async fn edit(
            &self,
            _: &ThreadKey,
            _: &MsgRef,
            _: &str,
            _: &[Button],
        ) -> crate::error::Result<()> {
            Ok(())
        }
        async fn download_audio(
            &self,
            _: &AudioRef,
            _: &std::path::Path,
            _: u64,
        ) -> crate::error::Result<()> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn channel_answers_and_button_presses_cannot_resolve_a_login() {
        use axum::{extract::Request, routing::any, Json, Router};
        let requests = Arc::new(RecordedMutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let ticket = login_ticket();
        let served = ticket.clone();
        let app = Router::new().fallback(any(move |request: Request| {
            let served = served.clone();
            let recorded = Arc::clone(&recorded);
            async move {
                recorded.lock().push((
                    request.method().to_string(),
                    request.uri().path().to_owned(),
                ));
                match request.uri().path() {
                    "/v1/tickets" => Json(json!({"tickets":[served]})),
                    "/v1/tickets/t-login" => Json(json!(served)),
                    path => panic!("login answer reached a mutation: {path}"),
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let thread = ThreadKey::new("telegram", "chat", "");
        store
            .update(&thread, |state| {
                state.session_id = Some("s-1".into());
                state.cards.insert(
                    "t-login".into(),
                    Card {
                        message: "message".into(),
                        text: card(&ticket, None).0,
                        done: false,
                    },
                );
            })
            .unwrap();
        let messenger = Arc::new(RecordedMessenger::default());
        let core = Core::new(
            Arc::clone(&messenger),
            crate::DaemonClient::new(
                &format!("http://{addr}"),
                crate::secret::Secret::new("token"),
            )
            .unwrap(),
            store,
            CoreOptions {
                edit_interval: Duration::ZERO,
                stt: None,
                web_url: None,
            },
            CancellationToken::new(),
        );
        core.answer(&thread, "s-1", "PASSWORD-CANARY").await;
        assert_eq!(
            messenger.0.lock().as_slice(),
            ["No question is waiting for a free-form answer."]
        );
        for action in ["a", "r", "o", "c"] {
            let press = Press {
                thread: thread.clone(),
                user: "user".into(),
                data: format!("{action}:t-login:0"),
                message: MsgRef("message".into()),
                token: String::new(),
            };
            assert_eq!(
                core.press(&press).await,
                Err("That request cannot be answered from chat.".into())
            );
        }
        let requests = requests.lock();
        assert_eq!(requests.len(), 5);
        assert!(
            requests.iter().all(|(method, _)| method == "GET"),
            "{requests:?}"
        );
        server.abort();
    }

    #[test]
    fn hire_cards_only_link_to_person_approval_in_the_app() {
        let mut ticket = otp_ticket();
        ticket.kind = TicketKind::Hire;
        ticket.otp = None;
        let (text, buttons) = card(&ticket, Some("https://host.example.ts.net/"));
        assert!(buttons.is_empty());
        assert!(text.contains("Review and approve this proposal in the openside app"));
        assert!(text.contains("https://host.example.ts.net/sessions/s-1"));
    }

    #[test]
    fn authenticator_setup_cards_offer_no_buttons() {
        let mut ticket = otp_ticket();
        ticket.kind = TicketKind::Confirm;
        ticket.otp = None;
        ticket.otp_register = Some(openside_proto::TicketOtpRegister {
            origin: "https://bank.test".into(),
            issuer: Some("Bank".into()),
            account: None,
        });
        let (text, buttons) = card(&ticket, Some("https://host.example.ts.net/"));
        assert!(buttons.is_empty(), "{text}");
        assert!(text.starts_with("Authenticator setup"), "{text}");
        assert!(
            text.contains("https://host.example.ts.net/sessions/s-1"),
            "{text}"
        );
    }
}
