use crate::Error;
use url::{Host, Url};

/// An HTTP(S) origin allowlist rule, optionally matching explicit subdomains.
///
/// Rules accept origins only (no path, query, credentials or fragment). Unicode
/// hosts and IP literals are deliberately unsupported; IDNA A-labels must be
/// supplied explicitly, preventing silent Unicode-to-punycode equivalence.
#[derive(Clone, Eq, PartialEq)]
pub struct AllowedOrigin {
    scheme: String,
    host: String,
    port: u16,
    subdomains: bool,
}

impl std::fmt::Debug for AllowedOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl AllowedOrigin {
    /// Parse an origin such as `https://example.com` or `https://*.example.com`.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let (scheme, authority) = value.split_once("://").ok_or(Error::InvalidOrigin)?;
        let subdomains = authority.starts_with("*.");
        let normalized = if subdomains {
            format!("{scheme}://{}", &authority[2..])
        } else {
            value.to_owned()
        };
        let mut result = parse_exact(&normalized)?;
        if subdomains && !result.host.contains('.') {
            return Err(Error::InvalidOrigin);
        }
        result.subdomains = subdomains;
        Ok(result)
    }

    /// Match scheme, DNS host and effective port without resolving DNS.
    pub fn matches(&self, origin: &str) -> bool {
        let Ok(candidate) = parse_exact(origin) else {
            return false;
        };
        self.scheme == candidate.scheme
            && self.port == candidate.port
            && if self.subdomains {
                candidate.host.len() > self.host.len() + 1
                    && candidate.host.ends_with(&self.host)
                    && candidate.host.as_bytes()[candidate.host.len() - self.host.len() - 1] == b'.'
            } else {
                candidate.host == self.host
            }
    }

    pub(crate) fn wire(&self) -> String {
        format!(
            "{}://{}{}:{}",
            self.scheme,
            if self.subdomains { "*." } else { "" },
            self.host,
            self.port
        )
    }
}

fn parse_exact(value: &str) -> Result<AllowedOrigin, Error> {
    if !value.is_ascii()
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(Error::InvalidOrigin);
    }
    let (_, authority) = value.split_once("://").ok_or(Error::InvalidOrigin)?;
    // URL parsers normalize backslashes, percent escapes and unusual IPv4 forms.
    // Reject these before parsing so normalization cannot broaden the allowlist.
    if authority.contains(['@', '%', '\\', '*', '?', '#'])
        || authority
            .strip_suffix('/')
            .unwrap_or(authority)
            .contains('/')
    {
        return Err(Error::InvalidOrigin);
    }
    let url = Url::parse(value).map_err(|_| Error::InvalidOrigin)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidOrigin);
    }
    let Some(Host::Domain(host)) = url.host() else {
        return Err(Error::InvalidOrigin);
    };
    if host.ends_with('.')
        || host.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(Error::InvalidOrigin);
    }
    Ok(AllowedOrigin {
        scheme: url.scheme().to_owned(),
        host: host.to_owned(),
        port: url.port_or_known_default().ok_or(Error::InvalidOrigin)?,
        subdomains: false,
    })
}

/// Whether a WebAuthn RP ID may be used from `origin`: the RP ID must equal the origin's
/// DNS host or be a dotted parent of it. Chromium additionally enforces the public suffix
/// list; this check keeps a stored passkey from being released to an unrelated page.
pub(crate) fn rp_id_allowed(rp_id: &str, origin: &str) -> bool {
    let Ok(origin) = parse_exact(origin) else {
        return false;
    };
    !rp_id.is_empty()
        && (origin.host == rp_id
            || (rp_id.contains('.')
                && origin.host.len() > rp_id.len() + 1
                && origin.host.ends_with(rp_id)
                && origin.host.as_bytes()[origin.host.len() - rp_id.len() - 1] == b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_scheme_host_port() {
        let rule = AllowedOrigin::parse("https://example.com").unwrap();
        for origin in ["https://example.com", "https://EXAMPLE.com:443/"] {
            assert!(rule.matches(origin));
        }
        for origin in [
            "http://example.com",
            "https://example.com:444",
            "https://a.example.com",
            "https://example.com.evil.test",
            "https://example.com/path",
        ] {
            assert!(!rule.matches(origin));
        }
    }

    #[test]
    fn explicit_subdomains_only() {
        let rule = AllowedOrigin::parse("https://*.example.com:8443").unwrap();
        assert!(rule.matches("https://a.b.example.com:8443"));
        for origin in [
            "https://example.com:8443",
            "https://notexample.com:8443",
            "https://a.example.com",
            "https://a.example.com.evil.test:8443",
        ] {
            assert!(!rule.matches(origin));
        }
        assert!(AllowedOrigin::parse("https://*.com").is_err());
    }

    #[test]
    fn rejects_parser_tricks_and_lookalikes() {
        let rule = AllowedOrigin::parse("https://example.com").unwrap();
        for origin in [
            "https://user@example.com",
            "https://@example.com",
            "https://example.com@evil.test",
            "https://example.com\\@evil.test",
            "https://%65xample.com",
            "https://example.com.",
            "https://ｅxample.com",
            "https://exаmple.com",
            "https://xn--exmple-4nf.com",
            "https://127.0.0.1",
            "https://127.1",
            "https://2130706433",
            "https://0x7f000001",
            "https://[::1]",
            "https://example.com#x",
            "https://example.com?x",
            " https://example.com",
        ] {
            assert!(!rule.matches(origin), "accepted disallowed origin");
        }
        for ip in [
            "https://127.0.0.1",
            "https://127.1",
            "https://2130706433",
            "https://[::1]",
        ] {
            assert!(AllowedOrigin::parse(ip).is_err());
        }
        let idna = AllowedOrigin::parse("https://xn--bcher-kva.example").unwrap();
        assert!(idna.matches("https://xn--bcher-kva.example"));
        assert!(!idna.matches("https://bücher.example"));
    }
}
