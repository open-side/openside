use super::*;

#[tokio::test]
async fn ordinary_conversation_delete_route_removes_only_the_selected_thread() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let doomed = create_session(&d, "doomed", PolicyMode::Guard, None).await;
    let keeper = create_session(&d, "keeper", PolicyMode::Guard, None).await;
    let router = api::router(d.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request("DELETE", "/v1/sessions/doomed", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(d.shared.db.state(&doomed).await.is_err());
    assert!(d.shared.db.state(&keeper).await.is_ok());
    let response = router
        .clone()
        .oneshot(request("GET", "/v1/sessions", json!({}), None))
        .await
        .unwrap();
    let list = json_body(response).await;
    assert!(!list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "doomed"));
    assert!(list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "keeper"));
    let response = router
        .oneshot(request("GET", "/v1/sessions/doomed", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    d.shutdown().await;
}

#[tokio::test]
async fn deletion_route_refuses_chief_and_teammate_home_conversations() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let draft: HireDraft = serde_json::from_value(json!({
        "name":"Mina",
        "persona":{"role":"Watches prices","voice":"Brief","responsibilities":["Track prices"],"boundaries":["Ask before purchases"]}
    })).unwrap();
    let teammate = crate::agents::create(&d.shared, &draft, "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let router = api::router(d.shared.clone(), true);
    for home in [&chief.home_session_id, &teammate.home_session_id] {
        let response = router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("/v1/sessions/{home}"),
                json!({}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(response).await["code"], "agent_home_immutable");
        assert!(d.shared.db.state(home).await.is_ok());
    }
    d.shutdown().await;
}

#[tokio::test]
async fn session_model_is_creation_metadata_and_usage_never_rewrites_its_title() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&d, "model-history", PolicyMode::Guard, None).await;
    let created = d.shared.db.state(&session).await.unwrap();
    d.shared
        .emit(
            &session,
            None,
            EventKind::Usage {
                model: "different:effective-model".into(),
                input_tokens: 1,
                output_tokens: 1,
            },
        )
        .await
        .unwrap();
    let router = api::router(d.shared.clone(), true);
    let response = router
        .oneshot(request(
            "GET",
            "/v1/sessions/model-history",
            json!({}),
            None,
        ))
        .await
        .unwrap();
    let current = json_body(response).await;
    assert_eq!(current["model"], created.model);
    assert_eq!(current["title"], created.title);
    let schema = schemars::schema_for!(openside_proto::Session).to_value();
    let description = schema["properties"]["model"]["description"]
        .as_str()
        .unwrap();
    assert!(description.contains("when the session was created"));
    assert!(description.contains("not its effective current model"));
    d.shutdown().await;
}

#[test]
fn app_help_is_front_only_and_connection_result_fields_remain_unambiguous() {
    let help = include_str!("../../../../docs/specs/app-help.md");
    assert!(!crate::prompt::parts().iter().any(|part| part == help));
    use crate::tools::DaemonTool;
    let list =
        crate::tools::connections::ConnectionTool(crate::tools::connections::Kind::List).spec();
    assert!(list
        .description
        .contains("status=candidate: discovered sign-in, not verified"));
    assert!(list
        .description
        .contains("access: agent permission, separate from status"));
}
