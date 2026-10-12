//! Outbound connections to `http(s)://` origins: plain TCP for loopback tests, rustls with
//! the Mozilla root set otherwise. Shared by the relay client, the daemon host and the CLI's
//! direct HTTPS profiles.

use std::{pin::Pin, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, http::HeaderValue},
    WebSocketStream,
};
use url::Url;

use crate::Error;

/// Byte stream to an origin, TLS or not.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// Boxed connection returned by [`connect`].
pub type BoxIo = Pin<Box<dyn Io>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Validated `http(s)` origin: `https`, or `http` only to a loopback host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// `true` for `https`.
    pub tls: bool,
    /// Host name or IP literal (without brackets).
    pub host: String,
    /// Port, defaulted from the scheme.
    pub port: u16,
}

impl Origin {
    /// Parse `http(s)://host[:port][/]`; anything with credentials, a path, query or
    /// fragment is refused, and plain `http` must stay on loopback.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let url = Url::parse(text.trim()).map_err(|_| Error::InvalidUrl)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::InvalidUrl);
        }
        let tls = url.scheme() == "https";
        let host = match url.host() {
            Some(url::Host::Domain(domain)) => domain.to_owned(),
            Some(url::Host::Ipv4(ip)) => ip.to_string(),
            Some(url::Host::Ipv6(ip)) => ip.to_string(),
            None => return Err(Error::InvalidUrl),
        };
        if !tls && !is_loopback(&host) {
            return Err(Error::InvalidUrl);
        }
        let port = url.port_or_known_default().ok_or(Error::InvalidUrl)?;
        Ok(Self { tls, host, port })
    }

    /// `Host` header value (port omitted when it is the scheme default).
    pub fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if (self.tls && self.port == 443) || (!self.tls && self.port == 80) {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// `https://authority` or `http://authority`.
    pub fn http_url(&self) -> String {
        format!(
            "{}://{}",
            if self.tls { "https" } else { "http" },
            self.authority()
        )
    }

    fn ws_url(&self, path: &str) -> String {
        format!(
            "{}://{}{path}",
            if self.tls { "wss" } else { "ws" },
            self.authority()
        )
    }
}

/// `localhost` or a loopback IP literal.
pub fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

static TLS_CONFIG: std::sync::LazyLock<Result<Arc<rustls::ClientConfig>, String>> =
    std::sync::LazyLock::new(|| {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map(|builder| Arc::new(builder.with_root_certificates(roots).with_no_client_auth()))
        .map_err(|error| error.to_string())
    });

/// Open a byte stream to `origin` (TLS for `https`).
pub async fn connect(origin: &Origin) -> Result<BoxIo, Error> {
    let tcp = tokio::time::timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((origin.host.as_str(), origin.port)),
    )
    .await
    .map_err(|_| Error::Io("connection timed out".into()))?
    .map_err(|error| Error::Io(error.to_string()))?;
    let _ = tcp.set_nodelay(true);
    if !origin.tls {
        return Ok(Box::pin(tcp));
    }
    let name = rustls::pki_types::ServerName::try_from(origin.host.clone())
        .map_err(|_| Error::InvalidUrl)?;
    let tls = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_rustls::TlsConnector::from(TLS_CONFIG.clone().map_err(Error::Io)?).connect(name, tcp),
    )
    .await
    .map_err(|_| Error::Io("TLS handshake timed out".into()))?
    .map_err(|error| Error::Io(error.to_string()))?;
    Ok(Box::pin(tls))
}

/// WebSocket to `origin` + `path`, optionally with a bearer token. A refused upgrade maps
/// 401 to [`Error::Unauthorized`], 404 to [`Error::DaemonOffline`], 503 to
/// [`Error::TooManyClients`].
pub(crate) async fn websocket(
    origin: &Origin,
    path: &str,
    bearer: Option<&str>,
) -> Result<WebSocketStream<BoxIo>, Error> {
    let io = connect(origin).await?;
    let mut request = origin
        .ws_url(path)
        .into_client_request()
        .map_err(|_| Error::InvalidUrl)?;
    if let Some(token) = bearer {
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Error::Protocol("token is not a valid header value"))?;
        value.set_sensitive(true);
        request.headers_mut().insert("authorization", value);
    }
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(crate::noise::MAX_MESSAGE + 64))
        .max_frame_size(Some(crate::noise::MAX_MESSAGE + 64));
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::client_async_with_config(request, io, Some(config)),
    )
    .await
    {
        Err(_) => Err(Error::Io("WebSocket upgrade timed out".into())),
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
            Err(match response.status().as_u16() {
                401 => Error::Unauthorized,
                404 => Error::DaemonOffline,
                503 => Error::TooManyClients,
                status => Error::Io(format!("relay refused the connection ({status})")),
            })
        }
        Ok(Err(error)) => Err(Error::Io(error.to_string())),
    }
}
