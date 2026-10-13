use openside_core::{CredentialAuth, CredentialAuthKind};
use openside_proto::AccountUsageWindow;
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerError {
    Configuration,
    Transport,
    Wire,
}

impl std::fmt::Display for BrokerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration => "Broker configuration unavailable",
            Self::Transport => "Broker request unavailable",
            Self::Wire => "Broker response incompatible",
        })
    }
}
impl std::error::Error for BrokerError {}

impl std::fmt::Debug for Broker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Broker")
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("auth", &"<redacted>")
            .finish_non_exhaustive()
    }
}

pub(crate) struct Document(pub Value);
impl Drop for Document {
    fn drop(&mut self) {
        fn erase(value: &mut Value) {
            match value {
                Value::String(value) => value.zeroize(),
                Value::Array(values) => values.iter_mut().for_each(erase),
                Value::Object(values) => values.values_mut().for_each(erase),
                _ => {}
            }
        }
        erase(&mut self.0);
    }
}

pub(crate) struct Broker {
    client: reqwest::Client,
    url: String,
    token: Zeroizing<String>,
}

pub(crate) struct Account {
    pub id: String,
    pub provider: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub windows: Vec<AccountUsageWindow>,
    pub blocked_until_ms: Option<f64>,
    pub model_blocks: Vec<(String, u64)>,
    pub auth: CredentialAuth,
}

fn read_token(path: PathBuf) -> Option<Zeroizing<String>> {
    use std::io::Read;
    let mut bytes = Zeroizing::new(Vec::new());
    std::fs::File::open(path)
        .ok()?
        .take(16_385)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 16_384 {
        return None;
    }
    let value = std::str::from_utf8(&bytes).ok()?.trim();
    (!value.is_empty()).then(|| Zeroizing::new(value.to_owned()))
}

fn variable(
    name: &str,
    env: &impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<Zeroizing<String>> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    env(name)?
        .into_string()
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(Zeroizing::new)
}

impl Broker {
    pub(crate) fn configured(
        credentials: &Value,
        env: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Option<Self>, BrokerError> {
        let config = &credentials["omp_broker"];
        let mut token = variable("OPENSIDE_BROKER_TOKEN", &env).or_else(|| {
            config["token_env"]
                .as_str()
                .and_then(|name| variable(name, &env))
        });
        if token.is_none() {
            if let Some(path) = config["token_file"].as_str() {
                token = read_token(PathBuf::from(path));
            }
        }
        if token.is_none() && config["token_file"].is_null() {
            if let Some(home) = env("HOME").or_else(|| env("USERPROFILE")) {
                token = read_token(PathBuf::from(home).join(".omp/auth-broker.token"));
            }
        }
        let Some(token) = token else {
            return Ok(None);
        };
        let environment_url = env("OPENSIDE_BROKER_URL")
            .and_then(|v| v.into_string().ok())
            .filter(|v| !v.is_empty());
        let url = environment_url
            .or_else(|| config["url"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "http://127.0.0.1:8765".into());
        let parsed = url::Url::parse(&url).map_err(|_| BrokerError::Configuration)?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !(parsed.scheme() == "https"
                || (parsed.scheme() == "http"
                    && parsed.host_str().is_some_and(|host| {
                        host == "localhost"
                            || host
                                .parse::<std::net::IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())
                    })))
        {
            return Err(BrokerError::Configuration);
        }
        let timeout = config["timeout_seconds"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(10.0)
            .clamp(0.1, 60.0);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs_f64(timeout))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| BrokerError::Configuration)?;
        Ok(Some(Self {
            client,
            url: url.trim_end_matches('/').into(),
            token,
        }))
    }

    async fn get(&self, path: &str) -> Result<Document, BrokerError> {
        let response = self
            .client
            .get(format!("{}{path}", self.url))
            .bearer_auth(&*self.token)
            .send()
            .await
            .map_err(|_| BrokerError::Transport)?;
        Self::document(response).await
    }

    async fn document(mut response: reqwest::Response) -> Result<Document, BrokerError> {
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|size| size > 1_048_576)
        {
            return Err(BrokerError::Transport);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| BrokerError::Transport)? {
            if bytes.len().saturating_add(chunk.len()) > 1_048_576 {
                return Err(BrokerError::Wire);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map(Document)
            .map_err(|_| BrokerError::Wire)
    }

    pub(crate) async fn accounts(&self) -> Result<Vec<Account>, BrokerError> {
        let snapshot = self.get("/v1/snapshot").await?;
        validate_snapshot(&snapshot.0)?;
        let entries = snapshot.0["credentials"]
            .as_array()
            .filter(|rows| rows.len() <= 256)
            .ok_or(BrokerError::Wire)?;
        let usage = self.get("/v1/usage").await?;
        validate_usage(&usage.0)?;
        let mut accounts = Vec::with_capacity(entries.len());
        for entry in entries {
            let id = entry["id"].as_u64().ok_or(BrokerError::Wire)?;
            let provider = entry["provider"]
                .as_str()
                .filter(|p| !p.is_empty() && p.len() <= 64)
                .ok_or(BrokerError::Wire)?;
            let credential = &entry["credential"];
            let (kind, value) = match credential["type"].as_str() {
                Some("api_key") => (CredentialAuthKind::ApiKey, credential["key"].as_str()),
                Some("oauth") => (
                    match provider {
                        "anthropic" => CredentialAuthKind::ClaudeCode,
                        "openai-codex" => CredentialAuthKind::CodexCli,
                        _ => CredentialAuthKind::Bearer,
                    },
                    credential["access"].as_str(),
                ),
                _ => return Err(BrokerError::Wire),
            };
            let value = value
                .filter(|v| !v.is_empty() && v.len() <= 16_384)
                .ok_or(BrokerError::Wire)?;
            let windows = usage.0["reports"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|report| {
                    report["provider"].as_str() == Some(provider)
                        && report["metadata"]["credentialId"].as_u64() == Some(id)
                })
                .flat_map(|report| report["limits"].as_array().into_iter().flatten())
                .take(16)
                .enumerate()
                .map(|(index, limit)| {
                    let amount = &limit["amount"];
                    let percent = amount["usedFraction"]
                        .as_f64()
                        .map(|v| v * 100.0)
                        .or_else(|| {
                            amount["remainingFraction"]
                                .as_f64()
                                .map(|v| (1.0 - v) * 100.0)
                        })
                        .or_else(|| {
                            let used = amount["used"].as_f64()?;
                            let maximum = amount["limit"].as_f64()?;
                            (maximum > 0.0).then_some(used / maximum * 100.0)
                        })
                        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
                    AccountUsageWindow {
                        name: format!("Window {}", index + 1),
                        used_percent: percent,
                        reset_at_ms: limit["window"]["resetsAt"]
                            .as_f64()
                            .filter(|v| v.is_finite() && *v >= 0.0),
                    }
                })
                .collect();
            let blocked_until_ms = entry["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|block| block["blockScope"].as_str().map_or(true, str::is_empty))
                .filter_map(|block| block["blockedUntilMs"].as_f64())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .max_by(f64::total_cmp);
            let model_blocks = entry["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| {
                    let scope = block["blockScope"].as_str().filter(|scope| {
                        !scope.is_empty()
                            && scope.len() <= 256
                            && !scope.chars().any(char::is_control)
                    })?;
                    let until = block["blockedUntilMs"]
                        .as_f64()
                        .filter(|value| value.is_finite() && *value >= 0.0)?;
                    Some((scope.to_owned(), until.min(u64::MAX as f64) as u64))
                })
                .take(64)
                .collect();
            let models = entry["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|model| model.len() <= 256 && !model.chars().any(char::is_control))
                .take(64)
                .map(str::to_owned)
                .collect();
            accounts.push(Account {
                id: id.to_string(),
                provider: provider.into(),
                models,
                windows,
                blocked_until_ms,
                model_blocks,
                enabled: !entry["disabled"].as_bool().unwrap_or(false)
                    && !credential["disabled"].as_bool().unwrap_or(false),
                auth: CredentialAuth {
                    kind,
                    value: value.into(),
                    upstream_account_id: credential["accountId"].as_str().map(str::to_owned),
                    expires_at_ms: credential["expires"]
                        .as_f64()
                        .filter(|v| v.is_finite() && *v >= 0.0)
                        .map(|v| v.min(u64::MAX as f64) as u64),
                },
            });
        }
        Ok(accounts)
    }

    pub(crate) async fn refresh(&self, id: &str) -> Result<(), BrokerError> {
        if id.parse::<u64>().is_err() {
            return Err(BrokerError::Wire);
        }
        let response = self
            .client
            .post(format!("{}/v1/credential/{id}/refresh", self.url))
            .bearer_auth(&*self.token)
            .send()
            .await
            .map_err(|_| BrokerError::Transport)?;
        response
            .status()
            .is_success()
            .then_some(())
            .ok_or(BrokerError::Transport)
    }

    pub(crate) async fn block(
        &self,
        id: &str,
        provider: &str,
        model: Option<&str>,
        until: u64,
    ) -> Result<(), BrokerError> {
        if id.parse::<u64>().is_err() {
            return Err(BrokerError::Wire);
        }
        let response = self.client.post(format!("{}/v1/credential/{id}/block", self.url)).bearer_auth(&*self.token)
            .json(&serde_json::json!({"providerKey":provider,"blockScope":model.unwrap_or(""),"blockedUntilMs":until,"updatedAtMs":chrono::Utc::now().timestamp_millis()}))
            .send().await.map_err(|_| BrokerError::Transport)?;
        let response = Self::document(response).await?;
        response.0["ok"]
            .as_bool()
            .filter(|ok| *ok)
            .map(|_| ())
            .ok_or(BrokerError::Wire)
    }
}

fn require(valid: bool) -> Result<(), BrokerError> {
    valid.then_some(()).ok_or(BrokerError::Wire)
}

fn validate_block(value: &Value) -> Result<(), BrokerError> {
    require(
        value["providerKey"].as_str().is_some_and(|v| !v.is_empty())
            && value["blockScope"].is_string()
            && value["blockedUntilMs"].is_number()
            && value.get("updatedAtMs").map_or(true, Value::is_number),
    )
}

fn validate_snapshot(value: &Value) -> Result<(), BrokerError> {
    require(value["generation"].is_u64() && value["serverNowMs"].is_number())?;
    for entry in value["credentials"].as_array().ok_or(BrokerError::Wire)? {
        require(entry["id"].is_u64() && entry["provider"].as_str().is_some_and(|v| !v.is_empty()))?;
        let credential = &entry["credential"];
        match credential["type"].as_str() {
            Some("oauth") => require(
                credential["access"].as_str().is_some_and(|v| !v.is_empty())
                    && credential["expires"].is_number(),
            )?,
            Some("api_key") => require(credential["key"].as_str().is_some_and(|v| !v.is_empty()))?,
            _ => return Err(BrokerError::Wire),
        }
        for name in ["accountId", "email", "orgId"] {
            require(credential.get(name).map_or(true, Value::is_string))?;
        }
        if let Some(blocks) = entry.get("blocks") {
            for block in blocks.as_array().ok_or(BrokerError::Wire)? {
                validate_block(block)?;
            }
        }
    }
    Ok(())
}

fn validate_usage(value: &Value) -> Result<(), BrokerError> {
    for report in value["reports"].as_array().ok_or(BrokerError::Wire)? {
        require(report["provider"].is_string())?;
        if let Some(metadata) = report.get("metadata") {
            require(
                metadata.is_object() && metadata.get("credentialId").map_or(true, Value::is_u64),
            )?;
        }
        for limit in report["limits"].as_array().ok_or(BrokerError::Wire)? {
            require(limit["scope"].is_object() && limit["amount"].is_object())?;
            for name in [
                "used",
                "limit",
                "remaining",
                "usedFraction",
                "remainingFraction",
            ] {
                require(limit["amount"].get(name).map_or(true, Value::is_number))?;
            }
            if let Some(window) = limit.get("window") {
                require(
                    window.is_object() && window.get("resetsAt").map_or(true, Value::is_number),
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::Arc;

    pub(crate) const ACCESS: &str = "broker-access-canary-71492";
    pub(crate) const REFRESH: &str = "broker-refresh-sentinel-92517";
    pub(crate) const TOKEN: &str = "broker-token-canary-59317";

    pub(crate) fn snapshot() -> Value {
        json!({"generation":1,"serverNowMs":0,"credentials":[{"id":1,"provider":"openai-codex",
            "credential":{"type":"oauth","access":ACCESS,"refresh":REFRESH,"expires":4102444800000u64},
            "models":["test-model"],"blocks":[{"providerKey":"openai-codex","blockScope":"chat","blockedUntilMs":1000}]}]})
    }
    fn usage() -> Value {
        json!({"reports":[{"provider":"openai-codex","metadata":{"credentialId":1},"limits":[
            {"scope":{},"amount":{"usedFraction":0.25},"window":{"resetsAt":1000,"label":ACCESS}}]}]})
    }
    #[derive(Clone)]
    struct Responses {
        snapshot: Value,
        usage: Value,
        block: Value,
        requests: Arc<Mutex<Vec<(String, Value)>>>,
    }
    pub(crate) struct FakeBroker {
        pub(crate) url: String,
        requests: Arc<Mutex<Vec<(String, Value)>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for FakeBroker {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl FakeBroker {
        pub(crate) async fn start(snapshot: Value, usage: Value, block: Value) -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let state = Responses {
                snapshot,
                usage,
                block,
                requests: requests.clone(),
            };
            let app = Router::new()
                .route(
                    "/v1/snapshot",
                    get(
                        |State(s): State<Responses>, headers: HeaderMap| async move {
                            assert!(headers["authorization"] == format!("Bearer {TOKEN}"));
                            s.requests.lock().push(("snapshot".into(), Value::Null));
                            Json(s.snapshot)
                        },
                    ),
                )
                .route(
                    "/v1/usage",
                    get(|State(s): State<Responses>| async move {
                        s.requests.lock().push(("usage".into(), Value::Null));
                        Json(s.usage)
                    }),
                )
                .route(
                    "/v1/credential/1/block",
                    post(
                        |State(s): State<Responses>, Json(body): Json<Value>| async move {
                            s.requests.lock().push(("block".into(), body));
                            Json(s.block)
                        },
                    ),
                )
                .route(
                    "/v1/credential/1/refresh",
                    post(|| async {
                        (
                            StatusCode::BAD_GATEWAY,
                            format!("{ACCESS} {REFRESH} {TOKEN}"),
                        )
                    }),
                )
                .with_state(state);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self {
                url,
                requests,
                task,
            }
        }
        fn broker(&self) -> Broker {
            Broker::configured(&json!({"omp_broker":{"url":self.url}}), |name| {
                (name == "OPENSIDE_BROKER_TOKEN").then(|| TOKEN.into())
            })
            .unwrap()
            .unwrap()
        }
    }
    fn redacted(text: &str) {
        for secret in [ACCESS, REFRESH, TOKEN] {
            assert!(!text.contains(secret), "secret leaked");
        }
    }

    #[tokio::test]
    async fn pinned_wire_accepts_snapshot_usage_and_scoped_block() {
        let mut data = snapshot();
        data["upstreamAddition"] = json!({"ignored":true});
        let server = FakeBroker::start(data, usage(), json!({"ok":true,"extra":true})).await;
        let broker = server.broker();
        let accounts = broker.accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "1");
        assert_eq!(accounts[0].auth.value, ACCESS);
        assert_eq!(accounts[0].windows[0].used_percent, Some(25.0));
        assert_eq!(accounts[0].windows[0].reset_at_ms, Some(1000.0));
        assert_eq!(accounts[0].model_blocks, vec![("chat".into(), 1000)]);
        broker
            .block("1", "openai-codex", Some("chat"), 1000)
            .await
            .unwrap();
        let requests = server.requests.lock();
        assert_eq!(
            requests
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["snapshot", "usage", "block"]
        );
        validate_block(&requests[2].1).unwrap();
        assert_eq!(requests[2].1["blockScope"], "chat");
        assert_eq!(requests[2].1["blockedUntilMs"], 1000);
        redacted(&format!("{broker:?} {accounts:?} {:?}", accounts[0].auth));
        redacted(&serde_json::to_string(&accounts[0].windows).unwrap());
    }

    #[tokio::test]
    async fn breaking_snapshot_fields_fail_closed_with_typed_redacted_error() {
        for (parent, key) in [
            ("", "generation"),
            ("", "serverNowMs"),
            ("", "credentials"),
            ("/credentials/0", "id"),
            ("/credentials/0", "provider"),
            ("/credentials/0/credential", "type"),
            ("/credentials/0/credential", "access"),
            ("/credentials/0/credential", "expires"),
        ] {
            for change in ["removed", "renamed", "type"] {
                let mut data = snapshot();
                let object = data.pointer_mut(parent).unwrap().as_object_mut().unwrap();
                let old = object.remove(key).unwrap();
                match change {
                    "renamed" => {
                        object.insert("renamed".into(), old);
                    }
                    "type" => {
                        object.insert(key.into(), json!({"secret":REFRESH}));
                    }
                    _ => {}
                }
                let server = FakeBroker::start(data, usage(), json!({"ok":true})).await;
                let error = server.broker().accounts().await.unwrap_err();
                assert_eq!(error, BrokerError::Wire, "{parent}/{key}: {change}");
                redacted(&format!("{error:?} {error}"));
            }
        }
        let mut data = snapshot();
        data["credentials"][0]["credential"]["type"] = json!("unknown-auth");
        let server = FakeBroker::start(data, usage(), json!({"ok":true})).await;
        assert_eq!(
            server.broker().accounts().await.unwrap_err(),
            BrokerError::Wire
        );
    }

    #[tokio::test]
    async fn breaking_usage_and_block_responses_fail_closed() {
        for bad in [
            json!({}),
            json!({"renamed":[]}),
            json!({"reports":{"secret":ACCESS}}),
            json!({"reports":[{"provider":"openai-codex","limits":[{"scope":{},"amount":{"usedFraction":REFRESH}}]}]}),
        ] {
            let server = FakeBroker::start(snapshot(), bad, json!({"ok":true})).await;
            let error = server.broker().accounts().await.unwrap_err();
            assert_eq!(error, BrokerError::Wire);
            redacted(&format!("{error:?} {error}"));
        }
        for bad in [
            json!({}),
            json!({"renamed":true}),
            json!({"ok":REFRESH}),
            json!({"ok":false}),
        ] {
            let server = FakeBroker::start(snapshot(), usage(), bad).await;
            assert_eq!(
                server.broker().block("1", "openai-codex", None, 1).await,
                Err(BrokerError::Wire)
            );
        }
    }

    #[tokio::test]
    async fn broker_errors_and_debug_never_expose_secrets() {
        let server = FakeBroker::start(snapshot(), usage(), json!({"ok":true})).await;
        let broker = server.broker();
        let error = broker.refresh("1").await.unwrap_err();
        assert_eq!(error, BrokerError::Transport);
        redacted(&format!("{broker:?} {error:?} {error}"));
        let error = Broker::configured(
            &json!({"omp_broker":{"url":format!("https://{TOKEN}@example.test/{ACCESS}")}}),
            |name| (name == "OPENSIDE_BROKER_TOKEN").then(|| TOKEN.into()),
        )
        .unwrap_err();
        assert_eq!(error, BrokerError::Configuration);
        redacted(&format!("{error:?} {error}"));
    }

    #[test]
    fn token_and_url_resolution_preserve_environment_file_and_default_precedence() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("configured.token");
        std::fs::write(&file, format!("  {TOKEN}\n")).unwrap();
        std::fs::create_dir(root.path().join(".omp")).unwrap();
        std::fs::write(root.path().join(".omp/auth-broker.token"), REFRESH).unwrap();
        let config = json!({"omp_broker":{"token_env":"CUSTOM_TOKEN","token_file":file,"url":"http://127.0.0.1:11"}});
        let mut env = std::collections::HashMap::<&str, std::ffi::OsString>::new();
        env.insert("HOME", root.path().as_os_str().to_owned());
        let resolve =
            |config: &Value, env: &std::collections::HashMap<&str, std::ffi::OsString>| {
                Broker::configured(config, |name| env.get(name).cloned())
                    .unwrap()
                    .unwrap()
            };
        let broker = resolve(&config, &env);
        assert!(broker.token.as_str() == TOKEN);
        assert_eq!(broker.url, "http://127.0.0.1:11");
        env.insert("CUSTOM_TOKEN", ACCESS.into());
        assert!(resolve(&config, &env).token.as_str() == ACCESS);
        env.insert("OPENSIDE_BROKER_TOKEN", "environment-override".into());
        env.insert("OPENSIDE_BROKER_URL", "http://127.0.0.1:12".into());
        let broker = resolve(&config, &env);
        assert!(broker.token.as_str() == "environment-override");
        assert_eq!(broker.url, "http://127.0.0.1:12");
        env.insert("OPENSIDE_BROKER_TOKEN", " \n".into());
        env.insert("OPENSIDE_BROKER_URL", "".into());
        assert!(resolve(&config, &env).token.as_str() == ACCESS);
        env.remove("CUSTOM_TOKEN");
        let broker = resolve(&json!({}), &env);
        assert!(broker.token.as_str() == REFRESH);
        assert_eq!(broker.url, "http://127.0.0.1:8765");
        assert!(std::fs::read_to_string(file).unwrap() == format!("  {TOKEN}\n"));
        env.remove("HOME");
        env.insert("USERPROFILE", root.path().as_os_str().to_owned());
        assert!(resolve(&json!({}), &env).token.as_str() == REFRESH);
    }

    #[test]
    fn missing_token_is_disabled_without_error_and_invalid_env_names_are_not_read() {
        let root = tempfile::tempdir().unwrap();
        let config = json!({"omp_broker":{"token_file":root.path().join("missing"),"token_env":"invalid-secret-name","url":"invalid-url"}});
        assert!(Broker::configured(&config, |name| {
            assert_ne!(name, "invalid-secret-name");
            None
        })
        .unwrap()
        .is_none());
        assert!(Broker::configured(&json!({}), |_| None).unwrap().is_none());
        let empty = root.path().join("empty");
        std::fs::write(&empty, " \n").unwrap();
        assert!(
            Broker::configured(&json!({"omp_broker":{"token_file":empty}}), |_| None)
                .unwrap()
                .is_none()
        );
    }
}
