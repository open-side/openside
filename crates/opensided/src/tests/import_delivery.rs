use super::*;
use openside_import::{
    Available, Bookmark, BrowserKind, CategoryReport, DataKind, DiscoveredProfile, ProfileRead,
    Visit,
};

async fn send(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        headers,
    )
}

fn profile(root: &Path) -> DiscoveredProfile {
    DiscoveredProfile {
        id: "chrome:Default".into(),
        kind: BrowserKind::Chrome,
        brand: "chrome".into(),
        name: "Personal".into(),
        email: None,
        avatar: None,
        dir: root.join("unused-source"),
        available: Available::default(),
        warnings: Vec::new(),
    }
}

#[tokio::test]
async fn import_delivery_replays_until_per_target_ack_and_survives_daemon_restart() {
    let root = tempfile::tempdir().unwrap();
    let fixture = profile(root.path());
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let mut read = ProfileRead::default();
    read.data.bookmarks.push(Bookmark {
        url: "https://example.test/".into(),
        title: "Saved".into(),
        folder: vec!["Work".into()],
        added_at_ms: Some(1),
    });
    read.data.history.push(Visit {
        url: "https://example.test/old".into(),
        title: None,
        visited_at_ms: 1,
    });
    read.reports.insert(
        DataKind::Bookmarks,
        CategoryReport {
            read: 1,
            imported: 1,
            ..CategoryReport::default()
        },
    );
    read.reports.insert(
        DataKind::History,
        CategoryReport {
            read: 1,
            imported: 1,
            ..CategoryReport::default()
        },
    );
    crate::import_delivery::stage(&daemon.shared, &fixture, &mut read)
        .await
        .map_err(|error| error.1)
        .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let (status, first, headers) = send(
        &router,
        "GET",
        "/v1/import/delivery?target=user",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert!(first["batches"][0].get("bookmarks").is_none());
    assert!(first["batches"][0].get("history").is_none());
    assert!(first["batches"][0].get("cookies").is_none());
    assert_eq!(
        send(
            &router,
            "GET",
            "/v1/import/delivery?target=user",
            Value::Null
        )
        .await
        .1,
        first
    );
    assert!(send(
        &router,
        "GET",
        "/v1/import/delivery?target=agent",
        Value::Null
    )
    .await
    .1["batches"]
        .as_array()
        .unwrap()
        .is_empty());
    let id = first["batches"][0]["id"].as_str().unwrap();
    let path = format!("/v1/import/delivery/{id}/ack");
    assert_eq!(
        send(&router, "POST", &path, json!({"target":"agent"}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(
            &router,
            "GET",
            "/v1/import/delivery?target=user",
            Value::Null
        )
        .await
        .1,
        first
    );
    daemon.shutdown().await;
    drop(router);
    drop(daemon);
    let reopened = super::daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(reopened.shared.clone(), true);
    assert_eq!(
        send(
            &router,
            "GET",
            "/v1/import/delivery?target=user",
            Value::Null
        )
        .await
        .1,
        first
    );
    for _ in 0..2 {
        assert_eq!(
            send(&router, "POST", &path, json!({"target":"user"}))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    assert!(send(
        &router,
        "GET",
        "/v1/import/delivery?target=user",
        Value::Null
    )
    .await
    .1["batches"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        send(
            &router,
            "POST",
            "/v1/import/delivery/not-a-batch/ack",
            json!({"target":"user"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    reopened.shutdown().await;
}

#[tokio::test]
async fn import_delivery_bounds_batches_and_keeps_lookup_data_in_default_source_store() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let fixture = profile(root.path());
    let mut read = ProfileRead::default();
    for index in 0..501 {
        read.data.history.push(Visit {
            url: format!("https://example.test/{index}"),
            title: None,
            visited_at_ms: index,
        });
    }
    read.reports.insert(
        DataKind::History,
        CategoryReport {
            read: 501,
            imported: 501,
            ..CategoryReport::default()
        },
    );
    let layout =
        openside_import::ProfileLayout::new(&daemon.shared.config.data_dir, "default").unwrap();
    openside_import::write_profile(&layout, &fixture, &read).unwrap();
    crate::import_delivery::stage(&daemon.shared, &fixture, &mut read)
        .await
        .map_err(|error| error.1)
        .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let (_, pending, _) = send(&router, "GET", "/v1/import/delivery", Value::Null).await;
    let batches = pending["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 3);
    let (device, _) = daemon
        .shared
        .db
        .pair_as(
            "extension".into(),
            vec![CapabilityScope::Read],
            openside_proto::DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    crate::import_delivery::create_pairing_secret(&daemon.shared, &device)
        .await
        .map_err(|error| error.1)
        .unwrap();
    let mut total = 0;
    for descriptor in batches {
        let batch = signed_batch(&daemon, descriptor["id"].as_str().unwrap(), &device).await;
        let count = batch["history"].as_array().unwrap().len();
        assert!(count <= 250);
        total += count;
    }
    assert_eq!(total, 501);
    let documents = crate::import_delivery::source_documents(&daemon.shared.config.data_dir)
        .map_err(|error| error.1)
        .unwrap();
    assert_eq!(documents[0]["history"].as_array().unwrap().len(), 501);
    assert!(documents[0].get("cookies").is_none());
    daemon.shutdown().await;
}

async fn signed_batch(daemon: &Daemon, batch: &str, device: &str) -> Value {
    let issued = crate::import_delivery::mint(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(crate::remote::LocalAuthority(true)),
        axum::extract::Path(batch.into()),
        axum::Json(openside_proto::ImportDeliveryTicketRequest {
            device_id: device.into(),
        }),
    )
    .await
    .map_err(|error| error.1)
    .unwrap()
    .0;
    let reply = crate::import_delivery::fetch(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(api::Caller(
            Some(device.into()),
            openside_proto::DeviceKind::PersonDevice,
        )),
        axum::extract::Path(issued.ticket),
    )
    .await
    .map_err(|error| error.1)
    .unwrap()
    .1
     .0;
    serde_json::from_str(&reply.payload).unwrap()
}

#[tokio::test]
async fn import_delivery_ticket_binds_device_is_single_use_and_has_verified_payload_signature() {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let mut read = ProfileRead::default();
    read.data.bookmarks.push(Bookmark {
        url: "https://example.test/".into(),
        title: "Saved".into(),
        folder: vec![],
        added_at_ms: None,
    });
    read.reports.insert(
        DataKind::Bookmarks,
        CategoryReport {
            imported: 1,
            ..Default::default()
        },
    );
    crate::import_delivery::stage(&daemon.shared, &profile(root.path()), &mut read)
        .await
        .map_err(|error| error.1)
        .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let (_, pending, _) = send(&router, "GET", "/v1/import/delivery", Value::Null).await;
    let id = pending["batches"][0]["id"].as_str().unwrap().to_owned();
    let (device, token) = daemon
        .shared
        .db
        .pair_as(
            "extension".into(),
            vec![CapabilityScope::Read],
            openside_proto::DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let secret = crate::import_delivery::create_pairing_secret(&daemon.shared, &device)
        .await
        .map_err(|error| error.1)
        .unwrap();
    let issued = crate::import_delivery::mint(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(crate::remote::LocalAuthority(true)),
        axum::extract::Path(id.clone()),
        axum::Json(openside_proto::ImportDeliveryTicketRequest {
            device_id: device.clone(),
        }),
    )
    .await
    .map_err(|error| error.1)
    .unwrap()
    .0;
    let request = || axum::extract::Path(issued.ticket.clone());
    let wrong = crate::import_delivery::fetch(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(api::Caller(
            Some("wrong".into()),
            openside_proto::DeviceKind::PersonDevice,
        )),
        request(),
    )
    .await;
    assert_eq!(wrong.err().unwrap().1, "invalid_import_ticket");
    let (headers, response) = crate::import_delivery::fetch(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(api::Caller(
            Some(device.clone()),
            openside_proto::DeviceKind::PersonDevice,
        )),
        request(),
    )
    .await
    .map_err(|error| error.1)
    .unwrap();
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(response.0.batch_id, id);
    let bytes = |value: &str| {
        (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    let hash: String = Sha256::digest(response.0.payload.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut mac = Hmac::<Sha256>::new_from_slice(&bytes(&secret)).unwrap();
    mac.update(issued.ticket.as_bytes());
    mac.update(id.as_bytes());
    mac.update(hash.as_bytes());
    mac.verify_slice(&bytes(&response.0.signature)).unwrap();
    assert!(crate::import_delivery::fetch(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(api::Caller(
            Some(device.clone()),
            openside_proto::DeviceKind::PersonDevice
        )),
        request()
    )
    .await
    .is_err());
    let (_, minted, _) = send(
        &router,
        "POST",
        &format!("/v1/import/delivery/{id}/ticket"),
        json!({"device_id":device}),
    )
    .await;
    let tcp = api::router(daemon.shared.clone(), false);
    let path = format!(
        "/v1/extension/import/{}",
        minted["ticket"].as_str().unwrap()
    );
    let fetched = tcp
        .clone()
        .oneshot(super::request("GET", &path, Value::Null, Some(&token)))
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(fetched.headers()["cache-control"], "no-store");
    assert_eq!(
        tcp.clone()
            .oneshot(super::request("GET", &path, Value::Null, Some(&token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        tcp.oneshot(super::request(
            "GET",
            "/v1/import/delivery",
            Value::Null,
            Some(&token)
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    crate::pairing::revoke(&daemon.shared, &device)
        .await
        .map_err(|error| error.1)
        .unwrap();
    assert!(crate::import_delivery::mint(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(crate::remote::LocalAuthority(true)),
        axum::extract::Path(id),
        axum::Json(openside_proto::ImportDeliveryTicketRequest { device_id: device })
    )
    .await
    .is_err());
    daemon.shutdown().await;
}

#[tokio::test]
async fn import_pairing_secret_is_refused_to_page_and_missing_origin() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);
    for origin in [
        None,
        Some("http://127.0.0.1:9999"),
        Some("chrome-extension://unrelated"),
    ] {
        let (_, offer, _) = send(&router, "POST", "/v1/pairing", json!({"name":"extension","scopes":["read"],"kind":"person_device","import_delivery":true})).await;
        let mut request = request(
            "POST",
            "/v1/pairing/exchange",
            json!({"code":offer["code"]}),
            None,
        );
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("Origin", origin.parse().unwrap());
        }
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let (_, offer, _) = send(
        &router,
        "POST",
        "/v1/pairing",
        json!({"name":"extension","scopes":["read"],"kind":"person_device","import_delivery":true}),
    )
    .await;
    let mut request = request(
        "POST",
        "/v1/pairing/exchange",
        json!({"code":offer["code"]}),
        None,
    );
    request.headers_mut().insert(
        "Origin",
        "chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj"
            .parse()
            .unwrap(),
    );
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["import_secret"].as_str().unwrap().len(), 64);
    daemon.shutdown().await;
}

#[tokio::test]
async fn import_lookup_person_grant_is_conversation_scoped_and_denial_is_cached() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let source = daemon
        .shared
        .config
        .data_dir
        .join("browser-imports/default/sources");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("fixture.json"), serde_json::to_vec(&json!({"id":"fixture","bookmarks":[{"url":"https://private.example.test","title":"Private saved page","folder":[]}],"history":[{"url":"https://private-history.example.test","title":"Private visit","visited_at_ms":1}],"autofill":[{"fields":{"email":"private-address@example.test"}}]})).unwrap()).unwrap();
    let first = create_session(&daemon, "first", PolicyMode::Full, None).await;
    let second = create_session(&daemon, "page-influenced task", PolicyMode::Full, None).await;
    let outcome = host(&daemon, &first)
        .await
        .call(call(
            "browser_import_lookup",
            json!({"kind":"bookmarks"}),
            "privacy-first",
        ))
        .await;
    let ToolOutcome::Deferred { ticket, .. } = outcome else {
        panic!("private records escaped without a grant")
    };
    let card = daemon.shared.db.state(&first).await.unwrap().tickets[&ticket].clone();
    assert_eq!(
        card.summary,
        "Let this task look through your imported bookmarks and history?"
    );
    assert_eq!(card.options[0].label, "Allow for this conversation");
    assert_eq!(card.tool.unwrap().name, "browser_import_lookup");
    let (person, token) = daemon
        .shared
        .db
        .pair_as(
            "person".into(),
            vec![
                CapabilityScope::Read,
                CapabilityScope::Approve,
                CapabilityScope::HumanStep,
            ],
            openside_proto::DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let _ = person;
    let tcp = api::router(daemon.shared.clone(), false);
    let answer = |ticket: &TicketId, value: &str| {
        request(
            "POST",
            &format!("/v1/tickets/{ticket}/resolve"),
            json!({"value":value}),
            Some(&token),
        )
    };
    let (_, agent_token) = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Approve, CapabilityScope::HumanStep],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    assert_eq!(
        tcp.clone()
            .oneshot(request(
                "POST",
                &format!("/v1/tickets/{ticket}/resolve"),
                json!({"value":"allow"}),
                Some(&agent_token)
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        tcp.clone()
            .oneshot(answer(&ticket, "allow"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let ToolOutcome::Done { for_model_json } = host(&daemon, &first)
        .await
        .call(call(
            "browser_import_lookup",
            json!({"kind":"bookmarks"}),
            "privacy-granted",
        ))
        .await
    else {
        panic!("grant not applied")
    };
    assert!(for_model_json.contains("private.example.test"));
    assert!(!for_model_json.contains("private-address"));
    let ToolOutcome::Deferred { ticket: denied, .. } = host(&daemon, &second)
        .await
        .call(call(
            "browser_import_lookup",
            json!({"kind":"history"}),
            "page-driven-lookup",
        ))
        .await
    else {
        panic!("another conversation inherited private access")
    };
    assert_eq!(
        tcp.clone()
            .oneshot(answer(&denied, "not_now"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(matches!(
        host(&daemon, &second)
            .await
            .call(call(
                "browser_import_lookup",
                json!({"kind":"bookmarks"}),
                "privacy-denied-repeat"
            ))
            .await,
        ToolOutcome::Denied { .. }
    ));
    assert_eq!(
        daemon.shared.db.state(&second).await.unwrap().tickets.len(),
        1
    );
    assert!(matches!(
        host(&daemon, &first)
            .await
            .call(call(
                "browser_import_lookup",
                json!({"kind":"autofill"}),
                "private-address-probe"
            ))
            .await,
        ToolOutcome::Denied { .. }
    ));
    let unix = api::router(daemon.shared.clone(), true);
    assert_eq!(send(&unix, "PUT", "/v1/policy", json!({"default_mode":"full","rules":[{"effect":"deny","capability":"privacy_read","pattern":"browser-imports/default"}]})).await.0, StatusCode::OK);
    assert!(matches!(
        host(&daemon, &first)
            .await
            .call(call(
                "browser_import_lookup",
                json!({"kind":"bookmarks"}),
                "privacy-policy-deny"
            ))
            .await,
        ToolOutcome::Denied { .. }
    ));
    daemon.shutdown().await;
}

#[tokio::test]
async fn import_delivery_native_cookie_fetch_is_limited_to_requested_batch_keys() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = super::otp::otp_config(root.path(), Vec::new()) else {
        return;
    };
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let mut ids = Vec::new();
    for (name, value) in [
        ("requested", "requested-cookie-canary"),
        ("unrelated", "unrelated-cookie-canary"),
    ] {
        let mut read = ProfileRead::default();
        read.data.cookies.push(openside_import::Cookie {
            host: "cookie-scope.example.test".into(),
            name: name.into(),
            value: value.into(),
            path: "/".into(),
            expires: Some(chrono::Utc::now().timestamp() + 3600),
            secure: true,
            http_only: true,
            same_site: Some(openside_import::SameSite::Lax),
            last_accessed_at_ms: None,
            created_at_ms: None,
        });
        read.reports.insert(
            DataKind::Cookies,
            CategoryReport {
                read: 1,
                ..Default::default()
            },
        );
        crate::import_delivery::stage(&daemon.shared, &profile(root.path()), &mut read)
            .await
            .map_err(|error| error.1)
            .unwrap();
        assert_eq!(read.reports[&DataKind::Cookies].imported, 1);
        let router = api::router(daemon.shared.clone(), true);
        let (_, pending, _) = send(&router, "GET", "/v1/import/delivery", Value::Null).await;
        assert!(!pending.to_string().contains(value));
        let id = pending["batches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|batch| !ids.contains(&batch["id"].as_str().unwrap().to_owned()))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        ids.push(id);
    }
    let (device, _) = daemon
        .shared
        .db
        .pair_as(
            "extension".into(),
            vec![CapabilityScope::Read],
            openside_proto::DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    crate::import_delivery::create_pairing_secret(&daemon.shared, &device)
        .await
        .map_err(|error| error.1)
        .unwrap();
    let delivered = signed_batch(&daemon, &ids[0], &device).await;
    assert_eq!(delivered["cookies"].as_array().unwrap().len(), 1);
    assert_eq!(delivered["cookies"][0]["name"], "requested");
    assert_eq!(delivered["cookies"][0]["value"], "requested-cookie-canary");
    assert!(!delivered.to_string().contains("unrelated-cookie-canary"));
    let connections = daemon.shared.connections.list().unwrap();
    let imported = connections
        .iter()
        .find(|connection| connection.site == "example.test")
        .expect("imported persistent HttpOnly cookies were not grouped");
    assert_eq!(imported.source, openside_proto::ConnectionSource::Import);
    assert_eq!(
        imported.status,
        openside_proto::ConnectionStatus::Unverified
    );
    let metadata = serde_json::to_string(&connections).unwrap();
    let audit = std::fs::read_to_string(
        openside_config::data_layout::DataPath::ConnectionsAudit
            .path(&daemon.shared.config.data_dir),
    )
    .unwrap();
    for canary in ["requested-cookie-canary", "unrelated-cookie-canary"] {
        assert!(!metadata.contains(canary));
        assert!(!audit.contains(canary));
    }
    daemon.shutdown().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn import_private_directories_refuse_reads_glob_discovery_symlinks_and_parent_traversal() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let data = &daemon.shared.config.data_dir;
    let session = create_session(
        &daemon,
        "private-import-files",
        PolicyMode::Full,
        Some(root.path()),
    )
    .await;
    let handler = host(&daemon, &session).await;
    for directory in ["browser-imports", "extension-import-keys"] {
        let private = data.join(directory).join("nested");
        std::fs::create_dir_all(&private).unwrap();
        let file = private.join("private.json");
        std::fs::write(&file, "private-file-canary").unwrap();
        let alias = root.path().join(format!("{directory}-alias"));
        std::os::unix::fs::symlink(&private, &alias).unwrap();
        for path in [
            file.clone(),
            alias.join("private.json"),
            private.join("../nested/private.json"),
        ] {
            let outcome = handler
                .call(call(
                    "read",
                    json!({"path":path}),
                    &format!("private-read-{directory}-{}", path.display()),
                ))
                .await;
            assert!(
                matches!(outcome, ToolOutcome::Denied { .. }),
                "private path was readable: {}",
                path.display()
            );
        }
        let descriptor = std::fs::File::open(&file).unwrap();
        assert_eq!(
            daemon
                .shared
                .credential_paths
                .check_fd(&descriptor)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(matches!(
            handler
                .call(call(
                    "read",
                    json!({"path":data.join(directory)}),
                    &format!("private-glob-root-{directory}")
                ))
                .await,
            ToolOutcome::Denied { .. }
        ));
    }
    let ToolOutcome::Done { for_model_json } = handler
        .call(call("read", json!({"path":data}), "private-glob-discovery"))
        .await
    else {
        panic!("safe parent listing failed")
    };
    assert!(!for_model_json.contains("browser-imports"));
    assert!(!for_model_json.contains("extension-import-keys"));
    assert!(!for_model_json.contains("private-file-canary"));
    #[cfg(target_os = "linux")]
    {
        let masks = daemon
            .shared
            .credential_paths
            .sandbox_masks(root.path())
            .unwrap();
        for directory in ["browser-imports", "extension-import-keys"] {
            let path = std::fs::canonicalize(data.join(directory)).unwrap();
            assert!(masks.contains(&path));
            assert!(daemon.shared.credential_paths.sandbox_directory(&path));
        }
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn import_address_suggestions_stay_in_person_form_cards_not_model_handoffs() {
    use super::{
        browser::open_tab,
        otp::{agent_saw, bounded, otp_config, ticket},
    };
    use axum::{response::Html, routing::get, Router};

    const ADDRESS: &str = "IMPORTED_ADDRESS_CANARY_79ce";
    const EMAIL: &str = "imported-email-canary-79ce@example.test";
    const PASSWORD: &str = "imported-password-canary-79ce";
    let root = tempfile::tempdir().unwrap();
    let config = otp_config(root.path(), Vec::new()).expect("patched Chromium required");
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let source = daemon
        .shared
        .config
        .data_dir
        .join("browser-imports/default/sources");
    std::fs::create_dir_all(&source).unwrap();
    let mut addresses =
        vec![json!({"fields":{"street-address":"x".repeat(161),"postal-code":"123456789"}})];
    addresses.extend((0..14).map(|index| {
        json!({"fields":{
            "street-address":format!("{ADDRESS} {index}"),
            "postal-code":"12345",
            "email":EMAIL,
            "password":PASSWORD
        }})
    }));
    std::fs::write(
        source.join("fixture.json"),
        serde_json::to_vec(&json!({"autofill":addresses})).unwrap(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let fixture = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(|| async {
            Html("<!doctype html><title>Shipping address</title><form><label>Street<textarea autocomplete='shipping street-address' maxlength='64'></textarea></label><label>Postal code<input autocomplete='postal-code' maxlength='8'></label><label>Email<input type='email' autocomplete='email'></label><label>Password<input type='password' autocomplete='current-password'></label><label>Code<input autocomplete='one-time-code' maxlength='6'></label><label>Unknown<input autocomplete='not-an-address-token'></label><button>Review</button></form>")
        }))).await.unwrap();
    });
    let session = create_session(&daemon, "import-address-form", PolicyMode::Full, None).await;
    crate::engine::prewarm_runtime(&daemon.shared, &session).await;
    let tab = open_tab(&daemon, &session, &origin, "import-address-open").await;
    let outcome = bounded(
        &daemon,
        &session,
        "request_form",
        json!({"tab_id":tab,"purpose":"form"}),
        "import-address-request",
    )
    .await;
    assert!(!format!("{outcome:?}").contains(ADDRESS));
    let ToolOutcome::Deferred {
        ticket: id,
        kind: DeferKind::External,
    } = outcome
    else {
        panic!("expected a person-only form card")
    };
    let opened = ticket(&daemon, &id).await;
    let fields = opened["form"]["fields"].as_array().unwrap();
    assert_eq!(fields.len(), 6);
    assert_eq!(fields[0]["suggestions"].as_array().unwrap().len(), 10);
    assert!(fields[0]["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|value| value
            .as_str()
            .is_some_and(|text| text.starts_with(ADDRESS) && text.chars().count() <= 64)));
    assert_eq!(fields[1]["suggestions"], json!(["12345"]));
    for field in &fields[2..] {
        assert_eq!(field["suggestions"], json!([]));
    }
    assert!(fields.iter().all(|field| field["initial"].is_null()));
    let card_fields = opened["card"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "input_fields")
        .unwrap();
    assert_eq!(card_fields["fields"], opened["form"]["fields"]);
    assert!(!opened.to_string().contains(EMAIL));
    assert!(!opened.to_string().contains(PASSWORD));
    let assert_projection = |events: &[openside_proto::Event]| {
        assert!(events.iter().any(|event| matches!(&event.kind, EventKind::TicketOpened { ticket } if serde_json::to_string(ticket).unwrap().contains(ADDRESS))));
        for event in events {
            if !matches!(
                event.kind,
                EventKind::TicketOpened { .. } | EventKind::CardPresented { .. }
            ) {
                let serialized = serde_json::to_string(event).unwrap();
                assert!(
                    !serialized.contains(ADDRESS),
                    "non-person event leaked imported addresses: {serialized}"
                );
                assert!(!serialized.contains(EMAIL));
                assert!(!serialized.contains(PASSWORD));
            }
        }
    };
    assert_projection(&daemon.shared.db.events(&session, 0).await.unwrap());
    assert!(!agent_saw(&runtime).contains(ADDRESS));
    let snapshot = bounded(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab}),
        "import-address-model-snapshot",
    )
    .await;
    assert!(!format!("{snapshot:?}").contains(ADDRESS));
    let response = api::router(daemon.shared.clone(), true).oneshot(request(
        "POST", &format!("/v1/tickets/{id}/resolve"),
        json!({"value":{"values":{"field_0":"A different street","field_1":"54321","field_2":"","field_3":"","field_4":"","field_5":""}}}), None,
    )).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        state.tickets[&id].resolution,
        Some(json!({"status":"filled","field_count":6,"next":"review_page"}))
    );
    let TicketResolution::Result { for_model_json } =
        crate::tickets::resolution(&state.tickets[&id])
    else {
        panic!("form resolved as an approval")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&for_model_json).unwrap(),
        json!({"status":"filled","field_count":6,"next":"review_page"})
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if runtime
                .0
                .lock()
                .resolved
                .iter()
                .any(|(ticket, _, _)| ticket == &id)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_projection(&daemon.shared.db.events(&session, 0).await.unwrap());
    assert!(!agent_saw(&runtime).contains(ADDRESS));
    {
        let runtime_state = runtime.0.lock();
        assert!(
            !runtime_state.opened.is_empty(),
            "no actual sidecar session handoff was checked"
        );
        for spec in &runtime_state.opened {
            for data in [
                spec.system_prompt_parts.join("\n"),
                spec.agent_json.clone(),
                spec.credentials_json.clone(),
                spec.resume_state
                    .as_ref()
                    .map(|state| state.data.clone())
                    .unwrap_or_default(),
            ] {
                for canary in [ADDRESS, EMAIL, PASSWORD] {
                    assert!(!data.contains(canary));
                }
            }
        }
    }
    daemon.shutdown().await;
    fixture.abort();
}

#[tokio::test]
async fn import_pairing_continuity_proves_nonce_and_exact_replacement_with_retained_key() {
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};
    async fn redeem(
        router: &axum::Router,
        device: Option<&str>,
        nonce: Option<&str>,
    ) -> (axum::http::HeaderMap, axum::body::Bytes) {
        let (_, offer, _) = send(router, "POST", "/v1/pairing", json!({"name":"extension","scopes":["read"],"kind":"person_device","import_delivery":true})).await;
        let mut request = super::request(
            "POST",
            "/v1/pairing/exchange",
            json!({"code":offer["code"]}),
            None,
        );
        request.headers_mut().insert(
            "Origin",
            "chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj"
                .parse()
                .unwrap(),
        );
        if let Some(device) = device {
            request
                .headers_mut()
                .insert("x-openside-import-device", device.parse().unwrap());
        }
        if let Some(nonce) = nonce {
            request
                .headers_mut()
                .insert("x-openside-import-nonce", nonce.parse().unwrap());
        }
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        (
            response.headers().clone(),
            response.into_body().collect().await.unwrap().to_bytes(),
        )
    }
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);
    let (first_headers, first) = redeem(&router, None, None).await;
    assert!(first_headers.get("x-openside-import-proof").is_none());
    let old: Value = serde_json::from_slice(&first).unwrap();
    let device = old["device_id"].as_str().unwrap();
    let decode = |text: &str| {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>()
    };
    let key = decode(old["import_secret"].as_str().unwrap());
    let nonce = "34".repeat(32);
    let (headers, payload) = redeem(&router, Some(device), Some(&nonce)).await;
    let proof = decode(headers["x-openside-import-proof"].to_str().unwrap());
    let mac = |payload: &[u8], nonce: &str| {
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
        mac.update(b"openside-import-pairing-v1\n");
        mac.update(device.as_bytes());
        mac.update(nonce.as_bytes());
        mac.update(&Sha256::digest(payload));
        mac
    };
    mac(&payload, &nonce).verify_slice(&proof).unwrap();
    let mut changed: Value = serde_json::from_slice(&payload).unwrap();
    changed["import_secret"] = json!("ff".repeat(32));
    assert!(mac(&serde_json::to_vec(&changed).unwrap(), &nonce)
        .verify_slice(&proof)
        .is_err());
    assert!(mac(&payload, &"35".repeat(32))
        .verify_slice(&proof)
        .is_err());
    let path = openside_config::data_layout::DataPath::ExtensionImportKeys
        .path(&daemon.shared.config.data_dir)
        .join(device);
    assert_eq!(std::fs::read(&path).unwrap(), key);
    for (device, nonce) in [
        (device, "bad"),
        ("ffffffffffffffffffffffffffffffff", nonce.as_str()),
    ] {
        assert!(redeem(&router, Some(device), Some(nonce))
            .await
            .0
            .get("x-openside-import-proof")
            .is_none());
    }
    std::fs::remove_file(&path).unwrap();
    assert!(redeem(&router, Some(device), Some(&nonce))
        .await
        .0
        .get("x-openside-import-proof")
        .is_none());
    crate::import_delivery::create_pairing_secret(&daemon.shared, device)
        .await
        .map_err(|error| error.1)
        .unwrap();
    crate::pairing::revoke(&daemon.shared, device)
        .await
        .map_err(|error| error.1)
        .unwrap();
    assert!(redeem(&router, Some(device), Some(&nonce))
        .await
        .0
        .get("x-openside-import-proof")
        .is_none());
    daemon.shutdown().await;
}
