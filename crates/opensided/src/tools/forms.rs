use super::{
    browser::Controller,
    otp::{approved_origin, origin_actions},
    parse, spec, DaemonTool, TicketRequest, ToolEnv, ToolError, ToolExec,
};
use crate::{api::HttpError, engine::Shared};
use axum::http::StatusCode;
use chrono::{Datelike, Utc};
use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_policy::Action;
use openside_proto::{
    CardField, InputField, InputKind, InputValidation, Ticket, TicketForm, TicketKind, TicketTarget,
};
use openside_vault::{Redactor, Secret};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {
    tab_id: String,
    reason: Option<String>,
    purpose: Purpose,
}
#[derive(Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    Form,
    Payment,
}

const DISCOVER: &str = r#"(() => {
 const nodes=Array.from(document.querySelectorAll('input,textarea,select[autocomplete*="cc-"]')).filter(e=> {
 const r=e.getBoundingClientRect(),s=getComputedStyle(e);
 return r.width>0&&r.height>0&&s.visibility==='visible'&&s.display!=='none'&&!e.disabled&&!e.readOnly&&!['hidden','submit','button','reset','file','image','radio','range','color'].includes(e.type);
 });
 return {nodes,data:nodes.map(e=>({kind:e.tagName==='TEXTAREA'?'textarea':e.type,autocomplete:e.autocomplete||'',required:e.required,max_length:e.maxLength>0?Math.min(e.maxLength,4096):4096,label:(e.labels?.[0]?.textContent||e.getAttribute('aria-label')||e.name||'Field').trim().slice(0,120)}))};
})()"#;

pub(crate) async fn locate_fields(
    browser: &openside_browser::Browser,
    owner: &str,
    tab: &str,
    expected: &openside_browser::FrameInfo,
) -> Result<(Value, Vec<openside_browser::NodeHandle>), ToolError> {
    let (data, nodes) = browser
        .locate_form_fields(owner, tab, &expected.id, DISCOVER)
        .await
        .map_err(|_| ToolError("form_target_unavailable_use_takeover".into()))?;
    if nodes.iter().any(|node| {
        !node.frame.as_ref().is_some_and(|pin| {
            pin.frame_id == expected.id
                && pin.origin == expected.origin
                && pin.loader_id == expected.loader
                && pin.host == expected.host
        })
    }) {
        return Err(ToolError("target_changed".into()));
    }
    Ok((data, nodes))
}

pub(crate) struct RequestForm;
impl DaemonTool for RequestForm {
    fn name(&self) -> &'static str {
        "request_form"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<Args>(self.name(), "Ask the person to fill live page fields. Payment requires recognized payment autocomplete fields. Values stay private. This only fills: it never clicks or submits. A later consequential submission needs separate approval.", ApprovalTier::Read, true)
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
            let args: Args = parse(args)?;
            origin_actions(ctx, &args.tab_id).await
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        let purpose = match args["purpose"].as_str() {
            Some("form") => "form",
            Some("payment") => "payment",
            _ => "invalid",
        };
        serde_json::json!({"purpose":purpose})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let result = async {
                let args: Args = parse(&args)?;
                let origin = approved_origin(ctx)?;
                let browser = ctx
                    .shared
                    .registry
                    .browser
                    .for_tab(&ctx.shared, &ctx.session, &args.tab_id)
                    .await?;
                if Controller::origin(&browser, &ctx.session, &args.tab_id).await? != origin {
                    return Err(ToolError("target_changed".into()));
                }
                let frames = browser
                    .frames(ctx.session.as_str(), &args.tab_id)
                    .await
                    .map_err(|_| ToolError("form_target_unavailable".into()))?;
                let mut fields = Vec::new();
                let mut pins = Vec::new();
                for frame in frames {
                    if !frame.origin.starts_with("http://") && !frame.origin.starts_with("https://")
                    {
                        continue;
                    }
                    let (data, nodes) =
                        locate_fields(&browser, ctx.session.as_str(), &args.tab_id, &frame).await?;
                    for (meta, node) in data.as_array().into_iter().flatten().zip(nodes) {
                        let target_origin = node
                            .frame
                            .as_ref()
                            .ok_or_else(|| ToolError("target_changed".into()))?
                            .origin
                            .clone();
                        let autocomplete = meta["autocomplete"]
                            .as_str()
                            .unwrap_or("")
                            .split_whitespace()
                            .last()
                            .unwrap_or("");
                        let card = match autocomplete {
                            "cc-name" => Some(CardField::Name),
                            "cc-number" => Some(CardField::Number),
                            "cc-exp" => Some(CardField::Expiry),
                            "cc-csc" => Some(CardField::Cvc),
                            "postal-code" if args.purpose == Purpose::Payment => {
                                Some(CardField::PostalCode)
                            }
                            _ => None,
                        };
                        if autocomplete.starts_with("cc-")
                            && (card.is_none()
                                || meta["kind"] == "select-one"
                                || meta["kind"] == "select-multiple")
                        {
                            return Err(ToolError("form_target_unavailable_use_takeover".into()));
                        }
                        if args.purpose == Purpose::Payment && card.is_none() {
                            continue;
                        }
                        let kind = match meta["kind"].as_str() {
                            Some("email") => InputKind::Email,
                            Some("tel") => InputKind::Tel,
                            Some("password") => InputKind::Password,
                            Some("checkbox") => InputKind::Checkbox,
                            Some("textarea") => InputKind::Textarea,
                            _ => InputKind::Text,
                        };
                        let otp = autocomplete == "one-time-code";
                        let (label, validation, max_length) = match &card {
                            Some(CardField::Name) => {
                                ("Name on card".to_owned(), InputValidation::Text, 160)
                            }
                            Some(CardField::Number) => {
                                ("Card number".to_owned(), InputValidation::CardNumber, 19)
                            }
                            Some(CardField::Expiry) => {
                                ("Expiry (MM/YY)".to_owned(), InputValidation::CardExpiry, 7)
                            }
                            Some(CardField::Cvc) => {
                                ("Security code".to_owned(), InputValidation::CardCvc, 4)
                            }
                            Some(CardField::PostalCode) => {
                                ("Postal code".to_owned(), InputValidation::PostalCode, 32)
                            }
                            None => (
                                meta["label"].as_str().unwrap_or("Field").to_owned(),
                                if otp {
                                    InputValidation::Otp
                                } else {
                                    match kind {
                                        InputKind::Email => InputValidation::Email,
                                        InputKind::Tel => InputValidation::Phone,
                                        _ => InputValidation::Text,
                                    }
                                },
                                meta["max_length"].as_u64().unwrap_or(4096).min(if otp {
                                    64
                                } else {
                                    4096
                                }) as u16,
                            ),
                        };
                        let id = format!("field_{}", fields.len());
                        let secret = args.purpose == Purpose::Payment
                            || card.is_some()
                            || otp
                            || kind == InputKind::Password
                            || kind == InputKind::Email
                            || matches!(autocomplete, "username" | "email");
                        fields.push(InputField {
                            id: id.clone(),
                            label,
                            kind,
                            secret,
                            otp,
                            card,
                            required: meta["required"] == true,
                            autocomplete: (!autocomplete.is_empty())
                                .then(|| autocomplete.to_owned()),
                            placeholder: None,
                            validation,
                            max_length: max_length
                                .min(meta["max_length"].as_u64().unwrap_or(4096) as u16),
                            initial: None,
                            suggestions: Vec::new(),
                            target_origin: Some(target_origin),
                        });
                        pins.push((id, node));
                    }
                }
                if fields.is_empty()
                    || (args.purpose == Purpose::Payment
                        && !fields.iter().any(|f| {
                            matches!(
                                f.card,
                                Some(
                                    CardField::Number
                                        | CardField::Expiry
                                        | CardField::Cvc
                                        | CardField::Name
                                )
                            )
                        }))
                {
                    return Err(ToolError("form_not_found".into()));
                }
                if fields.len() > 16 {
                    return Err(ToolError("too_many_form_fields".into()));
                }
                if fields.iter().any(|field| {
                    !field.secret
                        && field.autocomplete.is_some()
                        && field.kind != InputKind::Checkbox
                }) {
                    let root = ctx.shared.config.data_dir.clone();
                    let addresses = tokio::task::spawn_blocking(move || {
                        crate::import_delivery::source_addresses(&root)
                    })
                    .await
                    .map_err(|_| {
                        ToolError("Imported address suggestions could not be read".into())
                    })?
                    .map_err(|_| {
                        ToolError("Imported address suggestions could not be read".into())
                    })?;
                    for field in &mut fields {
                        if !field.secret && field.kind != InputKind::Checkbox {
                            field.suggestions = super::imports::address_suggestions(
                                &addresses,
                                field.autocomplete.as_deref().unwrap_or_default(),
                            )
                            .into_iter()
                            .filter(|text| {
                                text.chars().count() <= usize::from(field.max_length.min(160))
                            })
                            .take(10)
                            .collect();
                        }
                    }
                }
                browser
                    .validate_form_fields(
                        ctx.session.as_str(),
                        &args.tab_id,
                        &pins.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>(),
                    )
                    .await
                    .map_err(|_| ToolError("target_changed".into()))?;
                let site_name = origin
                    .split_once("://")
                    .map_or(origin.as_str(), |(_, host)| host)
                    .to_owned();
                let kind = if fields.iter().any(|field| field.secret) {
                    TicketKind::SecretInput
                } else {
                    TicketKind::Info
                };
                Ok(ToolExec::Defer(Box::new(TicketRequest {
                    hire: None,
                    kind,
                    summary: format!("Fill fields for {site_name}"),
                    instructions: args.reason,
                    options: Vec::new(),
                    target: Some(TicketTarget {
                        tab_id: Some(args.tab_id),
                        origin: Some(origin.clone()),
                        field_ref: None,
                    }),
                    resume_when: None,
                    otp: None,
                    otp_register: None,
                    login: None,
                    choice: None,
                    approval: None,
                    form: Some(TicketForm {
                        origin,
                        site_name,
                        fields,
                        payment: args.purpose == Purpose::Payment,
                        submit_label: "Fill fields".into(),
                    }),
                    pins,
                })))
            }
            .await;
            result.unwrap_or_else(|error:ToolError| {
   let hint=(error.0=="form_target_unavailable_use_takeover").then(||"Offer request_human_step with kind takeover for this tab; the provider frame could not be securely bound.".into());
   ToolExec::Denied {reason:error.0,hint}
  })
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FormStatus {
    Filled,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FormNext {
    ReviewPage,
}
#[derive(Serialize)]
pub(crate) struct FormResolution {
    pub status: FormStatus,
    pub field_count: usize,
    pub next: FormNext,
}
fn refused(code: &'static str) -> HttpError {
    HttpError(StatusCode::CONFLICT, code)
}

fn valid(field: &InputField, value: &Value) -> bool {
    if field.kind == InputKind::Checkbox {
        return value.as_bool().is_some_and(|on| !field.required || on);
    }
    let Some(text) = value.as_str() else {
        return false;
    };
    let length = text
        .chars()
        .filter(|c| field.validation != InputValidation::CardNumber || (*c != ' ' && *c != '-'))
        .count();
    if length > usize::from(field.max_length)
        || text.contains('\0')
        || (field.required && text.trim().is_empty())
    {
        return false;
    }
    if text.is_empty() {
        return !field.required;
    }
    match field.validation {
        InputValidation::CardNumber => {
            let mut count = 0;
            let mut sum = 0;
            for c in text.chars().rev().filter(|c| *c != ' ' && *c != '-') {
                let Some(d) = c.to_digit(10) else {
                    return false;
                };
                let n = if count % 2 == 1 { d * 2 } else { d };
                sum += if n > 9 { n - 9 } else { n };
                count += 1;
            }
            (12..=19).contains(&count) && sum > 0 && sum % 10 == 0
        }
        InputValidation::CardCvc => {
            (3..=4).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
        }
        InputValidation::CardExpiry => {
            let now = Utc::now();
            valid_expiry(text, now.year(), now.month())
        }
        InputValidation::Otp => text.chars().count() <= 64 && !text.chars().any(char::is_control),
        InputValidation::Email => text.split_once('@').is_some_and(|(a, b)| {
            !a.is_empty() && b.contains('.') && !text.chars().any(char::is_whitespace)
        }),
        InputValidation::Phone => text
            .chars()
            .all(|c| c.is_ascii_digit() || "+()- .".contains(c)),
        _ => true,
    }
}

fn valid_expiry(text: &str, year: i32, month: u32) -> bool {
    let Some((m, y)) = text.split_once('/') else {
        return false;
    };
    if m.len() != 2
        || !(y.len() == 2 || y.len() == 4)
        || !m.bytes().chain(y.bytes()).all(|b| b.is_ascii_digit())
    {
        return false;
    }
    let short_year = y.len() == 2;
    let (Ok(m), Ok(mut y)) = (m.parse::<u32>(), y.parse::<i32>()) else {
        return false;
    };
    if short_year {
        y += 2000;
    }
    (1..=12).contains(&m) && (y, m) >= (year, month)
}

struct TransientValue(Value);
impl Drop for TransientValue {
    fn drop(&mut self) {
        fn clear(value: &mut Value) {
            match value {
                Value::String(text) => text.zeroize(),
                Value::Array(values) => values.iter_mut().for_each(clear),
                Value::Object(values) => values.values_mut().for_each(clear),
                _ => {}
            }
        }
        clear(&mut self.0);
    }
}

pub(crate) async fn resolve_form(
    shared: &Shared,
    ticket: &Ticket,
    value: Value,
) -> Result<FormResolution, HttpError> {
    let mut value = TransientValue(value);
    // Install redaction even for malformed payloads, before validation or browser work.
    fn collect(value: &Value, secrets: &mut Vec<Secret>) {
        match value {
            Value::String(s) if !s.is_empty() => secrets.push(Secret::new(s.clone())),
            Value::Array(a) => {
                for v in a {
                    collect(v, secrets)
                }
            }
            Value::Object(o) => {
                for v in o.values() {
                    collect(v, secrets)
                }
            }
            _ => {}
        }
    }
    let mut secrets = Vec::new();
    collect(&value.0, &mut secrets);
    if !secrets.is_empty() {
        shared
            .registry
            .redactors
            .write()
            .push(Redactor::new(&secrets));
    }
    let form = ticket.form.as_ref().ok_or_else(HttpError::invalid_value)?;
    if let Some(values) = value.0.get_mut("values").and_then(Value::as_object_mut) {
        let mut normalized = Vec::new();
        for field in &form.fields {
            if field.validation == InputValidation::CardNumber {
                if let Some(Value::String(text)) = values.get_mut(&field.id) {
                    text.retain(|c| c != ' ' && c != '-');
                    if !text.is_empty() {
                        normalized.push(Secret::new(text.clone()));
                    }
                }
            }
        }
        if !normalized.is_empty() {
            shared
                .registry
                .redactors
                .write()
                .push(Redactor::new(&normalized));
        }
    }
    let values = value
        .0
        .as_object_mut()
        .filter(|o| o.len() == 1)
        .and_then(|o| o.get_mut("values"))
        .and_then(Value::as_object_mut)
        .ok_or_else(HttpError::invalid_value)?;
    if values.len() != form.fields.len()
        || !form
            .fields
            .iter()
            .all(|f| values.get(&f.id).is_some_and(|v| valid(f, v)))
    {
        return Err(HttpError::invalid_value());
    }
    let ordered = TransientValue(Value::Array(
        form.fields
            .iter()
            .map(|f| values.remove(&f.id).ok_or_else(HttpError::invalid_value))
            .collect::<Result<Vec<_>, _>>()?,
    ));
    let controller = &shared.registry.browser;
    let _gate = controller.gate.lock().await;
    if controller.fill_intent_pending(shared, &ticket.id).await? {
        return Err(refused("form_fill_indeterminate"));
    }
    let (browser, tab, pins) = super::login::pinned_target(shared, ticket).await?;
    if pins.len() != form.fields.len()
        || !pins
            .iter()
            .zip(&form.fields)
            .all(|((id, _), field)| id == &field.id)
    {
        return Err(refused("target_unavailable"));
    }
    let nodes = pins.into_iter().map(|(_, node)| node).collect::<Vec<_>>();
    browser
        .validate_form_fields(ticket.session_id.as_str(), &tab, &nodes)
        .await
        .map_err(|_| refused("target_changed"))?;
    let target = ticket
        .target
        .as_ref()
        .ok_or_else(HttpError::invalid_value)?;
    controller
        .mark_ticket_secret(shared, &ticket.id, &ticket.session_id, target)
        .await
        .map_err(|_| refused("target_unavailable"))?;
    controller
        .record_fill_intent(shared, &ticket.id, None)
        .await?;
    let values = ordered.0.as_array().ok_or_else(HttpError::invalid_value)?;
    browser
        .fill_form_fields(ticket.session_id.as_str(), &tab, &nodes, values)
        .await
        .map_err(|_| refused("form_fill_indeterminate"))?;
    controller.persist(shared).await?;
    Ok(FormResolution {
        status: FormStatus::Filled,
        field_count: nodes.len(),
        next: FormNext::ReviewPage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(validation: InputValidation) -> InputField {
        InputField {
            id: "f0".into(),
            label: "Field".into(),
            kind: InputKind::Text,
            secret: true,
            otp: false,
            card: None,
            required: true,
            autocomplete: None,
            placeholder: None,
            validation,
            max_length: 40,
            initial: None,
            suggestions: Vec::new(),
            target_origin: Some("https://provider.test".into()),
        }
    }

    #[test]
    fn typed_form_validation_rejects_wrong_types_and_payment_values() {
        let pan = field(InputValidation::CardNumber);
        assert!(valid(&pan, &json!("4242 4242 4242 4242")));
        for value in [
            json!(false),
            json!("4242424242424241"),
            json!("0000000000000000"),
            json!("42424242x4242424"),
        ] {
            assert!(!valid(&pan, &value));
        }
        assert!(valid(&field(InputValidation::CardExpiry), &json!("12/39")));
        assert!(!valid(&field(InputValidation::CardExpiry), &json!("13/39")));
        assert!(valid(&field(InputValidation::CardCvc), &json!("123")));
        assert!(!valid(&field(InputValidation::CardCvc), &json!("12a")));
        assert!(valid(&field(InputValidation::Otp), &json!("123456")));
        assert!(valid(&field(InputValidation::Otp), &json!("a23456")));
        assert!(!valid(&field(InputValidation::Otp), &json!("a".repeat(65))));
        assert!(valid_expiry("10/26", 2026, 10));
        assert!(!valid_expiry("09/26", 2026, 10));
        assert!(!valid_expiry("12/25", 2026, 10));
        assert!(!valid_expiry("12/0099", 2026, 10));
        let mut checkbox = field(InputValidation::Text);
        checkbox.kind = InputKind::Checkbox;
        assert!(!valid(&checkbox, &json!("true")));
        assert!(!valid(&checkbox, &json!(false)));
        assert!(valid(&checkbox, &json!(true)));
        let mut short = field(InputValidation::Text);
        short.max_length = 2;
        assert!(!valid(&short, &json!("long")));
    }

    #[test]
    fn resolution_is_value_free_and_strict_request_rejects_bindings() {
        assert_eq!(
            serde_json::to_value(FormResolution {
                status: FormStatus::Filled,
                field_count: 4,
                next: FormNext::ReviewPage
            })
            .unwrap(),
            json!({"status":"filled","field_count":4,"next":"review_page"})
        );
        assert!(serde_json::from_value::<Args>(
            json!({"tab_id":"tab","purpose":"form","selector":"#secret"})
        )
        .is_err());
        assert!(serde_json::from_value::<Args>(
            json!({"tab_id":"tab","purpose":"payment","origin":"https://forged.test"})
        )
        .is_err());
        assert_eq!(RequestForm.redact_args(&json!({"purpose":"payment","tab_id":{"value":"canary"},"reason":"canary","values":{"field_0":"canary"}})),json!({"purpose":"payment"}));
    }
}
