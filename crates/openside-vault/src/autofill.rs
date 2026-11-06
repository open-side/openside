//! Person-chosen autofill rules ([`AutofillRules`]) checked by every fill decision, after the
//! item's own policy and origin checks, so a refusal never says more than they already do.

use url::{Host, Url};

use crate::{Denial, Error};

/// Where fills are allowed at all, independent of each item's policy and origins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutofillRules {
    /// Off refuses every fill (`autofill_off`).
    pub enabled: bool,
    /// Off refuses fills into `http:` pages other than `localhost` (`insecure_origin`).
    pub allow_insecure: bool,
    /// Normalized DNS names; a page on one of them or a subdomain is refused
    /// (`excluded_domain`).
    excluded: Vec<String>,
}

impl Default for AutofillRules {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_insecure: false,
            excluded: Vec::new(),
        }
    }
}

impl AutofillRules {
    /// Rules with excluded domains, each normalized by [`normalize_domain`]; an invalid one
    /// is [`Error::InvalidOrigin`].
    pub fn new(enabled: bool, allow_insecure: bool, excluded: &[String]) -> Result<Self, Error> {
        let mut domains = Vec::with_capacity(excluded.len());
        for domain in excluded {
            let domain = normalize_domain(domain)?;
            if !domains.contains(&domain) {
                domains.push(domain);
            }
        }
        Ok(Self {
            enabled,
            allow_insecure,
            excluded: domains,
        })
    }

    /// The normalized excluded domains, in the order given.
    pub fn excluded(&self) -> &[String] {
        &self.excluded
    }

    /// Why a fill into a page at `origin` is refused, if it is.
    pub fn refusal(&self, origin: &str) -> Option<Denial> {
        if !self.enabled {
            return Some(Denial::AutofillOff);
        }
        let Ok(url) = Url::parse(origin) else {
            return Some(Denial::OriginMismatch);
        };
        let Some(Host::Domain(host)) = url.host() else {
            return Some(Denial::OriginMismatch);
        };
        let host = host.to_ascii_lowercase();
        if self.excluded.iter().any(|domain| {
            host == *domain
                || host
                    .strip_suffix(domain.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'))
        }) {
            return Some(Denial::ExcludedDomain);
        }
        // Browsers treat localhost as a secure context; everything else on http is not.
        let local = host == "localhost" || host.ends_with(".localhost");
        if url.scheme() == "http" && !local && !self.allow_insecure {
            return Some(Denial::InsecureOrigin);
        }
        None
    }
}

/// `example.com` from `example.com`, `Example.COM.`, `https://example.com/path` or
/// `*.example.com`; IP literals, single labels other than `localhost`, and anything that is
/// not a DNS name are [`Error::InvalidOrigin`].
pub fn normalize_domain(value: &str) -> Result<String, Error> {
    let value = value.trim();
    let value = value.strip_prefix("*.").unwrap_or(value);
    if value.is_empty() || value.len() > 253 {
        return Err(Error::InvalidOrigin);
    }
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let url = Url::parse(&candidate).map_err(|_| Error::InvalidOrigin)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::InvalidOrigin);
    }
    let Some(Host::Domain(host)) = url.host() else {
        return Err(Error::InvalidOrigin);
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || (!host.contains('.') && host != "localhost")
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(Error::InvalidOrigin);
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_normalize_from_hosts_urls_and_wildcards_and_refuse_ip_and_junk() {
        for (input, expected) in [
            ("example.com", "example.com"),
            ("  Example.COM. ", "example.com"),
            ("https://login.example.com/path?q=1", "login.example.com"),
            ("*.example.com", "example.com"),
            ("http://localhost:8080", "localhost"),
            ("xn--bcher-kva.example", "xn--bcher-kva.example"),
        ] {
            assert_eq!(normalize_domain(input).unwrap(), expected, "{input}");
        }
        for input in [
            "",
            "com",
            "127.0.0.1",
            "[::1]",
            "https://user:pw@example.com",
            "exa mple.com",
            "-bad.example",
        ] {
            assert_eq!(
                normalize_domain(input),
                Err(Error::InvalidOrigin),
                "{input}"
            );
        }
    }

    #[test]
    fn refusals_follow_enabled_excluded_and_insecure_rules() {
        let rules =
            AutofillRules::new(true, false, &["Example.com".into(), "example.com".into()]).unwrap();
        assert_eq!(rules.excluded(), ["example.com"]);
        assert_eq!(
            rules.refusal("https://example.com"),
            Some(Denial::ExcludedDomain)
        );
        assert_eq!(
            rules.refusal("https://login.example.com:8443"),
            Some(Denial::ExcludedDomain)
        );
        assert_eq!(rules.refusal("https://notexample.com"), None);
        assert_eq!(
            rules.refusal("http://shop.test"),
            Some(Denial::InsecureOrigin)
        );
        assert_eq!(rules.refusal("http://localhost:4100"), None);
        assert_eq!(rules.refusal("http://login.localhost:4100"), None);
        let insecure = AutofillRules::new(true, true, &[]).unwrap();
        assert_eq!(insecure.refusal("http://shop.test"), None);
        let off = AutofillRules::new(false, true, &[]).unwrap();
        assert_eq!(off.refusal("https://shop.test"), Some(Denial::AutofillOff));
    }
}
