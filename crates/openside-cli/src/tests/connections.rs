use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value, Option<String>)>>>;

fn connection() -> Value {
    json!({
        "id":"c_0000000000000001","site":"example.test","profile":"agent",
        "identity":"j***@example.test","identity_source":"login_ticket","status":"active",
        "source":"login","favicon":null,"home_url":"https://example.test/account",
        "vault_item_id":null,"jar_group":null,"account_space":null,"accounts_on_site":1,
        "created_at":"2026-01-01T00:00:00Z","verified_at":null,"used_at":null,
        "used_by":null,"expires_at":null,"grants":[{"agent_id":"chief","access":"allowed"}]
    })
}

#[tokio::test]
async fn connections_cli_routes_payloads_canonical_json_and_ticket_hints() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("connections.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let row = connection();
    let mut foreign = row.clone();
    foreign["id"] = json!("c_0000000000000002");
    foreign["site"] = json!("other.test");
    foreign["home_url"] = json!("https://other.test/account");
    let mut candidate = row.clone();
    candidate["id"] = json!("c_0000000000000003");
    candidate["status"] = json!("candidate");
    let list =
        json!({"browser_available":true,"connections":[row,foreign],"candidates":[candidate]});
    let filtered = json!({"browser_available":true,"connections":[row],"candidates":[candidate]});
    let relogin = json!({"status":"needs_you","ticket":{"session_id":"s-1","ticket_id":"t-1"}});
    let space = json!({"space_id":"s_0001","connection_id":"c_0000000000000001","result":relogin});
    let audit = json!({"records":[]});
    let router = Router::new()
        .fallback({
            let (row, list, relogin, space, audit) = (
                row.clone(),
                list.clone(),
                relogin.clone(),
                space.clone(),
                audit.clone(),
            );
            move |State(seen): State<Seen>, request: Request<Body>| {
                let (row, list, relogin, space, audit) = (
                    row.clone(),
                    list.clone(),
                    relogin.clone(),
                    space.clone(),
                    audit.clone(),
                );
                async move {
                    let method = request.method().to_string();
                    let path = request.uri().path().to_owned();
                    let query = request.uri().query().map(str::to_owned);
                    let bytes = to_bytes(request.into_body(), 32768).await.unwrap();
                    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    seen.lock()
                        .push((method.clone(), path.clone(), body, query));
                    match (method.as_str(), path.as_str()) {
                        ("GET", "/v1/connections") => respond(200, list),
                        ("GET", "/v1/connections/audit") => respond(200, audit),
                        ("POST", "/v1/connections/spaces") => respond(200, space),
                        ("DELETE", "/v1/connections/spaces/s_0001")
                        | ("POST", "/v1/connections/c_0000000000000001/revoke") => {
                            Response::builder().status(204).body(Body::empty()).unwrap()
                        }
                        ("POST", "/v1/connections/c_0000000000000001/relogin") => {
                            respond(200, relogin)
                        }
                        ("POST", "/v1/connections/c_signed_in/relogin") => {
                            respond(200, json!({"status":"signed_in"}))
                        }
                        ("GET" | "PATCH", "/v1/connections/c_0000000000000001")
                        | ("POST", "/v1/connections/c_0000000000000001/check") => respond(200, row),
                        _ => respond(
                            404,
                            json!({"code":"connection_not_found","message":"Connection not found"}),
                        ),
                    }
                }
            }
        })
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let cases = vec![
        (vec!["list"], Some(list.clone())),
        (vec!["list", "--site", "example.test"], Some(filtered)),
        (
            vec!["list", "--site", "missing.test"],
            Some(json!({"browser_available":true,"connections":[],"candidates":[]})),
        ),
        (vec!["show", "c_0000000000000001"], Some(row.clone())),
        (vec!["check", "c_0000000000000001"], Some(row.clone())),
        (
            vec!["grant", "c_0000000000000001", "mina", "allowed"],
            Some(row),
        ),
        (vec!["relogin", "c_0000000000000001"], Some(relogin)),
        (
            vec!["relogin", "c_signed_in"],
            Some(json!({"status":"signed_in"})),
        ),
        (vec!["revoke", "c_0000000000000001"], None),
        (
            vec!["revoke", "c_0000000000000001", "--keep-signed-in"],
            None,
        ),
        (
            vec![
                "audit",
                "--connection",
                "c_0000000000000001",
                "--agent",
                "mina",
                "--action",
                "sign_out",
            ],
            Some(audit),
        ),
        (vec!["spaces", "add", "example.test"], Some(space)),
        (vec!["spaces", "remove", "s_0001"], None),
    ];
    for (args, expected) in cases {
        let mut argv = vec![
            OsString::from("openside"),
            OsString::from("--socket"),
            socket.clone().into_os_string(),
            OsString::from("--json"),
            OsString::from("connections"),
        ];
        argv.extend(args.iter().map(OsString::from));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            run(argv, &mut out, &mut err).await,
            0,
            "{args:?}: {}",
            String::from_utf8_lossy(&err)
        );
        let records: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        match expected {
            Some(expected) => {
                assert_eq!(records.len(), 2, "{args:?}");
                assert_eq!(records[0], expected, "{args:?}");
            }
            None => assert_eq!(records.len(), 1, "204 must not invent an API response"),
        }
        let needs_you = args == ["relogin", "c_0000000000000001"]
            || args.first() == Some(&"spaces") && args.get(1) == Some(&"add");
        assert_eq!(
            records.last().unwrap()["status"],
            if needs_you { "waiting_human" } else { "ok" }
        );
        if needs_you {
            let err = String::from_utf8(err).unwrap();
            assert!(err.contains("Ticket t-1 in session s-1"), "{err}");
            assert!(err.contains("ticket resolve t-1"), "{err}");
            assert!(
                err.contains(socket.to_str().unwrap()),
                "continuation must keep the endpoint: {err}"
            );
        }
    }
    {
        let requests = seen.lock();
        assert_eq!(requests.len(), 13);
        assert_eq!(requests[4].0, "POST");
        assert_eq!(requests[4].1, "/v1/connections/c_0000000000000001/check");
        assert_eq!(requests[5].0, "PATCH");
        assert_eq!(
            requests[5].2,
            json!({"grants":[{"agent_id":"mina","access":"allowed"}],"home_url":null})
        );
        assert_eq!(requests[6].2, json!({"session_id":null}));
        assert_eq!(
            requests[8].2,
            json!({"sign_out":true,"delete_vault_item":false})
        );
        assert_eq!(
            requests[9].2,
            json!({"sign_out":false,"delete_vault_item":false})
        );
        let params: HashMap<String, String> =
            url::form_urlencoded::parse(requests[10].3.as_ref().unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(params.get("connection").unwrap(), "c_0000000000000001");
        assert_eq!(params.get("agent").unwrap(), "mina");
        assert_eq!(params.get("action").unwrap(), "sign_out");
        assert_eq!(requests[11].2, json!({"site":"example.test"}));
        assert_eq!(requests[12].0, "DELETE");
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn connections_cli_rejects_unknown_access_and_audit_action() {
    let _environment = Environment::new().await;
    for args in [
        vec!["openside", "connections", "grant", "c_1", "mina", "owner"],
        vec![
            "openside",
            "connections",
            "audit",
            "--action",
            "export_cookie",
        ],
    ] {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(run(args, &mut out, &mut err).await, 2);
        assert!(String::from_utf8_lossy(&err).contains("invalid value"));
    }
}

use std::collections::HashMap;
