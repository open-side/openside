use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Static web UI served by the daemon; unset serves no UI routes.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Built web bundle directory containing `index.html` (`OPENSIDE_UI_DIR`).
    pub dir: Option<PathBuf>,
}
