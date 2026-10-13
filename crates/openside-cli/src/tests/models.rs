use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

const KEY: &str = "model-cli-secret-canary-abcd";

#[tokio::test]
async fn model_catalog_and_key_commands_use_public_routes_and_stdin_without_echo() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("models.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new().fallback(|State(seen): State<Seen>, request: Request<Body>| async move {
        let method = request.method().to_string();
        let path = request.uri().path().to_owned();
        let bytes = to_bytes(request.into_body(), 16 * 1024).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        seen.lock().push((method.clone(), path.clone(), body));
        match (method.as_str(), path.as_str()) {
            ("GET", "/v1/settings/models") => respond(200, json!({"providers":[{"provider":"anthropic","pool":{"provider":"anthropic","accounts":[{"source":"vault","account_id":"key1"}]},"models":[{"id":"anthropic:test-ready","label":"test-ready","available":true,"reason":"Vault API key available","source":"vault","account_id":"key1","accounts":[]}]}],"default_model":null,"fallback_models":[],"cached_for_ms":5000,"truncated":false})),
            ("POST", "/v1/settings/keys/anthropic") => respond(201, json!({"provider":"anthropic","key_id":"key1","label":"Work","enabled":true,"reserve_percent":10,"last_four":"abcd","created_at":"2026-01-01T00:00:00Z"})),
            ("GET", "/v1/settings/keys") => respond(200, json!({"keys":[{"provider":"anthropic","key_id":"key1","label":"Work","enabled":true,"reserve_percent":10,"last_four":"abcd","created_at":"2026-01-01T00:00:00Z"}]})),
            ("DELETE", "/v1/settings/keys/anthropic/key1") => Response::builder().status(204).body(Body::empty()).unwrap(),
            ("PUT", "/v1/settings/keys/anthropic/key1") | ("PATCH", "/v1/settings/keys/anthropic/key1") => respond(200, json!({"provider":"anthropic","key_id":"key1","label":"Work","enabled":true,"reserve_percent":10,"last_four":"abcd","created_at":"2026-01-01T00:00:00Z"})),
            ("PUT", "/v1/settings/models") => respond(200, json!({"default_model":"anthropic:test-ready","fallback_models":[],"restart_required":false,"effective":"new_sessions_and_home_next_turn"})),
            ("GET", "/v1/settings/pools") => respond(200,json!({"pools":[{"provider":"anthropic","accounts":[{"source":"vault","account_id":"key1"}]}]})),
            ("PUT", "/v1/settings/pools/anthropic") => respond(200,json!({"provider":"anthropic","accounts":[{"source":"vault","account_id":"key1"}]})),
            ("PUT", "/v1/sessions/session-1/account") => respond(200,json!({"session_id":"session-1","provider":"anthropic","account":{"source":"vault","account_id":"key1"},"explicit":true,"effective":"next_request"})),
            ("POST", "/v1/settings/subscriptions/codex-cli") | ("GET", "/v1/settings/subscriptions/codex-cli/sub1") | ("PATCH", "/v1/settings/subscriptions/codex-cli/sub1") => respond(if method=="POST" {201} else {200},json!({"source":"codex-cli","provider":"openai-codex","account_id":"sub1","label":"Work","managed":true,"installed":true,"logged_in":false,"enabled":true,"available":false,"reason":"Log in externally","login_command":"CODEX_HOME=/fake codex login","created_at":"2026-01-01T00:00:00Z","reserve_percent":10})),
            ("DELETE", "/v1/settings/subscriptions/codex-cli/sub1") => Response::builder().status(204).body(Body::empty()).unwrap(),
            _ => respond(404, json!({"code":"not_found","message":"not found"})),
        }
    }).with_state(seen.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for args in [
        vec!["models"],
        vec!["key", "set", "anthropic", "--label", "Work"],
        vec!["key", "list"],
        vec!["key", "rm", "anthropic", "key1"],
        vec!["key", "set", "anthropic", "--key-id", "key1"],
        vec!["key", "update", "anthropic", "key1", "--enabled", "false"],
        vec!["models", "set", "anthropic:test-ready"],
        vec!["models", "pool", "list"],
        vec!["models", "pool", "set", "anthropic", "vault:key1"],
        vec!["models", "account", "session-1", "vault:key1"],
        vec!["accounts", "add", "codex-cli", "--label", "Work"],
        vec!["accounts", "show", "codex-cli", "sub1"],
        vec!["accounts", "enable", "codex-cli", "sub1"],
        vec!["accounts", "disable", "codex-cli", "sub1"],
        vec!["accounts", "rm", "codex-cli", "sub1"],
    ] {
        let mut argv = vec![
            OsString::from("openside"),
            OsString::from("--socket"),
            socket.clone().into_os_string(),
        ];
        argv.extend(args.iter().map(OsString::from));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let input = format!("{KEY}\n");
        let code = run_with_input(argv, &mut input.as_bytes(), &mut out, &mut err).await;
        assert_eq!(code, 0);
        assert!(!String::from_utf8_lossy(&out).contains(KEY));
        assert!(!String::from_utf8_lossy(&err).contains(KEY));
    }
    let requests = seen.lock();
    assert_eq!(requests[0].1, "/v1/settings/models");
    assert_eq!(requests[1].0, "POST");
    assert_eq!(requests[1].1, "/v1/settings/keys/anthropic");
    assert!(requests[1].2["key"] == KEY);
    assert_eq!(requests[2].1, "/v1/settings/keys");
    assert_eq!(requests[3].0, "DELETE");
    assert_eq!(requests[4].0, "PUT");
    assert!(requests[4].2["key"] == KEY);
    assert_eq!(requests[5].2["enabled"], false);
    assert_eq!(requests[6].2["default_model"], "anthropic:test-ready");
    assert_eq!(requests[8].2["accounts"][0]["account_id"], "key1");
    assert_eq!(requests[9].2["account"]["source"], "vault");
    assert_eq!(requests[10].2["label"], "Work");
    assert_eq!(requests[12].2["enabled"], true);
    assert_eq!(requests[13].2["enabled"], false);
    drop(requests);
    server.abort();
}

#[tokio::test]
async fn key_set_rejects_argv_secrets_without_echoing_them() {
    let _environment = Environment::new().await;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with_input(
        ["openside", "key", "set", "anthropic", KEY],
        &mut std::io::empty(),
        &mut out,
        &mut err,
    )
    .await;
    assert_eq!(code, 2);
    assert!(!String::from_utf8_lossy(&out).contains(KEY));
    assert!(!String::from_utf8_lossy(&err).contains(KEY));
    assert!(String::from_utf8_lossy(&err).contains("stdin"));
}
