use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Credential-source diagnostics, never credential material.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AccountsResponse {
    /// Sources in configured priority order.
    pub sources: Vec<AccountSource>,
    /// Maximum time this daemon may reuse the result.
    pub cached_for_ms: u32,
    /// Some entries exceeded the documented metadata limits.
    pub truncated: bool,
}

/// One credential plugin's availability.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AccountSource {
    /// Stable plugin name.
    pub name: String,
    /// Plugin kind: omp-broker, env, command, codex-cli or builtin.
    pub kind: String,
    /// At least one account is usable (command results remain unverified).
    pub available: bool,
    /// Credential-value-free diagnostic.
    pub reason: String,
    /// Registered broker accounts or configured local provider accounts.
    pub accounts: Vec<AccountStatus>,
}

/// Public account metadata; ids are opaque and labels never contain raw email addresses.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AccountStatus {
    /// Opaque account reference, not an upstream account id.
    pub id: String,
    /// Safe display label.
    pub label: String,
    /// Provider identifier.
    pub provider: String,
    /// Configured model identifiers for this provider, not an invented broker catalog.
    pub models: Vec<String>,
    /// Broker-reported usage windows; empty when unavailable.
    pub usage_windows: Vec<AccountUsageWindow>,
    /// Latest active block expiry, as Unix milliseconds, or null.
    pub blocked_until_ms: Option<f64>,
}

/// One attributed usage window, without broker identifiers or metadata.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AccountUsageWindow {
    /// Safe ordinal window label.
    pub name: String,
    /// Usage percentage in the inclusive 0–100 range, or null when unknown.
    pub used_percent: Option<f64>,
    /// Unix milliseconds of the broker-reported reset, or null when unknown.
    pub reset_at_ms: Option<f64>,
}
