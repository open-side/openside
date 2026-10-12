use std::fmt;

/// Adapter failure. Display strings never contain request URLs (Telegram puts the
/// bot token in the path) or secret values.
#[derive(Debug)]
pub enum ChannelError {
    /// Transport failure talking to the daemon or a chat platform.
    Http(String),
    /// A chat platform answered with an error.
    Platform(String),
    /// The daemon answered with a non-success status.
    Daemon {
        /// HTTP status code.
        status: u16,
        /// Machine-readable `ApiError.code`, empty when the body was not an `ApiError`.
        code: String,
    },
    /// Local filesystem or process failure.
    Io(String),
    /// Invalid or incomplete configuration.
    Config(String),
    /// A request was refused locally (size cap, unsupported content).
    Refused(String),
}

/// Result alias for adapter operations.
pub type Result<T, E = ChannelError> = std::result::Result<T, E>;

impl ChannelError {
    pub(crate) fn http(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            "timeout"
        } else if error.is_connect() {
            "connection failed"
        } else {
            "request failed"
        };
        Self::Http(kind.into())
    }

    /// Whether the daemon reported that the addressed resource does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Daemon { status: 404, .. })
    }
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(reason) => write!(f, "http: {reason}"),
            Self::Platform(reason) => write!(f, "platform: {reason}"),
            Self::Daemon { status, code } if code.is_empty() => write!(f, "daemon: HTTP {status}"),
            Self::Daemon { status, code } => write!(f, "daemon: HTTP {status} ({code})"),
            Self::Io(reason) => write!(f, "io: {reason}"),
            Self::Config(reason) => write!(f, "config: {reason}"),
            Self::Refused(reason) => write!(f, "refused: {reason}"),
        }
    }
}

impl std::error::Error for ChannelError {}

impl From<std::io::Error> for ChannelError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind().to_string())
    }
}

/// Metadata-only diagnostics on stderr; never pass message text or secrets.
pub(crate) fn log(message: impl fmt::Display) {
    eprintln!("openside-channels: {message}");
}
