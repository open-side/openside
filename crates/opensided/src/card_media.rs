//! Credential-free, policy-gated raster materialization. Persist only normalized local pixels.
use crate::tools::{ToolEnv, ToolError};
use futures_util::{future::BoxFuture, StreamExt};
use openside_policy::{Action, Capability, Effect};
use openside_proto::{Block, Card, CardValue, PresentImage};
use std::{
    collections::HashMap,
    io::Cursor,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;

const MAX_SOURCE: usize = 2 * 1024 * 1024;
const MAX_REQUEST: usize = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 4_000_000;

fn denied() -> ToolError {
    ToolError("card_media_denied: images: source, type or path is not permitted".into())
}

fn source(raw: &str) -> Result<Url, ToolError> {
    let url = Url::parse(raw).map_err(|_| denied())?;
    if raw.len() > 2048
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || openside_memory::looks_secret(raw)
        || url.query_pairs().any(|(key, _)| {
            let key = key.to_ascii_lowercase();
            [
                "token",
                "secret",
                "password",
                "signature",
                "credential",
                "authorization",
                "api_key",
                "apikey",
                "sig",
            ]
            .iter()
            .any(|part| key.contains(part))
        })
    {
        return Err(denied());
    }
    Ok(url)
}

// Restrict IPv6 to ordinary global unicast; translated, mapped and transition ranges must
// not turn an apparently public destination into a private IPv4 socket at the network edge.
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || ip.octets() == [168, 63, 129, 16]
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

struct Response {
    status: u16,
    location: Option<String>,
    mime: String,
    bytes: Vec<u8>,
}
trait Transport: Sync {
    fn resolve<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<Vec<SocketAddr>, ToolError>>;
    fn get<'a>(
        &'a self,
        url: &'a Url,
        pin: SocketAddr,
        cap: usize,
    ) -> BoxFuture<'a, Result<Response, ToolError>>;
}
struct Http;
impl Transport for Http {
    fn resolve<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<Vec<SocketAddr>, ToolError>> {
        Box::pin(async move {
            let host = url.host_str().ok_or_else(denied)?.trim_matches(['[', ']']);
            let port = url.port_or_known_default().ok_or_else(denied)?;
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Ok(vec![SocketAddr::new(ip, port)]);
            }
            tokio::net::lookup_host((host, port))
                .await
                .map(|a| a.take(32).collect())
                .map_err(|_| denied())
        })
    }
    fn get<'a>(
        &'a self,
        url: &'a Url,
        pin: SocketAddr,
        cap: usize,
    ) -> BoxFuture<'a, Result<Response, ToolError>> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .no_proxy()
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .no_zstd()
                .redirect(reqwest::redirect::Policy::none())
                .referer(false)
                .timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(3))
                .resolve(url.host_str().ok_or_else(denied)?, pin)
                .build()
                .map_err(|_| denied())?;
            let response = client.get(url.clone()).header("Accept-Encoding", "identity")
                .header("Accept", "image/png,image/jpeg,image/webp,image/gif,image/x-icon,image/vnd.microsoft.icon")
                .send().await.map_err(|_| denied())?;
            if response.remote_addr() != Some(pin) {
                return Err(denied());
            }
            let status = response.status().as_u16();
            if [301, 302, 303, 307, 308].contains(&status) {
                return Ok(Response {
                    status,
                    location: response
                        .headers()
                        .get("location")
                        .and_then(|h| h.to_str().ok())
                        .map(str::to_owned),
                    mime: String::new(),
                    bytes: Vec::new(),
                });
            }
            let headers = response.headers();
            if headers
                .get("content-encoding")
                .is_some_and(|h| h != "identity")
                || response.content_length().is_some_and(|n| n > cap as u64)
            {
                return Err(denied());
            }
            let mime = headers
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !raster_mime(&mime) {
                return Err(denied());
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| denied())?;
                if bytes.len().saturating_add(chunk.len()) > cap {
                    return Err(denied());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Response {
                status,
                location: None,
                mime,
                bytes,
            })
        })
    }
}

fn raster_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/webp"
            | "image/gif"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
    )
}

async fn fetch_with(
    transport: &impl Transport,
    raw: &str,
    cap: usize,
    authorize: impl Fn(&Url) -> bool,
) -> Result<(Vec<u8>, String), ToolError> {
    let mut url = source(raw)?;
    for redirect in 0..=2 {
        if !authorize(&url) {
            return Err(denied());
        }
        let addresses = transport.resolve(&url).await?;
        if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
            return Err(denied());
        }
        // A new credential-free client gets precisely the checked socket, not another DNS lookup.
        let response = transport.get(&url, addresses[0], cap).await?;
        if [301, 302, 303, 307, 308].contains(&response.status) {
            if redirect == 2 {
                return Err(denied());
            }
            let next = url
                .join(response.location.as_deref().ok_or_else(denied)?)
                .map_err(|_| denied())?;
            if url.scheme() == "https" && next.scheme() != "https" {
                return Err(denied());
            }
            url = source(next.as_str())?;
            continue;
        }
        if !(200..300).contains(&response.status)
            || response.bytes.len() > cap
            || !raster_mime(&response.mime)
        {
            return Err(denied());
        }
        return Ok((response.bytes, response.mime));
    }
    Err(denied())
}

pub(crate) async fn fetch_normalized(
    raw: &str,
    cap: usize,
    favicon: bool,
    ctx: &ToolEnv,
) -> Result<(Vec<u8>, usize), ToolError> {
    let egress = openside_policy::Egress::new(&ctx.shared.config.loaded.config.network.allow)
        .map_err(|_| denied())?;
    let authorize = |url: &Url| {
        let value = serde_json::Value::String(url.as_str().into());
        ctx.shared
            .registry
            .redact(value.clone())
            .is_ok_and(|safe| safe == value)
            && egress.check_url(url.as_str()).is_ok()
            && ctx
                .policy
                .decide(&Action::new(Capability::Fetch, url.as_str()))
                .effect
                == Effect::Allow
    };
    let work = async {
        let (bytes, mime) = fetch_with(&Http, raw, cap, authorize).await?;
        let consumed = bytes.len();
        let png = tokio::task::spawn_blocking(move || normalize(&bytes, &mime, favicon))
            .await
            .map_err(|_| denied())??;
        Ok((png, consumed))
    };
    tokio::select! {
        _ = ctx.cancellation.cancelled() => Err(denied()),
        result = tokio::time::timeout(Duration::from_secs(10), work) => result.map_err(|_| denied())?,
    }
}

fn normalize(bytes: &[u8], mime: &str, favicon: bool) -> Result<Vec<u8>, ToolError> {
    use image::{GenericImageView, ImageFormat};
    let format = image::guess_format(bytes).map_err(|_| denied())?;
    let matches = matches!(
        (mime, format),
        ("image/png", ImageFormat::Png)
            | ("image/jpeg", ImageFormat::Jpeg)
            | ("image/gif", ImageFormat::Gif)
            | ("image/webp", ImageFormat::WebP)
            | (
                "image/x-icon" | "image/vnd.microsoft.icon",
                ImageFormat::Ico
            )
    );
    if !matches || bytes.len() > if favicon { 65536 } else { MAX_SOURCE } {
        return Err(denied());
    }
    let dimensions = image::io::Reader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| denied())?;
    let (width, height) = dimensions;
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || (!favicon && u64::from(width) * u64::from(height) > MAX_PIXELS)
    {
        return Err(denied());
    }
    let mut reader = image::io::Reader::with_format(Cursor::new(bytes), format);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(if favicon {
        80 * 1024 * 1024
    } else {
        32 * 1024 * 1024
    });
    reader.limits(limits);
    let mut image = reader.decode().map_err(|_| denied())?;
    if image.dimensions() != dimensions {
        return Err(denied());
    }
    if favicon {
        image = image.thumbnail(32, 32);
    }
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, ImageFormat::Png)
        .map_err(|_| denied())?;
    let bytes = out.into_inner();
    if bytes.len() > if favicon { 8192 } else { MAX_SOURCE } {
        return Err(denied());
    }
    Ok(bytes)
}

async fn persist(bytes: Vec<u8>, ctx: &ToolEnv) -> Result<String, ToolError> {
    let path = format!("cards/media/{}.png", crate::artifacts::sha256_hex(&bytes));
    crate::artifacts::save_tool_output(ctx, "present", &path, bytes)
        .await
        .map_err(|_| denied())?;
    Ok(path)
}

fn paths(card: &Card) -> Vec<String> {
    let mut paths = Vec::new();
    let mut value = |value: &CardValue| {
        if let CardValue::Image { image, .. } = value {
            paths.push(image.path.clone());
        }
    };
    for block in &card.blocks {
        match block {
            Block::KvRows { rows, total, .. } => {
                for row in rows {
                    value(&row.value);
                }
                if let Some(row) = total {
                    value(&row.value);
                }
            }
            Block::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        value(cell);
                    }
                }
            }
            _ => {}
        }
    }
    for block in &card.blocks {
        match block {
            Block::ListItems { items, .. } => {
                for item in items {
                    if let Some(image) = &item.image {
                        paths.push(image.path.clone());
                    }
                }
            }
            Block::Media { media, .. } => paths.push(media.path.clone()),
            _ => {}
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) async fn materialize(
    card: &mut Card,
    images: &[PresentImage],
    ctx: &ToolEnv,
) -> Result<(), ToolError> {
    if images.len() > 8 {
        return Err(denied());
    }
    let mut resolved = HashMap::<String, Option<String>>::new();
    let mut total = 0;
    // Sequential sources keep memory bounded and are below the two-request concurrency ceiling.
    for image in images {
        if image.key.is_empty()
            || image.key.len() > 64
            || !image
                .key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || resolved.contains_key(&format!("@{}", image.key))
        {
            return Err(denied());
        }
        source(&image.url)?;
        let cap = MAX_SOURCE.min(MAX_REQUEST - total);
        let result = if cap == 0 {
            Err(denied())
        } else {
            fetch_normalized(&image.url, cap, false, ctx).await
        };
        let path = if let Ok((bytes, consumed)) = result {
            total += consumed;
            Some(persist(bytes, ctx).await?)
        } else {
            // A failed stream may already have consumed its entire allowance.
            total += cap;
            None
        };
        resolved.insert(format!("@{}", image.key), path);
    }
    for path in paths(card) {
        if path.starts_with('@') {
            if !resolved.contains_key(&path) {
                return Err(denied());
            }
            continue;
        }
        let cap = MAX_SOURCE.min(MAX_REQUEST - total);
        if cap == 0 {
            return Err(denied());
        }
        let (bytes, mime) =
            crate::artifacts::card_image(&ctx.shared, &ctx.session, &path, cap as u64)
                .await
                .map_err(|_| denied())?;
        total += bytes.len();
        let bytes = tokio::task::spawn_blocking(move || normalize(&bytes, &mime, false))
            .await
            .map_err(|_| denied())??;
        let normalized = persist(bytes, ctx).await?;
        resolved.insert(path, Some(normalized));
    }
    fn fix(value: &mut CardValue, resolved: &HashMap<String, Option<String>>) {
        if let CardValue::Image { image, alt } = value {
            if let Some(Some(path)) = resolved.get(&image.path) {
                image.path.clone_from(path);
            } else {
                *value = CardValue::Text { text: alt.clone() };
            }
        }
    }
    for block in &mut card.blocks {
        match block {
            Block::ListItems { items, .. } => {
                for item in items {
                    if let Some(image) = &mut item.image {
                        if let Some(Some(path)) = resolved.get(&image.path) {
                            image.path.clone_from(path);
                        } else {
                            item.image = None;
                        }
                    }
                }
            }
            Block::Media { id, media, alt, .. } => {
                if let Some(Some(path)) = resolved.get(&media.path) {
                    media.path.clone_from(path);
                } else {
                    *block = Block::Notice {
                        id: id.clone(),
                        tone: openside_proto::NoticeTone::Neutral,
                        text: alt.clone(),
                        placement: openside_proto::NoticePlacement::Inline,
                    };
                }
            }
            Block::KvRows { rows, total, .. } => {
                for row in rows {
                    fix(&mut row.value, &resolved);
                }
                if let Some(row) = total {
                    fix(&mut row.value, &resolved);
                }
            }
            Block::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        fix(cell, &resolved);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    #[test]
    fn ssrf_address_matrix() {
        for ip in [
            "127.0.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.31.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.100.100.200",
            "224.1.1.1",
            "198.18.0.1",
            "::1",
            "::ffff:8.8.8.8",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "64:ff9b::a00:1",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
    struct Fake {
        responses: Mutex<Vec<Response>>,
        pins: Mutex<Vec<SocketAddr>>,
        resolutions: Mutex<usize>,
    }
    impl Transport for Fake {
        fn resolve<'a>(
            &'a self,
            url: &'a Url,
        ) -> BoxFuture<'a, Result<Vec<SocketAddr>, ToolError>> {
            Box::pin(async move {
                *self.resolutions.lock() += 1;
                let private = url.host_str() == Some("private.test")
                    || (url.host_str() == Some("rebind.test") && *self.resolutions.lock() > 1);
                Ok(vec![SocketAddr::new(
                    if private { "127.0.0.1" } else { "8.8.8.8" }
                        .parse()
                        .unwrap(),
                    443,
                )])
            })
        }
        fn get<'a>(
            &'a self,
            _: &'a Url,
            pin: SocketAddr,
            _: usize,
        ) -> BoxFuture<'a, Result<Response, ToolError>> {
            Box::pin(async move {
                self.pins.lock().push(pin);
                Ok(self.responses.lock().remove(0))
            })
        }
    }
    fn fake(responses: Vec<Response>) -> Fake {
        Fake {
            responses: Mutex::new(responses),
            pins: Mutex::new(Vec::new()),
            resolutions: Mutex::new(0),
        }
    }
    fn response() -> Response {
        Response {
            status: 200,
            location: None,
            mime: "image/png".into(),
            bytes: vec![1],
        }
    }
    #[tokio::test]
    async fn resolved_socket_is_pinned_without_a_second_dns_lookup() {
        let transport = fake(vec![response()]);
        assert!(
            fetch_with(&transport, "https://public.test/a", 1024, |_| true)
                .await
                .is_ok()
        );
        assert_eq!(*transport.resolutions.lock(), 1);
        assert_eq!(transport.pins.lock()[0], "8.8.8.8:443".parse().unwrap());
    }
    #[tokio::test]
    async fn redirects_revalidate_private_addresses_and_policy() {
        for target in [
            "https://private.test/a",
            "http://public.test/a",
            "https://denied.test/a",
        ] {
            let transport = fake(vec![Response {
                status: 302,
                location: Some(target.into()),
                mime: String::new(),
                bytes: Vec::new(),
            }]);
            assert!(
                fetch_with(&transport, "https://public.test/a", 1024, |u| u.host_str()
                    != Some("denied.test"))
                .await
                .is_err()
            );
            assert_eq!(transport.pins.lock().len(), 1);
        }
    }
    #[tokio::test]
    async fn dns_rebinding_on_a_same_host_redirect_is_refused() {
        let transport = fake(vec![Response {
            status: 302,
            location: Some("/next".into()),
            mime: String::new(),
            bytes: Vec::new(),
        }]);
        assert!(
            fetch_with(&transport, "https://rebind.test/a", 1024, |_| true)
                .await
                .is_err()
        );
        assert_eq!(*transport.resolutions.lock(), 2);
        assert_eq!(transport.pins.lock().len(), 1);
    }

    #[tokio::test]
    async fn redirect_limit_is_two_hops() {
        let transport = fake(
            (0..3)
                .map(|_| Response {
                    status: 302,
                    location: Some("/next".into()),
                    mime: String::new(),
                    bytes: Vec::new(),
                })
                .collect(),
        );
        assert!(
            fetch_with(&transport, "https://public.test/a", 1024, |_| true)
                .await
                .is_err()
        );
        assert_eq!(transport.pins.lock().len(), 3);
    }
    #[tokio::test]
    async fn oversize_and_non_raster_mime_are_refused() {
        let transport = fake(vec![response()]);
        assert!(fetch_with(&transport, "https://public.test/a", 0, |_| true)
            .await
            .is_err());
        let transport = fake(vec![Response {
            mime: "text/html".into(),
            ..response()
        }]);
        assert!(
            fetch_with(&transport, "https://public.test/a", 1024, |_| true)
                .await
                .is_err()
        );
    }
    #[test]
    fn signatures_credentials_and_active_formats_are_refused() {
        assert!(normalize(b"<svg/>", "image/png", false).is_err());
        assert!(normalize(&vec![0; MAX_SOURCE + 1], "image/png", false).is_err());
        for url in [
            "file:///etc/passwd",
            "https://user:pass@example.test/a",
            "https://example.test/a?X-Amz-Signature=123",
            "https://example.test/a#secret",
        ] {
            assert!(source(url).is_err());
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0; width as usize * height as usize * 4])
                .unwrap();
        }
        bytes
    }

    #[test]
    fn raster_normalization_checks_mime_pixels_dimensions_and_favicon_size() {
        let small = png(16, 16);
        assert!(normalize(&small, "image/png", false).is_ok());
        assert!(normalize(&small, "image/jpeg", false).is_err());
        assert!(normalize(&png(4097, 1), "image/png", false).is_err());
        assert!(normalize(&png(2001, 2000), "image/png", false).is_err());
        let icon = normalize(&png(256, 256), "image/png", true).ok().unwrap();
        let data = format!(
            "data:image/png;base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &icon)
        );
        assert!(openside_browser::Browser::validated_favicon(&data).is_some());
    }
}
