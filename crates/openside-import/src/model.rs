use std::fmt;

use std::collections::BTreeMap;

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
    /// Source last-use time in Unix milliseconds; absent on formats that do not expose it.
    pub last_accessed_at_ms: Option<i64>,
    /// Source creation time in Unix milliseconds, used when last-use times tie or are absent.
    pub created_at_ms: Option<i64>,
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

/// Host-only credential transport; never serialize it or persist it outside encrypted Vault.
#[derive(Clone, PartialEq, Eq)]
pub struct SavedPassword {
    /// Strict HTTP(S) origin, normalized before Vault persistence.
    pub origin: String,
    /// Source account identifier; zeroized alongside the password.
    pub username: zeroize::Zeroizing<String>,
    /// Decrypted value, zeroized when the transport is dropped.
    pub password: zeroize::Zeroizing<String>,
}

impl fmt::Debug for SavedPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SavedPassword([REDACTED])")
    }
}

/// Address fields retained for trusted agent autofill, not native database mutation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct AutofillAddress {
    /// Source field labels mapped to their values; payment-card data is excluded.
    pub fields: BTreeMap<String, String>,
}

/// A saved open tab offered for restoration, never automatically opened.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct RestoreTab {
    /// Current navigation entry, excluding internal browser pages.
    pub url: String,
    /// Browser-provided display title.
    pub title: String,
}

/// Search-provider metadata retained without installing extensions or changing defaults.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct SearchEngine {
    /// Display label from the source provider.
    pub name: String,
    /// Source omnibox shortcut.
    pub keyword: String,
    /// Search template, potentially containing a substitution marker.
    pub url: String,
}

/// Source site preference metadata; not an authorization grant to openside.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct SitePermission {
    /// Source origin or complete primary/secondary match pattern.
    pub origin: String,
    /// Browser-specific capability label.
    pub permission: String,
    /// Browser-specific allow/block/default value.
    pub setting: String,
}

/// Extension inventory only; binaries are neither copied nor installed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct ExtensionMetadata {
    /// Browser/store extension identifier.
    pub id: String,
    /// Human-readable manifest or preference label.
    pub name: String,
    /// Official store page when the source identifies a supported store.
    pub store_url: Option<String>,
}

pub(crate) fn is_web_url(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}
