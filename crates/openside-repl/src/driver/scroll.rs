//! Bringing a node into view with trusted wheel input, the way a user scrolls: wheel events at a
//! point over the scrolling area that has to move, eased, repeated until the node's centre is
//! inside every scrolling area that clips it. A page script only *computes* where to wheel; the
//! scroll itself is input, so scroll handlers, lazy loaders and virtual lists see a real wheel.

use std::time::Duration;

use serde_json::{json, Value};

use super::{
    error::{OpError, OpResult},
    Driver,
};
use crate::dom::NodeRef;

/// Wheel attempts before the node is scrolled by the browser instead.
const ATTEMPTS: usize = 8;

/// Wheel delta of one eased step; larger scrolls are split.
const STEP_PIXELS: f64 = 240.0;

/// Page-side planner. For the node (`this`) it walks the scrolling ancestors from the inside
/// out and returns the wheel request for the innermost scroller that has to move: a point over
/// that scroller's visible part (clear of scrollers nested in it) and the deltas that bring the
/// subject's centre to the middle of that part. A scroller with no visible part is skipped and
/// the next one out must reveal it instead. `{done: true}` means the node's centre is already
/// inside every scroller; `null`, that there is nothing to plan from (no box).
pub(super) const PLAN: &str = r"function() {
    const rect = this.getBoundingClientRect();
    if (!(rect.width > 0 || rect.height > 0)) return null;
    const parentOf = (node) => node.assignedSlot || node.parentElement || (node.parentNode && node.parentNode.host) || null;
    const inner = [];
    for (let node = parentOf(this); node; node = parentOf(node)) {
        if (node === document.body || node === document.documentElement) continue;
        const style = getComputedStyle(node);
        const scrollsX = /(auto|scroll|overlay)/.test(style.overflowX) && node.scrollWidth > node.clientWidth;
        const scrollsY = /(auto|scroll|overlay)/.test(style.overflowY) && node.scrollHeight > node.clientHeight;
        if (scrollsX || scrollsY) inner.push({ node });
    }
    const root = document.documentElement;
    const viewport = { left: 0, top: 0, right: root.clientWidth, bottom: root.clientHeight };
    const levels = inner.concat([{ node: null }]);
    let clip = viewport;
    for (const level of levels.slice().reverse()) {
        if (level.node) {
            const box = level.node.getBoundingClientRect();
            level.border = { left: box.left, top: box.top, right: box.right, bottom: box.bottom };
            level.client = {
                left: box.left + level.node.clientLeft, top: box.top + level.node.clientTop,
                right: box.left + level.node.clientLeft + level.node.clientWidth,
                bottom: box.top + level.node.clientTop + level.node.clientHeight,
            };
        } else {
            level.border = level.client = viewport;
        }
        level.visible = {
            left: Math.max(clip.left, level.client.left), top: Math.max(clip.top, level.client.top),
            right: Math.min(clip.right, level.client.right), bottom: Math.min(clip.bottom, level.client.bottom),
        };
        clip = level.visible;
    }
    const composedContains = (outer, node) => {
        for (let current = node; current; current = parentOf(current)) if (current === outer) return true;
        return false;
    };
    const deepest = (x, y) => {
        let hit = document.elementFromPoint(x, y);
        while (hit && hit.shadowRoot) {
            const next = hit.shadowRoot.elementFromPoint(x, y);
            if (!next || next === hit) break;
            hit = next;
        }
        return hit;
    };
    let subject = { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
    const cx = (rect.left + rect.right) / 2, cy = (rect.top + rect.bottom) / 2;
    for (let index = 0; index < levels.length; index++) {
        const level = levels[index];
        const sx = (subject.left + subject.right) / 2, sy = (subject.top + subject.bottom) / 2;
        const region = level.client;
        const outX = sx < region.left || sx > region.right;
        const outY = sy < region.top || sy > region.bottom;
        if (!outX && !outY) continue;
        const visible = level.visible;
        if (visible.right - visible.left < 4 || visible.bottom - visible.top < 4) {
            subject = level.border;
            continue;
        }
        const deeper = levels.slice(0, index).map((entry) => entry.node);
        let point = null;
        for (const fx of [0.5, 0.25, 0.75, 0.1, 0.9]) {
            for (const fy of [0.5, 0.25, 0.75, 0.1, 0.9]) {
                const x = visible.left + (visible.right - visible.left) * fx;
                const y = visible.top + (visible.bottom - visible.top) * fy;
                const hit = deepest(x, y);
                if (!hit) continue;
                if (level.node && !composedContains(level.node, hit)) continue;
                if (deeper.some((scroller) => composedContains(scroller, hit))) continue;
                point = { x, y };
                break;
            }
            if (point) break;
        }
        if (!point) point = { x: (visible.left + visible.right) / 2, y: (visible.top + visible.bottom) / 2 };
        return {
            x: point.x, y: point.y, cx, cy,
            dx: outX ? sx - (visible.left + visible.right) / 2 : 0,
            dy: outY ? sy - (visible.top + visible.bottom) / 2 : 0,
        };
    }
    return { done: true, cx, cy };
}";

/// What the planner asked for, in the node's own frame coordinates.
struct Plan {
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    /// Planner centre of the node, to translate frame coordinates into top-viewport ones.
    cx: f64,
    cy: f64,
}

impl Driver {
    /// Run [`PLAN`] on `node`. `Ok(None)`: no scrolling is needed or none could be planned.
    async fn scroll_plan(&self, node: &NodeRef) -> OpResult<Option<Plan>> {
        let frame = match &node.frame {
            Some(frame) => frame.clone(),
            None => self.frame().await?["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };
        let world = self
            .host_cmd(
                node.host.as_deref(),
                "Page.createIsolatedWorld",
                json!({"frameId": frame, "worldName": "openside-scroll"}),
            )
            .await?;
        let resolved = self
            .node_cmd(
                node,
                "DOM.resolveNode",
                json!({"executionContextId": world["executionContextId"]}),
            )
            .await?;
        let Some(object) = resolved["object"]["objectId"].as_str().map(str::to_owned) else {
            return Ok(None);
        };
        let outcome = self
            .host_cmd(
                node.host.as_deref(),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object,
                    "functionDeclaration": PLAN,
                    "returnByValue": true,
                    "silent": true,
                }),
            )
            .await;
        let _ = self
            .host_cmd(
                node.host.as_deref(),
                "Runtime.releaseObject",
                json!({"objectId": object}),
            )
            .await;
        let value = outcome?["result"]["value"].clone();
        if !value.is_object() || value["done"] == true {
            return Ok(None);
        }
        let number = |name: &str| value[name].as_f64().unwrap_or_default();
        Ok(Some(Plan {
            x: number("x"),
            y: number("y"),
            dx: number("dx"),
            dy: number("dy"),
            cx: number("cx"),
            cy: number("cy"),
        }))
    }

    /// Wheel `(dx, dy)` over the top-viewport point `(x, y)` in eased steps, with no modifier
    /// keys: Ctrl+wheel would zoom the page instead of scrolling it.
    pub(super) async fn wheel_eased(&self, x: f64, y: f64, dx: f64, dy: f64) -> OpResult<()> {
        let steps = ((dx.abs().max(dy.abs()) / STEP_PIXELS).ceil() as usize).clamp(1, 8);
        let mut sent = (0.0, 0.0);
        for step in 1..=steps {
            let progress = step as f64 / steps as f64;
            let eased = 1.0 - (1.0 - progress).powi(3);
            let to = (dx * eased, dy * eased);
            self.gate_pointer_origin(x, y).await?;
            self.cmd(
                "Input.dispatchMouseEvent",
                json!({
                    "type": "mouseWheel", "x": x, "y": y,
                    "deltaX": to.0 - sent.0, "deltaY": to.1 - sent.1, "modifiers": 0,
                }),
            )
            .await?;
            sent = to;
            if step < steps {
                tokio::time::sleep(Duration::from_millis(12)).await;
            }
        }
        Ok(())
    }

    /// The node's centre once smooth scrolling has stopped moving it.
    async fn settled_center(&self, node: &NodeRef) -> OpResult<(f64, f64)> {
        let mut last = self.node_quad(node).await?.center();
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let now = self.node_quad(node).await?.center();
            if (now.0 - last.0).abs() < 0.5 && (now.1 - last.1).abs() < 0.5 {
                return Ok(now);
            }
            last = now;
        }
        Ok(last)
    }

    /// Scroll `node` until its centre is inside every scrolling area that clips it, by wheel.
    /// `loader` is the authorized document; the wheel is only sent while it is current. When the
    /// wheel cannot move the node (nothing scrollable under any usable point) the browser's own
    /// scroll-into-view is the last resort, so the hit test that follows decides.
    pub(super) async fn scroll_into_view(&self, node: &NodeRef, loader: &str) -> OpResult<()> {
        let mut stalled = 0;
        for _ in 0..ATTEMPTS {
            self.same_document(loader).await?;
            let Some(plan) = self.scroll_plan(node).await? else {
                return Ok(());
            };
            if plan.dx.abs() < 1.0 && plan.dy.abs() < 1.0 {
                return Ok(());
            }
            let before = self.node_quad(node).await?.center();
            // Planner coordinates are in the node's frame; shift them to the top viewport.
            let (x, y) = (plan.x + before.0 - plan.cx, plan.y + before.1 - plan.cy);
            self.gate_page(None).await?;
            self.wheel_eased(x, y, plan.dx, plan.dy).await?;
            let after = self.settled_center(node).await?;
            if (after.0 - before.0).abs() < 1.0 && (after.1 - before.1).abs() < 1.0 {
                stalled += 1;
                if stalled >= 2 {
                    break;
                }
            } else {
                stalled = 0;
            }
        }
        self.node_cmd(node, "DOM.scrollIntoViewIfNeeded", json!({}))
            .await
            .map(drop)
            .map_err(|_| OpError::new("element is not attached to the document"))
    }

    /// `locator.scrollIntoViewIfNeeded()`.
    pub(super) async fn scroll_action(&self, node: &NodeRef, loader: &str) -> OpResult<Value> {
        self.scroll_into_view(node, loader).await?;
        Ok(Value::Null)
    }
}
