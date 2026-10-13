use base64::Engine;
use serde_json::json;

use super::{Browser, BrowserError};

const MAX_FAVICON_BYTES: usize = 8192;
const PNG_PREFIX: &str = "data:image/png;base64,";

fn normalize_cached_icon(bytes: &[u8]) -> Option<String> {
    use std::io::Cursor;
    if bytes.is_empty() || bytes.len() > 65536 {
        return None;
    }
    let format = image::guess_format(bytes).ok()?;
    let mut reader = image::io::Reader::with_format(Cursor::new(bytes), format);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(80 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let image = if image.width() > 32 || image.height() > 32 {
        image.thumbnail(32, 32)
    } else {
        image
    };
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).ok()?;
    let bytes = output.into_inner();
    if bytes.len() > MAX_FAVICON_BYTES {
        return None;
    }
    Some(format!(
        "{PNG_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

impl Browser {
    /// Read a bounded icon already cached for an owned tab, without executing page code or fetching.
    pub async fn favicon(&self, owner: &str, tab: &str) -> Result<Option<String>, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab).await?;
        let tree = self
            .inner
            .cdp
            .call(Some(&session), "Page.getResourceTree", json!({}))
            .await?;
        let frame = &tree["frameTree"];
        let Some(page) = frame["frame"]["url"]
            .as_str()
            .and_then(|value| url::Url::parse(value).ok())
        else {
            return Ok(None);
        };
        let Some(resources) = frame["resources"].as_array() else {
            return Ok(None);
        };
        for resource in resources {
            let Some(url) = resource["url"]
                .as_str()
                .and_then(|value| url::Url::parse(value).ok())
            else {
                continue;
            };
            if !matches!(url.scheme(), "http" | "https")
                || url.origin() != page.origin()
                || !url.username().is_empty()
                || url.password().is_some()
                || resource["type"] != "Image"
                || !(url.path().to_lowercase().contains("favicon")
                    || matches!(
                        resource["mimeType"].as_str(),
                        Some("image/x-icon" | "image/vnd.microsoft.icon")
                    ))
            {
                continue;
            }
            let Ok(content) = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Page.getResourceContent",
                    json!({
                        "frameId": frame["frame"]["id"], "url": url.as_str(),
                    }),
                )
                .await
            else {
                continue;
            };
            let Some(encoded) = content["content"]
                .as_str()
                .filter(|value| value.len() <= 65536_usize.div_ceil(3) * 4)
            else {
                continue;
            };
            if content["base64Encoded"] != true {
                continue;
            }
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
                continue;
            };
            if let Some(icon) = tokio::task::spawn_blocking(move || normalize_cached_icon(&bytes))
                .await
                .ok()
                .flatten()
            {
                return Ok(Some(icon));
            }
        }
        Ok(None)
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

#[cfg(test)]
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

    #[test]
    fn cached_raster_normalization_bounds_input_and_output() {
        let input = icon(64, 64, 100);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(input.strip_prefix(PNG_PREFIX).unwrap())
            .unwrap();
        let normalized = normalize_cached_icon(&bytes).unwrap();
        assert!(valid_favicon(&normalized));
        assert!(normalize_cached_icon(&vec![0; 65537]).is_none());
        assert!(normalize_cached_icon(b"<svg/>").is_none());
    }
}
