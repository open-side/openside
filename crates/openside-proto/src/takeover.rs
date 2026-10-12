//! Remote takeover WebSocket messages (`GET /v1/tickets/{id}/takeover`, `docs/specs/human-step.md`).
//!
//! Both directions carry JSON text messages. Coordinates are CSS pixels of the tab viewport
//! (`width` x `height` of the latest frame, 1440x900): a client scales pointer positions from
//! the rendered image to that space before sending them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Daemon-to-client takeover message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TakeoverServerMessage {
    /// One frame of the ticket's tab.
    Frame {
        /// Base64 JPEG.
        data: String,
        /// Viewport width in CSS pixels that the frame shows.
        width: f64,
        /// Viewport height in CSS pixels that the frame shows.
        height: f64,
    },
    /// A frame was withheld; the stream and input continue.
    Withheld {
        /// Why: `screenshot_unavailable_after_secret_fill` while the tab shows a document
        /// that received a Vault or secret_input fill.
        code: String,
    },
    /// The daemon refused one input message; the stream continues.
    InputError {
        /// Stable code: `invalid_input` or `out_of_bounds` or `input_failed`.
        code: String,
    },
    /// The stream ended; the daemon closes the socket after this message.
    Ended {
        /// Why the stream ended.
        reason: TakeoverEndReason,
    },
}

/// Why a takeover stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverEndReason {
    /// The ticket was completed with `done`.
    Done,
    /// The ticket was rejected (including session stop).
    Rejected,
    /// The ticket reached its deadline.
    Expired,
    /// No input arrived within the idle limit.
    Idle,
    /// The controlling device's token was revoked.
    Revoked,
    /// The tab or its browser is gone.
    TabUnavailable,
}

/// Mouse button of a pointer event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverMouseButton {
    /// No button (moves).
    None,
    /// Primary button.
    Left,
    /// Auxiliary button.
    Middle,
    /// Secondary button.
    Right,
}

/// Pointer event phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverMouseEvent {
    /// Button pressed.
    Down,
    /// Button released.
    Up,
    /// Pointer moved.
    Move,
}

/// Key event phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverKeyEvent {
    /// Key pressed.
    Down,
    /// Key released.
    Up,
}

/// Client-to-daemon takeover input, forwarded to the ticket's tab only. Typed text and key
/// values are never logged or persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TakeoverInput {
    /// Pointer press, release or move.
    Mouse {
        /// Phase.
        event: TakeoverMouseEvent,
        /// Viewport x in CSS pixels.
        x: f64,
        /// Viewport y in CSS pixels.
        y: f64,
        /// Button; defaults to `left` for down/up and `none` for move.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<TakeoverMouseButton>,
        /// Click count for down/up (2 = double click); default 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        click_count: Option<u32>,
        /// CDP modifier bit mask: Alt=1, Ctrl=2, Meta=4, Shift=8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modifiers: Option<u32>,
    },
    /// Scroll at a point.
    Wheel {
        /// Viewport x in CSS pixels.
        x: f64,
        /// Viewport y in CSS pixels.
        y: f64,
        /// Horizontal scroll in CSS pixels.
        delta_x: f64,
        /// Vertical scroll in CSS pixels.
        delta_y: f64,
    },
    /// Raw key press or release (navigation and editing keys, shortcuts).
    Key {
        /// Phase.
        event: TakeoverKeyEvent,
        /// DOM `KeyboardEvent.key`.
        key: String,
        /// DOM `KeyboardEvent.code`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Text the key inserts on `down`, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Windows virtual key code (`KeyboardEvent.keyCode`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_code: Option<u32>,
        /// CDP modifier bit mask: Alt=1, Ctrl=2, Meta=4, Shift=8.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modifiers: Option<u32>,
    },
    /// Insert text at the focused element (IME-composed or pasted text).
    Text {
        /// Text to insert.
        text: String,
    },
}
