use super::*;
use axum::{extract::ConnectInfo, Router};
use openside_proto::DeviceKind;
use std::net::SocketAddr;

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn launcher(method: &str, path: &str, body: Value, token: &str, secret: &str) -> Request<Body> {
    let mut req = request(method, path, body, Some(token));
    req.headers_mut()
        .insert("host", "127.0.0.1:47821".parse().unwrap());
    req.headers_mut()
        .insert(crate::remote::LOCAL_SECRET_HEADER, secret.parse().unwrap());
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50001".parse::<SocketAddr>().unwrap(),
    ));
    req
}

#[tokio::test]
async fn operator_auth_matrix_covers_every_new_route_and_principal() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, access) = send(
        &unix,
        request("GET", "/v1/remote/local-access", Value::Null, None),
    )
    .await;
    let local = access["token"].as_str().unwrap();
    let secret = access["secret"].as_str().unwrap();
    let (_, person) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_local(
            "agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    for (method, path, admin, local_only, body) in [
        ("GET", "/v1/daemon", false, false, json!({})),
        ("POST", "/v1/daemon/restart", true, true, json!({})),
        ("GET", "/v1/doctor", true, false, json!({})),
        ("GET", "/v1/memory/model", false, false, json!({})),
        (
            "POST",
            "/v1/memory/model/download",
            true,
            false,
            json!({"unknown":true}),
        ),
        ("POST", "/v1/memory/model/cancel", true, false, json!({})),
        (
            "PUT",
            "/v1/external-agents/custom",
            true,
            true,
            json!({"skills_dir":"relative"}),
        ),
        (
            "DELETE",
            "/v1/external-agents/custom",
            true,
            true,
            json!({}),
        ),
        ("POST", "/v1/policy/validate", false, false, json!({})),
        ("GET", "/v1/policy/network", false, false, json!({})),
        ("PUT", "/v1/policy/network", true, true, json!({"allow":[]})),
        (
            "PUT",
            "/v1/policy",
            true,
            true,
            json!({"default_mode":"guard","rules":[]}),
        ),
        ("PUT", "/v1/sync/config", true, true, json!({})),
        ("POST", "/v1/sync/unlock", true, false, json!({})),
    ] {
        let (status, body_out) = send(&unix, request(method, path, body.clone(), None)).await;
        assert!(
            !matches!(
                status,
                StatusCode::UNAUTHORIZED
                    | StatusCode::FORBIDDEN
                    | StatusCode::METHOD_NOT_ALLOWED
                    | StatusCode::INTERNAL_SERVER_ERROR
            ),
            "socket {method} {path}: {status} {body_out}"
        );
        let (status, body_out) =
            send(&tcp, launcher(method, path, body.clone(), local, secret)).await;
        assert!(
            !matches!(
                status,
                StatusCode::UNAUTHORIZED
                    | StatusCode::FORBIDDEN
                    | StatusCode::METHOD_NOT_ALLOWED
                    | StatusCode::INTERNAL_SERVER_ERROR
            ),
            "launcher {method} {path}: {status} {body_out}"
        );
        for (name, token, denied) in [
            ("person", &person, local_only),
            ("agent", &agent, local_only),
            ("read", &read, admin),
        ] {
            let (status, output) =
                send(&tcp, launcher(method, path, body.clone(), token, secret)).await;
            if denied {
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{name} {method} {path}: {output}"
                );
                assert_eq!(
                    output["code"],
                    if name == "read" {
                        "forbidden"
                    } else {
                        "local_access_required"
                    }
                );
            } else {
                assert!(
                    !matches!(
                        status,
                        StatusCode::UNAUTHORIZED
                            | StatusCode::FORBIDDEN
                            | StatusCode::METHOD_NOT_ALLOWED
                    ),
                    "{name} {method} {path}: {status} {output}"
                );
            }
        }
    }
    for (path, body) in [
        (
            "/v1/memory/model/download",
            json!({"endpoint":"http://127.0.0.1:9"}),
        ),
        ("/v1/sync/setup", json!({"passphrase":"secret"})),
        (
            "/v1/sync/pair",
            json!({"pairing_code":"invalid", "passphrase":"secret"}),
        ),
        ("/v1/sync/unlock", json!({"passphrase":"secret"})),
    ] {
        for token in [&person, &agent] {
            assert_eq!(
                send(&tcp, launcher("POST", path, body.clone(), token, secret))
                    .await
                    .1["code"],
                "local_access_required"
            );
        }
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn operator_config_key_classes_enforce_local_authority() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, access) = send(
        &unix,
        request("GET", "/v1/remote/local-access", Value::Null, None),
    )
    .await;
    let local = access["token"].as_str().unwrap();
    let secret = access["secret"].as_str().unwrap();
    let (_, person) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_local(
            "agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, baseline) = send(
        &unix,
        request("GET", "/v1/settings/config", json!({}), None),
    )
    .await;
    let (_, local_doc) = send(
        &tcp,
        launcher("GET", "/v1/settings/config", json!({}), local, secret),
    )
    .await;
    assert_eq!(local_doc["remote"], false);
    for (section, key, value, remote_allowed) in [
        ("tickets", "confirm_ttl_secs", json!(321), true),
        ("models", "default", json!("script:test"), true),
        ("agent", "tool_retries", json!(3), true),
        ("service", "inhibit_sleep", json!(false), true),
        ("agent", "command", json!(["/not-executed"]), false),
        ("network", "allow", json!(["example.test"]), false),
        ("browser", "executable", json!("/not-executed"), false),
        ("daemon", "listen", json!("127.0.0.1:49000"), false),
        ("sync", "passphrase_cmd", json!(["/not-executed"]), false),
    ] {
        assert_eq!(
            send(
                &unix,
                request(
                    "PUT",
                    "/v1/settings/config",
                    json!({"config":baseline["config"]}),
                    None
                )
            )
            .await
            .0,
            StatusCode::OK
        );
        let mut config = baseline["config"].clone();
        config[section][key] = value;
        let body = json!({"config":config});
        let (status, result) = send(
            &tcp,
            request("PUT", "/v1/settings/config", body.clone(), Some(&person)),
        )
        .await;
        assert_eq!(
            status,
            if remote_allowed {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "{section}.{key}: {result}"
        );
        if !remote_allowed {
            assert_eq!(result["code"], "local_access_required");
        }
        assert_eq!(
            send(
                &tcp,
                launcher("PUT", "/v1/settings/config", body.clone(), &agent, secret)
            )
            .await
            .1["code"],
            "forbidden"
        );
        let (status, result) = send(
            &tcp,
            launcher("PUT", "/v1/settings/config", body, local, secret),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "local {section}.{key}: {result}");
    }
    let mut config = baseline["config"].clone();
    config["credentials"] = json!({});
    assert_eq!(
        send(
            &unix,
            request("PUT", "/v1/settings/config", json!({"config":config}), None)
        )
        .await
        .1["code"],
        "file_only_section"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn operator_sync_overlay_typed_secret_restart_and_policy_network() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    std::fs::write(
        &daemon.shared.config.loaded.paths.config_file,
        toml::to_string(&daemon.shared.config.loaded.config).unwrap(),
    )
    .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let dir = root.path().join("share");
    let config = json!({"dir":dir,"settings":true,"memory_tags":["sync"]});
    let (status, configured) = send(
        &router,
        request("PUT", "/v1/sync/config", config.clone(), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{configured}");
    assert_eq!(configured["configured"], true);
    assert_eq!(configured["location"], dir.to_str().unwrap());
    assert_eq!(
        crate::sync::effective(&daemon.shared).dir.as_deref(),
        Some(dir.as_path())
    );
    assert_eq!(
        send(
            &router,
            request("GET", "/v1/settings/config", json!({}), None)
        )
        .await
        .1["restart_required"],
        false
    );
    let (status, setup) = send(
        &router,
        request(
            "POST",
            "/v1/sync/setup",
            json!({"passphrase":"test passphrase"}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{setup}");
    let (status, report) = send(&router, request("POST", "/v1/sync/run", json!({}), None)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        send(&router, request("GET", "/v1/sync", json!({}), None))
            .await
            .1["last_run"],
        report
    );
    assert_eq!(
        send(
            &router,
            request(
                "POST",
                "/v1/sync/unlock",
                json!({"passphrase":"wrong"}),
                None
            )
        )
        .await
        .1["code"],
        "sync_wrong_passphrase"
    );
    assert_eq!(
        send(
            &router,
            request(
                "POST",
                "/v1/sync/unlock",
                json!({"passphrase":"test passphrase"}),
                None
            )
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        send(
            &router,
            request(
                "PUT",
                "/v1/sync/config",
                json!({"dir":root.path().join("other")}),
                None
            )
        )
        .await
        .1["code"],
        "sync_already_set_up"
    );
    assert!(
        !std::fs::read_to_string(&daemon.shared.config.loaded.paths.config_file)
            .unwrap()
            .contains("test passphrase")
    );
    let (_, validation) = send(
        &router,
        request(
            "POST",
            "/v1/policy/validate",
            json!({"network_allow":["https://valid.test", "https://"]}),
            None,
        ),
    )
    .await;
    assert_eq!(validation["valid"], false);
    assert_eq!(validation["problems"][0]["index"], 1);
    let (_, invalid_policy) = send(
        &router,
        request(
            "POST",
            "/v1/policy/validate",
            json!({"policy":{"default_mode":"guard","allow_origins":["https://"],"rules":[]}}),
            None,
        ),
    )
    .await;
    assert_eq!(invalid_policy["valid"], false);
    assert_eq!(invalid_policy["problems"][0]["list"], "allow_origins");
    let original = std::fs::read(&daemon.shared.config.loaded.paths.config_file).unwrap();
    assert_eq!(
        send(
            &router,
            request(
                "PUT",
                "/v1/policy/network",
                json!({"allow":["https://"]}),
                None
            )
        )
        .await
        .1["code"],
        "invalid_network_allow"
    );
    assert_eq!(
        std::fs::read(&daemon.shared.config.loaded.paths.config_file).unwrap(),
        original
    );
    let (status, network) = send(
        &router,
        request(
            "PUT",
            "/v1/policy/network",
            json!({"allow":["example.test"]}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{network}");
    assert_eq!(network["restart_required"], true);
    assert_eq!(
        send(
            &router,
            request("GET", "/v1/policy/network", json!({}), None)
        )
        .await
        .1,
        network
    );
    assert!(crate::settings_api::restart_required(&daemon.shared)
        .await
        .unwrap_or_else(|_| panic!("restart status unavailable")));
    let (_, before) = send(&router, request("GET", "/v1/daemon", json!({}), None)).await;
    let (status, restart) = send(
        &router,
        request("POST", "/v1/daemon/restart", json!({}), None),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(restart["boot_id"], before["boot_id"]);
    assert_eq!(restart["method"], "re_exec");
    tokio::time::timeout(
        Duration::from_secs(2),
        daemon.shared.ops.lifecycle.reexec_requested(),
    )
    .await
    .unwrap();
    daemon.shutdown().await;
}

#[tokio::test]
async fn operator_memory_model_job_installs_from_loopback_hub_and_reports_restart() {
    use axum::{routing::get, Json};
    use sha2::{Digest, Sha256};
    let tokenizer = serde_json::to_vec(&json!({
        "normalizer":{"type":"BertNormalizer","lowercase":true},
        "pre_tokenizer":{"type":"BertPreTokenizer"},
        "model":{"type":"WordPiece","unk_token":"[UNK]","continuing_subword_prefix":"##",
            "vocab":{"[PAD]":0,"[UNK]":1,"car":2,"garage":3}}
    }))
    .unwrap();
    let data: Vec<u8> = [0.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let mut header = serde_json::to_vec(
        &json!({"embeddings":{"dtype":"F32","shape":[4,2],"data_offsets":[0,data.len()]}}),
    )
    .unwrap();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let mut model = (header.len() as u64).to_le_bytes().to_vec();
    model.extend(header);
    model.extend(data);
    let listing = json!({"sha":"0123456789abcdef0123456789abcdef01234567","siblings":[
        {"rfilename":"tokenizer.json","size":tokenizer.len(),"lfs":{"sha256":format!("{:x}", Sha256::digest(&tokenizer)),"size":tokenizer.len()}},
        {"rfilename":"model.safetensors","size":model.len(),"lfs":{"sha256":format!("{:x}", Sha256::digest(&model)),"size":model.len()}}
    ]});
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let server_gate = gate.clone();
    let hub = Router::new()
        .route(
            "/api/models/{owner}/{name}/revision/{revision}",
            get(move || {
                let listing = listing.clone();
                let gate = server_gate.clone();
                async move {
                    gate.acquire_owned().await.unwrap().forget();
                    Json(listing)
                }
            }),
        )
        .route(
            "/{owner}/{name}/resolve/{revision}/tokenizer.json",
            get(move || {
                let bytes = tokenizer.clone();
                async move { bytes }
            }),
        )
        .route(
            "/{owner}/{name}/resolve/{revision}/model.safetensors",
            get(move || {
                let bytes = model.clone();
                async move { bytes }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, hub).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, started) = send(
        &router,
        request(
            "POST",
            "/v1/memory/model/download",
            json!({"endpoint":endpoint,"repo":"test/model"}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(
        send(
            &router,
            request(
                "POST",
                "/v1/memory/model/download",
                json!({"endpoint":endpoint,"repo":"test/model"}),
                None
            )
        )
        .await
        .1["code"],
        "download_in_progress"
    );
    gate.add_permits(1);
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, status) =
                send(&router, request("GET", "/v1/memory/model", json!({}), None)).await;
            if matches!(
                status["state"].as_str(),
                Some("installed" | "failed" | "cancelled")
            ) {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed["state"], "installed", "{completed}");
    assert_eq!(completed["installed"], true);
    assert_eq!(completed["restart_required"], true);
    assert_eq!(completed["files_done"], 2);
    assert!(completed["model_id"].as_str().is_some());
    assert_eq!(
        send(
            &router,
            request(
                "POST",
                "/v1/memory/model/download",
                json!({"endpoint":endpoint,"repo":"test/model"}),
                None
            )
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        send(
            &router,
            request("POST", "/v1/memory/model/cancel", json!({}), None)
        )
        .await
        .0,
        StatusCode::OK
    );
    let cancelled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, status) =
                send(&router, request("GET", "/v1/memory/model", json!({}), None)).await;
            if status["state"] == "cancelled" {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(cancelled["installed"], true);
    gate.add_permits(1);
    server.abort();
    let _ = server.await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn operator_custom_agent_folder_persists_applies_and_revalidates_install() {
    let home = std::env::var_os("HOME").expect("test requires HOME");
    let root = tempfile::tempdir_in(home).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    std::fs::write(
        &daemon.shared.config.loaded.paths.config_file,
        toml::to_string(&daemon.shared.config.loaded.config).unwrap(),
    )
    .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let folder = root.path().join("custom-skills");
    let (status, added) = send(
        &router,
        request(
            "PUT",
            "/v1/external-agents/custom-test",
            json!({"skills_dir":folder}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["custom"], true);
    assert_eq!(
        send(
            &router,
            request("GET", "/v1/settings/config", json!({}), None)
        )
        .await
        .1["restart_required"],
        false
    );
    assert!(
        std::fs::read_to_string(&daemon.shared.config.loaded.paths.config_file)
            .unwrap()
            .contains("custom-test")
    );
    std::os::unix::fs::symlink(outside.path(), &folder).unwrap();
    let (status, refused) = send(
        &router,
        request(
            "POST",
            "/v1/external-agents/custom-test/browser-skill",
            json!({}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "invalid_skills_dir");
    assert!(!outside.path().join("openside-browser").exists());
    std::fs::remove_file(&folder).unwrap();
    let (status, installed) = send(
        &router,
        request(
            "POST",
            "/v1/external-agents/custom-test/browser-skill",
            json!({}),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{installed}");
    assert!(folder.join("openside-browser/SKILL.md").is_file());
    let (status, removed) = send(
        &router,
        request("DELETE", "/v1/external-agents/custom-test", json!({}), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert!(!removed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["id"] == "custom-test"));
    assert!(folder.join("openside-browser/SKILL.md").is_file());
    daemon.shutdown().await;
}

#[tokio::test]
async fn operator_restart_handoff_is_reserved_from_agent_filesystem_access() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let handoff =
        openside_config::data_layout::DataPath::RestartHandoff.path(&daemon.shared.config.data_dir);
    assert!(daemon.shared.credential_paths.is_protected(&handoff));
    std::fs::write(&handoff, "synthetic handoff").unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&handoff, &alias).unwrap();
    assert!(daemon.shared.credential_paths.is_protected(&alias));
    let hardlink = root.path().join("hardlink");
    std::fs::hard_link(&handoff, &hardlink).unwrap();
    assert!(daemon.shared.credential_paths.is_protected(&hardlink));
    daemon.shutdown().await;
}
