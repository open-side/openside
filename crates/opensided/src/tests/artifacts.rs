//! Task artifacts (A-12, A-15, PP-14): attachments, tool-output tracking, signed sandboxed
//! previews, visual blocks and cleanup on session deletion.

use super::*;
use base64::Engine;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn png_image(width: u32, height: u32, noise: bool) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let mut state = 0x9e37_79b9u32;
        let data: Vec<u8> = (0..width * height * 4)
            .map(|index| {
                if noise {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                } else {
                    (index % 7) as u8
                }
            })
            .collect();
        writer.write_image_data(&data).unwrap();
    }
    out
}

async fn call_api(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

async fn get_json(router: &axum::Router, path: &str) -> Value {
    let (status, _, body) = call_api(router, "GET", path, json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{path}: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}

fn raw_path(session: &str, path: &str) -> String {
    format!(
        "/v1/sessions/{session}/artifacts/{}",
        path.replace('/', "%2F")
    )
}

fn queued(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::MessageQueued { .. }))
        .count()
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn attachments_are_stored_privately_logged_by_reference_and_reach_the_runtime() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let image = png_image(8, 8, false);
    let note = "line one\nline two".as_bytes();

    let (status, _, body) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"m1","text":"what is in these?","mode":"queue","attachments":[
            {"name":"../../shot.PNG","data_base64":b64(&image)},
            {"name":"notes.txt","data_base64":b64(note)},
        ]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&body)
    );

    // The runtime receives the bytes with sniffed types, once, on delivery.
    until(|| runtime.0.lock().parts.len() == 1).await;
    let (message, parts) = runtime.0.lock().parts[0].clone();
    assert_eq!(message.as_str(), "m1");
    assert_eq!(parts.len(), 2);
    assert_eq!(
        (parts[0].kind, parts[0].mime.as_str()),
        (openside_core::ContentKind::Image, "image/png")
    );
    assert_eq!(parts[0].data.as_deref(), Some(image.as_slice()));
    assert_eq!(
        parts[0].artifact_ref.as_deref(),
        Some("attachments/m1/1-shot.PNG")
    );
    assert_eq!(
        (parts[1].kind, parts[1].mime.as_str()),
        (openside_core::ContentKind::Text, "text/plain")
    );
    assert_eq!(parts[1].data.as_deref(), Some(note));

    // The transcript carries a reference with a digest, never the bytes.
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    let attachments = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::MessageQueued { attachments, .. } => Some(attachments.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(attachments[0].path, "attachments/m1/1-shot.PNG");
    assert_eq!(attachments[0].size, image.len() as u64);
    assert_eq!(attachments[0].sha256.len(), 64);
    assert_eq!(attachments[1].path, "attachments/m1/2-notes.txt");
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains(&b64(&image)) && !log.contains("line two"));

    let dir = daemon.shared.config.data_dir.join("artifacts/session");
    assert_eq!(
        std::fs::read(dir.join("attachments/m1/1-shot.PNG")).unwrap(),
        image
    );
    #[cfg(unix)]
    {
        assert_eq!(
            mode(&daemon.shared.config.data_dir.join("artifacts")),
            0o700
        );
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("attachments/m1")), 0o700);
        assert_eq!(mode(&dir.join("attachments/m1/1-shot.PNG")), 0o600);
    }

    // Listed with a preview, downloadable as an attachment.
    let list = get_json(&router, "/v1/sessions/session/artifacts").await;
    let listed = &list["artifacts"][0];
    assert_eq!(listed["path"], "attachments/m1/1-shot.PNG");
    assert_eq!(listed["source"], "attachment");
    assert_eq!(listed["mime"], "image/png");
    assert!(listed["preview_url"]
        .as_str()
        .unwrap()
        .contains("/preview?exp="));
    let (status, headers, bytes) = call_api(
        &router,
        "GET",
        &raw_path("session", "attachments/m1/1-shot.PNG"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, image);
    assert_eq!(headers["content-type"], "image/png");
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(headers["content-security-policy"]
        .to_str()
        .unwrap()
        .starts_with("sandbox"));

    // An idempotent retry neither duplicates the message nor rewrites stored files.
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"m1","text":"what is in these?","mode":"queue","attachments":[{"data_base64":b64(b"different")}]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        std::fs::read(dir.join("attachments/m1/1-shot.PNG")).unwrap(),
        image
    );
    assert!(!dir.join("attachments/m1/1-attachment.txt").exists());
    daemon.shutdown().await;
}

#[tokio::test]
async fn attachment_validation_rejects_without_queueing_anything() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let elf = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0xff, 0xfe];
    let cases = [
        // Declared names never decide the type: an executable named .png is refused.
        (
            json!([{"name":"cat.png","data_base64":b64(&elf)}]),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_attachment",
        ),
        (
            json!([{"data_base64":"not base64!"}]),
            StatusCode::BAD_REQUEST,
            "invalid_attachment",
        ),
        (
            json!(vec![json!({"data_base64":b64(b"x")}); 5]),
            StatusCode::PAYLOAD_TOO_LARGE,
            "attachments_too_large",
        ),
        (
            json!([{"data_base64":b64(&vec![b'a'; MAX_ATTACHMENT_BYTES + 1])}]),
            StatusCode::PAYLOAD_TOO_LARGE,
            "attachments_too_large",
        ),
        (
            json!([{"data_base64":"AAAA","mime":"image/png"}]),
            StatusCode::UNPROCESSABLE_ENTITY,
            "",
        ),
    ];
    for (attachments, expected, code) in cases {
        let (status, _, body) = call_api(
            &router,
            "POST",
            "/v1/sessions/session/messages",
            json!({"text":"hi","mode":"queue","attachments":attachments}),
        )
        .await;
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
        if !code.is_empty() {
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["code"],
                code
            );
        }
    }
    // A body beyond the submit limit is refused before it is parsed.
    let huge = "a".repeat(crate::artifacts::SUBMIT_BODY_LIMIT + 1);
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"text":huge,"mode":"queue"}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        queued(&daemon.shared.db.events(&session, 0).await.unwrap()),
        0
    );
    assert!(!daemon
        .shared
        .config
        .data_dir
        .join("artifacts/session/attachments")
        .exists());
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn write_edit_and_bash_outputs_are_collected_per_task() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    // Pass-through stand-in for bubblewrap: runs the command after `--`.
    let program = root.path().join("test-bwrap");
    std::fs::write(
        &program,
        "#!/bin/sh\nwhile [ \"$1\" != '--' ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = root.path().join("work");
    std::fs::create_dir_all(workspace.join(".git")).unwrap();
    std::fs::write(workspace.join("untouched.txt"), "old").unwrap();
    let mut config = resolved_config(root.path());
    config.bwrap = Some(program);
    // The data directory inside the workspace must not show up as tool output.
    config.data_dir = workspace.join(".openside-data");
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "session", PolicyMode::Full, Some(&workspace)).await;
    let host = host(&daemon, &session).await;

    let write = host
        .call(call(
            "write",
            json!({"path":"report.md","content":"# draft"}),
            "w1",
        ))
        .await;
    assert!(matches!(write, ToolOutcome::Done { .. }), "{write:?}");
    let edit = host
        .call(call(
            "edit",
            json!({"path":"report.md","old_text":"draft","new_text":"final"}),
            "e1",
        ))
        .await;
    assert!(matches!(edit, ToolOutcome::Done { .. }), "{edit:?}");
    let bash = host
        .call(call(
            "bash",
            json!({"command":"mkdir -p out && printf '<svg xmlns=\"http://www.w3.org/2000/svg\"/>' > out/chart.svg && printf x > .git/HEAD"}),
            "b1",
        ))
        .await;
    #[cfg(target_os = "linux")]
    assert!(matches!(bash, ToolOutcome::Done { .. }), "{bash:?}");
    #[cfg(not(target_os = "linux"))]
    assert!(
        matches!(bash, ToolOutcome::Denied { ref reason, .. } if reason == "Credential-safe sandbox setup is supported on Linux"),
        "{bash:?}"
    );

    let saved: Vec<(String, String, String)> = daemon
        .shared
        .db
        .events(&session, 0)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ArtifactSaved {
                path,
                tool_call_id,
                tool,
                ..
            } => Some((path, tool, tool_call_id)),
            _ => None,
        })
        .collect();
    let expected = [
        ("files/report.md", "write", "w1"),
        ("files/report.md", "edit", "e1"),
        #[cfg(target_os = "linux")]
        ("files/out/chart.svg", "bash", "b1"),
    ]
    .map(|(path, tool, id)| (path.to_owned(), tool.to_owned(), id.to_owned()));
    assert_eq!(saved, expected);

    let router = api::router(daemon.shared.clone(), true);
    let list = get_json(&router, "/v1/sessions/session/artifacts").await;
    let files: Vec<_> = list["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|artifact| {
            (
                artifact["path"].as_str().unwrap(),
                artifact["source"].as_str().unwrap(),
                artifact["tool"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        files,
        [
            ("files/report.md", "tool", Some("edit")),
            #[cfg(target_os = "linux")]
            ("files/out/chart.svg", "tool", Some("bash")),
            ("transcript.jsonl", "transcript", None),
        ]
    );
    let (_, _, report) = call_api(
        &router,
        "GET",
        &raw_path("session", "files/report.md"),
        json!({}),
    )
    .await;
    assert_eq!(report, b"# final");
    // The transcript export is the session's event log, one JSON event per line.
    let (status, headers, transcript) = call_api(
        &router,
        "GET",
        &raw_path("session", "transcript.jsonl"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json");
    let lines: Vec<Value> = String::from_utf8(transcript)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["kind"], "session.created");
    assert!(lines.iter().any(|event| event["kind"] == "artifact.saved"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn previews_are_signed_sandboxed_and_visual_blocks_are_indexed() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let other = create_session(&daemon, "other", PolicyMode::Guard, None).await;
    let html = "<h1>Hi</h1><script>document.title = 'x'</script>";
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"4\"/></svg>";
    let text = format!(
        "Here:\n```html visual\n{html}\n```\nand\n````svg visual\n{svg}\n````\nnot this:\n```html\n<p>code</p>\n```\n"
    );
    daemon
        .shared
        .emit(&session, None, EventKind::AssistantFinal { text })
        .await
        .unwrap();
    // Remote (TCP) router: previews are reachable without a bearer token, nothing else is.
    let router = api::router(daemon.shared.clone(), false);
    let unix = api::router(daemon.shared.clone(), true);

    let visuals = get_json(&unix, "/v1/sessions/session/visuals").await;
    let visuals = visuals["visuals"].as_array().unwrap();
    assert_eq!(visuals.len(), 2, "{visuals:?}");
    assert_eq!(
        (visuals[0]["kind"].as_str(), visuals[0]["source"].as_str()),
        (Some("html"), Some(html))
    );
    assert_eq!(
        (visuals[1]["kind"].as_str(), visuals[1]["source"].as_str()),
        (Some("svg"), Some(svg))
    );

    for (visual, content_type) in [
        (&visuals[0], "text/html; charset=utf-8"),
        (&visuals[1], "image/svg+xml"),
    ] {
        let url = visual["preview_url"].as_str().unwrap();
        let (status, headers, body) = call_api(&router, "GET", url, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{url}");
        assert_eq!(
            String::from_utf8(body).unwrap(),
            visual["source"].as_str().unwrap()
        );
        assert_eq!(headers["content-type"], content_type);
        let csp = headers["content-security-policy"].to_str().unwrap();
        // Opaque origin: no access to the app's storage, cookies or API.
        assert!(
            csp.starts_with("sandbox") && !csp.contains("allow-same-origin"),
            "{csp}"
        );
        assert!(
            csp.contains("default-src 'none'") && !csp.contains("connect-src"),
            "{csp}"
        );
        // Scripts could exfiltrate by navigating their own frame; no preview runs them.
        assert!(
            !csp.contains("allow-scripts") && !csp.contains("script-src"),
            "{csp}"
        );
        assert!(csp.contains("frame-ancestors 'self'"));
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
    }

    let url = visuals[0]["preview_url"].as_str().unwrap().to_owned();
    let path = visuals[0]["path"].as_str().unwrap();
    let past = Utc::now().timestamp() - 1;
    let flipped = {
        let last = url.chars().last().unwrap();
        format!(
            "{}{}",
            &url[..url.len() - 1],
            if last == '0' { '1' } else { '0' }
        )
    };
    for bad in [
        flipped,
        url.split('?').next().unwrap().to_owned(),
        crate::artifacts::signed_preview_url(&session, path, past),
        // A signature for one session never opens another session's file.
        crate::artifacts::signed_preview_url(&other, path, past + 600)
            .replace("/sessions/other/", "/sessions/session/"),
    ] {
        let (status, _, body) = call_api(&router, "GET", &bad, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            "invalid_preview_signature"
        );
    }
    // Listing and raw downloads stay behind authentication.
    for path in [
        "/v1/sessions/session/artifacts".to_owned(),
        raw_path("session", path),
        "/v1/sessions/session/visuals".into(),
    ] {
        let (status, _, _) = call_api(&router, "GET", &path, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    // Paths are confined to the session directory.
    for path in [
        "..%2Fother%2Ftranscript.jsonl",
        "%2Fetc%2Fpasswd",
        "visuals%2F..%2F..%2Fother",
    ] {
        let (status, _, _) = call_api(
            &unix,
            "GET",
            &format!("/v1/sessions/session/artifacts/{path}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    #[cfg(unix)]
    {
        let dir = daemon.shared.config.data_dir.join("artifacts/session");
        std::fs::write(root.path().join("secret.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(root.path().join("secret.txt"), dir.join("leak.txt")).unwrap();
        let (status, _, _) =
            call_api(&unix, "GET", &raw_path("session", "leak.txt"), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let leak = crate::artifacts::signed_preview_url(&session, "leak.txt", past + 600);
        let (status, _, _) = call_api(&router, "GET", &leak, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn deleting_a_session_removes_its_artifact_directory() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "doomed", PolicyMode::Guard, None).await;
    create_session(&daemon, "keeper", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    for session in ["doomed", "keeper"] {
        let (status, _, _) = call_api(
            &router,
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            json!({"text":"keep this","mode":"queue","attachments":[{"name":"a.txt","data_base64":b64(b"hello")}]}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
    let artifacts = daemon.shared.config.data_dir.join("artifacts");
    assert!(artifacts.join("doomed").is_dir());

    let (status, _, _) = call_api(&router, "DELETE", "/v1/sessions/doomed", json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!artifacts.join("doomed").exists());
    assert!(artifacts.join("keeper").is_dir());
    let (status, _, _) = call_api(&router, "GET", "/v1/sessions/doomed/artifacts", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    daemon.shutdown().await;
}

#[tokio::test]
async fn artifact_listing_excludes_working_screenshots_without_deleting_legacy_files() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Full, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let dir = crate::artifacts::session_dir(&daemon.shared, &session);
    std::fs::create_dir_all(dir.join("screenshots")).unwrap();
    std::fs::create_dir_all(dir.join("files")).unwrap();
    let png = png_image(8, 8, false);
    for (path, tool) in [
        ("toolu_old.png", "browser_screenshot"),
        ("old-cli.png", "browser_screenshot"),
        ("toolu_unlabelled.png", "unknown"),
        ("screenshots/shared.png", "browser_screenshot"),
        ("files/toolu_report.png", "write"),
    ] {
        std::fs::write(dir.join(path), &png).unwrap();
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::ArtifactSaved {
                    path: path.into(),
                    size: png.len() as u64,
                    mime: "image/png".into(),
                    tool_call_id: path.into(),
                    tool: tool.into(),
                },
            )
            .await
            .unwrap();
    }
    let list = get_json(&router, "/v1/sessions/session/artifacts").await;
    let artifacts = list["artifacts"].as_array().unwrap();
    assert_eq!(
        artifacts
            .iter()
            .map(|artifact| artifact["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "screenshots/shared.png",
            "files/toolu_report.png",
            "transcript.jsonl"
        ]
    );
    assert!(artifacts
        .iter()
        .all(|artifact| artifact["purpose"] == "output"));
    let (status, _, bytes) = call_api(
        &router,
        "GET",
        &raw_path("session", "toolu_old.png"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, png);
    assert!(dir.join("old-cli.png").is_file());
    daemon.shutdown().await;
}

#[test]
fn large_screenshots_are_downscaled_for_the_model() {
    let small = png_image(16, 16, false);
    assert_eq!(crate::artifacts::model_image(&small), Some(small));
    let large = png_image(1024, 1024, true);
    assert!(large.len() > crate::artifacts::MODEL_IMAGE_MAX_BYTES);
    let scaled = crate::artifacts::model_image(&large).unwrap();
    assert!(scaled.len() <= crate::artifacts::MODEL_IMAGE_MAX_BYTES);
    let decoder = png::Decoder::new(std::io::Cursor::new(&scaled));
    let reader = decoder.read_info().unwrap();
    let info = reader.info();
    assert!(
        info.width < 1024 && info.width == info.height,
        "{}x{}",
        info.width,
        info.height
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_session_roots_and_directories_are_never_followed() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"m1","text":"t","mode":"queue","attachments":[{"name":"a.txt","data_base64":b64(b"inside")}]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    until(|| runtime.0.lock().parts.len() == 1).await;
    let artifacts = daemon.shared.config.data_dir.join("artifacts");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(outside.join("attachments/m1")).unwrap();
    std::fs::write(outside.join("attachments/m1/1-a.txt"), "outside").unwrap();

    // An intermediate directory swapped for a link.
    std::fs::rename(
        artifacts.join("session/attachments"),
        root.path().join("moved"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.join("attachments"),
        artifacts.join("session/attachments"),
    )
    .unwrap();
    let (status, _, _) = call_api(
        &router,
        "GET",
        &raw_path("session", "attachments/m1/1-a.txt"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The whole session root swapped for a link: reads and writes refuse it.
    std::fs::remove_file(artifacts.join("session/attachments")).unwrap();
    std::fs::rename(artifacts.join("session"), root.path().join("moved-root")).unwrap();
    std::os::unix::fs::symlink(&outside, artifacts.join("session")).unwrap();
    let (status, _, _) = call_api(
        &router,
        "GET",
        &raw_path("session", "attachments/m1/1-a.txt"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) =
        call_api(&router, "GET", "/v1/sessions/session/artifacts", json!({})).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "the transcript is not written through the link"
    );
    assert!(!outside.join("transcript.jsonl").exists());
    // Delivery re-reads through the same primitive and checks the recorded digest.
    let attachments =
        daemon.shared.db.state(&session).await.unwrap().attachments[&MessageId::new("m1")].clone();
    let parts = crate::artifacts::message_parts(&daemon.shared, &session, Some(&attachments)).await;
    assert_eq!(
        parts[0].data, None,
        "a substituted file is never sent to the model"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn altered_attachments_are_not_delivered_and_uploads_keep_their_sniffed_type() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"m1","text":"t","mode":"queue","attachments":[{"name":"page.html","data_base64":b64(b"<script>alert(1)</script>")}]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, headers, _) = call_api(
        &router,
        "GET",
        &raw_path("session", "attachments/m1/1-page.html"),
        json!({}),
    )
    .await;
    assert_eq!(
        headers["content-type"], "text/plain",
        "a text upload named .html is not HTML"
    );
    let path = daemon
        .shared
        .config
        .data_dir
        .join("artifacts/session/attachments/m1/1-page.html");
    std::fs::write(&path, "<script>alert(2)</script>").unwrap();
    let attachments =
        daemon.shared.db.state(&session).await.unwrap().attachments[&MessageId::new("m1")].clone();
    let parts = crate::artifacts::message_parts(&daemon.shared, &session, Some(&attachments)).await;
    assert_eq!(parts[0].data, None);
    assert_eq!(
        parts[0].artifact_ref.as_deref(),
        Some("attachments/m1/1-page.html")
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn concurrent_retries_of_one_message_publish_exactly_one_set_of_files() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let post = |content: &'static [u8]| {
        let router = router.clone();
        async move {
            call_api(
                &router,
                "POST",
                "/v1/sessions/session/messages",
                json!({"message_id":"same","text":"t","mode":"queue","attachments":[{"name":"a.txt","data_base64":b64(content)}]}),
            )
            .await
            .0
        }
    };
    let (first, second) = tokio::join!(post(b"first body"), post(b"second body"));
    assert_eq!(
        (first, second),
        (StatusCode::ACCEPTED, StatusCode::ACCEPTED)
    );
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert_eq!(queued(&events), 1);
    let attachment = daemon.shared.db.state(&session).await.unwrap().attachments
        [&MessageId::new("same")][0]
        .clone();
    let stored = std::fs::read(
        daemon
            .shared
            .config
            .data_dir
            .join("artifacts/session/attachments/same/1-a.txt"),
    )
    .unwrap();
    assert_eq!(crate::artifacts::sha256_hex(&stored), attachment.sha256);
    until(|| runtime.0.lock().parts.len() == 1).await;
    assert_eq!(
        runtime.0.lock().parts[0].1[0].data.as_deref(),
        Some(stored.as_slice())
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn nothing_recreates_a_deleted_tasks_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "doomed", PolicyMode::Guard, None).await;
    daemon
        .shared
        .emit(
            &SessionId::new("doomed"),
            None,
            EventKind::AssistantFinal {
                text: "```svg visual\n<svg/>\n```".into(),
            },
        )
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let dir = daemon.shared.config.data_dir.join("artifacts/doomed");
    // Producers racing the deletion: whichever order the gate admits them in, none may
    // leave files behind once the tombstone exists.
    let producers = (0..8).map(|n| {
        let router = router.clone();
        async move {
            let path = [
                "/v1/sessions/doomed/artifacts",
                "/v1/sessions/doomed/visuals",
            ][n % 2];
            call_api(&router, "GET", path, json!({})).await.0
        }
    });
    let upload = call_api(
        &router,
        "POST",
        "/v1/sessions/doomed/messages",
        json!({"text":"t","mode":"queue","attachments":[{"name":"a.txt","data_base64":b64(b"x")}]}),
    );
    let delete = call_api(&router, "DELETE", "/v1/sessions/doomed", json!({}));
    let (_, (deleted, _, _), _) =
        tokio::join!(futures_util::future::join_all(producers), delete, upload);
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    for path in [
        "/v1/sessions/doomed/artifacts",
        "/v1/sessions/doomed/visuals",
    ] {
        assert_eq!(
            call_api(&router, "GET", path, json!({})).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert!(
        !dir.exists(),
        "artifact directory reappeared after deletion"
    );
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn credential_files_are_never_collected() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("work");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = resolved_config(root.path());
    // Protected storage with innocuous names inside the workspace.
    config.loaded.config.vault.path = Some(workspace.join("notes.db"));
    config.data_dir = workspace.join("state");
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([
            &openside_vault::Secret::new("sunflower-4402".into()),
        ]));
    let session = create_session(&daemon, "session", PolicyMode::Full, Some(&workspace)).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    let mut env = crate::tools::ToolEnv::new(daemon.shared.clone(), &state);
    env.tool_call_id = "b1".into();
    // Collector screening is portable even where credential-masked shell execution is refused.
    let tracker = crate::artifacts::Tracker::before(&env, "bash").await;
    std::fs::create_dir(workspace.join(".ssh")).unwrap();
    let files: [(&str, &[u8]); 8] = [
        (".env", b"TOKEN=abc"),
        (".ssh/id_ed25519", b"k"),
        ("server.pem", b"k"),
        ("notes.db", b"vault"),
        ("notes.db.lock", b"lock"),
        ("state/cache.bin", b"private"),
        ("blob.bin", b"\xff\xfe sunflower-4402 \xff"),
        ("notes.txt", b"ok"),
    ];
    for (path, bytes) in files {
        std::fs::write(workspace.join(path), bytes).unwrap();
    }
    tracker.record(&env, "bash").await;
    let saved: Vec<String> = daemon
        .shared
        .db
        .events(&session, 0)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ArtifactSaved { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(saved, ["files/notes.txt"]);
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn failed_cleanup_after_deletion_is_retried_at_startup() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let first = daemon(root.path(), &runtime).await;
    create_session(&first, "gone", PolicyMode::Guard, None).await;
    first
        .shared
        .emit(
            &SessionId::new("gone"),
            None,
            EventKind::AssistantFinal {
                text: "```svg visual\n<svg/>\n```".into(),
            },
        )
        .await
        .unwrap();
    let router = api::router(first.shared.clone(), true);
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/gone/messages",
        json!({"message_id":"m1","text":"t","mode":"queue","attachments":[{"name":"a.txt","data_base64":b64(b"x")}]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        call_api(&router, "GET", "/v1/sessions/gone/visuals", json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let dir = first.shared.config.data_dir.join("artifacts/gone");
    let locked = dir.join("attachments/m1");
    assert!(locked.join("1-a.txt").exists() && dir.join("visuals").exists());
    // Make the deletion's cleanup fail partway.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert_eq!(
        call_api(&router, "DELETE", "/v1/sessions/gone", json!({}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(
        locked.join("1-a.txt").exists(),
        "the injected failure left files behind"
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    drop(router);
    first.shutdown().await;
    drop(first);
    let second = daemon(root.path(), &runtime).await;
    assert!(
        !dir.exists(),
        "startup did not finish the deleted task's cleanup"
    );
    second.shutdown().await;
}

#[tokio::test]
async fn artifacts_and_transcript_include_every_event_page() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let workspace = root.path().join("work");
    std::fs::create_dir(&workspace).unwrap();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "paged", PolicyMode::Full, Some(&workspace)).await;
    for _ in 0..600 {
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::AssistantFinal {
                    text: "earlier".into(),
                },
            )
            .await
            .unwrap();
    }
    let router = api::router(daemon.shared.clone(), true);
    let (status, _, body) = call_api(
        &router,
        "POST",
        "/v1/sessions/paged/messages",
        json!({"message_id":"last-page","text":"attach","mode":"queue","attachments":[
            {"name":"late.txt","data_base64":b64(b"late attachment")}
        ]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    until(|| runtime.0.lock().sent.len() == 1).await;
    let output = host(&daemon, &session)
        .await
        .call(call(
            "write",
            json!({"path":"late.md","content":"late tool output"}),
            "late-tool",
        ))
        .await;
    assert!(matches!(output, ToolOutcome::Done { .. }), "{output:?}");
    let visual = daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::AssistantFinal {
                text: "```svg visual\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n```".into(),
            },
        )
        .await
        .unwrap();
    let last_seq = daemon.shared.db.state(&session).await.unwrap().last_seq;
    assert!(last_seq > 512);
    let visual_path = format!("visuals/{}-0.svg", visual.seq);
    let listed = get_json(&router, "/v1/sessions/paged/artifacts").await;
    let entries = listed["artifacts"].as_array().unwrap();
    assert_eq!(entries.len(), 4);
    for (path, source) in [
        ("attachments/last-page/1-late.txt", "attachment"),
        ("files/late.md", "tool"),
        (visual_path.as_str(), "visual"),
        ("transcript.jsonl", "transcript"),
    ] {
        assert!(
            entries
                .iter()
                .any(|entry| entry["path"] == path && entry["source"] == source),
            "{listed}"
        );
    }
    let visuals = get_json(&router, "/v1/sessions/paged/visuals").await;
    assert_eq!(visuals["visuals"].as_array().unwrap().len(), 1);
    assert_eq!(visuals["visuals"][0]["path"], visual_path);
    assert_eq!(
        visuals["visuals"][0]["source"],
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
    );
    let (status, _, body) = call_api(
        &router,
        "GET",
        &raw_path("paged", "transcript.jsonl"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let transcript = String::from_utf8(body).unwrap();
    let events: Vec<Event> = transcript
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.last().unwrap().seq, last_seq);
    assert_eq!(events.len() as u64, last_seq);
    assert!(events
        .iter()
        .enumerate()
        .all(|(index, event)| event.seq == index as u64 + 1));
    daemon.shutdown().await;
}

#[tokio::test]
async fn artifacts_message_text_cap_rejects_before_storage_and_queue_mutation() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let cap = openside_agent_sidecar::MAX_USER_TEXT_BYTES;
    let under = format!("{}x", "é".repeat(cap / 2 - 1));
    assert_eq!(under.len(), cap - 1);
    let over = format!("{under}xx");
    let (status, _, body) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"too-large","text":over,"mode":"steer","attachments":[
            {"name":"not-stored.txt","data_base64":b64(b"never published")}
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["code"],
        "message_too_large"
    );
    assert_eq!(
        queued(&daemon.shared.db.events(&session, 0).await.unwrap()),
        0
    );
    assert!(!daemon
        .shared
        .config
        .data_dir
        .join("artifacts/session/attachments")
        .exists());
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"under","text":under,"mode":"steer"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    until(|| runtime.0.lock().sent.len() == 1).await;
    let (status, _, _) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id":"editable","text":"original","mode":"queue"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _, body) = call_api(
        &router,
        "PATCH",
        "/v1/sessions/session/queue/editable",
        json!({"text":over}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["code"],
        "message_too_large"
    );
    assert_eq!(
        daemon
            .shared
            .message(&session, &MessageId::new("editable"))
            .await
            .unwrap()
            .text,
        "original"
    );
    let (status, _, _) = call_api(
        &router,
        "PATCH",
        "/v1/sessions/session/queue/editable",
        json!({"text":under}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        daemon
            .shared
            .message(&session, &MessageId::new("editable"))
            .await
            .unwrap()
            .text,
        under
    );
    let (status, _, body) = call_api(
        &router,
        "POST",
        "/v1/sessions/session/subagents/child/steer",
        json!({"text":over}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["code"],
        "message_too_large"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn artifacts_maximum_text_and_attachment_are_delivered_without_lowering_upload_cap() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "maximum", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let text = "x".repeat(openside_agent_sidecar::MAX_USER_TEXT_BYTES);
    let attachment = vec![b'a'; MAX_ATTACHMENT_BYTES];
    let (status, _, body) = call_api(
        &router,
        "POST",
        "/v1/sessions/maximum/messages",
        json!({"message_id":"maximum","text":text,"mode":"queue","attachments":[
            {"name":"maximum.txt","data_base64":b64(&attachment)}
        ]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    until(|| runtime.0.lock().parts.len() == 1).await;
    {
        let state = runtime.0.lock();
        assert_eq!(state.sent[0].2, text);
        assert_eq!(
            state.parts[0].1[0].data.as_deref(),
            Some(attachment.as_slice())
        );
    }
    daemon.shutdown().await;
}
