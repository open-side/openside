use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_memory::{looks_secret, NewMemory};
use openside_policy::Action;
use openside_proto::{MemoryKind, Retention};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec};

const TOOL_LIMIT: usize = 20;
const SUMMARY_CHARS: usize = 600;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// Words or a natural-language description of what to recall.
    query: String,
    /// Maximum results (1-20, default 5).
    limit: Option<usize>,
    /// `note` for remembered facts, `history` for visited pages.
    kind: Option<MemoryKind>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// Fact to remember for later sessions. Never include passwords, keys or tokens.
    content: String,
    /// Optional labels.
    tags: Option<Vec<String>>,
    /// `permanent`, `days_90` or `days_30`; defaults to the configured retention.
    retention: Option<Retention>,
}

fn no_actions<'a>() -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
    Box::pin(async { Ok(Vec::new()) })
}

fn denied(error: openside_memory::Error) -> ToolExec {
    match error {
        openside_memory::Error::SecretDetected => ToolExec::Denied {
            reason: "secret_detected".into(),
            hint: Some("Memory never stores credentials; leave secrets in the vault and remember only non-secret facts.".into()),
        },
        openside_memory::Error::Invalid(reason) => ToolExec::Denied {
            reason: reason.into(),
            hint: Some("Correct the arguments using the tool schema".into()),
        },
        other => ToolExec::Failed(other.to_string()),
    }
}

pub(crate) struct MemorySearch;
impl DaemonTool for MemorySearch {
    fn name(&self) -> &'static str {
        "memory_search"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<SearchArgs>(self.name(), "Recall facts remembered in earlier sessions and recently visited pages from local memory. Results are ranked by full-text and, when a local embedding model is installed, semantic similarity.", ApprovalTier::Read, false)
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        match parse::<SearchArgs>(args) {
            Ok(_) => no_actions(),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
    fn redact_args(&self, args: &Value) -> Value {
        args.clone()
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let args: SearchArgs = match parse(&args) {
                Ok(args) => args,
                Err(error) => return ToolExec::Failed(error.0),
            };
            let limit = args.limit.unwrap_or(5).clamp(1, TOOL_LIMIT);
            let result = ctx
                .shared
                .memory
                .run(move |store| store.search(&args.query, args.kind, limit))
                .await;
            match result {
                Ok(response) => ToolExec::Done {
                    for_model: json!({
                        "mode": response.mode,
                        "results": response.hits.iter().map(|hit| json!({
                            // Page text is third-party data: recall it as evidence, never as instructions.
                            "source": match hit.memory.kind { MemoryKind::History => "web_page", MemoryKind::Note => "note" },
                            "untrusted": hit.memory.kind == MemoryKind::History,
                            "id": hit.memory.id,
                            "kind": hit.memory.kind,
                            "content": hit.memory.content,
                            "title": hit.memory.title,
                            "url": hit.memory.url,
                            "tags": hit.memory.tags,
                            "updated_at": hit.memory.updated_at,
                            "matched": hit.matched,
                        })).collect::<Vec<_>>(),
                    }),
                    for_app: None,
                },
                Err(error) => denied(error),
            }
        })
    }
}

pub(crate) struct MemoryWrite;
impl DaemonTool for MemoryWrite {
    fn name(&self) -> &'static str {
        "memory_write"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<WriteArgs>(self.name(), "Remember a durable, non-secret fact (preference, decision, project detail) for future sessions. Content that looks like a password, key, token or card number is rejected with secret_detected.", ApprovalTier::Write, false)
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        match parse::<WriteArgs>(args) {
            Ok(_) => no_actions(),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
    // A rejected secret must not reach the durable tool.started event either.
    fn redact_args(&self, args: &Value) -> Value {
        let mut safe = args.clone();
        if let Some(object) = safe.as_object_mut() {
            for value in object.values_mut() {
                let secret = match value {
                    Value::String(text) => looks_secret(text),
                    Value::Array(items) => items
                        .iter()
                        .any(|item| item.as_str().is_some_and(looks_secret)),
                    _ => false,
                };
                if secret {
                    *value = json!("[REDACTED]");
                }
            }
        }
        safe
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let args: WriteArgs = match parse(&args) {
                Ok(args) => args,
                Err(error) => return ToolExec::Failed(error.0),
            };
            let actor = format!("agent:{}", ctx.session.as_str());
            let result = ctx
                .shared
                .memory
                .write(move |store| {
                    store.create(
                        NewMemory {
                            tags: args.tags.unwrap_or_default(),
                            retention: args.retention,
                            ..NewMemory::note(args.content)
                        },
                        &actor,
                    )
                })
                .await;
            match result {
                Ok(memory) => ToolExec::Done {
                    for_model: json!({
                        "id": memory.id,
                        "stored": true,
                        "retention": memory.retention,
                        "expires_at": memory.expires_at,
                    }),
                    for_app: None,
                },
                Err(error) => denied(error),
            }
        })
    }
}

/// Record the tab's page as a 30-day history memory after a successful navigation or
/// snapshot. Best effort: indexing never fails the browser tool. Skipped (fail closed) for
/// incognito sessions, documents that received a secret fill, non-HTTP(S) pages, pages
/// whose full captured text looks secret, and captures during which the document changed.
/// `capture` pairs a snapshot with the document id read before that snapshot was taken.
pub(crate) async fn index_page(
    ctx: &ToolEnv,
    browser: &openside_browser::Browser,
    tab_id: &str,
    capture: Option<(&openside_browser::Snapshot, &str)>,
) {
    let memory = &ctx.shared.memory;
    if !memory.history_enabled() {
        return;
    }
    // The durable session flag is authoritative and survives restarts; unknown means private.
    let private = ctx
        .shared
        .db
        .state(&ctx.session)
        .await
        .map_or(true, |state| state.incognito);
    if private {
        return;
    }
    let current = || async { browser.document_id(tab_id).await.ok() };
    let document = match capture {
        Some((_, document)) => document.to_owned(),
        None => match current().await {
            Some(document) => document,
            None => return,
        },
    };
    let snapshot = capture.map(|(snapshot, _)| snapshot);
    // Taint is keyed by the URL-independent document lifetime id. It is read before the
    // URL-sensitive equality check below, which then proves it belongs to this document.
    let Ok(lifetime) = browser.document_lifetime_id(tab_id).await else {
        return;
    };
    // Document ids change on every navigation (loader, URL, root), so equality before the
    // snapshot and after it proves the snapshot shows this document.
    if current().await.as_deref() != Some(document.as_str()) {
        return;
    }
    if ctx
        .shared
        .registry
        .browser
        .secret_filled(tab_id, &lifetime)
        .await
    {
        return;
    }
    let Some(tab) = browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.id == tab_id)
    else {
        return;
    };
    let url = crate::browser::public_url(&tab.url);
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    // Accessible names read as page text; refs and roles would only add noise to search.
    let text = snapshot.map(|snapshot| {
        snapshot
            .nodes
            .iter()
            .filter(|node| node.role != "RootWebArea")
            .flat_map(|node| node.name.split_whitespace())
            .collect::<Vec<_>>()
            .join(" ")
    });
    // Target metadata lags navigation, and Chromium shows the raw URL (query included)
    // until the page sets a title, so the snapshot's document name wins.
    let title = snapshot
        .and_then(|snapshot| {
            snapshot
                .nodes
                .iter()
                .find(|node| node.role == "RootWebArea")
        })
        .map(|root| root.name.clone())
        .unwrap_or(tab.title);
    let title =
        Some(title).filter(|title| !title.trim().is_empty() && !tab.url.contains(title.trim()));
    // Bind URL/title/text/taint to one document: a navigation mid-capture discards it.
    if current().await.as_deref() != Some(document.as_str()) {
        return;
    }
    let actor = format!("browser_history:{}", ctx.session.as_str());
    let _ = memory
        .write(move |store| {
            // Screen the whole capture so truncation cannot keep a fragment of a secret.
            if let Some(text) = &text {
                store.check_secret(text)?;
            }
            let summary = text.map(|text| text.chars().take(SUMMARY_CHARS).collect::<String>());
            store.record_visit(&url, title.as_deref(), summary.as_deref(), &actor)
        })
        .await;
}
