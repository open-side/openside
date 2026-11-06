//! Native computer use: a person connects one app window to a session; the agent then sees
//! frames of that window only, and its input lands only inside it.
//!
//! Invariants enforced here, beneath the daemon's policy and approval layer:
//! * A session has at most one connection. [`Computer::stop`] ends it at once: an in-flight
//!   action is cancelled (helper processes are killed with it) and the connection id changes on
//!   every connect, so an approval or frame from an earlier connection never acts on a later one.
//! * Pointer coordinates are pixels of the latest frame of the connection. The live window must
//!   still be the same app and process and keep the size it had in that frame.
//! * A point outside the window is refused before anything is focused, moved or clicked.
//! * Before every input the backend focuses the window and confirms it is the active one;
//!   otherwise nothing is sent.
//! * Browsers, password managers and openside itself cannot be connected: browser work keeps
//!   its site policy through the browser tools, and secrets and approvals stay with the person.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[cfg(any(test, feature = "fake"))]
pub mod fake;
#[cfg(target_os = "linux")]
mod hyprland;
mod keys;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod uinput;

pub use keys::{Key, KeyCombo, Modifier, NamedKey};

/// Window rectangle in the desktop's logical coordinates (points, not device pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// One top-level window of a desktop app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Window {
    /// Backend handle, stable while the window exists.
    pub id: String,
    /// App identity: Wayland app id or X11 class on Linux, owner name on macOS.
    pub app: String,
    /// Window title.
    pub title: String,
    /// Owning process.
    pub pid: u32,
    /// Position and size.
    pub bounds: Rect,
    /// On a visible workspace and not minimized.
    pub visible: bool,
    /// Path of the owning process's executable, read from the operating system rather than
    /// from anything the window says about itself.
    #[serde(skip)]
    pub program: Option<String>,
    /// Identity of this window that the system never hands to a later window, even when the
    /// backend id or the pid is reused: Hyprland's stable id, the macOS window number.
    #[serde(skip)]
    pub instance: String,
}

/// A window as offered to the person and the agent, with the reason it cannot be connected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct App {
    /// The window.
    #[serde(flatten)]
    pub window: Window,
    /// Set when the window can never be connected.
    pub blocked: Option<&'static str>,
}

/// Whether one part of computer use works on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Feature {
    /// Usable now.
    pub available: bool,
    /// Mechanism in use, such as `grim` or `uinput`.
    pub via: Option<String>,
    /// Why it is not usable; plain language.
    pub reason: Option<String>,
}

impl Feature {
    /// A usable feature.
    pub fn via(mechanism: &str) -> Self {
        Self {
            available: true,
            via: Some(mechanism.into()),
            reason: None,
        }
    }
    /// An unusable feature.
    pub fn missing(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            via: None,
            reason: Some(reason.into()),
        }
    }
}

/// What computer use can do on this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Eligibility {
    /// Operating system family.
    pub platform: &'static str,
    /// Desktop backend, when one is supported here.
    pub backend: Option<&'static str>,
    /// Window frames.
    pub capture: Feature,
    /// Clicks, moves and scrolling.
    pub pointer: Feature,
    /// Typing and key presses.
    pub keyboard: Feature,
}

impl Eligibility {
    /// Frames work and at least one kind of input does.
    pub fn eligible(&self) -> bool {
        self.capture.available && (self.pointer.available || self.keyboard.available)
    }
    /// Plain-language reasons for every unusable part.
    pub fn reasons(&self) -> Vec<String> {
        [&self.capture, &self.pointer, &self.keyboard]
            .iter()
            .filter_map(|feature| feature.reason.clone())
            .collect()
    }
    /// Nothing works, for one reason.
    pub fn unsupported(platform: &'static str, reason: &str) -> Self {
        Self {
            platform,
            backend: None,
            capture: Feature::missing(reason),
            pointer: Feature::missing(reason),
            keyboard: Feature::missing(reason),
        }
    }
}

/// An encoded window frame.
#[derive(Clone, Debug)]
pub struct Frame {
    /// PNG bytes.
    pub png: Vec<u8>,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
}

/// Mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    /// Primary.
    Left,
    /// Secondary.
    Right,
    /// Wheel button.
    Middle,
}

/// An agent action in frame pixels of the connection's latest frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Press and release a button `count` times (1 or 2).
    Click {
        /// Frame x.
        x: f64,
        /// Frame y.
        y: f64,
        /// Button.
        button: Button,
        /// Clicks.
        count: u8,
    },
    /// Move the pointer without pressing.
    Move {
        /// Frame x.
        x: f64,
        /// Frame y.
        y: f64,
    },
    /// Turn the wheel over a point; positive `dy` scrolls down, positive `dx` right.
    Scroll {
        /// Frame x.
        x: f64,
        /// Frame y.
        y: f64,
        /// Horizontal wheel steps.
        dx: i32,
        /// Vertical wheel steps.
        dy: i32,
    },
    /// Type text into the focused control.
    Type {
        /// Text.
        text: String,
    },
    /// Press a key, possibly with modifiers.
    Key {
        /// Parsed combination.
        combo: KeyCombo,
    },
}

impl Action {
    fn point(&self) -> Option<(f64, f64)> {
        match self {
            Self::Click { x, y, .. } | Self::Move { x, y } | Self::Scroll { x, y, .. } => {
                Some((*x, *y))
            }
            Self::Type { .. } | Self::Key { .. } => None,
        }
    }
}

/// Input in desktop logical coordinates, as a backend sends it.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// See [`Action::Click`].
    Click {
        /// Desktop x.
        x: f64,
        /// Desktop y.
        y: f64,
        /// Button.
        button: Button,
        /// Clicks.
        count: u8,
    },
    /// See [`Action::Move`].
    Move {
        /// Desktop x.
        x: f64,
        /// Desktop y.
        y: f64,
    },
    /// See [`Action::Scroll`].
    Scroll {
        /// Desktop x.
        x: f64,
        /// Desktop y.
        y: f64,
        /// Horizontal steps.
        dx: i32,
        /// Vertical steps.
        dy: i32,
    },
    /// See [`Action::Type`].
    Type {
        /// Text.
        text: String,
    },
    /// See [`Action::Key`].
    Key {
        /// Combination.
        combo: KeyCombo,
    },
}

impl Input {
    /// Whether the input needs pointer support (otherwise keyboard).
    pub fn pointer(&self) -> bool {
        matches!(
            self,
            Self::Click { .. } | Self::Move { .. } | Self::Scroll { .. }
        )
    }
}

/// Why a computer-use operation did not happen. [`Error::code`] is stable for API and tool
/// output; [`std::fmt::Display`] is a plain sentence without window contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The session has no connection, or not the one named.
    NotConnected,
    /// The person stopped computer use while the operation ran.
    Stopped,
    /// The connected window closed or now belongs to another app or process.
    WindowGone,
    /// The window was resized after the latest frame.
    WindowChanged,
    /// A pointer action came before any frame of this connection.
    NoFrame,
    /// The point lies outside the window.
    OutsideWindow,
    /// The window could not be made the active window.
    NotFocused,
    /// Another window covers the point.
    Covered,
    /// This window can never be connected.
    Blocked(&'static str),
    /// No such window.
    UnknownWindow,
    /// The part of computer use this needs does not work here.
    Unavailable(String),
    /// Invalid action arguments.
    Invalid(String),
    /// The desktop refused or a helper failed.
    Failed(String),
}

impl Error {
    /// Stable machine code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConnected => "computer_not_connected",
            Self::Stopped => "computer_use_stopped",
            Self::WindowGone => "computer_window_gone",
            Self::WindowChanged => "computer_window_changed",
            Self::NoFrame => "computer_frame_required",
            Self::OutsideWindow => "computer_outside_window",
            Self::NotFocused => "computer_not_focused",
            Self::Covered => "computer_window_covered",
            Self::Blocked(_) => "computer_app_blocked",
            Self::UnknownWindow => "computer_window_not_found",
            Self::Unavailable(_) => "computer_use_unavailable",
            Self::Invalid(_) => "invalid_value",
            Self::Failed(_) => "computer_use_failed",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => f.write_str("No app is connected; connect one first"),
            Self::Stopped => f.write_str("The person stopped computer use"),
            Self::WindowGone => f.write_str("The connected window closed; connect an app again"),
            Self::WindowChanged => {
                f.write_str("The window was resized; take a new screenshot and retry")
            }
            Self::NoFrame => f.write_str("Take a screenshot of the connected window first"),
            Self::OutsideWindow => f.write_str("That point is outside the connected window"),
            Self::NotFocused => f.write_str("The connected window could not be brought to front"),
            Self::Covered => f.write_str("Another window covers that point"),
            Self::Blocked(reason) => f.write_str(reason),
            Self::UnknownWindow => f.write_str("No such window; list the apps again"),
            Self::Unavailable(reason) | Self::Invalid(reason) | Self::Failed(reason) => {
                f.write_str(reason)
            }
        }
    }
}

/// A desktop backend. Every future must be cancel-safe: dropping it must stop the work and
/// kill helper processes it started (`kill_on_drop`).
pub trait Backend: Send + Sync {
    /// What works on this computer right now.
    fn eligibility(&self) -> BoxFuture<'_, Eligibility>;
    /// Top-level app windows, front to back where the desktop reports an order.
    fn windows(&self) -> BoxFuture<'_, Result<Vec<Window>, Error>>;
    /// A frame of exactly this window, never of what overlaps it.
    fn capture<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<Frame, Error>>;
    /// Make the window the active one and confirm it is; [`Error::NotFocused`] otherwise.
    fn focus<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, Result<(), Error>>;
    /// Send input to the focused `window`. Pointer input must refuse a point that another
    /// window covers ([`Error::Covered`]).
    fn input<'a>(
        &'a self,
        window: &'a Window,
        input: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>>;
    /// Free per-connection resources such as virtual input devices.
    fn release(&self) {}
}

/// Why `window` can never be connected, if it cannot.
pub fn blocked(window: &Window) -> Option<&'static str> {
    const BROWSERS: &[&str] = &[
        "chrom",
        "firefox",
        "brave",
        "msedge",
        "microsoft-edge",
        "vivaldi",
        "opera",
        "safari",
        "librewolf",
        "zen-browser",
        "zen_browser",
        "epiphany",
        "qutebrowser",
        "aside",
    ];
    const SECRETS: &[&str] = &[
        "1password",
        "bitwarden",
        "keepass",
        "lastpass",
        "dashlane",
        "enpass",
        "proton pass",
        "proton-pass",
        "seahorse",
        "keychain",
        "pinentry",
        "polkit",
    ];
    // The app id and title are whatever the window claims; the executable path is not.
    let Some(program) = window.program.as_deref() else {
        return Some("openside cannot tell which program owns this window");
    };
    let app = format!("{} {}", window.app.to_lowercase(), program.to_lowercase());
    if app.contains("openside") || window.pid == std::process::id() {
        Some("openside cannot control itself")
    } else if BROWSERS.iter().any(|name| app.contains(name)) {
        Some("Browsers are used through the browser tools, which keep site rules")
    } else if SECRETS.iter().any(|name| app.contains(name)) {
        Some("Password and sign-in prompts stay with the person")
    } else {
        None
    }
}

/// A short fingerprint of an executable path for a window's `instance`, which puts the
/// executable into every approval. Only compared within one daemon run, so the standard
/// hasher's fixed keys are enough.
pub(crate) fn digest(program: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut hasher);
    hasher.finish()
}

/// `text` without control characters (newlines included), at most `max` characters.
fn printable(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The latest frame's size and the window size it was taken at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct View {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Window width when captured.
    pub window_width: u32,
    /// Window height when captured.
    pub window_height: u32,
}

/// Where the agent's pointer is, in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Cursor {
    /// Frame x.
    pub x: f64,
    /// Frame y.
    pub y: f64,
    /// A button is down. Every action releases what it pressed, so this is only set while
    /// a click is being sent.
    pub held: bool,
}

/// What a connection is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Waiting for the agent.
    Connected,
    /// Sending input.
    Acting,
    /// Capturing a frame.
    Capturing,
}

/// One session's connected app.
#[derive(Clone, Debug, Serialize)]
pub struct Connection {
    /// Changes on every connect; part of every approval target.
    pub id: String,
    /// Monotonic across the daemon's lifetime.
    pub generation: u64,
    /// The window as of the latest check.
    pub window: Window,
    /// Current activity.
    pub phase: Phase,
    /// Latest frame geometry; pointer actions need it.
    pub view: Option<View>,
    /// Pointer after the latest pointer action.
    pub cursor: Option<Cursor>,
}

struct Entry {
    connection: Connection,
    stop: CancellationToken,
    serial: Arc<tokio::sync::Mutex<()>>,
}

/// Connections of every session over one backend.
pub struct Computer {
    backend: Arc<dyn Backend>,
    sessions: Mutex<HashMap<String, Entry>>,
    generation: AtomicU64,
    changes: watch::Sender<u64>,
}

impl Computer {
    /// Over `backend`.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            sessions: Mutex::default(),
            generation: AtomicU64::new(0),
            changes: watch::channel(0).0,
        }
    }

    /// Over this computer's desktop.
    pub fn for_platform() -> Self {
        Self::new(platform_backend())
    }

    /// Bumped whenever any connection changes; the value is meaningless.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes
            .send_modify(|value| *value = value.wrapping_add(1));
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What works on this computer.
    pub async fn eligibility(&self) -> Eligibility {
        self.backend.eligibility().await
    }

    /// Visible app windows with why any of them cannot be connected.
    pub async fn apps(&self) -> Result<Vec<App>, Error> {
        Ok(self
            .usable()
            .await?
            .into_iter()
            .map(|window| App {
                blocked: blocked(&window),
                window,
            })
            .collect())
    }

    /// Windows that are mapped and on screen, with names safe to show in a question. A hidden
    /// or minimized window is treated as gone, so it can be neither connected nor driven.
    async fn usable(&self) -> Result<Vec<Window>, Error> {
        Ok(self
            .backend
            .windows()
            .await?
            .into_iter()
            .filter(|window| window.visible && window.bounds.width > 0 && window.bounds.height > 0)
            .map(|mut window| {
                window.app = printable(&window.app, 80);
                window.title = printable(&window.title, 80);
                window
            })
            .collect())
    }

    async fn live(&self, id: &str) -> Result<Window, Error> {
        self.usable()
            .await?
            .into_iter()
            .find(|window| window.id == id)
            .ok_or(Error::UnknownWindow)
    }

    /// Resolve a window the person may be asked to connect.
    pub async fn window(&self, id: &str) -> Result<Window, Error> {
        let window = self.live(id).await?;
        match blocked(&window) {
            Some(reason) => Err(Error::Blocked(reason)),
            None => Ok(window),
        }
    }

    /// Connect `window_id` to `session`, replacing (and stopping) any earlier connection.
    /// `instance`, `app` and `pid` are what the person approved; any other window under that id,
    /// including a later window that reuses it, fails.
    pub async fn connect(
        &self,
        session: &str,
        window_id: &str,
        instance: &str,
        app: &str,
        pid: u32,
    ) -> Result<Connection, Error> {
        let eligibility = self.eligibility().await;
        if !eligibility.eligible() {
            return Err(Error::Unavailable(eligibility.reasons().join("; ")));
        }
        let window = self.window(window_id).await?;
        if window.instance != instance || window.app != app || window.pid != pid {
            return Err(Error::WindowGone);
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let connection = Connection {
            id: format!("cu{generation}"),
            generation,
            window,
            phase: Phase::Connected,
            view: None,
            cursor: None,
        };
        let previous = self.lock().insert(
            session.to_owned(),
            Entry {
                connection: connection.clone(),
                stop: CancellationToken::new(),
                serial: Arc::default(),
            },
        );
        if let Some(previous) = previous {
            // The input device is shared by every connection, so replacing one keeps it.
            previous.stop.cancel();
        }
        self.changed();
        Ok(connection)
    }

    /// The session's connection.
    pub fn connection(&self, session: &str) -> Option<Connection> {
        self.lock()
            .get(session)
            .map(|entry| entry.connection.clone())
    }

    /// Sessions with a connection.
    pub fn sessions(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn entry(
        &self,
        session: &str,
        id: &str,
    ) -> Result<(CancellationToken, Arc<tokio::sync::Mutex<()>>), Error> {
        let sessions = self.lock();
        let entry = sessions
            .get(session)
            .filter(|entry| entry.connection.id == id)
            .ok_or(Error::NotConnected)?;
        Ok((entry.stop.clone(), entry.serial.clone()))
    }

    fn update(&self, session: &str, id: &str, change: impl FnOnce(&mut Connection)) -> bool {
        let updated = match self.lock().get_mut(session) {
            Some(entry) if entry.connection.id == id => {
                change(&mut entry.connection);
                true
            }
            _ => false,
        };
        if updated {
            self.changed();
        }
        updated
    }

    /// The live window of the connection, checked to be the same app and process.
    async fn verified(&self, session: &str, id: &str) -> Result<Window, Error> {
        let expected = self
            .lock()
            .get(session)
            .filter(|entry| entry.connection.id == id)
            .map(|entry| entry.connection.window.clone())
            .ok_or(Error::NotConnected)?;
        let window = match self.live(&expected.id).await {
            Ok(window) => window,
            Err(Error::UnknownWindow) => return Err(Error::WindowGone),
            Err(error) => return Err(error),
        };
        // The backends fold the executable into `instance`; comparing it directly as well keeps
        // that true for any backend.
        if window.instance != expected.instance
            || window.program != expected.program
            || window.app != expected.app
            || window.pid != expected.pid
        {
            return Err(Error::WindowGone);
        }
        Ok(window)
    }

    async fn cancellable<T>(
        stop: &CancellationToken,
        work: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        tokio::select! {
            biased;
            () = stop.cancelled() => Err(Error::Stopped),
            result = work => result,
        }
    }

    /// A frame of the connected window. Call [`Self::viewed`] with the size the agent is shown.
    pub async fn capture(&self, session: &str, id: &str) -> Result<(Frame, Window), Error> {
        let (stop, serial) = self.entry(session, id)?;
        let _serial = Self::cancellable(&stop, async { Ok(serial.lock().await) }).await?;
        self.update(session, id, |connection| {
            connection.phase = Phase::Capturing
        });
        let result = Self::cancellable(&stop, async {
            let window = self.verified(session, id).await?;
            let frame = self.backend.capture(&window).await?;
            Ok((frame, window))
        })
        .await;
        self.update(session, id, |connection| {
            connection.phase = Phase::Connected;
            if let Ok((_, window)) = &result {
                connection.window = window.clone();
            }
        });
        result
    }

    /// Record that the agent was shown a `width` x `height` frame of `window`: later pointer
    /// coordinates are read in that frame.
    pub fn viewed(
        &self,
        session: &str,
        id: &str,
        width: u32,
        height: u32,
        window: &Window,
    ) -> Result<(), Error> {
        if width == 0 || height == 0 {
            return Err(Error::Invalid("Empty frame".into()));
        }
        let view = View {
            width,
            height,
            window_width: window.bounds.width,
            window_height: window.bounds.height,
        };
        if self.update(session, id, |connection| {
            connection.view = Some(view);
            // A cursor drawn in an older frame's pixels would point elsewhere now.
            connection.cursor = connection.cursor.map(|cursor| Cursor {
                x: cursor.x.min(f64::from(width) - 1.0),
                y: cursor.y.min(f64::from(height) - 1.0),
                held: false,
            });
        }) {
            Ok(())
        } else {
            Err(Error::NotConnected)
        }
    }

    /// Check an action against the connection without running it: the point must lie in the
    /// latest frame. Tools call this before asking the person, so a refused point never
    /// produces an approval request.
    pub fn check(&self, session: &str, id: &str, action: &Action) -> Result<(), Error> {
        let view = self
            .lock()
            .get(session)
            .filter(|entry| entry.connection.id == id)
            .map(|entry| entry.connection.view)
            .ok_or(Error::NotConnected)?;
        validate(action)?;
        if let Some((x, y)) = action.point() {
            let view = view.ok_or(Error::NoFrame)?;
            local(view, x, y)?;
        }
        Ok(())
    }

    /// Run `action` in the connected window.
    pub async fn act(&self, session: &str, id: &str, action: Action) -> Result<Connection, Error> {
        validate(&action)?;
        let eligibility = self.eligibility().await;
        let (stop, serial) = self.entry(session, id)?;
        let _serial = Self::cancellable(&stop, async { Ok(serial.lock().await) }).await?;
        let view = self
            .connection(session)
            .filter(|connection| connection.id == id)
            .ok_or(Error::NotConnected)?
            .view;
        let input = match &action {
            Action::Click { .. } | Action::Move { .. } | Action::Scroll { .. }
                if !eligibility.pointer.available =>
            {
                return Err(Error::Unavailable(
                    eligibility.pointer.reason.unwrap_or_default(),
                ))
            }
            Action::Type { .. } | Action::Key { .. } if !eligibility.keyboard.available => {
                return Err(Error::Unavailable(
                    eligibility.keyboard.reason.unwrap_or_default(),
                ))
            }
            _ => action.clone(),
        };
        // Bounds are checked against the latest frame before the window is even looked up.
        let point = match input.point() {
            Some((x, y)) => Some(local(view.ok_or(Error::NoFrame)?, x, y)?),
            None => None,
        };
        let result = Self::cancellable(&stop, async {
            let window = self.verified(session, id).await?;
            if let (Some(view), Some(_)) = (view, point) {
                if (view.window_width, view.window_height)
                    != (window.bounds.width, window.bounds.height)
                {
                    return Err(Error::WindowChanged);
                }
            }
            self.update(session, id, |connection| {
                connection.phase = Phase::Acting;
                connection.window = window.clone();
                if let (Action::Click { x, y, .. }, true) = (&input, point.is_some()) {
                    connection.cursor = Some(Cursor {
                        x: *x,
                        y: *y,
                        held: true,
                    });
                }
            });
            self.backend.focus(&window).await?;
            let desktop = |(lx, ly): (f64, f64)| {
                (
                    f64::from(window.bounds.x) + lx,
                    f64::from(window.bounds.y) + ly,
                )
            };
            let sent = match (&input, point) {
                (Action::Click { button, count, .. }, Some(point)) => {
                    let (x, y) = desktop(point);
                    Input::Click {
                        x,
                        y,
                        button: *button,
                        count: *count,
                    }
                }
                (Action::Move { .. }, Some(point)) => {
                    let (x, y) = desktop(point);
                    Input::Move { x, y }
                }
                (Action::Scroll { dx, dy, .. }, Some(point)) => {
                    let (x, y) = desktop(point);
                    Input::Scroll {
                        x,
                        y,
                        dx: *dx,
                        dy: *dy,
                    }
                }
                (Action::Type { text }, None) => Input::Type { text: text.clone() },
                (Action::Key { combo }, None) => Input::Key {
                    combo: combo.clone(),
                },
                _ => return Err(Error::Invalid("Malformed action".into())),
            };
            self.backend.input(&window, &sent).await
        })
        .await;
        let cursor = input.point().map(|(x, y)| Cursor { x, y, held: false });
        self.update(session, id, |connection| {
            connection.phase = Phase::Connected;
            if result.is_ok() {
                if let Some(cursor) = cursor {
                    connection.cursor = Some(cursor);
                }
            } else if let Some(cursor) = &mut connection.cursor {
                cursor.held = false;
            }
        });
        result?;
        self.connection(session)
            .filter(|connection| connection.id == id)
            .ok_or(Error::Stopped)
    }

    /// End the session's connection now. An action in flight is cancelled.
    pub fn stop(&self, session: &str) -> Option<Connection> {
        let entry = self.lock().remove(session)?;
        entry.stop.cancel();
        // Another chat may be mid-action on the shared input device; free it only when idle.
        if self.lock().is_empty() {
            self.backend.release();
        }
        self.changed();
        Some(entry.connection)
    }
}

fn validate(action: &Action) -> Result<(), Error> {
    match action {
        Action::Click { count, .. } if !(1..=2).contains(count) => {
            Err(Error::Invalid("count must be 1 or 2".into()))
        }
        Action::Scroll { dx: 0, dy: 0, .. } => Err(Error::Invalid("scroll needs dx or dy".into())),
        Action::Scroll { dx, dy, .. } if dx.abs() > 50 || dy.abs() > 50 => {
            Err(Error::Invalid("scroll at most 50 steps at once".into()))
        }
        Action::Type { text } if text.is_empty() || text.chars().count() > 2000 => {
            Err(Error::Invalid("type 1 to 2000 characters".into()))
        }
        Action::Type { text }
            if text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t') =>
        {
            Err(Error::Invalid("use key for control keys".into()))
        }
        _ => Ok(()),
    }
}

/// Frame pixels to window-local logical coordinates, refusing points outside the frame.
fn local(view: View, x: f64, y: f64) -> Result<(f64, f64), Error> {
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= f64::from(view.width)
        || y >= f64::from(view.height)
    {
        return Err(Error::OutsideWindow);
    }
    let lx = x * f64::from(view.window_width) / f64::from(view.width);
    let ly = y * f64::from(view.window_height) / f64::from(view.height);
    if lx >= f64::from(view.window_width) || ly >= f64::from(view.window_height) {
        return Err(Error::OutsideWindow);
    }
    Ok((lx, ly))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct Unsupported(&'static str);

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Backend for Unsupported {
    fn eligibility(&self) -> BoxFuture<'_, Eligibility> {
        Box::pin(async move {
            Eligibility::unsupported(
                self.0,
                "Computer use works on Linux with Hyprland and on macOS",
            )
        })
    }
    fn windows(&self) -> BoxFuture<'_, Result<Vec<Window>, Error>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn capture<'a>(&'a self, _: &'a Window) -> BoxFuture<'a, Result<Frame, Error>> {
        Box::pin(async { Err(Error::Unavailable("Not supported here".into())) })
    }
    fn focus<'a>(&'a self, _: &'a Window) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Err(Error::Unavailable("Not supported here".into())) })
    }
    fn input<'a>(&'a self, _: &'a Window, _: &'a Input) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Err(Error::Unavailable("Not supported here".into())) })
    }
}

/// This computer's desktop backend.
pub fn platform_backend() -> Arc<dyn Backend> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(hyprland::Hyprland::default())
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::Mac)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Arc::new(Unsupported(std::env::consts::OS))
    }
}

#[cfg(test)]
mod tests;
