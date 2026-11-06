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

fn document(config: Value, restart_required: bool) -> Value {
    json!({
        "config": config,
        "restart_required": restart_required,
        "file_only": ["credentials"],
        "schema": {"type": "object", "properties": {"tickets": {"type": "object"}}},
        "remote_editable": ["tickets.confirm_ttl_secs"],
        "remote": false,
    })
}

/// Fake daemon for `/v1/settings/config`: refuses `policy.mode` like a paired device away from
/// the computer, and answers a write with the written document.
async fn daemon() -> Daemon {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("config.sock");
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
                    ("GET", "/v1/settings/config") => respond(
                        200,
                        document(
                            json!({"tickets": {"confirm_ttl_secs": 600}, "policy": {"mode": "full"}, "ui": {}}),
                            false,
                        ),
                    ),
                    ("PUT", "/v1/settings/config") if body["config"]["policy"]["mode"] != "full" => {
                        respond(
                            403,
                            json!({"code": "local_access_required", "message": "local access required"}),
                        )
                    }
                    ("PUT", "/v1/settings/config") => {
                        respond(200, document(body["config"].clone(), true))
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

    fn writes(&self) -> Vec<Value> {
        self.seen
            .lock()
            .iter()
            .filter(|(method, _, _)| method == "PUT")
            .map(|(_, _, body)| body.clone())
            .collect()
    }
}

/// `$VISUAL` for one test; the environment lock in `Environment` keeps other tests out.
struct Editor(Option<OsString>);

impl Editor {
    fn set(command: &str) -> Self {
        let saved = std::env::var_os("VISUAL");
        std::env::set_var("VISUAL", command);
        Self(saved)
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("VISUAL", value),
            None => std::env::remove_var("VISUAL"),
        }
    }
}

#[tokio::test]
async fn config_get_prints_a_key_or_the_document_and_schema_prints_the_schema() {
    let daemon = daemon().await;
    let (code, out, err) = daemon
        .run(&["config", "get", "tickets.confirm_ttl_secs"])
        .await;
    assert_eq!((code, out.as_str(), err.as_str()), (0, "600\n", ""));
    let (code, out, _) = daemon.run(&["config", "get", "policy.mode"]).await;
    assert_eq!((code, out.as_str()), (0, "full\n"));
    let (code, out, _) = daemon.run(&["config", "get"]).await;
    assert_eq!(code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({"tickets": {"confirm_ttl_secs": 600}, "policy": {"mode": "full"}, "ui": {}})
    );
    let (code, out, _) = daemon.run(&["--json", "config", "schema"]).await;
    assert_eq!(code, 0);
    let schema: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(schema["type"], "object");
    let (code, _, err) = daemon.run(&["config", "get", "tickets.nope"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("no setting \"tickets.nope\""), "{err}");
    assert!(daemon.writes().is_empty());
}

#[tokio::test]
async fn config_set_keeps_json_types_writes_the_whole_document_and_relays_the_local_only_refusal() {
    let daemon = daemon().await;
    let (code, out, err) = daemon
        .run(&["config", "set", "tickets.confirm_ttl_secs", "900"])
        .await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "tickets.confirm_ttl_secs = 900\nRestart openside to apply it.\n"
    );
    assert_eq!(
        daemon.writes(),
        vec![
            json!({"config": {"tickets": {"confirm_ttl_secs": 900}, "policy": {"mode": "full"}, "ui": {}}})
        ]
    );

    // An unset optional key creates its section; a text key takes plain text.
    let (code, _, err) = daemon.run(&["config", "set", "ui.dir", "/srv/ui"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        daemon.writes()[1]["config"]["ui"],
        json!({"dir": "/srv/ui"})
    );

    let (code, _, err) = daemon
        .run(&["config", "set", "policy.mode", "read-only"])
        .await;
    assert_eq!(code, 1);
    assert!(err.contains("local_access_required"), "{err}");
    assert!(
        err.contains("Only the computer openside runs on can change that key"),
        "{err}"
    );
    assert_eq!(daemon.writes()[2]["config"]["policy"]["mode"], "read-only");
}

#[tokio::test]
async fn config_edit_saves_what_the_editor_wrote_and_nothing_when_unchanged() {
    let daemon = daemon().await;
    let script = daemon.environment.directory.path().join("edit.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nsed 's/600/601/' \"$1\" > \"$1.new\" && mv \"$1.new\" \"$1\"\n",
    )
    .unwrap();
    {
        let _editor = Editor::set(&format!("sh {}", script.display()));
        let (code, out, err) = daemon.run(&["config", "edit"]).await;
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out, "saved\nRestart openside to apply it.\n");
    }
    assert_eq!(
        daemon.writes()[0]["config"]["tickets"]["confirm_ttl_secs"],
        601
    );

    let _editor = Editor::set("true");
    let (code, out, _) = daemon.run(&["config", "edit"]).await;
    assert_eq!((code, out.as_str()), (0, "no changes\n"));
    assert_eq!(daemon.writes().len(), 1);
}
