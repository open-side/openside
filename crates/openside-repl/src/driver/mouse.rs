//! The pointer device: position, held buttons, HTML5 drag state, and the trusted mouse events
//! built on them. Every event is authorized right before it is sent (see [`Aim`]), so one
//! primitive serves locator actions (`click`, `hover`, `dragTo`) and `page.mouse`.

use std::time::Duration;

use serde_json::{json, Value};

use super::{
    error::{browser_error, OpError, OpResult},
    Driver,
};
use crate::dom::NodeRef;

/// Targets authorized during one operation with the role and name they had then.
pub(super) type Granted = Vec<(NodeRef, String, String)>;

/// How long the browser gets to announce that a pressed-and-moved pointer started an HTML5 drag.
const DRAG_START_WAIT: Duration = Duration::from_millis(40);

/// A content quad in top-viewport CSS pixels.
#[derive(Clone, Copy, Debug)]
pub(super) struct Quad(pub(super) [(f64, f64); 4]);

impl Quad {
    pub(super) fn center(&self) -> (f64, f64) {
        let (x, y) = self
            .0
            .iter()
            .fold((0.0, 0.0), |sum, p| (sum.0 + p.0, sum.1 + p.1));
        (x / 4.0, y / 4.0)
    }

    /// Smallest axis-aligned rectangle around the quad: `(x, y, width, height)`.
    pub(super) fn bounds(&self) -> (f64, f64, f64, f64) {
        let xs = self.0.map(|p| p.0);
        let ys = self.0.map(|p| p.1);
        let (left, right) = (min(&xs), max(&xs));
        let (top, bottom) = (min(&ys), max(&ys));
        (left, top, right - left, bottom - top)
    }

    fn area(&self) -> f64 {
        let p = &self.0;
        let twice: f64 = (0..4)
            .map(|i| p[i].0 * p[(i + 1) % 4].1 - p[(i + 1) % 4].0 * p[i].1)
            .sum();
        twice.abs() / 2.0
    }
}

fn min(values: &[f64; 4]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

fn max(values: &[f64; 4]) -> f64 {
    values.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// Pointer state of one execution. The browser-side state it mirrors is released when the
/// execution ends (see `Driver::release_input`).
#[derive(Default)]
pub(super) struct Pointer {
    pub(super) x: f64,
    pub(super) y: f64,
    /// Held buttons as the `buttons` bitmask of `Input.dispatchMouseEvent`.
    pub(super) buttons: i64,
    /// Payload of the HTML5 drag in progress, as announced by the browser.
    pub(super) drag: Option<Value>,
    /// Whether the browser reports drags instead of starting a system drag.
    pub(super) intercepting: bool,
}

/// One pointer event to send.
#[derive(Clone, Copy)]
pub(super) enum Event<'a> {
    Move,
    Down { button: &'a str, clicks: u64 },
    Up { button: &'a str, clicks: u64 },
}

/// Who an event is for. With a `target` the event must reach that element (or a descendant) and
/// policy judges the element; without one, policy judges whichever element is under the point.
pub(super) struct Aim<'a> {
    pub(super) target: Option<&'a NodeRef>,
    pub(super) description: &'a str,
    /// The document the caller authorized; events stop once another one is current.
    pub(super) loader: &'a str,
    /// Modifier bits of the action itself, on top of held keys.
    pub(super) modifiers: i64,
}

pub(super) fn button_bit(name: &str) -> OpResult<i64> {
    Ok(match name {
        "left" => 1,
        "right" => 2,
        "middle" => 4,
        "back" => 8,
        "forward" => 16,
        other => return Err(OpError::new(format!("unknown mouse button {other:?}"))),
    })
}

fn button_name(buttons: i64) -> &'static str {
    [
        (1, "left"),
        (2, "right"),
        (4, "middle"),
        (8, "back"),
        (16, "forward"),
    ]
    .into_iter()
    .find(|(bit, _)| buttons & bit != 0)
    .map_or("none", |(_, name)| name)
}

impl Driver {
    /// The first non-empty content quad of `node`, in top-viewport CSS pixels.
    pub(super) async fn node_quad(&self, node: &NodeRef) -> OpResult<Quad> {
        let reply = self
            .node_cmd(node, "DOM.getContentQuads", json!({}))
            .await?;
        let (dx, dy) = self.host_offset(node.host.as_deref()).await?;
        reply["quads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|quad| {
                let numbers = quad.as_array().filter(|numbers| numbers.len() == 8)?;
                let mut points = [(0.0, 0.0); 4];
                for (index, point) in points.iter_mut().enumerate() {
                    *point = (
                        numbers[2 * index].as_f64()? + dx,
                        numbers[2 * index + 1].as_f64()? + dy,
                    );
                }
                Some(Quad(points))
            })
            .find(|quad| quad.area() > 0.0)
            .ok_or_else(|| OpError::new("element has no box"))
    }

    /// CSS size of the viewport the pointer works in.
    pub(super) async fn viewport(&self) -> OpResult<(f64, f64)> {
        let metrics = self.cmd("Page.getLayoutMetrics", json!({})).await?;
        let size = &metrics["cssVisualViewport"];
        Ok((
            size["clientWidth"].as_f64().unwrap_or_default(),
            size["clientHeight"].as_f64().unwrap_or_default(),
        ))
    }

    /// Reject a point the pointer cannot reach, naming the viewport so the caller can adjust.
    pub(super) async fn require_inside(&self, x: f64, y: f64) -> OpResult<()> {
        let (width, height) = self.viewport().await?;
        if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < width && y < height {
            Ok(())
        } else {
            Err(OpError::new(format!(
                "point ({x}, {y}) is outside the {width}x{height} viewport"
            )))
        }
    }

    /// Authorize and send one pointer event; see [`Aim`]. `gate` off skips the per-node check
    /// (the origin is still checked): used for the moves of a drag in progress, where only the
    /// press and the drop reach their elements as actions.
    pub(super) async fn pointer_event(
        &self,
        aim: &Aim<'_>,
        event: Event<'_>,
        (x, y): (f64, f64),
        verb: &str,
        granted: &mut Granted,
        gate: bool,
    ) -> OpResult<()> {
        self.same_document(aim.loader).await?;
        if !gate {
            self.gate_pointer_origin(x, y).await?;
        } else if let Some(target) = aim.target {
            if let Err(covering) = self.receives(target, x, y).await? {
                return Err(OpError {
                    name: "TargetMoved",
                    message: format!(
                        "{} no longer receives pointer events at its point: it {covering}; stopped before the next input event",
                        aim.description
                    ),
                });
            }
            self.node_gate(target, verb, granted).await?;
        } else {
            let hit = self.hit_at(x, y).await?.ok_or_else(|| {
                OpError::new(format!(
                    "no element at ({x}, {y}); nothing to send the event to"
                ))
            })?;
            self.node_gate(&hit.node, verb, granted).await?;
        }
        self.send_pointer(aim.modifiers, event, x, y).await
    }

    async fn send_pointer(&self, extra: i64, event: Event<'_>, x: f64, y: f64) -> OpResult<()> {
        let modifiers = self.held.borrow().modifiers() | extra;
        let (mut buttons, drag) = {
            let pointer = self.pointer.borrow();
            (pointer.buttons, pointer.drag.clone())
        };
        match event {
            Event::Move => match drag {
                Some(data) => {
                    self.cmd(
                        "Input.dispatchDragEvent",
                        json!({"type":"dragOver","x":x,"y":y,"data":data,"modifiers":modifiers}),
                    )
                    .await?;
                }
                None => {
                    self.cmd(
                        "Input.dispatchMouseEvent",
                        json!({
                            "type":"mouseMoved","x":x,"y":y,"button":button_name(buttons),
                            "buttons":buttons,"modifiers":modifiers,
                        }),
                    )
                    .await?;
                    if buttons & 1 != 0 && self.pointer.borrow().intercepting {
                        let started = self
                            .browser
                            .take_intercepted_drag(&self.owner, &self.tab, DRAG_START_WAIT)
                            .await
                            .map_err(browser_error)?;
                        if let Some(data) = started {
                            self.cmd(
                                "Input.dispatchDragEvent",
                                json!({"type":"dragEnter","x":x,"y":y,"data":data,"modifiers":modifiers}),
                            )
                            .await?;
                            self.pointer.borrow_mut().drag = Some(data);
                        }
                    }
                }
            },
            Event::Down { button, clicks } => {
                let bit = button_bit(button)?;
                if bit == 1 && !self.pointer.borrow().intercepting {
                    self.browser
                        .intercept_drags(&self.owner, &self.tab, true)
                        .await
                        .map_err(browser_error)?;
                    self.pointer.borrow_mut().intercepting = true;
                }
                buttons |= bit;
                self.cmd(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type":"mousePressed","x":x,"y":y,"button":button,"buttons":buttons,
                        "clickCount":clicks,"modifiers":modifiers,
                    }),
                )
                .await?;
                self.pointer.borrow_mut().buttons = buttons;
            }
            Event::Up { button, clicks } => {
                let bit = button_bit(button)?;
                buttons &= !bit;
                if let Some(data) = drag {
                    self.cmd(
                        "Input.dispatchDragEvent",
                        json!({"type":"drop","x":x,"y":y,"data":data,"modifiers":modifiers}),
                    )
                    .await?;
                    self.pointer.borrow_mut().drag = None;
                }
                self.cmd(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type":"mouseReleased","x":x,"y":y,"button":button,"buttons":buttons,
                        "clickCount":clicks,"modifiers":modifiers,
                    }),
                )
                .await?;
                self.pointer.borrow_mut().buttons = buttons;
                if buttons & 1 == 0 && self.pointer.borrow().intercepting {
                    self.browser
                        .intercept_drags(&self.owner, &self.tab, false)
                        .await
                        .map_err(browser_error)?;
                    self.pointer.borrow_mut().intercepting = false;
                }
            }
        }
        let mut pointer = self.pointer.borrow_mut();
        pointer.x = x;
        pointer.y = y;
        Ok(())
    }

    pub(super) async fn gate_pointer_origin(&self, x: f64, y: f64) -> OpResult<()> {
        let hit = self.hit_at(x, y).await?.ok_or_else(|| {
            OpError::new(format!(
                "no element at ({x}, {y}); nothing to send the event to"
            ))
        })?;
        self.gate_node_page(&hit.node, None).await?;
        Ok(())
    }

    /// Wheel at the pointer's position (`page.mouse.wheel`), with the held modifiers.
    async fn wheel_here(&self, dx: f64, dy: f64) -> OpResult<()> {
        let (x, y, buttons) = {
            let pointer = self.pointer.borrow();
            (pointer.x, pointer.y, pointer.buttons)
        };
        self.gate_pointer_origin(x, y).await?;
        let modifiers = self.held.borrow().modifiers();
        self.cmd(
            "Input.dispatchMouseEvent",
            json!({
                "type":"mouseWheel","x":x,"y":y,"deltaX":dx,"deltaY":dy,
                "buttons":buttons,"modifiers":modifiers,
            }),
        )
        .await?;
        Ok(())
    }

    /// `page.mouse`: every event is authorized against the element under the pointer when it is
    /// sent, so the script can reach canvas content and custom widgets without a locator while
    /// policy still sees each target.
    pub(super) async fn mouse(&self, args: &Value) -> OpResult<Value> {
        let before = self.frame().await?;
        let loader = self.gate_page(None).await?;
        let aim = Aim {
            target: None,
            description: "the element under the pointer",
            loader: &loader,
            modifiers: 0,
        };
        let mut granted = Granted::new();
        let number = |name: &str| args[name].as_f64();
        let button = args["button"].as_str().unwrap_or("left");
        let clicks = args["clickCount"].as_u64().unwrap_or(1);
        if !(1..=3).contains(&clicks) {
            return Err(OpError::new("clickCount must be between 1 and 3"));
        }
        match args["action"].as_str() {
            Some("move") => {
                let (x, y) = (
                    number("x").unwrap_or(f64::NAN),
                    number("y").unwrap_or(f64::NAN),
                );
                self.require_inside(x, y).await?;
                let steps = args["steps"].as_u64().unwrap_or(1).clamp(1, 200);
                self.move_to(&aim, (x, y), steps, &mut granted).await?;
            }
            Some("down") => {
                let at = self.here();
                self.pointer_event(
                    &aim,
                    Event::Down { button, clicks },
                    at,
                    "click",
                    &mut granted,
                    true,
                )
                .await?;
            }
            Some("up") => {
                let at = self.here();
                self.pointer_event(
                    &aim,
                    Event::Up { button, clicks },
                    at,
                    "click",
                    &mut granted,
                    true,
                )
                .await?;
            }
            Some("click") => {
                let (x, y) = (
                    number("x").unwrap_or(f64::NAN),
                    number("y").unwrap_or(f64::NAN),
                );
                self.require_inside(x, y).await?;
                let delay = args["delay"].as_u64().unwrap_or_default();
                self.move_to(&aim, (x, y), 1, &mut granted).await?;
                for count in 1..=clicks {
                    self.pointer_event(
                        &aim,
                        Event::Down {
                            button,
                            clicks: count,
                        },
                        (x, y),
                        "click",
                        &mut granted,
                        true,
                    )
                    .await?;
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    self.pointer_event(
                        &aim,
                        Event::Up {
                            button,
                            clicks: count,
                        },
                        (x, y),
                        "click",
                        &mut granted,
                        true,
                    )
                    .await?;
                }
            }
            Some("wheel") => {
                let (dx, dy) = (
                    number("deltaX").unwrap_or_default(),
                    number("deltaY").unwrap_or_default(),
                );
                if !dx.is_finite() || !dy.is_finite() {
                    return Err(OpError::new("wheel deltas must be finite numbers"));
                }
                self.same_document(&loader).await?;
                self.wheel_here(dx, dy).await?;
            }
            other => {
                return Err(OpError {
                    name: "NotSupported",
                    message: format!("mouse.{} is not supported", other.unwrap_or_default()),
                })
            }
        }
        self.settle(before).await?;
        Ok(Value::Null)
    }

    pub(super) fn here(&self) -> (f64, f64) {
        let pointer = self.pointer.borrow();
        (pointer.x, pointer.y)
    }

    /// Move the pointer to `to` in `steps` equal moves from where it is now.
    pub(super) async fn move_to(
        &self,
        aim: &Aim<'_>,
        to: (f64, f64),
        steps: u64,
        granted: &mut Granted,
    ) -> OpResult<()> {
        let from = self.here();
        for step in 1..=steps {
            let progress = step as f64 / steps as f64;
            let at = (
                from.0 + (to.0 - from.0) * progress,
                from.1 + (to.1 - from.1) * progress,
            );
            let held = self.pointer.borrow().buttons != 0;
            // With a button down the moves are part of a drag or a press-and-hold; the element
            // that matters is the one under the release, which is authorized then.
            self.pointer_event(aim, Event::Move, at, "hover", granted, !held)
                .await?;
        }
        Ok(())
    }

    /// Abandon an HTML5 drag in progress: the page sees `dragleave`, nothing is dropped, and no
    /// release is sent, so the element under the pointer gets no `click` or `drop`.
    pub(super) async fn cancel_drag(&self) {
        let (x, y, drag) = {
            let pointer = self.pointer.borrow();
            (pointer.x, pointer.y, pointer.drag.clone())
        };
        if let Some(data) = drag {
            self.cleanup_cmd(
                "Input.dispatchDragEvent",
                json!({"type":"dragCancel","x":x,"y":y,"data":data,"modifiers":0}),
            )
            .await;
        }
        let intercepting = self.pointer.borrow().intercepting;
        if intercepting {
            let _ = self
                .browser
                .intercept_drags(&self.owner, &self.tab, false)
                .await;
        }
        let mut pointer = self.pointer.borrow_mut();
        pointer.drag = None;
        pointer.intercepting = false;
        pointer.buttons &= !1;
    }

    /// End-of-execution cleanup of input state. Only a drag is cancelled. Held keys and buttons
    /// are deliberately not released: a release is an input event (Space on a button, a mouse-up
    /// over a changed element) that policy has not authorized, and the browser keeps no key
    /// state beyond the modifiers each event carries.
    pub(crate) async fn release_input(&self) {
        self.cancel_drag().await;
    }
}
