use std::sync::LazyLock;

use openside_proto::DeviceKind;
use serde_json::{Map, Value};

use crate::api::Caller;

static SCHEMAS: LazyLock<Map<String, Value>> = LazyLock::new(openside_proto::schemas);

/// The replacement lives on the proto field, so new private fields need no parallel denylist.
pub(crate) fn strip_person_only(value: &mut Value, root: &str, caller: &Caller) {
    if caller.0.is_none() || caller.1 == DeviceKind::PersonDevice {
        return;
    }
    if let Some(schema) = SCHEMAS.get(root) {
        strip(value, schema);
    }
}

fn strip(value: &mut Value, schema: &Value) {
    if let Some(replacement) = schema.get("x-person-only") {
        *value = replacement.clone();
        return;
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            if let Some(target) = SCHEMAS.get(name) {
                strip(value, target);
            }
        }
    }
    // Walk every schema branch conservatively. Only marked fields are changed; branch
    // selection must never become a way to bypass privacy with historical payload shapes.
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                strip(value, branch);
            }
        }
    }
    if let (Some(object), Some(properties)) = (
        value.as_object_mut(),
        schema.get("properties").and_then(Value::as_object),
    ) {
        for (name, property) in properties {
            if let Some(field) = object.get_mut(name) {
                strip(field, property);
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array_mut(), schema.get("items")) {
        for item in items {
            strip(item, item_schema);
        }
    }
}

pub(crate) async fn response(
    response: axum::response::Response,
    root: Option<&str>,
    caller: &Caller,
) -> axum::response::Response {
    use axum::{
        body::{to_bytes, Body},
        http::header,
        response::IntoResponse,
    };
    if caller.0.is_none() || caller.1 == DeviceKind::PersonDevice || !response.status().is_success()
    {
        return response;
    }
    let Some(root) = root else {
        return response;
    };
    if !response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"))
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::api::HttpError::internal().into_response(),
    };
    let mut value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return crate::api::HttpError::internal().into_response(),
    };
    strip_person_only(&mut value, root, caller);
    let bytes = match serde_json::to_vec(&value) {
        Ok(bytes) => bytes,
        Err(_) => return crate::api::HttpError::internal().into_response(),
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    axum::response::Response::from_parts(parts, Body::from(bytes))
}
