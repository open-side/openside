use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A native tool of the person's own browser that openside's menus open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PersonNativeTool {
    /// The browser's Task manager.
    #[serde(rename = "task_manager")]
    TaskManager,
    /// Developer tools for the selected tab of the person's most recently active window.
    #[serde(rename = "devtools")]
    DevTools,
}

/// Open a native tool in the person's own browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenPersonNativeToolRequest {
    /// Which tool.
    pub tool: PersonNativeTool,
}
