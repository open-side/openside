use super::*;
use openside_vault::{PassphraseLock, PlatformLock, Secret, Vault};

const CANARY: &str = "channel-api-canary-1029384756";
async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, token))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
async fn fixture(root: &Path) -> Daemon {
    fixture_runtime(root, TestRuntime::default()).await
}
async fn fixture_runtime(root: &Path, runtime: TestRuntime) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.vault.path = Some(root.join("channel-vault"));
    config.loaded.config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    config.loaded.config.models.default = Some("script:test".into());
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let vault = Vault::create(
        root.join("channel-vault"),
        root.join("channel-vault-audit"),
        &Secret::new("channel-test-passphrase".into()),
        Box::new(PassphraseLock::default()) as Box<dyn PlatformLock + Send>,
    )
    .unwrap();
    *daemon.shared.registry.vault.vault.lock().await = Some(vault);
    daemon
}
fn add(id: &str) -> Value {
    json!({"id":id,"label":id,"enabled":false,"credentials":{"token":CANARY},"options":{"allowed_chats":["100"]}})
}

#[tokio::test]
async fn channel_accounts_require_admin_person_and_never_echo_debug_log_or_config_secrets() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let local = api::router(daemon.shared.clone(), true);
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let create = "/v1/settings/channels/telegram/accounts";
    let account = "/v1/settings/channels/telegram/accounts/work";
    for (method, path, body) in [
        ("POST", create, add("work")),
        ("PATCH", account, json!({"credentials":{"token":CANARY}})),
        ("DELETE", account, json!({})),
        (
            "POST",
            "/v1/settings/channels/telegram/accounts/work/test",
            json!({}),
        ),
    ] {
        assert_eq!(
            call(&tcp, method, path, body.clone(), Some(&read)).await.0,
            StatusCode::FORBIDDEN
        );
        let (status, result) = call(&tcp, method, path, body, Some(&agent)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(result["code"], "person_device_required");
        assert!(!result.to_string().contains(CANARY));
    }
    for (method, path, body) in [
        (
            "POST",
            create,
            json!({"id":"malformed","label":"Malformed","credentials":CANARY}),
        ),
        ("PATCH", account, json!({"credentials":CANARY})),
    ] {
        let (status, result) = call(&tcp, method, path, body, Some(&admin)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!result.to_string().contains(CANARY));
    }
    let request: openside_proto::AddChannelAccountRequest =
        serde_json::from_value(add("work")).unwrap();
    assert!(!format!("{request:?}").contains(CANARY));
    for id in ["work", "personal"] {
        let (status, result) = call(&tcp, "POST", create, add(id), Some(&admin)).await;
        assert_eq!(status, StatusCode::CREATED, "{result}");
        assert_eq!(result["id"], id);
        assert_eq!(result["status"], "disabled");
        assert_eq!(result["secret_present"], true);
        assert!(result["effective_agent_id"].is_null());
        assert!(!result.to_string().contains(CANARY));
        assert!(result.get("credentials").is_none());
    }
    let (_, list) = call(&tcp, "GET", "/v1/settings/channels", json!({}), Some(&read)).await;
    assert_eq!(list["accounts"].as_array().unwrap().len(), 2);
    assert!(!list.to_string().contains(CANARY));
    let (status, _) = call(
        &local,
        "PATCH",
        account,
        json!({"label":"Work bot","credentials":{"token":"rotated-channel-canary-abcdef"}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(&tcp, "POST", create, add("work"), Some(&admin))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, generic) = call(&local, "GET", "/v1/vault/items", json!({}), None).await;
    assert!(generic["items"].as_array().unwrap().is_empty());
    for method in ["DELETE", "PATCH"] {
        let (status, _) = call(
            &local,
            method,
            "/v1/vault/items/channel:telegram:work",
            json!({"policy":"always"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    for file in [
        root.path().join("config.toml"),
        root.path().join("channel-vault"),
        root.path().join("channel-vault-audit"),
        root.path().join("data/audit.jsonl"),
    ] {
        let bytes = std::fs::read(file).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(CANARY));
        assert!(!text.contains("rotated-channel-canary-abcdef"));
    }
    assert_eq!(
        call(&tcp, "DELETE", account, json!({}), Some(&admin))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    let (_, list) = call(&tcp, "GET", "/v1/settings/channels", json!({}), Some(&read)).await;
    assert_eq!(list["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(list["accounts"][0]["id"], "personal");
    let vault = daemon.shared.registry.vault.vault.lock().await;
    assert!(!vault
        .as_ref()
        .unwrap()
        .is_channel_secret("channel:telegram:work"));
    assert!(vault
        .as_ref()
        .unwrap()
        .is_channel_secret("channel:telegram:personal"));
    drop(vault);
    daemon.shutdown().await;
}

#[tokio::test]
async fn channel_legacy_listing_has_no_argv_and_remote_destinations_are_fixed() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.channels = openside_config::parse_document(
        Path::new("config.toml"),
        "[channels.telegram]\ntoken_cmd=['echo','legacy-command-canary']\nallowed_chats=[100]",
    )
    .unwrap()
    .channels;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let local = api::router(daemon.shared.clone(), true);
    let (_, list) = call(&local, "GET", "/v1/settings/channels", json!({}), None).await;
    assert_eq!(list["accounts"][0]["id"], "legacy-telegram");
    assert_eq!(list["accounts"][0]["credential_source"], "legacy");
    assert!(!list.to_string().contains("legacy-command-canary"));
    assert!(!list.to_string().contains("token_cmd"));
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    let mut body = add("exfiltration");
    body["options"]["api_base"] = json!("https://untrusted.example");
    let (status, result) = call(
        &tcp,
        "POST",
        "/v1/settings/channels/telegram/accounts",
        body,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(result["code"], "local_socket_required");
    daemon.shutdown().await;
}

async fn hook_call(
    router: &axum::Router,
    path: &str,
    timestamp: &str,
    signature: &str,
    body: String,
    slack: bool,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header(
            if slack {
                "x-slack-request-timestamp"
            } else {
                "x-signature-timestamp"
            },
            timestamp,
        )
        .header(
            if slack {
                "x-slack-signature"
            } else {
                "x-signature-ed25519"
            },
            signature,
        )
        .body(Body::from(body))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn channel_signed_hooks_reject_bad_signature_replay_skew_oversize_and_rate_abuse() {
    use axum::routing::{get, post};
    use ed25519_dalek::{Signer, SigningKey};
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    crate::channels_api::initialize(&daemon.shared, Some("127.0.0.1:1".parse().unwrap())).await;
    let platform = axum::Router::new()
        .route(
            "/auth.test",
            post(|| async { axum::Json(json!({"ok":true})) }),
        )
        .route(
            "/users/@me",
            get(|| async { axum::Json(json!({"id":"application"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, platform).await.unwrap();
    });
    let local = api::router(daemon.shared.clone(), true);
    const SIGNING: &str = "channel-hook-signing-canary";
    for id in ["work", "personal", "rate"] {
        let body = json!({"id":id,"label":id,"enabled":true,"credentials":{"token":CANARY,"signing_secret":SIGNING},
            "options":{"api_base":base,"allowed_users":["U1"]}});
        let (status, row) = call(
            &local,
            "POST",
            "/v1/settings/channels/slack/accounts",
            body,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{row}");
        assert_eq!(row["status"], "connected");
        assert_eq!(
            row["webhook_url"],
            format!("http://127.0.0.1:1/v1/hooks/slack/{id}")
        );
    }
    let key = SigningKey::from_bytes(&[9; 32]);
    let public_key = key
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let (status,row)=call(&local,"POST","/v1/settings/channels/discord/accounts",
        json!({"id":"application","label":"Application","enabled":true,"credentials":{"token":CANARY},
            "options":{"api_base":base,"public_key":public_key,"allowed_users":["42"]}}),None).await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    assert_eq!(row["status"], "connected");
    assert_eq!(row["effective_agent_id"], "chief");
    let ingress = api::router(daemon.shared.clone(), false);
    let now = Utc::now().timestamp().to_string();
    let body = json!({"type":"url_verification","challenge":"challenge"}).to_string();
    let signature = openside_channels::slack::sign(SIGNING, &now, body.as_bytes());
    let path = "/v1/hooks/slack/work";
    assert_eq!(
        hook_call(
            &ingress,
            path,
            &now,
            &format!("v0={}", "0".repeat(64)),
            body.clone(),
            true
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, response) = hook_call(&ingress, path, &now, &signature, body.clone(), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["challenge"], "challenge");
    assert_eq!(
        hook_call(
            &ingress,
            path,
            &now,
            &signature.to_uppercase().replacen("V0=", "v0=", 1),
            body.clone(),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        hook_call(
            &ingress,
            "/v1/hooks/slack/personal",
            &now,
            &signature,
            body.clone(),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    let stale = (Utc::now().timestamp() - 301).to_string();
    let stale_signature = openside_channels::slack::sign(SIGNING, &stale, body.as_bytes());
    assert_eq!(
        hook_call(&ingress, path, &stale, &stale_signature, body.clone(), true)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let huge = "x".repeat(256 * 1024 + 1);
    let huge_signature = openside_channels::slack::sign(SIGNING, &now, huge.as_bytes());
    assert_eq!(
        hook_call(&ingress, path, &now, &huge_signature, huge, true)
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        hook_call(
            &ingress,
            "/v1/hooks/slack/absent",
            &now,
            &signature,
            body,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for _ in 0..120 {
        assert_eq!(
            hook_call(
                &ingress,
                "/v1/hooks/slack/rate",
                &now,
                "invalid",
                "{}".into(),
                true
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        hook_call(
            &ingress,
            "/v1/hooks/slack/rate",
            &now,
            "invalid",
            "{}".into(),
            true
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let discord_body = json!({"type":1}).to_string();
    let signed = key
        .sign(format!("{now}{discord_body}").as_bytes())
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let discord = "/v1/hooks/discord/application";
    assert_eq!(
        hook_call(
            &ingress,
            discord,
            &now,
            &"0".repeat(128),
            discord_body.clone(),
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        hook_call(
            &ingress,
            discord,
            &now,
            &signed,
            discord_body.clone(),
            false
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        hook_call(
            &ingress,
            discord,
            &now,
            &signed.to_uppercase(),
            discord_body.clone(),
            false
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let stale_signed = key
        .sign(format!("{stale}{discord_body}").as_bytes())
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        hook_call(
            &ingress,
            discord,
            &stale,
            &stale_signed,
            discord_body,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let huge_discord = "x".repeat(256 * 1024 + 1);
    let huge_discord_signature = key
        .sign(format!("{now}{huge_discord}").as_bytes())
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        hook_call(
            &ingress,
            discord,
            &now,
            &huge_discord_signature,
            huge_discord,
            false
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        call(
            &local,
            "DELETE",
            "/v1/settings/channels/slack/accounts/work",
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let deleted_body = json!({"type":"url_verification","challenge":"removed"}).to_string();
    let deleted_signature = openside_channels::slack::sign(SIGNING, &now, deleted_body.as_bytes());
    assert_eq!(
        hook_call(&ingress, path, &now, &deleted_signature, deleted_body, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let remaining = json!({"type":"url_verification","challenge":"remaining"}).to_string();
    let remaining_signature = openside_channels::slack::sign(SIGNING, &now, remaining.as_bytes());
    assert_eq!(
        hook_call(
            &ingress,
            "/v1/hooks/slack/personal",
            &now,
            &remaining_signature,
            remaining.clone(),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&local, "POST", "/v1/vault/lock", json!({}), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        hook_call(
            &ingress,
            "/v1/hooks/slack/personal",
            &now,
            &remaining_signature,
            remaining,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &local,
            "POST",
            "/v1/vault/unlock",
            json!({"passphrase":"channel-test-passphrase"}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let resumed = json!({"type":"url_verification","challenge":"resumed"}).to_string();
    let resumed_signature = openside_channels::slack::sign(SIGNING, &now, resumed.as_bytes());
    assert_eq!(
        hook_call(
            &ingress,
            "/v1/hooks/slack/personal",
            &now,
            &resumed_signature,
            resumed,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    daemon.shutdown().await;
    server.abort();
    let _ = server.await;
}

async fn inbound_hook(router: &axum::Router, account: &str, text: &str, ts: &str) -> StatusCode {
    let now = Utc::now().timestamp().to_string();
    let body = json!({"type":"event_callback","team_id":"W1","event":{"type":"message",
        "channel":"D1","channel_type":"im","user":"U1","ts":ts,"text":text}})
    .to_string();
    let signature =
        openside_channels::slack::sign("channel-runtime-signing-canary", &now, body.as_bytes());
    hook_call(
        router,
        &format!("/v1/hooks/slack/{account}"),
        &now,
        &signature,
        body,
        true,
    )
    .await
    .0
}

async fn delivered(runtime: &TestRuntime, text: &str) -> SessionId {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(id) = runtime
                .0
                .lock()
                .sent
                .iter()
                .find(|(_, _, value, _)| value == text)
                .map(|(id, _, _, _)| id.clone())
            {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("channel message was not delivered: {text}"))
}

#[tokio::test]
async fn channel_two_accounts_rebind_existing_thread_to_teammate_without_reusing_chief_session() {
    use axum::routing::post;
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = fixture_runtime(root.path(), runtime.clone()).await;
    let local = api::router(daemon.shared.clone(), true);
    let (status,mina)=call(&local,"POST","/v1/agents",
        json!({"name":"Mina","persona":{"role":"Personal inbox assistant","responsibilities":["Handle personal messages"]}}),None).await;
    assert_eq!(status, StatusCode::CREATED, "{mina}");
    let agent_id = mina["id"].as_str().unwrap().to_owned();
    let ingress = api::router(daemon.shared.clone(), false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    crate::channels_api::initialize(&daemon.shared, Some(listener.local_addr().unwrap())).await;
    let server = tokio::spawn({
        let router = ingress.clone();
        async move {
            axum::serve(listener, router).await.unwrap();
        }
    });
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let platform = axum::Router::new()
        .route(
            "/auth.test",
            post(|| async { axum::Json(json!({"ok":true})) }),
        )
        .fallback({
            let notices = notices.clone();
            move |axum::Json(body): axum::Json<Value>| {
                let notices = notices.clone();
                async move {
                    if let Some(text) = body["text"].as_str() {
                        notices.lock().push(text.to_owned());
                    }
                    axum::Json(json!({"ok":true,"ts":"reply-1","channel":"D1"}))
                }
            }
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let platform_server = tokio::spawn(async move {
        axum::serve(listener, platform).await.unwrap();
    });
    for id in ["work", "personal"] {
        let (status,row)=call(&local,"POST","/v1/settings/channels/slack/accounts",
            json!({"id":id,"label":id,"enabled":true,"credentials":{"token":format!("{CANARY}-{id}"),
                "signing_secret":"channel-runtime-signing-canary"},"options":{"api_base":base,"allowed_users":["U1"]}}),None).await;
        assert_eq!(status, StatusCode::CREATED, "{row}");
        assert_eq!(row["status"], "connected");
    }
    assert_eq!(
        inbound_hook(&ingress, "personal", "before rebind", "0.01").await,
        StatusCode::OK
    );
    let old_personal = delivered(&runtime, "before rebind").await;
    assert_eq!(
        daemon
            .shared
            .db
            .state(&old_personal)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("chief")
    );
    runtime.emit(&old_personal, RuntimeEvent::Settled);
    let (status, row) = call(
        &local,
        "PATCH",
        &format!("/v1/agents/{agent_id}"),
        json!({"channel":{"kind":"slack","account_id":"personal"}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{row}");
    let (_, list) = call(&local, "GET", "/v1/settings/channels", json!({}), None).await;
    let personal = list["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "personal")
        .unwrap();
    assert_eq!(personal["effective_agent_id"], agent_id);
    assert!(personal["agent_id"].is_null());
    let (status, row) = call(
        &local,
        "PATCH",
        "/v1/settings/channels/slack/accounts/personal",
        json!({"agent_id":"chief"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(row["code"], "channel_binding_conflict");
    assert_eq!(
        inbound_hook(&ingress, "work", "from work", "1.01").await,
        StatusCode::OK
    );
    assert_eq!(
        inbound_hook(&ingress, "personal", "from personal", "1.01").await,
        StatusCode::OK
    );
    let work = delivered(&runtime, "from work").await;
    let personal = delivered(&runtime, "from personal").await;
    assert_ne!(work, personal);
    assert_ne!(old_personal, personal);
    assert_eq!(
        daemon
            .shared
            .db
            .state(&old_personal)
            .await
            .unwrap()
            .message_authors
            .len(),
        1
    );
    assert_eq!(
        runtime
            .0
            .lock()
            .sent
            .iter()
            .filter(|(id, _, _, _)| id == &old_personal)
            .count(),
        1
    );
    assert_eq!(
        notices
            .lock()
            .iter()
            .filter(|text| text.as_str() == "This conversation now continues with Mina.")
            .count(),
        1
    );
    assert_eq!(
        daemon
            .shared
            .db
            .state(&work)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("chief")
    );
    assert_eq!(
        daemon
            .shared
            .db
            .state(&personal)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some(agent_id.as_str())
    );
    assert_ne!(personal.as_str(), mina["home_session_id"].as_str().unwrap());
    {
        let capture = runtime.0.lock();
        let spec = capture
            .opened
            .iter()
            .find(|spec| spec.session == personal)
            .unwrap();
        let team = spec.team.as_ref().unwrap();
        assert_eq!(team.agent_id, agent_id);
        assert_eq!(team.role, openside_core::TeamRole::Teammate);
        assert!(team.persona.contains("Personal inbox assistant"));
    }
    // This hermetic model stays running until the test explicitly settles its first turn.
    runtime.emit(&personal, RuntimeEvent::Settled);
    assert_eq!(
        call(
            &local,
            "DELETE",
            "/v1/settings/channels/slack/accounts/work",
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        inbound_hook(&ingress, "work", "must not be delivered", "2.01").await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        inbound_hook(&ingress, "personal", "still personal", "2.01").await,
        StatusCode::OK
    );
    assert_eq!(delivered(&runtime, "still personal").await, personal);
    assert!(!runtime
        .0
        .lock()
        .sent
        .iter()
        .any(|(_, _, text, _)| text == "must not be delivered"));
    assert_eq!(
        call(
            &local,
            "DELETE",
            "/v1/settings/channels/slack/accounts/personal",
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(daemon
        .shared
        .agents
        .get(&agent_id)
        .unwrap()
        .unwrap()
        .channel
        .is_none());
    assert_eq!(
        daemon
            .shared
            .db
            .state(&personal)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some(agent_id.as_str())
    );
    assert_eq!(
        daemon
            .shared
            .db
            .state(&old_personal)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("chief")
    );
    assert_eq!(
        daemon
            .shared
            .db
            .state(&old_personal)
            .await
            .unwrap()
            .message_authors
            .len(),
        1
    );
    daemon.shutdown().await;
    server.abort();
    let _ = server.await;
    platform_server.abort();
    let _ = platform_server.await;
}
