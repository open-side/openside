//! Model-visible content beyond plain text (PP-14): user attachments and binary tool results.
//!
//! Parts travel inline to the runtime; the durable record keeps only artifact references,
//! so event logs never carry megabytes of bytes.

/// Broad media class of one content part; the runtime maps it to model input types.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    /// UTF-8 text document.
    Text,
    /// Raster image (PNG, JPEG, GIF, WebP).
    Image,
    /// Audio clip (WAV, MP3, OGG, FLAC).
    Audio,
    /// Any other document, e.g. PDF.
    File,
}

/// One content part handed to the runtime with a message or a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentPart {
    /// Media class.
    pub kind: ContentKind,
    /// Sniffed MIME type.
    pub mime: String,
    /// Inline bytes; `None` when only the artifact reference is offered.
    pub data: Option<Vec<u8>>,
    /// Session-relative artifact path holding the same bytes, when stored.
    pub artifact_ref: Option<String>,
    /// Display name supplied by the user or tool.
    pub name: Option<String>,
}
