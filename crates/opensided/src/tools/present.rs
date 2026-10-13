use super::{DaemonTool, ToolEnv, ToolError, ToolExec};
use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_proto::*;
use serde_json::{json, Value};
use std::collections::HashSet;

pub(crate) struct Present;
fn error(code: &str, path: &str, rule: &str) -> ToolError {
    ToolError(format!("{code}: {path}: {rule}"))
}
fn bound(ok: bool, path: &str) -> Result<(), ToolError> {
    if ok {
        Ok(())
    } else {
        Err(error("card_too_large", path, "limit exceeded"))
    }
}
fn shape(ok: bool, path: &str) -> Result<(), ToolError> {
    if ok {
        Ok(())
    } else {
        Err(error("invalid_card", path, "invalid shape"))
    }
}
fn url(text: &mut String, path: &str) -> Result<(), ToolError> {
    bound(text.len() <= 2048, path)?;
    let mut u = url::Url::parse(text).map_err(|_| error("invalid_card", path, "invalid URL"))?;
    shape(
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some(),
        path,
    )?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err(error(
            "card_secret_detected",
            path,
            "URL credentials forbidden",
        ));
    }
    if u.query_pairs().any(|(k, _)| {
        matches!(
            k.to_ascii_lowercase().as_str(),
            "token"
                | "password"
                | "secret"
                | "key"
                | "api_key"
                | "signature"
                | "sig"
                | "credential"
                | "authorization"
                | "x-amz-signature"
                | "x-amz-credential"
        )
    }) {
        return Err(error(
            "card_secret_detected",
            path,
            "secret URL material forbidden",
        ));
    }
    u.set_fragment(None);
    *text = u.to_string();
    Ok(())
}
fn site(site: &mut CardSite, path: &str) -> Result<(), ToolError> {
    shape(site.favicon_ref.is_none(), path)?;
    if let Some(origin) = &mut site.origin {
        url(origin, path)?;
        let parsed =
            url::Url::parse(origin).map_err(|_| error("invalid_card", path, "invalid origin"))?;
        shape(parsed.path() == "/" && parsed.query().is_none(), path)?;
        *origin = parsed.origin().ascii_serialization();
    }
    Ok(())
}
fn money(m: &Money) -> Result<(), ToolError> {
    shape(
        m.currency.len() == 3 && m.currency.bytes().all(|c| c.is_ascii_uppercase()),
        "money.currency",
    )
}
fn rating(r: &Rating) -> Result<(), ToolError> {
    shape(
        r.milli_max > 0 && r.milli_max <= 10000 && r.milli_value <= r.milli_max,
        "rating",
    )
}
fn media(m: &MediaRef, keys: &HashSet<String>) -> Result<(), ToolError> {
    let valid = if let Some(key) = m.path.strip_prefix('@') {
        keys.contains(key)
    } else {
        !m.path.is_empty()
            && !m.path.starts_with('/')
            && !m.path.contains('\\')
            && !m.path.contains(':')
            && m.path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != "..")
    };
    if valid {
        Ok(())
    } else {
        Err(error(
            "card_media_denied",
            "media.path",
            "invalid session artifact reference",
        ))
    }
}
fn value(v: &mut CardValue, keys: &HashSet<String>) -> Result<(), ToolError> {
    match v {
        CardValue::Text { text } => bound(text.chars().count() <= 1000, "value.text"),
        CardValue::Money { money: m } => money(m),
        CardValue::Rating { rating: r } => rating(r),
        CardValue::Link { link } => url(&mut link.url, "value.link.url"),
        CardValue::Image { image, .. } => media(image, keys),
    }
}
fn unique(set: &mut HashSet<String>, id: &str, path: &str) -> Result<(), ToolError> {
    shape(
        !id.is_empty()
            && id.chars().count() <= 128
            && !id.chars().any(char::is_control)
            && set.insert(id.into()),
        path,
    )
}
pub(crate) fn admit_document_text(text: &str, ctx: &ToolEnv) -> Result<(), ToolError> {
    admit_text(text, Some(ctx), "$document")
}

fn admit_text(text: &str, ctx: Option<&ToolEnv>, path: &str) -> Result<(), ToolError> {
    let reason = if openside_memory::looks_secret(text) {
        Some("secret-shaped content forbidden")
    } else if ctx.is_some_and(|ctx| {
        ctx.shared
            .registry
            .redactors
            .read()
            .iter()
            .any(|r| r.redact(text) != text)
    }) {
        Some("known secret value forbidden")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(error("card_secret_detected", path, reason)),
        None => Ok(()),
    }
}
fn scan(
    v: &Value,
    depth: usize,
    text: &mut usize,
    ctx: Option<&ToolEnv>,
    path: &str,
) -> Result<(), ToolError> {
    bound(depth <= 8, "$depth")?;
    match v {
        Value::String(s) => admit_text(s, ctx, path)?,
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                scan(item, depth + 1, text, ctx, &format!("{path}[{index}]"))?;
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let max = match key.as_str() {
                    "title" | "label" | "name" | "alt" | "image_alt" => Some(160),
                    "collapsed_summary" => Some(240),
                    "subtitle" | "description" | "caption" => Some(280),
                    "text" => Some(2000),
                    _ => None,
                };
                if let (Some(max), Some(s)) = (max, item.as_str()) {
                    bound(s.chars().count() <= max, "$text")?;
                    *text += s.len();
                }
                if key == "facts" {
                    if let Some(facts) = item.as_array() {
                        *text += facts
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::len)
                            .sum::<usize>();
                    }
                }
                scan(item, depth + 1, text, ctx, &format!("{path}.{key}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub(crate) fn admit(args: &Value, ctx: Option<&ToolEnv>) -> Result<PresentRequest, ToolError> {
    bound(
        serde_json::to_vec(args)
            .map_err(|_| error("invalid_card", "$", "invalid JSON"))?
            .len()
            <= 65536,
        "$",
    )?;
    let mut request: PresentRequest = serde_json::from_value(args.clone())
        .map_err(|_| error("invalid_card", "$", "unknown field or invalid type"))?;
    let mut bytes = 0;
    scan(args, 0, &mut bytes, ctx, "$")?;
    bound(bytes <= 24576, "$text")?;
    let c = &mut request.card;
    shape(
        c.id.is_empty()
            && c.status == CardStatus::Done
            && c.interaction == CardInteraction::None
            && c.ticket_id.is_none(),
        "card",
    )?;
    shape(
        matches!(
            c.kind,
            CardKind::ProductList
                | CardKind::Table
                | CardKind::PlaceList
                | CardKind::OrderSummary
                | CardKind::Payment
                | CardKind::Form
                | CardKind::ConnectAccount
        ),
        "card.kind",
    )?;
    bound(
        c.blocks.len() <= 16 && c.actions.len() <= 8 && request.images.len() <= 8,
        "card",
    )?;
    c.title = c.title.split_whitespace().collect::<Vec<_>>().join(" ");
    c.collapsed_summary = c
        .collapsed_summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    site(&mut c.site, "card.site")?;
    let mut keys = HashSet::new();
    for image in &mut request.images {
        shape(
            !image.key.is_empty()
                && image.key.len() <= 64
                && image
                    .key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                && keys.insert(image.key.clone()),
            "images.key",
        )?;
        url(&mut image.url, "images.url").map_err(|e| {
            if e.0.starts_with("invalid_card") {
                error("card_media_denied", "images.url", "invalid image source")
            } else {
                e
            }
        })?;
    }
    let mut blocks = HashSet::new();
    let mut items = HashSet::new();
    let mut actions = HashSet::new();
    let mut count = 0;
    let mut bubbles = 0;
    for b in &mut c.blocks {
        let id = match b {
            Block::KvRows { id, .. }
            | Block::ListItems { id, .. }
            | Block::Table { id, .. }
            | Block::Notice { id, .. }
            | Block::Media { id, .. } => id,
            _ => {
                return Err(error(
                    "card_action_forbidden",
                    "card.blocks",
                    "interactive or team block forbidden",
                ))
            }
        };
        unique(&mut blocks, id, "block.id")?;
        match b {
            Block::KvRows { rows, total, .. } => {
                for row in rows.iter_mut().chain(total.iter_mut()) {
                    value(&mut row.value, &keys)?;
                }
            }
            Block::ListItems { items: list, .. } => {
                for item in list {
                    count += 1;
                    unique(&mut items, &item.id, "item.id")?;
                    bound(
                        item.facts.len() <= 8
                            && item.facts.iter().all(|s| s.chars().count() <= 160),
                        "item.facts",
                    )?;
                    shape(
                        item.quantity.map_or(true, |q| (1..=9999).contains(&q)),
                        "item.quantity",
                    )?;
                    if let Some(m) = &item.price {
                        money(m)?;
                    }
                    if let Some(r) = &item.rating {
                        rating(r)?;
                    }
                    if let Some(l) = &mut item.link {
                        url(&mut l.url, "item.link")?;
                    }
                    if let Some(s) = &mut item.site {
                        site(s, "item.site")?;
                    }
                    if let Some(m) = &item.image {
                        media(m, &keys)?;
                    }
                }
            }
            Block::Table {
                columns,
                rows,
                row_sites,
                ..
            } => {
                bound(
                    columns.len() <= 12 && rows.len() <= 100 && columns.len() * rows.len() <= 600,
                    "table",
                )?;
                shape(!columns.is_empty(), "table.columns")?;
                let mut ids = HashSet::new();
                for col in columns.iter() {
                    unique(&mut ids, &col.id, "column.id")?;
                }
                for row in rows.iter_mut() {
                    shape(row.len() == columns.len(), "table.rows")?;
                    for v in row {
                        value(v, &keys)?;
                    }
                }
                if let Some(sites) = row_sites {
                    shape(sites.len() == rows.len(), "table.row_sites")?;
                    for s in sites.iter_mut().flatten() {
                        site(s, "table.row_sites")?;
                    }
                }
            }
            Block::Notice { placement, .. } => {
                if *placement == NoticePlacement::Bubble {
                    bubbles += 1;
                }
            }
            Block::Media { media: m, .. } => media(m, &keys)?,
            _ => unreachable!(),
        }
    }
    bound(count <= 30 && bubbles <= 1, "card.blocks")?;
    let mut primary = 0;
    for a in &mut c.actions {
        unique(&mut actions, &a.id, "action.id")?;
        if a.style == ActionStyle::Primary {
            primary += 1;
        }
        match &mut a.target {
            ActionTarget::FollowUp { .. } => {}
            ActionTarget::Link { url: u } => url(u, "action.url")?,
            _ => {
                return Err(error(
                    "card_action_forbidden",
                    "action.target",
                    "privileged target forbidden",
                ))
            }
        }
    }
    bound(primary <= 1, "card.actions")?;
    Ok(request)
}
fn model_schema() -> Value {
    let mut schema = schemars::schema_for!(PresentRequest).to_value();
    let defs = &mut schema["$defs"];
    let card = &mut defs["Card"]["properties"];
    card["id"] = json!({"const":""});
    card["kind"] = json!({"enum":["product_list","table","place_list","order_summary","payment","form","connect_account"]});
    card["status"] = json!({"const":"done"});
    card["interaction"] = json!({"const":"none"});
    card["ticket_id"] = json!({"type":"null"});
    card["blocks"]["maxItems"] = json!(16);
    card["actions"]["maxItems"] = json!(8);
    defs["CardSite"]["properties"]["favicon_ref"] = json!({"type":"null"});
    for (name, allowed) in [
        (
            "Block",
            &["kv_rows", "list_items", "table", "notice", "media"][..],
        ),
        ("ActionTarget", &["follow_up", "link"][..]),
    ] {
        if let Some(variants) = defs[name]["oneOf"].as_array_mut() {
            variants.retain(|variant| {
                variant["properties"]["type"]["const"]
                    .as_str()
                    .is_some_and(|tag| allowed.contains(&tag))
            });
        }
    }
    defs["PresentImage"]["properties"]["key"]["pattern"] = json!("^[A-Za-z0-9_-]{1,64}$");
    defs["PresentImage"]["properties"]["url"]["maxLength"] = json!(2048);
    schema["properties"]["images"]["maxItems"] = json!(8);

    fn compact(value: &mut Value) {
        match value {
            Value::Object(object) => {
                object.retain(|key, _| {
                    !matches!(
                        key.as_str(),
                        "description" | "title" | "$schema" | "examples" | "default"
                    ) && !key.starts_with("x-")
                });
                if object.contains_key("const") {
                    object.remove("type");
                }
                if let Some(variants) = object.get("oneOf").and_then(Value::as_array) {
                    if variants
                        .iter()
                        .all(|variant| variant.get("const").is_some())
                    {
                        let values: Vec<_> = variants
                            .iter()
                            .map(|variant| variant["const"].clone())
                            .collect();
                        object.remove("oneOf");
                        object.insert("enum".into(), Value::Array(values));
                    }
                }
                for (key, child) in object {
                    if matches!(key.as_str(), "properties" | "$defs") {
                        if let Some(fields) = child.as_object_mut() {
                            for field in fields.values_mut() {
                                compact(field);
                            }
                        }
                    } else {
                        compact(child);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    compact(item);
                }
            }
            _ => {}
        }
    }
    fn references(value: &Value, names: &mut HashSet<String>) {
        match value {
            Value::Object(object) => {
                if let Some(name) = object
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|reference| reference.strip_prefix("#/$defs/"))
                {
                    names.insert(name.into());
                }
                for child in object.values() {
                    references(child, names);
                }
            }
            Value::Array(items) => {
                for item in items {
                    references(item, names);
                }
            }
            _ => {}
        }
    }
    compact(&mut schema);
    // The wire model includes secure asks; the model catalog must not pay for unreachable
    // input/team definitions or receive person-only metadata from the public API schema.
    let mut defs = schema
        .as_object_mut()
        .and_then(|root| root.remove("$defs"))
        .and_then(|defs| defs.as_object().cloned())
        .unwrap_or_default();
    let mut reachable = HashSet::new();
    references(&schema, &mut reachable);
    loop {
        let previous = reachable.len();
        let names: Vec<_> = reachable.iter().cloned().collect();
        for name in names {
            if let Some(definition) = defs.get(&name) {
                references(definition, &mut reachable);
            }
        }
        if reachable.len() == previous {
            break;
        }
    }
    defs.retain(|name, _| reachable.contains(name));
    schema["$defs"] = Value::Object(defs);
    schema
}

impl DaemonTool for Present {
    fn name(&self) -> &'static str {
        "present"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        static PARAMETERS: std::sync::LazyLock<String> =
            std::sync::LazyLock::new(|| model_schema().to_string());
        openside_core::ToolSpec {
            name: self.name().into(),
            description: "Present a read-only card; no secrets or asks. Use request_login/request_form for input. Returns card_id; do not repeat the card in Markdown. Media paths may use @image_key.".into(),
            parameters_json: PARAMETERS.clone(),
            approval: ApprovalTier::Read,
            deferrable: false,
        }
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<openside_policy::Action>, ToolError>> {
        Box::pin(async move {
            admit(args, Some(ctx))?;
            Ok(vec![])
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        let kind = args["card"]["kind"].as_str().unwrap_or("invalid");
        json!({"kind":kind,"blocks":args["card"]["blocks"].as_array().map_or(0,Vec::len),"actions":args["card"]["actions"].as_array().map_or(0,Vec::len),"images":args["images"].as_array().map_or(0,Vec::len)})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result: Result<Value, ToolError> = async {
                let mut request = admit(&args, Some(ctx))?;
                // Serialize replay admission with the durable row so a retry never fetches twice.
                let _gate = ctx.shared.presentation_gate.lock().await;
                let state = ctx
                    .shared
                    .db
                    .state(&ctx.session)
                    .await
                    .map_err(|_| error("invalid_card", "$", "session unavailable"))?;
                let id = format!("present:{}", ctx.tool_call_id);
                if state.cards.contains_key(&id) {
                    return Ok(json!({"card_id":id}));
                }
                crate::card_media::materialize(&mut request.card, &request.images, ctx).await?;
                crate::favicons::populate(&mut request.card, ctx).await;
                request.card.id = id.clone();
                let mut text_bytes = 0;
                scan(&args, 0, &mut text_bytes, Some(ctx), "$")?;
                ctx.shared
                    .emit(
                        &ctx.session,
                        None,
                        EventKind::CardPresented {
                            card: request.card,
                            tool_call_id: ctx.tool_call_id.clone(),
                        },
                    )
                    .await
                    .map_err(|_| error("invalid_card", "$", "presentation commit failed"))?;
                Ok(json!({"card_id":id}))
            }
            .await;
            match result {
                Ok(for_model) => ToolExec::Done {
                    for_model,
                    for_app: None,
                },
                Err(e) => ToolExec::Failed(e.0),
            }
        })
    }
}
