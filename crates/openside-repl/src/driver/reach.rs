//! Reaching a pointer target the way a user does, and acting on it: scroll it into view, wait
//! for it to hold still, require that the pointer event would land on it (reporting the element
//! that covers it otherwise), then send the event sequence of a click, a hover, or a drag.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{
    error::{OpError, OpResult},
    locate::{located, Located, Resolved},
    mouse::{button_bit, Aim, Event, Granted},
    Driver, CHANGED, STOPPED,
};
use crate::{
    dom::{Dom, NodeRef},
    keys,
};

/// Upper bound on `clickCount`: triple-click is the largest multi-click pages distinguish, and
/// the bound keeps one operation from becoming an unbounded event stream.
const MAX_CLICKS: u64 = 3;

/// Moves between the press and the release of a drag, so pages see a pointer path, not a jump.
const DRAG_STEPS: u64 = 8;

pub(super) const STABLE_BOX: &str = r"function () {
  const node = this;
  return new Promise((resolve) => {
    let last, unchanged = 0, samples = 0;
    const sample = () => {
      const rect = node.getBoundingClientRect();
      const box = [rect.x, rect.y, rect.width, rect.height];
      unchanged = last && box.every((value, i) => Math.abs(value - last[i]) <= 0.25) ? unchanged + 1 : 0;
      last = box;
      if (++samples >= 4) resolve(node.isConnected && unchanged >= 3);
      else requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
}";

/// A pointer action's options (`click`, `dblclick`, `hover`, `dragTo`).
pub(super) struct PointerOptions {
    pub(super) button: String,
    pub(super) delay: u64,
    pub(super) force: bool,
    pub(super) trial: bool,
    pub(super) modifiers: i64,
    pub(super) position: Option<(f64, f64)>,
}

impl PointerOptions {
    pub(super) fn parse(options: &Value) -> OpResult<Self> {
        let mut modifiers = 0;
        for name in options["modifiers"].as_array().into_iter().flatten() {
            let name = name.as_str().unwrap_or_default();
            modifiers |= keys::modifier_bit(name)
                .ok_or_else(|| OpError::new(format!("unknown modifier {name:?}")))?;
        }
        let position = match &options["position"] {
            Value::Object(point) => Some((
                point.get("x").and_then(Value::as_f64).unwrap_or_default(),
                point.get("y").and_then(Value::as_f64).unwrap_or_default(),
            )),
            _ => None,
        };
        let button = options["button"].as_str().unwrap_or("left").to_owned();
        button_bit(&button)?;
        Ok(Self {
            button,
            delay: options["delay"].as_u64().unwrap_or_default(),
            force: options["force"] == true,
            trial: options["trial"] == true,
            modifiers,
            position,
        })
    }
}

/// How many presses a `click`-family action sends: `None` for a hover.
pub(super) fn click_count(action: &str, options: &Value) -> OpResult<u64> {
    Ok(match action {
        "hover" => 0,
        "dblclick" => 2,
        _ => {
            let clicks = options["clickCount"].as_u64().unwrap_or(1);
            if !(1..=MAX_CLICKS).contains(&clicks) {
                return Err(OpError::new(format!(
                    "clickCount must be between 1 and {MAX_CLICKS}"
                )));
            }
            clicks
        }
    })
}

/// A failure that must end the operation rather than be retried as "not reachable yet".
fn fatal(error: &OpError) -> bool {
    error.name == "PolicyError" || error.message == STOPPED || error.message == CHANGED
}

/// Short description of the element that takes an event meant for another one.
fn describe(dom: &Dom, index: usize) -> String {
    let node = &dom.nodes[index];
    let mut text = format!("<{}", node.name);
    for name in ["id", "class", "role", "aria-label", "name", "type"] {
        if let Some(value) = node.attribute(name) {
            let shown: String = value.chars().take(40).collect();
            text.push_str(&format!(" {name}=\"{shown}\""));
        }
    }
    text.push('>');
    text
}

impl Driver {
    /// Whether a pointer event at `(x, y)` would reach `target` (or something inside it). `Err`
    /// carries why not, naming the element that would receive it instead.
    pub(super) async fn receives(
        &self,
        target: &NodeRef,
        x: f64,
        y: f64,
    ) -> OpResult<Result<(), String>> {
        let Some(hit) = self.hit_at(x, y).await? else {
            return Ok(Err("is outside the viewport".into()));
        };
        if hit.node == *target {
            return Ok(Ok(()));
        }
        let dom = self.document().await?;
        let Some(wanted) = dom.index_of(target) else {
            return Ok(Err("is no longer in the document".into()));
        };
        Ok(match dom.index_of(&hit.node) {
            Some(receiver) if dom.contains(wanted, receiver) => Ok(()),
            Some(receiver) => Err(format!(
                "is covered by {} that would receive the pointer event",
                describe(&dom, receiver)
            )),
            None => Err("is covered by an element that appeared since the page was read".into()),
        })
    }

    /// The point a pointer action on `node` would use, provided the event would reach it: the
    /// node is scrolled into view by wheel, its full box must hold still across rendering frames,
    /// hit target at the point must be the node or a descendant. `Err` is why not, for the
    /// caller's retry and timeout message. `force` skips the stability and hit-target checks.
    pub(super) async fn pointer_point(
        &self,
        node: &NodeRef,
        position: Option<(f64, f64)>,
        force: bool,
        loader: &str,
        deadline: Instant,
    ) -> OpResult<Result<(f64, f64), String>> {
        match self.scroll_into_view(node, loader).await {
            Err(error) if fatal(&error) => return Err(error),
            Err(_) => return Ok(Err("is not attached".into())),
            Ok(()) => {}
        }
        let point = |quad: &super::mouse::Quad| match position {
            Some((dx, dy)) => {
                let (x, y, _, _) = quad.bounds();
                (x + dx, y + dy)
            }
            None => quad.center(),
        };
        if !force {
            let Ok(quad) = self.node_quad(node).await else {
                return Ok(Err("has no box".into()));
            };
            let at = point(&quad);
            if let Err(reason) = self.receives(node, at.0, at.1).await? {
                return Ok(Err(reason));
            }
        }
        if !force && !self.stable_box(node, deadline).await? {
            return Ok(Err("is not stable".into()));
        }
        let Ok(quad) = self.node_quad(node).await else {
            return Ok(Err("has no box".into()));
        };
        let at = point(&quad);
        let (width, height) = self.viewport().await?;
        if at.0 < 0.0 || at.1 < 0.0 || at.0 >= width || at.1 >= height {
            return Ok(Err("is outside the viewport".into()));
        }
        if !force {
            if let Err(reason) = self.receives(node, at.0, at.1).await? {
                return Ok(Err(reason));
            }
        }
        Ok(Ok(at))
    }

    async fn stable_box(&self, node: &NodeRef, deadline: Instant) -> OpResult<bool> {
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
                json!({"frameId": frame, "worldName": "openside-pointer"}),
            )
            .await?;
        let resolved = self.node_cmd(node, "DOM.resolveNode", json!({"executionContextId": world["executionContextId"], "objectGroup": "openside-pointer"})).await?;
        let sample = self.host_cmd(
            node.host.as_deref(),
            "Runtime.callFunctionOn",
            json!({
                "objectId": resolved["object"]["objectId"],
                "functionDeclaration": STABLE_BOX, "awaitPromise": true, "returnByValue": true,
            }),
        );
        let result =
            tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), sample).await;
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            self.browser.host_command(
                &self.owner,
                &self.tab,
                node.host.as_deref(),
                "Runtime.releaseObjectGroup",
                json!({"objectGroup": "openside-pointer"}),
            ),
        )
        .await;
        match result {
            Ok(reply) => Ok(reply?["result"]["value"] == true),
            Err(_) => Ok(false),
        }
    }

    /// Wait until `located` is reachable by the pointer: resolved to one visible, enabled
    /// element whose point passes [`Driver::pointer_point`]. Re-resolves between attempts, so a
    /// page that re-renders the element is followed. Returns the element and its point.
    pub(super) async fn approach(
        &self,
        located: &Located,
        mut resolved: Resolved,
        deadline: Instant,
        options: &PointerOptions,
    ) -> OpResult<(Resolved, NodeRef, (f64, f64))> {
        loop {
            let node = resolved.dom.node_ref(resolved.found[0]);
            match self
                .pointer_point(
                    &node,
                    options.position,
                    options.force,
                    &resolved.loader,
                    deadline,
                )
                .await?
            {
                Ok(point) => return Ok((resolved, node, point)),
                Err(reason) if Instant::now() >= deadline => {
                    return Err(OpError::timeout(format!(
                        "Timeout exceeded waiting for {} to receive pointer events: it {reason}",
                        located.description
                    )))
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            resolved = self
                .single(
                    located,
                    deadline.saturating_duration_since(Instant::now()),
                    !options.force,
                    !options.force,
                )
                .await?;
        }
    }

    /// `click`, `dblclick`, `hover` (`clicks == 0`): reach the target, then move, press, release.
    pub(super) async fn pointer(
        &self,
        located: &Located,
        resolved: Resolved,
        deadline: Instant,
        verb: &str,
        clicks: u64,
        options: &PointerOptions,
    ) -> OpResult<()> {
        let mut resolved = resolved;
        let mut granted = Granted::new();
        let (resolved, node, at) = loop {
            let (next, node, at) = self.approach(located, resolved, deadline, options).await?;
            if options.trial {
                return Ok(());
            }
            let aim = Aim {
                target: (!options.force).then_some(&node),
                description: &located.description,
                loader: &next.loader,
                modifiers: options.modifiers,
            };
            // Before any press, a moved target can safely re-enter approach. Never retry a
            // partially delivered click or turn a policy denial into a transient failure.
            if !options.force && self.receives(&node, at.0, at.1).await?.is_err() {
                resolved = next;
                if Instant::now() >= deadline {
                    return Err(OpError::timeout("target moved before input"));
                }
                continue;
            }
            if let Err(error) = self
                .pointer_event(&aim, Event::Move, at, verb, &mut granted, true)
                .await
            {
                if error.name != "TargetMoved" {
                    return Err(error);
                }
                resolved = next;
                if Instant::now() >= deadline {
                    return Err(OpError::timeout("target moved before input"));
                }
                continue;
            }
            if !options.force && clicks > 0 && self.receives(&node, at.0, at.1).await?.is_err() {
                resolved = next;
                if Instant::now() >= deadline {
                    return Err(OpError::timeout("target moved before press"));
                }
                continue;
            }
            if clicks > 0 {
                if let Err(error) = self
                    .pointer_event(
                        &aim,
                        Event::Down {
                            button: options.button.as_str(),
                            clicks: 1,
                        },
                        at,
                        verb,
                        &mut granted,
                        true,
                    )
                    .await
                {
                    if error.name != "TargetMoved" {
                        return Err(error);
                    }
                    resolved = next;
                    if Instant::now() >= deadline {
                        return Err(OpError::timeout("target moved before press"));
                    }
                    continue;
                }
            }
            break (next, node, at);
        };
        let aim = Aim {
            target: (!options.force).then_some(&node),
            description: &located.description,
            loader: &resolved.loader,
            modifiers: options.modifiers,
        };
        let button = options.button.as_str();
        for count in 1..=clicks {
            if count > 1 {
                self.pointer_event(
                    &aim,
                    Event::Down {
                        button,
                        clicks: count,
                    },
                    at,
                    verb,
                    &mut granted,
                    true,
                )
                .await?;
            }
            if options.delay > 0 {
                tokio::time::sleep(Duration::from_millis(options.delay)).await;
            }
            self.pointer_event(
                &aim,
                Event::Up {
                    button,
                    clicks: count,
                },
                at,
                verb,
                &mut granted,
                true,
            )
            .await?;
        }
        Ok(())
    }

    /// `locator.dragTo(target)`: press on the source, move to the target, release there. Pages
    /// that implement HTML5 drag and drop get real `dragstart`/`dragover`/`drop`; pages that
    /// track the pointer get the mouse events.
    pub(super) async fn drag(
        &self,
        located: &Located,
        resolved: Resolved,
        destination: &Value,
        deadline: Instant,
        timeout: Duration,
        options: &Value,
    ) -> OpResult<()> {
        let to = self.target_locator(destination, timeout)?;
        let source_options = PointerOptions {
            position: point_option(&options["sourcePosition"]),
            ..PointerOptions::parse(options)?
        };
        let target_options = PointerOptions {
            position: point_option(&options["targetPosition"]),
            ..PointerOptions::parse(options)?
        };
        let (resolved, source, from) = self
            .approach(located, resolved, deadline, &source_options)
            .await?;
        let first = self
            .single(&to, timeout, true, !target_options.force)
            .await?;
        if source_options.trial {
            self.approach(&to, first, deadline, &target_options).await?;
            return Ok(());
        }
        let result = self
            .drag_between(
                located,
                &resolved,
                &source,
                from,
                &to,
                first,
                deadline,
                &source_options,
                &target_options,
            )
            .await;
        if result.is_err() {
            self.cancel_drag().await;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn drag_between(
        &self,
        located: &Located,
        resolved: &Resolved,
        source: &NodeRef,
        from: (f64, f64),
        to: &Located,
        first: Resolved,
        deadline: Instant,
        source_options: &PointerOptions,
        target_options: &PointerOptions,
    ) -> OpResult<()> {
        let mut granted = Granted::new();
        let aim = Aim {
            target: (!source_options.force).then_some(source),
            description: &located.description,
            loader: &resolved.loader,
            modifiers: source_options.modifiers,
        };
        self.pointer_event(&aim, Event::Move, from, "drag", &mut granted, true)
            .await?;
        self.pointer_event(
            &aim,
            Event::Down {
                button: "left",
                clicks: 1,
            },
            from,
            "drag",
            &mut granted,
            true,
        )
        .await?;
        // The target may need scrolling into view, which the button being down does not prevent.
        let (arrived, destination, end) =
            self.approach(to, first, deadline, target_options).await?;
        let moving = Aim {
            target: None,
            ..aim
        };
        self.move_to(&moving, end, DRAG_STEPS, &mut granted).await?;
        let drop = Aim {
            target: (!target_options.force).then_some(&destination),
            description: &to.description,
            loader: &arrived.loader,
            modifiers: target_options.modifiers,
        };
        self.pointer_event(
            &drop,
            Event::Up {
                button: "left",
                clicks: 1,
            },
            end,
            "drop on",
            &mut granted,
            true,
        )
        .await
    }

    /// The locator `dragTo` was given, as the page-side serialized it.
    fn target_locator(&self, destination: &Value, timeout: Duration) -> OpResult<Located> {
        located(&json!({
            "steps": destination["steps"],
            "description": destination["description"],
            "timeout": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        }))
    }
}

fn point_option(value: &Value) -> Option<(f64, f64)> {
    value.as_object().map(|point| {
        (
            point.get("x").and_then(Value::as_f64).unwrap_or_default(),
            point.get("y").and_then(Value::as_f64).unwrap_or_default(),
        )
    })
}
