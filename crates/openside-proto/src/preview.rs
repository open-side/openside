use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Terminal causes for a read-only stream; reconnecting does not revive a stopped session.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewEndReason {
    /// The viewing device lost its capability, without affecting other viewers.
    Revoked,
    /// The session's stop event was committed.
    SessionStopped,
    /// The session became a tombstone and cannot expose browser content.
    SessionDeleted,
}

/// Temporary capture exclusions that leave the viewing connection open.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewPauseReason {
    /// A pending human step owns the tab's sole Chromium screencast.
    Takeover,
}

/// Server-only JSON messages for the browse-scoped session preview WebSocket.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreviewServerMessage {
    /// JPEG of the latest tab used successfully by this session's browser tools.
    Frame {
        /// Base64 bytes without a data-URL prefix.
        data: String,
        /// Encoded image width, not CDP's unscaled CSS viewport width.
        width: u32,
        /// Encoded image height, at most 450 pixels.
        height: u32,
        /// Owned tab identifier; changes when the session moves to another tab.
        tab_id: String,
    },
    /// No pixels are disclosed after any secret fill in this tab's history.
    Withheld {
        /// Fixed safety code; never includes page text or a secret.
        code: String,
        /// Tab whose entire history is excluded, including later clean navigations.
        tab_id: String,
    },
    /// Capture resumes automatically when the human step releases the tab.
    Paused {
        /// Safety exclusion, not a disconnected socket.
        reason: PreviewPauseReason,
        /// Human-controlled tab that this session would otherwise preview.
        tab_id: String,
    },
    /// No current live owned tab can be selected; no browser is launched by preview.
    NoTab,
    /// Final message before the socket's bounded close handshake.
    Ended {
        /// Durable lifetime or device-capability boundary that ended the stream.
        reason: PreviewEndReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preview_wire_shapes_and_browse_scope_are_documented() {
        let message = PreviewServerMessage::Paused {
            reason: PreviewPauseReason::Takeover,
            tab_id: "tab".into(),
        };
        assert_eq!(
            serde_json::to_value(message).unwrap(),
            json!({"type":"paused","reason":"takeover","tab_id":"tab"})
        );
        let document = crate::openapi();
        let route = &document["paths"]["/v1/sessions/{id}/browser/preview"]["get"];
        assert_eq!(
            route["responses"]["101"]["x-websocket-server-message"]["$ref"],
            "#/components/schemas/PreviewServerMessage"
        );
        assert_eq!(
            route["responses"]["101"]["x-websocket-client-message"],
            serde_json::Value::Null
        );
        assert_eq!(
            crate::ROUTES
                .iter()
                .find(|route| route.path.ends_with("/browser/preview"))
                .unwrap()
                .scope,
            crate::CapabilityScope::Browse
        );
    }
}
