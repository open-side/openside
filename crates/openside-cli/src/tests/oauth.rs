use super::*;
use crate::model_accounts::{follow, Followed, Prompt};
use openside_proto::OAuthTransaction;
use std::time::Duration;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

const CODE: &str = "oauth-cli-pasted-code-canary";
const URL: &str = "https://auth.example.test/authorize?state=s1";

fn account(id: &str) -> Value {
    json!({"source":"pi-oauth","provider":"openai-codex","account_id":id,"label":"Work",
        "enabled":true,"reserve_percent":10,"available":true,"reason":"OAuth account ready",
        "auth_invalid":false,"created_at_ms":1})
}

fn transaction(id: &str, status: &str, manual_input: bool) -> Value {
    let terminal = matches!(status, "ready" | "failed" | "cancelled" | "expired");
    json!({
        "transaction_id": id, "kind": "create", "provider": "openai-codex", "account_id": "acc1",
        "method": "browser", "status": status,
        "reason": if status == "failed" { json!("oauth_callback_busy") } else { Value::Null },
        "authorization": if terminal || status == "starting" { Value::Null } else {
            json!({"url": URL, "launch_url": null, "instructions": "Open the link", "user_code": null})
        },
        "manual_input": manual_input, "expires_at_ms": 600_000,
        "account": if status == "ready" { account("acc1") } else { Value::Null },
    })
}

async fn fake_daemon(socket: &std::path::Path, seen: Seen) -> JoinHandle<()> {
    let listener = UnixListener::bind(socket).unwrap();
    let polls: Arc<Mutex<u32>> = Arc::default();
    let router = Router::new()
        .fallback(
            |State((seen, polls)): State<(Seen, Arc<Mutex<u32>>)>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = to_bytes(request.into_body(), 16 * 1024).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock().push((method.clone(), path.clone(), body));
                let empty = || Response::builder().status(204).body(Body::empty()).unwrap();
                match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/settings/oauth/providers") => respond(200, json!({"providers":[
                        {"provider":"openai-codex","name":"ChatGPT (Codex)","methods":["browser","device"],"manual_input_methods":["browser"]},
                        {"provider":"github-copilot","name":"GitHub Copilot","methods":["device"],"manual_input_methods":[]}
                    ]})),
                    ("POST", "/v1/settings/oauth/accounts/openai-codex") => {
                        respond(202, transaction("tx-ok", "starting", false))
                    }
                    ("POST", "/v1/settings/oauth/accounts/github-copilot") => {
                        respond(202, transaction("tx-fail", "starting", false))
                    }
                    ("POST", "/v1/settings/oauth/accounts/openai-codex/acc1/relogin") => {
                        respond(202, transaction("tx-ok", "starting", false))
                    }
                    ("GET", "/v1/settings/oauth/transactions/tx-ok") => {
                        let mut count = polls.lock();
                        *count += 1;
                        if *count % 2 == 1 {
                            respond(200, transaction("tx-ok", "awaiting_browser", false))
                        } else {
                            respond(200, transaction("tx-ok", "ready", false))
                        }
                    }
                    ("GET", "/v1/settings/oauth/transactions/tx-fail") => {
                        respond(200, transaction("tx-fail", "failed", false))
                    }
                    ("GET", "/v1/settings/oauth/transactions/tx-manual") => {
                        respond(200, transaction("tx-manual", "ready", false))
                    }
                    ("POST", "/v1/settings/oauth/transactions/tx-manual/input") => {
                        respond(202, transaction("tx-manual", "exchanging", false))
                    }
                    ("POST", "/v1/settings/oauth/transactions/tx-manual/cancel") => empty(),
                    ("GET", "/v1/settings/oauth/accounts/openai-codex/acc1")
                    | ("PATCH", "/v1/settings/oauth/accounts/openai-codex/acc1") => {
                        respond(200, account("acc1"))
                    }
                    ("DELETE", "/v1/settings/oauth/accounts/openai-codex/acc1") => empty(),
                    ("POST", "/v1/settings/oauth/accounts/openai-codex/gone/relogin") => respond(
                        404,
                        json!({"code":"unknown_account","message":"unknown account"}),
                    ),
                    _ => respond(404, json!({"code":"not_found","message":"not found"})),
                }
            },
        )
        .with_state((seen, polls));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() })
}

async fn cli(socket: &std::path::Path, args: &[&str]) -> (u8, String, String) {
    let mut argv = vec![
        OsString::from("openside"),
        OsString::from("--socket"),
        socket.as_os_str().to_owned(),
    ];
    argv.extend(args.iter().map(OsString::from));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with_input(argv, &mut std::io::empty(), &mut out, &mut err).await;
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

#[tokio::test]
async fn oauth_account_verbs_map_to_the_settings_oauth_routes() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("oauth.sock");
    let seen: Seen = Arc::default();
    let server = fake_daemon(&socket, seen.clone()).await;

    let (code, out, _) = cli(&socket, &["models", "accounts", "providers"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("PROVIDER\tNAME\tMETHODS\tPASTE CODE"));
    assert!(out.contains("openai-codex\tChatGPT (Codex)\tbrowser, device\tbrowser"));
    assert!(out.contains("github-copilot\tGitHub Copilot\tdevice\t-"));

    let (code, out, err) = cli(
        &socket,
        &[
            "models",
            "accounts",
            "add",
            "oauth",
            "--provider",
            "openai-codex",
            "--label",
            "Work",
            "--method",
            "device",
            "--pool",
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    // The authorization link is guidance for the person, shown once, never on stdout.
    assert_eq!(err.matches(URL).count(), 1);
    assert!(!out.contains(URL));
    assert!(out.contains("pi-oauth\topenai-codex/acc1\tWork\tenabled=true\tavailable=true"));
    assert!(!out.contains("Not in a pool yet"));

    let (code, out, _) = cli(
        &socket,
        &[
            "models",
            "accounts",
            "add",
            "oauth",
            "--provider",
            "openai-codex",
            "--label",
            "Work",
        ],
    )
    .await;
    assert_eq!(code, 0);
    assert!(out.contains("Not in a pool yet: add pi-oauth:acc1"));

    let (code, out, err) = cli(
        &socket,
        &[
            "--json",
            "models",
            "accounts",
            "login",
            "oauth",
            "openai-codex/acc1",
            "--method",
            "browser",
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    let first: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(first["status"], "ready");
    assert_eq!(first["authorization"], Value::Null);
    assert!(!out.contains(URL));

    for args in [
        vec!["models", "accounts", "show", "oauth", "openai-codex/acc1"],
        vec!["models", "accounts", "enable", "oauth", "openai-codex/acc1"],
        vec![
            "models",
            "accounts",
            "disable",
            "oauth",
            "openai-codex/acc1",
        ],
        vec![
            "models",
            "accounts",
            "update",
            "oauth",
            "openai-codex/acc1",
            "--label",
            "Home",
            "--reserve-percent",
            "20",
        ],
        vec!["models", "accounts", "rm", "oauth", "openai-codex/acc1"],
    ] {
        let (code, _, err) = cli(&socket, &args).await;
        assert_eq!(code, 0, "{args:?}: {err}");
    }

    let (code, _, err) = cli(
        &socket,
        &[
            "models",
            "accounts",
            "add",
            "oauth",
            "--provider",
            "github-copilot",
            "--label",
            "Gh",
        ],
    )
    .await;
    assert_eq!(code, 1);
    assert!(
        err.contains("callback port") && err.contains("(oauth_callback_busy)"),
        "{err}"
    );

    let (code, _, err) = cli(
        &socket,
        &["models", "accounts", "login", "oauth", "openai-codex/gone"],
    )
    .await;
    assert_eq!(code, 1);
    assert!(
        err.contains("no such OAuth account (unknown_account)"),
        "{err}"
    );

    let requests = seen.lock().clone();
    let find = |method: &str, path: &str| {
        requests
            .iter()
            .filter(|(m, p, _)| m == method && p == path)
            .map(|(_, _, body)| body.clone())
            .collect::<Vec<_>>()
    };
    let creates = find("POST", "/v1/settings/oauth/accounts/openai-codex");
    assert_eq!(
        creates[0],
        json!({"label":"Work","join_pool":true,"method":"device"})
    );
    assert_eq!(
        creates[1],
        json!({"label":"Work","join_pool":false,"method":null})
    );
    assert_eq!(
        find(
            "POST",
            "/v1/settings/oauth/accounts/openai-codex/acc1/relogin"
        ),
        [json!({"method":"browser"})]
    );
    assert_eq!(
        find("PATCH", "/v1/settings/oauth/accounts/openai-codex/acc1"),
        [
            json!({"label":null,"enabled":true,"reserve_percent":null}),
            json!({"label":null,"enabled":false,"reserve_percent":null}),
            json!({"label":"Home","enabled":null,"reserve_percent":20}),
        ]
    );
    assert_eq!(
        find("GET", "/v1/settings/oauth/accounts/openai-codex/acc1").len(),
        1
    );
    assert_eq!(
        find("DELETE", "/v1/settings/oauth/accounts/openai-codex/acc1").len(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn oauth_argument_mistakes_are_refused_before_any_request() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("oauth-args.sock");
    let seen: Seen = Arc::default();
    let server = fake_daemon(&socket, seen.clone()).await;
    for (args, expected) in [
        (
            vec!["models", "accounts", "add", "oauth", "--label", "Work"],
            2,
        ),
        (
            vec!["models", "accounts", "update", "oauth", "openai-codex/acc1"],
            1,
        ),
        (vec!["models", "accounts", "show", "oauth", "acc1"], 1),
        (vec!["models", "accounts", "login", "codex-cli", "sub1"], 1),
        (
            vec![
                "models",
                "accounts",
                "update",
                "claude-code",
                "sub1",
                "--label",
                "x",
            ],
            1,
        ),
        (
            vec![
                "models",
                "accounts",
                "add",
                "codex-cli",
                "--label",
                "Work",
                "--pool",
            ],
            1,
        ),
        (
            vec![
                "models",
                "accounts",
                "login",
                "oauth",
                "openai-codex/acc1",
                "--method",
                "manual",
            ],
            2,
        ),
    ] {
        let (code, _, _) = cli(&socket, &args).await;
        assert_eq!(code, expected, "{args:?}");
    }
    // A code typed as an argument is refused without being repeated back.
    let (code, out, err) = cli(
        &socket,
        &[
            "models",
            "accounts",
            "login",
            "oauth",
            "openai-codex/acc1",
            CODE,
        ],
    )
    .await;
    assert_eq!(code, 2);
    assert!(!out.contains(CODE) && !err.contains(CODE));
    assert!(err.contains("hidden prompt"));
    assert!(seen.lock().is_empty());
    server.abort();
}

fn waiting_for_code() -> OAuthTransaction {
    serde_json::from_value(transaction("tx-manual", "awaiting_browser", true)).unwrap()
}

#[tokio::test]
async fn follow_sends_a_prompted_code_once_and_keeps_it_off_stderr() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("oauth-follow.sock");
    let seen: Seen = Arc::default();
    let server = fake_daemon(&socket, seen.clone()).await;
    let client = crate::client::Client::new(Some(&socket), None, None).unwrap();
    let mut asked = 0;
    let mut ask = || {
        asked += 1;
        Prompt::answered(CODE)
    };
    let mut err = Vec::new();
    let followed = follow(
        &client,
        waiting_for_code(),
        &mut err,
        Some(&mut ask),
        std::future::pending(),
        Duration::from_millis(10),
    )
    .await
    .unwrap();
    let Followed::Finished(done) = followed else {
        panic!("interrupted");
    };
    assert_eq!(done.status, openside_proto::OAuthTransactionStatus::Ready);
    assert_eq!(asked, 1);
    let err = String::from_utf8_lossy(&err);
    assert!(!err.contains(CODE));
    assert!(err.contains(URL) && err.contains("Finishing sign-in"));
    let requests = seen.lock().clone();
    assert_eq!(
        requests[0].1,
        "/v1/settings/oauth/transactions/tx-manual/input"
    );
    assert_eq!(requests[0].2, json!({"code": CODE}));
    assert_eq!(requests[1].1, "/v1/settings/oauth/transactions/tx-manual");
    server.abort();
}

#[tokio::test]
async fn follow_without_a_terminal_points_to_the_browser_and_interrupt_cancels() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("oauth-cancel.sock");
    let seen: Seen = Arc::default();
    let server = fake_daemon(&socket, seen.clone()).await;
    let client = crate::client::Client::new(Some(&socket), None, None).unwrap();
    let mut err = Vec::new();
    let followed = follow(
        &client,
        waiting_for_code(),
        &mut err,
        None,
        async {},
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(matches!(followed, Followed::Interrupted));
    let err = String::from_utf8_lossy(&err);
    assert!(err.contains("Finish signing in in the browser"));
    assert!(err.contains("Sign-in cancelled"));
    let requests = seen.lock().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "POST");
    assert_eq!(
        requests[0].1,
        "/v1/settings/oauth/transactions/tx-manual/cancel"
    );
    server.abort();
}
