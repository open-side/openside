use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

struct Daemon {
    environment: Environment,
    socket: PathBuf,
    seen: Seen,
    server: JoinHandle<()>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.server.abort();
    }
}

const POLICY: &str = r#"{"default_mode":"guard","allow_origins":[],"deny_origins":["https://*.evil.example"],"rules":[]}"#;

/// Fake daemon answering the pairing and policy routes with canned bodies.
async fn daemon() -> Daemon {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("policy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(
            |State(seen): State<Seen>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock()
                    .push((method.clone(), path.clone(), body.clone()));
                match (method.as_str(), path.as_str()) {
                    ("POST", "/v1/devices/pair") => respond(
                        201,
                        json!({"device_id":"dev-1","token":"tok-secret","scopes":body["scopes"]}),
                    ),
                    ("GET", "/v1/policy") => respond(200, serde_json::from_str(POLICY).unwrap()),
                    ("PUT", "/v1/policy") => respond(200, body),
                    ("POST", "/v1/policy/test") => {
                        respond(200, json!({"effect":"deny","reason":"origin_denied"}))
                    }
                    _ => respond(404, json!({"code":"not_found","message":"not found"})),
                }
            },
        )
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Daemon {
        environment,
        socket,
        seen,
        server,
    }
}

impl Daemon {
    async fn run(&self, args: &[&str]) -> (u8, String, String) {
        let mut argv = vec![
            OsString::from("openside"),
            OsString::from("--socket"),
            self.socket.clone().into_os_string(),
        ];
        argv.extend(args.iter().map(OsString::from));
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = run_with_input(argv, &mut std::io::empty(), &mut stdout, &mut stderr).await;
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    fn last(&self) -> (String, String, Value) {
        self.seen.lock().last().unwrap().clone()
    }
}

#[tokio::test]
async fn token_create_pairs_with_exact_wire_scopes_and_prints_the_token_once() {
    let daemon = daemon().await;
    let (code, out, err) = daemon
        .run(&[
            "token",
            "create",
            "--scope",
            "browse",
            "--scope",
            "vault-use",
            "--name",
            "sandbox",
        ])
        .await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "dev-1\ttok-secret\n");
    assert_eq!(
        daemon.last(),
        (
            "POST".into(),
            "/v1/devices/pair".into(),
            json!({"name":"sandbox","scopes":["browse","vault_use"],"kind":"agent_client"})
        )
    );
    let (code, _, err) = daemon.run(&["token", "create", "--scope", "root"]).await;
    assert_eq!(code, 2, "{err}");
    let (code, _, _) = daemon.run(&["token", "create"]).await;
    assert_eq!(code, 2, "--scope is required");
    assert_eq!(daemon.seen.lock().len(), 1);
}

#[tokio::test]
async fn person_token_flag_issues_a_person_device_for_extension_approvals() {
    let daemon = daemon().await;
    let (code, _, error) = daemon
        .run(&[
            "token", "create", "--person", "--name", "browser", "--scope", "read", "--scope",
            "steer", "--scope", "approve",
        ])
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        daemon.last().2,
        json!({
            "name":"browser","kind":"person_device","scopes":["read","steer","approve"]
        })
    );
}

#[tokio::test]
async fn policy_show_set_and_test_use_the_policy_routes() {
    let daemon = daemon().await;
    let (code, out, _) = daemon.run(&["policy", "show", "--json"]).await;
    assert_eq!(code, 0);
    let first: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(first, serde_json::from_str::<Value>(POLICY).unwrap());

    let file = daemon.environment.directory.path().join("policy.json");
    let wanted = json!({"default_mode":"read-only","allow_origins":["http://127.0.0.1:*"],
        "deny_origins":[],"rules":[{"effect":"deny","capability":"fs_write","pattern":"/etc/**"}]});
    std::fs::write(&file, wanted.to_string()).unwrap();
    let (code, _, err) = daemon.run(&["policy", "set", file.to_str().unwrap()]).await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(daemon.last(), ("PUT".into(), "/v1/policy".into(), wanted));

    std::fs::write(&file, r#"{"default_mode":"guard","bogus":1}"#).unwrap();
    let before = daemon.seen.lock().len();
    let (code, _, err) = daemon.run(&["policy", "set", file.to_str().unwrap()]).await;
    assert_eq!(code, 1);
    assert!(err.contains("invalid policy JSON"), "{err}");
    assert_eq!(
        daemon.seen.lock().len(),
        before,
        "invalid files are not sent"
    );

    let (code, out, _) = daemon
        .run(&[
            "policy",
            "test",
            "https://x.evil.example/pay",
            "browse",
            "--mode",
            "full",
        ])
        .await;
    assert_eq!(code, 0);
    assert_eq!(out, "deny\torigin_denied\n");
    assert_eq!(
        daemon.last(),
        (
            "POST".into(),
            "/v1/policy/test".into(),
            json!({"target":"https://x.evil.example/pay","capability":"browse","mode":"full"})
        )
    );
    let (code, _, _) = daemon.run(&["policy", "test", "x", "teleport"]).await;
    assert_eq!(code, 2);
}
