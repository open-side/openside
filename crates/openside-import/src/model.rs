use std::fmt;

use serde::Serialize;
use zeroize::Zeroize;

/// One page visit, Unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Visit {
    /// Page address (http or https only).
    pub url: String,
    /// Page title when the browser recorded one.
    pub title: Option<String>,
    /// When the page was visited.
    pub visited_at_ms: i64,
}

/// One bookmark with the folder names leading to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bookmark {
    /// Bookmarked address.
    pub url: String,
    /// Bookmark title.
    pub title: String,
    /// Folder path from the browser's bookmark root, outermost first.
    pub folder: Vec<String>,
    /// When the bookmark was added, Unix milliseconds.
    pub added_at_ms: Option<i64>,
}

/// Cookie `SameSite` attribute as stored by the source browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// `SameSite=None`.
    None,
    /// `SameSite=Lax`.
    Lax,
    /// `SameSite=Strict`.
    Strict,
}

/// A decrypted cookie. Its value is secret: `Debug` redacts it and it is never serialized.
#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    /// Host as stored; a leading dot marks a domain cookie.
    pub host: String,
    /// Cookie name.
    pub name: String,
    /// Plaintext value.
    pub value: String,
    /// Cookie path.
    pub path: String,
    /// Expiry in Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    /// `Secure` attribute.
    pub secure: bool,
    /// `HttpOnly` attribute.
    pub http_only: bool,
    /// `SameSite` attribute when the source recorded one.
    pub same_site: Option<SameSite>,
}

impl Drop for Cookie {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cookie")
            .field("host", &self.host)
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

pub(crate) fn is_web_url(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}
