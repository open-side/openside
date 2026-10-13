use super::{now, Error};
use base64::Engine;
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};

fn clear(value: &mut Value) {
    match value {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => a.iter_mut().for_each(clear),
        Value::Object(o) => o.values_mut().for_each(clear),
        _ => (),
    }
}
fn replace(value: &mut Value, replacement: Value) {
    clear(value);
    *value = replacement;
}
struct Document(Value);
impl Drop for Document {
    fn drop(&mut self) {
        clear(&mut self.0);
    }
}
fn document(raw: &str) -> Result<Document, Error> {
    if raw.len() > 262_144 {
        return Err(Error::AuthUnavailable);
    }
    serde_json::from_str(raw)
        .map(Document)
        .map_err(|_| Error::AuthUnavailable)
}

pub(super) struct Parsed {
    pub access: Zeroizing<String>,
    pub account_id: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub refreshable: bool,
}
impl Drop for Parsed {
    fn drop(&mut self) {
        self.account_id.zeroize();
    }
}
fn token(value: Option<&Value>) -> Result<Zeroizing<String>, Error> {
    let value = value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 32768 && s.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or(Error::AuthUnavailable)?;
    Ok(Zeroizing::new(value.to_owned()))
}
fn jwt_claims(token: &str) -> Option<Document> {
    let payload = token.split('.').nth(1)?;
    if payload.len() > 65536 {
        return None;
    }
    let bytes = Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?,
    );
    serde_json::from_slice(&bytes).ok().map(Document)
}
pub(super) fn parse(source: &str, raw: &str) -> Result<Parsed, Error> {
    let doc = document(raw)?;
    if source == "claude-code" {
        let oauth = &doc.0["claudeAiOauth"];
        Ok(Parsed {
            access: token(oauth.get("accessToken"))?,
            account_id: None,
            refreshable: oauth
                .get("refreshToken")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            expires_at_ms: oauth.get("expiresAt").and_then(Value::as_u64),
        })
    } else {
        let tokens = &doc.0["tokens"];
        let access = token(tokens.get("access_token"))?;
        let claims = jwt_claims(&access);
        let account_id = tokens
            .get("account_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                claims.as_ref()?.0["https://api.openai.com/auth"]["chatgpt_account_id"]
                    .as_str()
                    .map(str::to_owned)
            });
        if account_id
            .as_ref()
            .map_or(true, |s| s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err(Error::AuthUnavailable);
        }
        let expires_at_ms = tokens
            .get("expires_at_ms")
            .and_then(Value::as_u64)
            .or_else(|| claims.as_ref()?.0["exp"].as_u64()?.checked_mul(1000));
        Ok(Parsed {
            access,
            account_id,
            expires_at_ms,
            refreshable: tokens
                .get("refresh_token")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
        })
    }
}

pub(super) async fn refresh(
    client: &reqwest::Client,
    source: &str,
    raw: &str,
) -> Result<Zeroizing<String>, Error> {
    let endpoint = if source == "claude-code" {
        "https://api.anthropic.com/v1/oauth/token"
    } else {
        "https://auth.openai.com/oauth/token"
    };
    refresh_at(client, source, raw, endpoint).await
}

pub(super) async fn refresh_at(
    client: &reqwest::Client,
    source: &str,
    raw: &str,
    endpoint: &str,
) -> Result<Zeroizing<String>, Error> {
    let mut doc = document(raw)?;
    let claude = source == "claude-code";
    let refresh = token(if claude {
        doc.0["claudeAiOauth"].get("refreshToken")
    } else {
        doc.0["tokens"].get("refresh_token")
    })?;
    let client_id = if claude {
        "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
    } else {
        "app_EMoamEEZ73f0CkXaXp7hrann"
    };
    let request = if claude {
        let body = Document(
            json!({"grant_type":"refresh_token", "refresh_token":refresh.as_str(), "client_id":client_id}),
        );
        let serialized = Zeroizing::new(serde_json::to_string(&body.0).map_err(|_| Error::Io)?);
        client
            .post(endpoint)
            .header("content-type", "application/json")
            .header("anthropic-beta", "oauth-2025-04-20")
            .body(serialized.to_string())
    } else {
        client.post(endpoint).form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", client_id),
        ])
    };
    let mut response = request.send().await.map_err(|_| Error::Io)?;
    let status = response.status();
    if status.as_u16() == 401 {
        return Err(Error::AuthUnavailable);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Io)? {
        if bytes.len() + chunk.len() > 262_144 {
            return Err(Error::Io);
        }
        bytes.extend_from_slice(&chunk);
    }
    let reply = serde_json::from_slice::<Value>(&bytes)
        .map(Document)
        .map_err(|_| Error::Io)?;
    if !status.is_success() {
        return Err(
            if status.as_u16() == 400 && reply.0["error"].as_str() == Some("invalid_grant") {
                Error::AuthUnavailable
            } else {
                Error::Io
            },
        );
    }
    let access = token(reply.0.get("access_token")).map_err(|_| Error::Io)?;
    let rotated = reply
        .0
        .get("refresh_token")
        .map(|v| token(Some(v)))
        .transpose()
        .map_err(|_| Error::Io)?;
    let expires = reply
        .0
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|s| *s > 0)
        .and_then(|s| s.checked_mul(1000))
        .and_then(|s| now().checked_add(s))
        .ok_or(Error::Io)?;
    if claude {
        replace(
            &mut doc.0["claudeAiOauth"]["accessToken"],
            json!(access.as_str()),
        );
        doc.0["claudeAiOauth"]["expiresAt"] = json!(expires);
        if let Some(rotated) = rotated {
            replace(
                &mut doc.0["claudeAiOauth"]["refreshToken"],
                json!(rotated.as_str()),
            );
        }
    } else {
        replace(&mut doc.0["tokens"]["access_token"], json!(access.as_str()));
        doc.0["tokens"]["expires_at_ms"] = json!(expires);
        if let Some(rotated) = rotated {
            replace(
                &mut doc.0["tokens"]["refresh_token"],
                json!(rotated.as_str()),
            );
        }
        if let Some(id_token) = reply.0.get("id_token") {
            replace(
                &mut doc.0["tokens"]["id_token"],
                json!(token(Some(id_token)).map_err(|_| Error::Io)?.as_str()),
            );
        }
        replace(
            &mut doc.0["last_refresh"],
            json!(chrono::Utc::now().to_rfc3339()),
        );
    }
    let result = Zeroizing::new(serde_json::to_string(&doc.0).map_err(|_| Error::Io)?);
    parse(source, &result).map_err(|_| Error::Io)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};

    #[tokio::test]
    async fn claude_refresh_uses_json_and_preserves_scope_without_exporting_refresh() {
        let app = Router::new().route("/token", post(|headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
            assert!(headers.get("anthropic-beta").is_some_and(|v| v == "oauth-2025-04-20"));
            assert!(body["grant_type"] == "refresh_token");
            assert!(body["refresh_token"] == "fake-claude-refresh");
            Json(json!({"access_token":"fake-claude-new","refresh_token":"fake-claude-rotated","expires_in":3600}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let raw = json!({"claudeAiOauth":{"accessToken":"fake-old","refreshToken":"fake-claude-refresh","expiresAt":1,"scopes":["user:inference"],"organizationUuid":"fake-organization"}}).to_string();
        let updated = refresh_at(&reqwest::Client::new(), "claude-code", &raw, &endpoint)
            .await
            .unwrap();
        let parsed = parse("claude-code", &updated).unwrap();
        assert!(parsed.access.as_str() == "fake-claude-new");
        let doc = document(&updated).unwrap();
        assert!(doc.0["claudeAiOauth"]["organizationUuid"] == "fake-organization");
        assert!(doc.0["claudeAiOauth"]["scopes"][0] == "user:inference");
        assert!(doc.0["claudeAiOauth"]["refreshToken"] == "fake-claude-rotated");
        server.abort();
    }

    #[tokio::test]
    async fn definitive_invalid_grant_is_distinct_from_transient_server_failure() {
        let app = Router::new()
            .route("/invalid", post(|| async { (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid_grant","description":"fake-private-upstream-detail"}))).into_response() }))
            .route("/transient", post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"temporary"}))).into_response() }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let raw = json!({"tokens":{"access_token":"fake-old","refresh_token":"fake-refresh","account_id":"fake-account","expires_at_ms":1}}).to_string();
        let client = reqwest::Client::new();
        assert!(matches!(
            refresh_at(&client, "codex-cli", &raw, &format!("{base}/invalid")).await,
            Err(Error::AuthUnavailable)
        ));
        assert!(matches!(
            refresh_at(&client, "codex-cli", &raw, &format!("{base}/transient")).await,
            Err(Error::Io)
        ));
        server.abort();
    }
}
