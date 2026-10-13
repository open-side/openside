use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

fn session_json(last_seq: u64) -> Value {
    json!({"id":"s-1","title":"t","model":"m","mode":"guard","workspace":null,
        "status":"idle","last_seq":last_seq,"queue":[]})
}

/// Fake daemon for delegation and direct tool calls. A run's events follow sequence 3.
async fn daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("agent.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(|State(seen): State<Seen>, request: Request<Body>| async move {
            let method = request.method().to_string();
            let path = request.uri().path().to_owned();
            let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let polls = seen
                .lock()
                .iter()
                .filter(|(_, path, _)| path == "/v1/tickets/t-login")
                .count();
            seen.lock().push((method.clone(), path.clone(), body.clone()));
            let task = json!({"id":"task-1","parent_tool_call_id":"c1","task":"compare prices",
                "model":"m","tools":[],"status":"running","started_at":"2026-01-01T00:00:00Z",
                "background":true,"output":"two shops checked"});
            match (method.as_str(), path.as_str()) {
                ("POST", "/v1/sessions") => respond(201, session_json(3)),
                ("GET", "/v1/agents/chief") => respond(200, json!({
                    "id":"chief","kind":"chief","name":"openside","initials":"o","color":"#2f6fed",
                    "persona":{"role":"Coordinator","responsibilities":[],"boundaries":[],"voice":""},
                    "skills":null,"model":null,"account_pool":null,"channel":null,"mode":"guard",
                    "home_session_id":"s-1","status":"active","hidden":false,"pinned":true,
                    "created_at":"2026-01-01T00:00:00Z","retired_at":null,"persona_version":1
                })),
                ("GET", "/v1/sessions/s-1") => respond(200, session_json(3)),
                ("POST", "/v1/sessions/s-1/messages") => respond(
                    202,
                    json!({"id":"m-1","text":body["text"],"status":"delivered"}),
                ),
                ("GET", "/v1/sessions/s-1/events") => {
                    let events = [
                        event(4, "message.injected", json!({"message_id":"m-1"})),
                        event(
                            5,
                            "subagent.started",
                            json!({"subagent_id":"task-1","parent_tool_call_id":"c1",
                                "task":"compare prices","model":"m","tools":[],"background":true}),
                        ),
                        event(6, "assistant.final", json!({"text":"cheapest is 42"})),
                        event(
                            7,
                            "message.completed",
                            json!({"message_id":"m-1","outcome":"completed"}),
                        ),
                        event(8, "session.settled", json!({})),
                    ];
                    let text: String = events
                        .iter()
                        .map(|event| format!("id: {}\ndata: {event}\n\n", event["seq"]))
                        .collect();
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::from(text))
                        .unwrap()
                }
                ("GET", "/v1/tickets") => respond(200, json!({"tickets":[]})),
                ("GET", "/v1/sessions/s-1/subagents") => respond(200, json!({"subagents":[task]})),
                ("POST", "/v1/sessions/s-1/subagents/task-1/steer") => respond(202, task),
                ("POST", "/v1/sessions/s-1/subagents/task-1/cancel") => respond(200, task),
                ("POST", "/v1/sessions/s-1/stop") => respond(200, session_json(9)),
                ("GET", "/v1/tools") => respond(
                    200,
                    json!({"tools":[{"name":"browser_open_tab","description":"Open.",
                        "input_schema":{"type":"object"},"scope":"browse","human":false}]}),
                ),
                ("POST", "/v1/sessions/s-1/tools/browser_open_tab") => respond(
                    200,
                    json!({"tool_call_id":"api-tool-1","status":"done","result":{"tab_id":"tab-1","url":body["args"]["url"]}}),
                ),
                ("POST", "/v1/sessions/s-1/tools/browser_screenshot") => respond(
                    200,
                    if body["args"]["output"] == true {
                        json!({"tool_call_id":"api-tool-2","status":"done","result":{"saved":true,"purpose":"output"},
                            "artifacts":["screenshots/api-tool-2.png"]})
                    } else {
                        json!({"tool_call_id":"api-tool-2","status":"done","result":{"saved":false,"purpose":"working"}})
                    },
                ),
                ("GET", "/v1/sessions/s-1/artifacts/screenshots%2Fapi-tool-2.png") => {
                    Response::builder().status(200).body(Body::from("screenshot-bytes")).unwrap()
                }
                ("POST", "/v1/sessions/s-1/tools/request_login") => respond(
                    200,
                    json!({"tool_call_id":"api-tool-3","status":"deferred","ticket_id":"t-login","deferral":"external"}),
                ),
                ("GET", "/v1/tickets/t-login") => {
                    let mut ticket = ticket();
                    ticket["id"] = json!("t-login");
                    ticket["deferral"] = json!("external");
                    ticket["kind"] = json!("login");
                    if polls > 0 {
                        ticket["status"] = json!("resolved");
                        ticket["resolution"] = json!({"signed_in": true});
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

fn assert_public(seen: &Seen) {
    for (method, path, _) in seen.lock().iter() {
        let template_match = openside_proto::ROUTES.iter().any(|route| {
            let (t, a): (Vec<_>, Vec<_>) =
                (route.path.split('/').collect(), path.split('/').collect());
            route.method.as_str() == method
                && t.len() == a.len()
                && t.iter().zip(&a).all(|(t, a)| t.starts_with('{') || t == a)
        });
        assert!(template_match, "{method} {path}");
    }
}

#[tokio::test]
async fn run_delegates_detached_or_waits_for_the_replies() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["run", "find it"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "s-1\tm-1\n");
    assert!(seen
        .lock()
        .iter()
        .any(|(method, path, _)| method == "GET" && path == "/v1/agents/chief"));
    assert!(!seen
        .lock()
        .iter()
        .any(|(method, path, _)| method == "POST" && path == "/v1/sessions"));
    let (code, out, err) = cli(&socket, &["chat", "continue"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "s-1\tm-1\n");

    let (code, out, err) = cli(
        &socket,
        &["run", "--wait", "--session", "s-1", "find it"],
        "",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "cheapest is 42\n");
    assert!(
        err.contains("[task task-1] started: compare prices"),
        "{err}"
    );

    let (code, out, _) = cli(
        &socket,
        &["--json", "run", "--wait", "-"],
        "goal from stdin",
    )
    .await;
    assert_eq!(code, 0);
    let result: Value = serde_json::from_str(out.lines().rev().nth(1).unwrap()).unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["status"], "settled");
    assert_eq!(result["replies"], json!(["cheapest is 42"]));
    assert!(seen
        .lock()
        .iter()
        .any(|(_, path, body)| path == "/v1/sessions/s-1/messages"
            && body["text"] == "goal from stdin"));

    let (code, _, err) = cli(&socket, &["--mode", "full", "run", "x"], "").await;
    assert_eq!(code, 2);
    assert!(err.contains("openside session new"), "{err}");
    let (code, ..) = cli(
        &socket,
        &["--mode", "full", "run", "--session", "s-1", "x"],
        "",
    )
    .await;
    assert_eq!(code, 2);
    let (code, ..) = cli(&socket, &["--mode", "full", "tasks", "s-1"], "").await;
    assert_eq!(code, 2);
    assert_public(&seen);
    server.abort();
}

#[tokio::test]
async fn steer_cancel_and_tasks_reach_the_run_or_one_task() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["tasks", "s-1"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "task-1\trunning\tcompare prices\ntwo shops checked\n");
    let (code, ..) = cli(&socket, &["steer", "s-1", "hurry"], "").await;
    assert_eq!(code, 0);
    let (code, _, err) = cli(
        &socket,
        &["steer", "s-1", "only shop A", "--task", "task-1"],
        "",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    let (code, ..) = cli(&socket, &["cancel", "s-1", "--task", "task-1"], "").await;
    assert_eq!(code, 0);
    let (code, out, _) = cli(&socket, &["cancel", "s-1"], "").await;
    assert_eq!(code, 0);
    assert_eq!(out, "Stopped session s-1\n");

    let requests = seen.lock().clone();
    assert!(requests
        .iter()
        .any(|(_, path, body)| path == "/v1/sessions/s-1/messages"
            && body["mode"] == "steer"
            && body["text"] == "hurry"));
    assert!(requests.iter().any(|(_, path, body)| path
        == "/v1/sessions/s-1/subagents/task-1/steer"
        && body["text"] == "only shop A"));
    assert!(requests
        .iter()
        .any(|(_, path, _)| path == "/v1/sessions/s-1/subagents/task-1/cancel"));
    assert!(requests
        .iter()
        .any(|(_, path, _)| path == "/v1/sessions/s-1/stop"));
    assert_public(&seen);
    server.abort();
}

#[tokio::test]
async fn browser_verbs_call_the_daemon_tools_and_wait_for_a_person() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["session", "new", "--title", "drive"], "").await;
    assert_eq!((code, out.as_str()), (0, "s-1\n"), "{err}");

    let (code, out, err) = cli(
        &socket,
        &["browser", "open", "s-1", "url=https://example.test/"],
        "",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        "{\"tab_id\":\"tab-1\",\"url\":\"https://example.test/\"}\n"
    );
    let (code, out, _) = cli(
        &socket,
        &["browser", "screenshot", "s-1", "tab_id=tab-1"],
        "",
    )
    .await;
    assert_eq!(code, 0);
    assert!(!out.contains("artifact:"), "{out}");
    assert!(out.contains("\"purpose\":\"working\""), "{out}");
    let (code, out, _) = cli(
        &socket,
        &[
            "browser",
            "screenshot",
            "s-1",
            "tab_id=tab-1",
            "output=true",
        ],
        "",
    )
    .await;
    assert_eq!(code, 0);
    assert!(
        out.ends_with("artifact: screenshots/api-tool-2.png\n"),
        "{out}"
    );
    let out_file = environment.directory.path().join("shot.png");
    let (code, _, err) = cli(
        &socket,
        &[
            "browser",
            "screenshot",
            "s-1",
            "tab_id=tab-1",
            "--out",
            out_file.to_str().unwrap(),
        ],
        "",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(std::fs::read(out_file).unwrap(), b"screenshot-bytes");
    assert!(seen
        .lock()
        .iter()
        .filter(|(_, path, _)| path.ends_with("/browser_screenshot"))
        .map(|(_, _, body)| body["args"]["output"] == true)
        .eq([false, true, true]));

    // A login ticket is answered by the person; its resolution is the command's result.
    let (code, out, err) = cli(&socket, &["browser", "login", "s-1", "tab_id=tab-1"], "").await;
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("Waiting for a person to answer ticket t-login"),
        "{err}"
    );
    assert_eq!(out, "{\"signed_in\":true}\n");

    let (code, out, _) = cli(&socket, &["--json", "browser", "tools"], "").await;
    assert_eq!(code, 0);
    let catalog: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(catalog["tools"][0]["name"], "browser_open_tab");

    let (code, _, err) = cli(&socket, &["browser", "open", "s-1", "nonsense"], "").await;
    assert_eq!(code, 1);
    assert!(err.contains("key=value"), "{err}");
    assert_public(&seen);
    server.abort();
}
