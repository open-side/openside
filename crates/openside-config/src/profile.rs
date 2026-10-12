use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Named tool and system-prompt profile the daemon offers the sidecar
/// (`docs/specs/profiles.md`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum AgentProfile {
    /// Every daemon tool with the full system prompt.
    #[default]
    #[serde(rename = "full")]
    Full,
    /// Browser REPL, bash, file and human-step tools with a compact prompt.
    #[serde(rename = "browser-agent")]
    BrowserAgent,
    /// Research mode: browsing tools and a prompt that requires source URLs.
    #[serde(rename = "ultrabrowse")]
    Ultrabrowse,
}

impl AgentProfile {
    /// Every profile, in documentation order.
    pub const ALL: [Self; 3] = [Self::Full, Self::BrowserAgent, Self::Ultrabrowse];

    /// Configuration and wire name (`open.config.agent.profile`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::BrowserAgent => "browser-agent",
            Self::Ultrabrowse => "ultrabrowse",
        }
    }
}
