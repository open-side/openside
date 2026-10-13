use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, ToolSpec};
use openside_proto::{CatalogModel, ModelAccount, ModelAccountRef};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::{self, DaemonTool, ToolEnv, ToolError, ToolExec};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {}

pub(crate) struct Models;

fn denied(reason: &str) -> ToolExec {
    ToolExec::Denied {
        reason: reason.into(),
        hint: Some("Use get_models and select an available connected model".into()),
    }
}

async fn restriction(ctx: &ToolEnv) -> Result<Option<Vec<ModelAccountRef>>, ToolExec> {
    let state = ctx
        .shared
        .db
        .state(&ctx.session)
        .await
        .map_err(|_| ToolExec::Failed("Session state is unavailable".into()))?;
    let agent = crate::agents::for_session(&ctx.shared, &state)
        .map_err(|_| ToolExec::Failed("Agent state is unavailable".into()))?;
    if agent
        .as_ref()
        .is_some_and(|agent| agent.status != openside_proto::AgentStatus::Active)
    {
        return Err(denied("model_not_in_pool"));
    }
    Ok(agent.and_then(|agent| agent.account_pool))
}

fn eligible(account: &ModelAccount, restriction: Option<&[ModelAccountRef]>) -> bool {
    account.available
        && account.in_pool
        && restriction.map_or(true, |pool| {
            pool.iter().any(|reference| {
                reference.source == account.source && reference.account_id == account.account_id
            })
        })
}

pub(super) async fn validate(ctx: &ToolEnv, model: &str) -> Result<(), ToolExec> {
    let catalog = crate::models_api::catalog(&ctx.shared)
        .await
        .map_err(|_| denied("model_catalog_unavailable"))?;
    let candidate = catalog
        .providers
        .iter()
        .flat_map(|provider| &provider.models)
        .find(|candidate| candidate.id == model)
        .ok_or_else(|| denied("model_unavailable"))?;
    if !candidate.available {
        return Err(denied("model_unavailable"));
    }
    let restriction = restriction(ctx).await?;
    if !candidate
        .accounts
        .iter()
        .any(|account| eligible(account, restriction.as_deref()))
    {
        return Err(denied("model_not_in_pool"));
    }
    Ok(())
}

fn windows(account: &ModelAccount) -> Vec<Value> {
    account.usage_windows.iter().map(|window| json!({
        "name": window.name,
        "remaining_percent": window.used_percent.map(|used| (100.0 - used).clamp(0.0, 100.0)),
        "resets_at": window.reset_at_ms,
    })).collect()
}

fn remaining(account: &ModelAccount) -> Option<f64> {
    account
        .usage_windows
        .iter()
        .filter_map(|window| window.used_percent)
        .map(|used| (100.0 - used).clamp(0.0, 100.0))
        .reduce(f64::min)
}

fn reset(account: &ModelAccount) -> Option<f64> {
    account
        .usage_windows
        .iter()
        .filter(|window| window.used_percent.is_some())
        .max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .and_then(|window| window.reset_at_ms)
}

fn row(model: &CatalogModel, restriction: Option<&[ModelAccountRef]>) -> Option<Value> {
    if !model.available {
        return None;
    }
    let accounts: Vec<_> = model
        .accounts
        .iter()
        .filter(|account| eligible(account, restriction))
        .collect();
    let best = accounts.iter().copied().max_by(|a, b| {
        remaining(a)
            .unwrap_or(-1.0)
            .total_cmp(&remaining(b).unwrap_or(-1.0))
    })?;
    Some(json!({
        "id": model.id,
        "display_name": model.label,
        "tier": model.tier,
        "strengths": model.strengths,
        "remaining_percent": remaining(best),
        "resets_at": reset(best),
        "usage_windows": windows(best),
        "accounts": accounts.iter().enumerate().map(|(index, account)| json!({
            "label": format!("Account {}", index + 1),
            "remaining_percent": remaining(account),
            "resets_at": reset(account),
            "usage_windows": windows(account),
        })).collect::<Vec<_>>(),
    }))
}

impl DaemonTool for Models {
    fn name(&self) -> &'static str {
        "get_models"
    }
    fn spec(&self) -> ToolSpec {
        tools::spec::<Args>(self.name(), "List available connected models, routing tiers, strengths and remaining usage. Account labels are masked; reset times are Unix milliseconds and unknown usage is null.", ApprovalTier::Read, false)
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        super::front(&ctx.shared, ctx.agent_id.as_deref())
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<openside_policy::Action>, ToolError>> {
        Box::pin(async move { tools::parse::<Args>(args).map(|_| Vec::new()) })
    }
    fn redact_args(&self, args: &Value) -> Value {
        args.clone()
    }
    fn call<'a>(&'a self, _: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let restriction = match restriction(ctx).await {
                Ok(value) => value,
                Err(error) => return error,
            };
            let catalog = match crate::models_api::catalog(&ctx.shared).await {
                Ok(catalog) => catalog,
                Err(_) => return denied("model_catalog_unavailable"),
            };
            let models: Vec<_> = catalog
                .providers
                .iter()
                .flat_map(|provider| &provider.models)
                .filter_map(|model| row(model, restriction.as_deref()))
                .collect();
            ToolExec::Done {
                for_model: json!({"models": models}),
                for_app: None,
            }
        })
    }
}
