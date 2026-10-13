use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_policy::Action;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{self, DaemonTool, ToolEnv, ToolError, ToolExec};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {
    title: String,
    markdown: String,
}

pub(crate) struct WriteDocument;

impl DaemonTool for WriteDocument {
    fn name(&self) -> &'static str {
        "write_document"
    }

    fn spec(&self) -> openside_core::ToolSpec {
        tools::spec::<Args>(self.name(), "Create a Markdown Output artifact (up to 256 KiB). Return the artifact path in your Task result so the front can attach it to a short message.", ApprovalTier::Read, false)
    }

    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }

    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move { tools::parse::<Args>(args).map(|_| Vec::new()) })
    }

    fn redact_args(&self, _: &Value) -> Value {
        json!({"title": "[REDACTED]", "markdown": "[REDACTED]"})
    }

    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let args: Args = match tools::parse(&args) {
                Ok(args) => args,
                Err(error) => return ToolExec::Failed(error.0),
            };
            if args.markdown.len() > openside_proto::MAX_DOCUMENT_BYTES {
                return ToolExec::Denied {
                    reason: "document_too_large".into(),
                    hint: Some("Shorten the Markdown document to at most 256 KiB".into()),
                };
            }
            if args.title.trim().is_empty()
                || args.title.chars().count() > 160
                || args.title.chars().any(char::is_control)
            {
                return ToolExec::Denied {
                    reason: "invalid_title".into(),
                    hint: Some(
                        "Use a nonempty title of at most 160 characters without control characters"
                            .into(),
                    ),
                };
            }
            for value in [&args.title, &args.markdown] {
                if tools::present::admit_document_text(value, ctx).is_err() {
                    return ToolExec::Denied { reason: "document_secret_detected".into(), hint: Some("Remove credentials, tokens, cookies and other secrets before writing the document".into()) };
                }
            }
            let task = ctx.subagent.as_deref().unwrap_or("session");
            let path = format!("documents/tasks/{task}/{}.md", ctx.tool_call_id);
            let bytes = args.markdown.len();
            match crate::artifacts::save_tool_output(
                ctx,
                self.name(),
                &path,
                args.markdown.into_bytes(),
            )
            .await
            {
                Ok(()) => ToolExec::Done {
                    for_model: json!({"artifact": path, "title": args.title, "bytes": bytes, "content_type": "text/markdown"}),
                    for_app: None,
                },
                Err(_) => {
                    ToolExec::Failed("Document could not be saved; try writing it again".into())
                }
            }
        })
    }
}
