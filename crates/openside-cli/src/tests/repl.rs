use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

/// Fake daemon: `approve-me` needs an approval that is granted on the second ticket poll.
async fn daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("repl.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(|State(seen): State<Seen>, request: Request<Body>| async move {
            let method = request.method().to_string();
            let path = request.uri().path().to_owned();
            let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let polls = seen.lock().iter().filter(|(_, path, _)| path == "/v1/tickets/t-9").count();
            seen.lock().push((method.clone(), path.clone(), body.clone()));
            match (method.as_str(), path.as_str()) {
                ("POST", "/v1/sessions/s-1/repl") => {
                    let code = body["code"].as_str().unwrap();
                    if code == "approve-me" && body["approval_ticket"].is_null() {
                        return respond(200, json!({"tool_call_id":"api-repl-1","status":"deferred","ticket_id":"t-9"}));
                    }
                    if code.contains("throw") {
                        return respond(200, json!({"tool_call_id":"api-repl-2","status":"done",
                            "result":{"return_value":null,"console":[],"error":"Error: boom","tab_id":"tab-1"}}));
                    }
                    if code == "denied" {
                        return respond(200, json!({"tool_call_id":"api-repl-3","status":"denied",
                            "reason":"policy_denied: policy denies browsing","hint":"Choose a permitted action"}));
                    }
                    let value = if code == "approve-me" { json!("bought") } else { json!({"echo": code}) };
                    respond(200, json!({"tool_call_id":"api-repl-4","status":"done",
                        "result":{"return_value":value,"console":["log line"],"tab_id":"tab-1"}}))
                }
                ("GET", "/v1/tickets/t-9") => {
                    let mut ticket = ticket();
                    ticket["id"] = json!("t-9");
                    ticket["tool_call_id"] = json!("api-repl-1");
                    if polls > 0 {
                        ticket["status"] = json!("resolved");
                        ticket["resolution"] = json!({"approved": true});
                    }
                    respond(200, ticket)
                }
                _ => respond(404, json!({"code":"not_found","message":"unknown"})),
            }
        })
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (socket, seen, server)
}

async fn cli(socket: &std::path::Path, args: &[&str], stdin: &str) -> (u8, String, String) {
    let mut argv = vec![
        OsString::from("openside"),
        OsString::from("--socket"),
        socket.as_os_str().to_owned(),
    ];
    argv.extend(args.iter().map(OsString::from));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let mut input = std::io::Cursor::new(stdin.as_bytes().to_vec());
    let code = run_with_input(argv, &mut input, &mut stdout, &mut stderr).await;
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[tokio::test]
async fn repl_eval_stdin_and_interactive_post_snippets_and_print_results() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    let posts = || {
        seen.lock()
            .iter()
            .filter(|(method, ..)| method == "POST")
            .map(|(_, _, body)| body.clone())
            .collect::<Vec<_>>()
    };

    let (code, out, err) = cli(
        &socket,
        &["repl", "s-1", "--eval", "await page.title()"],
        "",
    )
    .await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "log line\n{\"echo\":\"await page.title()\"}\n");
    assert_eq!(
        posts().last().unwrap(),
        &json!({"code":"await page.title()"})
    );

    let script = "await page.goto('https://example.test');\npage.url()\n";
    let (code, ..) = cli(&socket, &["repl", "s-1", "--tab", "tab-7"], script).await;
    assert_eq!(code, 0);
    assert_eq!(
        posts().last().unwrap(),
        &json!({"code": script, "tab_id": "tab-7"})
    );

    // Interactive mode runs each non-empty line and keeps the tab the daemon reported.
    let before = posts().len();
    let (code, out, err) = cli(&socket, &["repl", "s-1", "-i"], "1 + 1\n\n2 + 2\n").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.matches("log line").count(), 2);
    assert_eq!(err, "> > > > ");
    let sent = posts();
    assert_eq!(
        sent[before..],
        [
            json!({"code":"1 + 1\n"}),
            json!({"code":"2 + 2\n","tab_id":"tab-1"})
        ]
    );

    let (code, out, err) = cli(&socket, &["--json", "repl", "s-1", "--eval", "x"], "").await;
    assert_eq!(code, 0, "{err}");
    let first: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(first["status"], "done");
    assert_eq!(first["result"]["return_value"], json!({"echo":"x"}));

    let (code, _, err) = cli(
        &socket,
        &["repl", "s-1", "--eval", "throw new Error('boom')"],
        "",
    )
    .await;
    assert_eq!(code, 1);
    assert_eq!(err, "Error: boom\n");
    let (code, _, err) = cli(&socket, &["repl", "s-1", "--eval", "denied"], "").await;
    assert_eq!(code, 1);
    assert!(err.contains("policy_denied"), "{err}");
    let (code, _, err) = cli(&socket, &["repl", "s-1"], "  \n").await;
    assert_eq!(code, 1);
    assert!(err.contains("no code"), "{err}");
    server.abort();
}

#[tokio::test]
async fn repl_waits_for_approval_then_reruns_with_the_ticket() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    let (code, out, err) = cli(&socket, &["repl", "s-1", "--eval", "approve-me"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "log line\n\"bought\"\n");
    assert!(err.contains("Approval needed (ticket t-9)"), "{err}");
    assert!(err.contains("ticket resolve t-9 true"), "{err}");
    let seen = seen.lock().clone();
    let paths: Vec<_> = seen
        .iter()
        .map(|(method, path, _)| format!("{method} {path}"))
        .collect();
    assert_eq!(
        paths,
        [
            "POST /v1/sessions/s-1/repl",
            "GET /v1/tickets/t-9",
            "GET /v1/tickets/t-9",
            "POST /v1/sessions/s-1/repl"
        ]
    );
    assert_eq!(
        seen[3].2,
        json!({"code":"approve-me","approval_ticket":"t-9"})
    );
    server.abort();
}
