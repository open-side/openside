//! `browser_pointer` (B-4): viewport-coordinate click/type/scroll for pages the accessibility
//! snapshot cannot address (canvas UIs, unlabeled custom widgets), aimed with
//! `browser_screenshot`. The point is hit-tested before policy runs, so the same Browse and
//! Sensitive rules as `browser_act` apply to whatever element lies under it, on that element's
//! real origin; a point over an uninspectable cross-process frame always counts as sensitive.

use futures_util::future::BoxFuture;
use openside_browser::{PointHit, PointerAction};
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::{parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec};
use super::{browser_error, done, sensitive, Controller};

pub(crate) struct BrowserPointer;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum PointerKind {
    Click,
    DoubleClick,
    Type,
    Scroll,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PointerArgs {
    tab_id: String,
    action: PointerKind,
    /// CSS pixels from the left edge of the 1440x900 viewport (screenshot coordinates).
    x: f64,
    /// CSS pixels from the top edge of the 1440x900 viewport.
    y: f64,
    /// type: text inserted into the element under the point after clicking it. There is no
    /// submit; press the form's button with browser_act.
    text: Option<String>,
    /// scroll: horizontal wheel distance in CSS pixels.
    delta_x: Option<f64>,
    /// scroll: vertical wheel distance in CSS pixels (positive scrolls down).
    delta_y: Option<f64>,
}

impl PointerArgs {
    fn action(&self) -> Result<PointerAction, ToolError> {
        let (x, y) = (self.x, self.y);
        Ok(match self.action {
            PointerKind::Click => PointerAction::Click { x, y, clicks: 1 },
            PointerKind::DoubleClick => PointerAction::Click { x, y, clicks: 2 },
            PointerKind::Type => PointerAction::Type {
                x,
                y,
                text: self
                    .text
                    .clone()
                    .ok_or_else(|| ToolError("type requires text".into()))?,
            },
            PointerKind::Scroll => {
                let (delta_x, delta_y) = (self.delta_x.unwrap_or(0.0), self.delta_y.unwrap_or(0.0));
                if delta_x == 0.0 && delta_y == 0.0 {
                    return Err(ToolError("scroll requires delta_x or delta_y".into()));
                }
                PointerAction::Scroll {
                    x,
                    y,
                    delta_x,
                    delta_y,
                }
            }
        })
    }
}

/// Policy actions for a hit: Browse on the tab's origin and on the hit element's origin, plus
/// Sensitive on the hit element's origin when its labels (or an opaque frame) call for it.
fn required(tab_origin: &str, hit: &PointHit) -> Vec<Action> {
    let mut actions = vec![Action::new(Capability::Browse, tab_origin)];
    if hit.origin != tab_origin {
        actions.push(Action::new(Capability::Browse, &hit.origin));
    }
    if hit.opaque_frame
        || hit
            .labels
            .iter()
            .any(|label| sensitive(&label.name, &label.role))
    {
        actions.push(Action::new(Capability::Sensitive, &hit.origin));
    }
    actions
}

async fn hit(
    ctx: &ToolEnv,
    args: &PointerArgs,
) -> Result<(std::sync::Arc<openside_browser::Browser>, String, PointHit), ToolError> {
    let browser = ctx
        .shared
        .registry
        .browser
        .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
        .await?;
    let origin = Controller::origin(&browser, &ctx.session, &args.tab_id).await?;
    let hit = browser
        .hit_test(ctx.session.as_str(), &args.tab_id, args.x, args.y)
        .await
        .map_err(|error| match error {
            openside_browser::BrowserError::InvalidAction(reason) => ToolError(reason.into()),
            other => browser_error(other),
        })?;
    Ok((browser, origin, hit))
}

impl DaemonTool for BrowserPointer {
    fn name(&self) -> &'static str {
        "browser_pointer"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<PointerArgs>(
            self.name(),
            "Click, double-click, type or scroll at viewport coordinates (1440x900 CSS pixels, as in browser_screenshot) in a session-owned tab. Use only when browser_snapshot has no ref for the target, e.g. canvas-drawn controls. The element under the point gets the same checks as browser_act: payment, purchase, send and delete matches in its accessible name or role require human confirmation. Typed text is recorded as length only.",
            ApprovalTier::Write,
            true,
        )
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.registry.browser.available()
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: PointerArgs = parse(args)?;
            args.action()?;
            let (_, origin, hit) = hit(ctx, &args).await?;
            Ok(required(&origin, &hit))
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        let mut safe = args.clone();
        if let Some(object) = safe.as_object_mut() {
            if let Some(text) = object.remove("text") {
                object.insert(
                    "text_length".into(),
                    json!(text.as_str().map(str::len).unwrap_or(0)),
                );
            }
            object.retain(|key, _| {
                [
                    "tab_id",
                    "action",
                    "x",
                    "y",
                    "text_length",
                    "delta_x",
                    "delta_y",
                ]
                .contains(&key.as_str())
            });
        }
        safe
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            match execute(args, ctx).await {
                Ok(result) => result,
                Err(error) => ToolExec::Denied {
                    reason: error.0,
                    hint: Some(
                        "Verify tab ownership, take a fresh browser_screenshot and retry".into(),
                    ),
                },
            }
        })
    }
}

async fn execute(args: Value, ctx: &ToolEnv) -> Result<ToolExec, ToolError> {
    let args: PointerArgs = parse(&args)?;
    let action = args.action()?;
    // The approval covered what the point hit at policy time; refuse if it now needs more.
    let (browser, origin, hit) = hit(ctx, &args).await?;
    let approved = |needed: &Action| {
        ctx.approved_actions
            .iter()
            .any(|action| action.capability == needed.capability && action.target == needed.target)
    };
    if !required(&origin, &hit).iter().all(approved) {
        return Err(ToolError(
            "The page changed under that point; take a new screenshot and retry".into(),
        ));
    }
    let outcome = browser
        .pointer(ctx.session.as_str(), &args.tab_id, action, &hit)
        .await
        .map_err(|error| match error {
            openside_browser::BrowserError::InvalidAction(reason) => ToolError(reason.into()),
            other => browser_error(other),
        })?;
    let controller = &ctx.shared.registry.browser;
    if outcome.navigated {
        controller.snapshots.lock().await.remove(&args.tab_id);
    }
    controller
        .persist(&ctx.shared)
        .await
        .map_err(|_| ToolError("Browser state could not be saved".into()))?;
    let target = hit
        .labels
        .first()
        .map(|label| json!({"role": label.role, "name": label.name}));
    Ok(done(json!({
        "url": crate::browser::public_url(&outcome.url),
        "navigated": outcome.navigated,
        "hit": target,
    })))
}
