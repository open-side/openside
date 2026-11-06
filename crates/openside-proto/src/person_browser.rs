use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One of the person's own (non-incognito) browser tabs, as the openside extension in their
/// browser reports it. Ids are Chrome's numbers and stay valid only for that browser run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonTab {
    /// Chrome tab id.
    pub tab_id: i64,
    /// Chrome window id.
    pub window_id: i64,
    /// Position in its window's tab strip.
    pub index: u32,
    /// Document title; untrusted page data.
    pub title: String,
    /// Current URL; untrusted page data.
    pub url: String,
    /// Favicon the browser reports for the tab, if any.
    pub favicon_url: Option<String>,
    /// The selected tab of its window.
    pub active: bool,
    /// Pinned in its window.
    pub pinned: bool,
    /// Playing sound.
    pub audible: bool,
    /// Muted by the person.
    pub muted: bool,
    /// Unloaded by the browser to save memory.
    pub discarded: bool,
    /// Tab group, or null when ungrouped.
    pub group_id: Option<i64>,
}

/// Chromium's fixed tab-group colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PersonTabGroupColor {
    /// Grey.
    Grey,
    /// Blue.
    Blue,
    /// Red.
    Red,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Pink.
    Pink,
    /// Purple.
    Purple,
    /// Cyan.
    Cyan,
    /// Orange.
    Orange,
}

/// One tab group in the person's browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonTabGroup {
    /// Chrome tab-group id.
    pub group_id: i64,
    /// Window the group lives in.
    pub window_id: i64,
    /// Group name; may be empty.
    pub title: String,
    /// Group color.
    pub color: PersonTabGroupColor,
    /// Collapsed in the tab strip.
    pub collapsed: bool,
}

/// The person's open tabs, read live from their browser on every request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonTabList {
    /// False when no openside extension is connected; the lists are then empty.
    pub connected: bool,
    /// The window the person last focused, when known.
    pub focused_window_id: Option<i64>,
    /// Tabs ordered by window, then strip position. Incognito tabs are never included.
    pub tabs: Vec<PersonTab>,
    /// Tab groups of the listed windows.
    pub groups: Vec<PersonTabGroup>,
}

/// One tab operation the person asked for in openside's tab list or tab menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum PersonTabCommand {
    /// Switch to the tab and focus its window.
    Activate {
        /// Tab to show.
        tab_id: i64,
    },
    /// Open a new tab: at an http(s) URL, at one of the browser pages `chrome://bookmarks`,
    /// `chrome://downloads`, `chrome://extensions`, `chrome://history`, `chrome://settings`,
    /// `chrome://inspect`, `chrome://profile-picker`, the settings subpages `clearBrowserData`,
    /// `appearance`, `adPrivacy`, `content`, `security`, `languages` and `performance`,
    /// `chrome://extensions/shortcuts`, or the new-tab page when omitted; right after
    /// `after_tab_id` when given (New Tab Below).
    Create {
        /// http(s) URL without credentials or an allowed browser page; omitted opens the
        /// new-tab page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Open next to this tab, in its window.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_tab_id: Option<i64>,
    },
    /// Close tabs (Close, Close Other Tabs, Close Tabs Below: the caller picks the ids).
    Close {
        /// 1-500 tabs.
        tab_ids: Vec<i64>,
    },
    /// Duplicate a tab.
    Duplicate {
        /// Tab to copy.
        tab_id: i64,
    },
    /// Reload a tab.
    Reload {
        /// Tab to reload.
        tab_id: i64,
    },
    /// Pin or unpin a tab.
    SetPinned {
        /// Tab to change.
        tab_id: i64,
        /// New pinned state.
        pinned: bool,
    },
    /// Mute or unmute a tab.
    SetMuted {
        /// Tab to change.
        tab_id: i64,
        /// New muted state.
        muted: bool,
    },
    /// Add tabs to an existing group, or to a new group when `group_id` is omitted.
    Group {
        /// 1-500 tabs.
        tab_ids: Vec<i64>,
        /// Existing group to join.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_id: Option<i64>,
        /// Name to give the group (at most 200 characters).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Color to give the group.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<PersonTabGroupColor>,
    },
    /// Take tabs out of their groups.
    Ungroup {
        /// 1-500 tabs.
        tab_ids: Vec<i64>,
    },
    /// Move a tab into a new window of its own.
    MoveToNewWindow {
        /// Tab to move.
        tab_id: i64,
    },
    /// Open a new incognito window. Its tabs are never listed or attached.
    NewIncognitoWindow,
}

/// Result of a [`PersonTabCommand`] or a restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonTabCommandResult {
    /// The created, moved or changed tab, when the command produces one.
    pub tab: Option<PersonTab>,
    /// The group tabs were added to (`group`).
    pub group_id: Option<i64>,
}

/// What a recently closed entry reopens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PersonRecentlyClosedKind {
    /// One tab.
    Tab,
    /// A whole window.
    Window,
}

/// A tab or window the person closed recently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonRecentlyClosed {
    /// Browser session id used to reopen it.
    pub session_id: String,
    /// Tab or window.
    pub kind: PersonRecentlyClosedKind,
    /// Tab title, or the window's first tab title.
    pub title: String,
    /// Tab URL; null for a window.
    pub url: Option<String>,
    /// Tab favicon, if any.
    pub favicon_url: Option<String>,
    /// When it was closed, if the browser reports it.
    pub closed_at: Option<DateTime<Utc>>,
    /// Number of tabs it reopens.
    pub tab_count: u32,
}

/// Recently closed tabs and windows, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonRecentlyClosedList {
    /// False when no openside extension is connected.
    pub connected: bool,
    /// Newest first.
    pub entries: Vec<PersonRecentlyClosed>,
}

/// One page in the person's browsing history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonHistoryItem {
    /// Page URL.
    pub url: String,
    /// Page title; may be empty.
    pub title: String,
    /// Last visit, if known.
    pub last_visit_at: Option<DateTime<Utc>>,
    /// Number of visits.
    pub visit_count: u32,
}

/// Browsing history matches, most recent first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonHistoryList {
    /// False when no openside extension is connected.
    pub connected: bool,
    /// Most recent first.
    pub items: Vec<PersonHistoryItem>,
}

/// A bookmark or bookmark folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonBookmark {
    /// Chrome bookmark id.
    pub id: String,
    /// Containing folder; null for the top folders.
    pub parent_id: Option<String>,
    /// Title; may be empty.
    pub title: String,
    /// Page URL; null for a folder.
    pub url: Option<String>,
    /// Folder contents in order; empty for a bookmark.
    pub children: Vec<PersonBookmark>,
}

/// The person's bookmarks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonBookmarkTree {
    /// False when no openside extension is connected.
    pub connected: bool,
    /// The browser's top folders (bookmarks bar, other bookmarks, ...).
    pub roots: Vec<PersonBookmark>,
}

/// Bookmark a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePersonBookmarkRequest {
    /// Folder to add to; omitted adds to the bookmarks bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Title (at most 1000 characters).
    pub title: String,
    /// http(s) URL without credentials.
    pub url: String,
}

/// A kind of data the person can clear from their own browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowsingDataType {
    /// Pages visited.
    History,
    /// Cookies and other site data.
    Cookies,
    /// Cached images and files.
    Cache,
    /// The download list (not the downloaded files).
    Downloads,
    /// Saved passwords.
    Passwords,
    /// Autofill form data.
    FormData,
}

/// Clear data from the person's own browser, as its "Delete browsing data" dialog does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearBrowsingDataRequest {
    /// At least one kind; each at most once.
    pub types: Vec<BrowsingDataType>,
    /// Clear data from this time on (Unix milliseconds); 0 clears everything.
    pub since_ms: i64,
}

/// What kind of item an installed extension is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PersonExtensionKind {
    /// A browser extension.
    Extension,
    /// A theme.
    Theme,
    /// An app.
    App,
}

/// An extension installed in the person's browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonExtension {
    /// Extension id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version string.
    pub version: String,
    /// Turned on.
    pub enabled: bool,
    /// Short description; may be empty.
    pub description: String,
    /// Extension, theme or app.
    pub kind: PersonExtensionKind,
}

/// Progress of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersonDownloadState {
    /// Still downloading.
    InProgress,
    /// Finished.
    Complete,
    /// Failed or cancelled.
    Interrupted,
}

/// A download in the person's browser. Only the file's base name is reported, never its folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonDownload {
    /// Chrome download id.
    pub download_id: i64,
    /// File base name.
    pub filename: String,
    /// Address the file came from.
    pub url: String,
    /// Progress.
    pub state: PersonDownloadState,
    /// Bytes received so far.
    pub received_bytes: u64,
    /// Size, when the server reported it.
    pub total_bytes: Option<u64>,
    /// When the download started, if known.
    pub started_at: Option<DateTime<Utc>>,
}

/// Recent downloads, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonDownloadList {
    /// False when no openside extension is connected.
    pub connected: bool,
    /// Newest first; incognito downloads are never included.
    pub downloads: Vec<PersonDownload>,
}

/// Extensions installed in the person's browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonExtensionList {
    /// False when no openside extension is connected.
    pub connected: bool,
    /// In the browser's order.
    pub extensions: Vec<PersonExtension>,
}

/// Who attached a tab to a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AttachedBy {
    /// The person (an `@` mention or Ask on this tab).
    Person,
    /// The agent, with its `browser_user_tab_attach` tool under the chat's policy.
    Agent,
}

/// A person tab the chat's agent may read, and act on under the chat's permission mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachedTab {
    /// Chrome tab id in the person's browser.
    pub tab_id: i64,
    /// Title when attached; untrusted page data.
    pub title: String,
    /// URL when attached; untrusted page data.
    pub url: String,
    /// Who attached it.
    pub attached_by: AttachedBy,
    /// When it was attached.
    pub attached_at: DateTime<Utc>,
}

/// Person tabs attached to one chat. Attachments last for the browser run: they are dropped
/// when the person's browser restarts, since Chrome reuses tab ids across runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachedTabList {
    /// In attach order.
    pub tabs: Vec<AttachedTab>,
}

/// Attach one of the person's tabs to a chat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachTabRequest {
    /// Tab to attach; omitted attaches the selected tab of the person's focused window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<i64>,
}
