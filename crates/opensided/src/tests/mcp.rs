use super::*;
use openside_vault::{PassphraseLock, PlatformLock, Secret, Vault};

const TOKEN: &str = "mcp-fake-token-5551234";

async fn send(router: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

async fn fixture(root: &Path, runtime: &TestRuntime) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.vault.path = Some(root.join("mcp-vault"));
    config.loaded.config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    config.loaded.config.models.default = Some("script:test".into());
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let vault = Vault::create(
        root.join("mcp-vault"),
        root.join("mcp-vault-audit"),
        &Secret::new("mcp-test-passphrase".into()),
        Box::new(PassphraseLock::default()) as Box<dyn PlatformLock + Send>,
    )
    .unwrap();
    *daemon.shared.registry.vault.vault.lock().await = Some(vault);
    daemon
}

/// A stdio MCP server in POSIX sh: `echo` returns its text, `token_length` proves the Vault
/// secret arrived as an environment variable without ever printing it, and `bad` has a
/// schema openside must refuse.
const STDIO_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake-stdio","version":"1"}}}\n' "$id" ;;
    tools/list)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}},{"name":"token_length","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"bad","inputSchema":{"type":"string"}}]}}\n' "$id" ;;
    tools/call)
      tool=$(printf '%s' "$line" | sed -n 's/.*"name":"\([^"]*\)".*/\1/p')
      if [ "$tool" = echo ]; then
        text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echo: %s"}]}}\n' "$id" "$text"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"length %s"}]}}\n' "$id" "${#FAKE_TOKEN}"
      fi ;;
    *)
      if [ -n "$id" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"unknown"}}\n' "$id"
      fi ;;
  esac
done
"#;

fn stdio_server(root: &Path) -> std::path::PathBuf {
    let path = root.join("fake-mcp.sh");
    std::fs::write(&path, STDIO_SERVER).unwrap();
    path
}

fn spec_names(runtime: &TestRuntime) -> Vec<String> {
    runtime
        .0
        .lock()
        .opened
        .last()
        .map(|spec| spec.tools.iter().map(|tool| tool.name.clone()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn stdio_server_tools_reach_the_chat_and_calls_need_approval() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = fixture(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    let remote = api::router(daemon.shared.clone(), false);
    let script = stdio_server(root.path());
    let body = json!({
        "label": "Fake tools",
        "transport": "stdio",
        "command": ["sh", script],
        "secrets": {"FAKE_TOKEN": TOKEN}
    });

    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let refused = remote
        .clone()
        .oneshot(request(
            "POST",
            "/v1/mcp/servers",
            body.clone(),
            Some(&admin),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(refused).await["code"], "local_access_required");

    let (status, server) = send(&local, "POST", "/v1/mcp/servers", body).await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    assert_eq!(server["id"], "fake-tools");
    assert_eq!(server["status"], "connected", "{server}");
    assert_eq!(server["server_name"], "fake-stdio");
    assert_eq!(server["secrets"], json!(["FAKE_TOKEN"]));
    let exposed: Vec<&str> = server["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["exposed_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        exposed,
        ["mcp__fake-tools__echo", "mcp__fake-tools__token_length"]
    );
    assert!(!server.to_string().contains(TOKEN));
    let config = std::fs::read_to_string(root.path().join("config.toml")).unwrap();
    assert!(config.contains("[[mcp.servers]]") && config.contains("FAKE_TOKEN"));
    assert!(!config.contains(TOKEN));

    let (status, list) = send(&local, "GET", "/v1/mcp/servers", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["servers"][0]["id"], "fake-tools");

    // A chat turn opens the runtime with the server's tools next to the built-in ones.
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let (status, _) = send(
        &local,
        "POST",
        "/v1/sessions/session/messages",
        json!({"text": "use the echo tool", "mode": "queue"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    until(|| !runtime.0.lock().sent.is_empty()).await;
    let names = spec_names(&runtime);
    assert!(
        names.iter().any(|name| name == "mcp__fake-tools__echo"),
        "{names:?}"
    );
    assert!(names.iter().any(|name| name == "read"));

    // Guard mode: the first call asks the person, the approved retry runs once.
    let host = host(&daemon, &session).await;
    let mut invocation = call(
        "mcp__fake-tools__echo",
        json!({"text": "hello"}),
        "echo-call",
    );
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::Approval,
    } = host.call(invocation.clone()).await
    else {
        panic!("expected approval");
    };
    let question = daemon.shared.db.state(&session).await.unwrap().tickets[&ticket]
        .summary
        .clone();
    assert_eq!(
        question,
        "Allow openside to use the echo tool from Fake tools?"
    );
    assert_eq!(
        person_resolve(&daemon, &ticket, json!(true)).await.status(),
        StatusCode::OK
    );
    invocation.approval_ticket = Some(ticket);
    let ToolOutcome::Done { for_model_json } = host.call(invocation.clone()).await else {
        panic!("expected the approved call to run");
    };
    let result: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(result["content"], "echo: hello");
    assert_eq!(result["untrusted"], true);
    assert_eq!(result["server"], "fake-tools");
    assert!(matches!(
        host.call(invocation).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_already_used"
    ));

    // Full access runs without a ticket; the secret arrived as an env var.
    let full = create_session(&daemon, "full", PolicyMode::Full, None).await;
    let ToolOutcome::Done { for_model_json } = crate::tests::host(&daemon, &full)
        .await
        .call(call("mcp__fake-tools__token_length", json!({}), "len-call"))
        .await
    else {
        panic!("expected full access to run the tool");
    };
    let result: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(result["content"], format!("length {}", TOKEN.len()));

    // Read-only sessions neither see nor run MCP tools, whatever the server's hints say.
    let read_only = create_session(&daemon, "read-only", PolicyMode::ReadOnly, None).await;
    let read_only_host = crate::tests::host(&daemon, &read_only).await;
    assert!(!read_only_host
        .specs()
        .iter()
        .any(|spec| spec.name.starts_with("mcp__")));
    assert!(matches!(
        read_only_host
            .call(call("mcp__fake-tools__token_length", json!({}), "ro-call"))
            .await,
        ToolOutcome::Denied { .. }
    ));

    // Disabling the server removes its tools; deleting it removes the Vault secret.
    let (status, server) = send(
        &local,
        "PATCH",
        "/v1/mcp/servers/fake-tools",
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(server["status"], "disabled");
    assert!(daemon
        .shared
        .registry
        .get("mcp__fake-tools__echo")
        .is_none());
    let (status, _) = send(&local, "DELETE", "/v1/mcp/servers/fake-tools", Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let guard = daemon.shared.registry.vault.vault.lock().await;
    assert!(!guard
        .as_ref()
        .unwrap()
        .is_mcp_secret("mcp:fake-tools:fake_token"));
    drop(guard);
    let (status, _) = send(&local, "GET", "/v1/mcp/servers/fake-tools", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    drop(host);
    daemon.shutdown().await;
}

/// Streamable HTTP: JSON for the handshake, a session id, 202 for notifications and an SSE
/// stream for the call; every request must carry the Vault-held header.
async fn http_server() -> (String, Arc<Mutex<Vec<String>>>) {
    use axum::{http::HeaderMap, response::IntoResponse, routing::post};
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    let app = axum::Router::new().route(
        "/mcp",
        post(move |headers: HeaderMap, body: String| {
            let log = log.clone();
            async move {
                let message: Value = serde_json::from_str(&body).unwrap();
                let method = message["method"].as_str().unwrap_or_default().to_owned();
                log.lock().push(method.clone());
                let authorized = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    == Some(&format!("Bearer {TOKEN}"));
                if !authorized {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let id = message["id"].clone();
                match method.as_str() {
                    "initialize" => (
                        [("mcp-session-id", "session-1")],
                        axum::Json(json!({"jsonrpc":"2.0","id":id,"result":{
                            "protocolVersion":"2025-06-18","capabilities":{"tools":{}},
                            "serverInfo":{"name":"fake-http","version":"1"}}})),
                    )
                        .into_response(),
                    _ if headers.get("mcp-session-id").and_then(|v| v.to_str().ok())
                        != Some("session-1") =>
                    {
                        StatusCode::NOT_FOUND.into_response()
                    }
                    "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
                    "tools/list" => axum::Json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
                        {"name":"lookup","description":"Look up","inputSchema":{"type":"object",
                         "properties":{"q":{"type":"string"}}}}]}}))
                    .into_response(),
                    "tools/call" => {
                        let q = message["params"]["arguments"]["q"].as_str().unwrap_or_default();
                        let ping = json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}});
                        let result = json!({"jsonrpc":"2.0","id":id,"result":{
                            "content":[{"type":"text","text":format!("found {q}")}],
                            "structuredContent":{"hits":1}}});
                        (
                            [("content-type", "text/event-stream")],
                            format!("event: message\ndata: {ping}\n\nevent: message\ndata: {result}\n\n"),
                        )
                            .into_response()
                    }
                    _ => axum::Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"no"}}))
                        .into_response(),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}/mcp"), seen)
}

#[tokio::test]
async fn http_server_uses_vault_header_and_reads_sse_results() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = fixture(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    let (url, seen) = http_server().await;

    let (status, server) = send(
        &local,
        "POST",
        "/v1/mcp/servers",
        json!({"id":"remote","label":"Remote","transport":"http","url":url,
               "secrets":{"Authorization":"Bearer wrong"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(server["status"], "failed");
    assert!(
        server["error"].as_str().unwrap().contains("401"),
        "{server}"
    );

    let (status, server) = send(
        &local,
        "PATCH",
        "/v1/mcp/servers/remote",
        json!({"secrets":{"Authorization":format!("Bearer {TOKEN}")}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(server["status"], "connected", "{server}");
    assert_eq!(server["tools"][0]["exposed_name"], "mcp__remote__lookup");
    assert!(!server.to_string().contains(TOKEN));

    let session = create_session(&daemon, "session", PolicyMode::Full, None).await;
    let ToolOutcome::Done { for_model_json } = host(&daemon, &session)
        .await
        .call(call("mcp__remote__lookup", json!({"q":"cats"}), "lookup"))
        .await
    else {
        panic!("expected the call to run");
    };
    let result: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(result["content"], "found cats");
    assert_eq!(result["structured"], json!({"hits":1}));
    assert!(seen
        .lock()
        .iter()
        .any(|method| method == "notifications/initialized"));

    // A URL that would send credentials in clear text off this machine is refused.
    let (status, _) = send(
        &local,
        "POST",
        "/v1/mcp/servers",
        json!({"label":"Plain","transport":"http","url":"http://example.com/mcp"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    daemon.shutdown().await;
}

/// A stdio server that repeats its Vault secret everywhere it can: stderr, `serverInfo.name`,
/// a tool name, a description, a schema, results, `isError` text and JSON-RPC error text.
/// `change` creates `@FLAG@` and announces a new `extra` tool.
const LEAKY_SERVER: &str = r#"#!/bin/sh
printf 'booting with %s\n' "$FAKE_TOKEN" >&2
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  case "$method" in
    initialize)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"leaky %s","version":"1"}}}\n' "$id" "$FAKE_TOKEN" ;;
    tools/list)
      extra=''
      if [ -f '@FLAG@' ]; then extra=',{"name":"extra","inputSchema":{"type":"object"}}'; fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"%s","inputSchema":{"type":"object"}},{"name":"describe","description":"key %s","inputSchema":{"type":"object","properties":{"q":{"type":"string","description":"default %s"}}}},{"name":"fail","inputSchema":{"type":"object"}},{"name":"rpc_error","inputSchema":{"type":"object"}},{"name":"change","inputSchema":{"type":"object"}}%s]}}\n' "$id" "$FAKE_TOKEN" "$FAKE_TOKEN" "$FAKE_TOKEN" "$extra" ;;
    tools/call)
      tool=$(printf '%s' "$line" | sed -n 's/.*"name":"\([^"]*\)".*/\1/p')
      case "$tool" in
        fail)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"isError":true,"content":[{"type":"text","text":"denied for %s"}]}}\n' "$id" "$FAKE_TOKEN" ;;
        rpc_error)
          printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"boom %s","data":{"token":"%s"}}}\n' "$id" "$FAKE_TOKEN" "$FAKE_TOKEN" ;;
        change)
          : > '@FLAG@'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"changed"}]}}\n' "$id"
          printf '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\n' ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"ok %s"}]}}\n' "$id" "$FAKE_TOKEN" ;;
      esac ;;
    *)
      if [ -n "$id" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"unknown"}}\n' "$id"
      fi ;;
  esac
done
"#;

/// Registers [`LEAKY_SERVER`] as `leaky` with `FAKE_TOKEN` in the Vault.
async fn leaky_server(root: &Path, local: &axum::Router) -> Value {
    let script = root.join("leaky-mcp.sh");
    let flag = root.join("leaky-flag");
    std::fs::write(
        &script,
        LEAKY_SERVER.replace("@FLAG@", flag.to_str().unwrap()),
    )
    .unwrap();
    let (status, server) = send(
        local,
        "POST",
        "/v1/mcp/servers",
        json!({"id":"leaky","label":"Leaky","transport":"stdio","command":["sh", script],
               "secrets":{"FAKE_TOKEN": TOKEN}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{server}");
    assert_eq!(server["status"], "connected", "{server}");
    server
}

#[tokio::test]
async fn vault_secrets_never_leak_through_what_a_server_reports() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = fixture(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    let remote = api::router(daemon.shared.clone(), false);

    let server = leaky_server(root.path(), &local).await;
    let name = server["server_name"].as_str().unwrap();
    assert!(name.starts_with("leaky "), "{server}");
    let tools: Vec<&str> = server["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    // The tool named after the secret no longer has a valid name once masked.
    assert_eq!(tools, ["describe", "fail", "rpc_error", "change"]);

    // A server that prints its secret to stderr and exits: the attached tail is masked.
    let (status, crashed) = send(
        &local,
        "POST",
        "/v1/mcp/servers",
        json!({"id":"crashy","label":"Crashy","transport":"stdio",
               "command":["sh","-c","printf 'cannot use %s\\n' \"$FAKE_TOKEN\" >&2; exit 3"],
               "secrets":{"FAKE_TOKEN": TOKEN}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{crashed}");
    assert_eq!(crashed["status"], "failed", "{crashed}");
    assert!(
        crashed["error"].as_str().unwrap().contains("cannot use"),
        "{crashed}"
    );

    let (_, list) = send(&local, "GET", "/v1/mcp/servers", Value::Null).await;
    assert_eq!(list["servers"].as_array().unwrap().len(), 2);
    assert!(!list.to_string().contains(TOKEN), "{list}");
    let (_, one) = send(&local, "GET", "/v1/mcp/servers/crashy", Value::Null).await;
    assert!(!one.to_string().contains(TOKEN), "{one}");

    let session = create_session(&daemon, "leaks", PolicyMode::Full, None).await;
    let host = host(&daemon, &session).await;
    let specs: Vec<_> = host
        .specs()
        .into_iter()
        .filter(|spec| spec.name.starts_with("mcp__leaky__"))
        .collect();
    assert_eq!(specs.len(), 4, "{specs:?}");
    assert!(!format!("{specs:?}").contains(TOKEN), "{specs:?}");

    let ToolOutcome::Done { for_model_json } = host
        .call(call("mcp__leaky__describe", json!({}), "describe"))
        .await
    else {
        panic!("expected the call to run");
    };
    assert!(!for_model_json.contains(TOKEN), "{for_model_json}");

    // isError keeps the untrusted envelope, with the failure flagged and the text masked.
    let ToolOutcome::Failed { message } =
        host.call(call("mcp__leaky__fail", json!({}), "fail")).await
    else {
        panic!("expected isError to fail the call");
    };
    assert!(!message.contains(TOKEN), "{message}");
    let envelope: Value = serde_json::from_str(&message).unwrap();
    assert_eq!(envelope["is_error"], true);
    assert_eq!(envelope["untrusted"], true);
    assert_eq!(envelope["authority"], "data_only");
    assert_eq!(envelope["source"], "mcp");
    assert!(envelope["content"]
        .as_str()
        .unwrap()
        .starts_with("denied for "));

    let ToolOutcome::Failed { message } = host
        .call(call("mcp__leaky__rpc_error", json!({}), "rpc"))
        .await
    else {
        panic!("expected a JSON-RPC error to fail the call");
    };
    assert!(message.contains("boom"), "{message}");
    assert!(!message.contains(TOKEN), "{message}");

    // Over 8 MiB never reaches the server.
    let outcome = host
        .call(call(
            "mcp__leaky__describe",
            json!({"q": "x".repeat(9 * 1024 * 1024)}),
            "huge",
        ))
        .await;
    assert!(
        matches!(&outcome, ToolOutcome::Failed { message } if message.contains("too large")),
        "{outcome:?}"
    );

    // Refreshing runs the server's program with its secrets: local access only.
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let refused = remote
        .clone()
        .oneshot(request(
            "POST",
            "/v1/mcp/servers/leaky/refresh",
            Value::Null,
            Some(&admin),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(refused).await["code"], "local_access_required");
    let (status, refreshed) =
        send(&local, "POST", "/v1/mcp/servers/leaky/refresh", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!refreshed.to_string().contains(TOKEN), "{refreshed}");
    drop(host);
    daemon.shutdown().await;
}

#[tokio::test]
async fn mcp_secrets_are_never_exported_or_replaced_by_sync() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = fixture(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    leaky_server(root.path(), &local).await;

    let ctx = openside_vault::AuditContext {
        who: "local",
        session_id: None,
        tool_call_id: None,
    };
    let site = "mcp:leaky:fake_token";
    let mut guard = daemon.shared.registry.vault.vault.lock().await;
    let vault = guard.as_mut().unwrap();
    assert!(vault.is_mcp_secret(site));
    assert!(!vault.item_ids().unwrap().iter().any(|id| id == site));
    assert!(matches!(
        vault.export_item(&ctx, site),
        Err(openside_vault::Error::UnknownItem)
    ));
    let csv = vault.export_csv(&ctx).unwrap();
    assert!(!csv.csv.contains(TOKEN));

    // A synced login that claims the secret's id cannot replace it.
    vault
        .add_item(
            &ctx,
            openside_vault::Entry::new(
                "example".into(),
                vec![openside_vault::AllowedOrigin::parse("https://example.com").unwrap()],
                Secret::new("person".into()),
                Secret::new("login-password".into()),
                None,
                openside_vault::AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    let exported = vault.export_item(&ctx, "example").unwrap();
    let mut forged: Value = serde_json::from_slice(&exported).unwrap();
    forged["site"] = json!(site);
    let prepared =
        openside_vault::PreparedImport::parse(site, forged.to_string().as_bytes()).unwrap();
    assert!(vault.commit_import(&ctx, prepared).is_err());
    let mut forged_typed: Value = serde_json::from_slice(&exported).unwrap();
    forged_typed["site"] = json!(site);
    forged_typed["mcp_secret"] =
        json!({"server_id": "leaky", "name": "FAKE_TOKEN", "created_at_ms": 1});
    let forged_typed = forged_typed.to_string();
    assert!(openside_vault::PreparedImport::parse(site, forged_typed.as_bytes()).is_err());
    assert!(vault.is_mcp_secret(site));
    drop(guard);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_tool_list_change_during_turn_setup_reaches_that_turn() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = fixture(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    leaky_server(root.path(), &local).await;
    let session = create_session(&daemon, "turns", PolicyMode::Full, None).await;

    // Hold admission: the chat opens its runtime, then waits here before sending the turn.
    let gate = daemon.shared.agents.gate.lock().await;
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("hello"),
                text: "hello".into(),
                mode: QueueMode::Steer,
                attachments: Vec::new(),
                author_kind: openside_proto::MessageAuthorKind::Person,
                from_agent: None,
            },
        )
        .await
        .unwrap();
    let shared = daemon.shared.clone();
    let delivery = tokio::spawn({
        let session = session.clone();
        async move {
            shared
                .command(&session, crate::engine::Action::Deliver)
                .await
        }
    });
    until(|| runtime.0.lock().opened.len() == 1).await;
    let opened_with = |index: usize| -> Vec<String> {
        runtime.0.lock().opened[index]
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    };
    assert!(!opened_with(0)
        .iter()
        .any(|name| name == "mcp__leaky__extra"));

    let ToolOutcome::Done { .. } = host(&daemon, &session)
        .await
        .call(call("mcp__leaky__change", json!({}), "change"))
        .await
    else {
        panic!("expected the call to run");
    };
    until(|| daemon.shared.registry.get("mcp__leaky__extra").is_some()).await;
    assert!(runtime.0.lock().sent.is_empty());
    drop(gate);

    until(|| !runtime.0.lock().sent.is_empty()).await;
    delivery.await.unwrap().unwrap();
    assert_eq!(runtime.0.lock().opened.len(), 2);
    assert!(opened_with(1)
        .iter()
        .any(|name| name == "mcp__leaky__extra"));
    daemon.shutdown().await;
}
