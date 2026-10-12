use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use openside_agent_sidecar::{SidecarConfig, SidecarRuntime};
use openside_core::{
    AgentRuntime, MessageId, MessageStatus, QueueMode, RunOutcome, RuntimeError, RuntimeEvent,
    SessionSpec, SubagentControl, TicketId,
};
use openside_proto::{
    title_from_message, Event, EventKind, Message, MessageOutcome, SessionId, SessionStatus,
    Ticket, TicketStatus, DEFAULT_SESSION_TITLE,
};
use openside_sessions::SessionState;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use crate::{
    config::{Config, Resolved},
    host::Host,
    persistence::{Database, Result},
    runtime::{Runtime, Session},
    tools::Registry,
};

#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) db: Database,
    pub(crate) config: Arc<Resolved>,
    pub(crate) actors: Arc<Mutex<HashMap<SessionId, mpsc::Sender<Command>>>>,
    pub(crate) events: broadcast::Sender<Event>,
    pub(crate) ticket_gate: Arc<Mutex<()>>,
    pub(crate) registry: Arc<Registry>,
    runtime: Arc<dyn Runtime>,
    pub(crate) shutdown: broadcast::Sender<()>,
    pub(crate) routines: Arc<crate::routines::Slot>,
    pub(crate) memory: Arc<crate::memory::MemoryService>,
    pub(crate) policy: Arc<crate::policy_api::LivePolicy>,
    pub(crate) sync: Arc<crate::sync::Service>,
    pub(crate) remote: Arc<crate::pairing::Remote>,
    pub(crate) transports: Arc<crate::remote::Transports>,
    pub(crate) previews: Arc<crate::preview::Registry>,
    pub(crate) accounts: Arc<crate::accounts::Service>,
    pub(crate) credential_paths: Arc<crate::credential_paths::ProtectedPaths>,
    pub(crate) host_calls: Arc<crate::host_calls::Calls>,
}

pub(crate) enum Action {
    Deliver,
    Edit(MessageId, String),
    Delete(MessageId),
    Promote(MessageId),
    Move(MessageId, u32),
    Stop,
    /// Stop the session and write its `session.deleted` tombstone; the actor then exits.
    Tombstone,
    Resume(TicketId),
    Subagent(String, SubagentControl),
    /// Open the runtime: restored children are reconciled with the sidecar snapshot, and a
    /// prewarmed session answers its next message without a sidecar cold start.
    Recover,
    Shutdown,
    /// Queue a browser notice on the steer lane, only while a run is in flight; an idle
    /// session is not woken by browser activity.
    BrowserNotice(String),
}
pub(crate) struct Command {
    action: Action,
    reply: oneshot::Sender<Result<()>>,
}

/// `open.config.agent`: runtime options plus the user's outbound allow-list, which the
/// sidecar enforces on every HTTP transport besides loopback and the built providers'
/// endpoints. The key is omitted when empty so default sessions keep their exact open message.
fn agent_options(config: &openside_config::Config) -> serde_json::Value {
    let mut options = config.agent.runtime_options();
    if !config.network.allow.is_empty() {
        options["network_allow"] = serde_json::json!(config.network.allow);
    }
    options
}

/// Durable API state and one supervised sidecar per active session.
pub struct Daemon {
    pub(crate) shared: Shared,
    _lock: std::fs::File,
}

impl Daemon {
    /// Resolve portable configuration, replay state, and reconcile interrupted runs.
    pub async fn open(config: Config) -> Result<Self> {
        let config = config.resolve()?;
        let runtime = SidecarRuntime::new(SidecarConfig {
            argv: config.argv.clone(),
            runtime_dir: config.loaded.paths.runtime_dir.clone(),
            startup_timeout: Duration::from_millis(config.loaded.config.agent.startup_timeout_ms),
            env: Vec::new(),
        });
        Self::open_runtime(config, runtime).await
    }

    pub(crate) async fn open_runtime<R: AgentRuntime + 'static>(
        config: Resolved,
        runtime: R,
    ) -> Result<Self>
    where
        R::Session: 'static,
    {
        // A malformed network.allow entry would silently admit nothing at the sidecar's
        // transport; refuse to start instead.
        openside_policy::Egress::new(&config.loaded.config.network.allow)?;
        tokio::fs::create_dir_all(&config.data_dir).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&config.data_dir, std::fs::Permissions::from_mode(0o700))
                .await?;
        }
        let lock_path = openside_config::data_layout::DataPath::Lock.path(&config.data_dir);
        let lock = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
            use fs2::FileExt;
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(lock_path)?;
            // A previous instance's lock can outlive it briefly: a child forked by any thread
            // shares the open file description until it execs. Wait a bounded time for that
            // release before refusing; a live daemon still holds it and is refused.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while file.try_lock_exclusive().is_err() {
                if std::time::Instant::now() >= deadline {
                    return Err("data directory already has a daemon".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(file)
        })
        .await??;
        let db = Database::open(&config.data_dir).await?;
        let (events, _) = broadcast::channel(1024);
        let (shutdown, _) = broadcast::channel(1);
        let registry = Arc::new(Registry::new(&config));
        let policy = Arc::new(crate::policy_api::LivePolicy::load(&config)?);
        let memory = Arc::new(
            crate::memory::MemoryService::open(&config, registry.redactors.clone()).await?,
        );
        let credential_paths = Arc::new(crate::credential_paths::ProtectedPaths::from_config(
            &config,
        ));
        let shared = Shared {
            db,
            config: Arc::new(config),
            actors: Default::default(),
            events,
            ticket_gate: Default::default(),
            registry,
            runtime: Arc::new(runtime),
            shutdown,
            routines: Default::default(),
            memory,
            policy,
            sync: Default::default(),
            remote: Default::default(),
            transports: Default::default(),
            previews: Default::default(),
            accounts: Default::default(),
            credential_paths,
            host_calls: Default::default(),
        };
        shared.registry.vault.unlock(&shared).await;
        shared.registry.browser.recover(&shared).await?;
        crate::push::start(&shared);
        crate::otp_ingest::start(&shared);
        crate::inhibit::start(&shared);
        let daemon = Self {
            shared,
            _lock: lock,
        };
        let sessions = daemon.shared.db.store(|s| Ok(s.list_sessions())).await?;
        for session in &sessions {
            complete_inflight(
                &daemon.shared,
                &session.id,
                MessageOutcome::Failed,
                "interrupted",
            )
            .await?;
            crate::subagents::interrupt_running(&daemon.shared, &session.id).await?;
            crate::subagents::repair(&daemon.shared, &session.id).await?;
            // A crash between a Task's end and its report leaves the report to queue now.
            crate::tasks::report_ended(&daemon.shared, &session.id).await?;
            if session.status == SessionStatus::Running {
                daemon
                    .shared
                    .emit(&session.id, None, EventKind::SessionSettled {})
                    .await?;
            }
        }
        crate::tickets::expire(&daemon.shared).await?;
        crate::artifacts::sweep_deleted(&daemon.shared).await;
        for session in sessions {
            // Children still waiting must be matched against the restored sidecar snapshot
            // before their tickets are allowed to hold the session's queue.
            if session.status != SessionStatus::Stopped
                && crate::subagents::any_open(&daemon.shared, &session.id).await?
            {
                let _ = daemon.shared.command(&session.id, Action::Recover).await;
            }
            let state = daemon.shared.db.state(&session.id).await?;
            let stored = session.id.clone();
            let undelivered = daemon
                .shared
                .db
                .store(move |store| Ok(store.undelivered_tickets(&stored)?))
                .await?;
            // A deferred message is still waiting only while a ticket can wake its run; once
            // none can (or the session is stopped) nothing would ever settle it.
            let waiting = undelivered.len()
                + state
                    .tickets
                    .values()
                    .filter(|t| t.status == TicketStatus::Pending)
                    .count();
            if state.status == SessionStatus::Stopped || waiting == 0 {
                let outcome = if state.status == SessionStatus::Stopped {
                    MessageOutcome::Aborted
                } else {
                    MessageOutcome::Failed
                };
                settle_deferred(&daemon.shared, &session.id, outcome, "interrupted").await?;
            }
            if state.status == SessionStatus::Stopped
                || daemon.shared.auth_failed(&session.id).await?
            {
                continue;
            }
            for ticket in undelivered {
                let _ = daemon
                    .shared
                    .command(&session.id, Action::Resume(ticket))
                    .await;
            }
            if !state.queue.pending_ids().any(|_| true) {
                continue;
            }
            // Pending human steps survive restart and hold all queue lanes.
            if !holds_queue(&daemon.shared, &session.id, &state).await? {
                let _ = daemon.shared.command(&session.id, Action::Deliver).await;
            }
        }
        crate::routines::start(&daemon.shared).await?;
        crate::memory::start(&daemon.shared);
        let shared = daemon.shared.clone();
        let mut shutdown = shared.shutdown.subscribe();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            let mut browser_tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = shutdown.recv() => { let _ = shared.registry.browser.shutdown(&shared).await; break; },
                    _ = tick.tick() => { let _ = crate::tickets::expire(&shared).await; }
                    _ = browser_tick.tick() => { let _ = shared.registry.browser.poll(&shared).await; }
                }
            }
        });
        Ok(daemon)
    }

    /// Bind a private Unix socket and an optional authenticated loopback listener.
    #[cfg(unix)]
    pub async fn serve(self) -> Result<()> {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        let socket = &self.shared.config.socket;
        let parent = socket.parent().ok_or("socket needs a parent directory")?;
        private_directory(parent).await?;
        match tokio::fs::symlink_metadata(socket).await {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err("socket path is not a socket".into());
                }
                if tokio::net::UnixStream::connect(socket).await.is_ok() {
                    return Err("daemon socket already active".into());
                }
                tokio::fs::remove_file(socket).await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let unix = tokio::net::UnixListener::bind(socket)?;
        tokio::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).await?;
        let tcp = if let Some(address) = self.shared.config.listen {
            let listener = tokio::net::TcpListener::bind(address).await?;
            tokio::fs::write(
                openside_config::data_layout::DataPath::Port.path(&self.shared.config.data_dir),
                listener.local_addr()?.port().to_string(),
            )
            .await?;
            Some(listener)
        } else {
            None
        };
        let listen = tcp.as_ref().and_then(|listener| listener.local_addr().ok());
        crate::remote::start(&self.shared, listen);
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let unix_router = crate::api::router(self.shared.clone(), true);
        let unix_task = tokio::spawn(async move { axum::serve(unix, unix_router).await });
        let tcp_task = tcp.map(|listener| {
            let router = crate::api::router(self.shared.clone(), false);
            tokio::spawn(async move { axum::serve(listener, router).await })
        });
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        unix_task.abort();
        if let Some(task) = tcp_task {
            task.abort();
        }
        self.shared.shutdown().await;
        tokio::fs::remove_file(socket).await?;
        Ok(())
    }

    /// Windows transport is not implemented; fail explicitly rather than exposing TCP.
    #[cfg(not(unix))]
    pub async fn serve(self) -> Result<()> {
        Err("Unix daemon transport is unsupported on this platform".into())
    }

    /// Shut down owned processes without rejecting durable pending human steps.
    pub async fn shutdown(&self) {
        self.shared.shutdown().await;
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.shared.shutdown.send(());
    }
}

#[cfg(unix)]
async fn private_directory(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => verify_private_directory(path, &metadata, nix::unistd::geteuid().as_raw())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(path).await?;
            tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// A 0700 directory owned by someone else (a pre-created `/tmp/openside-<uid>`) would deny the
/// socket to its rightful user, so it is refused rather than reused.
#[cfg(unix)]
fn verify_private_directory(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
    euid: u32,
) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err("socket parent must be an existing private 0700 directory".into());
    }
    if metadata.uid() != euid {
        return Err(format!(
            "socket parent {} is owned by uid {}, not by the daemon's uid {euid}; remove it or choose another socket path",
            path.display(),
            metadata.uid()
        )
        .into());
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod private_directory_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::verify_private_directory;

    #[test]
    fn refuses_a_private_directory_owned_by_another_user() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("openside-1234");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        let euid = nix::unistd::geteuid().as_raw();
        verify_private_directory(&dir, &metadata, euid).unwrap();
        let error = verify_private_directory(&dir, &metadata, euid.wrapping_add(1))
            .unwrap_err()
            .to_string();
        assert!(error.contains("is owned by uid"), "{error}");
        assert!(error.contains(&dir.display().to_string()), "{error}");
    }

    #[test]
    fn refuses_group_or_world_accessible_directories() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("open");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        let error = verify_private_directory(&dir, &metadata, nix::unistd::geteuid().as_raw())
            .unwrap_err()
            .to_string();
        assert!(error.contains("private 0700"), "{error}");
    }
}

impl Shared {
    pub(crate) async fn emit(
        &self,
        id: &SessionId,
        run: Option<String>,
        kind: EventKind,
    ) -> Result<Event> {
        let kind = serde_json::to_value(kind)?;
        let keep = crate::tools::event_keep(kind["kind"].as_str().unwrap_or_default());
        let keep: Vec<&[&str]> = keep.iter().map(Vec::as_slice).collect();
        let kind: EventKind = serde_json::from_value(self.registry.redact_except(kind, &keep)?)?;
        // The daemon is the only place that names an untitled session after its first message;
        // clients (web, extension, CLI) show whatever `session.updated` says.
        let derived = match &kind {
            EventKind::MessageQueued {
                text, attachments, ..
            } => self.first_message_title(id, text, attachments).await?,
            _ => None,
        };
        let event = self.db.append(id, run, kind).await?;
        crate::preview::committed(self, &event);
        let _ = self.events.send(event.clone());
        if let Some(title) = derived {
            let titled = self
                .db
                .append(
                    id,
                    None,
                    EventKind::SessionUpdated {
                        title: Some(title),
                        pinned: None,
                        archived: None,
                    },
                )
                .await?;
            let _ = self.events.send(titled);
        }
        Ok(event)
    }
    /// Title for a session still called "New session" from its first message: the first non-blank
    /// line, or the first file's name when the message is only files.
    async fn first_message_title(
        &self,
        id: &SessionId,
        text: &str,
        attachments: &[openside_proto::Attachment],
    ) -> Result<Option<String>> {
        let id = id.clone();
        let untitled = self
            .db
            .store(move |s| {
                Ok(s.session(&id).is_some_and(|state| {
                    state.first_message.is_none() && state.title == DEFAULT_SESSION_TITLE
                }))
            })
            .await?;
        Ok(untitled
            .then(|| {
                title_from_message(text).or_else(|| {
                    attachments
                        .iter()
                        .find_map(|a| a.name.as_deref().and_then(title_from_message))
                })
            })
            .flatten())
    }
    pub(crate) async fn auth_failed(&self, id: &SessionId) -> Result<bool> {
        let mut after = 0;
        loop {
            let events = self.db.events(id, after).await?;
            if events.is_empty() {
                return Ok(false);
            }
            for event in events {
                after = event.seq;
                if matches!(event.kind, EventKind::RuntimeError { code, .. } if code == "auth_unavailable")
                {
                    return Ok(true);
                }
            }
        }
    }
    pub(crate) async fn command(&self, id: &SessionId, action: Action) -> Result<()> {
        self.db.state(id).await?;
        if !matches!(action, Action::Stop | Action::Shutdown | Action::Tombstone)
            && self.auth_failed(id).await?
        {
            return Err("session authentication is unavailable".into());
        }
        let sender = {
            let mut actors = self.actors.lock().await;
            match actors.get(id) {
                Some(sender) => sender.clone(),
                None => {
                    // Checked under the map lock: a deleted session's actor removes itself
                    // while holding it, so no new actor can be spawned for a tombstone.
                    self.db.state(id).await?;
                    let (tx, rx) = mpsc::channel(64);
                    tokio::spawn(actor(self.clone(), id.clone(), rx));
                    actors.insert(id.clone(), tx.clone());
                    tx
                }
            }
        };
        let (reply, result) = oneshot::channel();
        sender
            .send(Command { action, reply })
            .await
            .map_err(|_| "session actor closed")?;
        result.await.map_err(|_| "session actor closed")?
    }
    async fn shutdown(&self) {
        self.transports.stop().await;
        let senders: Vec<_> = self
            .actors
            .lock()
            .await
            .drain()
            .map(|(_, sender)| sender)
            .collect();
        for sender in senders {
            let (reply, result) = oneshot::channel();
            if sender
                .send(Command {
                    action: Action::Shutdown,
                    reply,
                })
                .await
                .is_ok()
            {
                let _ = result.await;
            }
        }
        let _ = self.shutdown.send(());
        let _ = self.registry.browser.shutdown(self).await;
    }
    pub(crate) async fn message(&self, id: &SessionId, mid: &MessageId) -> Result<Message> {
        let state = self.db.state(id).await?;
        Ok(Message {
            id: mid.clone(),
            text: state.queue.text(mid).ok_or("unknown message")?.into(),
            status: state.queue.status(mid).ok_or("unknown message")?.clone(),
        })
    }
    pub(crate) async fn tickets(&self) -> Result<Vec<Ticket>> {
        self.db
            .store(|s| {
                Ok(s.list_sessions()
                    .iter()
                    .filter_map(|session| s.session(&session.id))
                    .flat_map(|state| state.tickets.values().cloned())
                    .collect())
            })
            .await
    }
}

/// What the actor still owes the log after a write failed (a full disk, typically).
#[derive(Clone, Copy, PartialEq)]
enum Owed {
    /// The runtime died or was closed, but its interruption was never recorded.
    Interruption,
    /// The run finished, but the `session.settled` marker was never recorded.
    Settled,
}

#[derive(Default)]
struct Live {
    session: Option<Box<dyn Session>>,
    running: bool,
    stopped: bool,
    resolutions: HashSet<TicketId>,
    deleted: bool,
    owed: Option<Owed>,
}

async fn actor(shared: Shared, id: SessionId, mut commands: mpsc::Receiver<Command>) {
    let mut live = Live::default();
    let mut shutdown = shared.shutdown.subscribe();
    loop {
        let owed = live.owed.is_some();
        tokio::select! {
            biased;
            _ = shutdown.recv() => break,
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let shutdown = matches!(command.action, Action::Shutdown);
                let result = handle(&shared, &id, &mut live, command.action).await;
                if live.deleted {
                    // Deregister before replying so the caller observes a finished actor.
                    shared.actors.lock().await.remove(&id);
                }
                let _ = command.reply.send(result);
                if shutdown || live.deleted { break; }
            }
            event = async { match live.session.as_mut() { Some(session) => session.next().await, None => std::future::pending().await } } => {
                match event {
                    Some(RuntimeEvent::Settled) => {
                        live.running = false;
                        if !live.stopped {
                            // The front agent is free, but a running Task still has work (and
                            // its report) to come: the session is not settled yet.
                            if !crate::tasks::any_running(&shared, &id).await.unwrap_or(false)
                                && shared.emit(&id, None, EventKind::SessionSettled {}).await.is_err()
                            {
                                live.owed = Some(Owed::Settled);
                            }
                            let _ = dispatch(&shared, &id, &mut live).await;
                        }
                    }
                    Some(RuntimeEvent::Error { error }) => {
                        if fail(&shared, &id, &mut live, error).await.is_err() {
                            live.owed = Some(Owed::Interruption);
                        }
                    }
                    Some(event) => {
                        let lifecycle = match &event {
                            RuntimeEvent::SubagentLifecycle { subagent_id, .. } => Some(subagent_id.clone()),
                            _ => None,
                        };
                        if record(&shared, &id, event).await.is_err() {
                            let failed = RuntimeError::Unavailable("event persistence failed".into());
                            if fail(&shared, &id, &mut live, failed).await.is_err() {
                                live.owed = Some(Owed::Interruption);
                            }
                        } else if let Some(child) = lifecycle.filter(|_| !live.stopped) {
                            if crate::tasks::report_ended(&shared, &id).await.unwrap_or(false) {
                                let _ = dispatch(&shared, &id, &mut live).await;
                            } else if !live.running {
                                // The sidecar does not settle for an ended Task (its report
                                // usually follows); one without a report settles here.
                                let _ = crate::tasks::settle_unreported(&shared, &id, &child).await;
                            }
                        }
                    }
                    None => {
                        if interrupted(&shared, &id, &mut live).await.is_err() {
                            live.owed = Some(Owed::Interruption);
                        }
                    }
                }
            }
            _ = async {
                if owed {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                if recover_owed(&shared, &id, &mut live).await.is_ok() {
                    live.owed = None;
                }
            }
        }
    }
    if let Some(mut session) = live.session {
        let _ = session.shutdown().await;
    }
}

async fn handle(shared: &Shared, id: &SessionId, live: &mut Live, action: Action) -> Result<()> {
    match action {
        Action::Deliver => {
            live.stopped = false;
            dispatch(shared, id, live).await?;
        }
        Action::BrowserNotice(text) => {
            if !live.running || live.stopped {
                return Ok(());
            }
            shared
                .emit(
                    id,
                    None,
                    EventKind::MessageQueued {
                        message_id: MessageId::new(crate::persistence::random_id()),
                        text,
                        mode: QueueMode::Steer,
                        attachments: Vec::new(),
                    },
                )
                .await?;
            dispatch(shared, id, live).await?;
        }
        Action::Edit(mid, text) => {
            shared
                .emit(
                    id,
                    None,
                    EventKind::MessageEdited {
                        message_id: mid,
                        text,
                    },
                )
                .await?;
        }
        Action::Delete(mid) => {
            shared
                .emit(
                    id,
                    None,
                    EventKind::MessageDropped {
                        message_id: mid,
                        reason: openside_core::DropReason::Removed,
                    },
                )
                .await?;
        }
        Action::Promote(mid) => {
            shared
                .emit(id, None, EventKind::MessagePromoted { message_id: mid })
                .await?;
            dispatch(shared, id, live).await?;
        }
        Action::Move(mid, position) => {
            shared
                .emit(
                    id,
                    None,
                    EventKind::MessageMoved {
                        message_id: mid,
                        position,
                    },
                )
                .await?;
        }
        Action::Stop => {
            live.stopped = true;
            if let Some(session) = live.session.as_mut() {
                let _ = session.abort().await;
            }
            // Finish cancelling reverse-RPC futures before closing tickets, so an
            // in-flight human tool cannot open a new ticket after rejection.
            close_runtime(shared, id, live).await;
            crate::tickets::reject_open(shared, id, "stopped").await?;
            crate::subagents::stop_all(shared, id).await?;
            complete_inflight(shared, id, MessageOutcome::Aborted, "stopped").await?;
            settle_deferred(shared, id, MessageOutcome::Aborted, "stopped").await?;
            shared.emit(id, None, EventKind::SessionStopped {}).await?;
            shared.registry.browser.cleanup_warm(shared, id, None).await;
        }
        Action::Tombstone => {
            // Everything Stop does, then the tombstone, inside the actor: no queued Deliver can
            // reopen the runtime in between, because commands are handled one at a time.
            live.stopped = true;
            if let Some(session) = live.session.as_mut() {
                let _ = session.abort().await;
            }
            close_runtime(shared, id, live).await;
            crate::tickets::reject_open(shared, id, "deleted").await?;
            crate::subagents::stop_all(shared, id).await?;
            complete_inflight(shared, id, MessageOutcome::Aborted, "deleted").await?;
            settle_deferred(shared, id, MessageOutcome::Aborted, "deleted").await?;
            shared.emit(id, None, EventKind::SessionDeleted {}).await?;
            shared.registry.browser.cleanup_warm(shared, id, None).await;
            live.deleted = true;
        }
        Action::Resume(ticket) => {
            if shared.db.state(id).await?.status == SessionStatus::Stopped {
                return Ok(());
            }
            // REPL approvals requested over the API are claimed by the API caller's rerun,
            // not by the agent runtime, which never saw that tool call.
            if shared
                .db
                .state(id)
                .await?
                .tickets
                .get(&ticket)
                .is_some_and(|ticket| {
                    ticket
                        .tool_call_id
                        .starts_with(crate::tools::API_CALL_PREFIX)
                })
            {
                return Ok(());
            }
            open_runtime(shared, id, live).await?;
            if !live.resolutions.contains(&ticket) {
                let state = shared.db.state(id).await?;
                let ticket = state.tickets.get(&ticket).ok_or("unknown ticket")?;
                if ticket.status != TicketStatus::Pending
                    && !child_ended(shared, id, ticket).await?
                {
                    if let Some(session) = live.session.as_mut() {
                        let resolution =
                            crate::tickets::delivery_resolution(shared, ticket).await?;
                        if let Err(error) = session
                            .resolve(ticket.id.clone(), ticket.tool_call_id.clone(), resolution)
                            .await
                        {
                            fail(shared, id, live, error).await?;
                            return Err("ticket delivery failed".into());
                        }
                        live.resolutions.insert(ticket.id.clone());
                        live.running = true;
                    }
                }
            }
        }
        Action::Subagent(subagent, control) => {
            let Some(child) = crate::subagents::get(shared, id, &subagent).await? else {
                return Err("subagent_finished".into());
            };
            if child.status.is_terminal() {
                return Err("subagent_finished".into());
            }
            let cancel = control == SubagentControl::Cancel;
            if cancel {
                crate::subagents::cancel(shared, id, &subagent).await?;
            }
            open_runtime(shared, id, live).await?;
            if let Some(session) = live.session.as_mut() {
                if let Err(error) = session.control(subagent, control).await {
                    if matches!(&error, RuntimeError::Rejected(reason) if reason == "subagent_finished")
                    {
                        return Err("subagent_finished".into());
                    }
                    fail(shared, id, live, error).await?;
                    return Err("subagent control failed".into());
                }
            }
            if cancel && !live.stopped && crate::tasks::report_ended(shared, id).await? {
                dispatch(shared, id, live).await?;
            }
        }
        Action::Recover => {
            if !shared.db.state(id).await?.archived && !live.stopped {
                open_runtime(shared, id, live).await?;
            }
        }
        Action::Shutdown => {
            close_runtime(shared, id, live).await;
        }
    }
    Ok(())
}

/// Start the session's sidecar ahead of its next message.
pub(crate) async fn prewarm_runtime(shared: &Shared, id: &SessionId) {
    let _ = shared.command(id, Action::Recover).await;
}

async fn close_runtime(shared: &Shared, id: &SessionId, live: &mut Live) {
    if let Some(mut session) = live.session.take() {
        let _ = session.shutdown().await;
        // Shutdown may have buffered the final consistent snapshot before bye.
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(100), session.next()).await
        {
            let _ = record(shared, id, event).await;
        }
    }
    live.running = false;
    live.resolutions.clear();
}

async fn open_runtime(shared: &Shared, id: &SessionId, live: &mut Live) -> Result<()> {
    if live.session.is_some() {
        return Ok(());
    }
    let state = shared.db.state(id).await?;
    let host = Host::new(shared.clone(), &state);
    let tools = host.specs();
    let stored_id = id.clone();
    let resume_state = shared
        .db
        .store(move |s| Ok(s.agent_state(&stored_id)?))
        .await?;
    let system_prompt_parts =
        crate::profiles::system_prompt_parts(shared, state.workspace.as_deref()).await;
    let mut agent_json = agent_options(&shared.config.loaded.config);
    if crate::tasks::front(shared) {
        agent_json["front_tools"] = serde_json::json!(crate::tasks::front_tools(&tools));
        if let Some(front) = &shared.config.loaded.config.models.front {
            agent_json["front_model"] = serde_json::json!(front);
        }
    }
    let spec = SessionSpec {
        session: id.clone(),
        model: state.model,
        fallback_models: shared.config.loaded.config.models.fallback.clone(),
        system_prompt_parts,
        tools,
        resume_state,
        credentials_json: shared.config.loaded.config.credentials.to_string(),
        agent_json: agent_json.to_string(),
    };
    let mut session = match shared.runtime.open(spec, Arc::new(host)).await {
        Ok(session) => session,
        Err(error) => {
            fail(shared, id, live, error).await?;
            return Err("runtime startup failed".into());
        }
    };
    shared
        .emit(
            id,
            None,
            EventKind::RuntimeStarted {
                tools: session.info().tools.clone(),
                startup_ms: session.info().startup_ms,
            },
        )
        .await?;
    if shared.config.bwrap.is_none() && !shared.config.loaded.config.sandbox.required {
        crate::diag!(
            "opensided: shell tools use policy-only isolation because bubblewrap is unavailable"
        );
        shared.emit(id, None, EventKind::RuntimeError { code: "sandbox_unavailable".into(), message: "Bubblewrap is unavailable; shell tools use policy-only isolation. Install bubblewrap or set sandbox.required=true.".into() }).await?;
    }
    live.resolutions.clear();
    let pending = session.info().pending_deferred.clone();
    let state = shared.db.state(id).await?;
    // A child is resumable only through a deferred call the restored sidecar still holds;
    // a ticket opened after the last snapshot belongs to a child the sidecar never saved.
    let known: HashSet<String> = pending
        .iter()
        .filter_map(|call| state.tickets.get(&call.ticket))
        .filter_map(|ticket| ticket.origin.subagent_id.clone())
        .collect();
    crate::subagents::reconcile(shared, id, &known).await?;
    let tickets: HashSet<TicketId> = pending.iter().map(|call| call.ticket.clone()).collect();
    if crate::subagents::continuation_finished(shared, id, &tickets).await? {
        // Its terminal output is durable but its snapshot is not; resuming would run the
        // continuation again and emit a second final.
        crate::subagents::discard_all(shared, id).await?;
        if let Err(error) = session.discard().await {
            fail(shared, id, live, error).await?;
            return Err("continuation discard failed".into());
        }
        live.session = Some(session);
        return Ok(());
    }
    let state = shared.db.state(id).await?;
    let mut deliver = Vec::new();
    let mut cancelled = HashSet::new();
    for call in pending {
        let Some(ticket) = state.tickets.get(&call.ticket) else {
            continue;
        };
        if ticket.tool_call_id != call.tool_call_id {
            return Err("deferred ticket correlation mismatch".into());
        }
        if child_ended(shared, id, ticket).await? {
            cancelled.extend(ticket.origin.subagent_id.clone());
        } else if ticket.status != TicketStatus::Pending {
            deliver.push(ticket);
        }
    }
    // Cancel ended children first: their parent `delegate` results must be in place
    // before a sibling's resolution lets the parent resume.
    for subagent in cancelled {
        if let Err(error) = session.control(subagent, SubagentControl::Cancel).await {
            fail(shared, id, live, error).await?;
            return Err("subagent recovery failed".into());
        }
        live.running = true;
    }
    for ticket in deliver {
        let resolution = crate::tickets::delivery_resolution(shared, ticket).await?;
        if let Err(error) = session
            .resolve(ticket.id.clone(), ticket.tool_call_id.clone(), resolution)
            .await
        {
            fail(shared, id, live, error).await?;
            return Err("ticket recovery failed".into());
        }
        live.resolutions.insert(ticket.id.clone());
        live.running = true;
    }
    live.session = Some(session);
    Ok(())
}

/// The sidecar already dropped an ended child's deferred calls; resolving one would
/// be a stale ticket, which the actor treats as a runtime failure.
async fn child_ended(shared: &Shared, id: &SessionId, ticket: &Ticket) -> Result<bool> {
    let Some(subagent) = &ticket.origin.subagent_id else {
        return Ok(false);
    };
    Ok(crate::subagents::get(shared, id, subagent)
        .await?
        .map_or(true, |child| child.status.is_terminal()))
}

/// A pending ticket holds every queue lane, except one opened by a background Task: the
/// front agent keeps talking while that Task waits for its person.
async fn holds_queue(shared: &Shared, id: &SessionId, state: &SessionState) -> Result<bool> {
    let pending: Vec<_> = state
        .tickets
        .values()
        .filter(|t| t.status == TicketStatus::Pending)
        .collect();
    if pending.is_empty() {
        return Ok(false);
    }
    let tasks: HashSet<String> = crate::subagents::list(shared, id)
        .await?
        .into_iter()
        .filter(|child| child.background)
        .map(|child| child.id)
        .collect();
    Ok(pending.iter().any(|ticket| {
        ticket
            .origin
            .subagent_id
            .as_ref()
            .map_or(true, |child| !tasks.contains(child))
    }))
}

async fn dispatch(shared: &Shared, id: &SessionId, live: &mut Live) -> Result<()> {
    loop {
        let state = shared.db.state(id).await?;
        if holds_queue(shared, id, &state).await? {
            return Ok(());
        }
        let Some(mid) = state.queue.pending_ids().next() else {
            return Ok(());
        };
        let mode = match state.queue.status(mid) {
            Some(MessageStatus::Queued(mode)) => *mode,
            _ => return Err("queue projection invalid".into()),
        };
        if live.running && mode == QueueMode::Queue {
            return Ok(());
        }
        open_runtime(shared, id, live).await?;
        // Recovery may have started a ticket-resumption run.
        if live.running && mode == QueueMode::Queue {
            return Ok(());
        }
        let text = state
            .queue
            .text(mid)
            .ok_or("queue text missing")?
            .to_owned();
        shared
            .emit(
                id,
                Some(mid.as_str().into()),
                EventKind::MessageInjected {
                    message_id: mid.clone(),
                },
            )
            .await?;
        if let Some(session) = live.session.as_mut() {
            let parts =
                crate::artifacts::message_parts(shared, id, state.attachments.get(mid)).await;
            if let Err(error) = session.send(mid.clone(), text, parts, mode).await {
                fail(shared, id, live, error).await?;
                return Err("runtime delivery failed".into());
            }
        }
        live.running = true;
    }
}

async fn complete_inflight(
    shared: &Shared,
    id: &SessionId,
    outcome: MessageOutcome,
    reason: &str,
) -> Result<()> {
    let state = shared.db.state(id).await?;
    for mid in state
        .injected
        .iter()
        .filter(|mid| !state.completions.contains_key(*mid))
    {
        shared
            .emit(
                id,
                Some(mid.as_str().into()),
                EventKind::MessageCompleted {
                    message_id: mid.clone(),
                    outcome,
                    reason: Some(reason.into()),
                },
            )
            .await?;
    }
    Ok(())
}

/// Redoes what a failed write left undone, so a session does not read "running" forever after the
/// disk filled mid-run. Every step checks the log's tail first, so a retry after a partial
/// success never records the interruption or the settle marker twice.
async fn recover_owed(shared: &Shared, id: &SessionId, live: &mut Live) -> Result<()> {
    let Some(owed) = live.owed else {
        return Ok(());
    };
    if live.running || live.session.is_some() {
        // A newer run took over; its own events settle the session.
        return Ok(());
    }
    if owed == Owed::Interruption {
        complete_inflight(shared, id, MessageOutcome::Failed, "interrupted").await?;
        crate::subagents::interrupt_running(shared, id).await?;
        crate::tasks::report_ended(shared, id).await?;
    }
    let last_seq = shared.db.state(id).await?.last_seq;
    let tail = shared
        .db
        .events(id, last_seq.saturating_sub(1))
        .await?
        .pop()
        .map(|event| event.kind);
    if matches!(
        tail,
        Some(EventKind::SessionSettled {} | EventKind::SessionStopped {})
    ) {
        if !live.stopped {
            dispatch(shared, id, live).await?;
        }
        return Ok(());
    }
    if owed == Owed::Interruption && !matches!(tail, Some(EventKind::RuntimeError { .. })) {
        shared
            .emit(
                id,
                None,
                EventKind::RuntimeError {
                    code: "interrupted".into(),
                    message: "Agent process interrupted; dispatched messages will not be resent."
                        .into(),
                },
            )
            .await?;
    }
    if !live.stopped {
        shared.emit(id, None, EventKind::SessionSettled {}).await?;
        dispatch(shared, id, live).await?;
    }
    Ok(())
}

async fn interrupted(shared: &Shared, id: &SessionId, live: &mut Live) -> Result<()> {
    live.session = None;
    live.running = false;
    live.resolutions.clear();
    complete_inflight(shared, id, MessageOutcome::Failed, "interrupted").await?;
    crate::subagents::interrupt_running(shared, id).await?;
    crate::tasks::report_ended(shared, id).await?;
    shared
        .emit(
            id,
            None,
            EventKind::RuntimeError {
                code: "interrupted".into(),
                message: "Agent process interrupted; dispatched messages will not be resent."
                    .into(),
            },
        )
        .await?;
    if !live.stopped {
        shared.emit(id, None, EventKind::SessionSettled {}).await?;
        // Queued messages were never handed to the dead process; start a fresh one from the
        // last snapshot. A startup failure goes through `fail`, which does not redispatch.
        dispatch(shared, id, live).await?;
    }
    Ok(())
}

async fn fail(shared: &Shared, id: &SessionId, live: &mut Live, error: RuntimeError) -> Result<()> {
    let auth = matches!(error, RuntimeError::AuthUnavailable(_));
    close_runtime(shared, id, live).await;
    crate::subagents::interrupt_running(shared, id).await?;
    crate::tasks::report_ended(shared, id).await?;
    complete_inflight(
        shared,
        id,
        MessageOutcome::Failed,
        if auth {
            "auth_unavailable"
        } else {
            "interrupted"
        },
    )
    .await?;
    shared
        .emit(
            id,
            None,
            EventKind::RuntimeError {
                code: if auth {
                    "auth_unavailable"
                } else {
                    "runtime_unavailable"
                }
                .into(),
                message: if auth {
                    "Configured credential sources are unavailable; the session is stopped."
                } else {
                    "Agent runtime unavailable; retrying a queued message will start a new process."
                }
                .into(),
            },
        )
        .await?;
    live.stopped = auth;
    shared
        .emit(
            id,
            None,
            if auth {
                EventKind::SessionStopped {}
            } else {
                EventKind::SessionSettled {}
            },
        )
        .await?;
    Ok(())
}

/// Settle messages whose run paused on a ticket (`deferred`) and will never resume.
async fn settle_deferred(
    shared: &Shared,
    id: &SessionId,
    outcome: MessageOutcome,
    reason: &str,
) -> Result<()> {
    let state = shared.db.state(id).await?;
    for (mid, _) in state
        .completions
        .iter()
        .filter(|(_, done)| **done == MessageOutcome::Deferred)
    {
        shared
            .emit(
                id,
                Some(mid.as_str().into()),
                EventKind::MessageCompleted {
                    message_id: mid.clone(),
                    outcome,
                    reason: Some(reason.into()),
                },
            )
            .await?;
    }
    Ok(())
}

async fn record(shared: &Shared, id: &SessionId, event: RuntimeEvent) -> Result<()> {
    // The sidecar re-reports originating messages when a deferred run finishes; a message
    // the daemon already settled (for example by stopping the session) stays settled.
    if let RuntimeEvent::MessageResult {
        id: message_id,
        outcome,
        ..
    } = &event
    {
        let state = shared.db.state(id).await?;
        if let Some(done) = state.completions.get(message_id) {
            if *done != MessageOutcome::Deferred || *outcome == RunOutcome::Deferred {
                return Ok(());
            }
        }
    }
    let kind = match event {
        RuntimeEvent::AssistantDelta { text } => EventKind::AssistantDelta { text },
        RuntimeEvent::AssistantFinal { text } => EventKind::AssistantFinal { text },
        RuntimeEvent::ThinkingDelta { text } => EventKind::ThinkingDelta { text },
        RuntimeEvent::TurnStarted => EventKind::TurnStarted {},
        RuntimeEvent::TurnEnded => EventKind::TurnEnded {},
        RuntimeEvent::Usage {
            model,
            input_tokens,
            output_tokens,
        } => EventKind::Usage {
            model,
            input_tokens,
            output_tokens,
        },
        RuntimeEvent::MessageResult {
            id: message_id,
            outcome,
            reason,
        } => EventKind::MessageCompleted {
            message_id,
            reason,
            outcome: match outcome {
                RunOutcome::Completed => MessageOutcome::Completed,
                RunOutcome::Aborted => MessageOutcome::Aborted,
                RunOutcome::Failed => MessageOutcome::Failed,
                RunOutcome::Deferred => MessageOutcome::Deferred,
            },
        },
        RuntimeEvent::StateSnapshot { state } => {
            let id = id.clone();
            shared
                .db
                .store(move |s| Ok(s.put_agent_state(&id, &state)?))
                .await?;
            return Ok(());
        }
        RuntimeEvent::Deferred { .. } | RuntimeEvent::MessageAccepted { .. } => return Ok(()),
        RuntimeEvent::SubagentLifecycle {
            subagent_id,
            status,
            output,
        } => match crate::subagents::lifecycle(shared, id, subagent_id, status, output).await? {
            Some(kind) => kind,
            None => return Ok(()),
        },
        RuntimeEvent::SubagentProgress {
            subagent_id,
            progress_json,
        } => EventKind::SubagentProgress {
            subagent_id,
            progress: serde_json::from_str(&progress_json)?,
        },
        RuntimeEvent::Settled => EventKind::SessionSettled {},
        RuntimeEvent::Error { .. } => return Ok(()),
    };
    shared.emit(id, None, kind).await?;
    Ok(())
}
