use chrono::{DateTime, Utc};
pub use openside_core::ModelAccountRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Local credential readiness, never a guarantee of upstream model entitlement.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelCatalog {
    /// Provider groups sorted by identifier, with deduplicated model ids.
    pub providers: Vec<ModelProvider>,
    /// Current default for new sessions and unoverridden Team homes on their next idle turn.
    pub default_model: Option<String>,
    /// Fast chief front-agent model; null uses the reasoning model.
    pub fast_model: Option<String>,
    /// Running model fallback chain, independent of credential-source order.
    pub fallback_models: Vec<String>,
    /// Maximum metadata cache lifetime; managed writes invalidate it immediately.
    pub cached_for_ms: u32,
    /// An input or output metadata limit omitted entries.
    pub truncated: bool,
}

/// Chat/tool-capable candidates for one Pydantic AI provider namespace.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelProvider {
    /// SDK provider namespace, not an upstream account identifier.
    pub provider: String,
    /// Configured, curated and account-reported candidates, deduplicated by id.
    pub models: Vec<CatalogModel>,
    /// Person-built ordered account membership; empty explicitly disables routing.
    pub pool: ProviderPool,
}

/// Curated role hint; any available model may serve either role.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Low-latency chief replies and Task dispatch.
    Fast,
    /// Task execution and ordinary session work.
    Reasoning,
}

/// Curated task-routing tier, not an entitlement or performance guarantee.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    /// Quick lookups and simple work.
    Fast,
    /// General multi-step work; also the neutral tier for uncataloged configured ids.
    #[default]
    Reasoning,
    /// The family's highest-capability models for hard work.
    Frontier,
}

/// One candidate and the credential source that would serve a new sidecar.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogModel {
    /// Explicit provider:model selector; image-only models are not chat defaults.
    pub id: String,
    /// Model-name portion, never an account label or private identity.
    pub label: String,
    /// Curated family recommendation, not a routing restriction.
    pub recommended_for: Option<ModelRole>,
    /// False for an explicitly configured id outside the current curated generations.
    #[serde(default)]
    pub in_catalog: bool,
    /// Task-routing guidance, independent of credential readiness.
    #[serde(default)]
    pub tier: ModelTier,
    /// Short curated guidance; unknown configured models have no capability claim.
    #[serde(default)]
    pub strengths: String,
    /// Local credential readiness under source priority and broker fail-close.
    pub available: bool,
    /// Credential-value-free readiness or refusal explanation.
    pub reason: String,
    /// First usable source, or null when unavailable.
    pub source: Option<String>,
    /// Opaque source-local account reference, or null when unavailable.
    pub account_id: Option<String>,
    /// Credential-free candidates, including blocked/unavailable choices.
    pub accounts: Vec<ModelAccount>,
}

/// A candidate account without token, email or upstream identity.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelAccount {
    /// Stable credential plugin name, distinct from provider namespace.
    pub source: String,
    /// Opaque source-local reference; no upstream account id.
    pub account_id: String,
    /// Person-assigned display label or an opaque source ordinal.
    #[serde(default)]
    pub label: String,
    /// Operator enablement, separate from credential readiness.
    #[serde(default)]
    pub enabled: bool,
    /// Local source readiness before pool membership/model blocks.
    #[serde(default)]
    pub ready: bool,
    /// Explicit provider-pool membership.
    #[serde(default)]
    pub in_pool: bool,
    /// Per-account measured windows; unknown usage remains null.
    #[serde(default)]
    pub usage_windows: Vec<crate::AccountUsageWindow>,
    /// Latest applicable block expiry in Unix milliseconds, or unknown.
    #[serde(default)]
    pub blocked_until_ms: Option<f64>,
    /// Readiness after applying higher-priority fail-close barriers.
    pub available: bool,
    /// Sanitized refusal or readiness explanation, never raw CLI output.
    pub reason: String,
}

/// Persist available catalog ids for new sessions and the next idle turn of Team homes.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UpdateModelsRequest {
    /// Available provider-qualified catalog id; must not be a fallback duplicate.
    pub default_model: String,
    /// Fast chief front-agent model; null uses the reasoning model.
    #[serde(default)]
    pub fast_model: Option<String>,
    /// At most 16 distinct available catalog ids in fallback order.
    pub fallback_models: Vec<String>,
}

/// Result of a dedicated model-selection write, not raw config PUT semantics.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    /// The newly running default, after successful private-file persistence.
    pub default_model: String,
    /// Persisted fast model; null uses the reasoning model.
    pub fast_model: Option<String>,
    /// The newly running ordered fallback chain.
    pub fallback_models: Vec<String>,
    /// Always false: dedicated selection applies without restarting the daemon.
    pub restart_required: bool,
    /// new_sessions_and_home_next_turn: ordinary sessions retain stored models; homes reopen idle.
    pub effective: String,
}

/// The only provider-key metadata allowed in public responses.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeyInfo {
    /// Managed API provider: anthropic, openai or google.
    pub provider: String,
    /// Durable opaque key account id, not upstream identity.
    pub key_id: String,
    /// Person-assigned display label.
    pub label: String,
    /// Operator enablement, independent of quota/authentication status.
    pub enabled: bool,
    /// Remaining measured usage protected from automatic selection.
    pub reserve_percent: u8,
    /// Exactly four trailing characters, never the complete key.
    pub last_four: String,
    /// UTC creation/replacement time, encoded as RFC 3339.
    pub created_at: DateTime<Utc>,
}

/// Administrator-visible inventory; no credential values.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeyList {
    /// Metadata only; unavailable Vaults fail rather than returning fake entries.
    pub keys: Vec<ProviderKeyInfo>,
}

/// Secret input only: intentionally no Debug or Serialize implementation.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetProviderKeyRequest {
    /// 8–8192 ASCII graphic bytes; never echoed or written to config.
    pub key: String,
    /// Optional replacement label; absent preserves the existing account label.
    pub label: Option<String>,
}

/// Secret creation input; never Debug/Serialize or error-quoted.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProviderKeyRequest {
    /// Non-control person-assigned label,1–80 characters.
    pub label: String,
    /// Provider key from the private input body, never argv or response.
    pub key: String,
}

/// Account metadata change, preserving the encrypted key.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderKeyRequest {
    /// New person-assigned label; missing keeps the current label.
    pub label: Option<String>,
    /// New operator enablement; missing keeps the current setting.
    pub enabled: Option<bool>,
    /// Protected remaining percentage0–99; missing keeps the current threshold.
    pub reserve_percent: Option<u8>,
}

/// Ordered trust boundary for one SDK provider namespace.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderPool {
    /// Exact SDK namespace; no implicit OpenAI/Codex aliasing.
    pub provider: String,
    /// Explicit person-built membership/order; empty disables routing.
    pub accounts: Vec<ModelAccountRef>,
}

/// Public provider-pool inventory, without credential material.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderPoolList {
    /// Provider-sorted trust boundaries.
    pub pools: Vec<ProviderPool>,
}

/// Replace the provider's explicit trust boundary; new sources are never implicit.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderPoolRequest {
    /// Known compatible opaque accounts, ordered without duplicates.
    pub accounts: Vec<ModelAccountRef>,
}

/// Administrator-visible external CLI account, never an OAuth value.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionAccount {
    /// CLI source: claude-code or codex-cli.
    pub source: String,
    /// Exact SDK provider namespace.
    pub provider: String,
    /// Durable source-local account id.
    pub account_id: String,
    /// Person-assigned display label.
    pub label: String,
    /// True only for an openside-managed isolated CLI home.
    pub managed: bool,
    /// CLI executable discovery, without running login.
    pub installed: bool,
    /// Detected usable external login, without public identity/token.
    pub logged_in: bool,
    /// Operator-controlled enablement.
    pub enabled: bool,
    /// Current local credential and account-policy readiness.
    pub available: bool,
    /// Sanitized status/recovery guidance.
    pub reason: String,
    /// Exact external home/environment command, never credentials.
    pub login_command: String,
    /// Registry creation time, RFC3339 UTC.
    pub created_at: DateTime<Utc>,
    /// Remaining measured usage protected from automatic selection.
    pub reserve_percent: u8,
}

/// Installed CLI source with independently managed labelled accounts.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionSource {
    /// CLI plugin name.
    pub source: String,
    /// Exact SDK namespace.
    pub provider: String,
    /// Executable discovery, not an OAuth request.
    pub installed: bool,
    /// Includes the read-only default-home account unless hidden by the person.
    pub accounts: Vec<SubscriptionAccount>,
}

/// Administrator-visible subscription source groups.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionList {
    /// Sources in stable CLI-name order.
    pub sources: Vec<SubscriptionSource>,
}

/// Create an isolated private CLI home; the person runs its external login command.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CreateSubscriptionAccountRequest {
    /// Non-control person-assigned label,1–80 characters.
    pub label: String,
}

/// Mutate registry metadata without modifying a default user's credential home.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UpdateSubscriptionAccountRequest {
    /// New label; missing keeps the current value.
    pub label: Option<String>,
    /// New enablement; missing keeps the current value.
    pub enabled: Option<bool>,
    /// Protected remaining percentage0–99; missing keeps the current threshold.
    pub reserve_percent: Option<u8>,
}

/// Select/clear a preferred sticky account within the already authorized effective pool.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionAccountRequest {
    /// Null clears preference; never adds or enables an account.
    pub account: Option<ModelAccountRef>,
}

/// Account preference result, applied at the next request boundary.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionAccountSelection {
    /// Durable session whose preference changed.
    pub session_id: crate::SessionId,
    /// Provider namespace governed by the preference.
    pub provider: String,
    /// Preferred authorized account, or cleared.
    pub account: Option<ModelAccountRef>,
    /// Whether a person/request explicitly set this preference.
    pub explicit: bool,
    /// Always next_request; no daemon or sidecar restart required.
    pub effective: String,
}
