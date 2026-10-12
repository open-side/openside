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
        path: "/v1/browser/tabs",
        request_schema: None,
        response_schema: Some("BrowserTabList"),
        scope: CapabilityScope::Read,
        success_status: 200,
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
        method: HttpMethod::Get,
        path: "/v1/settings",
        request_schema: None,
        response_schema: Some("Settings"),
        scope: CapabilityScope::Read,
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
        response_schema: Some("ImportReport"),
        scope: CapabilityScope::Admin,
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
        request_schema: None,
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
        path: "/v1/remote/status",
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
        Health,
        AccountsResponse,
        AccountSource,
        AccountStatus,
        AccountUsageWindow,
        BrowserTab,
        BrowserTabOwnership,
        BrowserTabList,
        Session,
        SessionList,
        CreateSessionRequest,
        Message,
        SubmitMessageRequest,
        EditMessageRequest,
        MoveMessageRequest,
        Ticket,
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
        PairingTransport,
        RemoteStatus,
        CloudflareStatus,
        RelayStatus,
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
        AttachmentUpload,
        Attachment,
        ContentKind,
        ArtifactSource,
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
        ImportReport
    );
    schemas
}

fn reference(name: &str) -> Value {
    json!({"$ref": format!("#/components/schemas/{name}")})
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
        if route.sse {
            parameters.push(
                json!({"name": "Last-Event-ID", "in": "header", "required": false,
                "description": "Last seen per-session sequence; only later events are returned.",
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
        if route.path == "/v1/sessions/{id}/subagents/{sid}/cancel" {
            operation["description"] = json!("Cancel a running or waiting child; its pending tickets are rejected and the parent's delegate call returns a cancelled result. Cancelling a finished child returns 409.");
        }
        if route.path == "/v1/sessions/{id}/subagents/{sid}/steer" {
            operation["description"] = json!("Queue steering text for the child's next model request; a waiting child receives it when it resumes.");
        }
        if route.path == "/v1/sessions" && route.method == HttpMethod::Get {
            parameters.push(json!({"name": "q", "in": "query", "required": false,
                "description": "Case-insensitive substring of the title or first message.",
                "schema": {"type": "string"}}));
            parameters.push(json!({"name": "archived", "in": "query", "required": false,
                "description": "List archived (true) or unarchived (false, default) sessions; pinned sessions come first.",
                "schema": {"type": "boolean", "default": false}}));
        }
        if route.path == "/v1/sessions" && route.method == HttpMethod::Post {
            operation["description"] = json!("Create a session. browser_space \"isolated\" gives its agent tabs their own cookies and storage in the shared managed browser, disposed on archive or delete and lost when the browser restarts; it cannot be combined with incognito (400 invalid_browser_space).");
        }
        if route.path == "/v1/sessions/{id}" && route.method == HttpMethod::Patch {
            operation["description"] =
                json!("Update title/pinned/archived as a durable session.updated event. Archiving an isolated-space session disposes its browser space.");
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
            operation["description"] = json!("Issue a one-time pairing code valid for 180 seconds. Only callable over the Unix socket (TCP callers get 403 local_socket_required). The operator fixes kind here: person_device for phones/browsers, agent_client for external CLI/MCP agents. Redemption cannot change it. Issuing, exchanging and revoking are written to audit.jsonl.");
        }
        if route.path == "/v1/pairing/exchange" {
            operation["description"] = json!("No bearer token: the one-time code is the credential. The token inherits the operator-selected kind from the offer; sending kind at redemption is rejected as an unknown field. 404 pairing_not_found for an unknown or already used code, 410 pairing_expired after 180 seconds. A valid request consumes the code on its first exchange attempt.");
        }
        if route.path == "/v1/onboarding" && route.method == HttpMethod::Get {
            operation["description"] = json!("State of the first-run guide, kept on the daemon so every device agrees: `pending` until someone finishes or skips it once.");
        }
        if route.path == "/v1/onboarding" && route.method == HttpMethod::Put {
            operation["description"] = json!("Record how the guide ended (`completed` or `skipped`; `pending` is 400 invalid_value). Reopening the guide from Settings does not reset the flag.");
        }
        if route.path == "/v1/external-agents" {
            operation["description"] = json!("The agent CLIs openside can install its browser skill for: the `claude`, `codex` and `omp` presets, then any `[skills.agents]` entry. `detected` means the executable is on the daemon's PATH or the agent's configuration directory exists; the skills directory follows the same rules as `openside skills install`.");
        }
        if route.path == "/v1/external-agents/{id}/browser-skill" {
            operation["description"] = json!("Install the bundled `openside-browser` skill into the agent's skills directory (what `openside skills install <agent>` does). The content is fixed and the directory comes from the preset or configuration, never from the request. 404 unknown_agent, 409 exists (retry with `force`), 400 skills_dir_unknown.");
        }
        if route.path == "/v1/import/profiles" {
            operation["description"] = json!("Chrome, Chromium and Firefox profiles found in their standard locations on the daemon's computer, with the data each has. Directory paths are never returned; `POST /v1/import` takes the profile id.");
        }
        if route.path == "/v1/import" {
            operation["description"] = json!("Import history, bookmarks and cookies of a discovered profile into the daemon's browser profile. `dry_run` reads and decrypts everything and reports counts without writing; a real run reads everything before it writes anything. Cookies encrypted with the OS keyring (`v11`) need the CLI (`openside import --keyring-password-cmd`). One import runs at a time. 400 unknown_profile; 409 import_in_progress, profile_in_use; 422 source_locked, source_unreadable, source_corrupt, source_unsupported, keyring_password_required; 500 import_failed; 503 browser_unavailable.");
        }
        if route.path == "/v1/devices/{id}" {
            operation["description"] = json!("Revoke a device token. The next request with it gets 401, its open SSE streams end, and its push subscriptions are deleted.");
        }
        if route.path == "/v1/push/presence" {
            operation["description"] = json!("focused=true suppresses pushes to that subscription for 60 seconds unless renewed; focused=false ends it. 404 unknown_subscription for another device's subscription.");
        }
        if route.path == "/v1/otp" {
            operation["description"] = json!("Forwarded SMS. 403 sender_not_allowed unless the sender is in otp.senders. A code extracted by an otp.sites pattern fills pending secret_input tickets of that exact origin, or is kept in memory for otp.ttl_secs for a step that opens later. Codes are never returned, logged or persisted.");
        }
        if route.path == "/v1/settings/config" {
            operation["description"] = json!("The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy).");
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
        if route.path == "/v1/sessions/{id}/repl" {
            operation["x-capability-also-granted-by"] = json!([CapabilityScope::Steer]);
        }
        if route.path == "/v1/policy" && route.method == HttpMethod::Put {
            operation["description"] = json!("Replace the live policy. Invalid globs or a sensitive allow rule return 400 invalid_policy and change nothing. Applies to the next tool call of every session and is persisted in <data_dir>/policy.json, which overrides [policy] in config.toml.");
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
            operation["description"] = json!("Add a login (password, optional username and authenticator) or an authenticator alone (`totp`: otpauth:// URI or base32 key). Values only travel to the daemon, are masked in every later output before the save, are saved encrypted and audited (`item_add`, attributed to the caller: `device:<id>` or `local`); the response is value-free metadata. With a keyring backend the first item creates the vault. 400 invalid_item, invalid_origin, invalid_totp, empty_item; 409 item_exists, vault_unavailable; 500 item_added_audit_incomplete (saved).");
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
            operation["description"] = json!("Create the vault file. Keyring backends take no body passphrase and keep a random wrapping key in the OS keyring; a passphrase vault needs `passphrase` (8+ characters), which is never stored or returned. Every attempt that reaches the vault is audited (`setup`: `completed` or `failed` with the code), attributed to the caller (`device:<id>` or `local`). 400 passphrase_required, passphrase_not_used, weak_passphrase; 409 vault_already_set_up; 500 vault_set_up_audit_incomplete (set up and unlocked); 503 keyring_unavailable, vault_unsupported.");
        }
        if route.path == "/v1/vault/unlock" {
            operation["description"] = json!("Decrypt the vault: a keyring vault re-reads its key from the OS keyring; a passphrase vault needs `passphrase`. Already unlocked is a no-op. Audited (`unlock`: `unlocked`, `denied` with wrong_passphrase, or `failed` with the code), attributed to the caller; an unlock whose record cannot be written does not happen (500 audit_unavailable). Each caller gets 5 wrong passphrases, then waits 30 s, doubling per further wrong one up to 15 min; a success resets it. Paired devices together get 20 wrong passphrases per 15 minutes; the Unix socket is exempt from that shared limit. At most one attempt per caller, and two across paired devices, run at once. Throttled attempts get 429 too_many_attempts with `Retry-After` seconds, before any key derivation, and are not audited. 400 passphrase_required, passphrase_not_used, wrong_passphrase; 409 vault_not_set_up; 503 keyring_unavailable, vault_unsupported.");
            operation["responses"]["429"] = json!({"description":"Too many wrong passphrases; retry after the `Retry-After` header's seconds", "headers":{"Retry-After":{"schema":{"type":"integer"}}}, "content":{"application/json":{"schema":reference("ApiError"),"example":{"code":"too_many_attempts","message":"too many attempts"}}}});
        }
        if route.path == "/v1/vault/lock" {
            operation["description"] = json!("Drop the decrypted vault from daemon memory; vault tools stop working until it is unlocked again. Audited (`lock`) after locking, attributed to the caller; 500 vault_locked_audit_incomplete means it is locked but unrecorded. Output masking of values seen while unlocked stays in place until the daemon restarts.");
        }
        if route.path == "/v1/sync/setup" {
            operation["description"] = json!("Create a sync account for this device: derive the key from sync.passphrase_cmd (kept in memory), create the account collection on the WebDAV share or directory and return the pairing code for the second device. 409 sync_not_configured, sync_already_set_up, sync_passphrase_unavailable, sync_password_unavailable; 502 sync_backend_unavailable, sync_unauthorized.");
        }
        if route.path == "/v1/sync/pair" {
            operation["description"] = json!("Join the account of a pairing code with the same passphrase. 400 invalid_pairing_code, sync_wrong_passphrase; 409 as setup; 502 sync_backend_unavailable, sync_unauthorized.");
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
        "info": {"title": "openside API", "version": "v1"},
        "paths": paths,
        "components": {
            "schemas": schemas(),
            "securitySchemes": {"capabilityToken": {"type": "http", "scheme": "bearer",
                "description": "Device token from POST /v1/devices/pair or `openside token create --scope <scope>`. Each operation's x-required-capability names the scope it needs."}}
        }
    })
}
