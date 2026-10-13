use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

fn status() -> Value {
    json!({"configured":true,"backend":"webdav","set_up":true,"unlocked":true,
        "account_id":"00112233445566778899aabbccddeeff","device_id":"0a0b0c0d0e0f1011",
        "last_sync":null,"records":2,"pending":1,"exhausted":true,
        "location":null,"webdav_username":null,"settings":false,"memory_tags":[],
        "passphrase_source":"entered","password_source":null,"last_run":null})
}

/// Fake daemon on a Unix socket answering `/v1/sync/*`.
async fn daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("sync.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(
            |State(seen): State<Seen>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock()
                    .push((method.clone(), path.clone(), body.clone()));
                match (method.as_str(), path.as_str()) {
                    ("POST", "/v1/sync/setup") => respond(
                        201,
                        json!({"pairing_code":"osync1-abcdefgh","status":status()}),
                    ),
                    ("POST", "/v1/sync/pair") if body["pairing_code"] == "osync1-bad" => respond(
                        400,
                        json!({"code":"sync_wrong_passphrase","message":"sync wrong passphrase"}),
                    ),
                    ("POST", "/v1/sync/pair") | ("GET", "/v1/sync") => respond(200, status()),
                    ("POST", "/v1/sync/run") => respond(
                        200,
                        json!({"pushed":1,"pulled":2,"applied":2,"conflicts":1,"rejected":0,
                            "deferred":0,"vault_available":true}),
                    ),
                    _ => respond(404, json!({"code":"not_found","message":"not found"})),
                }
            },
        )
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (socket, seen, server)
}

async fn cli(socket: &std::path::Path, args: &[&str]) -> (u8, String, String) {
    let mut argv = vec![
        OsString::from("openside"),
        OsString::from("--socket"),
        socket.as_os_str().to_owned(),
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

#[tokio::test]
async fn sync_setup_pair_now_and_status_commands() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    let last = || seen.lock().last().unwrap().clone();

    let (code, out, err) = cli(&socket, &["sync", "setup"]).await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.starts_with("pairing code\tosync1-abcdefgh\n"), "{out}");
    assert!(out.contains("openside sync pair osync1-abcdefgh"));
    assert_eq!(
        (last().0.as_str(), last().1.as_str()),
        ("POST", "/v1/sync/setup")
    );

    let (code, out, _) = cli(&socket, &["sync", "pair", "osync1-good"]).await;
    assert_eq!(code, 0);
    assert_eq!(last().2, json!({"pairing_code":"osync1-good"}));
    assert!(out.contains("account\t00112233445566778899aabbccddeeff\n"));
    let (code, _, err) = cli(&socket, &["sync", "pair", "osync1-bad"]).await;
    assert_ne!(code, 0);
    assert!(
        err.contains("sync_wrong_passphrase") || err.contains("sync wrong passphrase"),
        "{err}"
    );

    let (code, out, _) = cli(&socket, &["sync", "now"]).await;
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "pushed\t1\npulled\t2\napplied\t2\nconflicts\t1\nrejected\t0\ndeferred\t0\nvault\tsynced\n"
    );
    assert_eq!(last().1, "/v1/sync/run");

    let (code, out, _) = cli(&socket, &["--json", "sync", "status"]).await;
    assert_eq!(code, 0);
    // `--json` ends with a `{"type":"summary"}` line after the API object.
    let first = out.lines().next().unwrap();
    assert_eq!(serde_json::from_str::<Value>(first).unwrap(), status());
    let (code, out, _) = cli(&socket, &["sync", "status"]).await;
    assert_eq!(code, 0);
    assert!(
        out.contains("backend\twebdav\n") && out.contains("pending\t1\nexhausted\ttrue\n"),
        "{out}"
    );
    server.abort();
}
