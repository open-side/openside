use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::PolicyMode;

/// Live daemon policy: `GET`/`PUT /v1/policy`. A `PUT` replaces the whole document and
/// applies to the next tool call of every session, including running ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicySettings {
    /// Mode for sessions created without an explicit mode.
    pub default_mode: PolicyMode,
    /// When non-empty, browser/fetch/vault/sensitive actions may only target these origin
    /// globs (`https://*.example.com`, `http://127.0.0.1:*`). Never grants anything.
    #[serde(default)]
    pub allow_origins: Vec<String>,
    /// Origin globs that are always denied for browser/fetch/vault/sensitive actions.
    #[serde(default)]
    pub deny_origins: Vec<String>,
    /// Agent-level rules beneath each session's rules. A matching `deny` anywhere wins;
    /// `sensitive` actions can be tightened to `deny` but never allowed.
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

/// One Allow/Ask/Deny rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    /// Outcome when the rule matches.
    pub effect: PolicyEffect,
    /// Capability the rule applies to.
    pub capability: PolicyCapability,
    /// Glob over the action target: `*` within a `/` segment, `**` across segments.
    pub pattern: String,
}

/// Rule or decision outcome, ordered `allow < ask < deny`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    /// Proceed without asking.
    Allow,
    /// Open a confirm ticket first.
    Ask,
    /// Block; approval cannot override it.
    Deny,
}

/// What an action does; targets are absolute paths for `fs_*`/`exec`, URLs or origins
/// otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCapability {
    /// Read a file or directory.
    FsRead,
    /// Write, delete or rename a file.
    FsWrite,
    /// Run a command (target: working directory).
    Exec,
    /// Drive a browser page.
    Browse,
    /// Network fetch outside the browser.
    Fetch,
    /// Use a saved credential through the vault.
    VaultUse,
    /// Payment, purchase, posting or sending a message.
    Sensitive,
    /// Access private imported records subject to an explicit session person grant.
    PrivacyRead,
}

/// `POST /v1/policy/test`: evaluate one action against the live policy without running it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyTestRequest {
    /// Path, URL or origin the action touches.
    pub target: String,
    /// Capability to evaluate.
    pub capability: PolicyCapability,
    /// Session mode to evaluate under; defaults to `default_mode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PolicyMode>,
    /// Approved workspace root for Guard mode (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// Result of a [`PolicyTestRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyDecision {
    /// Final effect.
    pub effect: PolicyEffect,
    /// `deny_rule`, `origin_denied`, `origin_not_allowed`, `rule`, `mode` or
    /// `sensitive_floor`.
    pub reason: String,
}

impl From<PolicyEffect> for openside_policy::Effect {
    fn from(effect: PolicyEffect) -> Self {
        match effect {
            PolicyEffect::Allow => Self::Allow,
            PolicyEffect::Ask => Self::Ask,
            PolicyEffect::Deny => Self::Deny,
        }
    }
}

impl From<openside_policy::Effect> for PolicyEffect {
    fn from(effect: openside_policy::Effect) -> Self {
        match effect {
            openside_policy::Effect::Allow => Self::Allow,
            openside_policy::Effect::Ask => Self::Ask,
            openside_policy::Effect::Deny => Self::Deny,
        }
    }
}

impl From<PolicyCapability> for openside_policy::Capability {
    fn from(capability: PolicyCapability) -> Self {
        match capability {
            PolicyCapability::FsRead => Self::FsRead,
            PolicyCapability::FsWrite => Self::FsWrite,
            PolicyCapability::Exec => Self::Exec,
            PolicyCapability::Browse => Self::Browse,
            PolicyCapability::Fetch => Self::Fetch,
            PolicyCapability::VaultUse => Self::VaultUse,
            PolicyCapability::Sensitive => Self::Sensitive,
            PolicyCapability::PrivacyRead => Self::PrivacyRead,
        }
    }
}

impl From<openside_policy::Decision> for PolicyDecision {
    fn from(decision: openside_policy::Decision) -> Self {
        use openside_policy::Reason;
        let reason = match decision.reason {
            Reason::DenyRule { .. } => "deny_rule",
            Reason::OriginDenied => "origin_denied",
            Reason::OriginNotAllowed => "origin_not_allowed",
            Reason::Rule { .. } => "rule",
            Reason::Mode(_) => "mode",
            Reason::SensitiveFloor => "sensitive_floor",
        };
        Self {
            effect: decision.effect.into(),
            reason: reason.into(),
        }
    }
}
