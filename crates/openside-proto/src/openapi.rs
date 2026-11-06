use schemars::JsonSchema;
use serde_json::{json, Map, Value};

use crate::*;

/// HTTP method for a versioned route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// Retrieve a resource.
    Get,
    /// Create a resource or perform a command.
    Post,
    /// Edit a pending resource.
    Patch,
    /// Remove a resource.
    Delete,
    /// Replace a whole resource.
    Put,
}

impl HttpMethod {
    /// Uppercase HTTP wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Put => "PUT",
        }
    }
}

/// One versioned daemon HTTP operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// HTTP method.
    pub method: HttpMethod,
    /// Path template, excluding query parameters.
    pub path: &'static str,
    /// JSON request body schema, or no body.
    pub request_schema: Option<&'static str>,
    /// Successful JSON response schema, or SSE event schema.
    pub response_schema: Option<&'static str>,
    /// Baseline capability; ticket resolve/reject must check kind-specific capabilities.
    pub scope: CapabilityScope,
    /// Normal HTTP status; takeover reports the documented v1 conflict.
    pub success_status: u16,
    /// Whether the response is an SSE stream rather than ordinary JSON.
    pub sse: bool,
}

/// Complete public API v1 route contract.
pub const ROUTES: &[Route] = &[
    Route {
        method: HttpMethod::Get,
        path: "/v1/agents",
        request_schema: None,
        response_schema: Some("AgentList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/agents",
        request_schema: Some("HireDraft"),
        response_schema: Some("Agent"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/agents/{id}",
        request_schema: None,
        response_schema: Some("Agent"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/agents/{id}",
        request_schema: Some("UpdateAgentRequest"),
        response_schema: Some("Agent"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/agents/{id}/retire",
        request_schema: Some("RetireAgentRequest"),
        response_schema: Some("Agent"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/agents/{id}/restore",
        request_schema: None,
        response_schema: Some("Agent"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/agents/{id}/transfer",
        request_schema: Some("TransferRequest"),
        response_schema: Some("Agent"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/agents/{id}/handoffs",
        request_schema: None,
        response_schema: Some("HandoffList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/agents/{id}/handoffs",
        request_schema: Some("CreateHandoffRequest"),
        response_schema: Some("Handoff"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/team/proposals/suppressed",
        request_schema: None,
        response_schema: Some("SuppressedProposalList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/team/proposals/{key}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/favicons/{origin}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/health",
        request_schema: None,
        response_schema: Some("Health"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/accounts",
        request_schema: None,
        response_schema: Some("AccountsResponse"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/channels",
        request_schema: None,
        response_schema: Some("ChannelAccountList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/channels/{kind}/accounts",
        request_schema: Some("AddChannelAccountRequest"),
        response_schema: Some("ChannelAccount"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/settings/channels/{kind}/accounts/{account_id}",
        request_schema: Some("UpdateChannelAccountRequest"),
        response_schema: Some("ChannelAccount"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/settings/channels/{kind}/accounts/{account_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/channels/{kind}/accounts/{account_id}/test",
        request_schema: None,
        response_schema: Some("ChannelAccountTest"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/hooks/{kind}/{account_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/models",
        request_schema: None,
        response_schema: Some("ModelCatalog"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/settings/models",
        request_schema: Some("UpdateModelsRequest"),
        response_schema: Some("ModelSelection"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/models/providers",
        request_schema: None,
        response_schema: Some("ProviderConnectionList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/keys",
        request_schema: None,
        response_schema: Some("ProviderKeyList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/keys/{provider}",
        request_schema: Some("CreateProviderKeyRequest"),
        response_schema: Some("ProviderKeyInfo"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/settings/keys/{provider}/{key_id}",
        request_schema: Some("SetProviderKeyRequest"),
        response_schema: Some("ProviderKeyInfo"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/settings/keys/{provider}/{key_id}",
        request_schema: Some("UpdateProviderKeyRequest"),
        response_schema: Some("ProviderKeyInfo"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/settings/keys/{provider}/{key_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/subscriptions",
        request_schema: None,
        response_schema: Some("SubscriptionList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/subscriptions/{source}",
        request_schema: Some("CreateSubscriptionAccountRequest"),
        response_schema: Some("SubscriptionAccount"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/subscriptions/{source}/{account_id}",
        request_schema: None,
        response_schema: Some("SubscriptionAccount"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/settings/subscriptions/{source}/{account_id}",
        request_schema: Some("UpdateSubscriptionAccountRequest"),
        response_schema: Some("SubscriptionAccount"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/settings/subscriptions/{source}/{account_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/oauth/providers",
        request_schema: None,
        response_schema: Some("OAuthProviderList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/oauth/accounts",
        request_schema: None,
        response_schema: Some("OAuthAccountList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/oauth/accounts/{provider}",
        request_schema: Some("CreateOAuthAccountRequest"),
        response_schema: Some("OAuthTransaction"),
        scope: CapabilityScope::Admin,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/oauth/accounts/{provider}/{account_id}",
        request_schema: None,
        response_schema: Some("OAuthAccount"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/settings/oauth/accounts/{provider}/{account_id}",
        request_schema: Some("UpdateOAuthAccountRequest"),
        response_schema: Some("OAuthAccount"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/settings/oauth/accounts/{provider}/{account_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/oauth/accounts/{provider}/{account_id}/relogin",
        request_schema: Some("ReloginOAuthAccountRequest"),
        response_schema: Some("OAuthTransaction"),
        scope: CapabilityScope::Admin,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/oauth/transactions/{transaction_id}",
        request_schema: None,
        response_schema: Some("OAuthTransaction"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/oauth/transactions/{transaction_id}/input",
        request_schema: Some("OAuthInputRequest"),
        response_schema: Some("OAuthTransaction"),
        scope: CapabilityScope::Admin,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/settings/oauth/transactions/{transaction_id}/cancel",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/pools",
        request_schema: None,
        response_schema: Some("ProviderPoolList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/settings/pools/{provider}",
        request_schema: Some("UpdateProviderPoolRequest"),
        response_schema: Some("ProviderPool"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/sessions/{id}/account",
        request_schema: Some("UpdateSessionAccountRequest"),
        response_schema: Some("SessionAccountSelection"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/tabs",
        request_schema: None,
        response_schema: Some("BrowserTabList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/prefs",
        request_schema: None,
        response_schema: Some("BrowserPrefs"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/browser/prefs",
        request_schema: Some("UpdateBrowserPrefsRequest"),
        response_schema: Some("BrowserPrefs"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/tabs",
        request_schema: None,
        response_schema: Some("PersonTabList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/browser/person/tabs/command",
        request_schema: Some("PersonTabCommand"),
        response_schema: Some("PersonTabCommandResult"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/browser/person/native-tools",
        request_schema: Some("OpenPersonNativeToolRequest"),
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/recently-closed",
        request_schema: None,
        response_schema: Some("PersonRecentlyClosedList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/browser/person/recently-closed/{session_id}/restore",
        request_schema: None,
        response_schema: Some("PersonTabCommandResult"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/history",
        request_schema: None,
        response_schema: Some("PersonHistoryList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/bookmarks",
        request_schema: None,
        response_schema: Some("PersonBookmarkTree"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/browser/person/bookmarks",
        request_schema: Some("CreatePersonBookmarkRequest"),
        response_schema: Some("PersonBookmark"),
        scope: CapabilityScope::Steer,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/browser/person/bookmarks/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/browser/person/clear-data",
        request_schema: Some("ClearBrowsingDataRequest"),
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/downloads",
        request_schema: None,
        response_schema: Some("PersonDownloadList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/extensions",
        request_schema: None,
        response_schema: Some("PersonExtensionList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/browser/person/bridge",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 101,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/attached-tabs",
        request_schema: None,
        response_schema: Some("AttachedTabList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/attached-tabs",
        request_schema: Some("AttachTabRequest"),
        response_schema: Some("AttachedTab"),
        scope: CapabilityScope::Steer,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/sessions/{id}/attached-tabs/{tab_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions",
        request_schema: Some("CreateSessionRequest"),
        response_schema: Some("Session"),
        scope: CapabilityScope::Steer,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions",
        request_schema: None,
        response_schema: Some("SessionList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}",
        request_schema: None,
        response_schema: Some("Session"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/sessions/{id}",
        request_schema: Some("UpdateSessionRequest"),
        response_schema: Some("Session"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/fork",
        request_schema: Some("ForkSessionRequest"),
        response_schema: Some("Session"),
        scope: CapabilityScope::Steer,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/sessions/{id}/folder",
        request_schema: Some("SetSessionFolderRequest"),
        response_schema: Some("Session"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/projects",
        request_schema: None,
        response_schema: Some("ProjectList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/projects",
        request_schema: Some("CreateProjectRequest"),
        response_schema: Some("Project"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/projects/{id}",
        request_schema: None,
        response_schema: Some("Project"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/projects/{id}",
        request_schema: Some("UpdateProjectRequest"),
        response_schema: Some("Project"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/projects/{id}",
        request_schema: None,
        response_schema: Some("ProjectList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings",
        request_schema: None,
        response_schema: Some("Settings"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/preferences",
        request_schema: None,
        response_schema: Some("Preferences"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/settings/preferences",
        request_schema: Some("UpdatePreferencesRequest"),
        response_schema: Some("Preferences"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/cli",
        request_schema: None,
        response_schema: Some("CliDefaults"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/settings/cli",
        request_schema: Some("UpdateCliDefaultsRequest"),
        response_schema: Some("CliDefaults"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/computer-use",
        request_schema: None,
        response_schema: Some("ComputerUseStatus"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/computer-use",
        request_schema: None,
        response_schema: Some("ComputerUseSessionState"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/computer-use/frames",
        request_schema: None,
        response_schema: Some("ComputerUseStreamEvent"),
        scope: CapabilityScope::Browse,
        success_status: 200,
        sse: true,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/computer-use/stop",
        request_schema: None,
        response_schema: Some("ComputerUseStopResult"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/about",
        request_schema: None,
        response_schema: Some("About"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/about/update-check",
        request_schema: None,
        response_schema: Some("UpdateCheck"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/stt",
        request_schema: None,
        response_schema: Some("SttStatus"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/stt",
        request_schema: Some("TranscribeRequest"),
        response_schema: Some("Transcript"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/messages",
        request_schema: Some("SubmitMessageRequest"),
        response_schema: Some("Message"),
        scope: CapabilityScope::Steer,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/sessions/{id}/queue/{mid}",
        request_schema: Some("EditMessageRequest"),
        response_schema: Some("Message"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/sessions/{id}/queue/{mid}",
        request_schema: None,
        response_schema: Some("Message"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/queue/{mid}/promote",
        request_schema: None,
        response_schema: Some("Message"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/queue/{mid}/move",
        request_schema: Some("MoveMessageRequest"),
        response_schema: Some("Message"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/stop",
        request_schema: None,
        response_schema: Some("Session"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/prewarm",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/browser/prewarm",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/browser/thumbnails/{thumbnail_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Browse,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/events",
        request_schema: None,
        response_schema: Some("Event"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: true,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/events/poll",
        request_schema: None,
        response_schema: Some("EventPoll"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/tickets",
        request_schema: None,
        response_schema: Some("TicketList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/tickets/{id}",
        request_schema: None,
        response_schema: Some("Ticket"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/tickets/{id}/resolve",
        request_schema: Some("ResolveTicketRequest"),
        response_schema: Some("Ticket"),
        scope: CapabilityScope::Approve,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/tickets/{id}/done",
        request_schema: None,
        response_schema: Some("Ticket"),
        scope: CapabilityScope::HumanStep,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/tickets/{id}/reject",
        request_schema: Some("RejectTicketRequest"),
        response_schema: Some("Ticket"),
        scope: CapabilityScope::Approve,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/tickets/{id}/takeover",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::HumanStep,
        success_status: 101,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/browser/preview",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Browse,
        success_status: 101,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/browser/tabs/{tab_id}/control",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 101,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/devices/pair",
        request_schema: Some("PairDeviceRequest"),
        response_schema: Some("PairDeviceResponse"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/devices/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/devices/{id}",
        request_schema: Some("RenameDeviceRequest"),
        response_schema: Some("Device"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/sessions/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/routines",
        request_schema: None,
        response_schema: Some("RoutineList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/routines",
        request_schema: Some("CreateRoutineRequest"),
        response_schema: Some("Routine"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    // Static segment first: clients match the first template that fits a path.
    Route {
        method: HttpMethod::Get,
        path: "/v1/routines/suggestions",
        request_schema: None,
        response_schema: Some("RoutineSuggestionList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/routines/{id}",
        request_schema: None,
        response_schema: Some("Routine"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/routines/{id}",
        request_schema: Some("UpdateRoutineRequest"),
        response_schema: Some("Routine"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/routines/{id}",
        request_schema: None,
        response_schema: Some("Routine"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/routines/{id}/run",
        request_schema: None,
        response_schema: Some("RunRecord"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/routines/{id}/runs",
        request_schema: None,
        response_schema: Some("RunList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory",
        request_schema: None,
        response_schema: Some("MemorySearchResponse"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/memory",
        request_schema: Some("CreateMemoryRequest"),
        response_schema: Some("Memory"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    // Static segment first: clients match the first template that fits a path.
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/health",
        request_schema: None,
        response_schema: Some("MemoryHealth"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/history",
        request_schema: None,
        response_schema: Some("MemoryChangeList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/memory/rebuild",
        request_schema: None,
        response_schema: Some("MemoryRebuild"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/memory/reveal",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/folders/pick",
        request_schema: None,
        response_schema: Some("PickedFolder"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/files",
        request_schema: None,
        response_schema: Some("MemoryFileList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/files/{name}",
        request_schema: None,
        response_schema: Some("MemoryFile"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/memory/files/{name}",
        request_schema: Some("UpdateMemoryFileRequest"),
        response_schema: Some("MemoryFileSaved"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/{id}",
        request_schema: None,
        response_schema: Some("Memory"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/memory/{id}",
        request_schema: Some("UpdateMemoryRequest"),
        response_schema: Some("Memory"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/memory/{id}",
        request_schema: None,
        response_schema: Some("Memory"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/{id}/history",
        request_schema: None,
        response_schema: Some("MemoryHistory"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/mcp/servers",
        request_schema: None,
        response_schema: Some("McpServerList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/mcp/servers",
        request_schema: Some("CreateMcpServerRequest"),
        response_schema: Some("McpServer"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/mcp/servers/{id}",
        request_schema: None,
        response_schema: Some("McpServer"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/mcp/servers/{id}",
        request_schema: Some("UpdateMcpServerRequest"),
        response_schema: Some("McpServer"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/mcp/servers/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/mcp/servers/{id}/refresh",
        request_schema: None,
        response_schema: Some("McpServer"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/subagents",
        request_schema: None,
        response_schema: Some("SubagentList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/artifacts",
        request_schema: None,
        response_schema: Some("ArtifactList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/artifacts/{path}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/artifacts/{path}/preview",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/visuals",
        request_schema: None,
        response_schema: Some("VisualList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sessions/{id}/shares",
        request_schema: None,
        response_schema: Some("ShareLinkList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/shares",
        request_schema: Some("CreateShareRequest"),
        response_schema: Some("ShareLink"),
        scope: CapabilityScope::Steer,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/sessions/{id}/shares/{share_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Steer,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/shares/{share_id}",
        request_schema: None,
        response_schema: Some("SharedChat"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/shares/{share_id}/artifacts/{artifact}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/subagents/{sid}/cancel",
        request_schema: None,
        response_schema: Some("Subagent"),
        scope: CapabilityScope::Steer,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/subagents/{sid}/steer",
        request_schema: Some("SteerSubagentRequest"),
        response_schema: Some("Subagent"),
        scope: CapabilityScope::Steer,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/repl",
        request_schema: Some("ReplRequest"),
        response_schema: Some("ReplResponse"),
        scope: CapabilityScope::Browse,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/tools",
        request_schema: None,
        response_schema: Some("ToolCatalog"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sessions/{id}/tools/{name}",
        request_schema: Some("ToolCallRequest"),
        response_schema: Some("ToolCallResponse"),
        scope: CapabilityScope::Browse,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/skills",
        request_schema: None,
        response_schema: Some("SkillList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/skills",
        request_schema: Some("CreateSkillRequest"),
        response_schema: Some("SkillDetail"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/skills/{name}",
        request_schema: None,
        response_schema: Some("SkillDetail"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/skills/{name}",
        request_schema: Some("UpdateSkillRequest"),
        response_schema: Some("SkillDetail"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/skills/{name}",
        request_schema: None,
        response_schema: Some("SkillDetail"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/onboarding",
        request_schema: None,
        response_schema: Some("OnboardingStatus"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/onboarding",
        request_schema: Some("UpdateOnboardingRequest"),
        response_schema: Some("OnboardingStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/external-agents",
        request_schema: None,
        response_schema: Some("ExternalAgentList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/external-agents/{id}/browser-skill",
        request_schema: Some("InstallAgentSkillRequest"),
        response_schema: Some("InstalledAgentSkill"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/import/profiles",
        request_schema: None,
        response_schema: Some("ImportProfileList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/import",
        request_schema: Some("ImportRequest"),
        response_schema: Some("ImportJob"),
        scope: CapabilityScope::Admin,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/import/preview",
        request_schema: Some("ImportRequest"),
        response_schema: Some("ImportBatchReport"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/import/passwords",
        request_schema: Some("ImportPasswordsRequest"),
        response_schema: Some("ImportCategoryReport"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/import/jobs/{id}",
        request_schema: None,
        response_schema: Some("ImportJob"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/import/jobs/{id}/events",
        request_schema: None,
        response_schema: Some("ImportProgress"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: true,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/import/delivery",
        request_schema: None,
        response_schema: Some("ImportDeliveryList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/import/delivery/{id}/ack",
        request_schema: Some("ImportDeliveryAck"),
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/import/delivery/{id}/ticket",
        request_schema: Some("ImportDeliveryTicketRequest"),
        response_schema: Some("ImportDeliveryTicket"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/extension/import/{ticket}",
        request_schema: None,
        response_schema: Some("ImportDeliverySigned"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/policy",
        request_schema: None,
        response_schema: Some("PolicySettings"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/policy",
        request_schema: Some("PolicySettings"),
        response_schema: Some("PolicySettings"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/policy/test",
        request_schema: Some("PolicyTestRequest"),
        response_schema: Some("PolicyDecision"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/vault/items",
        request_schema: None,
        response_schema: Some("VaultItemList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/vault/items/{id}",
        request_schema: Some("UpdateVaultItemRequest"),
        response_schema: Some("VaultItem"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/vault/audit",
        request_schema: None,
        response_schema: Some("VaultAuditList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/connections",
        request_schema: None,
        response_schema: Some("ConnectionList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/connections/spaces",
        request_schema: Some("CreateAccountSpaceRequest"),
        response_schema: Some("CreateAccountSpaceResult"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/connections/spaces/{space_id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/connections/{id}",
        request_schema: None,
        response_schema: Some("Connection"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/connections/{id}",
        request_schema: Some("UpdateConnectionRequest"),
        response_schema: Some("Connection"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/connections/{id}/check",
        request_schema: None,
        response_schema: Some("Connection"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/connections/{id}/relogin",
        request_schema: Some("ReloginConnectionRequest"),
        response_schema: Some("ReloginResult"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/connections/{id}/revoke",
        request_schema: Some("RevokeConnectionRequest"),
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/connections/candidates/{id}/dismiss",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/connections/audit",
        request_schema: None,
        response_schema: Some("ConnectionAuditList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/vault",
        request_schema: None,
        response_schema: Some("VaultStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/setup",
        request_schema: Some("VaultUnlockRequest"),
        response_schema: Some("VaultStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/unlock",
        request_schema: Some("VaultUnlockRequest"),
        response_schema: Some("VaultStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/lock",
        request_schema: None,
        response_schema: Some("VaultStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/items",
        request_schema: Some("CreateVaultItemRequest"),
        response_schema: Some("VaultItem"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/vault/items/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/vault/settings",
        request_schema: None,
        response_schema: Some("VaultSettings"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Patch,
        path: "/v1/vault/settings",
        request_schema: Some("UpdateVaultSettingsRequest"),
        response_schema: Some("VaultSettings"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/vault/biometrics",
        request_schema: None,
        response_schema: Some("VaultBiometrics"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/export",
        request_schema: Some("VaultConfirmRequest"),
        response_schema: Some("VaultExport"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/clear",
        request_schema: Some("VaultConfirmRequest"),
        response_schema: Some("VaultClearResult"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/recovery-key",
        request_schema: Some("VaultConfirmRequest"),
        response_schema: Some("VaultRecoveryKey"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/vault/recover",
        request_schema: Some("VaultRecoverRequest"),
        response_schema: Some("VaultStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/pinned-tabs",
        request_schema: None,
        response_schema: Some("PinnedTabList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/pinned-tabs",
        request_schema: Some("CreatePinnedTabRequest"),
        response_schema: Some("PinnedTab"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/pinned-tabs/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/sync",
        request_schema: None,
        response_schema: Some("SyncStatus"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sync/setup",
        request_schema: Some("SyncSecrets"),
        response_schema: Some("SyncSetupResponse"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sync/pair",
        request_schema: Some("PairSyncRequest"),
        response_schema: Some("SyncStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sync/run",
        request_schema: None,
        response_schema: Some("SyncReport"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/sync/config",
        request_schema: Some("UpdateSyncConfigRequest"),
        response_schema: Some("SyncStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/sync/unlock",
        request_schema: Some("SyncSecrets"),
        response_schema: Some("SyncStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/policy/validate",
        request_schema: Some("ValidatePolicyRequest"),
        response_schema: Some("PolicyValidation"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/policy/network",
        request_schema: None,
        response_schema: Some("NetworkPolicy"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/policy/network",
        request_schema: Some("UpdateNetworkPolicyRequest"),
        response_schema: Some("NetworkPolicy"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/external-agents/{id}",
        request_schema: Some("SetExternalAgentRequest"),
        response_schema: Some("ExternalAgent"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/external-agents/{id}",
        request_schema: None,
        response_schema: Some("ExternalAgentList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/memory/model",
        request_schema: None,
        response_schema: Some("MemoryModelStatus"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/memory/model/download",
        request_schema: Some("StartMemoryModelRequest"),
        response_schema: Some("MemoryModelStatus"),
        scope: CapabilityScope::Admin,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/memory/model/cancel",
        request_schema: None,
        response_schema: Some("MemoryModelStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/doctor",
        request_schema: None,
        response_schema: Some("DoctorReport"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/daemon",
        request_schema: None,
        response_schema: Some("DaemonStatus"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/daemon/restart",
        request_schema: None,
        response_schema: Some("RestartDaemonResponse"),
        scope: CapabilityScope::Admin,
        success_status: 202,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/pairing",
        request_schema: Some("CreatePairingRequest"),
        response_schema: Some("PairingOffer"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/pairing/exchange",
        request_schema: Some("ExchangePairingRequest"),
        response_schema: Some("PairDeviceResponse"),
        scope: CapabilityScope::Admin,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/devices",
        request_schema: None,
        response_schema: Some("DeviceList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/devices/me",
        request_schema: None,
        response_schema: Some("MyDevice"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/devices/me/access-requests",
        request_schema: Some("RequestAccessRequest"),
        response_schema: Some("AccessRequest"),
        scope: CapabilityScope::Read,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/access-requests",
        request_schema: None,
        response_schema: Some("AccessRequestList"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/access-requests/{id}/approve",
        request_schema: None,
        response_schema: Some("Device"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/access-requests/{id}/deny",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Admin,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/remote/status",
        request_schema: None,
        response_schema: Some("RemoteStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/remote/local-access",
        request_schema: None,
        response_schema: Some("LocalAccess"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/remote/tailscale",
        request_schema: Some("ConfigureTailscaleRequest"),
        response_schema: Some("RemoteStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/remote/cloudflare",
        request_schema: Some("SetCloudflareRequest"),
        response_schema: Some("RemoteStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/remote/relay",
        request_schema: Some("SetRelayRequest"),
        response_schema: Some("RemoteStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/remote/disable",
        request_schema: None,
        response_schema: Some("RemoteDisabled"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/remote/enable",
        request_schema: None,
        response_schema: Some("RemoteStatus"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/push/vapid",
        request_schema: None,
        response_schema: Some("VapidPublicKey"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/push/subscriptions",
        request_schema: None,
        response_schema: Some("PushSubscriptionList"),
        scope: CapabilityScope::Read,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/push/subscriptions",
        request_schema: Some("CreatePushSubscriptionRequest"),
        response_schema: Some("PushSubscription"),
        scope: CapabilityScope::Read,
        success_status: 201,
        sse: false,
    },
    Route {
        method: HttpMethod::Delete,
        path: "/v1/push/subscriptions/{id}",
        request_schema: None,
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/push/presence",
        request_schema: Some("PushPresenceRequest"),
        response_schema: None,
        scope: CapabilityScope::Read,
        success_status: 204,
        sse: false,
    },
    Route {
        method: HttpMethod::Post,
        path: "/v1/otp",
        request_schema: Some("OtpIngestRequest"),
        response_schema: Some("OtpIngestResponse"),
        scope: CapabilityScope::OtpIngest,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Get,
        path: "/v1/settings/config",
        request_schema: None,
        response_schema: Some("SettingsDocument"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
    Route {
        method: HttpMethod::Put,
        path: "/v1/settings/config",
        request_schema: Some("UpdateSettingsRequest"),
        response_schema: Some("SettingsDocument"),
        scope: CapabilityScope::Admin,
        success_status: 200,
        sse: false,
    },
];

fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") {
                if let Some(name) = reference.strip_prefix("#/$defs/") {
                    *reference = format!("#/components/schemas/{name}");
                }
            }
            // A tagged newtype variant is `$ref` plus the tag's `properties`. TypeScript and
            // most OpenAPI generators drop siblings of `$ref`, losing the tag, so spell it as
            // an `allOf` of the referenced type and the tag object.
            if object.contains_key("$ref") && object.contains_key("properties") {
                let reference = object.remove("$ref").unwrap_or_default();
                let tag = Value::Object(std::mem::take(object));
                let description = tag.get("description").cloned();
                object.insert("allOf".into(), json!([{"$ref": reference}, tag]));
                if let Some(description) = description {
                    object.insert("description".into(), description);
                }
            }
            for child in object.values_mut() {
                rewrite_refs(child);
            }
        }
        Value::Array(array) => array.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

fn add_schema<T: JsonSchema>(schemas: &mut Map<String, Value>, name: &str) {
    let mut root: Value = schemars::schema_for!(T).into();
    rewrite_refs(&mut root);
    if let Some(object) = root.as_object_mut() {
        object.remove("$schema");
        if let Some(Value::Object(definitions)) = object.remove("$defs") {
            schemas.extend(definitions);
        }
    }
    schemas.insert(name.to_owned(), root);
}

/// JSON Schema 2020-12 components keyed by stable public type names.
///
/// `$ref` values point into OpenAPI `components.schemas`; use the returned object
/// as those components rather than treating an individual entry as a standalone file.
pub fn schemas() -> Map<String, Value> {
    let mut schemas = Map::new();
    macro_rules! add {
        ($($ty:ty),+ $(,)?) => { $(add_schema::<$ty>(&mut schemas, stringify!($ty));)+ };
    }
    add!(
        ComputerUseFeature,
        ComputerUseStatus,
        ComputerUsePhase,
        ComputerUseCursor,
        ComputerUseConnection,
        ComputerUseSessionState,
        ComputerUseFrame,
        ComputerUseStreamEvent,
        ComputerUseStopResult,
        Health,
        Agent,
        AgentSummary,
        AgentList,
        AgentPersona,
        AgentChannelBinding,
        AgentKind,
        AgentStatus,
        HireDraft,
        HireDecision,
        UpdateAgentRequest,
        RetireAgentRequest,
        TransferRequest,
        SuppressedProposal,
        SuppressedProposalList,
        Handoff,
        HandoffList,
        HandoffPacket,
        HandoffKind,
        HandoffStatus,
        CreateHandoffRequest,
        MessageAuthorKind,
        ChannelAccountStatus,
        ChannelAccountOptions,
        ChannelAccount,
        ChannelAccountList,
        ChannelCredentials,
        AddChannelAccountRequest,
        UpdateChannelAccountRequest,
        ChannelAccountTest,
        AccountsResponse,
        AccountSource,
        AccountStatus,
        AccountUsageWindow,
        BrowserTab,
        BrowserTabOwnership,
        BrowserTabList,
        BrowserPrefs,
        BrowserPrefsStatus,
        BrowserPrefValues,
        ThirdPartyCookies,
        BrowserSearchEngines,
        BrowserSearchEngine,
        UpdateBrowserPrefsRequest,
        PersonTab,
        PersonTabGroup,
        PersonTabGroupColor,
        PersonTabList,
        PersonTabCommand,
        PersonTabCommandResult,
        PersonNativeTool,
        OpenPersonNativeToolRequest,
        PersonRecentlyClosed,
        PersonRecentlyClosedKind,
        PersonRecentlyClosedList,
        PersonHistoryItem,
        PersonHistoryList,
        PersonBookmark,
        PersonBookmarkTree,
        CreatePersonBookmarkRequest,
        BrowsingDataType,
        ClearBrowsingDataRequest,
        PersonDownload,
        PersonDownloadState,
        PersonDownloadList,
        PersonExtension,
        PersonExtensionKind,
        PersonExtensionList,
        AttachedBy,
        AttachedTab,
        AttachedTabList,
        AttachTabRequest,
        Preferences,
        UpdatePreferencesRequest,
        NotificationKind,
        FollowUpMode,
        ChatConfirmation,
        CliProfile,
        CliDefaults,
        UpdateCliDefaultsRequest,
        ContextUsage,
        About,
        UpdateCheck,
        SttStatus,
        SttUnavailable,
        TranscribeRequest,
        Transcript,
        Session,
        SessionList,
        CreateSessionRequest,
        Project,
        ProjectList,
        CreateProjectRequest,
        UpdateProjectRequest,
        PickedFolder,
        SetSessionFolderRequest,
        Message,
        SubmitMessageRequest,
        EditMessageRequest,
        MoveMessageRequest,
        Ticket,
        Card,
        CardKind,
        CardStatus,
        CardInteraction,
        CardSite,
        FaviconRef,
        MediaRef,
        Money,
        Rating,
        CardLink,
        CardValue,
        KvRow,
        ListItem,
        TableColumn,
        TableLayout,
        ChoiceLayout,
        ChoiceOption,
        InputKind,
        CardField,
        InputValidation,
        FieldInitial,
        InputField,
        NoticeTone,
        NoticePlacement,
        CardPerson,
        Block,
        Action,
        ActionStyle,
        ChoiceAnswerEncoding,
        FieldAnswerEncoding,
        TicketAnswer,
        ActionTarget,
        TeamCardCommand,
        PresentRequest,
        PresentImage,
        PresentResult,
        TicketForm,
        TicketList,
        ResolveTicketRequest,
        RejectTicketRequest,
        PairDeviceRequest,
        PairDeviceResponse,
        ApiError,
        Event,
        EventKind,
        Settings,
        CapabilityScope,
        PolicyMode,
        MessageMode,
        SessionStatus,
        TicketKind,
        TicketDeferral,
        TicketStatus,
        TicketOrigin,
        TicketOption,
        TicketTool,
        TicketTarget,
        SessionId,
        MessageId,
        TicketId,
        QueueMode,
        MessageStatus,
        DropReason,
        Routine,
        RoutineList,
        RoutineSchedule,
        RoutineAction,
        RoutinePauseReason,
        CreateRoutineRequest,
        UpdateRoutineRequest,
        RunRecord,
        RunStatus,
        RunList,
        RoutineDraft,
        RoutineSuggestionList,
        UpdateSessionRequest,
        ForkSessionRequest,
        CreateShareRequest,
        ShareLink,
        ShareLinkList,
        SharedChat,
        SharedItem,
        SharedStep,
        SharedArtifact,
        Memory,
        MemoryKind,
        Retention,
        MemorySearchMode,
        MemoryMatch,
        MemoryHit,
        MemorySearchResponse,
        CreateMemoryRequest,
        UpdateMemoryRequest,
        MemoryOp,
        MemoryVersion,
        MemoryHistory,
        MemoryHealth,
        PreviewServerMessage,
        PreviewEndReason,
        PreviewPauseReason,
        Subagent,
        SubagentList,
        SubagentStatus,
        SteerSubagentRequest,
        ReplRequest,
        ReplStatus,
        ReplResponse,
        SkillSource,
        SkillSummary,
        SkillProblem,
        SkillList,
        SkillResource,
        SkillDetail,
        SkillFile,
        CreateSkillRequest,
        UpdateSkillRequest,
        PolicySettings,
        PolicyRule,
        PolicyEffect,
        PolicyCapability,
        PolicyTestRequest,
        PolicyDecision,
        VaultPolicy,
        VaultPolicyOverride,
        VaultItem,
        VaultItemList,
        UpdateVaultItemRequest,
        CreateVaultItemRequest,
        VaultBackend,
        VaultState,
        VaultStatus,
        VaultUnlockRequest,
        VaultAuditRecord,
        VaultAuditList,
        VaultAutoLock,
        VaultAutofillSettings,
        UpdateVaultAutofillSettings,
        VaultBiometrics,
        VaultRecoveryStatus,
        VaultSettings,
        UpdateVaultSettingsRequest,
        VaultConfirmRequest,
        VaultExport,
        VaultRecoveryKey,
        VaultRecoverRequest,
        VaultClearResult,
        Connection,
        ConnectionList,
        ConnectionListQuery,
        UpdateConnectionRequest,
        RevokeConnectionRequest,
        ReloginConnectionRequest,
        ConnectionAuditRecord,
        ConnectionAuditList,
        ConnectionAuditQuery,
        ConnectionError,
        ConnectionsListArgs,
        ConnectionOpenArgs,
        ConnectionCheckArgs,
        ConnectionConfirmArgs,
        ConnectionObserveArgs,
        ConnectionFetchArgs,
        TicketRef,
        ReloginResult,
        CreateAccountSpaceRequest,
        CreateAccountSpaceResult,
        PinnedTab,
        PinnedTabList,
        CreatePinnedTabRequest,
        SyncBackendKind,
        SyncStatus,
        SyncSetupResponse,
        PairSyncRequest,
        SyncReport,
        CreatePairingRequest,
        PairingOffer,
        ExchangePairingRequest,
        Device,
        DeviceKind,
        DeviceList,
        RenameDeviceRequest,
        MyDevice,
        AccessRequest,
        AccessRequestList,
        RequestAccessRequest,
        PairingTransport,
        RemoteStatus,
        RemoteDisabled,
        CloudflareStatus,
        RelayStatus,
        TailscaleStatus,
        TailscaleConfiguration,
        RemoteConfiguration,
        ConfigureTailscaleRequest,
        SetCloudflareRequest,
        SetRelayRequest,
        LocalAccess,
        EventPoll,
        VapidPublicKey,
        PushKeys,
        CreatePushSubscriptionRequest,
        PushSubscription,
        PushSubscriptionList,
        PushPresenceRequest,
        PushNotification,
        OtpIngestRequest,
        OtpIngestResponse,
        SettingsDocument,
        UpdateSettingsRequest,
        ModelCatalog,
        ModelProvider,
        CatalogModel,
        ModelAccount,
        UpdateModelsRequest,
        ModelSelection,
        ProviderConnectionList,
        ProviderConnection,
        ProviderKeyInfo,
        ProviderKeyList,
        SetProviderKeyRequest,
        CreateProviderKeyRequest,
        UpdateProviderKeyRequest,
        ModelAccountRef,
        ProviderPool,
        ProviderPoolList,
        UpdateProviderPoolRequest,
        SubscriptionAccount,
        SubscriptionSource,
        SubscriptionList,
        CreateSubscriptionAccountRequest,
        UpdateSubscriptionAccountRequest,
        OAuthMethod,
        OAuthProviderInfo,
        OAuthProviderList,
        OAuthAccount,
        OAuthAccountList,
        CreateOAuthAccountRequest,
        ReloginOAuthAccountRequest,
        UpdateOAuthAccountRequest,
        OAuthInputRequest,
        OAuthTransactionKind,
        OAuthTransactionStatus,
        OAuthAuthorization,
        OAuthTransaction,
        UpdateSessionAccountRequest,
        SessionAccountSelection,
        AttachmentUpload,
        Attachment,
        ContentKind,
        ArtifactSource,
        ArtifactPurpose,
        Artifact,
        ArtifactList,
        VisualKind,
        Visual,
        VisualList,
        TakeoverServerMessage,
        TakeoverEndReason,
        TakeoverInput,
        TakeoverMouseButton,
        TakeoverMouseEvent,
        TakeoverKeyEvent,
        ToolDescriptor,
        ToolCatalog,
        ToolCallRequest,
        ToolCallStatus,
        ToolCallResponse,
        OnboardingState,
        OnboardingStatus,
        UpdateOnboardingRequest,
        OnboardingAlreadySet,
        ExternalAgent,
        ExternalAgentList,
        InstallAgentSkillRequest,
        InstalledAgentSkill,
        ImportBrowser,
        ImportCategory,
        ImportAvailability,
        ImportProfile,
        ImportProfileList,
        ImportRequest,
        ImportRejectedCookie,
        ImportCategoryReport,
        ImportedRestoreTab,
        ImportedExtension,
        ImportedSearchEngine,
        ImportedPermission,
        ImportDeliveryTarget,
        ImportDeliveryQuery,
        ImportDeliveryAck,
        ImportDeliverySource,
        ImportDeliveryBookmark,
        ImportDeliveryVisit,
        ImportDeliveryCookie,
        ImportDeliveryCookiePartitionKey,
        ImportDeliveryBatch,
        ImportDeliveryList,
        ImportDeliveryPending,
        ImportDeliveryTicketRequest,
        ImportDeliveryTicket,
        ImportDeliverySigned,
        ImportBatchReport,
        ImportJobState,
        ImportJob,
        ImportProgress,
        ImportPasswordsRequest,
        ImportReport,
        RestartMethod,
        DaemonStatus,
        RestartDaemonResponse,
        DoctorStatus,
        DoctorFixAction,
        DoctorFix,
        DoctorCheck,
        DoctorGroup,
        DoctorReport,
        MemoryModelState,
        MemoryModelSource,
        MemoryModelStatus,
        StartMemoryModelRequest,
        SetExternalAgentRequest,
        PolicyList,
        PolicyProblem,
        ValidatePolicyRequest,
        PolicyValidation,
        NetworkPolicy,
        UpdateNetworkPolicyRequest,
        SyncSecretSource,
        SyncSecrets,
        UpdateSyncConfigRequest,
        MemoryView,
        MemoryFileInfo,
        MemoryFileList,
        MemoryFile,
        UpdateMemoryFileRequest,
        MemoryFileSaved,
        MemoryChange,
        MemoryChangeList,
        MemoryRebuild,
        McpServerTransport,
        McpServerStatus,
        McpServerTool,
        McpServer,
        McpServerList,
        CreateMcpServerRequest,
        UpdateMcpServerRequest
    );
    schemas
}

fn reference(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
}

/// Descriptions, query parameters and error responses of the person-browser routes.
fn person_browser_operation(route: &Route, operation: &mut Value, parameters: &mut Vec<Value>) {
    let error = |description: &str, code: &str| {
        json!({"description": description, "content": {"application/json": {
            "schema": reference("ApiError"),
            "example": {"code": code, "message": code.replace('_', " ")}}}})
    };
    operation["responses"]["403"] = error(
        "Agent clients (MCP/CLI agent tokens and marked socket clients) never read or drive the person's own browser",
        "person_device_required",
    );
    let reads = route.method == HttpMethod::Get;
    let session = route.path.starts_with("/v1/sessions/");
    if route.path == "/v1/browser/person/bridge" {
        operation["description"] = json!("WebSocket the openside extension in the person's own browser keeps open so the daemon can read and drive that browser. Only the extension may connect: a person_device token and Origin chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj; the bearer travels as on the takeover stream (`Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>`). The extension first sends {kind:\"hello\",protocol:\"openside.person-browser\",version:1,run}; the daemon then sends requests {id,method,params} and the extension answers {id,result} or {id,error:{code}}. A newer connection replaces the older one; a new run id (browser restart) drops every chat's tab attachments.");
        operation["responses"]["101"]["description"] = json!("Switching to WebSocket");
        operation["responses"]["403"] = error(
            "Not the openside extension of a person's device",
            "person_device_required",
        );
        return;
    }
    if route.path == "/v1/browser/person/native-tools" {
        operation["description"] = json!("Open the Task manager, or developer tools for the selected tab of the most recently active window, in the person's own browser. The daemon reaches it through the person-role gateway that `openside-browser` starts a patched browser with; that gateway serves only the person's settings and these tools, never their tabs.");
        operation["responses"]["409"] = error(
            "The person's browser is not open (browser_not_running), or cannot open the tool: a stock browser, no window of the person's profile, or a policy turns developer tools off (native_tools_unsupported)",
            "browser_not_running",
        );
        operation["responses"]["503"] = error(
            "The person's browser closed the connection or did not answer in time",
            "browser_disconnected",
        );
        operation["responses"]["400"] = error("Unknown tool", "invalid_value");
        return;
    }
    if session {
        operation["description"] = json!(match route.method {
            HttpMethod::Get => "Person tabs attached to this chat: the agent may snapshot them and act on them under the chat's permission mode. Attachments last for the browser run.",
            HttpMethod::Post => "Attach one of the person's tabs (or, without tab_id, the selected tab of the focused window) to this chat, as an @ mention or Ask on this tab does. The person's own action needs no approval; incognito tabs, browser pages and the openside UI cannot be attached (400 invalid_value).",
            _ => "Detach a person tab from this chat; the agent can no longer read or act on it.",
        });
    } else if reads {
        operation["description"] = json!(format!("Read live from the openside extension in the person's own browser; nothing is cached by the daemon. Incognito windows are never included. connected is false (with empty lists) when no extension is connected. {}",
            match route.path {
                "/v1/browser/person/history" => "q searches titles and URLs; without q the most recent pages are listed.",
                "/v1/browser/person/downloads" => "Only file base names are returned, never folders.",
                _ => "",
            }));
    } else {
        operation["description"] = json!(match route.path {
            "/v1/browser/person/tabs/command" => "Run one tab operation in the person's own browser through the openside extension (switch, open, close, duplicate, reload, pin, mute, group, move, new incognito window). create accepts http(s) URLs and a fixed set of browser pages only.",
            "/v1/browser/person/recently-closed/{session_id}/restore" => "Reopen a recently closed tab or window in the person's own browser.",
            "/v1/browser/person/bookmarks" => "Bookmark an http(s) page in the person's own browser.",
            "/v1/browser/person/clear-data" => "Delete the chosen kinds of data from the person's own browser profile, as the browser's own Delete browsing data dialog does. Only the person can do this; it is audited without the data itself.",
            _ => "Remove a bookmark or an empty bookmark folder from the person's own browser.",
        });
    }
    match route.path {
        "/v1/browser/person/recently-closed" => parameters
            .push(json!({"name": "limit", "in": "query", "required": false,
            "schema": {"type": "integer", "minimum": 1, "maximum": 25, "default": 10}})),
        "/v1/browser/person/history" => {
            parameters.push(json!({"name": "q", "in": "query", "required": false,
                "schema": {"type": "string", "maxLength": 200}}));
            parameters.push(json!({"name": "limit", "in": "query", "required": false,
                "schema": {"type": "integer", "minimum": 1, "maximum": 100, "default": 25}}));
        }
        "/v1/browser/person/downloads" => parameters
            .push(json!({"name": "limit", "in": "query", "required": false,
            "schema": {"type": "integer", "minimum": 1, "maximum": 50, "default": 10}})),
        "/v1/browser/person/recently-closed/{session_id}/restore" => parameters
            .push(json!({"name": "session_id", "in": "path", "required": true,
            "schema": {"type": "string"}})),
        _ => {}
    }
    // Listing and detaching read only the daemon's own memory: they never reach the browser.
    if session && route.method != HttpMethod::Post {
        operation["responses"]["404"] = if route.method == HttpMethod::Get {
            error("No such chat", "not_found")
        } else {
            error(
                "No such chat, or that tab is not attached to it",
                "tab_not_found",
            )
        };
        return;
    }
    if !reads || session {
        operation["responses"]["409"] = error(
            "No openside extension is connected (person_browser_not_connected), or it lacks access to that page (site_access_required)",
            "person_browser_not_connected",
        );
        operation["responses"]["404"] = error("No such tab, bookmark or session", "tab_not_found");
        operation["responses"]["400"] =
            error("A value the daemon or the browser refuses", "invalid_value");
    }
    operation["responses"]["504"] = error(
        "The person's browser did not answer in time",
        "person_browser_timeout",
    );
    operation["responses"]["502"] = error(
        "The person's browser reported a failure",
        "person_browser_failed",
    );
}

/// Descriptions and error responses of the person-facing speech-to-text routes.
fn stt_operation(route: &Route, operation: &mut Value) {
    let error = |description: &str, code: &str| {
        json!({"description": description, "content": {"application/json": {
            "schema": reference("ApiError"),
            "example": {"code": code, "message": code.replace('_', " ")}}}})
    };
    if route.method == HttpMethod::Get {
        operation["description"] = json!("Whether this caller can dictate right now. A caller that POST would refuse (agent clients: MCP/CLI agent tokens and marked socket clients) gets available false with reason person_device_required, without any transcriber lookup. Otherwise available is true when channels.stt_cmd is set or the openai account pool holds a first-party OpenAI API key (ChatGPT-plan sign-ins, relays and plugin accounts are never used), and false with reason stt_unavailable when neither is; nothing leaves the computer to answer. max_bytes is the largest decoded clip (8 MiB, or channels.max_audio_bytes when smaller); max_seconds is the longest clip a client should record (120).");
        return;
    }
    operation["responses"]["403"] = error(
        "Only the person's own device or the local socket dictates; agent clients (MCP/CLI agent tokens and marked socket clients) are refused",
        "person_device_required",
    );
    operation["description"] = json!("Transcribe one recorded clip (base64) for the person's composer. channels.stt_cmd runs locally first with the clip in a private temporary file that is deleted when the command ends; otherwise the clip goes to OpenAI's transcription API (gpt-4o-mini-transcribe) with the first usable OpenAI API key of the openai account pool. language only reaches OpenAI, as its primary subtag. The clip and the text are never stored, logged or added to a chat. At most two clips are transcribed at once and four more wait for a slot; beyond that the request is refused with 429 stt_busy before its clip is decoded. The request body may be at most the base64 size of max_bytes plus 64 KiB and must arrive within 30 seconds.");
    operation["responses"]["400"] = error(
        "A body that is not a TranscribeRequest, bad base64, an empty clip, a mime that is not audio/*, or a language that is not a short BCP-47 tag",
        "invalid_audio",
    );
    operation["responses"]["408"] = error(
        "The request body did not arrive within 30 seconds; its place is freed for others",
        "audio_timeout",
    );
    operation["responses"]["409"] = error(
        "No transcriber: set channels.stt_cmd or add an OpenAI API key",
        "stt_unavailable",
    );
    operation["responses"]["413"] = error(
        "The clip is larger than max_bytes from GET /v1/stt, or the body is larger than its base64 size plus 64 KiB",
        "audio_too_large",
    );
    operation["responses"]["422"] = error("The transcriber heard no speech", "no_speech");
    operation["responses"]["502"] = error(
        "The transcriber failed or timed out; its output is never returned",
        "stt_failed",
    );
    operation["responses"]["429"] = error(
        "Two clips are being transcribed and four more are waiting; try again shortly",
        "stt_busy",
    );
}

/// Generate the OpenAPI 3.1 document from [`ROUTES`] and the derived JSON Schemas.
pub fn openapi() -> Value {
    let mut paths = Map::<String, Value>::new();
    for route in ROUTES {
        let content = match route.response_schema {
            Some(name) if route.sse => json!({"text/event-stream": {
                "schema": {"type": "string"}, "x-sse-event-schema": reference(name)
            }}),
            Some(name) => json!({"application/json": {"schema": reference(name)}}),
            None => json!({}),
        };
        let mut success = json!({"description": "Success"});
        if route.response_schema.is_some() {
            success["content"] = content;
        }
        let mut operation = json!({
            "security": [{"capabilityToken": []}],
            "x-required-capability": route.scope,
            "responses": {
                route.success_status.to_string(): success,
                "default": {"description": "Sanitized error", "content": {
                    "application/json": {"schema": reference("ApiError")}
                }}
            }
        });
        if route.path == "/v1/browser/tabs" {
            operation["description"] = json!("Metadata-only ledger of explicitly agent-owned targets with a known non-incognito owner. Omits user/external targets and removes URL credentials, query and fragment. Empty before browser launch; never launches or reconnects the browser.");
            operation["responses"]["503"] = json!({"description": "The previously started browser controller is disconnected",
                "content": {"application/json": {"schema": reference("ApiError"),
                "example": {"code": "browser_disconnected", "message": "browser disconnected"}}}});
        }
        if route.path == "/v1/browser/prefs" {
            let error = |description: &str, code: &str| {
                json!({"description": description, "content": {"application/json": {
                    "schema": reference("ApiError"),
                    "example": {"code": code, "message": code.replace('_', " ")}}}})
            };
            operation["responses"]["503"] = error(
                "The person's browser closed the connection or did not answer in time (browser_disconnected), or could not load its search engines in time or has too many such requests waiting (browser_busy)",
                "browser_disconnected",
            );
            if route.method == HttpMethod::Get {
                operation["description"] = json!("Reads the settings of the person's own browser (the patched browser `openside-browser` opens, through its person-role gateway), from its startup profile; the agent browser's profile is never read. status is not_running when that browser is closed (the daemon never launches it) and unsupported for stock Chromium/Chrome; prefs is then null.");
            } else {
                operation["description"] = json!("Writes settings of the person's own browser through its person-role gateway, one allowlisted preference at a time in field order, and returns the values read back. Never launches a browser and never writes the agent browser's profile.");
                operation["responses"]["400"] =
                    error("A value the browser refuses", "invalid_value");
                operation["responses"]["409"] = error(
                    "The person's browser is not open (browser_not_running), cannot change settings (browser_prefs_unsupported), or a policy manages the setting (browser_pref_managed)",
                    "browser_not_running",
                );
            }
        }
        if let Some(name) = route.request_schema {
            operation["requestBody"] = json!({"required": true, "content": {
                "application/json": {"schema": reference(name)}
            }});
        }
        let mut parameters = Vec::new();
        if route.path.contains("{id}") {
            let schema = if route.path.starts_with("/v1/sessions/") {
                reference("SessionId")
            } else if route.path.starts_with("/v1/tickets/") {
                reference("TicketId")
            } else {
                json!({"type": "string"})
            };
            parameters
                .push(json!({"name": "id", "in": "path", "required": true, "schema": schema}));
        }
        if route.path.contains("{mid}") {
            parameters.push(json!({"name": "mid", "in": "path", "required": true, "schema": reference("MessageId")}));
        }
        if route.path.contains("{sid}") {
            parameters.push(json!({"name": "sid", "in": "path", "required": true,
                "description": "Subagent identifier from subagent.started.",
                "schema": {"type": "string"}}));
        }
        if route.path.contains("{thumbnail_id}") {
            parameters.push(
                json!({"name": "thumbnail_id", "in": "path", "required": true,
                "schema": {"type": "string"}}),
            );
            operation["description"] = json!("Read a bounded JPEG thumbnail referenced by browser.state. Requires browse; secret-tainted tabs, changed documents, takeover, private/stopped/archived/deleted sessions, expired IDs and IDs from another session are withheld. IDs expire after ten minutes and after daemon restart; responses are never cached.");
            operation["responses"]["200"]["content"] =
                json!({"image/jpeg": {"schema": {"type":"string","format":"binary"}}});
        }
        if route.path.contains("{tab_id}") {
            if route.path.starts_with("/v1/sessions/{id}/attached-tabs/") {
                parameters.push(json!({"name": "tab_id", "in": "path", "required": true,
                    "description": "Chrome tab id of an attached person tab.",
                    "schema": {"type": "integer", "format": "int64"}}));
            } else {
                parameters.push(json!({"name": "tab_id", "in": "path", "required": true,
                    "description": "Agent tab of this session, from GET /v1/browser/tabs.",
                    "schema": {"type": "string"}}));
            }
        }
        if route.path.starts_with("/v1/browser/person/")
            || route.path.starts_with("/v1/sessions/{id}/attached-tabs")
        {
            person_browser_operation(route, &mut operation, &mut parameters);
        }
        if route.path == "/v1/stt" {
            stt_operation(route, &mut operation);
        }
        if route.path.starts_with("/v1/settings/channels") {
            operation["description"] = json!("Manage independent labelled channel identities. Read lists are metadata only. Mutations require admin and a person device or the local socket. Credentials are encrypted in the Vault, never returned, synced or exposed to browser fill. Account changes affect only that account; failed credentials never select another identity.");
            for name in ["kind", "account_id"] {
                if route.path.contains(&format!("{{{name}}}")) {
                    parameters.push(
                        json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}),
                    );
                }
            }
        }
        if route.path.contains("/computer-use") {
            operation["description"] = json!(match (route.method, route.path) {
                (HttpMethod::Get, "/v1/computer-use") => "Whether computer use is turned on (preferences.computer_use, changeable only with local access) and what works on this computer: window frames, pointer and keyboard, each with the mechanism in use or the reason it does not work. Probes the desktop at most every 30 seconds.",
                (HttpMethod::Get, "/v1/sessions/{id}/computer-use/frames") => "Server-sent events for the chat's connected app: a `state` event now and on every change (connect, phase, cursor, stop), and a `frame` event with each frame the agent sees. Private chats are refused (409 private_session). The stream ends when the chat is deleted.",
                (HttpMethod::Post, _) => "End the chat's computer use now: an action in progress is cancelled, the virtual input device is removed, and later approvals for that connection no longer act. Audited.",
                _ => "The chat's connected desktop app, if any, with the cursor in pixels of the latest frame.",
            });
            if route.path.starts_with("/v1/sessions/") {
                operation["responses"]["404"] = json!({"description": "No such chat",
                    "content": {"application/json": {"schema": reference("ApiError"),
                    "example": {"code": "not_found", "message": "not found"}}}});
            }
            if route.path.ends_with("/frames") {
                operation["responses"]["409"] = json!({"description": "Private chats never stream frames",
                    "content": {"application/json": {"schema": reference("ApiError"),
                    "example": {"code": "private_session", "message": "private session"}}}});
            }
        }
        if route.path == "/v1/hooks/{kind}/{account_id}" {
            operation["description"] = json!("Public signed platform callback for one managed identity, not bearer authentication. Slack events/interactivity use v0 HMAC-SHA256; Discord uses strict Ed25519 verification. Both signed timestamps must be within five minutes. Exact-body replay detection is per account with five-minute monotonic expiry. Bodies are capped at 256 KiB with a ten-second read deadline; each account admits 120 requests per rolling minute. Unknown or inactive accounts return sanitized 404 without Vault or platform lookup. OS keyring idle lock does not stop service identities; an explicit openside Vault lock does.");
            parameters.push(json!({"name":"kind","in":"path","required":true,"schema":{"type":"string","enum":["slack","discord"]}}));
            parameters.push(json!({"name":"account_id","in":"path","required":true,"schema":{"type":"string","pattern":"^[a-z0-9_-]{1,40}$"}}));
            for name in [
                "X-Slack-Request-Timestamp",
                "X-Slack-Signature",
                "X-Signature-Timestamp",
                "X-Signature-Ed25519",
            ] {
                parameters.push(json!({"name":name,"in":"header","required":false,"description":"Required by the selected platform's signing protocol.","schema":{"type":"string"}}));
            }
            operation["requestBody"] = json!({"required":true,"content":{
                "application/json":{"schema":{"type":"object"}},
                "application/x-www-form-urlencoded":{"schema":{"type":"object","properties":{"payload":{"type":"string"}}}}
            }});
            operation["responses"]["200"]["content"] =
                json!({"application/json":{"schema":{"type":"object"}}});
            for (status, code) in [
                ("401", "channel_bad_signature"),
                ("404", "channel_account_not_found"),
                ("409", "channel_replay"),
                ("413", "channel_body_too_large"),
                ("408", "channel_body_timeout"),
                ("429", "channel_rate_limited"),
            ] {
                operation["responses"][status] = json!({"description":code,"content":{"application/json":{"schema":reference("ApiError")}}});
            }
            operation["responses"]["429"]["headers"] =
                json!({"Retry-After":{"schema":{"type":"string"},"description":"60 seconds"}});
        }
        if route.sse {
            parameters.push(
                json!({"name": "Last-Event-ID", "in": "header", "required": false,
                "description": "Last seen sequence in this stream; only later events are returned.",
                "schema": {"type": "string", "pattern": "^[0-9]+$"}}),
            );
        }
        if route.path == "/v1/sessions/{id}/events" {
            parameters.push(
                json!({"name": "subagents", "in": "query", "required": false,
                "description": "Set false to omit subagent.* events from the stream.",
                "schema": {"type": "boolean", "default": true}}),
            );
        }
        if route.path == "/v1/sessions/{id}/prewarm" {
            operation["description"] = json!("Start the session's agent process, launch the browser and open one blank agent tab its next browser_open_tab adopts, so the first reply and browser action skip cold starts. Idempotent; returns before the work finishes.");
        }
        if route.path == "/v1/browser/prewarm" {
            operation["description"] = json!("Launch the browser ahead of a new chat's first message. Idempotent; returns before the browser is up.");
        }
        if matches!(
            route.path,
            "/v1/sessions/{id}/messages"
                | "/v1/sessions/{id}/queue/{mid}/promote"
                | "/v1/sessions/{id}/subagents/{sid}/cancel"
                | "/v1/sessions/{id}/subagents/{sid}/steer"
        ) {
            operation["responses"]["503"] = json!({"description": "The agent runtime could not start or its connection is gone; a later request starts a new process",
                "content": {"application/json": {"schema": reference("ApiError"),
                "example": {"code": "runtime_unavailable", "message": "runtime unavailable"}}}});
        }
        if route.path == "/v1/sessions/{id}/subagents/{sid}/cancel" {
            operation["description"] = json!("Cancel a running or waiting child; its pending tickets are rejected and the parent's delegate call returns a cancelled result. Cancelling a finished child returns 409.");
        }
        if route.path == "/v1/sessions/{id}/subagents/{sid}/steer" {
            operation["description"] = json!("Queue steering text for the child's next model request; a waiting child receives it when it resumes.");
        }
        if route.path == "/v1/sessions" && route.method == HttpMethod::Get {
            parameters.push(json!({"name":"agent","in":"query","required":false,
                "description":"Filter immutable agent ownership: none selects earlier untagged conversations; an agent id selects all its threads.",
                "schema":{"type":"string"}}));
            parameters.push(json!({"name": "q", "in": "query", "required": false,
                "description": "Case-insensitive substring of the title or first message.",
                "schema": {"type": "string"}}));
            parameters.push(json!({"name": "archived", "in": "query", "required": false,
                "description": "List archived (true) or unarchived (false, default) sessions; pinned sessions come first.",
                "schema": {"type": "boolean", "default": false}}));
            parameters.push(json!({"name": "project", "in": "query", "required": false,
                "description": "Filter by project: none selects chats outside every project; a project id selects that project's chats.",
                "schema": {"type": "string"}}));
        }
        if route.path == "/v1/sessions" && route.method == HttpMethod::Post {
            operation["description"] = json!("Create a session. browser_space \"isolated\" gives its agent tabs their own cookies and storage in the shared managed browser, disposed on archive or delete and lost when the browser restarts; it cannot be combined with incognito (400 invalid_browser_space). project_id starts the chat in a project: its folder becomes the workspace and `session.created` is followed by `session.folder_changed` in the same commit (404 project_not_found; 409 project_root_changed when the project's folder was moved or replaced since the project was made, or still needs confirming (needs_confirmation); 400 invalid_workspace together with workspace). Any person device or the local socket may start a chat in an existing project; agent_client devices get 403 person_device_required. Paired devices get 403 local_socket_required for a mode looser than the daemon default, and for a workspace unless they have local access (an admin person device on the computer running openside); agent_client devices also get it for speed \"fast\", effort \"xhigh\"/\"max\" or profile \"ultrabrowse\". A model a person picks for a chat without an agent becomes the default for new chats; agent clients and `openside mcp` never change it.");
        }
        if route.path == "/v1/sessions/{id}/folder" {
            operation["description"] = json!("Choose where the chat works: project_id moves it into that project (its folder becomes the workspace), workspace attaches an absolute folder outside any project, and both null leaves the project and drops the folder. Recorded as one `session.folder_changed` event (none when nothing changes); the agent switches folders from its next start. 400 invalid_value when both are set; 404 project_not_found; 409 project_root_changed when the project's folder was moved or replaced since the project was made, or still needs confirming (needs_confirmation); 409 agent_home for an agent's home chat; 400 invalid_project_root for a workspace that is missing, not a folder, the file system root, the home folder or above it, inside openside's data, inside an operating-system folder or another person's home, or a credential folder such as ~/.ssh or above one. Attaching a workspace needs local access (403 local_access_required on other devices); moving into an existing project needs a person device or the local socket (403 person_device_required for agent_client devices); leaving needs neither.");
        }
        if route.path == "/v1/projects" && route.method == HttpMethod::Get {
            operation["description"] = json!("List every project, sorted by name.");
        }
        if route.path == "/v1/projects" && route.method == HttpMethod::Post {
            operation["description"] = json!("Create a project from an existing folder (root) or, without root, from scratch in a new empty folder under openside's data. Only the computer running openside creates projects (403 local_access_required elsewhere). 400 invalid_project_name for a blank name or one longer than 80 characters; 400 instructions_too_large over 16 KiB; 400 invalid_project_root for a folder that is missing, not a folder, the file system root, the home folder or above it, inside openside's data, inside an operating-system folder (such as /etc or /usr) or another person's home, or a credential folder such as ~/.ssh or above one; 409 project_exists when another project already uses the folder.");
        }
        if route.path == "/v1/projects/{id}" && route.method == HttpMethod::Get {
            operation["description"] = json!("One project; 404 project_not_found.");
        }
        if route.path == "/v1/projects/{id}" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Rename a project, replace its instructions, or confirm its folder; the folder is fixed. Chats in the project receive the new instructions from their agent's next start. confirm_root checks the project's folder again as at creation and records it, clearing needs_confirmation; only the computer running openside may confirm (403 local_access_required elsewhere; 400 invalid_project_root when the folder no longer passes). Any other change from the computer running openside to a project that needs confirming also records its folder when it still passes. 400 invalid_value for an empty body, invalid_project_name or instructions_too_large; 404 project_not_found; 403 person_device_required for agent_client devices (the instructions become part of every project chat's system prompt).");
        }
        if route.path == "/v1/projects/{id}" && route.method == HttpMethod::Delete {
            operation["description"] = json!("Delete a project and return the remaining ones. Its chats stay and leave the project first, in one commit (each gets `session.folder_changed` with no project and no folder); the folder and its files are never deleted. 404 project_not_found; 403 person_device_required for agent_client devices.");
        }
        if route.path == "/v1/agents/{id}" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Edit the profile for the next turn. Omitted nullable overrides are unchanged; explicit null inherits defaults. Permission widening and skills/model/account-pool/channel changes additionally require Admin and a person device (local socket exempt). Immutable identity and home fields cannot be patched.");
        }
        if route.path == "/v1/sessions/{id}" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Change a chat. title, pinned and archived are recorded as one session.updated event; archiving an isolated-space session disposes its browser space. model, effort, mode and confirmation are recorded as one session.options_updated event and apply from the chat's next model request. Paired devices get 403 local_socket_required for a mode looser than the daemon default, and agent_client devices also for effort \"xhigh\"/\"max\". A model a person picks becomes the default for new chats; agent clients and `openside mcp` change only this chat. read marks the chat read or unread as a session.read event. Fields equal to the current value record nothing; 409 agent_home for a model or mode change of an agent's home chat.");
        }
        if route.path == "/v1/tickets" {
            parameters.push(json!({"name": "status", "in": "query", "required": false,
                "description": "Return only tickets with the selected lifecycle status.",
                "schema": reference("TicketStatus")}));
        }
        if matches!(
            route.path,
            "/v1/tickets/{id}/resolve" | "/v1/tickets/{id}/reject"
        ) {
            operation["description"] = json!("The daemon checks the ticket kind: confirm/info/choice require approve; secret_input/device_approval/takeover/login require human_step. A secret_input answer that registers an authenticator ({\"value\":{\"totp\":…}} on /resolve) and a login answer that keeps the login ({\"value\":{\"values\":…,\"save\":true}}) also require approve.");
            operation["x-capability-by-ticket-kind"] = json!({
                "confirm": CapabilityScope::Approve,
                "info": CapabilityScope::Approve,
                "secret_input": CapabilityScope::HumanStep,
                "device_approval": CapabilityScope::HumanStep,
                "takeover": CapabilityScope::HumanStep,
                "login": CapabilityScope::HumanStep,
                "choice": CapabilityScope::Approve,
            });
        }
        if route.path == "/v1/tickets/{id}/resolve" {
            operation["description"] = json!(format!(
                "{} Confirm approval, choice answers, TOTP enrollment, and login save:true require a person_device; agent clients and the Unix socket receive 403 person_approval_required and the ticket stays pending. A person device that directly created the ticket, or was recorded as its session driver, receives 403 self_approval_forbidden. Pairing fixes the device kind. Deny/reject and cancel remain allowed with the appropriate scope.",
                operation["description"].as_str().unwrap_or_default()
            ));
            operation["responses"]["403"] = json!({
                "description": "Caller lacks the ticket scope or the required person-device authority, or tries to approve its own ticket",
                "content": {"application/json": {"schema": reference("ApiError"),
                    "example": {"code": "person_approval_required", "message": "person approval required"}}}
            });
        }
        if route.path == "/v1/tickets/{id}/done" {
            operation["description"] = json!("Complete a device_approval or takeover ticket.");
        }
        if route.path == "/v1/tickets/{id}/takeover" {
            operation["description"] = json!("WebSocket upgrade for a pending takeover ticket whose target has a tab_id. The daemon streams JPEG frames of that tab (TakeoverServerMessage, JSON text) and forwards TakeoverInput messages to that tab only, with coordinates in viewport CSS pixels. One controller per ticket (409 takeover_in_use); an Origin header must match the Host. Browsers, which cannot set Authorization on a WebSocket, send the token as `Sec-WebSocket-Protocol: openside.v1, openside.bearer.<token>`; the daemon answers with `openside.v1` only. The stream ends with an `ended` message on done/reject/expiry, idle timeout, token revocation or tab loss. While the ticket is pending, agent browser tools on the tab are refused with takeover_active.");
            operation["responses"]["101"]["description"] = json!("Switching to WebSocket");
            operation["responses"]["101"]["x-websocket-server-message"] =
                reference("TakeoverServerMessage");
            operation["responses"]["101"]["x-websocket-client-message"] =
                reference("TakeoverInput");
            operation["responses"]["409"] = json!({"description": "Another controller is connected",
                "content": {"application/json": {"schema": reference("ApiError"),
                "example": {"code": "takeover_in_use", "message": "takeover in use"}}}});
        }
        if route.path == "/v1/sessions/{id}/browser/preview" {
            operation["description"] = json!("Read-only WebSocket preview of the session's current owned tab. Requires browse (steer implies browse), since JPEG pixels cannot be redacted. JPEG quality 40, at most 720x450 and 2 fps delivery; capture has a bounded startup allowance of at most 3 frames, then sustained 2 fps. Client messages are ignored and limited to 1 KiB. Secret-tainted tabs are withheld; takeover pauses the preview and takes priority. At most one frame admitted before a safety transition may finish sending within the 5-second writer deadline; later captured frames are excluded. Private sessions return 409 private_session; at most 8 viewers per session (429 too_many_viewers). Device revocation ends its sockets; committed stop/deletion ends existing viewers even if the session immediately resumes. Origin and browser bearer subprotocol rules match takeover.");
            operation["responses"]["101"]["description"] =
                json!("Switching to read-only WebSocket");
            operation["responses"]["101"]["x-websocket-server-message"] =
                reference("PreviewServerMessage");
            operation["responses"]["101"]["x-websocket-client-message"] = Value::Null;
            operation["responses"]["409"] = json!({"description":"Private session", "content":{"application/json":{"schema":reference("ApiError"),"example":{"code":"private_session","message":"private session"}}}});
            operation["responses"]["429"] = json!({"description":"Too many preview viewers", "content":{"application/json":{"schema":reference("ApiError")}}});
        }
        if route.path == "/v1/sessions/{id}/browser/tabs/{tab_id}/control" {
            operation["description"] = json!("WebSocket upgrade that gives a person control of one of the session's agent tabs without a takeover ticket (the browser sheet). Requires steer, a person_device token or the local socket (agent_client tokens and MCP socket clients get 403 person_device_required), and an Origin matching the Host; the bearer subprotocol works as on the takeover stream. The daemon streams TakeoverServerMessage frames and `page` updates and accepts TakeoverInput, including navigate/back/forward/reload/close_tab. Navigation accepts only http(s) URLs without credentials and is refused with input_error policy_denied when the policy denies the origin, as for agent navigation. While the stream is open, agent browser tools on the tab are refused with takeover_active. One controller per tab (409 control_in_use); a pending takeover ticket on the tab returns 409 takeover_active; private sessions return 409 private_session. The control ends when the client closes the socket, after 300 s without accepted input (ended idle), on token revocation (ended revoked) or when the tab is gone (ended tab_unavailable); the agent then regains the tab with its earlier snapshots and references voided. Opening, navigation, tab closing and the end of control are written to the audit log; typed text is not.");
            operation["responses"]["101"]["description"] = json!("Switching to WebSocket");
            operation["responses"]["101"]["x-websocket-server-message"] =
                reference("TakeoverServerMessage");
            operation["responses"]["101"]["x-websocket-client-message"] =
                reference("TakeoverInput");
            operation["responses"]["403"] = json!({"description": "Not a person's device, or the Origin does not match",
                "content": {"application/json": {"schema": reference("ApiError"),
                "example": {"code": "person_device_required", "message": "person device required"}}}});
            operation["responses"]["409"] = json!({"description": "Another controller, a pending takeover or a private session",
                "content": {"application/json": {"schema": reference("ApiError"),
                "example": {"code": "control_in_use", "message": "control in use"}}}});
        }
        if route.path == "/v1/sessions/{id}/queue/{mid}/move" {
            operation["description"] = json!("Reorder a pending message inside its own lane: steering messages are delivered before follow-ups and a follow-up becomes a steer only through promote. position is zero-based and clamped to the lane length; a message that was already delivered or dropped returns 409.");
        }
        if route.path == "/v1/devices/pair" {
            operation["description"] = json!("Only callable over the Unix socket, whose callers are implicitly admin (local user); TCP callers get 403 local_socket_required even with an admin token, so a leaked token cannot mint further tokens. Grant the narrowest scopes: read, steer, approve, human_step, browse, vault_use, otp_ingest or admin. admin grants every scope and steer also grants browse; no other scope implies another.");
        }
        if PUBLIC_ROUTES.contains(&(route.method.as_str(), route.path)) {
            operation["security"] = json!([]);
            operation["x-public"] = json!(true);
        }
        if route.path == "/v1/pairing" {
            operation["description"] = json!("Issue a one-time pairing code valid for 180 monotonic seconds. Requires the local socket, or verified loopback TCP with a socket-issued person_device admin token and current X-Openside-Local-Secret. Proxied/relay traffic is refused with 403 local_access_required; loopback alone never proves locality. Requested scopes cannot exceed caller grants (403 scope_ceiling_exceeded). The local operator fixes person_device or agent_client in the offer; redemption cannot change kind. Codes and local secrets are never audited. While Remote Control is off the offer carries no link (url and transport are null) and naming a transport in via is refused with 409 remote_disabled.");
        }
        if route.path == "/v1/remote/local-access" {
            operation["description"] = json!("Local socket only: every call atomically replaces the one managed owner-bootstrap device/token, closes the predecessor's streams/polls, clears its push subscriptions, and audits rotation. A distinct persistent bootstrap marker survives boots; intentional CLI devices are never revoked by display name. Returns the per-boot locality secret and fresh person/admin token plus a loopback launch URL with credentials in its fragment. TCP/relay callers get 403 local_socket_required. Keep the proof only in sessionStorage; strip the fragment immediately and clear proof on sign-out, rejection or identity change. With a listener it also returns extension_code: a one-time pairing code only the openside extension's own origin may redeem, for a person device (read, steer, approve, human_step) that replaces the extension device of the previous launch. person_browser_runtime_dir is the private directory `openside-browser` gives the person's browser for its person-role gateway and launch record; the daemon looks for that browser nowhere else.");
        }
        if route.path == "/v1/remote/disable" {
            operation["description"] = json!("Turn Remote Control off. Needs local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret; 403 local_access_required otherwise) and a person: an `openside mcp` client (x-openside-client: mcp) or an agent-client token gets 403 person_access_required. Saves remote.enabled = false and, from that moment, refuses every TCP request that does not come directly from this computer with 403 remote_disabled: other addresses, the relay, and loopback requests carrying proxy or tunnel headers (Forwarded, Via, X-Forwarded-*, Cf-*, Tailscale-*) or a non-loopback Host. The Unix socket and the local web UI keep working. Then stops every transport (Tailscale Serve handler, cloudflared, relay registration), keeping each one's saved settings, and goes on past failures: still_on lists the transports that could not be stopped. Audited as remote/disabled.");
        }
        if route.path == "/v1/remote/enable" {
            operation["description"] = json!("Turn Remote Control back on with the saved transports: saves remote.enabled = true, then restarts each configured transport (Tailscale Serve handler, cloudflared, relay registration). A transport that fails to start shows its error in the returned status. Same access rules as /v1/remote/disable (403 local_access_required, 403 person_access_required). Audited as remote/enabled.");
        }
        if route.method == HttpMethod::Put && route.path.starts_with("/v1/remote/") {
            operation["description"] = json!("Turn one transport on or off and save its settings. Turning one on while Remote Control is off is refused with 409 remote_disabled and changes nothing; turn Remote Control on first (POST /v1/remote/enable). Turning one off works either way.");
        }
        if route.path == "/v1/pairing"
            || route.path == "/v1/access-requests/{id}/approve"
            || route.path.starts_with("/v1/remote/") && route.path != "/v1/remote/local-access"
        {
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot local-person proof obtained only over the local socket. Required for TCP pairing/configuration; never grants authority to a QR-paired, agent or proxied caller.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/sessions/{id}/events/poll" {
            operation["description"] = json!("Bounded long polling for transports that buffer SSE. At most 512 raw durable events per page, ordered by seq; last_seq is the exclusive replay cursor and includes filtered subagent events. Subscribe-before-read prevents replay/live gaps. Pending requests wake immediately on device revocation. Delivers the deletion tombstone as the final event, then 404. A cursor beyond an active session's durable log returns 400 invalid_event_id.");
            parameters.push(json!({"name":"after","in":"query","required":false,"schema":{"type":"integer","minimum":0,"default":0}}));
            parameters.push(json!({"name":"wait","in":"query","required":false,"schema":{"type":"integer","minimum":0,"maximum":25,"default":20}}));
            parameters.push(json!({"name":"subagents","in":"query","required":false,"schema":{"type":"boolean","default":true}}));
        }
        if route.path == "/v1/devices/me/access-requests" {
            operation["description"] = json!("A person's device asks for scopes it does not hold, for example admin to manage the vault. The request waits ten minutes for someone on the computer running openside to allow or decline it; a new request from the same device replaces the old one. 400 invalid_value when the device already holds every requested scope; 403 person_approval_required for an agent client.");
        }
        if route.path == "/v1/access-requests/{id}/approve" {
            operation["description"] = json!("Grants the requested scopes to the device, in addition to the ones it holds. Requires the same local-person authority as pairing (local socket, or verified loopback TCP with a socket-issued person_device admin token and the current X-Openside-Local-Secret); a QR-paired, agent or proxied caller gets 403 local_access_required. 404 not_found for an unknown, expired or already decided request.");
        }
        if route.path == "/v1/pairing/exchange" {
            operation["description"] = json!("No bearer token: the one-time code is the credential. The token inherits the operator-selected kind from the offer; sending kind at redemption is rejected as an unknown field. 404 pairing_not_found for an unknown or already used code, 410 pairing_expired after 180 seconds. A valid request consumes the code on its first exchange attempt.");
        }
        if route.path == "/v1/onboarding" && route.method == HttpMethod::Get {
            operation["description"] = json!("State of the first-run guide, kept on the daemon so every device agrees: `pending` until someone finishes or skips it once.");
        }
        if route.path == "/v1/onboarding" && route.method == HttpMethod::Put {
            operation["description"] = json!("Record how the guide ended (`completed` or `skipped`; `pending` is 400 invalid_value). A compare-and-set: it writes only while the stored state is `pending`, so of two devices finishing the guide at once exactly one outcome is kept. Reopening the guide from Settings does not reset the flag.");
            operation["responses"]["400"] = json!({"description": "`pending` was sent",
                "content": {"application/json": {"schema": reference("ApiError"),
                "example": {"code": "invalid_value", "message": "invalid value"}}}});
            operation["responses"]["409"] = json!({"description": "The guide already ended; nothing was written. The body carries the outcome already recorded",
                "content": {"application/json": {"schema": reference("OnboardingAlreadySet"),
                "example": {"code": "onboarding_already_set", "message": "onboarding already set",
                    "state": "completed", "updated_at": "2026-01-01T00:00:00Z"}}}});
        }
        if route.path == "/v1/external-agents" {
            operation["description"] = json!("The agent CLIs openside can install its browser skill for: the `claude`, `codex` and `omp` presets, then any agent whose skills folder was added with `PUT /v1/external-agents/{id}` (stored as a `[skills.agents]` entry). `detected` means the executable is on the daemon's PATH or the agent's configuration directory exists; the skills directory follows the same rules as `openside skills install`.");
        }
        if route.path == "/v1/external-agents/{id}/browser-skill" {
            operation["description"] = json!("Install the bundled `openside-browser` skill into the agent's skills directory (what `openside skills install <agent>` does). The content is fixed and the directory comes from the preset or configuration, never from the request. The agent trusts what lands there, so this needs local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret; 403 local_access_required otherwise) and a person: an `openside mcp` client (x-openside-client: mcp) or an agent-client token gets 403 person_access_required. Right before writing, the directory, presets included even when it comes from CLAUDE_CONFIG_DIR, CODEX_HOME, OMP_CODING_AGENT_DIR or PI_CODING_AGENT_DIR, must be absolute, inside the user's home, reached without any symbolic link below home and outside openside's data directory (400 invalid_skills_dir; nothing is written). 404 unknown_agent, 409 exists (retry with `force`), 400 skills_dir_unknown (add the folder with `PUT /v1/external-agents/{id}`). Audited.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/external-agents/{id}" && route.method == HttpMethod::Put {
            operation["description"] = json!("Add or change the skills folder of an agent that is not a preset. Requires local access (the computer's own user, or its launcher with X-Openside-Local-Secret): 403 local_access_required otherwise. The id is lowercase letters, digits and hyphens and not a preset name (400 invalid_agent); the folder must be absolute or `~/…`, without `..`, and resolve inside the user's home, config or data folders (400 invalid_skills_dir). Persisted as `[skills.agents]` in config.toml and effective at once; audited.");
        }
        if route.path == "/v1/external-agents/{id}" && route.method == HttpMethod::Delete {
            operation["description"] = json!("Forget a custom agent's skills folder. Requires local access. Presets cannot be removed (400 invalid_agent); 404 unknown_agent. Installed skill files are not touched.");
        }
        if route.path == "/v1/memory/model" {
            operation["description"] = json!("The local embedding model and the download job: whether both model files exist, the stage and byte progress of the current or last download, its error code and whether a restart is needed before vector search starts. The daemon downloads only when asked; nothing runs implicitly.");
        }
        if route.path == "/v1/memory/model/download" {
            operation["description"] = json!("Start downloading the embedding model into `memory.embedding.model_dir`: the file list and per-file checksums come from the model hub (SHA-256 for weights, git blob id for plain files), every request and redirect must pass `network.allow`, both files must match their checksum and load as a static model, and only then is the model swapped in atomically. One download at a time (409 download_in_progress). An empty body fetches the default model from the default source; naming a repo, revision or endpoint needs local access (403 local_access_required). 400 invalid_model_source; the outcome (including checksum_mismatch, checksum_unavailable, egress_refused, download_failed, unsupported_model) is reported by GET /v1/memory/model.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof; only needed to name a different repo, revision or endpoint.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/memory/model/cancel" {
            operation["description"] = json!("Stop the running download; the installed model and the staged files are left untouched. A no-op when nothing is running.");
        }
        if route.path == "/v1/memory/files" {
            operation["description"] = json!("The memory store shown as three files: MEMORY.md (notes not tagged profile), USER.md (notes tagged profile) and TAXONOMY.md (tags and kinds, read-only). Visited-page history is not part of these files.");
        }
        if route.path.starts_with("/v1/memory/files/") {
            parameters.push(json!({"name":"name","in":"path","required":true,
                "schema":{"type":"string","enum":["MEMORY.md","USER.md","TAXONOMY.md"]}}));
        }
        if route.path == "/v1/memory/files/{name}" && route.method == HttpMethod::Put {
            operation["description"] = json!("Save an edited MEMORY.md or USER.md in one transaction: blocks under a `<!-- memory id=… -->` marker update that note, removed blocks delete their note, and text before the first marker or under a bare `<!-- memory -->` marker adds notes (USER.md adds the profile tag; dropping or adding the profile tag moves a note between the two files). A note changed, added or removed elsewhere after `revision` fails the whole save with 409 memory_conflict. A note whose text contains a hidden Vault value cannot be edited here (400 redacted_memory); secrets are 400 secret_detected; TAXONOMY.md is 400 read_only_view.");
        }
        if route.path == "/v1/memory/history" {
            operation["description"] = json!("Every memory change (create, update, delete, purge), newest first, across all memories. Deleted and purged entries keep only operation, actor and time.");
            parameters.push(json!({"name":"limit","in":"query","required":false,"schema":{"type":"integer","minimum":1,"maximum":200,"default":50}}));
            parameters.push(json!({"name":"before","in":"query","required":false,"description":"next_before of the previous page.","schema":{"type":"integer"}}));
        }
        if route.path == "/v1/memory/rebuild" {
            operation["description"] = json!("Rebuild the full-text index of every live memory and recompute embeddings with the loaded local model. Expired memories are purged first.");
        }
        if route.path == "/v1/memory/reveal" {
            operation["description"] = json!("Write a copy of MEMORY.md, USER.md and TAXONOMY.md, as GET /v1/memory/files/{name} serves them, into `<data_dir>/memory/files` (0700 folder, 0600 files, replaced on every call) and open that folder in the file manager of the computer running openside. Edits made to the copy are not read back. Requires local access (403 local_access_required). 503 file_manager_unavailable when no file manager could be started.");
        }
        if route.path == "/v1/folders/pick" {
            operation["description"] = json!("Open the native folder picker on the screen of the computer running openside and wait until the person chooses a folder or closes it. Returns the canonical path, or `path: null` when the picker was closed without a choice. The folder must be usable as a project folder: not the file system root, the home folder or a folder above it, nor openside's data folder, a folder above it or anything inside it, nor inside an operating-system folder or another person's home, nor a credential folder such as ~/.ssh or a folder above one (400 invalid_project_root). Requires local access (403 local_access_required). 409 folder_picker_busy while another picker is open; 503 folder_picker_unavailable when no folder picker could be started.");
        }
        if route.path.starts_with("/v1/mcp/servers") {
            operation["description"] = json!(match (route.method, route.path) {
                (HttpMethod::Get, "/v1/mcp/servers") => "External MCP servers whose tools agents can call, with connection state and tool lists. Secret values are never returned; secrets lists their names.",
                (HttpMethod::Post, _) if route.path.ends_with("/refresh") => "Reconnect the server and list its tools again; returns the state after the attempt. Requires local access (403 local_access_required): reconnecting a stdio server runs its program.",
                (HttpMethod::Post, _) => "Connect a stdio (command) or http (URL) MCP server. Secret values go to the Vault, which must be set up and unlocked (409 vault_unavailable; 403 biometric_required when a new keyring vault would be needed but biometric unlock is on, so a person sets it up first). Requires local access (403 local_access_required): a stdio server runs a program on this computer. 409 mcp_server_exists, 409 mcp_server_limit, 400 invalid_value. The response reflects the first connection attempt.",
                (HttpMethod::Patch, _) => "Change a server; secrets set to null are removed from the Vault. Requires local access. The server reconnects with the new settings.",
                (HttpMethod::Delete, _) => "Stop the server, delete its Vault secrets and remove it from config.toml. Requires local access.",
                _ => "One configured MCP server.",
            });
        }
        if route.path == "/v1/sessions/{id}/fork" {
            operation["description"] = json!("Branch the chat at a reply: a new chat titled `<title> (branch)` holding the messages and replies up to and including the finished reply to `from_message`, with the same agent, model, effort, permission, confirmation, project, workspace and browser space (a branch of an agent's home chat is an ordinary chat of that agent). The agent receives that conversation as context; tool steps and attachments are not copied (attachments are named in the copied text). The copied events end with `session.branched`. 404 message_not_found; 409 message_not_finished while that message has no finished reply; 409 agent_retired; 409 project_root_changed when the project's folder was moved or replaced, or still needs confirming; 403 local_socket_required when a paired device could not create such a chat itself (a project's folder never counts against a person device; for an agent_client device, or when the project no longer exists, the branch keeps the folder as a plain workspace without the project).");
        }
        if route.path == "/v1/doctor" {
            operation["description"] = json!("Run the diagnostics of `openside doctor` inside the daemon and return them grouped by area, with a web fix for findings that have one. The vault probe reads only the file header and the keyring state. One report at a time; callers wait for the run in progress.");
        }
        if route.path == "/v1/daemon" {
            operation["description"] = json!("Identity of the running process: version, a random `boot_id` that changes on every start, when it started and how a restart would be carried out. A client that asked for a restart waits until `boot_id` changes.");
        }
        if route.path == "/v1/daemon/restart" {
            operation["description"] = json!("Restart the daemon by graceful shutdown and in-place exec with the same PID and arguments, including under systemd or launchd. Returns 202 with the current boot_id; wait for a new boot_id to reconnect. Requires local access (403 local_access_required). A private single-use 0600 handoff in the 0700 data directory preserves the launcher secret only for this controlled exec, bound to the old boot_id and restart request id by an exec-only environment marker, with a five-minute TTL. Ordinary external or service-manager restarts rotate the secret.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/policy/validate" {
            operation["description"] = json!("Check an edit of the live policy and/or the network allow-list without saving it. Returns every problem with the list and zero-based index of the entry, so a form can mark the exact row; PUT /v1/policy and PUT /v1/policy/network apply the same checks.");
        }
        if route.path == "/v1/policy/network" && route.method == HttpMethod::Get {
            operation["description"] = json!("The saved outbound allow-list (`network.allow`) and whether the running daemon still enforces an older one.");
        }
        if route.path == "/v1/policy/network" && route.method == HttpMethod::Put {
            operation["description"] = json!("Replace `network.allow` in config.toml (validated like the file; 400 invalid_network_allow). Requires local access; it applies after the daemon restarts. Audited.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/sync/config" {
            operation["description"] = json!("Choose where synced data is stored: a shared directory or a WebDAV URL (https, or http on loopback), exactly one or neither to turn sync off, plus whether the live policy syncs and which memory tags sync. Requires local access. The backend of a device that already joined an account cannot change (409 sync_already_set_up). Persisted in `[sync]` of config.toml and effective at once without a restart. 400 invalid_sync_config.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/sync/unlock" {
            operation["description"] = json!("Derive the sync key for the account this device joined from a passphrase typed in the app (or from `sync.passphrase_cmd` when none is given) and keep it in daemon memory only. A WebDAV password typed here is likewise memory-only. Requires local access when a secret is sent. 400 sync_wrong_passphrase; 409 sync_not_set_up, sync_passphrase_unavailable.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path.starts_with("/v1/import") {
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket; paired, agent and proxied callers cannot use it.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/import/profiles" {
            operation["description"] = json!("Detect all browser profiles in standard local locations, including Chromium brands, Firefox forks and Safari on macOS. Includes per-kind availability, local display metadata and permission warnings; never source paths. Requires local-person authority.");
        }
        if route.path == "/v1/import" {
            operation["description"] = json!("Start importing selected profiles and kinds into the agent's default browser profile; omitted selections mean everything from every detected profile. Requires local-person authority. Passwords go only into an unlocked openside Vault; 409 vault_setup_required or vault_locked before writes. This explicit run is the credential-access phase: macOS may show a Keychain permission request; Linux never requests unlocking. Returns an asynchronous job; progress is available through job polling or job SSE. One write at a time (409 import_in_progress). Keyring/encryption failures skip only the affected kind and are included in the summary.");
        }
        if route.path == "/v1/import/preview" {
            operation["description"] = json!("Preview selected local profiles without target writes or browser startup; omitted selections mean all profiles and kinds. Cookie/password counts come only from source rows or binary record headers: no decryption, OS-keyring access, Unlock or credential verification. No unlocked Vault is required. Notes explain that the system may ask for access during the explicitly started import; actual counts can decrease after expired/unsupported/duplicate filtering. Other kinds are parsed and URL-sanitized normally. Requires local-person authority.");
        }
        if route.path == "/v1/import/passwords" {
            operation["description"] = json!("Import browser-exported CSV passwords directly from an in-memory request into the encrypted Vault. Accepts Chrome, Edge, Firefox and Safari URL/username/password headers; never stores the CSV file or returns values. Dedupes normalized origin and username through Vault save_login. Requires local-person authority and an unlocked Vault (409 vault_setup_required, vault_locked).");
        }
        if route.path == "/v1/import/jobs/{id}/events" {
            operation["description"] = json!("Replay durable per-job progress then stream new progress with SSE event import_progress and monotonically increasing event ids. Last-Event-ID resumes a stream; final completed/failed progress closes it. No secret data is included; local-person authority is required.");
        }
        if route.path == "/v1/import/delivery" {
            parameters.push(json!({"name":"target","in":"query","required":false,
                "description":"Independent migration destination; defaults to the dedicated desktop user profile.",
                "schema":reference("ImportDeliveryTarget")}));
            operation["description"] = json!("Local-person-only, value-free pending revision descriptors. Returns batch IDs and exact cookie permission origins, never bookmarks/history records or cookie values. Mint a device-bound single-use ticket for the paired extension; only the extension fetches authenticated native payloads. Agent cookies and read-only side-store destinations are already applied.");
            operation["responses"]["200"]["headers"]["Cache-Control"] =
                json!({"schema":{"type":"string"},"description":"Always no-store."});
        }
        if route.path == "/v1/import/delivery/{id}/ack" {
            operation["description"] = json!("Acknowledge one migration revision only after the selected destination applied it. Targets acknowledge independently; retries are idempotent. Requires the same local-person authority as importing browser data.");
        }
        if route.path == "/v1/import/delivery/{id}/ticket" {
            operation["description"] = json!("Local-person-only authorization of a pending revision for one import-enabled extension device. Ticket expires after120 seconds and is consumed once by that device; not a cookie or data payload. A retry needs a new ticket and uses native-store idempotency.");
        }
        if route.path == "/v1/extension/import/{ticket}" {
            operation["description"] = json!("Paired device-token fetch of its one-use native delivery ticket. HMAC-SHA256 authenticates ticket || batch_id || hex(SHA256(exact UTF-8 payload)) with the32-byte secret established only at locally authorized, pinned-extension-origin redemption. Payload never goes through a web page; owned cookie/payload buffers are zeroized where feasible, with JS/HTTP/WebSocket/library residuals. The extension verifies before native writes.");
            operation["responses"]["200"]["headers"]["Cache-Control"] = json!({"schema":{"type":"string"},"description":"Always no-store; payload contains cookie values."});
        }
        if route.path == "/v1/devices/{id}" && matches!(route.method, HttpMethod::Delete) {
            operation["description"] = json!("Revoke a device token. The next request with it gets 401, its open SSE streams end, and its push subscriptions are deleted.");
        }
        if route.path == "/v1/devices/{id}" && matches!(route.method, HttpMethod::Patch) {
            operation["description"] = json!("Rename a paired device. The name is trimmed and must be 1-100 characters (400 invalid_value); 404 not_found for an unknown device. An agent client may rename only itself (403 person_approval_required). Audited as `device` `renamed`.");
        }
        if route.path == "/v1/push/presence" {
            operation["description"] = json!("focused=true suppresses pushes to that subscription for 60 seconds unless renewed; focused=false ends it. 404 unknown_subscription for another device's subscription.");
        }
        if route.path == "/v1/otp" {
            operation["description"] = json!("Forwarded SMS. 403 sender_not_allowed unless the sender is in otp.senders. A code extracted by an otp.sites pattern fills pending secret_input tickets of that exact origin, or is kept in memory for otp.ttl_secs for a step that opens later. Codes are never returned, logged or persisted.");
        }
        if route.path == "/v1/settings/config" {
            operation["description"] = json!("The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy). Who may change what: a caller with local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret) may change any key, except that preferences.mcp_server and preferences.computer_use change only for a person: an `openside mcp` client (x-openside-client: mcp) or an agent-client token gets 403 person_access_required, audited as `refused_agent`. A paired person device with admin scope anywhere else may change only the behaviour keys listed in `remote_editable`; changing any other key returns 403 local_access_required, writes nothing and is audited as `refused_remote`. Agent clients never write. Security-sensitive keys (sandbox, network allow-list, remote transports, credentials sources, executables, paths, trust lists) are never remote-editable.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof that makes a loopback TCP caller local; paired devices elsewhere omit it.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/settings/cli" {
            operation["description"] = json!(if route.method == HttpMethod::Get {
                "The browser profile new CLI and MCP sessions on this computer use: the `profile` key of `cli.toml` beside config.toml, which `openside settings set-default-profile` also writes. profile_is_default is true when none is saved (new sessions use the account's first profile, `shared`). 500 cli_settings_invalid when the file is not valid TOML or names an unknown profile; the file is left as it is."
            } else {
                "Save the default profile (`null` removes the key, back to the account's first profile). Only `profile` is rewritten; every other key and comment of cli.toml stays, and the file is created when missing. Requires local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret): 403 local_access_required otherwise. 500 cli_settings_invalid leaves an unreadable file untouched. Audited."
            });
            if route.method == HttpMethod::Patch {
                parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                    "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                    "schema":{"type":"string","minLength":64,"maxLength":64}}));
            }
        }
        if route.path == "/v1/settings/preferences" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Change some of the person's preferences; omitted fields keep their value and they apply at once. mcp_server and computer_use need local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret): 403 local_access_required otherwise, audited as `refused_remote`. They also need a person: an `openside mcp` client (x-openside-client: mcp) or an agent-client token gets 403 person_access_required even on the socket, audited as `refused_agent`. Agent clients without local access get 403 forbidden for every field. 400 invalid_preferences for a value the file would refuse, 400 invalid_value for an empty body.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof that makes a loopback TCP caller local; only needed for mcp_server and computer_use.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/settings/models" {
            operation["description"] = if route.method == HttpMethod::Get {
                json!("Local model readiness grouped by provider, from configured ids, source accounts and curated chat/tool-capable models. Credential values, email and upstream account identities are never included. Metadata is cached for at most five seconds; key/source/Vault lifecycle writes invalidate it. Readiness is not an upstream entitlement guarantee. Each model's default_effort is the effort a chat runs at on it when no effort is picked (null: the provider's own default, or an image model); a create or PATCH without effort keeps it unset.")
            } else {
                json!("Set the role models without raw TOML editing: default, fast, ordered fallbacks, and optional standard, deep, visual and image (omitted keeps, null clears; unset roles use the session model). Requires admin; every id must be available in the current catalog (400 model_unavailable), chat roles need kind chat and image needs kind image (400 invalid_model_kind), at most16 distinct fallbacks and no default duplicate (400 invalid_models). Atomically persists selection without daemon restart. Ordinary sessions retain stored models; Team homes use their Agent override or the current default and reopen on the next idle/real turn when effective selection changes. Running requests finish their captured model. Response effective:new_sessions_and_home_next_turn.")
            };
        }
        if route.path == "/v1/settings/models/providers" {
            operation["description"] = json!("Connected providers for the Providers list: one row per provider and connection kind (subscription or api_key) with a product display name and connected/needs_attention status. `accounts` carries the internal account references behind a row for Reconnect and Disconnect; it is not display text. Credential values and upstream identities are never included.");
        }
        if route.path == "/v1/settings/keys" {
            operation["description"] = json!("Administrator-only labelled account metadata: provider, key_id, label, last_four, created_at, enabled and reserve_percent. No API path returns key material; read-scoped devices receive403.");
        }
        if route.path == "/v1/settings/keys/{provider}"
            || route.path == "/v1/settings/keys/{provider}/{key_id}"
        {
            parameters.push(json!({"name":"provider","in":"path","required":true,
                "schema":{"type":"string","enum":["anthropic","openai","google"]}}));
            if route.path.contains("{key_id}") {
                parameters.push(
                    json!({"name":"key_id","in":"path","required":true,"schema":{"type":"string"}}),
                );
            }
            operation["description"] = json!("Requires admin plus person_device (local socket exempt). POST creates one labelled account; PUT replaces its key, PATCH edits metadata, DELETE removes only that account. Values arrive only in secret input bodies; no key in response/config/log/argv/env. Subsequent request snapshots observe changes; missing account404, locked Vault409; 403 biometric_required when a new keyring vault would be created while biometric unlock is on.");
            operation["responses"]["403"] = json!({"description":"Missing admin scope or person-device authority",
                "content":{"application/json":{"schema":reference("ApiError")}}});
            operation["responses"]["409"] = json!({"description":"Vault unavailable, or an ordinary legacy item conflicts with the provider id",
                "content":{"application/json":{"schema":reference("ApiError")}}});
        }
        if route.path.starts_with("/v1/settings/subscriptions") {
            if route.path.contains("{source}") {
                parameters.push(json!({"name":"source","in":"path","required":true,
                    "schema":{"type":"string","enum":["claude-code","codex-cli"]}}));
            }
            if route.path.contains("{account_id}") {
                parameters.push(json!({"name":"account_id","in":"path","required":true,"schema":{"type":"string"}}));
            }
            operation["description"] = json!("Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; openside does not sign these CLI accounts in (openside-held sign-in is /v1/settings/oauth). PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.");
        }
        if route.path.starts_with("/v1/settings/oauth/") {
            for name in ["provider", "account_id", "transaction_id"] {
                if route.path.contains(&format!("{{{name}}}")) {
                    parameters.push(
                        json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}}),
                    );
                }
            }
            operation["description"] = json!("openside-held OAuth accounts (source pi-oauth). Requires admin plus person_device (local socket exempt); agent clients and channel callers get 403 person_device_required. Sign-in runs as an in-memory transaction owned by the starting device (others get 404): poll it, paste a code only while manual_input is true, cancel it. A helper on the daemon host talks to the provider; tokens are stored only in the Vault and never appear in replies. join_pool appends a new account to its provider pool; DELETE removes it locally and from every pool without provider revocation.");
        }
        if route.path == "/v1/settings/pools/{provider}" {
            parameters.push(
                json!({"name":"provider","in":"path","required":true,"schema":{"type":"string"}}),
            );
            operation["description"] = json!("Replace the explicit ordered provider trust boundary. Requires admin plus person_device or local socket. Known compatible account refs only; no duplicates, at most64; empty disables routing. Unlisted sources never become automatic fallbacks.");
        }
        if route.path == "/v1/sessions/{id}/account" {
            operation["description"] = json!("Set/clear a preferred sticky account for the next request. Requires steer and membership in the existing effective global/Agent pool; preference never bypasses disablement, quota blocks or Agent restrictions. Invalid account400 account_not_in_pool.");
        }
        if route.path.contains("{path}") {
            parameters.push(json!({"name": "path", "in": "path", "required": true,
                "description": "Artifact path relative to the session artifact directory, percent-encoded as one segment (/ as %2F).",
                "schema": {"type": "string"}}));
        }
        if route.path == "/v1/sessions/{id}/artifacts/{path}" {
            operation["description"] = json!("Raw artifact bytes for download: Content-Disposition attachment, X-Content-Type-Options nosniff and a sandbox CSP, so the response never renders in the daemon origin.");
            operation["responses"]["200"]["content"] = json!({"application/octet-stream": {"schema": {"type": "string", "format": "binary"}}});
        }
        if route.path == "/v1/sessions/{id}/artifacts/{path}/preview" {
            operation["description"] = json!("Sandboxed rendering of an HTML, SVG, image, PDF or text artifact for an iframe. No bearer token: the exp/sig query from a listed preview_url is the credential (HMAC over session, path and expiry, valid 10 minutes, per daemon run). Responses carry Content-Security-Policy sandbox (scripts only for HTML, never allow-same-origin) with default-src 'none', so content runs in an opaque origin without network access. 400 invalid_preview_signature (missing, wrong or expired signature), 415 preview_unsupported.");
            for name in ["exp", "sig"] {
                parameters.push(json!({"name": name, "in": "query", "required": true,
                    "schema": {"type": "string"}}));
            }
            operation["responses"]["200"]["content"] =
                json!({"*/*": {"schema": {"type": "string", "format": "binary"}}});
        }
        if route.path == "/v1/sessions/{id}/visuals" {
            operation["description"] = json!("Visual blocks (fenced ```html visual or ```svg visual) from assistant.final events, each with its source text and a sandboxed preview_url.");
        }
        if route.path.contains("{share_id}") {
            parameters.push(json!({"name": "share_id", "in": "path", "required": true,
                "description": "Share id from ShareLink.id.",
                "schema": {"type": "string", "pattern": "^[A-Za-z0-9_-]{43}$"}}));
        }
        if route.path == "/v1/sessions/{id}/shares" && route.method == HttpMethod::Get {
            operation["description"] = json!("Live share links of the chat, newest first. Expired links are removed when listed.");
        }
        if route.path == "/v1/sessions/{id}/shares" && route.method == HttpMethod::Post {
            operation["description"] = json!("Publish a read-only snapshot of the chat as it is now. Only a person may share: agent-client and channel callers get 403 person_required. The snapshot holds delivered messages and replies, optionally one line per step (tool name and failure only, never arguments or results), and copies of the chosen artifacts; tickets, thinking, queued messages and tool data are never included. Live vault and session secrets are masked, a line that still looks like a credential is replaced whole, and a chosen text artifact that still looks like one is refused (400 sensitive_artifact). Only UTF-8 text and PNG, JPEG, GIF or WebP images can be shared (400 unsupported_share_artifact otherwise); images are re-encoded without metadata. 409 private_session for private chats, 409 vault_locked while a set-up vault is locked (its values could not be masked), 409 too_many_shares beyond 50 live links per chat, 400 invalid_share_artifact for an unknown, working or transcript path, more than 20 artifacts or a duplicate, 400 invalid_expiry outside 60 s..365 days, 413 share_too_large beyond 4 MiB of conversation, 10 MiB per artifact or 25 MiB of artifacts.");
        }
        if route.path == "/v1/sessions/{id}/shares/{share_id}" {
            operation["description"] = json!("Revoke a link: the snapshot and its artifact copies are deleted and the link answers 404 at once. Deleting the chat revokes all of its links.");
        }
        if route.path == "/v1/shares/{share_id}" {
            operation["description"] = json!("Public read-only snapshot behind a share link. No bearer token: the unguessable id is the credential. Unknown, revoked, expired and deleted-chat links all answer 404 share_not_found. Each link admits 120 requests per rolling minute and the daemon 60 failed lookups per rolling minute; beyond that 429 share_rate_limited with Retry-After 60. Responses are never cached and carry Referrer-Policy no-referrer and X-Robots-Tag noindex.");
            operation["responses"]["404"] = json!({"description":"share_not_found","content":{"application/json":{"schema":reference("ApiError")}}});
            operation["responses"]["429"] = json!({"description":"share_rate_limited","headers":{"Retry-After":{"schema":{"type":"string"},"description":"60 seconds"}},"content":{"application/json":{"schema":reference("ApiError")}}});
        }
        if route.path == "/v1/shares/{share_id}/artifacts/{artifact}" {
            parameters.push(json!({"name": "artifact", "in": "path", "required": true,
                "description": "SharedArtifact.index.",
                "schema": {"type": "integer", "minimum": 0}}));
            operation["description"] = json!("Public bytes of an artifact copied into a share, under the same id credential, 404 and rate limit as GET /v1/shares/{share_id}. PNG, JPEG, GIF and WebP images are inline; anything else is an attachment. Every response has X-Content-Type-Options nosniff and a sandbox CSP with default-src 'none', so nothing renders in the daemon origin.");
            operation["responses"]["200"]["content"] = json!({"application/octet-stream": {"schema": {"type": "string", "format": "binary"}}});
        }
        if route.path == "/v1/sessions/{id}/repl" {
            operation["x-capability-also-granted-by"] = json!([CapabilityScope::Steer]);
        }
        if route.path == "/v1/policy" && route.method == HttpMethod::Put {
            operation["description"] = json!("Replace the live policy. Invalid globs or a sensitive allow rule return 400 invalid_policy and change nothing; POST /v1/policy/validate reports which entry is wrong. Requires local access (403 local_access_required): the policy decides what agents may do, so it is edited from the computer itself. Applies to the next tool call of every session and is persisted in <data_dir>/policy.json, which overrides [policy] in config.toml.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP with a socket-issued local person/admin token. Omit only on the local socket.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/policy/test" {
            operation["description"] = json!(
                "Evaluate one capability/target against the live policy without running anything."
            );
        }
        if route.path == "/v1/routines/{id}/runs" {
            parameters.push(json!({"name": "limit", "in": "query", "required": false,
                "description": "Maximum runs to return, newest first; defaults to 100, capped at 500.",
                "schema": {"type": "integer", "minimum": 1}}));
        }
        if route.path == "/v1/sessions/{id}" && route.method == HttpMethod::Delete {
            operation["description"] = json!("Stop the session, record a durable session.deleted tombstone and hide it; later requests for the id return 404.");
        }
        if route.path == "/v1/routines/suggestions" {
            operation["description"] =
                json!("Draft routines inferred from repeated user messages; nothing is created.");
        }
        if route.path == "/v1/memory" && route.method == HttpMethod::Get {
            operation["description"] = json!("Hybrid search: FTS5 bm25 and local-embedding cosine ranks fused by reciprocal rank (k=60); lexical-only without a local model. An empty q lists recent memories.");
            parameters.push(json!({"name":"owner","in":"query","required":false,
                "description":"Filter by an exact agent id, or shared for notes without an owner; omitted includes all owners.",
                "schema":{"type":"string"}}));
            parameters.push(json!({"name": "q", "in": "query", "required": false,
                "description": "Search text; omit to list the most recently updated memories.",
                "schema": {"type": "string"}}));
            parameters.push(json!({"name": "kind", "in": "query", "required": false,
                "description": "Only memories of this kind.", "schema": reference("MemoryKind")}));
            parameters.push(json!({"name": "limit", "in": "query", "required": false,
                "description": "Maximum hits; defaults to 20, capped at 100.",
                "schema": {"type": "integer", "minimum": 1}}));
        }
        if route.path == "/v1/memory" && route.method == HttpMethod::Post {
            operation["description"] = json!("Create a note. Content, tags or values matching secret patterns (API keys, tokens, private keys, JWTs, password= pairs, Luhn-valid card numbers, unlocked vault values) are rejected with 400 secret_detected and never stored.");
        }
        if route.path == "/v1/vault/items" && route.method == HttpMethod::Get {
            operation["description"] = json!("Value-free item metadata (ids, allowed origins, policy, TOTP/passkey presence). Usernames, passwords, seeds and private keys are never returned.");
        }
        if route.path == "/v1/vault/items" && route.method == HttpMethod::Post {
            operation["description"] = json!("Add a login (password, optional username and authenticator) or an authenticator alone (`totp`: otpauth:// URI or base32 key). Values only travel to the daemon, are masked in every later output before the save, are saved encrypted and audited (`item_add`, attributed to the caller: `device:<id>` or `local`); the response is value-free metadata. With a keyring backend the first item creates the vault, unless biometric unlock is on (then a person sets it up first: 403 biometric_required). 400 invalid_item, invalid_origin, invalid_totp, empty_item; 403 biometric_required; 409 item_exists, vault_unavailable; 500 item_added_audit_incomplete (saved).");
        }
        if route.path == "/v1/vault/items/{id}" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Set or clear (`default`) the item's access-policy override; the change is saved encrypted and audited, attributed to the caller (`device:<id>` or `local`). 404 unknown_item, 409 vault_unavailable; 500 policy_applied_audit_incomplete (saved).");
        }
        if route.path == "/v1/vault/items/{id}" && route.method == HttpMethod::Delete {
            operation["description"] = json!("Delete the item; saved encrypted and audited (`item_delete`, attributed to the caller). 404 unknown_item, 409 vault_unavailable; 500 item_deleted_audit_incomplete (deleted).");
        }
        if route.path == "/v1/vault" {
            operation["description"] = json!("Vault state (`uninitialized`, `locked`, `unlocked`, `unavailable`), the unlock method in effect (`vault.unlock = auto`: an existing file keeps its method; a new vault uses this OS's keyring when reachable, else a passphrase) and why it is locked or unavailable. Never values.");
        }
        if route.path == "/v1/vault/setup" {
            operation["description"] = json!("Create the vault file. Keyring backends take no body passphrase and keep a random wrapping key in the OS keyring; a passphrase vault needs `passphrase` (8+ characters), which is never stored or returned. With biometric unlock on, a keyring vault is created only from the local socket with `biometric: true` and a passing check. Every attempt that reaches the vault is audited (`setup`: `completed` or `failed` with the code), attributed to the caller (`device:<id>` or `local`). 400 passphrase_required, passphrase_not_used, weak_passphrase; 403 biometric_required, biometric_failed; 409 vault_already_set_up; 500 vault_set_up_audit_incomplete (set up and unlocked); 503 keyring_unavailable, vault_unsupported.");
        }
        if route.path == "/v1/vault/unlock" {
            operation["description"] = json!("Decrypt the vault: a keyring vault re-reads its key from the OS keyring; a passphrase vault needs `passphrase`. Already unlocked is a no-op. Audited (`unlock`: `unlocked`, `denied` with wrong_passphrase, or `failed` with the code), attributed to the caller; an unlock whose record cannot be written does not happen (500 audit_unavailable). Each caller gets 5 wrong passphrases, then waits 30 s, doubling per further wrong one up to 15 min; a success resets it. Paired devices together get 20 wrong passphrases per 15 minutes; the Unix socket is exempt from that shared limit. At most one attempt per caller, and two across paired devices, run at once. Throttled attempts get 429 too_many_attempts with `Retry-After` seconds, before any key derivation, and are not audited. 400 passphrase_required, passphrase_not_used, wrong_passphrase; 409 vault_not_set_up; 503 keyring_unavailable, vault_unsupported.");
            operation["responses"]["429"] = json!({"description":"Too many wrong passphrases; retry after the `Retry-After` header's seconds", "headers":{"Retry-After":{"schema":{"type":"integer"}}}, "content":{"application/json":{"schema":reference("ApiError"),"example":{"code":"too_many_attempts","message":"too many attempts"}}}});
        }
        if route.path == "/v1/vault/lock" {
            operation["description"] = json!("Drop the decrypted vault from daemon memory; vault tools stop working until it is unlocked again. Audited (`lock`) after locking, attributed to the caller; 500 vault_locked_audit_incomplete means it is locked but unrecorded. Output masking of values seen while unlocked stays in place until the daemon restarts.");
        }
        if route.path == "/v1/vault/settings" && route.method == HttpMethod::Get {
            operation["description"] = json!("Password settings: auto-lock idle time, biometric unlock (with whether this computer supports it), autofill rules, default access policy, the fixed incognito exclusion and recovery-key state. Never values.");
        }
        if route.path == "/v1/vault/settings" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Change password settings; absent fields keep their value. Saved to the config file and applied at once; every change is audited (`settings_update`, with the changed key names only). Tightening is open to every vault caller; loosening (longer auto-lock, biometric unlock off, autofill or auto-submit on, insecure pages allowed, fewer excluded domains, a less strict default policy) needs the local socket or a person's device. Turning biometric unlock on needs the local socket, a keyring vault and a passing biometric check. 400 invalid_value, invalid_domain, incognito_always_disabled; 403 local_access_required, person_access_required, biometric_failed; 409 biometric_unavailable; 500 settings_saved_audit_incomplete (saved).");
        }
        if route.path == "/v1/vault/biometrics" {
            operation["description"] = json!("Whether this computer can verify a person biometrically (fprintd on Linux, Touch ID on macOS) without prompting; `reason` says why not.");
        }
        if route.path == "/v1/vault/export" {
            operation["description"] = json!("Every saved sign-in and authenticator as CSV (`name,url,username,password,otpauth`, one row per allowed origin; passkeys cannot be exported and are counted in `skipped`; model keys, channel and MCP secrets are never included). Local socket only, `confirm: true`, and fresh authentication: the passphrase in the body (passphrase vaults), `biometric: true`, or a person's unlock in the last 5 minutes; a keyring vault without biometric unlock needs only the confirmation. Wrong passphrases are throttled like unlock. Refusals are audited (`export`: `denied`); an export is audited `allowed`, then `exported`. Served with Cache-Control: no-store. 400 confirmation_required, wrong_passphrase, passphrase_not_used; 403 local_access_required, biometric_failed; 409 vault_unavailable, reauth_required, biometric_unavailable; 429 too_many_attempts; 500 audit_unavailable.");
        }
        if route.path == "/v1/vault/clear" {
            operation["description"] = json!("Delete every saved sign-in, authenticator and passkey; model keys, channel and MCP secrets stay. Same gate and codes as export (local socket, `confirm: true`, fresh authentication). Audited (`clear`: `allowed`, then `cleared` with the count; refusals `denied`). 500 vault_cleared_audit_incomplete (cleared).");
        }
        if route.path == "/v1/vault/recovery-key" {
            operation["description"] = json!("Generate a recovery key and return it once; any earlier key stops working. It is stored nowhere: only the vault key sealed with it is kept next to the vault file, and it is masked in later output. Same gate and codes as export (local socket, `confirm: true`, fresh authentication). Audited (`recovery_key`: `allowed`, then `generated`, both before the old key is replaced). Served with Cache-Control: no-store.");
        }
        if route.path == "/v1/vault/recover" {
            operation["description"] = json!("Open a locked vault with its recovery key and give it a new unlock key: `passphrase` (8+ characters) for a passphrase vault, the OS keyring's key (created when missing) for a keyring vault. The saved recovery key keeps working. Local socket only; wrong keys are throttled like passphrases. Audited (`recover`: `recovered`, `denied` with wrong_recovery_key, or `failed` with the code). 400 passphrase_required, passphrase_not_used, weak_passphrase; 403 local_access_required, wrong_recovery_key; 409 vault_already_unlocked, vault_not_set_up, recovery_not_configured; 429 too_many_attempts; 503 vault_unsupported, keyring_unavailable; 500 vault_recovered_audit_incomplete (recovered).");
        }
        if route.path == "/v1/sync/setup" {
            operation["description"] = json!("Create a sync account for this device: derive the key from the passphrase in the body (typed in the app, memory only, needs local access) or from sync.passphrase_cmd, create the account collection on the WebDAV share or directory and return the pairing code for the second device. The body may be empty. 409 sync_not_configured, sync_already_set_up, sync_passphrase_unavailable, sync_password_unavailable; 403 local_access_required when a secret is sent without local access; 502 sync_backend_unavailable, sync_unauthorized.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP when the body carries a passphrase or WebDAV password.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/sync/pair" {
            operation["description"] = json!("Join the account of a pairing code with the same passphrase, typed in the body (needs local access) or read from sync.passphrase_cmd. 400 invalid_pairing_code, sync_wrong_passphrase; 403 local_access_required when a secret is sent without local access; 409 as setup; 502 sync_backend_unavailable, sync_unauthorized.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof required on verified loopback TCP when the body carries a passphrase or WebDAV password.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/sync/run" {
            operation["description"] = json!("Sync now: seal local changes to the live policy, pinned tabs, vault items and tagged memories, pull and merge remote records (docs/specs/sync.md conflict rules), push winners with conditional writes. Tampered records are counted in `rejected` and never applied.");
        }
        if route.path == "/v1/vault/audit" {
            operation["description"] = json!("Vault audit records (who, what, item, origin, session/tool-call id, time, result), oldest first; never values.");
            parameters.push(json!({"name": "since", "in": "query", "required": false,
                "description": "Only records at or after this RFC 3339 time.",
                "schema": {"type": "string", "format": "date-time"}}));
            parameters.push(json!({"name": "item", "in": "query", "required": false,
                "description": "Only records for this item id.", "schema": {"type": "string"}}));
            parameters.push(json!({"name": "session", "in": "query", "required": false,
                "description": "Only records for this session (task) id.",
                "schema": reference("SessionId")}));
            parameters.push(json!({"name": "limit", "in": "query", "required": false,
                "description": "Newest N matches; defaults to 200, capped at 1000.",
                "schema": {"type": "integer", "minimum": 1}}));
        }
        if route.path.starts_with("/v1/connections") {
            operation["description"] = json!("Admin-only sign-in metadata. Cookie/header values and identity HMACs are never returned. Missing rows return connection_not_found; grant/policy refusal returns connection_denied; unavailable browser returns connection_browser_unavailable. Same-site URL validation returns connection_cross_site.");
        }
        if route.path == "/v1/connections" {
            parameters.push(
                json!({"name":"agent","in":"query","required":false,"schema":{"type":"string"},
                "description":"Exclude rows explicitly denied to this agent."}),
            );
            parameters.push(json!({"name":"status","in":"query","required":false,"schema":reference("ConnectionStatus")}));
        }
        if route.path == "/v1/connections/audit" {
            for name in ["connection", "agent"] {
                parameters.push(
                    json!({"name":name,"in":"query","required":false,"schema":{"type":"string"}}),
                );
            }
            parameters.push(json!({"name":"action","in":"query","required":false,"schema":reference("ConnectionAuditAction")}));
            parameters.push(json!({"name":"limit","in":"query","required":false,"schema":{"type":"integer","minimum":1,"maximum":1000,"default":100}}));
        }
        if route.path == "/v1/connections/{id}/relogin" {
            operation["description"] = json!("Start the existing login flow in the supplied shared session, or the chief agent's home session. Silent Vault completion returns signed_in. OTP, device approval and login continuations return needs_you with the matching existing ticket; other actionable states use a Finish signing in takeover ticket.");
        }
        if route.path.starts_with("/v1/skills/{name}") {
            parameters.push(json!({"name": "name", "in": "path", "required": true,
                "description": "Skill name.", "schema": {"type": "string"}}));
            parameters.push(json!({"name": "session", "in": "query", "required": false,
                "description": "Session whose workspace supplies project skills.",
                "schema": reference("SessionId")}));
        }
        if route.path == "/v1/skills" && route.method == HttpMethod::Get {
            operation["description"] = json!("Skills visible to a session. With q this is the data source of the `/` mention picker: name-prefix matches first, then name, then description matches. Invalid skill directories are listed under problems instead of failing the request.");
            parameters.push(json!({"name": "q", "in": "query", "required": false,
                "description": "Case-insensitive text matched against name and description.",
                "schema": {"type": "string"}}));
            parameters.push(json!({"name": "session", "in": "query", "required": false,
                "description": "Session whose workspace supplies project skills; without it only the user library is listed.",
                "schema": reference("SessionId")}));
            parameters.push(json!({"name": "enabled", "in": "query", "required": false,
                "description": "Only enabled (true) or only disabled (false) skills.",
                "schema": {"type": "boolean"}}));
        }
        if route.path == "/v1/skills" && route.method == HttpMethod::Post {
            operation["description"] = json!("Create a user skill from a name, description, Markdown body and optional text resources; the content is validated like any SKILL.md. 409 `exists` if the name is taken.");
        }
        if route.path == "/v1/skills/{name}" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Edit the description or body, or switch the skill on or off. Project skills are read-only (403 `read_only`).");
        }
        if !parameters.is_empty() {
            operation["parameters"] = Value::Array(parameters);
        }
        let path = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| json!({}));
        path[route.method.as_str().to_ascii_lowercase()] = operation;
    }
    json!({
        "openapi": "3.1.0",
        "jsonSchemaDialect": "https://json-schema.org/draft/2020-12/schema",
        "info": {"title": "openside API", "version": "v1",
            "description": "While Remote Control is off (remote.enabled = false; POST /v1/remote/disable), every TCP request that does not come directly from this computer, public routes and the web UI included, is refused with 403 remote_disabled. The Unix socket and loopback requests without proxy or tunnel headers are served as usual."},
        "paths": paths,
        "components": {
            "schemas": schemas(),
            "securitySchemes": {"capabilityToken": {"type": "http", "scheme": "bearer",
                "description": "Device token from POST /v1/devices/pair or `openside token create --scope <scope>`. Each operation's x-required-capability names the scope it needs."}}
        }
    })
}
