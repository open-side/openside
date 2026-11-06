//! `generate_image`: the daemon admits the call and owns where its images land; the sidecar
//! runs the provider request on a per-call credential lease and hands each image back through
//! the internal `__image_store` operation, which saves it as an Output artifact.

use base64::Engine as _;
use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, ToolOutcome};
use openside_policy::Action;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{self, DaemonTool, ToolEnv, ToolError, ToolExec};

pub(crate) const NAME: &str = "generate_image";
pub(crate) const STORE: &str = "__image_store";
const MAX_PROMPT_CHARS: usize = 4000;
const MAX_IMAGES: u8 = 4;
/// Raw bytes per image; base64 plus the envelope must stay under the 4 MiB wire line.
const MAX_IMAGE_BYTES: usize = 2_800_000;

#[derive(Deserialize, JsonSchema, Clone, Copy)]
enum AspectRatio {
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "9:16")]
    Tall,
    #[serde(rename = "4:3")]
    Landscape,
    #[serde(rename = "3:4")]
    Portrait,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {
    /// What to draw: subject, style, composition and any exact text that must appear.
    prompt: String,
    /// Frame shape; omitted means 1:1.
    aspect_ratio: Option<AspectRatio>,
    /// Number of images, 1 to 4; omitted means 1.
    count: Option<u8>,
}

pub(crate) struct GenerateImage;

impl DaemonTool for GenerateImage {
    fn name(&self) -> &'static str {
        NAME
    }

    fn spec(&self) -> openside_core::ToolSpec {
        tools::spec::<Args>(
            self.name(),
            "Generate images from a text description with the configured image model. The images are saved as Output files shown in this conversation; mention what you made rather than describing every pixel.",
            ApprovalTier::Read,
            false,
        )
    }

    fn visible(&self, ctx: &ToolEnv) -> bool {
        ctx.shared.models.image_model().is_some()
    }

    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move { tools::parse::<Args>(args).map(|_| Vec::new()) })
    }

    fn redact_args(&self, args: &Value) -> Value {
        json!({
            "prompt": "[REDACTED]",
            "aspect_ratio": args.get("aspect_ratio").cloned().unwrap_or(Value::Null),
            "count": args.get("count").cloned().unwrap_or(Value::Null),
        })
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let args: Args = match tools::parse(&args) {
                Ok(args) => args,
                Err(error) => return ToolExec::Failed(error.0),
            };
            let Some(model) = ctx.shared.models.image_model() else {
                return ToolExec::Denied {
                    reason: "image_model_not_configured".into(),
                    hint: Some(
                        "Image generation is off or no image model is connected; change it in Settings › Models"
                            .into(),
                    ),
                };
            };
            if args.prompt.trim().is_empty() || args.prompt.chars().count() > MAX_PROMPT_CHARS {
                return ToolExec::Denied {
                    reason: "invalid_prompt".into(),
                    hint: Some(format!(
                        "Describe the image in 1 to {MAX_PROMPT_CHARS} characters"
                    )),
                };
            }
            let count = args.count.unwrap_or(1);
            if !(1..=MAX_IMAGES).contains(&count) {
                return ToolExec::Denied {
                    reason: "invalid_count".into(),
                    hint: Some(format!("Ask for 1 to {MAX_IMAGES} images")),
                };
            }
            // The prompt leaves the machine for the image provider.
            if tools::present::admit_document_text(&args.prompt, ctx).is_err() {
                return ToolExec::Denied {
                    reason: "prompt_secret_detected".into(),
                    hint: Some(
                        "Remove credentials, tokens and other secrets from the prompt".into(),
                    ),
                };
            }
            // Also refreshes the lease filter's image ids (`Policy::image_models`) and the
            // automatic pick, which may have moved to another image model since the check above.
            let catalog = crate::models_api::catalog(&ctx.shared).await;
            let model = ctx.shared.models.image_model().unwrap_or(model);
            let available = catalog.is_ok_and(|catalog| {
                catalog
                    .providers
                    .iter()
                    .flat_map(|provider| &provider.models)
                    .any(|candidate| {
                        candidate.id == model
                            && candidate.available
                            && candidate.kind == openside_proto::ModelKind::Image
                    })
            });
            if !available {
                return ToolExec::Denied {
                    reason: "image_model_unavailable".into(),
                    hint: Some("The chosen image model has no connected account right now".into()),
                };
            }
            ctx.shared.models.grant_image(
                &ctx.session,
                &ctx.tool_call_id,
                ctx.subagent.as_deref(),
                &model,
                usize::from(count),
            );
            let aspect_ratio = match args.aspect_ratio.unwrap_or(AspectRatio::Square) {
                AspectRatio::Square => "1:1",
                AspectRatio::Wide => "16:9",
                AspectRatio::Tall => "9:16",
                AspectRatio::Landscape => "4:3",
                AspectRatio::Portrait => "3:4",
            };
            ToolExec::Done {
                for_model: json!({
                    "status": "admitted",
                    "model": model,
                    "count": count,
                    "aspect_ratio": aspect_ratio,
                }),
                for_app: None,
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreArgs {
    tool_call_id: String,
    #[serde(default)]
    index: usize,
    #[serde(default)]
    data_base64: Option<String>,
    /// The final image of the call, or (without data) a failed call ending its grant.
    #[serde(default)]
    last: bool,
}

fn refused(reason: &str, hint: &str) -> ToolOutcome {
    ToolOutcome::Denied {
        reason: reason.into(),
        hint: Some(hint.into()),
    }
}

fn extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("jpg")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// `__image_store`: saves one generated image of an admitted `generate_image` call.
pub(crate) async fn store(env: &ToolEnv, subagent: Option<&str>, arguments: &str) -> ToolOutcome {
    let Ok(args) = serde_json::from_str::<StoreArgs>(arguments) else {
        return refused("invalid_image_arguments", "Send valid JSON arguments");
    };
    let models = &env.shared.models;
    let Some(data) = args.data_base64 else {
        if args.last {
            models.end_image(&env.session, &args.tool_call_id, subagent);
            return ToolOutcome::Done {
                for_model_json: json!({"ended": true}).to_string(),
            };
        }
        return refused("invalid_image_arguments", "Send data_base64 or last=true");
    };
    if data.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return refused("image_too_large", "Images must stay under 2.8 MB");
    }
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()) else {
        return refused("invalid_image_data", "Send standard base64 image bytes");
    };
    let Some(ext) = extension(&bytes) else {
        return refused(
            "invalid_image_data",
            "Only PNG, JPEG and WebP images are stored",
        );
    };
    if !models.claim_image(
        &env.session,
        &args.tool_call_id,
        subagent,
        args.index,
        args.last,
    ) {
        return refused(
            "image_not_admitted",
            "Store images only for an admitted generate_image call",
        );
    }
    let Ok(state) = env.shared.db.state(&env.session).await else {
        return ToolOutcome::Failed {
            message: "Session state is unavailable".into(),
        };
    };
    let mut output = ToolEnv::new(env.shared.clone(), &state);
    output.tool_call_id = args.tool_call_id.clone();
    output.subagent = subagent.map(str::to_owned);
    // Provider call ids may carry separators that are not path-safe.
    let stem: String = args
        .tool_call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(96)
        .collect();
    let path = format!("images/{stem}-{}.{ext}", args.index + 1);
    match crate::artifacts::save_tool_output(&output, NAME, &path, bytes).await {
        Ok(()) => ToolOutcome::Done {
            for_model_json: json!({"path": path}).to_string(),
        },
        Err(_) => ToolOutcome::Failed {
            message: "Image could not be saved".into(),
        },
    }
}
