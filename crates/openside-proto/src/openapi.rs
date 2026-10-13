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
        Session,
        SessionList,
        CreateSessionRequest,
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
        MyDevice,
        AccessRequest,
        AccessRequestList,
        RequestAccessRequest,
        PairingTransport,
        RemoteStatus,
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
        UpdateSyncConfigRequest
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
        }
        if route.path == "/v1/sessions" && route.method == HttpMethod::Post {
            operation["description"] = json!("Create a session. browser_space \"isolated\" gives its agent tabs their own cookies and storage in the shared managed browser, disposed on archive or delete and lost when the browser restarts; it cannot be combined with incognito (400 invalid_browser_space).");
        }
        if route.path == "/v1/agents/{id}" && route.method == HttpMethod::Patch {
            operation["description"] = json!("Edit the profile for the next turn. Omitted nullable overrides are unchanged; explicit null inherits defaults. Permission widening and skills/model/account-pool/channel changes additionally require Admin and a person device (local socket exempt). Immutable identity and home fields cannot be patched.");
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
            operation["description"] = json!("Issue a one-time pairing code valid for 180 monotonic seconds. Requires the local socket, or verified loopback TCP with a socket-issued person_device admin token and current X-Openside-Local-Secret. Proxied/relay traffic is refused with 403 local_access_required; loopback alone never proves locality. Requested scopes cannot exceed caller grants (403 scope_ceiling_exceeded). The local operator fixes person_device or agent_client in the offer; redemption cannot change kind. Codes and local secrets are never audited.");
        }
        if route.path == "/v1/remote/local-access" {
            operation["description"] = json!("Local socket only: every call atomically replaces the one managed owner-bootstrap device/token, closes the predecessor's streams/polls, clears its push subscriptions, and audits rotation. A distinct persistent bootstrap marker survives boots; intentional CLI devices are never revoked by display name. Returns the per-boot locality secret and fresh person/admin token plus a loopback launch URL with credentials in its fragment. TCP/relay callers get 403 local_socket_required. Keep the proof only in sessionStorage; strip the fragment immediately and clear proof on sign-out, rejection or identity change.");
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
            operation["description"] = json!("Record how the guide ended (`completed` or `skipped`; `pending` is 400 invalid_value). Reopening the guide from Settings does not reset the flag.");
        }
        if route.path == "/v1/external-agents" {
            operation["description"] = json!("The agent CLIs openside can install its browser skill for: the `claude`, `codex` and `omp` presets, then any agent whose skills folder was added with `PUT /v1/external-agents/{id}` (stored as a `[skills.agents]` entry). `detected` means the executable is on the daemon's PATH or the agent's configuration directory exists; the skills directory follows the same rules as `openside skills install`.");
        }
        if route.path == "/v1/external-agents/{id}/browser-skill" {
            operation["description"] = json!("Install the bundled `openside-browser` skill into the agent's skills directory (what `openside skills install <agent>` does). The content is fixed and the directory comes from the preset or configuration, never from the request. 404 unknown_agent, 409 exists (retry with `force`), 400 skills_dir_unknown (add the folder with `PUT /v1/external-agents/{id}`).");
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
            operation["description"] = json!("The whole config.toml as JSON, with defaults filled in. PUT validates exactly like loading the file (400 invalid_config with the offending key) and writes it atomically with mode 0600; the credentials section is file-only. Changes apply when the daemon restarts, except the live policy (/v1/policy). Who may change what: a caller with local access (the Unix socket, or the computer's own launcher on loopback with X-Openside-Local-Secret) may change any key. A paired person device with admin scope anywhere else may change only the behaviour keys listed in `remote_editable`; changing any other key returns 403 local_access_required, writes nothing and is audited as `refused_remote`. Agent clients never write. Security-sensitive keys (sandbox, network allow-list, remote transports, credentials sources, executables, paths, trust lists) are never remote-editable.");
            parameters.push(json!({"name":"X-Openside-Local-Secret","in":"header","required":false,
                "description":"Per-boot launcher proof that makes a loopback TCP caller local; paired devices elsewhere omit it.",
                "schema":{"type":"string","minLength":64,"maxLength":64}}));
        }
        if route.path == "/v1/settings/models" {
            operation["description"] = if route.method == HttpMethod::Get {
                json!("Local model readiness grouped by provider, from configured ids, source accounts and curated chat/tool-capable models. Credential values, email and upstream account identities are never included. Metadata is cached for at most five seconds; key/source/Vault lifecycle writes invalidate it. Readiness is not an upstream entitlement guarantee.")
            } else {
                json!("Set the default and ordered fallback ids without raw TOML editing. Requires admin; every id must be available in the current catalog (400 model_unavailable), at most16 distinct fallbacks and no default duplicate (400 invalid_models). Atomically persists selection without daemon restart. Ordinary sessions retain stored models; Team homes use their Agent override or the current default and reopen on the next idle/real turn when effective selection changes. Running requests finish their captured model. Response effective:new_sessions_and_home_next_turn.")
            };
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
            operation["description"] = json!("Requires admin plus person_device (local socket exempt). POST creates one labelled account; PUT replaces its key, PATCH edits metadata, DELETE removes only that account. Values arrive only in secret input bodies; no key in response/config/log/argv/env. Subsequent request snapshots observe changes; missing account404, locked Vault409.");
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
            operation["description"] = json!("Admin-only external CLI account inventory/status; mutations also require person_device (local socket exempt). POST creates an isolated private home and exact external login command; no openside OAuth sign-in flow. PATCH edits one account; DELETE wipes only managed homes and hides defaults without writing/deleting user homes. Managed OAuth is canonical encrypted Vault state; daemon-only refresh. Refresh/access tokens never enter public replies.");
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
        "info": {"title": "openside API", "version": "v1"},
        "paths": paths,
        "components": {
            "schemas": schemas(),
            "securitySchemes": {"capabilityToken": {"type": "http", "scheme": "bearer",
                "description": "Device token from POST /v1/devices/pair or `openside token create --scope <scope>`. Each operation's x-required-capability names the scope it needs."}}
        }
    })
}
