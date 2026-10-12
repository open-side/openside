use std::path::Path;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::error::{ChannelError, Result};

/// Platform HTTP client: no proxy, no redirects (a redirect could carry the bot token to
/// another host), bounded request time, and every destination checked against the outbound
/// allow-list before the request is built.
pub(crate) struct Client {
    inner: reqwest::Client,
    egress: openside_policy::Egress,
}

/// `api_base` and the platform's own attachment hosts are the configured service; anything
/// else must be in `network.allow`. Loopback is always reachable.
pub(crate) fn client(
    api_base: &str,
    platform_hosts: &[&str],
    network_allow: &[String],
) -> Result<Client> {
    let base = openside_policy::Origin::parse(api_base)
        .ok_or_else(|| ChannelError::Config(format!("invalid api_base {api_base:?}")))?;
    let base = format!("{}://{}:{}", base.scheme(), base.host(), base.port());
    let entries = std::iter::once(base.as_str())
        .chain(platform_hosts.iter().copied())
        .chain(network_allow.iter().map(String::as_str));
    let egress = openside_policy::Egress::new(entries).map_err(ChannelError::Config)?;
    let inner = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(ChannelError::http)?;
    Ok(Client { inner, egress })
}

impl Client {
    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        url: impl AsRef<str>,
    ) -> Result<reqwest::RequestBuilder> {
        let url = url.as_ref();
        self.egress
            .check_url(url)
            .map_err(|error| ChannelError::Refused(error.to_string()))?;
        Ok(self.inner.request(method, url))
    }

    pub(crate) fn get(&self, url: impl AsRef<str>) -> Result<reqwest::RequestBuilder> {
        self.request(reqwest::Method::GET, url)
    }

    pub(crate) fn post(&self, url: impl AsRef<str>) -> Result<reqwest::RequestBuilder> {
        self.request(reqwest::Method::POST, url)
    }
}

/// Stream a response body into `dest` (created 0600), failing past `max_bytes`.
pub(crate) async fn save_body(
    response: reqwest::Response,
    dest: &Path,
    max_bytes: u64,
) -> Result<()> {
    if !response.status().is_success() {
        return Err(ChannelError::Platform(format!(
            "download failed (HTTP {})",
            response.status().as_u16()
        )));
    }
    if response.content_length().is_some_and(|len| len > max_bytes) {
        return Err(ChannelError::Refused(
            "audio attachment is too large".into(),
        ));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(dest).await?;
    let mut total = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ChannelError::http)?;
        total += chunk.len() as u64;
        if total > max_bytes {
            return Err(ChannelError::Refused(
                "audio attachment is too large".into(),
            ));
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Platform clients reach only their configured API, the platform's own hosts,
    /// `network.allow` and loopback; anything else is refused before a request exists.
    #[test]
    fn destinations_outside_the_service_and_network_allow_are_refused() {
        let allow = vec!["*.partner.example".to_owned()];
        let http = client(
            "https://api.chat.example/v1",
            &["https://cdn.chat.example"],
            &allow,
        )
        .unwrap();
        for url in [
            "https://API.chat.example/v1/send",
            "https://cdn.chat.example/a.ogg",
            "https://files.partner.example/x",
            "http://127.0.0.1:9/local",
        ] {
            assert!(http.get(url).is_ok(), "{url}");
        }
        for url in [
            "https://collector.example/steal",
            "http://api.chat.example/v1/send",
            "https://api.chat.example:8443/v1",
            "https://cdn.chat.example.evil/a",
            "not a url",
        ] {
            assert!(
                matches!(http.get(url), Err(ChannelError::Refused(_))),
                "{url}"
            );
        }
        assert!(client("https://api.chat.example", &[], &["bad/entry".to_owned()]).is_err());
    }
}
