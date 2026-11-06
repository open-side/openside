use super::*;
use crate::folder_picker::{set_picker, PICKING};

// The picker is process-wide: these tests take turns so each sees its own.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static PICKED: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn recorded() -> std::io::Result<Option<std::path::PathBuf>> {
    Ok(PICKED.lock().clone())
}

async fn send(router: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request("POST", "/v1/folders/pick", Value::Null, token))
        .await
        .unwrap();
    let status = response.status();
    (status, json_body(response).await)
}

#[tokio::test]
async fn local_pick_returns_the_canonical_folder_or_null_when_cancelled() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let chosen = tempfile::tempdir().unwrap();
    std::fs::create_dir(chosen.path().join("sub")).unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let local = api::router(daemon.shared.clone(), true);
    set_picker(recorded);

    *PICKED.lock() = Some(chosen.path().join("sub/.."));
    let (status, body) = send(&local, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let canonical = std::fs::canonicalize(chosen.path()).unwrap();
    assert_eq!(body, json!({"path": canonical.to_str().unwrap()}));

    *PICKED.lock() = None;
    let (status, body) = send(&local, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"path": null}));
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_picked_folder_that_cannot_be_a_project_root_is_refused() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let local = api::router(daemon.shared.clone(), true);
    set_picker(recorded);
    for picked in [
        std::path::PathBuf::from("/"),
        daemon.shared.config.data_dir.clone(),
    ] {
        *PICKED.lock() = Some(picked.clone());
        let (status, body) = send(&local, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{picked:?}");
        assert_eq!(body["code"], "invalid_project_root", "{picked:?}");
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn no_picker_is_unavailable_and_a_second_picker_is_busy() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let local = api::router(daemon.shared.clone(), true);

    set_picker(|| Err(std::io::ErrorKind::NotFound.into()));
    let (status, body) = send(&local, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "folder_picker_unavailable");

    set_picker(recorded);
    let open = PICKING.lock().await;
    let (status, body) = send(&local, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "folder_picker_busy");
    drop(open);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_remote_device_cannot_open_the_picker() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let remote = api::router(daemon.shared.clone(), false);
    set_picker(recorded);
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (status, body) = send(&remote, Some(&admin)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "local_access_required");
    daemon.shutdown().await;
}
