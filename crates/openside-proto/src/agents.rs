use crate::{MessageId, ModelAccountRef, PolicyMode, SessionId, SessionStatus};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The shipped avatar palette; arbitrary CSS colors are not accepted from profiles.
pub const ACCENT_COLORS: [&str; 8] = [
    "#2f6fed", "#a34dff", "#00a240", "#f0b429", "#f97316", "#ef476f", "#00a6a6", "#888888",
];

/// Immutable role that controls which team tools the daemon exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// The unique `chief` identity; delegates but cannot be retired.
    Chief,
    /// A durable specialist whose work is assigned through the chief.
    Teammate,
}
/// Registry lifecycle, distinct from the home conversation's running/waiting state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// May receive new conversations and messages.
    Active,
    /// History and channel ownership remain, but new work is refused.
    Retired,
}
/// Durable handoff state used for deduplication and terminal receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    /// Holds its deduplication key until completion or cancellation.
    Open,
    /// The assignee delivered an outcome.
    Done,
    /// The assignee could not meet the acceptance criteria.
    Failed,
    /// Explicitly ended without claiming successful execution.
    Cancelled,
}
/// Why the chief is handing work to one assignee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandoffKind {
    /// One self-contained job, not a lasting transfer.
    Task,
    /// Onboarding or a lasting responsibility transfer.
    Ownership,
    /// Independent review; the reviewer cannot be the subject's author.
    Review,
}
/// Person-authored response to a proposed hire, not authority supplied by a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HireDecision {
    /// Approve the authoritative draft with its permitted edits.
    Hire,
    /// Decline this topic for the 30-day cooldown.
    NotNow,
    /// Suppress future proposals for the same topic permanently.
    Never,
}
pub use openside_core::MessageAuthorKind;

/// Bounded durable instructions; task-specific instructions belong in handoff packets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentPersona {
    /// Specific non-generic role, 1–60 Unicode scalar characters.
    pub role: String,
    /// At most 100 characters; never truncated when building the prompt.
    #[serde(default)]
    pub voice: String,
    /// At most six nonempty lines of 100 characters; teammates require at least one.
    #[serde(default)]
    pub responsibilities: Vec<String>,
    /// At most six nonempty lines of 100 characters; policy remains the hard boundary.
    #[serde(default)]
    pub boundaries: Vec<String>,
}
/// One exact channel identity, without credentials or a duplicate settings object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentChannelBinding {
    /// One of telegram, slack, discord.
    pub kind: String,
    /// Opaque account id within this service, not its editable label.
    pub account_id: String,
}
/// Durable roster record; a conversation tag never changes its home identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Agent {
    /// Opaque stable identity; the chief always uses `chief`.
    pub id: String,
    /// Cannot be patched or changed during retirement/restoration.
    pub kind: AgentKind,
    /// Unique display name, 1–24 Unicode scalar characters.
    pub name: String,
    /// Display override of 1–3 characters, derived from the name when omitted at hire.
    pub initials: String,
    /// One exact value from ACCENT_COLORS.
    pub color: String,
    /// Validated so its rendered prompt fits the 1,500-character hard cap.
    pub persona: AgentPersona,
    /// Null inherits enabled skills; a list is an allowlist, not installed-skill ownership.
    pub skills: Option<Vec<String>>,
    /// Null inherits the person's default model.
    pub model: Option<String>,
    /// Null inherits; a nonempty list restricts and orders the person's provider pool.
    pub account_pool: Option<Vec<ModelAccountRef>>,
    /// Null removes the explicit profile binding, without rewriting historical threads.
    pub channel: Option<AgentChannelBinding>,
    /// Ceiling intersected with the chief, session and current daemon policy.
    pub mode: PolicyMode,
    /// Chosen once before the home event is written, enabling crash-safe repair.
    pub home_session_id: SessionId,
    /// Retirement is not deletion; identity and historical context survive.
    pub status: AgentStatus,
    /// Sidebar visibility only; hiding does not pause work.
    pub hidden: bool,
    /// Person-selected roster priority, independent of legacy session pinning.
    pub pinned: bool,
    /// Registry creation time, not the most recent activity time.
    pub created_at: DateTime<Utc>,
    /// Null while active; preserved lifecycle metadata for retired records.
    pub retired_at: Option<DateTime<Utc>>,
    /// Monotonic revision used to reopen warm runtimes after profile edits.
    pub persona_version: u64,
}
/// Roster projection with a device-local unread cursor rather than server receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSummary {
    /// Full profile flattened for a single roster-row wire object.
    #[serde(flatten)]
    pub agent: Agent,
    /// Home conversation state, not the registry's active/retired lifecycle.
    pub session_status: SessionStatus,
    /// Current work phrase, separate from the most recent message preview.
    pub activity: String,
    /// Whether the home conversation currently requires human attention.
    pub needs_you: bool,
    /// Home SSE sequence; compare with the device's locally stored seen cursor.
    pub last_seq: u64,
    /// Bounded latest-message preview; this field is not a timestamp.
    pub last_activity: String,
    /// Latest non-metadata event timestamp for stable roster ordering.
    pub last_message_at: DateTime<Utc>,
}
/// Roster ordered with the chief first, then by recent home-thread activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentList {
    /// Includes retired and hidden profiles so clients can present History and management.
    pub agents: Vec<AgentSummary>,
}
/// Authoritative hire payload shared by manual creation and approval tickets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HireDraft {
    /// Validated against all existing profiles, including retired teammates.
    pub name: String,
    /// Null derives an initial from the supplied name.
    pub initials: Option<String>,
    /// Null selects a stable id-derived value from ACCENT_COLORS.
    pub color: Option<String>,
    /// Durable role rules, not the initial assignment's conversation history.
    pub persona: AgentPersona,
    /// Null inherits enabled skills; transfers add grants to explicit allowlists.
    pub skills: Option<Vec<String>>,
    /// Null inherits the current default; it does not embed a fallback model.
    pub model: Option<String>,
    /// Null inherits provider membership; an empty list is rejected.
    pub account_pool: Option<Vec<ModelAccountRef>>,
    /// Validated against both channel settings and existing profile ownership.
    pub channel: Option<AgentChannelBinding>,
    /// Null inherits the current default, bounded by the chief's ceiling.
    pub mode: Option<PolicyMode>,
    /// Explicit ownership transfers recorded in the resumable hire saga.
    #[serde(default)]
    pub routine_ids: Vec<String>,
    /// Explicit memory transfers; unrelated notes remain with their current owners.
    #[serde(default)]
    pub memory_ids: Vec<String>,
    /// Initial handoff context; never injected as permanent persona rules.
    #[serde(default)]
    pub history_summary: String,
    /// Stable topic key for proposal cooldowns, absent for a manual hire without a topic.
    pub proposal_key: Option<String>,
}
/// Manual creation uses the same validation and transfer payload as approved proposals.
pub type CreateAgentRequest = HireDraft;

// The outer option distinguishes an absent patch from an explicit null override.
fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}
/// Partial profile edit; nullable overrides distinguish omission from explicit inheritance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentRequest {
    /// Renaming preserves stable identity and the home conversation.
    pub name: Option<String>,
    /// Explicit override; omission preserves existing initials when renaming.
    pub initials: Option<String>,
    /// Only the shipped avatar palette is accepted.
    pub color: Option<String>,
    /// Replaces the complete persona, not individual responsibility lines.
    pub persona: Option<AgentPersona>,
    /// Explicit null returns to enabled defaults; changes require Admin and a person.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub skills: Option<Option<Vec<String>>>,
    /// Explicit null returns to the default model; omission leaves the override unchanged.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub model: Option<Option<String>>,
    /// Explicit null inherits, a nonempty list restricts, omission preserves membership.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub account_pool: Option<Option<Vec<ModelAccountRef>>>,
    /// Explicit null clears the binding, never historical session tags.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub channel: Option<Option<AgentChannelBinding>>,
    /// Widening additionally needs Admin/person authority and cannot exceed policy.
    pub mode: Option<PolicyMode>,
    /// Sidebar visibility only, without pausing scheduled work.
    pub hidden: Option<bool>,
    /// Changes roster priority, not conversation content or authority.
    pub pinned: Option<bool>,
}
/// Retirement keeps history and records ownership movement for resumable recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetireAgentRequest {
    /// New owner; omission selects the chief unless routines are explicitly paused.
    pub successor: Option<String>,
    /// Pause owned schedules instead of silently dropping them.
    #[serde(default)]
    pub pause_routines: bool,
    /// Undo uses recorded pre-hire owners rather than guessing a successor.
    #[serde(default)]
    pub revert_hire: bool,
}
/// Explicit cross-store ownership transaction, persisted as a recoverable saga.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferRequest {
    /// Exactly one active destination agent.
    pub to: String,
    /// Only the selected schedules change ownership.
    #[serde(default)]
    pub routine_ids: Vec<String>,
    /// Only the selected notes change ownership.
    #[serde(default)]
    pub memory_ids: Vec<String>,
    /// Existing responsibility lines to remove from the source and add to the destination.
    #[serde(default)]
    pub responsibilities: Vec<String>,
    /// Allowlist grants; installed skill files never move between agents.
    #[serde(default)]
    pub skills: Vec<String>,
}
/// Self-contained work context; assignees do not inherit the chief's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffPacket {
    /// Required nonblank outcome the assignee should achieve.
    pub goal: String,
    /// Facts and prior results needed without reading another thread.
    pub context: String,
    /// Required nonblank evidence that makes completion observable.
    pub acceptance: String,
    /// Boundaries for this assignment; cannot widen daemon policy.
    pub constraints: String,
}
/// Durable one-assignee handoff, independent of its timeline presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Handoff {
    /// Stable identity used for receipts and report delivery deduplication.
    pub id: String,
    /// Chief identity in the first version's chief-mediated delegation model.
    pub from_agent: String,
    /// Exactly one active assignee when created.
    pub to_agent: String,
    /// Review additionally requires an independently authored subject.
    pub kind: HandoffKind,
    /// Full packet, not reconstructed from a rendered card.
    pub packet: HandoffPacket,
    /// Unique among open handoffs, preventing concurrent duplicate assignments.
    pub dedupe_key: String,
    /// Required review subject when independent-review checks apply.
    pub subject_handoff_id: Option<String>,
    /// Terminal transitions release the open deduplication key.
    pub status: HandoffStatus,
    /// Queued assignee message used to correlate settling with report delivery.
    pub message_id: MessageId,
    /// Assignment creation time, preserved on status transitions.
    pub created_at: DateTime<Utc>,
    /// Null until the handoff reaches a terminal state.
    pub ended_at: Option<DateTime<Utc>>,
}
/// History for profile and handoff navigation without polling model execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HandoffList {
    /// Durable handoff records rather than transient progress-only events.
    pub handoffs: Vec<Handoff>,
}
/// Person-initiated assignment still routes through the chief as sender.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateHandoffRequest {
    /// Validated self-contained assignment; goal and acceptance must be nonblank.
    pub packet: HandoffPacket,
    /// Selects task, ownership or independent-review rules.
    pub kind: HandoffKind,
    /// Existing open ownership of this key is a conflict, not another dispatch.
    pub dedupe_key: String,
    /// Review subject; never authorizes the subject's author to review itself.
    pub subject_handoff_id: Option<String>,
}

/// A topic the person explicitly turned off; clearing it allows new proposals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SuppressedProposal {
    /// Stable proposal identity used by the clear endpoint.
    pub key: String,
    /// Human-readable role captured at the decision; absent on legacy records.
    pub label: Option<String>,
    /// Persisted suppression decision, currently `never`.
    pub status: String,
    /// Time the person made the decision.
    pub decided_at: DateTime<Utc>,
}

/// Bare-array response for topics the person turned off.
pub type SuppressedProposalList = Vec<SuppressedProposal>;
