use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Metadata-only browser ledger entry; incognito tabs are never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTab {
    /// CDP target identity, not a Chrome extension tab number.
    pub tab_id: String,
    /// Current URL without credentials, query parameters, or fragment.
    pub url: String,
    /// Current document title.
    pub title: String,
    /// Explicit provenance, independent of URL matching.
    pub opened_by: BrowserTabOwnership,
    /// Session responsible for creation, if known.
    pub owner_session_id: Option<String>,
}

/// Browser tab provenance; only `agent` permits Agent Tabs grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BrowserTabOwnership {
    /// Created by a session.
    Agent,
    /// Adopted by the user.
    User,
    /// Discovered without an explicit creation operation.
    External,
}

/// Explicitly agent-owned tabs from the shared browser, empty before launch; a stale controller returns 503 browser_disconnected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserTabList {
    /// Stable target-id ordering; user, external, unknown-owner and private-session tabs are omitted.
    pub tabs: Vec<BrowserTab>,
}
