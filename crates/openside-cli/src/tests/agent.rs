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
                ("PATCH", "/v1/sessions/s-1") => respond(200, session_json(9)),
                ("DELETE", "/v1/sessions/s-1") => Response::builder().status(204).body(Body::empty()).unwrap(),
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

fn requests(seen: &Seen, method: &str, path: &str) -> Vec<Value> {
    seen.lock()
        .iter()
        .filter(|(m, p, _)| m == method && p == path)
        .map(|(_, _, body)| body.clone())
        .collect()
}

#[tokio::test]
async fn exec_follows_the_run_and_archives_unsaved_sessions() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["exec", "find", "it"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "cheapest is 42\n");
    assert!(
        err.contains("[task task-1] started: compare prices"),
        "{err}"
    );
    let created = requests(&seen, "POST", "/v1/sessions");
    assert_eq!(created.len(), 1);
    assert!(created[0].get("browser_space").is_none_or(Value::is_null));
    assert!(created[0].get("agent_id").is_none_or(Value::is_null));
    assert_eq!(
        requests(&seen, "POST", "/v1/sessions/s-1/messages")[0]["text"],
        "find it"
    );
    assert_eq!(
        requests(&seen, "PATCH", "/v1/sessions/s-1"),
        vec![json!({"archived": true})]
    );

    // A bare prompt is `exec`; `-` reads it from stdin.
    let (code, out, err) = cli(&socket, &["--json", "-"], "goal from stdin").await;
    assert_eq!(code, 0, "{err}");
    let summary: Value = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    assert_eq!(summary["status"], "settled");
    assert_eq!(summary["session_id"], "s-1");
    assert!(requests(&seen, "POST", "/v1/sessions/s-1/messages")
        .iter()
        .any(|body| body["text"] == "goal from stdin"));

    let (code, out, _) = cli(&socket, &["settings", "save-sessions", "true"], "").await;
    assert_eq!((code, out.as_str()), (0, "ok\n"));
    let archived = requests(&seen, "PATCH", "/v1/sessions/s-1").len();
    let (code, ..) = cli(
        &socket,
        &[
            "-m",
            "openai/gpt-5",
            "--permission",
            "full-access",
            "--profile",
            "incognito",
            "x",
        ],
        "",
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(requests(&seen, "PATCH", "/v1/sessions/s-1").len(), archived);
    let created = requests(&seen, "POST", "/v1/sessions");
    let last = created.last().unwrap();
    assert_eq!(last["model"], "openai:gpt-5");
    assert_eq!(last["mode"], "full");
    assert_eq!(last["incognito"], true);
    let (code, ..) = cli(
        &socket,
        &[
            "exec",
            "-m",
            "gpt-5",
            "-p",
            "openai",
            "--permission",
            "ask",
            "--effort",
            "xhigh",
            "-s",
            "fast",
            "x",
        ],
        "",
    )
    .await;
    assert_eq!(code, 0);
    let created = requests(&seen, "POST", "/v1/sessions");
    let last = created.last().unwrap();
    assert_eq!(last["model"], "openai:gpt-5");
    assert_eq!(last["mode"], "guard");
    assert_eq!(
        (&last["effort"], &last["speed"]),
        (&json!("xhigh"), &json!("fast"))
    );
    assert!(created[0].get("effort").is_none() && created[0].get("speed").is_none());
    assert!(last.get("profile").is_none());
    let (code, ..) = cli(&socket, &["--effort", "ultrabrowse", "x"], "").await;
    assert_eq!(code, 0);
    let created = requests(&seen, "POST", "/v1/sessions");
    let last = created.last().unwrap();
    assert_eq!(last["profile"], "ultrabrowse");
    assert_eq!(last["effort"], "max");
    assert_public(&seen);
    server.abort();
}

#[tokio::test]
async fn invalid_and_misplaced_task_options_are_usage_errors() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    for (args, message) in [
        (&["-p", "openai", "x"][..], "--provider needs --model"),
        (&["--profile", "work", "x"][..], "unknown browser profile"),
    ] {
        let (code, _, err) = cli(&socket, args, "").await;
        assert_eq!(code, 2, "{args:?}");
        assert!(err.contains(message), "{args:?}: {err}");
    }
    let (code, ..) = cli(&socket, &["--model", "m", "session", "list"], "").await;
    assert_eq!(code, 2);
    let (code, ..) = cli(&socket, &["--effort", "turbo", "x"], "").await;
    assert_eq!(code, 2);
    assert!(seen.lock().is_empty());
    server.abort();
}

#[tokio::test]
async fn session_commands_steer_queue_stop_and_tidy_the_run() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["session", "tasks", "s-1"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "task-1\trunning\tcompare prices\ntwo shops checked\n");
    let (code, out, _) = cli(&socket, &["session", "steer", "s-1", "hurry", "up"], "").await;
    assert_eq!((code, out.as_str()), (0, "ok\n"));
    let (code, out, err) = cli(
        &socket,
        &["session", "steer", "s-1", "only shop A", "--task", "task-1"],
        "",
    )
    .await;
    assert_eq!((code, out.as_str()), (0, "ok\n"), "{err}");
    let (code, out, _) = cli(&socket, &["session", "queue", "s-1", "then", "export"], "").await;
    assert_eq!((code, out.as_str()), (0, "ok\n"));
    let (code, ..) = cli(&socket, &["session", "stop", "s-1", "--task", "task-1"], "").await;
    assert_eq!(code, 0);
    let (code, out, _) = cli(&socket, &["session", "stop", "s-1"], "").await;
    assert_eq!((code, out.as_str()), (0, "Stopped session s-1\n"));
    let (code, out, _) = cli(&socket, &["session", "archive", "s-1"], "").await;
    assert_eq!((code, out.as_str()), (0, "ok\n"));
    let (code, out, _) = cli(&socket, &["session", "delete", "s-1"], "").await;
    assert_eq!((code, out.as_str()), (0, "ok\n"));

    let messages = requests(&seen, "POST", "/v1/sessions/s-1/messages");
    assert!(messages
        .iter()
        .any(|body| body["mode"] == "steer" && body["text"] == "hurry up"));
    assert!(messages
        .iter()
        .any(|body| body["mode"] == "queue" && body["text"] == "then export"));
    assert_eq!(
        requests(&seen, "POST", "/v1/sessions/s-1/subagents/task-1/steer")[0]["text"],
        "only shop A"
    );
    assert_eq!(
        requests(&seen, "POST", "/v1/sessions/s-1/subagents/task-1/cancel").len(),
        1
    );
    assert_eq!(requests(&seen, "POST", "/v1/sessions/s-1/stop").len(), 1);
    assert_eq!(
        requests(&seen, "PATCH", "/v1/sessions/s-1"),
        vec![json!({"archived": true})]
    );
    assert_eq!(requests(&seen, "DELETE", "/v1/sessions/s-1").len(), 1);
    assert_public(&seen);
    server.abort();
}

#[tokio::test]
async fn a_bare_url_opens_a_tab_instead_of_starting_a_task() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;

    let (code, out, err) = cli(&socket, &["https://example.org/a?b=c"], "").await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "Opened https://example.org/a?b=c in tab tab-1\n");
    assert_eq!(
        requests(&seen, "POST", "/v1/sessions/s-1/tools/browser_open_tab"),
        vec![json!({"args":{"url":"https://example.org/a?b=c"}})]
    );
    assert!(requests(&seen, "POST", "/v1/sessions/s-1/messages").is_empty());
    assert_eq!(
        requests(&seen, "PATCH", "/v1/sessions/s-1"),
        vec![json!({"archived": true})]
    );

    // An isolated profile would close the tab on archive, so that session is kept.
    let (code, ..) = cli(
        &socket,
        &["--profile", "isolated", "http://example.org"],
        "",
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(requests(&seen, "PATCH", "/v1/sessions/s-1").len(), 1);

    // Anything more than one URL, or another scheme, is still a task prompt.
    for prompt in [
        &["open", "https://example.org"][..],
        &["file:///etc/passwd"][..],
    ] {
        let (code, ..) = cli(&socket, prompt, "").await;
        assert_eq!(code, 0);
    }
    assert_eq!(
        requests(&seen, "POST", "/v1/sessions/s-1/tools/browser_open_tab").len(),
        2
    );
    assert_eq!(
        requests(&seen, "POST", "/v1/sessions/s-1/messages").len(),
        2
    );
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
