//! Screenshots of more than the viewport: the whole document, a clip, or one element.

use super::*;

/// Largest side of a capture in CSS pixels; Chromium cannot allocate textures beyond 16384.
const MAX_SIDE: f64 = 16384.0;
/// Largest capture area in CSS pixels, so a tall page cannot become a gigabyte of PNG.
const MAX_AREA: f64 = 40_000_000.0;

/// A rectangle in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenshotClip {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width; must be positive.
    pub width: f64,
    /// Height; must be positive.
    pub height: f64,
}

/// What a screenshot covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreenshotArea {
    /// The visible viewport, as [`Browser::screenshot`].
    Viewport,
    /// The whole scrollable document, limited to a sane size.
    FullPage,
    /// A rectangle in viewport coordinates: the same pixels a viewport screenshot and
    /// `browser_pointer` use.
    Clip(ScreenshotClip),
    /// A rectangle in document coordinates, origin at the top-left of the page.
    PageClip(ScreenshotClip),
    /// One element, by backend node id; it is scrolled into view first.
    Element(u64),
}

impl Browser {
    /// A small JPEG of an owned viewport; uses capture's focus-emulation cleanup.
    pub async fn thumbnail(&self, owner: &str, id: &str) -> Result<Vec<u8>, BrowserError> {
        let session = self.owned_session(owner, id).await?;
        let params = json!({
            "format": "jpeg", "quality": 35, "captureBeyondViewport": false,
            "clip": {"x": 0, "y": 0, "width": super::VIEWPORT.0,
                "height": super::VIEWPORT.1, "scale": 0.25},
        });
        tokio::spawn(Arc::clone(&self.inner).capture(session, params))
            .await
            .map_err(|error| BrowserError::Transport(error.to_string()))?
    }

    /// Capture `area` of a tab as PNG, with the same focus-emulation hygiene as
    /// [`Browser::screenshot`]. The caller has already checked that the tab is its own.
    pub async fn screenshot_area(
        &self,
        id: &str,
        area: ScreenshotArea,
    ) -> Result<Vec<u8>, BrowserError> {
        if area == ScreenshotArea::Viewport {
            return self.screenshot(id).await;
        }
        let session = self.session(id).await?;
        let metrics = self
            .inner
            .cdp
            .call(Some(&session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let content = &metrics["cssContentSize"];
        let (Some(width), Some(height)) = (content["width"].as_f64(), content["height"].as_f64())
        else {
            return Err(BrowserError::InvalidResponse("layout metrics"));
        };
        let offset = (
            metrics["cssVisualViewport"]["pageX"]
                .as_f64()
                .unwrap_or(0.0),
            metrics["cssVisualViewport"]["pageY"]
                .as_f64()
                .unwrap_or(0.0),
        );
        let clip = match area {
            ScreenshotArea::Viewport => unreachable!("returned above"),
            ScreenshotArea::FullPage => ScreenshotClip {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            ScreenshotArea::Clip(clip) => ScreenshotClip {
                x: clip.x + offset.0,
                y: clip.y + offset.1,
                ..clip
            },
            ScreenshotArea::PageClip(clip) => clip,
            ScreenshotArea::Element(node) => {
                let rect = self.element_rect(&session, node).await?;
                ScreenshotClip {
                    x: rect.x + offset.0,
                    y: rect.y + offset.1,
                    ..rect
                }
            }
        };
        let clip = fit(clip, width, height)?;
        let params = json!({
            "format": "png",
            "captureBeyondViewport": true,
            "clip": {"x": clip.x, "y": clip.y, "width": clip.width, "height": clip.height, "scale": 1},
        });
        tokio::spawn(Arc::clone(&self.inner).capture(session, params))
            .await
            .map_err(|error| BrowserError::Transport(error.to_string()))?
    }

    /// Border box of a node in viewport coordinates, after scrolling it into view.
    async fn element_rect(
        &self,
        session: &str,
        backend_node_id: u64,
    ) -> Result<ScreenshotClip, BrowserError> {
        // A node that cannot scroll (detached, display: none) fails the box model below.
        let _ = self
            .inner
            .cdp
            .call(
                Some(session),
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": backend_node_id}),
            )
            .await;
        let model = self
            .inner
            .cdp
            .call(
                Some(session),
                "DOM.getBoxModel",
                json!({"backendNodeId": backend_node_id}),
            )
            .await?;
        let quad: Vec<f64> = model["model"]["border"]
            .as_array()
            .map(|points| points.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if quad.len() != 8 {
            return Err(BrowserError::InvalidResponse("element box"));
        }
        let xs = || quad.iter().step_by(2).copied();
        let ys = || quad.iter().skip(1).step_by(2).copied();
        let (left, right) = (
            xs().fold(f64::INFINITY, f64::min),
            xs().fold(f64::NEG_INFINITY, f64::max),
        );
        let (top, bottom) = (
            ys().fold(f64::INFINITY, f64::min),
            ys().fold(f64::NEG_INFINITY, f64::max),
        );
        Ok(ScreenshotClip {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
}

/// Clamp a document-coordinate rectangle to the document and the size limits; an empty result
/// means the request asked for nothing that exists.
fn fit(
    clip: ScreenshotClip,
    page_width: f64,
    page_height: f64,
) -> Result<ScreenshotClip, BrowserError> {
    let finite = [
        clip.x,
        clip.y,
        clip.width,
        clip.height,
        page_width,
        page_height,
    ]
    .iter()
    .all(|value| value.is_finite());
    if !finite || clip.width <= 0.0 || clip.height <= 0.0 {
        return Err(BrowserError::InvalidAction("screenshot area has no size"));
    }
    let x = clip.x.max(0.0);
    let y = clip.y.max(0.0);
    let right = (clip.x + clip.width).min(page_width);
    let bottom = (clip.y + clip.height).min(page_height);
    let mut width = (right - x).min(MAX_SIDE);
    let mut height = (bottom - y).min(MAX_SIDE);
    if width < 1.0 || height < 1.0 {
        return Err(BrowserError::InvalidAction(
            "screenshot area lies outside the page",
        ));
    }
    if width * height > MAX_AREA {
        height = (MAX_AREA / width).floor();
        width = width.min(MAX_SIDE);
    }
    Ok(ScreenshotClip {
        x,
        y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(x: f64, y: f64, width: f64, height: f64) -> ScreenshotClip {
        ScreenshotClip {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn clips_are_clamped_to_the_document() {
        let fitted = fit(clip(-10.0, 90.0, 50.0, 50.0), 100.0, 100.0).unwrap();
        assert_eq!(fitted, clip(0.0, 90.0, 40.0, 10.0));
    }

    #[test]
    fn empty_or_outside_areas_are_refused() {
        assert!(fit(clip(0.0, 0.0, 0.0, 10.0), 100.0, 100.0).is_err());
        assert!(fit(clip(0.0, 0.0, f64::NAN, 10.0), 100.0, 100.0).is_err());
        assert!(fit(clip(200.0, 0.0, 10.0, 10.0), 100.0, 100.0).is_err());
    }

    #[test]
    fn tall_pages_are_capped() {
        let fitted = fit(clip(0.0, 0.0, 1440.0, 100_000.0), 1440.0, 100_000.0).unwrap();
        assert_eq!(fitted.height, MAX_SIDE.min((MAX_AREA / 1440.0).floor()));
        assert!(fitted.width * fitted.height <= MAX_AREA);
    }
}
