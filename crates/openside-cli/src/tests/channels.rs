use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

const TOKEN: &str = "channel-cli-canary-abcdefgh";
const SIGNING: &str = "slack-signing-canary-123456";

#[tokio::test]
async fn channels_cli_uses_public_routes_and_hidden_stdin_without_echo() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("channels.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let row = json!({"id":"work","kind":"telegram","label":"Work","enabled":false,"agent_id":null,"options":{},
        "secret_present":true,"credential_source":"vault","status":"disabled","restart_required":false,"receiver_address":null});
    let router=Router::new().fallback(move |State(seen):State<Seen>,request:Request<Body>| {
        let row=row.clone();
        async move {
            let method=request.method().to_string(); let path=request.uri().path().to_owned();
            let bytes=to_bytes(request.into_body(),32768).await.unwrap();
            let body=serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            seen.lock().push((method.clone(),path.clone(),body));
            match (method.as_str(),path.as_str()) {
                ("GET","/v1/settings/channels")=>respond(200,json!({"accounts":[row]})),
                ("POST","/v1/settings/channels/telegram/accounts")|("POST","/v1/settings/channels/slack/accounts")=>respond(201,row),
                ("DELETE","/v1/settings/channels/telegram/accounts/work")=>Response::builder().status(204).body(Body::empty()).unwrap(),
                ("POST","/v1/settings/channels/telegram/accounts/work/test")=>respond(200,json!({"account_id":"work","kind":"telegram","ok":true,"status":"connected","code":"ok","restart_required":false})),
                _=>respond(404,json!({"code":"not_found","message":"not found"})),
            }
        }
    }).with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for args in [
        vec!["channels", "list"],
        vec!["channels", "add", "telegram", "work", "--label", "Work"],
        vec!["channels", "add", "slack", "team", "--label", "Team"],
        vec!["channels", "test", "telegram", "work"],
        vec!["channels", "rm", "telegram", "work"],
    ] {
        let mut argv = vec![
            OsString::from("openside"),
            OsString::from("--socket"),
            socket.clone().into_os_string(),
        ];
        argv.extend(args.iter().map(OsString::from));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let input = format!("{TOKEN}\n{SIGNING}\n");
        assert_eq!(
            run_with_input(argv, &mut input.as_bytes(), &mut out, &mut err).await,
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
        for bytes in [&out, &err] {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains(TOKEN));
            assert!(!text.contains(SIGNING));
        }
    }
    {
        let requests = seen.lock();
        assert_eq!(requests.len(), 5);
        assert_eq!(requests[0].1, "/v1/settings/channels");
        assert_eq!(requests[1].2["credentials"]["token"], TOKEN);
        assert!(requests[1].2["credentials"]["signing_secret"].is_null());
        assert_eq!(requests[2].2["credentials"]["token"], TOKEN);
        assert_eq!(requests[2].2["credentials"]["signing_secret"], SIGNING);
        assert_eq!(
            requests[3].1,
            "/v1/settings/channels/telegram/accounts/work/test"
        );
        assert_eq!(requests[4].0, "DELETE");
    }
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn channels_add_rejects_argv_credentials_without_echo() {
    let _environment = Environment::new().await;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        run_with_input(
            ["openside", "channels", "add", "telegram", "work", "--label", "Work", TOKEN],
            &mut std::io::empty(),
            &mut out,
            &mut err
        )
        .await,
        2
    );
    assert!(!String::from_utf8_lossy(&out).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&err).contains(TOKEN));
    assert!(String::from_utf8_lossy(&err).contains("stdin"));
}
