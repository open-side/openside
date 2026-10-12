use super::*;

const ROUTINE: &str = r#"{"id":"r-1","name":"Daily report","schedule":{"type":"cron","expr":"0 9 * * *","timezone":"UTC"},
"action":{"type":"new_session","prompt":"report"},"enabled":true,"catch_up":false,
"created_at":"2026-01-01T00:00:00Z","next_run_at":"2026-01-02T09:00:00Z"}"#;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

struct Daemon {
    _environment: Environment,
    socket: PathBuf,
    seen: Seen,
    server: JoinHandle<()>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// Fake daemon: records every request and answers the routine endpoints with canned bodies.
async fn daemon() -> Daemon {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("routines.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(
            |State(seen): State<Seen>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let uri = request.uri().to_string();
                let path = request.uri().path().to_owned();
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock().push((method.clone(), uri, body));
                let routine: Value = serde_json::from_str(ROUTINE).unwrap();
                let answer = match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/routines") => json!({"routines":[routine]}),
                    ("GET", "/v1/routines/suggestions") => json!({"suggestions":[{
                        "name":"Check the build","prompt":"check the build",
                        "schedule":{"type":"cron","expr":"0 9 * * *","timezone":"UTC"},
                        "occurrences":5,"session_ids":["s-1"],
                        "first_seen":"2026-01-01T09:00:00Z","last_seen":"2026-01-05T09:00:00Z"}]}),
                    (_, p) if p.ends_with("/runs") => json!({"runs":[{
                        "id":7,"routine_id":"r-1","scheduled_at":"2026-01-01T09:00:00Z",
                        "status":"skipped_overlap","detail":"previous run has not settled yet"}]}),
                    (_, p) if p.ends_with("/run") => json!({
                        "id":8,"routine_id":"r-1","scheduled_at":"2026-01-01T09:00:00Z",
                        "status":"started","session_id":"s-9","message_id":"routine-1"}),
                    ("PATCH", _) => {
                        let mut paused = routine;
                        paused["enabled"] = request_enabled(&seen);
                        paused
                    }
                    _ => routine,
                };
                let status = if method == "POST" && path == "/v1/routines" {
                    201
                } else {
                    200
                };
                respond(status, answer)
            },
        )
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Daemon {
        _environment: environment,
        socket,
        seen,
        server,
    }
}

fn request_enabled(seen: &Seen) -> Value {
    seen.lock().last().unwrap().2["enabled"].clone()
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
async fn routine_add_builds_cron_and_interval_and_heartbeat_requests() {
    let daemon = daemon().await;
    let (code, out, err) = daemon
        .run(&[
            "routine",
            "add",
            "--name",
            "Daily report",
            "--cron",
            "0 9 * * 1-5",
            "--timezone",
            "Asia/Seoul",
            "--prompt",
            "report",
            "--catch-up",
        ])
        .await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.contains("r-1\tenabled\tcron 0 9 * * * UTC\tnew session\tnext 2026-01-02T09:00:00+00:00\tDaily report"), "{out}");
    let (method, uri, body) = daemon.last();
    assert_eq!((method.as_str(), uri.as_str()), ("POST", "/v1/routines"));
    assert_eq!(
        body,
        json!({"name":"Daily report","schedule":{"type":"cron","expr":"0 9 * * 1-5","timezone":"Asia/Seoul"},
            "action":{"type":"new_session","prompt":"report"},"catch_up":true})
    );

    let (code, ..) = daemon
        .run(&[
            "routine",
            "add",
            "--name",
            "n",
            "--interval-secs",
            "90",
            "--prompt",
            "p",
            "--disabled",
            "--model",
            "m",
        ])
        .await;
    assert_eq!(code, 0);
    assert_eq!(
        daemon.last().2,
        json!({"name":"n","schedule":{"type":"interval","secs":90},
            "action":{"type":"new_session","prompt":"p","model":"m"},"enabled":false})
    );

    let (code, ..) = daemon
        .run(&[
            "routine",
            "add",
            "--name",
            "beat",
            "--interval-secs",
            "60",
            "--prompt",
            "ping",
            "--heartbeat",
            "s-1",
        ])
        .await;
    assert_eq!(code, 0);
    assert_eq!(
        daemon.last().2["action"],
        json!({"type":"heartbeat","session_id":"s-1","prompt":"ping"})
    );
}

#[tokio::test]
async fn routine_add_rejects_conflicting_options_before_any_request() {
    let daemon = daemon().await;
    for args in [
        &["routine", "add", "--name", "n", "--prompt", "p"][..],
        &[
            "routine",
            "add",
            "--name",
            "n",
            "--prompt",
            "p",
            "--cron",
            "* * * * *",
            "--interval-secs",
            "5",
        ],
        &[
            "routine",
            "add",
            "--name",
            "n",
            "--prompt",
            "p",
            "--interval-secs",
            "5",
            "--heartbeat",
            "s",
            "--model",
            "m",
        ],
    ] {
        let (code, ..) = daemon.run(args).await;
        assert_eq!(code, 2, "{args:?}");
    }
    let (code, _, err) = daemon
        .run(&[
            "routine",
            "add",
            "--name",
            "n",
            "--prompt",
            "p",
            "--interval-secs",
            "5",
            "--timezone",
            "UTC",
        ])
        .await;
    assert_eq!(
        (code, err.as_str()),
        (1, "openside: --timezone only applies to --cron\n")
    );
    assert!(daemon.seen.lock().is_empty());
}

#[tokio::test]
async fn routine_list_show_run_runs_and_suggest_use_the_public_api() {
    let daemon = daemon().await;
    let (_, out, _) = daemon.run(&["routine", "list"]).await;
    assert_eq!(out, "r-1\tenabled\tcron 0 9 * * * UTC\tDaily report\n");
    let (_, out, _) = daemon.run(&["routine", "show", "r-1"]).await;
    assert!(out.starts_with("r-1\tenabled"), "{out}");
    assert_eq!(daemon.last().1, "/v1/routines/r-1");

    let (code, out, _) = daemon.run(&["routine", "run", "r-1"]).await;
    assert_eq!(code, 0);
    assert_eq!(out, "8\t2026-01-01T09:00:00+00:00\tstarted\ts-9\t\n");
    assert_eq!(daemon.last().0, "POST");

    let (_, out, _) = daemon
        .run(&["routine", "runs", "r-1", "--limit", "5"])
        .await;
    assert_eq!(
        out,
        "7\t2026-01-01T09:00:00+00:00\tskipped_overlap\t-\tprevious run has not settled yet\n"
    );
    assert_eq!(daemon.last().1, "/v1/routines/r-1/runs?limit=5");

    let (_, out, _) = daemon.run(&["routine", "suggest"]).await;
    assert_eq!(
        out,
        "Check the build\t5x\tcron 0 9 * * * UTC\tcheck the build\n"
    );
    let (code, out, _) = daemon.run(&["--json", "routine", "suggest"]).await;
    assert_eq!(code, 0);
    assert_eq!(json_lines(&out)[0]["suggestions"][0]["occurrences"], 5);
}

#[tokio::test]
async fn routine_pause_resume_and_rm_use_patch_and_delete() {
    let daemon = daemon().await;
    let (code, out, _) = daemon.run(&["routine", "pause", "r-1"]).await;
    assert_eq!(code, 0);
    let (method, uri, body) = daemon.last();
    assert_eq!(
        (method.as_str(), uri.as_str(), body),
        ("PATCH", "/v1/routines/r-1", json!({"enabled":false}))
    );
    assert!(out.starts_with("r-1\tpaused\t"), "{out}");

    let (_, out, _) = daemon.run(&["routine", "resume", "r-1"]).await;
    assert_eq!(daemon.last().2, json!({"enabled":true}));
    assert!(out.starts_with("r-1\tenabled\t"), "{out}");

    let (code, out, _) = daemon.run(&["routine", "rm", "r-1"]).await;
    assert_eq!(
        (code, out.as_str()),
        (0, "Deleted routine r-1 (Daily report)\n")
    );
    assert_eq!(daemon.last().0, "DELETE");
}
