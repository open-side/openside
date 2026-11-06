//! Native computer use: the person connects one desktop app window to a chat; the agent sees
//! frames of that window only and its input lands only inside it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether one part of computer use works on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseFeature {
    /// Usable now.
    pub available: bool,
    /// Mechanism in use, such as `grim`, `uinput`, `wtype` or `screencapturekit`.
    pub via: Option<String>,
    /// Why it is not usable, in plain language.
    pub reason: Option<String>,
}

/// `GET /v1/computer-use`: whether computer use is turned on and works here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseStatus {
    /// `preferences.computer_use`.
    pub enabled: bool,
    /// Window frames and at least one kind of input work.
    pub eligible: bool,
    /// `linux`, `macos` or another operating system name.
    pub platform: String,
    /// Desktop backend (`hyprland`, `macos`); null when this desktop is not supported.
    pub backend: Option<String>,
    /// Window frames.
    pub capture: ComputerUseFeature,
    /// Clicks, pointer moves and scrolling.
    pub pointer: ComputerUseFeature,
    /// Typing and key presses.
    pub keyboard: ComputerUseFeature,
    /// Every reason a part does not work, deduplicated.
    pub reasons: Vec<String>,
}

/// What a connection is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUsePhase {
    /// Idle and ready.
    Connected,
    /// Sending input.
    Acting,
    /// Taking a frame.
    Capturing,
}

/// The agent's pointer, in pixels of the latest frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseCursor {
    /// Frame x.
    pub x: f64,
    /// Frame y.
    pub y: f64,
    /// A button is down right now.
    pub held: bool,
}

/// One chat's connected app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseConnection {
    /// Changes on every connect; approvals name it, so they never carry over.
    pub id: String,
    /// Increases with every connect on this daemon.
    pub generation: u64,
    /// App name (Linux window class, macOS application name).
    pub app: String,
    /// Window title at the latest check.
    pub title: String,
    /// Owning process id.
    pub pid: u32,
    /// Current activity.
    pub phase: ComputerUsePhase,
    /// Width of the frame the agent last saw; cursor coordinates use it.
    pub frame_width: Option<u32>,
    /// Height of the frame the agent last saw.
    pub frame_height: Option<u32>,
    /// Pointer after the latest pointer action.
    pub cursor: Option<ComputerUseCursor>,
}

/// `GET /v1/sessions/{id}/computer-use` and the `state` event of the frames stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseSessionState {
    /// The connected app; null when none is connected.
    pub connection: Option<ComputerUseConnection>,
    /// Sequence of the latest frame, for matching `frame` events.
    pub frame_seq: Option<u64>,
}

/// The `frame` event of `GET /v1/sessions/{id}/computer-use/frames`: the frame the agent
/// last saw of the connected window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseFrame {
    /// Connection the frame belongs to.
    pub connection: String,
    /// Increases with every frame of this chat.
    pub seq: u64,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Always `image/png`.
    pub mime: String,
    /// Base64 image bytes.
    pub data: String,
}

/// One event of the frames stream; the SSE event name is the `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComputerUseStreamEvent {
    /// Connection state changed (connect, phase, cursor, stop).
    State(ComputerUseSessionState),
    /// A new frame.
    Frame(ComputerUseFrame),
}

/// `POST /v1/sessions/{id}/computer-use/stop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComputerUseStopResult {
    /// A connection was ended.
    pub stopped: bool,
    /// The connection that ended.
    pub connection: Option<ComputerUseConnection>,
}
