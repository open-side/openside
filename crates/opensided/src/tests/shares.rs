//! Read-only share links: what a snapshot keeps and drops, revocation, expiry, chat
//! deletion, token-less public reads and their rate limits.

use super::*;

const SECRET: &str = "sunflower-4402";

async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, token))
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, bytes.to_vec())
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

async fn emit(daemon: &Daemon, session: &SessionId, kind: EventKind) {
    daemon.shared.emit(session, None, kind).await.unwrap();
}

/// A delivered message, a failed step whose data holds secrets, a reply with a credential
/// line, a message still queued, thinking and a pending ticket.
async fn conversation(daemon: &Daemon, session: &SessionId) {
    emit(
        daemon,
        session,
        EventKind::MessageQueued {
            message_id: MessageId::new("m1"),
            text: format!("my code is {SECRET}, book the table"),
            mode: QueueMode::Queue,
            attachments: Vec::new(),
            author_kind: MessageAuthorKind::Person,
            from_agent: None,
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::MessageInjected {
            message_id: MessageId::new("m1"),
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::ThinkingDelta {
            text: "private thoughts".into(),
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::ToolStarted {
            tool_call_id: "c1".into(),
            name: "browser_navigate".into(),
            args: json!({"url": "https://bank.example/account?id=991"}),
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::ToolEnded {
            tool_call_id: "c1".into(),
            result: json!({"page": "balance 1234.56"}),
            is_error: true,
            card: None,
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::ToolStarted {
            tool_call_id: "c2".into(),
            name: "read".into(),
            args: json!({"path": "/home/me/notes.txt"}),
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::AssistantFinal {
            text: "Booked for seven.\napi_key = sk-ABCDEFGHIJKLMNOPQRSTUV\nEnjoy.".into(),
            attachments: Vec::new(),
        },
    )
    .await;
    emit(
        daemon,
        session,
        EventKind::MessageQueued {
            message_id: MessageId::new("m2"),
            text: "never delivered".into(),
            mode: QueueMode::Queue,
            attachments: Vec::new(),
            author_kind: MessageAuthorKind::Person,
            from_agent: None,
        },
    )
    .await;
    opened(
        daemon,
        session,
        TicketKind::Confirm,
        "pending-ticket",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
}

/// A tool-written file recorded the way the artifact tracker records it.
async fn artifact(daemon: &Daemon, session: &SessionId, path: &str, bytes: &[u8]) {
    let file = daemon
        .shared
        .config
        .data_dir
        .join("artifacts")
        .join(session.as_str())
        .join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, bytes).unwrap();
    emit(
        daemon,
        session,
        EventKind::ArtifactSaved {
            path: path.into(),
            size: bytes.len() as u64,
            mime: crate::artifacts::mime_for(path, bytes),
            tool_call_id: "c2".into(),
            tool: "bash".into(),
        },
    )
    .await;
}

fn every_file(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(every_file(&path));
        } else {
            out.push(std::fs::read(path).unwrap());
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

async fn share(router: &axum::Router, session: &str, body: Value) -> Value {
    let (status, _, bytes) = call(
        router,
        "POST",
        &format!("/v1/sessions/{session}/shares"),
        body,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    json_of(&bytes)
}

/// A 3×2 PNG with `text` in a metadata chunk.
fn tagged_png(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 3, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .add_text_chunk("Comment".into(), text.into())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[90_u8; 3 * 2 * 4]).unwrap();
    }
    out
}

#[tokio::test]
async fn a_snapshot_keeps_the_conversation_but_no_secrets_or_tool_data() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([
            &openside_vault::Secret::new(SECRET.into()),
        ]));
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    conversation(&daemon, &session).await;
    artifact(
        &daemon,
        &session,
        "files/plan.txt",
        format!("table for two, code {SECRET}").as_bytes(),
    )
    .await;
    artifact(
        &daemon,
        &session,
        "files/leak.env",
        b"token = sk-ABCDEFGHIJKLMNOPQRSTUV",
    )
    .await;
    let png = tagged_png(SECRET);
    artifact(&daemon, &session, "files/photo.png", &png).await;
    artifact(
        &daemon,
        &session,
        "files/report.pdf",
        b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n1 0 obj",
    )
    .await;
    artifact(
        &daemon,
        &session,
        "files/blob.bin",
        b"\xff\xfe\x00\x01binary",
    )
    .await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);

    let link = share(
        &unix,
        "chat",
        json!({"artifacts": ["files/plan.txt", "files/photo.png"], "expires_in_secs": 3600}),
    )
    .await;
    let id = link["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), SHARE_ID_LEN);
    assert_eq!(link["path"], format!("/share#{id}"));
    assert_eq!(link["url"], Value::Null, "no remote access configured");
    assert_eq!(link["messages"], 2);
    assert_eq!(link["artifacts"].as_array().unwrap().len(), 2);
    assert!(link["expires_at"].is_string());

    // No token at all, over TCP: the id is the credential.
    let (status, headers, body) =
        call(&tcp, "GET", &format!("/v1/shares/{id}"), json!({}), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-robots-tag"], "noindex");
    for leaked in [
        SECRET,
        "sk-ABCDEF",
        "private thoughts",
        "never delivered",
        "bank.example",
        "balance",
        "notes.txt",
        "Human step",
    ] {
        assert!(!contains(&body, leaked), "the snapshot leaks {leaked:?}");
    }
    let chat = json_of(&body);
    let items = chat["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{chat}");
    assert_eq!(items[0]["kind"], "user");
    assert_eq!(items[0]["text"], "my code is [REDACTED], book the table");
    assert_eq!(
        items[1],
        json!({"kind": "steps", "steps": [
            {"tool": "browser_navigate", "failed": true},
            {"tool": "read", "failed": false},
        ]})
    );
    assert_eq!(items[2]["kind"], "assistant");
    assert_eq!(items[2]["text"], "Booked for seven.\n[REDACTED]\nEnjoy.");

    let (status, headers, text) = call(
        &tcp,
        "GET",
        &format!("/v1/shares/{id}/artifacts/0"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, b"table for two, code [REDACTED]");
    assert_eq!(headers["content-type"], "text/plain; charset=utf-8");
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    assert_eq!(
        headers["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let (status, headers, image) = call(
        &tcp,
        "GET",
        &format!("/v1/shares/{id}/artifacts/1"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Re-encoded: the pixels survive, the metadata chunk holding the secret does not.
    assert!(!contains(&image, SECRET) && contains(&png, SECRET));
    assert_eq!(
        image::GenericImageView::dimensions(&image::load_from_memory(&image).unwrap()),
        (3, 2)
    );
    assert_eq!(headers["content-type"], "image/png");
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("inline;"));
    assert_eq!(
        call(
            &tcp,
            "GET",
            &format!("/v1/shares/{id}/artifacts/2"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Nothing stored behind the link holds the secret either.
    let stored = every_file(&daemon.shared.config.data_dir.join("shares"));
    assert!(!stored.is_empty());
    assert!(stored.iter().all(|file| !contains(file, SECRET)));

    // Without steps, only messages remain.
    let bare = share(&unix, "chat", json!({"steps": false})).await;
    let (_, _, body) = call(
        &tcp,
        "GET",
        &format!("/v1/shares/{}", bare["id"].as_str().unwrap()),
        json!({}),
        None,
    )
    .await;
    let kinds: Vec<_> = json_of(&body)["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["user", "assistant"]);

    // Refusals: a text artifact that still looks like a credential, the transcript, an
    // unknown or duplicate path, a bad lifetime.
    for (body, code) in [
        (
            json!({"artifacts": ["files/leak.env"]}),
            "sensitive_artifact",
        ),
        (
            json!({"artifacts": ["files/report.pdf"]}),
            "unsupported_share_artifact",
        ),
        (
            json!({"artifacts": ["files/blob.bin"]}),
            "unsupported_share_artifact",
        ),
        (
            json!({"artifacts": ["transcript.jsonl"]}),
            "invalid_share_artifact",
        ),
        (
            json!({"artifacts": ["files/absent.txt"]}),
            "invalid_share_artifact",
        ),
        (
            json!({"artifacts": ["files/plan.txt", "files/plan.txt"]}),
            "invalid_share_artifact",
        ),
        (json!({"expires_in_secs": 30}), "invalid_expiry"),
    ] {
        let (status, _, bytes) = call(&unix, "POST", "/v1/sessions/chat/shares", body, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json_of(&bytes)["code"], code);
    }
    // Only a person shares: an agent client holding steer is refused.
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Steer],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (status, _, bytes) = call(
        &tcp,
        "POST",
        "/v1/sessions/chat/shares",
        json!({}),
        Some(&agent),
    )
    .await;
    assert_eq!(
        (status, json_of(&bytes)["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("person_required"))
    );
    // A read token may list but not publish.
    let read = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap()
        .1;
    let (status, _, bytes) = call(
        &tcp,
        "GET",
        "/v1/sessions/chat/shares",
        json!({}),
        Some(&read),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&bytes)["shares"].as_array().unwrap().len(), 2);
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/v1/sessions/chat/shares",
            json!({}),
            Some(&read)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn revoked_expired_and_deleted_chat_links_answer_404() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.remote.public_url = Some("https://desk.example.test/".into());
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    conversation(&daemon, &session).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let get = |id: String| {
        let tcp = tcp.clone();
        async move {
            let (status, _, body) =
                call(&tcp, "GET", &format!("/v1/shares/{id}"), json!({}), None).await;
            (
                status,
                serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null)["code"].clone(),
            )
        }
    };

    let first = share(&unix, "chat", json!({})).await;
    let first_id = first["id"].as_str().unwrap().to_owned();
    assert_eq!(
        first["url"],
        format!("https://desk.example.test/share#{first_id}"),
        "the link goes through remote access"
    );
    assert_eq!(first["expires_at"], Value::Null);
    let second = share(&unix, "chat", json!({})).await;
    let second_id = second["id"].as_str().unwrap().to_owned();
    assert_ne!(first_id, second_id);
    let (_, _, listed) = call(&unix, "GET", "/v1/sessions/chat/shares", json!({}), None).await;
    let ids: Vec<_> = json_of(&listed)["shares"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| link["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, [second_id.clone(), first_id.clone()], "newest first");

    // Revoke: gone at once, and only through its own chat.
    let other = create_session(&daemon, "other", PolicyMode::Guard, None).await;
    assert_eq!(
        call(
            &unix,
            "DELETE",
            &format!("/v1/sessions/{other}/shares/{first_id}"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(first_id.clone()).await.0, StatusCode::OK);
    assert_eq!(
        call(
            &unix,
            "DELETE",
            &format!("/v1/sessions/chat/shares/{first_id}"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(first_id.clone()).await,
        (StatusCode::NOT_FOUND, json!("share_not_found"))
    );
    assert_eq!(
        call(
            &tcp,
            "GET",
            &format!("/v1/shares/{first_id}/artifacts/0"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &unix,
            "DELETE",
            &format!("/v1/sessions/chat/shares/{first_id}"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Expiry: a link past its time answers 404 and its files are removed.
    let expiring = share(&unix, "chat", json!({"expires_in_secs": 60})).await;
    let expiring_id = expiring["id"].as_str().unwrap().to_owned();
    assert_eq!(get(expiring_id.clone()).await.0, StatusCode::OK);
    let dir = daemon
        .shared
        .config
        .data_dir
        .join("shares")
        .join(&expiring_id);
    let record = dir.join("share.json");
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    stored["chat"]["expires_at"] = json!((Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
    std::fs::write(&record, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert_eq!(get(expiring_id).await.0, StatusCode::NOT_FOUND);
    assert!(!dir.exists());

    // Deleting the chat revokes every link it had.
    assert_eq!(get(second_id.clone()).await.0, StatusCode::OK);
    assert_eq!(
        call(&unix, "DELETE", "/v1/sessions/chat", json!({}), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(get(second_id.clone()).await.0, StatusCode::NOT_FOUND);
    assert!(!daemon
        .shared
        .config
        .data_dir
        .join("shares")
        .join(&second_id)
        .exists());
    assert_eq!(
        call(&unix, "GET", "/v1/sessions/chat/shares", json!({}), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn public_reads_need_no_token_and_are_rate_limited() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    conversation(&daemon, &session).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let id = share(&unix, "chat", json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/v1/shares/{id}");
    // An unknown bearer is irrelevant: the route never looks at it.
    assert_eq!(
        call(&tcp, "GET", &path, json!({}), Some("not-a-token"))
            .await
            .0,
        StatusCode::OK
    );
    for _ in 1..crate::shares::LINK_RATE {
        assert_eq!(
            call(&tcp, "GET", &path, json!({}), None).await.0,
            StatusCode::OK
        );
    }
    let (status, headers, body) = call(&tcp, "GET", &path, json!({}), None).await;
    assert_eq!(
        (status, json_of(&body)["code"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("share_rate_limited"))
    );
    assert_eq!(headers["retry-after"], "60");

    // Guessing spends a separate daemon-wide budget.
    for n in 0..crate::shares::MISS_RATE {
        let guess = format!("{n:0>43}");
        let (status, _, body) =
            call(&tcp, "GET", &format!("/v1/shares/{guess}"), json!({}), None).await;
        assert_eq!(
            (status, json_of(&body)["code"].as_str()),
            (StatusCode::NOT_FOUND, Some("share_not_found"))
        );
    }
    assert_eq!(
        call(&tcp, "GET", "/v1/shares/not-even-an-id", json!({}), None)
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    daemon.shutdown().await;
}

/// A public GET from `peer` whose body has not been read yet.
async fn held(router: &axum::Router, path: &str, peer: [u8; 4]) -> axum::response::Response {
    let mut request = request("GET", path, json!({}), None);
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            peer, 4000,
        ))));
    router.clone().oneshot(request).await.unwrap()
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn assert_public_headers(headers: &axum::http::HeaderMap) {
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers.contains_key("x-robots-tag"));
}

#[tokio::test]
async fn revocation_fails_closed_and_cuts_off_running_downloads() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    conversation(&daemon, &session).await;
    artifact(
        &daemon,
        &session,
        "files/notes.txt",
        "dinner plans\n".repeat(20_000).as_bytes(),
    )
    .await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let shares = daemon.shared.config.data_dir.join("shares");
    let id = share(&unix, "chat", json!({"artifacts": ["files/notes.txt"]})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let revoke = format!("/v1/sessions/chat/shares/{id}");
    let read = format!("/v1/shares/{id}");

    // No tombstone, no revocation: the person is told, and the link is still what it was.
    set_mode(&shares, 0o500);
    let (status, _, _) = call(&unix, "DELETE", &revoke, json!({}), None).await;
    set_mode(&shares, 0o700);
    assert!(status.is_server_error(), "{status}");
    assert_eq!(
        call(&tcp, "GET", &read, json!({}), None).await.0,
        StatusCode::OK
    );

    // A download already under way stops at its next chunk once the link is revoked, even
    // though the snapshot's files cannot be deleted yet.
    let download = held(&tcp, &format!("{read}/artifacts/0"), [10, 0, 0, 1]).await;
    assert_eq!(download.status(), StatusCode::OK);
    set_mode(&shares.join(&id), 0o500);
    assert_eq!(
        call(&unix, "DELETE", &revoke, json!({}), None).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(download.into_body().collect().await.is_err());
    let (status, headers, _) = call(&tcp, "GET", &read, json!({}), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_public_headers(&headers);
    assert!(shares.join(&id).exists());
    set_mode(&shares.join(&id), 0o700);
    assert_eq!(
        call(&unix, "GET", "/v1/sessions/chat/shares", json!({}), None)
            .await
            .0,
        StatusCode::OK
    );
    assert!(!shares.join(&id).exists());
    assert!(!shares.join(format!(".{id}.revoked")).exists());

    // Deleting the chat revokes its links the same way.
    let id = share(&unix, "chat", json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    set_mode(&shares.join(&id), 0o500);
    assert_eq!(
        call(&unix, "DELETE", "/v1/sessions/chat", json!({}), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&tcp, "GET", &format!("/v1/shares/{id}"), json!({}), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(shares.join(format!(".{id}.revoked")).exists());
    set_mode(&shares.join(&id), 0o700);
    daemon.shutdown().await;
}

#[tokio::test]
async fn one_caller_cannot_hold_every_download_slot() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    conversation(&daemon, &session).await;
    artifact(&daemon, &session, "files/notes.txt", b"dinner plans").await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let id = share(&unix, "chat", json!({"artifacts": ["files/notes.txt"]})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/v1/shares/{id}/artifacts/0");
    let mut open = Vec::new();
    for _ in 0..crate::shares::SOURCE_IN_FLIGHT {
        let response = held(&tcp, &path, [10, 0, 0, 1]).await;
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let busy = held(&tcp, &path, [10, 0, 0, 1]).await;
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_public_headers(busy.headers());
    // Someone else still gets through, and a finished download frees its slot.
    assert_eq!(
        held(&tcp, &path, [10, 0, 0, 2]).await.status(),
        StatusCode::OK
    );
    let first = open.remove(0);
    assert_eq!(
        first
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        b"dinner plans"
    );
    assert_eq!(
        held(&tcp, &path, [10, 0, 0, 1]).await.status(),
        StatusCode::OK
    );

    // One caller's guessing does not lock out another caller.
    for n in 0..crate::shares::MISS_RATE {
        held(&tcp, &format!("/v1/shares/{n:0>43}"), [10, 0, 0, 3]).await;
    }
    assert_eq!(
        held(&tcp, "/v1/shares/x", [10, 0, 0, 3]).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        held(&tcp, "/v1/shares/x", [10, 0, 0, 4]).await.status(),
        StatusCode::NOT_FOUND
    );
    drop(open);
    daemon.shutdown().await;
}

#[tokio::test]
async fn the_sweeper_deletes_expired_snapshots_and_stale_staging() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    conversation(&daemon, &session).await;
    let unix = api::router(daemon.shared.clone(), true);
    let hour = share(&unix, "chat", json!({"expires_in_secs": 3600})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let forever = share(&unix, "chat", json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let shares = daemon.shared.config.data_dir.join("shares");
    std::fs::create_dir(shares.join(".crashed.tmp")).unwrap();
    std::fs::write(shares.join(".crashed.tmp").join("0"), b"half").unwrap();

    assert_eq!(
        crate::shares::sweep(&daemon.shared, chrono::Utc::now()).await,
        0
    );
    assert!(shares.join(&hour).exists());
    assert!(!shares.join(".crashed.tmp").exists());
    let later = chrono::Utc::now() + chrono::Duration::hours(2);
    assert_eq!(crate::shares::sweep(&daemon.shared, later).await, 1);
    assert!(!shares.join(&hour).exists());
    assert!(shares.join(&forever).exists());
    daemon.shutdown().await;
}

#[tokio::test]
async fn oversized_snapshots_are_refused_before_they_are_built() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "chat", PolicyMode::Guard, None).await;
    let unix = api::router(daemon.shared.clone(), true);
    // Each file is small on disk, but the index lists it at 9 MiB: three of them exceed
    // the 25 MiB budget from the listed sizes alone.
    for n in 0..3 {
        let path = format!("files/part{n}.txt");
        let file = daemon
            .shared
            .config
            .data_dir
            .join("artifacts/chat")
            .join(&path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"part").unwrap();
        emit(
            &daemon,
            &session,
            EventKind::ArtifactSaved {
                path: path.clone(),
                size: 9 * 1024 * 1024,
                mime: "text/plain".into(),
                tool_call_id: "c2".into(),
                tool: "bash".into(),
            },
        )
        .await;
    }
    let (status, _, body) = call(
        &unix,
        "POST",
        "/v1/sessions/chat/shares",
        json!({"artifacts": ["files/part0.txt", "files/part1.txt", "files/part2.txt"]}),
        None,
    )
    .await;
    assert_eq!(
        (status, json_of(&body)["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("share_too_large"))
    );
    emit(
        &daemon,
        &session,
        EventKind::AssistantFinal {
            text: "x".repeat(5 * 1024 * 1024),
            attachments: Vec::new(),
        },
    )
    .await;
    let (status, _, _) = call(&unix, "POST", "/v1/sessions/chat/shares", json!({}), None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    daemon.shutdown().await;
}
