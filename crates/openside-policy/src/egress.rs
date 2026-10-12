//! Outbound connection allow-list (D-5, G-4, PP-25).
//!
//! openside sends nothing off the machine except requests to configured model providers and
//! services the user configured explicitly. Every Rust component that opens an outbound
//! connection checks the destination here first. Loopback destinations are always
//! permitted: they never leave the host.
//!
//! Entry syntax (case-insensitive): `host`, `*.domain` (subdomains only), `host:port`,
//! or `scheme://host[:port]` (scheme and port must then match too). An entry without a
//! port matches every port.

use std::{fmt, net::IpAddr};

/// Destination check failed; the connection must not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressError {
    /// The destination is not loopback and matches no allow-list entry.
    NotAllowed {
        /// Lower-cased `scheme://host:port` that was refused.
        destination: String,
    },
    /// The URL could not be parsed into scheme, host and port.
    InvalidUrl,
}

impl fmt::Display for EgressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed { destination } => {
                write!(
                    f,
                    "outbound connection to {destination} is not in network.allow"
                )
            }
            Self::InvalidUrl => f.write_str("outbound URL is invalid"),
        }
    }
}

impl std::error::Error for EgressError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    scheme: Option<String>,
    host: String,
    port: Option<u16>,
}

/// Allow-list of outbound destinations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Egress {
    entries: Vec<Entry>,
}

impl Egress {
    /// Builds the list from `network.allow` entries plus endpoints the caller was configured
    /// with. Invalid entries are returned as errors so configuration mistakes are visible.
    pub fn new<I, S>(entries: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let entries = entries
            .into_iter()
            .map(|entry| parse_entry(entry.as_ref()))
            .collect::<Result<_, _>>()?;
        Ok(Self { entries })
    }

    /// Checks a destination given as an absolute URL, canonicalized like policy origins
    /// ([`crate::Origin`]): IDNA, default ports, userinfo and backslashes as a browser reads them.
    pub fn check_url(&self, url: &str) -> Result<(), EgressError> {
        let origin = crate::Origin::parse(url).ok_or(EgressError::InvalidUrl)?;
        self.check(origin.scheme(), origin.host(), origin.port())
    }

    /// Checks a destination given as scheme, host (name or IP literal, IPv6 with or without
    /// brackets) and port.
    pub fn check(&self, scheme: &str, host: &str, port: u16) -> Result<(), EgressError> {
        let normalized = normalize_host(host);
        if is_loopback(&normalized) || self.allows_explicitly(scheme, host, port) {
            return Ok(());
        }
        Err(EgressError::NotAllowed {
            destination: format!("{}://{normalized}:{port}", scheme.to_ascii_lowercase()),
        })
    }

    /// Whether an allow-list entry names this destination. Unlike [`Self::check`], loopback
    /// is not implied: for destinations chosen by a remote party (Web Push endpoints) only
    /// what the user listed counts.
    pub fn allows_explicitly(&self, scheme: &str, host: &str, port: u16) -> bool {
        let scheme = scheme.to_ascii_lowercase();
        let host = normalize_host(host);
        self.entries.iter().any(|entry| {
            entry.scheme.as_deref().map_or(true, |s| s == scheme)
                && entry.port.map_or(true, |p| p == port)
                && host_matches(&entry.host, &host)
        })
    }

    /// Checks a socket peer address (`/proc/net/tcp`-style observation or a connector).
    pub fn check_ip(&self, ip: IpAddr, port: u16) -> Result<(), EgressError> {
        if ip.is_loopback() || ip.to_canonical().is_loopback() {
            return Ok(());
        }
        let host = ip.to_string();
        if self
            .entries
            .iter()
            .any(|entry| entry.port.map_or(true, |p| p == port) && entry.host == host)
        {
            return Ok(());
        }
        Err(EgressError::NotAllowed {
            destination: format!("{host}:{port}"),
        })
    }
}

fn normalize_host(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// `localhost`, `*.localhost` and loopback IP literals.
pub(crate) fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.to_canonical().is_loopback())
}

fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(domain) => host
            .strip_suffix(domain)
            .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
        None => pattern == host,
    }
}

fn parse_entry(raw: &str) -> Result<Entry, String> {
    let text = raw.trim().to_ascii_lowercase();
    let invalid = || format!("invalid network.allow entry {raw:?}");
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_owned()), rest),
        None => (None, text.as_str()),
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains(['/', '?', '#', '@']) {
        return Err(invalid());
    }
    let (host, port) = split_host_port(rest).ok_or_else(invalid)?;
    let wildcard_body = host.strip_prefix("*.").unwrap_or(&host);
    if wildcard_body.is_empty() || wildcard_body.contains('*') {
        return Err(invalid());
    }
    Ok(Entry { scheme, host, port })
}

fn split_host_port(text: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(port) => Some(port.parse().ok()?),
            None if after.is_empty() => None,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match text.rsplit_once(':') {
        // A bare IPv6 literal has several colons and no port.
        Some((host, _)) if host.contains(':') => Some((text.to_owned(), None)),
        Some((host, port)) => Some((host.to_owned(), Some(port.parse().ok()?))),
        None => Some((text.to_owned(), None)),
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn egress(entries: &[&str]) -> Egress {
        Egress::new(entries.iter().copied()).unwrap()
    }

    #[test]
    fn empty_list_allows_only_loopback() {
        let e = Egress::default();
        for url in [
            "http://127.0.0.1:9/x",
            "http://localhost:80",
            "http://[::1]:5/",
            "ws://127.9.9.9:1",
            "http://app.localhost/",
        ] {
            assert_eq!(e.check_url(url), Ok(()), "{url}");
        }
        for url in [
            "https://api.example.com/v1",
            "http://10.0.0.1/",
            "http://[2001:db8::1]:8080/",
            "https://localhost.evil.example/",
            "http://127.0.0.1.evil.example/",
        ] {
            assert!(
                matches!(e.check_url(url), Err(EgressError::NotAllowed { .. })),
                "{url}"
            );
        }
    }

    #[test]
    fn entries_match_host_wildcard_port_and_scheme() {
        let e = egress(&[
            "api.example.com",
            "*.provider.example",
            "hooks.example:8443",
            "https://strict.example",
        ]);
        assert!(e.check_url("https://API.example.com/v1").is_ok());
        assert!(e.check_url("http://api.example.com:9999").is_ok());
        assert!(e.check_url("https://eu.provider.example").is_ok());
        assert!(
            e.check_url("https://provider.example").is_err(),
            "wildcard is subdomains only"
        );
        assert!(e.check_url("https://evilprovider.example").is_err());
        assert!(e.check_url("https://hooks.example:8443/x").is_ok());
        assert!(e.check_url("https://hooks.example/x").is_err());
        assert!(e.check_url("https://strict.example/").is_ok());
        assert!(e.check_url("http://strict.example/").is_err());
        assert!(e
            .check_url("https://api.example.com@evil.example/")
            .is_err());
    }

    #[test]
    fn ip_peers_need_an_exact_ip_entry() {
        let e = egress(&["192.0.2.7:443", "[2001:db8::2]"]);
        assert!(e.check_ip("127.0.0.1".parse().unwrap(), 1).is_ok());
        assert!(e.check_ip("::ffff:127.0.0.1".parse().unwrap(), 1).is_ok());
        assert!(e.check_ip("192.0.2.7".parse().unwrap(), 443).is_ok());
        assert!(e.check_ip("192.0.2.7".parse().unwrap(), 80).is_err());
        assert!(e.check_ip("2001:db8::2".parse().unwrap(), 1).is_ok());
        assert!(e.check_ip("8.8.8.8".parse().unwrap(), 53).is_err());
    }

    #[test]
    fn invalid_entries_and_urls_are_rejected() {
        for bad in [
            "",
            "*",
            "**.example",
            "a.*.example",
            "example.com/path",
            "x:notaport",
            "user@host",
        ] {
            assert!(Egress::new([bad]).is_err(), "{bad:?}");
        }
        assert_eq!(
            Egress::default().check_url("not a url"),
            Err(EgressError::InvalidUrl)
        );
        assert_eq!(
            Egress::default().check_url("ftp://example.com/"),
            Err(EgressError::InvalidUrl)
        );
    }
}
