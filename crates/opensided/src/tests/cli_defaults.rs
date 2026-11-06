//! `GET`/`PATCH /v1/settings/cli` over the CLI's `cli.toml`, and local authority for
//! `PATCH /v1/settings/preferences`.

use axum::{extract::ConnectInfo, Router};
use openside_proto::DeviceKind;
use std::net::SocketAddr;

use super::*;

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The computer's own launcher on loopback: a socket-issued person token plus the boot secret.
fn launcher(method: &str, path: &str, body: Value, token: &str, secret: &str) -> Request<Body> {
    let mut request = request(method, path, body, Some(token));
    request
        .headers_mut()
        .insert("host", "127.0.0.1:47821".parse().unwrap());
    request
        .headers_mut()
        .insert(crate::remote::LOCAL_SECRET_HEADER, secret.parse().unwrap());
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50001".parse::<SocketAddr>().unwrap(),
    ));
    request
}

#[tokio::test]
async fn cli_defaults_read_and_write_only_the_profile_key_of_cli_toml() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let unix = api::router(daemon.shared.clone(), true);
    let file = root.path().join("cli.toml");
    let get = || send(&unix, request("GET", "/v1/settings/cli", Value::Null, None));
    let patch = |body: Value| send(&unix, request("PATCH", "/v1/settings/cli", body, None));

    // No file: the account's first profile, and nothing is created by reading.
    assert_eq!(
        get().await,
        (
            StatusCode::OK,
            json!({"profile": "shared", "profile_is_default": true})
        )
    );
    assert!(!file.exists());

    // A missing file is created with just the profile.
    assert_eq!(
        patch(json!({"profile": "incognito"})).await,
        (
            StatusCode::OK,
            json!({"profile": "incognito", "profile_is_default": false})
        )
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "profile = \"incognito\"\n"
    );
    assert_eq!(
        get().await,
        (
            StatusCode::OK,
            json!({"profile": "incognito", "profile_is_default": false})
        )
    );
    // Saving `shared` explicitly is not the same as having none saved.
    assert_eq!(
        patch(json!({"profile": "shared"})).await,
        (
            StatusCode::OK,
            json!({"profile": "shared", "profile_is_default": false})
        )
    );

    // The CLI's other keys and comments stay; `tests::agent::cli_settings_the_daemon_wrote_still_load`
    // in openside-cli parses this exact text.
    std::fs::write(
        &file,
        "# Written by hand.\nhost = \"work\"\naccount = \"chief\"\n",
    )
    .unwrap();
    assert_eq!(
        patch(json!({"profile": "isolated"})).await,
        (
            StatusCode::OK,
            json!({"profile": "isolated", "profile_is_default": false})
        )
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# Written by hand.\nhost = \"work\"\naccount = \"chief\"\nprofile = \"isolated\"\n"
    );
    // null removes the key and nothing else.
    assert_eq!(
        patch(json!({"profile": null})).await,
        (
            StatusCode::OK,
            json!({"profile": "shared", "profile_is_default": true})
        )
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# Written by hand.\nhost = \"work\"\naccount = \"chief\"\n"
    );

    // The field is required (an omitted one is not a clear), unknown fields and profiles are
    // refused, and none of them touch the file.
    for body in [
        json!({}),
        json!({"profile": "shared", "host": "work"}),
        json!({"profile": "private"}),
    ] {
        let (status, _) = patch(body.clone()).await;
        assert!(status.is_client_error(), "{body}: {status}");
    }
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# Written by hand.\nhost = \"work\"\naccount = \"chief\"\n"
    );

    // Replacing the value keeps the comment above the key and the one after the value.
    std::fs::write(
        &file,
        "# Chosen by hand.\nprofile = \"isolated\"  # mine\nhost = \"work\"\n",
    )
    .unwrap();
    assert_eq!(
        patch(json!({"profile": "incognito"})).await,
        (
            StatusCode::OK,
            json!({"profile": "incognito", "profile_is_default": false})
        )
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# Chosen by hand.\nprofile = \"incognito\"  # mine\nhost = \"work\"\n"
    );
}

#[tokio::test]
async fn a_malformed_cli_toml_is_reported_and_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let unix = api::router(daemon.shared.clone(), true);
    let file = root.path().join("cli.toml");
    for text in [
        "profile = \"isolated\"\nbroken",
        "profile = \"private\"\n",
        "profile = 3\n",
    ] {
        std::fs::write(&file, text).unwrap();
        for request in [
            request("GET", "/v1/settings/cli", Value::Null, None),
            request(
                "PATCH",
                "/v1/settings/cli",
                json!({"profile": "shared"}),
                None,
            ),
            request("PATCH", "/v1/settings/cli", json!({"profile": null}), None),
        ] {
            let (status, body) = send(&unix, request).await;
            assert_eq!(
                (status, body["code"].as_str()),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Some("cli_settings_invalid")
                ),
                "{text}"
            );
            // Sanitized: the file's content is not echoed back.
            assert!(!body.to_string().contains("broken") && !body.to_string().contains("private"));
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    }
}

#[tokio::test]
async fn only_this_computer_changes_the_cli_profile_and_the_local_only_preferences() {
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
    let (_, phone) = daemon
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
    let refused = (
        StatusCode::FORBIDDEN,
        Some("local_access_required".to_owned()),
    );
    let code =
        |(status, body): (StatusCode, Value)| (status, body["code"].as_str().map(str::to_owned));

    // A paired phone reads the default but cannot change it, even with the local secret.
    let (status, _) = send(
        &tcp,
        request("GET", "/v1/settings/cli", Value::Null, Some(&phone)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for request in [
        request(
            "PATCH",
            "/v1/settings/cli",
            json!({"profile": "isolated"}),
            Some(&phone),
        ),
        launcher(
            "PATCH",
            "/v1/settings/cli",
            json!({"profile": "isolated"}),
            &phone,
            secret,
        ),
        launcher(
            "PATCH",
            "/v1/settings/cli",
            json!({"profile": "isolated"}),
            &agent,
            secret,
        ),
        // The launcher's token without the boot secret is not local either.
        request(
            "PATCH",
            "/v1/settings/cli",
            json!({"profile": "isolated"}),
            Some(local),
        ),
    ] {
        assert_eq!(code(send(&tcp, request).await), refused);
    }
    assert!(!root.path().join("cli.toml").exists());
    let (status, body) = send(
        &tcp,
        launcher(
            "PATCH",
            "/v1/settings/cli",
            json!({"profile": "isolated"}),
            local,
            secret,
        ),
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::OK,
            json!({"profile": "isolated", "profile_is_default": false})
        )
    );

    // PATCH /v1/settings/preferences: the launcher may flip mcp_server and computer_use; a
    // phone still may not, but keeps changing everything else.
    let (status, prefs) = send(
        &tcp,
        launcher(
            "PATCH",
            "/v1/settings/preferences",
            json!({"mcp_server": false, "computer_use": true}),
            local,
            secret,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(
        (
            prefs["mcp_server"].as_bool(),
            prefs["computer_use"].as_bool()
        ),
        (Some(false), Some(true))
    );
    for body in [json!({"mcp_server": true}), json!({"computer_use": false})] {
        assert_eq!(
            code(
                send(
                    &tcp,
                    request(
                        "PATCH",
                        "/v1/settings/preferences",
                        body.clone(),
                        Some(&phone)
                    )
                )
                .await
            ),
            refused
        );
        assert_eq!(
            code(
                send(
                    &tcp,
                    launcher("PATCH", "/v1/settings/preferences", body, &phone, secret)
                )
                .await
            ),
            refused
        );
    }
    let (status, prefs) = send(
        &tcp,
        request(
            "PATCH",
            "/v1/settings/preferences",
            json!({"confirmation": "ask_first"}),
            Some(&phone),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(
        (
            prefs["mcp_server"].as_bool(),
            prefs["computer_use"].as_bool()
        ),
        (Some(false), Some(true))
    );
}
