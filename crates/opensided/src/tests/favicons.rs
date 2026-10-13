use super::*;
use base64::Engine as _;

fn image() -> (Vec<u8>, String) {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 16, 16);
        encoder.set_color(png::ColorType::Rgba);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![0; 16 * 16 * 4]).unwrap();
    }
    let data = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    (bytes, data)
}
fn card(origin: &str) -> Card {
    serde_json::from_value(json!({"id":"present:icon","kind":"table","site":{"origin":origin,"name":"Example","favicon_ref":{"type":"origin","origin":origin}},"title":"Result","subtitle":null,"status":"done","blocks":[],"actions":[],"interaction":"none","collapsed_summary":"Result"})).unwrap()
}

#[tokio::test]
async fn favicon_get_requires_read_and_a_persisted_reference_and_never_fetches_a_miss() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "favicon", PolicyMode::Full, None).await;
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, human) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    let uri = "/v1/favicons/https%3A%2F%2Fexample.test";
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", uri, json!({}), None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", uri, json!({}), Some(&human)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let missing = router
        .clone()
        .oneshot(request("GET", uri, json!({}), Some(&read)))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let body: Value =
        serde_json::from_slice(&missing.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["code"], "favicon_unavailable");
    assert!(daemon.shared.registry.browser.existing().await.is_err());
    let (bytes, data) = image();
    daemon
        .shared
        .favicons
        .seed(session.clone(), "https://example.test", &data);
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", uri, json!({}), Some(&read)))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::CardPresented {
                card: card("https://example.test"),
                tool_call_id: "icon".into(),
            },
        )
        .await
        .unwrap();
    let response = router
        .clone()
        .oneshot(request("GET", uri, json!({}), Some(&read)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["cache-control"], "private,max-age=300");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        bytes.as_slice()
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn favicon_origin_route_refuses_incognito_and_isolated_partitions_even_with_cached_pixels() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, data) = image();
    for (id, incognito, browser_space) in [
        ("private", true, BrowserSpace::Shared),
        ("isolated", false, BrowserSpace::Isolated),
    ] {
        let session = SessionId::new(id);
        let stored = session.clone();
        daemon
            .shared
            .db
            .store(move |store| {
                store.append(
                    &stored,
                    None,
                    EventKind::SessionCreated {
                        agent_id: None,
                        title: "Private".into(),
                        model: "script:test".into(),
                        mode: PolicyMode::Full,
                        workspace: None,
                        incognito,
                        browser_space,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::CardPresented {
                    card: card("https://private.example.test"),
                    tool_call_id: "icon".into(),
                },
            )
            .await
            .unwrap();
        daemon
            .shared
            .favicons
            .seed(session, "https://private.example.test", &data);
    }
    let response = api::router(daemon.shared.clone(), false)
        .oneshot(request(
            "GET",
            "/v1/favicons/https%3A%2F%2Fprivate.example.test",
            json!({}),
            Some(&read),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    daemon.shutdown().await;
}
