//! Operation errors reported to the script, and their mapping from browser errors.
//!
//! Every error has a `name`, and the name decides whether a retry can help; the prelude exposes
//! that as `error.transient`:
//!
//! - transient: `TimeoutError` (something was not there or not done yet) and `NetworkError`
//!   (the server could not be reached or dropped the connection).
//! - permanent: everything else, including `NavigationError` (a URL, certificate, or policy
//!   problem that repeating the navigation will not change), `PolicyError`, and plain `Error`.

use openside_browser::BrowserError;

pub(crate) struct OpError {
    pub(super) name: &'static str,
    pub(super) message: String,
}

impl OpError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            name: "Error",
            message: message.into(),
        }
    }
    pub(super) fn timeout(message: impl Into<String>) -> Self {
        Self {
            name: "TimeoutError",
            message: message.into(),
        }
    }
}

pub(super) type OpResult<T> = Result<T, OpError>;

pub(super) fn browser_error(error: BrowserError) -> OpError {
    match error {
        BrowserError::StaleReference => {
            OpError::new("stale aria-ref; take a new snapshot (page.snapshot()) and use its refs")
        }
        BrowserError::OwnershipDenied(_) => OpError::new("tab is not owned by this session"),
        BrowserError::Timeout(what) => OpError::timeout(format!("browser timeout: {what}")),
        BrowserError::Cdp { message, .. } if message.contains("invalid URL") => OpError {
            name: "NavigationError",
            message: format!("{message} (permanent: the URL is malformed)"),
        },
        other => OpError::new(format!("browser operation failed: {other}")),
    }
}

/// Chromium reports this for an HTTP 4xx/5xx answer with an empty body, after it already
/// committed its own error page. The navigation did reach the server and has a status, so
/// `goto` resolves with the response like any other navigation.
pub(super) const HTTP_STATUS_FAILURE: &str = "net::ERR_HTTP_RESPONSE_CODE_FAILURE";

/// Classify a failed navigation by Chromium's error text.
pub(super) fn navigation_error(api: &str, url: &str, error_text: &str) -> OpError {
    let code = error_text.strip_prefix("net::").unwrap_or(error_text);
    let (name, why) = match code {
        "ERR_CONNECTION_REFUSED" => (
            "NetworkError",
            "nothing is listening there; the server may not be up yet",
        ),
        "ERR_CONNECTION_RESET" | "ERR_CONNECTION_CLOSED" | "ERR_EMPTY_RESPONSE" => (
            "NetworkError",
            "the server closed the connection without a full answer",
        ),
        "ERR_CONNECTION_TIMED_OUT" | "ERR_TIMED_OUT" => {
            ("NetworkError", "the server did not answer in time")
        }
        "ERR_NETWORK_CHANGED" | "ERR_INTERNET_DISCONNECTED" | "ERR_NETWORK_IO_SUSPENDED" => {
            ("NetworkError", "the network connection was lost")
        }
        "ERR_ADDRESS_UNREACHABLE" | "ERR_NETWORK_ACCESS_DENIED" => {
            ("NetworkError", "the host cannot be reached from here")
        }
        "ERR_NAME_NOT_RESOLVED" => (
            "NavigationError",
            "the host name does not resolve; check the spelling of the address",
        ),
        "ERR_TOO_MANY_REDIRECTS" => ("NavigationError", "the server redirects in a loop"),
        "ERR_INVALID_URL" | "ERR_UNKNOWN_URL_SCHEME" => (
            "NavigationError",
            "the URL is malformed or has an unsupported scheme",
        ),
        "ERR_UNSAFE_PORT" => ("NavigationError", "the browser blocks that port"),
        "ERR_BLOCKED_BY_CLIENT" | "ERR_BLOCKED_BY_RESPONSE" | "ERR_ABORTED" => (
            "NavigationError",
            "the request was blocked or replaced by another navigation (an openside policy rule \
             blocks redirects to origins that are not allowed)",
        ),
        code if code.starts_with("ERR_CERT_") || code.starts_with("ERR_SSL_") => (
            "NavigationError",
            "the TLS certificate or handshake was rejected",
        ),
        _ => ("NavigationError", "the navigation failed"),
    };
    let class = if name == "NetworkError" {
        "transient: retrying can succeed"
    } else {
        "permanent: retrying will not help"
    };
    OpError {
        name,
        message: format!("{api}: {error_text} at {url}: {why} ({class})"),
    }
}
