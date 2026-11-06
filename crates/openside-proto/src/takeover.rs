//! Remote control WebSocket messages: takeover tickets (`GET /v1/tickets/{id}/takeover`,
//! `docs/specs/human-step.md`) and a person's ticket-less control of a tab from the browser
//! sheet (`GET /v1/sessions/{id}/browser/tabs/{tab_id}/control`, `docs/specs/remote-ui.md`).
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
        /// Stable code: `invalid_input`, `out_of_bounds` or `input_failed`; on control streams
        /// also `invalid_url` (an address that does not parse or is too long), `policy_denied`,
        /// `approval_required`, `connection_denied`, `navigation_failed` or `no_history`.
        code: String,
    },
    /// The page the controlled tab shows now. Control streams only: sent when the stream
    /// opens and whenever the address, title or history position changes.
    Page {
        /// Top-frame URL without embedded credentials, query or fragment. A document that is
        /// not http(s) (`data:`, `about:`, `file:`, …), typed or reached by the page, reports
        /// only its scheme; `about:blank` is reported as is.
        url: String,
        /// Document title, at most 200 characters.
        title: String,
        /// Whether `back` has an entry to go to.
        can_go_back: bool,
        /// Whether `forward` has an entry to go to.
        can_go_forward: bool,
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

/// Client-to-daemon input, forwarded to the stream's tab only. Typed text and key values are
/// never logged or persisted. `navigate`, `back`, `forward`, `reload` and `close_tab` are
/// accepted on control streams only; a takeover stream answers them with `invalid_input`.
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
    /// Load a typed address in the tab. Any absolute URL is accepted, whatever its scheme and
    /// with or without embedded credentials (`invalid_url` only for one that does not parse or
    /// is longer than 8192 bytes); a `javascript:` address runs in the current page. The
    /// session's browse policy judges an http(s) origin as it does for the agent: `deny` is
    /// `policy_denied`, and an `ask` origin loads because the typed address is the person's
    /// approval, for this navigation and its same-origin redirects only. Redirects to other
    /// http(s) origins and later link or script navigations follow the owner policy without
    /// that approval.
    Navigate {
        /// Absolute URL as the person typed it.
        url: String,
    },
    /// Go one entry back in the tab's history, whatever its scheme. An entry on an `ask`
    /// origin is refused with `approval_required`.
    Back,
    /// Go one entry forward in the tab's history, under the same rules as `Back`.
    Forward,
    /// Reload the current page, under the same rules as `Back`.
    Reload,
    /// Close the tab; the stream then ends with `tab_unavailable`.
    CloseTab,
}
