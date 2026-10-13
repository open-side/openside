use base64::Engine;
use serde_json::json;

use super::{Browser, BrowserError};

const MAX_FAVICON_BYTES: usize = 8192;
const PNG_PREFIX: &str = "data:image/png;base64,";

// Fetch in an isolated browser world, without credentials or redirects. Page scripts cannot
// replace fetch/canvas intrinsics, and neither cookies nor icon URLs reach daemon HTTP clients.
const FAVICON_JS: &str = r#"(async () => {
    if (!['http:', 'https:'].includes(location.protocol)) return null;
    const url = [...document.querySelectorAll('link[rel~=icon]')]
        .map(link => { try { return new URL(link.getAttribute('href') || '', location.href); } catch (_) { return null; } })
        .find(url => url && url.origin === location.origin && !url.username && !url.password) || new URL('/favicon.ico', location.origin);
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), 1500);
    let image;
    try {
        const response = await fetch(url.href, {credentials: 'omit', redirect: 'error', signal: abort.signal, cache: 'force-cache'});
        if (!response.ok || new URL(response.url).origin !== location.origin || !response.body) return null;
        const mime = (response.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
        if (!['image/png', 'image/x-icon', 'image/vnd.microsoft.icon', 'image/jpeg', 'image/gif', 'image/webp'].includes(mime)) return null;
        if (Number(response.headers.get('content-length')) > 65536) return null;
        const reader = response.body.getReader();
        const chunks = [];
        let size = 0;
        while (true) {
            const {done, value} = await reader.read();
            if (done) break;
            size += value.length;
            if (size > 65536) { await reader.cancel(); return null; }
            chunks.push(value);
        }
        if (!size) return null;
        image = await createImageBitmap(new Blob(chunks, {type: mime}));
        if (!image.width || !image.height || image.width > 4096 || image.height > 4096) return null;
        const canvas = document.createElement('canvas');
        const scale = Math.min(1, 32 / image.width, 32 / image.height);
        canvas.width = Math.max(1, Math.round(image.width * scale));
        canvas.height = Math.max(1, Math.round(image.height * scale));
        canvas.getContext('2d').drawImage(image, 0, 0, canvas.width, canvas.height);
        const data = canvas.toDataURL('image/png');
        return atob(data.split(',')[1]).length <= 8192 ? data : null;
    } catch (_) { return null; }
    finally { clearTimeout(timer); abort.abort(); if (image) image.close(); }
})()"#;

impl Browser {
    /// A bounded icon for an owned tab; absence is normal for pages without a usable icon.
    pub async fn favicon(&self, owner: &str, tab: &str) -> Result<Option<String>, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab).await?;
        let context = self.isolated_world(&session).await?;
        let value = self.inner.cdp.call(Some(&session), "Runtime.evaluate", json!({
            "expression": FAVICON_JS, "contextId": context, "returnByValue": true, "awaitPromise": true,
        })).await?;
        Ok(value["result"]["value"]
            .as_str()
            .filter(|data| valid_favicon(data))
            .map(str::to_owned))
    }

    /// Validates the bounded normalized PNG wire format before any consumer displays it.
    pub fn validated_favicon(data: &str) -> Option<Vec<u8>> {
        let encoded = data.strip_prefix(PNG_PREFIX)?;
        if encoded.len() > MAX_FAVICON_BYTES.div_ceil(3) * 4 {
            return None;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        if bytes.is_empty() || bytes.len() > MAX_FAVICON_BYTES {
            return None;
        }
        let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        decoder.set_limits(png::Limits { bytes: 64 * 1024 });
        let mut reader = decoder.read_info().ok()?;
        let info = reader.info();
        if !(1..=32).contains(&info.width)
            || !(1..=32).contains(&info.height)
            || info.animation_control.is_some()
            || reader.output_buffer_size() > 8192
        {
            return None;
        }
        let mut pixels = [0; 8192];
        reader.next_frame(&mut pixels).ok()?;
        reader.finish().ok()?;
        Some(bytes)
    }
}

fn valid_favicon(data: &str) -> bool {
    Browser::validated_favicon(data).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(width: u32, height: u32, size: usize) -> String {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width.max(1), height.max(1));
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![0; width.max(1) as usize * height.max(1) as usize * 4])
                .unwrap();
        }
        bytes.resize(size.max(bytes.len()), 0);
        format!(
            "{PNG_PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    #[test]
    fn favicon_contract_caps_size_dimensions_and_mime() {
        assert!(valid_favicon(&icon(32, 32, MAX_FAVICON_BYTES)));
        assert!(!valid_favicon(&icon(32, 32, MAX_FAVICON_BYTES + 1)));
        assert!(!valid_favicon(&icon(33, 32, 100)));
        assert!(!valid_favicon(&icon(32, 33, 100)));
        assert!(!valid_favicon(
            "https://example.test/favicon.png?secret=token"
        ));
        assert!(!valid_favicon("data:image/svg+xml;base64,PHN2Zz4="));
        assert!(!valid_favicon("data:image/png;base64,[REDACTED]"));
    }
}
