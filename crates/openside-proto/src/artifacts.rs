//! Task artifacts (A-12), message attachments (PP-14) and inline visuals (A-15).
//!
//! Every session owns one private artifact directory. Paths in this contract are relative to
//! it, use `/` separators, and are percent-encoded as a single URL path segment in routes.

use chrono::{DateTime, Utc};
use openside_core::ContentKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Most attachments one message may carry.
pub const MAX_ATTACHMENTS: usize = 4;
/// Decoded bytes allowed across all attachments of one message. Parts travel inline in one
/// sidecar line (4 MiB), so the base64 form must stay well below that.
pub const MAX_ATTACHMENT_BYTES: usize = 2 * 1024 * 1024;

/// One uploaded attachment; the daemon sniffs the type from the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachmentUpload {
    /// Display file name; directories are stripped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Standard base64 file content. Allowed: PNG, JPEG, GIF, WebP, PDF, WAV, MP3, OGG,
    /// FLAC and UTF-8 text; anything else is `415 unsupported_attachment`.
    pub data_base64: String,
}

/// Durable record of a stored attachment, carried by `message.queued`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    /// Artifact path holding the bytes.
    pub path: String,
    /// Media class.
    pub kind: ContentKind,
    /// Sniffed MIME type.
    pub mime: String,
    /// Byte size.
    pub size: u64,
    /// Lowercase hex SHA-256 of the bytes.
    pub sha256: String,
    /// Sanitized display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Where an artifact came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSource {
    /// A user attachment.
    Attachment,
    /// A file a tool created or modified (copied from the workspace) or a tool output.
    Tool,
    /// The exported session event log.
    Transcript,
    /// A visual block from an assistant message.
    Visual,
}

/// One file in a session's artifact directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    /// Path relative to the artifact directory.
    pub path: String,
    /// Byte size.
    pub size: u64,
    /// MIME type served by the raw route.
    pub mime: String,
    /// Origin of the file.
    pub source: ArtifactSource,
    /// First recording time.
    pub created_at: DateTime<Utc>,
    /// Latest recording time.
    pub modified_at: DateTime<Utc>,
    /// Tool call that last wrote it, for tool artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name that last wrote it, for tool artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Time-limited, unauthenticated URL path for sandboxed rendering, when previewable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
}

/// `GET /v1/sessions/{id}/artifacts` response, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactList {
    /// Artifacts in creation order.
    pub artifacts: Vec<Artifact>,
}

/// Renderable visual block kinds. Assistants mark them with a fenced code block whose info
/// string is the kind followed by `visual`, e.g. "```svg visual".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualKind {
    /// Self-contained HTML document; inline scripts run only inside the sandbox.
    Html,
    /// SVG image.
    Svg,
}

/// A visual part of an assistant message with its source and a sandboxed preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Visual {
    /// Stable id: `<assistant.final seq>-<block index>`.
    pub id: String,
    /// Sequence of the `assistant.final` event containing the block.
    pub seq: u64,
    /// Visual kind.
    pub kind: VisualKind,
    /// Block source text, exactly as written by the assistant.
    pub source: String,
    /// Artifact path of the materialized source.
    pub path: String,
    /// Time-limited, unauthenticated URL path for sandboxed rendering.
    pub preview_url: String,
}

/// `GET /v1/sessions/{id}/visuals` response in message order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VisualList {
    /// Visual parts, oldest first.
    pub visuals: Vec<Visual>,
}
