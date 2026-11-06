//! Person-facing speech-to-text: the web composer records a short clip and the daemon turns it
//! into text, so dictation works in browsers without a speech service of their own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `GET /v1/stt`: whether this caller can dictate right now and how long a clip may be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SttStatus {
    /// This caller may dictate and a transcriber is configured: `channels.stt_cmd`, or an
    /// OpenAI API key in the `openai` account pool.
    pub available: bool,
    /// Why `available` is false; null when it is true.
    #[serde(default)]
    pub reason: Option<SttUnavailable>,
    /// Largest decoded clip `POST /v1/stt` accepts, in bytes.
    pub max_bytes: u64,
    /// Longest clip a client should record, in seconds. The daemon cannot measure duration;
    /// clients stop recording at this point.
    pub max_seconds: u32,
}

/// Why dictation is unavailable to the caller; each value is the code `POST /v1/stt` would
/// answer with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SttUnavailable {
    /// The caller is an agent client, not the person's own device or the local socket.
    PersonDeviceRequired,
    /// Neither `channels.stt_cmd` nor a usable OpenAI API key is configured.
    #[serde(rename = "stt_unavailable")]
    NoTranscriber,
}

/// `POST /v1/stt`: one recorded clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscribeRequest {
    /// The clip, standard base64 with padding.
    pub audio_base64: String,
    /// The recorder's MIME type, such as `audio/webm;codecs=opus`, `audio/ogg`, `audio/mp4`,
    /// `audio/wav` or `audio/mpeg`.
    pub mime: String,
    /// Spoken language as a BCP-47 tag (`ko`, `en-US`); null lets the transcriber detect it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// `POST /v1/stt` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Transcript {
    /// Recognized text, trimmed, never empty, at most 64 KiB.
    pub text: String,
}
