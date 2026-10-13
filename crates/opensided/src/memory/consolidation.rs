use openside_memory::{ConsolidationEdit, NewMemory};
use openside_proto::Retention;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{ToolEnv, ToolExec};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    scope: String,
    #[serde(default)]
    edits: Vec<Edit>,
    #[serde(default)]
    additions: Vec<Addition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    id: String,
    version: u32,
    content: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Addition {
    layer: String,
    content: String,
}

pub(crate) async fn handle(ctx: &ToolEnv, args: Value) -> ToolExec {
    let request: Request = match serde_json::from_value(args) {
        Ok(request) => request,
        Err(_) => return ToolExec::Failed("Invalid memory consolidation".into()),
    };
    let state = match ctx.shared.db.state(&ctx.session).await {
        Ok(state) if !state.incognito => state,
        _ => {
            return ToolExec::Denied {
                reason: "memory_consolidation_unavailable".into(),
                hint: None,
            }
        }
    };
    if state.agent_id.as_deref().is_some_and(|id| id != "chief") {
        return ToolExec::Denied {
            reason: "chief_only".into(),
            hint: None,
        };
    }
    if request.edits.len() > 100
        || request.additions.len() > 20
        || request
            .additions
            .iter()
            .any(|addition| !matches!(addition.layer.as_str(), "profile" | "event_log" | "scratch"))
    {
        return ToolExec::Failed("Invalid memory consolidation limits or layer".into());
    }
    let additions = request
        .additions
        .into_iter()
        .map(|addition| NewMemory {
            retention: Some(match addition.layer.as_str() {
                "profile" => Retention::Permanent,
                "scratch" => Retention::Hours24,
                _ => Retention::Days30,
            }),
            tags: vec![addition.layer],
            ..NewMemory::note(addition.content)
        })
        .collect();
    let edits = request
        .edits
        .into_iter()
        .map(|edit| ConsolidationEdit {
            id: edit.id,
            version: edit.version,
            content: edit.content,
        })
        .collect();
    let session = ctx.session.as_str().to_owned();
    let actor = format!("consolidation:{session}");
    let owner = state.agent_id;
    let result = ctx
        .shared
        .memory
        .write(move |store| {
            store.consolidate(
                &request.scope,
                &session,
                owner.as_deref(),
                edits,
                additions,
                &actor,
            )
        })
        .await;
    match result {
        Ok(applied) => ToolExec::Done {
            for_model: json!({"applied": applied}),
            for_app: None,
        },
        Err(error) => ToolExec::Denied {
            reason: error.code().into(),
            hint: Some("Recall a fresh memory scope before consolidating changed facts.".into()),
        },
    }
}
