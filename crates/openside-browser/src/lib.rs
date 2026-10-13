//! Managed, isolated Chromium control over the flattened Chrome DevTools Protocol.
//!
//! Agent windows never activate user windows. Stock CDP does not expose a discarded-tab
//! flag: callers must mark sleeping tabs, and attachment refuses them with
//! [`BrowserError::TabAsleep`] before any CDP call (within the 12-second attach deadline).
//! Normal controller drop kills Chromium's process group. Detached browsers survive drop
//! and reconnect through a saved endpoint; explicit shutdown terminates either lifetime.
//! Launch disables extensions, OS-backed password stores, and browser-account sync even
//! when a distribution wrapper supplies conflicting defaults.
//!
//! Mutating actions and text insertion require a session-owned tab in its active context
//! (including popups in that session's isolated space) and a reference from its latest
//! snapshot. Navigation and reconnect invalidate actionable references; take a new snapshot
//! before acting. Document-bound NodeHandles let pending secret-input tickets survive reconnect
//! without reusing a reference in a new document.
//! Text insertion never includes supplied text or page-generated diagnostics in its errors.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod account_spaces;
mod browser;
mod cdp;
mod gateway;
mod snapshot;
mod snapshot_view;

pub use browser::{browser_shutdown_crashes, ProfileWriteGuard};
pub use browser::{
    ActOutcome, Action, Browser, BrowserConfig, BrowserEndpoint, BrowserEvent, DocumentCheck,
    DocumentPermission, DocumentRequest, DownloadStatus, NavigationCheck, NavigationGuard,
    NodeHandle, OpenedBy, OwnerNavigationCheck, PointHit, PointLabel, PointerAction, Screencast,
    ScreencastFrame, TabInfo, PAGE_COMMANDS, VIEWPORT,
};
pub use browser::{CookieBaseline, CookieKey};
pub use browser::{
    DialogClaim, DialogInfo, DownloadInfo, FileChooserInfo, PageEvent, ScreenshotArea,
    ScreenshotClip,
};
pub use browser::{
    DocumentResponse, DocumentWait, LoadState, LoadWait, NetworkEvent, NetworkEventKind,
    NetworkEvents, WaitUntil,
};
pub use browser::{FormFieldBinding, FrameHit, FrameInfo};
pub use gateway::{
    FocusDecision, Gateway, GatewayConfig, GatewayEvent, GatewayEventError, GatewayOwner,
    GatewayScreenshot, GatewayTab, GatewayTabs, EVENT_BUFFER as GATEWAY_EVENT_BUFFER,
};
pub use snapshot::{RefTarget, Snapshot, SnapshotDiff, SnapshotNode};
pub use snapshot_view::{Hunk, Omission, ScopeHint, SnapshotPatch, SnapshotView};

/// A launch, protocol, ownership, or readiness failure.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// An operating-system operation failed.
    #[error("browser I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Chromium is required; browser tests must not silently skip.
    #[error("Chromium executable missing: {0}; install Chromium before running browser tests")]
    ChromiumMissing(String),
    /// The browser WebSocket failed.
    #[error("CDP transport: {0}")]
    Transport(String),
    /// Chromium rejected a command.
    #[error("CDP error {code}: {message}")]
    Cdp {
        /// Chromium's protocol error code.
        code: i64,
        /// Chromium's protocol error message.
        message: String,
    },
    /// A bounded operation did not finish in time.
    #[error("browser timeout: {0}")]
    Timeout(&'static str),
    /// The browser transport closed.
    #[error("browser disconnected")]
    Disconnected,
    /// The owned Chromium process exited unsuccessfully without a completed, quiescent shutdown.
    #[error("Chromium exited unsuccessfully: {0}")]
    ProcessExit(std::process::ExitStatus),
    /// A response did not contain a required field.
    #[error("invalid CDP response: {0}")]
    InvalidResponse(&'static str),
    /// A tab was absent, discarded, unloaded, or not a live page target.
    #[error("tab is not ready: {0}")]
    TabNotReady(String),
    /// A discarded or unloaded tab; it is never attached (attaching would wake it).
    #[error("tab is asleep: {0}")]
    TabAsleep(String),
    /// An agent attempted to mutate a tab it does not own.
    #[error("agent does not own tab: {0}")]
    OwnershipDenied(String),
    /// The owner's isolated space is closed or lost; its tabs are never created in the shared
    /// profile instead.
    #[error("isolated browser space is closed: {0}")]
    SpaceClosed(String),
    /// JavaScript evaluation threw an exception.
    #[error("JavaScript evaluation failed: {0}")]
    JavaScript(String),
    /// A reference is absent from the latest snapshot or belongs to an old document.
    #[error("unknown or stale browser reference; take a new snapshot of this tab before acting")]
    StaleReference,
    /// Text insertion failed; page-controlled diagnostics are deliberately omitted.
    #[error("browser text insertion failed; verify tab ownership and take a new snapshot")]
    TextInsertionFailed,
    /// An action cannot be performed with the supplied arguments or target.
    #[error("invalid browser action: {0}")]
    InvalidAction(&'static str),
    /// A navigation was rejected by Chromium.
    #[error("browser navigation failed")]
    NavigationFailed,
    /// A connection grant or retired agent blocked document admission.
    #[error("connection_denied")]
    ConnectionDenied,
    /// A blocked navigation can be retried only after a person grants this tab access.
    #[error("connection_grant_required:{connection_id}; waiting for the person to allow access to {site}; retry navigation in tab {tab_id}")]
    ConnectionGrantRequired {
        /// Stable registry id, never an account label.
        connection_id: String,
        /// Registrable destination site.
        site: String,
        /// Owned tab whose next document navigation may consume the once grant.
        tab_id: String,
    },
    /// A saved browser process/profile/endpoint no longer identifies the same live browser.
    #[error("invalid browser endpoint: {0}")]
    InvalidEndpoint(&'static str),
    /// A document-bound input handle no longer has its resolved exact security origin.
    #[error("browser origin mismatch; refuse text insertion")]
    OriginMismatch,
    /// The patched browser gateway refused a request; messages never carry page data or secrets.
    #[error("browser gateway error {code}: {message}")]
    Gateway {
        /// Fixed gateway error code such as `auth_failed` or `tab_asleep`.
        code: String,
        /// Fixed browser-side description.
        message: String,
    },
}
