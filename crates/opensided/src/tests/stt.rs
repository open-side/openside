//! Person dictation (`GET`/`POST /v1/stt`): the local command, the OpenAI key choice, every
//! refusal, and that the clip never outlives the request.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode},
    Router,
};
use base64::Engine as _;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::stt::{transcribe_openai, Failure};

/// The transcription gate is process-wide, so tests that post clips take turns; otherwise the
/// busy test would see other tests' clips and they would see its refusals.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// No provider reads the test process environment, so a developer's own keys never count.
fn isolated_credentials() -> Value {
    json!({"order":{},"env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}})
}

struct Setup {
    stt: Option<String>,
    max_audio_bytes: Option<u64>,
    timeout_secs: Option<u32>,
    credentials: Value,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            stt: None,
            max_audio_bytes: None,
            timeout_secs: None,
            credentials: isolated_credentials(),
        }
    }
}

async fn open(root: &Path, setup: Setup) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.credentials = setup.credentials;
    if let Some(body) = setup.stt {
        let script = root.join("stt.sh");
        std::fs::write(&script, body).unwrap();
        // Run through `sh` so the freshly written file is never exec'd while still open.
        config.loaded.config.channels.stt_cmd =
            Some(vec!["sh".into(), script.display().to_string()]);
    }
    if let Some(bytes) = setup.max_audio_bytes {
        config.loaded.config.channels.max_audio_bytes = bytes;
    }
    if let Some(secs) = setup.timeout_secs {
        config.loaded.config.channels.stt_timeout_secs = secs;
    }
    Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap()
}

async fn call(
    router: &Router,
    method: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, "/v1/stt", body, token))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn clip(audio: &[u8], mime: &str) -> Value {
    json!({
        "audio_base64": base64::engine::general_purpose::STANDARD.encode(audio),
        "mime": mime,
    })
}

fn unix(daemon: &Daemon) -> Router {
    api::router(daemon.shared.clone(), true)
}

#[tokio::test]
async fn without_a_transcriber_status_is_unavailable_and_posting_is_a_conflict() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    // A ChatGPT-plan provider with a working credential command still does not qualify.
    let mut credentials = isolated_credentials();
    credentials["order"]["openai-codex"] = json!(["command"]);
    credentials["command"] = json!({"openai-codex": ["sh", "-c", "printf sk-codex"]});
    let daemon = open(
        root.path(),
        Setup {
            credentials,
            ..Setup::default()
        },
    )
    .await;
    let local = unix(&daemon);
    let (status, body) = call(&local, "GET", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"available": false, "reason": "stt_unavailable", "max_bytes": 8 * 1024 * 1024, "max_seconds": 120})
    );
    assert!(
        crate::model_accounts::openai_transcription_key(&daemon.shared)
            .await
            .is_none()
    );
    let (status, body) = call(&local, "POST", clip(b"hello", "audio/webm"), None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("stt_unavailable"))
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn an_openai_api_key_in_the_openai_pool_makes_dictation_available() {
    let root = tempfile::tempdir().unwrap();
    let mut credentials = isolated_credentials();
    credentials["order"]["openai"] = json!(["command"]);
    credentials["command"] = json!({"openai": ["sh", "-c", "printf sk-test-stt"]});
    let daemon = open(
        root.path(),
        Setup {
            credentials,
            ..Setup::default()
        },
    )
    .await;
    let (status, body) = call(&unix(&daemon), "GET", Value::Null, None).await;
    assert_eq!(
        (status, body["available"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    let key = crate::model_accounts::openai_transcription_key(&daemon.shared)
        .await
        .unwrap();
    assert_eq!(key.as_str(), "sk-test-stt");
    daemon.shutdown().await;
}

/// Points the OpenAI path at a fake for one test; cleared even when the test fails.
struct FakeOpenAi;

impl FakeOpenAi {
    fn at(base: &str) -> Self {
        *crate::stt::TEST_OPENAI_BASE.lock() = Some(base.to_owned());
        Self
    }
}

impl Drop for FakeOpenAi {
    fn drop(&mut self) {
        *crate::stt::TEST_OPENAI_BASE.lock() = None;
    }
}

#[tokio::test]
async fn dictation_skips_an_unready_account_and_uses_the_next_first_party_key() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    // The pool's first account reads a variable nobody sets, so it is never ready.
    let mut credentials = isolated_credentials();
    credentials["env"]["openai"] = json!(["OPENSIDE_STT_TEST_UNSET_KEY"]);
    credentials["order"]["openai"] = json!(["env", "command"]);
    credentials["command"] = json!({"openai": ["sh", "-c", "printf sk-second"]});
    let daemon = open(
        root.path(),
        Setup {
            credentials,
            ..Setup::default()
        },
    )
    .await;
    let (base, seen, server) = fake_openai((StatusCode::OK, r#"{"text":"from openai"}"#)).await;
    let _fake = FakeOpenAi::at(&base);
    let local = unix(&daemon);
    let (status, body) = call(&local, "GET", Value::Null, None).await;
    assert_eq!(
        (status, body["available"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    let (status, body) = call(&local, "POST", clip(b"hello", "audio/webm"), None).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, json!({"text": "from openai"}))
    );
    let (headers, _) = seen.lock().take().unwrap();
    assert_eq!(headers["authorization"], "Bearer sk-second");
    server.abort();
    daemon.shutdown().await;
}

#[test]
fn a_relay_or_non_api_key_credential_is_never_a_transcription_key() {
    let parse =
        |value: Value| -> openside_core::CredentialAuth { serde_json::from_value(value).unwrap() };
    assert!(crate::model_accounts::first_party(&parse(
        json!({"kind": "api_key", "value": "sk-direct"})
    )));
    let relay =
        parse(json!({"kind": "api_key", "value": "sk-relay", "base_url": "http://127.0.0.1:9/v1"}));
    assert!(!crate::model_accounts::first_party(&relay));
    let bearer = parse(json!({"kind": "bearer", "value": "token"}));
    assert!(!crate::model_accounts::first_party(&bearer));
    let empty = parse(json!({"kind": "api_key", "value": ""}));
    assert!(!crate::model_accounts::first_party(&empty));
}

#[tokio::test]
async fn no_error_reply_ever_carries_the_api_key() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let mut credentials = isolated_credentials();
    credentials["order"]["openai"] = json!(["command"]);
    credentials["command"] = json!({"openai": ["sh", "-c", "printf sk-leak-canary"]});
    let daemon = open(
        root.path(),
        Setup {
            credentials,
            ..Setup::default()
        },
    )
    .await;
    let local = unix(&daemon);
    for reply in [
        (
            StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"Incorrect API key provided: sk-leak-canary"}}"#,
        ),
        (StatusCode::INTERNAL_SERVER_ERROR, "sk-leak-canary"),
        (StatusCode::OK, r#"{"unexpected":"sk-leak-canary"}"#),
        (StatusCode::OK, "not json sk-leak-canary"),
    ] {
        let (base, _, server) = fake_openai(reply).await;
        let _fake = FakeOpenAi::at(&base);
        let response = local
            .clone()
            .oneshot(request(
                "POST",
                "/v1/stt",
                clip(b"hello", "audio/webm"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{reply:?}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("sk-leak-canary"), "{text}");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap()["code"],
            "stt_failed"
        );
        server.abort();
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn the_command_transcribes_a_private_copy_that_is_gone_afterwards() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let seen = root.path().join("seen");
    let mode = root.path().join("mode");
    let script = format!(
        "printf '%s' \"$1\" > '{}'\n(stat -c %a \"$1\" 2>/dev/null || stat -f %Lp \"$1\") > '{}'\ncat \"$1\"\n",
        seen.display(),
        mode.display()
    );
    let daemon = open(
        root.path(),
        Setup {
            stt: Some(script),
            max_audio_bytes: Some(1024),
            ..Setup::default()
        },
    )
    .await;
    let local = unix(&daemon);
    let (status, body) = call(&local, "GET", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"available": true, "reason": null, "max_bytes": 1024, "max_seconds": 120})
    );

    let (status, body) = call(
        &local,
        "POST",
        clip(b"  hello from the mic \n", "audio/webm;codecs=opus"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"text": "hello from the mic"}));
    let clip_path = PathBuf::from(std::fs::read_to_string(&seen).unwrap());
    let stt_root = daemon.shared.config.loaded.paths.runtime_dir.join("stt");
    assert!(clip_path.starts_with(&stt_root), "{}", clip_path.display());
    assert_eq!(clip_path.extension().unwrap(), "webm");
    assert_eq!(std::fs::read_to_string(&mode).unwrap().trim(), "600");
    assert!(!clip_path.exists());
    assert!(!clip_path.parent().unwrap().exists());
    assert_eq!(std::fs::read_dir(&stt_root).unwrap().count(), 0);

    // Refused before decoding: the encoded length alone implies more than max_bytes.
    let (status, body) = call(&local, "POST", clip(&[7; 2048], "audio/ogg"), None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("audio_too_large"))
    );
    // Refused after decoding: same encoded length as 1024 bytes, one byte more.
    let (status, body) = call(&local, "POST", clip(&[7; 1025], "audio/ogg"), None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("audio_too_large"))
    );

    for request in [
        json!({"audio_base64": "!!not base64!!", "mime": "audio/webm"}),
        json!({"audio_base64": "", "mime": "audio/webm"}),
        clip(b"hello", "video/webm"),
        clip(b"hello", "text/plain"),
        clip(b"hello", "audio/"),
        clip(b"hello", "audio/webm\r\nX-Injected: 1"),
        json!({"audio_base64": "aGVsbG8=", "mime": "audio/webm", "language": "en_US"}),
        json!({"audio_base64": "aGVsbG8=", "mime": "audio/webm", "language": "-ko"}),
        json!({"audio_base64": "aGVsbG8=", "mime": "audio/webm", "language": "a".repeat(17)}),
    ] {
        let (status, body) = call(&local, "POST", request.clone(), None).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_audio")),
            "{request}"
        );
    }
    let (status, body) = call(
        &local,
        "POST",
        json!({"audio_base64": "aGVsbG8=", "mime": "audio/mp4", "language": "en-US"}),
        None,
    )
    .await;
    assert_eq!((status, body), (StatusCode::OK, json!({"text": "hello"})));
    assert_eq!(std::fs::read_dir(&stt_root).unwrap().count(), 0);
    daemon.shutdown().await;
}

#[tokio::test]
async fn silence_failure_and_timeout_map_to_their_codes_and_leave_no_clip() {
    let _serial = SERIAL.lock().await;
    for (script, timeout, expected) in [
        (
            "printf '  \\n'\n",
            None,
            (StatusCode::UNPROCESSABLE_ENTITY, "no_speech"),
        ),
        (
            "echo leaked-stderr >&2\necho partial-output\nexit 3\n",
            None,
            (StatusCode::BAD_GATEWAY, "stt_failed"),
        ),
        (
            "exec sleep 5\n",
            Some(1),
            (StatusCode::BAD_GATEWAY, "stt_failed"),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let daemon = open(
            root.path(),
            Setup {
                stt: Some(script.into()),
                timeout_secs: timeout,
                ..Setup::default()
            },
        )
        .await;
        let (status, body) = call(&unix(&daemon), "POST", clip(b"hello", "audio/wav"), None).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (expected.0, Some(expected.1)),
            "{script}"
        );
        assert!(!body.to_string().contains("partial-output"));
        assert!(!body.to_string().contains("leaked-stderr"));
        let stt_root = daemon.shared.config.loaded.paths.runtime_dir.join("stt");
        assert_eq!(std::fs::read_dir(&stt_root).unwrap().count(), 0, "{script}");
        daemon.shutdown().await;
    }
}

#[tokio::test]
async fn only_the_person_dictates_and_an_oversized_body_is_audio_too_large() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = open(
        root.path(),
        Setup {
            stt: Some("cat \"$1\"\n".into()),
            ..Setup::default()
        },
    )
    .await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, person) = daemon
        .shared
        .db
        .pair_named(
            "phone".into(),
            vec![CapabilityScope::Read, CapabilityScope::Steer],
        )
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "bot".into(),
            vec![CapabilityScope::Read, CapabilityScope::Steer],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    // GET answers every caller: the person sees the transcriber, an agent only its refusal.
    let expected = |reason: Value| json!({"available": reason.is_null(), "reason": reason, "max_bytes": 8 * 1024 * 1024, "max_seconds": 120});
    let (status, body) = call(&tcp, "GET", Value::Null, Some(&person)).await;
    assert_eq!((status, body), (StatusCode::OK, expected(Value::Null)));
    let (status, body) = call(&tcp, "GET", Value::Null, Some(&agent)).await;
    assert_eq!(
        (status, body),
        (StatusCode::OK, expected(json!("person_device_required")))
    );
    let (status, body) = call(&tcp, "POST", clip(b"hi", "audio/webm"), Some(&agent)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("person_device_required"))
    );
    let (status, body) = call(&tcp, "POST", clip(b"hi", "audio/webm"), Some(&person)).await;
    assert_eq!((status, body), (StatusCode::OK, json!({"text": "hi"})));
    // A marked MCP client on the local socket is an agent too.
    let mcp = |method: &str, body: Value| {
        Request::builder()
            .method(method)
            .uri("/v1/stt")
            .header("content-type", "application/json")
            .header("x-openside-client", "mcp")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let response = unix(&daemon)
        .oneshot(mcp("GET", Value::Null))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        expected(json!("person_device_required"))
    );
    let response = unix(&daemon)
        .oneshot(mcp("POST", clip(b"hi", "audio/webm")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Base64 of a full 8 MiB clip fits the route's body cap; a far larger body is cut off by
    // the cap itself and still answers with this route's code.
    assert_eq!(
        crate::stt::body_limit(&daemon.shared),
        (8 * 1024 * 1024usize).div_ceil(3) * 4 + 64 * 1024
    );
    let big = json!({"audio_base64": "A".repeat(crate::stt::body_limit(&daemon.shared) + 1), "mime": "audio/webm"});
    let (status, body) = call(&tcp, "POST", big, Some(&person)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, Some("audio_too_large"))
    );
    daemon.shutdown().await;
}

async fn raw(router: &Router, token: Option<&str>, body: Vec<u8>) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/stt")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (status, json_body(response).await)
}

#[tokio::test]
async fn a_smaller_max_audio_bytes_caps_the_body_before_it_is_read() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = open(
        root.path(),
        Setup {
            stt: Some("cat \"$1\"\n".into()),
            max_audio_bytes: Some(1024),
            ..Setup::default()
        },
    )
    .await;
    let limit = crate::stt::body_limit(&daemon.shared);
    assert_eq!(limit, 1024usize.div_ceil(3) * 4 + 64 * 1024);
    let (_, person) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Steer])
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    // Unterminated JSON past the cap: read in full it would be a 400, so a 413 shows the cap
    // stopped it, on the local socket (handler) and on TCP (auth middleware) alike.
    let mut unterminated = br#"{"mime":"audio/webm","audio_base64":""#.to_vec();
    unterminated.resize(limit + 1, b'A');
    for (router, token) in [(unix(&daemon), None), (tcp, Some(person.as_str()))] {
        let (status, body) = raw(&router, token, unterminated.clone()).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::PAYLOAD_TOO_LARGE, Some("audio_too_large"))
        );
        // Within the cap the same malformed body is read and refused as invalid.
        let (status, body) = raw(&router, token, unterminated[..limit].to_vec()).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_audio"))
        );
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn daemon_start_removes_clips_a_crash_left_behind() {
    let root = tempfile::tempdir().unwrap();
    let stt_root = root.path().join("run").join("stt");
    std::fs::create_dir_all(stt_root.join("clip-crashed")).unwrap();
    std::fs::write(stt_root.join("clip-crashed").join("clip.webm"), b"secret").unwrap();
    std::fs::write(stt_root.join("stray"), b"secret").unwrap();
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, stt_root.join("link")).unwrap();
    let daemon = open(root.path(), Setup::default()).await;
    assert_eq!(
        daemon.shared.config.loaded.paths.runtime_dir.join("stt"),
        stt_root
    );
    assert_eq!(std::fs::read_dir(&stt_root).unwrap().count(), 0);
    // The sweep removes a planted symlink itself, never what it points at.
    assert!(outside.join("keep").exists());
    daemon.shutdown().await;
}

#[tokio::test]
async fn beyond_two_running_and_four_waiting_a_clip_is_refused_before_its_body_is_read() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let go = root.path().join("go");
    let script = format!(
        "while [ ! -e '{}' ]; do sleep 0.02; done\ncat \"$1\"\n",
        go.display()
    );
    let daemon = open(
        root.path(),
        Setup {
            stt: Some(script),
            timeout_secs: Some(30),
            ..Setup::default()
        },
    )
    .await;
    let local = unix(&daemon);
    let queued: Vec<_> = (0..6)
        .map(|index| {
            let local = local.clone();
            tokio::spawn(async move {
                let body = clip(format!("clip {index}").as_bytes(), "audio/webm");
                call(&local, "POST", body, None).await
            })
        })
        .collect();
    until(|| {
        crate::stt::ADMITTED.available_permits() == 0
            && crate::stt::RUNNING.available_permits() == 0
    })
    .await;
    let (_, person) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Steer])
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    // A body that never ends would hang any reader, so an answer at all proves the refusal
    // comes before the body is read: in the auth middleware on TCP, in the handler locally.
    for (router, token) in [(local.clone(), None), (tcp, Some(person.as_str()))] {
        let request = stalled(token);
        let response = tokio::time::timeout(Duration::from_secs(5), router.oneshot(request))
            .await
            .expect("refused without reading the body")
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(json_body(response).await["code"], "stt_busy");
    }
    std::fs::write(&go, b"").unwrap();
    let mut texts = Vec::new();
    for task in queued {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        texts.push(body["text"].as_str().unwrap().to_owned());
    }
    texts.sort();
    assert_eq!(
        texts,
        (0..6)
            .map(|index| format!("clip {index}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(crate::stt::ADMITTED.available_permits(), 6);
    assert_eq!(crate::stt::RUNNING.available_permits(), 2);
    daemon.shutdown().await;
}

/// A clip upload whose body never arrives.
fn stalled(token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/stt")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let endless = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    builder.body(Body::from_stream(endless)).unwrap()
}

#[tokio::test]
async fn stalled_bodies_time_out_and_free_their_places() {
    let _serial = SERIAL.lock().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = open(
        root.path(),
        Setup {
            stt: Some("cat \"$1\"\n".into()),
            ..Setup::default()
        },
    )
    .await;
    let (_, person) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Steer])
        .await
        .unwrap();
    let local = unix(&daemon);
    let tcp = api::router(daemon.shared.clone(), false);
    // Every place goes to a stalled body, half over each path.
    let uploads: Vec<_> = (0..6)
        .map(|index| {
            let (router, token) = if index % 2 == 0 {
                (local.clone(), None)
            } else {
                (tcp.clone(), Some(person.clone()))
            };
            tokio::spawn(async move {
                let response = router.oneshot(stalled(token.as_deref())).await.unwrap();
                (response.status(), json_body(response).await)
            })
        })
        .collect();
    until(|| crate::stt::ADMITTED.available_permits() == 0).await;
    for task in uploads {
        let (status, body) = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the body-read deadline ends a stalled upload")
            .unwrap();
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::REQUEST_TIMEOUT, Some("audio_timeout"))
        );
    }
    assert_eq!(crate::stt::ADMITTED.available_permits(), 6);
    let (status, body) = call(&tcp, "POST", clip(b"after", "audio/webm"), Some(&person)).await;
    assert_eq!((status, body), (StatusCode::OK, json!({"text": "after"})));
    daemon.shutdown().await;
}

type Seen = Arc<parking_lot::Mutex<Option<(HeaderMap, Bytes)>>>;

async fn fake_openai(
    reply: (StatusCode, &'static str),
) -> (String, Seen, tokio::task::JoinHandle<()>) {
    let seen: Seen = Arc::default();
    let captured = seen.clone();
    let app = Router::new().route(
        "/v1/audio/transcriptions",
        axum::routing::post(move |headers: HeaderMap, body: Bytes| {
            let captured = captured.clone();
            async move {
                *captured.lock() = Some((headers, body));
                (reply.0, [("content-type", "application/json")], reply.1)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, seen, server)
}

#[tokio::test]
async fn the_openai_call_is_a_multipart_upload_with_the_bearer_key() {
    let (base, seen, server) = fake_openai((StatusCode::OK, r#"{"text":"  안녕하세요 \n"}"#)).await;
    let audio = b"\x1aE\xdf\xa3 webm bytes \r\n--not-a-boundary";
    let text = transcribe_openai(&base, "sk-fake", audio, "audio/webm", Some("ko-KR"))
        .await
        .unwrap();
    assert_eq!(text, "안녕하세요");
    let (headers, body) = seen.lock().take().unwrap();
    assert_eq!(headers["authorization"], "Bearer sk-fake");
    let content_type = headers["content-type"].to_str().unwrap();
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let field = |name: &str, value: &str| {
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
    };
    let mut expected = Vec::new();
    expected.extend_from_slice(field("model", "gpt-4o-mini-transcribe").as_bytes());
    expected.extend_from_slice(field("response_format", "json").as_bytes());
    expected.extend_from_slice(field("language", "ko").as_bytes());
    expected.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"clip.webm\"\r\nContent-Type: audio/webm\r\n\r\n").as_bytes(),
    );
    expected.extend_from_slice(audio);
    expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    assert_eq!(body.as_ref(), expected.as_slice());
    server.abort();

    let (base, seen, server) = fake_openai((StatusCode::OK, r#"{"text":"   "}"#)).await;
    assert_eq!(
        transcribe_openai(&base, "sk-fake", b"x", "audio/ogg", None).await,
        Err(Failure::NoSpeech)
    );
    let (_, body) = seen.lock().take().unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("name=\"language\""));
    server.abort();

    let (base, _, server) = fake_openai((
        StatusCode::UNAUTHORIZED,
        r#"{"error":{"message":"Incorrect API key provided: sk-fake"}}"#,
    ))
    .await;
    assert_eq!(
        transcribe_openai(&base, "sk-fake", b"x", "audio/ogg", None).await,
        Err(Failure::Failed)
    );
    server.abort();
}

#[tokio::test]
async fn the_openai_call_never_follows_a_redirect() {
    let followed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hit = followed.clone();
    let app = Router::new()
        .route(
            "/v1/audio/transcriptions",
            axum::routing::post(|| async {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", "/v1/elsewhere")],
                )
            }),
        )
        .route(
            "/v1/elsewhere",
            axum::routing::post(move || {
                hit.store(true, std::sync::atomic::Ordering::SeqCst);
                async { (StatusCode::OK, r#"{"text":"redirected"}"#) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert_eq!(
        transcribe_openai(&base, "sk-fake", b"x", "audio/ogg", None).await,
        Err(Failure::Failed)
    );
    assert!(!followed.load(std::sync::atomic::Ordering::SeqCst));
    server.abort();
}
