//! Opens the Task manager or developer tools in the person's own browser, through its
//! person-role gateway (see [`crate::person_gateway`]). Only the person may: agent clients get
//! `person_device_required`.

use axum::{extract::State, http::StatusCode, Extension, Json};
use openside_browser::{BrowserError, NativeTool};
use openside_core::MessageAuthorKind;
use openside_proto::{OpenPersonNativeToolRequest, PersonNativeTool};
use serde_json::json;

use crate::api::{Caller, HttpError};
use crate::engine::Shared;
use crate::person_gateway::PersonBrowser;

pub(crate) async fn open(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Json(request): Json<OpenPersonNativeToolRequest>,
) -> Result<StatusCode, HttpError> {
    crate::takeover::person_only(&caller, author)?;
    let gateway = match s.registry.person_gateway.get().await {
        PersonBrowser::Gateway(gateway) => gateway,
        PersonBrowser::Stock => {
            return Err(HttpError(StatusCode::CONFLICT, "native_tools_unsupported"))
        }
        PersonBrowser::NotRunning => {
            return Err(HttpError(StatusCode::CONFLICT, "browser_not_running"))
        }
    };
    let (tool, name) = match request.tool {
        PersonNativeTool::TaskManager => (NativeTool::TaskManager, "task_manager"),
        PersonNativeTool::DevTools => (NativeTool::DevTools, "devtools"),
    };
    gateway.open_native_tool(tool).await.map_err(http_error)?;
    crate::pairing::audit(
        &s,
        "person_browser.native_tools",
        "allow",
        json!({"device": caller.0, "tool": name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

fn http_error(error: BrowserError) -> HttpError {
    match error {
        // Not a patched build, no window of the person's profile, or a policy that turns
        // developer tools off.
        BrowserError::Gateway { code, .. }
            if matches!(code.as_str(), "unsupported" | "policy_denied") =>
        {
            HttpError(StatusCode::CONFLICT, "native_tools_unsupported")
        }
        BrowserError::Disconnected | BrowserError::Timeout(_) => {
            HttpError(StatusCode::SERVICE_UNAVAILABLE, "browser_disconnected")
        }
        _ => HttpError::internal(),
    }
}
