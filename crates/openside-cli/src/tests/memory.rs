use super::*;
use axum::{extract::Path as UrlPath, routing::get};

const MEMORY: &str = r#"{"id":"m-1","kind":"note","content":"Aisle seat preferred","tags":["travel"],
"retention":"days_90","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z",
"expires_at":"2026-04-01T00:00:00Z","version":2}"#;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

/// Fake daemon on a Unix socket: records requests and answers memory endpoints.
async fn daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("memory.sock");
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
                let memory: Value = serde_json::from_str(MEMORY).unwrap();
                let answer = match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/memory") => json!({"mode":"lexical_only","hits":[{"memory":memory,"score":0.5,"matched":"lexical"}]}),
                    (_, p) if p.ends_with("/history") => json!({"id":"m-1","versions":[
                        {"version":1,"op":"create","actor":"api","at":"2026-01-01T00:00:00Z","content":"Aisle seat","tags":[],"retention":"permanent"},
                        {"version":2,"op":"delete","actor":"cli","at":"2026-01-02T00:00:00Z"}]}),
                    ("POST", "/v1/memory") => {
                        return respond(400, json!({"code":"secret_detected","message":"Content looks like a secret"}));
                    }
                    _ => memory,
                };
                respond(200, answer)
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
async fn memory_commands_build_requests_and_print_results() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    let last = || seen.lock().last().unwrap().clone();

    let (code, out, err) = cli(
        &socket,
        &[
            "memory",
            "search",
            "seat & aisle",
            "--kind",
            "note",
            "--limit",
            "3",
        ],
    )
    .await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "m-1\tnote\t0.500\tAisle seat preferred\n");
    assert_eq!(last().1, "/v1/memory?q=seat+%26+aisle&kind=note&limit=3");

    let (code, ..) = cli(&socket, &["memory", "search"]).await;
    assert_eq!(code, 0);
    assert_eq!(last().1, "/v1/memory");

    let (code, out, err) = cli(
        &socket,
        &[
            "memory",
            "add",
            "token sk-x",
            "--tag",
            "a",
            "--retention",
            "days_30",
        ],
    )
    .await;
    assert_eq!(code, 1, "{out}");
    assert!(err.contains("secret_detected"), "{err}");
    assert_eq!(
        last().2,
        json!({"content":"token sk-x","tags":["a"],"retention":"days_30"})
    );

    let (code, out, _) = cli(&socket, &["memory", "show", "m/1"]).await;
    assert_eq!(code, 0);
    assert_eq!(last().1, "/v1/memory/m%2F1");
    assert!(
        out.starts_with("m-1\tnote\tv2\tdays_90\texpires 2026-04-01T00:00:00+00:00\ttags travel\n"),
        "{out}"
    );

    let (code, ..) = cli(
        &socket,
        &[
            "memory",
            "edit",
            "m-1",
            "--content",
            "Window seat",
            "--retention",
            "permanent",
        ],
    )
    .await;
    assert_eq!(code, 0);
    let (method, uri, body) = last();
    assert_eq!((method.as_str(), uri.as_str()), ("PATCH", "/v1/memory/m-1"));
    assert_eq!(body["content"], "Window seat");
    assert_eq!(body["retention"], "permanent");
    assert!(
        body["tags"].is_null(),
        "tags untouched unless given: {body}"
    );
    let (code, ..) = cli(&socket, &["memory", "edit", "m-1", "--clear-tags"]).await;
    assert_eq!(code, 0);
    assert_eq!(last().2["tags"], json!([]));

    let (code, out, _) = cli(&socket, &["memory", "rm", "m-1"]).await;
    assert_eq!((code, out.as_str()), (0, "Deleted memory m-1\n"));
    assert_eq!(last().0, "DELETE");

    let (code, out, _) = cli(&socket, &["memory", "history", "m-1"]).await;
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "v1\t2026-01-01T00:00:00+00:00\tcreate\tapi\tAisle seat\nv2\t2026-01-02T00:00:00+00:00\tdelete\tcli\t[scrubbed]\n"
    );
    server.abort();
}

/// WordPiece tokenizer + F32 safetensors with the layout `memory model fetch` validates.
fn model_files() -> (Vec<u8>, Vec<u8>) {
    let words = ["[PAD]", "[UNK]", "car", "garage"];
    let vocab: serde_json::Map<String, Value> = words
        .iter()
        .enumerate()
        .map(|(id, word)| ((*word).to_owned(), json!(id)))
        .collect();
    let tokenizer = serde_json::to_vec(&json!({
        "normalizer": {"type": "BertNormalizer", "lowercase": true},
        "pre_tokenizer": {"type": "BertPreTokenizer"},
        "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "vocab": vocab}
    }))
    .unwrap();
    let rows: [[f32; 2]; 4] = [[0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let data: Vec<u8> = rows
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let mut header = serde_json::to_vec(&json!({
        "embeddings": {"dtype": "F32", "shape": [4, 2], "data_offsets": [0, data.len()]}
    }))
    .unwrap();
    while header.len() % 8 != 0 {
        header.push(b' ');
    }
    let mut model = (header.len() as u64).to_le_bytes().to_vec();
    model.extend(header);
    model.extend(data);
    (tokenizer, model)
}

async fn hub(
    tokenizer: Vec<u8>,
    model: Vec<u8>,
) -> (String, Arc<Mutex<Vec<String>>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let requested: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = requested.clone();
    let published = {
        use sha2::Digest as _;
        let sha = |bytes: &[u8]| format!("{:x}", sha2::Sha256::digest(bytes));
        json!({"siblings": [
            {"rfilename": "tokenizer.json", "size": tokenizer.len(), "lfs": {"sha256": sha(&tokenizer)}},
            {"rfilename": "model.safetensors", "size": model.len(), "lfs": {"sha256": sha(&model)}},
        ]})
        .to_string()
    };
    let router = Router::new()
        .route(
            "/api/models/{owner}/{name}/revision/{revision}",
            get(move || {
                let published = published.clone();
                async move {
                    Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(Body::from(published))
                        .unwrap()
                }
            }),
        )
        .route(
            "/{owner}/{name}/resolve/{revision}/{file}",
            get(
                move |UrlPath((owner, name, revision, file)): UrlPath<(
                    String,
                    String,
                    String,
                    String,
                )>| {
                    let (tokenizer, model, log) = (tokenizer.clone(), model.clone(), log.clone());
                    async move {
                        log.lock().push(format!("{owner}/{name}@{revision}/{file}"));
                        let bytes = match file.as_str() {
                            "tokenizer.json" => tokenizer,
                            "model.safetensors" => model,
                            _ => {
                                return Response::builder().status(404).body(Body::empty()).unwrap()
                            }
                        };
                        Response::builder()
                            .status(200)
                            .body(Body::from(bytes))
                            .unwrap()
                    }
                },
            ),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (endpoint, requested, server)
}

#[tokio::test]
async fn model_fetch_installs_a_verified_model_without_a_daemon() {
    let environment = Environment::new().await;
    let model_dir = environment.directory.path().join("models").join("static");
    let config = environment.directory.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[memory.embedding]\nmodel_dir = {:?}\n",
            model_dir.to_str().unwrap()
        ),
    )
    .unwrap();
    let (tokenizer, model) = model_files();
    let (endpoint, requested, server) = hub(tokenizer.clone(), model.clone()).await;
    let fetch = |repo: &'static str, endpoint: String| {
        let config = config.clone();
        async move {
            let argv = [
                "openside",
                "--json",
                "memory",
                "model",
                "fetch",
                repo,
                "--revision",
                "v1",
                "--endpoint",
                &endpoint,
                "--config",
                config.to_str().unwrap(),
            ];
            let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
            let code = run_with_input(argv, &mut std::io::empty(), &mut stdout, &mut stderr).await;
            (
                code,
                String::from_utf8(stdout).unwrap(),
                String::from_utf8(stderr).unwrap(),
            )
        }
    };

    let (code, out, err) = fetch("acme/static-mini", endpoint.clone()).await;
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let report: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert!(report["model_id"].as_str().unwrap().starts_with("static-"));
    assert_eq!(
        std::fs::read(model_dir.join("tokenizer.json")).unwrap(),
        tokenizer
    );
    assert_eq!(
        std::fs::read(model_dir.join("model.safetensors")).unwrap(),
        model
    );
    assert_eq!(
        *requested.lock(),
        [
            "acme/static-mini@v1/tokenizer.json",
            "acme/static-mini@v1/model.safetensors"
        ]
    );
    server.abort();

    // An unsupported model is rejected before anything replaces the installed one.
    let (endpoint, _, server) = hub(
        br#"{"model":{"type":"Unigram","vocab":[]}}"#.to_vec(),
        model.clone(),
    )
    .await;
    let (code, _, err) = fetch("acme/unigram", endpoint).await;
    assert_eq!(code, 1);
    assert!(
        err.contains("not a supported static embedding model"),
        "{err}"
    );
    assert_eq!(
        std::fs::read(model_dir.join("tokenizer.json")).unwrap(),
        tokenizer
    );
    server.abort();

    for (repo, endpoint, message) in [
        ("../etc", "http://127.0.0.1:9".to_owned(), "owner/name"),
        ("acme/x", "http://example.invalid".to_owned(), "https"),
    ] {
        let (code, _, err) = fetch(repo, endpoint).await;
        assert_eq!(code, 1, "{repo}");
        assert!(err.contains(message), "{err}");
    }
    // A redirect off the endpoint to a host outside network.allow is refused.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().fallback(get(|| async {
        Response::builder()
            .status(302)
            .header("location", "https://collector.invalid/model")
            .body(Body::empty())
            .unwrap()
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (code, _, err) = fetch("acme/redirected", endpoint).await;
    assert_eq!(code, 1);
    assert!(err.contains("not in network.allow"), "{err}");
    server.abort();
    let leftovers: Vec<_> = std::fs::read_dir(model_dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".model-fetch-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging directories are removed: {leftovers:?}"
    );
}

async fn redirector(location: String) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let router = Router::new().fallback(move |request: Request<Body>| {
        let location = location.replace("{path}", request.uri().path());
        async move {
            Response::builder()
                .status(302)
                .header("location", location)
                .body(Body::empty())
                .unwrap()
        }
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (endpoint, server)
}

async fn fetch_with(config: &std::path::Path, repo: &str, endpoint: &str) -> (u8, String) {
    let argv = [
        "openside",
        "memory",
        "model",
        "fetch",
        repo,
        "--endpoint",
        endpoint,
        "--config",
        config.to_str().unwrap(),
    ];
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_with_input(argv, &mut std::io::empty(), &mut stdout, &mut stderr).await;
    (code, String::from_utf8(stderr).unwrap())
}

#[tokio::test]
async fn model_fetch_enforces_https_on_redirects_and_swaps_whole_models() {
    let environment = Environment::new().await;
    let model_dir = environment.directory.path().join("models").join("static");
    let config = environment.directory.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[memory.embedding]\nmodel_dir = {:?}\n",
            model_dir.to_str().unwrap()
        ),
    )
    .unwrap();
    let (tokenizer_a, model_a) = model_files();
    let mut tokenizer_json: Value = serde_json::from_slice(&tokenizer_a).unwrap();
    tokenizer_json["version"] = json!("b");
    let tokenizer_b = serde_json::to_vec(&tokenizer_json).unwrap();
    let mut model_b = model_a.clone();
    let end = model_b.len();
    model_b[end - 4..].copy_from_slice(&0.5f32.to_le_bytes());
    let (hub_a, _, server_a) = hub(tokenizer_a.clone(), model_a.clone()).await;
    let (hub_b, _, server_b) = hub(tokenizer_b.clone(), model_b.clone()).await;

    // Loopback-to-loopback redirects are followed; a hop to plain http elsewhere is refused.
    let (good, good_server) = redirector(format!("{hub_a}{{path}}")).await;
    let (code, err) = fetch_with(&config, "acme/a", &good).await;
    assert_eq!((code, err.as_str()), (0, ""));
    let (bad, bad_server) = redirector("http://example.invalid{path}".into()).await;
    let (code, err) = fetch_with(&config, "acme/a", &bad).await;
    assert_eq!(code, 1);
    assert!(
        err.contains("redirect to a non-https destination refused"),
        "{err}"
    );
    assert_eq!(
        std::fs::read(model_dir.join("tokenizer.json")).unwrap(),
        tokenizer_a,
        "refused fetch keeps the model"
    );

    // Files outside the model pair disappear with the old generation: installs swap whole models.
    std::fs::write(model_dir.join("stale.txt"), "old").unwrap();
    let ((code_a, _), (code_b, _)) = tokio::join!(
        fetch_with(&config, "acme/a", &hub_a),
        fetch_with(&config, "acme/b", &hub_b)
    );
    assert_eq!((code_a, code_b), (0, 0));
    let pair = |dir: &std::path::Path| {
        (
            std::fs::read(dir.join("tokenizer.json")).unwrap(),
            std::fs::read(dir.join("model.safetensors")).unwrap(),
        )
    };
    let installed = pair(&model_dir);
    assert!(
        installed == (tokenizer_a.clone(), model_a.clone())
            || installed == (tokenizer_b.clone(), model_b.clone()),
        "tokenizer and weights must come from the same fetch"
    );
    assert!(!model_dir.join("stale.txt").exists());

    // A loader running while installs swap generations only ever sees one whole model.
    let ids: Vec<String> = [&tokenizer_a, &tokenizer_b]
        .iter()
        .zip([&model_a, &model_b])
        .map(|(tokenizer, model)| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("tokenizer.json"), tokenizer).unwrap();
            std::fs::write(dir.path().join("model.safetensors"), model).unwrap();
            let embedder = openside_memory::StaticEmbedder::load(dir.path()).unwrap();
            openside_memory::Embedder::id(&embedder).to_owned()
        })
        .collect();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let loader = {
        let (stop, dir) = (stop.clone(), model_dir.clone());
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let embedder = openside_memory::StaticEmbedder::load(&dir)
                    .expect("the active model is always loadable during installs");
                seen.push(openside_memory::Embedder::id(&embedder).to_owned());
            }
            seen
        })
    };
    for round in 0..6 {
        let (repo, hub) = if round % 2 == 0 {
            ("acme/a", &hub_a)
        } else {
            ("acme/b", &hub_b)
        };
        assert_eq!(fetch_with(&config, repo, hub).await.0, 0);
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let seen = loader.join().unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|id| ids.contains(id)),
        "mixed generation loaded: {seen:?}"
    );
    let mut entries: Vec<_> = std::fs::read_dir(model_dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, [".static.generations", ".static.lock", "static"]);
    let staging: Vec<_> =
        std::fs::read_dir(model_dir.parent().unwrap().join(".static.generations"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with("gen-"))
            .collect();
    assert!(staging.is_empty(), "{staging:?}");

    // A crash while a plain model directory is moved aside is completed by the next fetch,
    // even one that fails to download.
    let legacy = environment.directory.path().join("models").join("legacy");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("tokenizer.json"), &tokenizer_a).unwrap();
    std::fs::write(legacy.join("model.safetensors"), &model_a).unwrap();
    let legacy_config = environment.directory.path().join("legacy.toml");
    std::fs::write(
        &legacy_config,
        format!(
            "[memory.embedding]\nmodel_dir = {:?}\n",
            legacy.to_str().unwrap()
        ),
    )
    .unwrap();
    *openside_model_fetch::INTERRUPT_MIGRATION.lock() = Some(legacy.clone());
    let (code, err) = fetch_with(&legacy_config, "acme/b", &hub_b).await;
    *openside_model_fetch::INTERRUPT_MIGRATION.lock() = None;
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("simulated crash"), "{err}");
    assert!(
        openside_memory::StaticEmbedder::load(&legacy).is_err(),
        "crash left no active model"
    );
    let (code, _) = fetch_with(&legacy_config, "acme/b", &bad).await;
    assert_eq!(code, 1);
    assert_eq!(
        pair(&legacy),
        (tokenizer_b, model_b),
        "recovery activates the verified generation"
    );
    assert!(!environment
        .directory
        .path()
        .join("models")
        .join(".legacy.journal")
        .exists());
    for server in [server_a, server_b, good_server, bad_server] {
        server.abort();
    }
}
