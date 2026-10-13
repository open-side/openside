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
    /// `permanent`, `days_90`, `days_30` or `hours_24`; scratch-tagged notes always expire in 24 hours.
    retention: Option<Retention>,
    #[serde(default)]
    shared: bool,
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
            let state = match ctx.shared.db.state(&ctx.session).await {
                Ok(state) => state,
                Err(_) => return ToolExec::Failed("Session state is unavailable".into()),
            };
            let owner = state.agent_id;
            let result = ctx
                .shared
                .memory
                .run(move |store| {
                    let response = store.search_scoped(
                        &args.query,
                        args.kind,
                        limit,
                        Some(owner.as_deref()),
                        true,
                    )?;
                    let authors = response
                        .hits
                        .iter()
                        .map(|hit| store.last_actor(&hit.memory.id))
                        .collect::<openside_memory::Result<Vec<_>>>()?;
                    Ok((response, authors))
                })
                .await;
            match result {
                Ok((response, authors)) => ToolExec::Done {
                    for_model: json!({
                        "mode": response.mode,
                        "results": response.hits.iter().zip(authors).map(|(hit, author)| json!({
                            "source": match hit.memory.kind { MemoryKind::History => "web_page", MemoryKind::Note => "note" },
                            "untrusted": true,
                            "authority": "data_only",
                            "owner": hit.memory.owner,
                            "author": author,
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
            let state = match ctx.shared.db.state(&ctx.session).await {
                Ok(state) => state,
                Err(_) => return ToolExec::Failed("Session state is unavailable".into()),
            };
            let _gate = ctx.shared.agents.gate.lock().await;
            if let Some(id) = state.agent_id.as_deref() {
                match ctx.shared.agents.get(id) {
                    Ok(Some(agent)) if agent.status == openside_proto::AgentStatus::Active => {}
                    _ => {
                        return ToolExec::Denied {
                            reason: "agent_retired".into(),
                            hint: None,
                        }
                    }
                }
            }
            let owner = state.agent_id.filter(|id| id != "chief");
            if args.shared && owner.is_some() {
                return ToolExec::Denied {
                    reason: "forbidden".into(),
                    hint: None,
                };
            }
            let result = ctx
                .shared
                .memory
                .write(move |store| {
                    store.create_owned(
                        NewMemory {
                            tags: args.tags.unwrap_or_default(),
                            retention: args.retention,
                            ..NewMemory::note(args.content)
                        },
                        &actor,
                        owner.as_deref(),
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
/// incognito and isolated-space sessions, documents that received a secret fill, non-HTTP(S) pages, pages
/// whose full captured text looks secret, and captures during which the document changed.
/// `capture` pairs a snapshot with the document id read before that snapshot was taken.
pub(crate) async fn index_page(
    ctx: &ToolEnv,
    browser: &openside_browser::Browser,
    tab_id: &str,
    capture: Option<(&openside_browser::Snapshot, &str)>,
) {
    let Some((snapshot, document)) = capture else {
        return;
    };
    let Some(text) = history_text(snapshot) else {
        return;
    };
    let memory = &ctx.shared.memory;
    if !memory.history_enabled() {
        return;
    }
    // The durable session flags are authoritative and survive restarts; unknown means private.
    // An isolated space is wiped on archive, so its pages must not outlive it in memory either.
    let private = ctx
        .shared
        .db
        .state(&ctx.session)
        .await
        .map_or(true, |state| {
            state.incognito || state.browser_space == openside_proto::BrowserSpace::Isolated
        });
    if private {
        return;
    }
    let current = || async { browser.document_id(tab_id).await.ok() };
    // Taint is keyed by the URL-independent document lifetime id. It is read before the
    // URL-sensitive equality check below, which then proves it belongs to this document.
    let Ok(lifetime) = browser.document_lifetime_id(tab_id).await else {
        return;
    };
    // Document ids change on every navigation (loader, URL, root), so equality before the
    // snapshot and after it proves the snapshot shows this document.
    if current().await.as_deref() != Some(document) {
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
    let url = crate::browser::display_url(&tab.url);
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    // Target metadata lags navigation, and Chromium shows the raw URL (query included)
    // until the page sets a title, so the snapshot's document name wins.
    let title = snapshot
        .nodes
        .iter()
        .find(|node| node.role == "RootWebArea")
        .map(|root| root.name.clone())
        .unwrap_or(tab.title);
    let title =
        Some(title).filter(|title| !title.trim().is_empty() && !tab.url.contains(title.trim()));
    // Bind URL/title/text/taint to one document: a navigation mid-capture discards it.
    if current().await.as_deref() != Some(document) {
        return;
    }
    let actor = format!("browser_history:{}", ctx.session.as_str());
    let _ = memory
        .write(move |store| {
            // Screen the whole capture so truncation cannot keep a fragment of a secret.
            store.check_secret(&text)?;
            let summary = text.chars().take(SUMMARY_CHARS).collect::<String>();
            store.record_visit(&url, title.as_deref(), Some(&summary), &actor)
        })
        .await;
}

fn history_text(snapshot: &openside_browser::Snapshot) -> Option<String> {
    if snapshot.nodes.iter().any(|node| {
        if matches!(node.role.as_str(), "textbox" | "searchbox") && node.value == "<redacted>" {
            return true;
        }
        if !matches!(node.role.as_str(), "RootWebArea" | "form" | "textbox") {
            return false;
        }
        let name = node.name.to_lowercase();
        [
            "password",
            "passcode",
            "sign in",
            "log in",
            "login",
            "비밀번호",
            "로그인",
        ]
        .iter()
        .any(|label| name.contains(label))
    }) {
        return None;
    }
    let mut excluded_depth = None;
    let mut eligible = Vec::new();
    let mut prose_chars = 0;
    let mut other_chars = 0;
    for node in &snapshot.nodes {
        if excluded_depth.is_some_and(|depth| node.depth <= depth) {
            excluded_depth = None;
        }
        if matches!(
            node.role.as_str(),
            "navigation"
                | "banner"
                | "contentinfo"
                | "complementary"
                | "advertisement"
                | "link"
                | "button"
                | "menu"
                | "menubar"
                | "form"
                | "heading"
        ) {
            excluded_depth.get_or_insert(node.depth);
        }
        let chars = node.name.chars().filter(|ch| ch.is_alphanumeric()).count();
        if excluded_depth.is_none() && matches!(node.role.as_str(), "StaticText" | "paragraph") {
            prose_chars += chars;
            eligible.push(node.name.as_str());
        } else if node.role != "RootWebArea" {
            other_chars += chars;
        }
    }
    // Titles, account menus and advertising chrome are not durable visit evidence.
    if prose_chars < 80 || prose_chars < other_chars {
        return None;
    }
    Some(eligible.join(" "))
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use openside_browser::{Snapshot, SnapshotNode};

    const PROSE: &str = "Plant tomato seedlings after the last frost. Keep the roots moist and give each plant enough space and sunlight throughout the growing season.";

    fn snapshot(nodes: &[(&str, &str, &str, usize)]) -> Snapshot {
        Snapshot {
            nodes: nodes
                .iter()
                .map(|(role, name, value, depth)| SnapshotNode {
                    reference: None,
                    role: (*role).into(),
                    name: (*name).into(),
                    value: (*value).into(),
                    depth: *depth,
                })
                .collect(),
            text: String::new(),
            raw_bytes: 0,
            document_root: None,
        }
    }

    #[test]
    fn rejects_auth_forms_title_only_navigation_and_ads() {
        for nodes in [
            vec![("RootWebArea", "Weekly news", "", 0)],
            vec![("heading", PROSE, "", 0)],
            vec![
                ("textbox", "Password", "<redacted>", 0),
                ("StaticText", PROSE, "", 0),
            ],
            vec![("form", "Sign in", "", 0), ("StaticText", PROSE, "", 1)],
            vec![("navigation", "", "", 0), ("StaticText", PROSE, "", 1)],
            vec![("advertisement", "", "", 0), ("StaticText", PROSE, "", 1)],
            vec![("StaticText", "Mail", "", 0)],
        ] {
            assert!(history_text(&snapshot(&nodes)).is_none(), "{nodes:?}");
        }
        let page = snapshot(&[
            ("navigation", "", "", 0),
            ("StaticText", PROSE, "", 1),
            ("advertisement", "", "", 0),
            ("StaticText", PROSE, "", 1),
            ("StaticText", PROSE, "", 0),
        ]);
        assert!(history_text(&page).is_none());
    }

    #[test]
    fn keeps_article_text_but_excludes_account_labels_and_ads() {
        let page = snapshot(&[
            ("RootWebArea", "Garden", "", 0),
            ("navigation", "", "", 1),
            ("StaticText", "Account Person", "", 2),
            ("advertisement", "", "", 1),
            ("StaticText", "Buy now", "", 2),
            ("StaticText", PROSE, "", 1),
        ]);
        let text = history_text(&page).unwrap();
        assert_eq!(text, PROSE);
        assert!(!text.contains("Account Person"));
        assert!(!text.contains("Buy now"));
    }
}
