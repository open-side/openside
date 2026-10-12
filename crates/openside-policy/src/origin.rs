//! Origin canonicalization shared by origin patterns, policy targets and egress checks.
//!
//! Targets go through the WHATWG URL parser (`url` crate), the normalization Chromium applies
//! before it sends a request: lower-case scheme and host, IDNA to punycode, percent-decoded
//! host, default ports dropped, `\` read as `/` in special schemes, userinfo ignored. Patterns
//! are compiled through the same parser, so `https://EXAMPLE.com:443`, `https://bücher.example`
//! and `https://example.com.` mean exactly the origin the browser will contact. A trailing dot
//! on a domain is removed on both sides so `example.com.` cannot dodge a rule for
//! `example.com`.

use std::{fmt, sync::Arc};

use url::{Host, Url};

/// Schemes that carry a network origin. Other URLs (`data:`, `file:`, `about:`) have none.
const SCHEMES: [&str; 4] = ["http", "https", "ws", "wss"];

/// A canonical network origin: scheme, ASCII host and effective port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    /// Origin of an `http`/`https`/`ws`/`wss` URL, or `None` for anything else (paths,
    /// free text, opaque-origin schemes, URLs without a host).
    pub fn parse(target: &str) -> Option<Self> {
        Self::from_url(&Url::parse(target).ok()?)
    }

    /// Origin of an already parsed URL; see [`Origin::parse`].
    pub fn from_url(url: &Url) -> Option<Self> {
        let scheme = url.scheme();
        if !SCHEMES.contains(&scheme) {
            return None;
        }
        Some(Self {
            scheme: scheme.to_owned(),
            host: canonical_host(url.host()?)?,
            port: url.port_or_known_default()?,
        })
    }

    /// Lower-case scheme.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// ASCII host without a trailing dot; IPv6 literals keep their brackets.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Effective port (the scheme default when the URL names none).
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host)?;
        if self.port != default_port(&self.scheme) {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

fn default_port(scheme: &str) -> u16 {
    match scheme {
        "https" | "wss" => 443,
        _ => 80,
    }
}

fn canonical_host(host: Host<&str>) -> Option<String> {
    Some(match host {
        Host::Domain(domain) => {
            let domain = domain.strip_suffix('.').unwrap_or(domain);
            if domain.is_empty() {
                return None;
            }
            domain.to_owned()
        }
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => format!("[{ip}]"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostPattern {
    Any,
    Exact(String),
    /// Strict subdomains (any depth) of the domain; never the domain itself.
    Subdomains(String),
}

/// A compiled origin pattern: `scheme://host[:port]`.
///
/// * `scheme`: `http`, `https`, `ws` or `wss`.
/// * `host`: a host, `*.domain` (every subdomain, at any depth, not the domain itself) or `*`
///   (any host). A `*` anywhere else is rejected.
/// * `port`: a number or `*`. Without a port the pattern matches the scheme's default port
///   only, so `https://example.com` does not cover `https://example.com:8443`.
///
/// Userinfo, paths, queries, fragments, backslashes and whitespace are rejected rather than
/// ignored: such an entry could never match what the browser contacts and would silently
/// restrict nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginPattern {
    scheme: String,
    host: HostPattern,
    /// `None` = any port.
    port: Option<u16>,
}

const SHAPE: &str = "origin patterns must look like scheme://host[:port]";

impl OriginPattern {
    /// Compiles one pattern; the error names what is wrong with it.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (scheme, authority) = raw.split_once("://").ok_or(SHAPE)?;
        let scheme = scheme.to_ascii_lowercase();
        if !SCHEMES.contains(&scheme.as_str()) {
            return Err(format!("{raw:?}: scheme must be http, https, ws or wss"));
        }
        if authority.is_empty()
            || authority.chars().any(|c| {
                matches!(c, '/' | '\\' | '?' | '#' | '@') || c.is_whitespace() || c.is_control()
            })
        {
            return Err(format!(
                "{raw:?}: {SHAPE} without a path, query, credentials or whitespace"
            ));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !authority.ends_with(']') => (host, Some(port)),
            _ => (authority, None),
        };
        let port = match port {
            None => Some(default_port(&scheme)),
            Some("*") => None,
            Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                Some(
                    digits
                        .parse()
                        .map_err(|_| format!("{raw:?}: port out of range"))?,
                )
            }
            Some(_) => return Err(format!("{raw:?}: port must be a number or *")),
        };
        let host = if host == "*" {
            HostPattern::Any
        } else if let Some(domain) = host.strip_prefix("*.") {
            match parse_host(&scheme, domain) {
                Some((domain, true)) if !domain.contains('*') => HostPattern::Subdomains(domain),
                _ => return Err(format!("{raw:?}: *. must be followed by a domain name")),
            }
        } else if host.contains('*') {
            return Err(format!(
                "{raw:?}: * is only allowed as the whole host or as a leading *. label"
            ));
        } else {
            let (host, _) =
                parse_host(&scheme, host).ok_or_else(|| format!("{raw:?}: invalid host"))?;
            HostPattern::Exact(host)
        };
        Ok(Self { scheme, host, port })
    }

    /// Whether `origin` is covered by this pattern.
    pub fn matches(&self, origin: &Origin) -> bool {
        if self.scheme != origin.scheme || self.port.is_some_and(|port| port != origin.port) {
            return false;
        }
        match &self.host {
            HostPattern::Any => true,
            HostPattern::Exact(host) => *host == origin.host,
            HostPattern::Subdomains(domain) => origin
                .host
                .strip_suffix(domain.as_str())
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.')),
        }
    }
}

/// Runs `host` through the URL parser for `scheme`: the canonical host and whether it is a
/// domain name (not an IP literal), or `None` if it is not a valid bare host.
fn parse_host(scheme: &str, host: &str) -> Option<(String, bool)> {
    let url = Url::parse(&format!("{scheme}://{host}/")).ok()?;
    // A host the parser rewrote into something with a port or userinfo was not a bare host.
    if url.port().is_some() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let parsed = url.host()?;
    let domain = matches!(parsed, Host::Domain(_));
    Some((canonical_host(parsed)?, domain))
}

/// Origin allow/deny lists for web capabilities, compiled from [`OriginPattern`] strings.
///
/// They apply to `Browse`, `Fetch`, `VaultUse` and `Sensitive` actions whose target has an
/// origin; other targets are unaffected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginLists {
    allow: Arc<[OriginPattern]>,
    deny: Arc<[OriginPattern]>,
}

impl Default for OriginLists {
    fn default() -> Self {
        Self {
            allow: Arc::from(Vec::new()),
            deny: Arc::from(Vec::new()),
        }
    }
}

impl OriginLists {
    /// Compiles both lists; the first invalid entry is returned as an error.
    pub fn new<A, D>(allow: A, deny: D) -> Result<Self, String>
    where
        A: IntoIterator,
        A::Item: AsRef<str>,
        D: IntoIterator,
        D::Item: AsRef<str>,
    {
        let compile = |items: Vec<OriginPattern>| -> Arc<[OriginPattern]> { items.into() };
        Ok(Self {
            allow: compile(
                allow
                    .into_iter()
                    .map(|raw| OriginPattern::parse(raw.as_ref()))
                    .collect::<Result<_, _>>()?,
            ),
            deny: compile(
                deny.into_iter()
                    .map(|raw| OriginPattern::parse(raw.as_ref()))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }

    /// `Some(reason)` when `origin` may not be used at all.
    pub(crate) fn block(&self, origin: &Origin) -> Option<crate::Reason> {
        if self.deny.iter().any(|pattern| pattern.matches(origin)) {
            return Some(crate::Reason::OriginDenied);
        }
        if !self.allow.is_empty() && !self.allow.iter().any(|pattern| pattern.matches(origin)) {
            return Some(crate::Reason::OriginNotAllowed);
        }
        None
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn origin(target: &str) -> Origin {
        Origin::parse(target).unwrap()
    }

    fn matches(pattern: &str, target: &str) -> bool {
        OriginPattern::parse(pattern)
            .unwrap()
            .matches(&origin(target))
    }

    #[test]
    fn targets_canonicalize_like_the_browser() {
        for (input, expected) in [
            ("HTTPS://EXAMPLE.com/a?b#c", "https://example.com"),
            ("https://example.com:443/", "https://example.com"),
            ("http://example.com:80", "http://example.com"),
            ("https://example.com:8443", "https://example.com:8443"),
            ("https://example.com./x", "https://example.com"),
            ("https://bücher.example/", "https://xn--bcher-kva.example"),
            ("https://%65xample.com/", "https://example.com"),
            ("https://u:p@host.example/", "https://host.example"),
            ("https://good.example@evil.example/", "https://evil.example"),
            ("https:\\\\evil.example\\path", "https://evil.example"),
            ("http://[::1]:8080/", "http://[::1]:8080"),
            ("http://0x7f.1/", "http://127.0.0.1"),
            ("wss://chat.example/socket", "wss://chat.example"),
        ] {
            assert_eq!(origin(input).to_string(), expected, "{input}");
        }
        for not_an_origin in [
            "/etc/passwd",
            "pay now",
            "https:///",
            "data:text/html,x",
            "file:///etc/passwd",
            "about:blank",
            "javascript:alert(1)",
            "click button \"Pay\" on https://x.example",
        ] {
            assert_eq!(Origin::parse(not_an_origin), None, "{not_an_origin}");
        }
    }

    #[test]
    fn patterns_share_target_normalization() {
        assert!(matches("https://EXAMPLE.com", "https://example.com/x"));
        assert!(matches("https://example.com:443", "https://example.com"));
        assert!(matches("https://example.com", "https://EXAMPLE.COM.:443/"));
        assert!(matches("https://example.com.", "https://example.com"));
        assert!(matches(
            "https://bücher.example",
            "https://xn--bcher-kva.example"
        ));
        assert!(matches(
            "https://xn--bcher-kva.example",
            "https://bücher.example/"
        ));
        assert!(matches("http://[::1]:8080", "http://[0:0::1]:8080/"));
        assert!(matches("http://127.0.0.1:*", "http://127.0.0.1:4100"));
        assert!(matches("http://127.0.0.1:*", "http://127.0.0.1"));
        assert!(!matches("https://example.com", "https://example.com:8443"));
        assert!(!matches("https://example.com", "http://example.com"));
        assert!(!matches(
            "https://example.com",
            "https://example.com.evil.example"
        ));
        assert!(matches(
            "https://evil.example",
            "https://good.example@evil.example"
        ));
        assert!(matches("https://evil.example", "https:\\\\evil.example"));
    }

    #[test]
    fn wildcard_boundaries() {
        assert!(matches("https://*.example.com", "https://a.example.com"));
        assert!(matches("https://*.example.com", "https://a.b.example.com"));
        assert!(!matches("https://*.example.com", "https://example.com"));
        assert!(!matches("https://*.example.com", "https://badexample.com"));
        assert!(!matches(
            "https://*.example.com",
            "https://example.com.evil.example"
        ));
        assert!(matches("https://*.EXAMPLE.com.", "https://A.example.com"));
        assert!(matches("https://*:*", "https://anything.example:1234"));
        assert!(!matches("https://*", "https://anything.example:1234"));
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        for bad in [
            "example.com",
            "https://",
            "ftp://example.com",
            "javascript://x",
            "https://example.com/",
            "https://example.com/path",
            "https://user@example.com",
            "https://u:p@example.com",
            "https://example.com?q",
            "https://example.com#f",
            "https:\\\\example.com",
            "https://exa mple.com",
            "https://example.com:",
            "https://example.com:http",
            "https://example.com:70000",
            "https://a*.example.com",
            "https://a.*.example.com",
            "https://**.example.com",
            "https://*.",
            "https://*.1.2.3.4",
            "https://[::1",
        ] {
            assert!(OriginPattern::parse(bad).is_err(), "{bad:?} accepted");
        }
    }
}
