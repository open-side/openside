//! Viewport-coordinate input (CUA fallback) for pages whose accessibility tree cannot address
//! the target, such as canvas-drawn controls. Every action hit-tests its point first, and the
//! action itself re-checks that the same element of the same document is still there, so an
//! approval given for one target never lands on another.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{required_string, ActOutcome, Browser};
use crate::BrowserError;

/// Fixed agent viewport; coordinates are CSS pixels inside it.
pub const VIEWPORT: (u32, u32) = (1440, 900);

/// Ancestors examined above the hit element when describing what a point targets.
const LABEL_DEPTH: usize = 6;

/// One accessibility node on the path from the hit element to the document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PointLabel {
    /// Accessibility role.
    pub role: String,
    /// Accessible name.
    pub name: String,
}

/// What a viewport point lands on.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PointHit {
    /// Security origin of the document that owns the hit element; for a point over a
    /// cross-process frame, that frame's origin.
    pub origin: String,
    /// Hit element first, then its ancestors (the document root excluded).
    pub labels: Vec<PointLabel>,
    /// The point lies over a frame whose content this tab cannot inspect, so its labels are
    /// unknown; callers must treat the target as sensitive.
    pub opaque_frame: bool,
    document: String,
    /// Identity of the child document the point lands in (frame id, loader id, URL), when it
    /// is not the top document. A cross-process frame's element stays put while the page
    /// inside navigates, so the top document and element alone cannot see that change.
    frame_document: Option<String>,
    backend_node: u64,
}

/// A coordinate action.
#[derive(Clone, Debug, PartialEq)]
pub enum PointerAction {
    /// Press and release the left button `clicks` times (1 = click, 2 = double click).
    Click {
        /// Viewport x.
        x: f64,
        /// Viewport y.
        y: f64,
        /// Click count.
        clicks: u8,
    },
    /// Click the point, then insert text into the element under it if it took focus. There is
    /// no Enter/submit: an implicit form submission would reach a submitter the hit-test never
    /// saw, so submitting goes through `browser_act` on the form's button.
    Type {
        /// Viewport x.
        x: f64,
        /// Viewport y.
        y: f64,
        /// Inserted text.
        text: String,
    },
    /// Turn the mouse wheel over the point.
    Scroll {
        /// Viewport x.
        x: f64,
        /// Viewport y.
        y: f64,
        /// Horizontal CSS pixels.
        delta_x: f64,
        /// Vertical CSS pixels.
        delta_y: f64,
    },
}

impl PointerAction {
    /// The point this action targets.
    pub fn point(&self) -> (f64, f64) {
        match self {
            Self::Click { x, y, .. } | Self::Type { x, y, .. } | Self::Scroll { x, y, .. } => {
                (*x, *y)
            }
        }
    }
}

fn check_point(x: f64, y: f64) -> Result<(), BrowserError> {
    let inside =
        |value: f64, limit: u32| value.is_finite() && value >= 0.0 && value < f64::from(limit);
    if inside(x, VIEWPORT.0) && inside(y, VIEWPORT.1) {
        Ok(())
    } else {
        Err(BrowserError::InvalidAction(
            "coordinates must lie inside the 1440x900 viewport",
        ))
    }
}

fn find_frame<'a>(tree: &'a Value, id: &str) -> Option<&'a Value> {
    if tree["frame"]["id"] == id {
        return Some(&tree["frame"]);
    }
    tree["childFrames"]
        .as_array()?
        .iter()
        .find_map(|child| find_frame(child, id))
}

impl Browser {
    /// Backend node of the top document's keyboard-focused element, if any. Read from the
    /// browser's accessibility tree: `document.activeElement` is a page-overridable property,
    /// so a script could name the approved field while native focus is on another one.
    async fn focused_node(&self, session: &str) -> Result<Option<u64>, BrowserError> {
        let tree = self
            .inner
            .cdp
            .call(Some(session), "Accessibility.getFullAXTree", json!({}))
            .await?;
        let focused = |node: &&Value| {
            node["properties"].as_array().is_some_and(|properties| {
                properties.iter().any(|property| {
                    property["name"] == "focused" && property["value"]["value"] == true
                })
            })
        };
        // A focused node inside a child frame is not in this tree; the frame element itself is
        // then reported (or nothing), which never equals the approved hit.
        Ok(tree["nodes"]
            .as_array()
            .and_then(|nodes| nodes.iter().rfind(focused))
            .and_then(|node| node["backendDOMNodeId"].as_u64()))
    }

    /// `DOM.getNodeForLocation` reads its coordinates as document coordinates (Chromium 152:
    /// at scroll offset 500 a viewport point above y=500 finds nothing), while input events
    /// use viewport coordinates. Adding the visual viewport's page offset makes the hit-test
    /// answer for the same point an input event at `(x, y)` reaches.
    pub(crate) async fn node_at(
        &self,
        session: &str,
        x: f64,
        y: f64,
        include_user_agent_shadow_dom: bool,
    ) -> Result<Value, BrowserError> {
        let metrics = self
            .inner
            .cdp
            .call(Some(session), "Page.getLayoutMetrics", json!({}))
            .await?;
        let offset = |axis: &str| metrics["cssVisualViewport"][axis].as_f64().unwrap_or(0.0);
        self.inner
            .cdp
            .call(
                Some(session),
                "DOM.getNodeForLocation",
                // ignorePointerEventsNone=false: hit-test the element that real pointer input
                // reaches, so a pointer-events:none cover cannot stand in for the receiver
                // beneath it.
                json!({
                    "x": (x + offset("pageX")).floor(),
                    "y": (y + offset("pageY")).floor(),
                    "includeUserAgentShadowDOM": include_user_agent_shadow_dom,
                    "ignorePointerEventsNone": false,
                }),
            )
            .await
    }

    /// [`Browser::node_at`] for an explicitly owned agent tab; the REPL's pointer checks use it.
    pub async fn node_at_point(
        &self,
        owner: &str,
        tab_id: &str,
        x: f64,
        y: f64,
    ) -> Result<Value, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.node_at(&session, x, y, false).await
    }

    async fn hit(&self, session: &str, x: f64, y: f64) -> Result<PointHit, BrowserError> {
        let cdp = &self.inner.cdp;
        let (document, top_origin) = self.document_identity(session).await?;
        let located = self.node_at(session, x, y, true).await?;
        let backend_node = located["backendNodeId"]
            .as_u64()
            .ok_or(BrowserError::InvalidResponse("hit-test node"))?;
        let tree = cdp
            .call(Some(session), "Page.getFrameTree", json!({}))
            .await?;
        let frame_origin = |id: &str| {
            find_frame(&tree["frameTree"], id)
                .and_then(|frame| frame["securityOrigin"].as_str())
                .filter(|origin| origin.contains("://"))
                .map(str::to_owned)
        };
        let frame_document = |id: &str| {
            let frame = find_frame(&tree["frameTree"], id)?;
            serde_json::to_string(&(
                frame["id"].as_str()?,
                frame["loaderId"].as_str()?,
                frame["url"].as_str()?,
            ))
            .ok()
        };
        let top_id = required_string(&tree["frameTree"]["frame"], "id")?.to_owned();
        let mut child_document = None;
        let mut origin = match located["frameId"].as_str() {
            Some(frame) if frame != top_id => {
                child_document = Some(
                    frame_document(frame)
                        .ok_or(BrowserError::InvalidResponse("hit-test frame document"))?,
                );
                frame_origin(frame).ok_or(BrowserError::InvalidResponse("hit-test frame origin"))?
            }
            _ => top_origin,
        };
        let described = cdp
            .call(
                Some(session),
                "DOM.describeNode",
                json!({"backendNodeId":backend_node}),
            )
            .await?;
        // A point over a cross-process iframe resolves to the iframe element itself.
        let mut opaque_frame = false;
        if let Some(content) = described["node"]["frameId"].as_str() {
            if matches!(
                described["node"]["nodeName"].as_str(),
                Some("IFRAME" | "FRAME")
            ) {
                opaque_frame = true;
                child_document = Some(
                    frame_document(content)
                        .ok_or(BrowserError::InvalidResponse("hit-test iframe document"))?,
                );
                // Fail closed: gating the parent origin is no substitute for the child's.
                origin = frame_origin(content)
                    .ok_or(BrowserError::InvalidResponse("hit-test iframe origin"))?;
            }
        }
        if !origin.contains("://") {
            return Err(BrowserError::InvalidResponse("hit-test origin is opaque"));
        }
        let partial = cdp
            .call(
                Some(session),
                "Accessibility.getPartialAXTree",
                json!({"backendNodeId":backend_node,"fetchRelatives":true}),
            )
            .await?;
        let nodes = partial["nodes"].as_array().cloned().unwrap_or_default();
        let text = |node: &Value, field: &str| {
            node[field]["value"]
                .as_str()
                .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default()
        };
        let mut labels = Vec::new();
        let mut current = nodes
            .iter()
            .find(|node| node["backendDOMNodeId"].as_u64() == Some(backend_node));
        while let Some(node) = current {
            let role = text(node, "role");
            if role == "RootWebArea" || labels.len() > LABEL_DEPTH {
                break;
            }
            let name = text(node, "name");
            if !(role.is_empty()
                || (matches!(role.as_str(), "none" | "generic") && name.is_empty()))
            {
                labels.push(PointLabel { role, name });
            }
            current = node["parentId"]
                .as_str()
                .and_then(|parent| nodes.iter().find(|candidate| candidate["nodeId"] == parent));
        }
        Ok(PointHit {
            origin,
            labels,
            opaque_frame,
            document,
            frame_document: child_document,
            backend_node,
        })
    }

    /// Describe what the viewport point `(x, y)` of an owned agent tab lands on.
    pub async fn hit_test(
        &self,
        owner: &str,
        tab_id: &str,
        x: f64,
        y: f64,
    ) -> Result<PointHit, BrowserError> {
        check_point(x, y)?;
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        self.hit(&session, x, y).await
    }

    /// Perform a coordinate action on an owned agent tab, only if its point still hits the
    /// element `expected` described (same document, element and origin).
    pub async fn pointer(
        &self,
        owner: &str,
        tab_id: &str,
        action: PointerAction,
        expected: &PointHit,
    ) -> Result<ActOutcome, BrowserError> {
        let (x, y) = action.point();
        check_point(x, y)?;
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        if &self.hit(&session, x, y).await? != expected {
            return Err(BrowserError::InvalidAction(
                "the page changed under that point; take a new screenshot",
            ));
        }
        let before = self.top_frame(&session).await?;
        let cleanup = std::sync::Arc::clone(&self.inner)
            .begin_focus(session.clone())
            .await?;
        let mouse = |kind: &str, extra: Value| {
            let mut event = json!({"type":kind,"x":x,"y":y});
            if let (Some(event), Some(extra)) = (event.as_object_mut(), extra.as_object()) {
                event.extend(extra.clone());
            }
            event
        };
        // Renderer script keeps running between input events: a handler for one event can
        // move, cover or relabel the target before the next. Every event is preceded by a
        // fresh hit-test that must still match what policy approved.
        let unchanged = || async {
            if &self.hit(&session, x, y).await? == expected {
                Ok(())
            } else {
                Err(BrowserError::InvalidAction(
                    "the page changed under that point; take a new screenshot",
                ))
            }
        };
        let acted = async {
            let cdp = &self.inner.cdp;
            let send = |event: Value| cdp.call(Some(&session), "Input.dispatchMouseEvent", event);
            // The renderer ran page script since the hit-test above (focus emulation, layout).
            unchanged().await?;
            send(mouse("mouseMoved", json!({}))).await?;
            let clicks = match &action {
                PointerAction::Click { clicks, .. } => (*clicks).max(1),
                PointerAction::Type { .. } => 1,
                PointerAction::Scroll { .. } => 0,
            };
            for count in 1..=clicks {
                for kind in ["mousePressed", "mouseReleased"] {
                    unchanged().await?;
                    send(mouse(kind, json!({"button":"left","clickCount":count}))).await?;
                }
            }
            match &action {
                PointerAction::Click { .. } => {}
                PointerAction::Type { text, .. } => {
                    unchanged().await?;
                    // Text goes only to the approved element itself; a click handler that
                    // moved focus elsewhere makes this fail rather than type into it.
                    if self.focused_node(&session).await? != Some(expected.backend_node) {
                        return Err(BrowserError::InvalidAction(
                            "focus is not on the element under that point; nothing was typed",
                        ));
                    }
                    cdp.call(Some(&session), "Input.insertText", json!({"text":text}))
                        .await?;
                }
                PointerAction::Scroll {
                    delta_x, delta_y, ..
                } => {
                    unchanged().await?;
                    send(mouse(
                        "mouseWheel",
                        json!({"deltaX":delta_x,"deltaY":delta_y}),
                    ))
                    .await?;
                }
            }
            Ok::<_, BrowserError>(())
        }
        .await;
        let reset = cleanup.reset().await;
        acted?;
        reset?;
        // Same settle rule as `act`: a handler may commit a navigation after the input returns.
        let grace = tokio::time::Instant::now() + Duration::from_millis(200);
        let after = loop {
            let frame = self.top_frame(&session).await?;
            if frame["loaderId"] != before["loaderId"] || frame["url"] != before["url"] {
                break self.wait_loaded(&session, None, false).await?;
            }
            if tokio::time::Instant::now() >= grace {
                break frame;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let navigated = after["loaderId"] != before["loaderId"] || after["url"] != before["url"];
        if navigated {
            if let Some(refs) = self.inner.state.lock().await.references.get_mut(tab_id) {
                refs.invalidate();
            }
        }
        Ok(ActOutcome {
            url: required_string(&after, "url")?.to_owned(),
            navigated,
        })
    }
}

/// Drag payloads announced by `Input.dragIntercepted`, by CDP session: while interception is on,
/// the browser reports a drag the page started instead of running a system drag.
static DRAGS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Value>>> =
    std::sync::LazyLock::new(Default::default);

fn drags() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Value>> {
    DRAGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn record_drag(session: &str, data: Value) {
    drags().insert(session.to_owned(), data);
}

impl Browser {
    /// Switch drag interception of an owned agent tab on or off, dropping any payload left over
    /// from an earlier drag.
    pub async fn intercept_drags(
        &self,
        owner: &str,
        tab_id: &str,
        enabled: bool,
    ) -> Result<(), BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        drags().remove(&session);
        self.inner
            .cdp
            .call(
                Some(&session),
                "Input.setInterceptDrags",
                json!({"enabled": enabled}),
            )
            .await?;
        Ok(())
    }

    /// The payload of the drag the page started since interception was switched on, waiting up
    /// to `wait` for the browser to announce it; `None` when no drag started.
    pub async fn take_intercepted_drag(
        &self,
        owner: &str,
        tab_id: &str,
        wait: Duration,
    ) -> Result<Option<Value>, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        let until = tokio::time::Instant::now() + wait;
        loop {
            if let Some(data) = drags().remove(&session) {
                return Ok(Some(data));
            }
            if tokio::time::Instant::now() >= until {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
